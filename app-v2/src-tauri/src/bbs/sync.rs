//! BBS-owned content paths and mutations. No thread is started here. The sync
//! coordinator executes filesystem work through the transport's account file
//! worker; local CLI/App publication uses the existing BBS short flock.
use super::*;
use crate::bbs_sync::{
    self, reconcile,
    transport::{Cancellation, LocalIo, Resource, ResourceKind, VerifiedFile},
    AttachmentRef, AttachmentState, AvatarSidecar, FileStamp, LedgerEntry, Manifest, ManifestPhase,
    PostVersion, StateStore, SyncState, ThreadRecord, ThreadRecordItem, Tombstone, UnavailablePost,
    UnavailableReason,
};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

mod deletions;
mod inventory;
mod mutations;
mod unavailable;
pub use unavailable::UnavailableView;
pub(super) use unavailable::PLACEHOLDER_TEXT;
#[cfg(test)]
mod tests;

#[derive(Clone)]
pub(crate) struct ContentStore {
    root: PathBuf,
    pub(crate) state: StateStore,
    account: PathBuf,
    receiving_peer: Option<String>,
}
impl ContentStore {
    pub(crate) fn at(root: PathBuf, account: PathBuf) -> Self {
        Self {
            state: StateStore::at(&account).with_content_root(root.clone()),
            root,
            account,
            receiving_peer: None,
        }
    }
    pub(crate) fn account() -> Self {
        Self::at(account_root(), crate::kota_home_dir())
    }
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
    pub(crate) fn account_dir(&self) -> &Path {
        &self.account
    }
    /// Private delivery provenance from an authenticated Link. This is the
    /// forwarding device, not an attestation of the Markdown author's device.
    pub(crate) fn receiving_from(&self, peer: &str) -> Result<Self> {
        bbs_sync::valid_hash(peer).map_err(anyhow::Error::msg)?;
        let mut scoped = self.clone();
        scoped.receiving_peer = Some(peer.into());
        Ok(scoped)
    }
    pub(super) fn receiving_peer(&self) -> Option<&str> { self.receiving_peer.as_deref() }
    pub(crate) fn prepare_layout(&self) -> Result<PathBuf> {
        ensure_layout_at(&self.root)?;
        self.directory(Path::new(".sync-staging"))
    }
    /// Every relative component is constructed from a validated logical ID or
    /// a fixed BBS directory name. Existing symlinks are never followed.
    pub(super) fn managed(&self, relative: &Path, parents: bool) -> Result<PathBuf> {
        let mut path = self.root.canonicalize()?;
        let components = relative.components().collect::<Vec<_>>();
        for (index, component) in components.iter().enumerate() {
            let std::path::Component::Normal(name) = component else {
                bail!("invalid_sync_path")
            };
            path.push(name);
            let parent = index + 1 < components.len();
            match fs::symlink_metadata(&path) {
                Ok(meta) if meta.file_type().is_symlink() || (parent && !meta.is_dir()) => {
                    bail!("unsafe_sync_path")
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    if parent && parents {
                        fs::DirBuilder::new().mode(0o700).create(&path)?;
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(path)
    }
    fn directory(&self, relative: &Path) -> Result<PathBuf> {
        let path = self.managed(relative, true)?;
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            Ok(_) => bail!("unsafe_sync_directory"),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                fs::DirBuilder::new().mode(0o700).create(&path)?
            }
            Err(e) => return Err(e.into()),
        }
        Ok(path)
    }
    fn thread_relative(thread: &str) -> Result<PathBuf> {
        bbs_sync::safe_id(thread).map_err(anyhow::Error::msg)?;
        Ok(Path::new("threads").join(thread))
    }
    pub(super) fn post_relative(thread: &str, post: &str, extra: Option<&str>) -> Result<PathBuf> {
        bbs_sync::safe_id(post).map_err(anyhow::Error::msg)?;
        let thread = Self::thread_relative(thread)?;
        if let Some(version) = extra {
            bbs_sync::valid_hash(version).map_err(anyhow::Error::msg)?;
            Ok(thread
                .join("versions")
                .join(post)
                .join(version)
                .join("post.md"))
        } else {
            Ok(thread.join("posts").join(format!("{post}.md")))
        }
    }
    fn sidecar_relative(thread: &str, post: &str, version: &str) -> Result<PathBuf> {
        bbs_sync::safe_id(post).map_err(anyhow::Error::msg)?;
        bbs_sync::valid_hash(version).map_err(anyhow::Error::msg)?;
        Ok(Self::thread_relative(thread)?
            .join("sidecars")
            .join(post)
            .join(format!("{version}.json")))
    }
    pub(super) fn state_locked(&self, _lock: &BbsWriteLock) -> Result<SyncState> {
        self.state
            .load_state()
            .map_err(anyhow::Error::msg)
            .map(Option::unwrap_or_default)
    }
    pub(super) fn save_locked(&self, _lock: &BbsWriteLock, state: &SyncState) -> Result<()> {
        self.state.save_state(state).map_err(anyhow::Error::msg)
    }
}

/// A round is fenced by both the group and this device's membership incarnation.
/// It is captured by the authenticated coordinator; every commit re-reads the
/// durable membership under the same short lock as local publication.
#[derive(Clone, Debug)]
pub(crate) struct GroupFence {
    pub(crate) group_id: String,
    pub(crate) membership_id: String,
}
impl ContentStore {
    fn check_fence(&self, _lock: &BbsWriteLock, fence: &GroupFence) -> Result<()> {
        let member = bbs_sync::control::read_membership(&self.state)
            .map_err(anyhow::Error::msg)?
            .ok_or_else(|| anyhow!("group_context_changed"))?;
        if member.group_id != fence.group_id || member.membership_id != fence.membership_id {
            bail!("group_context_changed");
        }
        Ok(())
    }
    pub(super) fn before_local_publish(
        &self,
        lock: &BbsWriteLock,
        meta: &BbsPostMeta,
        raw: &[u8],
        new_root: bool,
    ) -> Result<Vec<String>> {
        let mut state = self.state_locked(lock)?;
        let version = bbs_sync::raw_sha256(raw);
        if reconcile::suppressed(&state, &meta.thread_id, &meta.post_id, &version) {
            bail!("BBS target was deleted");
        }
        if new_root {
            if let Some(member) =
                bbs_sync::control::read_membership(&self.state).map_err(anyhow::Error::msg)?
            {
                state
                    .groups
                    .entry(member.group_id)
                    .or_default()
                    .shared_threads
                    .insert(meta.thread_id.clone());
            }
        }
        // Sync eligibility must not become a new local publication limit.
        // Long legacy IDs or oversized bodies are still published locally;
        // the outbound inventory reports that individual item as ineligible.
        if raw.len() as u64 > reconcile::MAX_BODY_BYTES || bbs_sync::safe_id(&meta.post_id).is_err()
        {
            self.save_locked(lock, &state)?;
            return super::notify::prepare(self, lock, meta, raw, &version, true, &state);
        }
        // Reserve the minted identity and sharing intent before publication.
        // Failure before post.md leaves no visible post; inventory fills the
        // rebuildable file proofs later. This is not timestamp-based backfill.
        let descriptor = PostVersion {
            thread_id: meta.thread_id.clone(),
            post_id: meta.post_id.clone(),
            version_id: version.clone(),
            size_bytes: raw.len() as u64,
            kind: meta.kind.clone(),
            attachments: attachment_refs(meta)?,
            avatar: None,
        };
        let row = LedgerEntry {
            thread_id: meta.thread_id.clone(),
            post_id: meta.post_id.clone(),
            version_id: version.clone(),
            descriptor: Some(descriptor),
            updated_at: now_iso(),
            ..Default::default()
        };
        state
            .ledger
            .insert(row.key().map_err(anyhow::Error::msg)?, row);
        self.save_locked(lock, &state)?;
        super::notify::prepare(self, lock, meta, raw, &version, true, &state)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct OriginSnapshot {
    schema_version: u32,
    // Local posts keep their existing live UI avatar. Both origins forward the
    // same immutable captured sidecar; received posts never look up a local hero.
    origin: String,
    avatar: AvatarSidecar,
}
impl OriginSnapshot {
    fn validate(&self) -> Result<()> {
        if self.schema_version != 1 || !matches!(self.origin.as_str(), "local" | "received") {
            bail!("invalid_origin_snapshot");
        }
        self.avatar.validate().map_err(anyhow::Error::msg)
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum AvatarView {
    Builtin {
        id: String,
    },
    Image {
        sha256: String,
        ext: String,
        #[serde(rename = "localPath")]
        local_path: Option<String>,
        available: bool,
    },
    None,
}
pub(crate) fn stamp(meta: &fs::Metadata) -> FileStamp {
    FileStamp::of(meta)
}
fn current_stamp(path: &Path) -> Result<Option<FileStamp>> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => Ok(Some(stamp(&meta))),
        Ok(_) => bail!("unsafe_sync_file"),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
struct RawFile {
    bytes: Vec<u8>,
    sha256: String,
    stamp: FileStamp,
}
fn read_bounded(path: &Path, max: u64, io: &mut LocalIo) -> Result<RawFile> {
    io.check()?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > max {
        bail!("sync_source_too_large_or_not_regular");
    }
    let before = stamp(&metadata);
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    let mut hash = Sha256::new();
    let mut buf = io.buffer()?;
    loop {
        io.check()?;
        let n = file.read(&mut buf.bytes)?;
        if n == 0 {
            break;
        }
        io.charge(n)?;
        if bytes.len() as u64 + n as u64 > max {
            bail!("sync_source_grew");
        }
        hash.update(&buf.bytes[..n]);
        bytes.extend_from_slice(&buf.bytes[..n]);
    }
    if stamp(&file.metadata()?) != before
        || current_stamp(path)?.as_ref() != Some(&before)
        || bytes.len() as u64 != before.len
    {
        bail!("sync_source_changed");
    }
    Ok(RawFile {
        bytes,
        sha256: format!("{:x}", hash.finalize()),
        stamp: before,
    })
}
fn write_small(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| anyhow!("invalid_sync_path"))?;
    let temp = parent.join(format!(".content-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)?;
        File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(Into::into)
}
fn parse_post(raw: &[u8], thread: &str, post: &str) -> Result<BbsPostMeta> {
    if raw.len() as u64 > reconcile::MAX_BODY_BYTES {
        bail!("sync_post_too_large_or_empty");
    }
    let text = std::str::from_utf8(raw).map_err(|_| anyhow!("invalid_sync_post_utf8"))?;
    let rest = text
        .strip_prefix("---\n")
        .ok_or_else(|| anyhow!("invalid_sync_frontmatter"))?;
    let (yaml, _) = rest
        .split_once("\n---\n")
        .ok_or_else(|| anyhow!("invalid_sync_frontmatter"))?;
    if yaml.len() > reconcile::MAX_FRONTMATTER_BYTES {
        bail!("sync_frontmatter_too_large");
    }
    let meta: BbsPostMeta =
        serde_yaml::from_str(yaml).map_err(|_| anyhow!("invalid_sync_post_metadata"))?;
    validate_post_meta(&meta, thread, post)?;
    Ok(meta)
}
fn validate_post_meta(meta: &BbsPostMeta, thread: &str, post: &str) -> Result<()> {
    super::mentions::validate(&meta.mentions)?;
    if meta.schema != POST_SCHEMA
        || meta.thread_id != thread
        || meta.post_id != post
        || !matches!(meta.kind.as_str(), "topic" | "reply")
    {
        bail!("invalid_post_relationship");
    }
    for id in [
        &meta.thread_id,
        &meta.post_id,
        &meta.project_id,
        &meta.agent_id,
    ] {
        bbs_sync::safe_id(id).map_err(anyhow::Error::msg)?;
    }
    reconcile::timestamp(&meta.created_at).map_err(anyhow::Error::msg)?;
    attachment_refs(&meta)?;
    Ok(())
}
fn attachment_refs(meta: &BbsPostMeta) -> Result<Vec<AttachmentRef>> {
    if meta.attachments.len() > MAX_ATTACHMENTS {
        bail!("too_many_sync_attachments");
    }
    let mut ids = BTreeSet::new();
    let mut total = 0u64;
    let mut result = Vec::new();
    for a in &meta.attachments {
        bbs_sync::safe_id(&a.id).map_err(anyhow::Error::msg)?;
        bbs_sync::valid_hash(&a.sha256).map_err(anyhow::Error::msg)?;
        let prefix = format!("attachments/{}/{}", meta.post_id, a.id);
        let ext = if a.path == prefix {
            ""
        } else {
            a.path
                .strip_prefix(&(prefix + "."))
                .ok_or_else(|| anyhow!("invalid_sync_attachment_path"))?
        };
        if !ids.insert(&a.id)
            || !reconcile::valid_extension(ext)
            || a.size_bytes > MAX_ATTACHMENT_BYTES
        {
            bail!("invalid_sync_attachment");
        }
        total = total
            .checked_add(a.size_bytes)
            .ok_or_else(|| anyhow!("sync_attachment_total_overflow"))?;
        if total > MAX_POST_ATTACHMENT_BYTES {
            bail!("sync_attachments_too_large");
        }
        result.push(AttachmentRef {
            id: a.id.clone(),
            sha256: a.sha256.clone(),
            size_bytes: a.size_bytes,
            ext: ext.into(),
            available: false,
        });
    }
    Ok(result)
}
fn record_from_meta(meta: &BbsThreadMeta) -> Result<ThreadRecordItem> {
    reconcile::thread_item(ThreadRecord {
        schema: meta.schema.clone(),
        thread_id: meta.thread_id.clone(),
        status: meta.status.clone(),
        visibility: meta.visibility.clone(),
        project_tags: meta.project_tags.clone(),
        created_by_project: meta.created_by_project.clone(),
        created_by_agent: meta.created_by_agent.clone(),
        created_at: meta.created_at.clone(),
    })
    .map_err(anyhow::Error::msg)
}
fn meta_from_record(record: &ThreadRecord) -> BbsThreadMeta {
    BbsThreadMeta {
        schema: record.schema.clone(),
        thread_id: record.thread_id.clone(),
        status: record.status.clone(),
        visibility: record.visibility.clone(),
        project_tags: record.project_tags.clone(),
        created_by_project: record.created_by_project.clone(),
        created_by_agent: record.created_by_agent.clone(),
        created_at: record.created_at.clone(),
        updated_at: record.created_at.clone(),
        latest_post_id: String::new(),
    }
}
impl ContentStore {
    pub(super) fn attachment_views(
        &self,
        post: &LoadedPost,
        state: Option<&SyncState>,
    ) -> Vec<BbsAttachmentView> {
        let received = self
            .read_origin(&post.meta.thread_id, &post.meta.post_id, &post.version_id)
            .map(|o| o.is_some_and(|o| o.origin == "received"))
            .unwrap_or(true);
        let extra = (post.path.file_name().and_then(|s| s.to_str()) == Some("post.md"))
            .then(|| {
                post.path
                    .parent()
                    .and_then(|p| p.file_name())
                    .and_then(|s| s.to_str())
                    .map(str::to_owned)
            })
            .flatten();
        let location = inventory::PostLocation {
            path: post.path.clone(),
            post: post.meta.post_id.clone(),
            extra,
        };
        let Ok(refs) = attachment_refs(&post.meta) else {
            return vec![];
        };
        post.meta
            .attachments
            .iter()
            .zip(refs)
            .filter_map(|(entry, a)| {
                let path = self
                    .attachment_path(&post.meta.thread_id, &location, &a, false)
                    .ok()?;
                let observed = current_stamp(&path).ok().flatten();
                let available = if received {
                    state
                        .and_then(|s| {
                            s.ledger
                                .get(&format!("{}/{}", post.meta.post_id, post.version_id))
                        })
                        .and_then(|r| r.attachments.get(&a.id))
                        .is_some_and(|s| {
                            s.verified
                                && s.stamp.is_some()
                                && s.sha256 == a.sha256
                                && s.size_bytes == a.size_bytes
                                && s.stamp == observed
                        })
                } else {
                    observed.is_some_and(|m| m.len == a.size_bytes)
                };
                Some(BbsAttachmentView {
                    attachment: entry.clone(),
                    local_path: path.display().to_string(),
                    available,
                })
            })
            .collect()
    }
    fn read_origin(
        &self,
        thread: &str,
        post: &str,
        version: &str,
    ) -> Result<Option<OriginSnapshot>> {
        let path = self.managed(&Self::sidecar_relative(thread, post, version)?, false)?;
        let Some(meta) = current_stamp(&path)? else {
            return Ok(None);
        };
        if meta.len > 2048 {
            bail!("invalid_origin_snapshot");
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        let mut bytes = Vec::new();
        file.take(2049).read_to_end(&mut bytes)?;
        if bytes.len() > 2048 {
            bail!("invalid_origin_snapshot");
        }
        let value: OriginSnapshot =
            serde_json::from_slice(&bytes).map_err(|_| anyhow!("invalid_origin_snapshot"))?;
        value.validate()?;
        Ok(Some(value))
    }
    fn write_origin_locked(
        &self,
        _lock: &BbsWriteLock,
        thread: &str,
        post: &str,
        version: &str,
        origin: &OriginSnapshot,
    ) -> Result<OriginSnapshot> {
        origin.validate()?;
        if let Some(existing) = self.read_origin(thread, post, version)? {
            return Ok(existing);
        }
        let path = self.managed(&Self::sidecar_relative(thread, post, version)?, true)?;
        write_small(&path, &serde_json::to_vec(origin)?)?;
        Ok(origin.clone())
    }
    fn capture_avatar(
        &self,
        meta: &BbsPostMeta,
        io: &mut LocalIo,
    ) -> Result<(AvatarSidecar, Option<FileStamp>)> {
        let mut cache = BTreeMap::new();
        let Some(id) = post_agent_avatar(&self.root, meta, &mut cache) else {
            return Ok((AvatarSidecar::none(), None));
        };
        if bbs_sync::BUILTIN_AVATARS.contains(&id.as_str()) {
            return Ok((
                AvatarSidecar {
                    kind: "builtin".into(),
                    id: Some(id),
                    ..AvatarSidecar::none()
                },
                None,
            ));
        }
        if !id.starts_with("user:") {
            return Ok((AvatarSidecar::none(), None));
        }
        // Only the explicitly referenced registered avatar is read, never the
        // whole hero image pool or a peer-supplied pathname.
        let source_dir = self.account.join("avatars");
        let index = crate::read_user_avatar_index(&source_dir).map_err(anyhow::Error::msg)?;
        let Some(item) = index.iter().find(|item| item.id == id) else {
            return Ok((AvatarSidecar::none(), None));
        };
        self.capture_registered_avatar(item, io)
    }
    /// The roster and immutable post origin share this registered-image path.
    /// Its caller has resolved an existing local index entry, never a peer path.
    pub(crate) fn capture_registered_avatar(
        &self,
        item: &crate::StoredUserHeroAvatar,
        io: &mut LocalIo,
    ) -> Result<(AvatarSidecar, Option<FileStamp>)> {
        let source_dir = self.account.join("avatars");
        if !safe_component(&item.file_name) {
            bail!("invalid_origin_avatar_path");
        }
        let ext = match item.mime.as_str() {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/webp" => "webp",
            _ => bail!("invalid_origin_avatar_type"),
        };
        let raw = read_bounded(&source_dir.join(&item.file_name), 600_000, io)?;
        if raw.bytes.is_empty() {
            bail!("empty_origin_avatar");
        }
        let avatar = AvatarSidecar {
            kind: "image".into(),
            id: None,
            sha256: Some(raw.sha256.clone()),
            ext: Some(ext.into()),
            size_bytes: Some(raw.bytes.len() as u64),
        };
        let dir = self.directory(Path::new("avatars"))?;
        let destination = dir.join(format!("{}.{}", raw.sha256, ext));
        let valid = if destination.exists() {
            let stored = read_bounded(&destination, 600_000, io)?;
            stored.sha256 == raw.sha256 && stored.bytes.len() == raw.bytes.len()
        } else {
            false
        };
        if !valid {
            write_small(&destination, &raw.bytes)?;
        }
        Ok((avatar, current_stamp(&destination)?))
    }
    pub(crate) fn roster_avatar_path(&self, sha256: &str, ext: &str) -> Result<PathBuf> {
        bbs_sync::valid_hash(sha256).map_err(anyhow::Error::msg)?;
        if !matches!(ext, "png" | "jpg" | "webp") {
            bail!("invalid_roster_avatar");
        }
        self.managed(&Path::new("avatars").join(format!("{sha256}.{ext}")), false)
    }
    /// Reference authorization is performed by the roster caller, separately
    /// from post-ledger authority. This establishes a stamp only after hashing.
    pub(crate) fn verify_roster_avatar(
        &self,
        resource: &Resource,
        io: &mut LocalIo,
    ) -> Result<Option<FileStamp>> {
        let ResourceKind::Avatar { sha256, ext } = &resource.identity else {
            bail!("invalid_roster_avatar");
        };
        resource.validate()?;
        let path = self.roster_avatar_path(sha256, ext)?;
        if current_stamp(&path)?.is_none() {
            return Ok(None);
        }
        let raw = read_bounded(&path, 600_000, io)?;
        if raw.sha256 != resource.sha256 || raw.bytes.len() as u64 != resource.size_bytes {
            bail!("invalid_roster_avatar");
        }
        if current_stamp(&path)?.as_ref() != Some(&raw.stamp) {
            bail!("changed_roster_avatar");
        }
        Ok(Some(raw.stamp))
    }
    /// Used only after a device-scoped current-reference check by the new IPC.
    /// The existing read_verified_avatar remains ledger restricted.
    pub(crate) fn read_roster_avatar(&self, resource: &Resource) -> Result<String> {
        let ResourceKind::Avatar { sha256, ext } = &resource.identity else {
            bail!("invalid_roster_avatar");
        };
        resource.validate()?;
        let path = self.roster_avatar_path(sha256, ext)?;
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)?;
        let before = stamp(&file.metadata()?);
        if !file.metadata()?.is_file()
            || before.len != resource.size_bytes
            || before.len == 0
            || before.len > 600_000
        {
            bail!("invalid_roster_avatar");
        }
        let mut bytes = Vec::with_capacity(before.len as usize);
        Read::by_ref(&mut file)
            .take(600_001)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 != before.len
            || stamp(&file.metadata()?) != before
            || current_stamp(&path)?.as_ref() != Some(&before)
            || bbs_sync::raw_sha256(&bytes) != *sha256
        {
            bail!("changed_roster_avatar");
        }
        use base64::Engine as _;
        let mime = match ext.as_str() {
            "png" => "image/png",
            "jpg" => "image/jpeg",
            _ => "image/webp",
        };
        Ok(format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&bytes)
        ))
    }
    pub(crate) fn avatar_view(
        &self,
        thread: &str,
        post: &str,
        version: &str,
        state: Option<&SyncState>,
    ) -> Option<AvatarView> {
        let origin = match self.read_origin(thread, post, version) {
            Ok(None) => {
                // A local edit changes the raw hash before background inventory
                // writes its new sidecar. Retain a known received origin in
                // that interval instead of looking up the local hero table.
                let inherited = state.and_then(|s| {
                    s.ledger
                        .values()
                        .filter(|r| r.thread_id == thread && r.post_id == post)
                        .filter_map(|r| {
                            self.read_origin(thread, post, &r.version_id).ok().flatten()
                        })
                        .find(|o| o.origin == "received")
                });
                inherited?
            }
            Ok(Some(v)) => v,
            Err(_) => return Some(AvatarView::None),
        };
        if origin.origin == "local" {
            return None;
        }
        let avatar = origin.avatar;
        Some(match avatar.kind.as_str() {
            "builtin" => AvatarView::Builtin {
                id: avatar.id.unwrap(),
            },
            "image" => {
                let sha256 = avatar.sha256.unwrap();
                let ext = avatar.ext.unwrap();
                let path = self
                    .managed(&Path::new("avatars").join(format!("{sha256}.{ext}")), false)
                    .ok();
                let metadata = path.as_ref().and_then(|p| current_stamp(p).ok()).flatten();
                let available = state.is_some_and(|state| {
                    state.ledger.values().any(|row| {
                        row.avatar_stamp.is_some()
                            && row.avatar_stamp == metadata
                            && row
                                .descriptor
                                .as_ref()
                                .and_then(|p| p.avatar.as_ref())
                                .is_some_and(|a| {
                                    a.sha256.as_deref() == Some(&sha256)
                                        && a.ext.as_deref() == Some(&ext)
                                })
                    })
                });
                AvatarView::Image {
                    sha256,
                    ext,
                    local_path: if available {
                        path.map(|p| p.display().to_string())
                    } else {
                        None
                    },
                    available,
                }
            }
            _ => AvatarView::None,
        })
    }
}

impl ContentStore {
    /// BBS-only, bounded avatar read. Caller uses the dedicated async avatar
    /// admission gate; neither the status snapshot nor the room image API calls it.
    pub(crate) fn read_verified_avatar(&self, sha256: &str, ext: &str) -> Result<String> {
        bbs_sync::valid_hash(sha256).map_err(anyhow::Error::msg)?;
        if !matches!(ext, "png" | "jpg" | "webp") {
            bail!("invalid_sync_avatar")
        }
        let path = self.managed(&Path::new("avatars").join(format!("{sha256}.{ext}")), false)?;
        let state = self
            .state
            .load_state()
            .map_err(anyhow::Error::msg)?
            .ok_or_else(|| anyhow!("unverified_sync_avatar"))?;
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)?;
        let meta = file.metadata()?;
        if !meta.is_file() || meta.len() == 0 || meta.len() > 600_000 {
            bail!("invalid_sync_avatar")
        }
        let before = stamp(&meta);
        if !state.ledger.values().any(|r| {
            r.avatar_stamp.as_ref() == Some(&before)
                && r.descriptor
                    .as_ref()
                    .and_then(|p| p.avatar.as_ref())
                    .is_some_and(|a| {
                        a.kind == "image"
                            && a.sha256.as_deref() == Some(sha256)
                            && a.ext.as_deref() == Some(ext)
                    })
        }) {
            bail!("unverified_sync_avatar")
        }
        let mut bytes = Vec::with_capacity(meta.len() as usize);
        std::io::Read::read_to_end(
            &mut std::io::Read::by_ref(&mut file).take(600_001),
            &mut bytes,
        )?;
        if bytes.len() as u64 != meta.len()
            || stamp(&file.metadata()?) != before
            || current_stamp(&path)?.as_ref() != Some(&before)
            || bbs_sync::raw_sha256(&bytes) != sha256
        {
            bail!("changed_sync_avatar")
        }
        let mime = match ext {
            "png" => "image/png",
            "jpg" => "image/jpeg",
            _ => "image/webp",
        };
        use base64::Engine as _;
        Ok(format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&bytes)
        ))
    }
}
