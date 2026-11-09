//! Bartender's live room model (perf plan A v2).
//!
//! The panel used to recompute the room from scratch every 12 s. Now the
//! project the panel shows keeps one model in memory. A full compute builds it
//! when the panel first asks, and redoes it on explicit actions, window focus,
//! agent-set changes and watcher overflow. File-system events re-read only the
//! part they touch, and every change is pushed to the panel. An idle room runs
//! no git.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use notify::{RecursiveMode, Watcher};
use tauri::{AppHandle, Emitter, Manager};

use super::{
    agent_targets, ahead_commit_count, append_name_status_paths, behind_commit_count,
    changed_paths_against, changed_paths_from_commits, git, initial_push_status,
    normalize_git_path, path_string, pending_commits_between, room_status, source_head,
    status_cache_key, BartenderDirtyAgent, BartenderManager, BartenderRequest, BartenderStatus,
    BartenderStatusCacheKey, InitialPushStatus, BARTENDER_STATUS_CHANGED_EVENT,
};
use crate::integrations::{IntegrationManager, WorkspaceProject};

/// Events for one part of the room merge for this long before one re-read.
pub(super) const MERGE_WINDOW: Duration = Duration::from_secs(2);

/// Worktree content under these path segments never moves the room status.
const IGNORED_DIRS: [&str; 4] = [".git", "node_modules", "target", "dist"];

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

/// Upstream counts, re-read together because they share the upstream ref.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct GithubCounts {
    pub(super) ahead: usize,
    pub(super) behind: usize,
    pub(super) initial_push: Option<InitialPushStatus>,
}

impl GithubCounts {
    fn read(source: &Path, default_branch: &str) -> Self {
        Self {
            ahead: ahead_commit_count(source, default_branch).unwrap_or(0),
            behind: behind_commit_count(source, default_branch).unwrap_or(0),
            initial_push: initial_push_status(source, default_branch),
        }
    }
}

#[derive(Clone, Debug)]
struct SourceModel {
    path: PathBuf,
    head: String,
    dirty_paths: BTreeSet<String>,
    github: GithubCounts,
}

#[derive(Clone, Debug)]
struct AgentModel {
    agent_id: String,
    path: PathBuf,
    head: String,
    pending_commits: Vec<String>,
    /// Paths of each pending commit. A commit never changes, so each SHA is read once.
    commit_paths: HashMap<String, BTreeSet<String>>,
    /// What the agent's commits change against the room head: the pending
    /// commits' paths, or for an agent behind the room, the tree-to-tree diff.
    tree_changed_paths: BTreeSet<String>,
    /// The worktree against the agent's own HEAD, untracked files included.
    dirty_paths: BTreeSet<String>,
}

#[derive(Clone, Debug)]
pub(super) struct LiveModel {
    project_id: String,
    source_dir: String,
    default_branch: String,
    source: SourceModel,
    agents: Vec<AgentModel>,
    checked_at: DateTime<Utc>,
    /// When the git reads of the last full compute began.
    started: Instant,
}

/// A full compute: what the 12 s poll used to do, now only on explicit occasions.
pub(super) fn compute_live(workspace: &WorkspaceProject) -> LiveModel {
    let started = Instant::now();
    let source_path = PathBuf::from(&workspace.source_dir);
    let head = git(&source_path, &["rev-parse", "--verify", "HEAD"])
        .unwrap_or_else(|_| String::new())
        .trim()
        .to_string();
    let dirty_paths = dirty_paths(&source_path);
    let mut agents = Vec::new();
    for target in agent_targets(workspace) {
        if !target.path.exists() {
            continue;
        }
        let mut agent = AgentModel {
            agent_id: target.agent_id,
            path: target.path,
            head: String::new(),
            pending_commits: Vec::new(),
            commit_paths: HashMap::new(),
            tree_changed_paths: BTreeSet::new(),
            dirty_paths: BTreeSet::new(),
        };
        agent.read_head();
        agent.read_commits(&head);
        agent.read_dirty(&head);
        agents.push(agent);
    }
    let github = GithubCounts::read(&source_path, &workspace.default_branch);
    let model = LiveModel {
        project_id: workspace.project_id.clone(),
        source_dir: workspace.source_dir.clone(),
        default_branch: workspace.default_branch.clone(),
        source: SourceModel {
            path: source_path,
            head,
            dirty_paths,
            github,
        },
        agents,
        checked_at: Utc::now(),
        started,
    };
    let elapsed = started.elapsed();
    if elapsed > Duration::from_millis(3000) {
        let status = model.status();
        crate::kota_debug_log(&format!(
            "[bartender] compute_status project={} state={} room={} source={} github={} behind={} elapsed={}ms",
            status.project_id,
            status.state,
            status.room_change_count,
            status.source_change_count,
            status.github_change_count,
            status.github_behind_count,
            elapsed.as_millis()
        ));
    }
    model
}

impl LiveModel {
    pub(super) fn status(&self) -> BartenderStatus {
        let source_change_count = self.source.dirty_paths.len();
        let mut room_changed_paths = self.source.dirty_paths.clone();
        let mut dirty_agents = Vec::new();
        for agent in &self.agents {
            let mut changed = agent.tree_changed_paths.clone();
            changed.extend(agent.dirty_paths.iter().cloned());
            let change_count = changed.len();
            let pending_commit_count = agent.pending_commits.len();
            if change_count > 0 || pending_commit_count > 0 {
                room_changed_paths.extend(changed);
                dirty_agents.push(BartenderDirtyAgent {
                    agent_id: agent.agent_id.clone(),
                    path: path_string(&agent.path),
                    change_count,
                    pending_commit_count,
                });
            }
        }
        room_status(
            &self.project_id,
            &self.source_dir,
            &self.default_branch,
            room_changed_paths.len(),
            source_change_count,
            dirty_agents,
            &self.source.github,
            self.checked_at.to_rfc3339(),
        )
    }

    /// Re-reads only what `changes` touched.
    pub(super) fn apply(&mut self, changes: &BTreeSet<Change>) {
        let all_heads = changes.contains(&Change::AllHeads);
        let mut upstream = changes.contains(&Change::Upstream);
        let mut source_tree = changes.contains(&Change::SourceTree);
        let mut room_moved = false;
        if all_heads || changes.contains(&Change::SourceHead) {
            let head = git(&self.source.path, &["rev-parse", "--verify", "HEAD"])
                .unwrap_or_else(|_| String::new())
                .trim()
                .to_string();
            room_moved = head != self.source.head;
            self.source.head = head;
            // A new HEAD or a branch switch moves ahead/behind and the initial push.
            upstream = true;
            source_tree |= room_moved;
        }
        let room_head = self.source.head.clone();
        for agent in &mut self.agents {
            let head_event =
                all_heads || changes.contains(&Change::AgentHead(agent.agent_id.clone()));
            let moved = head_event && agent.read_head();
            if moved || room_moved {
                agent.read_commits(&room_head);
            }
            if moved || changes.contains(&Change::AgentTree(agent.agent_id.clone())) {
                agent.read_dirty(&room_head);
            }
        }
        if source_tree {
            self.source.dirty_paths = dirty_paths(&self.source.path);
        }
        if upstream {
            self.source.github = GithubCounts::read(&self.source.path, &self.default_branch);
        }
        self.checked_at = Utc::now();
    }
}

impl AgentModel {
    /// Returns whether HEAD moved.
    fn read_head(&mut self) -> bool {
        let head = source_head(&self.path).unwrap_or_default();
        let moved = head != self.head;
        self.head = head;
        moved
    }

    fn read_commits(&mut self, room_head: &str) {
        let mut known = std::mem::take(&mut self.commit_paths);
        if self.head.is_empty() {
            // No readable HEAD: compare the room head with the worktree, as before.
            self.pending_commits.clear();
            self.tree_changed_paths = self.room_against_worktree(room_head);
            return;
        }
        self.pending_commits = if room_head.is_empty() {
            Vec::new()
        } else {
            pending_commits_between(room_head, &self.head, &self.path).unwrap_or_default()
        };
        let mut failed = false;
        for commit in &self.pending_commits {
            let paths = match known.remove(commit) {
                Some(paths) => paths,
                None => match changed_paths_from_commits(&self.path, std::slice::from_ref(commit)) {
                    Ok(paths) => paths,
                    Err(_) => {
                        failed = true;
                        continue;
                    }
                },
            };
            self.commit_paths.insert(commit.clone(), paths);
        }
        self.tree_changed_paths = if failed {
            // One unreadable commit empties the commit paths, as before.
            BTreeSet::new()
        } else if !self.pending_commits.is_empty() {
            self.commit_paths.values().flatten().cloned().collect()
        } else if !room_head.is_empty() && self.head != room_head {
            // Behind the room: tree to tree, with the worktree counted in dirty_paths.
            git(
                &self.path,
                &["diff", "--name-status", "-z", "--no-renames", room_head, &self.head, "--"],
            )
            .map(|diff| {
                let mut paths = BTreeSet::new();
                append_name_status_paths(&mut paths, &diff);
                paths
            })
            .unwrap_or_default()
        } else {
            BTreeSet::new()
        };
    }

    fn read_dirty(&mut self, room_head: &str) {
        if self.head.is_empty() {
            // Without a HEAD there is nothing to diff against; the room comparison
            // reads the worktree itself.
            self.dirty_paths.clear();
            self.tree_changed_paths = self.room_against_worktree(room_head);
            return;
        }
        self.dirty_paths = dirty_paths(&self.path);
    }

    fn room_against_worktree(&self, room_head: &str) -> BTreeSet<String> {
        if room_head.is_empty() {
            return BTreeSet::new();
        }
        changed_paths_against(&self.path, room_head).unwrap_or_default()
    }
}

/// Worktree against HEAD plus untracked files. No optional locks, so the
/// read never rewrites the index and never wakes its own watcher.
fn dirty_paths(path: &Path) -> BTreeSet<String> {
    git(
        path,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "-z",
            "--no-renames",
            "--untracked-files=all",
        ],
    )
    .map(|out| porcelain_v2_paths(&out))
    .unwrap_or_default()
}

fn porcelain_v2_paths(out: &str) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    let mut records = out.split('\0');
    while let Some(record) = records.next() {
        let path = match record.as_bytes().first() {
            Some(b'1') => record.splitn(9, ' ').nth(8),
            Some(b'2') => {
                // A rename record carries its original path as the next record.
                if let Some(original) = records.next() {
                    insert_status_path(&mut paths, original);
                }
                record.splitn(10, ' ').nth(9)
            }
            Some(b'u') => record.splitn(11, ' ').nth(10),
            Some(b'?') => record.strip_prefix("? "),
            _ => None,
        };
        if let Some(path) = path {
            insert_status_path(&mut paths, path);
        }
    }
    paths
}

fn insert_status_path(paths: &mut BTreeSet<String>, path: &str) {
    let path = path.trim();
    if !path.is_empty() {
        paths.insert(normalize_git_path(path));
    }
}

/// One part of the room that an event says may have moved.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum Change {
    /// The window came back to the front: one full reconcile.
    Focus,
    /// Watcher overflow or error: a full reconcile and new watchers.
    Rescan,
    SourceHead,
    Upstream,
    AllHeads,
    SourceTree,
    AgentHead(String),
    AgentTree(String),
}

impl Change {
    fn label(&self) -> String {
        match self {
            Self::Focus => "focus".into(),
            Self::Rescan => "rescan".into(),
            Self::SourceHead => "source-head".into(),
            Self::Upstream => "upstream".into(),
            Self::AllHeads => "heads".into(),
            Self::SourceTree => "source-tree".into(),
            Self::AgentHead(agent_id) => format!("agent-head:{agent_id}"),
            Self::AgentTree(agent_id) => format!("agent-tree:{agent_id}"),
        }
    }
}

fn cause(changes: &BTreeSet<Change>) -> String {
    changes.iter().map(Change::label).collect::<Vec<_>>().join(",")
}

/// The merge window: the first event for a part opens a 2 s window and the
/// part is re-read once when it closes, however many events came in between.
#[derive(Debug, Default)]
pub(super) struct Pending {
    /// Change -> (when it is due, when it was last noted).
    entries: BTreeMap<Change, (Instant, Instant)>,
}

impl Pending {
    pub(super) fn note(&mut self, change: Change, at: Instant) {
        self.entries
            .entry(change)
            .and_modify(|entry| entry.1 = entry.1.max(at))
            .or_insert((at + MERGE_WINDOW, at));
    }

    pub(super) fn next_due(&self) -> Option<Instant> {
        self.entries.values().map(|entry| entry.0).min()
    }

    pub(super) fn take_due(&mut self, now: Instant) -> BTreeSet<Change> {
        let due = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.0 <= now)
            .map(|(change, _)| change.clone())
            .collect::<BTreeSet<_>>();
        for change in &due {
            self.entries.remove(change);
        }
        due
    }

    /// A full compute that began at `started` already saw everything noted before it.
    pub(super) fn forget_before(&mut self, started: Instant) {
        self.entries.retain(|_, entry| entry.1 >= started);
    }
}

#[derive(Clone, Debug)]
struct Repo {
    worktree: PathBuf,
    git_dir: PathBuf,
    common_dir: PathBuf,
}

impl Repo {
    fn resolve(worktree: &Path) -> Self {
        let worktree = canonical(worktree);
        let out = git(&worktree, &["rev-parse", "--absolute-git-dir", "--git-common-dir"])
            .unwrap_or_default();
        let mut lines = out.lines().map(str::trim).filter(|line| !line.is_empty());
        let git_dir = lines
            .next()
            .map(PathBuf::from)
            .unwrap_or_else(|| worktree.join(".git"));
        let common_dir = lines
            .next()
            .map(PathBuf::from)
            .map(|dir| if dir.is_absolute() { dir } else { worktree.join(dir) })
            .unwrap_or_else(|| git_dir.clone());
        Self {
            git_dir: canonical(&git_dir),
            common_dir: canonical(&common_dir),
            worktree,
        }
    }

    /// The branch this worktree has checked out, read from its HEAD file.
    fn branch_ref(&self) -> Option<String> {
        let head = fs::read_to_string(self.git_dir.join("HEAD")).ok()?;
        head.trim().strip_prefix("ref: ").map(str::to_string)
    }
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Maps a file-system path to the part of the room it can move.
#[derive(Clone, Debug)]
pub(super) struct WatchMap {
    source: Repo,
    agents: Vec<(String, Repo)>,
}

impl WatchMap {
    fn for_model(model: &LiveModel) -> Self {
        Self {
            source: Repo::resolve(&model.source.path),
            agents: model
                .agents
                .iter()
                .map(|agent| (agent.agent_id.clone(), Repo::resolve(&agent.path)))
                .collect(),
        }
    }

    fn repos(&self) -> impl Iterator<Item = (Option<&str>, &Repo)> {
        std::iter::once((None, &self.source)).chain(
            self.agents
                .iter()
                .map(|(agent_id, repo)| (Some(agent_id.as_str()), repo)),
        )
    }

    /// Recursive roots: every worktree, plus any git dir that lives outside them.
    pub(super) fn roots(&self) -> Vec<PathBuf> {
        let worktrees = self.repos().map(|(_, repo)| repo.worktree.clone());
        let git_dirs = self
            .repos()
            .flat_map(|(_, repo)| [repo.common_dir.clone(), repo.git_dir.clone()]);
        let mut roots = Vec::<PathBuf>::new();
        for dir in worktrees.chain(git_dirs) {
            if !roots.iter().any(|root| dir.starts_with(root)) {
                roots.push(dir);
            }
        }
        roots
    }

    pub(super) fn classify(&self, path: &Path) -> Vec<Change> {
        // A linked worktree's own git dir holds its HEAD and index.
        for (agent_id, repo) in self.repos() {
            if repo.git_dir == repo.common_dir {
                continue;
            }
            if let Ok(rel) = path.strip_prefix(&repo.git_dir) {
                return match rel.to_str() {
                    Some("HEAD") => vec![head_change(agent_id)],
                    Some("index") => vec![tree_change(agent_id)],
                    _ => Vec::new(),
                };
            }
        }
        let mut commons = self
            .repos()
            .map(|(_, repo)| repo.common_dir.as_path())
            .collect::<Vec<_>>();
        commons.dedup();
        for common in commons {
            if let Ok(rel) = path.strip_prefix(common) {
                return self.classify_git(common, rel);
            }
        }
        // Worktree content. Agents first, in case one sits inside the source tree.
        for (agent_id, repo) in &self.agents {
            if let Ok(rel) = path.strip_prefix(&repo.worktree) {
                return if ignored(rel) {
                    Vec::new()
                } else {
                    vec![Change::AgentTree(agent_id.clone())]
                };
            }
        }
        if let Ok(rel) = path.strip_prefix(&self.source.worktree) {
            return if ignored(rel) {
                Vec::new()
            } else {
                vec![Change::SourceTree]
            };
        }
        Vec::new()
    }

    /// Only refs, packed-refs, config and a main worktree's HEAD and index count.
    /// FETCH_HEAD, ORIG_HEAD, logs/, objects/, other worktrees and locks never do.
    fn classify_git(&self, common: &Path, rel: &Path) -> Vec<Change> {
        let Some(rel) = rel.to_str() else {
            return Vec::new();
        };
        if rel.ends_with(".lock") {
            return Vec::new();
        }
        let source_common = common == self.source.common_dir;
        let mains = || {
            self.repos()
                .filter(move |(_, repo)| repo.git_dir == common)
                .map(|(agent_id, _)| agent_id)
        };
        match rel {
            "HEAD" => return mains().map(head_change).collect(),
            "index" => return mains().map(tree_change).collect(),
            "packed-refs" => return vec![Change::AllHeads],
            "config" if source_common => return vec![Change::Upstream],
            _ => {}
        }
        if rel.starts_with("refs/remotes/") {
            return if source_common {
                vec![Change::Upstream]
            } else {
                Vec::new()
            };
        }
        if rel.starts_with("refs/heads/") {
            return self
                .repos()
                .filter(|(_, repo)| repo.common_dir == common)
                .filter(|(_, repo)| repo.branch_ref().as_deref() == Some(rel))
                .map(|(agent_id, _)| head_change(agent_id))
                .collect();
        }
        Vec::new()
    }

    /// Dropped events inside an ignored worktree tree (an npm install) need no rescan.
    fn rescan_needed(&self, paths: &[PathBuf]) -> bool {
        paths.is_empty()
            || paths.iter().any(|path| {
                let in_git = self.repos().any(|(_, repo)| {
                    path.starts_with(&repo.git_dir) || path.starts_with(&repo.common_dir)
                });
                in_git
                    || !self.repos().any(|(_, repo)| {
                        path.strip_prefix(&repo.worktree).is_ok_and(ignored)
                    })
            })
    }

    fn changes_for(&self, result: &notify::Result<notify::Event>) -> Vec<Change> {
        let event = match result {
            Err(_) => return vec![Change::Rescan],
            Ok(event) => event,
        };
        if event.need_rescan() {
            return if self.rescan_needed(&event.paths) {
                vec![Change::Rescan]
            } else {
                Vec::new()
            };
        }
        if matches!(event.kind, notify::EventKind::Access(_)) {
            return Vec::new();
        }
        let mut changes = Vec::new();
        for path in &event.paths {
            for change in self.classify(path) {
                if !changes.contains(&change) {
                    changes.push(change);
                }
            }
        }
        changes
    }
}

fn head_change(agent_id: Option<&str>) -> Change {
    agent_id.map_or(Change::SourceHead, |agent_id| {
        Change::AgentHead(agent_id.to_string())
    })
}

fn tree_change(agent_id: Option<&str>) -> Change {
    agent_id.map_or(Change::SourceTree, |agent_id| {
        Change::AgentTree(agent_id.to_string())
    })
}

fn ignored(rel: &Path) -> bool {
    rel.components().any(|component| match component {
        Component::Normal(name) => name.to_str().is_some_and(|name| IGNORED_DIRS.contains(&name)),
        _ => false,
    })
}

fn watch(map: Arc<WatchMap>, tx: Sender<Signal>) -> notify::Result<notify::RecommendedWatcher> {
    let roots = map.roots();
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        let at = Instant::now();
        for change in map.changes_for(&result) {
            let _ = tx.send(Signal::Change(change, at));
        }
    })?;
    for root in roots {
        if !root.exists() {
            continue;
        }
        if let Err(err) = watcher.watch(&root, RecursiveMode::Recursive) {
            crate::kota_debug_log(&format!(
                "[bartender] live watch skipped {}: {err}",
                root.display()
            ));
        }
    }
    Ok(watcher)
}

enum Signal {
    Change(Change, Instant),
    /// An action's full compute, begun at this instant, is now the model.
    Covered(Instant),
    Stop,
}

/// The one watched project: the one the panel shows.
pub(super) struct LiveSlot {
    key: BartenderStatusCacheKey,
    workspace: WorkspaceProject,
    model: LiveModel,
    /// The status the panel last received.
    published: BartenderStatus,
    generation: u64,
    tx: Sender<Signal>,
    app: AppHandle,
    watcher: Option<notify::RecommendedWatcher>,
}

impl LiveSlot {
    fn start(
        app: &AppHandle,
        workspace: WorkspaceProject,
        model: LiveModel,
        published: BartenderStatus,
    ) -> Self {
        let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        spawn_worker(app.clone(), generation, rx);
        let mut slot = Self {
            key: status_cache_key(&workspace),
            workspace,
            model,
            published,
            generation,
            tx,
            app: app.clone(),
            watcher: None,
        };
        slot.rewatch();
        slot
    }

    fn same_project(&self, workspace: &WorkspaceProject) -> bool {
        self.key.project_id == workspace.project_id && self.key.local_root == workspace.local_root
    }

    fn rewatch(&mut self) {
        self.watcher = None;
        let map = Arc::new(WatchMap::for_model(&self.model));
        match watch(map, self.tx.clone()) {
            Ok(watcher) => self.watcher = Some(watcher),
            Err(err) => crate::kota_debug_log(&format!(
                "[bartender] live watch failed project={}: {err}",
                self.model.project_id
            )),
        }
    }

    /// Takes a full compute; new watchers when asked or when the agent set moved.
    fn adopt(&mut self, workspace: WorkspaceProject, model: LiveModel, rewatch: bool) -> bool {
        let key = status_cache_key(&workspace);
        let rewatch = rewatch || key != self.key || self.watcher.is_none();
        self.key = key;
        self.workspace = workspace;
        self.model = model;
        if rewatch {
            self.rewatch();
        }
        let status = self.model.status();
        self.publish(&status)
    }

    fn publish(&mut self, status: &BartenderStatus) -> bool {
        if same_status(&self.published, status) {
            return false;
        }
        self.published = status.clone();
        let _ = self.app.emit(BARTENDER_STATUS_CHANGED_EVENT, status);
        true
    }
}

impl Drop for LiveSlot {
    fn drop(&mut self) {
        let _ = self.tx.send(Signal::Stop);
    }
}

fn same_status(left: &BartenderStatus, right: &BartenderStatus) -> bool {
    let unstamped = |status: &BartenderStatus| BartenderStatus {
        checked_at: String::new(),
        ..status.clone()
    };
    unstamped(left) == unstamped(right)
}

enum Round {
    Applied,
    Reconciled(Instant),
    Gone,
}

fn spawn_worker(app: AppHandle, generation: u64, rx: Receiver<Signal>) {
    let spawned = thread::Builder::new()
        .name("bartender-live".into())
        .spawn(move || run_worker(&app, generation, &rx));
    if let Err(err) = spawned {
        crate::kota_debug_log(&format!("[bartender] live worker failed to start: {err}"));
    }
}

fn run_worker(app: &AppHandle, generation: u64, rx: &Receiver<Signal>) {
    let mut pending = Pending::default();
    let mut reconciled_at: Option<Instant> = None;
    loop {
        let first = match pending.next_due() {
            Some(due) => match rx.recv_timeout(due.saturating_duration_since(Instant::now())) {
                Ok(signal) => Some(signal),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            },
            None => match rx.recv() {
                Ok(signal) => Some(signal),
                Err(_) => return,
            },
        };
        for signal in first.into_iter().chain(rx.try_iter()) {
            match signal {
                Signal::Stop => return,
                Signal::Covered(started) => pending.forget_before(started),
                // A focus that came while the last reconcile ran is already answered.
                Signal::Change(Change::Focus, at)
                    if reconciled_at.is_some_and(|finished| at < finished) => {}
                Signal::Change(change, at) => pending.note(change, at),
            }
        }
        let due = pending.take_due(Instant::now());
        if due.is_empty() {
            continue;
        }
        let manager = app.state::<BartenderManager>();
        let round = if due.contains(&Change::Rescan) || due.contains(&Change::Focus) {
            manager.live_reconcile(app, generation, &due)
        } else {
            manager.live_round(generation, &due)
        };
        match round {
            Round::Gone => return,
            Round::Reconciled(started) => {
                pending.forget_before(started);
                reconciled_at = Some(Instant::now());
            }
            Round::Applied => {}
        }
    }
}

fn log_round(project_id: &str, cause: &str, elapsed: Duration, changed: bool) {
    crate::kota_debug_log(&format!(
        "[bartender] live project={project_id} cause={cause} elapsed={}ms changed={changed}",
        elapsed.as_millis()
    ));
}

/// The project as registered now: agents may have been archived, called back
/// or hired since the panel opened.
fn current_workspace(app: &AppHandle, workspace: &WorkspaceProject) -> Option<WorkspaceProject> {
    let manager = app.state::<IntegrationManager>();
    crate::active_bartender_workspace(
        &manager,
        &BartenderRequest {
            project_root: Some(workspace.local_root.clone()),
            conflict_prompt: None,
        },
    )
    .ok()
}

impl BartenderManager {
    /// The panel's call. It answers from the live model, and builds the model
    /// and its watchers when the panel shows a project, or its agent set, for
    /// the first time.
    pub fn watch_status(
        &self,
        app: &AppHandle,
        workspace: &WorkspaceProject,
    ) -> Result<BartenderStatus> {
        let key = status_cache_key(workspace);
        if let Some(status) = self.live_status(&key)? {
            return Ok(status);
        }
        let _compute_guard = self
            .status_compute_lock
            .lock()
            .map_err(|_| anyhow!("bartender status compute lock poisoned"))?;
        if let Some(status) = self.live_status(&key)? {
            return Ok(status);
        }
        let model = compute_live(workspace);
        let elapsed = model.started.elapsed();
        let status = model.status();
        let slot = LiveSlot::start(app, workspace.clone(), model, status.clone());
        // Replacing the slot stops the previous project's worker and watchers.
        *self
            .live
            .lock()
            .map_err(|_| anyhow!("bartender live model lock poisoned"))? = Some(slot);
        log_round(&workspace.project_id, "open", elapsed, true);
        Ok(status)
    }

    /// Window back in front: the panel's project reconciles once per burst.
    /// Runs on the main thread, so it never waits: a busy slot is already
    /// being reconciled.
    pub fn note_focus(&self) {
        if let Ok(slot) = self.live.try_lock() {
            if let Some(slot) = slot.as_ref() {
                let _ = slot.tx.send(Signal::Change(Change::Focus, Instant::now()));
            }
        }
    }

    pub(super) fn live_status(
        &self,
        key: &BartenderStatusCacheKey,
    ) -> Result<Option<BartenderStatus>> {
        let slot = self
            .live
            .lock()
            .map_err(|_| anyhow!("bartender live model lock poisoned"))?;
        Ok(slot
            .as_ref()
            .filter(|slot| slot.key == *key)
            .map(|slot| slot.model.status()))
    }

    /// An action's full compute, taken under the compute lock, becomes the model.
    pub(super) fn settle(&self, workspace: &WorkspaceProject, model: LiveModel) -> BartenderStatus {
        let status = model.status();
        if let Ok(mut slot) = self.live.lock() {
            if let Some(slot) = slot.as_mut().filter(|slot| slot.same_project(workspace)) {
                let started = model.started;
                slot.adopt(workspace.clone(), model, false);
                let _ = slot.tx.send(Signal::Covered(started));
            }
        }
        status
    }

    pub(super) fn settle_now(&self, workspace: &WorkspaceProject) -> BartenderStatus {
        self.settle(workspace, compute_live(workspace))
    }

    fn live_round(&self, generation: u64, due: &BTreeSet<Change>) -> Round {
        let Ok(_compute_guard) = self.status_compute_lock.lock() else {
            return Round::Gone;
        };
        let model = self.live.lock().ok().and_then(|slot| {
            slot.as_ref()
                .filter(|slot| slot.generation == generation)
                .map(|slot| slot.model.clone())
        });
        let Some(mut model) = model else {
            return Round::Gone;
        };
        let started = Instant::now();
        model.apply(due);
        let elapsed = started.elapsed();
        let Ok(mut slot) = self.live.lock() else {
            return Round::Gone;
        };
        let Some(slot) = slot.as_mut().filter(|slot| slot.generation == generation) else {
            return Round::Gone;
        };
        let status = model.status();
        slot.model = model;
        let changed = slot.publish(&status);
        log_round(&status.project_id, &cause(due), elapsed, changed);
        Round::Applied
    }

    fn live_reconcile(&self, app: &AppHandle, generation: u64, due: &BTreeSet<Change>) -> Round {
        let Ok(_compute_guard) = self.status_compute_lock.lock() else {
            return Round::Gone;
        };
        let workspace = self.live.lock().ok().and_then(|slot| {
            slot.as_ref()
                .filter(|slot| slot.generation == generation)
                .map(|slot| slot.workspace.clone())
        });
        let Some(workspace) = workspace else {
            return Round::Gone;
        };
        let workspace = current_workspace(app, &workspace).unwrap_or(workspace);
        let model = compute_live(&workspace);
        let started = model.started;
        let elapsed = started.elapsed();
        let Ok(mut slot) = self.live.lock() else {
            return Round::Gone;
        };
        let Some(slot) = slot.as_mut().filter(|slot| slot.generation == generation) else {
            return Round::Gone;
        };
        let changed = slot.adopt(workspace, model, due.contains(&Change::Rescan));
        log_round(&slot.model.project_id, &cause(due), elapsed, changed);
        Round::Reconciled(started)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrations::AgentLaunchSpec;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("kota-live-{label}-{nanos}"))
    }

    fn run(path: &Path, args: &[&str]) {
        if let Err(err) = git(path, args) {
            panic!("{err}");
        }
    }

    fn commit(path: &Path, message: &str) {
        run(path, &["add", "-A"]);
        run(
            path,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                message,
            ],
        );
    }

    /// A source repo on main that tracks a local origin, and one worktree per agent.
    fn room(label: &str, agent_ids: &[&str]) -> (PathBuf, WorkspaceProject) {
        let root = temp_dir(label);
        let source = root.join("source");
        fs::create_dir_all(&source).unwrap();
        run(&root, &["init", "--bare", "-b", "main", "origin.git"]);
        run(&source, &["init", "-b", "main"]);
        fs::write(source.join("README.md"), "hello\n").unwrap();
        commit(&source, "initial");
        let origin = root.join("origin.git");
        run(&source, &["remote", "add", "origin", origin.to_str().unwrap()]);
        run(&source, &["push", "-u", "origin", "main"]);
        let mut agents = Vec::new();
        for agent_id in agent_ids {
            let cwd = root.join(".agent-workspaces").join(agent_id);
            let worktree = cwd.join("project-files");
            fs::create_dir_all(&cwd).unwrap();
            let branch = format!("kota/{agent_id}");
            run(&source, &["worktree", "add", "-b", &branch, worktree.to_str().unwrap()]);
            agents.push(AgentLaunchSpec {
                agent_id: agent_id.to_string(),
                cli: crate::pty::agent::AgentCli::Codex.into(),
                cwd: path_string(&cwd),
                project_root: path_string(&root),
                worktree_root: path_string(&worktree),
                shared_dir: path_string(&root.join("project-memory")),
                rules_dir: path_string(&root.join("rules")),
                adapter_path: path_string(&cwd.join("AGENTS.md")),
                project_id: "proj".into(),
                project_remote: path_string(&origin),
                project_base_ref: "origin/main".into(),
            });
        }
        let workspace = WorkspaceProject {
            project_id: "proj".into(),
            repo_full_name: "mock/proj".into(),
            remote_url: path_string(&origin),
            github_html_url: "https://github.com/mock/proj".into(),
            default_branch: "main".into(),
            base_ref: "origin/main".into(),
            local_root: path_string(&root),
            local_root_bytes: 0,
            source_dir: path_string(&source),
            source_dir_bytes: 0,
            shared_dir: path_string(&root.join("project-memory")),
            rules_dir: path_string(&root.join("rules")),
            agents,
            archived: false,
            archived_at: None,
            display_name: None,
            connection: None,
        };
        (root, workspace)
    }

    fn agent_counts(model: &LiveModel, agent_id: &str) -> Option<(usize, usize)> {
        model
            .status()
            .dirty_agents
            .into_iter()
            .find(|agent| agent.agent_id == agent_id)
            .map(|agent| (agent.change_count, agent.pending_commit_count))
    }

    fn assert_matches_full_compute(model: &LiveModel, workspace: &WorkspaceProject) {
        let live = model.status();
        let full = compute_live(workspace).status();
        assert!(same_status(&live, &full), "live {live:?} != full {full:?}");
    }

    #[test]
    fn a_new_room_head_rereads_upstream_counts_and_every_agent_tree() {
        let (root, workspace) = room("source-head", &["alice", "bob"]);
        let source = PathBuf::from(&workspace.source_dir);
        let mut model = compute_live(&workspace);
        assert_eq!(model.status().state, "idle");

        fs::write(source.join("room.txt"), "room\n").unwrap();
        commit(&source, "room update");
        assert_eq!(model.status().state, "idle", "nothing is re-read before the event");

        model.apply(&BTreeSet::from([Change::SourceHead]));
        let status = model.status();
        assert_eq!(status.github_change_count, 1);
        // Both agents now lag the room head by room.txt, read tree to tree.
        assert_eq!(agent_counts(&model, "alice"), Some((1, 0)));
        assert_eq!(agent_counts(&model, "bob"), Some((1, 0)));
        assert_eq!(status.room_change_count, 1);
        assert_matches_full_compute(&model, &workspace);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn an_agent_commit_rereads_only_that_agent() {
        let (root, workspace) = room("agent-head", &["alice", "bob"]);
        let alice = PathBuf::from(&workspace.agents[0].worktree_root);
        let bob = PathBuf::from(&workspace.agents[1].worktree_root);
        let mut model = compute_live(&workspace);

        fs::write(alice.join("alice.txt"), "a\n").unwrap();
        commit(&alice, "alice work");
        fs::write(bob.join("bob.txt"), "b\n").unwrap();

        model.apply(&BTreeSet::from([Change::AgentHead("alice".into())]));
        assert_eq!(agent_counts(&model, "alice"), Some((1, 1)));
        assert_eq!(agent_counts(&model, "bob"), None, "bob is not re-read");
        assert_eq!(model.agents[0].commit_paths.len(), 1);

        model.apply(&BTreeSet::from([Change::AgentTree("bob".into())]));
        assert_eq!(agent_counts(&model, "bob"), Some((1, 0)));
        assert_matches_full_compute(&model, &workspace);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn worktree_events_skip_ignored_trees_and_merge_for_two_seconds() {
        let (root, workspace) = room("tree-events", &["alice"]);
        let alice_path = PathBuf::from(&workspace.agents[0].worktree_root);
        let mut model = compute_live(&workspace);
        let map = WatchMap::for_model(&model);
        let alice = canonical(&alice_path);
        let tree = vec![Change::AgentTree("alice".into())];
        assert_eq!(map.classify(&alice.join("src/main.rs")), tree);
        assert_eq!(map.classify(&alice.join("Cargo.lock")), tree);
        for skipped in [
            "node_modules/left-pad/index.js",
            "app-v2/node_modules/react/index.js",
            "target/debug/kota",
            "app-v2/dist/index.js",
            ".git",
        ] {
            assert!(map.classify(&alice.join(skipped)).is_empty(), "{skipped}");
        }
        let source = canonical(Path::new(&workspace.source_dir));
        assert_eq!(map.classify(&source.join("README.md")), vec![Change::SourceTree]);

        fs::write(alice_path.join("notes.md"), "draft\n").unwrap();
        model.apply(&BTreeSet::from([Change::AgentTree("alice".into())]));
        assert_eq!(agent_counts(&model, "alice"), Some((1, 0)));

        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut pending = Pending::default();
        pending.note(Change::AgentTree("alice".into()), t0);
        pending.note(Change::AgentTree("bob".into()), t0 + ms(500));
        pending.note(Change::AgentTree("alice".into()), t0 + ms(1500));
        assert!(pending.take_due(t0 + ms(1999)).is_empty());
        assert_eq!(
            pending.take_due(t0 + MERGE_WINDOW),
            BTreeSet::from([Change::AgentTree("alice".into())])
        );
        assert_eq!(
            pending.take_due(t0 + ms(2500)),
            BTreeSet::from([Change::AgentTree("bob".into())])
        );
        // A full compute that began later already saw what was noted before it.
        pending.note(Change::SourceTree, t0 + ms(3000));
        pending.note(Change::Upstream, t0 + ms(5000));
        pending.forget_before(t0 + ms(4000));
        assert_eq!(pending.next_due(), Some(t0 + ms(7000)));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn git_metadata_maps_to_heads_upstream_and_index_but_not_fetch_logs_or_locks() {
        let (root, workspace) = room("git-events", &["alice"]);
        let model = compute_live(&workspace);
        let map = WatchMap::for_model(&model);
        let git_dir = canonical(&PathBuf::from(&workspace.source_dir).join(".git"));
        let at = |rel: &str| map.classify(&git_dir.join(rel));
        assert_eq!(at("HEAD"), vec![Change::SourceHead]);
        assert_eq!(at("index"), vec![Change::SourceTree]);
        assert_eq!(at("refs/heads/main"), vec![Change::SourceHead]);
        assert_eq!(at("refs/heads/kota/alice"), vec![Change::AgentHead("alice".into())]);
        assert_eq!(at("refs/remotes/origin/main"), vec![Change::Upstream]);
        assert_eq!(at("config"), vec![Change::Upstream]);
        assert_eq!(at("packed-refs"), vec![Change::AllHeads]);
        for skipped in [
            "FETCH_HEAD",
            "ORIG_HEAD",
            "logs/HEAD",
            "logs/refs/heads/main",
            "objects/ab/cdef",
            "refs/heads/main.lock",
            "index.lock",
            "refs/heads/someone/else",
            "worktrees/someone-else/HEAD",
        ] {
            assert!(at(skipped).is_empty(), "{skipped}");
        }
        let alice_git = map.agents[0].1.git_dir.clone();
        assert!(alice_git.starts_with(&git_dir));
        assert_eq!(
            map.classify(&alice_git.join("HEAD")),
            vec![Change::AgentHead("alice".into())]
        );
        assert_eq!(
            map.classify(&alice_git.join("index")),
            vec![Change::AgentTree("alice".into())]
        );
        assert!(map.classify(&alice_git.join("logs/HEAD")).is_empty());
        assert!(map.classify(&alice_git.join("index.lock")).is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn an_archived_agent_leaves_the_model_and_the_watch_roots() {
        let (root, workspace) = room("archive", &["alice", "bob"]);
        let bob = canonical(Path::new(&workspace.agents[1].worktree_root));
        let before = WatchMap::for_model(&compute_live(&workspace));
        assert!(before.roots().contains(&bob));
        assert_eq!(before.classify(&bob.join("work.txt")), vec![Change::AgentTree("bob".into())]);

        let key = status_cache_key(&workspace);
        fs::write(Path::new(&workspace.agents[1].cwd).join("agent.yaml"), "status: archived\n")
            .unwrap();
        // A new key is what makes Bartender reconcile and re-watch.
        assert_ne!(status_cache_key(&workspace), key);
        fs::write(bob.join("work.txt"), "x\n").unwrap();
        let model = compute_live(&workspace);
        assert!(model.agents.iter().all(|agent| agent.agent_id != "bob"));
        assert_eq!(model.status().room_change_count, 0);
        let after = WatchMap::for_model(&model);
        assert!(!after.roots().contains(&bob));
        assert!(after.classify(&bob.join("work.txt")).is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn porcelain_v2_records_yield_every_changed_path() {
        let out = concat!(
            "1 .M N... 100644 100644 100644 aaa bbb src/a b.rs\0",
            "2 R. N... 100644 100644 100644 aaa bbb R100 new.rs\0old.rs\0",
            "u UU N... 100644 100644 100644 100644 aaa bbb ccc conflict.rs\0",
            "? dir/untracked.txt\0",
            "! ignored.log\0",
        );
        let expected = ["src/a b.rs", "new.rs", "old.rs", "conflict.rs", "dir/untracked.txt"]
            .map(String::from);
        assert_eq!(porcelain_v2_paths(out), BTreeSet::from(expected));
    }
}
