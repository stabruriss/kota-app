//! Connecting a project to GitHub, changing its account, and going local
//! (local-project PLAN §3 D/E, §5).
//!
//! Checking only reads: it fetches the candidate's branch into a throwaway ref.
//! Connecting changes nothing about the current connection until every remote
//! step has succeeded, so a failure leaves the project exactly as it was. No path
//! force-pushes or replaces remote history.

use super::*;

pub const LARGE_FILE_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LargeFile {
    pub path: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionCandidate {
    pub full_name: String,
    pub private: bool,
    pub default_branch: String,
    /// The repository this project's `origin` already points at.
    pub previous: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionCandidates {
    pub account_login: String,
    pub candidates: Vec<ConnectionCandidate>,
    /// The previous origin, else a repository named like the project.
    pub selected: Option<String>,
    /// Files over GitHub's limit in this project's history; a new repository cannot take them.
    pub large_files: Vec<LargeFile>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionCheck {
    pub account_login: String,
    pub repo_full_name: String,
    /// empty | in_sync | local_ahead | remote_ahead | diverged (connectable),
    /// unrelated | no_access | already_open | blocked_large_file (not), or check_failed.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub large_files: Vec<LargeFile>,
    /// Why a check failed; never repository content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

const CONNECTABLE: [&str; 5] = ["empty", "in_sync", "local_ahead", "remote_ahead", "diverged"];

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ConnectionTarget {
    #[serde(rename_all = "camelCase")]
    Existing { repo_full_name: String },
    New { name: String, private: bool },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionRequest {
    pub project_id: String,
    pub account_login: String,
    pub target: ConnectionTarget,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionApplied {
    pub workspace: WorkspaceProject,
    /// Whether local history went to GitHub; remote_ahead and diverged pull through Bartender.
    pub pushed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_repo: Option<String>,
}

/// A failed connection. The project keeps its previous connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionError {
    /// github_account_* | project_not_found | check_failed | not_connectable |
    /// blocked_large_file | repo_create_failed | push_failed | fetch_failed | save_failed | internal
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<&'static str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub large_files: Vec<LargeFile>,
    /// A repository created for this attempt; it stays on GitHub, empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_repo: Option<String>,
}

impl ConnectionError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), state: None, large_files: Vec::new(), created_repo: None }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ConnectionStatus {
    Local,
    #[serde(rename_all = "camelCase")]
    Github {
        account_login: Option<String>,
        /// ok | needs_login (gh lost the token) | missing (removed from Kota) | unbound (no account yet)
        account_state: &'static str,
    },
}

pub(crate) struct ApiRepo {
    pub full_name: String,
    pub name: String,
    pub owner: String,
    pub private: bool,
    pub default_branch: String,
    pub can_push: bool,
    pub archived: bool,
}

pub(crate) enum ApiError {
    NotFound,
    Rejected(String),
    Network(String),
}

/// What Kota asks GitHub for one account. Tests use local bare repositories.
pub(crate) trait Api {
    fn repo(&self, full_name: &str) -> std::result::Result<ApiRepo, ApiError>;
    fn repos(&self) -> std::result::Result<Vec<ApiRepo>, ApiError>;
    fn create(&self, name: &str, private: bool) -> std::result::Result<ApiRepo, ApiError>;
    fn git_url(&self, full_name: &str) -> String;
}

pub(crate) struct GithubApi {
    pub token: String,
}

impl GithubApi {
    fn call(&self, request: ureq::Request, body: Option<Value>) -> std::result::Result<Value, ApiError> {
        let request = request
            .set("Accept", "application/vnd.github+json")
            .set("X-GitHub-Api-Version", "2022-11-28")
            .set("User-Agent", "Kota")
            .set("Authorization", &format!("Bearer {}", self.token));
        let response = match body {
            Some(body) => request.send_json(body),
            None => request.call(),
        };
        match response {
            Ok(response) => response.into_json::<Value>().map_err(|error| ApiError::Network(error.to_string())),
            Err(ureq::Error::Status(404, _)) => Err(ApiError::NotFound),
            Err(ureq::Error::Status(code, response)) => {
                let body = response.into_json::<Value>().ok();
                let message = body.as_ref().and_then(|b| b.get("message")).and_then(Value::as_str).unwrap_or("");
                Err(ApiError::Rejected(format!("HTTP {code}: {message}")))
            }
            Err(ureq::Error::Transport(error)) => Err(ApiError::Network(error.to_string())),
        }
    }
}

fn api_repo(value: &Value) -> std::result::Result<ApiRepo, ApiError> {
    let repo = parse_repo(value).map_err(|error| ApiError::Network(error.to_string()))?;
    Ok(ApiRepo {
        can_push: value.pointer("/permissions/push").and_then(Value::as_bool).unwrap_or(false),
        archived: value.get("archived").and_then(Value::as_bool).unwrap_or(false),
        full_name: repo.full_name,
        name: repo.name,
        owner: repo.owner,
        private: repo.private,
        default_branch: repo.default_branch,
    })
}

impl Api for GithubApi {
    fn repo(&self, full_name: &str) -> std::result::Result<ApiRepo, ApiError> {
        api_repo(&self.call(ureq::get(&format!("https://api.github.com/repos/{full_name}")), None)?)
    }
    fn repos(&self) -> std::result::Result<Vec<ApiRepo>, ApiError> {
        let url = "https://api.github.com/user/repos?affiliation=owner,collaborator,organization_member&sort=updated&per_page=100";
        let value = self.call(ureq::get(url), None)?;
        value.as_array().map(|list| list.iter().filter_map(|repo| api_repo(repo).ok()).collect())
            .ok_or_else(|| ApiError::Network("GitHub repo list response was not an array".into()))
    }
    fn create(&self, name: &str, private: bool) -> std::result::Result<ApiRepo, ApiError> {
        // auto_init would give the repository its own first commit: unrelated history.
        let body = json!({ "name": name, "private": private, "auto_init": false });
        api_repo(&self.call(ureq::post("https://api.github.com/user/repos"), Some(body))?)
    }
    fn git_url(&self, full_name: &str) -> String {
        format!("https://github.com/{full_name}.git")
    }
}

/// Everything a connection change reads besides the project itself.
pub(crate) struct Ctx<'a> {
    pub api: &'a dyn Api,
    pub token: &'a str,
    /// Every other registered project, archived ones included.
    pub others: &'a [WorkspaceProject],
    pub kota_home: &'a Path,
    pub large_file_bytes: u64,
}

fn git_text(source: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git").current_dir(source).args(args).env("GIT_TERMINAL_PROMPT", "0").output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// `owner/name` of a github.com remote URL.
fn github_full_name(url: &str) -> Option<String> {
    let path = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("git@github.com:"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    let path = path.trim_end_matches('/').trim_end_matches(".git");
    (path.split('/').count() == 2).then(|| path.to_owned())
}

fn large_files(source: &Path, limit: u64) -> Result<Vec<LargeFile>> {
    let objects = git_text(source, &["rev-list", "--objects", "HEAD"]).context("list project history")?;
    let paths: HashMap<&str, &str> = objects.lines().filter_map(|line| line.split_once(' ')).collect();
    let mut batch = Command::new("git")
        .current_dir(source)
        .args(["cat-file", "--batch-check=%(objecttype) %(objectname) %(objectsize)"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let ids = paths.keys().map(|id| format!("{id}\n")).collect::<String>();
    batch.stdin.take().context("cat-file stdin")?.write_all(ids.as_bytes())?;
    let output = batch.wait_with_output()?;
    let mut found: Vec<LargeFile> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split(' ');
            let (kind, id, size) = (parts.next()?, parts.next()?, parts.next()?.parse::<u64>().ok()?);
            (kind == "blob" && size > limit).then(|| LargeFile { path: paths.get(id).copied().unwrap_or(id).to_owned(), bytes: size })
        })
        .collect();
    found.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.path.cmp(&b.path)));
    found.dedup_by(|a, b| a.path == b.path);
    Ok(found)
}

/// History relation between this project and the candidate's default branch.
fn classify(source: &Path, url: &str, branch: &str, token: &str) -> Result<&'static str> {
    let heads = git_with_token(token, source, &["ls-remote", "--heads", url])?;
    if heads.trim().is_empty() {
        return Ok("empty");
    }
    let probe = format!("refs/kota/probe/{}", Uuid::new_v4().simple());
    git_with_token(token, source, &["fetch", "--no-tags", "--quiet", url, &format!("+refs/heads/{branch}:{probe}")])?;
    let relation = (|| {
        let remote = git_text(source, &["rev-parse", &probe]).context("read probe")?;
        let local = git_text(source, &["rev-parse", "HEAD"]).context("read HEAD")?;
        let ancestor = |a: &str, b: &str| git_plain_success(source, &["merge-base", "--is-ancestor", a, b]);
        Ok(if remote == local {
            "in_sync"
        } else if ancestor(&remote, &local) {
            "local_ahead"
        } else if ancestor(&local, &remote) {
            "remote_ahead"
        } else if git_text(source, &["merge-base", &local, &remote]).is_some() {
            "diverged"
        } else {
            "unrelated"
        })
    })();
    let _ = git_plain_success(source, &["update-ref", "-d", &probe]);
    relation
}

fn already_open(project_id: &str, full_name: &str, others: &[WorkspaceProject]) -> bool {
    others.iter().any(|other| {
        other.project_id != project_id && !other.is_local() && other.repo_full_name.eq_ignore_ascii_case(full_name)
    })
}

pub(crate) fn candidates(workspace: &WorkspaceProject, account_login: &str, ctx: &Ctx) -> Result<ConnectionCandidates> {
    let source = Path::new(&workspace.source_dir);
    let mut repos = ctx.api.repos().map_err(|error| anyhow!(api_message(&error)))?;
    let origin = git_text(source, &["remote", "get-url", "origin"]);
    let previous = origin.as_deref().and_then(github_full_name);
    if let Some(previous) = &previous {
        if !repos.iter().any(|repo| repo.full_name.eq_ignore_ascii_case(previous)) {
            if let Ok(repo) = ctx.api.repo(previous) {
                repos.insert(0, repo);
            }
        }
    }
    let name = workspace.display_name.clone().unwrap_or_else(|| {
        workspace.repo_full_name.rsplit('/').next().unwrap_or_default().to_owned()
    });
    let slug = safe_project_id(&name);
    let same_name = repos
        .iter()
        .find(|repo| repo.owner.eq_ignore_ascii_case(account_login) && repo.name.eq_ignore_ascii_case(&slug))
        .map(|repo| repo.full_name.clone());
    let is_previous = |full: &str| {
        previous.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(full)) || origin.as_deref() == Some(ctx.api.git_url(full).as_str())
    };
    let mut candidates: Vec<ConnectionCandidate> = repos
        .into_iter()
        .map(|repo| ConnectionCandidate {
            previous: is_previous(&repo.full_name),
            full_name: repo.full_name,
            private: repo.private,
            default_branch: repo.default_branch,
        })
        .collect();
    candidates.sort_by_key(|candidate| !candidate.previous);
    let selected = candidates.iter().find(|c| c.previous).map(|c| c.full_name.clone()).or(same_name);
    Ok(ConnectionCandidates {
        account_login: account_login.to_owned(),
        candidates,
        selected,
        large_files: large_files(source, ctx.large_file_bytes)?,
    })
}

fn api_message(error: &ApiError) -> String {
    match error {
        ApiError::NotFound => "GitHub could not find it for this account.".into(),
        ApiError::Rejected(message) | ApiError::Network(message) => message.clone(),
    }
}

pub(crate) fn check(workspace: &WorkspaceProject, account_login: &str, full_name: &str, ctx: &Ctx) -> ConnectionCheck {
    let mut result = ConnectionCheck {
        account_login: account_login.to_owned(),
        repo_full_name: full_name.to_owned(),
        state: "check_failed",
        default_branch: None,
        private: None,
        large_files: Vec::new(),
        message: None,
    };
    let repo = match ctx.api.repo(full_name) {
        Ok(repo) => repo,
        Err(ApiError::Network(message)) => return ConnectionCheck { message: Some(message), ..result },
        Err(_) => return ConnectionCheck { state: "no_access", ..result },
    };
    result.repo_full_name = repo.full_name.clone();
    result.default_branch = Some(repo.default_branch.clone());
    result.private = Some(repo.private);
    if !repo.can_push || repo.archived {
        return ConnectionCheck { state: "no_access", ..result };
    }
    if already_open(&workspace.project_id, &repo.full_name, ctx.others) {
        return ConnectionCheck { state: "already_open", ..result };
    }
    let source = Path::new(&workspace.source_dir);
    match classify(source, &ctx.api.git_url(&repo.full_name), &repo.default_branch, ctx.token) {
        Ok(state) => result.state = state,
        Err(error) => return ConnectionCheck { message: Some(error.to_string()), ..result },
    }
    if matches!(result.state, "empty" | "local_ahead") {
        match large_files(source, ctx.large_file_bytes) {
            Ok(files) if !files.is_empty() => {
                result.state = "blocked_large_file";
                result.large_files = files;
            }
            Ok(_) => {}
            Err(error) => {
                result.state = "check_failed";
                result.message = Some(error.to_string());
            }
        }
    }
    result
}

/// D and E: connect a project, or move it to another account or repository. The
/// project's registration, `origin` and helpers change only after the remote work
/// succeeded, all at the end.
pub(crate) fn apply(
    workspace: &WorkspaceProject,
    account_login: &str,
    target: &ConnectionTarget,
    ctx: &Ctx,
) -> std::result::Result<ConnectionApplied, ConnectionError> {
    let source = Path::new(&workspace.source_dir);
    let (repo, state, created) = match target {
        ConnectionTarget::Existing { repo_full_name } => {
            let checked = check(workspace, account_login, repo_full_name, ctx);
            if !CONNECTABLE.contains(&checked.state) {
                let code = match checked.state {
                    "blocked_large_file" => "blocked_large_file",
                    "check_failed" => "check_failed",
                    _ => "not_connectable",
                };
                return Err(ConnectionError {
                    state: Some(checked.state),
                    large_files: checked.large_files,
                    ..ConnectionError::new(code, checked.message.unwrap_or_default())
                });
            }
            let repo = ApiRepo {
                full_name: checked.repo_full_name.clone(),
                name: checked.repo_full_name.rsplit('/').next().unwrap_or_default().to_owned(),
                owner: checked.repo_full_name.split('/').next().unwrap_or_default().to_owned(),
                private: checked.private.unwrap_or(true),
                default_branch: checked.default_branch.clone().unwrap_or_else(|| workspace.default_branch.clone()),
                can_push: true,
                archived: false,
            };
            (repo, checked.state, None)
        }
        ConnectionTarget::New { name, private } => {
            let files = large_files(source, ctx.large_file_bytes)
                .map_err(|error| ConnectionError::new("check_failed", error.to_string()))?;
            if !files.is_empty() {
                return Err(ConnectionError { large_files: files, ..ConnectionError::new("blocked_large_file", "") });
            }
            let repo = ctx.api.create(name.trim(), *private).map_err(|error| ConnectionError::new("repo_create_failed", api_message(&error)))?;
            let created = repo.full_name.clone();
            (repo, "empty", Some(created))
        }
    };
    let failed = |code: &'static str, error: anyhow::Error| ConnectionError {
        created_repo: created.clone(),
        ..ConnectionError::new(code, error.to_string())
    };
    let url = ctx.api.git_url(&repo.full_name);
    // An empty repository has no default branch yet; it takes the local one.
    let branch = if state == "empty" {
        git_text(source, &["branch", "--show-current"]).filter(|b| !b.is_empty()).unwrap_or_else(|| workspace.default_branch.clone())
    } else {
        repo.default_branch.clone()
    };
    let pushed = matches!(state, "empty" | "local_ahead");
    if pushed {
        git_with_token(ctx.token, source, &["push", "--quiet", &url, &format!("HEAD:refs/heads/{branch}")])
            .map_err(|error| failed("push_failed", error))?;
    }
    // Fill origin's tracking ref before origin exists, so a failed fetch leaves no trace.
    git_with_token(ctx.token, source, &["fetch", "--no-tags", "--quiet", &url, &format!("+refs/heads/{branch}:refs/remotes/origin/{branch}")])
        .map_err(|error| failed("fetch_failed", error))?;

    let mut next = workspace.clone();
    next.connection = Some(ProjectConnection::Github { account_login: Some(account_login.to_owned()) });
    next.repo_full_name = repo.full_name.clone();
    next.remote_url = url.clone();
    next.github_html_url = format!("https://github.com/{}", repo.full_name);
    next.default_branch = branch.clone();
    next.base_ref = format!("origin/{branch}");
    if next.display_name.is_none() && workspace.is_local() {
        next.display_name = Some(workspace.project_id.clone());
    }
    refresh_workspace_agent_source_metadata(&mut next);
    let committed = (|| -> Result<()> {
        let origin = if git_text(source, &["remote", "get-url", "origin"]).is_some() {
            ["remote", "set-url", "origin", url.as_str()]
        } else {
            ["remote", "add", "origin", url.as_str()]
        };
        run_git_plain(source, &origin)?;
        if let Some(current) = git_text(source, &["branch", "--show-current"]).filter(|b| !b.is_empty()) {
            run_git_plain(source, &["branch", &format!("--set-upstream-to=origin/{branch}"), &current])?;
        }
        ensure_project_credentials(&next, ctx.kota_home)?;
        save_workspace_files(&next)
    })();
    committed.map_err(|error| failed("save_failed", error))?;
    Ok(ConnectionApplied { workspace: next, pushed, created_repo: created })
}

/// Connected → Local: Bartender and agents stop reaching GitHub. `origin` stays in
/// Git, so reconnecting offers it first.
pub(crate) fn set_local(workspace: &WorkspaceProject, kota_home: &Path) -> Result<WorkspaceProject> {
    let mut next = workspace.clone();
    if next.display_name.is_none() {
        let name = workspace.repo_full_name.rsplit('/').next().unwrap_or_default();
        next.display_name = Some(if name.is_empty() { workspace.project_id.clone() } else { name.to_owned() });
    }
    next.connection = Some(ProjectConnection::Local);
    next.repo_full_name = String::new();
    next.remote_url = String::new();
    next.github_html_url = String::new();
    next.base_ref = next.default_branch.clone();
    refresh_workspace_agent_source_metadata(&mut next);
    ensure_project_credentials(&next, kota_home)?;
    save_workspace_files(&next)?;
    Ok(next)
}

pub(crate) fn status(workspace: &WorkspaceProject, store: &crate::github_accounts::Store, gh: &dyn crate::github_accounts::Gh) -> ConnectionStatus {
    if workspace.is_local() {
        return ConnectionStatus::Local;
    }
    let bound = match &workspace.connection {
        Some(ProjectConnection::Github { account_login }) => account_login.clone(),
        _ => None,
    };
    let login = crate::github_accounts::project_login(store, workspace.connection.as_ref(), !workspace.remote_url.trim().is_empty());
    let account_state = match (&bound, &login) {
        (Some(_), None) => "missing",
        (_, Some(login)) if gh.token(login).is_none() => "needs_login",
        (_, Some(_)) => "ok",
        (None, None) => "unbound",
    };
    ConnectionStatus::Github { account_login: bound.or(login), account_state }
}

fn registered_workspaces(except: &str) -> Vec<WorkspaceProject> {
    let Ok(entries) = fs::read_dir(kota_workspaces_dir()) else { return Vec::new() };
    entries
        .filter_map(|entry| load_workspace_at(&entry.ok()?.path()).ok())
        .filter(|workspace| workspace.project_id != except)
        .collect()
}

fn with_ctx<T>(project_id: &str, token: &str, run: impl FnOnce(&Ctx) -> T) -> T {
    let others = registered_workspaces(project_id);
    let home = kota_home();
    let api = GithubApi { token: token.to_owned() };
    run(&Ctx { api: &api, token, others: &others, kota_home: &home, large_file_bytes: LARGE_FILE_BYTES })
}

impl IntegrationManager {
    fn connection_workspace(&self, project_id: &str) -> std::result::Result<WorkspaceProject, ConnectionError> {
        crate::bbs_sync::safe_id(project_id).map_err(|_| ConnectionError::new("project_not_found", ""))?;
        let mut workspace = load_workspace_at(&workspace_project_root(project_id))
            .map_err(|_| ConnectionError::new("project_not_found", ""))?;
        let _ = prepare_loaded_workspace(&mut workspace);
        Ok(workspace)
    }

    fn connection_token(account_login: &str) -> std::result::Result<String, ConnectionError> {
        account_token(Some(account_login)).map(|(_, token)| token).map_err(|error| {
            let code = match error.to_string().as_str() {
                "github_account_unknown" => "github_account_unknown",
                "github_account_required" => "github_account_required",
                _ => "github_account_needs_login",
            };
            ConnectionError::new(code, "")
        })
    }

    fn replace_active(&self, workspace: &WorkspaceProject) {
        let mut active = self.active_workspace.lock().expect("workspace state poisoned");
        if active.as_ref().is_some_and(|current| current.project_id == workspace.project_id) {
            *active = Some(workspace.clone());
            let _ = save_active_workspace(workspace);
        }
    }

    pub fn project_connection_candidates(&self, project_id: &str, account_login: &str) -> std::result::Result<ConnectionCandidates, ConnectionError> {
        let workspace = self.connection_workspace(project_id)?;
        let token = Self::connection_token(account_login)?;
        with_ctx(project_id, &token, |ctx| candidates(&workspace, account_login, ctx))
            .map_err(|error| ConnectionError::new("check_failed", error.to_string()))
    }

    pub fn project_connection_check(&self, project_id: &str, account_login: &str, repo_full_name: &str) -> std::result::Result<ConnectionCheck, ConnectionError> {
        let workspace = self.connection_workspace(project_id)?;
        let token = Self::connection_token(account_login)?;
        Ok(with_ctx(project_id, &token, |ctx| check(&workspace, account_login, repo_full_name, ctx)))
    }

    pub fn project_connection_apply(&self, request: ConnectionRequest) -> std::result::Result<ConnectionApplied, ConnectionError> {
        let workspace = self.connection_workspace(&request.project_id)?;
        let token = Self::connection_token(&request.account_login)?;
        let applied = with_ctx(&request.project_id, &token, |ctx| apply(&workspace, &request.account_login, &request.target, ctx))?;
        self.replace_active(&applied.workspace);
        Ok(applied)
    }

    pub fn project_connection_set_local(&self, project_id: &str) -> std::result::Result<WorkspaceProject, ConnectionError> {
        let workspace = self.connection_workspace(project_id)?;
        let next = set_local(&workspace, &kota_home()).map_err(|error| ConnectionError::new("save_failed", error.to_string()))?;
        self.replace_active(&next);
        Ok(next)
    }

    pub fn project_connection_status(&self, project_id: &str) -> std::result::Result<ConnectionStatus, ConnectionError> {
        let workspace = self.connection_workspace(project_id)?;
        let store = crate::github_accounts::read_store(&kota_home()).ok().flatten().unwrap_or_default();
        Ok(status(&workspace, &store, &crate::github_accounts::GhCli))
    }
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
