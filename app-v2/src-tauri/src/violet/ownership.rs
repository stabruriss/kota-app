//! Forward-only ownership checks for provider-native PTY sessions. Local RPC
//! sessions have their own scoped storage and never enter this module.
use super::*;
use crate::pty::agent::{AgentCli, AgentSpawnRequest};
use rusqlite::OptionalExtension;
use std::fmt;

pub(crate) const UNAVAILABLE: &str = "KOTA_SESSION_UNAVAILABLE:";

#[derive(Debug)]
pub(crate) struct Unavailable(pub String);
impl fmt::Display for Unavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{UNAVAILABLE} {}", self.0)
    }
}
impl std::error::Error for Unavailable {}
fn denied(reason: impl ToString) -> anyhow::Error {
    Unavailable(reason.to_string()).into()
}
fn transient(error: impl fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("Session check failed: {error}")
}
pub(crate) const RECOVERY_MESSAGE: &str =
    "The saved session can't be resumed. Agent memory is safe. Start a new session?";
pub(crate) fn display_error(error: &anyhow::Error) -> String {
    if error.is::<Unavailable>() {
        RECOVERY_MESSAGE.into()
    } else {
        error.to_string()
    }
}
pub(crate) fn availability(result: anyhow::Result<()>) -> Result<bool, String> {
    match result {
        Ok(()) => Ok(true),
        Err(error) if error.is::<Unavailable>() => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Key {
    provider: String,
    scope: PathBuf,
    session: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Receipt {
    version: u32,
    key: Key,
    owner: String,
    root: PathBuf,
    cwd: PathBuf,
    source: PathBuf,
    incarnation: String,
    evidence: String,
}

struct Store {
    dir: PathBuf,
}
impl Store {
    fn new(kota_home: &Path) -> Self {
        Self {
            dir: kota_home.join("session-owners/v1"),
        }
    }
    fn path(&self, key: &Key) -> anyhow::Result<PathBuf> {
        Ok(self.dir.join(format!(
            "{:x}.json",
            Sha256::digest(serde_json::to_vec(key)?)
        )))
    }

    // The lock is a separate, stable file: renaming the JSON must not replace
    // the inode on which another process is waiting for flock.
    fn lock(&self, key: &Key) -> anyhow::Result<OwnerLock> {
        fs::create_dir_all(&self.dir).map_err(transient)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.path(key)?.with_extension("lock"))
            .map_err(transient)?;
        #[cfg(unix)]
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(transient(std::io::Error::last_os_error()));
        }
        Ok(OwnerLock(file))
    }

    fn read(&self, key: &Key) -> anyhow::Result<Option<Receipt>> {
        let bytes = match fs::read(self.path(key)?) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(transient(e)),
        };
        let record: Receipt =
            serde_json::from_slice(&bytes).map_err(|_| denied("damaged ownership receipt"))?;
        if record.version != 1 || &record.key != key {
            return Err(denied("invalid ownership receipt"));
        }
        Ok(Some(record))
    }

    fn verify(
        &self,
        key: &Key,
        agent: &ProjectAgent,
        source: Option<(&Path, &str)>,
        evidence: impl FnOnce() -> Result<Option<String>, String>,
    ) -> anyhow::Result<()> {
        let _lock = self.lock(key)?;
        let previous = self.read(key)?;
        let cwd = canonical(&agent.cwd);
        let same_incarnation = previous
            .as_ref()
            .is_some_and(|r| source.is_none_or(|(_, stamp)| stamp == r.incarnation));
        if let Some(record) = previous.as_ref().filter(|_| same_incarnation) {
            if record.owner != agent.agent_id {
                return Err(denied("session belongs to another agent"));
            }
            if record.cwd != cwd
                && read_yaml_file(&record.cwd.join("agent.yaml"))
                    .map_err(transient)?
                    .is_some_and(|yaml| yaml_string(&yaml, "id").as_deref() == Some(&record.owner))
            {
                return Err(denied("agent identity exists in two locations"));
            }
            if record.cwd == cwd {
                return Ok(());
            }
            // A move keeps ownership. The former entity must no longer exist.
            let mut record = record.clone();
            record.cwd = cwd;
            record.root = canonical(&project_root(agent));
            return self.write(key, &record);
        }
        if let Some(old) = previous.as_ref() {
            let old_source = NativeSource {
                kind: native_source_kind_for_shell(&key.provider),
                session_id: key.session.clone(),
                path: old.source.clone(),
                aux_path: None,
            };
            if incarnation(&old_source)?.as_deref() == Some(&old.incarnation) {
                return Err(denied("two live native sources share a session identity"));
            }
        }
        let proof = evidence()
            .map_err(transient)?
            .ok_or_else(|| denied("session ownership cannot be verified"))?;
        let (path, incarnation) = source.ok_or_else(|| denied("native session is unavailable"))?;
        // Only registration/location changes read the account directory; normal
        // reuse is one small record plus one source stat, never a global scan.
        let kota_home = self.dir.parent().and_then(Path::parent).unwrap();
        if let Ok(directory) = crate::agent_directory::collect(kota_home) {
            if directory.projects.iter().any(|p| {
                p.agents.iter().any(|a| a.id == agent.agent_id)
                    && canonical(&p.root.join(".agent-workspaces").join(&agent.agent_id)) != cwd
            }) {
                return Err(denied("agent identity exists in two registered projects"));
            }
        }
        self.write(
            key,
            &Receipt {
                version: 1,
                key: key.clone(),
                owner: agent.agent_id.clone(),
                root: canonical(&project_root(agent)),
                cwd,
                source: path.to_path_buf(),
                incarnation: incarnation.into(),
                evidence: proof,
            },
        )
    }

    fn write(&self, key: &Key, record: &Receipt) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec(record).map_err(transient)?;
        crate::adapter_sync::atomic_replace(&self.path(key)?, &bytes).map_err(transient)
    }
}
struct OwnerLock(fs::File);
impl Drop for OwnerLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}
fn project_root(agent: &ProjectAgent) -> PathBuf {
    agent
        .cwd
        .parent()
        .and_then(Path::parent)
        .unwrap_or(&agent.cwd)
        .to_path_buf()
}

fn provider_scope(home: &Path, shell: &str) -> PathBuf {
    canonical(&match shell {
        "claude" => home.join(".claude"),
        "codex" => home.join(".codex"),
        "antigravity" => antigravity_app_dir(home),
        "opencode" => home.join(".local/share/opencode"),
        "pi" => home.join(".pi/agent"),
        "kimi" => kimi_code_home_from(home),
        _ => unreachable!("only PTY providers have native ownership receipts"),
    })
}
fn key(home: &Path, agent: &ProjectAgent, id: &str) -> Key {
    Key {
        provider: agent.shell.clone(),
        scope: provider_scope(home, &agent.shell),
        session: id.into(),
    }
}

// Append/mtime does not change identity. AGY's compact/full transcripts are
// alternate views of the same conversation directory. SQLite uses row birth.
fn file_exists(path: &Path) -> anyhow::Result<bool> {
    match fs::metadata(path) {
        Ok(meta) => Ok(meta.is_file()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}
fn timestamp_identity(time: SystemTime) -> anyhow::Result<String> {
    let elapsed = time.duration_since(SystemTime::UNIX_EPOCH)?;
    Ok(format!("{}:{}", elapsed.as_secs(), elapsed.subsec_nanos()))
}
pub(super) fn incarnation(source: &NativeSource) -> anyhow::Result<Option<String>> {
    if source.kind == "opencode-sqlite" {
        if !file_exists(&source.path)? {
            return Ok(None);
        }
        return opencode_session_created_checked(&source.path, &source.session_id)
            .map_err(transient)?
            .map(|time| timestamp_identity(time).map(|id| format!("sqlite:{id}")))
            .transpose();
    }
    let path = if source.kind == "antigravity-jsonl" {
        source
            .path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .ok_or_else(|| transient("invalid native source path"))?
    } else {
        &source.path
    };
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let birth = timestamp_identity(meta.created()?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(Some(format!("{}:{}:{birth}", meta.dev(), meta.ino())))
    }
    #[cfg(not(unix))]
    {
        Ok(Some(birth))
    }
}

pub(super) fn verify_source(agent: &ProjectAgent, source: &NativeSource) -> anyhow::Result<()> {
    let home = dirs::home_dir().ok_or_else(|| denied("native home is unavailable"))?;
    verify_source_in(&home, &crate::kota_home_dir(), agent, source)
}
fn verify_source_in(
    home: &Path,
    kota: &Path,
    agent: &ProjectAgent,
    source: &NativeSource,
) -> anyhow::Result<()> {
    let stamp = incarnation(source)?;
    if source.kind == "opencode-sqlite" && stamp.is_none() {
        return Store::new(kota).verify(&key(home, agent, &source.session_id), agent, None, || {
            Ok(None)
        });
    }
    let stamp = stamp.ok_or_else(|| denied("native session is unavailable"))?;
    Store::new(kota).verify(
        &key(home, agent, &source.session_id),
        agent,
        Some((&source.path, &stamp)),
        || native_evidence(home, agent, source),
    )
}

fn native_evidence(
    home: &Path,
    agent: &ProjectAgent,
    source: &NativeSource,
) -> Result<Option<String>, String> {
    let valid = match agent.shell.as_str() {
        "claude" => {
            source
                .path
                .parent()
                .is_some_and(|p| paths_same(p, &claude_project_dir(home, &agent.cwd)))
                && file_stem(&source.path).as_deref() == Some(&source.session_id)
        }
        "codex" => read_codex_session_meta(&source.path)?.is_some_and(|m| {
            !m.is_subagent && m.session_id == source.session_id && paths_same(&m.cwd, &agent.cwd)
        }),
        "pi" => {
            pi_session_header_matches(&source.path, Some(&source.session_id), Some(&agent.cwd))?
        }
        "kimi" => match source
            .path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
        {
            Some(dir) => kimi_state_matches_cwd(dir, &agent.cwd)?,
            None => false,
        },
        "opencode" => {
            let conn = open_opencode_db(&source.path)?;
            let directory: Option<String> = conn
                .query_row(
                    "select directory from session where id = ?1",
                    [&source.session_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            directory.is_some_and(|d| paths_same(Path::new(&d), &agent.cwd))
        }
        "antigravity" => agy_evidence(
            &provider_scope(home, "antigravity"),
            agent,
            &source.session_id,
        )?,
        _ => false,
    };
    Ok(valid.then(|| format!("{}:native-workspace", agent.shell)))
}

fn agy_evidence(app: &Path, agent: &ProjectAgent, session: &str) -> Result<bool, String> {
    let candidates = antigravity_cwd_candidates(agent);
    let index_path = app.join("cache/last_conversations.json");
    let mut matched = false;
    if file_exists(&index_path).map_err(|error| error.to_string())? {
        let index: JsonValue =
            serde_json::from_slice(&fs::read(index_path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        if let Some(map) = index.as_object() {
            for (cwd, value) in map {
                if value.as_str() != Some(session) {
                    continue;
                }
                if candidates.iter().any(|p| paths_same(Path::new(cwd), p)) {
                    matched = true;
                } else {
                    return Ok(false);
                } // Never first-claim an ambiguous index.
            }
        }
    }
    // A resume echo is not a creation receipt. Search only Created conversation
    // records, tied to the workspace recorded by that CLI invocation.
    let mut logs = Vec::new();
    collect_matching_files(&app.join("log"), &mut logs, |p| {
        p.extension().is_some_and(|e| e == "log")
    })?;
    logs.sort_by_key(|p| std::cmp::Reverse(file_modified_time(p)));
    for path in logs.into_iter().take(128) {
        let mut cwd = None;
        for line in
            std::io::BufReader::new(fs::File::open(path).map_err(|e| e.to_string())?).lines()
        {
            let line = line.map_err(|e| e.to_string())?;
            if let Some((_, value)) =
                line.split_once("Initializing CLI store manager for workspace ")
            {
                cwd = Some(antigravity_log_workspace_matches(value.trim(), &candidates));
            }
            if let Some((_, tail)) = line.split_once("Created conversation ") {
                if antigravity_log_conversation_token(tail).as_deref() == Some(session) {
                    return Ok(cwd == Some(true));
                }
            }
        }
    }
    Ok(matched)
}

fn source_for_id(
    home: &Path,
    agent: &ProjectAgent,
    id: &str,
) -> Result<Option<NativeSource>, String> {
    // IDs supplied by old metadata/CLI are selectors only, never path traversal.
    if id.is_empty() || id.contains(['/', '\\']) || id == "." || id == ".." {
        return Ok(None);
    }
    let path = match agent.shell.as_str() {
        "antigravity" => Some(preferred_antigravity_transcript_path(
            &antigravity_app_dir(home),
            id,
        )),
        "claude" => {
            let mut found = None;
            for path in [
                claude_project_dir(home, &agent.cwd).join(format!("{id}.jsonl")),
                home.join(".claude/transcripts").join(format!("{id}.jsonl")),
            ] {
                if file_exists(&path).map_err(|error| error.to_string())? {
                    found = Some(path);
                    break;
                }
            }
            found
        }
        "opencode" => Some(opencode_db_path(home)),
        "codex" => {
            let mut candidates = Vec::new();
            collect_codex_source_candidates(&home.join(".codex/sessions"), &mut candidates)?;
            candidates
                .sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
            let mut found = None;
            for (_, path) in candidates {
                if read_codex_session_meta(&path)?
                    .is_some_and(|m| m.session_id == id && !m.is_subagent)
                {
                    found = Some(path);
                    break;
                }
            }
            found
        }
        "pi" => locate_pi_session_file_by_id(&pi_sessions_dir(home), &agent.cwd, id)?,
        "kimi" => {
            let mut exact = agent.clone();
            exact.session_id = Some(id.into());
            locate_kimi_source_in(&kimi_code_home_from(home), &exact)?
                .filter(|s| s.session_id == id)
                .map(|s| s.path)
        }
        _ => None,
    };
    let path = match path {
        Some(path) if file_exists(&path).map_err(|error| error.to_string())? => Some(path),
        _ => None,
    };
    Ok(path.map(|path| NativeSource {
        kind: native_source_kind_for_shell(&agent.shell),
        session_id: id.into(),
        path,
        aux_path: None,
    }))
}

fn verify_id(agent: &ProjectAgent, id: &str) -> anyhow::Result<()> {
    let home = dirs::home_dir().ok_or_else(|| denied("native home is unavailable"))?;
    verify_id_in(&home, &crate::kota_home_dir(), agent, id)
}
fn verify_id_in(home: &Path, kota: &Path, agent: &ProjectAgent, id: &str) -> anyhow::Result<()> {
    let store = Store::new(kota);
    let key = key(home, agent, id);
    // First look at the recorded source. This remains fast even when the
    // provider removed its ownership header/index or renamed the workspace.
    if let Some(record) = store.read(&key)? {
        let source = NativeSource {
            kind: native_source_kind_for_shell(&agent.shell),
            session_id: id.into(),
            path: record.source,
            aux_path: None,
        };
        if file_exists(&source.path)? {
            // A surviving SQLite DB is not evidence that its session row still
            // exists. A deleted row is pruned evidence, not a foreign owner.
            if source.kind == "opencode-sqlite" && incarnation(&source)?.is_none() {
                return store.verify(&key, agent, None, || Ok(None));
            }
            return verify_source_in(home, kota, agent, &source);
        }
        // Still check a new source at the provider's current path before using
        // the receipt for a pruned transcript.
        if let Some(source) = source_for_id(home, agent, id).map_err(transient)? {
            return verify_source_in(home, kota, agent, &source);
        }
        return store.verify(&key, agent, None, || Ok(None));
    }
    let source = source_for_id(home, agent, id)
        .map_err(transient)?
        .ok_or_else(|| denied("native ownership evidence is unavailable"))?;
    verify_source_in(home, kota, agent, &source)
}

fn shell(cli: AgentCli) -> Option<&'static str> {
    Some(match cli {
        AgentCli::Claude => "claude",
        AgentCli::Codex => "codex",
        AgentCli::Antigravity => "antigravity",
        AgentCli::Opencode => "opencode",
        AgentCli::Pi => "pi",
        AgentCli::Kimi => "kimi",
        AgentCli::KotaLocal => return None,
    })
}
fn managed_agent(req: &AgentSpawnRequest) -> anyhow::Result<Option<ProjectAgent>> {
    let Some(shell) = shell(req.cli) else {
        return Ok(None);
    };
    let cwd = Path::new(&req.project_root)
        .join(".agent-workspaces")
        .join(&req.agent_id);
    if !file_exists(&cwd.join("agent.yaml"))? {
        return Ok(None);
    }
    Ok(Some(ProjectAgent {
        agent_id: req.agent_id.clone(),
        shell: shell.into(),
        cwd,
        session_id: req.session_id.clone(),
    }))
}

// Returns (explicit ID, continuation requested). Values of unrelated flags
// are never interpreted as session IDs. Provider-native forks are checked
// against their parent below, before the child gets a new identity.
fn selector(cli: AgentCli, args: &[String]) -> (Option<String>, bool) {
    let flags: &[&str] = match cli {
        AgentCli::Claude => &["--resume", "-r"],
        AgentCli::Antigravity => &["--conversation"],
        AgentCli::Opencode => &["--session", "-s"],
        AgentCli::Pi => &["--session", "--session-id"],
        AgentCli::Kimi => &["--session", "-S"],
        _ => &[],
    };
    let mut resume = false;
    for (index, arg) in args.iter().enumerate() {
        if flags.contains(&arg.as_str()) {
            return (
                args.get(index + 1).filter(|s| !s.starts_with('-')).cloned(),
                true,
            );
        }
        for flag in flags {
            if let Some(id) = arg.strip_prefix(&format!("{flag}=")) {
                return (Some(id.into()), true);
            }
        }
        if matches!(arg.as_str(), "--continue" | "-c") && cli != AgentCli::Codex {
            resume = true;
        }
        if cli == AgentCli::Codex && matches!(arg.as_str(), "resume" | "fork") {
            return (
                args.get(index + 1).filter(|s| !s.starts_with('-')).cloned(),
                true,
            );
        }
        if cli == AgentCli::Pi && matches!(arg.as_str(), "--resume" | "-r") {
            resume = true;
        }
    }
    (None, resume)
}

pub(crate) struct RunningSession {
    agent: Option<ProjectAgent>,
    bound: Option<String>,
    started: SystemTime,
    new_session: bool,
}

// Called after SHELL/provider-switch normalization, before any process is
// stopped or spawned. The returned guard belongs to that exact PTY instance.
pub(crate) fn prepare_launch(req: &mut AgentSpawnRequest) -> anyhow::Result<RunningSession> {
    prepare_launch_with(req, verify_id, |agent| {
        let source = locate_native_source(agent).map_err(transient)?;
        if let Some(source) = &source {
            verify_source(agent, source)?;
        }
        Ok(source.map(|s| s.session_id))
    })
}

fn prepare_launch_with(
    req: &mut AgentSpawnRequest,
    verify: impl Fn(&ProjectAgent, &str) -> anyhow::Result<()>,
    latest_owned: impl Fn(&ProjectAgent) -> anyhow::Result<Option<String>>,
) -> anyhow::Result<RunningSession> {
    let mut guard = RunningSession {
        agent: managed_agent(req)?,
        bound: None,
        started: SystemTime::now(),
        new_session: true,
    };
    let Some(agent) = guard.agent.as_ref() else {
        return Ok(guard);
    };
    if req.fresh_session {
        req.args = crate::fresh_project_agent_launch_args(req.cli, &req.args);
        req.session_id = crate::generated_session_id_for_cli(req.cli);
        return Ok(guard);
    }
    let (explicit, continuation) = selector(req.cli, &req.args);
    let id = explicit.or(req.session_id.clone());
    let fork = req
        .args
        .iter()
        .any(|a| a == "--fork-session" || a == "--fork")
        || req.args.first().is_some_and(|a| a == "fork");
    if fork {
        let yaml = read_yaml_file(&agent.cwd.join("agent.yaml")).map_err(transient)?;
        let parent = yaml
            .as_ref()
            .and_then(|y| yaml_string(y, "bunshin-from"))
            .ok_or_else(|| denied("fork parent is unavailable"))?;
        let parent_agent = ProjectAgent {
            agent_id: parent.clone(),
            shell: agent.shell.clone(),
            cwd: project_root(agent).join(".agent-workspaces").join(parent),
            session_id: id.clone(),
        };
        verify(
            &parent_agent,
            id.as_deref()
                .ok_or_else(|| denied("fork session is unavailable"))?,
        )?;
        return Ok(guard);
    }
    if let Some(id) = id {
        verify(agent, &id)?;
        req.session_id = Some(id.clone());
        guard.bound = Some(id);
        guard.new_session = false;
    } else if continuation {
        let id = latest_owned(agent)?.ok_or_else(|| denied("no owned session to continue"))?;
        // Pin the verified selection. A global/latest lookup at exec time must
        // not silently pick a different session between check and spawn.
        req.args = crate::fresh_project_agent_launch_args(req.cli, &req.args);
        req.session_id = Some(id.clone());
        guard.bound = Some(id);
        guard.new_session = false;
    }
    Ok(guard)
}

impl RunningSession {
    pub(crate) fn bound_id(&self) -> Option<String> {
        self.bound.clone()
    }

    pub(crate) fn check(&mut self) -> anyhow::Result<()> {
        self.check_with(verify_id, |agent, started| {
            let mut fresh = agent.clone();
            fresh.session_id = None;
            // A guaranteed-new process is safe before any native ID exists.
            // Missing/temporarily unreadable discovery data is not an old-ID
            // claim and cannot prevent its first prompt from creating the ID.
            let source = match locate_native_source(&fresh) {
                Ok(source) => source,
                Err(error) => {
                    crate::kota_debug_log(&format!(
                        "[session-owner] new-session discovery: {error}"
                    ));
                    None
                }
            };
            let Some(source) = source else {
                return Ok(None);
            };
            let born_here = if source.kind == "opencode-sqlite" {
                opencode_session_created_checked(&source.path, &source.session_id)
                    .map_err(transient)?
                    .is_some_and(|t| t >= started)
            } else {
                source_path_is_after_reset(
                    &source.path,
                    Some(SessionReset {
                        cutoff: started,
                        birth: true,
                    }),
                )
            };
            if !born_here {
                return Ok(None);
            }
            verify_source(&fresh, &source)?;
            Ok(Some(source.session_id))
        })
    }

    fn check_with(
        &mut self,
        verify: impl Fn(&ProjectAgent, &str) -> anyhow::Result<()>,
        discover_new: impl Fn(&ProjectAgent, SystemTime) -> anyhow::Result<Option<String>>,
    ) -> anyhow::Result<()> {
        let Some(agent) = self.agent.as_ref() else {
            return Ok(());
        };
        if let Some(id) = &self.bound {
            return verify(agent, id);
        }
        if !self.new_session {
            return Err(denied("session ownership cannot be verified"));
        }
        self.bound = discover_new(agent, self.started)?;
        Ok(())
    }
}

pub(crate) fn verify_agent_session(
    root: &Path,
    agent_id: &str,
    cli: AgentCli,
    id: &str,
) -> anyhow::Result<()> {
    let Some(shell) = shell(cli) else {
        return Ok(());
    };
    verify_id(
        &ProjectAgent {
            agent_id: agent_id.into(),
            shell: shell.into(),
            cwd: root.join(".agent-workspaces").join(agent_id),
            session_id: Some(id.into()),
        },
        id,
    )
}

pub(crate) fn check_launch(req: &AgentSpawnRequest) -> anyhow::Result<()> {
    if let Some(agent) = managed_agent(req)? {
        let yaml = read_yaml_file(&agent.cwd.join("agent.yaml")).map_err(transient)?;
        let saved_provider = yaml
            .as_ref()
            .and_then(|y| yaml_string(y, "shell").or_else(|| yaml_string(y, "provider")));
        if saved_provider.is_some_and(|p| normalize_shell(&p) != agent.shell) {
            // Startup's existing provider-switch transaction guarantees a new
            // session; an old provider's ID is not a resume target for this one.
            return Ok(());
        }
    }
    prepare_launch(&mut req.clone()).map(|_| ())
}

pub(super) fn verify_saved_session(agent: &ProjectAgent) -> anyhow::Result<()> {
    match agent.session_id.as_deref() {
        Some(id) => verify_id(agent, id),
        None => Ok(()),
    }
}

#[cfg(test)]
#[path = "ownership_tests.rs"]
mod tests;
