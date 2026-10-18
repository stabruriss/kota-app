use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use chrono::{DateTime, Local, SecondsFormat};
use notify::{RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

use crate::integrations::IntegrationManager;
use crate::message_origin::MessageOrigin;
use crate::pty::{agent::AgentSpawnRequest, PtyManager};
use crate::violet::{self, ActorMessageRecord};

#[path = "local_rpc/actor.rs"]
mod local;

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
    dispatch_processing: Mutex<()>,
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
    pub terminal_timing: Option<AgentBusTerminalTiming>,
    pub origin: MessageOrigin,
    /// Host-only: the Telegram text/caption before media paths, for Soul eligibility.
    pub soul_eligibility: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AgentBusTimingTrigger {
    Scheduled,
    Idle,
    Manual,
}

impl AgentBusTimingTrigger {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::Idle => "idle",
            Self::Manual => "manual",
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentBusTerminalTiming {
    pub trigger: AgentBusTimingTrigger,
    #[serde(default)]
    pub scheduled_for: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ActorDeliveryResult {
    pub submitted: bool,
    pub skipped_reason: Option<String>,
    pub duplicate: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ActorNoticeResult {
    pub recorded: bool,
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
    #[serde(default)]
    pub terminal_timing: Option<AgentBusTerminalTiming>,
    /// Set only by the Telegram bridge in Rust; never read from IPC.
    #[serde(skip)]
    pub soul_eligibility: Option<String>,
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
        let target = resolve_send_target(project_root, &sender_agent_id, &request)?;
        let sender = resolve_agent_identity(project_root, &sender_agent_id).ok();
        let actor_name = request
            .sender_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| {
                sender
                    .as_ref()
                    .map(|identity| identity.display_name.clone())
            })
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
        let intent = request
            .intent
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("message")
            .to_string();
        let origin = MessageOrigin::for_actor(sender.is_some(), &sender_agent_id, &intent);
        let terminal_timing =
            normalize_terminal_timing(&sender_agent_id, &intent, request.terminal_timing)?;
        let delivery = self.send_actor_message(
            app,
            pty,
            ActorMessage {
                project_root: project_root.to_path_buf(),
                actor_id: sender_agent_id,
                actor_name,
                target_agent_id: target.agent_id.clone(),
                intent,
                text,
                event_id: event_id.clone(),
                dedupe_key: Some(dedupe_key),
                launch_request,
                terminal_timing,
                origin,
                soul_eligibility: request.soul_eligibility,
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
            .or_else(|| {
                sender
                    .as_ref()
                    .map(|identity| identity.display_name.clone())
            })
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
        let intent = request
            .intent
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("handoff")
            .to_string();
        let origin = MessageOrigin::for_actor(sender.is_some(), &sender_agent_id, &intent);
        let message = ActorMessage {
            project_root: project_root.to_path_buf(),
            actor_id: sender_agent_id,
            actor_name,
            target_agent_id: target.agent_id.clone(),
            intent,
            text,
            event_id: event_id.clone(),
            dedupe_key: None,
            launch_request,
            terminal_timing: None,
            origin,
            soul_eligibility: None,
        };
        let delivery = if local::applies(pty, &message) {
            local::retry(app, pty, &message, original_event_id)
        } else {
            self.deliver_to_terminal(app, pty, &message)
        };
        Ok(AgentBusRetryDeliveryResult {
            event_id,
            target_agent_id: target.agent_id,
            submitted: delivery.is_ok(),
            skipped_reason: delivery.err().map(|err| crate::violet::ownership::display_error(&err)),
        })
    }

    pub fn send_actor_message(
        &self,
        app: &AppHandle,
        pty: &PtyManager,
        message: ActorMessage,
    ) -> Result<ActorDeliveryResult> {
        let ownership_retry = ownership_retry_path(&message.project_root, &message.event_id);
        if self.is_message_duplicate(&message.project_root, &message.event_id, message.dedupe_key.as_deref())? {
            return Ok(ActorDeliveryResult {
                submitted: false,
                skipped_reason: None,
                duplicate: true,
            });
        }

        if !local::applies(pty, &message) {
            let preflight = pty.agent_check_session(&message.target_agent_id).unwrap_or_else(|| {
                message.launch_request.as_ref().map_or(Ok(()), |request| {
                    crate::violet::ownership::check_launch(request)
                })
            });
            if let Err(error) = preflight {
                if error.is::<crate::violet::ownership::Unavailable>() {
                    let changed = self.persist_ownership_blocked(&message, false)?;
                    violet::emit_room_changed(app, &message.project_root, "actor-message", changed);
                }
                return Err(error);
            }
        }

        let changed_paths = self.record_actor_message_for_delivery(
            &message.project_root,
            &ActorMessageRecord {
                actor_id: message.actor_id.clone(),
                actor_name: message.actor_name.clone(),
                text: message.text.clone(),
                target_agent_ids: vec![message.target_agent_id.clone()],
                event_id: message.event_id.clone(),
                actor_intent: Some(message.intent.clone()),
                origin: message.origin,
            },
        )?;
        violet::emit_room_changed(app, &message.project_root, "actor-message", changed_paths);

        let delivery = self.deliver_to_terminal(app, pty, &message);
        if let Err(err) = delivery.as_ref() {
            if err.is::<crate::violet::ownership::Unavailable>() {
                let changed = self.persist_ownership_blocked(&message, true)?;
                violet::emit_room_changed(app, &message.project_root, "actor-message", changed);
                return Err(delivery.unwrap_err());
            }
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
                    actor_intent: Some("delivery-skipped".into()),
                    origin: MessageOrigin::System,
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
        let _ = fs::remove_file(&ownership_retry);
        Ok(ActorDeliveryResult {
            submitted: true,
            skipped_reason: None,
            duplicate: false,
        })
    }

    pub(crate) fn record_actor_notice(
        &self,
        app: &AppHandle,
        project_root: &Path,
        record: ActorMessageRecord,
        dedupe_key: Option<&str>,
    ) -> Result<ActorNoticeResult> {
        let (result, changed_paths) =
            self.persist_actor_notice(project_root, record, dedupe_key)?;
        if result.recorded {
            violet::emit_room_changed(app, project_root, "actor-message", changed_paths);
        }
        Ok(result)
    }

    #[cfg(test)]
    pub(crate) fn persist_bbs_notice_for_test(
        &self, project_root: &Path, record: ActorMessageRecord, key: &str,
    ) -> Result<ActorNoticeResult> {
        self.persist_actor_notice(project_root, record, Some(key)).map(|(result, _)| result)
    }

    fn persist_actor_notice(
        &self,
        project_root: &Path,
        record: ActorMessageRecord,
        dedupe_key: Option<&str>,
    ) -> Result<(ActorNoticeResult, Vec<PathBuf>)> {
        let guard = self
            .actor_message_log
            .lock()
            .map_err(|_| anyhow!("agent bus actor message log lock poisoned"))?;
        if self.is_duplicate(project_root, dedupe_key)? {
            return Ok((
                ActorNoticeResult {
                    recorded: false,
                    duplicate: true,
                },
                Vec::new(),
            ));
        }

        let changed_paths =
            violet::record_actor_message(project_root, &record).map_err(|err| anyhow!(err))?;
        if let Some(key) = dedupe_key.filter(|key| !key.trim().is_empty()) {
            self.delivered_keys
                .lock()
                .map_err(|_| anyhow!("agent bus dedupe lock poisoned"))?
                .insert(key.to_string());
        }
        drop(guard);
        Ok((
            ActorNoticeResult {
                recorded: true,
                duplicate: false,
            },
            changed_paths,
        ))
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

    fn record_actor_message_for_delivery(&self, root: &Path, record: &ActorMessageRecord) -> Result<Vec<PathBuf>> {
        if read_ownership_retry(&ownership_retry_path(root, &record.event_id))?.is_some_and(|retry| retry.recorded) {
            return Ok(Vec::new()); // A retry must not add a second time-bucketed room event.
        }
        self.record_actor_message(root, record)
    }

    fn persist_ownership_blocked(&self, message: &ActorMessage, recorded: bool) -> Result<Vec<PathBuf>> {
        let marker = ownership_retry_path(&message.project_root, &message.event_id);
        let recorded = recorded || read_ownership_retry(&marker)?.is_some_and(|retry| retry.recorded);
        mark_ownership_retry(&marker, recorded)?;
        let notice_id = format!("{}:ownership-blocked", message.event_id);
        let (_, changed) = self.persist_actor_notice(&message.project_root, ActorMessageRecord {
            actor_id: message.actor_id.clone(), actor_name: message.actor_name.clone(),
            target_agent_ids: vec![message.target_agent_id.clone()], event_id: notice_id.clone(),
            actor_intent: Some("delivery-skipped".into()),
            origin: MessageOrigin::System,
            text: format!("Message is pending. {}", crate::violet::ownership::RECOVERY_MESSAGE),
        }, Some(&notice_id))?;
        Ok(changed)
    }

    pub(crate) fn retry_after_terminal_spawn(app: &AppHandle, project_root: &Path) {
        if !dispatch_ownership_pending_dir(project_root).is_dir() { return; }
        let app = app.clone();
        let root = project_root.to_path_buf();
        thread::spawn(move || {
            let bus = app.state::<AgentBusManager>();
            if let Err(error) = bus.process_dispatch_outbox(&app, &root) {
                crate::kota_debug_log(&format!("[agent-bus] post-start retry: {error}"));
            }
        });
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
        self.process_dispatch_outbox_with(project_root, |path| self.process_dispatch_file(app, project_root, path))
    }

    fn process_dispatch_outbox_with(&self, project_root: &Path, mut process: impl FnMut(&Path) -> Result<AgentBusSendResult>) -> Result<()> {
        // A spawn notification waits for a concurrent rejection to finish
        // moving its payload into pending, avoiding a lost retry wake-up.
        let _guard = self.dispatch_processing.lock().map_err(|_| anyhow!("dispatch processing lock poisoned"))?;
        let outbox = dispatch_outbox_dir(project_root);
        if !outbox.is_dir() {
            return Ok(());
        }
        let pending = dispatch_ownership_pending_dir(project_root);
        fs::create_dir_all(&pending)?;
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
        paths.extend(fs::read_dir(&pending)?.filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json")));
        paths.sort();

        for path in paths {
            let Some(file_name) = path.file_name().map(|name| name.to_owned()) else {
                continue;
            };
            let processing_path = processing.join(&file_name);
            if fs::rename(&path, &processing_path).is_err() {
                continue;
            }
            let result = process(&processing_path);
            finish_dispatch(project_root, &processing_path, &result)?;
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
                terminal_timing: None,
                soul_eligibility: None,
            },
            launch_request,
        )
    }

    fn is_message_duplicate(&self, root: &Path, event: &str, key: Option<&str>) -> Result<bool> {
        if key.is_some_and(|key| self.delivered_keys.lock().is_ok_and(|keys| keys.contains(key))) { return Ok(true); }
        if ownership_retry_path(root, event).is_file() { return Ok(false); }
        self.is_duplicate(root, key)
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
        let now = Local::now();
        let body = app.state::<crate::temporal_context::TemporalContextManager>()
            .prepare_payload_best_effort(
                message.origin, &message.project_root, &message.target_agent_id, &message.text, now,
            );
        let payload = render_terminal_message_with_body_at(message, &body, now);
        if local::applies(pty, message) { return local::deliver(app, pty, message, payload, now, &message.event_id, false); }
        let input = format!("{BRACKETED_PASTE_START}{payload}{BRACKETED_PASTE_END}");
        let soul = soul_input(message, now);
        submit_with_resume(
            || submit_to_agent(app, pty, &message.target_agent_id, &input, &message.intent, soul.clone()),
            |first_err| {
                let Some(launch_request) = message.launch_request.clone() else {
                    return Err(first_err);
                };
                pty.agent_spawn(app, launch_request)?;
                thread::sleep(AGENT_BUS_RESUME_GRACE);
                Ok(())
            },
        )
    }
}

fn submit_with_resume(mut submit: impl FnMut() -> Result<()>, resume: impl FnOnce(anyhow::Error) -> Result<()>) -> Result<()> {
    match submit() {
        Ok(()) => Ok(()),
        Err(error) if error.is::<crate::violet::ownership::Unavailable>() => Err(error),
        Err(error) => { resume(error)?; submit() },
    }
}

// Keep the payload file durable and outside the watched outbox while blocked.
// Existing refreshes retry it; moving it must never create a new timer/queue.
fn finish_dispatch(root: &Path, processing: &Path, result: &Result<AgentBusSendResult>) -> Result<PathBuf> {
    let blocked = result.as_ref().err().is_some_and(|e| e.is::<crate::violet::ownership::Unavailable>());
    let directory = if blocked { dispatch_ownership_pending_dir(root) }
        else if result.is_ok() { dispatch_delivered_dir(root) } else { dispatch_failed_dir(root) };
    fs::create_dir_all(&directory)?;
    let target = unique_path(directory.join(processing.file_name().ok_or_else(|| anyhow!("invalid dispatch path"))?));
    fs::rename(processing, &target)?;
    if let Err(error) = result { fs::write(target.with_extension("error.txt"), error.to_string())?; }
    Ok(target)
}

fn submit_to_agent(app: &AppHandle, pty: &PtyManager, agent_id: &str, input: &str, intent: &str,
    soul: Option<crate::soul_push::input::Input>) -> Result<()> {
    pty.agent_submit_bus_prompt(app, agent_id, input.to_string(), intent, soul)
}

/// Host-only Soul metadata; only the Telegram bridge supplies eligibility.
fn soul_input(message: &ActorMessage, now: DateTime<Local>) -> Option<crate::soul_push::input::Input> {
    Some(crate::soul_push::input::Input {
        eligibility_text: message.soul_eligibility.clone()?, message_id: message.event_id.clone(),
        created_at: now.to_rfc3339(), origin: message.origin, original_text: message.text.clone(),
        target_agent_ids: vec![message.target_agent_id.clone()],
    })
}

fn normalize_terminal_timing(
    actor_id: &str,
    intent: &str,
    timing: Option<AgentBusTerminalTiming>,
) -> Result<Option<AgentBusTerminalTiming>> {
    let Some(timing) = timing else {
        return Ok(None);
    };
    if actor_id != "ember" || intent != "reminder" {
        bail!("terminalTiming is only supported for Ember reminders");
    }
    let scheduled_for = timing
        .scheduled_for
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match timing.trigger {
        AgentBusTimingTrigger::Scheduled => {
            let scheduled_for = scheduled_for
                .ok_or_else(|| anyhow!("scheduled Ember timing requires scheduledFor"))?;
            let parsed = DateTime::parse_from_rfc3339(scheduled_for)
                .map_err(|_| anyhow!("scheduledFor must be an RFC 3339 timestamp"))?;
            Ok(Some(AgentBusTerminalTiming {
                trigger: AgentBusTimingTrigger::Scheduled,
                scheduled_for: Some(
                    parsed
                        .with_timezone(&Local)
                        .to_rfc3339_opts(SecondsFormat::Secs, false),
                ),
            }))
        }
        AgentBusTimingTrigger::Idle | AgentBusTimingTrigger::Manual => {
            if scheduled_for.is_some() {
                bail!("idle/manual Ember timing must not include scheduledFor");
            }
            Ok(Some(AgentBusTerminalTiming {
                trigger: timing.trigger,
                scheduled_for: None,
            }))
        }
    }
}

fn render_terminal_timing_attributes(
    timing: Option<&AgentBusTerminalTiming>,
    now: DateTime<Local>,
) -> String {
    let Some(timing) = timing else {
        return String::new();
    };
    let scheduled_for = timing
        .scheduled_for
        .as_deref()
        .map(|value| format!(" scheduled_for=\"{}\"", escape_attr(value)))
        .unwrap_or_default();
    let current_time = now.to_rfc3339_opts(SecondsFormat::Secs, false);
    format!(
        " trigger=\"{}\"{} current_time=\"{}\"",
        timing.trigger.as_str(),
        scheduled_for,
        escape_attr(&current_time),
    )
}

pub(crate) fn render_terminal_message_with_body_at(
    message: &ActorMessage,
    body: &str,
    now: DateTime<Local>,
) -> String {
    let timing = render_terminal_timing_attributes(message.terminal_timing.as_ref(), now);
    format!(
        "<KOTA_MESSAGE id=\"{}\" from=\"{}\" to=\"{}\" intent=\"{}\"{}>\n{}\n</KOTA_MESSAGE>",
        escape_attr(&message.event_id),
        escape_attr(&message.actor_id),
        escape_attr(&message.target_agent_id),
        escape_attr(&message.intent),
        timing,
        body.trim()
    )
}

#[cfg(test)]
fn render_terminal_message_at(message: &ActorMessage, now: DateTime<Local>) -> String {
    render_terminal_message_with_body_at(message, &message.text, now)
}

#[cfg(test)]
fn render_terminal_message(message: &ActorMessage) -> String {
    render_terminal_message_at(message, Local::now())
}

pub(crate) fn escape_attr(value: &str) -> String {
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

fn dispatch_ownership_pending_dir(root: &Path) -> PathBuf {
    root.join("project-memory/.violet/agent-bus/ownership-pending")
}

fn ownership_retry_path(root: &Path, event_id: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    dispatch_ownership_pending_dir(root).join(format!("{:x}.retry", Sha256::digest(event_id.as_bytes())))
}

#[derive(Serialize, Deserialize)]
struct OwnershipRetry { recorded: bool }

fn read_ownership_retry(path: &Path) -> Result<Option<OwnershipRetry>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn mark_ownership_retry(path: &Path, recorded: bool) -> Result<()> {
    fs::create_dir_all(path.parent().ok_or_else(|| anyhow!("invalid retry path"))?)?;
    crate::adapter_sync::atomic_replace(path, &serde_json::to_vec(&OwnershipRetry { recorded })?).map_err(|e| anyhow!(e))
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

fn resolve_send_target(
    project_root: &Path,
    sender: &str,
    request: &AgentBusSendRequest,
) -> Result<AgentIdentity> {
    let target = resolve_agent_identity(project_root, &request.target)?;
    // Structured BBS destinations are IDs. If the intended agent vanished
    // after bridge validation, the legacy name resolver must not redirect the
    // notice to another agent whose AKA happens to match that old ID.
    if sender == "bbs"
        && request.intent.as_deref() == Some("bbs-thread")
        && request.event_id.as_deref().is_some_and(|id| id.starts_with("bbs-mention:"))
        && target.agent_id != request.target
    {
        bail!("BBS notification target changed");
    }
    Ok(target)
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
    fn only_the_telegram_bridge_supplies_soul_metadata_for_its_own_delivery() {
        let now: DateTime<Local> = DateTime::parse_from_rfc3339("2026-10-16T08:00:00Z").unwrap().into();
        let mut message = ActorMessage { project_root: "/tmp/kota".into(), actor_id: "laughing-man".into(),
            actor_name: "Laughing Man".into(), target_agent_id: "agent-target".into(), intent: "telegram".into(),
            text: "Look at this\n/tmp/kota/project-memory/attachments/composer/att_1_lm/photo.jpg".into(),
            event_id: "lm-update-7".into(), dedupe_key: None, launch_request: None, terminal_timing: None,
            origin: MessageOrigin::Telegram, soul_eligibility: None };
        assert!(soul_input(&message, now).is_none());
        message.soul_eligibility = Some("Look at this".into());
        let input = soul_input(&message, now).unwrap();
        assert_eq!((input.message_id.as_str(), input.eligibility_text.as_str()), ("lm-update-7", "Look at this"));
        assert!(input.valid_for("agent-target") && input.original_text.ends_with("photo.jpg"));
    }

    #[test]
    fn structured_bbs_target_cannot_fall_back_to_another_agents_matching_name() {
        let account = crate::agent_directory::tests::Account::new();
        let root = account.project("p", false);
        account.agent("p", "intended", "display-name: Intended\n");
        account.agent("p", "other", "display-name: intended\n");
        let mut request: AgentBusSendRequest = serde_json::from_value(serde_json::json!({
            "target":"intended", "text":"notice", "intent":"bbs-thread",
            "eventId":format!("bbs-mention:{}", "a".repeat(64))
        })).unwrap();
        assert_eq!(resolve_send_target(&root, "bbs", &request).unwrap().agent_id, "intended");
        account.agent("p", "intended", "display-name: Intended\nstatus: archived\n");
        assert!(resolve_send_target(&root, "bbs", &request).is_err());
        request.event_id = Some("ordinary-bus-message".into());
        assert_eq!(resolve_send_target(&root, "bbs", &request).unwrap().agent_id, "other");
    }

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
            terminal_timing: None,
            origin: MessageOrigin::Bartender,
            soul_eligibility: None,
        };

        let text = render_terminal_message(&message);

        assert!(text.contains("<KOTA_MESSAGE"));
        assert!(text.contains("from=\"bartender\""));
        assert!(text.contains("to=\"alice\""));
        assert!(text.contains("Resolve this conflict."));
        assert!(!text.contains("current_time="));
    }

    #[test]
    fn only_telegram_bus_input_passes_the_shared_user_origin_allowlist() {
        assert!(!MessageOrigin::for_actor(true, "agent-a", "custom-intent").allows_user_context());
        assert!(MessageOrigin::for_actor(false, "laughing-man", "telegram").allows_user_context());
        for (actor, intent) in [("laughing-man", "status"), ("ember", "reminder"),
            ("bbs", "bbs-thread"), ("bartender", "resolve-conflict")] {
            assert!(!MessageOrigin::for_actor(false, actor, intent).allows_user_context());
        }
    }

    #[test]
    fn ember_terminal_timing_is_hidden_in_the_structured_envelope() {
        let timing = normalize_terminal_timing(
            "ember",
            "reminder",
            Some(AgentBusTerminalTiming {
                trigger: AgentBusTimingTrigger::Scheduled,
                scheduled_for: Some("2026-08-18T09:00:00-07:00".into()),
            }),
        )
        .unwrap();
        let message = ActorMessage {
            project_root: PathBuf::from("/tmp/project"),
            actor_id: "ember".into(),
            actor_name: "Ember".into(),
            target_agent_id: "alice".into(),
            intent: "reminder".into(),
            text: "Take a break.".into(),
            event_id: "ember-reminder-1".into(),
            dedupe_key: None,
            launch_request: None,
            terminal_timing: timing,
            origin: MessageOrigin::Ember,
            soul_eligibility: None,
        };
        let now = DateTime::parse_from_rfc3339("2026-08-18T09:00:03-07:00")
            .unwrap()
            .with_timezone(&Local);

        let text = render_terminal_message_at(&message, now);

        assert!(text.contains("intent=\"reminder\" trigger=\"scheduled\""));
        assert!(text.contains("scheduled_for=\""));
        assert!(text.contains("current_time=\""));
        assert!(text.contains(">\nTake a break.\n</KOTA_MESSAGE>"));
        assert!(!text.contains("Ember scheduled prompt"));
    }

    #[test]
    fn ember_idle_and_manual_timing_do_not_claim_a_scheduled_time() {
        for trigger in [AgentBusTimingTrigger::Idle, AgentBusTimingTrigger::Manual] {
            let timing = normalize_terminal_timing(
                "ember",
                "reminder",
                Some(AgentBusTerminalTiming {
                    trigger,
                    scheduled_for: None,
                }),
            )
            .unwrap()
            .unwrap();
            let attrs = render_terminal_timing_attributes(
                Some(&timing),
                DateTime::parse_from_rfc3339("2026-08-18T09:00:03-07:00")
                    .unwrap()
                    .with_timezone(&Local),
            );

            assert!(attrs.contains(&format!("trigger=\"{}\"", timing.trigger.as_str())));
            assert!(attrs.contains("current_time=\""));
            assert!(!attrs.contains("scheduled_for="));
        }
    }

    #[test]
    fn terminal_timing_rejects_non_ember_or_ambiguous_metadata() {
        assert!(normalize_terminal_timing(
            "agent-one",
            "reminder",
            Some(AgentBusTerminalTiming {
                trigger: AgentBusTimingTrigger::Manual,
                scheduled_for: None,
            }),
        )
        .is_err());
        assert!(normalize_terminal_timing(
            "ember",
            "reminder",
            Some(AgentBusTerminalTiming {
                trigger: AgentBusTimingTrigger::Scheduled,
                scheduled_for: None,
            }),
        )
        .is_err());
        assert!(normalize_terminal_timing(
            "ember",
            "reminder",
            Some(AgentBusTerminalTiming {
                trigger: AgentBusTimingTrigger::Idle,
                scheduled_for: Some("2026-08-18T09:00:00-07:00".into()),
            }),
        )
        .is_err());
    }

    #[test]
    fn actor_notice_is_persisted_once_for_the_same_event() {
        let root = std::env::temp_dir().join(format!(
            "kota-actor-notice-test-{}",
            Uuid::new_v4().simple()
        ));
        let manager = AgentBusManager::default();
        let event_id = "ember-reminder-human-one";
        let record = ActorMessageRecord {
            actor_id: "ember".into(),
            actor_name: "Ember".into(),
            text: "Take a break.".into(),
            target_agent_ids: vec!["__kota_human_telegram__".into()],
            event_id: event_id.into(),
            actor_intent: Some("reminder".into()),
            origin: MessageOrigin::Ember,
        };

        let (first, first_paths) = manager
            .persist_actor_notice(&root, record.clone(), Some(event_id))
            .unwrap();
        let (second, second_paths) = manager
            .persist_actor_notice(&root, record, Some(event_id))
            .unwrap();

        assert!(first.recorded);
        assert!(!first.duplicate);
        assert!(!first_paths.is_empty());
        assert!(!second.recorded);
        assert!(second.duplicate);
        assert!(second_paths.is_empty());
        assert!(violet::actor_event_exists(&root, event_id).unwrap());
        let raw = fs::read_to_string(
            root.join("project-memory")
                .join("raw_logs")
                .join("actor-ember.md"),
        )
        .unwrap();
        assert_eq!(
            raw.matches(&format!("- native_event_id: {event_id}"))
                .count(),
            1
        );
        assert!(raw.contains("- actor_intent: reminder"));
        assert!(raw.contains("Assistant:\nTake a break."));
        assert!(!raw.contains("Ember scheduled prompt"));
        fs::remove_dir_all(root).unwrap();
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


#[cfg(test)]
mod ownership_delivery_tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn ownership_denial_never_respawns_or_reports_submit_success() {
        let submits = Cell::new(0);
        let resumes = Cell::new(0);
        let result = submit_with_resume(
            || { submits.set(submits.get() + 1); Err(crate::violet::ownership::Unavailable("fixture".into()).into()) },
            |_| { resumes.set(resumes.get() + 1); Ok(()) },
        );
        assert!(result.unwrap_err().is::<crate::violet::ownership::Unavailable>());
        assert_eq!(submits.get(), 1);
        assert_eq!(resumes.get(), 0);
    }

    #[test]
    fn ordinary_dead_process_keeps_its_single_resume_attempt() {
        let submits = Cell::new(0);
        let resumes = Cell::new(0);
        submit_with_resume(
            || { submits.set(submits.get() + 1); if submits.get() == 1 { Err(anyhow!("dead process")) } else { Ok(()) } },
            |_| { resumes.set(resumes.get() + 1); Ok(()) },
        ).unwrap();
        assert_eq!(submits.get(), 2);
        assert_eq!(resumes.get(), 1);
    }

    #[test]
    fn late_denial_stays_pending_and_fresh_retry_delivers_once() {
        let root = std::env::temp_dir().join(format!("kota-bus-ownership-{}", Uuid::new_v4()));
        let event = "test-ownership-event";
        let record = ActorMessageRecord { actor_id:"agent-sender".into(), actor_name:"Sender".into(),
            text:"synthetic payload".into(), target_agent_ids:vec!["agent-target".into()],
            event_id:event.into(), actor_intent:Some("message".into()), origin: MessageOrigin::A2a };
        let manager = AgentBusManager::default();
        // A race rejects only after the optimistic event has been recorded.
        manager.record_actor_message(&root, &record).unwrap();
        assert!(manager.is_duplicate(&root, Some(event)).unwrap());
        let marker = ownership_retry_path(&root, event);
        mark_ownership_retry(&marker, true).unwrap();
        let processing = dispatch_processing_dir(&root).join("fixture.json");
        fs::create_dir_all(processing.parent().unwrap()).unwrap();
        fs::write(&processing, b"synthetic dispatch").unwrap();
        let denial = Err(crate::violet::ownership::Unavailable("fixture".into()).into());
        let pending = finish_dispatch(&root, &processing, &denial).unwrap();
        assert!(pending.starts_with(dispatch_ownership_pending_dir(&root)));
        assert!(!dispatch_delivered_dir(&root).exists());
        assert_eq!(fs::read(&pending).unwrap(), b"synthetic dispatch");
        // Survives an App restart; room-event existence is not a delivery ACK.
        let manager = AgentBusManager::default();
        assert!(!manager.is_message_duplicate(&root, event, Some(event)).unwrap());
        let writes = Cell::new(0);
        submit_with_resume(|| { writes.set(writes.get() + 1); Ok(()) }, |_| panic!("fresh process is already alive")).unwrap();
        let raw = root.join("project-memory/raw_logs/actor-agent-sender.md");
        let recorded_bytes = fs::read(&raw).unwrap();
        assert!(manager.record_actor_message_for_delivery(&root, &record).unwrap().is_empty());
        assert_eq!(fs::read(&raw).unwrap(), recorded_bytes);
        let room = violet::read_cache(&root, serde_json::from_value(serde_json::json!({})).unwrap()).unwrap();
        assert_eq!(room.messages.iter().filter(|message| message.native_event_id.as_deref() == Some(event)).count(), 1);
        manager.delivered_keys.lock().unwrap().insert(event.into());
        fs::remove_file(marker).unwrap();
        fs::rename(&pending, &processing).unwrap();
        let delivered = finish_dispatch(&root, &processing, &Ok(AgentBusSendResult {
            event_id:event.into(), target_agent_id:"agent-target".into(), submitted:true, skipped_reason:None, duplicate:false,
        })).unwrap();
        assert!(delivered.starts_with(dispatch_delivered_dir(&root)));
        assert!(manager.is_message_duplicate(&root, event, Some(event)).unwrap());
        assert!(AgentBusManager::default().is_message_duplicate(&root, event, Some(event)).unwrap());
        assert_eq!(writes.get(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn early_denial_is_visible_once_and_existing_dispatch_refresh_delivers_after_recovery() {
        let root = std::env::temp_dir().join(format!("kota-bus-early-{}", Uuid::new_v4()));
        let message = ActorMessage {
            project_root: root.clone(), actor_id: "agent-sender".into(), actor_name: "Sender".into(),
            target_agent_id: "agent-target".into(), intent: "message".into(), text: "synthetic payload".into(),
            event_id: "fixture-early".into(), dedupe_key: Some("fixture-early".into()),
            launch_request: None, terminal_timing: None, origin: MessageOrigin::A2a, soul_eligibility: None,
        };
        let manager = AgentBusManager::default();
        fs::create_dir_all(dispatch_outbox_dir(&root)).unwrap();
        fs::write(dispatch_outbox_dir(&root).join("fixture.json"), b"synthetic dispatch").unwrap();
        manager.process_dispatch_outbox_with(&root, |_| {
            manager.persist_ownership_blocked(&message, false)?;
            Err(crate::violet::ownership::Unavailable("private diagnostic".into()).into())
        }).unwrap();
        assert!(manager.persist_ownership_blocked(&message, false).unwrap().is_empty());
        assert!(!violet::actor_event_exists(&root, &message.event_id).unwrap());
        let room = violet::read_cache(&root, serde_json::from_value(serde_json::json!({})).unwrap()).unwrap();
        assert_eq!(room.messages.len(), 1);
        assert!(room.messages[0].text.starts_with("Message is pending."));
        assert!(!room.messages[0].text.contains("private diagnostic"));
        assert!(!dispatch_delivered_dir(&root).join("fixture.json").exists());
        let record = ActorMessageRecord { actor_id: message.actor_id.clone(), actor_name: message.actor_name.clone(),
            text: message.text.clone(), target_agent_ids: vec![message.target_agent_id.clone()],
            event_id: message.event_id.clone(), actor_intent: Some(message.intent.clone()), origin: message.origin };
        let submits = Cell::new(0);
        let deliver = |_: &Path| {
            manager.record_actor_message_for_delivery(&root, &record)?;
            submits.set(submits.get() + 1);
            fs::remove_file(ownership_retry_path(&root, &message.event_id))?;
            Ok(AgentBusSendResult { event_id: message.event_id.clone(), target_agent_id: message.target_agent_id.clone(),
                submitted: true, skipped_reason: None, duplicate: false })
        };
        // The post-spawn wake uses this same processor; no new outbox event is needed.
        manager.process_dispatch_outbox_with(&root, deliver).unwrap();
        manager.process_dispatch_outbox_with(&root, |_| panic!("already delivered")).unwrap();
        assert_eq!(submits.get(), 1);
        assert!(dispatch_delivered_dir(&root).join("fixture.json").exists());
        let room = violet::read_cache(&root, serde_json::from_value(serde_json::json!({})).unwrap()).unwrap();
        assert_eq!(room.messages.iter().filter(|item| item.native_event_id.as_deref() == Some(message.event_id.as_str())).count(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}
