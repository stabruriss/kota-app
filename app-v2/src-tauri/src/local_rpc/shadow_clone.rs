//! Parent-owned, parallel Pi copies. No extension, ledger, routing or new prompt
//! is installed in a copy. Only its final reply enters the parent's input queue.
use super::{
    process::{PipeEvent, Process},
    protocol::LocalScope,
    runtime::Snapshot,
    workbench::Workbench,
    OnlineState,
};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub(crate) const CODEC: &str = "kota.clone.v1:";
pub(crate) const REPORT_TYPE: &str = "kota.shadow_clone";
const RUNNING_LIMIT: usize = 7;
const REPORT_LIMIT: usize = 20 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Task {
    pub task: String,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default = "default_timeout")]
    pub timeout_minutes: f64,
}
fn default_timeout() -> f64 {
    90.0
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "lowercase", deny_unknown_fields)]
pub(crate) enum Request {
    Create {
        tasks: Vec<Task>,
    },
    Ask {
        clone_id: u64,
        message: String,
        timeout_minutes: Option<f64>,
    },
    Status,
    Stop {
        clone_id: u64,
    },
    Destroy {
        clone_id: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum State {
    Running,
    Finished,
    Stopped,
}

pub(crate) struct Record {
    id: u64,
    state: State,
    started_at: String,
    elapsed_ms: u64,
    read_only: bool,
    timeout_minutes: f64,
    last_reply: String,
    directory: PathBuf,
    snapshot: Snapshot,
    view: Workbench,
}
pub(crate) type Records = Arc<Mutex<BTreeMap<u64, Record>>>;

/// The frontend clock supplies elapsed time while running; no query starts a Pi.
pub(crate) fn read(records: &Records, monitor: Option<u64>) -> Value {
    let records = records.lock().unwrap_or_else(|e| e.into_inner());
    json!({"clones":records.values().map(|record| json!({
        "id":record.id,"state":record.state,"startedAt":record.started_at,
        "elapsedMs":record.elapsed_ms,"lastReply":prefix(&record.last_reply, 1000),
        "view":(monitor == Some(record.id)).then(|| record.view.read(record.snapshot.clone(),None)),
    })).collect::<Vec<_>>()})
}

/// After the owner has exited all its processes are gone. Dismiss can still
/// remove a stopped record, without waking an agent or touching another owner.
pub(crate) fn dismiss(records: &Records, id: u64) -> Result<Value> {
    let mut records = records.lock().unwrap_or_else(|e| e.into_inner());
    let record = records.get(&id).context("clone_not_found")?;
    if record.state != State::Stopped {
        bail!("clone_running");
    }
    if record.directory.exists() {
        fs::remove_dir_all(&record.directory)?;
    }
    records.remove(&id);
    Ok(json!({"message":format!("Clone #{id} was destroyed.")}))
}

#[derive(Clone)]
struct Launch {
    executable: OsString,
    cwd: PathBuf,
    environment: Vec<(OsString, Option<OsString>)>,
    root: PathBuf,
    parent: LocalScope,
}
impl Launch {
    fn command(
        &self,
        id: u64,
        model: &super::models::ModelSelection,
        thinking: &str,
        read_only: bool,
    ) -> Command {
        let directory = self.root.join(format!("clone-{id}"));
        let mut command = Command::new(&self.executable);
        command.current_dir(&self.cwd).env_clear();
        for (key, value) in &self.environment {
            if let Some(value) = value {
                command.env(key, value);
            }
        }
        command
            .env_remove("KOTA_LOCAL_AUTH_HELPER")
            .env_remove("KOTA_LOCAL_INSTALLATION_UUID")
            .env("PI_CODING_AGENT_SESSION_DIR", &directory);
        command
            .args([
                "--mode",
                "rpc",
                "-nc",
                "-ns",
                "-ne",
                "--no-approve",
                "--no-themes",
                "--offline",
                "--session-id",
                &format!("clone-{id}"),
                "--session-dir",
            ])
            .arg(&directory)
            .args([
                "--model",
                &format!("{}/{}", model.provider_id, model.model_id),
                "--thinking",
                thinking,
            ]);
        if read_only {
            command.args(["--tools", "read,grep,find,ls"]);
        }
        command
    }
}

enum Pending {
    Ready { task: String, deadline: Instant },
    Prompt { deadline: Instant },
    None,
}
struct Job {
    process: Process,
    pending: Pending,
    since: Instant,
    deadline: Option<Instant>,
    reply: String,
}

pub(crate) struct Clones {
    launch: Launch,
    pub records: Records,
    jobs: BTreeMap<u64, Job>,
    next: Option<u64>,
    #[cfg(test)]
    fail_stop: bool,
}
impl Clones {
    pub fn new(command: &Command, parent: &LocalScope, records: Records) -> Self {
        let cwd = command
            .get_current_dir()
            .unwrap_or(&parent.project_root)
            .to_owned();
        // The normal Local cwd is the incarnation directory. Never use the
        // shared owned Pi directory or its parent session directory for records.
        let agent_cwd = command
            .get_envs()
            .find(|(key, _)| *key == "KOTA_AGENT_CWD")
            .and_then(|(_, value)| value)
            .map(PathBuf::from)
            .unwrap_or_else(|| cwd.clone());
        Self {
            launch: Launch {
                executable: command.get_program().to_owned(),
                cwd,
                environment: command
                    .get_envs()
                    .map(|(key, value)| (key.to_owned(), value.map(|v| v.to_owned())))
                    .collect(),
                root: agent_cwd.join("under-table"),
                parent: parent.clone(),
            },
            records,
            jobs: BTreeMap::new(),
            next: None,
            #[cfg(test)]
            fail_stop: command.get_envs().any(|(key, value)| {
                key == "KOTA_TEST_CLONE_STOP_FAILURE" && value.is_some_and(|value| value == "1")
            }),
        }
    }

    pub fn action(
        &mut self,
        request: Request,
        model: Option<super::models::ModelSelection>,
        thinking: &str,
    ) -> Result<Value> {
        match request {
            Request::Status => {
                let records = self.records.lock().unwrap_or_else(|e| e.into_inner());
                Ok(
                    json!({"clones":records.values().map(|r|json!({"id":r.id,"state":r.state,
                    "started_at":r.started_at,"last_reply":prefix(&r.last_reply,1000)})).collect::<Vec<_>>()}),
                )
            }
            Request::Create { tasks } => {
                if tasks.is_empty()
                    || tasks
                        .iter()
                        .any(|task| task.task.is_empty() || timeout(task.timeout_minutes).is_err())
                {
                    bail!("clone_invalid_request");
                }
                if self.running() + tasks.len() > RUNNING_LIMIT {
                    bail!("Too many clones running: at most 7 at once.");
                }
                let model = model.context("clone_model_unavailable")?;
                self.number()?;
                let mut ids = Vec::new();
                for task in tasks {
                    let id = self.next.context("clone_invalid_request")?;
                    self.next = id.checked_add(1);
                    let directory = self.launch.root.join(format!("clone-{id}"));
                    fs::create_dir(&directory)?;
                    self.records
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(
                            id,
                            Record {
                                id,
                                state: State::Stopped,
                                started_at: String::new(),
                                elapsed_ms: 0,
                                read_only: task.read_only,
                                timeout_minutes: task.timeout_minutes,
                                last_reply: String::new(),
                                directory,
                                snapshot: Snapshot {
                                    scope: LocalScope {
                                        session_id: format!("clone-{id}"),
                                        ..self.launch.parent.clone()
                                    },
                                    state: OnlineState::Offline,
                                    queued: 0,
                                    compacting: false,
                                    child_pid: None,
                                    model: None,
                                    thinking_level: None,
                                    error: None,
                                },
                                view: Workbench::default(),
                            },
                        );
                    self.begin(id, task.task, &model, thinking)?;
                    ids.push(id);
                }
                Ok(json!({"clone_ids":ids}))
            }
            Request::Ask {
                clone_id,
                message,
                timeout_minutes,
            } => {
                if message.is_empty() {
                    bail!("clone_invalid_request");
                }
                if self.state(clone_id)? == State::Running {
                    bail!("clone_running");
                }
                if self.running() >= RUNNING_LIMIT {
                    bail!("Too many clones running: at most 7 at once.");
                }
                let model = model.context("clone_model_unavailable")?;
                if let Some(minutes) = timeout_minutes {
                    timeout(minutes)?;
                    self.records
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .get_mut(&clone_id)
                        .unwrap()
                        .timeout_minutes = minutes;
                }
                // Reopen the exact session to apply this run's current physical
                // model and thinking. Pi retains the clone's own branch/history.
                if let Some(mut old) = self.jobs.remove(&clone_id) {
                    old.process.stop()?;
                }
                self.begin(clone_id, message, &model, thinking)?;
                Ok(json!({"clone_id":clone_id}))
            }
            Request::Stop { clone_id } => {
                self.state(clone_id)?;
                self.stop_one(clone_id)?;
                Ok(json!({"message":format!("Clone #{clone_id} stopped.")}))
            }
            Request::Destroy { clone_id } => {
                self.state(clone_id)?;
                self.stop_one(clone_id)?;
                dismiss(&self.records, clone_id)
            }
        }
    }

    fn state(&self, id: u64) -> Result<State> {
        self.records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .map(|r| r.state)
            .context("clone_not_found")
    }
    fn running(&self) -> usize {
        self.records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|r| r.state == State::Running)
            .count()
    }
    fn number(&mut self) -> Result<()> {
        if self.next.is_some() {
            return Ok(());
        }
        fs::create_dir_all(&self.launch.root)?;
        let maximum = fs::read_dir(&self.launch.root)?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| {
                e.file_name()
                    .to_str()
                    .and_then(|s| s.strip_prefix("clone-"))
                    .and_then(|s| s.parse::<u64>().ok())
            })
            .max()
            .unwrap_or(0);
        self.next = Some(maximum.checked_add(1).context("clone_invalid_request")?);
        Ok(())
    }
    fn begin(
        &mut self,
        id: u64,
        task: String,
        model: &super::models::ModelSelection,
        thinking: &str,
    ) -> Result<()> {
        let (read_only, minutes) = {
            let records = self.records.lock().unwrap_or_else(|e| e.into_inner());
            let record = records.get(&id).context("clone_not_found")?;
            (record.read_only, record.timeout_minutes)
        };
        let mut process = Process::spawn(self.launch.command(id, model, thinking, read_only))?;
        process.send(json!({"id":"clone-ready","type":"get_state"}))?;
        let since = Instant::now();
        self.jobs.insert(
            id,
            Job {
                process,
                pending: Pending::Ready {
                    task,
                    deadline: since + COMMAND_TIMEOUT,
                },
                since,
                deadline: timeout(minutes)?.map(|duration| since + duration),
                reply: String::new(),
            },
        );
        let mut records = self.records.lock().unwrap_or_else(|e| e.into_inner());
        let record = records.get_mut(&id).unwrap();
        record.state = State::Running;
        record.started_at = chrono::Utc::now().to_rfc3339();
        record.elapsed_ms = 0;
        record.snapshot.state = OnlineState::Starting;
        record.snapshot.error = None;
        record.snapshot.child_pid = Some(self.jobs[&id].process.id());
        record.snapshot.model = Some(json!({"provider":model.provider_id,"id":model.model_id}));
        record.snapshot.thinking_level = Some(thinking.into());
        Ok(())
    }

    /// Called by the existing owner loop. Each copy has a separate pipe and
    /// process group; a slow copy never waits in front of another copy's turn.
    pub fn tick(&mut self) -> Vec<(u64, String)> {
        let mut reports = Vec::new();
        for id in self.jobs.keys().copied().collect::<Vec<_>>() {
            match self.tick_one(id) {
                Ok(reply) => reports.extend(reply.into_iter().map(|report| (id, report))),
                Err(error) => self.fail(id, &error.to_string()),
            }
        }
        reports
    }

    fn tick_one(&mut self, id: u64) -> Result<Vec<String>> {
        let mut reports = Vec::new();
        if self.state(id)? == State::Running
            && self.jobs[&id]
                .deadline
                .is_some_and(|end| Instant::now() >= end)
        {
            let minutes =
                self.records.lock().unwrap_or_else(|e| e.into_inner())[&id].timeout_minutes;
            let message =
                format!("Clone #{id} timed out after {minutes} minutes. It can still be asked.");
            self.stop_one(id)?;
            self.note(id, &message);
            reports.push(message);
            return Ok(reports);
        }
        let mut failed = None;
        for _ in 0..64 {
            let Some(event) = self.jobs.get_mut(&id).unwrap().process.poll() else {
                break;
            };
            match event {
                PipeEvent::Written(_) => {}
                PipeEvent::Eof => {
                    failed = Some("clone_process_exited".to_string());
                    break;
                }
                PipeEvent::Record(event, _credit) => match self.record(id, &event) {
                    Ok(Some(report)) => reports.push(report),
                    Ok(None) => {}
                    Err(error) => {
                        failed = Some(error.to_string());
                        break;
                    }
                },
            }
        }
        if failed.is_none() {
            let job = self.jobs.get_mut(&id).unwrap();
            failed = job.process.fault().or_else(|| {
                job.process
                    .try_wait()
                    .ok()
                    .flatten()
                    .map(|_| "clone_process_exited".into())
            });
            let deadline = match &job.pending {
                Pending::Ready { deadline, .. } | Pending::Prompt { deadline } => Some(*deadline),
                Pending::None => None,
            };
            if deadline.is_some_and(|end| Instant::now() >= end) {
                failed = Some("clone_command_timeout".into());
            }
        }
        if let Some(error) = failed {
            let running = self.state(id)? == State::Running;
            self.stop_one(id)?;
            self.note(id, &error);
            if running {
                reports.push(format!("Clone #{id} finished:\n{error}"));
            }
        } else if self.state(id)? == State::Finished {
            // Finished records have no count limit. Release the idle Pi,
            // keeping its native session for ask rather than accumulating
            // one resident process per historical copy.
            if let Some(mut job) = self.jobs.remove(&id) {
                self.stop_process(&mut job)?;
            }
            self.records
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_mut(&id)
                .unwrap()
                .snapshot
                .child_pid = None;
        }
        Ok(reports)
    }

    fn record(&mut self, id: u64, event: &Value) -> Result<Option<String>> {
        if event["type"] == "response" {
            let job = self.jobs.get_mut(&id).unwrap();
            if event["id"] == "clone-ready" {
                let Pending::Ready { task, .. } =
                    std::mem::replace(&mut job.pending, Pending::None)
                else {
                    return Ok(None);
                };
                if event["success"] != true || event["data"]["sessionId"] != format!("clone-{id}") {
                    bail!("clone_session_unavailable");
                }
                // Pi must confirm the concrete selection; never let an old
                // saved session or ambient model replace the parent's choice.
                let expected =
                    &self.records.lock().unwrap_or_else(|e| e.into_inner())[&id].snapshot;
                if event["data"]["model"]["provider"]
                    != expected.model.as_ref().unwrap()["provider"]
                    || event["data"]["model"]["id"] != expected.model.as_ref().unwrap()["id"]
                {
                    bail!("clone_model_unavailable");
                }
                if event["data"]["thinkingLevel"].as_str() != expected.thinking_level.as_deref() {
                    bail!("clone_model_unavailable");
                }
                job.process
                    .send(json!({"id":"clone-prompt","type":"prompt","message":task}))?;
                job.pending = Pending::Prompt {
                    deadline: Instant::now() + COMMAND_TIMEOUT,
                };
            } else if event["id"] == "clone-prompt" {
                job.pending = Pending::None;
                if event["success"] != true {
                    bail!("clone_prompt_failed");
                }
            }
            return Ok(None);
        }
        let mut records = self.records.lock().unwrap_or_else(|e| e.into_inner());
        let record = records.get_mut(&id).unwrap();
        record.view.event(event);
        match event["type"].as_str() {
            Some("agent_start") => record.snapshot.state = OnlineState::Busy,
            Some("compaction_start") => record.snapshot.compacting = true,
            Some("compaction_end") => record.snapshot.compacting = false,
            Some("message_end") if event["message"]["role"] == "assistant" => {
                let message = &event["message"];
                if !matches!(
                    message["stopReason"].as_str(),
                    Some("toolUse" | "tool_calls")
                ) {
                    self.jobs.get_mut(&id).unwrap().reply = message["content"]
                        .as_array()
                        .map(|parts| {
                            parts
                                .iter()
                                .filter(|part| part["type"] == "text")
                                .filter_map(|part| part["text"].as_str())
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .unwrap_or_default();
                    if message["stopReason"] == "error" {
                        self.jobs.get_mut(&id).unwrap().reply = message["errorMessage"]
                            .as_str()
                            .unwrap_or("clone_request_failed")
                            .to_owned();
                    }
                }
            }
            Some("agent_settled") if record.state == State::Running => {
                let job = self.jobs.get_mut(&id).unwrap();
                record.state = State::Finished;
                record.snapshot.state = OnlineState::Ready;
                record.elapsed_ms = job.since.elapsed().as_millis().min(u64::MAX as u128) as u64;
                record.last_reply = std::mem::take(&mut job.reply);
                job.deadline = None;
                let report = report(id, &record.last_reply);
                // The whole reply is already in the native under-table file.
                // Keep only the report-sized excerpt in memory for status.
                let end = prefix(&record.last_reply, REPORT_LIMIT).len();
                record.last_reply.truncate(end);
                return Ok(Some(report));
            }
            _ => {}
        }
        Ok(None)
    }
    fn note(&mut self, id: u64, message: &str) {
        let mut records = self.records.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(record) = records.get_mut(&id) {
            if record.snapshot.error.as_deref() == Some(message) {
                return;
            }
            record.last_reply = message.into();
            record.snapshot.error = Some(message.into());
            record.view.event(&json!({"type":"message_end","message":{"role":"assistant","stopReason":"error","content":[{"type":"text","text":message}]}}));
            crate::kota_debug_log(&format!("local-rpc: clone {id} failed (payload omitted)"));
        }
    }

    /// A copy cannot fail the parent owner or abandon its pending inputs.
    pub fn fail(&mut self, id: u64, error: &str) {
        let _ = self.stop_one(id);
        self.note(id, error);
    }

    fn stop_process(&self, job: &mut Job) -> Result<()> {
        job.process.stop()?;
        // Exercise error isolation after real cleanup so the fixture cannot
        // leak a child. This injection does not exist in production builds.
        #[cfg(test)]
        if self.fail_stop {
            return Err(std::io::Error::from_raw_os_error(libc::EPERM).into());
        }
        Ok(())
    }
    fn stop_one(&mut self, id: u64) -> Result<()> {
        let mut result = Ok(());
        if let Some(mut job) = self.jobs.remove(&id) {
            result = self.stop_process(&mut job);
            let mut records = self.records.lock().unwrap_or_else(|e| e.into_inner());
            let record = records.get_mut(&id).context("clone_not_found")?;
            record.elapsed_ms = job.since.elapsed().as_millis().min(u64::MAX as u128) as u64;
        }
        if let Some(record) = self
            .records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(&id)
        {
            record.state = State::Stopped;
            record.snapshot.state = OnlineState::Offline;
            record.snapshot.child_pid = None;
            record.view.event(&json!({"type":"agent_settled"}));
        }
        if let Err(error) = &result {
            self.note(id, &error.to_string());
        }
        result
    }
    pub fn stop_all(&mut self) {
        let ids = self
            .records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for id in ids {
            let _ = self.stop_one(id);
        }
    }
}
impl Drop for Clones {
    fn drop(&mut self) {
        self.stop_all();
    }
}

fn timeout(minutes: f64) -> Result<Option<Duration>> {
    if minutes == 0.0 {
        return Ok(None);
    }
    if minutes < 0.0 {
        bail!("clone_invalid_request");
    }
    Duration::try_from_secs_f64(minutes * 60.0)
        .map(Some)
        .map_err(|_| anyhow!("clone_invalid_request"))
}
fn prefix(text: &str, limit: usize) -> &str {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
fn report(id: u64, reply: &str) -> String {
    let header = format!("Clone #{id} finished:\n");
    if header.len() + reply.len() <= REPORT_LIMIT {
        return header + reply;
    }
    let suffix = format!("\nThe full record is in under-table/clone-{id}/.");
    header.clone() + prefix(reply, REPORT_LIMIT - header.len() - suffix.len()) + &suffix
}

#[cfg(test)]
#[path = "shadow_clone_tests.rs"]
mod tests;
