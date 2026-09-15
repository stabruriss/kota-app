use super::*;
use crate::bbs_sync::transport::{Cancellation, FileIo};
use ::notify::{Event, EventKind, RecursiveMode, Watcher};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, OnceLock, Weak,
    },
};
use tokio::{
    sync::Notify,
    task::{JoinHandle, JoinSet},
};

const BATCH: usize = 64;
const HINT_BUDGET: usize = 256;
const WORKERS: usize = 2;

#[derive(Default)]
struct PendingHints {
    keys: BTreeSet<String>,
    rescan: bool,
    rewatch: bool,
}
#[derive(Default)]
struct Hints {
    pending: Mutex<PendingHints>,
    wake: Notify,
}
impl Hints {
    fn key(&self, key: String) {
        let mut pending = self.pending.lock().unwrap();
        if pending.keys.len() < HINT_BUDGET {
            pending.keys.insert(key);
        } else {
            pending.rescan = true;
        }
        drop(pending);
        self.wake.notify_one();
    }
    fn rescan(&self, rewatch: bool) {
        let mut pending = self.pending.lock().unwrap();
        pending.rescan = true;
        pending.rewatch |= rewatch;
        drop(pending);
        self.wake.notify_one();
    }
    fn take(&self) -> PendingHints {
        std::mem::take(&mut *self.pending.lock().unwrap())
    }
}

struct Inner {
    queue: store::Queue,
    stop: Cancellation,
    hints: Arc<Hints>,
    started: AtomicBool,
    waiting: Mutex<HashMap<String, (String, Arc<Vec<WaitingAttachment>>)>>,
    attachment_generation: AtomicU64,
    finished: AtomicBool,
    finish_hint: Notify,
    #[cfg(test)]
    scans: AtomicU64,
}
#[derive(Clone)]
pub(crate) struct Service(Arc<Inner>);
static SERVICES: OnceLock<Mutex<Vec<Weak<Inner>>>> = OnceLock::new();

/// Called only after the caller saved the state it already loaded. No file
/// scans/hashes, state clone, new timer, or second authoritative ledger.
pub(in crate::bbs) fn attachments_saved(
    content: &sync::ContentStore,
    thread: &str,
    state: &SyncState,
) {
    let Some(services) = SERVICES.get() else {
        return;
    };
    let mut services = services.lock().unwrap();
    services.retain(|service| service.strong_count() > 0);
    for inner in services.iter().filter_map(Weak::upgrade) {
        if inner.queue.content.state.state_path() != content.state.state_path() {
            continue;
        }
        inner.attachment_generation.fetch_add(1, Ordering::AcqRel);
        let waiting = inner.waiting.lock().unwrap();
        for (key, (root, refs)) in waiting.iter() {
            if root == thread && refs.iter().all(|r| r.verified(state)) {
                inner.hints.key(key.clone());
            }
        }
    }
}

impl Service {
    pub(crate) fn account() -> Self {
        Self::at(sync::ContentStore::account())
    }
    pub(super) fn at(content: sync::ContentStore) -> Self {
        Self(Arc::new(Inner {
            queue: store::Queue::new(&content),
            stop: Cancellation::default(),
            hints: Arc::new(Hints::default()),
            started: AtomicBool::new(false),
            waiting: Mutex::new(HashMap::new()),
            attachment_generation: AtomicU64::new(0),
            finished: AtomicBool::new(false),
            finish_hint: Notify::new(),
            #[cfg(test)]
            scans: AtomicU64::new(0),
        }))
    }
    pub(crate) fn start(&self, app: tauri::AppHandle, manager: crate::bbs_sync::manager::Manager) {
        let this = self.clone();
        tauri::async_runtime::spawn(async move {
            match manager.notification_files() {
                Ok(files) => this.start_with(files, Arc::new(delivery::AppSink(app))),
                Err(_) => diagnostic("file_service_unavailable"),
            }
        });
    }
    pub(super) fn start_with(&self, files: FileIo, sink: Arc<dyn delivery::Sink>) {
        if self.0.started.swap(true, Ordering::AcqRel) {
            return;
        }
        SERVICES
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .push(Arc::downgrade(&self.0));
        let this = self.0.clone();
        tauri::async_runtime::spawn(async move {
            if run(this.clone(), files, sink).await.is_err() {
                diagnostic("service_stopped_after_error");
            }
            this.finished.store(true, Ordering::Release);
            this.finish_hint.notify_waiters();
        });
    }
    pub(crate) fn shutdown(&self) {
        self.0.stop.cancel();
        self.0.hints.wake.notify_waiters();
    }
    #[cfg(test)]
    pub(super) async fn close(&self) {
        self.shutdown();
        while !self.0.finished.load(Ordering::Acquire) {
            let notified = self.0.finish_hint.notified();
            if self.0.finished.load(Ordering::Acquire) {
                break;
            }
            notified.await;
        }
    }
    #[cfg(test)]
    pub(super) fn scan_count(&self) -> u64 {
        self.0.scans.load(Ordering::Acquire)
    }
}

fn watch(queue: &store::Queue, hints: Arc<Hints>) -> Result<::notify::RecommendedWatcher> {
    // FSEvents reports canonical paths (/private/var for /var). Subscribe
    // and classify in the same spelling, including an aliased account root.
    let roots = [
        queue.directory("pending")?.canonicalize()?,
        queue.directory("processing")?.canonicalize()?,
    ];
    let watched = roots.clone();
    let mut watcher = ::notify::recommended_watcher(move |event: ::notify::Result<Event>| {
        let event = match event {
            Ok(event) => event,
            Err(_) => {
                hints.rescan(true);
                return;
            }
        };
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        if event.need_rescan() {
            hints.rescan(false);
            return;
        }
        for path in event.paths {
            if watched.contains(&path)
                && matches!(
                    event.kind,
                    EventKind::Remove(_) | EventKind::Modify(::notify::event::ModifyKind::Name(_))
                )
            {
                hints.rescan(true);
                continue;
            }
            if !watched
                .iter()
                .any(|root| path.parent() == Some(root.as_path()))
            {
                continue;
            }
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            if let Some(key) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .filter(|s| bbs_sync::valid_hash(s).is_ok())
            {
                hints.key(key.into());
            }
        }
    })?;
    for root in roots {
        watcher.watch(&root, RecursiveMode::NonRecursive)?;
    }
    Ok(watcher)
}
struct Scan {
    directories: Vec<fs::ReadDir>,
}
impl Scan {
    fn new(queue: &store::Queue) -> Result<Self> {
        Ok(Self {
            directories: vec![
                fs::read_dir(queue.directory("processing")?)?,
                fs::read_dir(queue.directory("pending")?)?,
            ],
        })
    }
    fn next(mut self) -> Result<(Option<Self>, Vec<String>)> {
        let mut keys = Vec::new();
        // Count entries examined, not only accepted JSON files. A directory of
        // temporary/recovery files must also yield after a bounded batch.
        for _ in 0..BATCH {
            let Some(directory) = self.directories.last_mut() else {
                return Ok((None, keys));
            };
            let Some(entry) = directory.next() else {
                self.directories.pop();
                continue;
            };
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name();
            let Some(key) = name.to_str().and_then(|s| s.strip_suffix(".json")) else {
                continue;
            };
            if bbs_sync::valid_hash(key).is_ok() {
                keys.push(key.into());
            }
        }
        Ok((Some(self), keys))
    }
}
async fn optional<T>(
    task: &mut Option<JoinHandle<T>>,
) -> std::result::Result<T, tokio::task::JoinError> {
    match task {
        Some(task) => task.await,
        None => std::future::pending().await,
    }
}
enum Work {
    Done,
    Wait { at: i64, record: Record },
    Recover,
}

async fn run(inner: Arc<Inner>, files: FileIo, sink: Arc<dyn delivery::Sink>) -> Result<()> {
    let queue = inner.queue.clone();
    let hints = inner.hints.clone();
    let (mut watcher, mut scan) = tokio::task::spawn_blocking(move || -> Result<_> {
        queue.initialize()?;
        // Register before the first enumeration, including identity-free use.
        let watcher = watch(&queue, hints).ok();
        Ok((watcher, Some(Scan::new(&queue)?)))
    })
    .await??;
    #[cfg(test)]
    inner.scans.fetch_add(1, Ordering::AcqRel);
    let run_id = Uuid::new_v4().to_string();
    let mut waiting = BTreeMap::<String, i64>::new();
    let mut deadlines = BTreeSet::<(i64, String)>::new();
    let mut todo = BTreeSet::<String>::new();
    let mut in_flight = HashSet::<String>::new();
    let mut changed_during_check = HashSet::<String>::new();
    let mut jobs = JoinSet::new();
    let mut scan_job = None;
    let mut recovery_job = None;
    let mut recover = BTreeSet::<String>::new();
    let mut needs_scan = false;
    let mut retry_watch = if watcher.is_some() {
        None
    } else {
        Some(now_ms() + 2_000)
    };
    let mut watch_job = None;
    let mut retry_delay = 2_000i64;
    loop {
        if inner.stop.is_cancelled() {
            break;
        }
        let hints = inner.hints.take();
        needs_scan |= hints.rescan;
        if hints.rewatch {
            retry_watch = Some(now_ms());
        }
        for key in hints.keys {
            if let Some(at) = waiting.remove(&key) {
                deadlines.remove(&(at, key.clone()));
            }
            if in_flight.contains(&key) {
                changed_during_check.insert(key);
            } else {
                todo.insert(key);
            }
        }
        let now = now_ms();
        while deadlines.first().is_some_and(|(at, _)| *at <= now) {
            let (_, key) = deadlines.pop_first().unwrap();
            waiting.remove(&key);
            if !in_flight.contains(&key) {
                todo.insert(key);
            }
        }
        while jobs.len() < WORKERS {
            let Some(key) = todo.pop_first() else { break };
            if let Some(at) = waiting.remove(&key) {
                deadlines.remove(&(at, key.clone()));
            }
            inner.waiting.lock().unwrap().remove(&key);
            let q = inner.queue.clone();
            let run = run_id.clone();
            let sink = sink.clone();
            let stop = inner.stop.clone();
            let source = inner.clone();
            in_flight.insert(key.clone());
            jobs.spawn_blocking(move || {
                let observed = source.attachment_generation.load(Ordering::Acquire);
                let result = (|| -> Result<Work> {
                    if stop.is_cancelled() {
                        return Ok(Work::Done);
                    }
                    match q.check(&key, &run, now_ms())? {
                        store::Check::Done => Ok(Work::Done),
                        store::Check::Recover => Ok(Work::Recover),
                        store::Check::Wait { at, record } => Ok(Work::Wait { at, record }),
                        store::Check::Deliver(claim) => {
                            if !stop.is_cancelled() {
                                delivery::dispatch(&q, claim, sink.as_ref())?;
                            }
                            Ok(Work::Done)
                        }
                    }
                })();
                (key, result, observed)
            });
        }
        if recovery_job.is_none() {
            if let Some(key) = recover.pop_first() {
                let queue = inner.queue.clone();
                let files = files.clone();
                let stop = inner.stop.clone();
                recovery_job = Some(tokio::spawn(async move {
                    let recovery_key = key.clone();
                    let result = files
                        .run_when_available(&stop, move |io| queue.recover(&recovery_key, io))
                        .await;
                    (key, result)
                }));
            }
        }
        if scan_job.is_none() && todo.len() < BATCH {
            if let Some(batch) = scan.take() {
                scan_job = Some(tokio::task::spawn_blocking(move || batch.next()));
            } else if needs_scan {
                needs_scan = false;
                let q = inner.queue.clone();
                #[cfg(test)]
                inner.scans.fetch_add(1, Ordering::AcqRel);
                scan_job = Some(tokio::task::spawn_blocking(move || Scan::new(&q)?.next()));
            }
        }
        if watch_job.is_none() && retry_watch.is_some_and(|t| t <= now) {
            retry_watch = None;
            let q = inner.queue.clone();
            let hints = inner.hints.clone();
            watch_job = Some(tokio::task::spawn_blocking(move || {
                q.initialize()?;
                watch(&q, hints)
            }));
        }
        let next = deadlines
            .first()
            .map(|(at, _)| *at)
            .into_iter()
            .chain(retry_watch)
            .min();
        let delay = next.map(|t| Duration::from_millis(t.saturating_sub(now).max(0) as u64));
        tokio::select! {
            _ = inner.stop.cancelled() => break,
            _ = inner.hints.wake.notified() => {},
            _ = async { match delay { Some(d) => tokio::time::sleep(d).await, None => std::future::pending().await } } => {},
            done = jobs.join_next(), if !jobs.is_empty() => {
                match done {
                    Some(Ok((key, result, observed))) => {
                        in_flight.remove(&key);
                        // Publishing ready can race a check of awaiting_post.
                        // Keep a single follow-up hint for active work instead
                        // of losing the event until the ten-minute orphan time.
                        if changed_during_check.remove(&key) { todo.insert(key.clone()); }
                        match result {
                            Ok(Work::Wait { at, record }) => {
                                inner.waiting.lock().unwrap().insert(key.clone(), (record.destination.thread_id, Arc::new(record.waiting)));
                                // A verified attachment may have been saved
                                // after check read its ledger but before this
                                // subscription was installed. Close that gap
                                // with one event-driven recheck, not polling.
                                if inner.attachment_generation.load(Ordering::Acquire) != observed { inner.hints.key(key.clone()); }
                                if let Some(old) = waiting.insert(key.clone(), at) { deadlines.remove(&(old, key.clone())); }
                                deadlines.insert((at, key));
                            }
                            Ok(Work::Recover) => { recover.insert(key); }
                            Ok(Work::Done) => {},
                            Err(_) => diagnostic("notification_attempt_incomplete"),
                        }
                    }
                    _ => diagnostic("notification_worker_failed"),
                }
            },
            done = optional(&mut recovery_job) => {
                recovery_job = None;
                match done {
                    Ok((key, Ok(Ok(())))) => { todo.insert(key); }
                    _ => diagnostic("awaiting_post_recovery_failed"),
                }
            },
            done = optional(&mut scan_job) => {
                scan_job = None;
                match done {
                    Ok(Ok((rest, keys))) => {
                        scan = rest;
                        for key in keys { if !in_flight.contains(&key) { todo.insert(key); } }
                    }
                    _ => { retry_watch = Some(now_ms() + retry_delay); diagnostic("conditional_scan_failed"); }
                }
            },
            done = optional(&mut watch_job) => {
                watch_job = None; needs_scan = true;
                match done {
                    Ok(Ok(healthy)) => { watcher = Some(healthy); retry_delay = 2_000; },
                    _ => {
                        watcher = None; retry_watch = Some(now_ms() + retry_delay);
                        retry_delay = (retry_delay * 2).min(60_000);
                        diagnostic("watch_unavailable_conditional_rescan");
                    }
                }
            }
        }
    }
    drop(watcher);
    // Do not abort an accepted blocking Bus operation or erase its record.
    // Unclaimed work stays on disk; in-flight records recover on next startup.
    inner.waiting.lock().unwrap().clear();
    while jobs.join_next().await.is_some() {}
    if let Some(task) = recovery_job {
        let _ = task.await;
    }
    if let Some(task) = scan_job {
        let _ = task.await;
    }
    if let Some(task) = watch_job {
        let _ = task.await;
    }
    Ok(())
}
