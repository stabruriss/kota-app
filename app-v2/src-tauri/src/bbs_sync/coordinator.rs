//! Account network actor. Blocking HTTPS belongs to the control owner; this
//! actor owns only bounded scheduling state and tasks on NetworkHost's runtime.
use super::{
    control::{Member, Membership, Signal},
    exchange::{self, Engine, Link},
    rendezvous::{Kind, Rendezvous, SignedWake},
    scheduler::Schedule,
    transport::{
        Cancellation, Connection, Context, Error, MembershipCheck, NetworkMode, PeerIdentity,
        PendingConnection, Result, SessionRole, SignedDescription,
    },
    DeviceIdentity,
};
use crate::bbs::sync::{ContentStore, GroupFence};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::SyncSender,
        Arc, Mutex, RwLock, Weak,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{mpsc, oneshot};

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
/// Cancellation and marker serials are memory-only and separate from membership.
#[derive(Default)]
pub(crate) struct Work {
    epoch: AtomicU64,
    changes: AtomicU64,
    cutoff: AtomicU64,
}
impl Work {
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }
    pub(crate) fn changed(&self) {
        self.changes.fetch_add(1, Ordering::AcqRel);
    }
    pub(crate) fn serial(&self) -> u64 {
        self.changes.load(Ordering::Acquire)
    }
    pub(crate) fn cancel(&self) {
        self.cutoff.store(self.serial(), Ordering::Release);
        self.epoch.fetch_add(1, Ordering::AcqRel);
    }
}
#[derive(Clone)]
pub(crate) struct Authority {
    pub membership: Membership,
    pub identity: DeviceIdentity,
    pub members: Vec<Member>,
    pub valid_until: u64,
}
impl Authority {
    pub(crate) fn validate(&self) -> Result<()> {
        let own = self.identity.device_id().map_err(|_| Error::Unauthorized)?;
        if self.members.is_empty()
            || self.members.len() > 32
            || self
                .members
                .iter()
                .filter(|m| m.role == super::control::Role::Owner)
                .count()
                != 1
        {
            return Err(Error::Unauthorized);
        }
        let mut seen = BTreeSet::new();
        for m in &self.members {
            if !seen.insert(&m.device_id)
                || m.name.is_empty()
                || m.name.encode_utf16().count() > 80
                || m.name.chars().any(char::is_control)
            {
                return Err(Error::Unauthorized);
            }
            if m.device_id != own {
                PeerIdentity::current(
                    &self.membership,
                    &self.members,
                    &self.identity,
                    &m.device_id,
                )?;
            }
        }
        let me = self
            .members
            .iter()
            .find(|m| m.device_id == own)
            .ok_or(Error::Unauthorized)?;
        if me.membership_id != self.membership.membership_id
            || me.public_key != self.identity.public_key
            || me.role != self.membership.role
        {
            return Err(Error::Unauthorized);
        }
        Ok(())
    }
    fn peer(&self, id: &str) -> Result<PeerIdentity> {
        if now() >= self.valid_until {
            return Err(Error::Unauthorized);
        }
        PeerIdentity::current(&self.membership, &self.members, &self.identity, id)
    }
}
#[derive(Clone)]
pub(crate) struct Port {
    pub tx: SyncSender<ControlWork>,
    pub fence: GroupFence,
    pub epoch: u64,
}
pub(crate) struct ControlWork {
    pub fence: GroupFence,
    pub epoch: u64,
    pub expires: u64,
    pub kind: ControlKind,
    pub reply: oneshot::Sender<std::result::Result<Vec<Signal>, String>>,
}
pub(crate) enum ControlKind {
    Send { to: String, payload: String },
    Pull,
}
impl Port {
    async fn request(&self, kind: ControlKind, expires: u64) -> Result<Vec<Signal>> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .try_send(ControlWork {
                fence: self.fence.clone(),
                epoch: self.epoch,
                expires,
                kind,
                reply: tx,
            })
            .map_err(|_| Error::Busy)?;
        tokio::time::timeout(Duration::from_secs(20), rx)
            .await
            .map_err(|_| Error::Timeout)?
            .map_err(|_| Error::Closed)?
            .map_err(|e| {
                if e == "stale_signature" {
                    Error::StaleSignature
                } else {
                    Error::InvalidSignal
                }
            })
    }
    async fn send(&self, to: &str, payload: Envelope, expires: u64) -> Result<()> {
        let payload = serde_json::to_string(&payload).map_err(|_| Error::Protocol)?;
        if payload.len() > 64 * 1024 {
            return Err(Error::InvalidSignal);
        }
        self.request(
            ControlKind::Send {
                to: to.into(),
                payload,
            },
            expires,
        )
        .await
        .map(|_| ())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "camelCase",
    deny_unknown_fields
)]
enum Envelope {
    Wake(SignedWake),
    Description(SignedDescription),
}
pub(crate) enum Command {
    Members(Authority, Vec<Signal>, u64),
    Manual(u64),
    Cancel,
}
#[derive(Clone)]
pub(crate) enum Notice {
    Exchange(exchange::Event),
    Connecting(String),
    Connected(String),
    Disconnected(String),
    Diagnostic(String, super::public::ConnectionInfo),
    Error(String, Error),
}
pub(crate) type Observe = Arc<dyn Fn(u64, Notice) + Send + Sync>;
enum Internal {
    Signals(u64, Result<Vec<Signal>>),
    Sent(String, String, Result<()>),
    Connected(String, PeerIdentity, String, Result<Connection>),
    Exchange(u64, exchange::Event),
    Refreshed(u64, Result<()>),
    Round(String, u64, Result<exchange::Outcome>),
    Diagnostic(String, Arc<Connection>, super::public::ConnectionInfo),
}
struct Running {
    nonce: String,
    cancel: Cancellation,
    answer: Option<oneshot::Sender<SignedDescription>>,
}
pub(crate) struct Actor {
    context: Context,
    mode: NetworkMode,
    engine: Arc<Engine>,
    authority: Arc<RwLock<Authority>>,
    previous_authority: Authority,
    connections: Arc<Mutex<Vec<Weak<Connection>>>>,
    port: Port,
    observe: Observe,
    schedule: Schedule,
    rendezvous: Rendezvous,
    links: BTreeMap<String, Arc<Link>>,
    running: BTreeMap<String, Running>,
    tx: mpsc::Sender<Internal>,
    rx: mpsc::Receiver<Internal>,
    refreshing: bool,
    needs_refresh: bool,
    pulling: bool,
    work: Arc<Work>,
    epoch: u64,
    seen_change: u64,
    work_stop: Cancellation,
    broadcast_changed: bool,
    manual_deadline: AtomicU64,
}
impl Actor {
    pub(crate) fn new(
        context: Context,
        store: ContentStore,
        authority: Authority,
        port: Port,
        observe: Observe,
    ) -> Result<Self> {
        authority.validate()?;
        let (tx, rx) = mpsc::channel(128);
        let events = tx.clone();
        let shutdown = context.shutdown.clone();
        let on_exchange = Arc::new(move |link, event| {
            if events.try_send(Internal::Exchange(link, event)).is_err() {
                shutdown.cancel();
            }
        });
        let engine = Engine::new(
            store,
            GroupFence {
                group_id: authority.membership.group_id.clone(),
                membership_id: authority.membership.membership_id.clone(),
            },
            context.clone(),
            on_exchange,
        );
        let mut schedule = Schedule::default();
        schedule.join(now());
        schedule.members(
            authority
                .members
                .iter()
                .filter(|m| m.device_id != authority.identity.device_id().unwrap_or_default())
                .map(|m| (m.device_id.clone(), m.online)),
        );
        Ok(Self {
            context,
            mode: NetworkMode::Direct,
            engine,
            previous_authority: authority.clone(),
            authority: Arc::new(RwLock::new(authority)),
            connections: Arc::new(Mutex::new(Vec::new())),
            port,
            observe,
            schedule,
            rendezvous: Rendezvous::default(),
            links: BTreeMap::new(),
            running: BTreeMap::new(),
            tx,
            rx,
            refreshing: false,
            needs_refresh: true,
            pulling: false,
            work: Arc::new(Work::default()),
            epoch: 0,
            seen_change: 0,
            work_stop: Cancellation::default(),
            broadcast_changed: false,
            manual_deadline: AtomicU64::new(0),
        })
    }
    pub(crate) fn roster(mut self, roster: Arc<super::roster::runtime::Runtime>) -> Self {
        Arc::get_mut(&mut self.engine)
            .expect("unstarted exchange engine")
            .roster = Some(roster);
        self
    }
    pub(crate) fn live_authority(
        mut self,
        authority: Arc<RwLock<Authority>>,
        connections: Arc<Mutex<Vec<Weak<Connection>>>>,
    ) -> Self {
        self.authority = authority;
        self.connections = connections;
        self
    }
    pub(crate) fn work_control(mut self, work: Arc<Work>) -> Self {
        self.epoch = work.epoch();
        self.seen_change = work.serial();
        self.work = work;
        self
    }
    fn emit(&self, notice: Notice) {
        if self.epoch == self.work.epoch() {
            if matches!(&notice, Notice::Connecting(_) | Notice::Exchange(
                exchange::Event::Started(_) | exchange::Event::Progress(..) | exchange::Event::Finished(..)
            )) || matches!(&notice, Notice::Error(_, e) if *e != Error::Busy) {
                // The connection/round now owns its own progress deadline, or
                // the manual attempt has already produced a terminal result.
                self.manual_deadline.store(0, Ordering::Relaxed);
            }
            (self.observe)(self.epoch, notice);
        }
    }
    fn sync_epoch(&mut self) {
        let next = self.work.epoch();
        if next == self.epoch {
            return;
        }
        self.epoch = next;
        self.manual_deadline.store(0, Ordering::Relaxed);
        self.seen_change = self.work.cutoff.load(Ordering::Acquire);
        self.work_stop.cancel();
        self.work_stop = Cancellation::default();
        self.rendezvous.cancel_current(now());
        self.cancel_all();
        self.schedule.cancel_current(now());
        self.needs_refresh = false;
        self.refreshing = false;
        self.pulling = false;
        self.broadcast_changed = false;
    }
    fn scoped_port(&self) -> Port {
        Port {
            epoch: self.epoch,
            ..self.port.clone()
        }
    }
    #[cfg(test)]
    pub(crate) fn loopback(mut self) -> Self {
        self.mode = NetworkMode::Loopback;
        self
    }
    pub(crate) async fn run(mut self, mut commands: mpsc::Receiver<Command>) -> Result<()> {
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _=self.context.shutdown.cancelled()=>break,
                command = commands.recv() => match command {
                    Some(Command::Members(authority, signals, epoch)) => {
                        self.members(authority, epoch)?;
                        if epoch == self.epoch { self.signals(signals); }
                    }
                    Some(Command::Manual(epoch)) => {
                        self.sync_epoch();
                        if epoch == self.epoch {
                            self.schedule.manual();
                            self.needs_refresh = true;
                            if !self.links.values().any(|l| l.active()) {
                                // Arm after metadata preparation, not a total
                                // duration cap on scanning a growing library.
                                self.manual_deadline.store(u64::MAX, Ordering::Relaxed);
                            }
                        }
                    }
                    Some(Command::Cancel) => {
                        self.work.cancel();
                        self.sync_epoch();
                    }
                    None => break,
                },
                event=self.rx.recv()=>if let Some(event)=event {self.event(event)?},
                _=tick.tick()=>self.tick(),
            }
        }
        self.cancel_all();
        Ok(())
    }
    fn cancel_all(&mut self) {
        for (id, link) in std::mem::take(&mut self.links) {
            link.cancel();
            self.emit(Notice::Disconnected(id));
        }
        for (_, task) in std::mem::take(&mut self.running) {
            task.cancel.cancel()
        }
        for id in self.schedule.peers.keys().cloned().collect::<Vec<_>>() {
            self.rendezvous.remove(&id);
            self.schedule.released(&id);
        }
    }
    fn members(&mut self, a: Authority, epoch: u64) -> Result<()> {
        self.sync_epoch();
        a.validate()?;
        let old = self.previous_authority.clone();
        if a.membership != old.membership || a.identity.public_key != old.identity.public_key {
            return Err(Error::Unauthorized);
        }
        let own = a.identity.device_id().map_err(|_| Error::Unauthorized)?;
        *self.authority.write().map_err(|_| Error::Runtime)? = a.clone();
        self.previous_authority = a.clone();
        let previous_need = self
            .schedule
            .peers
            .iter()
            .map(|(id, p)| (id.clone(), p.needed))
            .collect::<BTreeMap<_, _>>();
        self.schedule.members(
            a.members
                .iter()
                .filter(|m| m.device_id != own)
                .map(|m| (m.device_id.clone(), m.online)),
        );
        // Online/offline does not revoke identity; a changed membership/public key does.
        let changed = old
            .members
            .iter()
            .filter(|m| {
                !a.members.iter().any(|n| {
                    n.device_id == m.device_id
                        && n.public_key == m.public_key
                        && n.membership_id == m.membership_id
                })
            })
            .map(|m| m.device_id.clone())
            .collect::<Vec<_>>();
        for id in changed {
            self.remove(&id)
        }
        for link in self.links.values() {
            let _ = link.connection.recheck_membership();
        }
        if epoch == self.epoch && self.schedule.heartbeat_due(now()) {
            self.needs_refresh = true;
        } else if epoch != self.epoch {
            for (id, peer) in &mut self.schedule.peers {
                peer.needed = previous_need.get(id).copied().unwrap_or(false);
            }
        }
        Ok(())
    }
    fn remove(&mut self, id: &str) {
        if let Some(l) = self.links.remove(id) {
            l.cancel()
        }
        if let Some(r) = self.running.remove(id) {
            r.cancel.cancel()
        }
        self.rendezvous.remove(id);
        self.schedule.released(id);
        self.emit(Notice::Disconnected(id.into()));
    }
    fn peer(&self, id: &str) -> Result<PeerIdentity> {
        self.authority.read().map_err(|_| Error::Runtime)?.peer(id)
    }
    fn authorize(&self, peer: &PeerIdentity) -> MembershipCheck {
        let a = self.authority.clone();
        let p = peer.clone();
        let work = self.work.clone();
        let epoch = self.epoch;
        Arc::new(move || {
            if work.epoch() != epoch {
                return false;
            }
            a.read()
                .ok()
                .and_then(|a| a.peer(&p.remote_device_id).ok())
                .is_some_and(|current| current == p)
        })
    }
    fn tick(&mut self) {
        self.sync_epoch();
        let now = now();
        self.expire_manual_wait(now);
        self.tick_work(now);
    }
    fn expire_manual_wait(&mut self, now: u64) {
        let manual_deadline = self.manual_deadline.load(Ordering::Relaxed);
        if manual_deadline != 0 && manual_deadline != u64::MAX && now >= manual_deadline {
            // A manual request to an idle responder must not look "Connecting"
            // forever while the offerer is offline/in backoff. Do not bypass
            // its retry policy; terminate this attempt at the existing active
            // window bound. Normal future automatic eligibility is retained.
            self.manual_deadline.store(0, Ordering::Relaxed);
            self.cancel_all();
            for id in self.schedule.peers.keys().cloned().collect::<Vec<_>>() {
                self.schedule.failed(&id, now);
            }
            self.emit(Notice::Error(String::new(), Error::Timeout));
        }
    }
    fn tick_work(&mut self, now: u64) {
        if self.work.serial() != self.seen_change {
            self.seen_change = self.work.serial();
            self.broadcast_changed = true;
            self.schedule.changed(now);
        }
        if self.authority.read().map_or(true, |a| now >= a.valid_until) {
            self.cancel_all();
            return;
        }
        for id in self.rendezvous.expires(now) {
            self.remove(&id);
            self.schedule.failed(&id, now);
            self.emit(Notice::Error(id, Error::Timeout));
        }
        if self.rendezvous.poll_due(now) && !self.pulling {
            self.pulling = true;
            let epoch = self.epoch;
            let port = self.scoped_port();
            let tx = self.tx.clone();
            tokio::spawn(async move {
                let r = port.request(ControlKind::Pull, now + 20_000).await;
                let _ = tx.send(Internal::Signals(epoch, r)).await;
            });
        }
        if self.schedule.take_changed(now) {
            self.needs_refresh = true;
            if std::mem::take(&mut self.broadcast_changed) {
                for l in self.links.values() {
                    let l = l.clone();
                    tokio::spawn(async move {
                        let _ = l.changed().await;
                    });
                }
            }
        }
        let closed = self
            .links
            .iter()
            .filter(|(_, l)| l.is_stopped())
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in closed {
            self.remove(&id);
        }
        let waiting = self
            .schedule
            .peers
            .values()
            .any(|p| p.needed && !p.busy && p.online && now >= p.retry_at);
        self.engine.yield_requested.store(
            waiting && self.links.values().any(|l| l.active()),
            Ordering::Relaxed,
        );
        if self.links.len() + self.rendezvous.len().max(self.running.len()) >= 4 {
            if let Some(id) = self.schedule.yield_idle(now) {
                self.remove(&id)
            }
        }
        if self.needs_refresh && !self.refreshing && !self.links.values().any(|l| l.active()) {
            self.refreshing = true;
            let engine = self.engine.clone();
            let tx = self.tx.clone();
            let stop = self.work_stop.clone();
            let epoch = self.epoch;
            tokio::spawn(async move {
                let result = engine.refresh(&stop).await;
                let _ = tx.send(Internal::Refreshed(epoch, result)).await;
            });
        }
        if self.needs_refresh || self.refreshing {
            return;
        }
        let _ = self.manual_deadline.compare_exchange(
            u64::MAX, now.saturating_add(super::scheduler::ACTIVE_MS), Ordering::Relaxed, Ordering::Relaxed,
        );
        for id in self.schedule.candidates(now) {
            if let Some(link) = self.links.get(&id) {
                if self.links.values().any(|l| l.active()) {
                    break;
                }
                let own = self
                    .authority
                    .read()
                    .ok()
                    .and_then(|a| a.identity.device_id().ok())
                    .unwrap_or_default();
                if own < id {
                    self.schedule.busy(&id, now);
                    let l = link.clone();
                    let tx = self.tx.clone();
                    tokio::spawn(async move {
                        let result = l.round().await;
                        let _ = tx.send(Internal::Round(id, l.id, result)).await;
                    });
                    break;
                } else {
                    self.schedule.occupied(&id, now);
                    let l = link.clone();
                    tokio::spawn(async move {
                        let _ = l.changed().await;
                    });
                }
            } else if self.links.len() + self.rendezvous.len().max(self.running.len()) < 4
                && !self.rendezvous.contains(&id)
            {
                if let Some(nonce) = self.rendezvous.begin(&id, now) {
                    self.schedule.busy(&id, now);
                    self.wake(&id, Kind::Wake, &nonce, now + 60_000);
                }
            }
        }
    }
    fn wake(&self, id: &str, kind: Kind, nonce: &str, expires: u64) {
        let result = self.peer(id).and_then(|p| {
            let a = self.authority.read().map_err(|_| Error::Runtime)?;
            SignedWake::create(&a.identity, &p, kind, nonce, now())
        });
        let port = self.scoped_port();
        let tx = self.tx.clone();
        let id = id.to_owned();
        let nonce = nonce.to_owned();
        tokio::spawn(async move {
            let r = match result {
                Ok(w) => port.send(&id, Envelope::Wake(w), expires).await,
                Err(e) => Err(e),
            };
            let _ = tx.send(Internal::Sent(id, nonce, r)).await;
        });
    }
    fn signals(&mut self, signals: Vec<Signal>) {
        if signals.len() > 32 {
            return;
        }
        for signal in signals {
            let id = signal.from.clone();
            if let Err(e) = self.signal(signal) {
                if e != Error::Busy {
                    self.schedule.penalize(&id, now());
                }
                self.emit(Notice::Error(id, e));
            }
        }
    }
    fn signal(&mut self, signal: Signal) -> Result<()> {
        let now = now();
        if signal.expires_at <= now || signal.payload.len() > 64 * 1024 {
            return Err(Error::InvalidSignal);
        }
        let peer = self.peer(&signal.from)?;
        if signal.to != peer.local_device_id {
            return Err(Error::Unauthorized);
        }
        if self
            .schedule
            .peers
            .get(&signal.from)
            .is_none_or(|p| now < p.retry_at)
            && !self.rendezvous.contains(&signal.from)
        {
            return Err(Error::Busy);
        }
        let envelope: Envelope =
            serde_json::from_str(&signal.payload).map_err(|_| Error::InvalidSignal)?;
        match envelope {
            Envelope::Wake(wake) => {
                let message = wake.verify(&peer, now)?;
                if self
                    .rendezvous
                    .was_cancelled(&signal.from, &message.nonce, now)
                {
                    return Ok(());
                }
                match message.kind {
                    Kind::Wake => {
                        if self.rendezvous.wake_seen(&signal.from, &message.nonce) {
                            return Ok(());
                        }
                        if self.links.contains_key(&signal.from)
                            || self.running.contains_key(&signal.from)
                        {
                            return Err(Error::Busy);
                        }
                        if !self.rendezvous.contains(&signal.from)
                            && self.links.len() + self.rendezvous.len().max(self.running.len()) >= 4
                        {
                            self.wake(&signal.from, Kind::Busy, &message.nonce, now + 20_000);
                            return Err(Error::Busy);
                        }
                        let (kind, nonce) = self.rendezvous.wake(
                            &peer.local_device_id,
                            &signal.from,
                            &message.nonce,
                            now,
                        )?;
                        self.schedule.busy(&signal.from, now);
                        self.wake(&signal.from, kind, &nonce, now + 60_000);
                    }
                    Kind::Ready => {
                        let until = self.rendezvous.ready(
                            &peer.local_device_id,
                            &signal.from,
                            &message.nonce,
                            now,
                        )?;
                        self.begin_connect(
                            peer,
                            SessionRole::Offer,
                            message.nonce.clone(),
                            until,
                            None,
                        )?;
                    }
                    Kind::Busy => {
                        if !self.rendezvous.matches(&signal.from, &message.nonce) {
                            return Err(Error::InvalidSignal);
                        }
                        self.remove(&signal.from);
                        self.schedule.occupied(&signal.from, now);
                    }
                }
            }
            Envelope::Description(signed) => {
                let role = if peer.local_device_id < peer.remote_device_id {
                    SessionRole::Answer
                } else {
                    SessionRole::Offer
                };
                signed.verify(&peer, role, signed.nonce(), now)?;
                if self
                    .rendezvous
                    .was_cancelled(&signal.from, signed.nonce(), now)
                {
                    return Ok(());
                }
                let until = self.rendezvous.description(
                    &peer.local_device_id,
                    &signal.from,
                    signed.nonce(),
                    now,
                )?;
                if role == SessionRole::Answer {
                    let tx = self
                        .running
                        .get_mut(&signal.from)
                        .and_then(|r| r.answer.take())
                        .ok_or(Error::InvalidSignal)?;
                    tx.send(signed).map_err(|_| Error::Closed)?;
                } else {
                    self.begin_connect(
                        peer,
                        SessionRole::Answer,
                        signed.nonce().to_owned(),
                        until,
                        Some(signed),
                    )?;
                }
            }
        }
        Ok(())
    }
    fn begin_connect(
        &mut self,
        peer: PeerIdentity,
        role: SessionRole,
        nonce: String,
        until: u64,
        offer: Option<SignedDescription>,
    ) -> Result<()> {
        if self.running.contains_key(&peer.remote_device_id)
            || self.links.len() + self.running.len() >= 4
        {
            return Err(Error::Busy);
        }
        let cancel = Cancellation::default();
        let (answer_tx, answer_rx) = oneshot::channel();
        let id = peer.remote_device_id.clone();
        self.running.insert(
            id.clone(),
            Running {
                nonce: nonce.clone(),
                cancel: cancel.clone(),
                answer: if role == SessionRole::Offer {
                    Some(answer_tx)
                } else {
                    None
                },
            },
        );
        self.emit(Notice::Connecting(id.clone()));
        let context = self.context.clone();
        let mode = self.mode;
        let attempt = nonce.clone();
        let authority = self.authorize(&peer);
        let identity = self
            .authority
            .read()
            .map_err(|_| Error::Runtime)?
            .identity
            .clone();
        let port = self.scoped_port();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let work = async {
                let mut p =
                    PendingConnection::new(context, peer.clone(), role, nonce, authority, mode)
                        .await?;
                if let Some(offer) = offer {
                    p.remote_description(&offer).await?;
                }
                let local = p.local_description(&identity).await?;
                port.send(&id, Envelope::Description(local), until.min(now() + 30_000))
                    .await?;
                if role == SessionRole::Offer {
                    let answer = answer_rx.await.map_err(|_| Error::Closed)?;
                    p.remote_description(&answer).await?;
                }
                p.connect().await
            };
            let duration = Duration::from_millis(until.saturating_sub(now()).min(30_000));
            let result = tokio::select! {_=cancel.cancelled()=>Err(Error::Cancelled),v=tokio::time::timeout(duration,work)=>v.unwrap_or(Err(Error::Timeout))};
            let _ = tx
                .send(Internal::Connected(id, peer, attempt, result))
                .await;
        });
        Ok(())
    }
    fn event(&mut self, event: Internal) -> Result<()> {
        self.sync_epoch();
        let now = now();
        match event {
            Internal::Signals(epoch, result) => {
                if epoch != self.epoch {
                    return Ok(());
                }
                self.pulling = false;
                if let Ok(signals) = result {
                    self.signals(signals)
                }
            }
            Internal::Sent(id, nonce, result) => {
                if self.rendezvous.matches(&id, &nonce) {
                    if let Err(e) = result {
                        self.remove(&id);
                        self.schedule.failed(&id, now);
                        self.emit(Notice::Error(id, e));
                    }
                }
            }
            Internal::Connected(id, peer, nonce, result) => {
                if !self.running.get(&id).is_some_and(|r| r.nonce == nonce) {
                    if let Ok(connection) = result {
                        connection.cancel();
                    }
                    return Ok(());
                }
                let expected = self.running.remove(&id).is_some()
                    && self.peer(&id).is_ok_and(|p| p == peer)
                    && self.epoch == self.work.epoch();
                self.rendezvous.remove(&id);
                match result {
                    Ok(connection) if expected => {
                        let smaller = peer.local_device_id < peer.remote_device_id;
                        let link =
                            Link::start(connection, id.clone(), smaller, self.engine.clone());
                        {
                            let mut connections =
                                self.connections.lock().map_err(|_| Error::Runtime)?;
                            connections.retain(|c| {
                                c.upgrade().is_some_and(|c| c.recheck_membership().is_ok())
                            });
                            connections.push(Arc::downgrade(&link.connection));
                        }
                        self.links.insert(id.clone(), link);
                        self.schedule.released(&id);
                        self.schedule.connected(&id, now);
                        self.emit(Notice::Connected(id.clone()));
                        self.diagnose(&id);
                    }
                    Ok(c) => c.cancel(),
                    Err(e) => {
                        self.schedule.released(&id);
                        if e == Error::Busy {
                            self.schedule.occupied(&id, now)
                        } else {
                            self.schedule.failed(&id, now);
                            self.emit(Notice::Error(id, e));
                        }
                    }
                }
            }
            Internal::Refreshed(epoch, result) => {
                if epoch != self.epoch {
                    return Ok(());
                }
                self.refreshing = false;
                match result {
                    Ok(()) => self.needs_refresh = false,
                    Err(Error::Busy) => {}
                    Err(e) => {
                        self.needs_refresh = false;
                        self.emit(Notice::Error(String::new(), e));
                    }
                }
            }
            Internal::Round(id, link, result) => {
                if !self
                    .links
                    .get(&id)
                    .is_some_and(|current| current.id == link)
                {
                    return Ok(());
                }
                match result {
                    Ok(result) => {
                        self.finished(&id, &result);
                        self.emit(Notice::Exchange(exchange::Event::Finished(id, result)));
                    }
                    Err(Error::Busy) => {
                        self.schedule.occupied(&id, now);
                        // Yield is neither a failed member nor a completed
                        // sync. Settle an earlier Started/pending projection so
                        // it cannot stay spinning after this attempt ended.
                        self.emit(Notice::Exchange(exchange::Event::Finished(id, exchange::Outcome {
                            more: true, ..Default::default()
                        })));
                    }
                    Err(e) => {
                        self.remove(&id);
                        self.schedule.failed(&id, now);
                        self.emit(Notice::Error(id, e));
                    }
                }
            }
            Internal::Diagnostic(id, connection, info) => {
                // A late stats reply cannot resurrect a closed/replaced peer.
                if self
                    .links
                    .get(&id)
                    .is_some_and(|l| Arc::ptr_eq(&l.connection, &connection) && !l.is_stopped())
                {
                    self.emit(Notice::Diagnostic(id, info));
                }
            }
            Internal::Exchange(link, event) => {
                if !self
                    .links
                    .get(event.peer())
                    .is_some_and(|current| current.id == link)
                {
                    return Ok(());
                }
                match &event {
                    exchange::Event::Changed(_) => {
                        self.schedule.changed(now);
                    }
                    exchange::Event::Started(id) => self.schedule.busy(id, now),
                    exchange::Event::Progress(id, _, _) => self.schedule.busy(id, now),
                    exchange::Event::Finished(id, result) => self.finished(id, result),
                    exchange::Event::Failed(id, e) => {
                        self.remove(id);
                        if *e != Error::Cancelled {
                            self.schedule.failed(id, now)
                        }
                    }
                }
                self.emit(Notice::Exchange(event));
            }
        }
        Ok(())
    }
    fn finished(&mut self, id: &str, result: &exchange::Outcome) {
        #[cfg(not(test))]
        crate::kota_debug_log(&format!(
            "[bbs-sync] round peer={id} completed={} total={} failures={} omitted={} more={}",
            result.completed,
            result.total,
            result.failures.len(),
            result.omitted,
            result.more
        ));
        self.schedule.completed(id, now());
        if result.completed > 0 {
            // Forward newly installed content to other group members. A zero-
            // write round never creates another Changed loop.
            self.schedule.changed(now());
            self.broadcast_changed = true;
        }
        if result.more {
            if let Some(p) = self.schedule.peers.get_mut(id) {
                p.needed = true;
            }
        }
        self.needs_refresh = true;
        self.diagnose(id);
    }
    fn diagnose(&self, id: &str) {
        if let Some(link) = self.links.get(id) {
            let connection = link.connection.clone();
            let id = id.to_owned();
            let tx = self.tx.clone();
            tokio::spawn(async move {
                if let Ok(d) = connection.diagnostics().await {
                    #[cfg(not(test))]
                    crate::kota_debug_log(&format!(
                        "[bbs-sync] candidate peer={id} local={} remote={} basis={} sent_bytes={}",
                        d.local_candidate_type.as_deref().unwrap_or("unknown"),
                        d.remote_candidate_type.as_deref().unwrap_or("unknown"),
                        d.remote_candidate_type_basis
                            .as_deref()
                            .unwrap_or("unknown"),
                        d.sent_bytes
                    ));
                    let _ = tx
                        .send(Internal::Diagnostic(
                            id.clone(),
                            connection,
                            super::public::ConnectionInfo {
                                device_id: id,
                                state: "connected".into(),
                                local_candidate_type: d.local_candidate_type,
                                remote_candidate_type: d.remote_candidate_type,
                                basis: d.remote_candidate_type_basis,
                                remote_address: d.remote_address,
                                sent_bytes: d.sent_bytes,
                            },
                        ))
                        .await;
                }
            });
        }
    }
}

#[cfg(test)]
mod tests;
