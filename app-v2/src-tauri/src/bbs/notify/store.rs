use super::*;
use std::{
    io::{BufReader, BufWriter, Seek, SeekFrom},
    os::unix::fs::{DirBuilderExt, PermissionsExt},
};

#[derive(Clone)]
pub(super) struct Queue {
    pub content: sync::ContentStore,
}

pub(super) struct Claim {
    pub record: Record,
    pub body: PathBuf,
    pub attachment_warning: bool,
    // The inode stays locked through Bus submission and receipt settlement.
    // Another App or recovery pass cannot take over an active processing file.
    _file: File,
}
pub(super) enum Check {
    Done,
    Wait { at: i64, record: Record },
    Recover,
    Deliver(Claim),
}

fn private_directory(path: &Path) -> Result<()> {
    let mut created = false;
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
        Ok(_) => bail!("unsafe_notification_directory"),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            match fs::DirBuilder::new().mode(0o700).create(path) {
                Ok(()) => {
                    created = true;
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    return private_directory(path)
                }
                Err(e) => return Err(e.into()),
            }
        }
        Err(e) => return Err(e.into()),
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    if created {
        File::open(path)?.sync_all()?;
        sync_parent(path)?;
    }
    Ok(())
}
fn sync_parent(path: &Path) -> Result<()> {
    File::open(
        path.parent()
            .ok_or_else(|| anyhow!("notification_parent_missing"))?,
    )?
    .sync_all()?;
    Ok(())
}
fn durable_body(content: &sync::ContentStore, thread_id: &str, body: &Path) -> Result<()> {
    let canonical_root = content.root().canonicalize()?;
    let relative = body
        .strip_prefix(content.root())
        .or_else(|_| body.strip_prefix(&canonical_root))?;
    let body = content.managed(relative, false)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&body)?;
    if !file.metadata()?.is_file() {
        bail!("unsafe_notification_body");
    }
    file.sync_all()?;
    let thread = content.managed(&Path::new("threads").join(thread_id), false)?;
    let meta = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(thread.join("thread.yaml"))?;
    if !meta.metadata()?.is_file() {
        bail!("unsafe_notification_thread");
    }
    meta.sync_all()?;
    // Covers both the resident posts/ directory and a Fork's versions/p/v/.
    let mut directory = body
        .parent()
        .ok_or_else(|| anyhow!("notification_parent_missing"))?;
    if !directory.starts_with(&thread) {
        bail!("unsafe_notification_body");
    }
    loop {
        File::open(directory)?.sync_all()?;
        if directory == thread {
            break;
        }
        directory = directory
            .parent()
            .ok_or_else(|| anyhow!("notification_parent_missing"))?;
    }
    sync_parent(&thread)?;
    File::open(&canonical_root)?.sync_all()?;
    sync_parent(&canonical_root)?;
    Ok(())
}
fn atomic_record(path: &Path, record: &Record) -> Result<()> {
    let temp = path.with_file_name(format!(".notify-{}.tmp", Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        {
            let mut writer = BufWriter::new(&mut file);
            serde_json::to_writer(&mut writer, record)?;
            writer.flush()?;
        }
        file.sync_all()?;
        fs::rename(&temp, path)?;
        sync_parent(path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
fn open_record(path: &Path, key: &str) -> Result<Option<(File, Record)>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if !file.metadata()?.is_file() {
        bail!("unsafe_notification_file");
    }
    let record: Record = serde_json::from_reader(BufReader::new(&file))
        .map_err(|_| anyhow!("invalid_notification_record"))?;
    record.validate(key)?;
    Ok(Some((file, record)))
}
fn stamp_at(path: &Path) -> Result<Option<FileStamp>> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => {
            Ok(Some(FileStamp::of(&meta)))
        }
        Ok(_) => bail!("unsafe_notification_body"),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

impl Queue {
    pub fn new(content: &sync::ContentStore) -> Self {
        Self {
            content: content.clone(),
        }
    }
    pub fn root(&self) -> PathBuf {
        self.content
            .state
            .identity_path()
            .parent()
            .unwrap()
            .join("notify")
    }
    pub fn initialize(&self) -> Result<()> {
        let root = self.root();
        private_directory(root.parent().unwrap())?;
        private_directory(&root)?;
        for phase in ["pending", "processing", "processed"] {
            private_directory(&root.join(phase))?;
        }
        Ok(())
    }
    pub fn directory(&self, phase: &str) -> Result<PathBuf> {
        if !matches!(phase, "pending" | "processing" | "processed") {
            bail!("invalid_notification_phase");
        }
        let root = self.root();
        for parent in [
            root.parent().unwrap().to_path_buf(),
            root.clone(),
            root.join(phase),
        ] {
            match fs::symlink_metadata(&parent) {
                Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                _ => bail!("unsafe_notification_directory"),
            }
        }
        Ok(self.root().join(phase))
    }
    pub fn path(&self, phase: &str, key: &str) -> Result<PathBuf> {
        bbs_sync::valid_hash(key).map_err(anyhow::Error::msg)?;
        Ok(self.directory(phase)?.join(format!("{key}.json")))
    }
    fn existing(&self, key: &str) -> Result<bool> {
        for phase in ["processed", "processing", "pending"] {
            if open_record(&self.path(phase, key)?, key)?.is_some() {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn bodies(&self, record: &Record) -> Result<[PathBuf; 2]> {
        let dest = &record.destination;
        dest.validate()?;
        // Local publication predates the sync safe_id length budget. Its
        // valid filename remains addressable even if it cannot be shared.
        let thread = Path::new("threads").join(&dest.thread_id);
        Ok([
            self.content.managed(
                &thread.join("posts").join(format!("{}.md", dest.post_id)),
                false,
            )?,
            self.content.managed(
                &thread
                    .join("versions")
                    .join(&dest.post_id)
                    .join(&record.version_id)
                    .join("post.md"),
                false,
            )?,
        ])
    }
    fn body(&self, record: &Record, state: &SyncState) -> Result<Option<PathBuf>> {
        let ledger =
            bbs_sync::LedgerEntry::version_key(&record.destination.post_id, &record.version_id)
                .ok()
                .and_then(|key| state.ledger.get(&key))
                .and_then(|r| r.body_stamp.as_ref());
        for path in self.bodies(record)? {
            if let Some(stamp) = stamp_at(&path)? {
                if record.proof.as_ref() == Some(&stamp) || ledger == Some(&stamp) {
                    return Ok(Some(path));
                }
            }
        }
        Ok(None)
    }
    fn thread_present(&self, record: &Record) -> Result<bool> {
        let relative = Path::new("threads")
            .join(&record.destination.thread_id)
            .join("thread.yaml");
        let path = self.content.managed(&relative, false)?;
        Ok(read_thread_meta(&path).is_ok_and(|m| m.thread_id == record.destination.thread_id))
    }
    fn deleted(record: &Record, state: &SyncState) -> bool {
        reconcile::suppressed(
            state,
            &record.destination.thread_id,
            &record.destination.post_id,
            &record.version_id,
        )
    }
    fn addressed_here(&self, record: &Record) -> Result<bool> {
        let target = &record.destination.target.device_id;
        if target == "local" {
            return Ok(record.author.local);
        }
        Ok(self
            .content
            .state
            .load_identity()
            .map_err(anyhow::Error::msg)?
            .map(|id| id.device_id().map_err(anyhow::Error::msg))
            .transpose()?
            .as_ref()
            == Some(target))
    }
    fn settle_path(&self, phase: &str, key: &str, mut record: Record, result: &str) -> Result<()> {
        record.result = Some(result.into());
        record.waiting.clear();
        let path = self.path(phase, key)?;
        // Preserve the locked processing inode until it becomes the receipt.
        // The outcome is informational; the durable processed filename dedupes.
        let processed = self.path("processed", key)?;
        fs::rename(&path, &processed)?;
        sync_parent(&processed)?;
        sync_parent(&path)?;
        atomic_record(&processed, &record)?;
        let _ = fs::remove_file(
            self.root()
                .join("processing")
                .join(format!("{key}.recovered")),
        );
        Ok(())
    }
    pub fn finish(&self, claim: Claim, result: &str) -> Result<()> {
        let _lock = acquire_write_lock(self.content.root())?;
        self.settle_path(
            "processing",
            &claim.record.destination.key(),
            claim.record.clone(),
            result,
        )
    }
    /// Cheap ready checks and deadlines never enter the large-file worker.
    pub fn check(&self, key: &str, run: &str, now: i64) -> Result<Check> {
        let _lock = acquire_write_lock(self.content.root())?;
        if open_record(&self.path("processed", key)?, key)?.is_some() {
            return Ok(Check::Done);
        }
        let mut phase = "pending";
        let pair = match open_record(&self.path(phase, key)?, key)? {
            Some(pair) => pair,
            None => {
                phase = "processing";
                let Some(pair) = open_record(&self.path(phase, key)?, key)? else {
                    return Ok(Check::Done);
                };
                pair
            }
        };
        let (mut file, mut record) = pair;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::WouldBlock {
                return Ok(Check::Done);
            }
            return Err(e.into());
        }
        if phase == "processing" && record.owner_run.as_deref() == Some(run) {
            // An uncertain submission is left for one subsequent startup,
            // never immediately retried by our own rename watcher events.
            return Ok(Check::Done);
        }
        let state = self.content.state_locked(&_lock)?;
        if !self.addressed_here(&record)? {
            self.settle_path(phase, key, record, "device_unavailable")?;
            return Ok(Check::Done);
        }
        if Self::deleted(&record, &state) {
            self.settle_path(phase, key, record, "deleted")?;
            return Ok(Check::Done);
        }
        if !record.ready {
            let mut exists = false;
            for body in self.bodies(&record)? {
                exists |= stamp_at(&body)?.is_some();
            }
            if exists {
                return Ok(Check::Recover);
            }
            let expiry = record.created_at_ms.saturating_add(ORPHAN_WAIT_MS);
            if now < expiry {
                return Ok(Check::Wait { at: expiry, record });
            }
            // The short publication lock proves no publisher is still between
            // its durable intent and body rename. Do not retire on time alone.
            self.settle_path(phase, key, record, "unpublished")?;
            diagnostic("unpublished_intent_retired");
            return Ok(Check::Done);
        }
        let Some(body) = self
            .body(&record, &state)?
            .filter(|_| self.thread_present(&record).unwrap_or(false))
        else {
            self.settle_path(phase, key, record, "body_unavailable")?;
            return Ok(Check::Done);
        };
        let missing = record.missing(&state);
        if missing && now < record.deadline_ms {
            return Ok(Check::Wait {
                at: record.deadline_ms,
                record,
            });
        }
        if phase == "processing" {
            let marker = self
                .root()
                .join("processing")
                .join(format!("{key}.recovered"));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&marker)
            {
                Ok(f) => {
                    f.sync_all()?;
                    sync_parent(&marker)?;
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    self.settle_path(phase, key, record, "recovery_exhausted")?;
                    return Ok(Check::Done);
                }
                Err(e) => return Err(e.into()),
            }
        } else {
            record.owner_run = Some(run.into());
            atomic_record(&self.path(phase, key)?, &record)?;
            file = open_record(&self.path(phase, key)?, key)?.unwrap().0;
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                return Err(io::Error::last_os_error().into());
            }
            let source = self.path(phase, key)?;
            let target = self.path("processing", key)?;
            fs::rename(&source, &target)?;
            sync_parent(&target)?;
            sync_parent(&source)?;
        }
        Ok(Check::Deliver(Claim {
            record,
            body,
            attachment_warning: missing,
            _file: file,
        }))
    }
    /// Only the crash window before ready needs a content hash. Run through the
    /// account file budget, independently of ready notices and their deadlines.
    pub fn recover(&self, key: &str, io: &mut LocalIo) -> Result<()> {
        let path = self.path("pending", key)?;
        let Some((_, mut record)) = open_record(&path, key)? else {
            return Ok(());
        };
        if record.ready {
            return Ok(());
        }
        let mut found = None;
        for body in self.bodies(&record)? {
            io.check()?;
            let mut file = match OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&body)
            {
                Ok(file) => file,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            let meta = file.metadata()?;
            if !meta.is_file() || meta.len() != record.file_size {
                continue;
            }
            let stamp = FileStamp::of(&meta);
            let mut hash = Sha256::new();
            let mut buffer = io.buffer()?;
            loop {
                io.check()?;
                let n = file.read(&mut buffer.bytes)?;
                if n == 0 {
                    break;
                }
                io.charge(n)?;
                hash.update(&buffer.bytes[..n]);
            }
            if format!("{:x}", hash.finalize()) == record.version_id
                && FileStamp::of(&file.metadata()?) == stamp
                && stamp_at(&body)?.as_ref() == Some(&stamp)
            {
                durable_body(&self.content, &record.destination.thread_id, &body)?;
                found = Some((body, stamp));
                break;
            }
        }
        let Some((body, proof)) = found else {
            bail!("notification_recovery_body_changed");
        };
        let lock = acquire_write_lock(self.content.root())?;
        io.check()?;
        let state = self.content.state_locked(&lock)?;
        let Some((_, current)) = open_record(&path, key)? else {
            return Ok(());
        };
        if current.ready {
            return Ok(());
        }
        if Self::deleted(&current, &state) || stamp_at(&body)?.as_ref() != Some(&proof) {
            return Ok(());
        }
        record.ready = true;
        record.proof = Some(proof);
        // The persisted original deadline is not restarted after an App crash.
        atomic_record(&path, &record)
    }
    pub fn body_text(&self, claim: &Claim) -> Result<String> {
        let record = &claim.record;
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&claim.body)?;
        let before = FileStamp::of(&file.metadata()?);
        if !file.metadata()?.is_file() || before.len != record.file_size {
            bail!("notification_body_changed");
        }
        if record.file_size - record.body_offset > BODY_BUDGET {
            return Ok(format!("Post body omitted from this notice because it exceeds 16 KiB. Run kota-bbs show {} to read it.", record.destination.thread_id));
        }
        file.seek(SeekFrom::Start(record.body_offset))?;
        let mut raw = Vec::new();
        std::io::Read::by_ref(&mut file)
            .take(BODY_BUDGET + 1)
            .read_to_end(&mut raw)?;
        if raw.len() as u64 > BODY_BUDGET
            || FileStamp::of(&file.metadata()?) != before
            || stamp_at(&claim.body)?.as_ref() != Some(&before)
        {
            bail!("notification_body_changed");
        }
        String::from_utf8(raw)
            .map(|s| s.trim().to_string())
            .map_err(|_| anyhow!("invalid_notification_body"))
    }
    pub fn still_publishable(&self, claim: &Claim) -> Result<bool> {
        let lock = acquire_write_lock(self.content.root())?;
        let state = self.content.state_locked(&lock)?;
        Ok(!Self::deleted(&claim.record, &state)
            && self.addressed_here(&claim.record)?
            && self.thread_present(&claim.record)?
            && self.body(&claim.record, &state)?.as_ref() == Some(&claim.body))
    }
}

/// Caller holds the existing publication lock. Empty mentions cost no queue
/// directory creation or identity read, including old CLI/API publication.
pub(in crate::bbs) fn prepare(
    content: &sync::ContentStore,
    _lock: &BbsWriteLock,
    meta: &BbsPostMeta,
    raw: &[u8],
    version: &str,
    local: bool,
    state: &SyncState,
) -> Result<Vec<String>> {
    if meta.mentions.is_empty() {
        return Ok(Vec::new());
    }
    // Notification addressing needs the existing device key, not the peer
    // roster cache and never the control lease. A damaged directory must not
    // turn otherwise valid received content into a roster-dependent failure.
    let device_id = content
        .state
        .load_identity()
        .map_err(anyhow::Error::msg)?
        .map(|id| id.device_id().map_err(anyhow::Error::msg))
        .transpose()?;
    let selected = meta
        .mentions
        .iter()
        .filter(|m| {
            if local {
                m.device_id == "local" || device_id.as_deref() == Some(&m.device_id)
            } else {
                device_id.as_deref().is_some_and(|id| m.received_here(id))
            }
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    if selected.is_empty() {
        return Ok(Vec::new());
    }
    let delimiter = raw
        .windows(5)
        .position(|w| w == b"\n---\n")
        .ok_or_else(|| anyhow!("invalid_notification_post"))?;
    let mut waiting = BTreeSet::new();
    if !local {
        for row in state.ledger.values().filter(|r| {
            r.thread_id == meta.thread_id
                && r.body_stamp.is_some()
                && !reconcile::suppressed(state, &r.thread_id, &r.post_id, &r.version_id)
        }) {
            if let Some(p) = &row.descriptor {
                for a in &p.attachments {
                    let reference = WaitingAttachment {
                        post_id: p.post_id.clone(),
                        version_id: p.version_id.clone(),
                        attachment_id: a.id.clone(),
                        sha256: a.sha256.clone(),
                        size_bytes: a.size_bytes,
                    };
                    if !reference.verified(state) {
                        waiting.insert(reference);
                    }
                }
            }
        }
        for a in &meta.attachments {
            waiting.insert(WaitingAttachment {
                post_id: meta.post_id.clone(),
                version_id: version.into(),
                attachment_id: a.id.clone(),
                sha256: a.sha256.clone(),
                size_bytes: a.size_bytes,
            });
        }
    }
    let queue = Queue::new(content);
    queue.initialize()?;
    let created = now_ms();
    let mut keys = Vec::new();
    for target in selected {
        let record = Record {
            schema_version: SCHEMA,
            destination: Destination {
                thread_id: meta.thread_id.clone(),
                post_id: meta.post_id.clone(),
                target,
            },
            version_id: version.into(),
            author: Author {
                project_id: meta.project_id.clone(),
                project_name: meta.project_display_name.clone(),
                agent_id: meta.agent_id.clone(),
                agent_name: meta.agent_display_name.clone(),
                local,
                local_device_id: local.then(|| device_id.clone()).flatten(),
                received_via: (!local)
                    .then(|| content.receiving_peer().map(str::to_string))
                    .flatten(),
            },
            body_offset: (delimiter + 5) as u64,
            file_size: raw.len() as u64,
            created_at_ms: created,
            deadline_ms: created + if local { 0 } else { ATTACHMENT_WAIT_MS },
            waiting: waiting.iter().cloned().collect(),
            ready: false,
            proof: None,
            owner_run: None,
            result: None,
        };
        let key = record.destination.key();
        if !queue.existing(&key)? {
            atomic_record(&queue.path("pending", &key)?, &record)?;
            keys.push(key);
        } else if open_record(&queue.path("pending", &key)?, &key)?
            .is_some_and(|(_, old)| !old.ready && old.version_id == version)
        {
            keys.push(key);
        }
    }
    Ok(keys)
}

/// After the publication point, failure leaves awaiting_post recoverable and is
/// diagnostic only. It must never turn a successful post into "please repost".
pub(in crate::bbs) fn published(
    content: &sync::ContentStore,
    _lock: &BbsWriteLock,
    keys: &[String],
    body: &Path,
) {
    if keys.is_empty() {
        return;
    }
    let result = (|| -> Result<()> {
        let queue = Queue::new(content);
        // The legacy local publication helper renames without fsync. Only
        // explicit notifications need this additional commit barrier; a failed
        // barrier leaves their intent awaiting recovery, never asks to repost.
        let Some(first_key) = keys.first() else {
            return Ok(());
        };
        let Some((_, first)) = open_record(&queue.path("pending", first_key)?, first_key)? else {
            return Ok(());
        };
        durable_body(content, &first.destination.thread_id, body)?;
        let proof = stamp_at(body)?.ok_or_else(|| anyhow!("notification_post_missing"))?;
        let installed = now_ms();
        for key in keys {
            let path = queue.path("pending", key)?;
            if let Some((_, mut record)) = open_record(&path, key)? {
                if !record.ready {
                    record.ready = true;
                    record.proof = Some(proof.clone());
                    record.deadline_ms = installed
                        + if record.author.local {
                            0
                        } else {
                            ATTACHMENT_WAIT_MS
                        };
                    atomic_record(&path, &record)?;
                }
            }
        }
        Ok(())
    })();
    if result.is_err() {
        diagnostic("post_published_notification_pending_recovery");
    }
}
