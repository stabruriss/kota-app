//! Tauri's handle is a memory projection plus bounded mailboxes. Only the
//! explicitly owned control thread can open credentials or perform HTTPS.
use super::roster;
use super::{
    control::{self, ControlClient, HttpsTransport, InvitationResult, OwnerConnection},
    coordinator::{self, Actor, Authority, ControlKind, ControlWork, Notice, Port},
    exchange,
    public::{self, Status},
    transport::{self, NetworkHost},
};
use crate::bbs::sync::{ContentStore, GroupFence};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc, Mutex, RwLock, Weak,
    },
    time::Duration,
};
use tokio::sync::{oneshot, Notify, OwnedSemaphorePermit, Semaphore};

#[derive(Default)]
struct View {
    status: Status,
    diagnostics: public::Diagnostics,
    fence: Option<(String, String)>,
    // Control failures overlay the exchange projection. A healthy heartbeat
    // removes only this overlay, preserving any independent round failure.
    control_error: Option<(public::Error, String)>,
    // Accepted Manual/Retry work is visible until a real coordinator event or
    // a refreshed no-peer/control-error result settles it. This is not success.
    manual_pending: bool,
}
impl View {
    fn connection_state(&mut self, id: &str, state: &str) {
        if !self.status.group.members.iter().any(|m| m.id == id) {
            return;
        }
        if let Some(connection) = self
            .diagnostics
            .connections
            .iter_mut()
            .find(|c| c.device_id == id)
        {
            connection.state = state.into();
        } else if self.diagnostics.connections.len() < 32 {
            self.diagnostics.connections.push(public::ConnectionInfo {
                device_id: id.into(),
                state: state.into(),
                ..Default::default()
            });
        }
    }
}
struct Worker {
    generation: u64,
    tx: SyncSender<Message>,
}
struct Network {
    fence: GroupFence,
    _watch: Option<notify::RecommendedWatcher>,
    authority: Arc<RwLock<Authority>>,
    connections: Arc<Mutex<Vec<Weak<transport::Connection>>>>,
    host: NetworkHost,
    tx: tokio::sync::mpsc::Sender<coordinator::Command>,
}
struct Inner {
    store: ContentStore,
    roster: Arc<roster::runtime::Runtime>,
    transport: Arc<dyn Fn() -> Box<dyn control::Transport + Send> + Send + Sync>,
    owner: Arc<dyn Fn() -> Option<OwnerConnection> + Send + Sync>,
    view: Mutex<View>,
    worker: Mutex<Option<Worker>>,
    network: Mutex<Option<Network>>,
    retired: Mutex<Vec<NetworkHost>>,
    generation: AtomicU64,
    network_epoch: AtomicU64,
    booted: AtomicBool,
    ready: Notify,
    flight: Arc<Semaphore>,
    avatars: Arc<Semaphore>,
    shutdown: AtomicBool,
    manual: AtomicBool,
    work: Arc<coordinator::Work>,
    emit: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    last_progress_event: AtomicU64,
}
#[derive(Clone)]
pub(crate) struct Manager(Arc<Inner>);
// Raw code is deliberately neither Debug nor persisted in this mailbox.
enum Action {
    Invitation(bool),
    Join(String),
    Disconnect,
    Rename(String),
    Remove(String),
    ResumeSync(u64),
}
struct Message {
    expected: Option<String>,
    incarnation: Option<String>,
    action: Action,
    reply: oneshot::Sender<std::result::Result<Option<InvitationResult>, String>>,
    _flight: OwnedSemaphorePermit,
}
impl Manager {
    pub(crate) fn account() -> Self {
        Self::at(ContentStore::account())
    }
    fn at(store: ContentStore) -> Self {
        Self::services(
            store,
            Arc::new(|| Box::new(HttpsTransport::default())),
            Arc::new(|| {
                crate::laughing_man::load_state()
                    .standby
                    .as_ref()
                    .and_then(|c| OwnerConnection::from_paired(c).ok())
            }),
        )
    }
    fn services(
        store: ContentStore,
        transport: Arc<dyn Fn() -> Box<dyn control::Transport + Send> + Send + Sync>,
        owner: Arc<dyn Fn() -> Option<OwnerConnection> + Send + Sync>,
    ) -> Self {
        Self(Arc::new(Inner {
            roster: roster::runtime::Runtime::new(store.clone()),
            store,
            transport,
            owner,
            view: Mutex::new(View::default()),
            worker: Mutex::new(None),
            network: Mutex::new(None),
            retired: Mutex::new(Vec::new()),
            generation: AtomicU64::new(0),
            network_epoch: AtomicU64::new(0),
            booted: AtomicBool::new(false),
            ready: Notify::new(),
            flight: Arc::new(Semaphore::new(1)),
            avatars: Arc::new(Semaphore::new(2)),
            shutdown: AtomicBool::new(false),
            manual: AtomicBool::new(false),
            work: Arc::new(coordinator::Work::default()),
            emit: Mutex::new(None),
            last_progress_event: AtomicU64::new(0),
        }))
    }
    pub(crate) fn status(&self) -> Status {
        let view = self.0.view.lock().unwrap_or_else(|p| p.into_inner());
        let mut status = view.status.clone();
        if view.manual_pending {
            status.sync.phase = public::Phase::Connecting;
            status.sync.error = None;
            status.sync.completed = None;
            status.sync.total = None;
        } else if let Some((_, detail)) = &view.control_error {
            status.sync.phase = public::Phase::Failed;
            status.sync.error = Some(detail.clone());
        }
        status.sync.control_recoverable = status.group.id.is_some()
            && self
                .0
                .worker
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_none();
        status
    }
    pub(crate) fn diagnostics(&self) -> public::Diagnostics {
        let view = self.0.view.lock().unwrap_or_else(|p| p.into_inner());
        let mut d = view.diagnostics.clone();
        if let Some((error, _)) = &view.control_error {
            d.last_error = Some(error.code.into());
        }
        d.protocol_version = 1;
        d
    }
    pub(crate) fn setup(
        &self,
        emit: Arc<dyn Fn() + Send + Sync>,
        roster_emit: Arc<dyn Fn() + Send + Sync>,
    ) {
        let work = self.0.work.clone();
        self.0
            .roster
            .start(roster_emit, Arc::new(move || work.changed()));
        *self.0.emit.lock().unwrap() = Some(emit);
        let this = self.clone();
        tauri::async_runtime::spawn_blocking(move || this.bootstrap());
    }
    fn bootstrap(&self) {
        if self.0.roster.is_started() {
            tauri::async_runtime::block_on(self.0.roster.wait_initialized());
        }
        self.owner_configuration();
        match control::inspect_existing(&self.0.store.state) {
            Ok(Some((identity, name, membership, pending))) => {
                {
                    let mut view = self.0.view.lock().unwrap();
                    view.status.device.id = identity.device_id().unwrap_or_default();
                    view.status.device.name = name;
                    view.status.membership(membership.as_ref(), &[]);
                    view.fence = membership
                        .as_ref()
                        .map(|m| (m.group_id.clone(), m.membership_id.clone()));
                }
                if membership.is_some() || pending {
                    if let Err(e) = self.ensure_worker(false, None) {
                        self.error(&e)
                    }
                }
            }
            Ok(None) => {}
            Err(e) => self.error(&e),
        }
        self.0.booted.store(true, Ordering::Release);
        self.0.ready.notify_waiters();
        self.emit(false);
    }
    async fn ready(&self) {
        loop {
            let ready = self.0.ready.notified();
            if self.0.booted.load(Ordering::Acquire) {
                return;
            }
            ready.await;
        }
    }
    fn emit(&self, progress: bool) {
        if progress {
            let now = coordinator::now();
            let old = self.0.last_progress_event.load(Ordering::Relaxed);
            if now.saturating_sub(old) < 500 {
                return;
            }
            self.0.last_progress_event.store(now, Ordering::Relaxed);
        }
        if let Some(emit) = self.0.emit.lock().ok().and_then(|s| s.clone()) {
            emit();
        }
    }
    fn error(&self, code: &str) {
        self.control_error(code, None);
    }
    fn control_error(&self, code: &str, work: Option<u64>) {
        {
            let mut v = self.0.view.lock().unwrap();
            if work.is_some_and(|epoch| self.0.work.epoch() != epoch) {
                return;
            }
            v.manual_pending = false;
            v.control_error = Some((public::Error::from_code(code), public::display_error(code)));
        }
        self.emit(false);
    }
    fn control_recovered(&self, epoch: u64) {
        let changed = {
            let mut view = self.0.view.lock().unwrap();
            if self.0.work.epoch() != epoch {
                return;
            }
            let cleared = view.control_error.take().is_some();
            let no_peer = !view.status.group.members.iter()
                .any(|m| m.id != view.status.device.id && m.online);
            let settled = no_peer && std::mem::take(&mut view.manual_pending);
            cleared || settled
        };
        if changed {
            self.emit(false);
        }
    }
    fn work_error(&self, epoch: u64, code: &str) {
        {
            let mut v = self.0.view.lock().unwrap();
            if self.0.work.epoch() != epoch {
                return;
            }
            v.manual_pending = false;
            v.status.sync.phase = public::Phase::Failed;
            v.status.sync.error = Some(public::display_error(code));
            v.diagnostics.last_error = Some(public::Error::from_code(code).code.into());
        }
        self.emit(false);
    }
    fn owner_configuration(&self) -> Option<OwnerConnection> {
        let owner = (self.0.owner)();
        {
            let mut v = self.0.view.lock().unwrap();
            v.status.worker.configured = owner.is_some();
            v.status.worker.can_create_group = owner.is_some();
        }
        owner
    }
    /// Called after the existing LM pairing command succeeds, never by status.
    pub(crate) fn pairing_changed(&self) {
        let this = self.clone();
        tauri::async_runtime::spawn_blocking(move || {
            this.owner_configuration();
            this.emit(false);
        });
    }
    fn fence(&self, expected: Option<&str>) -> std::result::Result<Option<String>, public::Error> {
        let v = self
            .0
            .view
            .lock()
            .map_err(|_| public::Error::from_code("sync_unavailable"))?;
        if v.status.group.id.as_deref() != expected {
            return Err(public::Error::from_code("group_context_changed"));
        }
        Ok(v.fence.as_ref().map(|(_, m)| m.clone()))
    }
    fn ensure_worker(
        &self,
        initialize: bool,
        first: Option<Message>,
    ) -> std::result::Result<(), String> {
        let mut slot = self.0.worker.lock().map_err(|_| "control_busy")?;
        if let Some(w) = slot.as_ref() {
            if let Some(message) = first {
                w.tx.try_send(message).map_err(|_| "control_busy")?;
            }
            return Ok(());
        }
        let (tx, rx) = mpsc::sync_channel(1);
        if let Some(message) = first {
            tx.try_send(message).map_err(|_| "control_busy")?;
        }
        let generation = self.0.generation.fetch_add(1, Ordering::Relaxed) + 1;
        *slot = Some(Worker {
            generation,
            tx: tx.clone(),
        });
        let this = self.clone();
        std::thread::Builder::new()
            .name("kota-bbs-control".into())
            .spawn(move || {
                let result = this.control_thread(&rx, initialize, generation);
                if let Err(error) = &result {
                    this.error(error);
                }
                {
                    // Retire atomically with mailbox admission. Requests that
                    // raced a failed open receive its actual error, not a lost
                    // oneshot or a misleading not_joined response.
                    let mut slot = this.0.worker.lock().unwrap();
                    if slot.as_ref().is_some_and(|w| w.generation == generation) {
                        *slot = None;
                    }
                    while let Ok(message) = rx.try_recv() {
                        let error = result
                            .as_ref()
                            .err()
                            .map(String::as_str)
                            .unwrap_or("sync_unavailable");
                        let _ = message.reply.send(Err(error.into()));
                    }
                }
                this.emit(false);
            })
            .map_err(|_| {
                *slot = None;
                "sync_runtime_failed".to_owned()
            })?;
        Ok(())
    }
    async fn action(
        &self,
        expected: Option<String>,
        action: Action,
    ) -> std::result::Result<Option<InvitationResult>, public::Error> {
        self.ready().await;
        let incarnation = self.fence(expected.as_deref())?;
        self.request_action(expected, incarnation, action).await
    }
    async fn request_action(
        &self,
        expected: Option<String>,
        incarnation: Option<String>,
        action: Action,
    ) -> std::result::Result<Option<InvitationResult>, public::Error> {
        let flight = self
            .0
            .flight
            .clone()
            .try_acquire_owned()
            .map_err(|_| public::Error::from_code("control_busy"))?;
        if self.fence(expected.as_deref())? != incarnation {
            return Err(public::Error::from_code("group_context_changed"));
        }
        let initialize = matches!(action, Action::Invitation(_) | Action::Join(_));
        if matches!(action, Action::Disconnect) {
            self.0.work.cancel();
            self.stop_network();
        }
        let (reply, result) = oneshot::channel();
        self.ensure_worker(
            initialize,
            Some(Message {
                expected,
                incarnation,
                action,
                reply,
                _flight: flight,
            }),
        )
        .map_err(|e| public::Error::from_code(&e))?;
        result
            .await
            .map_err(|_| public::Error::from_code("sync_unavailable"))?
            .map_err(|e| public::Error::from_code(&e))
    }
    pub(crate) async fn invitation(
        &self,
        r: public::InvitationRequest,
    ) -> std::result::Result<InvitationResult, public::Error> {
        self.action(r.expected_group_id, Action::Invitation(r.refresh))
            .await?
            .ok_or_else(|| public::Error::from_code("sync_unavailable"))
    }
    pub(crate) async fn join(
        &self,
        r: public::JoinRequest,
    ) -> std::result::Result<(), public::Error> {
        self.action(r.expected_group_id, Action::Join(r.invitation))
            .await
            .map(|_| ())
    }
    pub(crate) async fn disconnect(
        &self,
        r: public::Command,
    ) -> std::result::Result<(), public::Error> {
        self.action(r.expected_group_id, Action::Disconnect)
            .await
            .map(|_| ())
    }
    pub(crate) async fn rename(
        &self,
        r: public::RenameRequest,
    ) -> std::result::Result<(), public::Error> {
        self.action(r.expected_group_id, Action::Rename(r.name))
            .await
            .map(|_| ())
    }
    pub(crate) async fn remove(
        &self,
        r: public::RemoveRequest,
    ) -> std::result::Result<(), public::Error> {
        self.action(r.expected_group_id, Action::Remove(r.device_id))
            .await
            .map(|_| ())
    }
    pub(crate) async fn start(&self, r: public::Command) -> std::result::Result<(), public::Error> {
        self.ready().await;
        let incarnation = self.fence(r.expected_group_id.as_deref())?;
        if self.admit_start(r.expected_group_id.as_deref(), incarnation.as_deref())? {
            return Ok(());
        }
        self.request_action(
            r.expected_group_id,
            incarnation,
            Action::ResumeSync(self.0.work.epoch()),
        )
        .await
        .map(|_| ())
    }
    fn admit_start(
        &self,
        expected: Option<&str>,
        incarnation: Option<&str>,
    ) -> std::result::Result<bool, public::Error> {
        let net = self.0.network.lock().unwrap();
        let mut view = self.0.view.lock().unwrap();
        if view.status.group.id.as_deref() != expected
            || view.fence.as_ref().map(|(_, m)| m.as_str()) != incarnation
        {
            return Err(public::Error::from_code("group_context_changed"));
        }
        if expected.is_none() {
            return Err(public::Error::from_code("not_joined"));
        }
        if self.0.worker.lock().unwrap().is_none() {
            return Ok(false);
        }
        if let Some(net) = net.as_ref() {
            if Some(net.fence.group_id.as_str()) != expected
                || Some(net.fence.membership_id.as_str()) != incarnation
            {
                return Err(public::Error::from_code("group_context_changed"));
            }
            net.tx
                .try_send(coordinator::Command::Manual(self.0.work.epoch()))
                .map_err(|_| public::Error::from_code("control_busy"))?;
        }
        // Admission is fenced with the snapshot: a stale request cannot leave
        // a global manual flag for a different group or membership incarnation.
        self.0.manual.store(true, Ordering::Release);
        if view.status.sync.phase != public::Phase::Syncing {
            view.manual_pending = true;
        }
        drop(view);
        drop(net);
        self.emit(false);
        Ok(true)
    }
    pub(crate) async fn cancel(
        &self,
        r: public::Command,
    ) -> std::result::Result<(), public::Error> {
        self.ready().await;
        let incarnation = self.fence(r.expected_group_id.as_deref())?;
        {
            let net = self.0.network.lock().unwrap();
            let mut v = self.0.view.lock().unwrap();
            if net
                .as_ref()
                .is_some_and(|n| Some(n.fence.group_id.as_str()) != r.expected_group_id.as_deref())
                || v.status.group.id != r.expected_group_id
                || v.fence.as_ref().map(|(_, m)| m) != incarnation.as_ref()
            {
                return Err(public::Error::from_code("group_context_changed"));
            }
            self.0.manual.store(false, Ordering::Release);
            self.0.work.cancel();
            v.manual_pending = false;
            v.status.sync.phase = public::Phase::Idle;
            v.status.sync.completed = None;
            v.status.sync.total = None;
            for connection in &mut v.diagnostics.connections {
                connection.state = "disconnected".into();
            }
            drop(v);
            if let Some(net) = net.as_ref() {
                for connection in net
                    .connections
                    .lock()
                    .unwrap()
                    .iter()
                    .filter_map(Weak::upgrade)
                {
                    connection.cancel();
                }
            }
        }
        self.emit(false);
        Ok(())
    }
    fn stop_network(&self) {
        self.0.network_epoch.fetch_add(1, Ordering::AcqRel);
        if let Some(net) = self.0.network.lock().unwrap().take() {
            net.host.stop();
            self.0.retired.lock().unwrap().push(net.host);
        }
    }
    fn cleanup_retired(&self) {
        let retired = {
            let mut retired = self.0.retired.lock().unwrap();
            std::mem::take(&mut *retired)
        };
        for old in retired {
            tauri::async_runtime::block_on(old.stop_and_wait());
        }
    }
    pub(crate) fn shutdown(&self) {
        self.0.shutdown.store(true, Ordering::Release);
        self.stop_network();
        self.0.roster.stop();
    }
    pub(crate) fn notification_files(
        &self,
    ) -> std::result::Result<transport::FileIo, transport::Error> {
        self.0.roster.files().map(|(files, _)| files)
    }
    pub(crate) async fn avatar(
        &self,
        request: public::AvatarRequest,
    ) -> std::result::Result<String, public::Error> {
        let permit = self
            .0
            .avatars
            .clone()
            .try_acquire_owned()
            .map_err(|_| public::Error::from_code("sync_busy"))?;
        let store = self.0.store.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let _permit = permit;
            store.read_verified_avatar(&request.sha256, &request.ext)
        })
        .await
        .map_err(|_| public::Error::from_code("sync_unavailable"))?
        .map_err(|_| public::Error::from_code("sync_unavailable"))
    }
    pub(crate) fn roster_page(
        &self,
        request: roster::public::ReadRequest,
    ) -> std::result::Result<roster::public::Page, roster::public::Error> {
        self.0.roster.page(request)
    }
    pub(crate) async fn roster_avatar(
        &self,
        request: roster::public::AvatarRequest,
    ) -> std::result::Result<String, roster::public::Error> {
        let (resource, authority) = self
            .0
            .roster
            .avatar_authorization(&request.device_id, &request.sha256)?;
        let permit = self
            .0
            .avatars
            .clone()
            .try_acquire_owned()
            .map_err(|_| roster::public::Error::unavailable())?;
        let this = self.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let _permit = permit;
            if let Some(peer) = &authority {
                peer.check(&this.0.store.state)
                    .map_err(|_| roster::public::Error::unavailable())?;
            }
            let value = this
                .0
                .store
                .read_roster_avatar(&resource)
                .map_err(|_| roster::public::Error::unavailable())?;
            if this
                .0
                .roster
                .avatar_resource(&request.device_id, &request.sha256)?
                != resource
            {
                return Err(roster::public::Error::unavailable());
            }
            if let Some(peer) = &authority {
                peer.check(&this.0.store.state)
                    .map_err(|_| roster::public::Error::unavailable())?;
            }
            Ok(value)
        })
        .await
        .map_err(|_| roster::public::Error::unavailable())?
    }
    fn control_thread(
        &self,
        rx: &Receiver<Message>,
        initialize: bool,
        generation: u64,
    ) -> Result<(), String> {
        transport::background_thread().map_err(|_| "sync_runtime_failed")?;
        let owner = self.owner_configuration();
        let transport = (self.0.transport)();
        let opened = if initialize {
            ControlClient::open(self.0.store.state.clone(), transport, owner, "This device")
        } else {
            ControlClient::open_existing(self.0.store.state.clone(), transport, owner)
        };
        let mut client = opened?;
        // A previous owner may have left or changed groups while holding the
        // lease. Publish the newly loaded state before accepting any request.
        self.project(&client);
        if tauri::async_runtime::block_on(self.0.roster.recover_staging()).is_err() {
            crate::kota_debug_log("[bbs-roster] staging_recovery_failed");
        }
        let (net_tx, net_rx) = mpsc::sync_channel(16);
        let mut heartbeat = 0;
        let mut retry = 5_000u64;
        let mut pending_at = 0;
        let mut queued = None;
        let mut work_epoch = self.0.work.epoch();
        loop {
            if self.0.shutdown.load(Ordering::Acquire) {
                break;
            }
            let now = coordinator::now();
            let current_work = self.0.work.epoch();
            if current_work != work_epoch {
                work_epoch = current_work;
                if heartbeat <= now {
                    heartbeat = now.saturating_add(60_000);
                }
            }
            if self.0.manual.swap(false, Ordering::AcqRel) {
                heartbeat = 0;
            }
            match queued.take().map(Ok).unwrap_or_else(|| rx.try_recv()) {
                Ok(message) => {
                    let resume_epoch = match &message.action {
                        Action::ResumeSync(epoch) => Some(*epoch),
                        _ => None,
                    };
                    client.update_owner(self.owner_configuration());
                    let same = client.membership().map(|m| m.group_id.as_str())
                        == message.expected.as_deref()
                        && client.membership().map(|m| m.membership_id.as_str())
                            == message.incarnation.as_deref();
                    let result = if !same {
                        Err("group_context_changed".into())
                    } else {
                        match message.action {
                            Action::Invitation(refresh) => client
                                .invitation(message.expected.as_deref(), refresh)
                                .map(Some),
                            Action::Join(code) => client
                                .join(message.expected.as_deref(), &code)
                                .map(|_| None),
                            Action::Disconnect => {
                                client.disconnect(message.expected.as_deref()).map(|_| None)
                            }
                            Action::Rename(name) => client
                                .rename(message.expected.as_deref(), &name)
                                .map(|_| None),
                            Action::Remove(id) => {
                                client.remove(message.expected.as_deref(), &id).map(|_| {
                                    self.revoke_member(&id);
                                    None
                                })
                            }
                            Action::ResumeSync(epoch) => (|| {
                                if self.0.work.epoch() != epoch {
                                    return Err("sync_cancelled".into());
                                }
                                // Resume the original durable intent first;
                                // Retry neither replaces its request ID nor
                                // uses an obsolete in-memory membership.
                                client.resume_pending()?;
                                if self.0.work.epoch() != epoch {
                                    return Err("sync_cancelled".into());
                                }
                                if client.membership().map(|m| m.group_id.as_str())
                                    != message.expected.as_deref()
                                    || client.membership().map(|m| m.membership_id.as_str())
                                        != message.incarnation.as_deref()
                                {
                                    return Err("group_context_changed".into());
                                }
                                Ok(None)
                            })(),
                        }
                    };
                    if let Err(e) = &result {
                        self.control_error(e, resume_epoch);
                        if client.has_pending() {
                            pending_at = now + 5_000;
                        }
                    }
                    self.project(&client);
                    if result.is_ok() {
                        if let Some(epoch) = resume_epoch {
                            let mut view = self.0.view.lock().unwrap();
                            if self.0.work.epoch() == epoch
                                && view.status.group.id == message.expected
                                && view.fence.as_ref().map(|(_, m)| m) == message.incarnation.as_ref()
                            {
                                view.manual_pending = true;
                            }
                            drop(view);
                            self.emit(false);
                        }
                    }
                    let _ = message.reply.send(result);
                    if resume_epoch.is_none_or(|epoch| epoch == self.0.work.epoch()) {
                        heartbeat = 0;
                    } else {
                        heartbeat = coordinator::now().saturating_add(60_000);
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => break,
                Err(mpsc::TryRecvError::Empty) => {}
            }
            if client.has_pending() && now >= pending_at {
                match client.resume_pending() {
                    Ok(()) => {
                        retry = 5_000;
                        heartbeat = 0
                    }
                    Err(e) => {
                        self.error(&e);
                        pending_at = now.saturating_add(retry);
                        retry = (retry * 2).min(300_000);
                    }
                }
                self.project(&client);
            }
            if client.membership().is_some() && now >= heartbeat {
                let group = client.membership().unwrap().group_id.clone();
                let epoch = work_epoch;
                let refreshed = client.refresh_members(Some(&group));
                heartbeat = coordinator::now() + 60_000;
                self.project(&client);
                match refreshed {
                    Ok(()) => {
                        self.control_recovered(epoch);
                        let signals = client.take_signals();
                        self.network(&client, net_tx.clone(), signals, epoch);
                    }
                    Err(e) => {
                        if client.has_pending() && pending_at <= coordinator::now() {
                            pending_at = coordinator::now().saturating_add(retry);
                        }
                        self.control_error(&e, Some(epoch));
                    }
                }
            }
            if client.membership().is_none() {
                self.stop_network();
            }
            if let Ok(work) = net_rx.try_recv() {
                let valid = !work.reply.is_closed()
                    && work.epoch == self.0.work.epoch()
                    && coordinator::now() < work.expires
                    && client.membership().is_some_and(|m| {
                        m.group_id == work.fence.group_id
                            && m.membership_id == work.fence.membership_id
                    });
                let result = if !valid {
                    Err("group_context_changed".into())
                } else {
                    match work.kind {
                        ControlKind::Send { to, payload } => client
                            .send_signal(&work.fence.group_id, &to, &payload)
                            .map(|_| vec![]),
                        ControlKind::Pull => client.pull_signals(&work.fence.group_id),
                    }
                };
                let _ = work.reply.send(result);
            }
            self.cleanup_retired();
            // Expiry invalidates the public code in memory even when HTTP failed.
            if client
                .invitation_expiry()
                .is_some_and(|expiry| coordinator::now() >= expiry)
            {
                self.project(&client);
            }
            if client.membership().is_none() && !client.has_pending() {
                // Admission and shutdown use the same slot lock. No accepted
                // action can be discarded between observing idle and exiting.
                let mut slot = self.0.worker.lock().unwrap();
                match rx.try_recv() {
                    Ok(message) => queued = Some(message),
                    Err(_) => {
                        if slot.as_ref().is_some_and(|w| w.generation == generation) {
                            *slot = None;
                        }
                        break;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.cleanup_retired();
        Ok(())
    }
    fn revoke_member(&self, id: &str) {
        let slot = self.0.network.lock().unwrap();
        if let Some(net) = slot.as_ref() {
            let authority = {
                let mut authority = net.authority.write().unwrap();
                authority.members.retain(|m| m.device_id != id);
                authority.clone()
            };
            net.connections
                .lock()
                .unwrap()
                .retain(|c| c.upgrade().is_some_and(|c| c.recheck_membership().is_ok()));
            if net
                .tx
                .try_send(coordinator::Command::Members(
                    authority,
                    vec![],
                    self.0.work.epoch(),
                ))
                .is_err()
            {
                net.host.stop();
            }
        }
    }
    fn project<T: control::Transport>(&self, client: &ControlClient<T>) {
        let members = client.status_memory().and_then(|r| r.members.as_deref());
        if members.is_some() || client.membership().is_none() {
            let members = members.unwrap_or(&[]);
            if roster::persist_members(&self.0.store.state, client.membership(), members).is_err() {
                crate::kota_debug_log("[bbs-roster] member_projection_failed");
                self.0.roster.member_projection_failed();
            } else {
                self.0.roster.member_projection(
                    client.device_id().unwrap_or_default(),
                    client.device_name().into(),
                    client.membership().cloned(),
                    members,
                );
            }
        }
        let generation = client.invitation_generation(coordinator::now());
        let changed = {
            let mut view = self.0.view.lock().unwrap();
            let previous = view.status.clone();
            let next = client
                .membership()
                .map(|m| (m.group_id.clone(), m.membership_id.clone()));
            if view.fence != next {
                view.status.sync = public::Progress::default();
                view.diagnostics = public::Diagnostics::default();
                view.control_error = None;
                view.manual_pending = false;
            }
            view.fence = next;
            view.status.device.id = client.device_id().unwrap_or_default();
            view.status.device.name = client.device_name().into();
            view.status.membership(
                client.membership(),
                client
                    .status_memory()
                    .and_then(|s| s.members.as_deref())
                    .unwrap_or(&[]),
            );
            let ids = view
                .status
                .group
                .members
                .iter()
                .map(|m| m.id.clone())
                .collect::<Vec<_>>();
            view.diagnostics
                .connections
                .retain(|c| ids.contains(&c.device_id));
            view.status.invitation_generation = generation.clone();
            view.status.invitation = if generation.is_some() {
                public::Invitation::Ready
            } else if client
                .membership()
                .is_some_and(|m| m.role == control::Role::Owner)
                && client.has_pending()
            {
                public::Invitation::Preparing
            } else {
                public::Invitation::None
            };
            view.status != previous
        };
        if changed {
            self.emit(false);
        }
    }
    fn network<T: control::Transport>(
        &self,
        client: &ControlClient<T>,
        tx: SyncSender<ControlWork>,
        signals: Vec<control::Signal>,
        work_epoch: u64,
    ) {
        if client.leaving() {
            self.stop_network();
            return;
        }
        let Some(membership) = client.membership().cloned() else {
            return;
        };
        let Some(members) = client.status_memory().and_then(|r| r.members.clone()) else {
            return;
        };
        let authority = Authority {
            membership: membership.clone(),
            identity: client.identity(),
            members,
            valid_until: coordinator::now() + 90_000,
        };
        if let Err(e) = authority.validate() {
            self.stop_network();
            self.work_error(work_epoch, &e.to_string());
            return;
        }
        let fence = GroupFence {
            group_id: membership.group_id.clone(),
            membership_id: membership.membership_id.clone(),
        };
        let mut slot = self.0.network.lock().unwrap();
        if slot.as_ref().is_some_and(|n| {
            n.fence.group_id != fence.group_id || n.fence.membership_id != fence.membership_id
        }) {
            if let Some(net) = slot.take() {
                net.host.stop();
                self.0.retired.lock().unwrap().push(net.host);
                self.0.network_epoch.fetch_add(1, Ordering::AcqRel);
            }
        }
        if let Some(net) = slot.as_ref() {
            *net.authority.write().unwrap() = authority.clone();
            {
                let mut connections = net.connections.lock().unwrap();
                connections.retain(|c| {
                    if let Some(connection) = c.upgrade() {
                        connection.recheck_membership().is_ok()
                    } else {
                        false
                    }
                });
            }
            if net
                .tx
                .try_send(coordinator::Command::Members(
                    authority, signals, work_epoch,
                ))
                .is_err()
            {
                net.host.stop();
                self.0.retired.lock().unwrap().push(net.host.clone());
                self.0.network_epoch.fetch_add(1, Ordering::AcqRel);
                *slot = None;
            }
            return;
        }
        let epoch = self.0.network_epoch.load(Ordering::Acquire);
        drop(slot);
        if work_epoch != self.0.work.epoch() {
            return;
        }
        // No peer means one control heartbeat only, not a network/file runtime.
        let own = authority.identity.device_id().unwrap_or_default();
        if signals.is_empty()
            && !authority
                .members
                .iter()
                .any(|m| m.device_id != own && m.online)
        {
            return;
        }
        // Stop/cleanup must finish before a replacement gets its own file
        // permit and worker. This wait is on the background control owner.
        self.cleanup_retired();
        if epoch != self.0.network_epoch.load(Ordering::Acquire) {
            return;
        }
        let host = match self.0.roster.files().and_then(|(io, limits)| {
            tauri::async_runtime::block_on(NetworkHost::start_with_files(io, limits))
        }) {
            Ok(h) => h,
            Err(e) => {
                self.work_error(work_epoch, &e.to_string());
                return;
            }
        };
        let (command_tx, command_rx) = tokio::sync::mpsc::channel(16);
        let store = self.0.store.clone();
        let this = self.clone();
        let expected = fence.clone();
        let observe = Arc::new(move |work, notice| this.notice(&expected, epoch, work, notice));
        let port = Port {
            tx,
            fence: fence.clone(),
            epoch: work_epoch,
        };
        let runner = host.clone();
        let this = self.clone();
        let expected = fence.clone();
        let first = authority.clone();
        let live_authority = Arc::new(RwLock::new(authority.clone()));
        let actor_authority = live_authority.clone();
        let connections = Arc::new(Mutex::new(Vec::new()));
        let actor_connections = connections.clone();
        let work = self.0.work.clone();
        let actor_work = work.clone();
        let roster = self.0.roster.clone();
        tauri::async_runtime::spawn(async move {
            let result = runner
                .execute(move |context| async move {
                    Actor::new(context, store, first, port, observe)?
                        .roster(roster)
                        .live_authority(actor_authority, actor_connections)
                        .work_control(actor_work)
                        .run(command_rx)
                        .await
                })
                .await;
            if let Err(e) = result {
                if e != transport::Error::Cancelled {
                    this.notice(
                        &expected,
                        epoch,
                        work_epoch,
                        Notice::Error(String::new(), e),
                    );
                }
            }
        });
        let _ = command_tx.try_send(coordinator::Command::Members(
            authority, signals, work_epoch,
        ));
        let watch = self.change_watcher(work);
        let mut slot = self.0.network.lock().unwrap();
        if epoch != self.0.network_epoch.load(Ordering::Acquire)
            || work_epoch != self.0.work.epoch()
            || slot.is_some()
        {
            host.stop();
            self.0.retired.lock().unwrap().push(host);
            return;
        }
        *slot = Some(Network {
            fence,
            host,
            tx: command_tx,
            _watch: watch,
            authority: live_authority,
            connections,
        });
    }
    fn change_watcher(
        &self,
        changed: Arc<coordinator::Work>,
    ) -> Option<notify::RecommendedWatcher> {
        use notify::Watcher;
        let result = (|| -> Result<_, String> {
            self.0.store.state.initialize_changes()?;
            let dir = self
                .0
                .store
                .state
                .changes_dir()
                .canonicalize()
                .map_err(|_| "change_watcher_failed")?;
            let marker = dir.join("content");
            let mut watcher =
                notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                    if event.is_ok_and(|e| {
                        !matches!(e.kind, notify::EventKind::Access(_))
                            && (e.need_rescan() || e.paths.iter().any(|p| p == &marker))
                    }) {
                        changed.changed();
                    }
                })
                .map_err(|_| "change_watcher_failed")?;
            watcher
                .watch(&dir, notify::RecursiveMode::NonRecursive)
                .map_err(|_| "change_watcher_failed")?;
            Ok(watcher)
        })();
        match result {
            Ok(w) => Some(w),
            Err(_) => {
                crate::kota_debug_log(
                    "[bbs-sync] change_watcher_failed; metadata fallback remains active",
                );
                None
            }
        }
    }
    fn notice(&self, fence: &GroupFence, epoch: u64, work: u64, notice: Notice) {
        let mut progress = false;
        {
            let mut v = self.0.view.lock().unwrap();
            // Validate while holding the same lock Cancel uses for advancing
            // the work epoch and publishing idle. A pre-lock check can race.
            if self.0.network_epoch.load(Ordering::Acquire) != epoch
                || self.0.work.epoch() != work
                || v.fence.as_ref() != Some(&(fence.group_id.clone(), fence.membership_id.clone()))
            {
                return;
            }
            match notice {
                Notice::Connecting(id) => {
                    v.connection_state(&id, "connecting");
                    v.status.sync.phase = public::Phase::Connecting;
                    v.status.sync.completed = None;
                    v.status.sync.total = None;
                }
                Notice::Connected(id) => v.connection_state(&id, "connected"),
                Notice::Disconnected(id) => v.connection_state(&id, "disconnected"),
                Notice::Diagnostic(id, d) => {
                    if v.status.group.members.iter().any(|m| m.id == id) {
                        v.diagnostics.connections.retain(|old| old.device_id != id);
                        if v.diagnostics.connections.len() < 32 {
                            v.diagnostics.connections.push(d);
                        }
                    }
                }
                Notice::Error(_, transport::Error::Busy) => return,
                Notice::Error(id, e) => {
                    v.manual_pending = false;
                    v.connection_state(&id, "failed");
                    v.status.sync.phase = public::Phase::Failed;
                    v.status.sync.error = Some(public::display_error(&e.to_string()));
                    v.diagnostics.last_error = Some(e.to_string());
                }
                Notice::Exchange(event) => match event {
                    exchange::Event::Changed(_) => return,
                    exchange::Event::Started(_) => {
                        v.manual_pending = false;
                        v.status.sync.phase = public::Phase::Syncing;
                        v.status.sync.error = None;
                        v.status.sync.completed = None;
                        v.status.sync.total = None;
                    }
                    exchange::Event::Progress(_, done, total) => {
                        v.manual_pending = false;
                        v.status.sync.phase = public::Phase::Syncing;
                        v.status.sync.completed = Some(done);
                        v.status.sync.total = Some(total);
                        progress = true;
                    }
                    exchange::Event::Finished(_, result) => {
                        v.manual_pending = false;
                        let okay = result.failures.is_empty()
                            && result.omitted == 0
                            && !result.more
                            && result.completed == result.total;
                        let failed = result.total > 0 && result.completed == 0 && !result.more;
                        v.status.sync.phase = if okay {
                            public::Phase::Idle
                        } else if failed {
                            public::Phase::Failed
                        } else {
                            public::Phase::Partial
                        };
                        v.status.sync.completed = Some(result.completed);
                        v.status.sync.total = Some(result.total);
                        v.status.sync.error = if result.failures.is_empty() && !failed {
                            None
                        } else {
                            Some(public::display_error("sync_item_failed"))
                        };
                        v.diagnostics.failures = result.failures;
                        v.diagnostics.omitted_failures = result.omitted;
                        if okay {
                            v.status.sync.last_successful_at =
                                Some(chrono::Utc::now().to_rfc3339());
                        }
                    }
                    exchange::Event::Failed(_, transport::Error::Cancelled) => {}
                    exchange::Event::Failed(id, e) => {
                        v.manual_pending = false;
                        v.connection_state(&id, "failed");
                        v.status.sync.phase = public::Phase::Failed;
                        v.status.sync.error = Some(public::display_error(&e.to_string()));
                        v.diagnostics.last_error = Some(e.to_string());
                    }
                },
            }
        }
        self.emit(progress);
    }
}

#[cfg(test)]
mod tests;
