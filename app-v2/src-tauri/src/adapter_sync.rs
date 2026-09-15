//! The single writer for generated agent context. Callbacks only mark work;
//! callers that need completion receive the result of their own ordered request.

use std::cell::Cell;
use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

mod files;
#[cfg(test)]
mod performance;
mod watch;
const CHECK_INTERVAL: Duration = Duration::from_secs(2);

type Request = Box<dyn FnOnce() + Send>;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct AgentKey {
    pub project: PathBuf,
    pub agent: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Task {
    Project(PathBuf),
    Skills(AgentKey),
}

struct Pending {
    reason: String,
    attempt: u8,
}

#[derive(Default)]
struct Queue {
    requests: VecDeque<Request>,
    tasks: VecDeque<Task>,
    dirty: BTreeMap<Task, Pending>,
    retries: BTreeMap<Task, (Instant, Pending)>,
    events: watch::Changes,
    next_check: Option<Instant>,
}

impl Queue {
    fn mark(&mut self, root: PathBuf, reason: String) {
        self.mark_task(Task::Project(root), reason);
    }

    fn mark_task(&mut self, task: Task, reason: String) {
        self.retries.remove(&task);
        self.push(task, Pending { reason, attempt: 0 });
    }

    fn push(&mut self, root: Task, pending: Pending) {
        if self.dirty.insert(root.clone(), pending).is_none() {
            self.tasks.push_back(root);
        }
    }

    fn schedule_retry(&mut self, root: Task, pending: Pending) {
        if pending.attempt < 2 && !self.dirty.contains_key(&root) {
            self.retries.insert(
                root,
                (
                    Instant::now() + CHECK_INTERVAL,
                    Pending {
                        attempt: pending.attempt + 1,
                        ..pending
                    },
                ),
            );
        }
    }

    fn promote_retries(&mut self, now: Instant) {
        let due = self
            .retries
            .iter()
            .filter(|(_, (at, _))| *at <= now)
            .map(|(root, _)| root.clone())
            .collect::<Vec<_>>();
        for root in due {
            if let Some((_, pending)) = self.retries.remove(&root) {
                self.push(root, pending);
            }
        }
    }

    fn take_project(&mut self, root: &Path) -> Option<Pending> {
        let task = Task::Project(root.to_path_buf());
        self.retries.remove(&task);
        let reason = self.dirty.remove(&task)?;
        self.tasks.retain(|key| key != &task);
        Some(reason)
    }
}

#[derive(Default)]
struct Worker {
    queue: Mutex<Queue>,
    ready: Condvar,
}

thread_local! {
    static ON_WORKER: Cell<bool> = const { Cell::new(false) };
    #[cfg(test)]
    static REQUEST_COUNT: Cell<usize> = const { Cell::new(0) };
}

fn worker() -> &'static Arc<Worker> {
    static INSTANCE: OnceLock<Arc<Worker>> = OnceLock::new();
    INSTANCE.get_or_init(|| {
        let worker = Arc::new(Worker::default());
        let background = Arc::clone(&worker);
        std::thread::Builder::new()
            .name("kota-adapter-sync".into())
            .spawn(move || background.run())
            .expect("start adapter sync worker");
        worker
    })
}

impl Worker {
    fn run(&self) {
        ON_WORKER.with(|flag| flag.set(true));
        loop {
            let request = {
                let mut queue = self.queue.lock().expect("adapter sync queue poisoned");
                loop {
                    // Receipt-bearing work goes before ordinary broadcasts.
                    if let Some(request) = queue.requests.pop_front() {
                        break request;
                    }
                    let now = Instant::now();
                    queue.promote_retries(now);
                    if queue.next_check.is_some_and(|due| due <= now) {
                        queue.next_check = Some(now + CHECK_INTERVAL);
                        break Box::new(|| {
                            watch::maintenance();
                            files::check();
                        }) as Request;
                    }
                    if !queue.events.is_empty() {
                        let events = std::mem::take(&mut queue.events);
                        break Box::new(move || watch::process(events)) as Request;
                    }
                    if let Some(root) = queue.tasks.pop_front() {
                        let pending = queue.dirty.remove(&root).expect("queued project");
                        break Box::new(move || {
                            let result = match &root {
                                Task::Project(project) => {
                                    crate::sync_project_adapters_once(project).into_result()
                                }
                                Task::Skills(key) => crate::sync_agent_skills(key),
                            };
                            if let Err(error) = result {
                                crate::kota_debug_log(&format!(
                                    "[adapter-sync] task={:?} reason={} attempt={} error={error}",
                                    root,
                                    pending.reason,
                                    pending.attempt + 1
                                ));
                                worker().queue.lock().unwrap().schedule_retry(root, pending);
                            }
                        }) as Request;
                    }
                    let deadline = queue
                        .next_check
                        .into_iter()
                        .chain(queue.retries.values().map(|(due, _)| *due))
                        .min();
                    queue = if let Some(deadline) = deadline {
                        self.ready
                            .wait_timeout(queue, deadline.saturating_duration_since(Instant::now()))
                            .expect("adapter sync queue poisoned")
                            .0
                    } else {
                        self.ready.wait(queue).expect("adapter sync queue poisoned")
                    };
                }
            };
            // No queue/registry/UI lock is held during user work or disk I/O.
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(request)).is_err() {
                crate::kota_debug_log("[adapter-sync] request panicked; worker continuing");
            }
        }
    }
}

fn enqueue(request: Request) {
    #[cfg(test)]
    REQUEST_COUNT.with(|count| count.set(count.get() + 1));
    let worker = worker();
    worker
        .queue
        .lock()
        .expect("adapter sync queue poisoned")
        .requests
        .push_back(request);
    worker.ready.notify_one();
}

pub(crate) fn on_worker() -> bool {
    ON_WORKER.with(Cell::get)
}

/// For callers already on a blocking/background thread. Nested helpers execute
/// inline on the worker, never enqueue and wait for the worker itself.
pub(crate) fn call<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    if on_worker() {
        return work();
    }
    let (send, receive) = mpsc::sync_channel(1);
    enqueue(Box::new(move || {
        let _ = send.send(work());
    }));
    receive
        .recv()
        .map_err(|_| "adapter sync request interrupted".to_string())?
}

/// Enqueue before awaiting. Saving source files belongs inside `work` too,
/// so multiple saves cannot finish their writes in reverse order.
pub(crate) fn submit<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> impl Future<Output = Result<T, String>> {
    let (send, mut receive) = tauri::async_runtime::channel(1);
    enqueue(Box::new(move || {
        let _ = send.try_send(work());
    }));
    async move {
        receive
            .recv()
            .await
            .unwrap_or_else(|| Err("adapter sync request interrupted".into()))
    }
}

pub(crate) fn mark_project(root: PathBuf, reason: impl Into<String>) {
    let worker = worker();
    worker
        .queue
        .lock()
        .expect("adapter sync queue poisoned")
        .mark(root, reason.into());
    worker.ready.notify_one();
}

pub(crate) fn take_project(root: &Path) -> bool {
    worker()
        .queue
        .lock()
        .expect("adapter sync queue poisoned")
        .take_project(root)
        .is_some()
}

/// App setup only schedules initialization; all subscriptions and scans happen
/// on the worker. Only the active project's adapters are eagerly rebuilt.
pub(crate) fn start_watchers() {
    enqueue(Box::new(|| {
        if let Err(error) = watch::start() {
            crate::kota_debug_log(&format!("[adapter-sync] watcher setup: {error}"));
        }
        if let Err(error) = files::start() {
            crate::kota_debug_log(&format!("[adapter-sync] config setup: {error}"));
        }
        crate::bbs_sync::roster::runtime::local_changed();
        worker().queue.lock().unwrap().next_check = Some(Instant::now() + CHECK_INTERVAL);
    }));
}

pub(crate) fn register_project(root: &Path) {
    debug_assert!(on_worker());
    watch::register_project(root);
    files::register_project(root);
}

pub(crate) fn register_roster_images(paths: Vec<PathBuf>) {
    enqueue(Box::new(move || files::register_roster_images(paths)));
}
/// One persistence/lifecycle hook, not a list maintained by every UI command.
pub(crate) fn roster_project_saved(root: PathBuf) {
    if !crate::bbs_sync::roster::runtime::active() {
        return;
    }
    enqueue(Box::new(move || {
        files::register_project(&root);
        crate::bbs_sync::roster::runtime::local_changed();
    }));
}

pub(crate) fn mark_skills(key: AgentKey, reason: impl Into<String>) {
    let worker = worker();
    worker
        .queue
        .lock()
        .unwrap()
        .mark_task(Task::Skills(key), reason.into());
    worker.ready.notify_one();
}

pub(crate) fn project_catalog() -> Result<Vec<PathBuf>, String> {
    files::projects()
        .map(Ok)
        .unwrap_or_else(crate::registered_adapter_projects)
}

pub(crate) fn account_identity_was_present() -> bool {
    files::account_identity_was_present()
}

pub(crate) fn forget_agent(key: &AgentKey) {
    debug_assert!(on_worker());
    files::forget_agent(key);
    let task = Task::Skills(key.clone());
    let mut queue = worker().queue.lock().unwrap();
    queue.tasks.retain(|queued| queued != &task);
    queue.dirty.remove(&task);
    queue.retries.remove(&task);
}

/// Called on a blocking caller thread. Only catalog/queue cleanup runs on the
/// sync worker; deleting a worktree must not delay other projects' receipts.
pub(crate) fn remove_after_forgetting<T>(
    project: PathBuf,
    agent: Option<String>,
    remove: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    debug_assert!(!on_worker());
    let restore_root = project.clone();
    let forget = move || {
        if let Some(agent) = &agent {
            forget_agent(&AgentKey { project: project.clone(), agent: agent.clone() });
        } else {
            forget_project(&project);
        }
        Ok(())
    };
    call(forget.clone())?;
    let result = remove();
    // A full check while deletion was underway may have rediscovered entries.
    let _ = call(forget);
    if result.is_err() {
        mark_project(restore_root, "restore registration after failed removal");
    }
    result
}

pub(crate) fn forget_project(root: &Path) {
    debug_assert!(on_worker());
    files::forget_project(root);
    watch::forget_project(root);
    let belongs = |task: &Task| match task {
        Task::Project(project) => project == root,
        Task::Skills(key) => key.project == root,
    };
    let mut queue = worker().queue.lock().unwrap();
    queue.tasks.retain(|task| !belongs(task));
    queue.dirty.retain(|task, _| !belongs(task));
    queue.retries.retain(|task, _| !belongs(task));
    queue.events.sources.retain(
        |source| !matches!(source, watch::Source::ProjectRules(project) if project == root),
    );
}

pub(crate) fn refresh_skill_pool_watch() {
    debug_assert!(on_worker());
    watch::register_skill_pool();
}

pub(crate) fn pool_changed(ids: Option<std::collections::BTreeSet<String>>) {
    debug_assert!(on_worker());
    match crate::skill_pool_referrers(ids.as_ref()) {
        Ok(references) => {
            for error in references.errors {
                crate::kota_debug_log(&format!("[adapter-sync] skill references: {error}"));
            }
            for key in references.agents {
                mark_skills(key, "skill pool change");
            }
        }
        Err(error) => crate::kota_debug_log(&format!("[adapter-sync] skill pool: {error}")),
    }
}

fn mark_rule_events(changes: watch::Changes) {
    let worker = worker();
    worker.queue.lock().unwrap().events.merge(changes);
    worker.ready.notify_one();
}

pub(crate) fn process_pending_events() {
    debug_assert!(on_worker());
    let events = std::mem::take(&mut worker().queue.lock().unwrap().events);
    if !events.is_empty() {
        watch::process(events);
    }
}

/// The adapter is also the external Ghost source. Recompile from the latest
/// contents when a concurrent edit is observed; repeated conflict preserves it.
/// The final read-to-rename interval remains optimistic, not filesystem CAS.
pub(crate) fn update_adapter(
    path: &Path,
    mut compile: impl FnMut(Option<&str>) -> Result<String, String>,
) -> Result<bool, String> {
    let mut observed = crate::read_optional_adapter(path)?;
    for _ in 0..3 {
        let next = compile(observed.as_deref())?;
        if observed.as_deref() == Some(next.as_str()) {
            return Ok(false);
        }
        let latest = crate::read_optional_adapter(path)?;
        if latest != observed {
            observed = latest;
            continue;
        }
        atomic_replace(path, next.as_bytes())?;
        return Ok(true);
    }
    Err(format!(
        "adapter changed during compilation; preserved {}, retry sync",
        path.display()
    ))
}

/// Write full files atomically, retaining permissions and avoiding unchanged
/// writes. This guarantees complete reads, not durable storage or filesystem CAS.
pub(crate) fn write_if_changed(path: &Path, content: &[u8]) -> Result<bool, String> {
    match std::fs::read(path) {
        Ok(previous) if previous == content => return Ok(false),
        Ok(_) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    }
    atomic_replace(path, content)?;
    Ok(true)
}

pub(crate) fn atomic_replace(path: &Path, content: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| format!("no parent: {}", path.display()))?;
    let temporary = parent.join(format!(".kota-adapter-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("create {}: {error}", temporary.display()))?;
        if let Ok(metadata) = std::fs::metadata(path) {
            file.set_permissions(metadata.permissions())
                .map_err(|error| error.to_string())?;
        }
        file.write_all(content)
            .map_err(|error| format!("write {}: {error}", temporary.display()))?;
        drop(file);
        std::fs::rename(&temporary, path)
            .map_err(|error| format!("replace {}: {error}", path.display()))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_projects_coalesce_and_marks_after_take_survive() {
        let mut queue = Queue::default();
        let root = PathBuf::from("project");
        queue.mark(root.clone(), "first".into());
        queue.mark(root.clone(), "last".into());
        assert_eq!(queue.tasks.len(), 1);
        assert_eq!(
            queue
                .take_project(&root)
                .map(|pending| pending.reason)
                .as_deref(),
            Some("last")
        );
        queue.mark(root.clone(), "during compilation".into());
        assert_eq!(
            queue
                .take_project(&root)
                .map(|pending| pending.reason)
                .as_deref(),
            Some("during compilation")
        );
    }

    #[test]
    fn retries_are_bounded_and_a_new_edit_replaces_the_retry() {
        let mut queue = Queue::default();
        let root = PathBuf::from("project");
        queue.mark(root.clone(), "first".into());
        for attempt in 0..3 {
            let pending = queue.take_project(&root).unwrap();
            assert_eq!(pending.attempt, attempt);
            queue.schedule_retry(Task::Project(root.clone()), pending);
            queue.promote_retries(Instant::now() + CHECK_INTERVAL);
        }
        assert!(queue.dirty.is_empty() && queue.retries.is_empty());
        queue.mark(root.clone(), "failed".into());
        let pending = queue.take_project(&root).unwrap();
        queue.schedule_retry(Task::Project(root.clone()), pending);
        queue.mark(root.clone(), "new edit".into());
        assert!(queue.retries.is_empty());
        let pending = queue.take_project(&root).unwrap();
        assert_eq!(pending.reason, "new edit");
        assert_eq!(pending.attempt, 0);
    }

    #[test]
    fn atomic_output_is_unchanged_without_replacing_inode() {
        use std::os::unix::fs::MetadataExt;
        let root = std::env::temp_dir().join(format!("kota-sync-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("AGENTS.md");
        assert!(write_if_changed(&path, b"first").unwrap());
        let before = std::fs::metadata(&path).unwrap().ino();
        assert!(!write_if_changed(&path, b"first").unwrap());
        assert_eq!(std::fs::metadata(&path).unwrap().ino(), before);
        assert!(write_if_changed(&path, b"second").unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ordered_requests_allow_nested_helpers_without_deadlock() {
        let (send, receive) = mpsc::channel();
        let first = submit(move || {
            send.send(1).unwrap();
            call(|| Ok(2))
        });
        assert_eq!(call(|| Ok(3)).unwrap(), 3);
        assert_eq!(receive.recv().unwrap(), 1);
        assert_eq!(tauri::async_runtime::block_on(first).unwrap(), 2);
    }

    #[test]
    fn receipt_runs_before_pending_broadcast() {
        let root = std::env::temp_dir().join(format!("kota-priority-{}", uuid::Uuid::new_v4()));
        let (entered, started) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        enqueue(Box::new(move || {
            entered.send(()).unwrap();
            blocked.recv().unwrap();
        }));
        started
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        mark_project(root.clone(), "test broadcast");
        let receipt = submit(move || Ok(take_project(&root)));
        release.send(()).unwrap();
        assert!(tauri::async_runtime::block_on(receipt).unwrap());
    }

    #[test]
    fn optimistic_update_recompiles_external_ghost_and_preserves_persistent_conflict() {
        let root = std::env::temp_dir().join(format!("kota-ghost-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("AGENTS.md");
        std::fs::write(&path, "old ghost").unwrap();
        let mut calls = 0;
        update_adapter(&path, |observed| {
            calls += 1;
            if calls == 1 {
                std::fs::write(&path, "external ghost").unwrap();
            }
            Ok(format!("{} + rules", observed.unwrap()))
        })
        .unwrap();
        assert_eq!(calls, 2);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "external ghost + rules"
        );
        let mut calls = 0;
        assert!(update_adapter(&path, |_| {
            calls += 1;
            std::fs::write(&path, format!("external edit {calls}")).unwrap();
            Ok("compiled".into())
        })
        .is_err());
        assert_eq!(calls, 3);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external edit 3");
        std::fs::remove_dir_all(root).unwrap();
    }

    fn fixture_agent(root: &Path, id: &str) -> PathBuf {
        let cwd = root.join(".agent-workspaces").join(id);
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(
            cwd.join("agent.yaml"),
            format!("id: {id}\ndisplay-name: {id}\nstatus: active\nrecruited-from: test\n"),
        )
        .unwrap();
        std::fs::write(cwd.join("SHELL.yaml"), "provider: codex\nskills: []\n").unwrap();
        std::fs::write(
            cwd.join("AGENTS.md"),
            "<!-- kota:ghost:start -->\nfixture ghost\n<!-- kota:ghost:end -->\n",
        )
        .unwrap();
        cwd
    }

    #[test]
    fn project_sync_updates_rules_and_roster_without_rewriting_identical_output() {
        use std::os::unix::fs::MetadataExt;
        let root = std::env::temp_dir().join(format!("kota-project-sync-{}", uuid::Uuid::new_v4()));
        let alice = fixture_agent(&root, "alice");
        let bob = fixture_agent(&root, "bob");
        let rules = root.join("project-rules");
        std::fs::create_dir_all(&rules).unwrap();
        std::fs::write(rules.join("always.md"), "# Test\nFollow the first rule.\n").unwrap();
        crate::regenerate_project_adapters_in_root(&root).unwrap();
        let path = alice.join("AGENTS.md");
        let first = std::fs::read_to_string(&path).unwrap();
        assert!(first.contains("Follow the first rule."));
        assert!(first.contains("fixture ghost"));
        assert!(first.contains("`bob`"));
        let inode = std::fs::metadata(&path).unwrap().ino();
        crate::regenerate_project_adapters_in_root(&root).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().ino(), inode);
        std::fs::write(rules.join("always.md"), "# Test\nFollow the second rule.\n").unwrap();
        std::fs::write(bob.join("agent.yaml"), "id: bob\nstatus: archived\n").unwrap();
        crate::regenerate_project_adapters_in_root(&root).unwrap();
        let second = std::fs::read_to_string(&path).unwrap();
        assert!(second.contains("Follow the second rule."));
        assert!(!second.contains("`bob`"));
        assert_eq!(
            crate::extract_adapter_ghost(&second).as_deref(),
            Some("fixture ghost")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_adapter_does_not_block_other_agents_or_erase_existing_ghost() {
        let root =
            std::env::temp_dir().join(format!("kota-project-error-{}", uuid::Uuid::new_v4()));
        let alice = fixture_agent(&root, "alice");
        let bob = fixture_agent(&root, "bob");
        std::fs::write(
            alice.join("AGENTS.md"),
            "malformed adapter with important text",
        )
        .unwrap();
        assert!(crate::regenerate_project_adapters_in_root(&root).is_err());
        assert_eq!(
            std::fs::read_to_string(alice.join("AGENTS.md")).unwrap(),
            "malformed adapter with important text"
        );
        assert!(std::fs::read_to_string(bob.join("AGENTS.md"))
            .unwrap()
            .contains("Always Follow These Rules"));
        assert!(
            crate::sync_adapter_before_launch(&root, "bob", Some(&bob.join("AGENTS.md"))).is_ok()
        );
        assert!(
            crate::sync_adapter_before_launch(&root, "alice", Some(&alice.join("AGENTS.md")))
                .is_ok()
        );
        std::fs::remove_file(alice.join("AGENTS.md")).unwrap();
        std::fs::create_dir(alice.join("AGENTS.md")).unwrap();
        assert!(
            crate::sync_adapter_before_launch(&root, "alice", Some(&alice.join("AGENTS.md")))
                .is_err()
        );
        assert!(alice.join("AGENTS.md").is_dir());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn completed_status_change_succeeds_when_another_agents_adapter_is_invalid() {
        use std::os::unix::fs::MetadataExt;
        let root =
            std::env::temp_dir().join(format!("kota-lifecycle-sync-{}", uuid::Uuid::new_v4()));
        let alice = fixture_agent(&root, "alice");
        let bob = fixture_agent(&root, "bob");
        std::fs::write(alice.join("AGENTS.md"), "malformed ghost; preserve me").unwrap();
        let identity = bob.join("agent.yaml");
        let inode = std::fs::metadata(&identity).unwrap().ino();
        let mut ctx = crate::sync_context(&root);
        ctx.cwd = bob.clone();
        let archived = ctx.clone();
        call(move || crate::persist_project_agent_status(&archived, "archived", Some("now")))
            .unwrap();
        assert_eq!(
            crate::yaml_string(&crate::read_yaml_mapping(&identity).unwrap(), "status").as_deref(),
            Some("archived")
        );
        assert_ne!(std::fs::metadata(&identity).unwrap().ino(), inode);
        assert_eq!(
            std::fs::read_to_string(alice.join("AGENTS.md")).unwrap(),
            "malformed ghost; preserve me"
        );
        call(move || crate::persist_project_agent_status(&ctx, "active", None)).unwrap();
        assert!(std::fs::read_to_string(bob.join("AGENTS.md"))
            .unwrap()
            .contains("Always Follow These Rules"));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn skills_follow_current_config_and_pool_without_writing_the_adapter() {
        use std::os::unix::fs::MetadataExt;
        let root = std::env::temp_dir().join(format!("kota-skill-sync-{}", uuid::Uuid::new_v4()));
        let cwd = fixture_agent(&root, "alice");
        let pool = root.join("pool");
        for skill in ["A", "B"] {
            std::fs::create_dir_all(pool.join(skill)).unwrap();
            std::fs::write(pool.join(skill).join("SKILL.md"), format!("# {skill}")).unwrap();
        }
        let adapter = cwd.join("AGENTS.md");
        let adapter_inode = std::fs::metadata(&adapter).unwrap().ino();
        let sync = || {
            let cwd = cwd.clone();
            let pool = pool.clone();
            call(move || {
                let (shell, cli) = crate::read_skill_config(&cwd)?;
                crate::project_account_skills_from_pool(&cwd, cli, &shell.skills, &pool)
            })
        };
        std::fs::write(
            cwd.join("SHELL.yaml"),
            "provider: codex\nskills: [A, own]\n",
        )
        .unwrap();
        let links = cwd.join(".agents/skills");
        std::fs::create_dir_all(links.join("own")).unwrap();
        assert_eq!(sync().unwrap().matched, vec!["A"]);
        let link_inode = std::fs::symlink_metadata(links.join("A")).unwrap().ino();
        let missing_inode = std::fs::metadata(cwd.join("missing-skills.txt"))
            .unwrap()
            .ino();
        sync().unwrap();
        assert_eq!(
            std::fs::symlink_metadata(links.join("A")).unwrap().ino(),
            link_inode
        );
        assert_eq!(
            std::fs::metadata(cwd.join("missing-skills.txt"))
                .unwrap()
                .ino(),
            missing_inode
        );
        std::fs::write(pool.join("A/SKILL.md"), "new native skill body").unwrap();
        assert_eq!(
            std::fs::read_to_string(links.join("A/SKILL.md")).unwrap(),
            "new native skill body"
        );
        std::fs::write(
            cwd.join("SHELL.yaml"),
            "provider: codex\nskills: [B, own]\n",
        )
        .unwrap();
        sync().unwrap();
        assert!(std::fs::symlink_metadata(links.join("A")).is_err());
        assert_eq!(std::fs::read_link(links.join("B")).unwrap(), pool.join("B"));
        std::fs::remove_dir_all(pool.join("B")).unwrap();
        assert_eq!(sync().unwrap().missing, vec!["B", "own"]);
        assert!(std::fs::symlink_metadata(links.join("B")).is_err());
        assert!(links.join("own").is_dir());
        std::fs::create_dir(pool.join("B")).unwrap();
        std::fs::write(pool.join("B/SKILL.md"), "restored").unwrap();
        assert_eq!(sync().unwrap().matched, vec!["B"]);
        std::fs::rename(pool.join("B"), pool.join("C")).unwrap();
        sync().unwrap();
        assert!(!links.join("C").exists());
        std::fs::write(cwd.join("SHELL.yaml"), "skills: [").unwrap();
        let missing = std::fs::read(cwd.join("missing-skills.txt")).unwrap();
        assert!(sync().is_err());
        assert_eq!(
            std::fs::read(cwd.join("missing-skills.txt")).unwrap(),
            missing
        );
        assert_eq!(std::fs::metadata(adapter).unwrap().ino(), adapter_inode);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pool_referrers_are_read_fresh_and_one_invalid_shell_does_not_hide_other_agents() {
        let root = std::env::temp_dir().join(format!("kota-skill-refs-{}", uuid::Uuid::new_v4()));
        let first = root.join("first");
        let second = root.join("second");
        let alice = fixture_agent(&first, "alice");
        let bob = fixture_agent(&second, "bob");
        let bad = fixture_agent(&first, "bad");
        std::fs::write(alice.join("SHELL.yaml"), "skills: [A]\n").unwrap();
        std::fs::write(bob.join("SHELL.yaml"), "skills: [B]\n").unwrap();
        std::fs::write(bad.join("SHELL.yaml"), "skills: [").unwrap();
        let ids = std::collections::BTreeSet::from(["A".to_string()]);
        let projects = [first, second];
        let refs = crate::skill_pool_referrers_in(&projects, Some(&ids)).unwrap();
        assert_eq!(refs.agents.len(), 1);
        assert_eq!(refs.agents[0].agent, "alice");
        assert_eq!(refs.errors.len(), 1);
        std::fs::write(alice.join("SHELL.yaml"), "skills: [B]\n").unwrap();
        assert!(crate::skill_pool_referrers_in(&projects, Some(&ids))
            .unwrap()
            .agents
            .is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn projection_request_reads_current_shell_instead_of_captured_hydration_values() {
        let root =
            std::env::temp_dir().join(format!("kota-skill-current-{}", uuid::Uuid::new_v4()));
        let cwd = fixture_agent(&root, "alice");
        let new_id = format!("kota-test-{}", uuid::Uuid::new_v4());
        std::fs::write(
            cwd.join("SHELL.yaml"),
            format!("provider: codex\nskills: [{new_id}]\n"),
        )
        .unwrap();
        let result = crate::project_account_skills(
            &cwd,
            crate::pty::agent::AgentCli::Claude,
            &["stale".into()],
        )
        .unwrap();
        assert_eq!(result.missing, vec![new_id]);
        assert!(cwd.join(".agents/skills").is_dir());
        assert!(!cwd.join(".claude/skills").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn launch_detail_reads_do_not_enqueue_repairs_while_worker_is_busy() {
        use std::os::unix::fs::MetadataExt;
        let root = std::env::temp_dir().join(format!("kota-launch-read-{}", uuid::Uuid::new_v4()));
        let cwd = fixture_agent(&root, "alice");
        std::fs::create_dir_all(cwd.join("project-files/.git")).unwrap();
        let mut ctx = crate::sync_context(&root);
        ctx.cwd = cwd.clone();
        ctx.worktree_root = cwd.join("project-files");
        let shell_inode = std::fs::metadata(cwd.join("SHELL.yaml")).unwrap().ino();
        let (entered, blocked) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        enqueue(Box::new(move || {
            entered.send(()).unwrap();
            gate.recv_timeout(Duration::from_secs(15)).unwrap();
        }));
        blocked.recv_timeout(Duration::from_secs(5)).unwrap();
        let (send, receive) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let before = REQUEST_COUNT.with(Cell::get);
            for _ in 0..5 {
                let detail =
                    crate::load_project_agent_detail_in_context(ctx.clone(), "alice", false)
                        .unwrap();
                assert_eq!(detail.cli, crate::pty::agent::AgentCli::Codex);
            }
            send.send(REQUEST_COUNT.with(Cell::get) - before).unwrap();
        });
        let result = receive.recv_timeout(Duration::from_secs(10));
        release.send(()).unwrap();
        reader.join().unwrap();
        assert_eq!(result.unwrap(), 0);
        assert_eq!(
            std::fs::metadata(cwd.join("SHELL.yaml")).unwrap().ino(),
            shell_inode
        );
        assert!(!cwd.join(".agents/skills").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn slow_removal_does_not_hold_other_projects_startup_receipts() {
        for remove_project in [false, true] {
            let root = std::env::temp_dir().join(format!("kota-slow-removal-{}", uuid::Uuid::new_v4()));
            let deleting = root.join("deleting");
            let remaining = root.join("remaining");
            let alice = fixture_agent(&deleting, "alice");
            let bob = fixture_agent(&remaining, "bob");
            let (entered, started) = mpsc::channel();
            let (release, gate) = mpsc::channel();
            let agent = (!remove_project).then(|| "alice".to_string());
            let removing = deleting.clone();
            let removal = std::thread::spawn(move || {
                remove_after_forgetting(removing.clone(), agent, || {
                    assert!(!on_worker());
                    entered.send(()).unwrap();
                    gate.recv().unwrap(); // Deterministic slow recursive deletion.
                    std::fs::remove_dir_all(if remove_project { &removing } else { &alice })
                        .map_err(|error| error.to_string())
                })
            });
            started.recv_timeout(Duration::from_secs(10)).unwrap();
            let (sent, received) = mpsc::channel();
            let startup = std::thread::spawn(move || {
                sent.send(crate::sync_adapter_before_launch(
                    &remaining, "bob", Some(&bob.join("AGENTS.md")),
                )).unwrap();
            });
            let result = received.recv_timeout(Duration::from_secs(10));
            // Always release deletion even if the receipt times out.
            release.send(()).unwrap();
            removal.join().unwrap().unwrap();
            startup.join().unwrap();
            result.expect("another project's startup waited for deletion").unwrap();
            assert!(call(move || crate::sync_agent_skills(&AgentKey { project: deleting, agent: "alice".into() })).is_err());
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn malformed_or_temporarily_missing_peer_identity_preserves_generated_context() {
        let root =
            std::env::temp_dir().join(format!("kota-identity-invalid-{}", uuid::Uuid::new_v4()));
        let alice = fixture_agent(&root, "alice");
        let bob = fixture_agent(&root, "bob");
        crate::regenerate_project_adapters_in_root(&root).unwrap();
        let original = std::fs::read(alice.join("AGENTS.md")).unwrap();
        for invalid in ["", "plain scalar", "name: ["] {
            std::fs::write(bob.join("agent.yaml"), invalid).unwrap();
            assert!(crate::regenerate_project_adapters_in_root(&root).is_err());
            assert_eq!(std::fs::read(alice.join("AGENTS.md")).unwrap(), original);
        }
        std::fs::remove_file(bob.join("agent.yaml")).unwrap();
        assert!(crate::regenerate_project_adapters_in_root(&root).is_err());
        assert_eq!(std::fs::read(alice.join("AGENTS.md")).unwrap(), original);
        std::fs::write(
            bob.join("agent.yaml"),
            "display-name: Robert\nstatus: active\nrecruited-from: Hero-New\n",
        )
        .unwrap();
        crate::regenerate_project_adapters_in_root(&root).unwrap();
        assert!(std::fs::read_to_string(alice.join("AGENTS.md"))
            .unwrap()
            .contains("Robert"));
        assert!(std::fs::read_to_string(bob.join("AGENTS.md"))
            .unwrap()
            .contains(&crate::formal_hero_code("Hero-New")));
        std::fs::remove_dir_all(root).unwrap();
    }
}
