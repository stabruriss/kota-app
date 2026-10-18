use super::{auth::Backend, credentials::AuthType, error::{identifier, LocalError, LocalResult}, ledger::private_dir};
use anyhow::Result;
use serde::Serialize;
use std::path::Path;

pub(crate) fn storage_directory(path: &Path) -> Result<()> { private_dir(path) }

pub(crate) fn arguments(cwd: &Path, extension: &Path, sessions: &Path, session_id: &str,
    model: Option<&str>, thinking: Option<&str>, auto_route: Option<&str>) -> Vec<String> {
    let mut args = ["--mode", "rpc", "--no-approve", "-ne", "-ns", "-np", "--no-themes", "-e"]
        .into_iter().map(str::to_owned).collect::<Vec<_>>();
    args.extend([extension.display().to_string(), "--skill".into(), cwd.join(".agents/skills").display().to_string(),
        "--session-id".into(), session_id.into(), "--session-dir".into(), sessions.display().to_string()]);
    if let Some(model) = model.filter(|m| !m.is_empty() && *m != "default") { args.extend(["--model".into(), model.into()]); }
    if let Some(thinking) = thinking.filter(|m| !m.is_empty() && *m != "default") { args.extend(["--thinking".into(), thinking.into()]); }
    // One argv element with `=`: Pi splits only at the first `=`, so the pair JSON stays whole.
    if let Some(pair) = auto_route { args.push(format!("--kota-auto-route={pair}")); }
    args
}

// Compatibility DTO for the existing, explicit Local-only Tavern entry.
// Installed/version are now pinned bundle facts, never command probes.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Discovery {
    pub installed: bool, pub version: Option<String>, pub version_supported: bool,
    pub auth: Option<Auth>, pub models: Vec<Model>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Auth { pub status: String, pub provider: String, pub reason: Option<String> }
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Model { pub id: String, pub provider: String, pub model: String }
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkbenchAuth { pub provider: String, pub status: String, pub auth_type: Option<String> }

pub(crate) fn workbench_auth(backend: &Backend, provider: &str) -> LocalResult<WorkbenchAuth> {
    if !identifier(provider,128) { return Err(LocalError::new("invalid_request")); }
    let availability = super::models::availability(backend);
    if availability.summary == "error" { return Err(LocalError::new("auth_store_read_failed")); }
    let row = availability.providers.iter().find(|row| row.provider_id == provider)
        .ok_or_else(|| LocalError::provider("auth_handler_unavailable", provider))?;
    Ok(WorkbenchAuth { provider:provider.into(), status:match row.status {
        "signed_in" => "ready", "not_signed_in" => "not_ready", "invalid" => "invalid",
        _ => return Err(LocalError::provider("auth_store_read_failed", provider)),
    }.into(), auth_type:row.auth_type.map(|kind| match kind { AuthType::Oauth => "oauth", AuthType::ApiKey => "api_key" }.into()) })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clean_launch_disables_project_settings_and_retains_explicit_resources() {
        let args = arguments(Path::new("/workspace"),Path::new("/app/kota-ext/index.ts"),Path::new("/sessions"),"session",Some("fable/model"),Some("off"),None);
        assert_eq!(&args[..8], ["--mode", "rpc", "--no-approve", "-ne", "-ns", "-np", "--no-themes", "-e"]);
        assert!(!args.contains(&"--approve".into()));
        for pair in [["--skill","/workspace/.agents/skills"],["--model","fable/model"],["--session-dir","/sessions"]] {
            assert!(args.windows(2).any(|args| args == pair));
        }
        assert!(!args.iter().any(|arg| arg.starts_with("--kota-auto-route")));
        let group = super::super::auto_route::fixture_group().launch_json();
        let pair = group.as_str();
        let routed = arguments(Path::new("/workspace"),Path::new("/app/kota-ext/index.ts"),Path::new("/sessions"),"session",Some("kota/auto"),Some("off"),Some(pair));
        assert_eq!(routed.last().cloned(), Some(format!("--kota-auto-route={pair}")));
    }
}
