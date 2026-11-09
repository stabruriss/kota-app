//! One bounded account file worker. All open/read/write/hash/fsync/cleanup runs
//! here, not on the async network executor. Callers only exchange one chunk.
use super::{
    background_thread, BytePermit, Cancellation, Error, Limits, Result, BYTES_PER_SECOND, MAX_FRAME,
};
use crate::bbs_sync::{safe_id, valid_hash, FileStamp};
use bytes::BytesMut;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tokio::sync::{oneshot, watch, OwnedSemaphorePermit};

/// Counts real permit waiters and queued/executing operations. This is only
/// liveness eligibility: it never increments activity bytes or a progress clock.
#[derive(Clone)]
struct FileWork(watch::Sender<usize>);
struct Working(watch::Sender<usize>);
impl FileWork {
    fn new() -> Self {
        Self(watch::channel(0).0)
    }
    fn begin(&self) -> Working {
        self.0.send_modify(|count| *count += 1);
        Working(self.0.clone())
    }
}
impl Drop for Working {
    fn drop(&mut self) {
        self.0.send_modify(|count| *count -= 1);
    }
}

pub(crate) const CHUNK_BYTES: usize = MAX_FRAME - 25;
const IO_TICK: Duration = Duration::from_millis(20);
/// A v5 attachment moves in Gets of at most this many bytes. Only slice ends
/// are fsynced, so they are the only durable resume points.
#[cfg(not(test))]
pub(crate) const RESUME_SLICE: u64 = 8 * 1024 * 1024;
// Tests exercise multi-slice files without multi-megabyte fixtures.
#[cfg(test)]
pub(crate) const RESUME_SLICE: u64 = 64 * 1024;
const RESUME_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The deterministic partial of a resumable attachment receive. Its length
/// after the last slice fsync is the only stored progress.
pub(crate) fn resumable_partial(resource: &Resource) -> String {
    resumable_partial_name(&resource.sha256, resource.size_bytes)
}
pub(crate) fn resumable_partial_name(sha256: &str, size_bytes: u64) -> String {
    format!(".receive-{sha256}-{size_bytes}.partial")
}
/// Startup/refresh recovery keeps a recent resumable partial and removes an
/// abandoned one. Uuid partials stay the caller's responsibility.
pub(crate) fn stale_resumable_partial(name: &str, modified: std::time::SystemTime) -> Option<bool> {
    let (hash, size) = name
        .strip_prefix(".receive-")?
        .strip_suffix(".partial")?
        .split_once('-')?;
    if valid_hash(hash).is_err() || size.is_empty() || !size.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(modified.elapsed().is_ok_and(|age| age > RESUME_MAX_AGE))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum ResourceKind {
    Post {
        thread_id: String,
        post_id: String,
        version_id: String,
    },
    Attachment {
        thread_id: String,
        post_id: String,
        version_id: String,
        attachment_id: String,
    },
    Avatar {
        sha256: String,
        ext: String,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Resource {
    pub(crate) identity: ResourceKind,
    pub(crate) sha256: String,
    pub(crate) size_bytes: u64,
}
impl Resource {
    pub(crate) fn validate(&self) -> Result<()> {
        valid_hash(&self.sha256).map_err(|_| Error::InvalidResource)?;
        if self.size_bytes > crate::bbs::MAX_ATTACHMENT_BYTES {
            return Err(Error::InvalidResource);
        }
        match &self.identity {
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
                safe_id(thread_id).map_err(|_| Error::InvalidResource)?;
                safe_id(post_id).map_err(|_| Error::InvalidResource)?;
                valid_hash(version_id).map_err(|_| Error::InvalidResource)?;
                if matches!(self.identity, ResourceKind::Post { .. }) && self.sha256 != *version_id
                {
                    return Err(Error::InvalidResource);
                }
                if let ResourceKind::Attachment { attachment_id, .. } = &self.identity {
                    safe_id(attachment_id).map_err(|_| Error::InvalidResource)?;
                }
            }
            ResourceKind::Avatar { sha256, ext } => {
                if sha256 != &self.sha256
                    || self.size_bytes > 600_000
                    || !matches!(ext.as_str(), "png" | "jpg" | "webp")
                {
                    return Err(Error::InvalidResource);
                }
            }
        }
        Ok(())
    }
}

pub(crate) struct Chunk {
    pub(crate) bytes: BytesMut,
    pub(crate) offset: u64,
    _bytes_permit: BytePermit,
}
enum Op {
    Read {
        done: oneshot::Sender<Result<Option<Chunk>>>,
        _work: Working,
    },
    Write {
        offset: u64,
        bytes: BytesMut,
        permit: BytePermit,
        done: oneshot::Sender<Result<u64>>,
        _work: Working,
    },
    Finish {
        hash: String,
        io: FileIo,
        permit: OwnedSemaphorePermit,
        done: oneshot::Sender<Result<VerifiedFile>>,
        _work: Working,
    },
    /// Resumable receive only: fsync a completed slice and remember its hash.
    Checkpoint {
        done: oneshot::Sender<Result<u64>>,
        _work: Working,
    },
}
#[derive(Clone, Copy)]
enum Mode {
    /// Send bytes [start, end) of a registered source.
    Read { start: u64, end: u64 },
    /// v4: a private uuid partial, removed unless verified.
    Receive,
    /// v5 attachment: a deterministic partial that survives a lost link.
    Resume,
}
/// Memory only. Restoring a long prefix hash after every lost link would cost
/// a paced re-read; the last fsynced slice's hash state avoids that until exit.
struct ResumeHash {
    name: String,
    offset: u64,
    hash: Sha256,
}
type ResumeCache = Arc<Mutex<Option<ResumeHash>>>;
enum Job {
    Stop,
    Local(Box<dyn FnOnce() + Send>),
    Transfer {
        resource: Resource,
        path: PathBuf,
        mode: Mode,
        ops: Receiver<Op>,
        own_cancel: Cancellation,
        parent_cancel: Cancellation,
        limits: Limits,
        ready: oneshot::Sender<Result<u64>>,
        opening: Working,
        work: FileWork,
        resume: ResumeCache,
    },
    Cleanup {
        path: PathBuf,
        _permit: OwnedSemaphorePermit,
        _work: Working,
    },
    RemoveSmall {
        path: PathBuf,
        _work: Working,
    },
}
#[derive(Clone)]
pub(crate) struct FileIo {
    jobs: SyncSender<Job>,
    limits: Limits,
    activity: Arc<AtomicU64>,
    work: FileWork,
    closing: Arc<AtomicBool>,
    finished: tokio::sync::watch::Receiver<bool>,
    resume: ResumeCache,
}
impl FileIo {
    /// Explicit start only. The account coordinator shares this instance.
    pub(crate) fn start(limits: Limits) -> Result<Self> {
        let (jobs, rx) = mpsc::sync_channel(1);
        let (ready, started) = mpsc::sync_channel(1);
        let (finished_tx, finished) = tokio::sync::watch::channel(false);
        thread::Builder::new()
            .name("bbs-sync-files".into())
            .spawn(move || {
                struct Finished(tokio::sync::watch::Sender<bool>);
                impl Drop for Finished {
                    fn drop(&mut self) {
                        self.0.send_replace(true);
                    }
                }
                let _finished = Finished(finished_tx);
                let state = background_thread();
                let ok = state.is_ok();
                let _ = ready.send(state);
                if !ok {
                    return;
                }
                while let Ok(job) = rx.recv() {
                    match job {
                        Job::Stop => break,
                        Job::Local(work) => work(),
                        Job::Cleanup { path, _work, .. } | Job::RemoveSmall { path, _work } => {
                            if fs::remove_file(path)
                                .is_err_and(|e| e.kind() != std::io::ErrorKind::NotFound)
                            {
                                crate::kota_debug_log(
                                    "[bbs-sync] partial_cleanup_failed; staging recovery required",
                                );
                            }
                        }
                        Job::Transfer {
                            resource,
                            path,
                            mode,
                            ops,
                            own_cancel,
                            parent_cancel,
                            limits,
                            ready,
                            opening,
                            work,
                            resume,
                        } => {
                            let _ = transfer(
                                resource,
                                path,
                                mode,
                                ops,
                                own_cancel,
                                parent_cancel,
                                limits,
                                ready,
                                opening,
                                work,
                                resume,
                            );
                        }
                    }
                }
            })
            .map_err(|_| Error::Runtime)?;
        started.recv().map_err(|_| Error::Runtime)??;
        Ok(Self {
            jobs,
            limits,
            activity: Arc::new(AtomicU64::new(0)),
            work: FileWork::new(),
            closing: Arc::new(AtomicBool::new(false)),
            finished,
            resume: Arc::new(Mutex::new(None)),
        })
    }
    pub(crate) async fn stop_and_wait(&self) {
        if !self.closing.swap(true, Ordering::AcqRel) {
            let mut stop = Job::Stop;
            loop {
                match self.jobs.try_send(stop) {
                    Ok(()) | Err(mpsc::TrySendError::Disconnected(_)) => break,
                    Err(mpsc::TrySendError::Full(job)) => {
                        stop = job;
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                }
            }
        }
        let mut finished = self.finished.clone();
        while !*finished.borrow_and_update() {
            if finished.changed().await.is_err() {
                break;
            }
        }
    }
    pub(crate) fn activity(&self) -> u64 {
        self.activity.load(Ordering::Relaxed)
    }
    /// Memory-only observation. An idle FileHandle/VerifiedFile or roster read
    /// is not work. Activity owners may use a nonzero count for small probes,
    /// never for Working messages, file completion or timeout refreshes.
    pub(crate) fn work_status(&self) -> watch::Receiver<usize> {
        self.work.0.subscribe()
    }
    /// Roster staging owns no transfer permit between pages. Dropping an RPC
    /// only schedules unlink; the network/IPC thread performs no filesystem I/O.
    pub(crate) fn remove_later(&self, path: PathBuf) {
        if self
            .jobs
            .try_send(Job::RemoveSmall {
                path,
                _work: self.work.begin(),
            })
            .is_err()
        {
            crate::kota_debug_log(
                "[bbs-roster] staging_cleanup_deferred; startup recovery required",
            );
        }
    }
    /// The single local cache builder waits for the account file permit without
    /// polling or competing with an active large transfer. Regular sync jobs
    /// still use try-admission and preserve their Busy/yield contract.
    pub(crate) async fn run_when_available<T, F>(
        &self,
        parent: &Cancellation,
        operation: F,
    ) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut LocalIo) -> T + Send + 'static,
    {
        let work = self.work.begin();
        let permit = tokio::select! {
            _=parent.cancelled()=>return Err(Error::Cancelled),
            p=self.limits.files.clone().acquire_owned()=>p.map_err(|_|Error::Closed)?,
        };
        self.schedule(parent, work, move |io| {
            let _permit = permit;
            operation(io)
        })
        .await
    }
    /// Once a peer has accepted a round, metadata work cannot use try-admission:
    /// roster publication now shares this permit. Wait within the existing
    /// no-progress deadline instead of abandoning half of an accepted round.
    pub(crate) async fn run_in_round<T, F>(&self, parent: &Cancellation, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut LocalIo) -> T + Send + 'static,
    {
        let work = self.work.begin();
        // Bound admission, not the whole scan/copy. Work that keeps making I/O
        // progress may legitimately exceed twenty seconds in a large library.
        let permit = tokio::select! {
            _=parent.cancelled()=>return Err(Error::Cancelled),
            p=tokio::time::timeout(super::PROGRESS_TIMEOUT, self.limits.files.clone().acquire_owned()) =>
                p.map_err(|_| Error::Timeout)?.map_err(|_| Error::Closed)?,
        };
        self.schedule(parent, work, move |io| {
            let _permit = permit;
            operation(io)
        })
        .await
    }
    /// Repository scans/hash/copies use the very same account file permit and
    /// background thread as network transfers. This starts no additional worker.
    pub(crate) async fn run<T, F>(&self, parent: &Cancellation, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut LocalIo) -> T + Send + 'static,
    {
        let permit = self.limits.file()?;
        self.schedule(parent, self.work.begin(), move |io| {
            let _permit = permit;
            operation(io)
        })
        .await
    }
    async fn schedule<T, F>(&self, parent: &Cancellation, work: Working, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut LocalIo) -> T + Send + 'static,
    {
        if self.closing.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        if parent.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let own = Cancellation::default();
        let worker_cancel = own.clone();
        struct CancelOnDrop(Cancellation);
        impl Drop for CancelOnDrop {
            fn drop(&mut self) {
                self.0.cancel();
            }
        }
        let _guard = CancelOnDrop(own);
        let worker_parent = parent.clone();
        let limits = self.limits.clone();
        let (done, received) = oneshot::channel();
        let activity = self.activity.clone();
        self.jobs
            .try_send(Job::Local(Box::new(move || {
                // The job owns this guard, even if the awaiting future is
                // cancelled while a syscall/closure is still on the worker.
                let _work = work;
                let mut io = LocalIo {
                    activity,
                    own: worker_cancel,
                    parent: worker_parent,
                    limits,
                    next: Instant::now(),
                    #[cfg(test)]
                    charged: 0,
                };
                let result = io.check().and_then(|_| {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| operation(&mut io)))
                        .map_err(|_| Error::Runtime)
                });
                let _ = done.send(result);
            })))
            .map_err(|_| Error::Busy)?;
        tokio::select! {
            value = received => value.map_err(|_| Error::Closed)?,
            _ = parent.cancelled() => Err(Error::Cancelled),
        }
    }
    async fn open(
        &self,
        resource: Resource,
        path: &Path,
        mode: Mode,
        parent: &Cancellation,
    ) -> Result<(FileHandle, u64)> {
        resource.validate()?;
        if self.closing.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        if parent.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let permit = self.limits.file()?;
        let (tx, ops) = mpsc::sync_channel(1);
        let own_cancel = Cancellation::default();
        let (ready, initialized) = oneshot::channel();
        let handle = FileHandle {
            tx,
            own_cancel: own_cancel.clone(),
            parent: parent.clone(),
            permit: Some(permit),
            work: self.work.clone(),
        };
        self.jobs
            .try_send(Job::Transfer {
                resource,
                path: path.to_owned(),
                mode,
                ops,
                own_cancel,
                parent_cancel: parent.clone(),
                limits: self.limits.clone(),
                ready,
                opening: self.work.begin(),
                work: self.work.clone(),
                resume: self.resume.clone(),
            })
            .map_err(|_| Error::Busy)?;
        let start = tokio::select! {
            result = initialized => result.map_err(|_| Error::Closed)??,
            _ = parent.cancelled() => return Err(Error::Cancelled),
        };
        Ok((handle, start))
    }
    /// Source paths must come from the registered BBS resource resolver (step4).
    /// No path is decoded from the peer's Resource value.
    pub(crate) async fn read(
        &self,
        resource: Resource,
        source: &Path,
        cancel: &Cancellation,
    ) -> Result<FileReader> {
        let end = resource.size_bytes;
        self.read_range(resource, source, 0, end, cancel).await
    }
    /// A whole-file read also checks the source hash; a range leaves the
    /// final hash check to the receiver, which hashes from byte zero.
    pub(crate) async fn read_range(
        &self,
        resource: Resource,
        source: &Path,
        start: u64,
        end: u64,
        cancel: &Cancellation,
    ) -> Result<FileReader> {
        if start > end || end > resource.size_bytes {
            return Err(Error::Protocol);
        }
        let (handle, _) = self.open(resource, source, Mode::Read { start, end }, cancel).await?;
        Ok(FileReader { handle })
    }
    pub(crate) async fn receive(
        &self,
        resource: Resource,
        staging: &Path,
        cancel: &Cancellation,
    ) -> Result<FileWriter> {
        let (handle, start) = self.open(resource, staging, Mode::Receive, cancel).await?;
        Ok(FileWriter { handle, io: self.clone(), start })
    }
    /// Opens the deterministic partial, cut back to its last slice boundary.
    /// After a restart this re-reads that prefix once to restore its hash.
    pub(crate) async fn resume(
        &self,
        resource: Resource,
        staging: &Path,
        cancel: &Cancellation,
    ) -> Result<FileWriter> {
        if resource.size_bytes == 0 {
            return Err(Error::Protocol);
        }
        let (handle, start) = self.open(resource, staging, Mode::Resume, cancel).await?;
        Ok(FileWriter { handle, io: self.clone(), start })
    }
}
struct FileHandle {
    tx: SyncSender<Op>,
    own_cancel: Cancellation,
    parent: Cancellation,
    permit: Option<OwnedSemaphorePermit>,
    work: FileWork,
}
impl FileHandle {
    async fn reply<T>(&self, rx: oneshot::Receiver<Result<T>>) -> Result<T> {
        tokio::select! {
            result = rx => result.map_err(|_| Error::Closed)?,
            _ = self.parent.cancelled() => Err(Error::Cancelled),
            _ = self.own_cancel.cancelled() => Err(Error::Cancelled),
        }
    }
}
impl Drop for FileHandle {
    fn drop(&mut self) {
        self.own_cancel.cancel();
    }
}
pub(crate) struct FileReader {
    handle: FileHandle,
}
impl FileReader {
    pub(crate) async fn next(&mut self) -> Result<Option<Chunk>> {
        let (done, rx) = oneshot::channel();
        self.handle
            .tx
            .try_send(Op::Read {
                done,
                _work: self.handle.work.begin(),
            })
            .map_err(|_| Error::Closed)?;
        self.handle.reply(rx).await
    }
}
pub(crate) struct FileWriter {
    handle: FileHandle,
    io: FileIo,
    start: u64,
}
impl FileWriter {
    /// The first byte this writer expects. Zero except for a resumed partial.
    pub(crate) fn start(&self) -> u64 {
        self.start
    }
    /// Resumable receive only: fsync the bytes written so far.
    pub(crate) async fn checkpoint(&mut self) -> Result<u64> {
        let (done, rx) = oneshot::channel();
        self.handle
            .tx
            .try_send(Op::Checkpoint {
                done,
                _work: self.handle.work.begin(),
            })
            .map_err(|_| Error::Closed)?;
        self.handle.reply(rx).await
    }
    /// Resolves only after this exact offset was written and included in SHA.
    pub(crate) async fn write(&mut self, offset: u64, bytes: BytesMut) -> Result<u64> {
        if bytes.is_empty() || bytes.len() > CHUNK_BYTES || bytes.capacity() > MAX_FRAME * 2 {
            return Err(Error::Protocol);
        }
        let permit = self.io.limits.reserve(bytes.capacity())?;
        let (done, rx) = oneshot::channel();
        self.handle
            .tx
            .try_send(Op::Write {
                offset,
                bytes,
                permit,
                done,
                _work: self.handle.work.begin(),
            })
            .map_err(|_| Error::Closed)?;
        self.handle.reply(rx).await
    }
    pub(crate) async fn finish(mut self, hash: String) -> Result<VerifiedFile> {
        let (done, rx) = oneshot::channel();
        let permit = self.handle.permit.take().ok_or(Error::Closed)?;
        self.handle
            .tx
            .try_send(Op::Finish {
                hash,
                io: self.io.clone(),
                permit,
                done,
                _work: self.handle.work.begin(),
            })
            .map_err(|_| Error::Closed)?;
        // A cancelled reply drops VerifiedFile itself, so success cannot orphan
        // a partial between the worker's send and this task's next poll.
        self.handle.reply(rx).await
    }
}
/// Not serializable: this path is local, private and only available after fsync.
pub(crate) struct VerifiedFile {
    verified_stamp: FileStamp,
    path: Option<PathBuf>,
    pub(crate) resource: Resource,
    io: FileIo,
    permit: Option<OwnedSemaphorePermit>,
    // A resumable partial that failed installation stays for the next round,
    // which re-verifies it from its last slice boundary.
    keep: bool,
}
impl VerifiedFile {
    pub(crate) fn path(&self) -> &Path {
        self.path.as_deref().expect("verified path consumed once")
    }
    /// Call only after the step4 installer has moved the validated temporary.
    pub(crate) fn installed(mut self) {
        self.path.take();
    }
    /// A verified receive already owns the account file permit. Transfer that
    /// ownership to repository installation without trying to acquire it twice.
    /// The installer returns Ok only after moving the private file into its
    /// validated BBS location; any failure/abandoned task cleans the partial.
    pub(crate) async fn install<T, E, F>(
        self,
        cancel: &Cancellation,
        operation: F,
    ) -> Result<std::result::Result<T, E>>
    where
        T: Send + 'static,
        E: Send + 'static,
        F: FnOnce(&Path, &Resource, &mut LocalIo) -> std::result::Result<T, E> + Send + 'static,
    {
        self.io
            .clone()
            .schedule(cancel, self.io.work.begin(), move |io| {
                let metadata = fs::symlink_metadata(self.path()).map_err(|_| Error::Io)?;
                if !metadata.is_file()
                    || metadata.file_type().is_symlink()
                    || FileStamp::of(&metadata) != self.verified_stamp
                {
                    return Err(Error::Integrity);
                }
                let result = operation(self.path(), &self.resource, io);
                if result.is_ok() {
                    self.installed();
                }
                Ok(result)
            })
            .await?
    }
}
impl Drop for VerifiedFile {
    fn drop(&mut self) {
        if self.keep {
            self.path.take();
        }
        if let Some(path) = self.path.take() {
            // This permit prevents any next file job until installation/cleanup.
            // The worker remains alive through io.jobs; its one job slot is free.
            if let Some(permit) = self.permit.take() {
                if self
                    .io
                    .jobs
                    .try_send(Job::Cleanup {
                        path,
                        _permit: permit,
                        _work: self.io.work.begin(),
                    })
                    .is_err()
                {
                    crate::kota_debug_log(
                        "[bbs-sync] partial_cleanup_queue_failed; staging recovery required",
                    );
                }
            }
        }
    }
}

/// Used only inside FileIo::run/VerifiedFile::install on the owned file thread.
/// Every streaming read/write/hash/copy loop charges <=16 KiB per iteration.
pub(crate) struct LocalIo {
    activity: Arc<AtomicU64>,
    own: Cancellation,
    parent: Cancellation,
    limits: Limits,
    next: Instant,
    #[cfg(test)]
    charged: u64,
}
pub(crate) struct IoBuffer {
    pub(crate) bytes: BytesMut,
    _permit: BytePermit,
}
impl LocalIo {
    #[cfg(test)]
    pub(crate) fn charged_bytes(&self) -> u64 {
        self.charged
    }
    pub(crate) fn check(&self) -> Result<()> {
        if stopped(&self.own, &self.parent) {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
    pub(crate) fn buffer(&self) -> Result<IoBuffer> {
        self.check()?;
        Ok(IoBuffer {
            bytes: BytesMut::zeroed(MAX_FRAME),
            _permit: self.limits.reserve(MAX_FRAME)?,
        })
    }
    pub(crate) fn charge(&mut self, bytes: usize) -> Result<()> {
        if bytes > MAX_FRAME {
            return Err(Error::Protocol);
        }
        pace(self.next, &self.own, &self.parent)?;
        self.next = advance(self.next, bytes);
        self.activity.fetch_add(bytes as u64, Ordering::Relaxed);
        #[cfg(test)]
        {
            self.charged += bytes as u64;
        }
        Ok(())
    }
}

fn stopped(own: &Cancellation, parent: &Cancellation) -> bool {
    own.is_cancelled() || parent.is_cancelled()
}
fn pace(deadline: Instant, own: &Cancellation, parent: &Cancellation) -> Result<()> {
    while Instant::now() < deadline {
        if stopped(own, parent) {
            return Err(Error::Cancelled);
        }
        thread::sleep(IO_TICK.min(deadline.saturating_duration_since(Instant::now())));
    }
    if stopped(own, parent) {
        return Err(Error::Cancelled);
    }
    Ok(())
}
fn advance(deadline: Instant, bytes: usize) -> Instant {
    let interval = Duration::from_secs_f64(bytes as f64 / BYTES_PER_SECOND as f64);
    let now = Instant::now();
    deadline.max(now.checked_sub(interval * 3).unwrap_or(now)) + interval
}
fn open_source(path: &Path, size: u64) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| Error::Io)?;
    let meta = file.metadata().map_err(|_| Error::Io)?;
    if !meta.is_file() || meta.len() != size {
        return Err(Error::InvalidResource);
    }
    Ok(file)
}
/// Restore the SHA state of an existing prefix with the same paced reads as
/// any other local file work. Only needed after the in-memory state is lost.
fn prefix_hash(
    file: &mut File,
    length: u64,
    own: &Cancellation,
    parent: &Cancellation,
    limits: &Limits,
) -> Result<Sha256> {
    let mut hash = Sha256::new();
    let mut next_io = Instant::now();
    let _permit = limits.reserve(MAX_FRAME)?;
    let mut buffer = vec![0u8; CHUNK_BYTES];
    file.seek(SeekFrom::Start(0)).map_err(|_| Error::Io)?;
    let mut read = 0;
    while read < length {
        pace(next_io, own, parent)?;
        let n = CHUNK_BYTES.min((length - read) as usize);
        file.read_exact(&mut buffer[..n]).map_err(|_| Error::Io)?;
        hash.update(&buffer[..n]);
        read += n as u64;
        next_io = advance(next_io, n);
    }
    Ok(hash)
}
fn transfer(
    resource: Resource,
    path: PathBuf,
    mode: Mode,
    ops: Receiver<Op>,
    own: Cancellation,
    parent: Cancellation,
    limits: Limits,
    ready: oneshot::Sender<Result<u64>>,
    opening: Working,
    work: FileWork,
    resume: ResumeCache,
) -> Result<()> {
    let receive = !matches!(mode, Mode::Read { .. });
    let (start, end) = match mode {
        Mode::Read { start, end } => (start, end),
        Mode::Receive | Mode::Resume => (0, resource.size_bytes),
    };
    // A uuid partial is removed on every exit that did not hand it over. A
    // resumable partial is kept, except after its final hash check failed.
    let mut partial = None;
    let mut kept: Option<PathBuf> = None;
    let mut offset = start;
    let mut hash = Sha256::new();
    let name = resumable_partial(&resource);
    let opened = (|| {
        if stopped(&own, &parent) {
            return Err(Error::Cancelled);
        }
        if !receive {
            let mut file = open_source(&path, resource.size_bytes)?;
            file.seek(SeekFrom::Start(start)).map_err(|_| Error::Io)?;
            return Ok(file);
        }
        let meta = fs::symlink_metadata(&path).map_err(|_| Error::Io)?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(Error::InvalidResource);
        }
        if matches!(mode, Mode::Receive) {
            let target = path.join(format!(".receive-{}.partial", uuid::Uuid::new_v4()));
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&target)
                .map_err(|_| Error::Io)?;
            partial = Some(target);
            return Ok(file);
        }
        let target = path.join(&name);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&target)
            .map_err(|_| Error::Io)?;
        let meta = file.metadata().map_err(|_| Error::Io)?;
        if !meta.is_file() {
            return Err(Error::InvalidResource);
        }
        // Only slice ends were fsynced; a longer tail may not be durable.
        // A complete file still re-receives its last slice to be verified.
        let size = resource.size_bytes;
        let mut resumed = meta.len().min(size) / RESUME_SLICE * RESUME_SLICE;
        if resumed >= size {
            resumed = (size - 1) / RESUME_SLICE * RESUME_SLICE;
        }
        file.set_len(resumed).map_err(|_| Error::Io)?;
        let cached = resume
            .lock()
            .map_err(|_| Error::Runtime)?
            .take()
            .filter(|c| c.name == name && c.offset == resumed);
        hash = match cached {
            Some(c) => c.hash,
            None => prefix_hash(&mut file, resumed, &own, &parent, &limits)?,
        };
        file.seek(SeekFrom::Start(resumed)).map_err(|_| Error::Io)?;
        offset = resumed;
        kept = Some(target);
        Ok(file)
    })();
    let mut file = match opened {
        Ok(file) => {
            if ready.send(Ok(offset)).is_err() {
                if let Some(path) = partial {
                    let _ = fs::remove_file(path);
                }
                return Err(Error::Cancelled);
            }
            file
        }
        Err(error) => {
            let _ = ready.send(Err(error));
            return Err(error);
        }
    };
    // Keeping the file/permit open while waiting for the next network frame is
    // not an I/O operation. Only each queued read/write/fsync is counted below.
    drop(opening);
    let result = (|| {
        let mut next_io = Instant::now();
        loop {
            if stopped(&own, &parent) {
                return Err(Error::Cancelled);
            }
            let op = match ops.recv_timeout(IO_TICK) {
                Ok(op) => op,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => return Err(Error::Closed),
            };
            match op {
                Op::Read { done, _work } if !receive => {
                    let read = (|| {
                        pace(next_io, &own, &parent)?;
                        if offset == end {
                            if end == resource.size_bytes {
                                let mut extra = [0u8; 1];
                                if file.read(&mut extra).map_err(|_| Error::Io)? != 0
                                    || (start == 0
                                        && format!("{:x}", hash.clone().finalize())
                                            != resource.sha256)
                                {
                                    return Err(Error::Integrity);
                                }
                            }
                            return Ok(None);
                        }
                        let permit = limits.reserve(MAX_FRAME)?;
                        let n = CHUNK_BYTES.min((end - offset) as usize);
                        let mut bytes = BytesMut::zeroed(n);
                        file.read_exact(&mut bytes).map_err(|_| Error::Integrity)?;
                        hash.update(&bytes);
                        let chunk = Chunk {
                            bytes,
                            offset,
                            _bytes_permit: permit,
                        };
                        offset += n as u64;
                        // No accumulated burst after a stalled consumer.
                        next_io = advance(next_io, n);
                        Ok(Some(chunk))
                    })();
                    let terminal = !matches!(&read, Ok(Some(_)));
                    let failure = read.as_ref().err().copied();
                    let _ = done.send(read);
                    if terminal {
                        return failure.map_or(Ok(()), Err);
                    }
                }
                Op::Write {
                    offset: expected,
                    bytes,
                    permit,
                    done,
                    _work,
                } if receive => {
                    let wrote = (|| {
                        if expected != offset
                            || bytes.is_empty()
                            || bytes.len() > CHUNK_BYTES
                            || offset
                                .checked_add(bytes.len() as u64)
                                .is_none_or(|end| end > resource.size_bytes)
                        {
                            return Err(Error::Protocol);
                        }
                        pace(next_io, &own, &parent)?;
                        file.write_all(&bytes).map_err(|_| Error::Io)?;
                        hash.update(&bytes);
                        offset += bytes.len() as u64;
                        next_io = advance(next_io, bytes.len());
                        Ok(offset)
                    })();
                    drop(bytes);
                    drop(permit); // Free handoff before authorizing the next frame.
                    let failure = wrote.as_ref().err().copied();
                    let _ = done.send(wrote);
                    if let Some(error) = failure {
                        return Err(error);
                    }
                }
                Op::Checkpoint { done, _work } if kept.is_some() => {
                    let synced = file.sync_all().map_err(|_| Error::Io).and_then(|()| {
                        *resume.lock().map_err(|_| Error::Runtime)? = Some(ResumeHash {
                            name: name.clone(),
                            offset,
                            hash: hash.clone(),
                        });
                        Ok(offset)
                    });
                    let failure = synced.as_ref().err().copied();
                    let _ = done.send(synced);
                    if let Some(error) = failure {
                        return Err(error);
                    }
                }
                Op::Finish {
                    hash: claimed,
                    io,
                    permit,
                    done,
                    _work,
                } if receive => {
                    let mismatch = offset != resource.size_bytes
                        || claimed != resource.sha256
                        || format!("{:x}", hash.clone().finalize()) != resource.sha256;
                    let finished = (|| {
                        if mismatch {
                            return Err(Error::Integrity);
                        }
                        if stopped(&own, &parent) {
                            return Err(Error::Cancelled);
                        }
                        file.sync_all().map_err(|_| Error::Io)?;
                        if stopped(&own, &parent) {
                            return Err(Error::Cancelled);
                        }
                        Ok(VerifiedFile {
                            verified_stamp: FileStamp::of(&file.metadata().map_err(|_| Error::Io)?),
                            path: Some(
                                partial.as_ref().or(kept.as_ref()).ok_or(Error::Io)?.clone(),
                            ),
                            resource: resource.clone(),
                            io,
                            permit: Some(permit),
                            keep: kept.is_some(),
                        })
                    })();
                    let success = finished.is_ok();
                    if success {
                        partial.take(); // Ownership, including a dropped receiver, is in VerifiedFile.
                        let _ = done.send(finished);
                        return Ok(());
                    }
                    if mismatch && kept.is_some() {
                        // Bytes that do not hash to the offer are not progress.
                        partial = kept.take();
                    }
                    let _ = done.send(finished);
                    return Err(Error::Cancelled);
                }
                _ => return Err(Error::Protocol),
            }
        }
    })();
    drop(file);
    if let Some(path) = partial {
        let _cleanup = work.begin();
        let _ = fs::remove_file(path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bbs_sync::raw_sha256;
    mod work;
    struct Root(PathBuf);
    impl Root {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("bbs-transfer-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Root {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn resource(bytes: &[u8]) -> Resource {
        let hash = raw_sha256(bytes);
        Resource {
            identity: ResourceKind::Post {
                thread_id: "thread-test".into(),
                post_id: "post-test".into(),
                version_id: hash.clone(),
            },
            sha256: hash,
            size_bytes: bytes.len() as u64,
        }
    }
    fn attachment(bytes: &[u8]) -> Resource {
        let hash = raw_sha256(bytes);
        Resource {
            identity: ResourceKind::Attachment {
                thread_id: "thread-test".into(),
                post_id: "post-test".into(),
                version_id: "a".repeat(64),
                attachment_id: "file".into(),
            },
            sha256: hash,
            size_bytes: bytes.len() as u64,
        }
    }
    async fn write_from(writer: &mut FileWriter, bytes: &[u8], mut at: u64) -> u64 {
        for chunk in bytes[at as usize..].chunks(CHUNK_BYTES) {
            at = writer.write(at, BytesMut::from(chunk)).await.unwrap();
        }
        at
    }
    #[tokio::test]
    async fn resume_cuts_an_unsynced_tail_and_restores_the_prefix_hash_from_disk() {
        let root = Root::new();
        let bytes: Vec<u8> = (0..2 * RESUME_SLICE + 5000).map(|i| (i % 253) as u8).collect();
        let resource = attachment(&bytes);
        let partial = root.0.join(resumable_partial(&resource));
        // A crash after the first slice fsync but before the second finished.
        fs::write(&partial, &bytes[..(RESUME_SLICE + 777) as usize]).unwrap();
        let io = FileIo::start(Limits::default()).unwrap();
        let mut writer = io.resume(resource.clone(), &root.0, &Cancellation::default()).await.unwrap();
        assert_eq!(writer.start(), RESUME_SLICE);
        assert_eq!(fs::metadata(&partial).unwrap().len(), RESUME_SLICE);
        // No in-memory state exists in this new worker: the hash is re-read.
        assert_eq!(write_from(&mut writer, &bytes, RESUME_SLICE).await, bytes.len() as u64);
        let verified = writer.finish(resource.sha256.clone()).await.unwrap();
        assert_eq!(fs::read(verified.path()).unwrap(), bytes);
        drop(verified);
        assert!(partial.exists(), "an uninstalled verified partial stays for retry");
        io.stop_and_wait().await;
    }
    #[tokio::test]
    async fn slice_checkpoint_survives_a_dropped_writer_and_a_bad_final_hash_removes_it() {
        let root = Root::new();
        let bytes: Vec<u8> = (0..RESUME_SLICE + 99).map(|i| (i % 241) as u8).collect();
        let resource = attachment(&bytes);
        let partial = root.0.join(resumable_partial(&resource));
        let io = FileIo::start(Limits::default()).unwrap();
        let mut writer = io.resume(resource.clone(), &root.0, &Cancellation::default()).await.unwrap();
        assert_eq!(writer.start(), 0);
        let mut at = 0;
        for chunk in bytes[..RESUME_SLICE as usize].chunks(CHUNK_BYTES) {
            at = writer.write(at, BytesMut::from(chunk)).await.unwrap();
        }
        assert_eq!(writer.checkpoint().await.unwrap(), RESUME_SLICE);
        drop(writer);
        for _ in 0..50 {
            if fs::metadata(&partial).is_ok_and(|m| m.len() == RESUME_SLICE) { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(fs::metadata(&partial).unwrap().len(), RESUME_SLICE, "lost link keeps progress");
        // Resume from the in-memory hash, then deliver a corrupted tail.
        let mut writer = io.resume(resource.clone(), &root.0, &Cancellation::default()).await.unwrap();
        assert_eq!(writer.start(), RESUME_SLICE);
        let mut tail = bytes[RESUME_SLICE as usize..].to_vec();
        tail[0] ^= 1;
        writer.write(RESUME_SLICE, BytesMut::from(&tail[..])).await.unwrap();
        assert_eq!(writer.finish(resource.sha256.clone()).await.err(), Some(Error::Integrity));
        cleaned(&root.0).await;
        io.stop_and_wait().await;
    }
    #[test]
    fn recovery_keeps_recent_resumable_partials_and_ignores_other_names() {
        let name = resumable_partial_name(&"b".repeat(64), 42);
        let now = std::time::SystemTime::now();
        assert_eq!(stale_resumable_partial(&name, now), Some(false));
        assert_eq!(stale_resumable_partial(&name, now - RESUME_MAX_AGE - Duration::from_secs(1)), Some(true));
        assert_eq!(stale_resumable_partial(&format!(".receive-{}.partial", uuid::Uuid::new_v4()), now), None);
        assert_eq!(stale_resumable_partial("user-recovery.partial", now), None);
    }
    async fn cleaned(root: &Path) {
        for _ in 0..50 {
            if fs::read_dir(root).unwrap().count() == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("partial survived cancellation/error");
    }
    #[tokio::test(flavor = "current_thread")]
    async fn round_metadata_wait_is_cancellable_bounded_and_keeps_the_single_file_limit() {
        let io = FileIo::start(Limits::default()).unwrap();
        let held = io.limits.file().unwrap();
        let cancel = Cancellation::default();
        let files = io.clone();
        let stop = cancel.clone();
        let waiting = tokio::spawn(async move {
            files.run_in_round(&stop, |_| -> () { panic!("cancelled work must not run") }).await
        });
        tokio::task::yield_now().await;
        cancel.cancel();
        assert_eq!(waiting.await.unwrap(), Err(Error::Cancelled));
        assert_eq!(io.run(&Cancellation::default(), |_| ()).await, Err(Error::Busy));
        let timeout = io.run_in_round(&Cancellation::default(), |_| ()).await;
        assert_eq!(timeout, Err(Error::Timeout));
        drop(held);
        assert_eq!(io.run_in_round(&Cancellation::default(), |_| 7).await, Ok(7));
        io.stop_and_wait().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn file_worker_streams_hashes_and_syncs_before_exposing_private_file() {
        let root = Root::new();
        let bytes = vec![7u8; CHUNK_BYTES * 3 + 23];
        let spec = resource(&bytes);
        let source = root.0.join("source");
        fs::write(&source, &bytes).unwrap();
        let io = FileIo::start(Limits::default()).unwrap();
        let cancel = Cancellation::default();
        let mut reader = io.read(spec.clone(), &source, &cancel).await.unwrap();
        let mut read = Vec::new();
        while let Some(chunk) = reader.next().await.unwrap() {
            assert_eq!(chunk.offset, read.len() as u64);
            read.extend_from_slice(&chunk.bytes);
        }
        assert_eq!(read, bytes);
        drop(reader);
        let mut writer = io.receive(spec.clone(), &root.0, &cancel).await.unwrap();
        let mut offset = 0;
        for chunk in bytes.chunks(CHUNK_BYTES) {
            offset = writer.write(offset, BytesMut::from(chunk)).await.unwrap();
        }
        let file = writer.finish(spec.sha256.clone()).await.unwrap();
        assert_eq!(fs::read(file.path()).unwrap(), bytes);
        assert_eq!(
            fs::metadata(file.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::rename(file.path(), root.0.join("installed")).unwrap();
        file.installed();
    }
    #[tokio::test(flavor = "current_thread")]
    async fn account_file_limit_and_cancel_remove_partial_without_waiting_for_more_input() {
        let root = Root::new();
        let io = FileIo::start(Limits::default()).unwrap();
        let spec = resource(&[1; 64]);
        let cancel = Cancellation::default();
        let mut writer = io.receive(spec.clone(), &root.0, &cancel).await.unwrap();
        assert!(matches!(
            io.receive(spec, &root.0, &Cancellation::default()).await,
            Err(Error::Busy)
        ));
        writer
            .write(0, BytesMut::from(&[1u8; 32][..]))
            .await
            .unwrap();
        cancel.cancel();
        drop(writer);
        cleaned(&root.0).await;
    }
    #[tokio::test(flavor = "current_thread")]
    async fn invalid_offset_size_hash_and_source_symlink_never_yield_verified_files() {
        for fault in 0..3 {
            let root = Root::new();
            let io = FileIo::start(Limits::default()).unwrap();
            let spec = resource(&[1; 32]);
            let mut writer = io
                .receive(spec.clone(), &root.0, &Cancellation::default())
                .await
                .unwrap();
            match fault {
                0 => assert_eq!(
                    writer.write(1, BytesMut::from(&[1u8; 32][..])).await,
                    Err(Error::Protocol)
                ),
                1 => assert_eq!(
                    writer.write(0, BytesMut::from(&[1u8; 33][..])).await,
                    Err(Error::Protocol)
                ),
                _ => {
                    writer
                        .write(0, BytesMut::from(&[2u8; 32][..]))
                        .await
                        .unwrap();
                    assert!(matches!(
                        writer.finish(spec.sha256).await,
                        Err(Error::Integrity)
                    ));
                    cleaned(&root.0).await;
                    continue;
                }
            }
            drop(writer);
            cleaned(&root.0).await;
        }
        let root = Root::new();
        let source = root.0.join("source");
        fs::write(&source, [1u8; 32]).unwrap();
        std::os::unix::fs::symlink(&source, root.0.join("link")).unwrap();
        let io = FileIo::start(Limits::default()).unwrap();
        assert!(io
            .read(
                resource(&[1; 32]),
                &root.0.join("link"),
                &Cancellation::default()
            )
            .await
            .is_err());
    }
}
