//! Search-only, bounded name snapshots. Browsing keeps its existing listing path.
//! One native catalogue per window/generation; keyword edits never walk or run Git.

use super::{
    agent_diff_identity_for_spec, ghost_project_file_entry, integrations, normalize_tree_path,
    tree_change_overview_from_status, workspace_tree_entry_with_overview,
    workspace_tree_file_change, workspace_tree_relative_path, workspace_tree_root_path,
    worktree_changed_files_with_reader, IntegrationManager, ProjectTreeDiffIndex,
    WorkspaceTreeChangeOverview, WorkspaceTreeChangeParticipant, WorkspaceTreeEntry,
    WorkspaceTreeRootInfo, WorkspaceTreeRootKind,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
use tauri::Manager;
use tokio::sync::{OnceCell, Semaphore};

const INDEX_LIMIT: usize = 200_000;
const MATCH_LIMIT: usize = 2_000;
const MAX_SNAPSHOTS: usize = 2;
const IDLE_TTL: Duration = Duration::from_secs(10 * 60);
const CANCELLED: &str = "name search cancelled";
const EXPIRED: &str = "Search snapshot expired. Clear search and try again.";

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SearchRequest {
    pub project_id: String,
    pub root_kind: WorkspaceTreeRootKind,
    pub include_ignored: bool,
    // Monotonic within this window, including remounts. An old begin cannot win
    // over a newer begin/cancel even if native command scheduling is reordered.
    pub generation: u64,
    pub query_revision: u64,
    pub query: String,
    #[serde(default)]
    pub cancel: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SearchResponse {
    pub generation: u64,
    pub query_revision: u64,
    pub root: Option<WorkspaceTreeRootInfo>,
    pub entries: Vec<WorkspaceTreeEntry>,
    pub matched_count: usize,
    pub indexed_count: usize,
    pub truncated: bool,
    pub index_truncated: bool,
    pub read_failed: bool,
    pub git_failed: bool,
    pub cancelled: bool,
}

impl SearchResponse {
    fn cancelled(request: &SearchRequest) -> Self {
        Self {
            generation: request.generation,
            query_revision: request.query_revision,
            cancelled: true,
            ..Self::default()
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Scope {
    project: String,
    root: WorkspaceTreeRootKind,
    include_ignored: bool,
}
impl From<&SearchRequest> for Scope {
    fn from(r: &SearchRequest) -> Self {
        Self {
            project: r.project_id.clone(),
            root: r.root_kind,
            include_ignored: r.include_ignored,
        }
    }
}

struct Slot {
    cancelled: Arc<AtomicBool>,
    latest_query: AtomicU64,
    value: OnceCell<Result<Arc<Catalogue>, String>>,
}
impl Slot {
    fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            latest_query: AtomicU64::new(0),
            value: OnceCell::new(),
        }
    }
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}
struct Owner {
    generation: u64,
    scope: Scope,
    touched: Instant,
    // Keep the generation tombstone after expiry/cancel to reject delayed begins.
    slot: Option<Arc<Slot>>,
    expired: bool,
}
struct Registry {
    owners: Mutex<HashMap<String, Owner>>,
    builders: Arc<Semaphore>,
}
impl Default for Registry {
    fn default() -> Self {
        Self {
            owners: Mutex::new(HashMap::new()),
            builders: Arc::new(Semaphore::new(1)),
        }
    }
}

impl Registry {
    fn prune(&self, now: Instant) {
        let mut owners = self.owners.lock().unwrap();
        for owner in owners.values_mut() {
            if now.saturating_duration_since(owner.touched) >= IDLE_TTL {
                if let Some(slot) = owner.slot.take() {
                    slot.cancel();
                    owner.expired = true;
                }
            }
        }
    }

    fn checkout(&self, window: &str, r: &SearchRequest) -> Result<Option<Arc<Slot>>, String> {
        let now = Instant::now();
        self.prune(now);
        let mut owners = self.owners.lock().unwrap();
        let scope = Scope::from(r);
        if let Some(owner) = owners.get_mut(window) {
            if r.generation < owner.generation {
                return Ok(None);
            }
            if r.generation == owner.generation {
                if owner.scope != scope {
                    return Err("name search generation scope mismatch".into());
                }
                owner.touched = now;
                if r.cancel || r.query.trim().is_empty() {
                    if let Some(slot) = owner.slot.take() {
                        slot.cancel();
                    }
                    owner.expired = false;
                    return Ok(None);
                }
                if owner.expired {
                    return Err(EXPIRED.into());
                }
                return Ok(owner.slot.clone());
            }
        }
        if let Some(old) = owners.remove(window).and_then(|o| o.slot) {
            old.cancel();
        }
        let cancel = r.cancel || r.query.trim().is_empty();
        if !cancel {
            while owners.values().filter(|o| o.slot.is_some()).count() >= MAX_SNAPSHOTS {
                let oldest = owners
                    .iter()
                    .filter(|(_, o)| o.slot.is_some())
                    .min_by_key(|(_, o)| o.touched)
                    .map(|(key, _)| key.clone())
                    .unwrap();
                let old = owners.get_mut(&oldest).unwrap();
                old.slot.take().unwrap().cancel();
                old.expired = true;
            }
        }
        let slot = (!cancel).then(|| Arc::new(Slot::new()));
        owners.insert(
            window.to_string(),
            Owner {
                generation: r.generation,
                scope,
                touched: now,
                slot: slot.clone(),
                expired: false,
            },
        );
        Ok(slot)
    }

    fn current(&self, window: &str, r: &SearchRequest, slot: &Arc<Slot>) -> Result<bool, String> {
        let owners = self.owners.lock().unwrap();
        let Some(owner) = owners.get(window) else {
            return Ok(false);
        };
        if owner.generation != r.generation {
            return Ok(false);
        }
        if owner.expired {
            return Err(EXPIRED.into());
        }
        Ok(owner.slot.as_ref().is_some_and(|s| Arc::ptr_eq(s, slot))
            && !slot.cancelled.load(Ordering::Acquire)
            && slot.latest_query.load(Ordering::Acquire) == r.query_revision)
    }

    fn release_window(&self, window: &str) {
        if let Some(slot) = self
            .owners
            .lock()
            .unwrap()
            .remove(window)
            .and_then(|o| o.slot)
        {
            slot.cancel();
        }
    }

    async fn search<F>(
        &self,
        window: &str,
        r: SearchRequest,
        load_workspace: F,
    ) -> Result<SearchResponse, String>
    where
        F: FnOnce() -> Result<integrations::WorkspaceProject, String> + Send + 'static,
    {
        let Some(slot) = self.checkout(window, &r)? else {
            return Ok(SearchResponse::cancelled(&r));
        };
        if r.query_revision
            < slot
                .latest_query
                .fetch_max(r.query_revision, Ordering::AcqRel)
        {
            return Ok(SearchResponse::cancelled(&r));
        }
        let cancelled = slot.cancelled.clone();
        let scope = Scope::from(&r);
        // Only the initializer resolves/prepares the workspace. Waiting keyword
        // requests await this cell, not a blocking thread or a global I/O lock.
        let value = slot
            .value
            .get_or_init(|| async {
                let _permit = self
                    .builders
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|e| e.to_string())?;
                check_cancelled(&cancelled)?;
                tauri::async_runtime::spawn_blocking(move || {
                    let workspace = load_workspace()?;
                    Catalogue::build(
                        &workspace,
                        scope.root,
                        scope.include_ignored,
                        &cancelled,
                        INDEX_LIMIT,
                    )
                    .map(Arc::new)
                })
                .await
                .map_err(|e| format!("join name index: {e}"))?
            })
            .await;
        if !self.current(window, &r, &slot)? {
            return Ok(SearchResponse::cancelled(&r));
        }
        let catalogue = value.clone()?;
        let request = r.clone();
        let cancelled = slot.cancelled.clone();
        let response = tauri::async_runtime::spawn_blocking(move || {
            catalogue.query(&request, &cancelled, MATCH_LIMIT)
        })
        .await
        .map_err(|e| format!("join name filter: {e}"))??;
        if self.current(window, &r, &slot)? {
            Ok(response)
        } else {
            Ok(SearchResponse::cancelled(&r))
        }
    }
}

static REGISTRY: OnceLock<Arc<Registry>> = OnceLock::new();
fn registry() -> Arc<Registry> {
    REGISTRY
        .get_or_init(|| {
            let registry = Arc::new(Registry::default());
            let weak = Arc::downgrade(&registry);
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    let Some(registry) = weak.upgrade() else {
                        break;
                    };
                    // Memory lifecycle only: never polls files or refreshes a snapshot.
                    registry.prune(Instant::now());
                }
            });
            registry
        })
        .clone()
}

pub(super) fn release_window(window: &str) {
    if let Some(registry) = REGISTRY.get() {
        registry.release_window(window);
    }
}

#[tauri::command]
pub(super) async fn workspace_search_names(
    app: tauri::AppHandle,
    window: tauri::Window,
    request: SearchRequest,
) -> Result<SearchResponse, String> {
    let project = request.project_id.clone();
    let label = window.label().to_string();
    if app.get_webview_window(&label).is_none() {
        return Ok(SearchResponse::cancelled(&request));
    }
    let registry = registry();
    let loader_registry = registry.clone();
    let loader_label = label.clone();
    registry
        .search(&label, request, move || {
            // Covers destruction between command dispatch and initial checkout.
            // Cached queries are fenced by the normal destroyed-window callback.
            if app.get_webview_window(&loader_label).is_none() {
                loader_registry.release_window(&loader_label);
                return Err(CANCELLED.into());
            }
            app.state::<IntegrationManager>()
                .workspace_project(project)
                .map_err(|e| e.to_string())
        })
        .await
}

struct IndexedEntry {
    folded_name: String,
    entry: WorkspaceTreeEntry,
}
struct Catalogue {
    root: WorkspaceTreeRootInfo,
    entries: BTreeMap<String, IndexedEntry>,
    index_truncated: bool,
    read_failed: bool,
    git_failed: bool,
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Acquire) {
        Err(CANCELLED.into())
    } else {
        Ok(())
    }
}

fn is_vcs_metadata_name(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        let name = name.to_string_lossy();
        [".git", ".hg", ".svn"]
            .iter()
            .any(|skip| name.eq_ignore_ascii_case(skip))
    })
}

fn git_read(path: &Path, args: &[&str], cancelled: &AtomicBool) -> Result<String, String> {
    check_cancelled(cancelled)?;
    #[cfg(test)]
    record_git_read(path);
    let output = Command::new("git")
        .current_dir(path)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn capture_diff(
    workspace: &integrations::WorkspaceProject,
    cancelled: &AtomicBool,
) -> Result<ProjectTreeDiffIndex, String> {
    let source = Path::new(&workspace.source_dir);
    let head = git_read(source, &["rev-parse", "--verify", "HEAD"], cancelled)?
        .trim()
        .to_string();
    let mut participants = BTreeMap::<String, Vec<WorkspaceTreeChangeParticipant>>::new();
    let mut append =
        |path: &Path, base: &str, actor: WorkspaceTreeChangeParticipant| -> Result<(), String> {
            let changes = worktree_changed_files_with_reader(path, base, |path, args| {
                git_read(path, args, cancelled)
            })?;
            for (path, change) in changes {
                let mut participant = actor.clone();
                participant.status = change.status;
                participant.added_lines = change.added_lines;
                participant.deleted_lines = change.deleted_lines;
                participants.entry(path).or_default().push(participant);
            }
            Ok(())
        };
    append(
        source,
        &head,
        WorkspaceTreeChangeParticipant {
            actor_id: "human".into(),
            display_name: "Human".into(),
            aka: "Human".into(),
            kind: "human".into(),
            provider: None,
            avatar_id: None,
            status: String::new(),
            added_lines: None,
            deleted_lines: None,
        },
    )?;
    for agent in workspace
        .agents
        .iter()
        .filter(|a| integrations::agent_launch_spec_is_active(a))
    {
        check_cancelled(cancelled)?;
        let identity = agent_diff_identity_for_spec(agent);
        append(
            Path::new(&agent.worktree_root),
            &head,
            WorkspaceTreeChangeParticipant {
                actor_id: agent.agent_id.clone(),
                display_name: identity.display_name,
                aka: identity.aka,
                kind: "agent".into(),
                provider: identity.provider,
                avatar_id: identity.avatar_id,
                status: String::new(),
                added_lines: None,
                deleted_lines: None,
            },
        )?;
    }
    Ok(ProjectTreeDiffIndex {
        files: participants
            .into_iter()
            .map(|(path, actors)| (path, workspace_tree_file_change(actors)))
            .collect(),
    })
}

impl Catalogue {
    fn build(
        workspace: &integrations::WorkspaceProject,
        kind: WorkspaceTreeRootKind,
        include_ignored: bool,
        cancelled: &AtomicBool,
        limit: usize,
    ) -> Result<Self, String> {
        check_cancelled(cancelled)?;
        let root = workspace_tree_root_path(workspace, kind);
        let git_failed = AtomicBool::new(false);
        let overviews = Mutex::new(HashMap::<
            PathBuf,
            Arc<OnceLock<Option<WorkspaceTreeChangeOverview>>>,
        >::new());
        let overview = |path: &Path, metadata: &fs::Metadata| {
            if !(metadata.is_dir() || metadata.file_type().is_symlink())
                || !path.join(".git").exists()
            {
                return None;
            }
            // Preserve existing repository badges, including aliases, without
            // repeating status per alias or per query. This never adds target
            // filenames: ignore's walker itself never descends a symlink.
            let canonical = path.canonicalize().unwrap_or_else(|_| path.to_owned());
            let value = overviews
                .lock()
                .unwrap()
                .entry(canonical)
                .or_default()
                .clone();
            value
                .get_or_init(|| {
                    match git_read(path, &["status", "--porcelain=v1", "-z"], cancelled) {
                        Ok(status) => tree_change_overview_from_status(status.as_bytes()),
                        Err(_) => {
                            git_failed.store(true, Ordering::Release);
                            None
                        }
                    }
                })
                .clone()
        };
        let root_metadata = fs::symlink_metadata(&root);
        let mut catalogue = Self {
            root: WorkspaceTreeRootInfo {
                kind,
                label: match kind {
                    WorkspaceTreeRootKind::ProjectFiles => "Project Files",
                    WorkspaceTreeRootKind::ProjectWorkspace => "Project Workspace",
                }
                .into(),
                absolute_path: root.display().to_string(),
                change_overview: root_metadata
                    .as_ref()
                    .ok()
                    .filter(|m| m.is_dir())
                    .and_then(|m| overview(&root, m)),
            },
            entries: BTreeMap::new(),
            index_truncated: false,
            read_failed: false,
            git_failed: false,
        };
        if !root_metadata.is_ok_and(|m| m.is_dir()) {
            catalogue.read_failed = true;
            catalogue.git_failed = git_failed.load(Ordering::Acquire);
            return Ok(catalogue);
        }
        let index = if kind == WorkspaceTreeRootKind::ProjectFiles {
            match capture_diff(workspace, cancelled) {
                Ok(index) => Some(index),
                Err(_) => {
                    git_failed.store(true, Ordering::Release);
                    None
                }
            }
        } else {
            None
        };
        check_cancelled(cancelled)?;
        // Seed changed/agent-only paths and their parents before physical entries.
        // These are an explicit Git overlay, not hidden by the physical ignore rule.
        if let Some(index) = index.as_ref() {
            let mut paths = BTreeSet::new();
            let mut directory_names = HashMap::<PathBuf, Result<BTreeSet<String>, String>>::new();
            for path in index.files.keys() {
                let relative = workspace_tree_relative_path(Some(path))?;
                let mut visible = PathBuf::new();
                for component in relative.components() {
                    visible.push(component.as_os_str());
                    if is_vcs_metadata_name(&visible) {
                        break;
                    }
                }
                for parent in visible.ancestors().filter(|p| !p.as_os_str().is_empty()) {
                    paths.insert(normalize_tree_path(parent));
                }
            }
            for path in paths {
                check_cancelled(cancelled)?;
                let relative = Path::new(&path);
                if relative
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .is_some_and(|parent| {
                        catalogue
                            .entries
                            .get(&normalize_tree_path(parent))
                            .is_none_or(|e| e.entry.kind != "folder")
                    })
                {
                    continue;
                }
                if catalogue.entries.len() >= limit {
                    catalogue.index_truncated = true;
                    break;
                }
                let physical = root.join(relative);
                let parent_ghost = relative
                    .parent()
                    .and_then(|p| catalogue.entries.get(&normalize_tree_path(p)))
                    .is_some_and(|e| e.entry.is_ghost);
                // Match the browser's exact existing_names semantics. lstat alone
                // incorrectly calls an agent's case-only rename a physical source
                // file on case-insensitive macOS filesystems. Cache names only for
                // changed-path parents; ignore still owns whole-tree traversal.
                let exists_exactly = if parent_ghost {
                    Ok(false)
                } else {
                    let parent = physical.parent().unwrap();
                    directory_names
                        .entry(parent.to_owned())
                        .or_insert_with(|| {
                            fs::read_dir(parent)
                                .and_then(|dir| {
                                    dir.map(|entry| {
                                        entry.map(|e| e.file_name().to_string_lossy().into_owned())
                                    })
                                    .collect::<Result<BTreeSet<_>, _>>()
                                })
                                .map_err(|e| e.to_string())
                        })
                        .as_ref()
                        .map(|names| {
                            names.contains(relative.file_name().unwrap().to_string_lossy().as_ref())
                        })
                        .map_err(Clone::clone)
                };
                let entry = match exists_exactly {
                    Ok(true) => workspace_tree_entry_with_overview(
                        workspace,
                        &root,
                        relative,
                        &physical,
                        Some(index),
                        &overview,
                    ),
                    Ok(false) => {
                        let prefix = format!("{path}/");
                        let folder = index
                            .files
                            .range(prefix.clone()..)
                            .next()
                            .is_some_and(|(p, _)| p.starts_with(&prefix));
                        Ok(ghost_project_file_entry(
                            workspace,
                            &root,
                            index,
                            path.clone(),
                            folder,
                        ))
                    }
                    Err(e) => Err(e),
                };
                match entry {
                    Ok(entry) => {
                        catalogue.entries.insert(
                            path,
                            IndexedEntry {
                                folded_name: entry.name.to_lowercase(),
                                entry,
                            },
                        );
                    }
                    Err(_) => catalogue.read_failed = true,
                }
            }
        }
        let entries = Mutex::new(catalogue.entries);
        let truncated = AtomicBool::new(catalogue.index_truncated);
        let read_failed = AtomicBool::new(catalogue.read_failed);
        let mut walker = ignore::WalkBuilder::new(&root);
        walker
            .standard_filters(false)
            .git_ignore(!include_ignored)
            .require_git(false) // Workspace roots need not themselves be repositories.
            .follow_links(false);
        walker.build_parallel().run(|| {
            Box::new(|result| {
                if cancelled.load(Ordering::Acquire) {
                    return ignore::WalkState::Quit;
                }
                let entry = match result {
                    Ok(entry) => entry,
                    Err(_) => {
                        read_failed.store(true, Ordering::Release);
                        return ignore::WalkState::Continue;
                    }
                };
                if entry.error().is_some() {
                    read_failed.store(true, Ordering::Release);
                }
                if entry.path() == root {
                    return ignore::WalkState::Continue;
                }
                let Ok(relative) = entry.path().strip_prefix(&root) else {
                    read_failed.store(true, Ordering::Release);
                    return ignore::WalkState::Skip;
                };
                let key = normalize_tree_path(relative);
                // Keep the VCS directory node, but never enumerate its contents.
                // node_modules is deliberately NOT here: only .gitignore governs it.
                let traversal = if entry.file_type().is_some_and(|t| t.is_dir())
                    && is_vcs_metadata_name(relative)
                {
                    ignore::WalkState::Skip
                } else {
                    ignore::WalkState::Continue
                };
                {
                    let entries = entries.lock().unwrap();
                    if entries.contains_key(&key) {
                        return traversal;
                    }
                    if entries.len() >= limit {
                        truncated.store(true, Ordering::Release);
                        return ignore::WalkState::Quit;
                    }
                }
                match workspace_tree_entry_with_overview(
                    workspace,
                    &root,
                    relative,
                    entry.path(),
                    index.as_ref(),
                    &overview,
                ) {
                    Ok(entry) => {
                        let mut entries = entries.lock().unwrap();
                        if entries.len() >= limit {
                            truncated.store(true, Ordering::Release);
                            return ignore::WalkState::Quit;
                        }
                        entries.insert(
                            key,
                            IndexedEntry {
                                folded_name: entry.name.to_lowercase(),
                                entry,
                            },
                        );
                    }
                    Err(_) => {
                        read_failed.store(true, Ordering::Release);
                    }
                }
                traversal
            })
        });
        check_cancelled(cancelled)?;
        catalogue.entries = entries.into_inner().unwrap();
        catalogue.index_truncated = truncated.load(Ordering::Acquire);
        catalogue.read_failed = read_failed.load(Ordering::Acquire);
        catalogue.git_failed = git_failed.load(Ordering::Acquire);
        Ok(catalogue)
    }

    fn query(
        &self,
        r: &SearchRequest,
        cancelled: &AtomicBool,
        limit: usize,
    ) -> Result<SearchResponse, String> {
        let query = r.query.trim().to_lowercase();
        let mut paths = BTreeSet::new();
        let mut matched = 0;
        let mut truncated = false;
        for (path, indexed) in &self.entries {
            check_cancelled(cancelled)?;
            if !indexed.folded_name.contains(&query) {
                continue;
            }
            if matched == limit {
                truncated = true;
                break;
            }
            matched += 1;
            let mut path = path.as_str();
            while !path.is_empty() && paths.insert(path) {
                path = path.rsplit_once('/').map_or("", |(parent, _)| parent);
            }
        }
        let mut entries: Vec<_> = paths
            .into_iter()
            .filter_map(|p| self.entries.get(p))
            .map(|e| e.entry.clone())
            .collect();
        // Same folder/symlink/file + case-insensitive ordering as ordinary listing.
        entries.sort_by(|a, b| {
            let rank = |kind: &str| match kind {
                "folder" => 0,
                "symlink" => 1,
                _ => 2,
            };
            rank(&a.kind)
                .cmp(&rank(&b.kind))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(SearchResponse {
            generation: r.generation,
            query_revision: r.query_revision,
            root: Some(self.root.clone()),
            entries,
            matched_count: matched,
            indexed_count: self.entries.len(),
            truncated,
            index_truncated: self.index_truncated,
            read_failed: self.read_failed,
            git_failed: self.git_failed,
            cancelled: false,
        })
    }
}

#[cfg(test)]
pub(super) fn record_git_read(path: &Path) {
    tests::record_git_read(path);
}
#[cfg(test)]
mod tests;
