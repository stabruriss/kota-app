//! Real provider integration scaffold for Kota's mixed sync model.
//!
//! GitHub owns project source. Google Drive owns Kota account/workspace
//! sync. Local git worktrees remain the execution surface for agent CLIs.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const SECRET_SERVICE: &str = "Kota";
const GOOGLE_SECRET_ACCOUNT: &str = "google-drive";
const GOOGLE_DRIVE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
const GOOGLE_IDENTITY_SCOPE: &str = "openid email profile";
const DRIVE_FOLDER_MIME: &str = "application/vnd.google-apps.folder";
pub(crate) const PROJECT_AGENT_ID_PREFIX: &str = "agent-";
const PROJECT_AGENT_ID_SUFFIX_LEN: usize = 10;
const KOTA_HOME_DIR: &str = "Kota";
const KOTA_WORKSPACES_DIR: &str = "Workspaces";
const STORAGE_MEASUREMENT_CACHE_FILE: &str = "storage-measurement.json";
const STORAGE_MEASUREMENT_CACHE_VERSION: u32 = 1;
const STORAGE_MEASUREMENT_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const STORAGE_MEASUREMENT_POLL_INTERVAL: Duration = Duration::from_millis(250);
#[cfg(not(test))]
const LEGACY_KOTA_HOME_DIR: &str = ".kota";
#[cfg(not(test))]
const LEGACY_WORKSPACES_DIR: &str = "projects";

pub struct IntegrationManager {
    active_workspace: Mutex<Option<WorkspaceProject>>,
    storage_measurement: StorageMeasurementController,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StorageMeasurementRecord {
    version: u32,
    on_disk_bytes: u64,
    available_bytes: u64,
    measured_at: i64,
    app_version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageMeasurementStatus {
    pub updating: bool,
    pub on_disk_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
    pub measured_at: Option<i64>,
    pub error: Option<String>,
}

struct StorageMeasurementRuntime {
    last_success: Option<StorageMeasurementRecord>,
    updating: bool,
    error: Option<String>,
    active_job_id: u64,
    child: Option<Child>,
    shutting_down: bool,
}

struct StorageMeasurementController {
    runtime: Arc<Mutex<StorageMeasurementRuntime>>,
    account_root: PathBuf,
    cache_path: PathBuf,
}

impl Default for IntegrationManager {
    fn default() -> Self {
        let account_root = kota_home();
        let cache_path = account_root.join(STORAGE_MEASUREMENT_CACHE_FILE);
        let last_success = match load_storage_measurement_record(&cache_path) {
            Ok(record) => record,
            Err(err) => {
                eprintln!("Kota storage measurement cache ignored: {err}");
                None
            }
        };
        Self {
            active_workspace: Mutex::new(None),
            storage_measurement: StorageMeasurementController {
                runtime: Arc::new(Mutex::new(StorageMeasurementRuntime {
                    last_success,
                    updating: false,
                    error: None,
                    active_job_id: 0,
                    child: None,
                    shutting_down: false,
                })),
                account_root,
                cache_path,
            },
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthConfig {
    pub google_client_id: Option<String>,
    pub google_client_secret: Option<String>,
    pub github_client_id: Option<String>,
    pub google_drive_path: Option<String>,
    pub local_project_root: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthConfigStatus {
    pub google_configured: bool,
    pub github_configured: bool,
    pub config_path: String,
    pub app_path: String,
    pub google_drive_path: String,
    pub local_account_folder: String,
    pub local_project_root: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleToken {
    access_token: String,
    refresh_token: Option<String>,
    token_type: Option<String>,
    scope: Option<String>,
    expires_at: i64,
    email: Option<String>,
    drive_folder_id: Option<String>,
    drive_folder_name: Option<String>,
    drive_folder_path: Option<String>,
    drive_folder_url: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleDriveStatus {
    pub connected: bool,
    pub email: Option<String>,
    pub scopes: Vec<String>,
    pub folder_id: Option<String>,
    pub folder_name: Option<String>,
    pub folder_path: Option<String>,
    pub folder_url: Option<String>,
    pub local_account_folder: String,
    pub config_missing: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubToken {
    access_token: String,
    token_type: Option<String>,
    scope: Option<String>,
    username: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubRepo {
    pub full_name: String,
    pub name: String,
    pub owner: String,
    pub private: bool,
    pub default_branch: String,
    pub clone_url: String,
    pub html_url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubCreateRepoRequest {
    pub name: String,
    pub private: bool,
    pub auto_init: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareGithubProjectRequest {
    pub repo_full_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceProject {
    pub project_id: String,
    pub repo_full_name: String,
    pub remote_url: String,
    #[serde(default)]
    pub github_html_url: String,
    pub default_branch: String,
    pub base_ref: String,
    pub local_root: String,
    #[serde(default)]
    pub local_root_bytes: u64,
    pub source_dir: String,
    #[serde(default)]
    pub source_dir_bytes: u64,
    pub shared_dir: String,
    pub rules_dir: String,
    pub agents: Vec<AgentLaunchSpec>,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub archived_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentLaunchSpec {
    pub agent_id: String,
    pub cli: WorkspaceAgentCli,
    pub cwd: String,
    pub project_root: String,
    pub worktree_root: String,
    pub shared_dir: String,
    pub rules_dir: String,
    pub adapter_path: String,
    pub project_id: String,
    pub project_remote: String,
    pub project_base_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WorkspaceAgentCli {
    Supported(crate::pty::agent::AgentCli),
    Unsupported(String),
}

impl WorkspaceAgentCli {
    fn from_name(name: &str) -> Option<Self> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        Some(
            agent_cli_from_name(name)
                .map(Self::Supported)
                .unwrap_or_else(|| Self::Unsupported(name.to_string())),
        )
    }

    pub fn supported(&self) -> Option<crate::pty::agent::AgentCli> {
        match self {
            Self::Supported(cli) => Some(*cli),
            Self::Unsupported(_) => None,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Supported(crate::pty::agent::AgentCli::Claude) => "claude",
            Self::Supported(crate::pty::agent::AgentCli::Codex) => "codex",
            Self::Supported(crate::pty::agent::AgentCli::Antigravity) => "antigravity",
            Self::Supported(crate::pty::agent::AgentCli::Opencode) => "opencode",
            Self::Supported(crate::pty::agent::AgentCli::Pi) => "pi",
            Self::Supported(crate::pty::agent::AgentCli::Kimi) => "kimi",
            Self::Unsupported(name) => name,
        }
    }

    fn unsupported_name(&self) -> Option<&str> {
        match self {
            Self::Supported(_) => None,
            Self::Unsupported(name) => Some(name),
        }
    }
}

impl From<crate::pty::agent::AgentCli> for WorkspaceAgentCli {
    fn from(cli: crate::pty::agent::AgentCli) -> Self {
        Self::Supported(cli)
    }
}

impl PartialEq<crate::pty::agent::AgentCli> for WorkspaceAgentCli {
    fn eq(&self, other: &crate::pty::agent::AgentCli) -> bool {
        self.supported().as_ref() == Some(other)
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct AgentWorkspaceCliYaml {
    shell: Option<String>,
    provider: Option<String>,
    command: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct AgentWorkspaceStatusYaml {
    status: Option<String>,
}

pub(crate) fn agent_launch_spec_is_active(agent: &AgentLaunchSpec) -> bool {
    agent_status_is_active(agent_workspace_status(&PathBuf::from(&agent.cwd)).as_deref())
}

fn agent_workspace_status(cwd: &Path) -> Option<String> {
    let parsed: AgentWorkspaceStatusYaml =
        serde_yaml::from_str(&fs::read_to_string(cwd.join("agent.yaml")).ok()?).ok()?;
    parsed.status
}

fn agent_status_is_active(status: Option<&str>) -> bool {
    // Whitelist active collaboration states; new active lifecycle states must be registered here.
    let status = status.unwrap_or("active").trim();
    status.is_empty() || status.eq_ignore_ascii_case("active")
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceStatus {
    pub active: Option<WorkspaceProject>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceProjectLifecycleRequest {
    pub project_id: String,
    #[serde(default)]
    pub force_dirty: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceProjectLifecycleResult {
    pub ok: bool,
    pub dirty: bool,
    pub dirty_summary: String,
    pub project: Option<WorkspaceProject>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceProjectDirtyStatus {
    pub dirty: bool,
    pub dirty_summary: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceProjectMeta {
    version: u32,
    project_id: String,
    repo_full_name: String,
    remote_url: String,
    github_html_url: String,
    default_branch: String,
    base_ref: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceLocalState {
    version: u32,
    local_root: String,
    source_dir: String,
    shared_dir: String,
    rules_dir: String,
    updated_at: i64,
}

impl IntegrationManager {
    pub fn storage_measure_status(&self) -> StorageMeasurementStatus {
        self.storage_measurement.status()
    }

    pub fn storage_measure_start(&self) -> StorageMeasurementStatus {
        self.storage_measurement.start()
    }

    pub fn shutdown_storage_measurement(&self) {
        self.storage_measurement.shutdown();
    }

    pub fn auth_config_status(&self) -> OAuthConfigStatus {
        let cfg = load_oauth_config().unwrap_or_default();
        let local_account_folder = kota_home();
        let local_project_root = local_project_root(&cfg);
        let _ = fs::create_dir_all(&local_account_folder);
        let _ = fs::create_dir_all(&local_project_root);
        OAuthConfigStatus {
            google_configured: cfg
                .google_client_id
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty()),
            github_configured: cfg
                .github_client_id
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty()),
            config_path: oauth_config_path().display().to_string(),
            app_path: std::env::current_exe()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| "unknown".into()),
            google_drive_path: google_drive_path(&cfg),
            local_account_folder: local_account_folder.display().to_string(),
            local_project_root: local_project_root.display().to_string(),
        }
    }

    pub fn save_auth_config(&self, next: OAuthConfig) -> Result<OAuthConfigStatus> {
        let mut merged = load_oauth_config().unwrap_or_default();
        if next
            .google_client_id
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
        {
            merged.google_client_id = next.google_client_id.map(|s| s.trim().to_string());
        }
        if next
            .google_client_secret
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
        {
            merged.google_client_secret = next.google_client_secret.map(|s| s.trim().to_string());
        }
        if next
            .github_client_id
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
        {
            merged.github_client_id = next.github_client_id.map(|s| s.trim().to_string());
        }
        if next
            .google_drive_path
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
        {
            merged.google_drive_path = next.google_drive_path.map(|s| s.trim().to_string());
        }
        if next
            .local_project_root
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
        {
            merged.local_project_root = next.local_project_root.map(|s| expand_home(s.trim()));
        }
        save_oauth_config_file(&merged)?;
        Ok(self.auth_config_status())
    }

    pub fn google_drive_status(&self) -> GoogleDriveStatus {
        let cfg = load_oauth_config().unwrap_or_default();
        let config_missing = cfg
            .google_client_id
            .as_deref()
            .map_or(true, |s| s.trim().is_empty());
        match read_secret_json::<GoogleToken>(GOOGLE_SECRET_ACCOUNT) {
            Ok(token) => GoogleDriveStatus {
                connected: true,
                email: token.email,
                scopes: split_scope(token.scope.as_deref()),
                folder_id: token.drive_folder_id,
                folder_name: token.drive_folder_name,
                folder_path: token.drive_folder_path,
                folder_url: token.drive_folder_url,
                local_account_folder: kota_home().display().to_string(),
                config_missing,
                error: None,
            },
            Err(err) => GoogleDriveStatus {
                connected: false,
                email: None,
                scopes: Vec::new(),
                folder_id: None,
                folder_name: None,
                folder_path: None,
                folder_url: None,
                local_account_folder: kota_home().display().to_string(),
                config_missing,
                error: Some(err.to_string()),
            },
        }
    }

    pub fn google_drive_connect_and_setup(
        &self,
        drive_path: Option<String>,
    ) -> Result<GoogleDriveStatus> {
        let mut token = google_oauth_loopback()?;
        let cfg = load_oauth_config().unwrap_or_default();
        let drive_path = drive_path
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| google_drive_path(&cfg));
        ensure_drive_folder(&mut token, &drive_path)?;
        write_secret_json(GOOGLE_SECRET_ACCOUNT, &token)?;
        Ok(self.google_drive_status())
    }

    pub fn google_drive_disconnect(&self) -> Result<GoogleDriveStatus> {
        delete_secret(GOOGLE_SECRET_ACCOUNT)?;
        Ok(self.google_drive_status())
    }

    pub fn github_list_repos(&self) -> Result<Vec<GithubRepo>> {
        let token = github_cli_token()?;
        let value = github_get_json(
            &token.access_token,
            "https://api.github.com/user/repos?affiliation=owner,collaborator,organization_member&sort=updated&per_page=100",
        )?;
        parse_repo_list(&value)
    }

    pub fn github_create_repo(&self, req: GithubCreateRepoRequest) -> Result<GithubRepo> {
        let token = github_cli_token()?;
        let name = req.name.trim();
        if name.is_empty() {
            bail!("repo name is required");
        }
        let value = github_post_json(
            &token.access_token,
            "https://api.github.com/user/repos",
            json!({
                "name": name,
                "private": req.private,
                "auto_init": req.auto_init,
            }),
        )?;
        parse_repo(&value)
    }

    pub fn prepare_github_project(
        &self,
        req: PrepareGithubProjectRequest,
    ) -> Result<WorkspaceProject> {
        let token = github_cli_token()?;
        let repo = github_repo(&token.access_token, &req.repo_full_name)?;
        let workspace = materialize_github_workspace(&token.access_token, repo)?;
        *self
            .active_workspace
            .lock()
            .expect("workspace state poisoned") = Some(workspace.clone());
        save_active_workspace(&workspace)?;
        Ok(workspace)
    }

    pub fn workspace_status(&self) -> WorkspaceStatus {
        let mut guard = self
            .active_workspace
            .lock()
            .expect("workspace state poisoned");
        let mut changed_on_load = false;
        if guard.is_none() {
            if let Ok(mut workspace) = load_active_workspace() {
                changed_on_load = prepare_loaded_workspace(&mut workspace).unwrap_or(false);
                *guard = Some(workspace);
            }
        }
        if let Some(workspace) = guard.as_mut() {
            let changed = changed_on_load || prepare_loaded_workspace(workspace).unwrap_or(false);
            if workspace.archived {
                *guard = None;
                let _ = clear_active_workspace();
            } else if changed {
                let _ = save_active_workspace(workspace);
            }
        }
        WorkspaceStatus {
            active: guard.clone(),
        }
    }

    pub fn list_workspaces(&self) -> Result<Vec<WorkspaceProject>> {
        let projects_dir = kota_workspaces_dir();
        if !projects_dir.exists() {
            return Ok(Vec::new());
        }
        let mut workspaces = Vec::new();
        for entry in fs::read_dir(projects_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if let Ok(mut workspace) = load_workspace_at(&path) {
                let _ = prepare_loaded_workspace(&mut workspace);
                if !workspace.archived {
                    workspaces.push(workspace);
                }
            }
        }
        workspaces.sort_by(|a, b| a.repo_full_name.cmp(&b.repo_full_name));
        Ok(workspaces)
    }

    pub fn list_archived_workspaces(&self) -> Result<Vec<WorkspaceProject>> {
        let projects_dir = kota_workspaces_dir();
        if !projects_dir.exists() {
            return Ok(Vec::new());
        }
        let mut workspaces = Vec::new();
        for entry in fs::read_dir(projects_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if let Ok(mut workspace) = load_workspace_at(&path) {
                let _ = prepare_loaded_workspace(&mut workspace);
                if workspace.archived {
                    workspaces.push(workspace);
                }
            }
        }
        workspaces.sort_by(|a, b| a.repo_full_name.cmp(&b.repo_full_name));
        Ok(workspaces)
    }

    pub fn open_workspace_project(&self, project_id: String) -> Result<WorkspaceProject> {
        let project_id = project_id.trim();
        if project_id.is_empty() {
            bail!("project id is required");
        }
        let mut workspace = load_workspace_at(&workspace_project_root(project_id))?;
        let _ = prepare_loaded_workspace(&mut workspace);
        workspace.archived = false;
        workspace.archived_at = None;
        save_workspace_files(&workspace)?;
        *self
            .active_workspace
            .lock()
            .expect("workspace state poisoned") = Some(workspace.clone());
        save_active_workspace(&workspace)?;
        Ok(workspace)
    }

    pub fn inspect_workspace_project(
        &self,
        project_id: String,
    ) -> Result<WorkspaceProjectDirtyStatus> {
        let mut workspace = load_workspace_at(&workspace_project_root(&project_id))?;
        let _ = prepare_loaded_workspace(&mut workspace);
        let dirty_summary = workspace_dirty_summary(&workspace);
        Ok(WorkspaceProjectDirtyStatus {
            dirty: !dirty_summary.trim().is_empty(),
            dirty_summary,
        })
    }

    pub fn workspace_project(&self, project_id: String) -> Result<WorkspaceProject> {
        let project_id = project_id.trim();
        if project_id.is_empty() {
            bail!("project id is required");
        }
        let mut guard = self
            .active_workspace
            .lock()
            .expect("workspace state poisoned");
        if let Some(workspace) = guard
            .as_mut()
            .filter(|workspace| workspace.project_id == project_id)
        {
            let _ = prepare_loaded_workspace(workspace);
            return Ok(workspace.clone());
        }
        let mut workspace = load_workspace_at(&workspace_project_root(project_id))?;
        let _ = prepare_loaded_workspace(&mut workspace);
        Ok(workspace)
    }

    pub fn archive_workspace_project(
        &self,
        req: WorkspaceProjectLifecycleRequest,
    ) -> Result<WorkspaceProjectLifecycleResult> {
        let mut workspace = load_workspace_at(&workspace_project_root(&req.project_id))?;
        let _ = prepare_loaded_workspace(&mut workspace);
        let dirty_summary = workspace_dirty_summary(&workspace);
        let dirty = !dirty_summary.trim().is_empty();
        if dirty && !req.force_dirty {
            return Ok(WorkspaceProjectLifecycleResult {
                ok: false,
                dirty,
                dirty_summary,
                project: Some(workspace),
            });
        }
        workspace.archived = true;
        workspace.archived_at = Some(chrono::Utc::now().to_rfc3339());
        save_workspace_files(&workspace)?;
        let mut guard = self
            .active_workspace
            .lock()
            .expect("workspace state poisoned");
        if guard
            .as_ref()
            .is_some_and(|active| active.project_id == workspace.project_id)
        {
            *guard = None;
            clear_active_workspace()?;
        }
        Ok(WorkspaceProjectLifecycleResult {
            ok: true,
            dirty,
            dirty_summary,
            project: Some(workspace),
        })
    }

    pub fn resume_workspace_project(&self, project_id: String) -> Result<WorkspaceProject> {
        self.open_workspace_project(project_id)
    }

    pub fn remove_workspace_project(
        &self,
        req: WorkspaceProjectLifecycleRequest,
    ) -> Result<WorkspaceProjectLifecycleResult> {
        let root = workspace_project_root(&req.project_id);
        let mut workspace = load_workspace_at(&root)?;
        let _ = prepare_loaded_workspace(&mut workspace);
        let dirty_summary = workspace_dirty_summary(&workspace);
        let dirty = !dirty_summary.trim().is_empty();
        if dirty && !req.force_dirty {
            return Ok(WorkspaceProjectLifecycleResult {
                ok: false,
                dirty,
                dirty_summary,
                project: Some(workspace),
            });
        }
        let mut guard = self
            .active_workspace
            .lock()
            .expect("workspace state poisoned");
        if guard
            .as_ref()
            .is_some_and(|active| active.project_id == workspace.project_id)
        {
            *guard = None;
            clear_active_workspace()?;
        }
        if root.exists() {
            fs::remove_dir_all(&root)
                .with_context(|| format!("remove workspace account dir {}", root.display()))?;
        }
        Ok(WorkspaceProjectLifecycleResult {
            ok: true,
            dirty,
            dirty_summary,
            project: Some(workspace),
        })
    }

    pub fn resolve_agent_launch(
        &self,
        agent_id: String,
        cli: crate::pty::agent::AgentCli,
    ) -> Result<AgentLaunchSpec> {
        let mut guard = self
            .active_workspace
            .lock()
            .expect("workspace state poisoned");
        if guard.is_none() {
            if let Ok(mut workspace) = load_active_workspace() {
                let launch_is_unsupported = workspace.agents.iter().any(|existing| {
                    existing.agent_id == agent_id && existing.cli.unsupported_name().is_some()
                });
                if !launch_is_unsupported {
                    let _ = prepare_loaded_workspace(&mut workspace);
                }
                *guard = Some(workspace);
            }
        }
        let workspace = guard
            .as_mut()
            .ok_or_else(|| anyhow!("no active GitHub workspace; prepare a repo first"))?;
        if workspace.archived {
            bail!("active workspace is archived; resume it before launching agents");
        }
        if let Some(existing) = workspace
            .agents
            .iter()
            .find(|existing| existing.agent_id == agent_id)
        {
            if let Some(provider) = existing.cli.unsupported_name() {
                bail!(
                    "agent {agent_id} uses unsupported CLI provider {provider:?}; update Kota before launching it"
                );
            }
        }
        let spec = materialize_workspace_agent(workspace, &agent_id, cli)?;
        if let Some(existing) = workspace
            .agents
            .iter_mut()
            .find(|existing| existing.agent_id == agent_id && existing.cli == cli)
        {
            *existing = spec.clone();
        } else {
            workspace.agents.push(spec.clone());
        }
        save_active_workspace(workspace)?;
        save_workspace_files(workspace)?;
        Ok(spec)
    }

    pub fn upsert_active_workspace_agent(&self, spec: AgentLaunchSpec) -> Result<()> {
        let mut guard = self
            .active_workspace
            .lock()
            .expect("workspace state poisoned");
        if guard.is_none() {
            if let Ok(mut workspace) = load_active_workspace() {
                let _ = prepare_loaded_workspace(&mut workspace);
                *guard = Some(workspace);
            }
        }
        let workspace = guard
            .as_mut()
            .ok_or_else(|| anyhow!("no active GitHub workspace; prepare a repo first"))?;
        if workspace.project_id != spec.project_id {
            return Ok(());
        }
        if upsert_workspace_agent_spec(workspace, spec) {
            save_active_workspace(workspace)?;
            save_workspace_files(workspace)?;
        }
        Ok(())
    }
}

fn google_oauth_loopback() -> Result<GoogleToken> {
    let cfg = load_oauth_config()?;
    let client_id = cfg
        .google_client_id
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow!("Google client id missing; save OAuth config first"))?
        .to_string();
    let client_secret = cfg.google_client_secret.clone();
    let listener = TcpListener::bind("127.0.0.1:0").context("bind Google OAuth loopback")?;
    listener
        .set_nonblocking(true)
        .context("set Google OAuth loopback nonblocking")?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/oauth/google");
    let state = Uuid::new_v4().simple().to_string();
    let verifier = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let scope = format!("{GOOGLE_DRIVE_SCOPE} {GOOGLE_IDENTITY_SCOPE}");
    let auth_url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?{}",
        form_body(&[
            ("client_id", client_id.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("response_type", "code"),
            ("scope", scope.as_str()),
            ("state", state.as_str()),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("access_type", "offline"),
            ("prompt", "consent"),
        ])
    );
    open_url(&auth_url)?;

    let query = wait_for_loopback_query(&listener, Duration::from_secs(180))?;
    if query.get("state") != Some(&state) {
        bail!("Google OAuth state mismatch");
    }
    if let Some(error) = query.get("error") {
        bail!("Google OAuth denied: {error}");
    }
    let code = query
        .get("code")
        .ok_or_else(|| anyhow!("Google OAuth callback missing code"))?;

    let mut pairs = vec![
        ("client_id", client_id.as_str()),
        ("code", code.as_str()),
        ("code_verifier", verifier.as_str()),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect_uri.as_str()),
    ];
    if let Some(secret) = client_secret.as_deref().filter(|s| !s.trim().is_empty()) {
        pairs.push(("client_secret", secret));
    }
    let value = response_json(
        ureq::post("https://oauth2.googleapis.com/token")
            .set("Accept", "application/json")
            .set("User-Agent", "Kota")
            .set("Content-Type", "application/x-www-form-urlencoded")
            .send_string(&form_body(&pairs)),
    )?;

    let access_token = json_string(&value, "access_token")?;
    let expires_in = json_i64(&value, "expires_in").unwrap_or(3600);
    let email = google_userinfo(&access_token).ok();
    Ok(GoogleToken {
        access_token,
        refresh_token: value
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        token_type: value
            .get("token_type")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        scope: value
            .get("scope")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        expires_at: now_ts() + expires_in,
        email,
        drive_folder_id: None,
        drive_folder_name: None,
        drive_folder_path: None,
        drive_folder_url: None,
    })
}

fn ensure_drive_folder(token: &mut GoogleToken, drive_path: &str) -> Result<()> {
    let access_token = google_valid_access_token(token)?;
    let root = ensure_drive_path(&access_token, drive_path)?;
    for child in ["projects", "blobs", "uploads", "leases"] {
        if drive_find_child_folder(&access_token, &root.id, child)?.is_none() {
            let _ = drive_create_folder(&access_token, child, Some(&root.id))?;
        }
    }
    token.drive_folder_id = Some(root.id);
    token.drive_folder_name = Some(root.name);
    token.drive_folder_path = Some(normalize_drive_path(drive_path));
    token.drive_folder_url = root.web_view_link;
    write_secret_json(GOOGLE_SECRET_ACCOUNT, token)?;
    Ok(())
}

#[derive(Clone, Debug)]
struct DriveFolder {
    id: String,
    name: String,
    web_view_link: Option<String>,
}

fn drive_find_folder(access_token: &str, name: &str) -> Result<Option<DriveFolder>> {
    let q = format!(
        "name = '{}' and mimeType = '{}' and trashed = false",
        escape_drive_query(name),
        DRIVE_FOLDER_MIME
    );
    drive_find_folder_query(access_token, &q)
}

fn ensure_drive_path(access_token: &str, raw_path: &str) -> Result<DriveFolder> {
    let parts = normalize_drive_path(raw_path)
        .split('/')
        .filter(|part| !part.trim().is_empty())
        .map(|part| part.trim().to_string())
        .collect::<Vec<_>>();
    let parts = if parts.is_empty() {
        vec!["Kota Sync".to_string()]
    } else {
        parts
    };
    let mut parent_id: Option<String> = None;
    let mut current: Option<DriveFolder> = None;
    for part in parts {
        let existing = if let Some(parent) = parent_id.as_deref() {
            drive_find_child_folder(access_token, parent, &part)?
        } else {
            drive_find_folder(access_token, &part)?
        };
        let folder = match existing {
            Some(folder) => folder,
            None => drive_create_folder(access_token, &part, parent_id.as_deref())?,
        };
        parent_id = Some(folder.id.clone());
        current = Some(folder);
    }
    current.ok_or_else(|| anyhow!("could not create Drive path"))
}

fn drive_find_child_folder(
    access_token: &str,
    parent: &str,
    name: &str,
) -> Result<Option<DriveFolder>> {
    let q = format!(
        "'{}' in parents and name = '{}' and mimeType = '{}' and trashed = false",
        escape_drive_query(parent),
        escape_drive_query(name),
        DRIVE_FOLDER_MIME
    );
    drive_find_folder_query(access_token, &q)
}

fn drive_find_folder_query(access_token: &str, q: &str) -> Result<Option<DriveFolder>> {
    let url = format!(
        "https://www.googleapis.com/drive/v3/files?q={}&fields=files(id,name,webViewLink)&pageSize=1&spaces=drive",
        urlencoding::encode(q)
    );
    let value = response_json(
        ureq::get(&url)
            .set("Accept", "application/json")
            .set("User-Agent", "Kota")
            .set("Authorization", &format!("Bearer {access_token}"))
            .call(),
    )?;
    Ok(value
        .get("files")
        .and_then(Value::as_array)
        .and_then(|files| files.first())
        .and_then(parse_drive_folder))
}

fn drive_create_folder(
    access_token: &str,
    name: &str,
    parent: Option<&str>,
) -> Result<DriveFolder> {
    let mut body = json!({
        "name": name,
        "mimeType": DRIVE_FOLDER_MIME,
    });
    if let Some(parent) = parent {
        body["parents"] = json!([parent]);
    }
    let value = response_json(
        ureq::post("https://www.googleapis.com/drive/v3/files?fields=id,name,webViewLink")
            .set("Accept", "application/json")
            .set("User-Agent", "Kota")
            .set("Authorization", &format!("Bearer {access_token}"))
            .send_json(body),
    )?;
    parse_drive_folder(&value)
        .ok_or_else(|| anyhow!("Drive folder create returned malformed response"))
}

fn parse_drive_folder(value: &Value) -> Option<DriveFolder> {
    Some(DriveFolder {
        id: value.get("id")?.as_str()?.to_string(),
        name: value.get("name")?.as_str()?.to_string(),
        web_view_link: value
            .get("webViewLink")
            .and_then(Value::as_str)
            .map(ToString::to_string),
    })
}

fn google_valid_access_token(token: &mut GoogleToken) -> Result<String> {
    if token.expires_at > now_ts() + 90 {
        return Ok(token.access_token.clone());
    }
    let refresh_token = token
        .refresh_token
        .as_deref()
        .ok_or_else(|| anyhow!("Google refresh token missing; reconnect Google Drive"))?;
    let cfg = load_oauth_config()?;
    let client_id = cfg
        .google_client_id
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow!("Google client id missing; save OAuth config first"))?
        .to_string();
    let mut pairs = vec![
        ("client_id", client_id.as_str()),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];
    if let Some(secret) = cfg
        .google_client_secret
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        pairs.push(("client_secret", secret));
    }
    let value = response_json(
        ureq::post("https://oauth2.googleapis.com/token")
            .set("Accept", "application/json")
            .set("User-Agent", "Kota")
            .set("Content-Type", "application/x-www-form-urlencoded")
            .send_string(&form_body(&pairs)),
    )?;
    token.access_token = json_string(&value, "access_token")?;
    token.expires_at = now_ts() + json_i64(&value, "expires_in").unwrap_or(3600);
    if let Some(scope) = value.get("scope").and_then(Value::as_str) {
        token.scope = Some(scope.to_string());
    }
    write_secret_json(GOOGLE_SECRET_ACCOUNT, token)?;
    Ok(token.access_token.clone())
}

fn google_userinfo(access_token: &str) -> Result<String> {
    let value = response_json(
        ureq::get("https://openidconnect.googleapis.com/v1/userinfo")
            .set("Accept", "application/json")
            .set("User-Agent", "Kota")
            .set("Authorization", &format!("Bearer {access_token}"))
            .call(),
    )?;
    json_string(&value, "email")
}

fn github_user(access_token: &str) -> Result<String> {
    let value = github_get_json(access_token, "https://api.github.com/user")?;
    json_string(&value, "login")
}

fn github_cli_token() -> Result<GithubToken> {
    let output = run_gh_auth_token()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            bail!("gh auth token exited with {}", output.status);
        }
        bail!("gh auth token exited with {}: {stderr}", output.status);
    }

    let access_token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if access_token.is_empty() {
        bail!("gh auth token returned an empty token");
    }

    let username = github_user(&access_token)?;
    let token = GithubToken {
        access_token,
        token_type: Some("gh-cli".into()),
        scope: None,
        username: Some(username),
    };
    Ok(token)
}

fn run_gh_auth_token() -> Result<std::process::Output> {
    let mut errors = Vec::new();
    for candidate in ["gh", "/opt/homebrew/bin/gh", "/usr/local/bin/gh"] {
        match Command::new(candidate)
            .args(["auth", "token", "--hostname", "github.com"])
            .output()
        {
            Ok(output) => return Ok(output),
            Err(err) => errors.push(format!("{candidate}: {err}")),
        }
    }
    bail!("gh executable not found ({})", errors.join("; "))
}

fn github_repo(access_token: &str, full_name: &str) -> Result<GithubRepo> {
    let safe = full_name.trim();
    if safe.split('/').count() != 2 {
        bail!("repo must be owner/name");
    }
    let value = github_get_json(
        access_token,
        &format!("https://api.github.com/repos/{safe}"),
    )?;
    parse_repo(&value)
}

fn github_get_json(access_token: &str, url: &str) -> Result<Value> {
    response_json(
        ureq::get(url)
            .set("Accept", "application/vnd.github+json")
            .set("X-GitHub-Api-Version", "2022-11-28")
            .set("User-Agent", "Kota")
            .set("Authorization", &format!("Bearer {access_token}"))
            .call(),
    )
}

fn github_post_json(access_token: &str, url: &str, body: Value) -> Result<Value> {
    response_json(
        ureq::post(url)
            .set("Accept", "application/vnd.github+json")
            .set("X-GitHub-Api-Version", "2022-11-28")
            .set("User-Agent", "Kota")
            .set("Authorization", &format!("Bearer {access_token}"))
            .send_json(body),
    )
}

fn parse_repo_list(value: &Value) -> Result<Vec<GithubRepo>> {
    value
        .as_array()
        .ok_or_else(|| anyhow!("GitHub repo list response was not an array"))?
        .iter()
        .map(parse_repo)
        .collect()
}

fn parse_repo(value: &Value) -> Result<GithubRepo> {
    let owner = value
        .get("owner")
        .and_then(|o| o.get("login"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("GitHub repo response missing owner.login"))?;
    Ok(GithubRepo {
        full_name: json_string(value, "full_name")?,
        name: json_string(value, "name")?,
        owner: owner.to_string(),
        private: value
            .get("private")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        default_branch: json_string(value, "default_branch").unwrap_or_else(|_| "main".into()),
        clone_url: json_string(value, "clone_url")?,
        html_url: json_string(value, "html_url")?,
    })
}

fn materialize_github_workspace(access_token: &str, repo: GithubRepo) -> Result<WorkspaceProject> {
    let cfg = load_oauth_config().unwrap_or_default();
    let project_id = safe_project_id(&repo.full_name);
    let root = workspace_project_root(&project_id);
    let source = local_project_root(&cfg).join(&repo.owner).join(&repo.name);
    let workspaces = root.join(".agent-workspaces");
    let shared = project_memory_dir(&root);
    let rules = project_rules_dir(&root);
    fs::create_dir_all(&root)?;
    fs::create_dir_all(&workspaces)?;
    ensure_project_memory_dirs(&shared)?;
    fs::create_dir_all(&rules)?;

    if !source.join(".git").exists() {
        if source.exists() && fs::read_dir(&source)?.next().is_some() {
            bail!(
                "source dir exists but is not a git clone: {}",
                source.display()
            );
        }
        if let Some(parent) = source.parent() {
            fs::create_dir_all(parent)?;
        }
        if source.exists() {
            fs::remove_dir_all(&source)?;
        }
        run_git_with_token(
            access_token,
            &root,
            &["clone", repo.clone_url.as_str(), path_str(&source).as_str()],
        )?;
    } else {
        run_git_with_token(
            access_token,
            &source,
            &["remote", "set-url", "origin", repo.clone_url.as_str()],
        )?;
        run_git_with_token(access_token, &source, &["fetch", "--prune", "origin"])?;
    }

    let base_ref = format!("origin/{}", repo.default_branch);
    let workspace = WorkspaceProject {
        project_id,
        repo_full_name: repo.full_name,
        remote_url: repo.clone_url,
        github_html_url: repo.html_url,
        default_branch: repo.default_branch,
        base_ref,
        local_root: path_str(&root),
        local_root_bytes: 0,
        source_dir: path_str(&source),
        source_dir_bytes: 0,
        shared_dir: path_str(&shared),
        rules_dir: path_str(&rules),
        agents: Vec::new(),
        archived: false,
        archived_at: None,
    };

    save_workspace_files(&workspace)?;
    Ok(workspace)
}

fn materialize_workspace_agent(
    workspace: &WorkspaceProject,
    agent_id: &str,
    cli: crate::pty::agent::AgentCli,
) -> Result<AgentLaunchSpec> {
    materialize_workspace_agent_with_auth(workspace, agent_id, cli, None)
}

fn materialize_workspace_agent_with_auth(
    workspace: &WorkspaceProject,
    agent_id: &str,
    cli: crate::pty::agent::AgentCli,
    access_token: Option<&str>,
) -> Result<AgentLaunchSpec> {
    let root = PathBuf::from(&workspace.local_root);
    let source = PathBuf::from(&workspace.source_dir);
    let workspaces = root.join(".agent-workspaces");
    let cwd = workspaces.join(agent_id);
    let worktree_root = cwd.join("project-files");
    fs::create_dir_all(&workspaces)?;

    if !source.join(".git").exists() {
        bail!(
            "source dir is not a git clone for workspace agent launch: {}",
            source.display()
        );
    }

    if let Some(token) = access_token {
        run_git_with_token(token, &source, &["fetch", "--prune", "origin"])?;
    }
    ensure_project_files_worktree(
        &source,
        &cwd,
        &worktree_root,
        agent_id,
        workspace.base_ref.as_str(),
    )?;
    ensure_workspace_projections(
        &cwd,
        &PathBuf::from(&workspace.shared_dir),
        &PathBuf::from(&workspace.rules_dir),
        cli,
    )?;

    let adapter_path = ensure_agent_files(&cwd, &worktree_root, agent_id, cli)?;
    Ok(AgentLaunchSpec {
        agent_id: agent_id.to_string(),
        cli: cli.into(),
        cwd: path_str(&cwd),
        project_root: workspace.local_root.clone(),
        worktree_root: path_str(&worktree_root),
        shared_dir: workspace.shared_dir.clone(),
        rules_dir: workspace.rules_dir.clone(),
        adapter_path,
        project_id: workspace.project_id.clone(),
        project_remote: workspace.remote_url.clone(),
        project_base_ref: workspace.base_ref.clone(),
    })
}

fn ensure_project_files_worktree(
    source: &Path,
    cwd: &Path,
    worktree_root: &Path,
    agent_id: &str,
    base_ref: &str,
) -> Result<()> {
    if worktree_root.join(".git").exists() {
        return Ok(());
    }

    if cwd.join(".git").exists() {
        migrate_legacy_root_worktree(source, cwd, worktree_root, agent_id)?;
        return Ok(());
    }

    if worktree_root.exists() && fs::read_dir(worktree_root)?.next().is_some() {
        bail!(
            "agent project-files exists but is not a git worktree: {}",
            worktree_root.display()
        );
    }
    if worktree_root.exists() {
        fs::remove_dir_all(worktree_root)?;
    }
    if let Some(parent) = worktree_root.parent() {
        fs::create_dir_all(parent)?;
    }
    add_agent_worktree(source, worktree_root, agent_id, base_ref)
}

fn migrate_legacy_root_worktree(
    source: &Path,
    cwd: &Path,
    worktree_root: &Path,
    agent_id: &str,
) -> Result<()> {
    let parent = cwd
        .parent()
        .ok_or_else(|| anyhow!("agent workspace has no parent: {}", cwd.display()))?;
    let suffix = Uuid::new_v4().simple().to_string();
    let temp_worktree = parent.join(format!(".{agent_id}-project-files-migrating-{suffix}"));
    let runtime_backup = parent.join(format!(".{agent_id}-runtime-backup-{suffix}"));
    backup_runtime_files(cwd, &runtime_backup)?;

    run_git_plain(
        source,
        &[
            "worktree",
            "move",
            path_str(cwd).as_str(),
            path_str(&temp_worktree).as_str(),
        ],
    )?;
    fs::create_dir_all(cwd)?;
    run_git_plain(
        source,
        &[
            "worktree",
            "move",
            path_str(&temp_worktree).as_str(),
            path_str(worktree_root).as_str(),
        ],
    )?;
    restore_runtime_files(&runtime_backup, cwd)?;
    cleanup_runtime_files_from_project_tree(worktree_root)?;
    let _ = fs::remove_dir_all(&runtime_backup);
    Ok(())
}

fn backup_runtime_files(source: &Path, backup: &Path) -> Result<()> {
    fs::create_dir_all(backup)?;
    for name in [
        "agent.yaml",
        "SHELL.yaml",
        "AGENTS.md",
        "CLAUDE.md",
        "opencode.json",
    ] {
        let path = source.join(name);
        if path.is_file()
            && (!is_adapter_filename(name) || file_contains_kota_adapter_marker(&path))
        {
            fs::copy(&path, backup.join(name))
                .with_context(|| format!("backup {}", path.display()))?;
        }
    }
    Ok(())
}

fn restore_runtime_files(backup: &Path, target: &Path) -> Result<()> {
    if !backup.exists() {
        return Ok(());
    }
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(backup)? {
        let entry = entry?;
        let source = entry.path();
        if source.is_file() {
            fs::copy(&source, target.join(entry.file_name()))
                .with_context(|| format!("restore {}", source.display()))?;
        }
    }
    Ok(())
}

fn cleanup_runtime_files_from_project_tree(worktree_root: &Path) -> Result<()> {
    for name in ["agent.yaml", "SHELL.yaml", "opencode.json"] {
        remove_path_if_exists(&worktree_root.join(name))?;
    }
    for name in ["AGENTS.md", "CLAUDE.md"] {
        let path = worktree_root.join(name);
        if path.is_file() && file_contains_kota_adapter_marker(&path) {
            remove_path_if_exists(&path)?;
        }
    }
    for name in [
        ".agents",
        ".kota",
        "shared",
        "project-memory",
        "project-rules",
    ] {
        remove_path_if_exists(&worktree_root.join(name))?;
    }
    Ok(())
}

fn is_adapter_filename(name: &str) -> bool {
    matches!(name, "AGENTS.md" | "CLAUDE.md")
}

fn file_contains_kota_adapter_marker(path: &Path) -> bool {
    fs::read_to_string(path)
        .map(|text| text.contains("Kota agent adapter") || text.contains("kota:adapter"))
        .unwrap_or(false)
}

fn add_agent_worktree(
    source: &Path,
    worktree_root: &Path,
    agent_id: &str,
    base_ref: &str,
) -> Result<()> {
    let branch = format!("kota/{agent_id}");
    let branch_ref = format!("refs/heads/{branch}");
    let worktree_path = path_str(worktree_root);
    if git_plain_success(
        source,
        &["show-ref", "--verify", "--quiet", branch_ref.as_str()],
    ) {
        run_git_plain(
            source,
            &["worktree", "add", worktree_path.as_str(), branch.as_str()],
        )
    } else {
        run_git_plain(
            source,
            &[
                "worktree",
                "add",
                "-b",
                branch.as_str(),
                worktree_path.as_str(),
                base_ref,
            ],
        )
    }
}

fn ensure_workspace_projections(
    cwd: &Path,
    shared_dir: &Path,
    rules_dir: &Path,
    cli: crate::pty::agent::AgentCli,
) -> Result<()> {
    fs::create_dir_all(cwd)?;
    fs::create_dir_all(shared_dir)?;
    fs::create_dir_all(rules_dir)?;
    replace_symlink(rules_dir, &cwd.join("project-rules"))?;
    replace_symlink(shared_dir, &cwd.join("project-memory"))?;
    crate::bbs::ensure_project_projection(shared_dir)?;
    remove_matching_symlink(&cwd.join("shared"), shared_dir)?;
    remove_symlink_if_exists(&cwd.join(".kota").join("rules"))?;
    remove_symlink_if_exists(&cwd.join(".kota").join("memory"))?;
    migrate_missing_skills_file(cwd)?;
    remove_empty_dir(&cwd.join(".kota"))?;
    ensure_skill_projections(cwd, cli)?;
    Ok(())
}

fn ensure_skill_projections(cwd: &Path, cli: crate::pty::agent::AgentCli) -> Result<()> {
    let account_skills = kota_home().join("skills");
    fs::create_dir_all(&account_skills)?;
    let active_skills = skill_projection_dir_for_cli(cwd, cli);
    let inactive_skills = inactive_skill_projection_dir_for_cli(cwd, cli);
    remove_skill_projection_dir_symlink(&active_skills, &account_skills, &inactive_skills)?;
    fs::create_dir_all(&active_skills)?;
    for entry in fs::read_dir(&account_skills)? {
        let entry = entry?;
        let source = entry.path();
        if !source.is_dir() {
            continue;
        }
        replace_symlink(&source, &active_skills.join(entry.file_name()))?;
    }
    prune_inactive_skill_projection(cwd, &inactive_skills, &account_skills, &active_skills)?;
    Ok(())
}

fn skill_projection_dir_for_cli(cwd: &Path, cli: crate::pty::agent::AgentCli) -> PathBuf {
    if cli == crate::pty::agent::AgentCli::Claude {
        cwd.join(".claude").join("skills")
    } else {
        cwd.join(".agents").join("skills")
    }
}

fn inactive_skill_projection_dir_for_cli(cwd: &Path, cli: crate::pty::agent::AgentCli) -> PathBuf {
    if cli == crate::pty::agent::AgentCli::Claude {
        cwd.join(".agents").join("skills")
    } else {
        cwd.join(".claude").join("skills")
    }
}

fn save_workspace_files(workspace: &WorkspaceProject) -> Result<()> {
    let root = PathBuf::from(&workspace.local_root);
    fs::create_dir_all(&root)?;
    fs::write(
        root.join("workspace.json"),
        serde_json::to_string_pretty(workspace)?,
    )?;
    let meta = WorkspaceProjectMeta {
        version: 1,
        project_id: workspace.project_id.clone(),
        repo_full_name: workspace.repo_full_name.clone(),
        remote_url: workspace.remote_url.clone(),
        github_html_url: workspace.github_html_url.clone(),
        default_branch: workspace.default_branch.clone(),
        base_ref: workspace.base_ref.clone(),
    };
    fs::write(root.join("meta.yaml"), serde_yaml::to_string(&meta)?)?;
    let local = WorkspaceLocalState {
        version: 1,
        local_root: workspace.local_root.clone(),
        source_dir: workspace.source_dir.clone(),
        shared_dir: workspace.shared_dir.clone(),
        rules_dir: workspace.rules_dir.clone(),
        updated_at: now_ts(),
    };
    fs::write(
        root.join("local-state.json"),
        serde_json::to_string_pretty(&local)?,
    )?;
    write_account_ignore_files(&root)?;
    Ok(())
}

fn write_account_ignore_files(root: &Path) -> Result<()> {
    let ignore = [
        "project-memory/dreams.md",
        "project-memory/hot_memory.md",
        ".agent-workspaces/*/.claude/rules",
        ".agent-workspaces/*/.claude/skills",
        ".agent-workspaces/*/.agents/skills",
        ".agent-workspaces/*/project-rules",
        ".agent-workspaces/*/project-memory",
        ".agent-workspaces/*/project-files",
        ".agent-workspaces/*/local",
        "",
    ]
    .join("\n");
    fs::write(root.join(".gitignore"), &ignore)?;
    fs::write(root.join(".kotaignore"), ignore)?;
    Ok(())
}

fn ensure_agent_files(
    cwd: &Path,
    worktree_root: &Path,
    agent_id: &str,
    cli: crate::pty::agent::AgentCli,
) -> Result<String> {
    let adapter_name = adapter_file_for_cli(cli);
    let adapter = cwd.join(adapter_name);
    if !adapter.exists() {
        fs::write(
            &adapter,
            format!(
                "# Kota agent adapter\n\nAgent id: `{agent_id}`.\n\nThe agent runtime CWD is this directory. Edit project files under `project-files/` (`KOTA_WORKTREE_ROOT`). Project memory is available through `project-memory/` (`KOTA_PROJECT_MEMORY_DIR`); project rules through `project-rules/` (`KOTA_PROJECT_RULES_DIR`).\n"
            ),
        )?;
    }
    let yaml = cwd.join("agent.yaml");
    if !yaml.exists() {
        fs::write(
            &yaml,
            format!("id: {agent_id}\nrecruited-from: github\nstatus: active\n"),
        )?;
    }
    let shell_yaml = cwd.join("SHELL.yaml");
    if !shell_yaml.exists() {
        fs::write(
            &shell_yaml,
            format!(
                "provider: {}\nagent-cwd: {}\nproject-files: {}\nskills: []\n",
                match cli {
                    crate::pty::agent::AgentCli::Claude => "claude",
                    crate::pty::agent::AgentCli::Codex => "codex",
                    crate::pty::agent::AgentCli::Antigravity => "antigravity",
                    crate::pty::agent::AgentCli::Opencode => "opencode",
                    crate::pty::agent::AgentCli::Pi => "pi",
                    crate::pty::agent::AgentCli::Kimi => "kimi",
                },
                yaml_quote(&path_str(cwd)),
                yaml_quote(&path_str(worktree_root)),
            ),
        )?;
    }
    Ok(path_str(&adapter))
}

fn adapter_file_for_cli(cli: crate::pty::agent::AgentCli) -> &'static str {
    match cli {
        crate::pty::agent::AgentCli::Claude => "CLAUDE.md",
        crate::pty::agent::AgentCli::Antigravity
        | crate::pty::agent::AgentCli::Codex
        | crate::pty::agent::AgentCli::Opencode
        | crate::pty::agent::AgentCli::Pi
        | crate::pty::agent::AgentCli::Kimi => "AGENTS.md",
    }
}

fn run_git_with_token(access_token: &str, cwd: &Path, args: &[&str]) -> Result<()> {
    let helper =
        std::env::temp_dir().join(format!("kota-git-askpass-{}.sh", Uuid::new_v4().simple()));
    fs::write(
        &helper,
        "#!/bin/sh\ncase \"$1\" in\n  *Username*) printf '%s\\n' x-access-token ;;\n  *) printf '%s' \"$KOTA_GIT_TOKEN\" ;;\nesac\n",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&helper)?.permissions();
        perms.set_mode(0o700);
        fs::set_permissions(&helper, perms)?;
    }

    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", &helper)
        .env("KOTA_GIT_TOKEN", access_token)
        .output()
        .with_context(|| format!("spawn git {}", args.join(" ")));
    let _ = fs::remove_file(&helper);
    let output = output?;
    if output.status.success() {
        return Ok(());
    }
    bail!(
        "git {} failed: {}{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn run_git_plain(cwd: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .with_context(|| format!("spawn git {}", args.join(" ")))?;
    if output.status.success() {
        return Ok(());
    }
    bail!(
        "git {} failed: {}{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn git_plain_success(cwd: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .current_dir(cwd)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn replace_symlink<T: AsRef<Path>>(target: T, link: &Path) -> Result<()> {
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent)?;
    }
    remove_path_if_exists(link)?;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target.as_ref(), link).with_context(|| {
            format!(
                "symlink {} -> {}",
                link.display(),
                target.as_ref().display()
            )
        })?;
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(target.as_ref(), link).with_context(|| {
            format!(
                "symlink {} -> {}",
                link.display(),
                target.as_ref().display()
            )
        })?;
    }
    Ok(())
}

fn prune_inactive_skill_projection(
    cwd: &Path,
    dir: &Path,
    account_skills: &Path,
    active_dir: &Path,
) -> Result<()> {
    let Ok(meta) = fs::symlink_metadata(dir) else {
        return Ok(());
    };
    if meta.file_type().is_symlink() {
        if is_kota_skill_projection_link(dir, account_skills, active_dir)? {
            fs::remove_file(dir).with_context(|| format!("remove {}", dir.display()))?;
        }
        return Ok(());
    }

    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if is_kota_skill_projection_link(&path, account_skills, active_dir)? {
            fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        }
    }
    remove_empty_dir(dir)?;
    if let Some(parent) = dir.parent() {
        if parent != cwd {
            remove_empty_dir(parent)?;
        }
    }
    Ok(())
}

fn is_kota_skill_projection_link(
    path: &Path,
    account_skills: &Path,
    active_dir: &Path,
) -> Result<bool> {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return Ok(false);
    };
    if !meta.file_type().is_symlink() {
        return Ok(false);
    }
    let target = fs::read_link(path).with_context(|| format!("readlink {}", path.display()))?;
    if target.is_absolute()
        && (target.starts_with(account_skills) || target.starts_with(active_dir))
    {
        return Ok(true);
    }
    let target_text = target.to_string_lossy().replace('\\', "/");
    Ok(target_text == "../.agents/skills"
        || target_text.starts_with("../.agents/skills/")
        || target_text == "../../.agents/skills"
        || target_text.starts_with("../../.agents/skills/"))
}

fn remove_skill_projection_dir_symlink(
    dir: &Path,
    account_skills: &Path,
    other_dir: &Path,
) -> Result<()> {
    let Ok(meta) = fs::symlink_metadata(dir) else {
        return Ok(());
    };
    if meta.file_type().is_symlink()
        && is_kota_skill_projection_link(dir, account_skills, other_dir)?
    {
        fs::remove_file(dir).with_context(|| format!("remove {}", dir.display()))?;
    }
    Ok(())
}

fn remove_matching_symlink(link: &Path, target: &Path) -> Result<()> {
    let Ok(meta) = fs::symlink_metadata(link) else {
        return Ok(());
    };
    if !meta.file_type().is_symlink() {
        return Ok(());
    }
    let current = fs::read_link(link).with_context(|| format!("readlink {}", link.display()))?;
    if current == target {
        fs::remove_file(link).with_context(|| format!("remove {}", link.display()))?;
    }
    Ok(())
}

fn remove_symlink_if_exists(link: &Path) -> Result<()> {
    let Ok(meta) = fs::symlink_metadata(link) else {
        return Ok(());
    };
    if meta.file_type().is_symlink() {
        fs::remove_file(link).with_context(|| format!("remove {}", link.display()))?;
    }
    Ok(())
}

fn migrate_missing_skills_file(cwd: &Path) -> Result<()> {
    let legacy = cwd.join(".kota").join("missing-skills.txt");
    if !legacy.is_file() {
        return Ok(());
    }
    let next = cwd.join("missing-skills.txt");
    if next.exists() {
        fs::remove_file(&legacy).with_context(|| format!("remove {}", legacy.display()))?;
    } else {
        fs::rename(&legacy, &next)
            .with_context(|| format!("move {} -> {}", legacy.display(), next.display()))?;
    }
    Ok(())
}

fn remove_empty_dir(path: &Path) -> Result<()> {
    match fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::DirectoryNotEmpty => Ok(()),
        Err(err) => Err(err).with_context(|| format!("remove empty dir {}", path.display())),
    }
}

fn remove_path_if_exists(path: &Path) -> Result<()> {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    if meta.is_dir() && !meta.file_type().is_symlink() {
        fs::remove_dir_all(path).with_context(|| format!("remove {}", path.display()))?;
    } else {
        fs::remove_file(path).with_context(|| format!("remove {}", path.display()))?;
    }
    Ok(())
}

fn wait_for_loopback_query(
    listener: &TcpListener,
    timeout: Duration,
) -> Result<HashMap<String, String>> {
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let mut buf = [0_u8; 4096];
                let n = stream.read(&mut buf).context("read OAuth callback")?;
                let request = String::from_utf8_lossy(&buf[..n]);
                let first = request.lines().next().unwrap_or_default();
                let query = parse_callback_query(first)?;
                let body = "<!doctype html><title>Kota</title><body style=\"font-family:-apple-system,BlinkMacSystemFont,sans-serif;padding:32px\">Kota authorization complete. You can close this tab.</body>";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
                return Ok(query);
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() > deadline {
                    bail!("OAuth callback timed out");
                }
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(err) => return Err(err).context("accept OAuth callback"),
        }
    }
}

fn parse_callback_query(request_line: &str) -> Result<HashMap<String, String>> {
    let path = request_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| anyhow!("malformed OAuth callback request"))?;
    let query = path.split_once('?').map(|(_, q)| q).unwrap_or_default();
    let mut result = HashMap::new();
    for pair in query.split('&').filter(|s| !s.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        result.insert(url_decode(key)?, url_decode(value)?);
    }
    Ok(result)
}

fn form_body(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn url_decode(value: &str) -> Result<String> {
    Ok(urlencoding::decode(value)
        .context("decode url component")?
        .into_owned())
}

fn open_url(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut cmd = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = Command::new("xdg-open");

    cmd.arg(url);
    let status = cmd.status().context("open system browser")?;
    if status.success() {
        Ok(())
    } else {
        bail!("open system browser failed with {status}")
    }
}

fn load_oauth_config() -> Result<OAuthConfig> {
    let mut cfg = if oauth_config_path().is_file() {
        serde_json::from_slice(&fs::read(oauth_config_path())?)?
    } else {
        OAuthConfig::default()
    };
    if let Ok(v) = std::env::var("KOTA_GOOGLE_CLIENT_ID") {
        if !v.trim().is_empty() {
            cfg.google_client_id = Some(v);
        }
    }
    if let Ok(v) = std::env::var("KOTA_GOOGLE_CLIENT_SECRET") {
        if !v.trim().is_empty() {
            cfg.google_client_secret = Some(v);
        }
    }
    if let Ok(v) = std::env::var("KOTA_GITHUB_CLIENT_ID") {
        if !v.trim().is_empty() {
            cfg.github_client_id = Some(v);
        }
    }
    if cfg
        .github_client_id
        .as_deref()
        .map_or(true, |s| s.trim().is_empty())
    {
        cfg.github_client_id = option_env!("KOTA_GITHUB_CLIENT_ID")
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().to_string());
    }
    Ok(cfg)
}

fn save_oauth_config_file(cfg: &OAuthConfig) -> Result<()> {
    let path = oauth_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(cfg)?)?;
    Ok(())
}

fn oauth_config_path() -> PathBuf {
    kota_home().join("oauth-config.json")
}

fn google_drive_path(cfg: &OAuthConfig) -> String {
    cfg.google_drive_path
        .as_deref()
        .map(normalize_drive_path)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Kota Sync".into())
}

fn local_project_root(cfg: &OAuthConfig) -> PathBuf {
    cfg.local_project_root
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(|s| PathBuf::from(expand_home(s.trim())))
        .filter(|path| !is_legacy_default_project_root(path))
        .unwrap_or_else(default_local_project_root)
}

fn default_local_project_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Kota")
        .join("Projects")
}

fn is_legacy_default_project_root(path: &Path) -> bool {
    path == legacy_default_project_root()
}

fn normalize_drive_path(raw: &str) -> String {
    raw.split('/')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

fn expand_home(raw: &str) -> String {
    if raw == "~" {
        return dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("~"))
            .display()
            .to_string();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("~"))
            .join(rest)
            .display()
            .to_string();
    }
    raw.to_string()
}

impl StorageMeasurementController {
    fn status(&self) -> StorageMeasurementStatus {
        let runtime = storage_measurement_runtime(&self.runtime);
        storage_measurement_status_from_runtime(&runtime)
    }

    fn start(&self) -> StorageMeasurementStatus {
        let job_id = {
            let mut runtime = storage_measurement_runtime(&self.runtime);
            if runtime.shutting_down {
                runtime.error = Some("Kota is shutting down".into());
                return storage_measurement_status_from_runtime(&runtime);
            }
            if runtime.updating {
                return storage_measurement_status_from_runtime(&runtime);
            }
            runtime.updating = true;
            runtime.error = None;
            runtime.active_job_id = runtime.active_job_id.wrapping_add(1).max(1);
            runtime.active_job_id
        };

        let child = match storage_measurement_command(&self.account_root).spawn() {
            Ok(child) => child,
            Err(err) => {
                finish_storage_measurement_error(
                    &self.runtime,
                    job_id,
                    format!("Could not start storage scan: {err}"),
                );
                return self.status();
            }
        };

        let mut child = Some(child);
        let accepted = {
            let mut runtime = storage_measurement_runtime(&self.runtime);
            if runtime.shutting_down
                || !runtime.updating
                || runtime.active_job_id != job_id
                || runtime.child.is_some()
            {
                false
            } else {
                runtime.child = child.take();
                true
            }
        };
        if !accepted {
            if let Some(child) = child.as_mut() {
                let _ = child.kill();
                let _ = child.wait();
            }
            return self.status();
        }

        let runtime = Arc::clone(&self.runtime);
        let account_root = self.account_root.clone();
        let cache_path = self.cache_path.clone();
        if let Err(err) = std::thread::Builder::new()
            .name("kota-storage-measurement".into())
            .spawn(move || {
                run_storage_measurement_worker(runtime, account_root, cache_path, job_id)
            })
        {
            let mut child = {
                let mut state = storage_measurement_runtime(&self.runtime);
                if state.active_job_id == job_id {
                    state.child.take()
                } else {
                    None
                }
            };
            if let Some(child) = child.as_mut() {
                let _ = child.kill();
                let _ = child.wait();
            }
            finish_storage_measurement_error(
                &self.runtime,
                job_id,
                format!("Could not start storage scan worker: {err}"),
            );
        }
        self.status()
    }

    fn shutdown(&self) {
        let mut child = {
            let mut runtime = storage_measurement_runtime(&self.runtime);
            runtime.shutting_down = true;
            runtime.updating = false;
            runtime.active_job_id = runtime.active_job_id.wrapping_add(1);
            runtime.child.take()
        };
        if let Some(child) = child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn storage_measurement_runtime(
    runtime: &Arc<Mutex<StorageMeasurementRuntime>>,
) -> std::sync::MutexGuard<'_, StorageMeasurementRuntime> {
    runtime
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn storage_measurement_status_from_runtime(
    runtime: &StorageMeasurementRuntime,
) -> StorageMeasurementStatus {
    StorageMeasurementStatus {
        updating: runtime.updating,
        on_disk_bytes: runtime
            .last_success
            .as_ref()
            .map(|record| record.on_disk_bytes),
        available_bytes: runtime
            .last_success
            .as_ref()
            .map(|record| record.available_bytes),
        measured_at: runtime
            .last_success
            .as_ref()
            .map(|record| record.measured_at),
        error: runtime.error.clone(),
    }
}

fn storage_measurement_command(account_root: &Path) -> Command {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("/usr/sbin/taskpolicy");
        command.args([
            "-c",
            "background",
            "-b",
            "/usr/bin/nice",
            "-n",
            "10",
            "/usr/bin/du",
            "-skx",
        ]);
        command
    };
    #[cfg(not(target_os = "macos"))]
    let mut command = {
        let mut command = Command::new("du");
        command.args(["-skx"]);
        command
    };
    command
        .arg(account_root)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
}

enum StorageMeasurementPoll {
    Pending,
    Exited(Child),
    Failed(Child, String),
    TimedOut(Child),
    Cancelled,
}

fn run_storage_measurement_worker(
    runtime: Arc<Mutex<StorageMeasurementRuntime>>,
    account_root: PathBuf,
    cache_path: PathBuf,
    job_id: u64,
) {
    let started_at = Instant::now();
    loop {
        let poll = {
            let mut state = storage_measurement_runtime(&runtime);
            if state.shutting_down || !state.updating || state.active_job_id != job_id {
                StorageMeasurementPoll::Cancelled
            } else if started_at.elapsed() >= STORAGE_MEASUREMENT_TIMEOUT {
                match state.child.take() {
                    Some(child) => StorageMeasurementPoll::TimedOut(child),
                    None => StorageMeasurementPoll::Cancelled,
                }
            } else {
                match state.child.as_mut() {
                    Some(child) => match child.try_wait() {
                        Ok(Some(_)) => StorageMeasurementPoll::Exited(
                            state.child.take().expect("storage scan child disappeared"),
                        ),
                        Ok(None) => StorageMeasurementPoll::Pending,
                        Err(err) => StorageMeasurementPoll::Failed(
                            state.child.take().expect("storage scan child disappeared"),
                            err.to_string(),
                        ),
                    },
                    None => StorageMeasurementPoll::Cancelled,
                }
            }
        };

        match poll {
            StorageMeasurementPoll::Pending => {
                std::thread::sleep(STORAGE_MEASUREMENT_POLL_INTERVAL);
            }
            StorageMeasurementPoll::Cancelled => return,
            StorageMeasurementPoll::TimedOut(mut child) => {
                let _ = child.kill();
                let _ = child.wait();
                finish_storage_measurement_error(
                    &runtime,
                    job_id,
                    "Storage scan timed out after 10 minutes".into(),
                );
                return;
            }
            StorageMeasurementPoll::Failed(mut child, err) => {
                let _ = child.kill();
                let _ = child.wait();
                finish_storage_measurement_error(
                    &runtime,
                    job_id,
                    format!("Storage scan failed: {err}"),
                );
                return;
            }
            StorageMeasurementPoll::Exited(child) => {
                let result = finish_storage_measurement(child, &account_root, &cache_path);
                match result {
                    Ok(record) => {
                        let mut state = storage_measurement_runtime(&runtime);
                        if !state.shutting_down && state.updating && state.active_job_id == job_id {
                            state.last_success = Some(record);
                            state.updating = false;
                            state.error = None;
                        }
                    }
                    Err(err) => {
                        finish_storage_measurement_error(
                            &runtime,
                            job_id,
                            format!("Storage scan failed: {err}"),
                        );
                    }
                }
                return;
            }
        }
    }
}

fn finish_storage_measurement(
    child: Child,
    account_root: &Path,
    cache_path: &Path,
) -> Result<StorageMeasurementRecord> {
    let output = child.wait_with_output().context("wait for storage scan")?;
    if !output.status.success() {
        bail!("du exited with {}", output.status);
    }
    let on_disk_bytes = parse_du_kib_output(&output.stdout)?;
    let available_bytes = available_space_bytes(account_root)?;
    let record = StorageMeasurementRecord {
        version: STORAGE_MEASUREMENT_CACHE_VERSION,
        on_disk_bytes,
        available_bytes,
        measured_at: now_ts(),
        app_version: env!("CARGO_PKG_VERSION").into(),
    };
    save_storage_measurement_record(cache_path, &record)?;
    Ok(record)
}

fn finish_storage_measurement_error(
    runtime: &Arc<Mutex<StorageMeasurementRuntime>>,
    job_id: u64,
    error: String,
) {
    let mut state = storage_measurement_runtime(runtime);
    if !state.shutting_down && state.active_job_id == job_id {
        state.child = None;
        state.updating = false;
        state.error = Some(error);
    }
}

fn parse_du_kib_output(stdout: &[u8]) -> Result<u64> {
    let stdout = std::str::from_utf8(stdout).context("decode du output")?;
    let line = stdout
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .ok_or_else(|| anyhow!("du returned no size"))?;
    let kib = line
        .split_whitespace()
        .next()
        .ok_or_else(|| anyhow!("du returned no size"))?
        .parse::<u64>()
        .context("parse du size")?;
    Ok(kib.saturating_mul(1024))
}

#[cfg(unix)]
fn available_space_bytes(path: &Path) -> Result<u64> {
    use std::ffi::CString;
    use std::mem::MaybeUninit;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(path.as_os_str().as_bytes()).context("storage path contains NUL")?;
    let mut stats = MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error()).context("read available disk space");
    }
    let stats = unsafe { stats.assume_init() };
    let block_size = if stats.f_frsize > 0 {
        stats.f_frsize as u64
    } else {
        stats.f_bsize as u64
    };
    Ok((stats.f_bavail as u64).saturating_mul(block_size))
}

#[cfg(not(unix))]
fn available_space_bytes(_path: &Path) -> Result<u64> {
    bail!("storage measurement is not supported on this platform")
}

fn load_storage_measurement_record(path: &Path) -> Result<Option<StorageMeasurementRecord>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("read {}", path.display())),
    };
    let record: StorageMeasurementRecord =
        serde_json::from_slice(&bytes).with_context(|| format!("decode {}", path.display()))?;
    if record.version != STORAGE_MEASUREMENT_CACHE_VERSION {
        bail!(
            "unsupported storage measurement cache version {}",
            record.version
        );
    }
    Ok(Some(record))
}

fn save_storage_measurement_record(path: &Path, record: &StorageMeasurementRecord) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(record)?)
        .with_context(|| format!("write {}", tmp.display()))?;
    if let Err(err) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(err).with_context(|| format!("rename {}", path.display()));
    }
    Ok(())
}

fn active_workspace_path() -> PathBuf {
    kota_home().join("active-workspace.json")
}

fn workspace_project_root(project_id: &str) -> PathBuf {
    kota_workspaces_dir().join(project_id.trim())
}

fn save_active_workspace(workspace: &WorkspaceProject) -> Result<()> {
    let path = active_workspace_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(workspace)?)?;
    Ok(())
}

fn clear_active_workspace() -> Result<()> {
    let path = active_workspace_path();
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn load_active_workspace() -> Result<WorkspaceProject> {
    let mut workspace: WorkspaceProject =
        serde_json::from_slice(&fs::read(active_workspace_path())?)?;
    normalize_workspace_metadata(&mut workspace);
    let root = workspace_project_root(&workspace.project_id);
    if rebase_workspace_project_root(&mut workspace, &root) {
        save_active_workspace(&workspace)?;
    }
    Ok(workspace)
}

fn load_workspace_at(root: &Path) -> Result<WorkspaceProject> {
    let mut workspace: WorkspaceProject =
        serde_json::from_slice(&fs::read(root.join("workspace.json"))?)?;
    normalize_workspace_metadata(&mut workspace);
    if rebase_workspace_project_root(&mut workspace, root) {
        save_workspace_files(&workspace)?;
    }
    Ok(workspace)
}

fn normalize_workspace_metadata(workspace: &mut WorkspaceProject) -> bool {
    let mut changed = false;
    if workspace.github_html_url.trim().is_empty()
        && workspace.repo_full_name.split('/').count() == 2
    {
        workspace.github_html_url = format!("https://github.com/{}", workspace.repo_full_name);
        changed = true;
    }
    for spec in &mut workspace.agents {
        if spec.worktree_root == spec.cwd {
            spec.worktree_root = path_str(&PathBuf::from(&spec.cwd).join("project-files"));
            changed = true;
        }
    }
    changed
}

fn rebase_workspace_project_root(workspace: &mut WorkspaceProject, root: &Path) -> bool {
    let old_root = PathBuf::from(&workspace.local_root);
    if paths_same(&old_root, root) {
        return false;
    }
    let next_root = path_str(root);
    workspace.local_root = next_root.clone();
    workspace.shared_dir = path_str(&project_memory_dir(root));
    workspace.rules_dir = path_str(&project_rules_dir(root));
    for spec in &mut workspace.agents {
        rebase_agent_path(&mut spec.cwd, &old_root, root);
        spec.project_root = next_root.clone();
        rebase_agent_path(&mut spec.worktree_root, &old_root, root);
        spec.shared_dir = workspace.shared_dir.clone();
        spec.rules_dir = workspace.rules_dir.clone();
        rebase_agent_path(&mut spec.adapter_path, &old_root, root);
    }
    true
}

fn rebase_agent_path(value: &mut String, old_root: &Path, new_root: &Path) -> bool {
    let path = PathBuf::from(value.as_str());
    let Ok(relative) = path.strip_prefix(old_root) else {
        return false;
    };
    *value = path_str(&new_root.join(relative));
    true
}

fn paths_same(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    left == right
}

fn migrate_workspace_storage_layout(workspace: &mut WorkspaceProject) -> Result<bool> {
    let root = PathBuf::from(&workspace.local_root);
    let next_memory = project_memory_dir(&root);
    let next_rules = project_rules_dir(&root);
    let mut changed = false;

    changed |= move_legacy_dir_if_needed(&legacy_project_memory_dir(&root), &next_memory)?;
    changed |= move_legacy_dir_if_needed(&legacy_project_rules_dir(&root), &next_rules)?;
    ensure_project_memory_dirs(&next_memory)?;
    fs::create_dir_all(&next_rules)?;
    remove_empty_dir(&root.join(".kota"))?;

    let next_memory_text = path_str(&next_memory);
    let next_rules_text = path_str(&next_rules);
    if workspace.shared_dir != next_memory_text {
        workspace.shared_dir = next_memory_text.clone();
        changed = true;
    }
    if workspace.rules_dir != next_rules_text {
        workspace.rules_dir = next_rules_text.clone();
        changed = true;
    }

    for spec in &mut workspace.agents {
        if spec.shared_dir != next_memory_text {
            spec.shared_dir = next_memory_text.clone();
            changed = true;
        }
        if spec.rules_dir != next_rules_text {
            spec.rules_dir = next_rules_text.clone();
            changed = true;
        }
    }

    let agents_root = root.join(".agent-workspaces");
    if agents_root.is_dir() {
        for entry in fs::read_dir(&agents_root)? {
            let entry = entry?;
            let cwd = entry.path();
            if !cwd.is_dir() || !cwd.join("agent.yaml").is_file() {
                continue;
            }
            if agent_projection_needs_update(&cwd, &next_memory, &next_rules)? {
                let fallback_cli =
                    cwd.file_name()
                        .and_then(|name| name.to_str())
                        .and_then(|agent_id| {
                            workspace
                                .agents
                                .iter()
                                .find(|spec| spec.agent_id == agent_id)
                                .map(|spec| &spec.cli)
                        });
                if let Some(cli) = agent_workspace_cli(&cwd, fallback_cli).supported() {
                    ensure_workspace_projections(&cwd, &next_memory, &next_rules, cli)?;
                    changed = true;
                }
            }
        }
    }

    Ok(changed)
}

fn agent_projection_needs_update(cwd: &Path, memory: &Path, rules: &Path) -> Result<bool> {
    Ok(!symlink_points_to(&cwd.join("project-memory"), memory)?
        || !symlink_points_to(&cwd.join("project-rules"), rules)?
        || fs::symlink_metadata(cwd.join("shared")).is_ok()
        || fs::symlink_metadata(cwd.join(".kota").join("memory")).is_ok()
        || fs::symlink_metadata(cwd.join(".kota").join("rules")).is_ok())
}

fn symlink_points_to(link: &Path, target: &Path) -> Result<bool> {
    let Ok(meta) = fs::symlink_metadata(link) else {
        return Ok(false);
    };
    if !meta.file_type().is_symlink() {
        return Ok(false);
    }
    Ok(fs::read_link(link)? == target)
}

fn move_legacy_dir_if_needed(old: &Path, new: &Path) -> Result<bool> {
    if old == new || !old.exists() {
        return Ok(false);
    }
    if !new.exists() {
        if let Some(parent) = new.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(old, new)
            .with_context(|| format!("move {} -> {}", old.display(), new.display()))?;
        return Ok(true);
    }
    merge_directory_contents(old, new)?;
    remove_empty_dir(old)?;
    Ok(true)
}

fn merge_directory_contents(old: &Path, new: &Path) -> Result<()> {
    fs::create_dir_all(new)?;
    for entry in fs::read_dir(old)? {
        let entry = entry?;
        let source = entry.path();
        let target = new.join(entry.file_name());
        let meta = fs::symlink_metadata(&source)?;
        if target.exists() && meta.is_dir() && !meta.file_type().is_symlink() {
            merge_directory_contents(&source, &target)?;
            remove_empty_dir(&source)?;
            continue;
        }
        if target.exists() {
            continue;
        }
        fs::rename(&source, &target)
            .with_context(|| format!("move {} -> {}", source.display(), target.display()))?;
    }
    Ok(())
}

fn prepare_loaded_workspace(workspace: &mut WorkspaceProject) -> Result<bool> {
    let mut changed = normalize_workspace_metadata(workspace);
    changed |= migrate_workspace_storage_layout(workspace)?;
    changed |= maybe_migrate_legacy_workspace_source(workspace)?;
    changed |= migrate_legacy_gemini_agents(workspace)?;
    changed |= sync_workspace_agent_specs_from_disk(workspace)?;
    crate::bbs::ensure_project_projection(Path::new(&workspace.shared_dir))?;
    if changed {
        save_workspace_files(workspace)?;
    }
    Ok(changed)
}

pub fn mint_project_agent_id(occupied_ids: &HashSet<String>) -> String {
    for _ in 0..100 {
        let uuid = Uuid::new_v4().simple().to_string();
        let candidate = format!(
            "{}{}",
            PROJECT_AGENT_ID_PREFIX,
            &uuid[..PROJECT_AGENT_ID_SUFFIX_LEN]
        );
        if !occupied_ids.contains(&candidate) {
            return candidate;
        }
    }
    format!("{}{}", PROJECT_AGENT_ID_PREFIX, Uuid::new_v4().simple())
}

pub fn mint_project_agent_id_for_root(project_root: &Path) -> String {
    let mut occupied = HashSet::new();
    let agents_root = project_root.join(".agent-workspaces");
    if let Ok(entries) = fs::read_dir(&agents_root) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                occupied.insert(name.to_string());
            }
        }
    }
    mint_project_agent_id(&occupied)
}

fn upsert_workspace_agent_spec(workspace: &mut WorkspaceProject, spec: AgentLaunchSpec) -> bool {
    if let Some(existing) = workspace
        .agents
        .iter_mut()
        .find(|existing| existing.agent_id == spec.agent_id)
    {
        if *existing == spec {
            return false;
        }
        *existing = spec;
        return true;
    }
    workspace.agents.push(spec);
    true
}

fn sync_workspace_agent_specs_from_disk(workspace: &mut WorkspaceProject) -> Result<bool> {
    let agents_root = PathBuf::from(&workspace.local_root).join(".agent-workspaces");
    if !agents_root.exists() {
        let changed = !workspace.agents.is_empty();
        workspace.agents.clear();
        return Ok(changed);
    }

    let existing = workspace
        .agents
        .iter()
        .map(|spec| (spec.agent_id.clone(), spec.clone()))
        .collect::<HashMap<_, _>>();
    let mut discovered = HashMap::new();
    for entry in fs::read_dir(&agents_root)? {
        let entry = entry?;
        let cwd = entry.path();
        if !cwd.is_dir() || !cwd.join("agent.yaml").is_file() {
            continue;
        }
        let Some(agent_id) = cwd
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
        else {
            continue;
        };
        let fallback_cli = existing.get(&agent_id).map(|spec| &spec.cli);
        let cli = agent_workspace_cli(&cwd, fallback_cli);
        let worktree_root = cwd.join("project-files");
        let adapter_path = cli
            .supported()
            .map(|cli| path_str(&cwd.join(adapter_file_for_cli(cli))))
            .or_else(|| {
                existing
                    .get(&agent_id)
                    .map(|spec| spec.adapter_path.clone())
            })
            .unwrap_or_default();
        discovered.insert(
            agent_id.clone(),
            AgentLaunchSpec {
                agent_id,
                cli,
                cwd: path_str(&cwd),
                project_root: workspace.local_root.clone(),
                worktree_root: path_str(&worktree_root),
                shared_dir: workspace.shared_dir.clone(),
                rules_dir: workspace.rules_dir.clone(),
                adapter_path,
                project_id: workspace.project_id.clone(),
                project_remote: workspace.remote_url.clone(),
                project_base_ref: workspace.base_ref.clone(),
            },
        );
    }

    let mut next = Vec::new();
    for spec in &workspace.agents {
        if let Some(discovered_spec) = discovered.remove(&spec.agent_id) {
            next.push(discovered_spec);
        }
    }
    let mut added = discovered.into_values().collect::<Vec<_>>();
    added.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
    next.extend(added);

    if workspace.agents == next {
        return Ok(false);
    }
    workspace.agents = next;
    Ok(true)
}

fn agent_workspace_cli(cwd: &Path, fallback: Option<&WorkspaceAgentCli>) -> WorkspaceAgentCli {
    read_agent_cli_yaml(&cwd.join("SHELL.yaml"))
        .or_else(|| read_agent_cli_yaml(&cwd.join("agent.yaml")))
        .or_else(|| fallback.cloned())
        .unwrap_or_else(|| crate::pty::agent::AgentCli::Codex.into())
}

fn read_agent_cli_yaml(path: &Path) -> Option<WorkspaceAgentCli> {
    let parsed: AgentWorkspaceCliYaml =
        serde_yaml::from_str(&fs::read_to_string(path).ok()?).ok()?;
    parsed
        .provider
        .as_deref()
        .or(parsed.command.as_deref())
        .or(parsed.shell.as_deref())
        .and_then(WorkspaceAgentCli::from_name)
}

fn agent_cli_from_name(name: &str) -> Option<crate::pty::agent::AgentCli> {
    match name.trim().to_lowercase().as_str() {
        "claude" | "cc" | "claude-code" => Some(crate::pty::agent::AgentCli::Claude),
        "codex" => Some(crate::pty::agent::AgentCli::Codex),
        "antigravity" | "agy" | "antigravity-cli" | "gemini" | "gemini-cli" => {
            Some(crate::pty::agent::AgentCli::Antigravity)
        }
        "opencode" | "open-code" => Some(crate::pty::agent::AgentCli::Opencode),
        "pi" => Some(crate::pty::agent::AgentCli::Pi),
        "kimi" | "kimi-code" => Some(crate::pty::agent::AgentCli::Kimi),
        _ => None,
    }
}

fn migrate_legacy_gemini_agents(workspace: &WorkspaceProject) -> Result<bool> {
    let agents_root = PathBuf::from(&workspace.local_root).join(".agent-workspaces");
    if !agents_root.is_dir() {
        return Ok(false);
    }
    let mut changed = false;
    for entry in fs::read_dir(&agents_root)? {
        let entry = entry?;
        let cwd = entry.path();
        if !cwd.is_dir() {
            continue;
        }
        changed |= migrate_legacy_gemini_shell_yaml(&cwd.join("SHELL.yaml"))?;
        changed |= migrate_legacy_gemini_agent_yaml(&cwd.join("agent.yaml"))?;
        changed |= migrate_legacy_gemini_adapter(&cwd)?;
    }
    Ok(changed)
}

fn migrate_legacy_gemini_shell_yaml(path: &Path) -> Result<bool> {
    if !path.is_file() {
        return Ok(false);
    }
    let text = fs::read_to_string(path)?;
    if !text.contains("gemini") && !text.contains("GEMINI") {
        return Ok(false);
    }
    let mut yaml: serde_yaml::Mapping = serde_yaml::from_str(&text)
        .map(|value: serde_yaml::Value| value.as_mapping().cloned().unwrap_or_default())?;
    let mut changed = false;
    let mut is_legacy_gemini_shell = false;
    for key in ["provider", "command", "shell"] {
        let yaml_key = serde_yaml::Value::String(key.into());
        if let Some(value) = yaml.get_mut(&yaml_key) {
            if value.as_str().is_some_and(|raw| {
                matches!(
                    raw.trim().to_ascii_lowercase().as_str(),
                    "gemini" | "gemini-cli"
                )
            }) {
                is_legacy_gemini_shell = true;
                *value = serde_yaml::Value::String(if key == "command" {
                    "agy".into()
                } else {
                    "antigravity".into()
                });
                changed = true;
            }
        }
    }
    if !is_legacy_gemini_shell {
        return Ok(changed);
    }
    for key in ["model", "effort"] {
        if yaml.remove(serde_yaml::Value::String(key.into())).is_some() {
            changed = true;
        }
    }
    let args_key = serde_yaml::Value::String("args".into());
    if let Some(args) = yaml.get(&args_key).and_then(|value| value.as_sequence()) {
        let raw = args
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect::<Vec<_>>();
        let next = normalize_legacy_gemini_args_for_antigravity(&raw);
        if next != raw {
            yaml.insert(
                args_key,
                serde_yaml::Value::Sequence(
                    next.into_iter().map(serde_yaml::Value::String).collect(),
                ),
            );
            changed = true;
        }
    }
    if changed {
        fs::write(path, serde_yaml::to_string(&yaml)?)?;
    }
    Ok(changed)
}

fn normalize_legacy_gemini_args_for_antigravity(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut allow_all = false;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--model" || arg == "--approval-mode" {
            if arg == "--approval-mode"
                && matches!(
                    args.get(i + 1).map(String::as_str),
                    Some("yolo" | "full-auto")
                )
            {
                allow_all = true;
            }
            i += if args.get(i + 1).is_some() { 2 } else { 1 };
            continue;
        }
        if arg.starts_with("--model=") {
            i += 1;
            continue;
        }
        if matches!(
            arg.strip_prefix("--approval-mode="),
            Some("yolo" | "full-auto")
        ) {
            allow_all = true;
            i += 1;
            continue;
        }
        if arg.starts_with("--approval-mode=") {
            i += 1;
            continue;
        }
        if arg == "--skip-trust" || arg == "--yolo" || arg == "--dangerously-skip-permissions" {
            allow_all = true;
            i += 1;
            continue;
        }
        out.push(args[i].clone());
        i += 1;
    }
    if allow_all
        && !out
            .iter()
            .any(|arg| arg == "--dangerously-skip-permissions")
    {
        out.push("--dangerously-skip-permissions".into());
    }
    out
}

fn migrate_legacy_gemini_agent_yaml(path: &Path) -> Result<bool> {
    if !path.is_file() {
        return Ok(false);
    }
    let text = fs::read_to_string(path)?;
    if !text.contains("gemini") {
        return Ok(false);
    }
    let mut yaml: serde_yaml::Mapping = serde_yaml::from_str(&text)
        .map(|value: serde_yaml::Value| value.as_mapping().cloned().unwrap_or_default())?;
    let key = serde_yaml::Value::String("avatar-id".into());
    let changed = yaml
        .get_mut(&key)
        .and_then(|value| value.as_str())
        .is_some_and(|value| value == "gemini");
    if changed {
        yaml.insert(key, serde_yaml::Value::String("antigravity".into()));
        fs::write(path, serde_yaml::to_string(&yaml)?)?;
    }
    Ok(changed)
}

fn migrate_legacy_gemini_adapter(cwd: &Path) -> Result<bool> {
    let legacy = cwd.join("GEMINI.md");
    if !legacy.is_file() || !file_contains_kota_adapter_marker(&legacy) {
        return Ok(false);
    }
    let target = cwd.join("AGENTS.md");
    if !target.is_file() {
        let text = fs::read_to_string(&legacy)?
            .replace("kota:adapter:GEMINI.md", "kota:adapter:AGENTS.md");
        fs::write(&target, text)?;
    }
    fs::remove_file(&legacy)?;
    Ok(true)
}

fn maybe_migrate_legacy_workspace_source(workspace: &mut WorkspaceProject) -> Result<bool> {
    let current = PathBuf::from(&workspace.source_dir);
    if !is_under_legacy_default_project_root(&current) {
        return Ok(false);
    }
    let Some(relative) = legacy_source_relative_path(&current) else {
        return Ok(false);
    };
    let target = default_local_project_root().join(relative);
    if target == current {
        return Ok(false);
    }
    if target.join(".git").exists() {
        workspace.source_dir = path_str(&target);
        refresh_workspace_agent_source_metadata(workspace);
        return Ok(true);
    }
    let Ok(token) = github_cli_token() else {
        return Ok(false);
    };
    if target.exists() && fs::read_dir(&target)?.next().is_some() {
        return Ok(false);
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    if target.exists() {
        fs::remove_dir_all(&target)?;
    }
    let clone_url = if workspace.remote_url.trim().is_empty() {
        format!("https://github.com/{}.git", workspace.repo_full_name)
    } else {
        workspace.remote_url.clone()
    };
    run_git_with_token(
        &token.access_token,
        &workspace_project_root(&workspace.project_id),
        &["clone", clone_url.as_str(), path_str(&target).as_str()],
    )?;
    workspace.source_dir = path_str(&target);
    refresh_workspace_agent_source_metadata(workspace);
    Ok(true)
}

fn refresh_workspace_agent_source_metadata(workspace: &mut WorkspaceProject) {
    for spec in &mut workspace.agents {
        spec.project_remote = workspace.remote_url.clone();
        spec.project_base_ref = workspace.base_ref.clone();
    }
}

fn legacy_source_relative_path(path: &Path) -> Option<PathBuf> {
    let legacy = legacy_default_project_root();
    path.strip_prefix(legacy).ok().map(Path::to_path_buf)
}

fn is_under_legacy_default_project_root(path: &Path) -> bool {
    path.starts_with(legacy_default_project_root())
}

fn legacy_default_project_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Developer")
        .join("Kota")
}

fn workspace_dirty_summary(workspace: &WorkspaceProject) -> String {
    let mut sections = Vec::new();
    push_git_warning_section(
        &mut sections,
        "Local project files",
        Path::new(&workspace.source_dir),
    );
    for spec in &workspace.agents {
        push_git_warning_section(
            &mut sections,
            &format!("Agent {} project-files", spec.agent_id),
            Path::new(&spec.worktree_root),
        );
    }
    sections.join("\n\n")
}

fn push_git_warning_section(sections: &mut Vec<String>, label: &str, path: &Path) {
    let Some(status) = git_sync_warning_status(path) else {
        return;
    };
    sections.push(format!("{label} ({})\n{status}", path.display()));
}

fn git_sync_warning_status(cwd: &Path) -> Option<String> {
    if !cwd.join(".git").exists() {
        return None;
    }
    let output = Command::new("git")
        .current_dir(cwd)
        .args(["status", "--short", "--branch"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        return None;
    }
    let mut lines = text.lines();
    let branch = lines.next().unwrap_or_default();
    let local_changes = lines.any(|line| !line.trim().is_empty());
    let unsynced = branch.contains("ahead")
        || branch.contains("diverged")
        || branch.contains("no upstream")
        || branch.contains("gone");
    if local_changes || unsynced {
        Some(text)
    } else {
        None
    }
}

fn project_memory_dir(root: &Path) -> PathBuf {
    root.join("project-memory")
}

fn ensure_project_memory_dirs(memory: &Path) -> Result<()> {
    fs::create_dir_all(memory.join("scratch"))?;
    Ok(())
}

fn project_rules_dir(root: &Path) -> PathBuf {
    root.join("project-rules")
}

fn legacy_project_memory_dir(root: &Path) -> PathBuf {
    root.join("shared")
}

fn legacy_project_rules_dir(root: &Path) -> PathBuf {
    root.join(".kota").join("rules")
}

pub fn ensure_storage_layout() {
    ensure_kota_storage_layout_once();
}

fn ensure_kota_storage_layout_once() {
    #[cfg(not(test))]
    {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            if let Err(err) = migrate_legacy_kota_storage() {
                eprintln!("Kota storage migration failed: {err}");
            }
        });
    }
}

#[cfg(not(test))]
fn migrate_legacy_kota_storage() -> Result<()> {
    let legacy_home = legacy_kota_home_raw();
    let next_home = kota_home_raw();
    if !legacy_home.exists() {
        fs::create_dir_all(kota_workspaces_dir_raw())?;
        return Ok(());
    }

    fs::create_dir_all(&next_home)?;
    move_path_preserving_existing(
        &legacy_home.join(LEGACY_WORKSPACES_DIR),
        &kota_workspaces_dir_raw(),
    )?;

    for entry in fs::read_dir(&legacy_home)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == std::ffi::OsStr::new(LEGACY_WORKSPACES_DIR) {
            continue;
        }
        move_path_preserving_existing(&entry.path(), &next_home.join(name))?;
    }
    remove_empty_dir(&legacy_home)?;
    rewrite_migrated_workspace_root_references()?;
    repair_migrated_workspace_projections()?;
    repair_migrated_git_worktrees()?;
    Ok(())
}

#[cfg(not(test))]
fn move_path_preserving_existing(source: &Path, target: &Path) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    if !target.exists() {
        fs::rename(source, target)
            .with_context(|| format!("move {} -> {}", source.display(), target.display()))?;
        return Ok(());
    }
    if source.is_dir() && target.is_dir() {
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            move_path_preserving_existing(&entry.path(), &target.join(entry.file_name()))?;
        }
        remove_empty_dir(source)?;
    }
    Ok(())
}

#[cfg(not(test))]
fn rewrite_migrated_workspace_root_references() -> Result<()> {
    let old_prefix = legacy_kota_home_raw().join(LEGACY_WORKSPACES_DIR);
    let new_prefix = kota_workspaces_dir_raw();
    let old = path_str(&old_prefix);
    let new = path_str(&new_prefix);
    let mut stack = vec![new_prefix];
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            for entry in fs::read_dir(&path)? {
                stack.push(entry?.path());
            }
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !matches!(
            name,
            "workspace.json"
                | "local-state.json"
                | "SHELL.yaml"
                | "AGENTS.md"
                | "CLAUDE.md"
                | "opencode.json"
        ) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        if !text.contains(&old) {
            continue;
        }
        fs::write(&path, text.replace(&old, &new))?;
    }
    let active = kota_home_raw().join("active-workspace.json");
    if let Ok(text) = fs::read_to_string(&active) {
        if text.contains(&old) {
            fs::write(&active, text.replace(&old, &new))?;
        }
    }
    Ok(())
}

#[cfg(not(test))]
fn repair_migrated_workspace_projections() -> Result<()> {
    let workspaces = kota_workspaces_dir_raw();
    if !workspaces.is_dir() {
        return Ok(());
    }
    for project in fs::read_dir(&workspaces)? {
        let project = project?.path();
        let agents_root = project.join(".agent-workspaces");
        if !agents_root.is_dir() {
            continue;
        }
        for entry in fs::read_dir(&agents_root)? {
            let cwd = entry?.path();
            if !cwd.is_dir() {
                continue;
            }
            replace_symlink(project.join("project-memory"), &cwd.join("project-memory"))?;
            replace_symlink(project.join("project-rules"), &cwd.join("project-rules"))?;
        }
    }
    Ok(())
}

#[cfg(not(test))]
fn repair_migrated_git_worktrees() -> Result<()> {
    let workspaces = kota_workspaces_dir_raw();
    if !workspaces.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(&workspaces)? {
        let root = entry?.path();
        let workspace_path = root.join("workspace.json");
        if !workspace_path.is_file() {
            continue;
        }
        let Ok(text) = fs::read_to_string(&workspace_path) else {
            continue;
        };
        let Ok(json) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Some(source_dir) = json.get("sourceDir").and_then(Value::as_str) else {
            continue;
        };
        let source = PathBuf::from(source_dir);
        if !source.join(".git").exists() {
            continue;
        }
        let agents_root = root.join(".agent-workspaces");
        if !agents_root.is_dir() {
            continue;
        }
        for agent in fs::read_dir(&agents_root)? {
            let worktree = agent?.path().join("project-files");
            if !worktree.join(".git").exists() {
                continue;
            }
            let _ = Command::new("git")
                .arg("-C")
                .arg(&source)
                .args(["worktree", "repair"])
                .arg(&worktree)
                .status();
        }
    }
    Ok(())
}

fn kota_home() -> PathBuf {
    ensure_kota_storage_layout_once();
    kota_home_raw()
}

fn kota_home_raw() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(KOTA_HOME_DIR)
}

#[cfg(not(test))]
fn legacy_kota_home_raw() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(LEGACY_KOTA_HOME_DIR)
}

fn kota_workspaces_dir() -> PathBuf {
    ensure_kota_storage_layout_once();
    kota_workspaces_dir_raw()
}

fn kota_workspaces_dir_raw() -> PathBuf {
    kota_home_raw().join(KOTA_WORKSPACES_DIR)
}

fn write_secret_json<T: Serialize>(account: &str, value: &T) -> Result<()> {
    let raw = serde_json::to_string(value)?;
    write_secret(account, &raw)
}

fn read_secret_json<T: for<'de> Deserialize<'de>>(account: &str) -> Result<T> {
    let raw = read_secret(account)?;
    Ok(serde_json::from_str(&raw)?)
}

fn write_secret(account: &str, value: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let status = Command::new("security")
            .args([
                "add-generic-password",
                "-U",
                "-a",
                account,
                "-s",
                SECRET_SERVICE,
                "-w",
                value,
            ])
            .status();
        if let Ok(status) = status {
            if status.success() {
                return Ok(());
            }
        }
    }
    let path = local_secret_path(account);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, value)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(path, perms)?;
    }
    Ok(())
}

fn read_secret(account: &str) -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("security")
            .args([
                "find-generic-password",
                "-a",
                account,
                "-s",
                SECRET_SERVICE,
                "-w",
            ])
            .output();
        if let Ok(output) = output {
            if output.status.success() {
                return Ok(String::from_utf8_lossy(&output.stdout)
                    .trim_end_matches('\n')
                    .to_string());
            }
        }
    }
    let path = local_secret_path(account);
    Ok(String::from_utf8_lossy(&fs::read(path)?).to_string())
}

fn delete_secret(account: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("security")
            .args([
                "delete-generic-password",
                "-a",
                account,
                "-s",
                SECRET_SERVICE,
            ])
            .status();
    }
    let path = local_secret_path(account);
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn local_secret_path(account: &str) -> PathBuf {
    kota_home()
        .join("local-secrets")
        .join(format!("{account}.json"))
}

fn json_string(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| anyhow!("response missing string field `{key}`: {value}"))
}

fn json_i64(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(Value::as_i64)
}

fn ureq_err(err: ureq::Error) -> anyhow::Error {
    match err {
        ureq::Error::Status(code, response) => {
            let body = response.into_string().unwrap_or_default();
            anyhow!("HTTP {code}: {body}")
        }
        ureq::Error::Transport(err) => anyhow!("network error: {err}"),
    }
}

fn response_json(response: std::result::Result<ureq::Response, ureq::Error>) -> Result<Value> {
    let response = response.map_err(ureq_err)?;
    response
        .into_json::<Value>()
        .context("decode JSON response")
}

fn split_scope(scope: Option<&str>) -> Vec<String> {
    scope
        .unwrap_or_default()
        .split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn safe_project_id(full_name: &str) -> String {
    full_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

fn escape_drive_query(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}

fn path_str(path: &Path) -> String {
    path.display().to_string()
}

fn yaml_quote(value: &str) -> String {
    serde_yaml::to_string(value)
        .unwrap_or_else(|_| format!("{value:?}"))
        .trim()
        .trim_start_matches("--- ")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kota-integrations-test-{}-{}",
            label,
            Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn workspace_for_test(root: &Path, agents: Vec<AgentLaunchSpec>) -> WorkspaceProject {
        WorkspaceProject {
            project_id: "owner-repo".into(),
            repo_full_name: "owner/repo".into(),
            remote_url: "https://github.com/owner/repo.git".into(),
            github_html_url: "https://github.com/owner/repo".into(),
            default_branch: "main".into(),
            base_ref: "origin/main".into(),
            local_root: path_str(root),
            local_root_bytes: 0,
            source_dir: path_str(&root.join("source")),
            source_dir_bytes: 0,
            shared_dir: path_str(&root.join("project-memory")),
            rules_dir: path_str(&root.join("project-rules")),
            agents,
            archived: false,
            archived_at: None,
        }
    }

    fn workspace_agent_for_test(
        root: &Path,
        agent_id: &str,
        cli: WorkspaceAgentCli,
    ) -> AgentLaunchSpec {
        let cwd = root.join(".agent-workspaces").join(agent_id);
        AgentLaunchSpec {
            agent_id: agent_id.into(),
            cli,
            cwd: path_str(&cwd),
            project_root: path_str(root),
            worktree_root: path_str(&cwd.join("project-files")),
            shared_dir: path_str(&root.join("project-memory")),
            rules_dir: path_str(&root.join("project-rules")),
            adapter_path: String::new(),
            project_id: "owner-repo".into(),
            project_remote: "https://github.com/owner/repo.git".into(),
            project_base_ref: "origin/main".into(),
        }
    }

    fn manager_with_workspace_for_test(workspace: WorkspaceProject) -> IntegrationManager {
        let account_root = PathBuf::from(&workspace.local_root);
        IntegrationManager {
            active_workspace: Mutex::new(Some(workspace)),
            storage_measurement: StorageMeasurementController {
                runtime: Arc::new(Mutex::new(StorageMeasurementRuntime {
                    last_success: None,
                    updating: false,
                    error: None,
                    active_job_id: 0,
                    child: None,
                    shutting_down: false,
                })),
                cache_path: account_root.join(STORAGE_MEASUREMENT_CACHE_FILE),
                account_root,
            },
        }
    }

    fn init_git_repo(root: &Path) {
        run_git_plain(root, &["init"]).unwrap();
        run_git_plain(root, &["checkout", "-b", "main"]).unwrap();
        fs::write(root.join("README.md"), "hello\n").unwrap();
        run_git_plain(root, &["add", "."]).unwrap();
        run_git_plain(
            root,
            &[
                "-c",
                "user.name=Kota Test",
                "-c",
                "user.email=kota@example.test",
                "commit",
                "-m",
                "init",
            ],
        )
        .unwrap();
    }

    #[test]
    fn workspace_metadata_serializes_repo_and_path_boundaries() {
        let workspace = WorkspaceProject {
            project_id: "owner-repo".into(),
            repo_full_name: "owner/repo".into(),
            remote_url: "https://github.com/owner/repo.git".into(),
            github_html_url: "https://github.com/owner/repo".into(),
            default_branch: "main".into(),
            base_ref: "origin/main".into(),
            local_root: "/Users/me/Kota/Workspaces/owner-repo".into(),
            local_root_bytes: 0,
            source_dir: "/Users/me/Kota/Projects/owner/repo".into(),
            source_dir_bytes: 0,
            shared_dir: "/Users/me/Kota/Workspaces/owner-repo/project-memory".into(),
            rules_dir: "/Users/me/Kota/Workspaces/owner-repo/project-rules".into(),
            agents: Vec::new(),
            archived: false,
            archived_at: None,
        };
        let json = serde_json::to_string(&workspace).unwrap();
        assert!(json.contains("\"githubHtmlUrl\":\"https://github.com/owner/repo\""));
        assert!(json.contains("\"localRoot\":\"/Users/me/Kota/Workspaces/owner-repo\""));
        assert!(json.contains("\"sourceDir\":\"/Users/me/Kota/Projects/owner/repo\""));

        let meta = WorkspaceProjectMeta {
            version: 1,
            project_id: workspace.project_id.clone(),
            repo_full_name: workspace.repo_full_name.clone(),
            remote_url: workspace.remote_url.clone(),
            github_html_url: workspace.github_html_url.clone(),
            default_branch: workspace.default_branch.clone(),
            base_ref: workspace.base_ref.clone(),
        };
        let yaml = serde_yaml::to_string(&meta).unwrap();
        assert!(yaml.contains("githubHtmlUrl: https://github.com/owner/repo"));
        assert!(yaml.contains("defaultBranch: main"));
    }

    #[test]
    fn workspace_unknown_cli_round_trips_as_unsupported() {
        let root = temp_dir("workspace-unknown-cli-roundtrip");
        let workspace = workspace_for_test(
            &root,
            vec![workspace_agent_for_test(
                &root,
                "future-agent",
                crate::pty::agent::AgentCli::Codex.into(),
            )],
        );
        let mut json = serde_json::to_value(workspace).unwrap();
        json["agents"][0]["cli"] = serde_json::Value::String("future-cli".into());

        let loaded: WorkspaceProject = serde_json::from_value(json).unwrap();

        assert_eq!(
            loaded.agents[0].cli,
            WorkspaceAgentCli::Unsupported("future-cli".into())
        );
        assert_eq!(
            serde_json::to_value(&loaded).unwrap()["agents"][0]["cli"],
            serde_json::Value::String("future-cli".into())
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[cfg(unix)]
    fn workspace_projection_keeps_single_memory_rules_and_provider_skills() {
        let root = temp_dir("workspace-projection");
        let cwd = root.join(".agent-workspaces/alice");
        let shared = root.join("project-memory");
        let rules = root.join("project-rules");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&shared).unwrap();
        fs::create_dir_all(&rules).unwrap();
        replace_symlink(&shared, &cwd.join("shared")).unwrap();
        fs::create_dir_all(cwd.join(".kota")).unwrap();
        replace_symlink(&rules, &cwd.join(".kota/rules")).unwrap();
        replace_symlink(&shared, &cwd.join(".kota/memory")).unwrap();
        fs::create_dir_all(cwd.join(".agents/skills")).unwrap();

        ensure_workspace_projections(&cwd, &shared, &rules, crate::pty::agent::AgentCli::Claude)
            .unwrap();

        assert!(!cwd.join("shared").exists());
        assert_eq!(fs::read_link(cwd.join("project-rules")).unwrap(), rules);
        assert_eq!(fs::read_link(cwd.join("project-memory")).unwrap(), shared);
        assert!(!cwd.join(".kota").exists());
        assert!(cwd.join(".claude/skills").is_dir());
        assert!(!cwd.join(".agents/skills").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[cfg(unix)]
    fn workspace_storage_layout_migrates_legacy_memory_rules_paths() {
        let root = temp_dir("workspace-storage-layout");
        let legacy_shared = root.join("shared");
        let legacy_rules = root.join(".kota/rules");
        let cwd = root.join(".agent-workspaces/alice");
        fs::create_dir_all(&legacy_shared).unwrap();
        fs::create_dir_all(&legacy_rules).unwrap();
        fs::create_dir_all(cwd.join(".kota")).unwrap();
        fs::write(legacy_shared.join("hot_memory.md"), "memory").unwrap();
        fs::write(legacy_rules.join("coding-style.md"), "rules").unwrap();
        fs::write(cwd.join(".kota/missing-skills.txt"), "missing").unwrap();
        fs::write(cwd.join("agent.yaml"), "id: alice\n").unwrap();
        fs::write(cwd.join("SHELL.yaml"), "provider: codex\n").unwrap();
        replace_symlink(&legacy_shared, &cwd.join("shared")).unwrap();
        replace_symlink(&legacy_shared, &cwd.join(".kota/memory")).unwrap();
        replace_symlink(&legacy_rules, &cwd.join(".kota/rules")).unwrap();

        let mut workspace = WorkspaceProject {
            project_id: "owner-repo".into(),
            repo_full_name: "owner/repo".into(),
            remote_url: "https://github.com/owner/repo.git".into(),
            github_html_url: "https://github.com/owner/repo".into(),
            default_branch: "main".into(),
            base_ref: "origin/main".into(),
            local_root: path_str(&root),
            local_root_bytes: 0,
            source_dir: path_str(&root.join("source")),
            source_dir_bytes: 0,
            shared_dir: path_str(&legacy_shared),
            rules_dir: path_str(&legacy_rules),
            agents: Vec::new(),
            archived: false,
            archived_at: None,
        };

        assert!(migrate_workspace_storage_layout(&mut workspace).unwrap());

        let memory = root.join("project-memory");
        let rules = root.join("project-rules");
        assert!(memory.join("hot_memory.md").is_file());
        assert!(memory.join("scratch").is_dir());
        assert!(rules.join("coding-style.md").is_file());
        assert_eq!(workspace.shared_dir, path_str(&memory));
        assert_eq!(workspace.rules_dir, path_str(&rules));
        assert_eq!(fs::read_link(cwd.join("project-memory")).unwrap(), memory);
        assert_eq!(fs::read_link(cwd.join("project-rules")).unwrap(), rules);
        assert_eq!(
            fs::read_to_string(cwd.join("missing-skills.txt")).unwrap(),
            "missing"
        );
        assert!(!cwd.join("shared").exists());
        assert!(!cwd.join(".kota").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn normalize_legacy_agent_specs_points_worktree_to_project_files() {
        let mut workspace = WorkspaceProject {
            project_id: "owner-repo".into(),
            repo_full_name: "owner/repo".into(),
            remote_url: "https://github.com/owner/repo.git".into(),
            github_html_url: String::new(),
            default_branch: "main".into(),
            base_ref: "origin/main".into(),
            local_root: "/Users/me/Kota/Workspaces/owner-repo".into(),
            local_root_bytes: 0,
            source_dir: "/Users/me/Kota/Projects/owner/repo".into(),
            source_dir_bytes: 0,
            shared_dir: "/Users/me/Kota/Workspaces/owner-repo/project-memory".into(),
            rules_dir: "/Users/me/Kota/Workspaces/owner-repo/project-rules".into(),
            agents: vec![AgentLaunchSpec {
                agent_id: "alice".into(),
                cli: crate::pty::agent::AgentCli::Codex.into(),
                cwd: "/Users/me/Kota/Workspaces/owner-repo/.agent-workspaces/alice".into(),
                project_root: "/Users/me/Kota/Workspaces/owner-repo".into(),
                worktree_root: "/Users/me/Kota/Workspaces/owner-repo/.agent-workspaces/alice"
                    .into(),
                shared_dir: "/Users/me/Kota/Workspaces/owner-repo/project-memory".into(),
                rules_dir: "/Users/me/Kota/Workspaces/owner-repo/project-rules".into(),
                adapter_path:
                    "/Users/me/Kota/Workspaces/owner-repo/.agent-workspaces/alice/AGENTS.md".into(),
                project_id: "owner-repo".into(),
                project_remote: "https://github.com/owner/repo.git".into(),
                project_base_ref: "origin/main".into(),
            }],
            archived: false,
            archived_at: None,
        };

        normalize_workspace_metadata(&mut workspace);

        assert_eq!(workspace.github_html_url, "https://github.com/owner/repo");
        assert!(workspace.agents[0]
            .worktree_root
            .ends_with(".agent-workspaces/alice/project-files"));
    }

    #[test]
    fn rebase_workspace_project_root_moves_only_the_outer_root() {
        let mut workspace = WorkspaceProject {
            project_id: "owner-repo".into(),
            repo_full_name: "owner/repo".into(),
            remote_url: "https://github.com/owner/repo.git".into(),
            github_html_url: String::new(),
            default_branch: "main".into(),
            base_ref: "origin/main".into(),
            local_root: "/Users/me/.kota/projects/owner-repo".into(),
            local_root_bytes: 0,
            source_dir: "/Users/me/Kota/Projects/owner/repo".into(),
            source_dir_bytes: 0,
            shared_dir: "/Users/me/.kota/projects/owner-repo/project-memory".into(),
            rules_dir: "/Users/me/.kota/projects/owner-repo/project-rules".into(),
            agents: vec![AgentLaunchSpec {
                agent_id: "agent-123".into(),
                cli: crate::pty::agent::AgentCli::Codex.into(),
                cwd: "/Users/me/.kota/projects/owner-repo/.agent-workspaces/agent-123".into(),
                project_root: "/Users/me/.kota/projects/owner-repo".into(),
                worktree_root:
                    "/Users/me/.kota/projects/owner-repo/.agent-workspaces/agent-123/project-files"
                        .into(),
                shared_dir: "/Users/me/.kota/projects/owner-repo/project-memory".into(),
                rules_dir: "/Users/me/.kota/projects/owner-repo/project-rules".into(),
                adapter_path:
                    "/Users/me/.kota/projects/owner-repo/.agent-workspaces/agent-123/AGENTS.md"
                        .into(),
                project_id: "owner-repo".into(),
                project_remote: "https://github.com/owner/repo.git".into(),
                project_base_ref: "origin/main".into(),
            }],
            archived: false,
            archived_at: None,
        };

        let changed = rebase_workspace_project_root(
            &mut workspace,
            Path::new("/Users/me/Kota/Workspaces/owner-repo"),
        );

        assert!(changed);
        assert_eq!(workspace.local_root, "/Users/me/Kota/Workspaces/owner-repo");
        assert_eq!(
            workspace.agents[0].cwd,
            "/Users/me/Kota/Workspaces/owner-repo/.agent-workspaces/agent-123"
        );
        assert_eq!(
            workspace.agents[0].worktree_root,
            "/Users/me/Kota/Workspaces/owner-repo/.agent-workspaces/agent-123/project-files"
        );
        assert_eq!(
            workspace.agents[0].adapter_path,
            "/Users/me/Kota/Workspaces/owner-repo/.agent-workspaces/agent-123/AGENTS.md"
        );
    }

    #[test]
    fn workspace_agent_specs_sync_from_agent_workspaces() {
        let root = temp_dir("workspace-agent-sync");
        let agents_root = root.join(".agent-workspaces");
        let custom = agents_root.join("jr-cc-47-1m");
        fs::create_dir_all(&custom).unwrap();
        fs::write(
            custom.join("agent.yaml"),
            "id: jr-cc-47-1m\ndisplay-name: Jr. CC-47-1M\nshell: claude\n",
        )
        .unwrap();
        fs::write(custom.join("SHELL.yaml"), "provider: claude\n").unwrap();
        fs::write(custom.join("CLAUDE.md"), "# Jr. CC-47-1M\n").unwrap();

        let stale = agents_root.join("stale");
        fs::create_dir_all(&stale).unwrap();

        let mut workspace = WorkspaceProject {
            project_id: "owner-repo".into(),
            repo_full_name: "owner/repo".into(),
            remote_url: "https://github.com/owner/repo.git".into(),
            github_html_url: "https://github.com/owner/repo".into(),
            default_branch: "main".into(),
            base_ref: "origin/main".into(),
            local_root: path_str(&root),
            local_root_bytes: 0,
            source_dir: path_str(&root.join("source")),
            source_dir_bytes: 0,
            shared_dir: path_str(&root.join("project-memory")),
            rules_dir: path_str(&root.join("project-rules")),
            agents: vec![AgentLaunchSpec {
                agent_id: "stale".into(),
                cli: crate::pty::agent::AgentCli::Codex.into(),
                cwd: path_str(&stale),
                project_root: path_str(&root),
                worktree_root: path_str(&stale.join("project-files")),
                shared_dir: path_str(&root.join("project-memory")),
                rules_dir: path_str(&root.join("project-rules")),
                adapter_path: path_str(&stale.join("AGENTS.md")),
                project_id: "owner-repo".into(),
                project_remote: "https://github.com/owner/repo.git".into(),
                project_base_ref: "origin/main".into(),
            }],
            archived: false,
            archived_at: None,
        };

        assert!(sync_workspace_agent_specs_from_disk(&mut workspace).unwrap());
        assert_eq!(workspace.agents.len(), 1);
        assert_eq!(workspace.agents[0].agent_id, "jr-cc-47-1m");
        assert_eq!(workspace.agents[0].cli, crate::pty::agent::AgentCli::Claude);
        assert!(workspace.agents[0]
            .worktree_root
            .ends_with(".agent-workspaces/jr-cc-47-1m/project-files"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn workspace_agent_sync_preserves_unknown_cli_without_provider_artifacts() {
        let root = temp_dir("workspace-agent-sync-unknown");
        let declared_cwd = root.join(".agent-workspaces/declared");
        let missing_cwd = root.join(".agent-workspaces/missing");
        for cwd in [&declared_cwd, &missing_cwd] {
            fs::create_dir_all(cwd).unwrap();
            fs::write(
                cwd.join("agent.yaml"),
                format!("id: {}\n", cwd.file_name().unwrap().to_string_lossy()),
            )
            .unwrap();
        }
        fs::write(declared_cwd.join("SHELL.yaml"), "provider: future-cli\n").unwrap();
        fs::write(missing_cwd.join("SHELL.yaml"), "model: future-model\n").unwrap();

        let mut workspace = workspace_for_test(
            &root,
            vec![
                workspace_agent_for_test(
                    &root,
                    "declared",
                    crate::pty::agent::AgentCli::Codex.into(),
                ),
                workspace_agent_for_test(
                    &root,
                    "missing",
                    WorkspaceAgentCli::Unsupported("stored-future-cli".into()),
                ),
            ],
        );

        migrate_workspace_storage_layout(&mut workspace).unwrap();
        assert!(sync_workspace_agent_specs_from_disk(&mut workspace).unwrap());
        assert_eq!(
            workspace.agents[0].cli,
            WorkspaceAgentCli::Unsupported("future-cli".into())
        );
        assert_eq!(
            workspace.agents[1].cli,
            WorkspaceAgentCli::Unsupported("stored-future-cli".into())
        );
        for cwd in [&declared_cwd, &missing_cwd] {
            assert!(!cwd.join("AGENTS.md").exists());
            assert!(!cwd.join("CLAUDE.md").exists());
            assert!(!cwd.join(".agents/skills").exists());
            assert!(!cwd.join(".claude/skills").exists());
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn workspace_agent_launch_rejects_existing_unsupported_cli_without_writes() {
        let root = temp_dir("workspace-agent-launch-unknown");
        let workspace = workspace_for_test(
            &root,
            vec![workspace_agent_for_test(
                &root,
                "future-agent",
                WorkspaceAgentCli::Unsupported("future-cli".into()),
            )],
        );
        let manager = manager_with_workspace_for_test(workspace);

        let error = manager
            .resolve_agent_launch("future-agent".into(), crate::pty::agent::AgentCli::Codex)
            .unwrap_err()
            .to_string();

        assert!(error.contains("unsupported CLI provider"));
        assert!(error.contains("future-cli"));
        assert!(!root.join(".agent-workspaces").exists());
        assert!(!root.join("workspace.json").exists());
        assert_eq!(
            manager
                .active_workspace
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .agents[0]
                .cli,
            WorkspaceAgentCli::Unsupported("future-cli".into())
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_gemini_agent_files_migrate_to_antigravity() {
        let root = temp_dir("legacy-gemini-agent-migration");
        let cwd = root.join(".agent-workspaces/agent-old");
        fs::create_dir_all(&cwd).unwrap();
        fs::write(
            cwd.join("SHELL.yaml"),
            r#"provider: gemini
command: gemini
cwd: "$KOTA_WORKTREE_ROOT"
model: gemini-3.5-flash
effort: max
args:
  - "--model"
  - "gemini-3.5-flash"
  - "--approval-mode"
  - "yolo"
  - "--skip-trust"
"#,
        )
        .unwrap();
        fs::write(cwd.join("agent.yaml"), "id: agent-old\navatar-id: gemini\n").unwrap();
        fs::write(
            cwd.join("GEMINI.md"),
            "# Kota agent adapter\n\n<!-- kota:adapter:GEMINI.md -->\n",
        )
        .unwrap();
        let workspace = WorkspaceProject {
            project_id: "owner-repo".into(),
            repo_full_name: "owner/repo".into(),
            remote_url: "https://github.com/owner/repo.git".into(),
            github_html_url: "https://github.com/owner/repo".into(),
            default_branch: "main".into(),
            base_ref: "origin/main".into(),
            local_root: path_str(&root),
            local_root_bytes: 0,
            source_dir: path_str(&root.join("source")),
            source_dir_bytes: 0,
            shared_dir: path_str(&root.join("project-memory")),
            rules_dir: path_str(&root.join("project-rules")),
            agents: Vec::new(),
            archived: false,
            archived_at: None,
        };

        assert!(migrate_legacy_gemini_agents(&workspace).unwrap());

        let shell = fs::read_to_string(cwd.join("SHELL.yaml")).unwrap();
        assert!(shell.contains("provider: antigravity"));
        assert!(shell.contains("command: agy"));
        assert!(!shell.contains("model:"));
        assert!(!shell.contains("effort:"));
        assert!(shell.contains("--dangerously-skip-permissions"));
        assert!(!shell.contains("gemini"));
        let agent_yaml = fs::read_to_string(cwd.join("agent.yaml")).unwrap();
        assert!(agent_yaml.contains("avatar-id: antigravity"));
        assert!(!cwd.join("GEMINI.md").exists());
        let adapter = fs::read_to_string(cwd.join("AGENTS.md")).unwrap();
        assert!(adapter.contains("kota:adapter:AGENTS.md"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[cfg(unix)]
    fn legacy_root_worktree_migrates_runtime_files_above_project_files() {
        let root = temp_dir("legacy-root-worktree");
        let source = root.join("source");
        fs::create_dir_all(&source).unwrap();
        init_git_repo(&source);

        let cwd = root.join("account/projects/owner-repo/.agent-workspaces/alice");
        let worktree_root = cwd.join("project-files");
        add_agent_worktree(&source, &cwd, "alice", "HEAD").unwrap();
        fs::write(cwd.join("agent.yaml"), "id: alice\n").unwrap();
        fs::write(cwd.join("SHELL.yaml"), "provider: codex\nskills: []\n").unwrap();
        fs::write(
            cwd.join("AGENTS.md"),
            "# Kota agent adapter\n\n<!-- kota:adapter -->\n",
        )
        .unwrap();

        ensure_project_files_worktree(&source, &cwd, &worktree_root, "alice", "HEAD").unwrap();

        assert!(cwd.join("agent.yaml").is_file());
        assert!(cwd.join("SHELL.yaml").is_file());
        assert!(cwd.join("AGENTS.md").is_file());
        assert!(!cwd.join(".git").exists());
        assert!(worktree_root.join(".git").exists());
        assert!(worktree_root.join("README.md").is_file());
        run_git_plain(&worktree_root, &["status", "--short"]).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn storage_measurement_parses_du_kib_as_bytes() {
        assert_eq!(
            parse_du_kib_output(b"47185920\t/Users/example/Kota\n").unwrap(),
            48_318_382_080
        );
        assert!(parse_du_kib_output(b"").is_err());
        assert!(parse_du_kib_output(b"not-a-size\t/tmp\n").is_err());
    }

    #[test]
    fn storage_measurement_cache_round_trips_atomically() {
        let root = temp_dir("storage-measurement-cache");
        let path = root.join(STORAGE_MEASUREMENT_CACHE_FILE);
        let record = StorageMeasurementRecord {
            version: STORAGE_MEASUREMENT_CACHE_VERSION,
            on_disk_bytes: 48_318_382_080,
            available_bytes: 172_872_433_664,
            measured_at: 1_753_000_000,
            app_version: "0.1.7".into(),
        };

        save_storage_measurement_record(&path, &record).unwrap();

        assert_eq!(
            load_storage_measurement_record(&path).unwrap(),
            Some(record)
        );
        assert!(!path.with_extension("json.tmp").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn storage_measurement_cache_rejects_unknown_versions() {
        let root = temp_dir("storage-measurement-version");
        let path = root.join(STORAGE_MEASUREMENT_CACHE_FILE);
        fs::write(
            &path,
            r#"{"version":99,"onDiskBytes":1,"availableBytes":2,"measuredAt":3,"appVersion":"test"}"#,
        )
        .unwrap();

        assert!(load_storage_measurement_record(&path).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn storage_measurement_status_keeps_stale_values_while_updating() {
        let runtime = StorageMeasurementRuntime {
            last_success: Some(StorageMeasurementRecord {
                version: STORAGE_MEASUREMENT_CACHE_VERSION,
                on_disk_bytes: 46,
                available_bytes: 161,
                measured_at: 123,
                app_version: "test".into(),
            }),
            updating: true,
            error: None,
            active_job_id: 7,
            child: None,
            shutting_down: false,
        };

        assert_eq!(
            storage_measurement_status_from_runtime(&runtime),
            StorageMeasurementStatus {
                updating: true,
                on_disk_bytes: Some(46),
                available_bytes: Some(161),
                measured_at: Some(123),
                error: None,
            }
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn storage_measurement_runs_once_in_background_and_persists_success() {
        let root = temp_dir("storage-measurement-worker");
        fs::write(root.join("payload.bin"), vec![7_u8; 4096]).unwrap();
        let cache_path = root.join(STORAGE_MEASUREMENT_CACHE_FILE);
        let controller = StorageMeasurementController {
            runtime: Arc::new(Mutex::new(StorageMeasurementRuntime {
                last_success: None,
                updating: false,
                error: None,
                active_job_id: 0,
                child: None,
                shutting_down: false,
            })),
            account_root: root.clone(),
            cache_path: cache_path.clone(),
        };

        assert!(controller.start().updating);
        assert!(controller.start().updating);
        assert_eq!(
            storage_measurement_runtime(&controller.runtime).active_job_id,
            1
        );

        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            let status = controller.status();
            if !status.updating {
                break status;
            }
            assert!(Instant::now() < deadline, "storage measurement did not finish");
            std::thread::sleep(Duration::from_millis(50));
        };

        assert!(status.error.is_none(), "{:?}", status.error);
        assert!(status.on_disk_bytes.is_some_and(|bytes| bytes > 0));
        assert!(status.available_bytes.is_some_and(|bytes| bytes > 0));
        assert!(load_storage_measurement_record(&cache_path)
            .unwrap()
            .is_some());
        controller.shutdown();
        let _ = fs::remove_dir_all(root);
    }
}
