//! One account round at a time, two directional pulls. Control consumption is
//! independent of file work; a rejected request never opens an arbitrary path.
use super::roster;
use super::{
    public::Failure,
    reconcile::{self, Catalog, Issue, ManifestReader},
    transport::{
        self, data_channel::Slice, Cancellation, Connection, Context, Error, ExchangeVersion,
        Resource, ResourceKind, Result,
    },
    valid_hash, Manifest, ManifestPhase, PostVersion,
};
use crate::bbs::sync::{ContentStore, GroupFence};
use crate::bbs::cache::Deps;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex, OwnedSemaphorePermit, Semaphore};

mod delta;

const MAX_JOBS: usize = 32;
// Legacy wire fixture version. Relay connections use their TLS selection.
#[cfg(test)]
const EXCHANGE_PROTOCOL_VERSION: u32 = 4;
const MAX_FAILURES: usize = 128;
const PAGE_BUDGET: usize = 12_000;
const PLAN_BUDGET: usize = 256 * 1024;

fn check_protocol_version(peer: u32, negotiated: ExchangeVersion) -> Result<()> {
    if peer == negotiated.number() {
        Ok(())
    } else {
        Err(Error::ProtocolVersion)
    }
}

/// Transport failures retain their cause. Only an unexpected successful
/// response is a protocol violation, not a timeout/closure/cancellation.
fn expect_ok(answer: Result<Answer>) -> Result<()> {
    match answer? {
        Answer::Ok => Ok(()),
        _ => Err(Error::Protocol),
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Outcome {
    pub completed: u64,
    pub total: u64,
    pub failures: Vec<Failure>,
    pub omitted: u64,
    pub more: bool,
}
impl Outcome {
    /// Every planned item was attempted. A failed item is reported and stays
    /// for the next full catalog or Manual round; it does not hold back the
    /// checkpoint, the other items, or the peer.
    pub(crate) fn settled(&self) -> bool {
        !self.more && self.completed + self.failures.len() as u64 + self.omitted >= self.total
    }
    fn failure(&mut self, f: Failure) {
        if self.failures.len() < MAX_FAILURES {
            self.failures.push(f)
        } else {
            self.omitted += 1
        }
    }
    fn issues(&mut self, side: &str, issues: impl IntoIterator<Item = Issue>) {
        for i in issues {
            self.failure(Failure::checked(
                side,
                &i.code,
                i.thread_id.as_deref(),
                i.post_id.as_deref(),
            ));
        }
    }
    fn merge(&mut self, other: Self) {
        self.completed += other.completed;
        self.total += other.total;
        self.more |= other.more;
        self.omitted += other.omitted;
        for f in other.failures {
            self.failure(f)
        }
    }
    fn validate(&self) -> Result<()> {
        if self.completed > self.total
            || self.failures.len() > MAX_FAILURES
            || self.total > MAX_JOBS as u64
            || self.failures.iter().any(|f| {
                Failure::checked(
                    &f.side,
                    &f.code,
                    f.thread_id.as_deref(),
                    f.post_id.as_deref(),
                ) != *f
            })
        {
            return Err(Error::Protocol);
        }
        Ok(())
    }
}
#[derive(Clone)]
pub(crate) enum Event {
    Changed(String),
    Started(String),
    Progress(String, u64, u64),
    Finished(String, Outcome),
    Failed(String, Error),
}
impl Event {
    pub(crate) fn peer(&self) -> &str {
        match self {
            Self::Changed(id)
            | Self::Started(id)
            | Self::Progress(id, ..)
            | Self::Finished(id, ..)
            | Self::Failed(id, ..) => id,
        }
    }
}
pub(crate) type Observe = Arc<dyn Fn(u64, Event) + Send + Sync>;
static NEXT_LINK: AtomicU64 = AtomicU64::new(1);
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum Packet {
    Request { id: u64, body: Request },
    Response { id: u64, body: Answer },
    Changed,
    Working { counter: u64 },
}
#[derive(Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Request {
    Open {
        protocol_version: u32,
        round: String,
        revision: String,
        checkpoint: Option<String>,
    },
    Page {
        round: String,
        phase: ManifestPhase,
        after: Option<Value>,
    },
    Roster {
        round: String,
        after: Option<String>,
        known_version: Option<String>,
    },
    Get {
        round: String,
        resource: Resource,
        // v5 attachments only. Absent means the whole file, exactly as v4.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        range: Option<Range>,
    },
    Turn {
        round: String,
    },
    Finish {
        round: String,
        result: Outcome,
    },
}
/// One attachment slice: offsets are absolute, aligned to RESUME_SLICE.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Range {
    offset: u64,
    length: u64,
}
impl Range {
    fn valid_for(&self, resource: &Resource) -> bool {
        let size = resource.size_bytes;
        matches!(resource.identity, ResourceKind::Attachment { .. })
            && self.length > 0
            && self.offset % transport::RESUME_SLICE == 0
            && self.offset.checked_add(self.length).is_some_and(|end| {
                end <= size && (end == size || self.length == transport::RESUME_SLICE)
            })
    }
}
impl Request {
    fn stage(&self) -> &'static str {
        match self {
            Self::Open { .. } => "open",
            Self::Page { phase, .. } => match phase {
                ManifestPhase::Tombstones => "tombstones",
                ManifestPhase::Threads => "threads",
                ManifestPhase::Versions => "versions",
                ManifestPhase::Unavailable => "unavailable",
            },
            Self::Roster { .. } => "roster",
            Self::Get { resource, .. } => match resource.identity {
                ResourceKind::Post { .. } => "post",
                ResourceKind::Attachment { .. } => "attachment",
                ResourceKind::Avatar { .. } => "avatar",
            },
            Self::Turn { .. } => "turn",
            Self::Finish { .. } => "finish",
        }
    }
}
// Fixed-size stage evidence, not a packet log. Never retain body, paths, SDP,
// signaling payloads, invitation material or arbitrary remote error strings.
#[derive(Default, Clone, Serialize)]
struct Trace {
    outgoing: Option<&'static str>,
    outgoing_id: u64,
    incoming: Option<&'static str>,
    incoming_id: u64,
    remote_protocol: Option<u32>,
    // Bounded wire evidence for checkpoint tests; absent from production and logs.
    #[cfg(test)]
    #[serde(skip)]
    last_open: Option<(String, Option<String>)>,
    #[cfg(test)]
    #[serde(skip)]
    manifest_items: [u64; 4],
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum Answer {
    Roster {
        version: String,
        items: Vec<roster::wire::Item>,
        next: Option<String>,
    },
    RosterUnchanged {
        version: String,
    },
    Ok,
    Busy,
    Rejected {
        code: String,
    },
    Page {
        page: Manifest,
        failures: Vec<Failure>,
        omitted: u64,
    },
    Open {
        #[serde(rename = "protocolVersion")]
        protocol_version: u32,
        revision: String,
        checkpoint: Option<String>,
        failures: Vec<Failure>,
        omitted: u64,
    },
}
struct Response {
    answer: Answer,
    _delivery: transport::control_channel::RetainedDelivery,
}
struct Pending {
    id: u64,
    reply: oneshot::Sender<Result<Response>>,
}
#[derive(Default)]
struct Session {
    id: Option<String>,
    stage: u8,
    revision: String,
    peer_revision: Option<String>,
    _permit: Option<OwnedSemaphorePermit>,
    done: Option<oneshot::Sender<Outcome>>,
    catalog: Option<Arc<Catalog>>,
    roster: Option<Arc<roster::wire::Local>>,
    pages: Option<Arc<Catalog>>,
    snapshot: Option<Snapshot>,
    delta: Option<delta::Start>,
    full: bool,
}
#[derive(Clone)]
struct Snapshot {
    catalog: Arc<Catalog>,
    inputs: Arc<Deps>,
    roster: Option<Arc<roster::wire::Local>>,
    revision: String,
    issues: Vec<Failure>,
    omitted: u64,
    cache_bytes: Option<usize>,
}
/// All peers share these permits and one immutable source snapshot. Refreshing
/// the snapshot uses the same file worker as transfers and blocks new rounds.
pub(crate) struct Engine {
    pub store: ContentStore,
    pub fence: GroupFence,
    pub context: Context,
    rounds: Arc<Semaphore>,
    snapshot: Mutex<Option<Snapshot>>,
    /// Last fully confirmed remote revision, scoped by device for this boot.
    /// This is a rebuildable hint, never content authority.
    checkpoints: Mutex<BTreeMap<String, String>>,
    delta: Mutex<delta::Cache>,
    pub yield_requested: AtomicBool,
    observe: Observe,
    pub(crate) roster: Option<Arc<roster::runtime::Runtime>>,
    #[cfg(test)]
    manifest_items: [AtomicU64; 4],
    // 2(a), memory only and per account, because a relay link lives for one
    // round: `local_serial` counts local content changes; `yielded` is, per
    // peer, the latest serial it was yielded to or pulled completely. So a
    // peer gets at most one yield per local change, whatever its outcome, and
    // a catalog that merely echoes its own content back re-arms nothing.
    local_serial: AtomicU64,
    yielded: Mutex<BTreeMap<String, u64>>,
    /// Inner Request/Response pairs this account initiated.
    #[cfg(test)]
    rpc_requests: AtomicU64,
}
impl Engine {
    #[cfg(test)]
    pub(crate) fn manifest_counts(&self) -> [u64; 4] {
        std::array::from_fn(|i| self.manifest_items[i].load(Ordering::Relaxed))
    }
    #[cfg(test)]
    pub(crate) fn rpc_requests(&self) -> u64 {
        self.rpc_requests.load(Ordering::Relaxed)
    }
    /// Local content changed (publication, deletion or local roster).
    pub(crate) fn local_changed(&self) {
        self.local_serial.fetch_add(1, Ordering::Relaxed);
    }
    fn yielded(&self, remote: &str) -> u64 {
        self.yielded.lock().map_or(0, |m| m.get(remote).copied().unwrap_or(0))
    }
    fn mark_yielded(&self, remote: &str, serial: u64) {
        if let Ok(mut m) = self.yielded.lock() {
            let entry = m.entry(remote.to_string()).or_insert(0);
            *entry = (*entry).max(serial);
        }
    }
    pub(crate) fn new(
        store: ContentStore,
        fence: GroupFence,
        context: Context,
        observe: Observe,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            fence,
            context,
            rounds: Arc::new(Semaphore::new(1)),
            snapshot: Mutex::new(None),
            checkpoints: Mutex::new(BTreeMap::new()),
            delta: Mutex::new(delta::Cache::default()),
            yield_requested: AtomicBool::new(false),
            observe,
            roster: None,
            #[cfg(test)]
            manifest_items: std::array::from_fn(|_| AtomicU64::new(0)),
            local_serial: AtomicU64::new(1),
            yielded: Mutex::new(BTreeMap::new()),
            #[cfg(test)]
            rpc_requests: AtomicU64::new(0),
        })
    }
    pub(crate) async fn refresh(&self, cancel: &Cancellation) -> Result<()> {
        let _round = self
            .rounds
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Busy)?;
        let store = self.store.clone();
        let fence = self.fence.clone();
        let roster = self.roster.as_ref().and_then(|r| r.source());
        let flush = self.delta.lock().map_err(|_| Error::Runtime)?.flush(&store.state);
        let generation = flush.generation;
        let (snapshot, cached) = self
            .context
            .io
            .run_in_round(cancel, move |io| -> anyhow::Result<_> {
                store.recover_staging(io)?;
                let mut inputs = Deps::new();
                let issues = store.refresh_tracked(&fence, io, &mut inputs)?;
                let catalog = store.catalog_tracked(&fence, &mut inputs)?;
                let revision = catalog
                    .revision_with_roster(roster.as_ref().map(|r| r.version.as_str()));
                let cache_bytes = delta::cache_bytes(&catalog);
                let cached = flush.run(io).map_err(anyhow::Error::new)?;
                let mut outcome = Outcome::default();
                outcome.issues("local", issues);
                Ok((Snapshot {
                    catalog: Arc::new(catalog),
                    inputs: Arc::new(inputs),
                    roster,
                    revision,
                    issues: outcome.failures,
                    omitted: outcome.omitted,
                    cache_bytes,
                }, cached))
            })
            .await?
            .map_err(|_| Error::Io)?;
        *self.snapshot.lock().map_err(|_| Error::Runtime)? = Some(snapshot);
        self.delta.lock().map_err(|_| Error::Runtime)?.flushed(generation, cached);
        Ok(())
    }

    /// Stat-only fallback for publications whose marker was missed. Keep all
    /// work on the existing account file service and round permit.
    pub(crate) async fn inspect_idle(&self, cancel: &Cancellation) -> Result<bool> {
        let _round = self.rounds.clone().try_acquire_owned().map_err(|_| Error::Busy)?;
        let snapshot = self.snapshot.lock().map_err(|_| Error::Runtime)?.clone();
        let Some(snapshot) = snapshot else { return Ok(true) };
        let roster = self.roster.as_ref().and_then(|r| r.source());
        if snapshot.roster.as_ref().map(|r| &r.version) != roster.as_ref().map(|r| &r.version) {
            return Ok(true);
        }
        self.context.io.run_in_round(cancel, move |io| {
            snapshot.inputs.unchanged_with(|| io.check()).map(|same| !same)
        }).await?
    }

    pub(crate) fn revision(&self) -> Result<String> {
        self.snapshot.lock().map_err(|_| Error::Runtime)?.as_ref()
            .map(|s| s.revision.clone()).ok_or(Error::Busy)
    }
    pub(crate) fn forget_checkpoint(&self, remote: &str) {
        if let Ok(mut values) = self.checkpoints.lock() { values.remove(remote); }
        if let Ok(mut cache) = self.delta.lock() { cache.forget(remote); }
    }
    pub(crate) fn catalog_members(&self, peers: impl IntoIterator<Item = transport::PeerIdentity>) {
        if let Ok(mut cache) = self.delta.lock() { cache.members(peers); }
    }
    pub(crate) fn force_full_catalogs(&self) {
        if let Ok(mut cache) = self.delta.lock() { cache.force_full(); }
    }
    #[cfg(test)]
    pub(crate) fn expire_full_catalog(&self, remote: &str) {
        self.delta.lock().unwrap().expire(remote, std::time::Instant::now());
    }
    pub(crate) fn full_catalog_due(&self, remote: &str) -> bool {
        self.delta.lock().is_ok_and(|cache| cache.due(remote, std::time::Instant::now()))
    }
    fn delta_start(&self, remote: &str) -> Result<Option<delta::Start>> {
        let checkpoint = self.checkpoint(remote)?;
        Ok(self.delta.lock().map_err(|_| Error::Runtime)?
            .start(remote, checkpoint, std::time::Instant::now()))
    }
    pub(crate) fn checkpoint(&self, remote: &str) -> Result<Option<String>> {
        Ok(self
            .checkpoints
            .lock()
            .map_err(|_| Error::Runtime)?
            .get(remote)
            .cloned())
    }

    fn confirm_checkpoint(&self, remote: &str, revision: &str) -> Result<()> {
        valid_hash(revision).map_err(|_| Error::Protocol)?;
        self.checkpoints
            .lock()
            .map_err(|_| Error::Runtime)?
            .insert(remote.to_owned(), revision.to_owned());
        Ok(())
    }
    fn completed_catalog(&self, session: &Session) -> Result<()> {
        if let (Some(start), Some(snapshot)) = (&session.delta, &session.snapshot) {
            self.delta.lock().map_err(|_| Error::Runtime)?.complete(start,
                snapshot.clone(), session.full, std::time::Instant::now());
        }
        Ok(())
    }
}
pub(crate) struct Link {
    pub(crate) id: u64,
    pub(crate) connection: Arc<Connection>,
    pub(crate) remote: String,
    smaller: bool,
    engine: Arc<Engine>,
    stop: Cancellation,
    session: Mutex<Session>,
    pending: Mutex<Option<Pending>>,
    rpc_lock: AsyncMutex<()>,
    next: AtomicU64,
    remote_work: AtomicU64,
    last_input: Mutex<std::time::Instant>,
    file_work: AtomicBool,
    // Either side changed content after this round opened. A v5 attachment
    // pull stops at its next slice boundary so the new bodies go first.
    changed_in_round: AtomicBool,
    reported: AtomicBool,
    roster_unavailable: AtomicBool,
    trace: Mutex<Trace>,
    #[cfg(test)]
    finish_credit: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
    /// Receiver: called with the durable offset after each completed slice.
    #[cfg(test)]
    pub(crate) after_slice: Mutex<Option<Box<dyn Fn(u64) + Send + Sync>>>,
    /// Sender: every ranged Get served, as (offset, length); whole Gets as None.
    #[cfg(test)]
    pub(crate) served: Mutex<Vec<Option<(u64, u64)>>>,
}
impl Link {
    fn emit(&self, event: Event) {
        (self.engine.observe)(self.id, event);
    }

    pub(crate) fn start(
        connection: Connection,
        remote: String,
        smaller: bool,
        engine: Arc<Engine>,
    ) -> Arc<Self> {
        let stop = connection.cancellation();
        let link = Arc::new(Self {
            id: NEXT_LINK.fetch_add(1, Ordering::Relaxed),
            connection: Arc::new(connection),
            remote,
            smaller,
            engine,
            stop,
            session: Mutex::new(Session::default()),
            pending: Mutex::new(None),
            rpc_lock: AsyncMutex::new(()),
            next: AtomicU64::new(1),
            remote_work: AtomicU64::new(0),
            last_input: Mutex::new(std::time::Instant::now()),
            file_work: AtomicBool::new(false),
            changed_in_round: AtomicBool::new(false),
            reported: AtomicBool::new(false),
            roster_unavailable: AtomicBool::new(false),
            trace: Mutex::new(Trace::default()),
            #[cfg(test)]
            finish_credit: Mutex::new(None),
            #[cfg(test)]
            after_slice: Mutex::new(None),
            #[cfg(test)]
            served: Mutex::new(Vec::new()),
        });
        let (tx, rx) = mpsc::channel(1);
        // Transport cancellation can make a loop return Ok. Every exit must
        // release round state, without waiting for coordinator pruning.
        let l = link.clone();
        tokio::spawn(async move {
            if let Err(e) = l.pump(tx).await {
                l.fail_at("control_receive", e)
            }
            l.cancel();
        });
        let l = link.clone();
        tokio::spawn(async move {
            if let Err(e) = l.clone().serve(rx).await {
                l.fail_at("serve", e)
            }
            l.cancel();
        });
        let l = link.clone();
        tokio::spawn(async move {
            l.progress().await;
            l.cancel();
        });
        link
    }
    pub(crate) fn cancel(&self) {
        self.stop.cancel();
        self.connection.cancel();
        if let Ok(mut s) = self.session.lock() {
            *s = Session::default()
        }
        if let Ok(mut p) = self.pending.lock() {
            p.take();
        }
    }
    fn fail(&self, error: Error) {
        self.fail_at("round", error);
    }
    fn fail_at(&self, site: &'static str, error: Error) {
        if error == Error::Cancelled {
            self.cancel();
            return;
        }
        if !self.reported.swap(true, Ordering::AcqRel) {
            let trace = self.trace.lock().unwrap().clone();
            crate::kota_debug_log(&format!("[bbs-sync-trace] {}", serde_json::json!({
                "pid": std::process::id(), "link": self.id,
                "role": if self.smaller { "initiator" } else { "responder" },
                "localProtocol": self.connection.exchange_version().number(),
                "site": site, "error": error.to_string(), "stage": trace,
            })));
            self.emit(Event::Failed(self.remote.clone(), error));
            self.cancel();
        }
    }
    pub(crate) fn is_stopped(&self) -> bool {
        self.stop.is_cancelled()
    }
    pub(crate) fn active(&self) -> bool {
        self.session.lock().map(|s| s.id.is_some()).unwrap_or(true)
    }
    #[cfg(test)]
    pub(crate) fn defer_finish_credit(&self) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (reached, observed) = oneshot::channel();
        let (release, released) = oneshot::channel();
        *self.finish_credit.lock().unwrap() = Some((reached, released));
        (observed, release)
    }
    /// Content changed: tell the peer, and let both sides' attachment pulls
    /// stop at their next slice boundary so the new bodies go first. The
    /// account counts a local change itself (Engine::local_changed).
    pub(crate) async fn changed(&self) -> Result<()> {
        self.changed_in_round.store(true, Ordering::Relaxed);
        self.send(Packet::Changed).await
    }
    #[cfg(test)]
    pub(crate) fn note_local_change(&self) {
        self.changed_in_round.store(true, Ordering::Relaxed);
        self.engine.local_changed();
    }
    async fn send(&self, packet: Packet) -> Result<()> {
        let bytes = serde_json::to_vec(&packet).map_err(|_| Error::Protocol)?;
        if bytes.len() > reconcile::MAX_PAGE_BYTES {
            return Err(Error::Protocol);
        }
        self.connection.send_control(&bytes).await
    }
    async fn rpc(&self, body: Request) -> Result<Response> {
        let _serial = self.rpc_lock.lock().await;
        #[cfg(test)]
        self.engine.rpc_requests.fetch_add(1, Ordering::Relaxed);
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        {
            let mut trace = self.trace.lock().unwrap();
            trace.outgoing = Some(body.stage());
            trace.outgoing_id = id;
        }
        let (tx, rx) = oneshot::channel();
        *self.pending.lock().map_err(|_| Error::Runtime)? = Some(Pending { id, reply: tx });
        let value = async {
            self.send(Packet::Request { id, body }).await?;
            tokio::time::timeout(transport::PROGRESS_TIMEOUT, rx)
                .await
                .map_err(|_| Error::Timeout)?
                .map_err(|_| Error::Closed)?
        };
        let result = tokio::select! {_=self.stop.cancelled()=>Err(Error::Cancelled),v=value=>v};
        self.pending.lock().map_err(|_| Error::Runtime)?.take();
        if let Err(error) = &result {
            // Preserve the RPC failure before closing wakes the receive pump
            // with Cancelled, which otherwise can win the first-error race.
            self.fail_at("rpc_wait", *error);
        }
        result
    }
    async fn pump(
        &self,
        requests: mpsc::Sender<(u64, Request, transport::control_channel::RetainedDelivery)>,
    ) -> Result<()> {
        loop {
            let delivery = tokio::select! {_=self.stop.cancelled()=>return Ok(()),v=self.connection.next_control()=>v?};
            let packet: Packet =
                serde_json::from_slice(delivery.bytes()).map_err(|_| Error::Protocol)?;
            *self.last_input.lock().map_err(|_| Error::Runtime)? = std::time::Instant::now();
            match packet {
                Packet::Request { id, body } => {
                    {
                        let mut trace = self.trace.lock().unwrap();
                        trace.incoming = Some(body.stage());
                        trace.incoming_id = id;
                        if let Request::Open { protocol_version, .. } = &body {
                            trace.remote_protocol = Some(*protocol_version);
                        }
                        #[cfg(test)]
                        if let Request::Open { revision, checkpoint, .. } = &body {
                            trace.last_open = Some((revision.clone(), checkpoint.clone()));
                        }
                    }
                    let slot = requests.try_reserve().map_err(|_| Error::Protocol)?;
                    // Return credits in wire order, after parsing and bounded
                    // admission, while retaining the frame budget through service.
                    slot.send((id, body, delivery.acknowledge_retaining()?));
                }
                Packet::Response { id, body } => {
                    if let Answer::Open { protocol_version, .. } = &body {
                        self.trace.lock().unwrap().remote_protocol = Some(*protocol_version);
                    }
                    #[cfg(test)]
                    if let Answer::Open { revision, checkpoint, .. } = &body {
                        self.trace.lock().unwrap().last_open = Some((revision.clone(), checkpoint.clone()));
                    }
                    let pending = self
                        .pending
                        .lock()
                        .map_err(|_| Error::Runtime)?
                        .take()
                        .filter(|p| p.id == id)
                        .ok_or(Error::Protocol)?;
                    #[cfg(test)]
                    let gate = if matches!(body, Answer::Ok)
                        && self.trace.lock().unwrap().outgoing == Some("finish")
                    {
                        self.finish_credit.lock().unwrap().take()
                    } else {
                        None
                    };
                    #[cfg(test)]
                    let (retained, deferred) = if gate.is_some() {
                        let (retained, credit) = delivery.defer_credit();
                        (retained, Some(credit))
                    } else {
                        (delivery.acknowledge_retaining()?, None)
                    };
                    #[cfg(not(test))]
                    let retained = delivery.acknowledge_retaining()?;
                    pending
                        .reply
                        .send(Ok(Response {
                            answer: body,
                            _delivery: retained,
                        }))
                        .map_err(|_| Error::Closed)?;
                    #[cfg(test)]
                    if let Some((reached, release)) = gate {
                        let _ = reached.send(());
                        tokio::select! {
                            _ = self.stop.cancelled() => return Ok(()),
                            value = release => value.map_err(|_| Error::Closed)?,
                        }
                        deferred.expect("gated credit")()?;
                    }
                }
                Packet::Changed => {
                    delivery.acknowledge()?;
                    self.changed_in_round.store(true, Ordering::Relaxed);
                    self.emit(Event::Changed(self.remote.clone()));
                }
                Packet::Working { counter } => {
                    if counter <= self.remote_work.load(Ordering::Relaxed) {
                        return Err(Error::Protocol);
                    }
                    self.remote_work.store(counter, Ordering::Relaxed);
                    delivery.acknowledge()?;
                }
            }
        }
    }
    async fn progress(&self) {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut previous = self.engine.context.io.activity();
        let mut counter = 0;
        let mut last_progress = std::time::Instant::now();
        loop {
            let deadline =
                tokio::time::Instant::from_std(last_progress + transport::PROGRESS_TIMEOUT);
            tokio::select! {
                _=self.stop.cancelled()=>break,
                _=interval.tick()=>{},
                _=tokio::time::sleep_until(deadline)=>{},
            }
            let current = self.engine.context.io.activity();
            let progressed = current != previous;
            previous = current;
            if !self.active() || progressed || self.file_work.load(Ordering::Relaxed) {
                last_progress = std::time::Instant::now();
            } else {
                // Use the actual arrival, not the following five-second sample.
                // A separate deadline prevents sampling jitter adding a full tick.
                last_progress = last_progress.max(*self.last_input.lock().unwrap());
            }
            if self.active() && last_progress.elapsed() >= transport::PROGRESS_TIMEOUT {
                self.fail_at("idle_progress", Error::Timeout);
                break;
            }
            if self.active() && (progressed || self.file_work.load(Ordering::Relaxed)) {
                counter += 1;
                if let Err(e) = self.send(Packet::Working { counter }).await {
                    self.fail(e);
                    break;
                }
            }
        }
    }
    async fn serve(
        self: Arc<Self>,
        mut requests: mpsc::Receiver<(u64, Request, transport::control_channel::RetainedDelivery)>,
    ) -> Result<()> {
        loop {
            let (id, body, delivery) = tokio::select! {_=self.stop.cancelled()=>return Ok(()),v=requests.recv()=>v.ok_or(Error::Closed)?};
            let _retained = delivery;
            match body {
                Request::Open {
                    protocol_version,
                    round,
                    revision,
                    checkpoint,
                } => {
                    check_protocol_version(protocol_version, self.connection.exchange_version())?;
                    if self.smaller
                        || uuid::Uuid::parse_str(&round)
                            .map_err(|_| Error::Protocol)?
                            .to_string()
                            != round
                    {
                        return Err(Error::Protocol);
                    }
                    if self.active() {
                        return Err(Error::Protocol);
                    }
                    if valid_hash(&revision).is_err()
                        || checkpoint.as_deref().is_some_and(|v| valid_hash(v).is_err())
                    {
                        return Err(Error::Protocol);
                    }
                    let permit = self.engine.rounds.clone().try_acquire_owned();
                    let snapshot = self
                        .engine
                        .snapshot
                        .lock()
                        .map_err(|_| Error::Runtime)?
                        .clone();
                    if let (Ok(permit), Some(snapshot)) = (permit, snapshot) {
                        let delta = self.engine.delta_start(&self.remote)?;
                        let (pages, incremental) = delta.as_ref()
                            .map(|d| d.select(&snapshot, checkpoint.as_deref()))
                            .unwrap_or((snapshot.catalog.clone(), false));
                        let answer_checkpoint = if incremental {
                            delta.as_ref().and_then(|d| d.checkpoint.clone())
                        } else { None };
                        self.changed_in_round.store(false, Ordering::Relaxed);
                        *self.session.lock().map_err(|_| Error::Runtime)? = Session {
                            id: Some(round),
                            stage: 1,
                            revision: snapshot.revision.clone(),
                            peer_revision: Some(revision),
                            _permit: Some(permit),
                            catalog: Some(snapshot.catalog.clone()),
                            roster: snapshot.roster.clone(),
                            pages: Some(pages),
                            snapshot: Some(snapshot.clone()),
                            delta,
                            full: !incremental,
                            done: None,
                        };
                        self.emit(Event::Started(self.remote.clone()));
                        self.send(Packet::Response {
                            id,
                            body: Answer::Open {
                                protocol_version: self.connection.exchange_version().number(),
                                revision: snapshot.revision,
                                checkpoint: answer_checkpoint,
                                omitted: snapshot.omitted
                                    + snapshot.issues.len().saturating_sub(16) as u64,
                                failures: snapshot.issues.into_iter().take(16).collect(),
                            },
                        })
                        .await?;
                    } else {
                        self.send(Packet::Response {
                            id,
                            body: Answer::Busy,
                        })
                        .await?;
                    }
                }
                Request::Page {
                    round,
                    phase,
                    after,
                } => {
                    let _ = self.catalog_for(&round)?;
                    let catalog = self.session.lock().map_err(|_| Error::Runtime)?
                        .pages.clone().ok_or(Error::Protocol)?;
                    let (page, issues) = catalog
                        .page_with_budget(phase, after.as_ref(), PAGE_BUDGET)
                        .map_err(|_| Error::Protocol)?;
                    #[cfg(test)]
                    {
                        let index = match phase { ManifestPhase::Tombstones => 0,
                            ManifestPhase::Threads => 1, ManifestPhase::Versions => 2,
                            ManifestPhase::Unavailable => 3 };
                        self.trace.lock().unwrap().manifest_items[index] += page.items.len() as u64;
                        self.engine.manifest_items[index].fetch_add(page.items.len() as u64, Ordering::Relaxed);
                    }
                    let count = issues.len();
                    let failures = issues
                        .into_iter()
                        .take(8)
                        .map(|i| {
                            Failure::checked(
                                "peer",
                                &i.code,
                                i.thread_id.as_deref(),
                                i.post_id.as_deref(),
                            )
                        })
                        .collect();
                    self.send(Packet::Response {
                        id,
                        body: Answer::Page {
                            page,
                            failures,
                            omitted: count.saturating_sub(8) as u64,
                        },
                    })
                    .await?;
                }
                Request::Roster {
                    round,
                    after,
                    known_version,
                } => {
                    let body =
                        self.roster_response(&round, after.as_deref(), known_version.as_deref())?;
                    self.send(Packet::Response { id, body }).await?;
                }
                Request::Get { round, resource, range } => {
                    let catalog = self.catalog_for(&round)?;
                    resource.validate()?;
                    if range.is_some_and(|r| {
                        self.connection.exchange_version() != ExchangeVersion::V5
                            || !r.valid_for(&resource)
                    }) {
                        return Err(Error::Protocol);
                    }
                    let store = self.engine.store.clone();
                    let fence = self.engine.fence.clone();
                    let r = resource.clone();
                    let roster = self.session.lock().map_err(|_| Error::Runtime)?.roster.clone();
                    let path = self
                        .engine
                        .context
                        .io
                        .run(&self.stop, move |_| {
                            store.source_for_exchange(&fence, &r, &catalog, roster.as_deref())
                        })
                        .await;
                    match path {
                        Ok(Ok(path)) => {
                            #[cfg(test)]
                            self.served.lock().unwrap().push(range.map(|r| (r.offset, r.length)));
                            self.send(Packet::Response {
                                id,
                                body: Answer::Ok,
                            })
                            .await?;
                            self.file_work.store(true, Ordering::Relaxed);
                            let result = match range {
                                Some(r) => {
                                    self.connection
                                        .send_range(resource, &path, r.offset, r.offset + r.length)
                                        .await
                                }
                                None => self.connection.send_file(resource, &path).await,
                            };
                            self.file_work.store(false, Ordering::Relaxed);
                            result?;
                        }
                        _ => {
                            self.send(Packet::Response {
                                id,
                                body: Answer::Rejected {
                                    code: "source_unavailable".into(),
                                },
                            })
                            .await?
                        }
                    }
                }
                Request::Turn { round } => {
                    {
                        let mut s = self.session.lock().map_err(|_| Error::Runtime)?;
                        if self.smaller || s.id.as_ref() != Some(&round) || s.stage != 1 {
                            return Err(Error::Protocol);
                        }
                        s.stage = 2;
                    }
                    self.send(Packet::Response {
                        id,
                        body: Answer::Ok,
                    })
                    .await?;
                    let link = self.clone();
                    tokio::spawn(async move {
                        let result = link.pull(&round, None).await;
                        match result {
                            Ok(outcome) => {
                                let sent = link
                                    .rpc(Request::Finish {
                                        round,
                                        result: outcome.clone(),
                                    })
                                    .await;
                                match expect_ok(sent.map(|response| response.answer)) {
                                    Ok(()) => {
                                        let peer_revision = link
                                            .session
                                            .lock()
                                            .ok()
                                            .and_then(|s| s.peer_revision.clone());
                                        if outcome.settled() {
                                            if let Some(peer_revision) = peer_revision {
                                                let _ = link.engine.confirm_checkpoint(
                                                    &link.remote,
                                                    &peer_revision,
                                                );
                                                let s = link.session.lock().unwrap();
                                                let _ = link.engine.completed_catalog(&s);
                                            }
                                        }
                                        *link.session.lock().unwrap() = Session::default();
                                        link.emit(Event::Finished(link.remote.clone(), outcome));
                                    }
                                    Err(error) => link.fail(error),
                                }
                            }
                            Err(e) => link.fail(e),
                        }
                    });
                }
                Request::Finish { round, result } => {
                    result.validate()?;
                    let done = {
                        let mut s = self.session.lock().map_err(|_| Error::Runtime)?;
                        if !self.smaller || s.id.as_ref() != Some(&round) || s.stage != 2 {
                            return Err(Error::Protocol);
                        }
                        s.done.take().ok_or(Error::Protocol)?
                    };
                    self.send(Packet::Response {
                        id,
                        body: Answer::Ok,
                    })
                    .await?;
                    let _ = done.send(result);
                }
            }
        }
    }
    fn catalog_for(&self, round: &str) -> Result<Arc<Catalog>> {
        let s = self.session.lock().map_err(|_| Error::Runtime)?;
        if s.id.as_deref() != Some(round)
            || (self.smaller && s.stage != 2)
            || (!self.smaller && s.stage != 1)
        {
            return Err(Error::Protocol);
        }
        s.catalog.clone().ok_or(Error::Protocol)
    }
    pub(crate) async fn round(&self) -> Result<Outcome> {
        if !self.smaller {
            return Err(Error::Protocol);
        }
        let permit = self
            .engine
            .rounds
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Busy)?;
        let snapshot = self
            .engine
            .snapshot
            .lock()
            .map_err(|_| Error::Runtime)?
            .clone()
            .ok_or(Error::Busy)?;
        let round = uuid::Uuid::new_v4().to_string();
        self.changed_in_round.store(false, Ordering::Relaxed);
        let delta = self.engine.delta_start(&self.remote)?;
        let checkpoint = delta.as_ref().and_then(|d| d.checkpoint.clone());
        let (done, mut finished) = oneshot::channel();
        {
            let mut s = self.session.lock().map_err(|_| Error::Runtime)?;
            if s.id.is_some() {
                return Err(Error::Busy);
            }
            *s = Session {
                id: Some(round.clone()),
                stage: 1,
                revision: snapshot.revision.clone(),
                peer_revision: None,
                _permit: Some(permit),
                catalog: Some(snapshot.catalog.clone()),
                done: Some(done),
                roster: snapshot.roster.clone(),
                pages: Some(snapshot.catalog.clone()),
                snapshot: Some(snapshot.clone()),
                delta,
                full: true,
            };
        }
        let revision = self
            .session
            .lock()
            .map_err(|_| Error::Runtime)?
            .revision
            .clone();
        let mut peer_declined = false;
        let mut remote_revision = None;
        // The peer still has local content of ours to pull in this round.
        let mut owed = false;
        let serial = self.engine.local_serial.load(Ordering::Relaxed);
        let result = async {
            let response = self
                .rpc(Request::Open {
                    protocol_version: self.connection.exchange_version().number(),
                    round: round.clone(),
                    revision: revision.clone(),
                    checkpoint: checkpoint.clone(),
                })
                .await?;
            let mut outcome = Outcome::default();
            match response.answer {
                Answer::Busy => {
                    peer_declined = true;
                    return Err(Error::Busy);
                }
                Answer::Open {
                    protocol_version,
                    revision,
                    checkpoint,
                    failures,
                    omitted,
                } => {
                    check_protocol_version(protocol_version, self.connection.exchange_version())?;
                    if valid_hash(&revision).is_err()
                        || checkpoint.as_deref().is_some_and(|v| valid_hash(v).is_err())
                    {
                        return Err(Error::Protocol);
                    }
                    remote_revision = Some(revision);
                    {
                        let mut session = self.session.lock().map_err(|_| Error::Runtime)?;
                        let (pages, _) = session.delta.as_ref()
                            .map(|d| d.select(&snapshot, checkpoint.as_deref()))
                            .unwrap_or((snapshot.catalog.clone(), false));
                        owed = !pages.is_empty() && self.engine.yielded(&self.remote) < serial;
                        session.pages = Some(pages);
                        // A null response requests full in both directions.
                        // Mismatched non-null baselines only force our outbound
                        // full; do not falsely claim a full inbound repair.
                        session.full = checkpoint.is_none();
                    }
                    if failures.len() > 16 {
                        return Err(Error::Protocol);
                    }
                    for failure in failures {
                        outcome.failure(Failure::checked(
                            "peer",
                            &failure.code,
                            failure.thread_id.as_deref(),
                            failure.post_id.as_deref(),
                        ));
                    }
                    outcome.omitted += omitted;
                }
                _ => return Err(Error::Protocol),
            }
            drop(response._delivery);
            self.emit(Event::Started(self.remote.clone()));
            outcome.merge(self.pull(&round, owed.then_some(serial)).await?);
            {
                let mut session = self.session.lock().map_err(|_| Error::Runtime)?;
                session.stage = 2;
                // Keep the catalog and roster named by Open for both halves.
                // Installs and local changes are offered after the next refresh.
            }
            if !matches!(self.rpc(Request::Turn { round }).await?.answer, Answer::Ok) {
                return Err(Error::Protocol);
            }
            let mut progress = self.remote_work.load(Ordering::Relaxed);
            loop {
                tokio::select! {
                    _ = self.stop.cancelled() => return Err(Error::Cancelled),
                    result = &mut finished => {
                        let peer = result.map_err(|_| Error::Closed)?;
                        if peer.settled() {
                            // It pulled the whole catalog named by this Open.
                            self.engine.mark_yielded(&self.remote, serial);
                        }
                        outcome.merge(peer);
                        break;
                    }
                    _ = tokio::time::sleep(transport::PROGRESS_TIMEOUT) => {
                        let next = self.remote_work.load(Ordering::Relaxed);
                        if next == progress {
                            return Err(Error::Timeout);
                        }
                        progress = next;
                    }
                }
            }
            if outcome.settled() {
                if let Some(remote_revision) = remote_revision.as_deref() {
                    self.engine.confirm_checkpoint(&self.remote, remote_revision)?;
                    let session = self.session.lock().map_err(|_| Error::Runtime)?;
                    self.engine.completed_catalog(&session)?;
                }
            }
            Ok(outcome)
        }
        .await;
        *self.session.lock().map_err(|_| Error::Runtime)? = Session::default();
        if let Err(e) = result {
            if e == Error::Busy && !peer_declined {
                // Only Open/Busy means the peer has no round. A later local
                // resource refusal must retire the link; reusing it would send
                // a second Open into the responder's still-active old round.
                self.cancel();
            } else if e != Error::Busy {
                self.fail(e)
            }
        }
        result
    }
    /// `owed`: the local serial the peer still has to pull from us after this
    /// half of the round, if any.
    async fn pull(&self, round: &str, owed: Option<u64>) -> Result<Outcome> {
        // Includes parsed descriptors, jobs and their transient serialization.
        let _budget = self.engine.context.limits.reserve(PLAN_BUDGET)?;
        let mut reader = ManifestReader::default();
        let mut jobs: Vec<Job> = Vec::new();
        let mut outcome = Outcome::default();
        for phase in [
            ManifestPhase::Tombstones,
            ManifestPhase::Threads,
            ManifestPhase::Versions,
            ManifestPhase::Unavailable,
        ] {
            let mut after = None;
            loop {
                let response = self
                    .rpc(Request::Page {
                        round: round.into(),
                        phase,
                        after: after.clone(),
                    })
                    .await?;
                let Answer::Page {
                    page,
                    failures,
                    omitted,
                } = response.answer
                else {
                    return Err(Error::Protocol);
                };
                if failures.len() > 8 {
                    return Err(Error::Protocol);
                }
                for f in failures {
                    outcome.failure(Failure::checked(
                        "peer",
                        &f.code,
                        f.thread_id.as_deref(),
                        f.post_id.as_deref(),
                    ))
                }
                outcome.omitted += omitted;
                after = page.next.clone();
                let store = self.engine.store.clone();
                let fence = self.engine.fence.clone();
                let (plan, next_reader) = self
                    .engine
                    .context
                    .io
                    .run_in_round(&self.stop, move |io| {
                        let plan = store.process_page(&fence, &page, &mut reader, io);
                        (plan, reader)
                    })
                    .await?;
                reader = next_reader;
                let plan = plan.map_err(|_| Error::Integrity)?;
                outcome.issues("local", plan.issues);
                for warning in plan.warnings {
                    outcome.failure(Failure::checked(
                        "local",
                        "thread_record_mismatch",
                        Some(&warning.thread_id),
                        None,
                    ));
                }
                for r in plan.unavailable {
                    outcome.failure(resource_failure(&r, "peer", "attachment_unavailable"));
                }
                for resource in plan.fetch {
                    let offered = plan
                        .versions
                        .iter()
                        .find(|p| matches_resource(p, &resource))
                        .cloned();
                    let job = Job { resource, offered };
                    if !jobs.iter().any(|old| {
                        old.resource == job.resource
                            || (matches!(old.resource.identity, ResourceKind::Avatar { .. })
                                && matches!(job.resource.identity, ResourceKind::Avatar { .. })
                                && old.resource.sha256 == job.resource.sha256)
                    }) {
                        jobs.push(job);
                        jobs.sort_by_key(Job::priority);
                        if jobs.len() > MAX_JOBS {
                            jobs.pop();
                            outcome.more = true;
                        }
                    }
                }
                drop(response._delivery);
                if after.is_none() {
                    break;
                }
            }
        }
        if !reader.is_complete() {
            return Err(Error::Protocol);
        }
        let peer_roster = self.pull_roster(round, &mut outcome).await?;
        self.roster_avatar_jobs(peer_roster.as_ref(), &mut jobs, &mut outcome)
            .await?;
        outcome.total = jobs.len() as u64;
        self.emit(Event::Progress(self.remote.clone(), 0, outcome.total));
        let ranged = self.connection.exchange_version() == ExchangeVersion::V5;
        let mut slicing = Slicing { bytes: 0 };
        let count = jobs.len();
        let attachment_bytes: u64 = jobs
            .iter()
            .filter(|job| job.ranged())
            .map(|job| job.resource.size_bytes)
            .sum();
        for (index, job) in jobs.into_iter().enumerate() {
            if index > 0 && self.engine.yield_requested.load(Ordering::Relaxed) {
                outcome.more = true;
                break;
            }
            // Slice boundary 0: every body of this round is in. If the peer
            // still pulls our new content and more than one slice of files
            // remains, let it pull first instead of making its bodies wait.
            if let Some(serial) = owed.filter(|_| {
                ranged
                    && job.ranged()
                    && slicing.bytes == 0
                    && attachment_bytes > transport::RESUME_SLICE
            }) {
                // Once per local serial, even if the peer's pull stays partial.
                self.engine.mark_yielded(&self.remote, serial);
                outcome.more = true;
                break;
            }
            let result = if ranged && job.ranged() {
                self.fetch_ranged(round, &job, peer_roster.clone(), &mut slicing).await
            } else {
                self.fetch(round, &job, peer_roster.clone()).await.map(|()| Fetched::Done)
            };
            match result {
                Ok(Fetched::Done) => {
                    outcome.completed += 1;
                    if job.ranged() && index + 1 < count && self.bodies_waiting(&slicing) {
                        outcome.more = true;
                        self.emit(Event::Progress(
                            self.remote.clone(),
                            outcome.completed,
                            outcome.total,
                        ));
                        break;
                    }
                }
                Ok(Fetched::Cut) => {
                    outcome.more = true;
                    break;
                }
                Err(error @ (Error::Cancelled | Error::Closed | Error::Timeout | Error::Unauthorized)) => {
                    return Err(error)
                }
                Err(Error::Busy) => {
                    outcome.more = true;
                    break;
                }
                Err(_) => outcome.failure(resource_failure(
                    &job.resource,
                    "local",
                    "resource_install_failed",
                )),
            };
            self.emit(Event::Progress(
                self.remote.clone(),
                outcome.completed,
                outcome.total,
            ));
        }
        Ok(outcome)
    }
    /// At a later slice boundary, stop receiving attachment bytes when either
    /// side changed content after this round opened: its new bodies go first.
    fn bodies_waiting(&self, slicing: &Slicing) -> bool {
        slicing.bytes >= transport::RESUME_SLICE
            && self.changed_in_round.load(Ordering::Relaxed)
    }
    /// Local reuse first, then admission for a network receive. None means the
    /// resource is already installed from a local copy.
    async fn admit(
        &self,
        job: &Job,
        roster: Option<roster::reference::Peer>,
    ) -> Result<Option<std::path::PathBuf>> {
        let store = self.engine.store.receiving_from(&self.remote).map_err(|_| Error::Unauthorized)?;
        let fence = self.engine.fence.clone();
        let resource = job.resource.clone();
        let offered = job.offered.clone();
        let peer = roster.clone();
        let copied = self
            .engine
            .context
            .io
            .run(&self.stop, move |io| {
                store.copy_available_with_roster(
                    &fence,
                    &resource,
                    offered.as_ref(),
                    peer.as_ref(),
                    io,
                )
            })
            .await?
            .map_err(|_| Error::Io)?;
        if copied {
            self.avatar_installed(&job.resource);
            return Ok(None);
        }
        let store = self.engine.store.receiving_from(&self.remote).map_err(|_| Error::Unauthorized)?;
        let fence = self.engine.fence.clone();
        let resource = job.resource.clone();
        let offered = job.offered.clone();
        let peer = roster;
        let staging = self
            .engine
            .context
            .io
            .run(&self.stop, move |_| {
                store.prepare_receive_with_roster(
                    &fence,
                    &resource,
                    offered.as_ref(),
                    peer.as_ref(),
                )
            })
            .await?
            .map_err(|_| Error::InvalidResource)?;
        Ok(Some(staging))
    }
    async fn fetch(
        &self,
        round: &str,
        job: &Job,
        roster: Option<roster::reference::Peer>,
    ) -> Result<()> {
        let Some(staging) = self.admit(job, roster.clone()).await? else {
            return Ok(());
        };
        #[cfg(not(test))]
        let started = std::time::Instant::now();
        let incoming = self
            .connection
            .expect_file(job.resource.clone(), &staging)
            .await?;
        let response = self
            .rpc(Request::Get {
                round: round.into(),
                resource: job.resource.clone(),
                range: None,
            })
            .await?;
        match response.answer {
            Answer::Rejected { .. } | Answer::Busy => {
                incoming.declined();
                return Err(Error::InvalidResource);
            }
            Answer::Ok => {}
            _ => return Err(Error::Protocol),
        }
        self.file_work.store(true, Ordering::Relaxed);
        let verified = incoming.finish().await;
        self.file_work.store(false, Ordering::Relaxed);
        self.connection.recheck_membership()?;
        let result = self
            .engine
            .store
            .receiving_from(&self.remote).map_err(|_| Error::Unauthorized)?
            .install_received_with_roster(
                verified?,
                self.engine.fence.clone(),
                job.offered.clone(),
                roster,
                &self.stop,
            )
            .await
            .map_err(|_| Error::Integrity);
        if result.is_ok() {
            self.avatar_installed(&job.resource);
        }
        #[cfg(not(test))]
        crate::kota_debug_log(&format!(
            "[bbs-sync] receive peer={} bytes={} elapsed_ms={} installed={} bytes_per_second={:.0}",
            self.remote,
            job.resource.size_bytes,
            started.elapsed().as_millis(),
            result.is_ok(),
            job.resource.size_bytes as f64 / started.elapsed().as_secs_f64().max(0.001)
        ));
        result
    }
    /// v5 attachment: one Get per slice into a deterministic partial, so a
    /// lost link or a stop at a slice boundary resumes where it left off.
    async fn fetch_ranged(
        &self,
        round: &str,
        job: &Job,
        roster: Option<roster::reference::Peer>,
        slicing: &mut Slicing,
    ) -> Result<Fetched> {
        let Some(staging) = self.admit(job, roster.clone()).await? else {
            crate::bbs::sync::mark_attachment_paused(&job.resource, false);
            return Ok(Fetched::Done);
        };
        let size = job.resource.size_bytes;
        self.file_work.store(true, Ordering::Relaxed);
        let file = self.connection.resume_file(job.resource.clone(), &staging).await;
        self.file_work.store(false, Ordering::Relaxed);
        let mut file = file?;
        loop {
            let offset = file.offset();
            let length = transport::RESUME_SLICE.min(size - offset);
            let incoming = file.expect(length)?;
            let response = self
                .rpc(Request::Get {
                    round: round.into(),
                    resource: job.resource.clone(),
                    range: Some(Range { offset, length }),
                })
                .await?;
            match response.answer {
                Answer::Rejected { .. } | Answer::Busy => {
                    incoming.declined();
                    crate::bbs::sync::mark_attachment_paused(&job.resource, true);
                    return Err(Error::InvalidResource);
                }
                Answer::Ok => {}
                _ => return Err(Error::Protocol),
            }
            self.file_work.store(true, Ordering::Relaxed);
            let slice = incoming.finish().await;
            self.file_work.store(false, Ordering::Relaxed);
            self.connection.recheck_membership()?;
            slicing.bytes += length;
            #[cfg(test)]
            if let (Ok(Slice::Partial(at)), Some(hook)) = (&slice, self.after_slice.lock().unwrap().as_ref()) {
                hook(*at);
            }
            let verified = match slice {
                Ok(Slice::Partial(_)) if self.bodies_waiting(slicing) => return Ok(Fetched::Cut),
                Ok(Slice::Partial(_)) => continue,
                Ok(Slice::Verified(verified)) => verified,
                Err(error @ Error::Integrity) => {
                    crate::bbs::sync::mark_attachment_paused(&job.resource, true);
                    return Err(error);
                }
                Err(error @ Error::Io) => {
                    crate::bbs::sync::mark_attachment_paused(&job.resource, false);
                    return Err(error);
                }
                Err(error) => return Err(error),
            };
            let installed = self
                .engine
                .store
                .receiving_from(&self.remote)
                .map_err(|_| Error::Unauthorized)?
                .install_received_with_roster(
                    verified,
                    self.engine.fence.clone(),
                    job.offered.clone(),
                    roster,
                    &self.stop,
                )
                .await;
            // A failed install is local; the verified partial stays for retry.
            crate::bbs::sync::mark_attachment_paused(&job.resource, false);
            return installed.map(|()| Fetched::Done).map_err(|_| Error::Io);
        }
    }
}
/// Attachment bytes received in one pull, for the slice-boundary rule.
struct Slicing {
    bytes: u64,
}
enum Fetched {
    Done,
    /// Stopped at a slice boundary; the partial resumes in the next round.
    Cut,
}
struct Job {
    resource: Resource,
    offered: Option<PostVersion>,
}
impl Job {
    fn ranged(&self) -> bool {
        matches!(self.resource.identity, ResourceKind::Attachment { .. })
            && self.resource.size_bytes > 0
    }
    fn priority(&self) -> (u8, String) {
        match &self.resource.identity {
            ResourceKind::Post { post_id, .. } => (
                if self.offered.as_ref().is_some_and(|p| p.kind == "topic") {
                    0
                } else {
                    1
                },
                post_id.clone(),
            ),
            ResourceKind::Attachment { attachment_id, .. } => (2, attachment_id.clone()),
            ResourceKind::Avatar { sha256, .. } => (3, sha256.clone()),
        }
    }
}
fn matches_resource(post: &PostVersion, r: &Resource) -> bool {
    match &r.identity {
        ResourceKind::Post {
            thread_id,
            post_id,
            version_id,
        }
        | ResourceKind::Attachment {
            thread_id,
            post_id,
            version_id,
            ..
        } => {
            post.thread_id == *thread_id
                && post.post_id == *post_id
                && post.version_id == *version_id
        }
        ResourceKind::Avatar { .. } => {
            post.avatar
                .as_ref()
                .and_then(reconcile::avatar_resource)
                .as_ref()
                == Some(r)
        }
    }
}
fn resource_failure(r: &Resource, side: &str, code: &str) -> Failure {
    match &r.identity {
        ResourceKind::Post {
            thread_id, post_id, ..
        }
        | ResourceKind::Attachment {
            thread_id, post_id, ..
        } => Failure::checked(side, code, Some(thread_id), Some(post_id)),
        ResourceKind::Avatar { .. } => Failure::checked(side, code, None, None),
    }
}

#[cfg(test)]
pub(crate) mod tests;

mod directory;
