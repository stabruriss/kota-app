use super::{ledger::{Entry, Ledger}, process::{PipeEvent, Process}, protocol::*};
use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const CONTROL_WAIT: Duration = Duration::from_secs(35);
const MAX_OUTSTANDING: usize = 64;
const MAX_OUTSTANDING_BYTES: usize = 32 * 1024 * 1024;
type Reply<T> = mpsc::SyncSender<std::result::Result<T, String>>;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    pub scope: LocalScope,
    pub state: OnlineState,
    pub queued: usize,
    pub compacting: bool,
    pub child_pid: Option<u32>,
    pub model: Option<Value>,
    pub thinking_level: Option<String>,
    pub error: Option<String>,
}

pub(crate) struct Config {
    pub scope: LocalScope,
    pub ledger: PathBuf,
    pub command: Command,
    pub expected_model: Option<super::models::ModelSelection>,
    /// Registry replacement epoch check; old owners stop before further writes.
    pub current: Arc<dyn Fn() -> bool + Send + Sync>,
}

enum Control {
    Activate { fresh: bool, reply: Reply<()> },
    Submit(Injection, Reply<()>),
    Retry(String, Reply<()>),
    Input(String, Reply<Option<Injection>>),
    QueueText(String, Reply<Value>),
    Rpc(Value, Reply<Value>),
    QueueAction(Value, Reply<Value>),
    Stop(Reply<Value>),
    Close(Reply<()>),
    RefreshAuth,
    CloneAction(super::shadow_clone::Request, Reply<Value>),
}

#[derive(Clone)]
pub(crate) struct LocalRuntime { inner: Arc<Handle> }
struct Handle {
    controls: mpsc::SyncSender<Control>,
    snapshot: Arc<Mutex<Snapshot>>,
    view: Arc<Mutex<super::workbench::Workbench>>,
    clones: super::shadow_clone::Records,
    join: Mutex<Option<thread::JoinHandle<()>>>,
    current: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl LocalRuntime {
    /// First get_state + full entry reconciliation complete before returning.
    /// Injection stays paused until the registry commits the launch metadata.
    pub fn start(config: Config, sink: Arc<dyn Sink>) -> Result<Self> {
        let ledger = Ledger::open(&config.ledger)?;
        let snapshot = Arc::new(Mutex::new(Snapshot {
            scope: config.scope.clone(), state: OnlineState::Starting, queued: ledger.queued(&config.scope.session_id),
            compacting: false, child_pid: None, model: None, thinking_level: None, error: None,
        }));
        let mut view = super::workbench::Workbench::default();
        view.refresh_queue(&ledger, &config.scope.session_id);
        let view = Arc::new(Mutex::new(view));
        let shared_view = view.clone();
        let clones = Arc::new(Mutex::new(BTreeMap::new()));
        let shared_clones = clones.clone();
        let (controls, rx) = mpsc::sync_channel(64);
        let (ready, ready_rx) = mpsc::sync_channel(1);
        let shared = snapshot.clone();
        let current = config.current.clone();
        let join = thread::Builder::new().name("local-rpc-owner".into()).spawn(move || {
            Owner::run(config, ledger, sink, shared, shared_view, shared_clones, rx, ready);
        })?;
        let runtime = Self { inner: Arc::new(Handle { controls, snapshot, view, clones, join: Mutex::new(Some(join)), current }) };
        match ready_rx.recv_timeout(CONTROL_WAIT) {
            Ok(Ok(())) => Ok(runtime),
            Ok(Err(error)) => { let _ = runtime.close(); Err(anyhow!(error)) },
            Err(_) => { let _ = runtime.close(); bail!("Local startup timed out"); },
        }
    }

    fn request<T>(&self, make: impl FnOnce(Reply<T>) -> Control) -> Result<T> {
        self.request_with_timeout(CONTROL_WAIT, make)
    }

    fn request_with_timeout<T>(&self, timeout: Duration, make: impl FnOnce(Reply<T>) -> Control) -> Result<T> {
        let (reply, rx) = mpsc::sync_channel(1);
        self.inner.controls.try_send(make(reply)).map_err(|_| anyhow!("Local owner is busy or stopped"))?;
        rx.recv_timeout(timeout).map_err(|_| anyhow!("Local command timed out; delivery remains unconfirmed"))?
            .map_err(|error| anyhow!(error))
    }

    pub fn activate(&self, fresh: bool) -> Result<()> { self.request(|reply| Control::Activate { fresh, reply }) }
    pub fn submit(&self, input: Injection) -> Result<()> { self.request(|reply| Control::Submit(input, reply)) }
    pub fn retry(&self, id: String) -> Result<()> { self.request(|reply| Control::Retry(id, reply)) }
    pub fn input(&self, id: String) -> Result<Option<Injection>> { self.request(|reply| Control::Input(id, reply)) }
    pub fn rpc(&self, request: Value) -> Result<Value> {
        // Stop has four existing, individually bounded commands. The caller
        // waits for their result; it must not time out while the owner proceeds.
        if request["type"] == "abort" { return self.request_with_timeout(COMMAND_TIMEOUT * 4 + Duration::from_secs(5), Control::Stop); }
        self.request(|reply| Control::Rpc(request, reply))
    }
    pub fn queue_action(&self, request: Value) -> Result<Value> { self.request(|reply| Control::QueueAction(request, reply)) }
    pub fn snapshot(&self) -> Snapshot { self.inner.snapshot.lock().unwrap().clone() }
    pub fn workbench(&self, since: Option<&super::WorkbenchCursor>) -> super::WorkbenchView {
        let snapshot = self.snapshot();
        self.inner.view.lock().unwrap().read(snapshot, since)
    }
    pub fn queue_text(&self, id: String) -> Result<Value> { self.request(|reply| Control::QueueText(id, reply)) }
    pub fn clones(&self, monitor: Option<u64>) -> Value { super::shadow_clone::read(&self.inner.clones,monitor) }
    pub fn clone_control(&self, request: super::shadow_clone::Request) -> Result<Value> {
        if self.snapshot().child_pid.is_none() {
            return match request {
                super::shadow_clone::Request::Destroy { clone_id } => super::shadow_clone::dismiss(&self.inner.clones,clone_id),
                _ => Err(anyhow!("Local session is offline")),
            };
        }
        self.request(|reply| Control::CloneAction(request,reply))
    }

    #[cfg(test)]
    pub(super) fn inner_join_finished_for_test(&self) -> bool {
        self.inner.join.lock().unwrap().as_ref().is_none_or(thread::JoinHandle::is_finished)
    }
    pub fn is_current(&self) -> bool { (self.inner.current)() }
    pub fn refresh_auth(&self) {
        if self.inner.controls.try_send(Control::RefreshAuth).is_err() { crate::kota_debug_log("local-rpc: auth refresh owner unavailable"); }
    }
    pub fn close(&self) -> Result<()> {
        if self.inner.join.lock().unwrap().is_none() { return Ok(()); }
        let result = self.request(Control::Close);
        let snapshot = self.snapshot();
        if result.is_ok() || (snapshot.child_pid.is_none()
            && (!self.is_current() || matches!(snapshot.state, OnlineState::Offline | OnlineState::Error)))
            || self.inner.join.lock().unwrap().as_ref().is_some_and(|join| join.is_finished()) {
            if let Some(join) = self.inner.join.lock().unwrap().take() {
                join.join().map_err(|_| anyhow!("Local owner panicked"))?;
            }
            return Ok(());
        }
        result
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        let (tx, _rx) = mpsc::sync_channel(1);
        let _ = self.controls.try_send(Control::Close(tx));
        // Dropping controls also wakes the owner and closes its process. No
        // process lifetime is tied to a mounted terminal or room component.
    }
}

enum Purpose {
    InitialState,
    Entries { recovery: bool, since: Option<String> },
    Inject { kota_id: String, rejected: Option<Rejection> },
    External { kind: String, model: Option<super::models::ModelSelection>, reply: Reply<Value> },
    AuthRefresh { operation: String, acknowledged: bool, result: Option<bool> },
    McpStatus { acknowledged: bool, result: Option<Value>, reply: Reply<Value> },
    Cancel { kota_id: String, edit: bool, generation: u64, rejected: Option<Rejection>, reply: Reply<Value> },
    Reorder { ids: Vec<String>, changed: bool, reply: Reply<Value> },
    StopHold { action: HoldAction, acknowledged: bool, receipt: Option<Vec<String>>, reply: Reply<Value> },
    StopAbort(Reply<Value>),
    StopQueue(Reply<Value>),
}
struct Pending { purpose: Purpose, deadline: Instant }
struct QueuedCancel { request: Value, reply: Reply<Value>, deadline: Instant }
struct QueuedReorder { ids: Vec<String>, reply: Reply<Value>, deadline: Instant }
struct Persisted { token: u64, next: Option<String>, result: std::result::Result<(), String> }
struct Notification {
    notice: LocalNotice,
    acknowledgement: Option<(u64, Option<String>)>,
    // Recovery is one ordered projection operation, not a burst into the
    // bounded live-event channel that could permanently block a large queue.
    after_recovery: Vec<LocalNotice>,
}

struct Owner {
    clones: super::shadow_clone::Clones,
    physical_model: Option<super::models::ModelSelection>,
    physical_thinking: Option<String>,
    scope: LocalScope,
    ledger: Ledger,
    process: Option<Process>,
    current: Arc<dyn Fn() -> bool + Send + Sync>,
    shared: Arc<Mutex<Snapshot>>,
    view: Arc<Mutex<super::workbench::Workbench>>,
    pending: BTreeMap<String, Pending>,
    counter: u64,
    ready: Option<Reply<()>>,
    active: bool,
    initialized: bool,
    busy: bool,
    compacting: bool,
    active_inject: Option<String>,
    stop_ids: BTreeMap<String, u64>,
    stopping: bool,
    edit_tokens: BTreeMap<String, String>,
    edit_generation: u64,
    cancellations: VecDeque<QueuedCancel>,
    reorder: Option<QueuedReorder>,
    credits: BTreeMap<String, usize>,
    credit_bytes: usize,
    since: Option<String>,
    entries_pending: bool,
    entries_again: bool,
    entries_token: u64,
    notices: mpsc::SyncSender<Notification>,
    persisted: mpsc::Receiver<Persisted>,
    last_status: Option<(OnlineState, usize, bool, Option<String>)>,
    expected_model: Option<super::models::ModelSelection>,
    refresh_auth: bool,
}

impl Owner {
    fn run(config: Config, ledger: Ledger, sink: Arc<dyn Sink>, shared: Arc<Mutex<Snapshot>>,
        view: Arc<Mutex<super::workbench::Workbench>>, clones: super::shadow_clone::Records,
        controls: mpsc::Receiver<Control>, ready: Reply<()>) {
        let (notices, incoming) = mpsc::sync_channel::<Notification>(128);
        let (ack, persisted) = mpsc::sync_channel(8);
        let notify_current = config.current.clone();
        let notifier = thread::Builder::new().name("local-rpc-notifier".into()).spawn(move || {
            for notification in incoming {
                if !matches!(notification.notice, LocalNotice::Entries { .. }) && !notify_current() { continue; }
                let result = sink.notify(&notification.notice);
                for fact in notification.after_recovery {
                    if !notify_current() { break; }
                    // Terminal disk facts also retire stale pending entries
                    // when the recovery projection itself could not be saved.
                    if result.is_err() && !matches!(fact, LocalNotice::Delivery { state, .. } if state.terminal()) { continue; }
                    if sink.notify(&fact).is_err() {
                        crate::kota_debug_log("local-rpc: delivery projection failed (payload omitted)");
                    }
                }
                if let Some((token, next)) = notification.acknowledgement {
                    if ack.send(Persisted { token, next, result }).is_err() { break; }
                } else if result.is_err() {
                    crate::kota_debug_log("local-rpc: projection notification failed (payload omitted)");
                }
            }
        });
        let mut owner = Self {
            clones:super::shadow_clone::Clones::new(&config.command,&config.scope,clones),
            physical_model:None,physical_thinking:None,
            scope: config.scope, ledger, process: None, current: config.current, shared, view,
            pending: BTreeMap::new(), counter: 0, ready: Some(ready), active: false, initialized: false, busy: false,
            compacting: false, active_inject: None, stop_ids: BTreeMap::new(), stopping: false,
            edit_tokens: BTreeMap::new(), edit_generation: 0, cancellations: VecDeque::new(), reorder: None, credits: BTreeMap::new(), credit_bytes: 0, since: None,
            entries_pending: false, entries_again: false, entries_token: 0, notices, persisted, last_status: None,
            expected_model:config.expected_model, refresh_auth:false,
        };
        let startup = (|| -> Result<()> {
            notifier.context("start Local notifier")?;
            if !(owner.current)() { bail!("Local launch superseded"); }
            owner.process = Some(Process::spawn(config.command)?);
            owner.shared.lock().unwrap().child_pid = owner.process.as_ref().map(Process::id);
            owner.status(OnlineState::Starting, None)?;
            owner.send(json!({"type":"get_state"}), Purpose::InitialState)?;
            Ok(())
        })();
        if let Err(error) = startup { owner.failure(error.to_string()); }
        loop {
            if !(owner.current)() {
                if owner.stop(false).is_ok() { break; }
                // Retain the owner if OS process cleanup failed. No more input
                // is dispatched under a superseded epoch.
                owner.active = false;
            }
            let mut pipe_count = 0;
            while let Some(event) = owner.process.as_ref().and_then(Process::poll) {
                if let Err(error) = owner.pipe(event) { owner.failure(error.to_string()); break; }
                pipe_count += 1;
                if pipe_count >= 64 { break; }
            }
            while let Ok(ack) = owner.persisted.try_recv() {
                if ack.token == owner.entries_token {
                    owner.entries_pending = false;
                    if ack.result.is_ok() {
                        owner.since = ack.next;
                        if owner.ready.is_some() {
                            if let Err(error) = owner.status(if owner.busy { OnlineState::Busy } else { OnlineState::Ready }, None) {
                                owner.failure(error.to_string());
                            } else if let Some(ready) = owner.ready.take() { let _ = ready.send(Ok(())); }
                        }
                    } else if owner.ready.is_some() {
                        // Recovery clears/rebuilds the projection's pending
                        // set. Do not activate a child if that boundary failed.
                        owner.failure("Local recovery projection failed; queued deliveries abandoned".into());
                    } else { crate::kota_debug_log("local-rpc: entries projection failed; cursor retained"); }
                }
            }
            let failure = owner.process.as_mut().and_then(|process| {
                process.fault().or_else(|| match process.try_wait() {
                    Ok(Some(status)) => Some(format!("Local Pi RPC exited: {status}")),
                    Err(error) => Some(format!("Local Pi RPC wait failed: {error}")),
                    Ok(None) => None,
                })
            });
            if let Some(error) = failure { owner.failure(error); }
            let now = Instant::now();
            let expired_refreshes: Vec<_> = owner.pending.iter().filter(|(_,p)| p.deadline <= now
                && matches!(p.purpose,Purpose::AuthRefresh {..})).map(|(id,_)|id.clone()).collect();
            for id in expired_refreshes {
                owner.pending.remove(&id);
                crate::kota_debug_log("local-rpc: idle auth refresh timed out (payload omitted)");
            }
            let expired_mcp: Vec<_> = owner.pending.iter().filter(|(_,p)| p.deadline <= now
                && matches!(p.purpose,Purpose::McpStatus {..})).map(|(id,_)|id.clone()).collect();
            for id in expired_mcp { owner.fail_mcp(&id); }
            if owner.pending.values().any(|request| request.deadline <= now)
                || owner.reorder.as_ref().is_some_and(|request| request.deadline <= now)
                || owner.cancellations.front().is_some_and(|request| request.deadline <= Instant::now()) {
                owner.failure("Local RPC command timed out; queued deliveries abandoned".into());
            }
            // A failed child has no live input owner. Release its ledger lock
            // even while the registry retains the error snapshot, so a later
            // launch (including another App's lease takeover) can reconcile.
            if owner.process.is_none() { break; }
            let reports = owner.clones.tick();
            owner.clone_reports(reports);
            if let Err(error) = owner.pump() { owner.failure(error.to_string()); }
            match controls.recv_timeout(Duration::from_millis(10)) {
                Ok(Control::Close(reply)) => {
                    let result = owner.stop(false);
                    let done = result.is_ok();
                    let _ = reply.send(result.map_err(|e| e.to_string()));
                    if done { break; }
                },
                Ok(command) => if let Err(error) = owner.control(command) { owner.failure(error.to_string()); },
                Err(mpsc::RecvTimeoutError::Disconnected) => { let _ = owner.stop(false); break; },
                Err(mpsc::RecvTimeoutError::Timeout) => {},
            }
        }
    }

    fn notify(&self, notice: LocalNotice) -> Result<()> {
        self.notices.try_send(Notification { notice, acknowledgement: None, after_recovery: Vec::new() })
            .map_err(|_| anyhow!("Local projection queue is full or stopped"))
    }

    fn status(&mut self, state: OnlineState, error: Option<String>) -> Result<()> {
        let queued = self.ledger.queued(&self.scope.session_id);
        let key = (state, queued, self.compacting, error.clone());
        {
            let mut snapshot = self.shared.lock().unwrap();
            snapshot.state = state; snapshot.error = error.clone(); snapshot.queued = queued;
            snapshot.compacting = self.compacting;
        }
        if self.last_status.as_ref() == Some(&key) { return Ok(()); }
        self.notify(LocalNotice::Status { scope: self.scope.clone(), state, queued,
            compacting: self.compacting, error })?;
        self.last_status = Some(key);
        Ok(())
    }

    fn delivery_notice(&self, entry: &Entry) -> LocalNotice {
        let mut scope = self.scope.clone(); scope.session_id = entry.session.clone();
        LocalNotice::Delivery { scope, kota_id: entry.injection.kota_id.clone(),
            message_id: entry.injection.details.message_id.clone(), state: entry.state, at: entry.at.clone(),
            custom_type: entry.injection.custom_type.clone() }
    }

    fn delivery(&self, entry: &Entry) -> Result<()> {
        self.view.lock().unwrap().refresh_queue(&self.ledger, &self.scope.session_id);
        self.notify(self.delivery_notice(entry))
    }

    fn fact(&mut self, id: &str, state: DeliveryState) -> Result<()> {
        if let Some(entry) = self.ledger.record(&self.scope.session_id, id, state)? {
            if !entry.state.queued() {
                if let Some(bytes) = self.credits.remove(id) { self.credit_bytes -= bytes; }
            }
            self.delivery(&entry)?;
        }
        Ok(())
    }

    fn send(&mut self, mut request: Value, purpose: Purpose) -> Result<String> {
        if self.pending.len() >= 32 { bail!("Local RPC has too many pending commands"); }
        self.counter += 1;
        let id = format!("local-{}-{}", self.scope.epoch, self.counter);
        request["id"] = json!(id);
        let timeout = COMMAND_TIMEOUT;
        self.process.as_ref().context("Local process is offline")?.send(request)?;
        self.pending.insert(id.clone(), Pending { purpose, deadline: Instant::now() + timeout });
        Ok(id)
    }

    fn request_entries(&mut self, recovery: bool) -> Result<()> {
        if self.entries_pending { self.entries_again = true; return Ok(()); }
        let since = if recovery { None } else { self.since.clone() };
        let mut request = json!({"type":"get_entries"});
        if let Some(since) = &since { request["since"] = json!(since); }
        self.send(request, Purpose::Entries { recovery, since })?;
        self.entries_pending = true;
        Ok(())
    }

    fn send_hold(&mut self, action: HoldAction, reply: Reply<Value>) -> Result<()> {
        let request_id = format!("local-{}-{}", self.scope.epoch, self.counter + 1);
        self.send(json!({"type":"prompt","message":format!("/{} {}", action.command(), json!({"requestId":request_id}))}),
            Purpose::StopHold { action, acknowledged:false, receipt:None, reply })?;
        Ok(())
    }

    fn finish_hold(&mut self, action: HoldAction, ids: Vec<String>, reply: Reply<Value>) -> Result<()> {
        match action {
            HoldAction::Pause => {
                let held: BTreeSet<_> = ids.into_iter().collect();
                self.stop_ids.retain(|id, _| !held.contains(id));
                self.send(json!({"type":"abort"}), Purpose::StopAbort(reply))?;
            },
            HoldAction::Resume => {
                self.stopping = false;
                let _ = reply.send(Ok(json!({})));
                self.status(if self.busy { OnlineState::Busy } else { OnlineState::Ready }, None)?;
            },
        }
        Ok(())
    }

    fn control(&mut self, control: Control) -> Result<()> {
        match control {
            Control::Activate { fresh, reply } => {
                let result = (|| -> Result<()> {
                    if self.process.is_none() { bail!("Local process exited before activation"); }
                    if fresh {
                        for entry in self.ledger.abandon_other_sessions(&self.scope.session_id)? { self.delivery(&entry)?; }
                    }
                    self.active = true;
                    Ok(())
                })();
                let _ = reply.send(result.map_err(|e| e.to_string()));
            },
            Control::Submit(input, reply) => {
                let result = (|| -> Result<()> {
                    input.validate(&self.scope.agent_id)?;
                    let id = input.kota_id.clone();
                    if self.ledger.hold(&self.scope.session_id, input)? {
                        self.delivery(self.ledger.get(&self.scope.session_id, &id).unwrap())?;
                    }
                    self.shared.lock().unwrap().queued = self.ledger.queued(&self.scope.session_id);
                    Ok(())
                })();
                let _ = reply.send(result.map_err(|e| e.to_string()));
            },
            Control::Retry(id, reply) => {
                let result = if self.edit_tokens.contains_key(&id) { Err(anyhow!("This message is being edited.")) }
                    else { self.ledger.retry(&self.scope.session_id, &id)
                    .and_then(|entry| if let Some(entry) = entry { self.delivery(&entry) } else { Ok(()) }) };
                let _ = reply.send(result.map_err(|e| e.to_string()));
            },
            Control::Input(id, reply) => {
                let _ = reply.send(Ok(self.ledger.get(&self.scope.session_id, &id).map(|e| e.injection.clone())));
            },
            Control::QueueText(id, reply) => {
                let result = self.ledger.get(&self.scope.session_id, &id).filter(|e| e.state.queued())
                    .map(|e| { let (text, truncated) = super::workbench::queue_text(&e.injection, 16 * 1024);
                        json!({"text":text,"truncated":truncated}) })
                    .ok_or_else(|| "This message is no longer queued.".to_string());
                let _ = reply.send(result);
            },
            Control::Rpc(request, reply) if request["type"] == "get_mcp_status" => {
                if request.as_object().is_none_or(|r| r.len() != 1) {
                    let _ = reply.send(Err("mcp_status_unavailable".into())); return Ok(());
                }
                let id = format!("local-{}-{}",self.scope.epoch,self.counter + 1);
                let fallback = reply.clone();
                if self.send(json!({"type":"prompt","message":format!("/kota-mcp {}",json!({"schema":1,"requestId":id}))}),
                    Purpose::McpStatus { acknowledged:false, result:None, reply }).is_err() {
                    let _ = fallback.send(Err("mcp_status_unavailable".into()));
                }
            },
            Control::Rpc(request, reply) if request["type"] == "kota_auto_route" => {
                // Only a validated model group travels as extension JSON; no other field passes.
                let group = serde_json::from_value::<super::auto_route::Group>(request["group"].clone()).ok()
                    .filter(|group| group.validate().is_ok());
                let Some(group) = group else {
                    let _ = reply.send(Err("invalid Auto String group".into()));
                    return Ok(());
                };
                self.send(json!({"type":"prompt","message":format!("/kota-auto-route {}", group.launch_json())}),
                    Purpose::External { kind:"kota_auto_route".into(), model:None, reply })?;
            },
            Control::Rpc(request, reply) => {
                let kind = request.get("type").and_then(Value::as_str).unwrap_or_default();
                if !matches!(kind, "get_state" | "abort" | "get_available_models" | "set_model" |
                    "get_available_thinking_levels" | "set_thinking_level" | "get_session_stats") {
                    let _ = reply.send(Err("unsupported Local control command".into()));
                } else {
                    let kind = kind.to_owned();
                    let model = if kind == "set_model" {
                        Some(super::models::ModelSelection { provider_id:request["provider"].as_str().unwrap_or_default().into(), model_id:request["modelId"].as_str().unwrap_or_default().into() })
                    } else if kind == "get_state" { self.expected_model.clone() } else { None };
                    self.send(request, Purpose::External { kind, model, reply })?;
                }
            },
            Control::QueueAction(request, reply) => self.queue_action(request, reply)?,
            Control::Stop(reply) => {
                if self.stopping { let _ = reply.send(Err("Local Stop already pending".into())); return Ok(()); }
                self.clones.stop_all(); // Human Stop; natural agent_settled never enters here.
                self.stopping = true;
                if let Some(queued) = self.reorder.take() { let _ = queued.reply.send(Err("queue_changed".into())); }
                self.fail_cancellations("Stop cancelled the pending removal.");
                // Includes the one write already in flight, ordered before
                // pause on stdin. Rust-held and later inputs are not frozen.
                self.stop_ids = self.credits.keys().filter_map(|id| self.ledger.get(&self.scope.session_id, id)
                    .map(|entry| (id.clone(), entry.sequence))).collect();
                self.send_hold(HoldAction::Pause, reply)?;
            },
            Control::Close(_) => unreachable!(),
            Control::RefreshAuth => self.refresh_auth = true,
            Control::CloneAction(request, reply) => { let result=self.clone_action(request).map_err(|e|e.to_string()); let _=reply.send(result); },
        }
        Ok(())
    }

    fn reordering(&self) -> bool {
        self.reorder.is_some() || self.pending.values().any(|p| matches!(p.purpose, Purpose::Reorder { .. }))
    }

    fn pump(&mut self) -> Result<()> {
        if self.process.is_none() || !self.active { return Ok(()); }
        // The one earlier injection completes before reordering. No later
        // injection or removal can overtake it; all waits stay in this loop.
        if self.active_inject.is_none() {
            if let Some(queued) = self.reorder.take() {
                let request_id = format!("local-{}-{}", self.scope.epoch, self.counter + 1);
                let id = self.send(json!({"type":"prompt","message":format!("/kota-reorder {}", json!({"requestId":request_id,"ids":queued.ids}))}),
                    Purpose::Reorder { ids:queued.ids, changed:false, reply:queued.reply })?;
                self.pending.get_mut(&id).unwrap().deadline = queued.deadline;
            }
        }
        if self.refresh_auth && !self.busy && !self.compacting && !self.stopping
            && !self.pending.values().any(|p| matches!(p.purpose, Purpose::AuthRefresh { .. })) {
            self.refresh_auth = false;
            let operation = uuid::Uuid::new_v4().to_string();
            self.send(json!({"type":"prompt","message":format!("/kota-auth {}",json!({"schema":1,"operationId":operation,"operation":"refresh"}))}),
                Purpose::AuthRefresh { operation, acknowledged:false, result:None })?;
        }
        if !self.stopping && !self.reordering() && !self.pending.values().any(|p| matches!(p.purpose, Purpose::Cancel { .. })) {
            if let Some(queued) = self.cancellations.pop_front() {
                let kota_id = queued.request["kotaId"].as_str().unwrap_or_default().to_owned();
                self.queue_action(queued.request, queued.reply)?;
                for pending in self.pending.values_mut() {
                    if matches!(&pending.purpose, Purpose::Cancel { kota_id: id, .. } if id == &kota_id) {
                        pending.deadline = pending.deadline.min(queued.deadline);
                    }
                }
            }
        }
        if self.entries_again && !self.entries_pending {
            self.entries_again = false;
            self.request_entries(false)?;
        }
        if self.active_inject.is_some() || self.stopping || self.reordering() || self.credits.len() >= MAX_OUTSTANDING { return Ok(()); }
        if let Some(entry) = self.ledger.next_held_ref(&self.scope.session_id) {
            if self.credit_bytes + entry.bytes > MAX_OUTSTANDING_BYTES { return Ok(()); }
            let kota_id = entry.injection.kota_id.clone(); let bytes = entry.bytes;
            let prompt = entry.injection.prompt()?;
            let id = self.send(json!({"type":"prompt", "message":prompt}),
                Purpose::Inject { kota_id: kota_id.clone(), rejected: None })?;
            self.credits.insert(kota_id, bytes); self.credit_bytes += bytes;
            self.active_inject = Some(id);
        }
        Ok(())
    }

    fn pipe(&mut self, event: PipeEvent) -> Result<()> {
        match event {
            PipeEvent::Eof => bail!("Local RPC stdout closed"),
            PipeEvent::Written(id) => {
                if let Some(Pending { purpose: Purpose::Inject { kota_id, rejected: None }, .. }) = self.pending.get(&id) {
                    let id = kota_id.clone(); self.fact(&id, DeliveryState::Sent)?;
                }
            },
            PipeEvent::Record(value, _credit) => {
                if value.get("type").and_then(Value::as_str) == Some("response") { self.response(value)?; }
                else { self.event(value)?; }
            },
        }
        Ok(())
    }

    fn response(&mut self, response: Value) -> Result<()> {
        let Some(id) = response.get("id").and_then(Value::as_str) else { bail!("Local response has no request ID") };
        let Some(pending) = self.pending.remove(id) else { return Ok(()); };
        let success = response.get("success").and_then(Value::as_bool).context("Local response has no success flag")?;
        let data = response.get("data").cloned().unwrap_or(Value::Null);
        let error = || response.get("error").and_then(Value::as_str).unwrap_or("Local RPC command failed").to_string();
        match pending.purpose {
            Purpose::InitialState => {
                if !success { bail!("{}", error()); }
                self.observe_state(&data)?;
                self.initialized = true;
                self.request_entries(true)?;
            },
            Purpose::Entries { recovery, since } => {
                if !success {
                    self.entries_pending = false;
                    if since.is_some() { self.since = None; self.request_entries(recovery)?; }
                    else { bail!("{}", error()); }
                    return Ok(());
                }
                let entries = data.get("entries").and_then(Value::as_array).context("Local entries response is invalid")?.clone();
                let leaf_id = data.get("leafId").and_then(Value::as_str).map(str::to_owned);
                let present: BTreeSet<_> = entries.iter().filter_map(custom_receipt)
                    .filter(|receipt| receipt.details.target_agent_id == self.scope.agent_id)
                    .map(|receipt| receipt.kota_id).collect();
                let recovered_terminal = if recovery {
                    self.ledger.reconcile(&self.scope.session_id, &present)?.into_iter()
                        .filter(|entry| entry.state.terminal()).collect::<Vec<_>>()
                } else {
                    for id in &present { self.fact(id, DeliveryState::Consumed)?; }
                    Vec::new()
                };
                if recovery { self.view.lock().unwrap().refresh_queue(&self.ledger, &self.scope.session_id); }
                let next = entries.last().and_then(|entry| entry.get("id")).and_then(Value::as_str).map(str::to_owned).or(since.clone());
                self.entries_token += 1;
                let after_recovery = if recovery {
                    recovered_terminal.iter().chain(self.ledger.pending(&self.scope.session_id).iter())
                        .map(|entry| self.delivery_notice(entry)).collect()
                } else { Vec::new() };
                self.notices.try_send(Notification {
                    notice: LocalNotice::Entries { scope: self.scope.clone(), entries, leaf_id, since, recovery },
                    acknowledgement: Some((self.entries_token, next)),
                    after_recovery,
                }).map_err(|_| anyhow!("Local projection queue is full or stopped"))?;
            },
            Purpose::Inject { kota_id, rejected } => {
                self.active_inject = None;
                if rejected.is_none() { self.fact(&kota_id, if success { DeliveryState::Accepted } else { DeliveryState::Failed })?; }
                self.status(if self.busy { OnlineState::Busy } else { OnlineState::Ready }, None)?;
            },
            Purpose::External { kind, model, reply } => {
                if success && kind == "set_model" { if let Some(model) = &model {
                    if data["provider"] != model.provider_id || data["id"] != model.model_id {
                        let _ = reply.send(Err("Local Pi selected a different model".into()));
                        bail!("Local Pi selected a different model");
                    }
                    self.expected_model = Some(model.clone());
                    self.shared.lock().unwrap().model = Some(data.clone());
                    self.physical_model=None; self.physical_thinking=None;
                } }
                if success && kind == "get_state" && model == self.expected_model { self.observe_state(&data)?; }
                if success && kind == "set_thinking_level" { self.physical_thinking=None; }
                let _ = reply.send(if success { Ok(data) } else { Err(error()) });
            },
            Purpose::AuthRefresh { operation, result, .. } => {
                if !success || result == Some(false) {
                    crate::kota_debug_log("local-rpc: idle auth refresh failed (payload omitted)");
                    return Ok(());
                }
                if result.is_none() {
                    self.pending.insert(id.into(), Pending { purpose:Purpose::AuthRefresh { operation, acknowledged:true, result }, deadline:pending.deadline });
                }
            },
            Purpose::McpStatus { result, reply, .. } => {
                if !success || data["disposition"] != "handled" {
                    let _ = reply.send(Err("mcp_status_unavailable".into()));
                } else if let Some(result) = result { let _ = reply.send(Ok(result)); }
                else { self.pending.insert(id.into(),Pending { purpose:Purpose::McpStatus { acknowledged:true, result:None, reply }, deadline:pending.deadline }); }
            },
            Purpose::Cancel { kota_id, edit, generation, rejected, reply } => {
                let result = if rejected == Some(Rejection::TooLate) {
                    self.view.lock().unwrap().mark_handed(&kota_id);
                    Ok(json!({"cancelled":false,"reason":"too_late"}))
                } else if rejected.is_some() || !success { Err("Couldn't remove this message.".into()) }
                else {
                    self.fact(&kota_id, DeliveryState::Cleared)?;
                    let token = if edit { Some(uuid::Uuid::new_v4().to_string()) } else { None };
                    let valid = generation == self.edit_generation;
                    if valid { if let Some(token) = &token { self.edit_tokens.insert(kota_id.clone(), token.clone()); } }
                    Ok(json!({"cancelled":true,"editToken":token,"valid":valid}))
                };
                let _ = reply.send(result);
            },
            Purpose::Reorder { ids, changed, reply } => {
                if changed { let _ = reply.send(Err("queue_changed".into())); return Ok(()); }
                let result = (|| -> Result<()> {
                    if !success || data["disposition"] != "handled" { bail!("Local queue reorder failed"); }
                    let before: Vec<_> = ids.iter().map(|id| (id.clone(), self.ledger.get(&self.scope.session_id, id).unwrap().sequence)).collect();
                    self.ledger.reorder(&self.scope.session_id, ids)?;
                    // Stop may already be in flight behind this command. Its
                    // frozen IDs keep following the same attempt, not a slot
                    // reassigned by this ordering fact (Retry still differs).
                    for (id, old) in before {
                        if self.stop_ids.get(&id) == Some(&old) {
                            self.stop_ids.insert(id.clone(), self.ledger.get(&self.scope.session_id, &id).unwrap().sequence);
                        }
                    }
                    self.view.lock().unwrap().refresh_queue(&self.ledger, &self.scope.session_id);
                    Ok(())
                })();
                match result {
                    Ok(()) => { let _ = reply.send(if self.stopping { Err("queue_changed".into()) } else { Ok(json!({"reordered":true})) }); },
                    Err(error) => { let _ = reply.send(Err("queue_reorder_failed".into())); return Err(error); },
                }
            },
            Purpose::StopHold { action, receipt, reply, .. } => {
                if !success || data["disposition"] != "handled" {
                    let error = "Kota Local stopped because the queue handoff failed.";
                    let _ = reply.send(Err(error.into())); bail!("{error}");
                }
                if let Some(ids) = receipt { self.finish_hold(action, ids, reply)?; }
                else { self.pending.insert(id.into(), Pending { purpose:Purpose::StopHold { action, acknowledged:true, receipt:None, reply }, deadline:pending.deadline }); }
            },
            Purpose::StopAbort(reply) => {
                if !success { let error = error(); let _ = reply.send(Err(error.clone())); bail!("{error}"); }
                self.send(json!({"type":"clear_queue"}), Purpose::StopQueue(reply))?;
            },
            Purpose::StopQueue(reply) => {
                if !success { let error = error(); let _ = reply.send(Err(error.clone())); bail!("{error}"); }
                for (id, sequence) in std::mem::take(&mut self.stop_ids) {
                    // A cancel already in flight may finish during Stop. A
                    // subsequent same-ID Retry is a new queue entry, not frozen.
                    if self.ledger.get(&self.scope.session_id, &id).is_some_and(|e| e.sequence == sequence) {
                        self.fact(&id, DeliveryState::Cleared)?;
                    }
                }
                self.send_hold(HoldAction::Resume, reply)?;
            },
        }
        Ok(())
    }

    fn observe_state(&mut self, data: &Value) -> Result<()> {
        if data.get("sessionId").and_then(Value::as_str) != Some(self.scope.session_id.as_str()) {
            bail!("Local Pi loaded a different session ID");
        }
        self.busy = data.get("isStreaming").and_then(Value::as_bool).context("Local state missing isStreaming")?;
        self.compacting = data.get("isCompacting").and_then(Value::as_bool).unwrap_or(false);
        let changing = self.pending.values().any(|p| matches!(&p.purpose,Purpose::External {kind,model:Some(m),..}
            if kind=="set_model" && data["model"]["provider"]==m.provider_id && data["model"]["id"]==m.model_id));
        if !changing && self.expected_model.as_ref().is_some_and(|m| data["model"]["provider"] != m.provider_id || data["model"]["id"] != m.model_id) {
            bail!("Local Pi loaded a different model; open Link to select a signed-in model");
        }
        let mut snapshot = self.shared.lock().unwrap();
        // A state read can precede set_model's acknowledgement. Until that
        // acknowledgement, sign-out's in-use list keeps the confirmed model.
        if !changing {
            snapshot.model = data.get("model").cloned();
            snapshot.thinking_level = data.get("thinkingLevel").and_then(Value::as_str).map(str::to_owned);
        }
        Ok(())
    }

    fn event(&mut self, event: Value) -> Result<()> {
        if event["type"] == "extension_ui_request" && event["title"].as_str().is_some_and(|s|s.starts_with(super::shadow_clone::CODEC)) {
            // Private tool transport, never a session entry or display block.
            if event["method"] == "input" {
                if let Some(id)=event["id"].as_str().filter(|id|valid_id(id,128)) {
                    let result=event["title"].as_str().and_then(|s|s.strip_prefix(super::shadow_clone::CODEC))
                        .and_then(|s|serde_json::from_str::<super::shadow_clone::Request>(s).ok())
                        .ok_or_else(||anyhow!("clone_invalid_request")).and_then(|request|self.clone_action(request));
                    let result=match result { Ok(data)=>json!({"success":true,"data":data}),Err(error)=>json!({"success":false,"error":error.to_string()}) };
                    self.process.as_ref().context("Local process unavailable")?.send(json!({"type":"extension_ui_response","id":id,"value":result.to_string()}))?;
                }
            }
            return Ok(());
        }
        if event["type"] == "extension_ui_request" && event["message"].as_str().is_some_and(|s| s.starts_with(super::mcp::CODEC)) {
            if let Some(receipt) = super::mcp::receipt(&event) {
                let id = &receipt.request_id;
                if self.pending.get(id).is_some_and(|p| matches!(p.purpose,Purpose::McpStatus {..})) {
                    let pending = self.pending.remove(id).unwrap();
                    if let Purpose::McpStatus { acknowledged, reply, .. } = pending.purpose {
                        if acknowledged { let _ = reply.send(Ok(receipt.data())); }
                        else { self.pending.insert(id.clone(),Pending { purpose:Purpose::McpStatus { acknowledged, result:Some(receipt.data()), reply }, deadline:pending.deadline }); }
                    }
                }
            } else { self.fail_all_mcp(); }
            return Ok(()); // Private state never reaches workbench, session or Violet.
        }
        if event["type"] == "extension_error" && event["extensionPath"] == "command:kota-mcp" {
            self.fail_all_mcp(); return Ok(());
        }
        if event["type"] == "extension_ui_request" && event["message"].as_str().is_some_and(|s| s.starts_with(QUEUE_CONTROL_CODEC)) {
            let receipt = match hold_receipt(&event, &self.scope.agent_id) {
                Ok(receipt) => receipt,
                Err(_) if self.pending.values().any(|p| matches!(p.purpose, Purpose::StopHold { .. })) => bail!("Invalid Kota queue control receipt"),
                Err(_) => return Ok(()),
            };
            let matches = self.pending.get(&receipt.request_id).is_some_and(|p|
                matches!(&p.purpose, Purpose::StopHold { action, .. } if *action == receipt.action));
            if matches {
                if receipt.kota_ids.iter().any(|id| self.ledger.get(&self.scope.session_id, id).is_none()) { bail!("Invalid Kota queue control receipt"); }
                let pending = self.pending.remove(&receipt.request_id).unwrap();
                if let Purpose::StopHold { action, acknowledged, reply, .. } = pending.purpose {
                    if acknowledged { self.finish_hold(action, receipt.kota_ids, reply)?; }
                    else { self.pending.insert(receipt.request_id, Pending { purpose:Purpose::StopHold { action, acknowledged, receipt:Some(receipt.kota_ids), reply }, deadline:pending.deadline }); }
                }
            } else { crate::kota_debug_log("local-rpc: uncorrelated queue control receipt (payload omitted)"); }
            return Ok(()); // Control metadata never reaches workbench blocks/history.
        }
        if event["type"] == "extension_ui_request" && event["message"].as_str().is_some_and(|s| s.starts_with(super::protocol::ROUTE_CODEC)) {
            if event["method"] == "notify" {
                let fact = event["message"].as_str().and_then(|s| s.strip_prefix(super::protocol::ROUTE_CODEC))
                    .and_then(|s| serde_json::from_str::<Value>(s).ok()).filter(|v| v["schema"] == 1);
                if let Some(fact) = fact {
                    if fact["event"]=="decision" {
                        if let (Some(provider),Some(model),Some(thinking))=(fact["provider"].as_str(),fact["model"].as_str(),fact["thinkingLevel"].as_str()) {
                            self.physical_model=Some(super::models::ModelSelection {provider_id:provider.into(),model_id:model.into()});
                            self.physical_thinking=Some(thinking.into());
                        }
                    }
                    self.view.lock().unwrap().route_fact(&fact);
                }
            }
            return Ok(()); // Route facts reach only the workbench projection.
        }
        if event["type"] == "extension_ui_request" && event["message"].as_str().is_some_and(|s| s.starts_with(super::auth_protocol::CODEC)) {
            if let Some((id, operation)) = self.pending.iter().find_map(|(id,p)| if let Purpose::AuthRefresh { operation,.. } = &p.purpose { Some((id.clone(),operation.clone())) } else { None }) {
                let clean = match super::auth_protocol::decode(&event,&operation,None,None) {
                    Ok(clean) => clean,
                    Err(_) => { self.pending.remove(&id); crate::kota_debug_log("local-rpc: invalid idle auth refresh result (payload omitted)"); return Ok(()); },
                };
                if clean["event"] == "result" {
                    let succeeded = clean["operation"] == "refresh" && clean["outcome"] == "succeeded";
                    if let Some(pending) = self.pending.get_mut(&id) {
                        if let Purpose::AuthRefresh { acknowledged,result,.. } = &mut pending.purpose {
                            *result = Some(succeeded);
                            if *acknowledged { self.pending.remove(&id); }
                        }
                    }
                    if !succeeded { crate::kota_debug_log("local-rpc: idle auth refresh failed (payload omitted)"); }
                }
            }
            return Ok(()); // Auth metadata is never a history/model event.
        }
        if event["type"] == "extension_error" && event["extensionPath"] == "command:kota-auth" {
            self.pending.retain(|_,p|!matches!(p.purpose,Purpose::AuthRefresh {..}));
            crate::kota_debug_log("local-rpc: idle auth command failed (payload omitted)");
            return Ok(());
        }
        match event.get("type").and_then(Value::as_str).unwrap_or_default() {
            "message_end" => if let Some(receipt) = custom_receipt(&event) {
                if receipt.details.target_agent_id == self.scope.agent_id { self.fact(&receipt.kota_id, DeliveryState::Consumed)?; }
            },
            "agent_start" => self.busy = true,
            "agent_settled" => {
                self.busy = false;
                self.request_entries(false)?;
            },
            "turn_end" => self.request_entries(false)?,
            "compaction_start" => self.compacting = true,
            "compaction_end" => { self.compacting = false; self.request_entries(false)?; },
            "extension_error" => {
                let active = self.active_inject.clone();
                if let Some(Pending { purpose: Purpose::Inject { kota_id, rejected }, .. }) = active.as_ref().and_then(|id| self.pending.get_mut(id)) {
                    if rejected.is_none() && rejection(&event, kota_id).is_some() {
                        *rejected = Some(Rejection::Failed);
                        let id = kota_id.clone(); self.fact(&id, DeliveryState::Failed)?;
                    }
                }
                if let Some(error) = command_error(&event, "kota-cancel") {
                    // There is only one cancellation in flight. An ID-bearing
                    // error for another command remains uncorrelated; an
                    // unexpected handler error must never become a success.
                    let identity = error.split_once(" kotaId=").and_then(|(_, rest)| rest.split_whitespace().next())
                        .and_then(|id| urlencoding::decode(id).ok()).map(|id| id.into_owned());
                    let mut matched = false;
                    for request in self.pending.values_mut() {
                        if let Purpose::Cancel { kota_id, rejected, .. } = &mut request.purpose {
                            if identity.as_deref().is_some_and(|id| id != "unknown" && id != kota_id) { continue; }
                            matched = true;
                            if rejected.is_none() { *rejected = Some(command_rejection(&event, "kota-cancel", kota_id).unwrap_or(Rejection::Failed)); }
                        }
                    }
                    if !matched { crate::kota_debug_log("local-rpc: uncorrelated cancellation error (payload omitted)"); }
                }
                if let Some(error) = command_error(&event, "kota-reorder") {
                    if let Some((id, request)) = self.pending.iter_mut().find(|(_, p)| matches!(p.purpose, Purpose::Reorder { .. })) {
                        if error == format!("kota-reorder-changed requestId={id}") {
                            if let Purpose::Reorder { changed, .. } = &mut request.purpose { *changed = true; }
                        } else if error.starts_with("kota-reorder-changed requestId=") {
                            crate::kota_debug_log("local-rpc: uncorrelated reorder error (payload omitted)");
                        } else { bail!("Local queue reorder failed"); }
                    } else { crate::kota_debug_log("local-rpc: uncorrelated reorder error (payload omitted)"); }
                    return Ok(());
                }
                for action in [HoldAction::Pause, HoldAction::Resume] {
                    if command_error(&event, action.command()).is_some() {
                        if self.pending.values().any(|p| matches!(&p.purpose, Purpose::StopHold { action: pending, .. } if *pending == action)) {
                            bail!("Kota Local stopped because the queue handoff failed.");
                        }
                        crate::kota_debug_log("local-rpc: uncorrelated queue control error (payload omitted)");
                        return Ok(());
                    }
                }
                if command_error(&event, "kota-clear-held").is_some() {
                    crate::kota_debug_log("local-rpc: uncorrelated queue-clear error (payload omitted)");
                }
            },
            _ => {},
        }
        if matches!(event["type"].as_str(),Some("message_start"|"message_end")) && event["message"]["role"]=="assistant" {
            let message=&event["message"];
            if let (Some(provider),Some(model))=(message["provider"].as_str(),message["model"].as_str()) {
                if provider!=super::auto_route::PROVIDER {
                    self.physical_model=Some(super::models::ModelSelection {provider_id:provider.into(),model_id:model.into()});
                    if let Some(level)=message["thinkingLevel"].as_str() { self.physical_thinking=Some(level.into()); }
                }
            }
        }
        self.view.lock().unwrap().event(&event);
        self.notify(LocalNotice::Event { scope: self.scope.clone(), event })?;
        self.status(if !self.initialized { OnlineState::Starting } else if self.busy { OnlineState::Busy } else { OnlineState::Ready }, None)
    }

    fn queue_action(&mut self, request: Value, reply: Reply<Value>) -> Result<()> {
        let id = request["kotaId"].as_str().unwrap_or_default().to_owned();
        let kind = request["type"].as_str().unwrap_or_default();
        let token_valid = request["editToken"].as_str().is_some_and(|token| self.edit_tokens.get(&id).is_some_and(|current| current == token));
        let result = match kind {
            "queue_reorder" => {
                let ids = serde_json::from_value::<Vec<String>>(request["ids"].clone()).ok();
                if self.stopping || self.reordering() || !self.cancellations.is_empty()
                    || self.pending.values().any(|p| matches!(p.purpose, Purpose::Cancel { .. }))
                    || ids.as_ref().is_none_or(|ids| !(2..=MAX_OUTSTANDING).contains(&ids.len())
                        || ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
                        || ids.iter().any(|id| self.ledger.get(&self.scope.session_id, id).is_none_or(|e| !e.state.queued()))) {
                    Err("queue_changed".into())
                } else {
                    self.reorder = Some(QueuedReorder { ids:ids.unwrap(), reply, deadline:Instant::now() + COMMAND_TIMEOUT });
                    return Ok(());
                }
            },
            "queue_edit_valid" => Ok(json!({"valid":token_valid})),
            "queue_release" => {
                if token_valid { self.edit_tokens.remove(&id); }
                Ok(json!({"released":token_valid}))
            },
            "queue_requeue" => {
                if !token_valid { Err("This edit no longer belongs to the current session.".into()) }
                else {
                    let entry = self.ledger.retry(&self.scope.session_id, &id)?;
                    if let Some(entry) = entry { self.edit_tokens.remove(&id); self.delivery(&entry)?; Ok(json!({"queued":true})) }
                    else { Err("The original message can no longer be queued.".into()) }
                }
            },
            "queue_cancel" => {
                let edit = request["edit"].as_bool().unwrap_or(false);
                let entry = self.ledger.get(&self.scope.session_id, &id);
                if self.stopping { Err("Local Stop already pending".into()) }
                else if entry.is_none_or(|e| !e.state.queued()) { Ok(json!({"cancelled":false,"reason":"too_late"})) }
                else if self.reordering() || self.pending.values().any(|p| matches!(&p.purpose, Purpose::Cancel { .. })) {
                    if self.cancellations.len() >= MAX_OUTSTANDING { Err("Too many pending message removals.".into()) }
                    else {
                        self.cancellations.push_back(QueuedCancel { request, reply, deadline: Instant::now() + COMMAND_TIMEOUT });
                        return Ok(());
                    }
                } else {
                    let writing = self.active_inject.as_ref().and_then(|key| self.pending.get(key))
                        .is_some_and(|p| matches!(&p.purpose, Purpose::Inject { kota_id, .. } if *kota_id == id));
                    if entry.unwrap().state == DeliveryState::Held && !writing {
                        self.fact(&id, DeliveryState::Cleared)?;
                        let token = if edit { let token = uuid::Uuid::new_v4().to_string(); self.edit_tokens.insert(id.clone(), token.clone()); Some(token) } else { None };
                        Ok(json!({"cancelled":true,"editToken":token,"valid":true}))
                    } else {
                        self.send(json!({"type":"prompt","message":format!("/kota-cancel {id}")}),
                            Purpose::Cancel { kota_id:id, edit, generation:self.edit_generation, rejected:None, reply })?;
                        return Ok(());
                    }
                }
            },
            _ => Err("Unsupported Local queue action.".into()),
        };
        let _ = reply.send(result);
        Ok(())
    }

    fn stop(&mut self, error: bool) -> Result<()> {
        self.clones.stop_all();
        if let Some(process) = self.process.as_mut() { process.stop()?; }
        self.process = None;
        self.edit_generation += 1; self.edit_tokens.clear();
        self.fail_cancellations("This session stopped before the message could be removed.");
        if let Some(queued) = self.reorder.take() { let _ = queued.reply.send(Err("queue_reorder_failed".into())); }
        let ids: Vec<_> = self.ledger.pending(&self.scope.session_id).into_iter().filter(|e| e.state.queued()).map(|e| e.injection.kota_id).collect();
        for id in ids { self.fact(&id, DeliveryState::Abandoned)?; }
        self.view.lock().unwrap().clear_run();
        self.shared.lock().unwrap().child_pid = None;
        if !error { let _ = self.status(OnlineState::Offline, None); }
        Ok(())
    }

    fn fail_cancellations(&mut self, error: &str) {
        for queued in self.cancellations.drain(..) { let _ = queued.reply.send(Err(error.into())); }
    }

    fn failure(&mut self, error: String) {
        let stop_error = self.stop(true).err();
        let error = match stop_error { Some(stop) => format!("{error}; Local cleanup failed: {stop}"), None => error };
        let _ = self.status(OnlineState::Error, Some(error.clone()));
        if let Some(ready) = self.ready.take() { let _ = ready.send(Err(error.clone())); }
        for (_, request) in std::mem::take(&mut self.pending) {
            match request.purpose {
                Purpose::External { reply, .. } | Purpose::Cancel { reply, .. } | Purpose::Reorder { reply, .. } | Purpose::StopHold { reply, .. } | Purpose::StopAbort(reply) | Purpose::StopQueue(reply) => { let _ = reply.send(Err(error.clone())); },
                Purpose::McpStatus { reply, .. } => { let _ = reply.send(Err("mcp_status_unavailable".into())); },
                _ => {},
            }
        }
        self.active_inject = None;
        self.active = false;
        // Old waiting inputs are abandoned; neither close nor recovery replays them.
    }

    fn fail_mcp(&mut self, id: &str) {
        if let Some(Pending { purpose:Purpose::McpStatus { reply,.. }, .. }) = self.pending.remove(id) {
            let _ = reply.send(Err("mcp_status_unavailable".into()));
        }
    }
    fn fail_all_mcp(&mut self) {
        let ids: Vec<_> = self.pending.iter().filter(|(_,p)| matches!(p.purpose,Purpose::McpStatus {..})).map(|(id,_)| id.clone()).collect();
        for id in ids { self.fail_mcp(&id); }
    }

    fn clone_action(&mut self, request: super::shadow_clone::Request) -> Result<Value> {
        let snapshot=self.shared.lock().unwrap().clone();
        let model=self.physical_model.clone().or_else(|| snapshot.model.as_ref().and_then(|model| {
            Some(super::models::ModelSelection {provider_id:model["provider"].as_str()?.into(),model_id:model["id"].as_str()?.into()})
        })).filter(|model|model.provider_id!=super::auto_route::PROVIDER);
        let thinking=self.physical_thinking.clone().or(snapshot.thinking_level).unwrap_or_else(||"off".into());
        self.clones.action(request,model,&thinking)
    }
    fn clone_reports(&mut self, reports: Vec<(u64, String)>) {
        for (clone_id, report) in reports {
            let result = (|| -> Result<()> {
                let message=format!("clone-report-{}",uuid::Uuid::new_v4());
                let input=Injection::text(&message,&self.scope.agent_id,chrono::Utc::now().to_rfc3339(),true,
                    super::shadow_clone::REPORT_TYPE,report,None);
                input.validate(&self.scope.agent_id)?;
                let id=input.kota_id.clone();
                if self.ledger.hold(&self.scope.session_id,input)? {
                    let entry=self.ledger.get(&self.scope.session_id,&id).context("clone_report_missing")?.clone();
                    self.delivery(&entry)?;
                }
                Ok(())
            })();
            if let Err(error) = result {
                self.clones.fail(clone_id, &error.to_string());
            }
        }
    }
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
