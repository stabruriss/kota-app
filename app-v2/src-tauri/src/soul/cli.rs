//! The `kota-my-soul` command (SOUL-PLAN v4 §14/§17; TEXTS-FOR-REVIEW.md §D): the verbs
//! the Local tool `kota_my_soul` runs through this same binary. The help and reply
//! texts are the approved model-visible copy and are used verbatim.
use super::{date_of, dream_usage, episodes, marks, now_ts, project_slug, pushes, search, SoulPaths};
use anyhow::{anyhow, Result};
use std::{
    collections::HashMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

pub(crate) const HELP: &str = r#"kota-my-soul — your marks (lessons you keep across projects) and episodes (past room messages).

  add-mark "<text>"                 Add a mark. Up to 200 characters. Start with when it applies. At most 20 marks.
  edit-mark <id> "<text>"           Replace a mark's text.
  delete-mark <id>                  Remove a mark. Its text is kept as an episode.
  read-mark                         List your marks.
  search-episode "<words>" [--project <slug>]
                                    Search past room messages. Returns 5 hits, each with a neighbor line.
  search-episode --id <id>          Show one episode in full.
  clear-heat <id>                   Reset the usage count of an episode or dream after you have handled its reminder."#;

// Reply texts, TEXTS-FOR-REVIEW.md §D.
const LIMIT_REPLY: &str =
    "You already have 20 marks, the maximum. Run read-mark, then delete-mark or edit-mark one before adding.";
const NO_MARKS_REPLY: &str = "No marks yet. Add one with add-mark \"<text>\".";
const FULL_TEXT_FOOTER: &str = "Use `search-episode --id <id>` for the full text.";
// Not on the approved sheet yet; listed in the receipt for 老无.
const EMPTY_TEXT_REPLY: &str = "Mark text is empty.";

/// Where this run reads and writes. Built from the agent's environment by `run_cli`,
/// by hand in tests.
pub(crate) struct Env {
    pub agent_cwd: PathBuf,
    pub kota_home: PathBuf,
    pub agent_id: String,
    pub project: Option<String>,
    pub project_root: Option<PathBuf>,
    /// `Some` is injected by tests; `None` asks the Jev store (read-only, never the key).
    pub jev_ready: Option<bool>,
}

impl Env {
    pub(crate) fn from_process(flags: &HashMap<String, String>) -> Result<Self> {
        Self::from_process_with(flags, std::env::current_dir().ok().as_deref(), |key| {
            std::env::var(key).ok()
        })
    }

    pub(crate) fn from_process_with(
        flags: &HashMap<String, String>,
        cwd: Option<&Path>,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self> {
        let flag = |name: &str| {
            flags
                .get(name)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        let value = |flag_name: &str, var: &str| {
            flag(flag_name).or_else(|| {
                env(var)
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
            })
        };
        let kota_home = value("--kota-home", "KOTA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("Kota")
            });
        if let Some(dir) = crate::cli_identity::resolve(cwd, &env)? {
            let agent_cwd = flag("--agent-cwd")
                .map(PathBuf::from)
                .unwrap_or(dir.agent_cwd);
            let agent_id = flag("--agent-id").unwrap_or(dir.agent_id);
            let project_root = flag("--project-root")
                .map(PathBuf::from)
                .unwrap_or(dir.project_root);
            let project = project_slug(
                flag("--project-id").as_deref().or(Some(&dir.project_id)),
                Some(&project_root),
            );
            return Ok(Self {
                agent_cwd,
                kota_home,
                agent_id,
                project,
                project_root: Some(project_root),
                jev_ready: None,
            });
        }
        let agent_cwd = value("--agent-cwd", "KOTA_AGENT_CWD")
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("kota-my-soul requires KOTA_AGENT_CWD or --agent-cwd"))?;
        let agent_id = value("--agent-id", "KOTA_AGENT_ID")
            .or_else(|| agent_cwd.file_name().map(|name| name.to_string_lossy().to_string()))
            .ok_or_else(|| anyhow!("kota-my-soul requires KOTA_AGENT_ID or --agent-id"))?;
        let project_root = value("--project-root", "KOTA_PROJECT_ROOT").map(PathBuf::from);
        let project = project_slug(value("--project-id", "KOTA_PROJECT_ID").as_deref(), project_root.as_deref());
        Ok(Self { agent_cwd, kota_home, agent_id, project, project_root, jev_ready: None })
    }

    /// The gate Puppeteer uses: the selected Jev channel holds a verified key.
    fn jev_ready(&self) -> bool {
        self.jev_ready.unwrap_or_else(|| {
            let owned = crate::local_rpc::owned::OwnedDirectory::new(&self.kota_home);
            crate::local_rpc::jev::read(&owned).map(|settings| settings.ready).unwrap_or(false)
        })
    }
}

const GLOBAL_FLAGS: [&str; 5] = ["--agent-cwd", "--kota-home", "--agent-id", "--project-root", "--project-id"];

/// Global flags come out wherever they appear before a `--`; everything else keeps its order.
pub(crate) fn split_flags(args: &[String]) -> Result<(HashMap<String, String>, Vec<String>)> {
    let mut flags = HashMap::new();
    let mut rest = Vec::new();
    let mut literal = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if !literal && arg == "--" {
            literal = true;
            rest.push(arg.clone());
        } else if !literal && GLOBAL_FLAGS.contains(&arg.as_str()) {
            i += 1;
            let value = args.get(i).ok_or_else(|| anyhow!("{arg} requires a value"))?;
            flags.insert(arg.clone(), value.clone());
        } else {
            rest.push(arg.clone());
        }
        i += 1;
    }
    Ok((flags, rest))
}

pub fn run_cli() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let (flags, rest) = split_flags(&args)?;
    match rest.first().map(String::as_str) {
        None | Some("help" | "--help" | "-h") => {
            println!("{HELP}");
            return Ok(());
        }
        Some("install-shim") => {
            println!("{}", install_cli_shim()?.display());
            return Ok(());
        }
        _ => {}
    }
    let env = Env::from_process(&flags)?;
    let mut stdin = String::new();
    if wants_stdin(&rest) {
        std::io::stdin().read_to_string(&mut stdin)?;
    }
    let output = run(&rest, &env, &stdin, &now_ts()).map_err(|message| anyhow!(message))?;
    println!("{output}");
    Ok(())
}

/// Only a text verb without its text reads stdin, so a terminal never blocks.
fn wants_stdin(rest: &[String]) -> bool {
    match rest.first().map(String::as_str) {
        Some("add-mark") => text_after(rest, 1).is_none(),
        Some("edit-mark") => text_after(rest, 2).is_none(),
        _ => false,
    }
}

/// The text after position `from`: an optional `--`, then the remaining words joined
/// by one space. The Local tool always sends the `--`, so a text starting with `-`
/// is never read as an option.
fn text_after(rest: &[String], from: usize) -> Option<String> {
    let mut words = rest.get(from..).unwrap_or(&[]);
    if words.first().map(String::as_str) == Some("--") {
        words = &words[1..];
    }
    let text = words.join(" ");
    (!text.trim().is_empty()).then_some(text)
}

/// Runs one verb. `Ok` is the reply the model sees (refusals included); `Err` is a
/// failure of the command itself.
pub(crate) fn run(args: &[String], env: &Env, stdin: &str, ts: &str) -> Result<String, String> {
    let paths = SoulPaths::new(&env.agent_cwd);
    let verb = args.first().map(String::as_str).unwrap_or("help");
    match verb {
        "help" | "--help" | "-h" => Ok(HELP.into()),
        "add-mark" => {
            let text = text_after(args, 1).unwrap_or_else(|| stdin.to_string());
            Ok(match marks::add(&paths, &text, env.project.clone(), ts)? {
                marks::Added::Added { id, marks } => {
                    // add() has committed and released the Soul lock. A room notification
                    // must never turn this successful write into an error/retry.
                    if let Some(mark) = marks.find(&id) {
                        if let Err(error) = record_note(env, mark, marks.revision) {
                            crate::kota_debug_log(&format!("[soul] note progress: {error}"));
                        }
                    }
                    with_list(
                        format!(
                            "Added {id} (revision {}). {} of {} marks in use.",
                            marks.revision,
                            marks.marks.len(),
                            marks::MARK_LIMIT
                        ),
                        &marks,
                        env,
                    )
                },
                marks::Added::Limit => LIMIT_REPLY.into(),
                marks::Added::Empty => EMPTY_TEXT_REPLY.into(),
                marks::Added::TooLong(chars) => too_long(chars),
            })
        }
        "edit-mark" => {
            let id = id_argument(args, 1)?;
            let text = text_after(args, 2).unwrap_or_else(|| stdin.to_string());
            Ok(match marks::edit(&paths, &id, &text, ts)? {
                marks::Edited::Updated(marks) => with_list(
                    format!("Updated {id} (revision {}).", marks.revision),
                    &marks,
                    env,
                ),
                marks::Edited::NotFound => not_found(&id),
                marks::Edited::Empty => EMPTY_TEXT_REPLY.into(),
                marks::Edited::TooLong(chars) => too_long(chars),
            })
        }
        "delete-mark" => {
            let id = id_argument(args, 1)?;
            Ok(match marks::delete(&paths, &id, ts)? {
                marks::Deleted::Deleted(marks) => with_list(
                    format!("Deleted {id} (revision {}). Its text is kept as an episode.", marks.revision),
                    &marks,
                    env,
                ),
                marks::Deleted::NotFound => not_found(&id),
            })
        }
        "read-mark" => {
            let marks = marks::snapshot(&paths)?;
            Ok(if marks.marks.is_empty() { NO_MARKS_REPLY.into() } else { list(&marks, env.jev_ready()) })
        }
        "search-episode" => search_episode(&paths, &parse_search(args)?, ts),
        "clear-heat" => {
            let id = id_argument(args, 1)?;
            clear_heat(&paths, env, &id, ts)
        }
        _ => Err(format!("unknown kota-my-soul command: {verb}\n{HELP}")),
    }
}

fn record_note(env: &Env, mark: &marks::Mark, revision: u64) -> Result<(), String> {
    let Some(project_root) = &env.project_root else { return Ok(()); };
    let text = fs::read_to_string(env.agent_cwd.join("SHELL.yaml")).map_err(|e| e.to_string())?;
    let shell = crate::parse_shell_yaml(&text)?;
    let identity = crate::read_yaml_mapping(&env.agent_cwd.join("agent.yaml"))?;
    let cli = crate::cli_from_project_agent_files(&env.agent_cwd, &shell, &identity)?;
    crate::violet::record_note_added(project_root, &env.agent_id, crate::shell_name_for_cli(cli), mark, revision)?;
    Ok(())
}

fn id_argument(args: &[String], at: usize) -> Result<String, String> {
    args.get(at)
        .filter(|id| !id.is_empty() && !id.starts_with('-'))
        .cloned()
        .ok_or_else(|| HELP.to_string())
}

fn too_long(chars: usize) -> String {
    format!("Mark too long: {chars} of {} characters.", marks::MARK_TEXT_CHARS)
}

fn not_found(id: &str) -> String {
    format!("No mark with id {id}. Run read-mark to see your marks.")
}

/// A write's reply: the first line, then the whole list as read-mark shows it.
fn with_list(first: String, marks: &marks::Marks, env: &Env) -> String {
    if marks.marks.is_empty() {
        first
    } else {
        format!("{first}\n{}", list(marks, env.jev_ready()))
    }
}

/// `read-mark` lines: the `used N times` column only where Jev is configured (§D).
fn list(marks: &marks::Marks, jev: bool) -> String {
    marks
        .marks
        .iter()
        .map(|mark| {
            if jev {
                format!("{}  used {} times  {}  {}", mark.id, mark.used, date_of(&mark.created_at), mark.text)
            } else {
                format!("{}  {}  {}", mark.id, date_of(&mark.created_at), mark.text)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

struct SearchArgs {
    id: Option<String>,
    project: Option<String>,
    query: String,
}

/// `search-episode [--id <id>] [--project <slug>] [--] <words...>`: options first, the
/// words after an optional `--` (the Local tool always sends it).
fn parse_search(args: &[String]) -> Result<SearchArgs, String> {
    let mut parsed = SearchArgs { id: None, project: None, query: String::new() };
    let mut words = Vec::new();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--" => {
                words.extend(args[i + 1..].iter().cloned());
                break;
            }
            flag @ ("--id" | "--project") => {
                let value = args.get(i + 1).filter(|v| !v.is_empty() && !v.starts_with('-')).ok_or_else(|| HELP.to_string())?;
                if flag == "--id" { parsed.id = Some(value.clone()) } else { parsed.project = Some(value.clone()) }
                i += 1;
            }
            word => words.push(word.to_string()),
        }
        i += 1;
    }
    parsed.query = words.join(" ").trim().to_string();
    if parsed.id.is_none() && parsed.query.is_empty() {
        return Err(HELP.to_string());
    }
    Ok(parsed)
}

/// `[id] date · project · who` (project left out when the episode has none).
fn hit_header(episode: &search::Stored) -> String {
    let mut parts = vec![date_of(&episode.ts).to_string()];
    if let Some(project) = episode.project.as_deref().filter(|p| !p.is_empty()) {
        parts.push(project.to_string());
    }
    parts.push(search::who(episode));
    format!("[{}] {}", episode.id, parts.join(" · "))
}

/// TEXTS §K.3, after the hit it is about.
fn reminder(id: &str, returned: u64, expanded: u64) -> String {
    format!("↑ Returned {returned} times, expanded {expanded}. If this is a lasting lesson, add-mark it; then run clear-heat {id} either way.")
}

fn search_episode(paths: &SoulPaths, args: &SearchArgs, ts: &str) -> Result<String, String> {
    if let Some(id) = args.id.as_deref() {
        let conn = search::open_current(paths)?;
        let Some(episode) = search::stored(&conn, id)? else {
            return Ok(format!("No episode with id {id}."));
        };
        episodes::append_usage(paths, &[episodes::Usage { ts: ts.into(), op: "expand".into(), id: id.into(), w: Some(search::EXPAND_W), q: None }])?;
        return Ok(format!("{}\n{}", hit_header(&episode), episode.text));
    }
    let (hits, usage) = search::search(paths, &args.query, args.project.as_deref(), ts)?;
    if hits.is_empty() {
        return Ok(format!("No episodes match \"{}\".", args.query));
    }
    let terms = search::tokens(&args.query);
    let mut blocks = Vec::new();
    let mut returns = Vec::new();
    for hit in &hits {
        let mut lines = vec![hit_header(&hit.episode), search::first_slice(&hit.episode.text)];
        if let Some(window) = search::match_window(&hit.episode.text, &terms) {
            lines.push(window);
        }
        if let Some(neighbour) = &hit.neighbour {
            lines.push(format!("↳ before: {}: {}", search::who(neighbour), search::neighbour_slice(&neighbour.text)));
        }
        let (returned, expanded) = search::counts_since_clear(&usage, &hit.episode.id);
        let returned = returned + 1;
        if returned >= search::REMIND_RETURNED || expanded >= search::REMIND_EXPANDED {
            lines.push(reminder(&hit.episode.id, returned, expanded));
        }
        blocks.push(lines.join("\n"));
        returns.push(episodes::Usage { ts: ts.into(), op: "return".into(), id: hit.episode.id.clone(), w: Some(search::RETURN_W), q: Some(args.query.clone()) });
    }
    episodes::append_usage(paths, &returns)?;
    blocks.push(FULL_TEXT_FOOTER.into());
    Ok(blocks.join("\n\n"))
}

/// A Dream id clears the account ledger (this agent's reminder count) and, when the
/// same id is also an old_dream episode here, the episode ledger; any other id clears
/// the episode ledger. Nothing recorded anywhere is said so, not written.
fn clear_heat(paths: &SoulPaths, env: &Env, id: &str, ts: &str) -> Result<String, String> {
    let mut cleared = false;
    if pushes::is_dream_id(id)
        && (dream_usage::has_record(&env.kota_home, id)? || pushes::last_push(paths, id, None)?.is_some())
    {
        dream_usage::append_clear(&env.kota_home, &env.agent_id, id, ts)?;
        cleared = true;
    }
    if episodes::has_usage(paths, id)? {
        episodes::append_usage(paths, &[episodes::Usage::clear(id, ts)])?;
        cleared = true;
    }
    Ok(if cleared { format!("Cleared usage for {id}.") } else { format!("No usage recorded for {id}.") })
}

/// `~/Kota/bin/kota-my-soul`, the shim the App installs at start, as for kota-ember.
pub fn install_cli_shim() -> Result<PathBuf> {
    install_named_cli_shim("kota-my-soul", "KOTA_MY_SOUL_BIN")
}

pub(crate) fn install_named_cli_shim(name: &str, override_env: &str) -> Result<PathBuf> {
    let bin_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join("Kota").join("bin");
    fs::create_dir_all(&bin_dir)?;
    let shim = bin_dir.join(name);
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join(name));
            candidates.push(parent.join(format!("../Resources/{name}")));
            if let Some(triple) = current_target_triple_guess() {
                candidates.push(parent.join(format!("{name}-{triple}")));
                candidates.push(parent.join(format!("../Resources/{name}-{triple}")));
            }
        }
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("target/debug/{name}")));
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("target/release/{name}")));
    let mut script = String::from("#!/bin/sh\nset -eu\n");
    script.push_str(&format!(
        "if [ -n \"${{{override_env}:-}}\" ] && [ -x \"${override_env}\" ]; then exec \"${override_env}\" \"$@\"; fi\n",
    ));
    for candidate in candidates {
        let quoted = shell_quote(&candidate.display().to_string());
        script.push_str(&format!("if [ -x {quoted} ]; then exec {quoted} \"$@\"; fi\n"));
    }
    script.push_str(&format!(
        "echo '{name} binary is not installed. Build it with: cargo build --bin {name}' >&2\nexit 127\n",
    ));
    fs::write(&shim, script)?;
    make_executable(&shim)?;
    Ok(shim)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn current_target_triple_guess() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
        _ => None,
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
