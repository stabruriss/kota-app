use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock, TryLockError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::bail;
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use notify::{RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

use crate::agent_bus::AgentBusManager;
use crate::integrations::{agent_launch_spec_is_active, IntegrationManager, WorkspaceProject};
use crate::pty::PtyManager;

#[derive(Default)]
pub struct BartenderManager {
    sync_lock: Mutex<()>,
    status_compute_lock: Mutex<()>,
    status_cache: Mutex<Option<CachedBartenderStatus>>,
    dispatch_watchers: Mutex<HashMap<PathBuf, BartenderDispatchWatch>>,
}

const BARTENDER_STATUS_CACHE_TTL: Duration = Duration::from_secs(5);
const BARTENDER_DISPATCH_SCHEMA: &str = "kota.bartender.dispatch.v1";
pub const BARTENDER_SYNC_EVENT: &str = "bartender-sync-local";
pub const BARTENDER_SYNC_PROGRESS_EVENT: &str = "bartender-sync-progress";
const KOTA_HOME_DIR: &str = "Kota";
const CLI_SYNC_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const CLI_SYNC_POLL_INTERVAL: Duration = Duration::from_millis(250);

struct BartenderDispatchWatch {
    _watcher: notify::RecommendedWatcher,
    watched_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BartenderStatusCacheKey {
    project_id: String,
    source_dir: String,
    local_root: String,
    default_branch: String,
    agents: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
struct CachedBartenderStatus {
    key: BartenderStatusCacheKey,
    checked_at: Instant,
    status: BartenderStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BartenderDispatchFile {
    schema: String,
    created_at: String,
    project_root: String,
    request_id: String,
    action: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BartenderSyncEvent {
    pub project_root: String,
    pub request_id: String,
    pub phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<BartenderSyncResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BartenderSyncReceipt {
    pub project_root: String,
    pub request_id: String,
    pub phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BartenderSyncProgressEvent {
    pub project_root: String,
    pub phase: String,
    pub message: String,
    pub elapsed_ms: u128,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BartenderRequest {
    pub project_root: Option<String>,
    pub conflict_prompt: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BartenderDirtyAgent {
    pub agent_id: String,
    pub path: String,
    pub change_count: usize,
    pub pending_commit_count: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BartenderStatus {
    pub project_id: String,
    pub source_dir: String,
    pub default_branch: String,
    pub room_change_count: usize,
    pub source_change_count: usize,
    pub github_change_count: usize,
    pub github_behind_count: usize,
    pub github_needs_initial_push: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github_push_branch: Option<String>,
    pub github_initial_push_commit_count: usize,
    pub dirty_agents: Vec<BartenderDirtyAgent>,
    pub checked_at: String,
    pub state: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BartenderPublishedAgent {
    pub agent_id: String,
    pub commit_count: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BartenderConflict {
    pub agent_id: String,
    pub commit: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BartenderSyncResult {
    pub ok: bool,
    pub message: String,
    pub snapshot_count: usize,
    pub published_commit_count: usize,
    pub published_agents: Vec<BartenderPublishedAgent>,
    pub conflicts: Vec<BartenderConflict>,
    pub status: BartenderStatus,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BartenderPushResult {
    pub ok: bool,
    pub message: String,
    pub pushed_commit_count: usize,
    pub status: BartenderStatus,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BartenderFetchResult {
    pub ok: bool,
    pub message: String,
    pub status: BartenderStatus,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BartenderPullConflict {
    pub message: String,
    pub source_head: String,
    pub upstream: String,
    pub upstream_head: String,
    pub default_branch: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BartenderPullResult {
    pub ok: bool,
    pub message: String,
    pub pulled_commit_count: usize,
    pub needs_human_pick: bool,
    pub conflict: Option<BartenderPullConflict>,
    pub status: BartenderStatus,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BartenderRoutePullConflictRequest {
    pub project_root: Option<String>,
    pub agent_id: String,
    pub pull_conflict_prompt: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BartenderRoutePullConflictResult {
    pub ok: bool,
    pub message: String,
    pub status: BartenderStatus,
}

#[derive(Clone, Debug)]
struct AgentSyncTarget {
    agent_id: String,
    path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct InitialPushStatus {
    branch: String,
    commit_count: usize,
}

struct BartenderSyncProgress {
    app: AppHandle,
    project_root: String,
    started_at: Instant,
}

impl BartenderSyncProgress {
    fn new(app: &AppHandle, workspace: &WorkspaceProject) -> Self {
        Self {
            app: app.clone(),
            project_root: workspace.local_root.clone(),
            started_at: Instant::now(),
        }
    }

    fn emit(&self, phase: &str, message: &str) {
        let _ = self.app.emit(
            BARTENDER_SYNC_PROGRESS_EVENT,
            BartenderSyncProgressEvent {
                project_root: self.project_root.clone(),
                phase: phase.to_string(),
                message: message.to_string(),
                elapsed_ms: self.started_at.elapsed().as_millis(),
            },
        );
    }

    fn elapsed_ms(&self) -> u128 {
        self.started_at.elapsed().as_millis()
    }
}

#[derive(Clone, Debug)]
struct PublishOutcome {
    published_commits: usize,
    conflict: Option<BartenderConflict>,
}

impl BartenderManager {
    pub fn status(&self, workspace: &WorkspaceProject) -> Result<BartenderStatus> {
        let key = status_cache_key(workspace);
        if let Some(status) = self.cached_status(&key)? {
            return Ok(status);
        }
        let _compute_guard = self
            .status_compute_lock
            .lock()
            .map_err(|_| anyhow!("bartender status compute lock poisoned"))?;
        if let Some(status) = self.cached_status(&key)? {
            return Ok(status);
        }
        let status = compute_status(workspace)?;
        self.store_status(key, status.clone())?;
        Ok(status)
    }

    pub fn sync_local(&self, workspace: &WorkspaceProject) -> Result<BartenderSyncResult> {
        self.sync_local_impl(workspace, None)
    }

    pub fn sync_local_with_progress(
        &self,
        app: &AppHandle,
        workspace: &WorkspaceProject,
    ) -> Result<BartenderSyncResult> {
        let progress = BartenderSyncProgress::new(app, workspace);
        self.sync_local_impl(workspace, Some(&progress))
    }

    fn sync_local_impl(
        &self,
        workspace: &WorkspaceProject,
        progress: Option<&BartenderSyncProgress>,
    ) -> Result<BartenderSyncResult> {
        if let Some(progress) = progress {
            progress.emit("starting", "Starting local sync");
        }
        let _guard = match self.sync_lock.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => bail!("Bartender sync is already running."),
            Err(TryLockError::Poisoned(_)) => bail!("bartender sync lock poisoned"),
        };
        if let Some(progress) = progress {
            progress.emit("waiting_status", "Waiting for status check");
        }
        let lock_started = Instant::now();
        let _compute_guard = self
            .status_compute_lock
            .lock()
            .map_err(|_| anyhow!("bartender status compute lock poisoned"))?;
        let lock_wait = lock_started.elapsed();
        if lock_wait > Duration::from_millis(250) {
            crate::kota_debug_log(&format!(
                "[bartender] sync waited {}ms for status_compute_lock",
                lock_wait.as_millis()
            ));
        }
        if let Some(progress) = progress {
            progress.emit("preparing", "Preparing local sync");
        }
        self.invalidate_status_cache()?;
        let result = sync_local_inner(workspace, progress);
        if let (Some(progress), Err(err)) = (progress, &result) {
            progress.emit("failed", &format!("Sync failed: {err}"));
            crate::kota_debug_log(&format!(
                "[bartender] sync failed elapsed={}ms {err}",
                progress.elapsed_ms()
            ));
        }
        if let (Some(progress), Ok(result)) = (progress, &result) {
            let elapsed_ms = progress.elapsed_ms();
            if elapsed_ms > 10_000 {
                crate::kota_debug_log(&format!(
                    "[bartender] sync slow elapsed={}ms ok={} snapshots={} published={} conflicts={}",
                    elapsed_ms,
                    result.ok,
                    result.snapshot_count,
                    result.published_commit_count,
                    result.conflicts.len()
                ));
            }
        }
        if let Ok(result) = &result {
            self.store_status(status_cache_key(workspace), result.status.clone())?;
        }
        result
    }

    pub fn refresh_dispatch_watcher(&self, app: &AppHandle, project_root: &Path) -> Result<()> {
        let outbox = dispatch_outbox_dir(project_root);
        fs::create_dir_all(&outbox)?;
        self.process_dispatch_outbox(app, project_root)?;

        let key = project_root
            .canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf());
        let mut watchers = self
            .dispatch_watchers
            .lock()
            .map_err(|_| anyhow!("bartender dispatch watcher lock poisoned"))?;
        if watchers
            .get(&key)
            .is_some_and(|existing| existing.watched_path == outbox)
        {
            return Ok(());
        }

        let app_handle = app.clone();
        let project_root_for_callback = project_root.to_path_buf();
        let last_process = std::sync::Arc::new(Mutex::new(
            Instant::now()
                .checked_sub(Duration::from_secs(60))
                .unwrap_or_else(Instant::now),
        ));
        let last_process_for_callback = std::sync::Arc::clone(&last_process);
        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                let Ok(event) = res else {
                    return;
                };
                if event.paths.is_empty() {
                    return;
                }
                let now = Instant::now();
                if let Ok(mut last) = last_process_for_callback.lock() {
                    if now.duration_since(*last) < Duration::from_millis(250) {
                        return;
                    }
                    *last = now;
                }
                let app_for_process = app_handle.clone();
                let project_root_for_process = project_root_for_callback.clone();
                thread::spawn(move || {
                    let bartender = app_for_process.state::<BartenderManager>();
                    if let Err(err) = bartender
                        .process_dispatch_outbox(&app_for_process, &project_root_for_process)
                    {
                        crate::kota_debug_log(&format!(
                            "[bartender] process dispatch outbox failed: {err}"
                        ));
                    }
                });
            })?;
        watcher.watch(&outbox, RecursiveMode::NonRecursive)?;
        watchers.insert(
            key,
            BartenderDispatchWatch {
                _watcher: watcher,
                watched_path: outbox,
            },
        );
        Ok(())
    }

    pub fn process_dispatch_outbox(&self, app: &AppHandle, project_root: &Path) -> Result<()> {
        let outbox = dispatch_outbox_dir(project_root);
        if !outbox.is_dir() {
            return Ok(());
        }
        let processing = dispatch_processing_dir(project_root);
        let delivered = dispatch_delivered_dir(project_root);
        let failed = dispatch_failed_dir(project_root);
        fs::create_dir_all(&processing)?;
        fs::create_dir_all(&delivered)?;
        fs::create_dir_all(&failed)?;

        let mut paths = fs::read_dir(&outbox)?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
            .collect::<Vec<_>>();
        paths.sort();

        for path in paths {
            let Some(file_name) = path.file_name().map(|name| name.to_owned()) else {
                continue;
            };
            let processing_path = processing.join(&file_name);
            if fs::rename(&path, &processing_path).is_err() {
                continue;
            }
            let result = self.process_dispatch_file(app, project_root, &processing_path);
            let target_dir = if result.is_ok() { &delivered } else { &failed };
            let target_path = unique_path(target_dir.join(&file_name));
            let _ = fs::rename(&processing_path, &target_path);
            match result {
                Ok(response) => {
                    let _ = fs::write(
                        result_path_for_dispatch_file(&target_path),
                        serde_json::to_vec_pretty(&response)?,
                    );
                }
                Err(err) => {
                    let _ = fs::write(target_path.with_extension("error.txt"), err.to_string());
                }
            }
        }
        Ok(())
    }

    fn process_dispatch_file(
        &self,
        app: &AppHandle,
        project_root: &Path,
        path: &Path,
    ) -> Result<JsonValue> {
        let text = fs::read_to_string(path)?;
        let request = serde_json::from_str::<BartenderDispatchFile>(&text)?;
        if request.schema != BARTENDER_DISPATCH_SCHEMA {
            bail!("unsupported Bartender dispatch schema: {}", request.schema);
        }
        let dispatch_project_root = PathBuf::from(&request.project_root);
        if !paths_same(project_root, &dispatch_project_root) {
            bail!(
                "Bartender dispatch project mismatch: {} != {}",
                dispatch_project_root.display(),
                project_root.display()
            );
        }
        match request.action.as_str() {
            "sync" => self.process_sync_dispatch(app, project_root, &request),
            other => bail!("unknown Bartender dispatch action: {other}"),
        }
    }

    fn process_sync_dispatch(
        &self,
        app: &AppHandle,
        project_root: &Path,
        request: &BartenderDispatchFile,
    ) -> Result<JsonValue> {
        let manager = app.state::<IntegrationManager>();
        let pty = app.state::<PtyManager>();
        let agent_bus = app.state::<AgentBusManager>();
        let workspace = crate::active_bartender_workspace(
            &manager,
            &BartenderRequest {
                project_root: Some(path_string(project_root)),
                conflict_prompt: None,
            },
        )
        .map_err(anyhow::Error::msg)?;
        emit_sync_event(app, &workspace, &request.request_id, "started", None, None);
        match self.sync_local_with_progress(app, &workspace) {
            Ok(result) => {
                if let Some(conflict) = result.conflicts.first() {
                    crate::deliver_bartender_conflict(
                        app,
                        &manager,
                        &pty,
                        &agent_bus,
                        &workspace,
                        conflict,
                        result.conflicts.len(),
                        None,
                    );
                }
                emit_sync_event(
                    app,
                    &workspace,
                    &request.request_id,
                    "finished",
                    Some(result.clone()),
                    None,
                );
                Ok(json!({
                    "ok": result.ok,
                    "message": result.message,
                    "requestId": request.request_id,
                    "result": result,
                }))
            }
            Err(err) => {
                let error = err.to_string();
                emit_sync_event(
                    app,
                    &workspace,
                    &request.request_id,
                    "failed",
                    None,
                    Some(error.clone()),
                );
                bail!("{error}")
            }
        }
    }

    pub fn fetch_github(&self, workspace: &WorkspaceProject) -> Result<BartenderFetchResult> {
        let _compute_guard = self
            .status_compute_lock
            .lock()
            .map_err(|_| anyhow!("bartender status compute lock poisoned"))?;
        self.invalidate_status_cache()?;
        let source = PathBuf::from(&workspace.source_dir);
        fetch_origin_branch(&source, &workspace.default_branch)?;
        let status = compute_status(workspace)?;
        self.store_status(status_cache_key(workspace), status.clone())?;
        Ok(BartenderFetchResult {
            ok: true,
            message: "Fetched GitHub.".into(),
            status,
        })
    }

    pub fn pull_from_github(&self, workspace: &WorkspaceProject) -> Result<BartenderPullResult> {
        let _guard = self
            .sync_lock
            .lock()
            .map_err(|_| anyhow!("bartender sync lock poisoned"))?;
        let _compute_guard = self
            .status_compute_lock
            .lock()
            .map_err(|_| anyhow!("bartender status compute lock poisoned"))?;
        self.invalidate_status_cache()?;

        let source = PathBuf::from(&workspace.source_dir);
        fetch_origin_branch(&source, &workspace.default_branch)?;
        let before = compute_status(workspace)?;
        if before.room_change_count > 0 {
            return Ok(BartenderPullResult {
                ok: false,
                message: "Sync Local before pulling from GitHub.".into(),
                pulled_commit_count: 0,
                needs_human_pick: false,
                conflict: None,
                status: before,
            });
        }
        if before.github_behind_count == 0 {
            return Ok(BartenderPullResult {
                ok: true,
                message: "Nothing to pull.".into(),
                pulled_commit_count: 0,
                needs_human_pick: false,
                conflict: None,
                status: before,
            });
        }
        if let Some(operation) = active_git_operation(&source) {
            return Ok(BartenderPullResult {
                ok: false,
                message: format!("Resolve the active {operation} before pulling from GitHub."),
                pulled_commit_count: 0,
                needs_human_pick: false,
                conflict: None,
                status: before,
            });
        }

        let upstream =
            upstream_ref(&source).unwrap_or_else(|| format!("origin/{}", workspace.default_branch));
        let local_source_head = source_head(&source)?;
        let upstream_head = git(&source, &["rev-parse", "--verify", &upstream])?
            .trim()
            .to_string();
        if before.github_change_count == 0 {
            git(&source, &["merge", "--ff-only", &upstream])?;
            let new_head = source_head(&source)?;
            for target in agent_targets(workspace) {
                if target.path.exists() {
                    reset_worktree_to(&target.path, &new_head)?;
                }
            }
            let status = compute_status(workspace)?;
            self.store_status(status_cache_key(workspace), status.clone())?;
            return Ok(BartenderPullResult {
                ok: true,
                message: format!(
                    "Pulled {} from GitHub.",
                    change_phrase(before.github_behind_count)
                ),
                pulled_commit_count: before.github_behind_count,
                needs_human_pick: false,
                conflict: None,
                status,
            });
        }

        match git(&source, &["merge", "--no-ff", "--no-edit", &upstream]) {
            Ok(_) => {
                let new_head = source_head(&source)?;
                for target in agent_targets(workspace) {
                    if target.path.exists() {
                        reset_worktree_to(&target.path, &new_head)?;
                    }
                }
                let status = compute_status(workspace)?;
                self.store_status(status_cache_key(workspace), status.clone())?;
                Ok(BartenderPullResult {
                    ok: true,
                    message: format!(
                        "Merged {} from GitHub.",
                        change_phrase(before.github_behind_count)
                    ),
                    pulled_commit_count: before.github_behind_count,
                    needs_human_pick: false,
                    conflict: None,
                    status,
                })
            }
            Err(err) => {
                let status = compute_status(workspace)?;
                let conflict = BartenderPullConflict {
                    message: err.to_string(),
                    source_head: local_source_head,
                    upstream,
                    upstream_head,
                    default_branch: workspace.default_branch.clone(),
                };
                Ok(BartenderPullResult {
                    ok: false,
                    message:
                        "There is a pull conflict when pulling GitHub to the local version. Human, pick an agent to resolve it."
                            .into(),
                    pulled_commit_count: 0,
                    needs_human_pick: true,
                    conflict: Some(conflict),
                    status,
                })
            }
        }
    }

    pub fn push_to_github(&self, workspace: &WorkspaceProject) -> Result<BartenderPushResult> {
        let _guard = self
            .sync_lock
            .lock()
            .map_err(|_| anyhow!("bartender sync lock poisoned"))?;
        let _compute_guard = self
            .status_compute_lock
            .lock()
            .map_err(|_| anyhow!("bartender status compute lock poisoned"))?;
        self.invalidate_status_cache()?;

        let before = compute_status(workspace)?;
        if before.room_change_count > 0 {
            return Ok(BartenderPushResult {
                ok: false,
                message: "Sync Local before pushing to GitHub.".into(),
                pushed_commit_count: 0,
                status: before,
            });
        }
        if before.github_behind_count > 0 {
            return Ok(BartenderPushResult {
                ok: false,
                message: format!(
                    "GitHub has {}. Pull first, then push.",
                    change_phrase(before.github_behind_count)
                ),
                pushed_commit_count: 0,
                status: before,
            });
        }
        if before.github_needs_initial_push {
            let source = PathBuf::from(&workspace.source_dir);
            let branch = before
                .github_push_branch
                .clone()
                .unwrap_or_else(|| workspace.default_branch.clone());
            let push_result = git(
                &source,
                &["push", "-u", "origin", &format!("HEAD:{branch}")],
            );
            if let Err(err) = push_result {
                let _ = fetch_origin_branch(&source, &branch);
                let status = compute_status(workspace)?;
                return Ok(BartenderPushResult {
                    ok: false,
                    message: format!("Initial GitHub push failed: {err}"),
                    pushed_commit_count: 0,
                    status,
                });
            }

            let _ = fetch_origin_branch(&source, &branch);
            let status = compute_status(workspace)?;
            return Ok(BartenderPushResult {
                ok: true,
                message: "Initial push to GitHub complete.".into(),
                pushed_commit_count: before.github_initial_push_commit_count,
                status,
            });
        }
        if before.github_change_count == 0 {
            return Ok(BartenderPushResult {
                ok: true,
                message: "Nothing to push.".into(),
                pushed_commit_count: 0,
                status: before,
            });
        }

        let source = PathBuf::from(&workspace.source_dir);
        let upstream =
            upstream_ref(&source).unwrap_or_else(|| format!("origin/{}", workspace.default_branch));
        let push_result = if upstream.starts_with("origin/") {
            git(
                &source,
                &[
                    "push",
                    "origin",
                    &format!("HEAD:{}", workspace.default_branch),
                ],
            )
        } else {
            git(&source, &["push"])
        };

        if let Err(err) = push_result {
            let _ = fetch_origin_branch(&source, &workspace.default_branch);
            let status = compute_status(workspace)?;
            if status.github_behind_count > 0 {
                return Ok(BartenderPushResult {
                    ok: false,
                    message: format!(
                        "GitHub has {}. Pull first, then push.",
                        change_phrase(status.github_behind_count)
                    ),
                    pushed_commit_count: 0,
                    status,
                });
            }
            return Ok(BartenderPushResult {
                ok: false,
                message: format!("GitHub push failed: {err}"),
                pushed_commit_count: 0,
                status,
            });
        }

        let _ = fetch_origin_branch(&source, &workspace.default_branch);
        let status = compute_status(workspace)?;
        Ok(BartenderPushResult {
            ok: true,
            message: "Pushed to GitHub.".into(),
            pushed_commit_count: before.github_change_count,
            status,
        })
    }

    fn cached_status(&self, key: &BartenderStatusCacheKey) -> Result<Option<BartenderStatus>> {
        let cache = self
            .status_cache
            .lock()
            .map_err(|_| anyhow!("bartender status cache lock poisoned"))?;
        let Some(cached) = cache.as_ref() else {
            return Ok(None);
        };
        if cached.key == *key && cached.checked_at.elapsed() < BARTENDER_STATUS_CACHE_TTL {
            return Ok(Some(cached.status.clone()));
        }
        Ok(None)
    }

    fn store_status(&self, key: BartenderStatusCacheKey, status: BartenderStatus) -> Result<()> {
        let mut cache = self
            .status_cache
            .lock()
            .map_err(|_| anyhow!("bartender status cache lock poisoned"))?;
        *cache = Some(CachedBartenderStatus {
            key,
            checked_at: Instant::now(),
            status,
        });
        Ok(())
    }

    fn invalidate_status_cache(&self) -> Result<()> {
        let mut cache = self
            .status_cache
            .lock()
            .map_err(|_| anyhow!("bartender status cache lock poisoned"))?;
        *cache = None;
        Ok(())
    }
}

fn compute_status(workspace: &WorkspaceProject) -> Result<BartenderStatus> {
    let started = Instant::now();
    let source = PathBuf::from(&workspace.source_dir);
    let source_head_sha = git(&source, &["rev-parse", "--verify", "HEAD"])
        .unwrap_or_else(|_| String::new())
        .trim()
        .to_string();
    let mut dirty_agents = Vec::new();
    let mut room_changed_paths = BTreeSet::new();
    let source_changed_paths = changed_paths_against(&source, "HEAD").unwrap_or_default();
    let source_change_count = source_changed_paths.len();
    room_changed_paths.extend(source_changed_paths);

    for agent in agent_targets(workspace) {
        if !agent.path.exists() {
            continue;
        }
        let agent_head = source_head(&agent.path).unwrap_or_default();
        let pending_commits = if source_head_sha.is_empty() {
            Vec::new()
        } else {
            pending_commits_from_room_head(&source_head_sha, &agent.path).unwrap_or_default()
        };
        let mut agent_changed_paths =
            changed_paths_from_commits(&agent.path, &pending_commits).unwrap_or_default();
        agent_changed_paths.extend(changed_paths_against(&agent.path, "HEAD").unwrap_or_default());
        if pending_commits.is_empty()
            && !source_head_sha.is_empty()
            && agent_head != source_head_sha
        {
            agent_changed_paths
                .extend(changed_paths_against(&agent.path, &source_head_sha).unwrap_or_default());
        }
        let change_count = agent_changed_paths.len();
        let pending_commit_count = pending_commits.len();
        if change_count > 0 || pending_commit_count > 0 {
            room_changed_paths.extend(agent_changed_paths);
            dirty_agents.push(BartenderDirtyAgent {
                agent_id: agent.agent_id,
                path: path_string(&agent.path),
                change_count,
                pending_commit_count,
            });
        }
    }

    let room_change_count = room_changed_paths.len();
    let github_change_count = ahead_commit_count(&source, &workspace.default_branch).unwrap_or(0);
    let github_behind_count = behind_commit_count(&source, &workspace.default_branch).unwrap_or(0);
    let initial_push = initial_push_status(&source, &workspace.default_branch);
    let github_needs_initial_push = initial_push.is_some();
    let github_push_branch = initial_push.as_ref().map(|status| status.branch.clone());
    let github_initial_push_commit_count = initial_push
        .as_ref()
        .map(|status| status.commit_count)
        .unwrap_or(0);
    let (state, message) = if room_change_count > 0 {
        let location = match (source_change_count > 0, dirty_agents.is_empty()) {
            (true, true) => "source repo",
            (true, false) => "source repo and agent worktrees",
            (false, _) => "agent worktrees",
        };
        (
            "roomDiff".into(),
            format!(
                "{} waiting in {location}.",
                changed_file_phrase(room_change_count)
            ),
        )
    } else if github_change_count > 0 && github_behind_count > 0 {
        (
            "githubDiverged".into(),
            format!(
                "GitHub has {}; local has {}. Pull first.",
                change_phrase(github_behind_count),
                change_phrase(github_change_count)
            ),
        )
    } else if github_behind_count > 0 {
        (
            "githubBehind".into(),
            format!(
                "{} available from GitHub.",
                change_phrase(github_behind_count)
            ),
        )
    } else if github_change_count > 0 {
        (
            "githubDiff".into(),
            format!("{} ready for GitHub.", change_phrase(github_change_count)),
        )
    } else if let Some(initial_push) = &initial_push {
        (
            "githubInitialPush".into(),
            format!("{} is ready for initial GitHub push.", initial_push.branch),
        )
    } else {
        ("idle".into(), "Room is in sync.".into())
    };

    let status = BartenderStatus {
        project_id: workspace.project_id.clone(),
        source_dir: workspace.source_dir.clone(),
        default_branch: workspace.default_branch.clone(),
        room_change_count,
        source_change_count,
        github_change_count,
        github_behind_count,
        github_needs_initial_push,
        github_push_branch,
        github_initial_push_commit_count,
        dirty_agents,
        checked_at: Utc::now().to_rfc3339(),
        state,
        message,
    };
    let elapsed = started.elapsed();
    if elapsed > Duration::from_millis(3000) {
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
    Ok(status)
}

fn sync_local_inner(
    workspace: &WorkspaceProject,
    progress: Option<&BartenderSyncProgress>,
) -> Result<BartenderSyncResult> {
    let source = PathBuf::from(&workspace.source_dir);
    if let Some(progress) = progress {
        progress.emit("checking_source", "Checking source repository");
    }
    ensure_git_repo(&source)?;

    if let Some(operation) = active_git_operation(&source) {
        if let Some(progress) = progress {
            progress.emit("blocked", "Source repository has an active Git operation");
        }
        return Ok(BartenderSyncResult {
            ok: false,
            message: format!("Resolve the active {operation} before syncing local."),
            snapshot_count: 0,
            published_commit_count: 0,
            published_agents: Vec::new(),
            conflicts: Vec::new(),
            status: compute_status(workspace)?,
        });
    }

    let mut snapshot_count = 0usize;
    let mut published_commit_count = 0usize;
    let mut published_agents = Vec::new();
    let mut conflicts = Vec::new();
    let mut reset_failures = BTreeMap::<String, String>::new();

    if porcelain_change_count(&source)? > 0 {
        if let Some(progress) = progress {
            progress.emit("snapshot_source", "Snapshotting source changes");
        }
        snapshot_worktree(&source, "Bartender checkpoint: room edits")?;
        snapshot_count += 1;
    }

    if let Some(progress) = progress {
        progress.emit("checking_agents", "Checking agent worktrees");
    }
    let mut remaining = Vec::new();
    for target in agent_targets(workspace) {
        if !target.path.exists() {
            continue;
        }
        ensure_git_repo(&target.path)?;
        if porcelain_change_count(&target.path)? > 0 {
            if let Some(progress) = progress {
                progress.emit("snapshot_agent", "Snapshotting agent changes");
            }
            snapshot_worktree(
                &target.path,
                &format!("Bartender snapshot: {}", target.agent_id),
            )?;
            snapshot_count += 1;
        }
        if !pending_commits(&source, &target.path)?.is_empty() {
            remaining.push(target);
        }
    }

    let mut stalled_conflicts = Vec::new();
    while !remaining.is_empty() {
        if let Some(progress) = progress {
            progress.emit("publishing_agents", "Publishing agent commits");
        }
        let mut pass_published = 0usize;
        let mut next_remaining = Vec::new();
        let mut pass_conflicts = Vec::new();

        for target in remaining {
            let outcome = publish_agent(&source, &target)?;
            if outcome.published_commits > 0 {
                pass_published += outcome.published_commits;
                published_commit_count += outcome.published_commits;
            }

            if let Some(conflict) = outcome.conflict {
                pass_conflicts.push(conflict);
                next_remaining.push(target);
            } else {
                let current_head = source_head(&source)?;
                match reset_worktree_to(&target.path, &current_head) {
                    Ok(()) => {
                        reset_failures.remove(&target.agent_id);
                        published_agents.push(BartenderPublishedAgent {
                            agent_id: target.agent_id,
                            commit_count: outcome.published_commits,
                        });
                    }
                    Err(err) => {
                        record_reset_failure(&mut reset_failures, &target, err);
                    }
                }
            }
        }

        if next_remaining.is_empty() {
            stalled_conflicts.clear();
            break;
        }
        if pass_published == 0 {
            stalled_conflicts = pass_conflicts;
            break;
        }

        remaining = next_remaining;
    }

    if stalled_conflicts.is_empty() {
        if let Some(progress) = progress {
            progress.emit("refreshing_agents", "Refreshing agent worktrees");
        }
        let current_head = source_head(&source)?;
        for target in agent_targets(workspace) {
            if target.path.exists() {
                match reset_worktree_to(&target.path, &current_head) {
                    Ok(()) => {
                        reset_failures.remove(&target.agent_id);
                    }
                    Err(err) => {
                        record_reset_failure(&mut reset_failures, &target, err);
                    }
                }
            }
        }
    } else {
        conflicts = stalled_conflicts;
    }

    if let Some(progress) = progress {
        progress.emit("refreshing_status", "Refreshing sync status");
    }
    let status = compute_status(workspace)?;
    let ok = conflicts.is_empty() && reset_failures.is_empty();
    let message = if !reset_failures.is_empty() {
        reset_failure_message(&reset_failures)
    } else if ok {
        if published_commit_count == 0 && snapshot_count == 0 {
            "Nothing to sync.".into()
        } else {
            format!(
                "Synced {}.",
                change_phrase(published_commit_count.max(snapshot_count))
            )
        }
    } else {
        format!(
            "{} need agent conflict resolution.",
            change_phrase(conflicts.len())
        )
    };

    if let Some(progress) = progress {
        progress.emit("finished", &message);
    }

    Ok(BartenderSyncResult {
        ok,
        message,
        snapshot_count,
        published_commit_count,
        published_agents,
        conflicts,
        status,
    })
}

fn record_reset_failure(
    reset_failures: &mut BTreeMap<String, String>,
    target: &AgentSyncTarget,
    err: anyhow::Error,
) {
    reset_failures.insert(target.agent_id.clone(), concise_git_error(&err.to_string()));
}

fn reset_failure_message(reset_failures: &BTreeMap<String, String>) -> String {
    let count = reset_failures.len();
    let first = reset_failures.values().next().cloned().unwrap_or_default();
    format!(
        "Could not refresh {} agent worktree{} after sync. {}",
        count,
        if count == 1 { "" } else { "s" },
        first,
    )
}

fn publish_agent(source: &Path, target: &AgentSyncTarget) -> Result<PublishOutcome> {
    let commits = pending_commits(source, &target.path)?;
    let mut published_commits = 0usize;

    for commit in commits {
        match git(source, &["cherry-pick", &commit]) {
            Ok(_) => {
                published_commits += 1;
            }
            Err(err) => {
                let _ = git(source, &["cherry-pick", "--abort"]);
                if looks_like_empty_cherry_pick(&err.to_string()) {
                    continue;
                }
                return Ok(PublishOutcome {
                    published_commits,
                    conflict: Some(BartenderConflict {
                        agent_id: target.agent_id.clone(),
                        commit: Some(commit),
                        message: err.to_string(),
                    }),
                });
            }
        }
    }

    Ok(PublishOutcome {
        published_commits,
        conflict: None,
    })
}

fn snapshot_worktree(path: &Path, message: &str) -> Result<()> {
    git(path, &["add", "-A"])?;
    let staged = git(path, &["diff", "--cached", "--name-only"])?;
    if staged.trim().is_empty() {
        return Ok(());
    }
    git(
        path,
        &[
            "-c",
            "user.name=Kota Bartender",
            "-c",
            "user.email=bartender@kota.local",
            "commit",
            "-m",
            message,
        ],
    )?;
    Ok(())
}

fn reset_worktree_to(path: &Path, commit: &str) -> Result<()> {
    let mut last_lock_error = None;
    for attempt in 0..=4 {
        match reset_worktree_to_once(path, commit) {
            Ok(()) => return Ok(()),
            Err(err) => {
                if !is_index_lock_error(&err) {
                    return Err(err);
                }
                last_lock_error = Some(err);
                if attempt < 4 {
                    std::thread::sleep(Duration::from_millis(160));
                }
            }
        }
    }
    Err(last_lock_error.unwrap_or_else(|| anyhow!("git index.lock exists")))
}

fn reset_worktree_to_once(path: &Path, commit: &str) -> Result<()> {
    git(path, &["reset", "--hard", commit])?;
    git(path, &["clean", "-fd"])?;
    Ok(())
}

fn is_index_lock_error(err: &anyhow::Error) -> bool {
    err.to_string().contains("index.lock")
}

fn pending_commits(source: &Path, agent: &Path) -> Result<Vec<String>> {
    let room_head = source_head(source)?;
    pending_commits_from_room_head(&room_head, agent)
}

fn pending_commits_from_room_head(room_head: &str, agent: &Path) -> Result<Vec<String>> {
    let agent_head = source_head(agent)?;
    if room_head == agent_head {
        return Ok(Vec::new());
    }
    let range = format!("{room_head}...{agent_head}");
    let out = git(
        agent,
        &[
            "rev-list",
            "--reverse",
            "--cherry-pick",
            "--right-only",
            &range,
        ],
    )?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn status_cache_key(workspace: &WorkspaceProject) -> BartenderStatusCacheKey {
    BartenderStatusCacheKey {
        project_id: workspace.project_id.clone(),
        source_dir: workspace.source_dir.clone(),
        local_root: workspace.local_root.clone(),
        default_branch: workspace.default_branch.clone(),
        agents: workspace
            .agents
            .iter()
            .filter(|agent| agent_launch_spec_is_active(agent))
            .map(|agent| (agent.agent_id.clone(), agent.worktree_root.clone()))
            .collect(),
    }
}

fn ahead_commit_count(source: &Path, default_branch: &str) -> Result<usize> {
    let upstream = upstream_ref(source)
        .or_else(|| verify_ref(source, &format!("origin/{default_branch}")))
        .ok_or_else(|| anyhow!("no upstream branch"))?;
    let out = git(
        source,
        &["rev-list", "--count", &format!("{upstream}..HEAD")],
    )?;
    out.trim()
        .parse::<usize>()
        .with_context(|| format!("parse ahead count from {out:?}"))
}

fn behind_commit_count(source: &Path, default_branch: &str) -> Result<usize> {
    let upstream = upstream_ref(source)
        .or_else(|| verify_ref(source, &format!("origin/{default_branch}")))
        .ok_or_else(|| anyhow!("no upstream branch"))?;
    let out = git(
        source,
        &["rev-list", "--count", &format!("HEAD..{upstream}")],
    )?;
    out.trim()
        .parse::<usize>()
        .with_context(|| format!("parse behind count from {out:?}"))
}

fn initial_push_status(source: &Path, default_branch: &str) -> Option<InitialPushStatus> {
    if !origin_remote_exists(source) || current_branch(source).as_deref() != Some(default_branch) {
        return None;
    }
    let commit_count = commit_count_from_head(source).ok()?;
    if commit_count == 0 {
        return None;
    }

    match upstream_ref(source) {
        Some(upstream) if verify_ref(source, &upstream).is_some() => None,
        Some(upstream) if upstream.starts_with("origin/") => Some(InitialPushStatus {
            branch: upstream.trim_start_matches("origin/").to_string(),
            commit_count,
        }),
        Some(_) => None,
        None => Some(InitialPushStatus {
            branch: default_branch.to_string(),
            commit_count,
        }),
    }
}

fn commit_count_from_head(source: &Path) -> Result<usize> {
    let out = git(source, &["rev-list", "--count", "HEAD"])?;
    out.trim()
        .parse::<usize>()
        .with_context(|| format!("parse head commit count from {out:?}"))
}

fn current_branch(source: &Path) -> Option<String> {
    git(source, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && value != "HEAD")
}

fn origin_remote_exists(source: &Path) -> bool {
    git(source, &["remote", "get-url", "origin"]).is_ok()
}

fn fetch_origin_branch(source: &Path, default_branch: &str) -> Result<()> {
    git(source, &["fetch", "--no-tags", "origin", default_branch]).map(|_| ())
}

fn active_git_operation(path: &Path) -> Option<&'static str> {
    if git(path, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_ok() {
        return Some("Git merge");
    }
    if git(path, &["rev-parse", "-q", "--verify", "CHERRY_PICK_HEAD"]).is_ok() {
        return Some("Git cherry-pick");
    }
    None
}

fn upstream_ref(path: &Path) -> Option<String> {
    git(
        path,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    )
    .ok()
    .map(|value| value.trim().to_string())
    .filter(|value| !value.is_empty())
}

fn verify_ref(path: &Path, value: &str) -> Option<String> {
    git(path, &["rev-parse", "--verify", value])
        .ok()
        .map(|_| value.to_string())
}

fn source_head(path: &Path) -> Result<String> {
    Ok(git(path, &["rev-parse", "--verify", "HEAD"])?
        .trim()
        .to_string())
}

fn ensure_git_repo(path: &Path) -> Result<()> {
    git(path, &["rev-parse", "--is-inside-work-tree"]).map(|_| ())
}

fn porcelain_change_count(path: &Path) -> Result<usize> {
    let out = git(path, &["status", "--porcelain=v1", "--untracked-files=all"])?;
    Ok(count_porcelain_changes(&out))
}

fn changed_paths_against(path: &Path, base: &str) -> Result<BTreeSet<String>> {
    let mut paths = BTreeSet::new();
    let diff = git(
        path,
        &["diff", "--name-status", "-z", "--no-renames", base, "--"],
    )?;
    append_name_status_paths(&mut paths, &diff);

    let untracked = git(path, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    for path in untracked
        .split('\0')
        .map(str::trim)
        .filter(|path| !path.is_empty())
    {
        paths.insert(normalize_git_path(path));
    }
    Ok(paths)
}

fn changed_paths_from_commits(path: &Path, commits: &[String]) -> Result<BTreeSet<String>> {
    let mut paths = BTreeSet::new();
    for commit in commits {
        let diff = git(
            path,
            &[
                "show",
                "--format=",
                "--name-status",
                "-z",
                "--no-renames",
                commit,
            ],
        )?;
        append_name_status_paths(&mut paths, &diff);
    }
    Ok(paths)
}

fn append_name_status_paths(paths: &mut BTreeSet<String>, diff: &str) {
    let mut parts = diff
        .split('\0')
        .map(str::trim)
        .filter(|part| !part.is_empty());
    while let Some(_status) = parts.next() {
        if let Some(path) = parts.next() {
            paths.insert(normalize_git_path(path));
        }
    }
}

fn count_porcelain_changes(out: &str) -> usize {
    out.lines().filter(|line| !line.trim().is_empty()).count()
}

fn git(path: &Path, args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    configure_github_cli_credential_helper(&mut command);
    let output = command
        .arg("-C")
        .arg(path)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .with_context(|| format!("run git -C {}", path.display()))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    Err(anyhow!(
        "git -C {} {} failed: {}",
        path.display(),
        args.join(" "),
        detail
    ))
}

fn configure_github_cli_credential_helper(command: &mut Command) {
    if let Some(helper) = github_cli_credential_helper() {
        command
            .arg("-c")
            .arg("credential.https://github.com.helper=")
            .arg("-c")
            .arg(format!("credential.https://github.com.helper={helper}"));
    }
}

fn github_cli_credential_helper() -> Option<&'static str> {
    static HELPER: OnceLock<Option<String>> = OnceLock::new();
    HELPER
        .get_or_init(resolve_github_cli_credential_helper)
        .as_deref()
}

fn resolve_github_cli_credential_helper() -> Option<String> {
    for candidate in ["/opt/homebrew/bin/gh", "/usr/local/bin/gh", "gh"] {
        let ok = Command::new(candidate)
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
        if ok {
            return Some(format!("!{candidate} auth git-credential"));
        }
    }
    None
}

fn agent_targets(workspace: &WorkspaceProject) -> Vec<AgentSyncTarget> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for agent in workspace
        .agents
        .iter()
        .filter(|agent| agent_launch_spec_is_active(agent))
    {
        if seen.insert(agent.agent_id.clone()) {
            out.push(AgentSyncTarget {
                agent_id: agent.agent_id.clone(),
                path: PathBuf::from(&agent.worktree_root),
            });
        }
    }
    out
}

fn path_string(path: &Path) -> String {
    path.display().to_string()
}

fn paths_same(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    left == right
}

fn now_iso() -> String {
    Utc::now().to_rfc3339()
}

fn dispatch_root(project_root: &Path) -> PathBuf {
    project_root
        .join("project-memory")
        .join(".violet")
        .join("bartender")
}

fn dispatch_outbox_dir(project_root: &Path) -> PathBuf {
    dispatch_root(project_root).join("outbox")
}

fn dispatch_processing_dir(project_root: &Path) -> PathBuf {
    dispatch_root(project_root).join("processing")
}

fn dispatch_delivered_dir(project_root: &Path) -> PathBuf {
    dispatch_root(project_root).join("delivered")
}

fn dispatch_failed_dir(project_root: &Path) -> PathBuf {
    dispatch_root(project_root).join("failed")
}

fn result_path_for_dispatch_file(path: &Path) -> PathBuf {
    path.with_extension("result.json")
}

fn emit_sync_event(
    app: &AppHandle,
    workspace: &WorkspaceProject,
    request_id: &str,
    phase: &str,
    result: Option<BartenderSyncResult>,
    error: Option<String>,
) {
    if let Err(err) = app.emit(
        BARTENDER_SYNC_EVENT,
        BartenderSyncEvent {
            project_root: workspace.local_root.clone(),
            request_id: request_id.to_string(),
            phase: phase.to_string(),
            result,
            error,
        },
    ) {
        crate::kota_debug_log(&format!(
            "[bartender] failed to emit sync lifecycle project={} request={} phase={}: {err}",
            workspace.project_id, request_id, phase
        ));
    }
}

pub fn sync_dispatch_receipt(
    project_root: &Path,
    request_id: &str,
) -> Result<BartenderSyncReceipt> {
    let request_id = request_id.trim();
    if !valid_sync_request_id(request_id) {
        bail!("invalid Bartender sync request id");
    }

    let delivered_request = dispatch_delivered_dir(project_root).join(format!("{request_id}.json"));
    let result_path = result_path_for_dispatch_file(&delivered_request);
    if result_path.is_file() {
        let payload = serde_json::from_slice::<JsonValue>(&fs::read(&result_path)?)?;
        let stored_request_id = payload
            .get("requestId")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| anyhow!("Bartender sync result is missing requestId"))?;
        if stored_request_id != request_id {
            bail!("Bartender sync result request id mismatch");
        }
        let result = payload
            .get("result")
            .cloned()
            .ok_or_else(|| anyhow!("Bartender sync result payload is missing result"))?;
        return Ok(BartenderSyncReceipt {
            project_root: path_string(project_root),
            request_id: request_id.to_string(),
            phase: "finished".into(),
            result: Some(result),
            error: None,
        });
    }

    let failed_request = dispatch_failed_dir(project_root).join(format!("{request_id}.json"));
    let error_path = failed_request.with_extension("error.txt");
    if error_path.is_file() {
        return Ok(BartenderSyncReceipt {
            project_root: path_string(project_root),
            request_id: request_id.to_string(),
            phase: "failed".into(),
            result: None,
            error: Some(fs::read_to_string(error_path)?.trim().to_string()),
        });
    }

    Ok(BartenderSyncReceipt {
        project_root: path_string(project_root),
        request_id: request_id.to_string(),
        phase: "pending".into(),
        result: None,
        error: None,
    })
}

fn valid_sync_request_id(request_id: &str) -> bool {
    request_id.len() > "sync-".len()
        && request_id.len() <= 128
        && request_id.starts_with("sync-")
        && request_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn unique_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("file")
        .to_string();
    let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
    for index in 1.. {
        let file_name = if ext.is_empty() {
            format!("{stem}-{index}")
        } else {
            format!("{stem}-{index}.{ext}")
        };
        let candidate = parent.join(file_name);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

pub fn install_cli_shim() -> Result<PathBuf> {
    let bin_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(KOTA_HOME_DIR)
        .join("bin");
    fs::create_dir_all(&bin_dir)?;
    let shim = bin_dir.join("kota-bartender");
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("kota-bartender"));
            candidates.push(parent.join("../Resources/kota-bartender"));
            if let Some(triple) = current_target_triple_guess() {
                candidates.push(parent.join(format!("kota-bartender-{triple}")));
                candidates.push(parent.join(format!("../Resources/kota-bartender-{triple}")));
            }
        }
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/kota-bartender"));
    candidates
        .push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/release/kota-bartender"));

    let mut script = String::from("#!/bin/sh\nset -eu\n");
    script.push_str("if [ -n \"${KOTA_BARTENDER_BIN:-}\" ] && [ -x \"$KOTA_BARTENDER_BIN\" ]; then exec \"$KOTA_BARTENDER_BIN\" \"$@\"; fi\n");
    for candidate in candidates {
        script.push_str("if [ -x ");
        script.push_str(&shell_quote(&candidate.display().to_string()));
        script.push_str(" ]; then exec ");
        script.push_str(&shell_quote(&candidate.display().to_string()));
        script.push_str(" \"$@\"; fi\n");
    }
    script.push_str("echo 'kota-bartender binary is not installed. Build it with: cargo build --bin kota-bartender' >&2\nexit 127\n");
    fs::write(&shim, script)?;
    make_executable(&shim)?;
    Ok(shim)
}

pub fn run_cli() -> Result<()> {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty()
        || matches!(
            args.first().map(String::as_str),
            Some("help" | "--help" | "-h")
        )
    {
        print_cli_usage();
        return Ok(());
    }
    let command = args.remove(0);
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
    {
        print_cli_usage();
        return Ok(());
    }
    match command.as_str() {
        "status" => cli_status(args),
        "sync" => cli_sync(args),
        _ => {
            print_cli_usage();
            bail!("unknown kota-bartender command: {command}");
        }
    }
}

fn cli_status(args: Vec<String>) -> Result<()> {
    let options = parse_cli_options(args)?;
    let project_root = resolve_cli_project_root(options.project_root.as_deref())?;
    let workspace = cli_workspace_for_project_root(&project_root)?;
    let status = BartenderManager::default().status(&workspace)?;
    if options.json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        println!("{}", status.message);
    }
    Ok(())
}

fn cli_sync(args: Vec<String>) -> Result<()> {
    let options = parse_cli_options(args)?;
    let project_root = resolve_cli_project_root(options.project_root.as_deref())?;
    let request_id = enqueue_sync_dispatch(&project_root)?;
    match wait_for_sync_dispatch_result(&project_root, &request_id) {
        Ok(result_json) => {
            if options.json {
                print!("{result_json}");
                if !result_json.ends_with('\n') {
                    println!();
                }
            } else {
                let message = serde_json::from_str::<JsonValue>(&result_json)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("message")
                            .and_then(JsonValue::as_str)
                            .map(ToOwned::to_owned)
                    })
                    .unwrap_or_else(|| "Bartender sync finished.".into());
                println!("{message}");
            }
            Ok(())
        }
        Err(err) => {
            if options.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "ok": false,
                        "message": err.to_string(),
                        "requestId": request_id,
                    }))?
                );
            }
            Err(err)
        }
    }
}

#[derive(Default)]
struct CliOptions {
    json: bool,
    project_root: Option<String>,
}

fn parse_cli_options(args: Vec<String>) -> Result<CliOptions> {
    let mut options = CliOptions::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => {
                options.json = true;
                i += 1;
            }
            "--project-root" => {
                i += 1;
                options.project_root = Some(
                    args.get(i)
                        .ok_or_else(|| anyhow!("--project-root requires a path"))?
                        .clone(),
                );
                i += 1;
            }
            value => bail!("unknown kota-bartender argument: {value}"),
        }
    }
    Ok(options)
}

fn resolve_cli_project_root(explicit: Option<&str>) -> Result<PathBuf> {
    explicit
        .map(PathBuf::from)
        .or_else(|| std::env::var("KOTA_PROJECT_ROOT").ok().map(PathBuf::from))
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(find_project_root_from)
        })
        .ok_or_else(|| anyhow!("kota-bartender requires KOTA_PROJECT_ROOT or --project-root"))
}

fn find_project_root_from(start: PathBuf) -> Option<PathBuf> {
    for dir in start.ancestors() {
        if dir.join("agent.yaml").is_file() && dir.join("project-memory").exists() {
            if let Some(project_root) = dir.parent().and_then(Path::parent) {
                if project_root.join("project-memory").exists()
                    && project_root.join(".agent-workspaces").is_dir()
                {
                    return Some(project_root.to_path_buf());
                }
            }
        }
        if dir.join("project-memory").exists() && dir.join(".agent-workspaces").is_dir() {
            return Some(dir.to_path_buf());
        }
    }
    None
}

fn cli_workspace_for_project_root(project_root: &Path) -> Result<WorkspaceProject> {
    let manager = IntegrationManager::default();
    for workspace in manager.list_workspaces()? {
        if paths_same(Path::new(&workspace.local_root), project_root)
            || paths_same(Path::new(&workspace.source_dir), project_root)
            || paths_same(Path::new(&workspace.shared_dir), project_root)
        {
            return Ok(workspace);
        }
    }
    bail!(
        "No Kota workspace matches project root {}",
        project_root.display()
    )
}

fn enqueue_sync_dispatch(project_root: &Path) -> Result<String> {
    let outbox = dispatch_outbox_dir(project_root);
    fs::create_dir_all(&outbox)?;
    let request_id = format!("sync-{}", Uuid::new_v4().simple());
    let request = BartenderDispatchFile {
        schema: BARTENDER_DISPATCH_SCHEMA.into(),
        created_at: now_iso(),
        project_root: path_string(project_root),
        request_id: request_id.clone(),
        action: "sync".into(),
    };
    let path = outbox.join(format!("{request_id}.json"));
    let tmp = path.with_extension(format!("json.tmp-{}", Uuid::new_v4().simple()));
    fs::write(&tmp, serde_json::to_vec_pretty(&request)?)?;
    fs::rename(&tmp, &path)?;
    Ok(request_id)
}

fn wait_for_sync_dispatch_result(project_root: &Path, request_id: &str) -> Result<String> {
    let delivered_request = dispatch_delivered_dir(project_root).join(format!("{request_id}.json"));
    let failed_request = dispatch_failed_dir(project_root).join(format!("{request_id}.json"));
    let outbox_request = dispatch_outbox_dir(project_root).join(format!("{request_id}.json"));
    let start = Instant::now();
    loop {
        let result_path = result_path_for_dispatch_file(&delivered_request);
        if result_path.exists() {
            return Ok(fs::read_to_string(result_path)?);
        }
        let error_path = failed_request.with_extension("error.txt");
        if error_path.exists() {
            let message = fs::read_to_string(error_path)?;
            bail!("{}", message.trim());
        }
        if start.elapsed() >= CLI_SYNC_TIMEOUT {
            let _ = fs::remove_file(&outbox_request);
            bail!("Bartender sync request was not processed. Open Kota and retry.");
        }
        thread::sleep(CLI_SYNC_POLL_INTERVAL);
    }
}

fn print_cli_usage() {
    eprintln!("usage:");
    eprintln!("  kota-bartender status [--json] [--project-root <path>]");
    eprintln!("  kota-bartender sync [--json] [--project-root <path>]");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  status  Print the current Bartender room/GitHub sync status.");
    eprintln!("  sync    Request the same local room sync as the Bartender card button.");
    eprintln!();
    eprintln!("Notes:");
    eprintln!(
        "  sync requires the Kota app to be running; it does not run a separate git sync path."
    );
    eprintln!("  concurrent sync attempts fail instead of queueing another sync.");
    eprintln!("  Use this only when asked to sync this project's agent worktrees.");
}

fn current_target_triple_guess() -> Option<String> {
    let output = Command::new("rustc").arg("-vV").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(|value| value.trim().to_string())
}

fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | ':'))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn make_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions)?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn change_phrase(count: usize) -> String {
    if count == 1 {
        "1 change".into()
    } else {
        format!("{count} changes")
    }
}

fn changed_file_phrase(count: usize) -> String {
    if count == 1 {
        "1 changed file".into()
    } else {
        format!("{count} changed files")
    }
}

fn concise_git_error(value: &str) -> String {
    if value.contains("index.lock") {
        return "Git index.lock exists; stop any running git process or remove the stale lock, then sync again.".into();
    }
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_for_message(&compact, 180)
}

fn truncate_for_message(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut out = value.chars().take(max_chars).collect::<String>();
    out.push_str("...");
    out
}

fn normalize_git_path(path: &str) -> String {
    path.replace('\\', "/").trim_matches('/').to_string()
}

fn looks_like_empty_cherry_pick(value: &str) -> bool {
    value.contains("previous cherry-pick is now empty")
        || value.contains("The previous cherry-pick is now empty")
        || value.contains("nothing to commit")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn counts_porcelain_lines() {
        assert_eq!(count_porcelain_changes(" M a\n?? b\nR  c -> d\n"), 3);
        assert_eq!(count_porcelain_changes("\n"), 0);
    }

    #[test]
    fn formats_change_phrase() {
        assert_eq!(change_phrase(1), "1 change");
        assert_eq!(change_phrase(2), "2 changes");
        assert_eq!(changed_file_phrase(1), "1 changed file");
        assert_eq!(changed_file_phrase(2), "2 changed files");
    }

    #[test]
    fn reads_finished_and_failed_sync_dispatch_receipts() {
        let root = temp_dir("bartender-sync-receipt");
        let finished_id = "sync-finished123";
        let delivered = dispatch_delivered_dir(&root).join(format!("{finished_id}.json"));
        fs::create_dir_all(delivered.parent().unwrap()).unwrap();
        fs::write(&delivered, "{}").unwrap();
        fs::write(
            result_path_for_dispatch_file(&delivered),
            serde_json::to_vec(&json!({
                "requestId": finished_id,
                "result": { "ok": true, "message": "Synced 1 change." }
            }))
            .unwrap(),
        )
        .unwrap();

        let finished = sync_dispatch_receipt(&root, finished_id).unwrap();
        assert_eq!(finished.phase, "finished");
        assert_eq!(finished.request_id, finished_id);
        assert_eq!(
            finished
                .result
                .as_ref()
                .and_then(|result| result.get("message"))
                .and_then(JsonValue::as_str),
            Some("Synced 1 change.")
        );

        let failed_id = "sync-failed456";
        let failed = dispatch_failed_dir(&root).join(format!("{failed_id}.json"));
        fs::create_dir_all(failed.parent().unwrap()).unwrap();
        fs::write(&failed, "{}").unwrap();
        fs::write(failed.with_extension("error.txt"), "sync failed\n").unwrap();
        let failed = sync_dispatch_receipt(&root, failed_id).unwrap();
        assert_eq!(failed.phase, "failed");
        assert_eq!(failed.error.as_deref(), Some("sync failed"));

        let pending = sync_dispatch_receipt(&root, "sync-pending789").unwrap();
        assert_eq!(pending.phase, "pending");
        assert!(sync_dispatch_receipt(&root, "../escape").is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn status_counts_agents_that_lag_room_head() {
        let root = temp_dir("bartender-lagging-agent");
        let source = root.join("source");
        let agent = root.join("alice");
        let agent_cwd = root.join(".agent-workspaces").join("alice");
        fs::create_dir_all(&source).unwrap();
        git(&source, &["init"]).unwrap();
        git(&source, &["checkout", "-b", "main"]).unwrap();
        fs::write(source.join("README.md"), "hello\n").unwrap();
        git(&source, &["add", "-A"]).unwrap();
        git(
            &source,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "initial",
            ],
        )
        .unwrap();
        git(
            &source,
            &[
                "worktree",
                "add",
                "-b",
                "kota/alice",
                agent.to_str().unwrap(),
            ],
        )
        .unwrap();
        fs::create_dir_all(&agent_cwd).unwrap();
        fs::write(source.join("room.txt"), "room\n").unwrap();
        git(&source, &["add", "-A"]).unwrap();
        git(
            &source,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "room update",
            ],
        )
        .unwrap();

        let workspace = workspace_with_agent(&root, &source, &agent_cwd, &agent);

        let status = compute_status(&workspace).unwrap();
        assert_eq!(status.room_change_count, 1);
        assert_eq!(status.dirty_agents.len(), 1);
        assert_eq!(status.dirty_agents[0].change_count, 1);
        assert_eq!(status.dirty_agents[0].pending_commit_count, 0);
        assert_eq!(status.state, "roomDiff");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn status_counts_pending_agent_commits() {
        let root = temp_dir("bartender-pending-agent");
        let source = root.join("source");
        let agent = root.join("alice");
        let agent_cwd = root.join(".agent-workspaces").join("alice");
        fs::create_dir_all(&source).unwrap();
        git(&source, &["init"]).unwrap();
        git(&source, &["checkout", "-b", "main"]).unwrap();
        fs::write(source.join("README.md"), "hello\n").unwrap();
        git(&source, &["add", "-A"]).unwrap();
        git(
            &source,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "initial",
            ],
        )
        .unwrap();
        git(
            &source,
            &[
                "worktree",
                "add",
                "-b",
                "kota/alice",
                agent.to_str().unwrap(),
            ],
        )
        .unwrap();
        fs::create_dir_all(&agent_cwd).unwrap();
        fs::write(agent.join("alice.txt"), "agent\n").unwrap();
        git(&agent, &["add", "-A"]).unwrap();
        git(
            &agent,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "agent update",
            ],
        )
        .unwrap();

        let workspace = workspace_with_agent(&root, &source, &agent_cwd, &agent);

        let status = compute_status(&workspace).unwrap();
        assert_eq!(status.room_change_count, 1);
        assert_eq!(status.dirty_agents.len(), 1);
        assert_eq!(status.dirty_agents[0].change_count, 1);
        assert_eq!(status.dirty_agents[0].pending_commit_count, 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn status_excludes_left_agent_changes() {
        let root = temp_dir("bartender-left-agent");
        let source = root.join("source");
        let agent = root.join("alice");
        let agent_cwd = root.join(".agent-workspaces").join("alice");
        fs::create_dir_all(&source).unwrap();
        git(&source, &["init"]).unwrap();
        git(&source, &["checkout", "-b", "main"]).unwrap();
        fs::write(source.join("README.md"), "hello\n").unwrap();
        git(&source, &["add", "-A"]).unwrap();
        git(
            &source,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "initial",
            ],
        )
        .unwrap();
        git(
            &source,
            &[
                "worktree",
                "add",
                "-b",
                "kota/alice",
                agent.to_str().unwrap(),
            ],
        )
        .unwrap();
        fs::create_dir_all(&agent_cwd).unwrap();
        fs::write(agent_cwd.join("agent.yaml"), "status: left\n").unwrap();
        fs::write(agent.join("left.txt"), "left behind\n").unwrap();

        let workspace = workspace_with_agent(&root, &source, &agent_cwd, &agent);
        let status = compute_status(&workspace).unwrap();

        assert_eq!(status.room_change_count, 0);
        assert!(status.dirty_agents.is_empty());
        assert_eq!(status.state, "idle");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sync_does_not_publish_or_reset_archived_agent() {
        let root = temp_dir("bartender-archived-sync");
        let source = root.join("source");
        let agent = root.join("alice");
        let agent_cwd = root.join(".agent-workspaces").join("alice");
        fs::create_dir_all(&source).unwrap();
        git(&source, &["init"]).unwrap();
        git(&source, &["checkout", "-b", "main"]).unwrap();
        fs::write(source.join("README.md"), "hello\n").unwrap();
        git(&source, &["add", "-A"]).unwrap();
        git(
            &source,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "initial",
            ],
        )
        .unwrap();
        git(
            &source,
            &[
                "worktree",
                "add",
                "-b",
                "kota/alice",
                agent.to_str().unwrap(),
            ],
        )
        .unwrap();
        fs::create_dir_all(&agent_cwd).unwrap();
        fs::write(agent_cwd.join("agent.yaml"), "status: archived\n").unwrap();
        fs::write(agent.join("archived.txt"), "preserve me\n").unwrap();
        git(&agent, &["add", "-A"]).unwrap();
        git(
            &agent,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "archived work",
            ],
        )
        .unwrap();
        let archived_head = source_head(&agent).unwrap();

        let workspace = workspace_with_agent(&root, &source, &agent_cwd, &agent);
        let manager = BartenderManager::default();
        let result = manager.sync_local(&workspace).unwrap();

        assert!(result.ok, "{:?}", result.conflicts);
        assert_eq!(result.published_commit_count, 0);
        assert!(result.published_agents.is_empty());
        assert!(!source.join("archived.txt").exists());
        assert_eq!(source_head(&agent).unwrap(), archived_head);
        assert_eq!(
            fs::read_to_string(agent.join("archived.txt")).unwrap(),
            "preserve me\n"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sync_snapshots_dirty_agent_and_resets_worktree() {
        let root = temp_dir("bartender-sync");
        let source = root.join("source");
        let agent = root.join("alice");
        let agent_cwd = root.join(".agent-workspaces").join("alice");
        fs::create_dir_all(&source).unwrap();
        git(&source, &["init"]).unwrap();
        git(&source, &["checkout", "-b", "main"]).unwrap();
        fs::write(source.join("README.md"), "hello\n").unwrap();
        git(&source, &["add", "-A"]).unwrap();
        git(
            &source,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "initial",
            ],
        )
        .unwrap();
        git(
            &source,
            &[
                "worktree",
                "add",
                "-b",
                "kota/alice",
                agent.to_str().unwrap(),
            ],
        )
        .unwrap();
        fs::create_dir_all(&agent_cwd).unwrap();
        fs::write(agent.join("alice.txt"), "draft\n").unwrap();

        let workspace = WorkspaceProject {
            project_id: "proj".into(),
            repo_full_name: "mock/proj".into(),
            remote_url: "https://github.com/mock/proj.git".into(),
            github_html_url: "https://github.com/mock/proj".into(),
            default_branch: "main".into(),
            base_ref: "origin/main".into(),
            local_root: path_string(&root),
            local_root_bytes: 0,
            source_dir: path_string(&source),
            source_dir_bytes: 0,
            shared_dir: path_string(&root.join("project-memory")),
            rules_dir: path_string(&root.join("rules")),
            agents: vec![crate::integrations::AgentLaunchSpec {
                agent_id: "alice".into(),
                cli: crate::pty::agent::AgentCli::Codex.into(),
                cwd: path_string(&agent_cwd),
                project_root: path_string(&root),
                worktree_root: path_string(&agent),
                shared_dir: path_string(&root.join("project-memory")),
                rules_dir: path_string(&root.join("rules")),
                adapter_path: path_string(&agent_cwd.join("AGENTS.md")),
                project_id: "proj".into(),
                project_remote: "https://github.com/mock/proj.git".into(),
                project_base_ref: "origin/main".into(),
            }],
            archived: false,
            archived_at: None,
        };

        let manager = BartenderManager::default();
        let result = manager.sync_local(&workspace).unwrap();

        assert!(result.ok, "{:?}", result.conflicts);
        assert_eq!(
            fs::read_to_string(source.join("alice.txt")).unwrap(),
            "draft\n"
        );
        assert_eq!(porcelain_change_count(&agent).unwrap(), 0);
        assert_eq!(source_head(&source).unwrap(), source_head(&agent).unwrap());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sync_reports_agent_refresh_lock_failures() {
        let root = temp_dir("bartender-reset-lock");
        let source = root.join("source");
        let agent = root.join("alice");
        let agent_cwd = root.join(".agent-workspaces").join("alice");
        fs::create_dir_all(&source).unwrap();
        git(&source, &["init"]).unwrap();
        git(&source, &["checkout", "-b", "main"]).unwrap();
        fs::write(source.join("README.md"), "hello\n").unwrap();
        git(&source, &["add", "-A"]).unwrap();
        git(
            &source,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "initial",
            ],
        )
        .unwrap();
        git(
            &source,
            &[
                "worktree",
                "add",
                "-b",
                "kota/alice",
                agent.to_str().unwrap(),
            ],
        )
        .unwrap();
        fs::create_dir_all(&agent_cwd).unwrap();
        fs::write(source.join("room.txt"), "room\n").unwrap();
        git(&source, &["add", "-A"]).unwrap();
        git(
            &source,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "room update",
            ],
        )
        .unwrap();
        let lock_path = PathBuf::from(
            git(&agent, &["rev-parse", "--git-path", "index.lock"])
                .unwrap()
                .trim(),
        );
        fs::write(&lock_path, "locked\n").unwrap();

        let workspace = workspace_with_agent(&root, &source, &agent_cwd, &agent);
        let manager = BartenderManager::default();
        let result = manager.sync_local(&workspace).unwrap();

        assert!(!result.ok);
        assert!(result.conflicts.is_empty());
        assert!(result.message.contains("index.lock"));
        assert_eq!(result.status.room_change_count, 1);

        let _ = fs::remove_file(lock_path);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn status_counts_unique_changed_files_across_worktrees() {
        let root = temp_dir("bartender-unique-files");
        let source = root.join("source");
        let alice = root.join("alice");
        let bob = root.join("bob");
        let alice_cwd = root.join(".agent-workspaces").join("alice");
        let bob_cwd = root.join(".agent-workspaces").join("bob");
        fs::create_dir_all(&source).unwrap();
        git(&source, &["init"]).unwrap();
        git(&source, &["checkout", "-b", "main"]).unwrap();
        fs::write(source.join("shared.txt"), "base\n").unwrap();
        git(&source, &["add", "-A"]).unwrap();
        git(
            &source,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "initial",
            ],
        )
        .unwrap();
        git(
            &source,
            &[
                "worktree",
                "add",
                "-b",
                "kota/alice",
                alice.to_str().unwrap(),
            ],
        )
        .unwrap();
        git(
            &source,
            &["worktree", "add", "-b", "kota/bob", bob.to_str().unwrap()],
        )
        .unwrap();
        fs::create_dir_all(&alice_cwd).unwrap();
        fs::create_dir_all(&bob_cwd).unwrap();
        fs::write(source.join("shared.txt"), "human\n").unwrap();
        fs::write(alice.join("shared.txt"), "alice\n").unwrap();
        fs::write(bob.join("shared.txt"), "bob\n").unwrap();

        let agent =
            |agent_id: &str, cwd: &Path, worktree: &Path| crate::integrations::AgentLaunchSpec {
                agent_id: agent_id.into(),
                cli: crate::pty::agent::AgentCli::Codex.into(),
                cwd: path_string(cwd),
                project_root: path_string(&root),
                worktree_root: path_string(worktree),
                shared_dir: path_string(&root.join("project-memory")),
                rules_dir: path_string(&root.join("rules")),
                adapter_path: path_string(&cwd.join("AGENTS.md")),
                project_id: "proj".into(),
                project_remote: "https://github.com/mock/proj.git".into(),
                project_base_ref: "origin/main".into(),
            };
        let workspace = WorkspaceProject {
            project_id: "proj".into(),
            repo_full_name: "mock/proj".into(),
            remote_url: "https://github.com/mock/proj.git".into(),
            github_html_url: "https://github.com/mock/proj".into(),
            default_branch: "main".into(),
            base_ref: "origin/main".into(),
            local_root: path_string(&root),
            local_root_bytes: 0,
            source_dir: path_string(&source),
            source_dir_bytes: 0,
            shared_dir: path_string(&root.join("project-memory")),
            rules_dir: path_string(&root.join("rules")),
            agents: vec![
                agent("alice", &alice_cwd, &alice),
                agent("bob", &bob_cwd, &bob),
            ],
            archived: false,
            archived_at: None,
        };

        let status = compute_status(&workspace).unwrap();
        assert_eq!(status.source_change_count, 1);
        assert_eq!(status.room_change_count, 1);
        assert!(status.message.contains("1 changed file"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn status_counts_dirty_source_worktree() {
        let root = temp_dir("bartender-source-dirty");
        let source = root.join("source");
        fs::create_dir_all(&source).unwrap();
        git(&source, &["init"]).unwrap();
        git(&source, &["checkout", "-b", "main"]).unwrap();
        fs::write(source.join("README.md"), "hello\n").unwrap();
        git(&source, &["add", "-A"]).unwrap();
        git(
            &source,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "initial",
            ],
        )
        .unwrap();
        fs::write(source.join("room.txt"), "draft\n").unwrap();

        let workspace = WorkspaceProject {
            project_id: "proj".into(),
            repo_full_name: "mock/proj".into(),
            remote_url: "https://github.com/mock/proj.git".into(),
            github_html_url: "https://github.com/mock/proj".into(),
            default_branch: "main".into(),
            base_ref: "origin/main".into(),
            local_root: path_string(&root),
            local_root_bytes: 0,
            source_dir: path_string(&source),
            source_dir_bytes: 0,
            shared_dir: path_string(&root.join("project-memory")),
            rules_dir: path_string(&root.join("rules")),
            agents: Vec::new(),
            archived: false,
            archived_at: None,
        };

        let status = compute_status(&workspace).unwrap();
        assert_eq!(status.source_change_count, 1);
        assert_eq!(status.room_change_count, 1);
        assert_eq!(status.state, "roomDiff");
        assert!(status.message.contains("source repo"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn initial_push_publishes_empty_remote_and_sets_upstream() {
        let root = temp_dir("bartender-initial-push");
        let source = root.join("source");
        let remote = root.join("remote.git");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&remote).unwrap();
        git(&remote, &["init", "--bare"]).unwrap();
        git(&source, &["init"]).unwrap();
        git(&source, &["checkout", "-b", "main"]).unwrap();
        fs::write(source.join("README.md"), "hello\n").unwrap();
        git(&source, &["add", "-A"]).unwrap();
        git(
            &source,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "initial",
            ],
        )
        .unwrap();
        git(
            &source,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        )
        .unwrap();

        let workspace = workspace_without_agents(&root, &source);
        let status = compute_status(&workspace).unwrap();
        assert_eq!(status.github_change_count, 0);
        assert!(status.github_needs_initial_push);
        assert_eq!(status.github_push_branch.as_deref(), Some("main"));
        assert_eq!(status.github_initial_push_commit_count, 1);
        assert_eq!(status.state, "githubInitialPush");

        let manager = BartenderManager::default();
        let result = manager.push_to_github(&workspace).unwrap();
        assert!(result.ok, "{}", result.message);
        assert_eq!(result.pushed_commit_count, 1);
        assert_eq!(upstream_ref(&source).as_deref(), Some("origin/main"));
        assert!(git(&remote, &["show-ref", "--verify", "refs/heads/main"]).is_ok());
        assert!(!result.status.github_needs_initial_push);
        assert_eq!(result.status.github_change_count, 0);
        assert_eq!(result.status.state, "idle");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn initial_push_requires_origin_remote() {
        let root = temp_dir("bartender-initial-no-origin");
        let source = root.join("source");
        fs::create_dir_all(&source).unwrap();
        git(&source, &["init"]).unwrap();
        git(&source, &["checkout", "-b", "main"]).unwrap();
        fs::write(source.join("README.md"), "hello\n").unwrap();
        git(&source, &["add", "-A"]).unwrap();
        git(
            &source,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "initial",
            ],
        )
        .unwrap();

        let status = compute_status(&workspace_without_agents(&root, &source)).unwrap();
        assert!(!status.github_needs_initial_push);
        assert_eq!(status.state, "idle");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn initial_push_ignores_detached_head() {
        let root = temp_dir("bartender-initial-detached");
        let source = root.join("source");
        let remote = root.join("remote.git");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&remote).unwrap();
        git(&remote, &["init", "--bare"]).unwrap();
        git(&source, &["init"]).unwrap();
        git(&source, &["checkout", "-b", "main"]).unwrap();
        fs::write(source.join("README.md"), "hello\n").unwrap();
        git(&source, &["add", "-A"]).unwrap();
        git(
            &source,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "initial",
            ],
        )
        .unwrap();
        git(
            &source,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        )
        .unwrap();
        git(&source, &["checkout", "--detach", "HEAD"]).unwrap();

        let status = compute_status(&workspace_without_agents(&root, &source)).unwrap();
        assert!(!status.github_needs_initial_push);
        assert_eq!(status.state, "idle");

        let _ = fs::remove_dir_all(root);
    }

    fn temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("kota-{label}-{nanos}"))
    }

    fn workspace_without_agents(root: &Path, source: &Path) -> WorkspaceProject {
        WorkspaceProject {
            project_id: "proj".into(),
            repo_full_name: "mock/proj".into(),
            remote_url: "https://github.com/mock/proj.git".into(),
            github_html_url: "https://github.com/mock/proj".into(),
            default_branch: "main".into(),
            base_ref: "origin/main".into(),
            local_root: path_string(root),
            local_root_bytes: 0,
            source_dir: path_string(source),
            source_dir_bytes: 0,
            shared_dir: path_string(&root.join("project-memory")),
            rules_dir: path_string(&root.join("rules")),
            agents: Vec::new(),
            archived: false,
            archived_at: None,
        }
    }

    fn workspace_with_agent(
        root: &Path,
        source: &Path,
        agent_cwd: &Path,
        agent: &Path,
    ) -> WorkspaceProject {
        WorkspaceProject {
            project_id: "proj".into(),
            repo_full_name: "mock/proj".into(),
            remote_url: "https://github.com/mock/proj.git".into(),
            github_html_url: "https://github.com/mock/proj".into(),
            default_branch: "main".into(),
            base_ref: "origin/main".into(),
            local_root: path_string(root),
            local_root_bytes: 0,
            source_dir: path_string(source),
            source_dir_bytes: 0,
            shared_dir: path_string(&root.join("project-memory")),
            rules_dir: path_string(&root.join("rules")),
            agents: vec![crate::integrations::AgentLaunchSpec {
                agent_id: "alice".into(),
                cli: crate::pty::agent::AgentCli::Codex.into(),
                cwd: path_string(agent_cwd),
                project_root: path_string(root),
                worktree_root: path_string(agent),
                shared_dir: path_string(&root.join("project-memory")),
                rules_dir: path_string(&root.join("rules")),
                adapter_path: path_string(&agent_cwd.join("AGENTS.md")),
                project_id: "proj".into(),
                project_remote: "https://github.com/mock/proj.git".into(),
                project_base_ref: "origin/main".into(),
            }],
            archived: false,
            archived_at: None,
        }
    }
}
