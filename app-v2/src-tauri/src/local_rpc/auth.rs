//! Independent, bounded RPC helpers. No agent model turn, terminal, history
//! sink, prompt injection, or credential value is shared with this channel.
use super::{
    auth_protocol::{self, AuthReply, AuthStart, AuthStarted, Dialog},
    bundle::BundledPi,
    credentials::AuthStore,
    error::{LocalError, LocalResult},
    owned::{FileLock, OwnedDirectory},
    process::{PipeEvent, Process},
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::process::Command;
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(600);
const CANCEL_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_OPERATIONS: usize = 64;
type Reply = mpsc::SyncSender<LocalResult<()>>;
enum Input {
    Respond(AuthReply, Reply),
    Cancel(Reply),
}
#[derive(Clone)]
struct Handle {
    input: mpsc::SyncSender<Input>,
}
fn operations() -> &'static Mutex<BTreeMap<String, Handle>> {
    static MAP: OnceLock<Mutex<BTreeMap<String, Handle>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(BTreeMap::new()))
}

#[derive(Clone)]
pub(crate) struct Backend {
    pub bundled: BundledPi,
    pub owned: OwnedDirectory,
}
impl Backend {
    pub fn for_app(app: &tauri::AppHandle) -> LocalResult<Self> {
        Ok(Self {
            bundled: BundledPi::for_app(app)?,
            owned: OwnedDirectory::new(&crate::kota_home_dir()),
        })
    }
    pub fn query(&self, request: Value) -> LocalResult<Value> {
        self.query_observed(request, &super::query::Query::standard())
    }
    pub fn query_observed(&self, mut request: Value, query: &super::query::Query) -> LocalResult<Value> {
        let query = query.renewed();
        let stage = match request["operation"].as_str() {
            Some("catalog") => "model_catalog",
            Some("resolve") => "resolve_model",
            _ => "credential_status",
        };
        query.stage("helper_start")?;
        let started = Instant::now();
        let mut helper = Helper::spawn_observed(self, &query, true)?;
        let readiness = query.readiness();
        readiness.stage("helper_ready")?;
        helper.precheck_cancellable(readiness.deadline, || {
            if credential_store_locked(&self.owned) { let _ = readiness.stage("credential_store"); }
            readiness.check("helper_ready").is_err()
        }).map_err(|e| {
            let error = readiness.check("helper_ready").err().unwrap_or(e);
            if error.code == "auth_timeout" && credential_store_locked(&self.owned) {
                LocalError::new("auth_store_busy").at("credential_store")
            } else { error.at("helper_ready") }
        })?;
        crate::kota_debug_log(&format!("local-query {stage}: helper_ready {}ms", started.elapsed().as_millis()));
        let query = query.renewed();
        query.stage(stage)?;
        let id = uuid::Uuid::new_v4().to_string();
        request["schema"] = json!(1);
        request["operationId"] = json!(&id);
        let provider = request["providerId"].as_str().map(str::to_owned);
        let rpc = helper.auth_command(&request)?;
        loop {
            query.check(stage)?;
            helper.check(query.deadline).map_err(|e| e.at(stage))?;
            if let Some(event) = helper.record()? {
                if event["type"] == "response" && event["id"] == rpc && event["success"] != true {
                    return Err(LocalError::new("auth_helper_failed"));
                }
                if event["type"] == "extension_error"
                    && event["extensionPath"] == "command:kota-auth"
                {
                    return Err(LocalError::new("auth_helper_failed"));
                }
                if event["type"] == "extension_ui_request" {
                    let clean = auth_protocol::decode(&event, &id, provider.as_deref(), None)?;
                    if clean["event"] == "prompt" {
                        return Err(LocalError::new("auth_protocol_invalid"));
                    }
                    if clean["event"] == "result" {
                        if clean["operation"] != request["operation"] {
                            return Err(LocalError::new("auth_protocol_invalid"));
                        }
                        crate::kota_debug_log(&format!("local-query {stage}: completed {}ms", started.elapsed().as_millis()));
                        return result_success(clean).map_err(|e| {
                            if e.code == "auth_store_read_failed" && credential_store_locked(&self.owned) {
                                LocalError::new("auth_store_busy").at(stage)
                            } else { e.at(stage) }
                        });
                    }
                }
            } else {
                thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

pub(super) struct Helper {
    process: Process,
    cwd: std::path::PathBuf,
    sequence: u64,
}
impl Helper {
    fn spawn(backend: &Backend) -> LocalResult<Self> {
        Self::spawn_observed(backend, &super::query::Query::new(QUERY_TIMEOUT), false)
    }
    fn spawn_observed(backend: &Backend, query: &super::query::Query, read_only: bool) -> LocalResult<Self> {
        query.stage("configuration_lock")?;
        let installation = backend.owned.initialize_observed(query)?;
        query.stage("helper_start")?;
        AuthStore::load(&backend.owned)?.preflight()?;
        let cwd = backend
            .owned
            .agent
            .join(".kota-auth")
            .join(format!("helper-{}", uuid::Uuid::new_v4()));
        super::ledger::private_dir(&cwd).map_err(|_| LocalError::new("auth_helper_failed"))?;
        let mut command = Command::new(&backend.bundled.executable);
        command
            .current_dir(&cwd)
            .args([
                "--mode",
                "rpc",
                "--no-session",
                "--no-approve",
                "-ne",
                "-ns",
                "-np",
                "--no-themes",
                "--no-context-files",
                "--no-tools",
                "-e",
            ])
            .arg(&backend.bundled.extension);
        // RPC startup otherwise starts a network catalog refresh, including
        // OAuth renewal under auth.json.lock. A metadata query must not do it.
        // Login helpers and ordinary owners keep native Pi networking.
        if read_only { command.arg("--offline"); }
        super::environment::helper_environment(&mut command, &backend.owned.agent, &installation);
        let process = match Process::spawn(command) {
            Ok(process) => process,
            Err(_) => {
                let _ = fs::remove_dir_all(cwd);
                return Err(LocalError::new("auth_helper_failed"));
            }
        };
        Ok(Self {
            process,
            cwd,
            sequence: 0,
        })
    }
    fn send(&mut self, mut request: Value) -> LocalResult<String> {
        self.sequence += 1;
        let id = format!("auth-rpc-{}", self.sequence);
        request["id"] = json!(&id);
        self.process
            .send(request)
            .map_err(|_| LocalError::new("auth_helper_failed"))?;
        Ok(id)
    }
    fn auth_command(&mut self, request: &Value) -> LocalResult<String> {
        self.send(json!({"type":"prompt","message":format!("/kota-auth {}", serde_json::to_string(request).map_err(|_| LocalError::new("invalid_request"))?)}))
    }
    fn precheck_cancellable(
        &mut self,
        deadline: Instant,
        mut cancelled: impl FnMut() -> bool,
    ) -> LocalResult<()> {
        let id = self.send(json!({"type":"get_commands"}))?;
        loop {
            if cancelled() {
                return Err(LocalError::new("auth_cancelled"));
            }
            self.check(deadline)?;
            if let Some(record) = self.record()? {
                if record["type"] == "response" && record["id"] == id {
                    let commands = record["data"]["commands"].as_array();
                    if record["success"] != true
                        || !commands.is_some_and(|list| {
                            list.iter()
                                .any(|c| c["name"] == "kota-auth" && c["source"] == "extension")
                        })
                    {
                        return Err(LocalError::new("local_auth_extension_unavailable"));
                    }
                    return Ok(());
                }
                if record["type"] == "extension_ui_request" {
                    return Err(LocalError::new("auth_protocol_invalid"));
                }
            } else {
                thread::sleep(Duration::from_millis(5));
            }
        }
    }
    fn record(&mut self) -> LocalResult<Option<Value>> {
        while let Some(event) = self.process.poll() {
            match event {
                PipeEvent::Record(value, _credit) => return Ok(Some(value)),
                PipeEvent::Written(_) => {}
                PipeEvent::Eof => return Err(LocalError::new("auth_helper_exited")),
            }
        }
        Ok(None)
    }
    fn check(&mut self, deadline: Instant) -> LocalResult<()> {
        if Instant::now() >= deadline {
            return Err(LocalError::new("auth_timeout"));
        }
        if self.process.fault().is_some()
            || self
                .process
                .try_wait()
                .map_err(|_| LocalError::new("auth_helper_exited"))?
                .is_some()
        {
            return Err(LocalError::new("auth_helper_exited"));
        }
        Ok(())
    }
}

// Existence is only a progress/error diagnostic. Pi owns acquisition,
// staleness, backoff and reclamation; Kota never removes this directory.
fn credential_store_locked(owned: &OwnedDirectory) -> bool { owned.agent.join("auth.json.lock").is_dir() }
impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.process.stop();
        let _ = fs::remove_dir_all(&self.cwd);
    }
}

fn result_success(event: Value) -> LocalResult<Value> {
    if event["outcome"] == "succeeded" {
        return Ok(event);
    }
    let code = match event["code"].as_str() {
        Some("model-not-found") => "model_not_found",
        Some("thinking-invalid") => "thinking_invalid",
        Some("unsupported-provider" | "unsupported-auth") => "auth_handler_unavailable",
        Some("storage-failed") => "auth_store_read_failed",
        _ => "auth_helper_failed",
    };
    Err(LocalError {
        code: code.into(),
        provider_id: event["providerId"].as_str().map(str::to_owned),
        stage: None,
    })
}

pub(crate) fn start(
    backend: Backend,
    request: AuthStart,
    notify: Arc<dyn Fn(Value) + Send + Sync>,
    changed: Arc<dyn Fn() + Send + Sync>,
) -> LocalResult<AuthStarted> {
    request.validate()?;
    backend.owned.initialize()?;
    AuthStore::load(&backend.owned)?.preflight()?;
    let lock = backend.owned.provider_lock(&request.provider_id)?;
    let id = uuid::Uuid::new_v4().to_string();
    let (input, receiver) = mpsc::sync_channel(16);
    {
        let mut active = operations().lock().unwrap();
        if active.len() >= MAX_OPERATIONS {
            return Err(LocalError::new("busy-auth"));
        }
        active.insert(id.clone(), Handle { input });
    }
    let operation = id.clone();
    let cleanup = id.clone();
    if thread::Builder::new().name("local-auth-owner".into()).spawn(move || {
        let mut submitted = false;
        let result = run(backend, &operation, &request, receiver, &notify, lock, &mut submitted);
        match result {
            Ok(mut event) => {
                for key in ["status", "statuses", "catalog", "resolved"] { event.as_object_mut().unwrap().remove(key); }
                notify(event);
            },
            Err(error) => notify(json!({"schema":1,"channel":"kota.auth","operationId":operation,"event":"result",
                "operation":request.operation,"providerId":request.provider_id,
                "outcome":if error.code == "auth_cancelled" {"cancelled"} else if error.code == "auth_timeout" {"timeout"} else if submitted {"indeterminate"} else {"failed"},
                "commit":if submitted {"unknown"} else {"not-written"},"code":"runtime-exited"})),
        }
        // Written, failed synchronization, and unknown all need a new status
        // check. Never undo or retry a credential mutation automatically.
        changed();
        operations().lock().unwrap().remove(&operation);
    }).is_err() {
        operations().lock().unwrap().remove(&cleanup);
        return Err(LocalError::new("auth_helper_failed"));
    }
    Ok(AuthStarted { operation_id: id })
}

fn run(
    backend: Backend,
    id: &str,
    request: &AuthStart,
    receiver: mpsc::Receiver<Input>,
    notify: &Arc<dyn Fn(Value) + Send + Sync>,
    _lock: FileLock,
    submitted: &mut bool,
) -> LocalResult<Value> {
    let mut deadline = Instant::now()
        + if request.operation == "login" {
            LOGIN_TIMEOUT
        } else {
            QUERY_TIMEOUT
        };
    let mut helper = Helper::spawn(&backend)?;
    helper.precheck_cancellable(deadline.min(Instant::now() + QUERY_TIMEOUT), || {
        while let Ok(input) = receiver.try_recv() {
            match input {
                Input::Cancel(ack) => {
                    let _ = ack.send(Ok(()));
                    return true;
                }
                Input::Respond(_, ack) => {
                    let _ = ack.send(Err(LocalError::new("auth_prompt_closed")));
                }
            }
        }
        false
    })?;
    let mut root = json!({"schema":1,"operationId":id,"operation":request.operation,"providerId":request.provider_id});
    if let Some(kind) = request.auth_type {
        root["authType"] = json!(kind);
        root["uiSchema"] = json!(1);
    }
    if let Some(url) = &request.resource_url {
        root["resourceUrl"] = json!(url);
    }
    let rpc = helper.auth_command(&root)?;
    *submitted = true;
    let mut dialogs: BTreeMap<String, Dialog> = BTreeMap::new();
    let mut cancelling: Option<(String, Instant)> = None;
    let mut terminal = None;
    let mut failure = None;
    let loop_result = (|| -> LocalResult<()> {
        loop {
            if Instant::now() >= deadline && cancelling.is_none() {
                let cancel_id = uuid::Uuid::new_v4().to_string();
                helper.auth_command(&json!({"schema":1,"operationId":cancel_id,"operation":"cancel","targetOperationId":id}))?;
                cancelling = Some((cancel_id, Instant::now() + CANCEL_TIMEOUT));
            }
            if let Err(error) = helper.check(cancelling.as_ref().map(|c| c.1).unwrap_or(deadline)) {
                failure = Some(error);
                break;
            }
            let mut count = 0;
            while let Some(record) = helper.record()? {
                count += 1;
                if record["type"] == "response" && record["id"] == rpc && record["success"] != true
                    || record["type"] == "extension_error"
                        && record["extensionPath"] == "command:kota-auth"
                {
                    failure = Some(LocalError::new("auth_helper_failed"));
                    break;
                }
                if record["type"] == "extension_ui_request" {
                    // Cancel has its own command ID. Its result is bookkeeping;
                    // only the original operation's result is a terminal fact.
                    if cancelling.as_ref().is_some_and(|c| {
                        record["message"].as_str().is_some_and(|s| {
                            s.strip_prefix(auth_protocol::CODEC)
                                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                                .is_some_and(|v| v["operationId"] == c.0)
                        })
                    }) {
                        continue;
                    }
                    let event = auth_protocol::decode(
                        &record,
                        id,
                        Some(&request.provider_id),
                        request.auth_type,
                    )?;
                    match event["event"].as_str().unwrap() {
                        "prompt" => {
                            let native = record["id"]
                                .as_str()
                                .filter(|s| super::error::identifier(s, 128))
                                .ok_or_else(|| LocalError::new("auth_protocol_invalid"))?;
                            let prompt = event["promptId"].as_str().unwrap().to_owned();
                            if dialogs.len() >= 64 || dialogs.contains_key(&prompt) {
                                return Err(LocalError::new("auth_protocol_invalid"));
                            }
                            dialogs.insert(
                                prompt,
                                Dialog {
                                    native_id: native.into(),
                                    event: event.clone(),
                                },
                            );
                        }
                        "prompt_closed" => {
                            dialogs.remove(event["promptId"].as_str().unwrap());
                        }
                        "auth_event" => {
                            if let Some(seconds) =
                                event["notification"]["expiresInSeconds"].as_f64()
                            {
                                deadline = deadline.min(
                                    Instant::now() + Duration::from_secs_f64(seconds.min(600.0)),
                                );
                            }
                        }
                        "result" => {
                            if event["operation"] != request.operation {
                                return Err(LocalError::new("auth_protocol_invalid"));
                            }
                            terminal = Some(event);
                            break;
                        }
                        _ => {}
                    }
                    notify(event);
                }
                if count >= 64 {
                    break;
                }
            }
            if terminal.is_some() || failure.is_some() {
                break;
            }
            match receiver.recv_timeout(Duration::from_millis(5)) {
                Ok(Input::Respond(reply, ack)) => {
                    let result = (|| {
                        if cancelling.is_some() {
                            return Err(LocalError::new("auth_prompt_closed"));
                        }
                        let dialog = dialogs
                            .get(&reply.prompt_id)
                            .ok_or_else(|| LocalError::new("auth_prompt_closed"))?;
                        dialog.validate_reply(&reply)?;
                        let mut message =
                            json!({"type":"extension_ui_response","id":dialog.native_id});
                        if reply.cancelled == Some(true) {
                            message["cancelled"] = json!(true);
                        } else {
                            message["value"] = json!(reply.value);
                        }
                        helper
                            .process
                            .send(message)
                            .map_err(|_| LocalError::new("auth_helper_failed"))?;
                        dialogs.remove(&reply.prompt_id);
                        Ok(())
                    })();
                    let _ = ack.send(result);
                }
                Ok(Input::Cancel(ack)) => {
                    let result = if cancelling.is_some() {
                        Ok(())
                    } else {
                        let cancel_id = uuid::Uuid::new_v4().to_string();
                        helper.auth_command(&json!({"schema":1,"operationId":cancel_id,"operation":"cancel","targetOperationId":id})).map(|_| {
                        cancelling = Some((cancel_id, Instant::now() + CANCEL_TIMEOUT));
                    })
                    };
                    let _ = ack.send(result);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    failure = Some(LocalError::new("auth_helper_exited"));
                    break;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        Ok(())
    })();
    for (prompt, _) in dialogs {
        notify(
            json!({"schema":1,"channel":"kota.auth","operationId":id,"event":"prompt_closed",
        "providerId":request.provider_id,"promptId":prompt,"reason":"interrupted"}),
        );
    }
    loop_result?;
    terminal.ok_or_else(|| failure.unwrap_or_else(|| LocalError::new("auth_helper_failed")))
}

/// Save-time Jev check on an offline, read-only helper. The key travels once, as the reply to
/// the extension's private prompt; the outcome is `verified`, `rejected` or `unavailable`.
pub(super) fn jev_verify(backend: &Backend, provider: &str, key: &str, cancelled: &dyn Fn() -> bool)
    -> LocalResult<&'static str> {
    const CODEC: &str = "kota.jev.v1:";
    let query = super::query::Query::new(QUERY_TIMEOUT);
    let mut helper = Helper::spawn_observed(backend, &query, true)?;
    helper.precheck_cancellable(query.deadline, cancelled)?;
    let id = uuid::Uuid::new_v4().to_string();
    let command = json!({"schema":1,"operationId":id,"providerId":provider});
    let rpc = helper.send(json!({"type":"prompt","message":format!("/kota-jev-verify {command}")}))?;
    let decode = |text: Option<&str>| text.and_then(|s| s.strip_prefix(CODEC)).and_then(|s| serde_json::from_str::<Value>(s).ok())
        .filter(|v| v["schema"] == 1 && v["operationId"] == id.as_str());
    let mut answered = false;
    loop {
        if cancelled() { return Err(LocalError::new("jev_verification_cancelled")); }
        helper.check(query.deadline)?;
        let Some(record) = helper.record()? else { thread::sleep(Duration::from_millis(5)); continue; };
        if record["type"] == "response" && record["id"] == rpc && record["success"] != true
            || record["type"] == "extension_error" && record["extensionPath"] == "command:kota-jev-verify" {
            return Err(LocalError::new("auth_helper_failed"));
        }
        if record["type"] != "extension_ui_request" { continue; }
        match (record["method"].as_str(), decode(record["title"].as_str()), decode(record["message"].as_str())) {
            (Some("input"), Some(prompt), _) if prompt["event"] == "key" && !answered => {
                let native = record["id"].as_str().filter(|s| super::error::identifier(s, 128))
                    .ok_or_else(|| LocalError::new("auth_protocol_invalid"))?;
                helper.process.send(json!({"type":"extension_ui_response","id":native,"value":key}))
                    .map_err(|_| LocalError::new("auth_helper_failed"))?;
                answered = true;
            }
            (Some("notify"), _, Some(result)) if result["event"] == "result" && answered => {
                return match result["outcome"].as_str() {
                    Some("verified") => Ok("verified"),
                    Some("rejected") => Ok("rejected"),
                    Some("unavailable") => Ok("unavailable"),
                    _ => Err(LocalError::new("auth_protocol_invalid")),
                };
            }
            _ => return Err(LocalError::new("auth_protocol_invalid")),
        }
    }
}

/// Magpie already has its key (the local marker or Settings input). Reuse the
/// private auth protocol and Pi login writer without a user sign-in dialog.
/// The caller holds Magpie's provider lock. No other provider uses this path.
pub(super) fn save_magpie_key(backend: &Backend, key: &str) -> LocalResult<()> {
    use super::credentials::{validate_literal_key, AuthType};
    validate_literal_key(key, "magpie")?;
    let query = super::query::Query::new(QUERY_TIMEOUT);
    let mut helper = Helper::spawn_observed(backend, &query, true)?;
    helper.precheck_cancellable(query.deadline, || false)?;
    let id = uuid::Uuid::new_v4().to_string();
    let rpc = helper.auth_command(&json!({"schema":1,"operationId":id,"operation":"login",
        "providerId":"magpie","authType":"api_key","uiSchema":1}))?;
    let mut answered = false;
    loop {
        helper.check(query.deadline)?;
        let Some(record) = helper.record()? else { thread::sleep(Duration::from_millis(5)); continue; };
        if record["type"] == "response" && record["id"] == rpc && record["success"] != true
            || record["type"] == "extension_error" && record["extensionPath"] == "command:kota-auth" {
            return Err(LocalError::new("auth_helper_failed"));
        }
        if record["type"] != "extension_ui_request" { continue; }
        let event = auth_protocol::decode(&record, &id, Some("magpie"), Some(AuthType::ApiKey))?;
        if event["event"] == "prompt" {
            if answered || event["credentialField"] != "api_key.key" || event["prompt"]["type"] != "secret" {
                return Err(LocalError::new("auth_protocol_invalid"));
            }
            let native = record["id"].as_str().filter(|s| super::error::identifier(s, 128))
                .ok_or_else(|| LocalError::new("auth_protocol_invalid"))?;
            helper.process.send(json!({"type":"extension_ui_response","id":native,"value":key}))
                .map_err(|_| LocalError::new("auth_helper_failed"))?;
            answered = true;
        } else if event["event"] == "result" {
            if !answered || event["operation"] != "login" || event["commit"] != "confirmed" {
                return Err(LocalError::new("auth_helper_failed"));
            }
            result_success(event)?;
            return Ok(());
        }
    }
}

pub(crate) fn respond(reply: AuthReply) -> LocalResult<()> {
    let handle = operations()
        .lock()
        .unwrap()
        .get(&reply.operation_id)
        .cloned()
        .ok_or_else(|| LocalError::new("auth_prompt_closed"))?;
    let (tx, rx) = mpsc::sync_channel(1);
    handle
        .input
        .try_send(Input::Respond(reply, tx))
        .map_err(|_| LocalError::new("auth_prompt_closed"))?;
    rx.recv_timeout(QUERY_TIMEOUT)
        .map_err(|_| LocalError::new("auth_timeout"))?
}
pub(crate) fn cancel(id: &str) -> LocalResult<()> {
    let handle = operations()
        .lock()
        .unwrap()
        .get(id)
        .cloned()
        .ok_or_else(|| LocalError::new("auth_prompt_closed"))?;
    let (tx, rx) = mpsc::sync_channel(1);
    handle
        .input
        .try_send(Input::Cancel(tx))
        .map_err(|_| LocalError::new("auth_prompt_closed"))?;
    rx.recv_timeout(CANCEL_TIMEOUT)
        .map_err(|_| LocalError::new("auth_timeout"))?
}
