use super::*;
use std::cell::RefCell;

/// GitHub for one account, played by bare repositories under `base`. No network, no tokens.
struct FakeApi {
    base: PathBuf,
    login: String,
    repos: RefCell<Vec<(String, bool, bool)>>, // full name, can push, archived
    network_down: bool,
    broken_push: bool,
}

impl FakeApi {
    fn new(base: &Path) -> Self {
        Self { base: base.to_owned(), login: "fable-copper".into(), repos: RefCell::default(), network_down: false, broken_push: false }
    }
    fn bare(&self, full: &str) -> PathBuf {
        self.base.join(format!("{full}.git"))
    }
    fn add(&self, full: &str, can_push: bool, archived: bool) -> PathBuf {
        let bare = self.bare(full);
        fs::create_dir_all(&bare).unwrap();
        run(&bare, &["init", "-q", "--bare", "-b", "main"]);
        self.repos.borrow_mut().push((full.to_owned(), can_push, archived));
        bare
    }
    fn api_repo(&self, full: &str, can_push: bool, archived: bool) -> ApiRepo {
        let (owner, name) = full.split_once('/').unwrap();
        ApiRepo { full_name: full.into(), name: name.into(), owner: owner.into(), private: true, default_branch: "main".into(), can_push, archived }
    }
}

impl Api for FakeApi {
    fn repo(&self, full_name: &str) -> std::result::Result<ApiRepo, ApiError> {
        if self.network_down {
            return Err(ApiError::Network("network error: offline".into()));
        }
        let repos = self.repos.borrow();
        let found = repos.iter().find(|(full, ..)| full.eq_ignore_ascii_case(full_name)).ok_or(ApiError::NotFound)?;
        Ok(self.api_repo(&found.0, found.1, found.2))
    }
    fn repos(&self) -> std::result::Result<Vec<ApiRepo>, ApiError> {
        Ok(self.repos.borrow().iter().map(|(full, push, archived)| self.api_repo(full, *push, *archived)).collect())
    }
    fn create(&self, name: &str, _private: bool) -> std::result::Result<ApiRepo, ApiError> {
        let full = format!("{}/{name}", self.login);
        if self.repos.borrow().iter().any(|(existing, ..)| existing == &full) {
            return Err(ApiError::Rejected("HTTP 422: name already exists on this account".into()));
        }
        self.add(&full, true, false);
        Ok(self.api_repo(&full, true, false))
    }
    fn git_url(&self, full_name: &str) -> String {
        if self.broken_push {
            return path_str(&self.base.join("not-a-repository"));
        }
        path_str(&self.bare(full_name))
    }
}

fn run(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git").current_dir(dir).args(args)
        .env("GIT_AUTHOR_NAME", "Fable").env("GIT_AUTHOR_EMAIL", "fable@example.test")
        .env("GIT_COMMITTER_NAME", "Fable").env("GIT_COMMITTER_EMAIL", "fable@example.test")
        .output().unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn commit(dir: &Path, file: &str) {
    fs::write(dir.join(file), file).unwrap();
    run(dir, &["add", file]);
    run(dir, &["-c", "commit.gpgSign=false", "commit", "-q", "-m", file]);
}

struct Fixture {
    base: PathBuf,
    api: FakeApi,
    workspace: WorkspaceProject,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let base = std::env::temp_dir().join(format!("kota-connection-{label}-{}", Uuid::new_v4().simple()));
        let workspace = materialize_local_workspace("Field Notes", &base.join("Workspaces"), &base.join("Projects")).unwrap();
        commit(Path::new(&workspace.source_dir), "notes.md");
        let api = FakeApi::new(&base.join("github"));
        Self { base, api, workspace }
    }
    fn source(&self) -> &Path {
        Path::new(&self.workspace.source_dir)
    }
    fn ctx<'a>(&'a self, others: &'a [WorkspaceProject], limit: u64) -> Ctx<'a> {
        Ctx { api: &self.api, token: "fixture-token", others, kota_home: &self.base, large_file_bytes: limit }
    }
    fn saved(&self) -> Vec<u8> {
        fs::read(Path::new(&self.workspace.local_root).join("workspace.json")).unwrap()
    }
    fn helpers(&self) -> Vec<String> {
        // Not trimmed: the first helper is the empty value.
        let output = Command::new("git").current_dir(self.source()).args(["config", "--local", "--get-all", GITHUB_HELPER_KEY]).output().unwrap();
        String::from_utf8_lossy(&output.stdout).lines().map(str::to_owned).collect()
    }
    /// A clone of the fake remote whose `main` holds `commits` of this project's history plus `extra`.
    fn seed(&self, full: &str, from: &str, extra: Option<&str>) {
        let bare = self.api.add(full, true, false);
        run(self.source(), &["push", "-q", &path_str(&bare), &format!("{from}:refs/heads/main")]);
        if let Some(file) = extra {
            let peer = self.base.join(format!("peer-{}", Uuid::new_v4().simple()));
            run(&self.base, &["clone", "-q", &path_str(&bare), &path_str(&peer)]);
            commit(&peer, file);
            run(&peer, &["push", "-q", "origin", "main"]);
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

#[test]
fn every_history_and_access_state_is_told_apart_without_changing_the_project() {
    let f = Fixture::new("states");
    f.api.add("fable-copper/empty", true, false);
    f.seed("fable-copper/same", "HEAD", None);
    f.seed("fable-copper/ahead", "HEAD~1", None);
    f.seed("fable-copper/behind", "HEAD", Some("remote.md"));
    f.seed("fable-copper/diverged", "HEAD~1", Some("remote-side.md"));
    let stranger = f.base.join("stranger");
    fs::create_dir_all(&stranger).unwrap();
    run(&stranger, &["init", "-q", "-b", "main"]);
    commit(&stranger, "README.md");
    let unrelated = f.api.add("fable-copper/unrelated", true, false);
    run(&stranger, &["push", "-q", &path_str(&unrelated), "HEAD:refs/heads/main"]);
    f.api.add("fable-indigo/read-only", false, false);
    f.api.add("fable-copper/archived", true, true);
    let mut taken = f.workspace.clone();
    taken.project_id = "fable-copper-taken".into();
    taken.connection = Some(ProjectConnection::Github { account_login: Some("fable-copper".into()) });
    taken.repo_full_name = "fable-copper/same".into();
    let others = [taken];
    let head = run(f.source(), &["rev-parse", "HEAD"]);
    let before = f.saved();
    let state = |full: &str, others: &[WorkspaceProject]| check(&f.workspace, "fable-copper", full, &f.ctx(others, LARGE_FILE_BYTES)).state;
    for (full, expected) in [
        ("fable-copper/empty", "empty"),
        ("fable-copper/same", "in_sync"),
        ("fable-copper/ahead", "local_ahead"),
        ("fable-copper/behind", "remote_ahead"),
        ("fable-copper/diverged", "diverged"),
        ("fable-copper/unrelated", "unrelated"),
        ("fable-indigo/read-only", "no_access"),
        ("fable-copper/archived", "no_access"),
        ("fable-copper/missing", "no_access"),
    ] {
        assert_eq!(state(full, &[]), expected, "{full}");
    }
    assert_eq!(state("fable-copper/same", &others), "already_open");
    let offline = FakeApi { network_down: true, ..FakeApi::new(&f.base.join("github")) };
    let checked = check(&f.workspace, "fable-copper", "fable-copper/same", &Ctx { api: &offline, ..f.ctx(&[], LARGE_FILE_BYTES) });
    assert_eq!((checked.state, checked.message.as_deref()), ("check_failed", Some("network error: offline")));
    assert_eq!(run(f.source(), &["rev-parse", "HEAD"]), head);
    assert!(run(f.source(), &["for-each-ref", "refs/kota", "refs/remotes"]).is_empty(), "probes leave no refs");
    assert_eq!(f.saved(), before);
}

#[test]
fn files_over_the_limit_block_only_states_that_would_push_them() {
    let f = Fixture::new("large");
    f.api.add("fable-copper/empty", true, false);
    f.seed("fable-copper/behind", "HEAD", Some("remote.md"));
    // ".gitignore" and "notes.md" exceed an eight-byte test limit; GitHub's is 100 MB.
    let blocked = check(&f.workspace, "fable-copper", "fable-copper/empty", &f.ctx(&[], 8));
    assert_eq!(blocked.state, "blocked_large_file");
    assert_eq!(blocked.large_files, [LargeFile { path: ".gitignore".into(), bytes: 10 }]);
    assert_eq!(check(&f.workspace, "fable-copper", "fable-copper/behind", &f.ctx(&[], 8)).state, "remote_ahead");
    let error = apply(&f.workspace, "fable-copper", &ConnectionTarget::New { name: "notes".into(), private: true }, &f.ctx(&[], 8)).unwrap_err();
    assert_eq!((error.code, error.large_files.len(), error.created_repo), ("blocked_large_file", 1, None));
    assert!(f.api.repos.borrow().iter().all(|(full, ..)| full != "fable-copper/notes"), "nothing is created first");
}

#[test]
fn connecting_to_a_new_repository_pushes_first_and_then_writes_everything_once() {
    let f = Fixture::new("new");
    let applied = apply(&f.workspace, "fable-copper", &ConnectionTarget::New { name: "field-notes".into(), private: true }, &f.ctx(&[], LARGE_FILE_BYTES)).unwrap();
    let bare = f.api.bare("fable-copper/field-notes");
    assert!(applied.pushed);
    assert_eq!(applied.created_repo.as_deref(), Some("fable-copper/field-notes"));
    assert_eq!(run(&bare, &["rev-parse", "main"]), run(f.source(), &["rev-parse", "HEAD"]));
    assert_eq!(run(f.source(), &["remote", "get-url", "origin"]), path_str(&bare));
    assert_eq!(run(f.source(), &["rev-parse", "--abbrev-ref", "@{u}"]), "origin/main");
    let saved: WorkspaceProject = serde_json::from_slice(&f.saved()).unwrap();
    assert_eq!(saved.connection, Some(ProjectConnection::Github { account_login: Some("fable-copper".into()) }));
    assert_eq!((saved.repo_full_name.as_str(), saved.base_ref.as_str(), saved.display_name.as_deref()), ("fable-copper/field-notes", "origin/main", Some("Field Notes")));
    assert_eq!(saved.project_id, f.workspace.project_id, "the project keeps its id");
    assert_eq!(f.helpers(), ["".to_owned(), crate::github_accounts::credential_helper(&f.base, &f.workspace.project_id)]);
    assert_eq!(serde_json::to_value(&applied.workspace).unwrap(), serde_json::to_value(&saved).unwrap());
}

#[test]
fn a_failed_push_leaves_the_project_as_it_was_and_names_the_created_repository() {
    let mut f = Fixture::new("push-fails");
    f.api.broken_push = true;
    let before = (f.saved(), f.helpers());
    let error = apply(&f.workspace, "fable-copper", &ConnectionTarget::New { name: "field-notes".into(), private: true }, &f.ctx(&[], LARGE_FILE_BYTES)).unwrap_err();
    assert_eq!((error.code, error.created_repo.as_deref()), ("push_failed", Some("fable-copper/field-notes")));
    assert_eq!((f.saved(), f.helpers()), before);
    assert!(Command::new("git").current_dir(f.source()).args(["remote", "get-url", "origin"]).output().unwrap().stdout.is_empty());
    f.api.broken_push = false;
    let taken = apply(&f.workspace, "fable-copper", &ConnectionTarget::New { name: "field-notes".into(), private: true }, &f.ctx(&[], LARGE_FILE_BYTES)).unwrap_err();
    assert_eq!((taken.code, taken.created_repo), ("repo_create_failed", None));
}

#[test]
fn remote_ahead_connects_without_pushing_and_leaves_the_pull_to_bartender() {
    let f = Fixture::new("behind");
    f.seed("fable-copper/field-notes", "HEAD", Some("remote.md"));
    let head = run(f.source(), &["rev-parse", "HEAD"]);
    let applied = apply(&f.workspace, "fable-copper", &ConnectionTarget::Existing { repo_full_name: "fable-copper/field-notes".into() }, &f.ctx(&[], LARGE_FILE_BYTES)).unwrap();
    assert!(!applied.pushed);
    assert_eq!(run(f.source(), &["rev-parse", "HEAD"]), head, "nothing is merged here");
    assert_eq!(run(f.source(), &["rev-parse", "origin/main"]), run(&f.api.bare("fable-copper/field-notes"), &["rev-parse", "main"]));
    assert_eq!(run(f.source(), &["rev-list", "--count", "HEAD..@{u}"]), "1");
}

#[test]
fn a_target_that_cannot_connect_changes_nothing() {
    let f = Fixture::new("refused");
    let stranger = f.base.join("stranger");
    fs::create_dir_all(&stranger).unwrap();
    run(&stranger, &["init", "-q", "-b", "main"]);
    commit(&stranger, "README.md");
    let bare = f.api.add("fable-copper/unrelated", true, false);
    run(&stranger, &["push", "-q", &path_str(&bare), "HEAD:refs/heads/main"]);
    let before = (f.saved(), f.helpers());
    let error = apply(&f.workspace, "fable-copper", &ConnectionTarget::Existing { repo_full_name: "fable-copper/unrelated".into() }, &f.ctx(&[], LARGE_FILE_BYTES)).unwrap_err();
    assert_eq!((error.code, error.state), ("not_connectable", Some("unrelated")));
    assert_eq!((f.saved(), f.helpers()), before);
    assert_eq!(run(&bare, &["log", "--format=%s", "main"]), "README.md", "the remote is untouched");
}

#[test]
fn changing_account_on_the_same_repository_changes_only_the_account_and_going_local_keeps_origin() {
    let f = Fixture::new("switch");
    let connected = apply(&f.workspace, "fable-copper", &ConnectionTarget::New { name: "field-notes".into(), private: true }, &f.ctx(&[], LARGE_FILE_BYTES)).unwrap().workspace;
    let switched = apply(&connected, "fable-indigo", &ConnectionTarget::Existing { repo_full_name: "fable-copper/field-notes".into() }, &f.ctx(&[], LARGE_FILE_BYTES)).unwrap();
    assert!(!switched.pushed);
    assert_eq!(switched.workspace.connection, Some(ProjectConnection::Github { account_login: Some("fable-indigo".into()) }));
    assert_eq!((switched.workspace.remote_url.as_str(), switched.workspace.repo_full_name.as_str()), (connected.remote_url.as_str(), "fable-copper/field-notes"));
    let local = set_local(&switched.workspace, &f.base).unwrap();
    assert!(local.is_local());
    assert_eq!((local.repo_full_name.as_str(), local.remote_url.as_str(), local.base_ref.as_str()), ("", "", "main"));
    assert_eq!(local.display_name.as_deref(), Some("Field Notes"));
    assert_eq!(f.helpers(), ["", LOCAL_ONLY_HELPER]);
    assert_eq!(run(f.source(), &["remote", "get-url", "origin"]), connected.remote_url, "origin stays for reconnecting");
    let offered = candidates(&local, "fable-copper", &f.ctx(&[], LARGE_FILE_BYTES)).unwrap();
    assert_eq!(offered.candidates[0].full_name, "fable-copper/field-notes");
    assert!(offered.candidates[0].previous);
    assert_eq!(offered.selected.as_deref(), Some("fable-copper/field-notes"));
}

#[test]
fn candidates_preselect_a_repository_named_like_the_project() {
    let f = Fixture::new("candidates");
    f.api.add("fable-copper/other", true, false);
    f.api.add("fable-copper/Field-Notes", true, false);
    f.api.add("fable-indigo/field-notes", true, false);
    let offered = candidates(&f.workspace, "fable-copper", &f.ctx(&[], LARGE_FILE_BYTES)).unwrap();
    assert_eq!(offered.selected.as_deref(), Some("fable-copper/Field-Notes"));
    assert!(offered.candidates.iter().all(|c| !c.previous));
    assert!(offered.large_files.is_empty());
}

#[test]
fn status_names_the_project_mode_and_its_account_state() {
    use crate::github_accounts::{tests::FakeGh, Account, Store};
    let f = Fixture::new("status");
    let store = Store { schema: 1, default_login: Some("fable-copper".into()), accounts: vec![Account { login: "fable-copper".into(), added_at: String::new() }] };
    let gh = FakeGh::with(&["fable-copper"], Some("fable-copper"));
    assert_eq!(status(&f.workspace, &store, &gh), ConnectionStatus::Local);
    let mut github = f.workspace.clone();
    github.remote_url = "https://github.com/fable-copper/field-notes.git".into();
    let mut state = |login: Option<&str>, store: &Store, gh: &FakeGh| {
        github.connection = Some(ProjectConnection::Github { account_login: login.map(str::to_owned) });
        status(&github, store, gh)
    };
    let github_status = |login: &str, account_state| ConnectionStatus::Github { account_login: Some(login.into()), account_state };
    assert_eq!(state(Some("fable-copper"), &store, &gh), github_status("fable-copper", "ok"));
    assert_eq!(state(Some("removed"), &store, &gh), github_status("removed", "missing"));
    assert_eq!(state(Some("fable-copper"), &store, &FakeGh::default()), github_status("fable-copper", "needs_login"));
    assert_eq!(state(None, &Store::default(), &gh), ConnectionStatus::Github { account_login: None, account_state: "unbound" });
}
