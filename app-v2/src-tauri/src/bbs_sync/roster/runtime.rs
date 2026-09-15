//! Independent local directory cache. The async task only handles invalidation
//! and publication; all collection, hashing and disk cache work shares FileIo.
use super::{
    context, images::Images, public, wire, AgentView, Avatar, Context, DeviceView, DirectoryView,
    Member, PeerRoster, Project, ProjectView, PublicAvatar,
};
use crate::{
    bbs::sync::ContentStore,
    bbs_sync::{
        control,
        transport::{Cancellation, Error, FileIo, Limits, Resource},
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        Arc, Mutex, OnceLock, RwLock, Weak,
    },
};
use tokio::sync::Notify;

const LOCAL: u8 = 1;
const PUBLIC: u8 = 2;
const PEERS: u8 = 4;
static ACTIVE: OnceLock<Mutex<Option<Weak<Runtime>>>> = OnceLock::new();
pub(crate) fn active() -> bool {
    ACTIVE.get().is_some_and(|s| {
        s.lock()
            .is_ok_and(|v| v.as_ref().is_some_and(|w| w.strong_count() > 0))
    })
}
/// Central config/lifecycle hooks call this, never a UI button or IPC read.
pub(crate) fn local_changed() {
    if let Some(runtime) = ACTIVE
        .get()
        .and_then(|s| s.lock().ok()?.as_ref()?.upgrade())
    {
        runtime.mark(LOCAL);
    }
}

struct Published {
    revision: u64,
    ctx: Option<Context>,
    local: Option<Arc<wire::Local>>,
    peers: BTreeMap<String, super::reference::Peer>,
    page: Option<(u64, Arc<public::Snapshot>)>,
}
#[derive(Default)]
struct Material {
    scope: Option<(String, String)>,
    local: Option<Arc<wire::Local>>,
    peers: BTreeMap<String, super::reference::Peer>,
    images: Images,
}
pub(crate) struct Runtime {
    store: ContentStore,
    files: OnceLock<Result<(FileIo, Limits), Error>>,
    published: RwLock<Published>,
    material: Arc<Mutex<Material>>,
    dirty: AtomicU8,
    pending_peers: Mutex<BTreeSet<String>>,
    ready: Notify,
    started: AtomicBool,
    initialized: AtomicBool,
    init_ready: Notify,
    stop: Cancellation,
    emit: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    changed: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}
impl Runtime {
    pub(crate) fn new(store: ContentStore) -> Arc<Self> {
        Arc::new(Self {
            store,
            files: OnceLock::new(),
            published: RwLock::new(Published {
                revision: 0,
                ctx: None,
                local: None,
                peers: BTreeMap::new(),
                page: None,
            }),
            material: Arc::new(Mutex::new(Material::default())),
            dirty: AtomicU8::new(0),
            pending_peers: Mutex::new(BTreeSet::new()),
            ready: Notify::new(),
            started: AtomicBool::new(false),
            initialized: AtomicBool::new(false),
            init_ready: Notify::new(),
            stop: Cancellation::default(),
            emit: Mutex::new(None),
            changed: Mutex::new(None),
        })
    }
    /// Background callers only. Constructing Runtime and reading pages are inert.
    pub(crate) fn files(&self) -> Result<(FileIo, Limits), Error> {
        self.files
            .get_or_init(|| {
                let limits = Limits::default();
                FileIo::start(limits.clone()).map(|io| (io, limits))
            })
            .clone()
    }
    pub(crate) fn start(
        self: &Arc<Self>,
        emit: Arc<dyn Fn() + Send + Sync>,
        changed: Arc<dyn Fn() + Send + Sync>,
    ) {
        *self.emit.lock().unwrap() = Some(emit);
        *self.changed.lock().unwrap() = Some(changed);
        if self.started.swap(true, Ordering::AcqRel) {
            return;
        }
        #[cfg(not(test))]
        {
            *ACTIVE.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(Arc::downgrade(self));
        }
        let this = self.clone();
        tauri::async_runtime::spawn(async move {
            this.run().await;
        });
    }
    fn mark(&self, kind: u8) {
        self.dirty.fetch_or(kind, Ordering::AcqRel);
        self.ready.notify_one();
    }
    pub(crate) fn is_started(&self) -> bool {
        self.started.load(Ordering::Acquire)
    }
    pub(crate) async fn wait_initialized(&self) {
        if !self.started.load(Ordering::Acquire) {
            return;
        }
        loop {
            let ready = self.init_ready.notified();
            if self.initialized.load(Ordering::Acquire) {
                return;
            }
            tokio::select! {_=self.stop.cancelled()=>return,_=ready=>{}}
        }
    }
    fn initialized(&self) {
        self.initialized.store(true, Ordering::Release);
        self.init_ready.notify_waiters();
    }
    pub(crate) fn member_projection(
        &self,
        device_id: String,
        name: String,
        current: Option<control::Membership>,
        members: &[control::Member],
    ) {
        let ctx = Context {
            device_id: Some(device_id),
            device_name: name,
            membership: current,
            members: members
                .iter()
                .map(|m| Member {
                    device_id: m.device_id.clone(),
                    membership_id: m.membership_id.clone(),
                    name: m.name.clone(),
                    online: m.online,
                })
                .collect(),
        };
        self.update_context(ctx);
    }
    fn update_context(&self, ctx: Context) {
        let mut published = self.published.write().unwrap();
        if published.ctx.as_ref() == Some(&ctx) {
            return;
        }
        published.revision = published.revision.wrapping_add(1);
        if published.ctx.as_ref().and_then(|c| c.membership.as_ref()) != ctx.membership.as_ref() {
            published.peers.clear();
        }
        published.peers.retain(|id, r| {
            ctx.membership
                .as_ref()
                .is_some_and(|m| m.group_id == r.group_id)
                && ctx
                    .members
                    .iter()
                    .any(|m| m.device_id == *id && m.membership_id == r.membership_id)
        });
        published.ctx = Some(ctx);
        drop(published);
        self.mark(PUBLIC | PEERS);
    }
    pub(crate) fn member_projection_failed(&self) {
        let mut p = self.published.write().unwrap();
        p.revision = p.revision.wrapping_add(1);
        p.ctx = None;
        p.page = None;
        p.peers.clear();
        drop(p);
        if let Some(emit) = self.emit.lock().unwrap().clone() {
            emit();
        }
    }
    pub(crate) fn peer_changed(&self, peer: &str) {
        let mut p = self.published.write().unwrap();
        if !p
            .ctx
            .as_ref()
            .is_some_and(|c| c.members.iter().any(|m| m.device_id == peer))
        {
            return;
        }
        p.revision = p.revision.wrapping_add(1);
        drop(p);
        self.pending_peers.lock().unwrap().insert(peer.into());
        self.mark(PUBLIC | PEERS);
    }
    pub(crate) fn avatar_changed(&self, sha: &str) {
        let p = self.published.read().unwrap();
        let referenced = p
            .local
            .as_ref()
            .is_some_and(|r| references(&r.projects, sha).is_some())
            || p.peers
                .values()
                .any(|r| references(&r.projects, sha).is_some());
        drop(p);
        if referenced {
            self.mark(PUBLIC);
        }
    }
    pub(crate) fn image_available(
        &self,
        avatar: &Avatar,
        io: &mut crate::bbs_sync::transport::LocalIo,
    ) -> bool {
        self.material
            .lock()
            .unwrap()
            .images
            .available(&self.store, avatar, io)
    }
    #[cfg(test)]
    pub(crate) fn test_source(store: ContentStore, projects: Vec<Project>) -> Arc<Self> {
        let this = Self::new(store);
        let mut p = this.published.write().unwrap();
        p.ctx = Some(context(&this.store.state).unwrap());
        p.local = Some(Arc::new(wire::Local::new(projects).unwrap()));
        drop(p);
        this
    }
    pub(crate) fn source(&self) -> Option<Arc<wire::Local>> {
        self.published.read().ok()?.local.clone()
    }
    pub(crate) fn page(&self, request: public::ReadRequest) -> Result<public::Page, public::Error> {
        let p = self
            .published
            .read()
            .map_err(|_| public::Error::unavailable())?;
        let Some((revision, page)) = &p.page else {
            return Err(public::Error::unavailable());
        };
        if *revision != p.revision {
            return Err(public::Error::changed());
        }
        page.page(request)
    }
    pub(crate) fn avatar_resource(
        &self,
        device: &str,
        sha: &str,
    ) -> Result<Resource, public::Error> {
        self.avatar_authorization(device, sha)
            .map(|(resource, _)| resource)
    }
    pub(crate) fn avatar_authorization(
        &self,
        device: &str,
        sha: &str,
    ) -> Result<(Resource, Option<super::reference::Peer>), public::Error> {
        super::valid_hash(sha).map_err(|_| public::Error::invalid())?;
        if device != "local" {
            super::valid_hash(device).map_err(|_| public::Error::invalid())?;
        }
        let p = self
            .published
            .read()
            .map_err(|_| public::Error::unavailable())?;
        if p.page
            .as_ref()
            .is_none_or(|(revision, _)| *revision != p.revision)
        {
            return Err(public::Error::unavailable());
        }
        let ctx = p.ctx.as_ref().ok_or_else(public::Error::unavailable)?;
        let local = device == "local" || ctx.device_id.as_deref() == Some(device);
        let resource = if local {
            p.local.as_ref().and_then(|r| references(&r.projects, sha))
        } else {
            if self.pending_peers.lock().unwrap().contains(device) {
                return Err(public::Error::unavailable());
            }
            p.peers
                .get(device)
                .filter(|r| {
                    ctx.membership
                        .as_ref()
                        .is_some_and(|m| m.group_id == r.group_id)
                        && ctx
                            .members
                            .iter()
                            .any(|m| m.device_id == device && m.membership_id == r.membership_id)
                })
                .and_then(|r| references(&r.projects, sha))
        };
        let resource = resource.ok_or_else(public::Error::unavailable)?;
        let peer = if local {
            None
        } else {
            Some(
                p.peers
                    .get(device)
                    .ok_or_else(public::Error::unavailable)?
                    .clone(),
            )
        };
        Ok((resource, peer))
    }
    /// Only the holder of ControlLease calls this on startup. A second App
    /// may collect its local directory, but must not remove the owner's stages.
    pub(crate) async fn recover_staging(&self) -> Result<(), Error> {
        let (io, _) = self.files()?;
        let store = self.store.clone();
        io.run_when_available(&self.stop, move |io| -> anyhow::Result<()> {
            let entries = match std::fs::read_dir(store.state.root.join("rosters")) {
                Ok(e) => e,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(e.into()),
            };
            for item in entries {
                io.check()?;
                let item = item?;
                let name = item.file_name();
                let name = name.to_string_lossy();
                if name.starts_with(".roster-")
                    && name.ends_with(".partial")
                    && item.file_type()?.is_file()
                {
                    io.charge(64)?;
                    std::fs::remove_file(item.path())?;
                }
            }
            Ok(())
        })
        .await?
        .map_err(|_| Error::Io)
    }
    pub(crate) fn stop(&self) {
        self.stop.cancel();
        self.ready.notify_one();
        if let Some(Ok((io, _))) = self.files.get() {
            let io = io.clone();
            tauri::async_runtime::spawn(async move {
                io.stop_and_wait().await;
            });
        }
    }
    async fn run(self: Arc<Self>) {
        let store = self.store.clone();
        let this = self.clone();
        let initial = tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<_> {
            this.files()?;
            let ctx = context(&store.state)?;
            Ok(ctx)
        })
        .await;
        match initial {
            Ok(Ok(ctx)) => {
                let mut p = self.published.write().unwrap();
                if p.ctx.is_none() {
                    p.ctx = Some(ctx);
                    p.revision += 1;
                }
            }
            _ => {
                self.initialized();
                crate::kota_debug_log("[bbs-roster] initialization_failed");
                return;
            }
        }
        self.initialized();
        self.mark(LOCAL | PEERS | PUBLIC);
        loop {
            let notified = self.ready.notified();
            let mut flags = self.dirty.swap(0, Ordering::AcqRel);
            if self.stop.is_cancelled() {
                return;
            }
            if flags == 0 {
                tokio::select! {_=self.stop.cancelled()=>return,_=notified=>{}};
                continue;
            }
            let (revision, ctx) = {
                let p = self.published.read().unwrap();
                (p.revision, p.ctx.clone())
            };
            let Some(ctx) = ctx else {
                continue;
            };
            let peers = std::mem::take(&mut *self.pending_peers.lock().unwrap());
            if !peers.is_empty() {
                flags |= PEERS;
            }
            let retry_peers = peers.clone();
            let Ok((file_io, _)) = self.files() else {
                return;
            };
            let store = self.store.clone();
            let material = self.material.clone();
            let build = file_io
                .run_when_available(&self.stop, move |io| -> anyhow::Result<_> {
                    let mut m = material.lock().unwrap();
                    let scope = ctx
                        .membership
                        .as_ref()
                        .map(|g| (g.group_id.clone(), g.membership_id.clone()));
                    if m.scope != scope {
                        m.peers.clear();
                        m.scope = scope;
                    }
                    if flags & LOCAL != 0 || m.local.is_none() {
                        let projects = m.images.collect(&store, io)?;
                        let next = Arc::new(wire::Local::new(projects)?);
                        if !m
                            .local
                            .as_ref()
                            .is_some_and(|old| old.version == next.version)
                        {
                            m.local = Some(next);
                        }
                    }
                    m.peers.retain(|id, r| {
                        ctx.membership
                            .as_ref()
                            .is_some_and(|g| g.group_id == r.group_id)
                            && ctx
                                .members
                                .iter()
                                .any(|a| a.device_id == *id && a.membership_id == r.membership_id)
                    });
                    if flags & PEERS != 0 {
                        for member in &ctx.members {
                            if ctx.device_id.as_deref() == Some(&member.device_id) {
                                continue;
                            }
                            if !m.peers.contains_key(&member.device_id)
                                || peers.contains(&member.device_id)
                            {
                                match super::reference::Peer::load_on_file_thread(
                                    &store.state,
                                    &member.device_id,
                                    io,
                                ) {
                                    Ok(Some(r)) => {
                                        m.peers.insert(member.device_id.clone(), r);
                                    }
                                    Ok(None) => {
                                        m.peers.remove(&member.device_id);
                                    }
                                    Err(_) => {
                                        crate::kota_debug_log("[bbs-roster] invalid_peer_cache");
                                    }
                                }
                            }
                        }
                    }
                    let local = m.local.clone().unwrap();
                    let peers = m.peers.clone();
                    let view =
                        directory_view(&store, &ctx, &local.projects, &peers, &mut m.images, io);
                    let snapshot = Arc::new(public::Snapshot::new(view)?);
                    let paths = m.images.source_paths();
                    m.images.retain(
                        local
                            .projects
                            .iter()
                            .chain(peers.values().flat_map(|r| r.projects.iter()))
                            .flat_map(|p| p.agents.iter().map(|a| a.avatar.clone())),
                    );
                    io.check()?;
                    Ok((local, peers, snapshot, paths))
                })
                .await;
            match build {
                Ok(Ok((local, peers, page, paths))) => {
                    #[cfg(not(test))]
                    crate::adapter_sync::register_roster_images(paths);
                    #[cfg(test)]
                    let _ = paths;
                    let mut p = self.published.write().unwrap();
                    let source_changed =
                        p.local.as_ref().is_none_or(|r| r.version != local.version);
                    p.local = Some(local);
                    if p.revision != revision {
                        drop(p);
                        self.pending_peers.lock().unwrap().extend(retry_peers);
                        self.mark(PUBLIC | PEERS);
                        if source_changed {
                            if let Some(cb) = self.changed.lock().unwrap().clone() {
                                cb();
                            }
                        }
                        continue;
                    }
                    let changed = p
                        .page
                        .as_ref()
                        .is_none_or(|(_, old)| old.version != page.version);
                    p.page = Some((revision, page));
                    p.peers = peers;
                    drop(p);
                    if source_changed {
                        if let Some(cb) = self.changed.lock().unwrap().clone() {
                            cb();
                        }
                    }
                    if changed {
                        if let Some(cb) = self.emit.lock().unwrap().clone() {
                            cb();
                        }
                    }
                }
                Err(Error::Busy) => {
                    self.pending_peers.lock().unwrap().extend(retry_peers);
                    tokio::select! {_=self.stop.cancelled()=>return,_=tokio::time::sleep(std::time::Duration::from_millis(100))=>{}}
                    self.mark(flags);
                }
                Err(Error::Cancelled | Error::Closed) => return,
                _ => {
                    // Retry these cache changes on the next real invalidation;
                    // an unrelated malformed local config must not lose them.
                    self.pending_peers.lock().unwrap().extend(retry_peers);
                    crate::kota_debug_log(
                        "[bbs-roster] rebuild_failed; previous_complete_view_retained",
                    );
                }
            }
        }
    }
}
pub(crate) fn references(projects: &[Project], sha: &str) -> Option<Resource> {
    projects
        .iter()
        .flat_map(|p| &p.agents)
        .filter_map(|a| a.avatar.resource())
        .find(|r| r.sha256 == sha)
}
fn directory_view(
    store: &ContentStore,
    ctx: &Context,
    local: &[Project],
    peers: &BTreeMap<String, super::reference::Peer>,
    images: &mut Images,
    io: &mut crate::bbs_sync::transport::LocalIo,
) -> DirectoryView {
    let mut available = BTreeMap::new();
    let mut project_views = |projects: &[Project], device: &str| {
        projects
            .iter()
            .map(|p| ProjectView {
                project_id: p.project_id.clone(),
                name: p.name.clone(),
                agents: p
                    .agents
                    .iter()
                    .map(|a| {
                        let ready = if let Avatar::Image { .. } = &a.avatar {
                            *available
                                .entry(serde_json::to_string(&a.avatar).unwrap())
                                .or_insert_with(|| images.available(store, &a.avatar, io))
                        } else {
                            false
                        };
                        AgentView {
                            agent_id: a.agent_id.clone(),
                            name: a.name.clone(),
                            target_ref: format!("{device}/{}/{}", p.project_id, a.agent_id),
                            avatar: PublicAvatar::new(&a.avatar, ready),
                        }
                    })
                    .collect(),
            })
            .collect()
    };
    let mut devices = vec![DeviceView {
        device_id: ctx.device_id.clone(),
        name: ctx.device_name.clone(),
        local: true,
        online: true,
        roster_status: "synced",
        received_at: None,
        projects: Some(project_views(
            local,
            ctx.device_id.as_deref().unwrap_or("local"),
        )),
    }];
    for member in &ctx.members {
        if ctx.device_id.as_deref() == Some(&member.device_id) {
            continue;
        }
        let peer = peers.get(&member.device_id);
        devices.push(DeviceView {
            device_id: Some(member.device_id.clone()),
            name: member.name.clone(),
            local: false,
            online: member.online,
            roster_status: if peer.is_some() {
                "synced"
            } else {
                "not_synced"
            },
            received_at: peer.map(|r| r.received_at.clone()),
            projects: peer.map(|r| project_views(&r.projects, &member.device_id)),
        });
    }
    DirectoryView { devices }
}

#[cfg(test)]
mod tests;
