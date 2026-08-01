// agent.rs — per-Agent PTY (CC / Codex / Antigravity / OpenCode).
//
// Mirrors `pty/smart.rs` but specialised for *agent* CLI sessions:
//   - cwd is the agent's incarnation workspace (.agent-workspaces/{id}/)
//   - bin is the chosen CLI (claude | codex | agy | opencode)
//   - env carries Kota's 4 path env vars + git author identity per I-15 / I-26
//   - per-agent events: pty://agent/{id}/{output|exit|status}
//
// Per ARCHITECTURE-INVARIANTS:
//   - I-1 child process (no SDK embed)
//   - I-4 same portable-pty + alacritty_terminal infra as smart.rs
//   - I-5 zero protocol parsing — whatever the CLI prints, the seat renders
//   - I-15 path env vars include KOTA_PROJECT_MEMORY_DIR and KOTA_PROJECT_RULES_DIR
//   - I-26 GIT_AUTHOR_EMAIL = "{agent_id}@kota.local"
//   - I-31 dogfood-min ships CC + Codex; Antigravity / OpenCode reserved for M7+

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use super::ansi::{AnsiLineDecoder, GridSnapshot};
use super::path_env::{augmented_path, resolve_on_augmented_path};

const DEFAULT_COLS: u16 = 100;
const DEFAULT_ROWS: u16 = 28;
const OSC_11_QUERY: &[u8] = b"\x1b]11;?\x1b\\";
const OSC_11_RESPONSE: &[u8] = b"\x1b]11;rgb:1a/18/20\x1b\\";
const TERMINAL_NAME_QUERY: &[u8] = b"\x1b[>q";
const TERMINAL_NAME_RESPONSE: &[u8] = b"\x1bP>|Kota\x1b\\";
const MODIFY_OTHER_KEYS_QUERY: &[u8] = b"\x1b[>4;?m";
const MODIFY_OTHER_KEYS_RESPONSE: &[u8] = b"\x1b[>4;0m";
const DEVICE_ATTRIBUTES_QUERY: &[u8] = b"\x1b[c";
const DEVICE_ATTRIBUTES_RESPONSE: &[u8] = b"\x1b[?1;2c";
const CURSOR_POSITION_QUERY: &[u8] = b"\x1b[6n";
const AGENT_PROMPT_INTERRUPT_CLEAR_INPUT: &str = "\x15\x0b";
const AGENT_PROMPT_FIRST_OUTPUT_WAIT: Duration = Duration::from_millis(800);
const AGENT_PROMPT_QUIET_WAIT: Duration = Duration::from_millis(120);
const AGENT_PROMPT_MAX_SETTLE_WAIT: Duration = Duration::from_millis(1500);
const AGENT_PROMPT_SUBMIT_CONFIRM_WAIT: Duration = Duration::from_millis(1500);
const AGENT_PROMPT_NATIVE_CONFIRM_POLL: Duration = Duration::from_millis(50);
const AGENT_SESSION_LEASE_ERROR_PREFIX: &str = "KOTA_AGENT_SESSION_LEASE_CONFLICT:";
static OPENCODE_LAUNCH_MODE_CACHE: OnceLock<Mutex<HashMap<String, OpencodeLaunchMode>>> =
    OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentCli {
    Claude,
    Codex,
    #[serde(alias = "gemini", alias = "gemini-cli")]
    Antigravity,
    Opencode,
    Pi,
    Kimi,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpencodeLaunchMode {
    RunInteractive,
    Mini {
        supports_dangerously_skip_permissions: bool,
        supports_session: bool,
    },
}

impl AgentCli {
    /// Bin name resolved on PATH. CLI flag args (if any) come from agent.yaml later;
    /// dogfood-min spawns the bin bare and lets the user / persona drive the prompt.
    fn bin(self) -> &'static str {
        match self {
            AgentCli::Claude => "claude",
            AgentCli::Codex => "codex",
            AgentCli::Antigravity => "agy",
            AgentCli::Opencode => "opencode",
            AgentCli::Pi => "pi",
            AgentCli::Kimi => "kimi",
        }
    }
}

#[cfg(test)]
fn args_for_spawn(
    cli: AgentCli,
    args: &[String],
    cwd: &Path,
    session_id: Option<&str>,
) -> Vec<String> {
    args_for_spawn_with_opencode_launch_mode(
        cli,
        args,
        cwd,
        session_id,
        OpencodeLaunchMode::RunInteractive,
    )
}

fn args_for_spawn_with_opencode_launch_mode(
    cli: AgentCli,
    args: &[String],
    cwd: &Path,
    session_id: Option<&str>,
    opencode_launch_mode: OpencodeLaunchMode,
) -> Vec<String> {
    let mut out = normalize_runtime_args(cli, args);
    let opencode_launch_mode =
        if cli == AgentCli::Opencode && opencode_args_request_subcommand(&out) {
            OpencodeLaunchMode::RunInteractive
        } else {
            opencode_launch_mode
        };
    ensure_default_runtime_args(cli, &mut out, opencode_launch_mode);
    if let Some(session_id) = session_id.filter(|session_id| !session_id.trim().is_empty()) {
        match cli {
            AgentCli::Claude => {
                if !claude_args_request_resume(&out) && !claude_args_request_subcommand(&out) {
                    out.splice(0..0, ["--resume".to_string(), session_id.to_string()]);
                }
            }
            AgentCli::Codex => {
                if !codex_args_request_session(&out) && !codex_args_request_subcommand(&out) {
                    let mut next = vec!["resume".to_string(), session_id.to_string()];
                    next.extend(out);
                    out = next;
                }
            }
            AgentCli::Antigravity => {
                if !antigravity_args_request_conversation(&out) {
                    out.splice(0..0, ["--conversation".to_string(), session_id.to_string()]);
                }
            }
            AgentCli::Opencode => {
                if !opencode_args_request_session(&out) && !opencode_args_request_subcommand(&out) {
                    out.splice(0..0, ["--session".to_string(), session_id.to_string()]);
                }
            }
            AgentCli::Pi => {
                if !pi_args_request_session(&out) && !pi_args_request_subcommand(&out) {
                    out.splice(0..0, ["--session-id".to_string(), session_id.to_string()]);
                }
            }
            AgentCli::Kimi => {
                if !kimi_args_request_session(&out) && !kimi_args_request_subcommand(&out) {
                    out.splice(0..0, ["--session".to_string(), session_id.to_string()]);
                }
            }
        }
    }
    if cli == AgentCli::Antigravity && !antigravity_args_have_workspace_dir(&out) {
        out.splice(0..0, ["--add-dir".to_string(), cwd.display().to_string()]);
    }
    let mut opencode_mini_project = None;
    if cli == AgentCli::Opencode {
        match opencode_launch_mode {
            OpencodeLaunchMode::RunInteractive => {
                if !opencode_args_have_dir(&out) {
                    out.extend(["--dir".to_string(), cwd.display().to_string()]);
                }
            }
            OpencodeLaunchMode::Mini {
                supports_dangerously_skip_permissions,
                supports_session,
            } => {
                opencode_mini_project =
                    take_opencode_dir_arg(&mut out).or_else(|| Some(cwd.display().to_string()));
                if !supports_dangerously_skip_permissions {
                    strip_opencode_dangerously_skip_permissions_arg(&mut out);
                }
                if !supports_session {
                    strip_opencode_session_arg(&mut out);
                }
            }
        }
    }
    normalize_spawn_subcommand_args(cli, &mut out, opencode_launch_mode, opencode_mini_project);
    out
}

fn normalize_runtime_args(cli: AgentCli, args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if cli == AgentCli::Claude && arg == "--allow-dangerously-skip-permissions" {
            i += 1;
            continue;
        }
        if cli == AgentCli::Codex && arg == "--dangerously-bypass-hook-trust" {
            i += 1;
            continue;
        }
        out.push(args[i].clone());
        i += 1;
    }
    out
}

fn ensure_default_runtime_args(
    cli: AgentCli,
    args: &mut Vec<String>,
    opencode_launch_mode: OpencodeLaunchMode,
) {
    match cli {
        AgentCli::Claude => {
            if !has_any_arg(
                args,
                &["--dangerously-skip-permissions", "--permission-mode"],
                &["--permission-mode="],
            ) {
                args.push("--dangerously-skip-permissions".into());
            }
        }
        AgentCli::Codex => {
            if !has_any_arg(
                args,
                &[
                    "--dangerously-bypass-approvals-and-sandbox",
                    "--ask-for-approval",
                    "--sandbox",
                ],
                &["--ask-for-approval=", "--sandbox="],
            ) {
                args.push("--dangerously-bypass-approvals-and-sandbox".into());
            }
        }
        AgentCli::Antigravity => {
            if !args
                .iter()
                .any(|arg| arg == "--dangerously-skip-permissions")
            {
                args.push("--dangerously-skip-permissions".into());
            }
        }
        AgentCli::Opencode => {
            if !args.iter().any(|arg| arg == "--pure") {
                args.push("--pure".into());
            }
            let supports_dangerously_skip_permissions = match opencode_launch_mode {
                OpencodeLaunchMode::RunInteractive => true,
                OpencodeLaunchMode::Mini {
                    supports_dangerously_skip_permissions,
                    ..
                } => supports_dangerously_skip_permissions,
            };
            if supports_dangerously_skip_permissions
                && !args
                    .iter()
                    .any(|arg| arg == "--dangerously-skip-permissions")
            {
                args.push("--dangerously-skip-permissions".into());
            }
        }
        AgentCli::Pi => {
            if !has_any_arg(args, &["--approve", "-a", "--no-approve", "-na"], &[]) {
                args.push("--approve".into());
            }
        }
        AgentCli::Kimi => {
            if !has_any_arg(args, &["--yolo", "-y", "--auto"], &[]) {
                args.push("--yolo".into());
            }
        }
    }
}

fn normalize_spawn_subcommand_args(
    cli: AgentCli,
    args: &mut Vec<String>,
    opencode_launch_mode: OpencodeLaunchMode,
    opencode_mini_project: Option<String>,
) {
    if cli != AgentCli::Opencode {
        return;
    }
    if opencode_args_request_subcommand(args) {
        return;
    }
    match opencode_launch_mode {
        OpencodeLaunchMode::RunInteractive => {
            args.splice(0..0, ["run".to_string(), "--interactive".to_string()]);
        }
        OpencodeLaunchMode::Mini { .. } => {
            args.insert(0, "--mini".to_string());
            if let Some(project) = opencode_mini_project {
                args.push(project);
            }
        }
    }
}

fn resolve_opencode_launch_mode(cli: AgentCli, bin: &str) -> OpencodeLaunchMode {
    if cli != AgentCli::Opencode {
        return OpencodeLaunchMode::RunInteractive;
    }
    let cache = OPENCODE_LAUNCH_MODE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(mode) = cache
        .lock()
        .expect("opencode launch mode cache poisoned")
        .get(bin)
        .copied()
    {
        return mode;
    }

    let mode = probe_opencode_launch_mode(bin).unwrap_or(OpencodeLaunchMode::RunInteractive);
    cache
        .lock()
        .expect("opencode launch mode cache poisoned")
        .insert(bin.to_string(), mode);
    mode
}

fn probe_opencode_launch_mode(bin: &str) -> Result<OpencodeLaunchMode> {
    let run_help = opencode_help_text(bin, &["run", "--help"])?;
    if opencode_run_help_supports_interactive(&run_help) {
        return Ok(OpencodeLaunchMode::RunInteractive);
    }
    let mini_help = opencode_help_text(bin, &["--mini", "--help"]).unwrap_or_default();
    let supports_dangerously_skip_permissions =
        opencode_mini_help_supports_dangerously_skip_permissions(&mini_help);
    let supports_session = opencode_mini_help_supports_session(&mini_help);
    Ok(OpencodeLaunchMode::Mini {
        supports_dangerously_skip_permissions,
        supports_session,
    })
}

fn opencode_help_text(bin: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(bin)
        .args(args)
        .env("NO_COLOR", "1")
        .env_remove("FORCE_COLOR")
        .output()
        .with_context(|| format!("probe {bin} {}", args.join(" ")))?;
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    if output.status.success() || !text.trim().is_empty() {
        return Ok(text);
    }
    Err(anyhow!(
        "probe {bin} {} failed with status {}",
        args.join(" "),
        output.status
    ))
}

fn opencode_run_help_supports_interactive(help: &str) -> bool {
    help.contains("--interactive")
}

fn opencode_mini_help_supports_dangerously_skip_permissions(help: &str) -> bool {
    help.contains("--dangerously-skip-permissions")
}

fn opencode_mini_help_supports_session(help: &str) -> bool {
    help.contains("--session")
}

fn take_opencode_dir_arg(args: &mut Vec<String>) -> Option<String> {
    let mut next = Vec::with_capacity(args.len());
    let mut dir = None;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--dir" {
            if let Some(value) = args.get(i + 1) {
                dir = Some(value.clone());
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if let Some(value) = arg.strip_prefix("--dir=") {
            dir = Some(value.to_string());
            i += 1;
            continue;
        }
        next.push(arg.clone());
        i += 1;
    }
    *args = next;
    dir
}

fn strip_opencode_dangerously_skip_permissions_arg(args: &mut Vec<String>) {
    args.retain(|arg| arg != "--dangerously-skip-permissions");
}

fn strip_opencode_session_arg(args: &mut Vec<String>) {
    let mut next = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--session" || arg == "-s" {
            i += if args.get(i + 1).is_some() { 2 } else { 1 };
            continue;
        }
        if arg.starts_with("--session=") {
            i += 1;
            continue;
        }
        next.push(arg.clone());
        i += 1;
    }
    *args = next;
}

fn has_any_arg(args: &[String], exact: &[&str], prefixes: &[&str]) -> bool {
    args.iter().any(|arg| {
        exact.contains(&arg.as_str()) || prefixes.iter().any(|prefix| arg.starts_with(prefix))
    })
}

fn prepare_provider_workspace(cli: AgentCli, cwd: &Path, home: &Path) -> Result<()> {
    match cli {
        AgentCli::Claude => ensure_claude_kota_hooks(cwd),
        AgentCli::Codex => ensure_codex_trusted_project(home, cwd),
        AgentCli::Antigravity => ensure_antigravity_trusted_workspace(home, cwd),
        AgentCli::Opencode | AgentCli::Pi | AgentCli::Kimi => Ok(()),
    }
}

const CLAUDE_ASK_USER_QUESTION_HOOK_SCRIPT: &str = r#"import fs from 'node:fs';
import path from 'node:path';

try {
  const raw = fs.readFileSync(0, 'utf8');
  const input = raw.trim() ? JSON.parse(raw) : {};
  if (String(input.tool_name ?? '').toLowerCase() !== 'askuserquestion') {
    process.exit(0);
  }

  const memoryDir = process.env.KOTA_PROJECT_MEMORY_DIR;
  const agentId = process.env.KOTA_AGENT_ID;
  if (!memoryDir || !agentId) {
    process.exit(0);
  }

  const dir = path.join(memoryDir, '.violet', 'claude-hooks');
  fs.mkdirSync(dir, { recursive: true });
  const safeAgentId = String(agentId).replace(/[^A-Za-z0-9_-]/g, '_') || 'agent';
  const event = {
    schema: 'kota.claude.ask-user-question.v1',
    captured_at: new Date().toISOString(),
    agent_id: agentId,
    project_root: process.env.KOTA_PROJECT_ROOT ?? null,
    session_id: input.session_id ?? null,
    transcript_path: input.transcript_path ?? null,
    hook_event_name: input.hook_event_name ?? null,
    tool_name: input.tool_name ?? null,
    tool_use_id: input.tool_use_id ?? null,
    tool_input: input.tool_input ?? {},
  };
  fs.appendFileSync(path.join(dir, `${safeAgentId}.jsonl`), `${JSON.stringify(event)}\n`);
} catch {
  process.exit(0);
}
"#;

fn ensure_claude_kota_hooks(cwd: &Path) -> Result<()> {
    let claude_dir = cwd.join(".claude");
    let hook_dir = claude_dir.join("kota-hooks");
    fs::create_dir_all(&hook_dir).with_context(|| format!("create {}", hook_dir.display()))?;
    let script_path = hook_dir.join("capture-ask-user-question.mjs");
    write_if_changed(
        &script_path,
        CLAUDE_ASK_USER_QUESTION_HOOK_SCRIPT.as_bytes(),
    )?;

    let settings_path = claude_dir.join("settings.json");
    let text = read_text_if_exists(&settings_path)?;
    let mut value = if text.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str::<serde_json::Value>(&text)
            .with_context(|| format!("parse {}", settings_path.display()))?
    };
    if !value.is_object() {
        value = serde_json::json!({});
    }

    let command = format!("node {}", shell_single_quote(&path_str(&script_path)));
    if ensure_claude_pre_tool_hook(&mut value, &command) || text.trim().is_empty() {
        let next = serde_json::to_string_pretty(&value)?;
        write_if_changed(&settings_path, next.as_bytes())?;
    }
    Ok(())
}

fn ensure_claude_pre_tool_hook(value: &mut serde_json::Value, command: &str) -> bool {
    let object = value.as_object_mut().expect("settings object");
    let hooks = object
        .entry("hooks".to_string())
        .or_insert_with(|| serde_json::json!({}));
    if !hooks.is_object() {
        *hooks = serde_json::json!({});
    }
    let hooks = hooks.as_object_mut().expect("hooks object");
    let pre_tool = hooks
        .entry("PreToolUse".to_string())
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    if !pre_tool.is_array() {
        *pre_tool = serde_json::Value::Array(Vec::new());
    }
    let pre_tool = pre_tool.as_array_mut().expect("PreToolUse array");
    let mut changed = remove_kota_ask_user_question_hooks(pre_tool);

    let ask_index = pre_tool.iter().position(|entry| {
        entry
            .get("matcher")
            .and_then(serde_json::Value::as_str)
            .map_or(false, |matcher| matcher == "AskUserQuestion")
    });
    let index = if let Some(index) = ask_index {
        index
    } else {
        pre_tool.push(serde_json::json!({
            "matcher": "AskUserQuestion",
            "hooks": []
        }));
        changed = true;
        pre_tool.len() - 1
    };

    let entry = pre_tool[index].as_object_mut();
    let Some(entry) = entry else {
        pre_tool[index] = serde_json::json!({
            "matcher": "AskUserQuestion",
            "hooks": []
        });
        return ensure_claude_pre_tool_hook(value, command);
    };
    let entry_hooks = entry
        .entry("hooks".to_string())
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    if !entry_hooks.is_array() {
        *entry_hooks = serde_json::Value::Array(Vec::new());
        changed = true;
    }
    let entry_hooks = entry_hooks.as_array_mut().expect("entry hooks array");
    if !entry_hooks.iter().any(|hook| {
        hook.get("command")
            .and_then(serde_json::Value::as_str)
            .map_or(false, |existing| existing == command)
    }) {
        entry_hooks.push(serde_json::json!({
            "type": "command",
            "command": command
        }));
        changed = true;
    }
    changed
}

fn remove_kota_ask_user_question_hooks(pre_tool: &mut Vec<serde_json::Value>) -> bool {
    let mut changed = false;
    for entry in pre_tool {
        let Some(hooks) = entry
            .get_mut("hooks")
            .and_then(serde_json::Value::as_array_mut)
        else {
            continue;
        };
        let before = hooks.len();
        hooks.retain(|hook| {
            !hook
                .get("command")
                .and_then(serde_json::Value::as_str)
                .map_or(false, |command| {
                    command.contains("capture-ask-user-question.mjs")
                })
        });
        if hooks.len() != before {
            changed = true;
        }
    }
    changed
}

fn ensure_codex_trusted_project(home: &Path, cwd: &Path) -> Result<()> {
    let path = home.join(".codex").join("config.toml");
    let mut text = read_text_if_exists(&path)?;
    let cwd_string = cwd.display().to_string();
    let header = format!("[projects.\"{}\"]", toml_basic_string(&cwd_string));
    if text.contains(&header) {
        return Ok(());
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(&header);
    text.push_str("\ntrust_level = \"trusted\"\n");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, text)?;
    Ok(())
}

fn ensure_antigravity_trusted_workspace(home: &Path, cwd: &Path) -> Result<()> {
    let path = home
        .join(".gemini")
        .join("antigravity-cli")
        .join("settings.json");
    let text = read_text_if_exists(&path)?;
    let mut value = if text.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str::<serde_json::Value>(&text)
            .with_context(|| format!("parse {}", path.display()))?
    };
    if !value.is_object() {
        value = serde_json::json!({});
    }
    let cwd_string = cwd.display().to_string();
    let object = value.as_object_mut().expect("settings object");
    let trusted = object
        .entry("trustedWorkspaces".to_string())
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    if !trusted.is_array() {
        *trusted = serde_json::Value::Array(Vec::new());
    }
    let trusted = trusted.as_array_mut().expect("trusted workspaces array");
    if trusted
        .iter()
        .any(|entry| entry.as_str() == Some(cwd_string.as_str()))
    {
        return Ok(());
    }
    trusted.push(serde_json::Value::String(cwd_string));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_string_pretty(&value)?)?;
    Ok(())
}

fn read_text_if_exists(path: &Path) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(err.into()),
    }
}

fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Ok(existing) = fs::read(path) {
        if existing == bytes {
            return Ok(());
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)?;
    Ok(())
}

fn shell_single_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for ch in value.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

fn toml_basic_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

struct StartupTrustPromptAutoAccept {
    cli: AgentCli,
    enabled: bool,
    sent: bool,
    tail: Vec<u8>,
}

impl StartupTrustPromptAutoAccept {
    fn new(cli: AgentCli, args: &[String], cwd: &Path) -> Self {
        Self {
            cli,
            enabled: startup_trust_auto_accept_enabled(cli, args, cwd),
            sent: false,
            tail: Vec::new(),
        }
    }

    fn observe(
        &mut self,
        chunk: &[u8],
        writer: &Arc<Mutex<Box<dyn Write + Send>>>,
        agent_id: &str,
        spawn_t0: std::time::Instant,
    ) {
        if !self.enabled || self.sent {
            return;
        }
        self.tail.extend_from_slice(chunk);
        if self.tail.len() > 4096 {
            let keep_from = self.tail.len() - 4096;
            self.tail.drain(0..keep_from);
        }
        if !self.prompt_seen() {
            return;
        }
        if let Ok(mut writer) = writer.lock() {
            if writer.write_all(b"\r").and_then(|_| writer.flush()).is_ok() {
                self.sent = true;
                crate::kota_debug_log(&format!(
                    "[agent:{}] auto-accepted {} workspace trust prompt after {}ms",
                    agent_id,
                    self.cli.bin(),
                    spawn_t0.elapsed().as_millis()
                ));
            }
        }
    }

    fn prompt_seen(&self) -> bool {
        let text = String::from_utf8_lossy(&self.tail).to_lowercase();
        match self.cli {
            AgentCli::Claude => {
                contains_all(&text, &["quick", "safety", "check", "trust", "folder"])
            }
            AgentCli::Codex => contains_all(
                &text,
                &["trust", "contents", "directory", "yes", "continue"],
            ),
            AgentCli::Antigravity => {
                contains_all(&text, &["trust", "contents", "project", "yes", "folder"])
            }
            AgentCli::Opencode | AgentCli::Pi | AgentCli::Kimi => false,
        }
    }
}

fn startup_trust_auto_accept_enabled(cli: AgentCli, args: &[String], cwd: &Path) -> bool {
    if !matches!(
        cli,
        AgentCli::Claude | AgentCli::Codex | AgentCli::Antigravity
    ) {
        return false;
    }
    if !is_kota_managed_agent_cwd(cwd) {
        return false;
    }
    match cli {
        AgentCli::Claude => args.iter().any(|arg| {
            arg == "--dangerously-skip-permissions"
                || arg == "--permission-mode=bypassPermissions"
                || arg == "bypassPermissions"
        }),
        AgentCli::Codex => args
            .iter()
            .any(|arg| arg == "--dangerously-bypass-approvals-and-sandbox"),
        AgentCli::Antigravity => args
            .iter()
            .any(|arg| arg == "--dangerously-skip-permissions"),
        AgentCli::Opencode | AgentCli::Pi | AgentCli::Kimi => false,
    }
}

fn is_kota_managed_agent_cwd(cwd: &Path) -> bool {
    let path = cwd.to_string_lossy();
    path.contains("/.agent-workspaces/")
        || (path.contains("/Kota/Workspaces/") && path.contains("/.agent-workspaces/"))
        || path.contains("/Kota/AgentWorkspaces/")
        || (path.contains("/.kota/projects/") && path.contains("/.agent-workspaces/"))
}

fn contains_all(text: &str, needles: &[&str]) -> bool {
    needles.iter().all(|needle| text.contains(needle))
}

fn wait_for_native_session_change(marker: &NativeSessionMarker, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if marker.changed() {
            return true;
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return false;
        };
        if remaining.is_zero() {
            return false;
        }
        thread::sleep(remaining.min(AGENT_PROMPT_NATIVE_CONFIRM_POLL));
    }
}

fn claude_project_dir_name(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | '.' => '-',
            other => other,
        })
        .collect()
}

fn pi_project_dir_name(cwd: &Path) -> String {
    let normalized = cwd.to_string_lossy();
    let trimmed = normalized.trim_start_matches(['/', '\\']);
    let safe = trimmed
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' => '-',
            other => other,
        })
        .collect::<String>();
    format!("--{safe}--")
}

fn pi_session_marker_for_id(project_dir: &Path, session_id: &str) -> Option<NativeSessionMarker> {
    if !project_dir.is_dir() {
        return None;
    }
    let suffix = format!("_{session_id}.jsonl");
    let entries = fs::read_dir(project_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == format!("{session_id}.jsonl") || name.ends_with(&suffix))
        {
            if let Some(marker) = NativeSessionMarker::read(path) {
                return Some(marker);
            }
        }
    }
    None
}

fn kimi_code_home(home: &Path) -> PathBuf {
    std::env::var_os("KIMI_CODE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".kimi-code"))
}

fn kimi_session_marker(
    home: &Path,
    cwd: &Path,
    session_id: Option<&str>,
) -> Option<NativeSessionMarker> {
    let kimi_home = kimi_code_home(home);
    kimi_session_marker_in(&kimi_home, cwd, session_id)
}

fn kimi_session_marker_in(
    kimi_home: &Path,
    cwd: &Path,
    session_id: Option<&str>,
) -> Option<NativeSessionMarker> {
    let index: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(kimi_home.join("workspaces.json")).ok()?).ok()?;
    let workspaces = index.get("workspaces")?.as_object()?;
    let workspace_id = workspaces.iter().find_map(|(id, workspace)| {
        let root = workspace.get("root")?.as_str()?;
        paths_refer_to_same_directory(Path::new(root), cwd).then(|| id.clone())
    })?;
    let sessions_dir = kimi_home.join("sessions").join(workspace_id);

    if let Some(session_id) = session_id.filter(|value| !value.trim().is_empty()) {
        let wire = sessions_dir
            .join(session_id)
            .join("agents")
            .join("main")
            .join("wire.jsonl");
        if kimi_session_state_matches_cwd(&sessions_dir.join(session_id), cwd) {
            return NativeSessionMarker::read(wire);
        }
        return None;
    }

    let mut candidates = fs::read_dir(&sessions_dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|session_dir| {
            session_dir.is_dir() && kimi_session_state_matches_cwd(session_dir, cwd)
        })
        .filter_map(|session_dir| {
            let wire = session_dir.join("agents").join("main").join("wire.jsonl");
            let modified = fs::metadata(&wire).ok()?.modified().ok()?;
            Some((modified, wire))
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    candidates
        .into_iter()
        .find_map(|(_, wire)| NativeSessionMarker::read(wire))
}

fn kimi_session_state_matches_cwd(session_dir: &Path, cwd: &Path) -> bool {
    let Ok(text) = fs::read_to_string(session_dir.join("state.json")) else {
        return false;
    };
    let Ok(state) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    state
        .get("workDir")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|work_dir| paths_refer_to_same_directory(Path::new(work_dir), cwd))
}

fn paths_refer_to_same_directory(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    left == right
}

fn claude_args_request_resume(args: &[String]) -> bool {
    args.iter().any(|arg| {
        arg == "--continue"
            || arg == "-c"
            || arg == "--resume"
            || arg == "-r"
            || arg.starts_with("--resume=")
            || arg == "--session-id"
            || arg.starts_with("--session-id=")
            || arg == "--from-pr"
            || arg.starts_with("--from-pr=")
    })
}

fn claude_args_request_subcommand(args: &[String]) -> bool {
    const COMMANDS: &[&str] = &[
        "agents",
        "auth",
        "auto-mode",
        "doctor",
        "install",
        "mcp",
        "plugin",
        "plugins",
        "project",
        "setup-token",
        "ultrareview",
        "update",
        "upgrade",
    ];
    args.iter().any(|arg| COMMANDS.contains(&arg.as_str()))
}

fn antigravity_args_have_workspace_dir(args: &[String]) -> bool {
    args.iter()
        .any(|arg| arg == "--add-dir" || arg.starts_with("--add-dir="))
}

fn antigravity_args_request_conversation(args: &[String]) -> bool {
    args.iter().any(|arg| {
        arg == "--conversation"
            || arg.starts_with("--conversation=")
            || arg == "--continue"
            || arg == "-c"
    })
}

fn codex_args_request_session(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "resume" || arg == "fork")
}

fn codex_args_request_subcommand(args: &[String]) -> bool {
    const COMMANDS: &[&str] = &[
        "exec",
        "e",
        "review",
        "login",
        "logout",
        "mcp",
        "plugin",
        "mcp-server",
        "app-server",
        "remote-control",
        "app",
        "completion",
        "update",
        "sandbox",
        "debug",
        "apply",
        "a",
        "cloud",
        "exec-server",
        "features",
        "help",
    ];
    args.iter().any(|arg| COMMANDS.contains(&arg.as_str()))
}

fn opencode_args_request_session(args: &[String]) -> bool {
    args.iter().any(|arg| {
        arg == "--continue"
            || arg == "-c"
            || arg == "--session"
            || arg == "-s"
            || arg.starts_with("--session=")
    })
}

fn opencode_args_have_dir(args: &[String]) -> bool {
    args.iter()
        .any(|arg| arg == "--dir" || arg.starts_with("--dir="))
}

fn opencode_args_request_subcommand(args: &[String]) -> bool {
    const COMMANDS: &[&str] = &[
        "completion",
        "acp",
        "mcp",
        "attach",
        "run",
        "debug",
        "providers",
        "auth",
        "agent",
        "upgrade",
        "uninstall",
        "serve",
        "web",
        "models",
        "stats",
        "export",
        "import",
        "github",
        "pr",
        "session",
        "plugin",
        "plug",
        "db",
    ];
    args.iter().any(|arg| COMMANDS.contains(&arg.as_str()))
}

fn pi_args_request_session(args: &[String]) -> bool {
    args.iter().any(|arg| {
        arg == "--continue"
            || arg == "-c"
            || arg == "--resume"
            || arg == "-r"
            || arg == "--session"
            || arg.starts_with("--session=")
            || arg == "--session-id"
            || arg.starts_with("--session-id=")
            || arg == "--fork"
            || arg.starts_with("--fork=")
            || arg == "--no-session"
    })
}

fn pi_args_request_subcommand(args: &[String]) -> bool {
    const COMMANDS: &[&str] = &[
        "config", "update", "install", "remove", "auth", "login", "logout", "models", "help",
    ];
    args.iter().any(|arg| COMMANDS.contains(&arg.as_str()))
}

fn kimi_args_request_session(args: &[String]) -> bool {
    args.iter().any(|arg| {
        arg == "--continue"
            || arg == "-c"
            || arg == "--session"
            || arg == "-S"
            || arg.starts_with("--session=")
    })
}

fn kimi_args_request_subcommand(args: &[String]) -> bool {
    const COMMANDS: &[&str] = &[
        "export", "provider", "acp", "server", "web", "login", "doctor", "vis", "migrate",
        "upgrade", "update", "help",
    ];
    args.iter().any(|arg| COMMANDS.contains(&arg.as_str()))
}

fn prompt_submit_sequence(cli: AgentCli) -> &'static str {
    if cli == AgentCli::Kimi {
        // Kimi enables the Kitty keyboard protocol in its TUI. A raw CR is
        // inserted into the editor; CSI-u Enter is the actual submit key.
        "\x1b[13;1u"
    } else {
        "\r"
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSpawnRequest {
    pub agent_id: String,
    pub cli: AgentCli,
    /// Absolute path to the launch cwd. Most CLIs use the incarnation workspace;
    /// Antigravity may use a visible symlink because `agy --add-dir` ignores
    /// hidden workspace paths.
    pub cwd: String,
    /// Legacy Kota project root. Kept as KOTA_PROJECT_ROOT for current
    /// terminals while KOTA_WORKTREE_ROOT becomes the clearer execution root.
    pub project_root: String,
    #[serde(default)]
    pub worktree_root: Option<String>,
    #[serde(default)]
    pub shared_dir: Option<String>,
    #[serde(default)]
    pub rules_dir: Option<String>,
    #[serde(default)]
    pub adapter_path: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub project_remote: Option<String>,
    #[serde(default)]
    pub project_base_ref: Option<String>,
    #[serde(default)]
    pub takeover: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRoute {
    pub agent_id: String,
    pub output_event: String,
    pub exit_event: String,
    pub status_event: String,
    pub work_event: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentOutputEvent {
    pub agent_id: String,
    /// Full visible-grid snapshot. Per I-4 / I-5: alacritty `Term` is the
    /// authoritative state; the frontend is a dumb cell renderer.
    pub snapshot: GridSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatusEvent {
    pub agent_id: String,
    pub running: bool,
    pub cli: AgentCli,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<AgentStatusPhase>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_bin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tty_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub byte_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forkable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_source: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentStatusPhase {
    Spawned,
    FirstBytes,
    TerminalQuery,
    Exited,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentExitEvent {
    pub agent_id: String,
    pub code: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentWorkStateEvent {
    pub agent_id: String,
    pub state: String,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cli: Option<AgentCli>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_event_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSummary {
    pub agent_id: String,
    pub cli: AgentCli,
    pub cwd: String,
    pub project_root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub running: bool,
}

struct AgentProcess {
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    decoder: Arc<Mutex<AnsiLineDecoder>>,
    child_pid: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentSessionLease {
    version: u8,
    agent_id: String,
    cli: AgentCli,
    cwd: String,
    project_root: String,
    owner_pid: u32,
    child_pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentSessionLeaseConflict {
    code: &'static str,
    agent_id: String,
    owner_pid: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    child_pid: Option<u32>,
}

struct OutputSignal {
    epoch: Mutex<u64>,
    changed: Condvar,
}

struct AgentState {
    agent_id: String,
    cli: AgentCli,
    cwd: PathBuf,
    project_root: PathBuf,
    worktree_root: PathBuf,
    shared_dir: PathBuf,
    rules_dir: PathBuf,
    adapter_path: Option<PathBuf>,
    args: Vec<String>,
    session_id: Option<String>,
    project_id: Option<String>,
    project_remote: Option<String>,
    project_base_ref: Option<String>,
    home: PathBuf,
    route: AgentRoute,
    size: Mutex<PtySize>,
    generation: AtomicU64,
    clear_next_submit: AtomicBool,
    submit_lock: Mutex<()>,
    output_signal: OutputSignal,
    process: Mutex<Option<AgentProcess>>,
}

#[derive(Clone)]
pub struct AgentTerminalPty {
    inner: Arc<AgentState>,
}

fn agent_session_lease_path(req: &AgentSpawnRequest) -> PathBuf {
    let shared_dir = req
        .shared_dir
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&req.project_root).join("project-memory"));
    shared_dir
        .join(".kota")
        .join("agent-session-leases")
        .join(format!("{}.json", safe_lease_filename(&req.agent_id)))
}

fn safe_lease_filename(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn read_agent_session_lease(path: &Path) -> Option<AgentSessionLease> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice::<AgentSessionLease>(&bytes).ok()
}

fn write_agent_session_lease(path: &Path, lease: &AgentSessionLease) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create agent session lease dir {}", parent.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(lease)?)
        .with_context(|| format!("write agent session lease {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("publish agent session lease {}", path.display()))?;
    Ok(())
}

fn remove_agent_session_lease(path: &Path, owner_pid: u32, child_pid: Option<u32>) {
    let Some(lease) = read_agent_session_lease(path) else {
        return;
    };
    if lease.owner_pid == owner_pid && lease.child_pid == child_pid {
        let _ = fs::remove_file(path);
    }
}

fn active_foreign_agent_session_lease(path: &Path) -> Option<AgentSessionLease> {
    let lease = read_agent_session_lease(path)?;
    let current_pid = std::process::id();
    if lease.owner_pid == current_pid {
        return None;
    }
    let owner_alive = process_is_alive(lease.owner_pid);
    let child_alive = lease.child_pid.is_some_and(process_is_alive);
    if owner_alive && (lease.child_pid.is_none() || child_alive) {
        Some(lease)
    } else {
        None
    }
}

fn lease_conflict_error(lease: &AgentSessionLease) -> anyhow::Error {
    let payload = AgentSessionLeaseConflict {
        code: "agent-session-lease-conflict",
        agent_id: lease.agent_id.clone(),
        owner_pid: lease.owner_pid,
        child_pid: lease.child_pid,
    };
    let json = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string());
    anyhow!("{AGENT_SESSION_LEASE_ERROR_PREFIX}{json}")
}

fn terminate_foreign_agent_session(lease: &AgentSessionLease) {
    if let Some(pid) = lease.child_pid {
        if pid != std::process::id() {
            terminate_process(pid);
        }
    }
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    // Probe via kill(pid, 0): 0 means the process exists. Use the libc syscall
    // directly instead of spawning `kill` — agent spawn scrubs env (including
    // PATH), which can make `Command::new("kill")` fail to launch and falsely
    // report a LIVE process as dead. A false "dead" is dangerous here: it would
    // trigger a respawn that kills a healthy session.
    unsafe { agent_process_kill(pid as i32, 0) == 0 }
}

#[cfg(unix)]
extern "C" {
    #[link_name = "kill"]
    fn agent_process_kill(pid: i32, sig: i32) -> i32;
}

#[cfg(windows)]
fn process_is_alive(pid: u32) -> bool {
    Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}")])
        .output()
        .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
}

#[cfg(not(any(unix, windows)))]
fn process_is_alive(_pid: u32) -> bool {
    false
}

#[cfg(unix)]
fn terminate_process(pid: u32) {
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status();
    thread::sleep(Duration::from_millis(450));
    if process_is_alive(pid) {
        let _ = Command::new("kill")
            .arg("-KILL")
            .arg(pid.to_string())
            .status();
    }
}

#[cfg(windows)]
fn terminate_process(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .status();
}

#[cfg(not(any(unix, windows)))]
fn terminate_process(_pid: u32) {}

#[derive(Clone, Debug)]
enum NativeSessionMarker {
    /// Watch one known session jsonl (session id resolved at spawn).
    File {
        path: PathBuf,
        modified: SystemTime,
        len: u64,
    },
    /// Watch the whole CLI project dir (session id unknown — e.g. fresh
    /// launch without --resume). Any session jsonl growing or appearing
    /// counts as submit confirmation; weaker than File but far stronger
    /// than falling back to "any terminal output" (which a composer
    /// redraw can satisfy without an actual submit).
    Dir {
        dir: PathBuf,
        files: usize,
        latest_modified: SystemTime,
        total_len: u64,
    },
}

impl NativeSessionMarker {
    fn read(path: PathBuf) -> Option<Self> {
        let metadata = fs::metadata(&path).ok()?;
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        Some(Self::File {
            path,
            modified,
            len: metadata.len(),
        })
    }

    fn read_dir(dir: PathBuf) -> Option<Self> {
        let entries = fs::read_dir(&dir).ok()?;
        let mut files = 0_usize;
        let mut latest_modified = SystemTime::UNIX_EPOCH;
        let mut total_len = 0_u64;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            files += 1;
            total_len = total_len.saturating_add(metadata.len());
            let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            if modified > latest_modified {
                latest_modified = modified;
            }
        }
        Some(Self::Dir {
            dir,
            files,
            latest_modified,
            total_len,
        })
    }

    fn changed(&self) -> bool {
        match self {
            Self::File {
                path,
                modified,
                len,
            } => {
                let Some(Self::File {
                    modified: next_modified,
                    len: next_len,
                    ..
                }) = Self::read(path.clone())
                else {
                    return false;
                };
                next_len != *len || next_modified > *modified
            }
            Self::Dir {
                dir,
                files,
                latest_modified,
                total_len,
            } => {
                let Some(Self::Dir {
                    files: next_files,
                    latest_modified: next_latest,
                    total_len: next_total,
                    ..
                }) = Self::read_dir(dir.clone())
                else {
                    return false;
                };
                next_files != *files || next_total != *total_len || next_latest > *latest_modified
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SubmitConfirmation {
    NativeLog,
    Output,
    PtyWrite,
    None,
}

impl SubmitConfirmation {
    fn is_confirmed(self) -> bool {
        !matches!(self, Self::None)
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::NativeLog => "native-log",
            Self::Output => "output",
            Self::PtyWrite => "pty-write",
            Self::None => "none",
        }
    }
}

impl AgentTerminalPty {
    pub fn new(req: AgentSpawnRequest) -> Result<Self> {
        let cwd = PathBuf::from(&req.cwd);
        if !cwd.exists() {
            return Err(anyhow!("agent cwd does not exist: {}", cwd.display()));
        }

        let project_root = PathBuf::from(&req.project_root);
        let worktree_root = req
            .worktree_root
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| cwd.clone());
        let shared_dir = req
            .shared_dir
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| project_root.join("project-memory"));
        let rules_dir = req
            .rules_dir
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| project_root.join("project-rules"));
        let adapter_path = req.adapter_path.as_deref().map(PathBuf::from);
        let args = req.args;
        let session_id = req.session_id;
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let route = AgentRoute {
            agent_id: req.agent_id.clone(),
            output_event: format!("pty://agent/{}/output", req.agent_id),
            exit_event: format!("pty://agent/{}/exit", req.agent_id),
            status_event: format!("pty://agent/{}/status", req.agent_id),
            work_event: format!("pty://agent/{}/work", req.agent_id),
        };

        Ok(Self {
            inner: Arc::new(AgentState {
                agent_id: req.agent_id,
                cli: req.cli,
                cwd,
                project_root,
                worktree_root,
                shared_dir,
                rules_dir,
                adapter_path,
                args,
                session_id,
                project_id: req.project_id,
                project_remote: req.project_remote,
                project_base_ref: req.project_base_ref,
                home,
                route,
                size: Mutex::new(PtySize {
                    cols: DEFAULT_COLS,
                    rows: DEFAULT_ROWS,
                    pixel_width: 0,
                    pixel_height: 0,
                }),
                generation: AtomicU64::new(0),
                clear_next_submit: AtomicBool::new(false),
                submit_lock: Mutex::new(()),
                output_signal: OutputSignal {
                    epoch: Mutex::new(0),
                    changed: Condvar::new(),
                },
                process: Mutex::new(None),
            }),
        })
    }

    pub fn route(&self) -> AgentRoute {
        self.inner.route.clone()
    }

    pub fn summary(&self) -> AgentSummary {
        AgentSummary {
            agent_id: self.inner.agent_id.clone(),
            cli: self.inner.cli,
            cwd: self.inner.cwd.display().to_string(),
            project_root: self.inner.project_root.display().to_string(),
            project_id: self.inner.project_id.clone(),
            running: self.is_running(),
        }
    }

    pub fn is_running(&self) -> bool {
        self.inner
            .process
            .lock()
            .expect("agent process poisoned")
            .is_some()
    }

    fn child_pid(&self) -> Option<u32> {
        self.inner
            .process
            .lock()
            .expect("agent process poisoned")
            .as_ref()
            .and_then(|process| process.child_pid)
    }

    /// Submit-only liveness guard. A dead provider child can leave a stale
    /// `process` entry whose master fd still accepts writes silently, so
    /// `write()` would "succeed" into a dead PTY and the agent-bus recovery
    /// (spawn-on-error in `deliver_to_terminal`) would never fire. This reports
    /// stale ONLY when we have a child pid and it is provably gone — a `None`
    /// process (lets `write()` spawn) or an unknown pid pass through untouched,
    /// so a live agent is never misjudged as dead (which would respawn-kill it).
    fn submit_child_is_dead(&self) -> bool {
        let guard = self.inner.process.lock().expect("agent process poisoned");
        match guard.as_ref() {
            None => false,
            Some(process) => match process.child_pid {
                None => false,
                Some(pid) => !process_is_alive(pid),
            },
        }
    }

    /// Spawn the CLI process (or no-op if already running).
    pub fn init(&self, app: &AppHandle) -> Result<()> {
        if self.is_running() {
            self.emit_status(app, true);
            return Ok(());
        }
        self.spawn(app)
    }

    pub fn write(&self, app: &AppHandle, input: String) -> Result<()> {
        if !self.is_running() {
            self.spawn(app)?;
        }

        let bytes = input.as_bytes();
        let preview: String = bytes
            .iter()
            .take(24)
            .map(|b| {
                if b.is_ascii_graphic() || *b == b' ' {
                    (*b as char).to_string()
                } else {
                    format!("\\x{:02x}", b)
                }
            })
            .collect();
        crate::kota_debug_log(&format!(
            "[agent:{}] write len={} bytes={:?}",
            self.inner.agent_id,
            bytes.len(),
            preview,
        ));

        let mut process_guard = self.inner.process.lock().expect("agent process poisoned");
        let process = process_guard
            .as_mut()
            .ok_or_else(|| anyhow!("agent process missing for {}", self.inner.agent_id))?;
        let mut writer = process.writer.lock().expect("agent writer poisoned");
        writer
            .write_all(bytes)
            .with_context(|| format!("write agent input for {}", self.inner.agent_id))?;
        writer
            .flush()
            .with_context(|| format!("flush agent input for {}", self.inner.agent_id))?;
        if bytes == [0x03] {
            self.inner.clear_next_submit.store(true, Ordering::Release);
        }
        Ok(())
    }

    pub fn submit_prompt(&self, app: &AppHandle, input: String) -> Result<()> {
        let _submit_guard = self
            .inner
            .submit_lock
            .lock()
            .expect("agent submit lock poisoned");
        // If the provider child has died but a stale process entry lingers,
        // write() would silently succeed on the dead master fd, so the
        // agent-bus recovery (spawn-on-error in deliver_to_terminal) would
        // never fire and the message would vanish. Drop the stale entry and
        // return an error so delivery resumes the agent via its launch_request
        // (composer/non-bus submits then respawn on the next write).
        if self.submit_child_is_dead() {
            let _ = self.stop_current();
            return Err(anyhow!(
                "agent process for {} is not alive",
                self.inner.agent_id
            ));
        }
        let started = Instant::now();
        let native_before = self.native_session_marker();
        let baseline_epoch = self.output_epoch();
        let clear_after_interrupt = self.inner.clear_next_submit.swap(false, Ordering::AcqRel);
        let input = if clear_after_interrupt {
            format!("{AGENT_PROMPT_INTERRUPT_CLEAR_INPUT}{input}")
        } else {
            input
        };
        if let Err(err) = self.write(app, input) {
            if clear_after_interrupt {
                self.inner.clear_next_submit.store(true, Ordering::Release);
            }
            return Err(err);
        }
        let first_output_epoch =
            self.wait_for_output_after(baseline_epoch, AGENT_PROMPT_FIRST_OUTPUT_WAIT);
        let first_output_ms = first_output_epoch.map(|_| started.elapsed().as_millis());
        let settle_deadline = started + AGENT_PROMPT_MAX_SETTLE_WAIT;
        let quieted = first_output_epoch.is_some_and(|epoch| {
            self.wait_for_output_quiet_after(epoch, AGENT_PROMPT_QUIET_WAIT, settle_deadline)
        });
        let settle_ms = quieted.then(|| started.elapsed().as_millis());
        let submit_state = if quieted {
            "output-quiet"
        } else if first_output_epoch.is_some() {
            "settle-timeout"
        } else {
            "first-output-timeout"
        };

        let mut enter_attempts = 1_u8;
        let first_enter_baseline = self.output_epoch();
        self.write(app, prompt_submit_sequence(self.inner.cli).to_string())?;
        let mut submit_confirm = if self.inner.cli == AgentCli::Pi {
            SubmitConfirmation::PtyWrite
        } else {
            self.wait_for_submit_confirmation(
                native_before.as_ref(),
                first_enter_baseline,
                AGENT_PROMPT_SUBMIT_CONFIRM_WAIT,
            )
        };
        let mut submit_confirm_ms = submit_confirm
            .is_confirmed()
            .then(|| started.elapsed().as_millis());
        if !submit_confirm.is_confirmed() {
            enter_attempts += 1;
            let retry_baseline = self.output_epoch();
            crate::kota_debug_log(&format!(
                "[agent:{}] submit prompt retrying enter after unconfirmed submit",
                self.inner.agent_id
            ));
            self.write(app, prompt_submit_sequence(self.inner.cli).to_string())?;
            submit_confirm = self.wait_for_submit_confirmation(
                native_before.as_ref(),
                retry_baseline,
                AGENT_PROMPT_SUBMIT_CONFIRM_WAIT,
            );
            submit_confirm_ms = submit_confirm
                .is_confirmed()
                .then(|| started.elapsed().as_millis());
        }

        let elapsed = started.elapsed().as_millis();
        crate::kota_debug_log(&format!(
            "[agent:{}] submit prompt enter sent after {}ms ({}, clear_after_interrupt={}, first_ack_ms={}, quiet_ms={}, enter_attempts={}, submit_confirm={}, submit_confirm_ms={})",
            self.inner.agent_id,
            elapsed,
            submit_state,
            clear_after_interrupt,
            first_output_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into()),
            settle_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into()),
            enter_attempts,
            submit_confirm.as_str(),
            submit_confirm_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into()),
        ));
        Ok(())
    }

    fn native_session_marker(&self) -> Option<NativeSessionMarker> {
        match self.inner.cli {
            AgentCli::Claude => {
                let project_dir = self
                    .inner
                    .home
                    .join(".claude")
                    .join("projects")
                    .join(claude_project_dir_name(&self.inner.cwd));
                if let Some(session_id) = self.inner.session_id.as_deref() {
                    let project_path = project_dir.join(format!("{session_id}.jsonl"));
                    let transcript_path = self
                        .inner
                        .home
                        .join(".claude")
                        .join("transcripts")
                        .join(format!("{session_id}.jsonl"));
                    if let Some(marker) = NativeSessionMarker::read(project_path)
                        .or_else(|| NativeSessionMarker::read(transcript_path))
                    {
                        return Some(marker);
                    }
                }
                // Fresh launches (no --resume) have no session id yet; without
                // this fallback submit confirmation silently degrades to the
                // output heuristic, which a textarea redraw can false-positive.
                NativeSessionMarker::read_dir(project_dir)
            }
            AgentCli::Pi => {
                let project_dir = self
                    .inner
                    .home
                    .join(".pi")
                    .join("agent")
                    .join("sessions")
                    .join(pi_project_dir_name(&self.inner.cwd));
                if let Some(session_id) = self.inner.session_id.as_deref() {
                    if let Some(marker) = pi_session_marker_for_id(&project_dir, session_id)
                        .or_else(|| {
                            // pi creates the timestamped session file lazily. Before
                            // the first write, watching the project session directory is
                            // the strongest confirmation source available.
                            NativeSessionMarker::read_dir(project_dir.clone())
                        })
                    {
                        return Some(marker);
                    }
                }
                NativeSessionMarker::read_dir(project_dir)
            }
            AgentCli::Kimi => kimi_session_marker(
                &self.inner.home,
                &self.inner.cwd,
                self.inner.session_id.as_deref(),
            ),
            AgentCli::Codex | AgentCli::Antigravity | AgentCli::Opencode => None,
        }
    }

    fn wait_for_submit_confirmation(
        &self,
        native_before: Option<&NativeSessionMarker>,
        output_baseline_epoch: u64,
        timeout: Duration,
    ) -> SubmitConfirmation {
        if let Some(native_before) = native_before {
            if wait_for_native_session_change(native_before, timeout) {
                return SubmitConfirmation::NativeLog;
            }
            return SubmitConfirmation::None;
        }

        if self
            .wait_for_output_after(output_baseline_epoch, timeout)
            .is_some()
        {
            SubmitConfirmation::Output
        } else {
            SubmitConfirmation::None
        }
    }

    fn output_epoch(&self) -> u64 {
        match self.inner.output_signal.epoch.lock() {
            Ok(epoch) => *epoch,
            Err(poisoned) => {
                self.inner.output_signal.epoch.clear_poison();
                *poisoned.into_inner()
            }
        }
    }

    fn wait_for_output_after(&self, baseline_epoch: u64, timeout: Duration) -> Option<u64> {
        let signal = &self.inner.output_signal;
        let epoch = match signal.epoch.lock() {
            Ok(epoch) => epoch,
            Err(poisoned) => {
                signal.epoch.clear_poison();
                poisoned.into_inner()
            }
        };
        let result = signal
            .changed
            .wait_timeout_while(epoch, timeout, |epoch| *epoch <= baseline_epoch);
        match result {
            Ok((epoch, wait)) => {
                if !wait.timed_out() && *epoch > baseline_epoch {
                    Some(*epoch)
                } else {
                    None
                }
            }
            Err(poisoned) => {
                signal.epoch.clear_poison();
                let (epoch, wait) = poisoned.into_inner();
                if !wait.timed_out() && *epoch > baseline_epoch {
                    Some(*epoch)
                } else {
                    None
                }
            }
        }
    }

    fn wait_for_output_quiet_after(
        &self,
        mut observed_epoch: u64,
        quiet_for: Duration,
        deadline: Instant,
    ) -> bool {
        let signal = &self.inner.output_signal;
        let mut epoch = match signal.epoch.lock() {
            Ok(epoch) => epoch,
            Err(poisoned) => {
                signal.epoch.clear_poison();
                poisoned.into_inner()
            }
        };
        loop {
            if *epoch > observed_epoch {
                observed_epoch = *epoch;
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            if remaining.is_zero() {
                return false;
            }
            let timeout = quiet_for.min(remaining);
            let result = signal
                .changed
                .wait_timeout_while(epoch, timeout, |epoch| *epoch <= observed_epoch);
            match result {
                Ok((next_epoch, wait)) => {
                    epoch = next_epoch;
                    if *epoch > observed_epoch {
                        continue;
                    }
                    if wait.timed_out() {
                        return timeout == quiet_for;
                    }
                }
                Err(poisoned) => {
                    signal.epoch.clear_poison();
                    let (next_epoch, wait) = poisoned.into_inner();
                    epoch = next_epoch;
                    if *epoch > observed_epoch {
                        continue;
                    }
                    if wait.timed_out() {
                        return timeout == quiet_for;
                    }
                }
            }
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        // Resize is fired by the frontend on EVERY window geom change,
        // including the first paint after recruit (before the spawn
        // IPC has fully settled) and during respawn windows. None of
        // these conditions warrant aborting the whole process — yet
        // a poisoned mutex (e.g. from a panic in portable_pty's
        // ioctl wrapper) used to do exactly that. Treat all failures
        // as soft no-ops so resize is fire-and-forget for callers.
        let size = PtySize {
            cols: cols.max(1),
            rows: rows.max(1),
            pixel_width: 0,
            pixel_height: 0,
        };
        if let Ok(mut sz) = self.inner.size.lock() {
            *sz = size;
        }
        let mut process_guard = match self.inner.process.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                crate::kota_debug_log(&format!(
                    "[agent:{}] process mutex poisoned during resize; recovering",
                    self.inner.agent_id,
                ));
                self.inner.process.clear_poison();
                poisoned.into_inner()
            }
        };
        let Some(process) = process_guard.as_mut() else {
            return Ok(());
        };
        // master.resize can fail if the PTY fd is already gone
        // (race with respawn / kill). Drop the error.
        let _ = process.master.resize(size);
        let mut decoder = match process.decoder.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                crate::kota_debug_log(&format!(
                    "[agent:{}] decoder mutex poisoned during resize; recovering",
                    self.inner.agent_id,
                ));
                process.decoder.clear_poison();
                poisoned.into_inner()
            }
        };
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            decoder.resize(size.cols, size.rows);
        }))
        .is_err()
        {
            crate::kota_debug_log(&format!(
                "[agent:{}] decoder resize panicked at {}x{}; resetting decoder",
                self.inner.agent_id, size.cols, size.rows,
            ));
            *decoder = AnsiLineDecoder::new(size.cols, size.rows);
        }
        Ok(())
    }

    pub fn scroll(&self, app: &AppHandle, lines: i32) -> Result<()> {
        let Some(snapshot) = ({
            let mut process_guard = match self.inner.process.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    crate::kota_debug_log(&format!(
                        "[agent:{}] process mutex poisoned during scroll; recovering",
                        self.inner.agent_id,
                    ));
                    self.inner.process.clear_poison();
                    poisoned.into_inner()
                }
            };
            let Some(process) = process_guard.as_mut() else {
                return Ok(());
            };
            let mut decoder = match process.decoder.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    crate::kota_debug_log(&format!(
                        "[agent:{}] decoder mutex poisoned during scroll; recovering",
                        self.inner.agent_id,
                    ));
                    process.decoder.clear_poison();
                    poisoned.into_inner()
                }
            };
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                decoder.scroll_display(lines);
            }))
            .is_err()
            {
                crate::kota_debug_log(&format!(
                    "[agent:{}] decoder scroll panicked; resetting decoder",
                    self.inner.agent_id,
                ));
                *decoder = AnsiLineDecoder::new(
                    self.inner
                        .size
                        .lock()
                        .map(|s| s.cols)
                        .unwrap_or(DEFAULT_COLS),
                    self.inner
                        .size
                        .lock()
                        .map(|s| s.rows)
                        .unwrap_or(DEFAULT_ROWS),
                );
                None
            } else {
                Some(decoder.snapshot())
            }
        }) else {
            return Ok(());
        };

        let _ = app.emit(
            &self.inner.route.output_event,
            AgentOutputEvent {
                agent_id: self.inner.agent_id.clone(),
                snapshot,
            },
        );
        Ok(())
    }

    pub fn interrupt(&self, app: &AppHandle) -> Result<()> {
        let mut process_guard = self.inner.process.lock().expect("agent process poisoned");
        let process = process_guard
            .as_mut()
            .ok_or_else(|| anyhow!("agent process missing for {}", self.inner.agent_id))?;
        let mut writer = process.writer.lock().expect("agent writer poisoned");
        writer.write_all(&[0x03])?;
        writer.flush()?;
        self.inner.clear_next_submit.store(true, Ordering::Release);
        self.emit_work_state(app, "interrupted", Some("interrupt".into()), None);
        Ok(())
    }

    pub fn close(&self, app: &AppHandle) -> Result<()> {
        let child_pid = self.stop_current();
        let lease_path = self
            .inner
            .shared_dir
            .join(".kota")
            .join("agent-session-leases")
            .join(format!(
                "{}.json",
                safe_lease_filename(&self.inner.agent_id)
            ));
        remove_agent_session_lease(&lease_path, std::process::id(), child_pid);
        self.emit_exit(app, None);
        self.emit_status(app, false);
        Ok(())
    }

    fn spawn(&self, app: &AppHandle) -> Result<()> {
        self.stop_current();
        if let Err(err) =
            prepare_provider_workspace(self.inner.cli, &self.inner.cwd, &self.inner.home)
        {
            crate::kota_debug_log(&format!(
                "[agent:{}] provider workspace preparation failed: {err}",
                self.inner.agent_id
            ));
        }

        let spawn_t0 = std::time::Instant::now();
        let size = *self.inner.size.lock().expect("agent size poisoned");
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(size)
            .with_context(|| format!("open agent PTY for {}", self.inner.agent_id))?;

        let bin = self.inner.cli.bin();
        let env_path = augmented_path(Some(&self.inner.home));
        let resolved_bin = resolve_on_augmented_path(bin, Some(&self.inner.home));
        let opencode_launch_mode =
            resolve_opencode_launch_mode(self.inner.cli, resolved_bin.as_str());
        let mut cmd = CommandBuilder::new(resolved_bin.as_str());
        cmd.cwd(&self.inner.cwd);
        let spawn_args = args_for_spawn_with_opencode_launch_mode(
            self.inner.cli,
            &self.inner.args,
            &self.inner.cwd,
            self.inner.session_id.as_deref(),
            opencode_launch_mode,
        );
        for arg in &spawn_args {
            cmd.arg(arg.as_str());
        }

        // CommandBuilder seeds from the process environment at creation,
        // but copy it explicitly so late process-env changes are reflected
        // before we apply Kota's terminal/persona overrides below. When Kota
        // itself was launched from an agent, scrub provider runtime/session env
        // first; otherwise child agents can inherit the host agent's session id.
        for (k, v) in std::env::vars() {
            if should_inherit_agent_spawn_env(&k) {
                cmd.env(k, v);
            } else {
                cmd.env_remove(k);
            }
        }
        cmd.env("PATH", env_path);
        cmd.env("PWD", path_str(&self.inner.cwd));

        // Standard terminal env (override anything inherited). TERM_PROGRAM
        // gives CLIs that fingerprint their host terminal a clear interactive
        // signal; CI-style env flags are explicitly cleared. Kota owns this PTY,
        // so remove inherited NO_COLOR before forcing color to avoid conflicting
        // env hints for Node-based CLIs.
        cmd.env("TERM_PROGRAM", "Kota");
        cmd.env_remove("NO_COLOR");
        cmd.env("FORCE_COLOR", "1");
        cmd.env("CLICOLOR_FORCE", "1");
        cmd.env_remove("CI");
        cmd.env_remove("BUILDKITE");
        cmd.env_remove("GITHUB_ACTIONS");
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        if let Some(config) = opencode_inline_config_env(self.inner.cli, &self.inner.project_root) {
            cmd.env("OPENCODE_CONFIG_CONTENT", config);
        }

        // Per I-15: KOTA path env vars.
        cmd.env("KOTA_AGENT_CWD", path_str(&self.inner.cwd));
        cmd.env("KOTA_PROJECT_ROOT", path_str(&self.inner.project_root));
        cmd.env("KOTA_WORKTREE_ROOT", path_str(&self.inner.worktree_root));
        cmd.env("KOTA_PROJECT_MEMORY_DIR", path_str(&self.inner.shared_dir));
        cmd.env("KOTA_PROJECT_RULES_DIR", path_str(&self.inner.rules_dir));
        cmd.env("KOTA_HOME", path_str(&self.inner.home.join("Kota")));
        cmd.env(
            "KOTA_ACCOUNT_RULES_DIR",
            path_str(&self.inner.home.join("Kota").join("rules")),
        );
        cmd.env("KOTA_AGENT_ID", &self.inner.agent_id);
        cmd.env(
            "KOTA_BBS_ROOT",
            path_str(&self.inner.home.join("Kota").join("Workspaces").join("bbs")),
        );
        if let Some(adapter_path) = self.inner.adapter_path.as_deref() {
            cmd.env("KOTA_AGENT_ADAPTER", path_str(adapter_path));
        }
        if let Some(project_id) = self.inner.project_id.as_deref() {
            cmd.env("KOTA_PROJECT_ID", project_id);
            cmd.env(
                "KOTA_PROJECT_DISPLAY_NAME",
                crate::bbs::display_project_name_with_fallback(project_id, None),
            );
        }
        if let Some(project_remote) = self.inner.project_remote.as_deref() {
            cmd.env("KOTA_PROJECT_REMOTE", project_remote);
        }
        if let Some(project_base_ref) = self.inner.project_base_ref.as_deref() {
            cmd.env("KOTA_PROJECT_BASE_REF", project_base_ref);
        }

        // Per I-26: agent commits authored as {id}@kota.local.
        cmd.env(
            "GIT_AUTHOR_EMAIL",
            format!("{}@kota.local", self.inner.agent_id),
        );
        cmd.env(
            "GIT_AUTHOR_NAME",
            format!("{} (Agent)", self.inner.agent_id),
        );
        cmd.env(
            "GIT_COMMITTER_EMAIL",
            format!("{}@kota.local", self.inner.agent_id),
        );
        cmd.env(
            "GIT_COMMITTER_NAME",
            format!("{} (Agent)", self.inner.agent_id),
        );

        #[cfg(unix)]
        let tty_name = pair
            .master
            .tty_name()
            .map(|path| path.display().to_string());
        #[cfg(not(unix))]
        let tty_name: Option<String> = None;
        crate::kota_debug_log(&format!(
            "[agent:{}] spawn bin={} resolved={} session_id={:?} args={:?} cwd={} cols={} rows={} tty={:?} HOME={:?} PATH-len={} TERM_PROGRAM={:?} CI={:?}",
            self.inner.agent_id,
            bin,
            resolved_bin,
            self.inner.session_id,
            spawn_args,
            self.inner.cwd.display(),
            size.cols,
            size.rows,
            tty_name,
            cmd.get_env("HOME")
                .map(|v| v.to_string_lossy().into_owned()),
            cmd.get_env("PATH")
                .map(|p| p.to_string_lossy().len())
                .unwrap_or(0),
            cmd.get_env("TERM_PROGRAM")
                .map(|v| v.to_string_lossy().into_owned()),
            cmd.get_env("CI").map(|v| v.to_string_lossy().into_owned()),
        ));

        let mut child = pair.slave.spawn_command(cmd).with_context(|| {
            format!(
                "spawn `{}` ({}) for agent {}",
                bin, resolved_bin, self.inner.agent_id
            )
        })?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().context("clone PTY reader")?;
        let writer = Arc::new(Mutex::new(
            pair.master.take_writer().context("take PTY writer")?,
        ));
        let child_pid = child.process_id();
        let killer = child.clone_killer();
        let decoder = Arc::new(Mutex::new(AnsiLineDecoder::new(size.cols, size.rows)));
        // Same throttle pattern as smart.rs — reader sets dirty, emitter
        // ticks at 30 fps and snapshots only when dirty. CC banner /
        // Codex onboarding can produce dozens of snapshots per second.
        let dirty = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let generation = self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let state = Arc::clone(&self.inner);
        let decode_handle = Arc::clone(&decoder);
        let dirty_handle = Arc::clone(&dirty);
        let writer_handle = Arc::clone(&writer);
        let mut trust_prompt_auto_accept =
            StartupTrustPromptAutoAccept::new(self.inner.cli, &spawn_args, &self.inner.cwd);
        // Native terminals answer basic capability/cursor queries. Codex
        // asks for CPR during boot and waits if nobody answers, which makes
        // Kota look much slower than macOS Terminal.
        let answer_terminal_queries = true;
        let status_handle = app.clone();
        let resolved_bin_for_status = resolved_bin.clone();
        let tty_name_for_status = tty_name.clone();

        let agent_id_for_reader = self.inner.agent_id.clone();
        thread::Builder::new()
            .name(format!("kota-agent-pty-reader-{}", state.agent_id))
            .spawn(move || {
                let mut buffer = [0u8; 8192];
                let mut first_bytes_logged = false;
                let mut query_tail: Vec<u8> = Vec::new();
                let mut query_state = TerminalQueryState::default();
                loop {
                    if state.generation.load(Ordering::SeqCst) != generation {
                        break;
                    }
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => {
                            if !first_bytes_logged {
                                first_bytes_logged = true;
                                let elapsed = spawn_t0.elapsed().as_millis();
                                let preview: String = buffer[..n.min(48)]
                                    .iter()
                                    .map(|b| {
                                        if b.is_ascii_graphic() || *b == b' ' {
                                            (*b as char).to_string()
                                        } else {
                                            format!("\\x{:02x}", b)
                                        }
                                    })
                                    .collect();
                                crate::kota_debug_log(&format!(
                                    "[agent:{}] first-bytes after {}ms len={} preview={:?}",
                                    agent_id_for_reader, elapsed, n, preview,
                                ));
                                let _ = status_handle.emit(
                                    &state.route.status_event,
                                    AgentStatusEvent {
                                        agent_id: state.agent_id.clone(),
                                        running: true,
                                        cli: state.cli,
                                        cwd: state.cwd.display().to_string(),
                                        phase: Some(AgentStatusPhase::FirstBytes),
                                        detail: Some(format!(
                                            "first PTY read: {n} bytes after {elapsed}ms"
                                        )),
                                        resolved_bin: Some(resolved_bin_for_status.clone()),
                                        tty_name: tty_name_for_status.clone(),
                                        elapsed_ms: Some(elapsed as u64),
                                        byte_count: Some(n),
                                        session_id: state.session_id.clone(),
                                        forkable: Some(state.session_id.is_some()),
                                        session_source: state
                                            .session_id
                                            .as_ref()
                                            .map(|_| "manual".to_string()),
                                    },
                                );
                            }
                            trust_prompt_auto_accept.observe(
                                &buffer[..n],
                                &writer_handle,
                                &agent_id_for_reader,
                                spawn_t0,
                            );
                            {
                                let mut dec = match decode_handle.lock() {
                                    Ok(guard) => guard,
                                    Err(poisoned) => {
                                        crate::kota_debug_log(&format!(
                                            "[agent:{}] decoder mutex poisoned in reader; recovering",
                                            agent_id_for_reader,
                                        ));
                                        decode_handle.clear_poison();
                                        poisoned.into_inner()
                                    }
                                };
                                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    let _ = dec.push_bytes(&buffer[..n]);
                                }))
                                .is_err()
                                {
                                    let size = state
                                        .size
                                        .lock()
                                        .map(|s| *s)
                                        .unwrap_or(PtySize {
                                            cols: DEFAULT_COLS,
                                            rows: DEFAULT_ROWS,
                                            pixel_width: 0,
                                            pixel_height: 0,
                                        });
                                    crate::kota_debug_log(&format!(
                                        "[agent:{}] decoder push panicked; resetting decoder",
                                        agent_id_for_reader,
                                    ));
                                    *dec = AnsiLineDecoder::new(size.cols, size.rows);
                                }
                            }
                            if answer_terminal_queries {
                                let previous_query_tail = query_tail.clone();
                                query_tail.extend_from_slice(&buffer[..n]);
                                if query_tail.len() > 512 {
                                    let keep_from = query_tail.len() - 512;
                                    query_tail.drain(0..keep_from);
                                }
                                let cursor_reports = count_bytes(
                                    &query_tail,
                                    CURSOR_POSITION_QUERY,
                                )
                                .saturating_sub(count_bytes(
                                    &previous_query_tail,
                                    CURSOR_POSITION_QUERY,
                                ));
                                let cursor = if cursor_reports > 0 {
                                    decoder_cursor_report(
                                        &decode_handle,
                                        &agent_id_for_reader,
                                    )
                                } else {
                                    (1, 1)
                                };
                                let responses =
                                    terminal_query_responses(
                                        &query_tail,
                                        &mut query_state,
                                        cursor,
                                        cursor_reports,
                                    );
                                if !responses.is_empty() {
                                    if let Ok(mut writer) = writer_handle.lock() {
                                        let _ = writer.write_all(&responses);
                                        let _ = writer.flush();
                                    }
                                    let elapsed = elapsed_ms(spawn_t0);
                                    crate::kota_debug_log(&format!(
                                        "[agent:{}] terminal query response after {}ms len={} cursor_reports={}",
                                        agent_id_for_reader,
                                        elapsed,
                                        responses.len(),
                                        cursor_reports,
                                    ));
                                    let detail = if cursor_reports > 0 {
                                        format!(
                                            "answered terminal query: {} bytes ({} cursor position report{})",
                                            responses.len(),
                                            cursor_reports,
                                            if cursor_reports == 1 { "" } else { "s" },
                                        )
                                    } else {
                                        format!(
                                            "answered terminal query: {} bytes",
                                            responses.len(),
                                        )
                                    };
                                    let _ = status_handle.emit(
                                        &state.route.status_event,
                                        AgentStatusEvent {
                                            agent_id: state.agent_id.clone(),
                                            running: true,
                                            cli: state.cli,
                                            cwd: state.cwd.display().to_string(),
                                            phase: Some(AgentStatusPhase::TerminalQuery),
                                            detail: Some(detail),
                                            resolved_bin: Some(resolved_bin_for_status.clone()),
                                            tty_name: tty_name_for_status.clone(),
                                            elapsed_ms: Some(elapsed),
                                            byte_count: Some(responses.len()),
                                            session_id: state.session_id.clone(),
                                            forkable: Some(state.session_id.is_some()),
                                            session_source: state
                                                .session_id
                                                .as_ref()
                                                .map(|_| "manual".to_string()),
                                        },
                                    );
                                }
                            }
                            dirty_handle.store(true, Ordering::Release);
                            match state.output_signal.epoch.lock() {
                                Ok(mut epoch) => {
                                    *epoch = epoch.saturating_add(1);
                                    state.output_signal.changed.notify_all();
                                }
                                Err(poisoned) => {
                                    state.output_signal.epoch.clear_poison();
                                    let mut epoch = poisoned.into_inner();
                                    *epoch = epoch.saturating_add(1);
                                    state.output_signal.changed.notify_all();
                                }
                            }
                        }
                        Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
                crate::kota_debug_log(&format!(
                    "[agent:{}] reader thread exit after {}ms",
                    agent_id_for_reader,
                    spawn_t0.elapsed().as_millis(),
                ));
            })
            .with_context(|| format!("spawn agent reader thread for {}", self.inner.agent_id))?;

        // Emitter: 30 fps trailing-edge throttle.
        let state = Arc::clone(&self.inner);
        let app_handle = app.clone();
        let decode_handle = Arc::clone(&decoder);
        let dirty_handle = Arc::clone(&dirty);
        thread::Builder::new()
            .name(format!("kota-agent-pty-emitter-{}", state.agent_id))
            .spawn(move || {
                let frame = std::time::Duration::from_millis(33);
                loop {
                    if state.generation.load(Ordering::SeqCst) != generation {
                        break;
                    }
                    std::thread::sleep(frame);
                    if !dirty_handle.swap(false, Ordering::AcqRel) {
                        continue;
                    }
                    let snapshot = {
                        let mut dec = match decode_handle.lock() {
                            Ok(guard) => guard,
                            Err(poisoned) => {
                                crate::kota_debug_log(&format!(
                                    "[agent:{}] decoder mutex poisoned in emitter; recovering",
                                    state.agent_id,
                                ));
                                decode_handle.clear_poison();
                                poisoned.into_inner()
                            }
                        };
                        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            dec.snapshot()
                        })) {
                            Ok(snapshot) => snapshot,
                            Err(_) => {
                                let size = state.size.lock().map(|s| *s).unwrap_or(PtySize {
                                    cols: DEFAULT_COLS,
                                    rows: DEFAULT_ROWS,
                                    pixel_width: 0,
                                    pixel_height: 0,
                                });
                                crate::kota_debug_log(&format!(
                                    "[agent:{}] decoder snapshot panicked; resetting decoder",
                                    state.agent_id,
                                ));
                                *dec = AnsiLineDecoder::new(size.cols, size.rows);
                                continue;
                            }
                        }
                    };
                    let _ = app_handle.emit(
                        &state.route.output_event,
                        AgentOutputEvent {
                            agent_id: state.agent_id.clone(),
                            snapshot,
                        },
                    );
                }
            })
            .with_context(|| format!("spawn agent emitter thread for {}", self.inner.agent_id))?;

        let state = Arc::clone(&self.inner);
        let app_handle = app.clone();
        let lease_path = state
            .shared_dir
            .join(".kota")
            .join("agent-session-leases")
            .join(format!("{}.json", safe_lease_filename(&state.agent_id)));
        thread::Builder::new()
            .name(format!("kota-agent-pty-wait-{}", state.agent_id))
            .spawn(move || {
                let exit_code = child.wait().ok().and_then(exit_status_code);
                if state.generation.load(Ordering::SeqCst) != generation {
                    return;
                }

                state.process.lock().expect("agent process poisoned").take();
                remove_agent_session_lease(&lease_path, std::process::id(), child_pid);
                let _ = app_handle.emit(
                    &state.route.exit_event,
                    AgentExitEvent {
                        agent_id: state.agent_id.clone(),
                        code: exit_code,
                    },
                );
                let _ = app_handle.emit(
                    &state.route.status_event,
                    AgentStatusEvent {
                        agent_id: state.agent_id.clone(),
                        running: false,
                        cli: state.cli,
                        cwd: state.cwd.display().to_string(),
                        phase: Some(AgentStatusPhase::Exited),
                        detail: Some(match exit_code {
                            Some(code) => format!("process exited with code {code}"),
                            None => "process exited".to_string(),
                        }),
                        resolved_bin: None,
                        tty_name: None,
                        elapsed_ms: None,
                        byte_count: None,
                        session_id: state.session_id.clone(),
                        forkable: Some(state.session_id.is_some()),
                        session_source: state.session_id.as_ref().map(|_| "manual".to_string()),
                    },
                );
            })
            .with_context(|| format!("spawn agent wait thread for {}", self.inner.agent_id))?;

        self.inner
            .process
            .lock()
            .expect("agent process poisoned")
            .replace(AgentProcess {
                master: pair.master,
                writer,
                killer,
                decoder,
                child_pid,
            });

        self.emit_status_detail(
            app,
            true,
            Some(AgentStatusPhase::Spawned),
            Some("process spawned; waiting for PTY output".to_string()),
            Some(resolved_bin),
            tty_name,
            Some(elapsed_ms(spawn_t0)),
            None,
        );
        Ok(())
    }

    fn stop_current(&self) -> Option<u32> {
        self.inner.generation.fetch_add(1, Ordering::SeqCst);
        if let Some(mut process) = self
            .inner
            .process
            .lock()
            .expect("agent process poisoned")
            .take()
        {
            let child_pid = process.child_pid;
            let _ = process.killer.kill();
            child_pid
        } else {
            None
        }
    }

    /// Kill the child without emitting an exit event. Used by the
    /// registry on respawn so the frontend's exit listener (which is
    /// keyed by agent_id, same topic for old and new) doesn't see a
    /// phantom exit and clear `liveAgents`.
    pub fn stop_silently(&self) {
        self.stop_current();
    }

    fn emit_status(&self, app: &AppHandle, running: bool) {
        self.emit_status_detail(app, running, None, None, None, None, None, None);
    }

    fn emit_status_detail(
        &self,
        app: &AppHandle,
        running: bool,
        phase: Option<AgentStatusPhase>,
        detail: Option<String>,
        resolved_bin: Option<String>,
        tty_name: Option<String>,
        elapsed_ms: Option<u64>,
        byte_count: Option<usize>,
    ) {
        let _ = app.emit(
            &self.inner.route.status_event,
            AgentStatusEvent {
                agent_id: self.inner.agent_id.clone(),
                running,
                cli: self.inner.cli,
                cwd: self.inner.cwd.display().to_string(),
                phase,
                detail,
                resolved_bin,
                tty_name,
                elapsed_ms,
                byte_count,
                session_id: self.inner.session_id.clone(),
                forkable: Some(self.inner.session_id.is_some()),
                session_source: self.inner.session_id.as_ref().map(|_| "manual".to_string()),
            },
        );
    }

    fn emit_exit(&self, app: &AppHandle, code: Option<i32>) {
        let _ = app.emit(
            &self.inner.route.exit_event,
            AgentExitEvent {
                agent_id: self.inner.agent_id.clone(),
                code,
            },
        );
    }

    fn emit_work_state(
        &self,
        app: &AppHandle,
        state: &str,
        reason: Option<String>,
        turn_id: Option<String>,
    ) {
        let _ = app.emit(
            &self.inner.route.work_event,
            AgentWorkStateEvent {
                agent_id: self.inner.agent_id.clone(),
                state: state.to_string(),
                timestamp: Utc::now().to_rfc3339(),
                cli: Some(self.inner.cli),
                cwd: Some(self.inner.cwd.display().to_string()),
                session_id: self.inner.session_id.clone(),
                turn_id,
                reason,
                source_path: None,
                native_event_id: None,
            },
        );
    }
}

fn path_str(p: &Path) -> String {
    p.display().to_string()
}

fn exit_status_code(status: portable_pty::ExitStatus) -> Option<i32> {
    if status.signal().is_some() {
        return None;
    }
    Some(status.exit_code() as i32)
}

fn elapsed_ms(t0: std::time::Instant) -> u64 {
    t0.elapsed().as_millis().min(u64::MAX as u128) as u64
}

#[derive(Default)]
struct TerminalQueryState {
    bg: bool,
    name: bool,
    modify_other_keys: bool,
    device_attributes: bool,
}

fn decoder_cursor_report(decoder: &Arc<Mutex<AnsiLineDecoder>>, agent_id: &str) -> (u16, u16) {
    let dec = match decoder.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            crate::kota_debug_log(&format!(
                "[agent:{}] decoder mutex poisoned during cursor query; recovering",
                agent_id,
            ));
            decoder.clear_poison();
            poisoned.into_inner()
        }
    };

    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| dec.snapshot())) {
        Ok(snapshot) => (
            snapshot.cursor_row.saturating_add(1).max(1),
            snapshot.cursor_col.saturating_add(1).max(1),
        ),
        Err(_) => {
            crate::kota_debug_log(&format!(
                "[agent:{}] decoder snapshot panicked during cursor query; using 1;1",
                agent_id,
            ));
            (1, 1)
        }
    }
}

fn terminal_query_responses(
    bytes: &[u8],
    state: &mut TerminalQueryState,
    cursor: (u16, u16),
    cursor_position_reports: usize,
) -> Vec<u8> {
    let mut out = Vec::new();

    if !state.bg && contains_bytes(bytes, OSC_11_QUERY) {
        state.bg = true;
        out.extend_from_slice(OSC_11_RESPONSE);
    }
    if !state.name && contains_bytes(bytes, TERMINAL_NAME_QUERY) {
        state.name = true;
        out.extend_from_slice(TERMINAL_NAME_RESPONSE);
    }
    if !state.modify_other_keys && contains_bytes(bytes, MODIFY_OTHER_KEYS_QUERY) {
        state.modify_other_keys = true;
        out.extend_from_slice(MODIFY_OTHER_KEYS_RESPONSE);
    }
    if !state.device_attributes && contains_bytes(bytes, DEVICE_ATTRIBUTES_QUERY) {
        state.device_attributes = true;
        out.extend_from_slice(DEVICE_ATTRIBUTES_RESPONSE);
    }
    if cursor_position_reports > 0 {
        let response = format!("\x1b[{};{}R", cursor.0.max(1), cursor.1.max(1));
        for _ in 0..cursor_position_reports {
            out.extend_from_slice(response.as_bytes());
        }
    }

    out
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|part| part == needle)
}

fn count_bytes(haystack: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() {
        return 0;
    }
    haystack
        .windows(needle.len())
        .filter(|part| *part == needle)
        .count()
}

fn should_inherit_agent_spawn_env(key: &str) -> bool {
    if key == "PWD" || key == "OLDPWD" || key == "AI_AGENT" || key.starts_with("KOTA_") {
        return false;
    }
    if key == "CLAUDECODE" || key == "CLAUDE_EFFORT" || key.starts_with("CLAUDE_CODE_") {
        return false;
    }
    if key.starts_with("CODEX_") {
        return false;
    }
    if key.starts_with("OPENCODE_") || key == "OPENCODE" {
        return false;
    }
    if key.starts_with("ANTIGRAVITY_") || key.starts_with("AGY_") {
        return false;
    }
    if key == "PI_CODING_AGENT" {
        return false;
    }
    if key == "KIMI_SESSION_ID" || key == "KIMI_WORK_DIR" {
        return false;
    }
    true
}

fn opencode_inline_config_env(cli: AgentCli, project_root: &Path) -> Option<String> {
    if cli != AgentCli::Opencode {
        return None;
    }
    if project_root.as_os_str().is_empty() {
        return None;
    }
    let pattern = format!("{}/**", path_str(project_root));
    let mut external_directory = serde_json::Map::new();
    external_directory.insert(pattern, serde_json::Value::String("allow".into()));
    let payload = serde_json::json!({
        "permission": {
            "*": "allow",
            "external_directory": external_directory,
        },
    });
    serde_json::to_string(&payload).ok()
}

/// Per-Agent registry — analog of SmartPool in manager.rs but keyed by agent_id.
#[derive(Default)]
pub struct AgentRegistry {
    ptys: Mutex<HashMap<String, AgentTerminalPty>>,
}

impl AgentRegistry {
    pub fn spawn(&self, app: &AppHandle, req: AgentSpawnRequest) -> Result<AgentRoute> {
        let agent_id = req.agent_id.clone();
        let lease_path = agent_session_lease_path(&req);
        if let Some(lease) = active_foreign_agent_session_lease(&lease_path) {
            if req.takeover {
                terminate_foreign_agent_session(&lease);
            } else {
                return Err(lease_conflict_error(&lease));
            }
        }
        let lease_cli = req.cli;
        let lease_cwd = req.cwd.clone();
        let lease_project_root = req.project_root.clone();
        let lease_session_id = req.session_id.clone();
        // If an existing pty for this agent_id is still alive, kill it
        // silently (recruit cycle / restart). We must NOT call close()
        // here — close() emits an exit event on the agent_id-keyed
        // topic, which the frontend's brand-new exit listener (registered
        // before the spawn IPC call) catches and clears liveAgents.
        if let Some(existing) = self
            .ptys
            .lock()
            .expect("agent registry poisoned")
            .remove(&agent_id)
        {
            existing.stop_silently();
        }

        let pty = AgentTerminalPty::new(req)?;
        let route = pty.route();
        pty.init(app)?;
        let now = Utc::now().to_rfc3339();
        if let Err(err) = write_agent_session_lease(
            &lease_path,
            &AgentSessionLease {
                version: 1,
                agent_id: agent_id.clone(),
                cli: lease_cli,
                cwd: lease_cwd,
                project_root: lease_project_root,
                owner_pid: std::process::id(),
                child_pid: pty.child_pid(),
                session_id: lease_session_id,
                created_at: now.clone(),
                updated_at: now,
            },
        ) {
            pty.stop_silently();
            return Err(err);
        }
        self.ptys
            .lock()
            .expect("agent registry poisoned")
            .insert(agent_id, pty);
        Ok(route)
    }

    pub fn write(&self, app: &AppHandle, agent_id: &str, input: String) -> Result<()> {
        self.lookup(agent_id)?.write(app, input)
    }

    pub fn submit_prompt(&self, app: &AppHandle, agent_id: &str, input: String) -> Result<()> {
        self.lookup(agent_id)?.submit_prompt(app, input)
    }

    pub fn resize(&self, agent_id: &str, cols: u16, rows: u16) -> Result<()> {
        self.lookup(agent_id)?.resize(cols, rows)
    }

    pub fn scroll(&self, app: &AppHandle, agent_id: &str, lines: i32) -> Result<()> {
        self.lookup(agent_id)?.scroll(app, lines)
    }

    pub fn interrupt(&self, app: &AppHandle, agent_id: &str) -> Result<()> {
        self.lookup(agent_id)?.interrupt(app)
    }

    pub fn close(&self, app: &AppHandle, agent_id: &str) -> Result<()> {
        let pty = self
            .ptys
            .lock()
            .expect("agent registry poisoned")
            .remove(agent_id)
            .ok_or_else(|| anyhow!("unknown agent: {agent_id}"))?;
        pty.close(app)
    }

    pub fn list(&self) -> Vec<AgentSummary> {
        let mut summaries = self
            .ptys
            .lock()
            .expect("agent registry poisoned")
            .values()
            .map(AgentTerminalPty::summary)
            .collect::<Vec<_>>();
        summaries.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
        summaries
    }

    pub fn route_for(&self, agent_id: &str) -> Option<AgentRoute> {
        self.ptys
            .lock()
            .expect("agent registry poisoned")
            .get(agent_id)
            .map(AgentTerminalPty::route)
    }

    fn lookup(&self, agent_id: &str) -> Result<AgentTerminalPty> {
        self.ptys
            .lock()
            .expect("agent registry poisoned")
            .get(agent_id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown agent: {agent_id}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_bin_names() {
        assert_eq!(AgentCli::Claude.bin(), "claude");
        assert_eq!(AgentCli::Codex.bin(), "codex");
        assert_eq!(AgentCli::Antigravity.bin(), "agy");
        assert_eq!(AgentCli::Opencode.bin(), "opencode");
        assert_eq!(AgentCli::Pi.bin(), "pi");
        assert_eq!(AgentCli::Kimi.bin(), "kimi");
    }

    #[cfg(unix)]
    #[test]
    fn process_is_alive_tracks_child_lifecycle() {
        // A running child reads alive; once killed and reaped it reads dead.
        // This guards the submit-time liveness check that gates session
        // recovery — a false "dead" here would respawn-kill a healthy agent.
        let mut child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();
        assert!(process_is_alive(pid), "running child should read alive");
        child.kill().expect("kill child");
        child.wait().expect("reap child");
        assert!(!process_is_alive(pid), "reaped child should read dead");
    }

    #[test]
    fn legacy_gemini_cli_deserializes_as_antigravity() {
        assert_eq!(
            serde_json::from_str::<AgentCli>("\"gemini\"").unwrap(),
            AgentCli::Antigravity
        );
        assert_eq!(
            serde_json::from_str::<AgentCli>("\"gemini-cli\"").unwrap(),
            AgentCli::Antigravity
        );
    }

    #[test]
    fn ensure_claude_kota_hooks_installs_ask_user_question_capture() {
        let cwd = std::env::temp_dir().join(format!("kota-claude-hooks-{}", std::process::id()));
        let _ = fs::remove_dir_all(&cwd);
        fs::create_dir_all(&cwd).unwrap();

        ensure_claude_kota_hooks(&cwd).unwrap();
        ensure_claude_kota_hooks(&cwd).unwrap();

        let script_path = cwd
            .join(".claude")
            .join("kota-hooks")
            .join("capture-ask-user-question.mjs");
        assert!(script_path.is_file());
        let settings_path = cwd.join(".claude").join("settings.json");
        let settings_text = fs::read_to_string(settings_path).unwrap();
        let settings: serde_json::Value = serde_json::from_str(&settings_text).unwrap();
        let hooks = settings
            .get("hooks")
            .and_then(|hooks| hooks.get("PreToolUse"))
            .and_then(serde_json::Value::as_array)
            .unwrap();
        let commands = hooks
            .iter()
            .flat_map(|entry| {
                entry
                    .get("hooks")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .filter_map(|hook| hook.get("command").and_then(serde_json::Value::as_str))
            .filter(|command| command.contains("capture-ask-user-question.mjs"))
            .collect::<Vec<_>>();
        assert_eq!(commands.len(), 1);
        assert!(commands[0].starts_with("node "));

        let _ = fs::remove_dir_all(&cwd);
    }

    #[test]
    fn antigravity_spawn_args_inject_agent_cwd_as_workspace_dir() {
        assert_eq!(
            args_for_spawn(
                AgentCli::Antigravity,
                &["--dangerously-skip-permissions".into()],
                Path::new("/tmp/agent-cwd"),
                None,
            ),
            vec![
                "--add-dir".to_string(),
                "/tmp/agent-cwd".to_string(),
                "--dangerously-skip-permissions".to_string()
            ]
        );
    }

    #[test]
    fn antigravity_spawn_args_preserve_explicit_workspace_dir() {
        assert_eq!(
            args_for_spawn(
                AgentCli::Antigravity,
                &["--add-dir=/tmp/custom".into()],
                Path::new("/tmp/agent-cwd"),
                None,
            ),
            vec![
                "--add-dir=/tmp/custom".to_string(),
                "--dangerously-skip-permissions".to_string()
            ]
        );
    }

    #[test]
    fn antigravity_spawn_args_resume_exact_conversation_and_workspace() {
        assert_eq!(
            args_for_spawn(
                AgentCli::Antigravity,
                &["--dangerously-skip-permissions".into()],
                Path::new("/tmp/agent-cwd"),
                Some("agy-session-1"),
            ),
            vec![
                "--add-dir".to_string(),
                "/tmp/agent-cwd".to_string(),
                "--conversation".to_string(),
                "agy-session-1".to_string(),
                "--dangerously-skip-permissions".to_string()
            ]
        );
    }

    #[test]
    fn antigravity_spawn_args_preserve_explicit_conversation() {
        assert_eq!(
            args_for_spawn(
                AgentCli::Antigravity,
                &["--conversation".into(), "manual".into()],
                Path::new("/tmp/agent-cwd"),
                Some("agy-session-1"),
            ),
            vec![
                "--add-dir".to_string(),
                "/tmp/agent-cwd".to_string(),
                "--conversation".to_string(),
                "manual".to_string(),
                "--dangerously-skip-permissions".to_string()
            ]
        );
    }

    #[test]
    fn codex_spawn_args_resume_exact_session() {
        assert_eq!(
            args_for_spawn(
                AgentCli::Codex,
                &["--ask-for-approval=never".into()],
                Path::new("/tmp/agent-cwd"),
                Some("codex-session-1"),
            ),
            vec![
                "resume".to_string(),
                "codex-session-1".to_string(),
                "--ask-for-approval=never".to_string()
            ]
        );
    }

    #[test]
    fn claude_spawn_args_resume_exact_session() {
        assert_eq!(
            args_for_spawn(
                AgentCli::Claude,
                &["--model".into(), "opus".into()],
                Path::new("/tmp/agent-cwd"),
                Some("claude-session-1"),
            ),
            vec![
                "--resume".to_string(),
                "claude-session-1".to_string(),
                "--model".to_string(),
                "opus".to_string(),
                "--dangerously-skip-permissions".to_string()
            ]
        );
    }

    #[test]
    fn claude_spawn_args_preserve_explicit_resume() {
        assert_eq!(
            args_for_spawn(
                AgentCli::Claude,
                &["--continue".into(), "--model".into(), "opus".into()],
                Path::new("/tmp/agent-cwd"),
                Some("claude-session-1"),
            ),
            vec![
                "--continue".to_string(),
                "--model".to_string(),
                "opus".to_string(),
                "--dangerously-skip-permissions".to_string()
            ]
        );
    }

    #[test]
    fn opencode_spawn_args_resume_exact_session() {
        assert_eq!(
            args_for_spawn(
                AgentCli::Opencode,
                &["--model".into(), "x".into()],
                Path::new("/tmp/agent-cwd"),
                Some("opencode-session-1"),
            ),
            vec![
                "run".to_string(),
                "--interactive".to_string(),
                "--session".to_string(),
                "opencode-session-1".to_string(),
                "--model".to_string(),
                "x".to_string(),
                "--pure".to_string(),
                "--dangerously-skip-permissions".to_string(),
                "--dir".to_string(),
                "/tmp/agent-cwd".to_string()
            ]
        );
    }

    #[test]
    fn pi_spawn_args_resume_exact_session_and_approve_project() {
        assert_eq!(
            args_for_spawn(
                AgentCli::Pi,
                &["--model".into(), "google/gemini-2.5-pro".into()],
                Path::new("/tmp/agent-cwd"),
                Some("pi-session-1"),
            ),
            vec![
                "--session-id".to_string(),
                "pi-session-1".to_string(),
                "--model".to_string(),
                "google/gemini-2.5-pro".to_string(),
                "--approve".to_string()
            ]
        );
    }

    #[test]
    fn pi_spawn_args_preserve_explicit_session_and_no_approve() {
        assert_eq!(
            args_for_spawn(
                AgentCli::Pi,
                &["--session".into(), "manual".into(), "--no-approve".into(),],
                Path::new("/tmp/agent-cwd"),
                Some("pi-session-1"),
            ),
            vec![
                "--session".to_string(),
                "manual".to_string(),
                "--no-approve".to_string()
            ]
        );
    }

    #[test]
    fn kimi_spawn_args_resume_exact_session_and_use_yolo() {
        assert_eq!(
            args_for_spawn(
                AgentCli::Kimi,
                &[],
                Path::new("/tmp/agent-cwd"),
                Some("session_kimi_1"),
            ),
            vec![
                "--session".to_string(),
                "session_kimi_1".to_string(),
                "--yolo".to_string(),
            ]
        );
        assert_eq!(prompt_submit_sequence(AgentCli::Kimi), "\x1b[13;1u");
        assert_eq!(prompt_submit_sequence(AgentCli::Claude), "\r");
    }

    #[test]
    fn kimi_spawn_args_preserve_explicit_session_and_permission_mode() {
        assert_eq!(
            args_for_spawn(
                AgentCli::Kimi,
                &["--session=session_manual".into(), "--auto".into()],
                Path::new("/tmp/agent-cwd"),
                Some("session_kimi_1"),
            ),
            vec!["--session=session_manual".to_string(), "--auto".to_string()]
        );
    }

    #[test]
    fn kimi_session_marker_uses_only_matching_main_agent_wire() {
        let root = std::env::temp_dir().join(format!(
            "kota-kimi-marker-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let cwd = root.join("agent-cwd");
        let kimi_home = root.join("kimi-home");
        let matching = kimi_home.join("sessions/wd_matching/session_kimi_1");
        let child_wire = matching.join("agents/child-1/wire.jsonl");
        let main_wire = matching.join("agents/main/wire.jsonl");
        fs::create_dir_all(child_wire.parent().unwrap()).unwrap();
        fs::create_dir_all(main_wire.parent().unwrap()).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(
            kimi_home.join("workspaces.json"),
            format!(
                r#"{{"version":1,"workspaces":{{"wd_matching":{{"root":{}}}}}}}"#,
                serde_json::to_string(cwd.to_str().unwrap()).unwrap()
            ),
        )
        .unwrap();
        fs::write(
            matching.join("state.json"),
            format!(
                r#"{{"workDir":{}}}"#,
                serde_json::to_string(cwd.to_str().unwrap()).unwrap()
            ),
        )
        .unwrap();
        fs::write(&child_wire, "child").unwrap();
        fs::write(&main_wire, "main").unwrap();

        let marker = kimi_session_marker_in(&kimi_home, &cwd, Some("session_kimi_1"))
            .expect("matching Kimi main wire marker");
        assert!(matches!(marker, NativeSessionMarker::File { path, .. } if path == main_wire));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn opencode_spawn_args_use_pure_direct_interactive_mode() {
        assert_eq!(
            args_for_spawn(
                AgentCli::Opencode,
                &[
                    "--model".into(),
                    "kimi-for-coding/k2p6".into(),
                    "--dangerously-skip-permissions".into()
                ],
                Path::new("/tmp/agent-cwd"),
                None,
            ),
            vec![
                "run".to_string(),
                "--interactive".to_string(),
                "--model".to_string(),
                "kimi-for-coding/k2p6".to_string(),
                "--dangerously-skip-permissions".to_string(),
                "--pure".to_string(),
                "--dir".to_string(),
                "/tmp/agent-cwd".to_string()
            ]
        );
    }

    #[test]
    fn opencode_spawn_args_preserve_explicit_dir() {
        assert_eq!(
            args_for_spawn(
                AgentCli::Opencode,
                &["--dir".into(), "/tmp/manual".into()],
                Path::new("/tmp/agent-cwd"),
                None,
            ),
            vec![
                "run".to_string(),
                "--interactive".to_string(),
                "--dir".to_string(),
                "/tmp/manual".to_string(),
                "--pure".to_string(),
                "--dangerously-skip-permissions".to_string()
            ]
        );
    }

    #[test]
    fn opencode_spawn_args_use_mini_mode_when_interactive_flag_is_unavailable() {
        assert_eq!(
            args_for_spawn_with_opencode_launch_mode(
                AgentCli::Opencode,
                &[
                    "--model".into(),
                    "zai-coding-plan/glm-5.2".into(),
                    "--dangerously-skip-permissions".into()
                ],
                Path::new("/tmp/agent-cwd"),
                None,
                OpencodeLaunchMode::Mini {
                    supports_dangerously_skip_permissions: false,
                    supports_session: true,
                },
            ),
            vec![
                "--mini".to_string(),
                "--model".to_string(),
                "zai-coding-plan/glm-5.2".to_string(),
                "--pure".to_string(),
                "/tmp/agent-cwd".to_string()
            ]
        );
    }

    #[test]
    fn opencode_mini_spawn_args_keep_dangerous_flag_when_supported() {
        assert_eq!(
            args_for_spawn_with_opencode_launch_mode(
                AgentCli::Opencode,
                &["--model".into(), "x".into()],
                Path::new("/tmp/agent-cwd"),
                None,
                OpencodeLaunchMode::Mini {
                    supports_dangerously_skip_permissions: true,
                    supports_session: true,
                },
            ),
            vec![
                "--mini".to_string(),
                "--model".to_string(),
                "x".to_string(),
                "--pure".to_string(),
                "--dangerously-skip-permissions".to_string(),
                "/tmp/agent-cwd".to_string()
            ]
        );
    }

    #[test]
    fn opencode_mini_spawn_args_preserve_session_when_supported() {
        assert_eq!(
            args_for_spawn_with_opencode_launch_mode(
                AgentCli::Opencode,
                &["--model".into(), "x".into()],
                Path::new("/tmp/agent-cwd"),
                Some("opencode-session-1"),
                OpencodeLaunchMode::Mini {
                    supports_dangerously_skip_permissions: false,
                    supports_session: true,
                },
            ),
            vec![
                "--mini".to_string(),
                "--session".to_string(),
                "opencode-session-1".to_string(),
                "--model".to_string(),
                "x".to_string(),
                "--pure".to_string(),
                "/tmp/agent-cwd".to_string()
            ]
        );
    }

    #[test]
    fn opencode_mini_spawn_args_strip_session_when_unsupported() {
        assert_eq!(
            args_for_spawn_with_opencode_launch_mode(
                AgentCli::Opencode,
                &["--model".into(), "x".into()],
                Path::new("/tmp/agent-cwd"),
                Some("opencode-session-1"),
                OpencodeLaunchMode::Mini {
                    supports_dangerously_skip_permissions: false,
                    supports_session: false,
                },
            ),
            vec![
                "--mini".to_string(),
                "--model".to_string(),
                "x".to_string(),
                "--pure".to_string(),
                "/tmp/agent-cwd".to_string()
            ]
        );
    }

    #[test]
    fn opencode_mini_spawn_args_translate_explicit_dir_to_project() {
        assert_eq!(
            args_for_spawn_with_opencode_launch_mode(
                AgentCli::Opencode,
                &["--dir".into(), "/tmp/manual".into()],
                Path::new("/tmp/agent-cwd"),
                None,
                OpencodeLaunchMode::Mini {
                    supports_dangerously_skip_permissions: false,
                    supports_session: true,
                },
            ),
            vec![
                "--mini".to_string(),
                "--pure".to_string(),
                "/tmp/manual".to_string()
            ]
        );
    }

    #[test]
    fn opencode_explicit_subcommand_is_not_rewritten_for_mini_mode() {
        assert_eq!(
            args_for_spawn_with_opencode_launch_mode(
                AgentCli::Opencode,
                &["run".into(), "--model".into(), "x".into()],
                Path::new("/tmp/agent-cwd"),
                None,
                OpencodeLaunchMode::Mini {
                    supports_dangerously_skip_permissions: false,
                    supports_session: true,
                },
            ),
            vec![
                "run".to_string(),
                "--model".to_string(),
                "x".to_string(),
                "--pure".to_string(),
                "--dangerously-skip-permissions".to_string(),
                "--dir".to_string(),
                "/tmp/agent-cwd".to_string()
            ]
        );
    }

    #[test]
    fn non_opencode_spawn_args_ignore_opencode_launch_mode() {
        assert_eq!(
            args_for_spawn_with_opencode_launch_mode(
                AgentCli::Codex,
                &["--ask-for-approval=never".into()],
                Path::new("/tmp/agent-cwd"),
                Some("codex-session-1"),
                OpencodeLaunchMode::Mini {
                    supports_dangerously_skip_permissions: false,
                    supports_session: true,
                },
            ),
            vec![
                "resume".to_string(),
                "codex-session-1".to_string(),
                "--ask-for-approval=never".to_string()
            ]
        );
    }

    #[test]
    fn opencode_run_help_probe_detects_legacy_interactive_flag() {
        assert!(opencode_run_help_supports_interactive(
            "  -i, --interactive  run in direct interactive split-footer mode"
        ));
        assert!(!opencode_run_help_supports_interactive(
            "      --dangerously-skip-permissions  auto-approve permissions\n      --dir  directory"
        ));
    }

    #[test]
    fn opencode_mini_help_probe_detects_session_support() {
        let mini_help = "      --session  session id to continue\n      --model  model to use";
        assert!(opencode_mini_help_supports_session(mini_help));
        assert!(!opencode_mini_help_supports_session(
            "      --model  model to use"
        ));
    }

    #[test]
    fn opencode_mini_help_probe_detects_dangerous_flag_support() {
        let mini_help =
            "      --dangerously-skip-permissions  auto-approve permissions\n      --model";
        assert!(opencode_mini_help_supports_dangerously_skip_permissions(
            mini_help
        ));
        assert!(!opencode_mini_help_supports_dangerously_skip_permissions(
            "      --session  session id to continue"
        ));
    }

    #[test]
    fn spawn_args_strip_stale_permission_flags() {
        assert_eq!(
            args_for_spawn(
                AgentCli::Claude,
                &[
                    "--model".into(),
                    "opus".into(),
                    "--allow-dangerously-skip-permissions".into()
                ],
                Path::new("/tmp/agent-cwd"),
                None,
            ),
            vec![
                "--model".to_string(),
                "opus".to_string(),
                "--dangerously-skip-permissions".to_string()
            ]
        );
        assert_eq!(
            args_for_spawn(
                AgentCli::Codex,
                &[
                    "--model".into(),
                    "gpt-5.5".into(),
                    "--dangerously-bypass-hook-trust".into()
                ],
                Path::new("/tmp/agent-cwd"),
                None,
            ),
            vec![
                "--model".to_string(),
                "gpt-5.5".to_string(),
                "--dangerously-bypass-approvals-and-sandbox".to_string()
            ]
        );
    }

    #[test]
    fn agent_spawn_env_scrubs_parent_provider_session_state() {
        for key in [
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_CODE_CHILD_SESSION",
            "CLAUDECODE",
            "CLAUDE_EFFORT",
            "CODEX_THREAD_ID",
            "CODEX_CI",
            "OPENCODE_CONFIG_CONTENT",
            "OPENCODE_SESSION_ID",
            "ANTIGRAVITY_SESSION_ID",
            "AGY_SESSION_ID",
            "PI_CODING_AGENT",
            "KIMI_SESSION_ID",
            "KIMI_WORK_DIR",
            "KOTA_AGENT_ID",
            "AI_AGENT",
            "PWD",
            "OLDPWD",
        ] {
            assert!(
                !should_inherit_agent_spawn_env(key),
                "{key} should be scrubbed"
            );
        }
        for key in [
            "HOME",
            "PATH",
            "SHELL",
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GEMINI_API_KEY",
            "GOOGLE_API_KEY",
            "PI_CODING_AGENT_DIR",
            "PI_CODING_AGENT_SESSION_DIR",
            "KIMI_CODE_HOME",
            "KIMI_MODEL_NAME",
        ] {
            assert!(
                should_inherit_agent_spawn_env(key),
                "{key} should be inherited"
            );
        }
    }

    #[test]
    fn opencode_inline_config_env_allows_yolo_permissions_and_project_root_external_directory() {
        let config = opencode_inline_config_env(
            AgentCli::Opencode,
            Path::new("/Users/me/Kota/Workspaces/demo"),
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&config).unwrap();
        assert_eq!(value["permission"]["*"], serde_json::json!("allow"));
        assert_eq!(
            value["permission"]["external_directory"]["/Users/me/Kota/Workspaces/demo/**"],
            serde_json::json!("allow")
        );
        assert!(value["permission"]["doom_loop"].is_null());
    }

    #[test]
    fn opencode_inline_config_env_skips_empty_project_root() {
        assert!(opencode_inline_config_env(AgentCli::Opencode, Path::new("")).is_none());
    }

    #[test]
    fn opencode_inline_config_env_skips_non_opencode_cli() {
        assert!(opencode_inline_config_env(
            AgentCli::Claude,
            Path::new("/Users/me/Kota/Workspaces/demo")
        )
        .is_none());
    }

    #[test]
    fn startup_trust_auto_accept_covers_agent_workspaces() {
        assert!(startup_trust_auto_accept_enabled(
            AgentCli::Claude,
            &["--dangerously-skip-permissions".into()],
            Path::new("/Users/me/conductor/workspaces/kota/valencia/.agent-workspaces/alice")
        ));
        assert!(startup_trust_auto_accept_enabled(
            AgentCli::Codex,
            &["--dangerously-bypass-approvals-and-sandbox".into()],
            Path::new("/Users/me/Kota/Workspaces/owner-repo/.agent-workspaces/bob")
        ));
        assert!(startup_trust_auto_accept_enabled(
            AgentCli::Antigravity,
            &["--dangerously-skip-permissions".into()],
            Path::new("/Users/me/Kota/AgentWorkspaces/owner-repo/agy")
        ));
        assert!(!startup_trust_auto_accept_enabled(
            AgentCli::Claude,
            &["--dangerously-skip-permissions".into()],
            Path::new("/Users/me/Downloads/random")
        ));
    }

    #[test]
    fn route_event_names_use_agent_id() {
        let req = AgentSpawnRequest {
            agent_id: "alice".into(),
            cli: AgentCli::Claude,
            cwd: "/tmp".into(),
            project_root: "/tmp".into(),
            worktree_root: None,
            shared_dir: None,
            rules_dir: None,
            adapter_path: None,
            args: Vec::new(),
            session_id: None,
            project_id: None,
            project_remote: None,
            project_base_ref: None,
            takeover: false,
        };
        // PathBuf::exists won't fail for /tmp on test hosts; if it does the test
        // is irrelevant, so skip.
        let pty = match AgentTerminalPty::new(req) {
            Ok(p) => p,
            Err(_) => return,
        };
        let route = pty.route();
        assert_eq!(route.agent_id, "alice");
        assert_eq!(route.output_event, "pty://agent/alice/output");
        assert_eq!(route.exit_event, "pty://agent/alice/exit");
        assert_eq!(route.status_event, "pty://agent/alice/status");
        assert_eq!(route.work_event, "pty://agent/alice/work");
    }

    #[test]
    fn terminal_query_responses_include_cursor_position_report() {
        let mut state = TerminalQueryState::default();
        let out = terminal_query_responses(CURSOR_POSITION_QUERY, &mut state, (7, 9), 1);
        assert_eq!(out, b"\x1b[7;9R");
    }

    #[test]
    fn cursor_position_query_count_ignores_old_tail() {
        let previous = b"\x1b[6n";
        let combined = b"\x1b[6nhello\x1b[6n";
        assert_eq!(
            count_bytes(combined, CURSOR_POSITION_QUERY)
                .saturating_sub(count_bytes(previous, CURSOR_POSITION_QUERY)),
            1
        );
    }

    #[test]
    fn one_time_terminal_queries_are_not_repeated() {
        let mut state = TerminalQueryState::default();
        let first = terminal_query_responses(DEVICE_ATTRIBUTES_QUERY, &mut state, (1, 1), 0);
        let second = terminal_query_responses(DEVICE_ATTRIBUTES_QUERY, &mut state, (1, 1), 0);
        assert_eq!(first, DEVICE_ATTRIBUTES_RESPONSE);
        assert!(second.is_empty());
    }
}
