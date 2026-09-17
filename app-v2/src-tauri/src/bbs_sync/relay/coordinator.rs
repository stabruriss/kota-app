//! One account owner for metadata, declaration handshakes and content activity.
//! Blocking work stays on the existing file/HTTP workers. All futures below
//! are polled by this owner, not spawned once per request or per polling peer.
use super::{
    activity::{Activity, Event as ActivityEvent, Failure},
    announcement::{Completion, Intent, Outbox, Receipt, Scope},
    discovery::Item,
    handshake::Handshake,
    http::{HttpClient, Reply},
    metadata::{self, Call, Client, Snapshot, WakeIntent, WakeResult},
    pump::Pump,
    Error, Result, TlsStream,
};
use crate::{
    bbs::sync::{ContentStore, GroupFence},
    bbs_sync::{
        coordinator::{now, Authority, Command, Notice, Observe, Work},
        exchange::{self, Engine, Link},
        roster,
        scheduler::{Nonces, Schedule, ACTIVE_MS, HEARTBEAT_MS, MAX_POLLS, POLL_MS},
        transport::{
            Cancellation, Connection, Context, MembershipCheck, PeerIdentity, PROGRESS_TIMEOUT,
        },
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    future::{pending, Future},
    pin::Pin,
    sync::{atomic::Ordering, Arc, Mutex, RwLock, Weak},
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

const IDLE_POLL_MS: u64 = 30_000;
#[derive(Clone, Copy)]
enum ServiceTask {
    Poll,
    Announce,
    Catalog,
    Exchange,
}
#[derive(Clone, Copy, Default)]
struct ServiceBackoff {
    failures: u8,
    until: u64,
}
type Flight<T> = Pin<Box<dyn Future<Output = T> + Send>>;
enum FileDone {
    Prepared(Result<Option<Intent>>),
    Confirmed(Result<Completion>),
}
enum Op {
    Announce(Intent),
    Wake(String, String, WakeIntent),
    Ready(String, String, String),
    Open(String, String),
}
enum MetaDone {
    Poll(Result<Snapshot>),
    Response(Op, Result<Reply>),
}
struct Attempt {
    tag: String,
    peer: PeerIdentity,
    instance: String,
    until: u64,
    polls: u32,
    wake: Option<String>,
    nonce: String,
    ready_boot: Option<String>,
    handshake: Option<Handshake>,
    offered: bool,
    waiting_poll: bool,
    tls: Option<TlsStream>,
    cancel: Cancellation,
}
struct Connected {
    peer: PeerIdentity,
    instance: String,
    session: String,
    wake: String,
    link: Option<Arc<Link>>,
    finished: bool,
    closing_at: Option<u64>,
}

pub(crate) struct Actor {
    context: Context,
    engine: Arc<Engine>,
    authority: Arc<RwLock<Authority>>,
    initial: Authority,
    connections: Arc<Mutex<Vec<Weak<Connection>>>>,
    observe: Observe,
    work: Arc<Work>,
    epoch: u64,
    serial: u64,
    cancel: Cancellation,
    local: String,
    instance: String,
    http: HttpClient,
    outbox: Outbox,
    schedule: Schedule,
    forced: BTreeSet<String>,
    // Unlike queued Manual/change triggers, a failed verification survives
    // Cancel until a future allowed round succeeds (or membership is removed).
    verification: BTreeSet<String>,
    seen_wakes: Nonces,
    attempts: BTreeMap<String, Attempt>,
    connected: BTreeMap<String, Connected>,
    losses: BTreeMap<String, u8>,
    progress: BTreeMap<String, u64>,
    snapshot: Option<Snapshot>,
    discovery_stale: bool,
    activity: Option<Activity>,
    meta: Option<Flight<MetaDone>>,
    file: Option<Flight<FileDone>>,
    round: Option<Flight<(String, u64, Result<exchange::Outcome>)>>,
    events: mpsc::Receiver<(u64, exchange::Event)>,
    pending: Option<Intent>,
    confirm: Option<(Intent, Receipt)>,
    announcing: bool,
    needs_refresh: bool,
    refresh_at: u64,
    poll_at: u64,
    // A successful read must not discharge a failed announcement/write. Keep
    // four bounded operation streaks, not one lifetime failure counter.
    service_backoff: [ServiceBackoff; 4],
    manual_until: Option<u64>,
    cancelled_until: Option<u64>,
}
impl Drop for Actor {
    fn drop(&mut self) {
        self.cancel_all();
    }
}
impl Actor {
    #[cfg(test)]
    pub(crate) fn test_engine(&self) -> Arc<Engine> {
        self.engine.clone()
    }
    pub(crate) fn new(
        context: Context,
        store: ContentStore,
        authority: Authority,
        http: HttpClient,
        observe: Observe,
        work: Arc<Work>,
        epoch: u64,
        roster: Arc<roster::runtime::Runtime>,
        live: Arc<RwLock<Authority>>,
        connections: Arc<Mutex<Vec<Weak<Connection>>>>,
    ) -> Result<Self> {
        authority.validate()?;
        http.check_binding()?;
        if context.shutdown.is_cancelled() || epoch != work.epoch() {
            return Err(Error::Cancelled);
        }
        let (tx, events) = mpsc::channel(128);
        let stop = context.shutdown.clone();
        let mut engine = Engine::new(
            store.clone(),
            GroupFence {
                group_id: authority.membership.group_id.clone(),
                membership_id: authority.membership.membership_id.clone(),
            },
            context.clone(),
            Arc::new(move |id, event| {
                if tx.try_send((id, event)).is_err() {
                    stop.cancel();
                }
            }),
        );
        Arc::get_mut(&mut engine).ok_or(Error::Runtime)?.roster = Some(roster);
        let auth = Self::scope_check(live.clone(), authority.clone(), work.clone(), epoch);
        let outbox = Outbox::new(
            &store.state,
            Scope::new(&authority.membership, &authority.identity)?,
            context.io.clone(),
            auth,
        );
        let mut schedule = Schedule::default();
        schedule.join(now());
        let own = authority
            .identity
            .device_id()
            .map_err(|_| Error::Unauthorized)?;
        engine.catalog_members(authority.members.iter().filter(|m| m.device_id != own)
            .filter_map(|m| authority.peer(&m.device_id).ok()));
        schedule.members(
            authority
                .members
                .iter()
                .filter(|m| m.device_id != own)
                .map(|m| (m.device_id.clone(), m.online)),
        );
        Ok(Self {
            context,
            engine,
            authority: live,
            initial: authority,
            connections,
            observe,
            serial: work.serial(),
            epoch,
            work,
            cancel: Cancellation::default(),
            local: own,
            instance: uuid::Uuid::new_v4().to_string(),
            http,
            outbox,
            schedule,
            forced: BTreeSet::new(),
            verification: BTreeSet::new(),
            seen_wakes: Nonces::default(),
            attempts: BTreeMap::new(),
            connected: BTreeMap::new(),
            losses: BTreeMap::new(),
            progress: BTreeMap::new(),
            snapshot: None,
            discovery_stale: false,
            activity: None,
            meta: None,
            file: None,
            round: None,
            events,
            pending: None,
            confirm: None,
            announcing: false,
            needs_refresh: true,
            refresh_at: now() + HEARTBEAT_MS,
            poll_at: now(),
            service_backoff: [ServiceBackoff::default(); 4],
            manual_until: None,
            cancelled_until: None,
        })
    }
    fn scope_check(
        live: Arc<RwLock<Authority>>,
        expected: Authority,
        work: Arc<Work>,
        epoch: u64,
    ) -> MembershipCheck {
        Arc::new(move || {
            work.epoch() == epoch
                && live.read().ok().is_some_and(|a| {
                    a.membership == expected.membership
                        && a.identity == expected.identity
                        && now() < a.valid_until
                })
        })
    }
    fn authorized(&self) -> MembershipCheck {
        Self::scope_check(
            self.authority.clone(),
            self.initial.clone(),
            self.work.clone(),
            self.epoch,
        )
    }
    fn peer_check(&self, peer: &PeerIdentity) -> MembershipCheck {
        let live = self.authority.clone();
        let expected = peer.clone();
        let membership = self.initial.membership.clone();
        let identity = self.initial.identity.clone();
        let work = self.work.clone();
        let epoch = self.epoch;
        // Validate/key-bind once when creating the capability. Every use still
        // checks the current rows, incarnation, identity, epoch and expiry.
        // Re-deriving the Ed25519 key on every TLS/frame step blocks this owner.
        let anchored = self.initial.peer(&peer.remote_device_id).ok().as_ref() == Some(peer);
        let public = self
            .initial
            .members
            .iter()
            .find(|m| m.device_id == peer.remote_device_id)
            .map(|m| m.public_key.clone());
        Arc::new(move || {
            anchored
                && work.epoch() == epoch
                && live.read().ok().is_some_and(|a| {
                    a.membership == membership
                        && a.identity == identity
                        && now() < a.valid_until
                        && a.members.iter().any(|m| {
                            m.device_id == expected.local_device_id
                                && m.membership_id == expected.local_membership_id
                                && m.public_key == a.identity.public_key
                                && m.role == a.membership.role
                        })
                        && a.members.iter().any(|m| {
                            m.device_id == expected.remote_device_id
                                && m.membership_id == expected.remote_membership_id
                                && Some(&m.public_key) == public.as_ref()
                        })
                })
        })
    }
    fn client(&self, peer: Option<&PeerIdentity>, cancel: Cancellation) -> Result<Client> {
        Client::new(
            self.http.clone(),
            self.initial.identity.clone(),
            self.initial.membership.clone(),
            cancel,
            peer.map_or_else(|| self.authorized(), |p| self.peer_check(p)),
        )
    }
    fn emit(&self, event: Notice) {
        if self.epoch == self.work.epoch() {
            (self.observe)(self.epoch, event);
        }
    }
    fn active(&self) -> bool {
        self.connected
            .values()
            .any(|p| p.link.as_ref().is_some_and(|l| l.active()))
    }
    fn opening_or_syncing(&self) -> bool {
        !self.attempts.is_empty() || self.connected.values().any(|c| !c.finished)
    }
    fn own(&self) -> String {
        self.local.clone()
    }
    fn item(&self, id: &str) -> Option<Item> {
        self.snapshot.as_ref()?.items.get(id).cloned()
    }
    fn candidates(&self, at: u64) -> Vec<String> {
        let mut peers: Vec<_> = self
            .schedule
            .peers
            .iter()
            .filter(|(id, p)| {
                if !p.needed || p.busy || at < p.retry_at {
                    return false;
                }
                if p.online {
                    return true;
                }
                // A current authenticated wake is usable even if the last control
                // heartbeat still says offline. It never changes public presence.
                let Some(s) = &self.snapshot else {
                    return false;
                };
                let Some(i) = s.items.get(*id) else {
                    return false;
                };
                let (Some(a), Some(w)) = (&i.announcement, &i.wake) else {
                    return false;
                };
                if self.seen_wakes.contains(id, &w.id, at)
                    || !i.handshake.as_ref().is_some_and(|h| !h.closed)
                {
                    return false;
                }
                self.initial.peer(id).ok().is_some_and(|p| {
                    w.context(
                        p,
                        self.initial.membership.worker_url.clone(),
                        s.boot.clone(),
                        "candidate-context".into(),
                        &self.instance,
                        &a.instance,
                        at,
                    )
                    .is_ok()
                })
            })
            .collect();
        peers.sort_by_key(|(id, p)| (p.last_served, *id));
        peers.into_iter().map(|(id, _)| id.clone()).collect()
    }
    fn retired(&mut self, id: &str, wake: &str) {
        let _ = self
            .seen_wakes
            .consume(id, wake, now().saturating_add(120_000), now());
    }
    fn remove(&mut self, id: &str) {
        // Progress counters restart with the next Link. Retaining an old
        // maximum would hide real installs in a later recovery round.
        self.progress.remove(id);
        if let Some(a) = self.attempts.remove(id) {
            a.cancel.cancel();
            if let Some(w) = a.wake {
                self.retired(id, &w);
            }
        }
        if let Some(c) = self.connected.remove(id) {
            if let Some(l) = c.link {
                l.cancel();
            }
            if let Some(activity) = self.activity.as_mut() {
                activity.remove(&c.session);
            }
            self.retired(id, &c.wake);
        }
        if self.connected.is_empty() {
            self.activity = None;
        }
        self.schedule.released(id);
        self.emit(Notice::Disconnected(id.into()));
    }
    fn cancel_all(&mut self) {
        self.cancel.cancel();
        let ids: BTreeSet<_> = self
            .attempts
            .keys()
            .chain(self.connected.keys())
            .cloned()
            .collect();
        for id in ids {
            self.remove(&id);
        }
        self.meta = None;
        self.file = None;
        self.round = None;
        self.activity = None;
    }
    fn sync_epoch(&mut self) {
        if self.epoch == self.work.epoch() {
            return;
        }
        self.cancel_all();
        self.epoch = self.work.epoch();
        self.serial = self.work.cutoff();
        self.cancel = Cancellation::default();
        self.outbox = Outbox::new(
            &self.engine.store.state,
            Scope::new(&self.initial.membership, &self.initial.identity).expect("validated scope"),
            self.context.io.clone(),
            self.authorized(),
        );
        self.schedule.cancel_current(now());
        self.forced.clear();
        self.pending = None;
        self.confirm = None;
        self.announcing = false;
        self.needs_refresh = false;
        self.manual_until = None;
        self.cancelled_until = Some(self.refresh_at);
        self.instance = uuid::Uuid::new_v4().to_string();
        // Cancel leaves future automatic work enabled, but no queued pre-cancel
        // marker/wake can immediately start it again.
        self.poll_at = now().saturating_add(IDLE_POLL_MS);
    }
    fn fail(&mut self, id: &str, error: Error) {
        self.remove(id);
        if error == Error::Cancelled {
            return;
        }
        if error == Error::Busy {
            self.schedule.occupied(id, now());
            return;
        }
        // A matching old checkpoint is not evidence that this failed attempt
        // recovered. Keep one verification round owed until complete success.
        self.verification.insert(id.into());
        if error == Error::RelaySessionLost {
            // Do not start another attempt using the old boot while its
            // replacement poll is in flight, or count one reset a second time.
            self.discovery_stale = true;
            self.poll_at = self.poll_at.min(now() + POLL_MS);
            let count = self.losses.entry(id.into()).or_default();
            *count = count.saturating_add(1);
            if *count < 3 {
                self.schedule.occupied(id, now());
                if let Some(p) = self.schedule.peers.get_mut(id) {
                    p.needed = true;
                }
                self.poll_at = self.poll_at.min(now() + POLL_MS);
                return;
            }
        }
        self.schedule.failed(id, now());
        if let Some(peer) = self.schedule.peers.get(id) {
            self.poll_at = self.poll_at.min(peer.retry_at.max(self.service_retry()));
        }
        self.manual_until = None;
        self.emit(Notice::Error(id.into(), error));
    }
    fn service_retry(&self) -> u64 {
        self.service_backoff.iter().map(|b| b.until).max().unwrap_or(0)
    }
    fn service_ok(&mut self, task: ServiceTask) {
        self.service_backoff[task as usize] = ServiceBackoff::default();
    }
    fn service_error(&mut self, task: ServiceTask, error: Error) {
        if error == Error::Cancelled {
            return;
        }
        let backoff = &mut self.service_backoff[task as usize];
        backoff.failures = backoff.failures.saturating_add(1).min(7);
        let delay = if error == Error::Busy {
            POLL_MS
        } else {
            (5_000u64 << (backoff.failures - 1)).min(300_000)
        };
        backoff.until = now() + delay;
        // Retry at the existing failure deadline, not at a stale 30-second
        // idle poll. Other operation/peer backoffs remain admission guards.
        self.poll_at = self.poll_at.min(backoff.until);
        self.manual_until = None;
        // No confirmed daily quota classifier: do not stop control heartbeats.
        if error != Error::Busy {
            self.verification.extend(self.schedule.peers.keys().cloned());
            for peer in self.schedule.peers.values_mut() { peer.needed = true; }
            self.emit(Notice::Error(String::new(), error));
        }
    }
    fn metadata_call(&mut self, op: Op, call: Call, deadline: Instant) {
        self.announcing = matches!(&op, Op::Announce(_));
        self.meta = Some(Box::pin(async move {
            let mut delay = Duration::from_millis(250);
            let result = loop {
                match call.execute().await {
                    Err(Error::Busy | Error::Closed) if Instant::now() + delay < deadline => {
                        tokio::time::sleep(delay).await;
                        delay = (delay * 2).min(Duration::from_secs(1));
                    }
                    result => break result,
                }
            };
            MetaDone::Response(op, result)
        }));
    }
    fn start_poll(&mut self) -> Result<()> {
        let client = self.client(None, self.cancel.clone())?;
        let at = now();
        let active = !self.attempts.is_empty();
        for a in self.attempts.values_mut() {
            a.polls += 1;
        }
        // Non-negative per-instance jitter keeps idle rate at or below 1/30 s.
        let jitter = self
            .instance
            .as_bytes()
            .iter()
            .fold(0u64, |n, b| n + u64::from(*b))
            % 2001;
        self.poll_at = at
            + if active {
                POLL_MS
            } else {
                IDLE_POLL_MS + jitter
            };
        self.meta = Some(Box::pin(async move {
            MetaDone::Poll(client.poll(at, Instant::now() + PROGRESS_TIMEOUT).await)
        }));
        Ok(())
    }
    fn prepare(&mut self) {
        let engine = self.engine.clone();
        let outbox = self.outbox.clone();
        let cancel = self.cancel.clone();
        let instance = self.instance.clone();
        self.needs_refresh = false;
        self.file = Some(Box::pin(async move {
            FileDone::Prepared(
                async {
                    engine.refresh(&cancel).await?;
                    outbox
                        .prepare(&engine.revision()?, &instance, now(), &cancel)
                        .await
                }
                .await,
            )
        }));
    }
    fn file_done(&mut self, done: FileDone) {
        match done {
            FileDone::Prepared(Ok(intent)) => {
                self.service_ok(ServiceTask::Catalog);
                self.pending = intent;
            }
            FileDone::Confirmed(Ok(Completion::Confirmed)) => {
                self.service_ok(ServiceTask::Catalog);
                self.service_ok(ServiceTask::Announce);
                self.poll_at = self.poll_at.min(now());
            }
            FileDone::Confirmed(Ok(Completion::Ignored)) => self.service_ok(ServiceTask::Catalog),
            FileDone::Confirmed(Ok(Completion::Rebased(intent))) => {
                self.service_ok(ServiceTask::Catalog);
                self.pending = Some(intent);
                let b = &mut self.service_backoff[ServiceTask::Announce as usize];
                b.until = b.until.max(now() + POLL_MS);
            }
            FileDone::Prepared(Err(e)) | FileDone::Confirmed(Err(e)) => {
                self.needs_refresh = true;
                self.service_error(ServiceTask::Catalog, e);
            }
        }
    }
    fn adopt_snapshot(&mut self, value: Snapshot) {
        for attempt in self.attempts.values_mut() {
            attempt.waiting_poll = false;
        }
        if self
            .snapshot
            .as_ref()
            .is_some_and(|old| old.boot != value.boot)
        {
            let ids: Vec<_> = self
                .attempts
                .keys()
                .chain(self.connected.keys())
                .cloned()
                .collect();
            for id in ids {
                if self.connected.get(&id).is_some_and(|c| c.finished) {
                    // A boot loss after verified completion only retires the
                    // close handshake; it does not create failed content work.
                    self.remove(&id);
                } else {
                    self.fail(&id, Error::RelaySessionLost);
                }
            }
        }
        // An announcement is a hint, not member authority. Changed process or
        // membership invalidates an old handshake before it can publish a Link.
        let changed: Vec<_> = self
            .connected
            .iter()
            .filter(|(id, c)| {
                value
                    .items
                    .get(*id)
                    .and_then(|i| i.announcement.as_ref())
                    .is_some_and(|a| {
                        a.instance != c.instance || a.membership != c.peer.remote_membership_id
                    })
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in changed {
            self.fail(&id, Error::RelaySessionLost);
        }
        for (id, item) in &value.items {
            if id == &self.own() {
                continue;
            }
            let Some(peer) = self.schedule.peers.get_mut(id) else {
                continue;
            };
            let Some(a) = &item.announcement else {
                continue;
            };
            let valid = self
                .authority
                .read()
                .ok()
                .and_then(|v| v.peer(id).ok())
                .is_some_and(|p| p.remote_membership_id == a.membership);
            if !valid {
                continue;
            }
            let fresh_wake = item.wake.as_ref().is_some_and(|w| {
                w.expires_at > now() && !self.seen_wakes.contains(id, &w.id, now())
            }) && item.handshake.as_ref().is_some_and(|h| !h.closed);
            if needs_round(
                self.engine.checkpoint(id).ok().flatten().as_deref(),
                &a.revision,
                self.forced.contains(id) || self.verification.contains(id)
                    || self.engine.full_catalog_due(id),
                fresh_wake,
            ) {
                peer.needed = true;
            } else if !peer.busy {
                peer.needed = false;
            }
        }
        self.snapshot = Some(value);
        self.discovery_stale = false;
    }
    fn meta_done(&mut self, done: MetaDone) -> Result<()> {
        match done {
            MetaDone::Poll(Ok(value)) => {
                self.service_ok(ServiceTask::Poll);
                self.adopt_snapshot(value);
            }
            MetaDone::Poll(Err(e)) => self.service_error(ServiceTask::Poll, e),
            MetaDone::Response(Op::Announce(intent), result) => {
                self.announcing = false;
                match self
                    .http
                    .checked(result.and_then(|r| Receipt::decode(r.json()?)))
                {
                    Ok(receipt) => {
                        self.confirm = Some((intent, receipt));
                    }
                    Err(e) => {
                        self.pending = Some(intent);
                        self.service_error(ServiceTask::Announce, e);
                    }
                }
            }
            MetaDone::Response(op, result) => {
                let (id, tag) = match &op {
                    Op::Wake(id, tag, _) | Op::Ready(id, tag, _) | Op::Open(id, tag) => {
                        (id.clone(), tag.clone())
                    }
                    _ => unreachable!(),
                };
                if !self.attempts.get(&id).is_some_and(|a| a.tag == tag) {
                    return Ok(());
                }
                let result = result.and_then(|r| {
                    self.attempts.get_mut(&id).unwrap().waiting_poll = true;
                    let bytes = r.json()?;
                    match op {
                        Op::Wake(_, _, intent) => match intent.response(bytes)? {
                            WakeResult::Accepted(wake) => {
                                self.attempts.get_mut(&id).unwrap().wake = Some(wake)
                            }
                            WakeResult::Current(_) => {
                                // The winner is discovered, never overwritten by
                                // a stale CAS retry. No new ID is generated here.
                                self.attempts.get_mut(&id).unwrap().wake = None;
                            }
                        },
                        Op::Ready(_, _, boot) => {
                            let context = self.attempt_context(&id, &boot)?;
                            metadata::ready_response(bytes, &context)?;
                            self.attempts.get_mut(&id).unwrap().ready_boot = Some(boot);
                        }
                        Op::Open(_, _) => {
                            let a = self.attempts.get_mut(&id).unwrap();
                            let tls = a.handshake.as_mut().ok_or(Error::Protocol)?.response(
                                bytes,
                                now(),
                                Instant::now(),
                            )?;
                            a.offered = true;
                            if let Some(tls) = tls {
                                self.install_tls(&id, tls)?;
                            }
                        }
                        _ => unreachable!(),
                    }
                    Ok(())
                });
                if let Err(e) = self.http.checked(result) {
                    self.fail(&id, e);
                }
            }
        }
        Ok(())
    }
    fn attempt_context(&self, id: &str, boot: &str) -> Result<super::SessionContext> {
        let a = self.attempts.get(id).ok_or(Error::Cancelled)?;
        let item = self.item(id).ok_or(Error::RelaySessionLost)?;
        let wake = item.wake.ok_or(Error::RelaySessionLost)?;
        if a.wake.as_deref() != Some(&wake.id) {
            return Err(Error::RelaySessionLost);
        }
        let nonce = if a.peer.local_device_id < a.peer.remote_device_id {
            a.nonce.clone()
        } else {
            item.handshake
                .as_ref()
                .and_then(|h| h.client.as_ref())
                .map(|c| c.signed.statement.nonce.clone())
                .unwrap_or_else(|| a.nonce.clone())
        };
        wake.context(
            a.peer.clone(),
            self.initial.membership.worker_url.clone(),
            boot.into(),
            nonce,
            &self.instance,
            &a.instance,
            now(),
        )
    }
    fn install_tls(&mut self, id: &str, tls: TlsStream) -> Result<()> {
        if self.activity.as_ref().is_some_and(|a| !a.can_add()) {
            self.attempts.get_mut(id).ok_or(Error::Cancelled)?.tls = Some(tls);
            return Ok(());
        }
        if self.activity.is_none() {
            self.activity = Some(Activity::new(
                Pump::new(self.context.clone(), self.initial.identity.clone()),
                self.http.clone(),
                self.cancel.clone(),
                self.authorized(),
                now(),
            )?);
        }
        let session = self.activity.as_mut().unwrap().add(tls)?;
        let attempt = self.attempts.remove(id).ok_or(Error::Cancelled)?;
        self.connected.insert(
            id.into(),
            Connected {
                peer: attempt.peer,
                instance: attempt.instance,
                session,
                wake: attempt.wake.ok_or(Error::Protocol)?,
                link: None,
                finished: false,
                closing_at: None,
            },
        );
        Ok(())
    }
    fn drive_attempt(&mut self, id: &str) -> Result<()> {
        if self.attempts.get(id).is_some_and(|a| a.tls.is_some()) {
            if self.activity.as_ref().is_none_or(|a| a.can_add()) {
                let tls = self.attempts.get_mut(id).unwrap().tls.take().unwrap();
                self.install_tls(id, tls)?;
            }
            return Ok(());
        }
        let boot = self.snapshot.as_ref().ok_or(Error::Busy)?.boot.clone();
        let item = self.item(id).ok_or(Error::Busy)?;
        let announcement = item.announcement.as_ref().ok_or(Error::Busy)?;
        if announcement.peer_version != super::PEER_VERSION {
            return Err(Error::ProtocolVersion);
        }
        let a = self.attempts.get(id).ok_or(Error::Cancelled)?;
        if a.waiting_poll {
            return Ok(());
        }
        if announcement.membership != a.peer.remote_membership_id
            || announcement.instance != a.instance
        {
            return Err(Error::RelaySessionLost);
        }
        let client = self.client(Some(&a.peer), a.cancel.clone())?;
        let tag = a.tag.clone();
        let until =
            Instant::now() + Duration::from_millis(a.until.saturating_sub(now()).min(20_000));
        let wake = item
            .wake
            .as_ref()
            .filter(|w| w.expires_at > now() && !self.seen_wakes.contains(id, &w.id, now()))
            .filter(|_| item.handshake.as_ref().is_some_and(|h| !h.closed));
        if a.wake.is_none() {
            if let Some(wake) = wake {
                // An old instance wake is unusable, but its ID remains the CAS
                // precondition when making a fresh intent below.
                if wake
                    .context(
                        a.peer.clone(),
                        self.initial.membership.worker_url.clone(),
                        boot.clone(),
                        a.nonce.clone(),
                        &self.instance,
                        &a.instance,
                        now(),
                    )
                    .is_ok()
                {
                    self.attempts.get_mut(id).unwrap().wake = Some(wake.id.clone());
                    return self.drive_attempt(id);
                }
            }
            let intent = WakeIntent::new(
                &self.initial.identity,
                &self.initial.membership,
                &a.peer,
                &self.instance,
                &a.instance,
                item.current_wake.as_deref(),
                now(),
            )?;
            let call = client.wake(&intent, now(), until)?;
            self.metadata_call(Op::Wake(id.into(), tag, intent), call, until);
            return Ok(());
        }
        if item.current_wake.as_deref() != a.wake.as_deref() {
            // waiting_poll already fenced the local pre-ACK snapshot. A fresh
            // poll now proves replacement/expiry, not a wake still in transit.
            return Err(Error::RelaySessionLost);
        }
        if wake.is_none() {
            return Err(Error::RelaySessionLost);
        }
        let context = self.attempt_context(id, &boot)?;
        if a.ready_boot.as_deref() != Some(&boot) {
            let call = client.ready(&context, now(), until)?;
            self.metadata_call(Op::Ready(id.into(), tag, boot), call, until);
            return Ok(());
        }
        let view = item.handshake.as_ref().ok_or(Error::Protocol)?;
        if a.handshake.is_none() {
            let hs = Handshake::start(
                &self.initial.identity,
                context,
                view,
                self.peer_check(&a.peer),
                a.cancel.clone(),
                now(),
                Instant::now(),
            )?;
            self.attempts.get_mut(id).unwrap().handshake = hs;
        }
        let a = self.attempts.get_mut(id).unwrap();
        if let Some(hs) = &mut a.handshake {
            if a.offered {
                if let Some(tls) = hs.observe(
                    &boot,
                    a.wake.as_deref().unwrap(),
                    view,
                    now(),
                    Instant::now(),
                )? {
                    self.install_tls(id, tls)?;
                }
            } else {
                let until = hs.deadline();
                let call = client.open(hs, now(), Instant::now())?;
                self.metadata_call(Op::Open(id.into(), tag), call, until);
            }
        }
        Ok(())
    }
    fn finished(&mut self, id: &str, result: &exchange::Outcome) {
        self.schedule.completed(id, now());
        let okay = result.failures.is_empty() && result.omitted == 0
            && !result.more && result.completed == result.total;
        if okay {
            self.forced.remove(id);
            self.verification.remove(id);
            self.service_ok(ServiceTask::Exchange);
        }
        if result.completed > 0
            || (result.failures.is_empty()
                && result.omitted == 0
                && !result.more
                && result.completed == result.total)
        {
            self.losses.remove(id);
        }
        self.progress.remove(id);
        self.manual_until = None;
        // Persist candidate catalogs on the next out-of-round FileIo refresh.
        // Open/Finish never acquire a file permit for this optimization.
        self.needs_refresh = true;
        if result.completed > 0 {
            self.schedule.changed(now());
        }
        if !result.failures.is_empty() || result.omitted > 0 {
            self.schedule.failed(id, now());
        } else if result.more {
            if let Some(p) = self.schedule.peers.get_mut(id) {
                p.needed = true;
            }
            self.schedule.occupied(id, now());
        }
        if let Some(c) = self.connected.get_mut(id) {
            c.finished = true;
            c.closing_at = Some(now());
            if let Some(activity) = self.activity.as_mut() {
                let _ = activity.close(&c.session);
            }
        }
    }
    fn event(&mut self, link: u64, event: exchange::Event) {
        let id = event.peer().to_string();
        if !self
            .connected
            .get(&id)
            .and_then(|c| c.link.as_ref())
            .is_some_and(|l| l.id == link)
        {
            return;
        }
        match &event {
            exchange::Event::Changed(_) => self.schedule.changed(now()),
            exchange::Event::Started(_) => {
                self.schedule.busy(&id, now());
                self.manual_until = None;
            }
            exchange::Event::Progress(_, done, _) => {
                self.record_progress(&id, *done);
            }
            exchange::Event::Finished(_, outcome) => self.finished(&id, outcome),
            exchange::Event::Failed(_, error) => {
                if self.connected.get(&id).is_some_and(|c| c.finished) {
                    self.remove(&id);
                    return;
                }
                self.fail(&id, *error);
                return;
            }
        }
        self.emit(Notice::Exchange(event));
    }
    fn activity_event(
        &mut self,
        result: std::result::Result<ActivityEvent, Failure>,
    ) -> Result<()> {
        match result {
            Ok(ActivityEvent::Ready { session, channels }) => {
                let Some(id) = self
                    .connected
                    .iter()
                    .find(|(_, c)| c.session == session)
                    .map(|(id, _)| id.clone())
                else {
                    return Ok(());
                };
                let connection = channels.into_connection()?;
                let link =
                    Link::start(connection, id.clone(), self.own() < id, self.engine.clone());
                {
                    let mut refs = self.connections.lock().map_err(|_| Error::Runtime)?;
                    refs.retain(|old| {
                        old.upgrade()
                            .is_some_and(|c| !c.cancellation().is_cancelled())
                    });
                    refs.push(Arc::downgrade(&link.connection));
                }
                self.connected.get_mut(&id).unwrap().link = Some(link);
                self.schedule.connected(&id, now());
                self.emit(Notice::Connected(id));
            }
            Ok(ActivityEvent::Retired { session, error }) => {
                if let Some(id) = self
                    .connected
                    .iter()
                    .find(|(_, c)| c.session == session)
                    .map(|(id, _)| id.clone())
                {
                    if self.connected[&id].finished {
                        self.remove(&id);
                    } else {
                        self.fail(&id, error);
                    }
                }
            }
            Err(Failure::Transport(error) | Failure::Response { error, .. }) => {
                let ids: Vec<_> = self.connected.keys().cloned().collect();
                for id in ids {
                    if self.connected[&id].finished {
                        self.remove(&id);
                    } else {
                        self.fail(&id, error);
                    }
                }
                self.activity = None;
                if error == Error::CloudflareResourceLimit {
                    self.service_error(ServiceTask::Exchange, error);
                }
            }
        }
        Ok(())
    }
    fn record_progress(&mut self, id: &str, done: u64) {
        let old = self.progress.entry(id.into()).or_default();
        if done > *old {
            *old = done;
            self.losses.remove(id);
        }
    }
    fn tick(&mut self) -> Result<()> {
        self.sync_epoch();
        let at = now();
        if !(self.authorized())() {
            return Ok(());
        }
        if self.work.serial() != self.serial {
            self.serial = self.work.serial();
            self.schedule.changed(at);
        }
        if self.schedule.take_changed(at) {
            self.cancelled_until = None;
            self.needs_refresh = true;
            self.forced.extend(self.schedule.peers.keys().cloned());
        }
        if at >= self.refresh_at {
            self.cancelled_until = None;
            self.refresh_at = at + HEARTBEAT_MS;
            self.needs_refresh = true;
        }
        let expired: Vec<_> = self
            .attempts
            .iter()
            .filter(|(_, a)| {
                at >= a.until
                    || (a.polls >= MAX_POLLS && self.meta.is_none() && at >= self.poll_at)
                    || a.handshake
                        .as_ref()
                        .is_some_and(|h| Instant::now() >= h.deadline())
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            self.fail(&id, Error::Timeout);
        }
        if self.manual_until.is_some_and(|until| at >= until) {
            self.manual_until = None;
            self.service_error(ServiceTask::Exchange, Error::Timeout);
        }
        if self.file.is_none() && !self.opening_or_syncing() && self.confirm.is_some() {
            let (intent, receipt) = self.confirm.take().unwrap();
            let outbox = self.outbox.clone();
            let cancel = self.cancel.clone();
            self.file = Some(Box::pin(async move {
                FileDone::Confirmed(outbox.complete(&intent, receipt, now(), &cancel).await)
            }));
        }
        if at < self.service_retry() {
            return Ok(());
        }
        if self.file.is_none() && self.needs_refresh && !self.opening_or_syncing() {
            self.prepare();
        }
        // A pending refresh/outbox write cannot be bypassed by a newly started
        // round. Existing Activity keeps moving concurrently with this owner.
        if self.file.is_some() {
            return Ok(());
        }
        if self.manual_until == Some(u64::MAX) {
            self.manual_until = Some(at + ACTIVE_MS);
        }
        if self.meta.is_none() && self.confirm.is_none() && !self.opening_or_syncing() {
            if let Some(intent) = self.pending.take() {
                let deadline = Instant::now() + PROGRESS_TIMEOUT;
                match self
                    .client(None, self.cancel.clone())?
                    .announce(&intent, at, deadline)
                {
                    Ok(call) => self.metadata_call(Op::Announce(intent), call, deadline),
                    Err(e) => {
                        self.pending = Some(intent);
                        self.service_error(ServiceTask::Announce, e);
                    }
                }
            }
        }
        if self.meta.is_none() && at >= self.poll_at {
            self.start_poll()?;
        }
        // An in-flight announcement has left pending but is not yet confirmed.
        // Opening now would block its FileIo confirmation behind that attempt,
        // while the attempt itself waits for confirmation: neither could run.
        if self.snapshot.is_none()
            || self.discovery_stale
            || self.pending.is_some()
            || self.confirm.is_some()
            || self.announcing
        {
            return Ok(());
        }
        if self.cancelled_until.is_some_and(|until| at < until) {
            return Ok(());
        }
        self.engine.yield_requested.store(
            self.candidates(at).iter().any(|id| {
                !self
                    .connected
                    .get(id)
                    .and_then(|c| c.link.as_ref())
                    .is_some_and(|l| l.active())
            }),
            Ordering::Relaxed,
        );
        // A remote caller must see our current process announcement before
        // its wake can bind both instances. Stale announcement is only a hint.
        let own = self.own();
        let announced = self
            .snapshot
            .as_ref()
            .and_then(|s| s.items.get(&own))
            .and_then(|i| i.announcement.as_ref())
            .is_some_and(|a| {
                a.instance == self.instance && a.membership == self.initial.membership.membership_id
            });
        if !announced {
            return Ok(());
        }
        for id in self.candidates(at) {
            if self.needs_refresh {
                break;
            }
            if self.attempts.contains_key(&id) || self.connected.contains_key(&id) {
                continue;
            }
            if self.attempts.len() + self.connected.len() >= 4 {
                break;
            }
            let Some(item) = self.item(&id) else { continue };
            let Some(a) = item.announcement else { continue };
            let peer = self
                .authority
                .read()
                .map_err(|_| Error::Runtime)?
                .peer(&id)?;
            if a.membership != peer.remote_membership_id {
                continue;
            }
            if a.peer_version != super::PEER_VERSION {
                self.schedule.failed(&id, at);
                self.emit(Notice::Error(id, Error::ProtocolVersion));
                continue;
            }
            self.attempts.insert(
                id.clone(),
                Attempt {
                    tag: uuid::Uuid::new_v4().to_string(),
                    peer,
                    instance: a.instance,
                    until: at + ACTIVE_MS,
                    polls: 0,
                    wake: None,
                    nonce: uuid::Uuid::new_v4().to_string(),
                    ready_boot: None,
                    handshake: None,
                    offered: false,
                    waiting_poll: false,
                    tls: None,
                    cancel: Cancellation::default(),
                },
            );
            self.schedule.busy(&id, at);
            self.poll_at = self.poll_at.min(at + POLL_MS);
            self.emit(Notice::Connecting(id));
        }
        if self.meta.is_none() {
            let ids: Vec<_> = self.attempts.keys().cloned().collect();
            for id in ids {
                match self.drive_attempt(&id) {
                    Ok(()) | Err(Error::Busy) => {}
                    Err(e) => self.fail(&id, e),
                };
                if self.meta.is_some() {
                    break;
                }
            }
        }
        if self.round.is_none() && !self.active() {
            if let Some((id, l)) = self
                .connected
                .iter()
                .filter(|(id, c)| own < **id && !c.finished)
                .find_map(|(id, c)| c.link.as_ref().map(|l| (id.clone(), l.clone())))
            {
                self.round = Some(Box::pin(async move {
                    let result = l.round().await;
                    (id, l.id, result)
                }));
            }
        }
        Ok(())
    }
    fn members(&mut self, a: Authority) -> Result<()> {
        a.validate()?;
        if a.membership != self.initial.membership
            || a.identity.public_key != self.initial.identity.public_key
        {
            return Err(Error::Unauthorized);
        }
        let replaced: Vec<_> = self
            .initial
            .members
            .iter()
            .filter(|m| {
                !a.members.iter().any(|n| {
                    n.device_id == m.device_id
                        && n.membership_id == m.membership_id
                        && n.public_key == m.public_key
                })
            })
            .map(|m| m.device_id.clone())
            .collect();
        for id in replaced {
            self.remove(&id);
            self.verification.remove(&id);
            self.engine.forget_checkpoint(&id);
            self.forced.insert(id);
        }
        self.initial = a.clone();
        *self.authority.write().map_err(|_| Error::Runtime)? = a.clone();
        let own = self.own();
        self.engine.catalog_members(a.members.iter().filter(|m| m.device_id != own)
            .filter_map(|m| a.peer(&m.device_id).ok()));
        let removed = self.schedule.members(
            a.members
                .iter()
                .filter(|m| m.device_id != own)
                .map(|m| (m.device_id.clone(), m.online)),
        );
        for id in removed {
            self.remove(&id);
            self.verification.remove(&id);
            self.engine.forget_checkpoint(&id);
            self.losses.remove(&id);
            self.forced.remove(&id);
        }
        let revoked: Vec<_> = self
            .connected
            .iter()
            .filter(|(id, c)| a.peer(id).ok().as_ref() != Some(&c.peer))
            .map(|(id, _)| id.clone())
            .chain(
                self.attempts
                    .iter()
                    .filter(|(id, c)| a.peer(id).ok().as_ref() != Some(&c.peer))
                    .map(|(id, _)| id.clone()),
            )
            .collect();
        for id in revoked {
            self.remove(&id);
            self.engine.forget_checkpoint(&id);
        }
        Ok(())
    }
    pub(crate) async fn run(mut self, mut commands: mpsc::Receiver<Command>) -> Result<()> {
        loop {
            self.tick()?;
            let work = self.work.clone();
            let shutdown = self.context.shutdown.clone();
            // All deadlines are finite state deadlines, not a fast idle tick.
            let at = now();
            let mut deadline = self.refresh_at.min(self.poll_at.max(self.service_retry()));
            if let Some(t) = self.schedule.changed_at {
                deadline = deadline.min(t);
            }
            if let Some(t) = self.manual_until {
                deadline = deadline.min(t);
            }
            for a in self.attempts.values() {
                deadline = deadline.min(a.until);
            }
            // Completed futures or messages re-run tick; an overdue deadline
            // currently blocked on owned IO must not spin in the meantime.
            if deadline <= at {
                deadline = at + POLL_MS;
            }
            tokio::select! {
                biased;
                _=shutdown.cancelled()=>break,
                _=work.notified()=>{},
                cmd=commands.recv()=>match cmd {
                    Some(Command::Members(a,_,_))=>self.members(a)?,
                    Some(Command::Manual(epoch))=>{self.sync_epoch();if epoch==self.epoch {self.schedule.manual();self.forced.extend(self.schedule.peers.keys().cloned());
                        self.engine.force_full_catalogs();
                        self.cancelled_until=None;self.needs_refresh=true;for b in &mut self.service_backoff {b.until=0;}self.poll_at=now();self.losses.clear();self.manual_until=Some(u64::MAX);}},
                    Some(Command::Cancel)=>{self.work.cancel();self.sync_epoch();},None=>break,
                },
                result=optional(&mut self.file)=>{self.file=None;self.file_done(result);},
                result=optional(&mut self.meta)=>{self.meta=None;self.meta_done(result)?;},
                result=optional(&mut self.round)=>{self.round=None;let(id,link,r)=result;
                    if self.connected.get(&id).and_then(|c|c.link.as_ref()).is_some_and(|l|l.id==link) {
                        match r {Ok(outcome)=>{self.finished(&id,&outcome);self.emit(Notice::Exchange(exchange::Event::Finished(id,outcome)));},
                            Err(Error::Busy)=>{self.fail(&id,Error::Busy);self.emit(Notice::Exchange(exchange::Event::Finished(id,exchange::Outcome{more:true,..Default::default()})));},
                            Err(e)=>self.fail(&id,e),}
                    }
                },
                event=self.events.recv()=>if let Some((id,e))=event{self.event(id,e)},
                result=async {match self.activity.as_mut(){Some(a)=>a.next().await,None=>pending().await}}=>self.activity_event(result)?,
                _=tokio::time::sleep(Duration::from_millis(deadline.saturating_sub(at)))=>{},
            }
        }
        self.cancel_all();
        Ok(())
    }
}
async fn optional<T>(value: &mut Option<Flight<T>>) -> T {
    match value {
        Some(v) => v.await,
        None => pending().await,
    }
}
fn needs_round(checkpoint: Option<&str>, revision: &str, forced: bool, fresh_wake: bool) -> bool {
    forced || fresh_wake || checkpoint != Some(revision)
}

#[cfg(test)]
mod tests;
