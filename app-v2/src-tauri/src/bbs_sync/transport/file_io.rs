//! One bounded account file worker. All open/read/write/hash/fsync/cleanup runs
//! here, not on the async network executor. Callers only exchange one chunk.
use super::{background_thread, Cancellation, Error, Limits, Result, BYTES_PER_SECOND, MAX_FRAME};
use crate::bbs_sync::{safe_id, valid_hash, FileStamp};
use bytes::BytesMut;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use tokio::sync::{oneshot, OwnedSemaphorePermit};

pub(crate) const CHUNK_BYTES: usize = MAX_FRAME - 25;
const IO_TICK: Duration = Duration::from_millis(20);

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
    _bytes_permit: OwnedSemaphorePermit,
}
enum Op {
    Read(oneshot::Sender<Result<Option<Chunk>>>),
    Write {
        offset: u64,
        bytes: BytesMut,
        permit: OwnedSemaphorePermit,
        done: oneshot::Sender<Result<u64>>,
    },
    Finish {
        hash: String,
        io: FileIo,
        permit: OwnedSemaphorePermit,
        done: oneshot::Sender<Result<VerifiedFile>>,
    },
}
enum Job {
    Stop,
    Local(Box<dyn FnOnce() + Send>),
    Transfer {
        resource: Resource,
        path: PathBuf,
        receive: bool,
        ops: Receiver<Op>,
        own_cancel: Cancellation,
        parent_cancel: Cancellation,
        limits: Limits,
        ready: oneshot::Sender<Result<()>>,
    },
    Cleanup {
        path: PathBuf,
        _permit: OwnedSemaphorePermit,
    },
    RemoveSmall {
        path: PathBuf,
    },
}
#[derive(Clone)]
pub(crate) struct FileIo {
    jobs: SyncSender<Job>,
    limits: Limits,
    activity: Arc<AtomicU64>,
    closing: Arc<AtomicBool>,
    finished: tokio::sync::watch::Receiver<bool>,
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
                        Job::Cleanup { path, .. } | Job::RemoveSmall { path } => {
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
                            receive,
                            ops,
                            own_cancel,
                            parent_cancel,
                            limits,
                            ready,
                        } => {
                            let _ = transfer(
                                resource,
                                path,
                                receive,
                                ops,
                                own_cancel,
                                parent_cancel,
                                limits,
                                ready,
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
            closing: Arc::new(AtomicBool::new(false)),
            finished,
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
    /// Roster staging owns no transfer permit between pages. Dropping an RPC
    /// only schedules unlink; the network/IPC thread performs no filesystem I/O.
    pub(crate) fn remove_later(&self, path: PathBuf) {
        if self.jobs.try_send(Job::RemoveSmall { path }).is_err() {
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
        let permit = tokio::select! {
            _=parent.cancelled()=>return Err(Error::Cancelled),
            p=self.limits.files.clone().acquire_owned()=>p.map_err(|_|Error::Closed)?,
        };
        self.schedule(parent, move |io| {
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
        // Bound admission, not the whole scan/copy. Work that keeps making I/O
        // progress may legitimately exceed twenty seconds in a large library.
        let permit = tokio::select! {
            _=parent.cancelled()=>return Err(Error::Cancelled),
            p=tokio::time::timeout(super::PROGRESS_TIMEOUT, self.limits.files.clone().acquire_owned()) =>
                p.map_err(|_| Error::Timeout)?.map_err(|_| Error::Closed)?,
        };
        self.schedule(parent, move |io| {
            let _permit = permit;
            operation(io)
        }).await
    }
    /// Repository scans/hash/copies use the very same account file permit and
    /// background thread as network transfers. This starts no additional worker.
    pub(crate) async fn run<T, F>(&self, parent: &Cancellation, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut LocalIo) -> T + Send + 'static,
    {
        let permit = self.limits.file()?;
        self.schedule(parent, move |io| {
            let _permit = permit;
            operation(io)
        })
        .await
    }
    async fn schedule<T, F>(&self, parent: &Cancellation, operation: F) -> Result<T>
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
        receive: bool,
        parent: &Cancellation,
    ) -> Result<FileHandle> {
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
        };
        self.jobs
            .try_send(Job::Transfer {
                resource,
                path: path.to_owned(),
                receive,
                ops,
                own_cancel,
                parent_cancel: parent.clone(),
                limits: self.limits.clone(),
                ready,
            })
            .map_err(|_| Error::Busy)?;
        tokio::select! {
            result = initialized => result.map_err(|_| Error::Closed)??,
            _ = parent.cancelled() => return Err(Error::Cancelled),
        }
        Ok(handle)
    }
    /// Source paths must come from the registered BBS resource resolver (step4).
    /// No path is decoded from the peer's Resource value.
    pub(crate) async fn read(
        &self,
        resource: Resource,
        source: &Path,
        cancel: &Cancellation,
    ) -> Result<FileReader> {
        Ok(FileReader {
            handle: self.open(resource, source, false, cancel).await?,
        })
    }
    pub(crate) async fn receive(
        &self,
        resource: Resource,
        staging: &Path,
        cancel: &Cancellation,
    ) -> Result<FileWriter> {
        Ok(FileWriter {
            handle: self.open(resource, staging, true, cancel).await?,
            io: self.clone(),
        })
    }
}
struct FileHandle {
    tx: SyncSender<Op>,
    own_cancel: Cancellation,
    parent: Cancellation,
    permit: Option<OwnedSemaphorePermit>,
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
            .try_send(Op::Read(done))
            .map_err(|_| Error::Closed)?;
        self.handle.reply(rx).await
    }
}
pub(crate) struct FileWriter {
    handle: FileHandle,
    io: FileIo,
}
impl FileWriter {
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
            .schedule(cancel, move |io| {
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
    _permit: OwnedSemaphorePermit,
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
fn transfer(
    resource: Resource,
    path: PathBuf,
    receive: bool,
    ops: Receiver<Op>,
    own: Cancellation,
    parent: Cancellation,
    limits: Limits,
    ready: oneshot::Sender<Result<()>>,
) -> Result<()> {
    let mut partial = None;
    let opened = (|| {
        if stopped(&own, &parent) {
            return Err(Error::Cancelled);
        }
        if receive {
            let meta = fs::symlink_metadata(&path).map_err(|_| Error::Io)?;
            if !meta.is_dir() || meta.file_type().is_symlink() {
                return Err(Error::InvalidResource);
            }
            let target = path.join(format!(".receive-{}.partial", uuid::Uuid::new_v4()));
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&target)
                .map_err(|_| Error::Io)?;
            partial = Some(target);
            Ok(file)
        } else {
            open_source(&path, resource.size_bytes)
        }
    })();
    let mut file = match opened {
        Ok(file) => {
            if ready.send(Ok(())).is_err() {
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
    let result = (|| {
        let mut offset = 0u64;
        let mut hash = Sha256::new();
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
                Op::Read(done) if !receive => {
                    let read = (|| {
                        pace(next_io, &own, &parent)?;
                        if offset == resource.size_bytes {
                            let mut extra = [0u8; 1];
                            if file.read(&mut extra).map_err(|_| Error::Io)? != 0
                                || format!("{:x}", hash.clone().finalize()) != resource.sha256
                            {
                                return Err(Error::Integrity);
                            }
                            return Ok(None);
                        }
                        let permit = limits.reserve(MAX_FRAME)?;
                        let n = CHUNK_BYTES.min((resource.size_bytes - offset) as usize);
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
                Op::Finish {
                    hash: claimed,
                    io,
                    permit,
                    done,
                } if receive => {
                    let finished = (|| {
                        if offset != resource.size_bytes
                            || claimed != resource.sha256
                            || format!("{:x}", hash.clone().finalize()) != resource.sha256
                        {
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
                            path: Some(partial.as_ref().ok_or(Error::Io)?.clone()),
                            resource: resource.clone(),
                            io,
                            permit: Some(permit),
                        })
                    })();
                    let success = finished.is_ok();
                    if success {
                        partial.take(); // Ownership, including a dropped receiver, is in VerifiedFile.
                        let _ = done.send(finished);
                        return Ok(());
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
        let _ = fs::remove_file(path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bbs_sync::raw_sha256;
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
