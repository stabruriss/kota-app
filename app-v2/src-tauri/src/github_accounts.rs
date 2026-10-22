//! GitHub accounts for projects (local-project PLAN §1.5, §2.2–§2.4).
//!
//! gh holds every token. Kota records only the accounts it uses, the default for
//! new projects and, in each workspace.json, the account a project is bound to.
//! Every Kota path names its account (`gh auth token --user`), never gh's active
//! one, so switching gh elsewhere cannot move a project to another account.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::integrations::ProjectConnection;

pub const HOST: &str = "github.com";
const GH_CANDIDATES: [&str; 3] = ["gh", "/opt/homebrew/bin/gh", "/usr/local/bin/gh"];

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Store {
    pub schema: u8,
    #[serde(default)]
    pub default_login: Option<String>,
    #[serde(default)]
    pub accounts: Vec<Account>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub login: String,
    pub added_at: String,
}

impl Store {
    fn has(&self, login: &str) -> bool {
        self.accounts.iter().any(|account| account.login == login)
    }
}

/// What gh knows. Tests replace it; the app uses [`GhCli`].
pub trait Gh {
    /// The token gh stores for this login, without asking the network.
    fn token(&self, login: &str) -> Option<String>;
    fn logins(&self) -> Result<GhLogins>;
    fn switch(&self, login: &str) -> Result<()>;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GhLogins {
    pub logins: Vec<String>,
    pub active: Option<String>,
}

pub struct GhCli;

impl GhCli {
    fn run(args: &[&str]) -> Result<std::process::Output> {
        let mut missing = Vec::new();
        for candidate in GH_CANDIDATES {
            match Command::new(candidate).args(args).env("GH_PROMPT_DISABLED", "1").output() {
                Ok(output) => return Ok(output),
                Err(error) => missing.push(format!("{candidate}: {error}")),
            }
        }
        bail!("gh executable not found ({})", missing.join("; "))
    }
}

impl Gh for GhCli {
    fn token(&self, login: &str) -> Option<String> {
        let output = Self::run(&["auth", "token", "--hostname", HOST, "--user", login]).ok()?;
        let token = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        (output.status.success() && !token.is_empty()).then_some(token)
    }

    fn logins(&self) -> Result<GhLogins> {
        let output = Self::run(&["auth", "status", "--hostname", HOST])?;
        Ok(parse_gh_status(&format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )))
    }

    fn switch(&self, login: &str) -> Result<()> {
        let output = Self::run(&["auth", "switch", "--hostname", HOST, "--user", login])?;
        if !output.status.success() {
            bail!("gh auth switch failed: {}", String::from_utf8_lossy(&output.stderr).trim());
        }
        Ok(())
    }
}

/// `gh auth status` lists each account as "Logged in to github.com account <login>"
/// followed by its own "- Active account: true|false" line.
pub fn parse_gh_status(text: &str) -> GhLogins {
    let mut parsed = GhLogins::default();
    for line in text.lines().map(str::trim) {
        if let Some(rest) = line.split("Logged in to github.com account ").nth(1) {
            if let Some(login) = rest.split_whitespace().next().map(strip_ansi) {
                parsed.logins.push(login);
            }
        } else if line.contains("Active account: true") {
            parsed.active = parsed.logins.last().cloned();
        }
    }
    parsed
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

pub fn kota_home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join("Kota")
}

fn store_path(kota_home: &Path) -> PathBuf {
    kota_home.join("github").join("accounts.json")
}

pub fn read_store(kota_home: &Path) -> Result<Option<Store>> {
    match fs::read(store_path(kota_home)) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes).context("read GitHub accounts")?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("read GitHub accounts"),
    }
}

pub fn write_store(kota_home: &Path, store: &Store) -> Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let path = store_path(kota_home);
    let dir = path.parent().expect("accounts file has a directory");
    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    let temporary = dir.join(format!(".accounts-{}.tmp", uuid::Uuid::new_v4()));
    let written = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(store)?)?;
        file.sync_all()?;
        fs::rename(&temporary, &path)?;
        Ok(())
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

/// First use after upgrade takes over gh's active login as the default (老无, item 6),
/// so existing projects keep pushing without a new sign-in. Nothing is written while
/// gh has no login; the first account added later becomes the default instead.
pub fn ensure_store(kota_home: &Path, gh: &dyn Gh) -> Result<Store> {
    if let Some(store) = read_store(kota_home)? {
        return Ok(store);
    }
    let Some(active) = gh.logins()?.active else {
        return Ok(Store { schema: 1, ..Store::default() });
    };
    let store = Store {
        schema: 1,
        default_login: Some(active.clone()),
        accounts: vec![Account { login: active, added_at: chrono::Utc::now().to_rfc3339() }],
    };
    write_store(kota_home, &store)?;
    Ok(store)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    pub login: String,
    pub default: bool,
    /// `needs_login` once gh no longer holds a token for this login.
    pub state: &'static str,
    pub source: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountsView {
    pub default_login: Option<String>,
    pub accounts: Vec<AccountView>,
}

pub fn view(store: &Store, gh: &dyn Gh) -> AccountsView {
    AccountsView {
        default_login: store.default_login.clone(),
        accounts: store
            .accounts
            .iter()
            .map(|account| AccountView {
                login: account.login.clone(),
                default: store.default_login.as_deref() == Some(&account.login),
                state: if gh.token(&account.login).is_some() { "ok" } else { "needs_login" },
                source: "gh-cli",
            })
            .collect(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddResult {
    pub added: Vec<String>,
    /// gh's own active account after the login; Kota switches it back to `previous`.
    pub active: Option<String>,
    pub restore_failed: bool,
    pub view: AccountsView,
}

/// After `gh auth login` in the Smart Terminal: record the new logins and switch gh's
/// active account back, so the user's own gh elsewhere is unchanged. A failed switch
/// only affects gh outside Kota; every Kota path names its account.
pub fn finish_add(kota_home: &Path, gh: &dyn Gh, previous_active: Option<&str>) -> Result<AddResult> {
    let mut store = ensure_store(kota_home, gh)?;
    let now = gh.logins()?;
    let added: Vec<String> = now.logins.iter().filter(|login| !store.has(login)).cloned().collect();
    for login in &added {
        store.accounts.push(Account { login: login.clone(), added_at: chrono::Utc::now().to_rfc3339() });
    }
    if store.default_login.is_none() {
        store.default_login = added.first().cloned();
    }
    if !added.is_empty() {
        write_store(kota_home, &store)?;
    }
    let mut active = now.active;
    let mut restore_failed = false;
    if let Some(previous) = previous_active.filter(|previous| active.as_deref() != Some(*previous) && now.logins.iter().any(|l| l == previous)) {
        match gh.switch(previous) {
            Ok(()) => active = Some(previous.to_owned()),
            Err(_) => restore_failed = true,
        }
    }
    Ok(AddResult { added, active, restore_failed, view: view(&store, gh) })
}

pub fn set_default(kota_home: &Path, login: &str) -> Result<Store> {
    let mut store = read_store(kota_home)?.unwrap_or(Store { schema: 1, ..Store::default() });
    if !store.has(login) {
        bail!("github_account_unknown");
    }
    store.default_login = Some(login.to_owned());
    write_store(kota_home, &store)?;
    Ok(store)
}

/// Projects bound to a removed account keep their binding and show "account missing"
/// (老无, item 7); gh itself keeps the login for the user's own use.
pub fn remove(kota_home: &Path, login: &str) -> Result<Store> {
    let mut store = read_store(kota_home)?.unwrap_or(Store { schema: 1, ..Store::default() });
    store.accounts.retain(|account| account.login != login);
    if store.default_login.as_deref() == Some(login) {
        store.default_login = store.accounts.first().map(|account| account.login.clone());
    }
    write_store(kota_home, &store)?;
    Ok(store)
}

/// The account a GitHub project uses: its binding while that account is still in
/// Kota, or the default for a registration that predates bindings. None means the
/// project may not reach GitHub (local, or its account is missing).
pub fn project_login(store: &Store, connection: Option<&ProjectConnection>, has_remote: bool) -> Option<String> {
    match connection {
        Some(ProjectConnection::Local) => None,
        Some(ProjectConnection::Github { account_login: Some(login) }) => store.has(login).then(|| login.clone()),
        Some(ProjectConnection::Github { account_login: None }) => store.default_login.clone(),
        None => has_remote.then(|| store.default_login.clone()).flatten(),
    }
}

/// `credential.https://github.com.helper` value for a GitHub project. The absolute
/// shim path also works in a user's own terminal, where ~/Kota/bin may not be on PATH.
pub fn credential_helper(kota_home: &Path, project_id: &str) -> String {
    format!("!\"{}\" git-credential --project {project_id}", kota_home.join("bin").join("kota-bartender").display())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Registration {
    #[serde(default)]
    remote_url: String,
    #[serde(default)]
    connection: Option<ProjectConnection>,
}

/// The git credential helper (`kota-bartender git-credential --project <id> get`).
/// Answers only `get`; with no usable account it says `quit`, so git fails at once
/// instead of trying another helper or waiting at a prompt.
pub fn credential_reply(kota_home: &Path, project_id: &str, gh: &dyn Gh) -> String {
    let reply = (|| {
        crate::bbs_sync::safe_id(project_id).ok()?;
        let path = kota_home.join("Workspaces").join(project_id).join("workspace.json");
        let registration: Registration = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
        if registration.connection == Some(ProjectConnection::Local) {
            return None;
        }
        // A push can come before the startup takeover; take over gh's login here too.
        let store = ensure_store(kota_home, gh).ok()?;
        let login = project_login(&store, registration.connection.as_ref(), !registration.remote_url.trim().is_empty())?;
        let token = gh.token(&login)?;
        Some(format!("username={login}\npassword={token}\n"))
    })();
    reply.unwrap_or_else(|| "quit=1\n".to_owned())
}

pub fn run_credential_cli(args: &[String]) -> Result<()> {
    let project = args
        .iter()
        .position(|arg| arg == "--project")
        .and_then(|index| args.get(index + 1))
        .context("git-credential needs --project <id>")?;
    let mut request = String::new();
    std::io::stdin().read_to_string(&mut request)?;
    if args.last().map(String::as_str) == Some("get") {
        print!("{}", credential_reply(&kota_home(), project, &GhCli));
    }
    Ok(())
}

#[cfg(test)]
#[path = "github_accounts_tests.rs"]
pub(crate) mod tests;
