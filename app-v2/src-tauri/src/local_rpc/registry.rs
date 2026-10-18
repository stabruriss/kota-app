//! This is a child module of pty::agent, so the new carrier reuses its private
//! lease/replacement boundary without making terminal internals public.
use super::*;
use crate::local_rpc::{self, Injection, LocalRuntime, LocalScope, OnlineState};
use serde_json::{json, Value};
use tauri::Emitter;

#[derive(Clone)]
pub(super) struct LocalAgent {
    pub runtime: LocalRuntime,
    request: AgentSpawnRequest,
    lease_path: PathBuf,
    child_pid: Option<u32>,
    backend: Option<local_rpc::auth::Backend>,
}

impl LocalAgent {
    pub(super) fn route(&self) -> AgentRoute {
        let root = format!("local-rpc://agent/{}", self.request.agent_id);
        AgentRoute { agent_id: self.request.agent_id.clone(), output_event: format!("{root}/notice"),
            exit_event: format!("{root}/exit"), status_event: format!("{root}/notice"), work_event: format!("{root}/notice") }
    }
    fn running(&self) -> bool {
        let state = self.runtime.snapshot();
        state.child_pid.is_some() && !matches!(state.state, OnlineState::Offline | OnlineState::Error)
    }
    fn stop(&self) -> Result<()> {
        self.runtime.close()?;
        remove_agent_session_lease(&self.lease_path, std::process::id(), self.child_pid);
        Ok(())
    }
}

/// Two carriers share replacement/lease ownership, but never share an input or
/// terminal-output path. The Terminal arm is the existing PTY object unchanged.
#[derive(Clone)]
pub(crate) enum Runtime { Terminal(AgentTerminalPty), LocalRpc(LocalRuntime) }

/// The one input-routing rule shared by bus and composer. Saved changes only
/// choose a carrier when no registered child is alive.
fn delivery_cli(live: Option<AgentCli>, saved: Option<AgentCli>) -> Option<AgentCli> { live.or(saved) }

pub(super) fn ensure_expected_cli(actual: AgentCli, expected: Option<AgentCli>) -> Result<()> {
    if expected.is_some_and(|expected| expected != actual) {
        return Err(anyhow!("Local input target changed before startup"));
    }
    Ok(())
}

impl AgentRegistry {
    pub(super) fn start_local(&self, app: &AppHandle, request: AgentSpawnRequest) -> Result<AgentRoute> {
        let target = request.agent_id.clone(); let app_notify = app.clone();
        let begun = local_rpc::startup::begin(request.project_root.clone(), target.clone(), None, Arc::new(move |view| {
            let _ = app_notify.emit(&format!("local-rpc://agent/{target}/startup"), view);
        }))?;
        let attempt = local_rpc::startup::get(&request.project_root, &request.agent_id, &begun.view.attempt_id)?;
        if begun.reused {
            attempt.join()?;
            return self.local(&request.agent_id).filter(|local| local.running()).map(|local| local.route())
                .context("Local startup completed without a current owner");
        }
        self.start_local_workbench(app, request, attempt)
    }
    pub(crate) fn start_local_workbench(&self, app: &AppHandle, request: AgentSpawnRequest, attempt: Arc<local_rpc::startup::Attempt>) -> Result<AgentRoute> {
        use local_rpc::{configuration::SavedModel, startup::{self, Selection}, error::LocalError};
        let execution = attempt.execution()?;
        let result = (|| -> Result<AgentRoute> {
            let query = attempt.query();
            let cwd = local_rpc::configuration::agent_cwd(app, Path::new(&request.project_root), &request.agent_id)?;
            if request.cli != AgentCli::KotaLocal { return Err(LocalError::new("local_provider_changed").into()); }
            let replacement = self.replacement(&request.agent_id);
            if !request.fresh_session {
                if let Some(local) = self.local(&request.agent_id).filter(|local| local.running()) { return Ok(local.route()); }
            }
            let epoch = replacement.epoch.fetch_add(1, Ordering::SeqCst) + 1;
            query.stage("configuration_read")?;
            let metadata = crate::shell_switch::metadata_lock(&cwd);
            let saved = { let _guard = startup::lock(&metadata, &query, "configuration_read")?; SavedModel::read(&cwd)? };
            let requested = attempt.view().selection;
            let selection = match &requested {
                Some(selection) => selection.clone(),
                None => Selection { model:local_rpc::models::selected_model(saved.model.as_deref())?, thinking_level:saved.thinking.clone(),
                    auto_route:saved.auto_route.clone() },
            };
            attempt.selection(selection.clone());
            let backend = local_rpc::auth::Backend::for_app(app)?;
            // A chosen pair is confirmed by Pi mode by mode; the saved one only passes the release condition.
            let resolved = match (&requested, &selection.auto_route) {
                (Some(_), Some(pair)) if local_rpc::auto_route::is_auto(&selection.model) => {
                    query.stage("credential_status")?;
                    local_rpc::auto_route::choose(&backend.owned, pair, saved.auto_route.as_ref(),
                        |model, thinking| local_rpc::models::resolve_model_observed(&backend, model, thinking, &query.renewed()))?
                },
                _ => local_rpc::models::resolve_selection(&backend, &selection.model, &selection.thinking_level,
                    selection.auto_route.as_ref(), &query)?,
            };
            let query = query.renewed();
            attempt.selection(Selection { model:selection.model, thinking_level:resolved.thinking_level.clone(), auto_route:resolved.auto_route.clone() });
            if requested.is_some() {
                query.stage("configuration_save")?;
                let _serial = startup::lock(&replacement.serial, &query, "configuration_save")?;
                if replacement.epoch.load(Ordering::SeqCst) != epoch { return Err(LocalError::new("startup_superseded").into()); }
                let _guard = startup::lock(&metadata, &query, "configuration_save")?;
                attempt.save(|| saved.save(&resolved.model_arg, &resolved.thinking_level, resolved.auto_route.as_ref()).map(|_| ()))?;
            }
            query.stage("launch_configuration")?;
            let mut req = request;
            // The serial lock pins metadata and lease ownership, never a Pi query.
            let (launch, lease_path, foreign, previous) = {
                let _serial = startup::lock(&replacement.serial, &query, "launch_configuration")?;
                if replacement.epoch.load(Ordering::SeqCst) != epoch { return Err(LocalError::new("startup_superseded").into()); }
                attempt.check()?;
                let previous = self.lookup(&req.agent_id).ok();
                let live = previous.as_ref().filter(|p| p.is_running() && !p.submit_child_is_dead()).map(|p| p.inner.cli)
                    .or_else(|| live_provider_for(Path::new(&req.project_root), &req.agent_id, &cwd));
                let lease_path = agent_session_lease_path(&req);
                let foreign = active_foreign_agent_session_lease(&lease_path);
                if let Some(lease) = &foreign { if !req.takeover { return Err(lease_conflict_error(lease)); } }
                let launch = crate::shell_switch::Launch::prepare(&mut req, live).map_err(|_| LocalError::new("local_launch_preparation_failed"))?;
                ensure_expected_cli(req.cli, Some(AgentCli::KotaLocal))?;
                (launch, lease_path, foreign, previous)
            };
            let prepared = startup::Prepared { attempt:attempt.clone(), model:resolved, backend };
            self.spawn_local_prepared(app, req, launch, lease_path, foreign, previous, epoch, &prepared)
        })();
        // The existing takeover confirmation is a user decision, not a failure.
        if result.as_ref().err().is_some_and(|e| e.to_string().starts_with("KOTA_AGENT_SESSION_LEASE_CONFLICT:")) {
            execution.release_for_takeover();
        } else {
            attempt.finish(result.as_ref().map(|_| ()).map_err(|e| e.downcast_ref::<LocalError>().cloned()
                .unwrap_or_else(|| LocalError::new("local_start_failed"))));
        }
        result
    }
    pub(super) fn local(&self, agent: &str) -> Option<LocalAgent> {
        self.locals.lock().expect("Local registry poisoned").get(agent).cloned()
    }

    pub(crate) fn runtime(&self, agent: &str) -> Option<Runtime> {
        if let Some(local) = self.local(agent) { return Some(Runtime::LocalRpc(local.runtime)); }
        self.lookup(agent).ok().map(Runtime::Terminal)
    }

    pub(crate) fn is_local(&self, agent: &str) -> bool { self.local(agent).is_some() }

    pub(crate) fn local_target(&self, agent: &str, fallback: Option<&AgentSpawnRequest>) -> bool {
        delivery_cli(self.running_cli(agent), fallback.map(|r| r.cli)) == Some(AgentCli::KotaLocal)
    }

    fn running_cli(&self, agent: &str) -> Option<AgentCli> {
        match self.runtime(agent)? {
            Runtime::LocalRpc(runtime) => {
                let state = runtime.snapshot();
                (state.child_pid.is_some() && !matches!(state.state, OnlineState::Offline | OnlineState::Error))
                    .then_some(AgentCli::KotaLocal)
            },
            Runtime::Terminal(terminal) => (terminal.is_running() && !terminal.submit_child_is_dead()).then_some(terminal.inner.cli),
        }
    }

    pub(crate) fn local_input_targets(&self, root: &Path, agents: &[String]) -> Result<Vec<String>> {
        if !root.is_absolute() || agents.len() > 8 { return Err(anyhow!("invalid Local routing scope")); }
        let mut targets = Vec::new();
        for agent in agents {
            if agent.is_empty() || !agent.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')) {
                return Err(anyhow!("invalid Local routing identity"));
            }
            let live = self.running_cli(agent);
            let saved = if live.is_some() { None } else {
                let cwd = root.join(".agent-workspaces").join(agent);
                let yaml = crate::read_yaml_mapping(&cwd.join("agent.yaml")).map_err(|e| anyhow!(e))?;
                let shell = crate::parse_shell_yaml(&std::fs::read_to_string(cwd.join("SHELL.yaml"))?).map_err(|e| anyhow!(e))?;
                Some(crate::cli_from_project_agent_files(&cwd, &shell, &yaml).map_err(|e| anyhow!(e))?)
            };
            if delivery_cli(live, saved) == Some(AgentCli::KotaLocal) { targets.push(agent.clone()); }
        }
        Ok(targets)
    }

    pub(super) fn stop_local_for_terminal(&self, agent: &str) -> Result<()> {
        if let Some(local) = self.local(agent) {
            local.stop()?;
            self.locals.lock().unwrap().remove(agent);
        }
        Ok(())
    }

    pub(super) fn close_local(&self, agent: &str) -> Result<()> {
        let replacement = self.replacement(agent);
        replacement.epoch.fetch_add(1, Ordering::SeqCst);
        let _guard = replacement.serial.lock().unwrap();
        self.stop_local_for_terminal(agent)
    }

    pub(super) fn spawn_local_prepared(&self, app: &AppHandle, mut req: AgentSpawnRequest,
        mut launch: Option<crate::shell_switch::Launch>, lease_path: PathBuf,
        foreign: Option<AgentSessionLease>, previous: Option<AgentTerminalPty>, epoch: u64,
        prepared: &local_rpc::startup::Prepared) -> Result<AgentRoute> {
        let agent = req.agent_id.clone();
        let previous_local = self.local(&agent);
        if previous_local.as_ref().is_some_and(|old| old.request.project_root != req.project_root) {
            return Err(anyhow!("Local launch belongs to another project"));
        }
        let sink = local_rpc::sink(app)?;
        let home = dirs::home_dir().context("Local launch requires a home directory")?;
        let backend = prepared.backend.clone();
        let query = prepared.attempt.query();
        backend.owned.initialize_observed(&query)?;
        let selected = {
            let argument = option(&req.args, "--model");
            let model = local_rpc::models::selected_model(argument.as_deref())?;
            if model.provider_id != prepared.model.provider_id || model.model_id != prepared.model.model_id {
                return Err(local_rpc::error::LocalError::new("local_config_changed").into());
            }
            local_rpc::models::gate_resolved(&backend, &prepared.model, &query)?;
            prepared.model.clone()
        };
        let model = local_rpc::models::ModelSelection { provider_id:selected.provider_id.clone(), model_id:selected.model_id.clone() };
        let extension = &backend.bundled.extension;
        query.stage("prepare_adapter")?;
        let replacement = self.replacement(&agent);
        {
            let _serial = local_rpc::startup::lock(&replacement.serial, &query, "prepare_adapter")?;
            if replacement.epoch.load(Ordering::SeqCst) != epoch { return Err(local_rpc::error::LocalError::new("startup_superseded").into()); }
            if let Some(launch) = launch.as_mut() { launch.prepare_adapter().map_err(|e| anyhow!(e))?; }
            else { crate::sync_adapter_before_launch(Path::new(&req.project_root), &agent, req.adapter_path.as_deref().map(Path::new)).map_err(|e| anyhow!(e))?; }
        }
        if req.session_id.is_none() { req.session_id = Some(uuid::Uuid::new_v4().to_string()); }
        let session = req.session_id.clone().unwrap();
        let project = req.project_id.clone().or_else(|| Path::new(&req.project_root).file_name().map(|s| s.to_string_lossy().into_owned()))
            .context("Local project identity missing")?;
        for component in [&project, &agent] {
            if component.is_empty() || component == "." || component == ".." || !component.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')) {
                return Err(anyhow!("invalid Local storage identity"));
            }
        }
        let storage = home.join("Kota/kota-local").join(&project).join(&agent);
        let sessions = storage.join("sessions");
        local_rpc::launch::storage_directory(&storage)?;
        local_rpc::launch::storage_directory(&sessions)?;
        let mut command = Command::new(&backend.bundled.executable);
        // The gated pair reaches Pi as one JSON argv element.
        let route = selected.auto_route.as_ref().filter(|_| local_rpc::auto_route::is_auto(&model)).map(local_rpc::auto_route::Group::launch_json);
        command.current_dir(&req.cwd).args(local_rpc::launch::arguments(Path::new(&req.cwd), extension,
            &sessions, &session, Some(&selected.model_arg), Some(&selected.thinking_level), route.as_deref()));
        local_environment(&mut command, &req, &home);
        local_rpc::environment::prepend_bundled_pi(&mut command, &backend.bundled.executable)?;
        command.env("PI_CODING_AGENT_DIR", &backend.owned.agent);
        query.check("prepare_adapter")?;
        if replacement.epoch.load(Ordering::SeqCst) != epoch { return Err(anyhow!("agent startup cancelled or superseded")); }
        if let Some(foreign) = foreign {
            terminate_foreign_agent_session(&foreign);
            let deadline = Instant::now() + Duration::from_secs(5);
            while foreign.child_pid.is_some_and(process_is_alive) {
                if Instant::now() >= deadline { return Err(anyhow!("previous Local session process has not exited")); }
                thread::sleep(Duration::from_millis(10));
            }
        }
        if let Some(previous) = previous { previous.stop_for_replacement()?; }
        if let Some(previous) = &previous_local { previous.stop()?; }
        query.stage("session_ready")?;
        let fence = replacement.clone();
        let attempt = prepared.attempt.clone();
        let runtime = match LocalRuntime::start(local_rpc::Config {
            scope: LocalScope { project_root: PathBuf::from(&req.project_root), agent_id: agent.clone(), session_id: session.clone(), epoch },
            ledger: storage.join("ledger.jsonl"), command,
            expected_model:Some(model),
            current: Arc::new(move || fence.epoch.load(Ordering::SeqCst) == epoch && attempt.check().is_ok()),
        }, sink) {
            Ok(runtime) => runtime,
            Err(error) => {
                let _serial = replacement.serial.lock().unwrap();
                if replacement.epoch.load(Ordering::SeqCst) == epoch {
                    if let Some(launch) = launch.as_mut() { launch.rollback().map_err(|e| anyhow!("{error}; Local adapter rollback failed: {e}"))?; }
                }
                return Err(error);
            },
        };
        let local = LocalAgent { child_pid: runtime.snapshot().child_pid, runtime,
            request: req.clone(), lease_path: lease_path.clone(), backend:Some(backend) };
        let result = (|| -> Result<AgentRoute> {
            prepared.attempt.check()?;
            let _serial = local_rpc::startup::lock(&replacement.serial, &prepared.attempt.query(), "session_commit")?;
            if replacement.epoch.load(Ordering::SeqCst) != epoch { return Err(anyhow!("agent startup cancelled or superseded")); }
            self.locals.lock().unwrap().insert(agent.clone(), local.clone());
            let now = Utc::now().to_rfc3339();
            write_agent_session_lease(&lease_path, &AgentSessionLease {
                version: 1, agent_id: agent.clone(), cli: AgentCli::KotaLocal, cwd: req.cwd.clone(),
                project_root: req.project_root.clone(), owner_pid: std::process::id(), child_pid: local.child_pid,
                session_id: Some(session), created_at: now.clone(), updated_at: now,
            })?;
            if let Some(launch) = launch.as_mut() { launch.commit(req.session_id.as_deref()).map_err(|e| anyhow!(e))?; }
            // Only the successfully installed Local owner replaces the old map entry.
            self.ptys.lock().unwrap().remove(&agent);
            Ok(local.route())
        })();
        // Waiting for the owner ACK must never hold replacement.serial.
        let result = result.and_then(|route| {
            prepared.attempt.step("session_commit");
            prepared.attempt.check()?;
            local.runtime.activate(true)?;
            prepared.attempt.check()?;
            Ok(route)
        });
        if let Err(error) = result {
            let cleanup = local.stop().err();
            let _serial = replacement.serial.lock().unwrap();
            let rollback = if replacement.epoch.load(Ordering::SeqCst) == epoch { launch.as_mut().and_then(|launch| launch.rollback().err()) } else { None };
            if cleanup.is_none() {
                let mut owners = self.locals.lock().unwrap();
                if owners.get(&agent).is_some_and(|owner| owner.child_pid == local.child_pid) { owners.remove(&agent); }
            }
            return Err(anyhow!("{error}{}{}", cleanup.map(|e| format!("; Local cleanup failed: {e}")).unwrap_or_default(),
                rollback.map(|e| format!("; adapter rollback failed: {e}")).unwrap_or_default()));
        }
        result
    }

    pub(crate) fn local_snapshot(&self, agent: &str) -> Result<local_rpc::Snapshot> {
        Ok(self.local(agent).context("unknown Local agent")?.runtime.snapshot())
    }

    pub(crate) fn local_workbench(&self, root: &Path, agent: &str, since: Option<&local_rpc::WorkbenchCursor>) -> Result<Option<local_rpc::WorkbenchView>> {
        let Some(local) = self.local(agent).filter(|l| l.runtime.is_current()) else { return Ok(None); };
        if local.runtime.snapshot().scope.project_root != root { return Err(anyhow!("Local view belongs to another project")); }
        Ok(Some(local.runtime.workbench(since)))
    }

    pub(crate) fn local_clones(&self, root: &Path, agent: &str, monitor: Option<u64>) -> Result<Option<Value>> {
        let Some(local)=self.local(agent).filter(|local|local.runtime.is_current()) else { return Ok(None); };
        let scope=local.runtime.snapshot().scope;
        if scope.project_root!=root { return Err(anyhow!("Local view belongs to another project")); }
        let mut view=local.runtime.clones(monitor); view["scope"]=json!(scope); Ok(Some(view))
    }

    /// A visible card can only control the exact owner it read. In particular,
    /// this never calls local_ready and cannot wake or control a replacement.
    pub(crate) fn local_workbench_control(&self, scope: &LocalScope, request: Value) -> Result<Value> {
        let local = self.local(&scope.agent_id).context("Local session is offline")?;
        if !local.runtime.is_current() || local.runtime.snapshot().scope != *scope {
            return Err(anyhow!("Local session changed; reopen the workbench."));
        }
        match request.get("type").and_then(Value::as_str) {
            Some("clone_stop" | "clone_destroy") => {
                if request.as_object().is_none_or(|request|request.len()!=2) { return Err(anyhow!("clone_invalid_request")); }
                let clone_id=request["cloneId"].as_u64().context("clone_invalid_request")?;
                let action=if request["type"]=="clone_stop" { local_rpc::shadow_clone::Request::Stop {clone_id} }
                    else { local_rpc::shadow_clone::Request::Destroy {clone_id} };
                local.runtime.clone_control(action)
            },
            Some("queue_text") => local.runtime.queue_text(request["kotaId"].as_str().context("queued message missing")?.into()),
            Some("queue_cancel" | "queue_edit_valid" | "queue_requeue" | "queue_release" | "queue_reorder") => local.runtime.queue_action(request),
            Some("set_model" | "set_thinking_level") => configure_model(&local, request),
            Some("get_available_models") => gated_models(&local),
            Some("get_state" | "get_available_thinking_levels" | "get_session_stats" | "get_mcp_status" | "abort") => local.runtime.rpc(request),
            _ => Err(anyhow!("unsupported workbench command")),
        }
    }

    /// Detail may apply to an existing owner, but must never start one or follow a replacement.
    pub(crate) fn local_detail_control(&self, root: &Path, agent: &str, request: Value) -> Result<Option<Value>> {
        let Some(local) = self.local(agent) else { return Ok(None); };
        if !local.runtime.is_current() || local.runtime.snapshot().scope.project_root != root {
            return Err(local_rpc::error::LocalError::new("local_session_changed").into());
        }
        if !local.running() { return Ok(None); }
        configure_model(&local, request).map(Some)
    }

    fn local_ready(&self, app: &AppHandle, agent: &str,
        fallback: Option<AgentSpawnRequest>) -> Result<LocalAgent> {
        if let Some(local) = self.local(agent) {
            if local.running() { return Ok(local); }
            let mut req = local.request;
            req.fresh_session = false;
            // Consent applies to the explicit launch only. A later cold wake
            // must surface a new foreign lease through the normal confirmation.
            req.takeover = false;
            self.start_local(app, req)?;
        } else {
            let req = fallback.context("Local launch request is unavailable")?;
            if req.cli != AgentCli::KotaLocal { return Err(anyhow!("Local launch request targets a terminal")); }
            self.start_local(app, req)?;
        }
        self.local(agent).context("agent was replaced with another provider")
    }

    pub(crate) fn submit_local(&self, app: &AppHandle, agent: &str, mut input: Injection,
        source: crate::shell_switch::PromptSource, retry: bool, fallback: Option<AgentSpawnRequest>,
        soul: Option<crate::soul_push::input::Input>) -> Result<()> {
        let replacement = self.replacement(agent);
        let local = self.local_ready(app, agent, fallback)?;
        // Jev runs before the serial lock, so Close and startup never wait on it.
        // An input the runtime already has keeps its first content.
        let soul = match soul.zip(app.try_state::<Arc<crate::soul_push::service::Service>>()) {
            Some((soul, service)) if local.runtime.input(input.kota_id.clone())?.is_none() => {
                let scope = local.runtime.snapshot().scope;
                service.prepare(crate::soul_push::input::Scope {
                    agent_cwd: scope.project_root.join(".agent-workspaces").join(agent), project_id: local.request.project_id.clone(),
                    agent_id: agent.into(), key: format!("local:{}:{}", scope.session_id, scope.epoch), project_root: scope.project_root,
                }, &soul).map(|delivery| (service.inner().clone(), delivery))
            }
            _ => None,
        };
        let _guard = replacement.serial.lock().unwrap();
        if !local.runtime.is_current() { return Err(anyhow!("Local input target changed before delivery")); }
        input.validate(agent)?;
        if local.runtime.input(input.kota_id.clone())?.is_some() {
            if retry { local.runtime.retry(input.kota_id)?; }
            return Ok(());
        }
        let cwd = Path::new(&local.request.project_root).join(".agent-workspaces").join(agent);
        let text = input.content.as_str().context("Local production input must be prepared text")?;
        let text = match &soul { Some((_, delivery)) => format!("{}\n{text}", delivery.choice.envelope), None => text.into() };
        let (text, handoff) = crate::shell_switch::compose(&cwd, AgentCli::KotaLocal, text, source).map_err(|e| anyhow!(e))?;
        input.content = Value::String(text);
        if let Some((service, delivery)) = soul { service.hold(input.kota_id.clone(), delivery); }
        local.runtime.submit(input)?;
        if let Some(handoff) = handoff {
            if crate::shell_switch::submitted(&cwd, &handoff).is_err() {
                crate::kota_debug_log("local-rpc: submitted handoff cleanup failed (payload omitted)");
            }
        }
        Ok(())
    }

    pub(crate) fn local_control(&self, app: &AppHandle, agent: &str, request: Value) -> Result<Value> {
        if request["type"] == "set_model" && request.get("projectRoot").is_some() {
            let root = request["projectRoot"].as_str().ok_or_else(|| local_rpc::error::LocalError::new("invalid_request"))?;
            let cwd = local_rpc::configuration::agent_cwd(app, Path::new(root), agent)?;
            let backend = local_rpc::auth::Backend::for_app(app)?;
            return self.configure_offline_model(&backend, agent, &cwd, &request);
        }
        let local = self.local_ready(app, agent, None)?;
        if matches!(request.get("type").and_then(Value::as_str), Some("set_model" | "set_thinking_level")) {
            return configure_model(&local, request);
        }
        if request["type"] == "get_available_models" { return gated_models(&local); }
        local.runtime.rpc(request)
    }

    fn configure_offline_model(&self, backend: &local_rpc::auth::Backend, agent: &str, cwd: &Path, request: &Value) -> Result<Value> {
        use local_rpc::{configuration::SavedModel, error::LocalError, models::ModelSelection};
        let replacement = self.replacement(agent);
        let epoch = replacement.epoch.load(Ordering::SeqCst);
        if self.running_cli(agent).is_some() { return Err(LocalError::new("local_session_running").into()); }
        let lock = crate::shell_switch::metadata_lock(cwd);
        let saved = { let _guard = lock.lock().unwrap(); SavedModel::read(cwd)? };
        let model = ModelSelection {
            provider_id: request["provider"].as_str().ok_or_else(|| LocalError::new("model_not_found"))?.into(),
            model_id: request["modelId"].as_str().ok_or_else(|| LocalError::new("model_not_found"))?.into(),
        };
        let resolved = if local_rpc::auto_route::is_auto(&model) {
            local_rpc::auto_route::choose(&backend.owned, &requested_pair(request)?, saved.auto_route.as_ref(),
                |model, thinking| local_rpc::models::resolve_model(backend, model, thinking))?
        } else { local_rpc::models::resolve_model(backend, &model, &saved.thinking)? };
        let _serial = replacement.serial.lock().unwrap();
        let _guard = lock.lock().unwrap();
        if replacement.epoch.load(Ordering::SeqCst) != epoch || self.running_cli(agent).is_some() { return Err(LocalError::new("local_config_changed").into()); }
        let revision = saved.save(&resolved.model_arg, &resolved.thinking_level, resolved.auto_route.as_ref())?;
        let mut result = json!({"provider":resolved.provider_id,"modelId":resolved.model_id,"thinkingLevel":resolved.thinking_level,"shellRevision":revision});
        if let Some(pair) = &resolved.auto_route { result["autoRoute"] = json!(pair); }
        Ok(result)
    }

    pub(crate) fn local_auth_in_use(&self, provider: &str) -> Vec<local_rpc::registry_types::AuthInUse> {
        // Clone the owners under one map lock. Their confirmed snapshots are
        // independently atomic; no saved shell or other runtime is examined.
        let owners: Vec<_> = self.locals.lock().unwrap().values().cloned().collect();
        owners.into_iter().filter_map(|owner| {
            let snapshot = owner.runtime.snapshot();
            (owner.runtime.is_current() && snapshot.child_pid.is_some()
                && !matches!(snapshot.state,OnlineState::Offline|OnlineState::Error)
                && snapshot.model.as_ref().is_some_and(|m| m["provider"] == provider))
                .then(|| local_rpc::registry_types::AuthInUse { agent_id:snapshot.scope.agent_id, project_root:owner.request.project_root })
        }).collect()
    }
    pub(crate) fn local_refresh_auth(&self) {
        let owners: Vec<_> = self.locals.lock().unwrap().values().cloned().collect();
        for owner in owners { if owner.running() && owner.runtime.is_current() { owner.runtime.refresh_auth(); } }
    }

    pub(super) fn local_summaries(&self) -> Vec<AgentSummary> {
        self.locals.lock().unwrap().values().map(|local| AgentSummary {
            agent_id: local.request.agent_id.clone(), cli: AgentCli::KotaLocal, cwd: local.request.cwd.clone(),
            project_root: local.request.project_root.clone(), project_id: local.request.project_id.clone(), running: local.running(),
        }).collect()
    }
}

#[cfg(test)]
mod routing_tests {
    use super::*;
    use std::fs;

    struct Discard;
    impl local_rpc::Sink for Discard {
        fn notify(&self, _: &local_rpc::LocalNotice) -> std::result::Result<(), String> { Ok(()) }
    }

    #[test]
    fn clone_monitor_and_stop_cross_the_registry_without_waking_or_controlling_a_replacement() {
        use std::os::unix::fs::PermissionsExt;
        let root=std::env::temp_dir().join(format!("clone-routing-{}",uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let executable=root.join("pi-fixture");
        fs::write(&executable,r#"#!/usr/bin/python3
import sys,json,time
args=sys.argv[1:]; session=args[args.index('--session-id')+1] if '--session-id' in args else 'parent'
for line in sys.stdin:
 r=json.loads(line)
 if r['type']=='get_state': data={'sessionId':session,'isStreaming':False,'model':{'provider':'fixture','id':'model'},'thinkingLevel':'off'}
 elif r['type']=='get_entries': data={'entries':[],'leafId':None}
 elif r['type']=='prompt':
  print(json.dumps({'type':'response','id':r['id'],'success':True,'data':{'disposition':'submitted'}}),flush=True)
  while True: time.sleep(.05)
 else: data={}
 print(json.dumps({'type':'response','id':r['id'],'success':True,'data':data}),flush=True)
"#).unwrap();
        fs::set_permissions(&executable,fs::Permissions::from_mode(0o700)).unwrap();
        let request: AgentSpawnRequest=serde_json::from_value(json!({"agentId":"parent","cli":"kota-local","cwd":root,
            "projectRoot":root,"sessionId":"parent"})).unwrap();
        let scope=LocalScope {project_root:root.clone(),agent_id:"parent".into(),session_id:"parent".into(),epoch:1};
        let mut command=Command::new(&executable);command.current_dir(&root).env_clear();
        let runtime=LocalRuntime::start(local_rpc::Config {scope:scope.clone(),ledger:root.join("ledger.jsonl"),command,
            expected_model:None,current:Arc::new(||true)},Arc::new(Discard)).unwrap();
        runtime.activate(false).unwrap();
        let registry=AgentRegistry::default();
        registry.locals.lock().unwrap().insert("parent".into(),LocalAgent {child_pid:runtime.snapshot().child_pid,
            runtime:runtime.clone(),request,lease_path:root.join("lease.json"),backend:None});
        runtime.clone_control(local_rpc::shadow_clone::Request::Create {tasks:vec![local_rpc::shadow_clone::Task {
            task:"Synthetic independent task".into(),read_only:true,timeout_minutes:0.0}]}).unwrap();
        let view=registry.local_clones(&root,"parent",Some(1)).unwrap().unwrap();
        assert_eq!(view["scope"],json!(scope));assert_eq!(view["clones"][0]["state"],"running");
        assert!(view["clones"][0]["view"].is_object());
        assert!(registry.local_clones(&root,"terminal-agent",None).unwrap().is_none());
        assert!(registry.local_clones(&root.join("other"),"parent",None).is_err());
        let stale=LocalScope {epoch:2,..scope.clone()};
        assert!(registry.local_workbench_control(&stale,json!({"type":"clone_stop","cloneId":1})).is_err());
        assert!(registry.local_workbench_control(&scope,json!({"type":"clone_stop","cloneId":1,"path":"other"})).is_err());
        registry.local_workbench_control(&scope,json!({"type":"clone_stop","cloneId":1})).unwrap();
        assert_eq!(registry.local_clones(&root,"parent",None).unwrap().unwrap()["clones"][0]["state"],"stopped");
        runtime.close().unwrap();
        registry.local_workbench_control(&scope,json!({"type":"clone_destroy","cloneId":1})).unwrap();
        assert!(registry.local_clones(&root,"parent",None).unwrap().unwrap()["clones"].as_array().unwrap().is_empty());
        assert_eq!(fs::read(root.join("ledger.jsonl")).unwrap(),b"");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mcp_status_crosses_the_registry_and_only_controls_the_exact_local_owner() {
        let root = std::env::temp_dir().join(format!("local-mcp-routing-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let request: AgentSpawnRequest = serde_json::from_value(json!({
            "agentId":"agent-mcp-fixture", "cli":"kota-local", "cwd":root,
            "projectRoot":root, "sessionId":"mcp-routing-fixture",
        })).unwrap();
        let scope = LocalScope { project_root: root.clone(), agent_id: request.agent_id.clone(),
            session_id: "mcp-routing-fixture".into(), epoch: 1 };
        let mut command = Command::new("/usr/bin/python3");
        command.args(["-u", "-c", r#"import sys,json
for line in sys.stdin:
 r=json.loads(line)
 if r['type']=='prompt':
  assert r['message'].startswith('/kota-mcp ')
  q=json.loads(r['message'][len('/kota-mcp '):])
  result={'schema':1,'requestId':q['requestId'],'available':True,'servers':[{'name':'fixture','status':'connected'}]}
  print(json.dumps({'type':'extension_ui_request','method':'notify','id':'mcp-status','message':'kota.mcp.v1:'+json.dumps(result)}),flush=True)
  data={'disposition':'handled'}
 else:
  data={'sessionId':'mcp-routing-fixture','isStreaming':False} if r['type']=='get_state' else {'entries':[],'leafId':None}
 print(json.dumps({'type':'response','id':r['id'],'success':True,'data':data}),flush=True)
"#]);
        let runtime = LocalRuntime::start(local_rpc::Config { scope: scope.clone(), ledger: root.join("ledger.jsonl"),
            command, expected_model: None, current: Arc::new(|| true) }, Arc::new(Discard)).unwrap();
        let registry = AgentRegistry::default();
        registry.locals.lock().unwrap().insert(request.agent_id.clone(), LocalAgent {
            child_pid: runtime.snapshot().child_pid, runtime: runtime.clone(), request,
            lease_path: root.join("lease.json"), backend: None,
        });
        assert_eq!(registry.local_workbench_control(&scope, json!({"type":"get_mcp_status"})).unwrap(),
            json!({"available":true,"servers":[{"name":"fixture","status":"connected"}]}));
        let stale = LocalScope { epoch: scope.epoch + 1, ..scope.clone() };
        assert!(registry.local_workbench_control(&stale, json!({"type":"get_mcp_status"})).unwrap_err().to_string().contains("changed"));
        assert!(registry.local_workbench_control(&scope, json!({"type":"get_mcp_status","path":"/other"})).is_err());
        assert!(runtime.workbench(None).blocks.unwrap().is_empty());
        assert_eq!(fs::read(root.join("ledger.jsonl")).unwrap(), b"");
        runtime.close().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn slow_offline_model_queries_do_not_own_the_replacement_lock() {
        let f = local_rpc::auth_fixture::Fixture::new(json!({"delayResolve":0.3}));
        f.store(json!({"fable":{"type":"api_key","key":"fake-owned"}}));
        let cwd = f.backend.owned.agent.join("workspace"); fs::create_dir(&cwd).unwrap();
        fs::write(cwd.join("SHELL.yaml"), "provider: kota-local\nmodel: default\n").unwrap();
        let registry = AgentRegistry::default();
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| registry.configure_offline_model(&f.backend, "agent-lock", &cwd,
                &json!({"type":"set_model","provider":"fable","modelId":"plain"})).unwrap());
            local_rpc::auth_fixture::until(|| f.requests("resolve") == 1);
            let replacement = registry.replacement("agent-lock");
            let guard = replacement.serial.try_lock().expect("Pi query must be outside replacement.serial");
            drop(guard); worker.join().unwrap();
        });
        assert!(fs::read_to_string(cwd.join("SHELL.yaml")).unwrap().contains("fable/plain"));
    }

    #[test]
    fn offline_model_control_does_not_wake_and_cannot_bypass_the_owned_gate_or_edit_a_terminal() {
        let f = local_rpc::auth_fixture::Fixture::new(json!({"actualThinking":"off"}));
        let agent = "agent-offline-model";
        let cwd = f.backend.owned.agent.join("fixture-workspace");
        fs::create_dir(&cwd).unwrap();
        fs::write(cwd.join("SHELL.yaml"), "provider: kota-local\nmodel: other/other-model\neffort: max\nskills: [preserved]\n").unwrap();
        let registry = AgentRegistry::default();
        let request = json!({"type":"set_model","provider":"fable","modelId":"plain"});
        let before = fs::read(cwd.join("SHELL.yaml")).unwrap();
        let error = registry.configure_offline_model(&f.backend, agent, &cwd, &request).unwrap_err();
        assert_eq!(error.downcast_ref::<local_rpc::error::LocalError>().unwrap().code, "provider_not_signed_in");
        assert_eq!(fs::read(cwd.join("SHELL.yaml")).unwrap(), before);
        f.store(json!({"fable":{"type":"api_key","key":"fake-owned"}}));
        assert_eq!(registry.configure_offline_model(&f.backend, agent, &cwd, &request).unwrap(),
            json!({"provider":"fable","modelId":"plain","thinkingLevel":"off","shellRevision":crate::puppeteer_setup::revision(Some(&fs::read(cwd.join("SHELL.yaml")).unwrap()))}));
        assert!(registry.local(agent).is_none(), "saving cannot start a Local owner");
        let yaml: serde_yaml::Value = serde_yaml::from_str(&fs::read_to_string(cwd.join("SHELL.yaml")).unwrap()).unwrap();
        assert_eq!(yaml["model"].as_str(), Some("fable/plain"));
        assert_eq!(yaml["effort"].as_str(), Some("off"));
        assert_eq!(yaml["skills"][0].as_str(), Some("preserved"));
        fs::write(cwd.join("SHELL.yaml"), "provider: pi\nmodel: terminal-only\neffort: high\n").unwrap();
        let before = fs::read(cwd.join("SHELL.yaml")).unwrap();
        let error = registry.configure_offline_model(&f.backend, agent, &cwd, &request).unwrap_err();
        assert_eq!(error.downcast_ref::<local_rpc::error::LocalError>().unwrap().code, "local_provider_changed");
        assert_eq!(fs::read(cwd.join("SHELL.yaml")).unwrap(), before);
    }

    #[test]
    fn a_retired_azure_selection_can_be_replaced_without_migrating_its_credentials() {
        let f = local_rpc::auth_fixture::Fixture::new(json!({"actualThinking":"off"}));
        f.store(json!({"azure-openai-responses":{"type":"api_key","key":"fake-legacy"},
            "fable":{"type":"api_key","key":"fake-owned"}}));
        let auth = fs::read(f.backend.owned.auth_path()).unwrap();
        let cwd = f.backend.owned.agent.join("fixture-workspace");
        fs::create_dir(&cwd).unwrap();
        fs::write(cwd.join("SHELL.yaml"), "provider: kota-local\nmodel: azure-openai-responses/legacy\neffort: high\nskills: [preserved]\nargs: []\nunknown: {keep: true}\n").unwrap();
        let saved = local_rpc::configuration::SavedModel::read(&cwd).unwrap();
        assert_eq!(saved.check(&f.backend).unwrap_err().code, "model_not_found");
        assert_eq!(f.requests("resolve"), 0);
        let registry = AgentRegistry::default();
        assert_eq!(registry.configure_offline_model(&f.backend, "agent-azure-repaired", &cwd,
            &json!({"type":"set_model","provider":"fable","modelId":"plain"})).unwrap(),
            json!({"provider":"fable","modelId":"plain","thinkingLevel":"off","shellRevision":crate::puppeteer_setup::revision(Some(&fs::read(cwd.join("SHELL.yaml")).unwrap()))}));
        let yaml: serde_yaml::Value = serde_yaml::from_str(&fs::read_to_string(cwd.join("SHELL.yaml")).unwrap()).unwrap();
        assert_eq!(yaml["model"].as_str(), Some("fable/plain"));
        assert_eq!(yaml["effort"].as_str(), Some("off"));
        assert_eq!(yaml["skills"][0].as_str(), Some("preserved"));
        assert!(yaml["args"].as_sequence().unwrap().is_empty());
        assert_eq!(yaml["unknown"]["keep"].as_bool(), Some(true));
        assert_eq!(fs::read(f.backend.owned.auth_path()).unwrap(), auth);
        assert!(registry.local("agent-azure-repaired").is_none());
    }

    #[test]
    fn a_legacy_default_can_be_repaired_without_validating_the_old_selection() {
        for shell in ["provider: kota-local\nmodel: default\nargs: []\nskills: [preserved]\n", "provider: kota-local\nargs: []\nskills: [preserved]\n"] {
            let f = local_rpc::auth_fixture::Fixture::new(json!({"actualThinking":"off"}));
            f.store(json!({"fable":{"type":"api_key","key":"fake-owned"}}));
            let cwd = f.backend.owned.agent.join("fixture-workspace"); fs::create_dir(&cwd).unwrap();
            fs::write(cwd.join("SHELL.yaml"), shell).unwrap();
            let saved = local_rpc::configuration::SavedModel::read(&cwd).unwrap();
            assert_eq!(saved.check(&f.backend).unwrap_err().code, "model_required");
            assert_eq!(f.requests("resolve"), 0);
            let registry = AgentRegistry::default();
            registry.configure_offline_model(&f.backend, "agent-repaired", &cwd, &json!({"type":"set_model","provider":"fable","modelId":"plain"})).unwrap();
            let yaml: serde_yaml::Value = serde_yaml::from_str(&fs::read_to_string(cwd.join("SHELL.yaml")).unwrap()).unwrap();
            assert_eq!(yaml["model"].as_str(), Some("fable/plain"));
            assert_eq!(yaml["effort"].as_str(), Some("off"));
            assert_eq!(yaml["args"].as_sequence().unwrap().len(), 0);
            assert_eq!(yaml["skills"][0].as_str(), Some("preserved"));
            assert!(f.events().iter().any(|event| event["operation"] == "resolve" && event["thinkingLevel"] == "medium"));
        }
    }

    #[test]
    fn an_offline_auto_route_pair_is_confirmed_gated_and_saved() {
        let f = local_rpc::auth_fixture::Fixture::new(json!({}));
        f.store(json!({"fable":{"type":"api_key","key":"fake-owned"}}));
        let cwd = f.backend.owned.agent.join("fixture-workspace"); fs::create_dir(&cwd).unwrap();
        fs::write(cwd.join("SHELL.yaml"), "provider: kota-local\nmodel: fable/plain\neffort: high\nskills: [preserved]\n").unwrap();
        let registry = AgentRegistry::default();
        let request = |pair: Value| json!({"type":"set_model","provider":"kota","modelId":"auto","autoRoute":pair});
        let code = |error: anyhow::Error| error.downcast_ref::<local_rpc::error::LocalError>().unwrap().code.clone();
        let before = fs::read(cwd.join("SHELL.yaml")).unwrap();
        let mut group = local_rpc::auto_route::fixture_group(); group.models[0].thinking_level = "high".into();
        let pair = serde_json::to_value(&group).unwrap();
        for (body, expected) in [(request(json!({"models":[],"defaultModel":"model-1","threshold":0.85})), "auto_route_models_required"),
            (json!({"type":"set_model","provider":"kota","modelId":"auto"}), "auto_route_models_required"),
            (request(pair.clone()), "jev_key_required")] {
            assert_eq!(code(registry.configure_offline_model(&f.backend, "agent-auto", &cwd, &body).unwrap_err()), expected);
        }
        assert_eq!(fs::read(cwd.join("SHELL.yaml")).unwrap(), before);
        local_rpc::jev::save_key_with(&f.backend, "r", "openrouter", "fixture-jev-key", |_, _, _, _| Ok("verified")).unwrap();
        assert_eq!(f.requests("resolve"), 0, "a refused selection never costs a Pi query");
        let result = registry.configure_offline_model(&f.backend, "agent-auto", &cwd, &request(pair.clone())).unwrap();
        assert_eq!(result, json!({"provider":"kota","modelId":"auto","thinkingLevel":"off","autoRoute":pair,"shellRevision":crate::puppeteer_setup::revision(Some(&fs::read(cwd.join("SHELL.yaml")).unwrap()))}));
        assert_eq!(f.requests("resolve"), 2, "Pi confirms each new physical configuration once");
        let yaml: serde_yaml::Value = serde_yaml::from_str(&fs::read_to_string(cwd.join("SHELL.yaml")).unwrap()).unwrap();
        assert_eq!((yaml["model"].as_str(), yaml["effort"].as_str()), (Some("kota/auto"), Some("off")));
        assert_eq!((yaml["auto-route"]["models"][0]["model"].as_str(), yaml["auto-route"]["models"][0]["effort"].as_str()), (Some("fable/reason"), Some("high")));
        assert_eq!(yaml["skills"][0].as_str(), Some("preserved"));
        // Saving the same pair again needs no Pi query; switching to a concrete model keeps it.
        registry.configure_offline_model(&f.backend, "agent-auto", &cwd, &request(pair.clone())).unwrap();
        assert_eq!(f.requests("resolve"), 2);
        registry.configure_offline_model(&f.backend, "agent-auto", &cwd, &json!({"type":"set_model","provider":"fable","modelId":"plain"})).unwrap();
        let concrete = local_rpc::configuration::SavedModel::read(&cwd).unwrap();
        assert_eq!(concrete.model.as_deref(), Some("fable/plain"));
        assert_eq!(serde_json::to_value(concrete.auto_route.unwrap()).unwrap(), pair);
    }

    #[test]
    fn a_legacy_default_with_invalid_effort_can_be_repaired_using_medium_then_clamp() {
        let f = local_rpc::auth_fixture::Fixture::new(json!({"actualThinking":"off"}));
        f.store(json!({"fable":{"type":"api_key","key":"fake-owned"}}));
        let cwd = f.backend.owned.agent.join("fixture-workspace");
        fs::create_dir(&cwd).unwrap();
        fs::write(cwd.join("SHELL.yaml"), "provider: kota-local\nmodel: default\neffort: High\nargs: []\nskills: [preserved]\n").unwrap();
        let registry = AgentRegistry::default();
        let result = registry.configure_offline_model(&f.backend, "agent-repaired", &cwd,
            &json!({"type":"set_model","provider":"fable","modelId":"plain"})).unwrap();
        assert_eq!(result, json!({"provider":"fable","modelId":"plain","thinkingLevel":"off","shellRevision":crate::puppeteer_setup::revision(Some(&fs::read(cwd.join("SHELL.yaml")).unwrap()))}));
        let yaml: serde_yaml::Value = serde_yaml::from_str(&fs::read_to_string(cwd.join("SHELL.yaml")).unwrap()).unwrap();
        assert_eq!(yaml["model"].as_str(), Some("fable/plain"));
        assert_eq!(yaml["effort"].as_str(), Some("off"));
        assert!(yaml["args"].as_sequence().unwrap().is_empty());
        assert_eq!(yaml["skills"][0].as_str(), Some("preserved"));
        assert!(f.events().iter().any(|event| event["operation"] == "resolve" && event["thinkingLevel"] == "medium"));
        assert!(registry.local("agent-repaired").is_none());
        assert_eq!(local_rpc::models::validate_saved_selection("fable/plain", Some("High")).unwrap_err().code, "thinking_invalid");
    }

    #[test]
    fn a_concrete_model_without_effort_uses_medium_clamp_in_memory_and_never_writes_shell() {
        let f = local_rpc::auth_fixture::Fixture::new(json!({"actualThinking":"off"}));
        f.store(json!({"fable":{"type":"api_key","key":"fake-owned"}}));
        let cwd = f.backend.owned.agent.join(".agent-workspaces/agent-fable-copper");
        fs::create_dir_all(cwd.join("project-files/.git")).unwrap();
        fs::write(cwd.join("agent.yaml"), "id: agent-fable-copper\nstatus: active\n").unwrap();
        let shell = "provider: kota-local\nmodel: fable/plain\nargs: []\nunknown: {preserve: true}\n";
        fs::write(cwd.join("SHELL.yaml"), shell).unwrap();
        let mut request: AgentSpawnRequest = serde_json::from_value(json!({"agentId":"agent-fable-copper", "cli":"kota-local", "cwd":cwd, "projectRoot":f.backend.owned.agent})).unwrap();
        let prepared = crate::shell_switch::Launch::prepare(&mut request, None).unwrap().unwrap();
        assert_eq!(request.args, ["--model", "fable/plain"]);
        let selected = resolve_launch_model(&f.backend, &request.args).unwrap();
        assert_eq!(selected.model_arg, "fable/plain");
        assert_eq!(selected.thinking_level, "off");
        assert!(f.events().iter().any(|event| event["operation"] == "resolve" && event["thinkingLevel"] == "medium"));
        let arguments = local_rpc::launch::arguments(&cwd, &f.backend.bundled.extension, &f.backend.owned.agent.join("sessions"), "test-session", Some(&selected.model_arg), Some(&selected.thinking_level), None);
        assert_eq!(option(&arguments, "--thinking").as_deref(), Some("off"));
        drop(prepared);
        assert_eq!(fs::read_to_string(cwd.join("SHELL.yaml")).unwrap(), shell);
        assert_eq!(resolve_launch_model(&f.backend, &[]).err().unwrap().code, "model_required");
        let cleanup = f.backend.owned.agent.clone();
        crate::adapter_sync::call(move || {crate::adapter_sync::take_project(&cleanup); Ok(())}).unwrap();
    }

    #[test]
    fn a_concrete_model_with_invalid_effort_uses_medium_clamp_in_memory_and_never_writes_shell() {
        let f = local_rpc::auth_fixture::Fixture::new(json!({"actualThinking":"off"}));
        f.store(json!({"fable":{"type":"api_key","key":"fake-owned"}}));
        let cwd = f.backend.owned.agent.join(".agent-workspaces/agent-fable-copper");
        fs::create_dir_all(cwd.join("project-files/.git")).unwrap();
        fs::write(cwd.join("agent.yaml"), "id: agent-fable-copper\nstatus: active\n").unwrap();
        let shell = "provider: kota-local\nmodel: fable/plain\neffort: High\nargs: []\nunknown: {preserve: true}\n";
        fs::write(cwd.join("SHELL.yaml"), shell).unwrap();
        let mut request: AgentSpawnRequest = serde_json::from_value(json!({"agentId":"agent-fable-copper", "cli":"kota-local", "cwd":cwd, "projectRoot":f.backend.owned.agent})).unwrap();
        let prepared = crate::shell_switch::Launch::prepare(&mut request, None).unwrap().unwrap();
        assert_eq!(option(&request.args, "--thinking").as_deref(), Some("High"));
        let selected = resolve_launch_model(&f.backend, &request.args).unwrap();
        assert_eq!(selected.model_arg, "fable/plain");
        assert_eq!(selected.thinking_level, "off");
        assert!(f.events().iter().any(|event| event["operation"] == "resolve" && event["thinkingLevel"] == "medium"));
        let arguments = local_rpc::launch::arguments(&cwd, &f.backend.bundled.extension, &f.backend.owned.agent.join("sessions"), "test-session", Some(&selected.model_arg), Some(&selected.thinking_level), None);
        assert_eq!(option(&arguments, "--thinking").as_deref(), Some("off"));
        drop(prepared);
        assert_eq!(fs::read_to_string(cwd.join("SHELL.yaml")).unwrap(), shell);
        let cleanup = f.backend.owned.agent.clone();
        crate::adapter_sync::call(move || {crate::adapter_sync::take_project(&cleanup); Ok(())}).unwrap();
    }

    #[test]
    fn composer_and_bus_share_live_first_routing_and_read_only_cold_target_selection() {
        let root = std::env::temp_dir().join(format!("local-routing-{}", uuid::Uuid::new_v4()));
        let agent = "agent-fable-copper";
        let cwd = root.join(".agent-workspaces").join(agent);
        fs::create_dir_all(&cwd).unwrap();
        fs::write(cwd.join("agent.yaml"), format!("id: {agent}\nprovider: codex\n")).unwrap();
        fs::write(cwd.join("SHELL.yaml"), "provider: kota-local\nmodel: default\n").unwrap();
        let before = fs::read(cwd.join("agent.yaml")).unwrap();
        let mut request: AgentSpawnRequest = serde_json::from_value(json!({
            "agentId": agent, "cli":"kota-local", "cwd":cwd, "projectRoot":root, "sessionId":"routing-fixture",
        })).unwrap();
        let registry = AgentRegistry::default();
        // A stopped terminal saved as Local must take the raw-input branch in
        // BOTH callers; merely finding an old PTY object is not liveness.
        let mut terminal = request.clone(); terminal.cli = AgentCli::Codex;
        // This object is never started and does not claim a saved native session.
        terminal.session_id = None;
        registry.ptys.lock().unwrap().insert(agent.into(), AgentTerminalPty::new(terminal.clone()).unwrap());
        assert_eq!(registry.local_input_targets(&root, &[agent.into()]).unwrap(), [agent]);
        assert!(registry.local_target(agent, Some(&request)));
        assert_eq!(fs::read(cwd.join("agent.yaml")).unwrap(), before);
        assert!(!root.join("ledger.jsonl").exists(), "classification cannot start a process or create a ledger");
        for old in [AgentCli::Claude, AgentCli::Codex, AgentCli::Antigravity, AgentCli::Opencode, AgentCli::Pi, AgentCli::Kimi] {
            assert_eq!(delivery_cli(Some(old), Some(AgentCli::KotaLocal)), Some(old));
            assert_eq!(delivery_cli(None, Some(AgentCli::KotaLocal)), Some(AgentCli::KotaLocal));
            assert_eq!(delivery_cli(Some(AgentCli::KotaLocal), Some(old)), Some(AgentCli::KotaLocal));
            assert_eq!(delivery_cli(None, Some(old)), Some(old));
        }
        registry.ptys.lock().unwrap().clear();
        let mut command = Command::new("/usr/bin/python3");
        command.args(["-u", "-c", r#"import sys,json
for line in sys.stdin:
 r=json.loads(line)
 d={'sessionId':'routing-fixture','isStreaming':False} if r['type']=='get_state' else {'entries':[],'leafId':None}
 print(json.dumps({'type':'response','id':r['id'],'success':True,'data':d}),flush=True)
"#]);
        let runtime = LocalRuntime::start(local_rpc::Config {
            scope: LocalScope { project_root: root.clone(), agent_id: agent.into(), session_id: "routing-fixture".into(), epoch: 1 },
            ledger: root.join("ledger.jsonl"), command, expected_model:None, current: Arc::new(|| true),
        }, Arc::new(Discard)).unwrap();
        registry.locals.lock().unwrap().insert(agent.into(), LocalAgent { runtime: runtime.clone(), request: request.clone(),
            lease_path: root.join("lease.json"), child_pid: runtime.snapshot().child_pid, backend:None });
        let never_spawn = local_rpc::auth::Backend { owned: local_rpc::owned::OwnedDirectory::new(&root),
            bundled: local_rpc::bundle::BundledPi { executable: root.join("must-not-run"), extension: root.join("must-not-load") } };
        let error = registry.configure_offline_model(&never_spawn, agent, &cwd, &json!({"type":"set_model","provider":"fable","modelId":"plain"})).unwrap_err();
        assert_eq!(error.downcast_ref::<local_rpc::error::LocalError>().unwrap().code, "local_session_running");
        let scope = runtime.snapshot().scope;
        assert!(registry.local_workbench(Path::new("/different-project"), agent, None).is_err());
        assert_eq!(registry.local_workbench(&root, agent, None).unwrap().unwrap().snapshot.scope, scope);
        for stale in [
            LocalScope { epoch: scope.epoch + 1, ..scope.clone() },
            LocalScope { project_root: root.join("different"), ..scope.clone() },
            LocalScope { session_id: "different-session".into(), ..scope.clone() },
        ] {
            assert!(registry.local_workbench_control(&stale, json!({"type":"abort"})).unwrap_err().to_string().contains("changed"));
        }
        fs::write(cwd.join("SHELL.yaml"), "provider: codex\nmodel: default\n").unwrap();
        request.cli = AgentCli::Codex;
        assert!(registry.local_target(agent, Some(&request)), "live Local retains its registered carrier");
        assert_eq!(registry.local_input_targets(&root, &[agent.into()]).unwrap(), [agent]);
        runtime.close().unwrap();
        assert!(!registry.local_target(agent, Some(&request)), "stopped Local follows the saved terminal target");
        assert!(registry.local_input_targets(&root, &[agent.into()]).unwrap().is_empty());
        assert_eq!(fs::read(cwd.join("agent.yaml")).unwrap(), before);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_late_save_rejects_local_input_before_adapter_session_or_child_commit() {
        let root = std::env::temp_dir().join(format!("local-routing-race-{}", uuid::Uuid::new_v4()));
        let cwd = root.join(".agent-workspaces/agent-fable-copper");
        fs::create_dir_all(&cwd).unwrap();
        let original = "id: agent-fable-copper\nprovider: kota-local\nshell: kota-local\nsession-id: old-session\n";
        fs::write(cwd.join("agent.yaml"), original).unwrap();
        fs::write(cwd.join("SHELL.yaml"), "provider: codex\nmodel: default\n").unwrap();
        let mut stale: AgentSpawnRequest = serde_json::from_value(json!({ "agentId":"agent-fable-copper",
            "cli":"kota-local", "cwd":cwd, "projectRoot":root })).unwrap();
        let launch = crate::shell_switch::Launch::prepare(&mut stale, None).unwrap().unwrap();
        assert!(ensure_expected_cli(stale.cli, Some(AgentCli::KotaLocal)).is_err());
        drop(launch);
        assert_eq!(fs::read_to_string(cwd.join("agent.yaml")).unwrap(), original);
        assert!(!cwd.join("AGENTS.md").exists());
        assert!(!root.join("project-memory/.kota/agent-session-leases").exists());
        assert!(ensure_expected_cli(stale.cli, None).is_ok(), "ordinary launches keep the original contract");
        // The failed prepared launch also released its metadata guard.
        drop(crate::shell_switch::Launch::prepare(&mut stale, None).unwrap());
        let cleanup = root.clone();
        crate::adapter_sync::call(move || { crate::adapter_sync::take_project(&cleanup); Ok(()) }).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn auth_in_use_reads_confirmed_local_snapshots_during_registry_changes() {
        let root = std::env::temp_dir().join(format!("local-in-use-{}",uuid::Uuid::new_v4()));fs::create_dir(&root).unwrap();
        let request:AgentSpawnRequest=serde_json::from_value(json!({"agentId":"agent-fable-copper","cli":"kota-local","cwd":root,
            "projectRoot":root,"sessionId":"in-use-fixture","args":["--model","other/saved-but-not-confirmed"]})).unwrap();
        let mut command=Command::new("/usr/bin/python3");command.args(["-u","-c",r#"import sys,json
for line in sys.stdin:
 r=json.loads(line)
 data={'sessionId':'in-use-fixture','isStreaming':False,'model':{'provider':'fable','id':'confirmed'}} if r['type']=='get_state' else {'entries':[],'leafId':None}
 print(json.dumps({'type':'response','id':r['id'],'success':True,'data':data}),flush=True)
"#]);
        let runtime=LocalRuntime::start(local_rpc::Config {scope:LocalScope {project_root:root.clone(),agent_id:request.agent_id.clone(),session_id:"in-use-fixture".into(),epoch:1},
            ledger:root.join("ledger.jsonl"),command,expected_model:None,current:Arc::new(||true)},Arc::new(Discard)).unwrap();
        let local=LocalAgent {child_pid:runtime.snapshot().child_pid,runtime:runtime.clone(),request:request.clone(),lease_path:root.join("lease.json"),backend:None};
        let registry=AgentRegistry::default();
        std::thread::scope(|scope| {
            scope.spawn(|| {for _ in 0..500 {registry.locals.lock().unwrap().insert(request.agent_id.clone(),local.clone());std::thread::yield_now();registry.locals.lock().unwrap().remove(&request.agent_id);}});
            for _ in 0..500 {
                let rows=registry.local_auth_in_use("fable");assert!(rows.len()<=1);
                for row in rows {assert_eq!(row.agent_id,request.agent_id);assert_eq!(row.project_root,request.project_root);}
                assert!(registry.local_auth_in_use("other").is_empty());
            }
        });
        registry.locals.lock().unwrap().insert(request.agent_id.clone(),local);
        assert_eq!(registry.local_auth_in_use("fable").len(),1);
        runtime.close().unwrap();assert!(registry.local_auth_in_use("fable").is_empty());
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
fn resolve_launch_model(backend: &local_rpc::auth::Backend, args: &[String]) -> local_rpc::error::LocalResult<local_rpc::models::ResolvedRecruit> {
    let model_arg = option(args, "--model");
    let model = local_rpc::models::selected_model(model_arg.as_deref())?;
    let thinking = option(args, "--thinking");
    local_rpc::models::resolve_model(backend, &model, local_rpc::models::saved_thinking(thinking.as_deref()))
}

fn option(args: &[String], flag: &str) -> Option<String> {
    args.iter().enumerate().find_map(|(i, arg)| {
        if arg == flag { args.get(i + 1).cloned() } else { arg.strip_prefix(&format!("{flag}=")).map(str::to_owned) }
    })
}

fn local_environment(command: &mut Command, req: &AgentSpawnRequest, home: &Path) {
    command.env_clear();
    for (key, value) in std::env::vars() {
        if should_inherit_agent_spawn_env(&key) && !local_rpc::environment::TERMINAL_ENV_NAMES.contains(&key.as_str()) {
            command.env(key, value);
        }
    }
    let root = Path::new(&req.project_root); let cwd = Path::new(&req.cwd);
    command.env("PATH", crate::pty::path_env::augmented_path(Some(home))).env("PWD", cwd).env("PI_SKIP_VERSION_CHECK", "1")
        .env("KOTA_AGENT_CWD", cwd).env("KOTA_PROJECT_ROOT", root)
        .env("KOTA_WORKTREE_ROOT", req.worktree_root.as_deref().map(PathBuf::from).unwrap_or_else(|| cwd.join("project-files")))
        .env("KOTA_PROJECT_MEMORY_DIR", req.shared_dir.as_deref().map(PathBuf::from).unwrap_or_else(|| root.join("project-memory")))
        .env("KOTA_PROJECT_RULES_DIR", req.rules_dir.as_deref().map(PathBuf::from).unwrap_or_else(|| root.join("project-rules")))
        .env("KOTA_HOME", home.join("Kota")).env("KOTA_ACCOUNT_RULES_DIR", home.join("Kota/rules"))
        .env("KOTA_AGENT_ID", &req.agent_id).env("KOTA_BBS_ROOT", home.join("Kota/Workspaces/bbs"));
    if let Some(path) = &req.adapter_path { command.env("KOTA_AGENT_ADAPTER", path); }
    if let Some(id) = &req.project_id {
        command.env("KOTA_PROJECT_ID", id).env("KOTA_PROJECT_DISPLAY_NAME", crate::bbs::display_project_name_with_fallback(id, None));
    }
    if let Some(remote) = &req.project_remote { command.env("KOTA_PROJECT_REMOTE", remote); }
    if let Some(base) = &req.project_base_ref { command.env("KOTA_PROJECT_BASE_REF", base); }
    let mut identity = CommandBuilder::new("pi");
    apply_agent_git_identity(&mut identity, root, &req.agent_id);
    for key in ["GIT_AUTHOR_NAME", "GIT_AUTHOR_EMAIL", "GIT_COMMITTER_NAME", "GIT_COMMITTER_EMAIL"] {
        if let Some(value) = identity.get_env(key) { command.env(key, value); }
    }
    local_rpc::environment::remove_model_keys(command);
}

fn gated_models(local: &LocalAgent) -> Result<Value> {
    let catalog = local_rpc::models::model_catalog(local.backend.as_ref().context("Local auth context missing")?)?;
    Ok(json!({"models":catalog.providers.into_iter().filter(|p| p.linked).flat_map(|p| p.models.into_iter().map(move |m|
        json!({"provider":p.provider_id,"id":m.model_id,"name":m.name}))).collect::<Vec<_>>()}))
}

fn configure_model(local: &LocalAgent, mut request: Value) -> Result<Value> {
    let cwd = Path::new(&local.request.project_root).join(".agent-workspaces").join(&local.request.agent_id);
    let path = cwd.join("SHELL.yaml");
    let lock = crate::shell_switch::metadata_lock(&cwd);
    let detailed = request.get("shellRevision").is_some();
    let before = {
        let _guard = lock.lock().unwrap();
        let text = fs::read_to_string(&path)?;
        if detailed && request["shellRevision"].as_str() != Some(crate::puppeteer_setup::revision(Some(text.as_bytes())).as_str()) {
            return Err(local_rpc::error::LocalError::new("local_config_changed").into());
        }
        let shell = crate::parse_shell_yaml(&text).map_err(|e| anyhow!(e))?;
        if shell.provider.as_deref().or(shell.command.as_deref()) != Some("kota-local") {
            return Err(anyhow!("saved provider is no longer Kota Local"));
        }
        text
    };
    let backend = local.backend.as_ref().context("Local auth context missing")?;
    let snapshot = local.runtime.snapshot();
    let model = if request["type"] == "set_model" {
        local_rpc::models::ModelSelection { provider_id:request["provider"].as_str().context("model_not_found")?.into(),
            model_id:request["modelId"].as_str().context("model_not_found")?.into() }
    } else {
        let model = snapshot.model.as_ref().context("Local selected model missing")?;
        local_rpc::models::ModelSelection { provider_id:model["provider"].as_str().context("model_not_found")?.into(), model_id:model["id"].as_str().context("model_not_found")?.into() }
    };
    let thinking = if request["type"] == "set_thinking_level" || request.get("level").is_some() {
        request["level"].as_str().context("thinking_invalid")?
    } else { snapshot.thinking_level.as_deref().unwrap_or(local_rpc::models::INITIAL_THINKING_LEVEL) };
    if local_rpc::auto_route::is_auto(&model) {
        // The pair owns thinking; only a pair selection reaches Pi.
        if request["type"] != "set_model" { return Err(local_rpc::error::LocalError::new("thinking_invalid").into()); }
        return configure_auto_route(local, &path, &*lock, before, &request, snapshot.model.as_ref());
    }
    let resolved = local_rpc::models::resolve_model(backend, &model, thinking)?;
    if !local.runtime.is_current() { return Err(local_rpc::error::LocalError::new("local_session_changed").into()); }
    let set_level = request["type"] == "set_model" && request.get("level").is_some();
    if request["type"] == "set_thinking_level" { request["level"] = json!(resolved.thinking_level); }
    if let Some(fields) = request.as_object_mut() {
        fields.remove("shellRevision"); fields.remove("autoRoute");
        if set_level { fields.remove("level"); }
    }
    let mut response = configuration_rpc(local, request.clone(), detailed)?;
    if set_level { configuration_rpc(local, json!({"type":"set_thinking_level","level":resolved.thinking_level}), detailed)?; }
    let state = configuration_rpc(local, json!({"type":"get_state"}), detailed)?;
    if state["model"]["provider"] != model.provider_id || state["model"]["id"] != model.model_id {
        let _ = local.runtime.close();
        return Err(configuration_failure(detailed, "local_model_session_closed", anyhow!("Local Pi selected a different model")));
    }
    let _guard = lock.lock().unwrap();
    if !local.runtime.is_current() { return Err(configuration_failure(detailed, "local_model_session_replaced", anyhow!("Local model update belongs to a replaced session"))); }
    let saved_model = if request["type"] == "set_model" {
        let model = state.get("model").context("Local selected model missing")?;
        let provider = model.get("provider").and_then(Value::as_str).context("Local selected provider missing")?;
        let id = model.get("id").and_then(Value::as_str).context("Local selected model ID missing")?;
        Some(format!("{provider}/{id}"))
    } else { None };
    // A concrete model keeps any saved pair for the next switch back to Auto Route.
    let revision = local_rpc::configuration::save_model(&path, &before, saved_model.as_deref(), configuration_thinking(&state, detailed)?, None, detailed)
        .map_err(|error| configuration_failure(detailed, "local_model_applied_save_failed", error.into()))?;
    if !response.is_object() { response = json!({}); }
    response["model"] = state["model"].clone(); response["thinkingLevel"] = state["thinkingLevel"].clone();
    response["shellRevision"] = json!(revision);
    response["autoRoute"] = json!(crate::parse_shell_yaml(&before).map_err(|e| anyhow!(e))?.auto_route.map(|group| group.group()));
    Ok(response)
}

#[cfg(test)]
#[path = "registry_configuration_tests.rs"]
mod configuration_tests;

fn configuration_failure(detailed: bool, code: &str, error: anyhow::Error) -> anyhow::Error {
    if detailed { local_rpc::error::LocalError::new(code).into() } else { error }
}
fn configuration_rpc(local: &LocalAgent, request: Value, detailed: bool) -> Result<Value> {
    local.runtime.rpc(request).map_err(|error| configuration_failure(detailed,
        if local.runtime.is_current() { "local_model_apply_unconfirmed" } else { "local_model_session_replaced" }, error))
}
fn configuration_thinking(state: &Value, detailed: bool) -> Result<Option<&str>> {
    let thinking = state.get("thinkingLevel").and_then(Value::as_str);
    if detailed && thinking.is_none() { return Err(local_rpc::error::LocalError::new("local_model_apply_unconfirmed").into()); }
    Ok(thinking)
}

fn requested_pair(request: &Value) -> std::result::Result<local_rpc::auto_route::Group, local_rpc::error::LocalError> {
    serde_json::from_value(request.get("autoRoute").cloned().unwrap_or(Value::Null))
        .map_err(|_| local_rpc::error::LocalError::new("auto_route_models_required"))
}

/// Tell the router the new pair first, then select `kota/auto` if it is not selected yet. Changing
/// the pair of an Auto Route agent never re-selects the model, so the current mode stays.
fn configure_auto_route<T>(local: &LocalAgent, path: &Path, lock: &std::sync::Mutex<T>, before: String, request: &Value,
    selected: Option<&Value>) -> Result<Value> {
    let backend = local.backend.as_ref().context("Local auth context missing")?;
    let saved = crate::parse_shell_yaml(&before).map_err(|e| anyhow!(e))?.auto_route.map(|p| p.group());
    let resolved = local_rpc::auto_route::choose(&backend.owned, &requested_pair(request)?, saved.as_ref(),
        |model, thinking| local_rpc::models::resolve_model(backend, model, thinking))?;
    let pair = resolved.auto_route.clone().context("auto_route_models_required")?;
    let detailed = request.get("shellRevision").is_some();
    if !local.runtime.is_current() { return Err(local_rpc::error::LocalError::new("local_session_changed").into()); }
    configuration_rpc(local, json!({"type":"kota_auto_route","group":pair}), detailed)?;
    let already = selected.is_some_and(|m| m["provider"] == resolved.provider_id && m["id"] == resolved.model_id);
    let mut response = if already { json!({"provider":resolved.provider_id,"id":resolved.model_id}) }
        else { configuration_rpc(local, json!({"type":"set_model","provider":resolved.provider_id,"modelId":resolved.model_id}), detailed)? };
    let state = configuration_rpc(local, json!({"type":"get_state"}), detailed)?;
    if state["model"]["provider"] != resolved.provider_id || state["model"]["id"] != resolved.model_id {
        let _ = local.runtime.close();
        return Err(configuration_failure(detailed, "local_model_session_closed", anyhow!("Local Pi selected a different model")));
    }
    let _guard = lock.lock().unwrap();
    if !local.runtime.is_current() { return Err(configuration_failure(detailed, "local_model_session_replaced", anyhow!("Local model update belongs to a replaced session"))); }
    let revision = local_rpc::configuration::save_model(path, &before, Some(&resolved.model_arg), configuration_thinking(&state, detailed)?, Some(&pair), detailed)
        .map_err(|error| configuration_failure(detailed, "local_model_applied_save_failed", error.into()))?;
    if !response.is_object() { response = json!({}); }
    response["model"] = state["model"].clone(); response["thinkingLevel"] = state["thinkingLevel"].clone();
    response["shellRevision"] = json!(revision); response["autoRoute"] = json!(pair);
    Ok(response)
}
