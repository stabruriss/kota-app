use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use notify::{RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

use crate::integrations::IntegrationManager;
use crate::pty::{agent::AgentSpawnRequest, PtyManager};
use crate::violet::{self, ActorMessageRecord};

const BRACKETED_PASTE_START: &str = "\x1b[200~";
const BRACKETED_PASTE_END: &str = "\x1b[201~";
const AGENT_BUS_RESUME_GRACE: Duration = Duration::from_secs(5);
const DISPATCH_SCHEMA: &str = "kota.agent-bus.dispatch.v1";
const KOTA_HOME_DIR: &str = "Kota";

#[derive(Default)]
pub struct AgentBusManager {
    delivered_keys: Mutex<HashSet<String>>,
    dispatch_watchers: Mutex<HashMap<PathBuf, AgentBusDispatchWatch>>,
    actor_message_log: Mutex<()>,
}

struct AgentBusDispatchWatch {
    _watcher: notify::RecommendedWatcher,
    watched_path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ActorMessage {
    pub project_root: PathBuf,
    pub actor_id: String,
    pub actor_name: String,
    pub target_agent_id: String,
    pub intent: String,
    pub text: String,
    pub event_id: String,
    pub dedupe_key: Option<String>,
    pub launch_request: Option<AgentSpawnRequest>,
}

#[derive(Clone, Debug)]
pub struct ActorDeliveryResult {
    pub submitted: bool,
    pub skipped_reason: Option<String>,
    pub duplicate: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBusSendRequest {
    #[serde(default)]
    pub project_root: Option<String>,
    #[serde(default)]
    pub sender_agent_id: Option<String>,
    #[serde(default)]
    pub sender_name: Option<String>,
    pub target: String,
    #[serde(default)]
    pub intent: Option<String>,
    pub text: String,
    #[serde(default)]
    pub event_id: Option<String>,
    #[serde(default)]
    pub dedupe_key: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBusSendResult {
    pub event_id: String,
    pub target_agent_id: String,
    pub submitted: bool,
    pub duplicate: bool,
    pub skipped_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBusRetryDeliveryRequest {
    #[serde(default)]
    pub project_root: Option<String>,
    pub sender_agent_id: String,
    #[serde(default)]
    pub sender_name: Option<String>,
    pub target_agent_id: String,
    #[serde(default)]
    pub intent: Option<String>,
    pub text: String,
    pub original_event_id: String,
    #[serde(default)]
    pub attempt_event_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBusRetryDeliveryResult {
    pub event_id: String,
    pub target_agent_id: String,
    pub submitted: bool,
    pub skipped_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentBusDispatchFile {
    schema: String,
    created_at: String,
    project_root: String,
    sender_agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sender_name: Option<String>,
    target: String,
    intent: String,
    text: String,
    event_id: String,
    dedupe_key: String,
}

#[derive(Clone, Debug)]
struct AgentIdentity {
    agent_id: String,
    display_name: String,
    aka: String,
}

impl AgentBusManager {
    pub fn send_request(
        &self,
        app: &AppHandle,
        pty: &PtyManager,
        project_root: &Path,
        request: AgentBusSendRequest,
        launch_request: Option<AgentSpawnRequest>,
    ) -> Result<AgentBusSendResult> {
        let sender_agent_id = request
            .sender_agent_id
            .as_deref()
            .map(normalize_agent_ref)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("agent bus send requires senderAgentId"))?;
        let target = resolve_agent_identity(project_root, &request.target)?;
        let sender = resolve_agent_identity(project_root, &sender_agent_id).ok();
        let actor_name = request
            .sender_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| sender.map(|identity| identity.display_name))
            .unwrap_or_else(|| sender_agent_id.clone());
        let text = request.text.trim().to_string();
        if text.is_empty() {
            bail!("agent bus send requires non-empty text");
        }
        let event_id = request
            .event_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| mint_event_id(&sender_agent_id, &target.agent_id));
        let dedupe_key = request
            .dedupe_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| event_id.clone());
        let delivery = self.send_actor_message(
            app,
            pty,
            ActorMessage {
                project_root: project_root.to_path_buf(),
                actor_id: sender_agent_id,
                actor_name,
                target_agent_id: target.agent_id.clone(),
                intent: request
                    .intent
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .unwrap_or("message")
                    .to_string(),
                text,
                event_id: event_id.clone(),
                dedupe_key: Some(dedupe_key),
                launch_request,
            },
        )?;
        Ok(AgentBusSendResult {
            event_id,
            target_agent_id: target.agent_id,
            submitted: delivery.submitted,
            duplicate: delivery.duplicate,
            skipped_reason: delivery.skipped_reason,
        })
    }

    pub fn retry_delivery(
        &self,
        app: &AppHandle,
        pty: &PtyManager,
        project_root: &Path,
        request: AgentBusRetryDeliveryRequest,
        launch_request: Option<AgentSpawnRequest>,
    ) -> Result<AgentBusRetryDeliveryResult> {
        let sender_agent_id = normalize_agent_ref(&request.sender_agent_id);
        if sender_agent_id.is_empty() {
            bail!("agent bus retry requires senderAgentId");
        }
        let target = resolve_agent_identity(project_root, &request.target_agent_id)?;
        let sender = resolve_agent_identity(project_root, &sender_agent_id).ok();
        let actor_name = request
            .sender_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| sender.map(|identity| identity.display_name))
            .unwrap_or_else(|| sender_agent_id.clone());
        let text = request.text.trim().to_string();
        if text.is_empty() {
            bail!("agent bus retry requires non-empty text");
        }
        let original_event_id = request.original_event_id.trim();
        if original_event_id.is_empty() {
            bail!("agent bus retry requires originalEventId");
        }
        let event_id = request
            .attempt_event_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("{original_event_id}:retry:{}", Uuid::new_v4().simple()));
        let message = ActorMessage {
            project_root: project_root.to_path_buf(),
            actor_id: sender_agent_id,
            actor_name,
            target_agent_id: target.agent_id.clone(),
            intent: request
                .intent
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("handoff")
                .to_string(),
            text,
            event_id: event_id.clone(),
            dedupe_key: None,
            launch_request,
        };
        let delivery = self.deliver_to_terminal(app, pty, &message);
        Ok(AgentBusRetryDeliveryResult {
            event_id,
            target_agent_id: target.agent_id,
            submitted: delivery.is_ok(),
            skipped_reason: delivery.err().map(|err| err.to_string()),
        })
    }

    pub fn send_actor_message(
        &self,
        app: &AppHandle,
        pty: &PtyManager,
        message: ActorMessage,
    ) -> Result<ActorDeliveryResult> {
        if self.is_duplicate(&message.project_root, message.dedupe_key.as_deref())? {
            return Ok(ActorDeliveryResult {
                submitted: false,
                skipped_reason: None,
                duplicate: true,
            });
        }

        let changed_paths = self.record_actor_message(
            &message.project_root,
            &ActorMessageRecord {
                actor_id: message.actor_id.clone(),
                actor_name: message.actor_name.clone(),
                text: message.text.clone(),
                target_agent_ids: vec![message.target_agent_id.clone()],
                event_id: message.event_id.clone(),
            },
        )?;
        violet::emit_room_changed(app, &message.project_root, "actor-message", changed_paths);

        let delivery = self.deliver_to_terminal(app, pty, &message);
        if let Err(err) = delivery.as_ref() {
            let skipped_text = format!(
                "{} is not responding. Let's skip this for now.",
                message.target_agent_id
            );
            let event_id = format!("{}:skipped", message.event_id);
            let changed_paths = self.record_actor_message(
                &message.project_root,
                &ActorMessageRecord {
                    actor_id: message.actor_id.clone(),
                    actor_name: message.actor_name.clone(),
                    text: skipped_text,
                    target_agent_ids: vec![message.target_agent_id.clone()],
                    event_id,
                },
            )?;
            violet::emit_room_changed(app, &message.project_root, "actor-message", changed_paths);
            return Ok(ActorDeliveryResult {
                submitted: false,
                skipped_reason: Some(err.to_string()),
                duplicate: false,
            });
        }

        if let Some(key) = message.dedupe_key.as_deref() {
            if let Ok(mut keys) = self.delivered_keys.lock() {
                keys.insert(key.to_string());
            }
        }
        Ok(ActorDeliveryResult {
            submitted: true,
            skipped_reason: None,
            duplicate: false,
        })
    }

    fn record_actor_message(
        &self,
        project_root: &Path,
        record: &ActorMessageRecord,
    ) -> Result<Vec<PathBuf>> {
        let _guard = self
            .actor_message_log
            .lock()
            .map_err(|_| anyhow!("agent bus actor message log lock poisoned"))?;
        violet::record_actor_message(project_root, record).map_err(|err| anyhow!(err))
    }

    pub fn resolve_target_agent_id(&self, project_root: &Path, target: &str) -> Result<String> {
        Ok(resolve_agent_identity(project_root, target)?.agent_id)
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
            .map_err(|_| anyhow!("agent bus dispatch watcher lock poisoned"))?;
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
                    let bus = app_for_process.state::<AgentBusManager>();
                    if let Err(err) =
                        bus.process_dispatch_outbox(&app_for_process, &project_root_for_process)
                    {
                        crate::kota_debug_log(&format!(
                            "[agent-bus] process dispatch outbox failed: {err}"
                        ));
                    }
                });
            })?;
        watcher.watch(&outbox, RecursiveMode::NonRecursive)?;
        watchers.insert(
            key,
            AgentBusDispatchWatch {
                _watcher: watcher,
                watched_path: outbox,
            },
        );
        Ok(())
    }

    fn process_dispatch_outbox(&self, app: &AppHandle, project_root: &Path) -> Result<()> {
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
            if let Err(err) = result {
                let _ = fs::write(target_path.with_extension("error.txt"), err.to_string());
            }
        }
        Ok(())
    }

    fn process_dispatch_file(
        &self,
        app: &AppHandle,
        project_root: &Path,
        path: &Path,
    ) -> Result<AgentBusSendResult> {
        let text = fs::read_to_string(path)?;
        let request = serde_json::from_str::<AgentBusDispatchFile>(&text)?;
        if request.schema != DISPATCH_SCHEMA {
            bail!("unsupported agent bus dispatch schema: {}", request.schema);
        }
        let dispatch_project_root = PathBuf::from(&request.project_root);
        if !paths_same(project_root, &dispatch_project_root) {
            bail!(
                "agent bus dispatch project mismatch: {} != {}",
                dispatch_project_root.display(),
                project_root.display()
            );
        }
        let target = resolve_agent_identity(project_root, &request.target)?;
        let manager = app.state::<IntegrationManager>();
        let launch_request = crate::resolve_project_agent_launch(
            &manager,
            Some(&path_string(project_root)),
            &target.agent_id,
        )
        .ok();
        let pty = app.state::<PtyManager>();
        self.send_request(
            app,
            &pty,
            project_root,
            AgentBusSendRequest {
                project_root: Some(path_string(project_root)),
                sender_agent_id: Some(request.sender_agent_id),
                sender_name: request.sender_name,
                target: target.agent_id,
                intent: Some(request.intent),
                text: request.text,
                event_id: Some(request.event_id),
                dedupe_key: Some(request.dedupe_key),
            },
            launch_request,
        )
    }

    fn is_duplicate(&self, project_root: &Path, key: Option<&str>) -> Result<bool> {
        let Some(key) = key.filter(|key| !key.trim().is_empty()) else {
            return Ok(false);
        };
        if self
            .delivered_keys
            .lock()
            .map_err(|_| anyhow!("agent bus dedupe lock poisoned"))?
            .contains(key)
        {
            return Ok(true);
        }
        violet::actor_event_exists(project_root, key).map_err(|err| anyhow!(err))
    }

    fn deliver_to_terminal(
        &self,
        app: &AppHandle,
        pty: &PtyManager,
        message: &ActorMessage,
    ) -> Result<()> {
        let payload = render_terminal_message(message);
        let input = format!("{BRACKETED_PASTE_START}{payload}{BRACKETED_PASTE_END}");
        match submit_to_agent(app, pty, &message.target_agent_id, &input) {
            Ok(()) => return Ok(()),
            Err(first_err) => {
                let Some(launch_request) = message.launch_request.clone() else {
                    return Err(first_err);
                };
                pty.agent_spawn(app, launch_request)?;
                thread::sleep(AGENT_BUS_RESUME_GRACE);
                submit_to_agent(app, pty, &message.target_agent_id, &input)
            }
        }
    }
}

fn submit_to_agent(app: &AppHandle, pty: &PtyManager, agent_id: &str, input: &str) -> Result<()> {
    pty.agent_submit_prompt(app, agent_id.to_string(), input.to_string())
}

fn render_terminal_message(message: &ActorMessage) -> String {
    format!(
        "<KOTA_MESSAGE id=\"{}\" from=\"{}\" to=\"{}\" intent=\"{}\">\n{}\n</KOTA_MESSAGE>",
        escape_attr(&message.event_id),
        escape_attr(&message.actor_id),
        escape_attr(&message.target_agent_id),
        escape_attr(&message.intent),
        message.text.trim()
    )
}

fn escape_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn install_cli_shim() -> Result<PathBuf> {
    let bin_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(KOTA_HOME_DIR)
        .join("bin");
    fs::create_dir_all(&bin_dir)?;
    let shim = bin_dir.join("kota-agent-bus");
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("kota-agent-bus"));
            candidates.push(parent.join("../Resources/kota-agent-bus"));
            if let Some(triple) = current_target_triple_guess() {
                candidates.push(parent.join(format!("kota-agent-bus-{triple}")));
                candidates.push(parent.join(format!("../Resources/kota-agent-bus-{triple}")));
            }
        }
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/kota-agent-bus"));
    candidates
        .push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/release/kota-agent-bus"));

    let mut script = String::from("#!/bin/sh\nset -eu\n");
    script.push_str("if [ -n \"${KOTA_AGENT_BUS_BIN:-}\" ] && [ -x \"$KOTA_AGENT_BUS_BIN\" ]; then exec \"$KOTA_AGENT_BUS_BIN\" \"$@\"; fi\n");
    for candidate in candidates {
        script.push_str("if [ -x ");
        script.push_str(&shell_quote(&candidate.display().to_string()));
        script.push_str(" ]; then exec ");
        script.push_str(&shell_quote(&candidate.display().to_string()));
        script.push_str(" \"$@\"; fi\n");
    }
    script.push_str("echo 'kota-agent-bus binary is not installed. Build it with: cargo build --bin kota-agent-bus' >&2\nexit 127\n");
    fs::write(&shim, script)?;
    make_executable(&shim)?;
    Ok(shim)
}

pub fn run_cli() -> Result<()> {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() {
        print_cli_usage();
        return Ok(());
    }
    let command = args.remove(0);
    match command.as_str() {
        "send" => {
            let mut target = None;
            let mut intent = "message".to_string();
            let mut project_root = std::env::var("KOTA_PROJECT_ROOT").ok();
            let mut sender_agent_id = std::env::var("KOTA_AGENT_ID").ok();
            let mut sender_name = std::env::var("KOTA_AGENT_DISPLAY_NAME").ok();
            let mut i = 0;
            while i < args.len() {
                match args[i].as_str() {
                    "--to" => {
                        i += 1;
                        target = args.get(i).cloned();
                        i += 1;
                    }
                    "--intent" => {
                        i += 1;
                        intent = args
                            .get(i)
                            .ok_or_else(|| anyhow!("usage: kota-agent-bus send --intent <intent>"))?
                            .clone();
                        i += 1;
                    }
                    "--project-root" => {
                        i += 1;
                        project_root = args.get(i).cloned();
                        i += 1;
                    }
                    "--from" => {
                        i += 1;
                        sender_agent_id = args.get(i).cloned();
                        i += 1;
                    }
                    "--from-name" => {
                        i += 1;
                        sender_name = args.get(i).cloned();
                        i += 1;
                    }
                    value => bail!("unknown kota-agent-bus send argument: {value}"),
                }
            }
            let project_root = project_root
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::current_dir()
                        .ok()
                        .and_then(find_project_root_from)
                })
                .ok_or_else(|| {
                    anyhow!("kota-agent-bus send requires KOTA_PROJECT_ROOT or --project-root")
                })?;
            let sender_agent_id = sender_agent_id
                .as_deref()
                .map(normalize_agent_ref)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| anyhow!("kota-agent-bus send requires KOTA_AGENT_ID or --from"))?;
            let sender_name = sender_name
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .or_else(|| {
                    agent_yaml_path(&project_root, &sender_agent_id)
                        .and_then(|path| read_agent_display_name(&path))
                });
            let target_ref = target
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| anyhow!("usage: kota-agent-bus send --to <AKA-or-agent-id>"))?;
            let target = resolve_cli_target_agent_id(&project_root, target_ref)?;
            let body = read_stdin_body()?;
            let event_id = mint_event_id(&sender_agent_id, &target);
            let request = AgentBusDispatchFile {
                schema: DISPATCH_SCHEMA.into(),
                created_at: now_iso(),
                project_root: path_string(&project_root),
                sender_agent_id,
                sender_name,
                target,
                intent,
                text: body,
                event_id: event_id.clone(),
                dedupe_key: event_id.clone(),
            };
            enqueue_dispatch(&project_root, &request)?;
            println!("{event_id}");
        }
        "install-shim" => {
            let path = install_cli_shim()?;
            println!("{}", path.display());
        }
        _ => {
            print_cli_usage();
            bail!("unknown kota-agent-bus command: {command}");
        }
    }
    Ok(())
}

fn print_cli_usage() {
    eprintln!("usage:");
    eprintln!("  kota-agent-bus send --to <AKA-or-agent-id> [--intent <intent>] <<'EOF'");
    eprintln!("  kota-agent-bus install-shim");
}

fn enqueue_dispatch(project_root: &Path, request: &AgentBusDispatchFile) -> Result<PathBuf> {
    let outbox = dispatch_outbox_dir(project_root);
    fs::create_dir_all(&outbox)?;
    let path = outbox.join(format!("{}.json", sanitize_id(&request.event_id)));
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(request)?)?;
    fs::rename(&tmp, &path)?;
    Ok(path)
}

fn read_stdin_body() -> Result<String> {
    let mut body = String::new();
    io::stdin().read_to_string(&mut body)?;
    let body = body.trim().to_string();
    if body.is_empty() {
        bail!("kota-agent-bus requires a non-empty message body on stdin");
    }
    Ok(body)
}

fn resolve_cli_target_agent_id(project_root: &Path, target: &str) -> Result<String> {
    resolve_agent_identity(project_root, target)
        .map(|identity| identity.agent_id)
        .map_err(|err| {
            anyhow!(
                "{err}. Use the canonical agent id from the Teammates list, for example `agent-...`"
            )
        })
}

fn dispatch_outbox_dir(project_root: &Path) -> PathBuf {
    project_root
        .join("project-memory")
        .join(".violet")
        .join("agent-bus")
        .join("outbox")
}

fn dispatch_processing_dir(project_root: &Path) -> PathBuf {
    project_root
        .join("project-memory")
        .join(".violet")
        .join("agent-bus")
        .join("processing")
}

fn dispatch_delivered_dir(project_root: &Path) -> PathBuf {
    project_root
        .join("project-memory")
        .join(".violet")
        .join("agent-bus")
        .join("delivered")
}

fn dispatch_failed_dir(project_root: &Path) -> PathBuf {
    project_root
        .join("project-memory")
        .join(".violet")
        .join("agent-bus")
        .join("failed")
}

fn resolve_agent_identity(project_root: &Path, raw: &str) -> Result<AgentIdentity> {
    let wanted = normalize_agent_ref(raw).to_lowercase();
    if wanted.is_empty() {
        bail!("empty agent reference");
    }
    let mut exact = Vec::new();
    let mut aka = Vec::new();
    for identity in list_agent_identities(project_root)? {
        if identity.agent_id.to_lowercase() == wanted {
            exact.push(identity.clone());
        } else if identity.aka.to_lowercase() == wanted
            || identity.display_name.to_lowercase() == wanted
        {
            aka.push(identity.clone());
        }
    }
    let mut matches = if exact.is_empty() { aka } else { exact };
    if matches.is_empty() {
        bail!("agent not found for reference: {raw}");
    }
    matches.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
    matches.dedup_by(|a, b| a.agent_id == b.agent_id);
    if matches.len() > 1 {
        bail!("agent reference is ambiguous: {raw}");
    }
    Ok(matches.remove(0))
}

fn list_agent_identities(project_root: &Path) -> Result<Vec<AgentIdentity>> {
    let agents_root = project_root.join(".agent-workspaces");
    if !agents_root.is_dir() {
        return Ok(Vec::new());
    }
    let mut identities = Vec::new();
    for entry in fs::read_dir(&agents_root)? {
        let entry = entry?;
        let cwd = entry.path();
        if !cwd.is_dir() {
            continue;
        }
        let Some(agent_id) = cwd
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        let yaml_path = cwd.join("agent.yaml");
        let yaml = read_yaml_value(&yaml_path).unwrap_or(serde_yaml::Value::Null);
        let status = yaml_string(&yaml, "status").unwrap_or_else(|| "active".into());
        if matches!(
            status.trim().to_ascii_lowercase().as_str(),
            "archived" | "deleted" | "dismissed" | "removed"
        ) {
            continue;
        }
        let display_name = yaml_string(&yaml, "display-name")
            .or_else(|| yaml_string(&yaml, "displayName"))
            .unwrap_or_else(|| agent_id.clone());
        identities.push(AgentIdentity {
            aka: agent_aka_from_display_name(&display_name),
            agent_id,
            display_name,
        });
    }
    Ok(identities)
}

fn normalize_agent_ref(raw: &str) -> String {
    raw.trim().trim_start_matches('@').trim().to_string()
}

fn agent_aka_from_display_name(display_name: &str) -> String {
    let trimmed = display_name.trim();
    if trimmed.is_empty() {
        return "Agent".into();
    }
    let without_project = trimmed
        .split_once(" v. ")
        .map(|(head, _)| head)
        .unwrap_or(trimmed)
        .trim();
    let without_bunshin = without_project
        .trim_end_matches("-Bunshin")
        .trim_end_matches("-Bunshin II")
        .trim();
    let words = without_bunshin.split_whitespace().collect::<Vec<_>>();
    if words.len() >= 2 && matches!(words[0], "Jr." | "Sr." | "Dr." | "Lady" | "Lord") {
        return format!("{} {}", words[0], words[1]);
    }
    if words.len() > 1 && matches!(words[0], "CC" | "Dex" | "Gem" | "Op") {
        return words[0].to_string();
    }
    without_bunshin.to_string()
}

fn agent_yaml_path(project_root: &Path, agent_id: &str) -> Option<PathBuf> {
    let path = project_root
        .join(".agent-workspaces")
        .join(agent_id)
        .join("agent.yaml");
    path.is_file().then_some(path)
}

fn read_agent_display_name(path: &Path) -> Option<String> {
    let yaml = read_yaml_value(path).ok()?;
    yaml_string(&yaml, "display-name")
        .or_else(|| yaml_string(&yaml, "displayName"))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn read_yaml_value(path: &Path) -> Result<serde_yaml::Value> {
    let text = fs::read_to_string(path)?;
    Ok(serde_yaml::from_str(&text)?)
}

fn yaml_string(value: &serde_yaml::Value, key: &str) -> Option<String> {
    let serde_yaml::Value::Mapping(map) = value else {
        return None;
    };
    map.get(&serde_yaml::Value::String(key.to_string()))
        .and_then(|value| match value {
            serde_yaml::Value::String(value) => Some(value.clone()),
            serde_yaml::Value::Number(value) => Some(value.to_string()),
            serde_yaml::Value::Bool(value) => Some(value.to_string()),
            _ => None,
        })
}

fn find_project_root_from(mut path: PathBuf) -> Option<PathBuf> {
    loop {
        if path.join(".agent-workspaces").is_dir() && path.join("project-memory").is_dir() {
            return Some(path);
        }
        if !path.pop() {
            return None;
        }
    }
}

fn unique_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("dispatch");
    let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
    for index in 1.. {
        let name = if ext.is_empty() {
            format!("{stem}-{index}")
        } else {
            format!("{stem}-{index}.{ext}")
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    path
}

fn paths_same(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    left == right
}

fn mint_event_id(sender_agent_id: &str, target: &str) -> String {
    format!(
        "agentbus-{}-{}-{}",
        sanitize_id(sender_agent_id),
        sanitize_id(target),
        &Uuid::new_v4().simple().to_string()[..12]
    )
}

fn sanitize_id(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if ch == '-' || ch == '_' {
            out.push(ch);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "message".into()
    } else {
        out
    }
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn make_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(perms.mode() | 0o755);
        fs::set_permissions(path, perms)?;
    }
    Ok(())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn current_target_triple_guess() -> Option<&'static str> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        return Some("aarch64-apple-darwin");
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        return Some("x86_64-apple-darwin");
    }
    #[allow(unreachable_code)]
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_message_uses_structured_envelope() {
        let message = ActorMessage {
            project_root: PathBuf::from("/tmp/project"),
            actor_id: "bartender".into(),
            actor_name: "Bartender".into(),
            target_agent_id: "alice".into(),
            intent: "resolve-conflict".into(),
            text: "Resolve this conflict.".into(),
            event_id: "msg_1".into(),
            dedupe_key: None,
            launch_request: None,
        };

        let text = render_terminal_message(&message);

        assert!(text.contains("<KOTA_MESSAGE"));
        assert!(text.contains("from=\"bartender\""));
        assert!(text.contains("to=\"alice\""));
        assert!(text.contains("Resolve this conflict."));
    }

    #[test]
    fn resolves_agent_reference_by_aka_or_at_id() {
        let root =
            std::env::temp_dir().join(format!("kota-agent-bus-test-{}", Uuid::new_v4().simple()));
        let agents = root.join(".agent-workspaces");
        fs::create_dir_all(agents.join("agent-alice")).unwrap();
        fs::write(
            agents.join("agent-alice").join("agent.yaml"),
            "id: agent-alice\ndisplay-name: Dex III v. kota\nstatus: active\n",
        )
        .unwrap();

        let by_aka = resolve_agent_identity(&root, "Dex").unwrap();
        let by_id = resolve_agent_identity(&root, "@agent-alice").unwrap();

        assert_eq!(by_aka.agent_id, "agent-alice");
        assert_eq!(by_id.agent_id, "agent-alice");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cli_target_resolution_returns_canonical_agent_id() {
        let root =
            std::env::temp_dir().join(format!("kota-agent-bus-test-{}", Uuid::new_v4().simple()));
        let agents = root.join(".agent-workspaces");
        fs::create_dir_all(agents.join("agent-alice")).unwrap();
        fs::write(
            agents.join("agent-alice").join("agent.yaml"),
            "id: agent-alice\ndisplay-name: Dex III v. kota\nstatus: active\n",
        )
        .unwrap();

        let target = resolve_cli_target_agent_id(&root, "Dex").unwrap();

        assert_eq!(target, "agent-alice");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cli_target_resolution_rejects_unknown_target() {
        let root =
            std::env::temp_dir().join(format!("kota-agent-bus-test-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(root.join(".agent-workspaces")).unwrap();

        let err = resolve_cli_target_agent_id(&root, "座山雕CC48")
            .unwrap_err()
            .to_string();

        assert!(err.contains("agent not found for reference: 座山雕CC48"));
        assert!(err.contains("canonical agent id"));
        fs::remove_dir_all(root).unwrap();
    }
}
