//! Private BBS device-sync state. No coordinator or IPC is started by this module.
pub mod control;
pub(crate) mod roster;
pub(crate) mod reconcile;
pub(crate) mod transport;
pub(crate) mod relay;
pub(crate) mod scheduler;
pub(crate) mod rendezvous;
pub(crate) mod public;
pub(crate) mod exchange;
pub(crate) mod coordinator;
pub(crate) mod manager;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use ring::{
    rand::{SecureRandom, SystemRandom},
    signature::{Ed25519KeyPair, KeyPair},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::fd::AsRawFd,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

pub const SCHEMA_VERSION: u32 = 1;
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceIdentity {
    pub schema_version: u32,
    pub algorithm: String,
    pub public_key: String,
    pub private_key: String,
}
impl fmt::Debug for DeviceIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceIdentity")
            .field("public_key", &self.public_key)
            .field("private_key", &"<redacted>")
            .finish()
    }
}
impl DeviceIdentity {
    pub fn generate() -> Result<Self, String> {
        let mut seed = [0; 32];
        SystemRandom::new()
            .fill(&mut seed)
            .map_err(|_| "device key generation failed")?;
        Self::from_seed(seed)
    }
    pub fn from_seed(seed: [u8; 32]) -> Result<Self, String> {
        let pair = Ed25519KeyPair::from_seed_unchecked(&seed).map_err(|_| "invalid device key")?;
        Ok(Self {
            schema_version: SCHEMA_VERSION,
            algorithm: "ed25519".into(),
            public_key: BASE64.encode(pair.public_key().as_ref()),
            private_key: BASE64.encode(seed),
        })
    }
    fn key_pair(&self) -> Result<Ed25519KeyPair, String> {
        if self.schema_version != SCHEMA_VERSION || self.algorithm != "ed25519" {
            return Err("unsupported device identity schema or algorithm".into());
        }
        let private = BASE64
            .decode(&self.private_key)
            .map_err(|_| "invalid private key encoding")?;
        let public = BASE64
            .decode(&self.public_key)
            .map_err(|_| "invalid public key encoding")?;
        if BASE64.encode(&public) != self.public_key || BASE64.encode(&private) != self.private_key
        {
            return Err("noncanonical identity key encoding".into());
        }
        Ed25519KeyPair::from_seed_and_public_key(&private, &public)
            .map_err(|_| "invalid device key pair".into())
    }
    pub fn validate(&self) -> Result<(), String> {
        self.key_pair().map(|_| ())
    }
    pub fn device_id(&self) -> Result<String, String> {
        Ok(raw_sha256(self.key_pair()?.public_key().as_ref()))
    }
    pub fn sign(&self, bytes: &[u8]) -> Result<String, String> {
        Ok(BASE64.encode(self.key_pair()?.sign(bytes).as_ref()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct GroupState {
    pub worker_url: String,
    pub shared_threads: BTreeSet<String>,
    pub member_ids: BTreeSet<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Tombstone {
    pub thread_id: String,
    pub post_id: Option<String>,
    pub version_id: Option<String>,
    pub deleted_at: String,
}
impl Tombstone {
    pub fn validate(&self) -> Result<(), String> {
        safe_id(&self.thread_id)?;
        if let Some(post) = &self.post_id {
            safe_id(post)?;
        }
        if let Some(version) = &self.version_id {
            if self.post_id.is_none() {
                return Err("version deletion requires post_id".into());
            }
            valid_hash(version)?;
        }
        Ok(())
    }
    pub fn key(&self) -> Result<String, String> {
        self.validate()?;
        Ok(match (&self.post_id, &self.version_id) {
            (None, None) => format!("thread:{}", self.thread_id),
            (Some(p), None) => format!("post:{}/{}", self.thread_id, p),
            (Some(p), Some(v)) => format!("version:{}/{}/{}", self.thread_id, p, v),
            _ => return Err("invalid deletion target".into()),
        })
    }
    pub fn from_key(key: &str, deleted_at: String) -> Result<Self, String> {
        let (kind, value) = key.split_once(':').ok_or("invalid deletion key")?;
        let parts: Vec<_> = value.split('/').collect();
        let result = match (kind, parts.as_slice()) {
            ("thread", [t]) => Self {
                thread_id: (*t).into(),
                post_id: None,
                version_id: None,
                deleted_at,
            },
            ("post", [t, p]) => Self {
                thread_id: (*t).into(),
                post_id: Some((*p).into()),
                version_id: None,
                deleted_at,
            },
            ("version", [t, p, v]) => Self {
                thread_id: (*t).into(),
                post_id: Some((*p).into()),
                version_id: Some((*v).into()),
                deleted_at,
            },
            _ => return Err("invalid deletion key".into()),
        };
        result.validate()?;
        Ok(result)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AttachmentState {
    pub sha256: String,
    pub size_bytes: u64,
    pub verified: bool,
    #[serde(default)]
    pub stamp: Option<FileStamp>,
}
/// Rebuildable verification cache, never part of the peer manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileStamp {
    pub len: u64,
    pub modified_ns: i128,
    pub changed_ns: i128,
    pub dev: u64,
    pub ino: u64,
}
impl FileStamp {
    pub(crate) fn of(meta: &fs::Metadata) -> Self {
        Self {
            len: meta.len(),
            modified_ns: meta.mtime() as i128 * 1_000_000_000 + meta.mtime_nsec() as i128,
            changed_ns: meta.ctime() as i128 * 1_000_000_000 + meta.ctime_nsec() as i128,
            dev: meta.dev(),
            ino: meta.ino(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LedgerEntry {
    pub thread_id: String,
    pub post_id: String,
    pub version_id: String,
    pub updated_at: String,
    pub attachments: BTreeMap<String, AttachmentState>,
    #[serde(default)]
    pub descriptor: Option<PostVersion>,
    #[serde(default)]
    pub body_stamp: Option<FileStamp>,
    #[serde(default)]
    pub avatar_stamp: Option<FileStamp>,
}
impl LedgerEntry {
    pub fn key(&self) -> Result<String, String> {
        safe_id(&self.thread_id)?;
        Self::version_key(&self.post_id, &self.version_id)
    }
    pub fn version_key(post: &str, version: &str) -> Result<String, String> {
        safe_id(post)?;
        valid_hash(version)?;
        Ok(format!("{post}/{version}"))
    }
    pub fn parse_key(key: &str) -> Result<(&str, &str), String> {
        let (post, version) = key.split_once('/').ok_or("invalid ledger key")?;
        Self::version_key(post, version)?;
        Ok((post, version))
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncState {
    pub schema_version: u32,
    pub groups: BTreeMap<String, GroupState>,
    pub tombstones: BTreeMap<String, Tombstone>,
    pub ledger: BTreeMap<String, LedgerEntry>,
    /// Rebuildable cache of the existing thread.yaml creation record, not a
    /// second authoritative relationship table.
    #[serde(default)]
    pub thread_records: BTreeMap<String, ThreadRecordItem>,
    /// Rebuildable facts about files physically present on this device. Received
    /// placeholder sidecars are separate and never enter the outbound catalog.
    #[serde(default)]
    pub local_unavailable: BTreeMap<String, UnavailablePost>,
}
impl Default for SyncState {
    fn default() -> Self {
        Self::new()
    }
}
impl SyncState {
    pub fn new() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            groups: BTreeMap::new(),
            tombstones: BTreeMap::new(),
            ledger: BTreeMap::new(),
            thread_records: BTreeMap::new(),
            local_unavailable: BTreeMap::new(),
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err("unsupported BBS sync schema".into());
        }
        for (key, value) in &self.tombstones {
            if value.key()? != *key {
                return Err("deletion key does not match target".into());
            }
        }
        for (key, value) in &self.ledger {
            if value.key()? != *key {
                return Err("ledger key does not match version".into());
            }
            if let Some(p) = &value.descriptor {
                if p.thread_id != value.thread_id
                    || p.post_id != value.post_id
                    || p.version_id != value.version_id
                    || !matches!(p.kind.as_str(), "topic" | "reply")
                {
                    return Err("ledger descriptor does not match identity".into());
                }
            }
        }
        for (key, value) in &self.local_unavailable {
            if value.key()? != *key {
                return Err("unavailable key does not match identity".into());
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub phase: ManifestPhase,
    pub items: Vec<serde_json::Value>,
    pub next: Option<serde_json::Value>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ManifestPhase {
    Tombstones,
    Threads,
    Versions,
    Unavailable,
}
/// A source-held file outside sync eligibility, not a body or version identity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UnavailablePost {
    pub thread_id: String,
    pub post_id: String,
    pub reason: UnavailableReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UnavailableReason {
    TooLargeToSync,
}
impl UnavailablePost {
    pub(crate) fn key(&self) -> Result<String, String> {
        safe_id(&self.thread_id)?;
        safe_id(&self.post_id)?;
        if self
            .kind
            .as_deref()
            .is_some_and(|k| !matches!(k, "topic" | "reply"))
        {
            return Err("invalid_unavailable_kind".into());
        }
        Ok(format!("{}/{}", self.thread_id, self.post_id))
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PostVersion {
    pub thread_id: String,
    pub post_id: String,
    pub version_id: String,
    pub size_bytes: u64,
    pub kind: String,
    pub attachments: Vec<AttachmentRef>,
    pub avatar: Option<AvatarSidecar>,
}
/// Thread properties, excluding board/latest-post caches. These are not a
/// separate version identity; the one logical root is checked against posts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadRecord {
    pub schema: String,
    #[serde(rename = "threadId")]
    pub thread_id: String,
    pub status: String,
    pub visibility: String,
    #[serde(rename = "projectTags")]
    pub project_tags: Vec<String>,
    #[serde(rename = "createdByProject")]
    pub created_by_project: String,
    #[serde(rename = "createdByAgent")]
    pub created_by_agent: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadRecordItem {
    pub record: ThreadRecord,
    #[serde(rename = "threadRecordSha256")]
    pub sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttachmentRef {
    pub id: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub ext: String,
    pub available: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AvatarSidecar {
    pub kind: String,
    pub id: Option<String>,
    pub sha256: Option<String>,
    pub ext: Option<String>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
}
impl AvatarSidecar {
    pub(crate) fn none() -> Self {
        Self {
            kind: "none".into(),
            id: None,
            sha256: None,
            ext: None,
            size_bytes: None,
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        match self.kind.as_str() {
            "builtin"
                if self.sha256.is_none() && self.ext.is_none() && self.size_bytes.is_none() =>
            {
                let id = self.id.as_deref().ok_or("missing builtin avatar id")?;
                if !BUILTIN_AVATARS.contains(&id) {
                    return Err("invalid builtin avatar id".into());
                }
                Ok(())
            }
            "image" if self.id.is_none() => {
                valid_hash(self.sha256.as_deref().ok_or("missing avatar hash")?)?;
                if !matches!(self.ext.as_deref(), Some("png" | "jpg" | "webp")) {
                    return Err("invalid avatar extension".into());
                }
                if !self.size_bytes.is_some_and(|n| n > 0 && n <= 600_000) {
                    return Err("invalid avatar size".into());
                }
                Ok(())
            }
            "none"
                if self.id.is_none()
                    && self.sha256.is_none()
                    && self.ext.is_none()
                    && self.size_bytes.is_none() =>
            {
                Ok(())
            }
            _ => Err("invalid avatar sidecar".into()),
        }
    }
}
pub(crate) const BUILTIN_AVATARS: &[&str] = &[
    "claude",
    "claude-lantern",
    "claude-quill",
    "codex",
    "codex-slate",
    "codex-prism",
    "antigravity",
    "opencode",
    "pi",
    "kimi",
    "magi",
    "violet",
    "ember",
    "laughing-man",
    "puppeteer",
    "bartender",
    "user-default",
];
pub fn raw_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
pub(crate) fn safe_id(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 80
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err("invalid BBS id".into());
    }
    Ok(())
}
pub(crate) fn valid_hash(value: &str) -> Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err("invalid SHA-256".into());
    }
    Ok(())
}
// One account controller owns private credentials, including across app processes.
// flock releases automatically on process exit; a second process fails promptly.
pub(crate) struct ControlLease(File);
impl ControlLease {
    fn acquire(root: &Path) -> Result<Self, String> {
        if let Ok(meta) = fs::symlink_metadata(root) {
            if !meta.is_dir() || meta.file_type().is_symlink() {
                return Err("invalid private control directory".into());
            }
        }
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(root)
            .map_err(|error| error.to_string())?;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(root.join(".control.lock"))
            .map_err(|_| "control_lock_failed")?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("control_in_use".into());
        }
        Ok(Self(file))
    }
}
impl Drop for ControlLease {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

#[derive(Clone)]
pub struct StateStore {
    root: PathBuf,
    content_root: PathBuf,
}
impl StateStore {
    pub fn at(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().join("bbs-sync"),
            content_root: root.as_ref().join("Workspaces/bbs"),
        }
    }
    pub(crate) fn with_content_root(mut self, root: PathBuf) -> Self {
        self.content_root = root;
        self
    }
    pub(crate) fn content_lock(&self) -> Result<crate::bbs::BbsWriteLock, String> {
        crate::bbs::acquire_write_lock(&self.content_root).map_err(|_| "bbs_busy".into())
    }
    pub fn state_path(&self) -> PathBuf {
        self.root.join("state.json")
    }
    pub fn identity_path(&self) -> PathBuf {
        self.root.join("identity.json")
    }
    pub(crate) fn control_lease(&self) -> Result<ControlLease, String> {
        ControlLease::acquire(&self.root)
    }
    pub(crate) fn control_path(&self) -> PathBuf {
        self.root.join("control.json")
    }
    pub(crate) fn changes_dir(&self) -> PathBuf { self.root.join("changes") }
    pub(crate) fn initialize_changes(&self) -> Result<(), String> {
        let marker=self.changes_dir().join("content");
        if !marker.exists() { write(&marker, &0u64)?; }
        Ok(())
    }
    pub(crate) fn mark_content_changed(&self) {
        // Local publication/deletion remains successful even if a notification
        // fails. The next metadata fallback/open recovers it without backfilling scope.
        if matches!(control::read_membership(self), Ok(Some(_))) {
            if write(&self.changes_dir().join("content"), &uuid::Uuid::new_v4().to_string()).is_err() {
                crate::kota_debug_log("[bbs-sync] content_change_notification_failed");
            }
        }
    }
    pub fn load_state(&self) -> Result<Option<SyncState>, String> {
        load(&self.state_path(), |state: SyncState| {
            state.validate()?;
            Ok(state)
        })
    }
    pub fn save_state(&self, state: &SyncState) -> Result<(), String> {
        state.validate()?;
        write(&self.state_path(), state)
    }
    pub fn load_identity(&self) -> Result<Option<DeviceIdentity>, String> {
        load(&self.identity_path(), |identity: DeviceIdentity| {
            identity.validate()?;
            Ok(identity)
        })
    }
    pub fn save_identity(&self, identity: &DeviceIdentity) -> Result<(), String> {
        identity.validate()?;
        write(&self.identity_path(), identity)
    }
}
fn load<T: for<'de> Deserialize<'de>, U>(
    path: &Path,
    check: impl FnOnce(T) -> Result<U, String>,
) -> Result<Option<U>, String> {
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_file() => return Err("private state must be a regular file".into()),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read private state metadata: {error}")),
    }
    let bytes = fs::read(path).map_err(|error| format!("read private state: {error}"))?;
    // Parse errors must not quote private credential values.
    let value = serde_json::from_slice(&bytes).map_err(|_| "invalid private state JSON")?;
    check(value).map(Some)
}
fn write<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| "serialize private state failed")?;
    atomic_write(path, |file| file.write_all(&bytes))
}
fn atomic_write(
    path: &Path,
    write_bytes: impl FnOnce(&mut File) -> io::Result<()>,
) -> Result<(), String> {
    let dir = path.parent().ok_or("private state has no parent")?;
    if let Ok(meta) = fs::symlink_metadata(dir) {
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err("private state directory must not be a symlink".into());
        }
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|error| error.to_string())?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())?;
    let temp = dir.join(format!(".state-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        write_bytes(&mut file)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)?;
        File::open(dir)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(|error: io::Error| format!("persist private state: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct TestRoot(PathBuf);
    impl TestRoot {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("bbs-sync-{}", uuid::Uuid::new_v4())))
        }
    }
    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn identity_state_are_separate_private_and_redacted() {
        let root = TestRoot::new();
        let store = StateStore::at(&root.0);
        let identity = DeviceIdentity::from_seed([1; 32]).unwrap();
        fs::create_dir_all(store.state_path().parent().unwrap()).unwrap();
        fs::set_permissions(
            store.state_path().parent().unwrap(),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        store.save_identity(&identity).unwrap();
        store.save_state(&SyncState::new()).unwrap();
        assert_eq!(store.load_identity().unwrap(), Some(identity.clone()));
        assert_eq!(
            fs::metadata(store.state_path().parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        for path in [store.state_path(), store.identity_path()] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert!(!fs::read_to_string(store.state_path())
            .unwrap()
            .contains(&identity.private_key));
        assert!(!format!("{identity:?}").contains(&identity.private_key));
    }
    #[test]
    fn unknown_schema_corrupt_json_and_invalid_key_fail_without_overwrite() {
        let root = TestRoot::new();
        let store = StateStore::at(&root.0);
        store.save_state(&SyncState::new()).unwrap();
        let mut state = SyncState::new();
        state.schema_version = 99;
        for raw in [
            serde_json::to_vec(&state).unwrap(),
            b"corrupt-private-value".to_vec(),
        ] {
            fs::write(store.state_path(), &raw).unwrap();
            assert!(store.load_state().is_err());
            assert_eq!(fs::read(store.state_path()).unwrap(), raw);
        }
        let mut identity = DeviceIdentity::from_seed([1; 32]).unwrap();
        identity.public_key = DeviceIdentity::from_seed([2; 32]).unwrap().public_key;
        assert!(identity.validate().is_err());
        fs::write(store.identity_path(), b"broken-secret").unwrap();
        assert!(!store.load_identity().unwrap_err().contains("broken-secret"));
    }
    #[test]
    fn failed_write_and_failed_rename_leave_no_temporary_files() {
        let root = TestRoot::new();
        let path = root.0.join("private/state.json");
        atomic_write(&path, |file| file.write_all(b"old")).unwrap();
        assert!(atomic_write(&path, |file| {
            file.write_all(b"partial")?;
            Err(io::Error::new(io::ErrorKind::Other, "simulated full disk"))
        })
        .is_err());
        assert_eq!(fs::read(&path).unwrap(), b"old");
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(atomic_write(&path, |file| file.write_all(b"new")).is_err());
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }
    #[test]
    fn deletion_and_ledger_keys_round_trip_and_reject_invalid_shapes() {
        let hash = raw_sha256(b"abc");
        for key in [
            "thread:t".to_string(),
            "post:t/p".to_string(),
            format!("version:t/p/{hash}"),
        ] {
            assert_eq!(
                Tombstone::from_key(&key, "now".into())
                    .unwrap()
                    .key()
                    .unwrap(),
                key
            );
        }
        let invalid = Tombstone {
            thread_id: "t".into(),
            post_id: None,
            version_id: Some(hash.clone()),
            deleted_at: "now".into(),
        };
        assert!(invalid.key().is_err());
        assert!(Tombstone::from_key("post:t/../escape", "now".into()).is_err());
        let key = LedgerEntry::version_key("p", &hash).unwrap();
        assert_eq!(LedgerEntry::parse_key(&key).unwrap(), ("p", hash.as_str()));
        assert_eq!(
            hash,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
    #[test]
    fn avatar_sidecars_reject_unknown_kind_and_mixed_fields() {
        let mut avatar = AvatarSidecar {
            kind: "none".into(),
            id: None,
            sha256: None,
            ext: None,
            size_bytes: None,
        };
        assert!(avatar.validate().is_ok());
        avatar.kind = "path".into();
        assert!(avatar.validate().is_err());
        avatar.kind = "image".into();
        avatar.sha256 = Some(raw_sha256(b"x"));
        avatar.ext = Some("png".into());
        avatar.size_bytes = Some(1);
        assert!(avatar.validate().is_ok());
        avatar.id = Some("builtin".into());
        assert!(avatar.validate().is_err());
    }
}
