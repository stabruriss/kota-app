//! Explicit Local-only IPC. Ordinary provider discovery never calls these.
use super::{Injection, Snapshot};
use crate::{integrations::IntegrationManager, pty::PtyManager};
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;
use tauri::{AppHandle, Manager, State};
use super::{auth::Backend, error::{LocalError, LocalResult}, models};

pub(crate) async fn model_options(app: &AppHandle) -> Result<Vec<crate::SupportedProviderModel>, String> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let catalog = models::model_catalog(&Backend::for_app(&app).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        Ok(catalog.providers.into_iter().filter(|p| p.linked).flat_map(|p| p.models.into_iter().map(move |m| {
            let id = format!("{}/{}", p.provider_id, m.model_id);
            crate::model(&id, &id, "Kota Local")
        })).collect())
    }).await.map_err(|_| "local_model_catalog_failed".to_string())?
}

/// Archive remains recallable; neither operation removes the private Local
/// ledger/session directory. Only the owned process follows the lifecycle.
pub(crate) fn archive(app: &AppHandle, request: crate::ProjectAgentLifecycleRequest)
    -> Result<crate::ProjectAgentLifecycleResult, String> {
    let agent = request.agent_id.clone();
    let result = crate::archive_project_agent(&app.state::<IntegrationManager>(), request)?;
    if result.ok { app.state::<PtyManager>().agent_close(app, agent).map_err(|e| e.to_string())?; }
    Ok(result)
}

pub(crate) fn dismiss(app: &AppHandle, request: crate::ProjectAgentLifecycleRequest)
    -> Result<crate::ProjectAgentLifecycleResult, String> {
    let manager = app.state::<IntegrationManager>();
    let ctx = crate::resolve_incarnation_context(&manager, request.project_root.as_deref(), &request.agent_id)?;
    crate::ensure_incarnation_project_files_worktree(&ctx, &request.agent_id)?;
    // Keep the existing dirty-work confirmation ahead of stopping anything.
    if !request.force_dirty && !crate::project_work_dirty_status_short(&ctx.worktree_root).unwrap_or_default().trim().is_empty() {
        return crate::dismiss_project_agent(&manager, request);
    }
    app.state::<PtyManager>().agent_close(app, request.agent_id.clone()).map_err(|e| e.to_string())?;
    crate::dismiss_project_agent(&manager, request)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ComposerInput {
    pub project_root: String,
    pub agent_id: String,
    pub message_id: String,
    pub created_at: String,
    pub private: bool,
    pub text: String,
    #[serde(default)]
    pub soul_input: Option<crate::soul_push::input::Input>,
}

#[tauri::command]
pub(crate) async fn local_rpc_targets(app: AppHandle, project_root: String, agent_ids: Vec<String>) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<PtyManager>().agent_local_input_targets(Path::new(&project_root), &agent_ids).map_err(|e| e.to_string())
    }).await.map_err(|e| format!("join Local routing: {e}"))?
}

#[tauri::command]
pub(crate) async fn local_rpc_submit(app: AppHandle, request: ComposerInput) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let pty = app.state::<PtyManager>();
        let input = Injection::text(&request.message_id, &request.agent_id, request.created_at,
            request.private, "kota.user", request.text, None);
        input.validate(&request.agent_id).map_err(|e| e.to_string())?;
        let fallback = if pty.agent_is_local(&request.agent_id) {
            let scope = pty.agent_local_snapshot(&request.agent_id).map_err(|e| e.to_string())?.scope;
            if scope.project_root != Path::new(&request.project_root) {
                return Err("Local input belongs to another project".into());
            }
            None
        } else {
            let launch = crate::resolve_project_agent_launch(&app.state::<IntegrationManager>(),
                Some(&request.project_root), &request.agent_id)?;
            if !pty.agent_local_target(&request.agent_id, Some(&launch)) { return Err("Local input targets a terminal provider".into()); }
            Some(launch)
        };
        let soul = request.soul_input.filter(|soul| soul.composer_for(&request.agent_id, Some(&request.message_id)));
        pty.agent_submit_local(&app, &request.agent_id, input, crate::shell_switch::PromptSource::Composer, false, fallback, soul)
            .map_err(|e| e.to_string())
    }).await.map_err(|e| format!("join Local submit: {e}"))?
}

#[tauri::command]
pub(crate) fn local_rpc_snapshot(manager: State<'_, PtyManager>, agent_id: String) -> Result<Snapshot, String> {
    manager.agent_local_snapshot(&agent_id).map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
#[serde(untagged)]
pub(crate) enum ControlError { Local(LocalError), Existing(String) }

#[tauri::command]
pub(crate) async fn local_rpc_control(app: AppHandle, agent_id: String, request: Value) -> Result<Value, ControlError> {
    let offline_model = request["type"] == "set_model" && request.get("projectRoot").is_some();
    tauri::async_runtime::spawn_blocking(move || {
        let root = request["projectRoot"].as_str().map(str::to_owned);
        let result = app.state::<PtyManager>().agent_local_control(&app, &agent_id, request).map_err(|e| {
            if offline_model { ControlError::Local(e.downcast_ref::<LocalError>().cloned().unwrap_or_else(|| LocalError::new("local_model_update_failed"))) }
            else { ControlError::Existing(e.to_string()) }
        })?;
        if offline_model {
            if let (Some(root), Some(revision)) = (root, result["shellRevision"].as_str()) {
                super::configuration::emit_change(&app, Path::new(&root), &agent_id, revision);
            }
        }
        Ok(result)
    }).await.map_err(|e| ControlError::Existing(format!("join Local control: {e}")))?
}

#[tauri::command]
pub(crate) async fn local_rpc_launch_check(app: AppHandle, project_root: String, agent_id: String) -> super::configuration::LaunchCheck {
    use super::configuration::{LaunchCheck, SavedModel, agent_cwd};
    tauri::async_runtime::spawn_blocking(move || LaunchCheck::from_result((|| {
        let cwd = agent_cwd(&app, Path::new(&project_root), &agent_id)?;
        let saved = SavedModel::read(&cwd)?;
        saved.check(&Backend::for_app(&app)?)
    })())).await.unwrap_or_else(|_| LaunchCheck::from_result(Err(LocalError::new("local_launch_check_failed"))))
}

#[derive(Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub(crate) struct LaunchAction {
    action: String, project_root: String, agent_id: String,
    attempt_id: Option<String>, selection: Option<super::startup::Selection>,
}
/// Snapshot and cancellation remain available before a Pi owner exists.
#[tauri::command]
pub(crate) async fn local_rpc_launch_action(app: AppHandle, request: LaunchAction) -> LocalResult<Value> {
    use super::{startup, configuration::{agent_cwd, SavedModel}};
    tauri::async_runtime::spawn_blocking(move || {
        if !Path::new(&request.project_root).is_absolute() || !super::error::identifier(&request.agent_id,128) {
            return Err(LocalError::new("invalid_request"));
        }
        let value = match request.action.as_str() {
            "read" => serde_json::to_value(startup::read(&request.project_root, &request.agent_id)),
            "configuration" => {
                let cwd = agent_cwd(&app, Path::new(&request.project_root), &request.agent_id)?;
                // Read only SHELL, without adapter repairs or Pi helpers.
                let saved = SavedModel::read(&cwd)?;
                Ok(serde_json::json!({"model":saved.model,"thinkingLevel":saved.thinking,"autoRoute":saved.auto_route}))
            },
            "begin" => {
                agent_cwd(&app, Path::new(&request.project_root), &request.agent_id)?;
                let target = request.agent_id.clone();
                let notify = std::sync::Arc::new(move |view| {
                    use tauri::Emitter;
                    let _ = app.emit(&format!("local-rpc://agent/{target}/startup"), view);
                });
                serde_json::to_value(startup::begin(request.project_root, request.agent_id, request.selection, notify)?)
            },
            "cancel" | "failed" => {
                let attempt = startup::get(&request.project_root, &request.agent_id,
                    request.attempt_id.as_deref().ok_or_else(|| LocalError::new("invalid_request"))?)?;
                attempt.finish(Err(LocalError::new(if request.action == "cancel" { "startup_cancelled" } else { "local_launch_preparation_failed" })));
                serde_json::to_value(attempt.view())
            },
            _ => return Err(LocalError::new("invalid_request")),
        };
        value.map_err(|_| LocalError::new("local_start_failed"))
    }).await.map_err(|_| LocalError::new("local_start_failed"))?
}

/// Workbench-only transport; the shared pty_agent_spawn contract is unchanged.
#[tauri::command]
pub(crate) async fn local_rpc_start(app: AppHandle, request: crate::pty::agent::AgentSpawnRequest,
    attempt_id: String) -> Result<crate::pty::agent::AgentRoute, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let attempt = super::startup::get(&request.project_root, &request.agent_id, &attempt_id).map_err(|e| e.to_string())?;
        app.state::<PtyManager>().agent_start_local_workbench(&app, request, attempt).map_err(|e| e.to_string())
    }).await.map_err(|_| "local_start_failed".to_string())?
}

#[tauri::command]
pub(crate) async fn local_rpc_discover(app: AppHandle, provider: Option<String>) -> Result<super::Discovery, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let backend = Backend::for_app(&app).map_err(|e| e.to_string())?;
        let catalog = models::model_catalog(&backend).map_err(|e| e.to_string())?;
        let auth = provider.map(|provider| super::launch::workbench_auth(&backend, &provider))
            .transpose().map_err(|e| e.to_string())?.map(|auth| super::launch::Auth { status:auth.status, provider:auth.provider, reason:None });
        Ok(super::Discovery { installed:true, version:Some(super::bundle::PI_VERSION.into()), version_supported:true, auth,
            models:catalog.providers.into_iter().filter(|p| p.linked).flat_map(|p| p.models.into_iter().map(move |m| super::launch::Model {
                id:format!("{}/{}",p.provider_id,m.model_id),provider:p.provider_id.clone(),model:m.model_id,
            })).collect() })
    }).await.map_err(|e| format!("join Local discovery: {e}"))?
}

#[tauri::command]
pub(crate) fn local_rpc_workbench(manager: State<'_, PtyManager>, project_root: String, agent_id: String,
    since: Option<super::WorkbenchCursor>) -> Result<Option<super::WorkbenchView>, String> {
    manager.agent_local_workbench(Path::new(&project_root), &agent_id, since.as_ref()).map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn local_rpc_workbench_control(app: AppHandle, scope: super::LocalScope, request: Value) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let changes_configuration = matches!(request["type"].as_str(), Some("set_model" | "set_thinking_level"));
        let result = app.state::<PtyManager>().agent_local_workbench_control(&scope, request).map_err(|e| e.to_string())?;
        if changes_configuration {
            if let Some(revision) = result["shellRevision"].as_str() {
                super::configuration::emit_change(&app, &scope.project_root, &scope.agent_id, revision);
            }
        }
        Ok(result)
    }).await.map_err(|e| format!("join Local workbench control: {e}"))?
}

#[tauri::command]
pub(crate) fn local_rpc_shadow_clones(manager: State<'_,PtyManager>, project_root: String, agent_id: String,
    monitor: Option<u64>) -> Result<Option<Value>,String> {
    manager.agent_local_clones(Path::new(&project_root),&agent_id,monitor).map_err(|e|e.to_string())
}

#[tauri::command]
pub(crate) async fn local_rpc_mcp_status(app: AppHandle, project_root: String, agent_id: String,
    scope: Option<super::LocalScope>) -> LocalResult<super::mcp::View> {
    tauri::async_runtime::spawn_blocking(move || {
        let cwd = super::configuration::agent_cwd(&app,Path::new(&project_root),&agent_id)?;
        super::configuration::SavedModel::read(&cwd)?; // Local-only, no credential/probe calls.
        let pty = app.state::<PtyManager>();
        let current = pty.agent_local_workbench(Path::new(&project_root),&agent_id,None)
            .map_err(|_| LocalError::new("mcp_status_unavailable"))?;
        let live = if let Some(view) = current.filter(|v| matches!(v.snapshot.state,super::OnlineState::Ready|super::OnlineState::Busy)) {
            if scope.as_ref().is_some_and(|scope| scope != &view.snapshot.scope) { return Err(LocalError::new("mcp_status_unavailable")); }
            Some(pty.agent_local_workbench_control(&view.snapshot.scope,serde_json::json!({"type":"get_mcp_status"}))
                .map_err(|_| LocalError::new("mcp_status_unavailable"))?)
        } else { None };
        super::mcp::view(&Backend::for_app(&app)?.owned,live)
    }).await.map_err(|_| LocalError::new("mcp_status_unavailable"))?
}

#[tauri::command]
pub(crate) async fn local_rpc_mcp_remove(app: AppHandle, project_root: String, agent_id: String, name: String) -> LocalResult<bool> {
    tauri::async_runtime::spawn_blocking(move || {
        let cwd = super::configuration::agent_cwd(&app,Path::new(&project_root),&agent_id)?;
        super::configuration::SavedModel::read(&cwd)?;
        super::mcp::remove(&Backend::for_app(&app)?.owned,&name)
    }).await.map_err(|_| LocalError::new("mcp_config_write_failed"))?
}

#[tauri::command]
pub(crate) async fn local_rpc_workbench_auth(app: AppHandle, provider: String) -> Result<super::launch::WorkbenchAuth, String> {
    tauri::async_runtime::spawn_blocking(move || {
        super::launch::workbench_auth(&Backend::for_app(&app).map_err(|e| e.to_string())?, &provider).map_err(|e| e.to_string())
    }).await.map_err(|e| format!("join Local credential status: {e}"))?
}

#[tauri::command]
pub(crate) async fn local_rpc_auth_availability(app: AppHandle) -> LocalResult<models::Availability> {
    tauri::async_runtime::spawn_blocking(move || {
        Ok(models::availability_for_app(&app))
    }).await.map_err(|_| LocalError::new("auth_helper_failed"))?
}

#[tauri::command]
pub(crate) async fn local_rpc_model_catalog(app: AppHandle) -> LocalResult<models::Catalog> {
    tauri::async_runtime::spawn_blocking(move || models::model_catalog(&Backend::for_app(&app)?))
        .await.map_err(|_| LocalError::new("auth_helper_failed"))?
}

#[tauri::command]
pub(crate) async fn local_rpc_auth_start(app: AppHandle, request: super::auth_protocol::AuthStart) -> LocalResult<super::auth_protocol::AuthStarted> {
    use tauri::Emitter;
    tauri::async_runtime::spawn_blocking(move || {
        let backend = Backend::for_app(&app)?;
        let events = app.clone(); let owners = app.clone();
        super::auth::start(backend, request, std::sync::Arc::new(move |event| { let _ = events.emit("local-auth-event", event); }),
            std::sync::Arc::new(move || { owners.state::<PtyManager>().agent_local_refresh_auth(); }))
    }).await.map_err(|_| LocalError::new("auth_helper_failed"))?
}

#[tauri::command]
pub(crate) async fn local_rpc_auth_respond(request: super::auth_protocol::AuthReply) -> LocalResult<()> {
    tauri::async_runtime::spawn_blocking(move || super::auth::respond(request))
        .await.map_err(|_| LocalError::new("auth_helper_failed"))?
}

#[tauri::command]
pub(crate) async fn local_rpc_auth_cancel(operation_id: String) -> LocalResult<()> {
    tauri::async_runtime::spawn_blocking(move || super::auth::cancel(&operation_id))
        .await.map_err(|_| LocalError::new("auth_helper_failed"))?
}

#[tauri::command]
pub(crate) fn local_rpc_auth_in_use(manager: State<'_, PtyManager>, provider_id: String) -> LocalResult<Vec<super::registry_types::AuthInUse>> {
    if !super::error::identifier(&provider_id,128) { return Err(LocalError::new("invalid_request")); }
    Ok(manager.agent_local_auth_in_use(&provider_id))
}

// --- User-managed Magpie. Only these explicit UI actions probe its gateway. ---
#[tauri::command]
pub(crate) async fn local_rpc_magpie_read() -> LocalResult<super::magpie::State> {
    tauri::async_runtime::spawn_blocking(|| super::magpie::read(&super::owned::OwnedDirectory::new(&crate::kota_home_dir())))
        .await.map_err(|_| LocalError::new("save_failed"))?
}

#[tauri::command]
pub(crate) async fn local_rpc_magpie_probe(app: AppHandle) -> LocalResult<super::magpie::State> {
    tauri::async_runtime::spawn_blocking(move || {
        let result = super::magpie::probe(&Backend::for_app(&app)?);
        app.state::<PtyManager>().agent_local_refresh_auth();
        result
    }).await.map_err(|_| LocalError::new("save_failed"))?
}

#[tauri::command]
pub(crate) async fn local_rpc_magpie_save(app: AppHandle, request: super::magpie::Save) -> LocalResult<super::magpie::State> {
    tauri::async_runtime::spawn_blocking(move || {
        let result = super::magpie::save(&Backend::for_app(&app)?, request);
        app.state::<PtyManager>().agent_local_refresh_auth();
        result
    }).await.map_err(|_| LocalError::new("save_failed"))?
}

// --- Jev (shared by Auto Route and Puppeteer). Keys never cross this boundary. ---

fn jev_changed(app: &AppHandle, owned: &super::owned::OwnedDirectory) {
    use tauri::Emitter;
    if let Ok(jev) = super::jev::read(owned) { let _ = app.emit("local-jev://changed", jev); }
}

#[tauri::command]
pub(crate) async fn local_rpc_jev_read(app: AppHandle) -> LocalResult<super::jev::JevSettings> {
    tauri::async_runtime::spawn_blocking(move || super::jev::read(&Backend::for_app(&app)?.owned))
        .await.map_err(|_| LocalError::new("jev_store_invalid"))?
}

#[tauri::command]
pub(crate) async fn local_rpc_jev_select(app: AppHandle, provider_id: String) -> LocalResult<super::jev::JevSettings> {
    tauri::async_runtime::spawn_blocking(move || {
        let backend = Backend::for_app(&app)?;
        let settings = super::jev::select(&backend, &provider_id)?;
        jev_changed(&app, &backend.owned);
        Ok(settings)
    }).await.map_err(|_| LocalError::new("jev_store_write_failed"))?
}

/// Deliberately not Debug: the key is held only for the save-time check.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct JevKeySave { request_id: String, provider_id: String, key: String }

#[tauri::command]
pub(crate) async fn local_rpc_jev_key_save(app: AppHandle, request: JevKeySave) -> LocalResult<super::jev::JevSettings> {
    tauri::async_runtime::spawn_blocking(move || {
        let backend = Backend::for_app(&app)?;
        let settings = super::jev::save_key(&backend, &request.request_id, &request.provider_id, &request.key)?;
        jev_changed(&app, &backend.owned);
        Ok(settings)
    }).await.map_err(|_| LocalError::new("jev_verification_failed"))?
}

#[tauri::command]
pub(crate) fn local_rpc_jev_key_cancel(request_id: String) -> LocalResult<()> { super::jev::cancel(&request_id) }
