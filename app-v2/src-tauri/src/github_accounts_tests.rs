use super::*;
use std::cell::RefCell;
use std::collections::HashMap;

/// gh without a keychain or network. Tokens are fixtures, never real credentials.
#[derive(Default)]
pub(crate) struct FakeGh {
    pub tokens: HashMap<String, String>,
    pub status: GhLogins,
    pub switch_fails: bool,
    pub switched: RefCell<Vec<String>>,
}

impl FakeGh {
    pub(crate) fn with(logins: &[&str], active: Option<&str>) -> Self {
        Self {
            tokens: logins.iter().map(|login| (login.to_string(), format!("fixture-token-{login}"))).collect(),
            status: GhLogins { logins: logins.iter().map(|login| login.to_string()).collect(), active: active.map(str::to_owned) },
            ..Self::default()
        }
    }
}

impl Gh for FakeGh {
    fn token(&self, login: &str) -> Option<String> {
        self.tokens.get(login).cloned()
    }
    fn logins(&self) -> Result<GhLogins> {
        Ok(self.status.clone())
    }
    fn switch(&self, login: &str) -> Result<()> {
        if self.switch_fails {
            bail!("switch failed");
        }
        self.switched.borrow_mut().push(login.to_owned());
        Ok(())
    }
}

fn home(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kota github {label} {}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn register(home: &Path, project_id: &str, workspace: serde_json::Value) {
    let root = home.join("Workspaces").join(project_id);
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("workspace.json"), serde_json::to_vec(&workspace).unwrap()).unwrap();
}

#[test]
fn gh_status_lists_every_account_and_the_active_one_without_colour_codes() {
    let text = "github.com\n  \u{1b}[0;32m✓\u{1b}[0m Logged in to github.com account \u{1b}[0;1;39mfable-copper\u{1b}[0m (keyring)\n  - Active account: \u{1b}[0;1;39mfalse\u{1b}[0m\n\
        \n  ✓ Logged in to github.com account fable-indigo (keyring)\n  - Active account: true\n  - Token: gho_************************************\n";
    assert_eq!(parse_gh_status(text), GhLogins { logins: vec!["fable-copper".into(), "fable-indigo".into()], active: Some("fable-indigo".into()) });
    assert_eq!(parse_gh_status("You are not logged into any GitHub hosts."), GhLogins::default());
}

#[test]
fn first_use_takes_over_gh_active_login_once_and_writes_nothing_without_one() {
    let home = home("store");
    let empty = ensure_store(&home, &FakeGh::default()).unwrap();
    assert_eq!((empty.default_login, empty.accounts.len()), (None, 0));
    assert!(!store_path(&home).exists());
    let store = ensure_store(&home, &FakeGh::with(&["fable-copper", "fable-indigo"], Some("fable-indigo"))).unwrap();
    assert_eq!(store.default_login.as_deref(), Some("fable-indigo"));
    assert_eq!(store.accounts.iter().map(|a| a.login.as_str()).collect::<Vec<_>>(), ["fable-indigo"]);
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(fs::metadata(store_path(&home)).unwrap().permissions().mode() & 0o777, 0o600);
    // Later reads keep the file, even once gh's active account has moved.
    assert_eq!(ensure_store(&home, &FakeGh::with(&["fable-copper"], Some("fable-copper"))).unwrap(), store);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn adding_records_new_logins_and_switches_gh_back_to_the_previous_account() {
    let home = home("add");
    ensure_store(&home, &FakeGh::with(&["fable-copper"], Some("fable-copper"))).unwrap();
    // gh made the new login active; Kota restores the one the user had.
    let gh = FakeGh::with(&["fable-copper", "fable-indigo"], Some("fable-indigo"));
    let result = finish_add(&home, &gh, Some("fable-copper")).unwrap();
    assert_eq!(result.added, ["fable-indigo"]);
    assert_eq!((result.active.as_deref(), result.restore_failed), (Some("fable-copper"), false));
    assert_eq!(*gh.switched.borrow(), ["fable-copper"]);
    assert_eq!(result.view.default_login.as_deref(), Some("fable-copper"), "adding never moves the default");
    // Signing in again to a known login adds nothing.
    assert!(finish_add(&home, &FakeGh::with(&["fable-copper", "fable-indigo"], Some("fable-copper")), Some("fable-copper")).unwrap().added.is_empty());
    let failing = FakeGh { switch_fails: true, ..FakeGh::with(&["fable-copper", "fable-indigo", "fable-amber"], Some("fable-amber")) };
    let failed = finish_add(&home, &failing, Some("fable-copper")).unwrap();
    assert_eq!((failed.added.as_slice(), failed.active.as_deref(), failed.restore_failed), (&["fable-amber".to_owned()][..], Some("fable-amber"), true));
    let _ = fs::remove_dir_all(home);
}

#[test]
fn the_first_account_added_becomes_the_default_and_removing_it_moves_the_default() {
    let home = home("default");
    let result = finish_add(&home, &FakeGh::with(&["fable-copper"], Some("fable-copper")), None).unwrap();
    assert_eq!(result.view.default_login.as_deref(), Some("fable-copper"));
    finish_add(&home, &FakeGh::with(&["fable-copper", "fable-indigo"], Some("fable-indigo")), None).unwrap();
    assert_eq!(set_default(&home, "fable-indigo").unwrap().default_login.as_deref(), Some("fable-indigo"));
    assert_eq!(set_default(&home, "stranger").unwrap_err().to_string(), "github_account_unknown");
    let store = remove(&home, "fable-indigo").unwrap();
    assert_eq!((store.default_login.as_deref(), store.accounts.len()), (Some("fable-copper"), 1));
    let gh = FakeGh::with(&["fable-indigo"], None);
    assert_eq!(view(&store, &gh).accounts[0].state, "needs_login", "gh no longer holds this login");
    let _ = fs::remove_dir_all(home);
}

#[test]
fn a_project_uses_its_own_account_the_default_only_when_unbound_and_never_a_removed_one() {
    let store = Store {
        schema: 1,
        default_login: Some("fable-copper".into()),
        accounts: ["fable-copper", "fable-indigo"].map(|login| Account { login: login.into(), added_at: String::new() }).to_vec(),
    };
    let bound = |login: &str| ProjectConnection::Github { account_login: Some(login.into()) };
    assert_eq!(project_login(&store, Some(&bound("fable-indigo")), true).as_deref(), Some("fable-indigo"));
    assert_eq!(project_login(&store, Some(&bound("removed")), true), None);
    assert_eq!(project_login(&store, Some(&ProjectConnection::Github { account_login: None }), true).as_deref(), Some("fable-copper"));
    assert_eq!(project_login(&store, None, true).as_deref(), Some("fable-copper"));
    assert_eq!(project_login(&store, None, false), None);
    assert_eq!(project_login(&store, Some(&ProjectConnection::Local), true), None);
}

#[test]
fn the_credential_helper_answers_with_the_project_account_or_quits() {
    let home = home("credential");
    let gh = FakeGh::with(&["fable-copper", "fable-indigo"], Some("fable-copper"));
    finish_add(&home, &gh, None).unwrap();
    register(&home, "owner-repo", serde_json::json!({"remoteUrl":"https://github.com/owner/repo.git","connection":{"kind":"github","accountLogin":"fable-indigo"}}));
    register(&home, "owner-legacy", serde_json::json!({"remoteUrl":"https://github.com/owner/legacy.git"}));
    register(&home, "owner-removed", serde_json::json!({"remoteUrl":"x","connection":{"kind":"github","accountLogin":"removed"}}));
    register(&home, "local_notes", serde_json::json!({"remoteUrl":"","connection":{"kind":"local"}}));
    assert_eq!(credential_reply(&home, "owner-repo", &gh), "username=fable-indigo\npassword=fixture-token-fable-indigo\n");
    assert_eq!(credential_reply(&home, "owner-legacy", &gh), "username=fable-copper\npassword=fixture-token-fable-copper\n");
    for project in ["owner-removed", "local_notes", "missing", "../owner-repo"] {
        assert_eq!(credential_reply(&home, project, &gh), "quit=1\n", "{project}");
    }
    let signed_out = FakeGh::with(&["fable-copper"], Some("fable-copper"));
    assert_eq!(credential_reply(&home, "owner-repo", &signed_out), "quit=1\n");
    let _ = fs::remove_dir_all(home);
}

#[test]
fn a_push_before_the_startup_takeover_takes_over_gh_login_first() {
    let home = home("credential-first");
    register(&home, "owner-repo", serde_json::json!({"remoteUrl":"https://github.com/owner/repo.git"}));
    register(&home, "local_notes", serde_json::json!({"remoteUrl":"","connection":{"kind":"local"}}));
    let gh = FakeGh::with(&["fable-copper"], Some("fable-copper"));
    assert_eq!(credential_reply(&home, "local_notes", &gh), "quit=1\n");
    assert!(read_store(&home).unwrap().is_none(), "a local project never takes over gh");
    assert_eq!(credential_reply(&home, "owner-repo", &gh), "username=fable-copper\npassword=fixture-token-fable-copper\n");
    assert_eq!(read_store(&home).unwrap().unwrap().default_login.as_deref(), Some("fable-copper"));
    let _ = fs::remove_dir_all(home);
}

#[test]
fn git_runs_the_helper_command_even_from_a_path_with_spaces() {
    let home = home("helper path");
    let bin = home.join("bin");
    fs::create_dir_all(&bin).unwrap();
    // Stands in for the ~/Kota/bin shim; it echoes its own arguments as the answer.
    let shim = bin.join("kota-bartender");
    fs::write(&shim, "#!/bin/sh\ncat >/dev/null\necho username=fixture\necho \"password=$*\"\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
    let repo = home.join("repo");
    fs::create_dir_all(&repo).unwrap();
    assert!(Command::new("git").current_dir(&repo).args(["init", "-q"]).status().unwrap().success());
    let helper = credential_helper(&home, "owner-repo");
    for value in ["", helper.as_str()] {
        assert!(Command::new("git").current_dir(&repo).args(["config", "--local", "--add", "credential.https://github.com.helper", value]).status().unwrap().success());
    }
    let mut fill = Command::new("git").current_dir(&repo).args(["credential", "fill"]).env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
    fill.stdin.take().unwrap().write_all(b"protocol=https\nhost=github.com\n\n").unwrap();
    let output = String::from_utf8(fill.wait_with_output().unwrap().stdout).unwrap();
    assert!(output.contains("password=git-credential --project owner-repo get"), "{output}");
    let _ = fs::remove_dir_all(home);
}
