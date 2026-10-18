//! Saved Local model changes; never hydrate an adapter, wake an owner or write auth.
use super::{auth::Backend, error::{LocalError, LocalResult}, models};
use serde::Serialize;
use std::{fs, path::{Path, PathBuf}};

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum LaunchCheck {
    Ok,
    Blocked { #[serde(flatten)] error: LocalError },
}
impl LaunchCheck {
    pub fn from_result(result: LocalResult<()>) -> Self {
        match result { Ok(()) => Self::Ok, Err(error) => Self::Blocked { error } }
    }
}

pub(crate) fn agent_cwd(app: &tauri::AppHandle, root: &Path, agent: &str) -> LocalResult<PathBuf> {
    use tauri::Manager;
    if !root.is_absolute() || agent.is_empty() || agent.len() > 128
        || !agent.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')) {
        return Err(LocalError::new("invalid_request"));
    }
    let context = crate::resolve_incarnation_context(&app.state::<crate::integrations::IntegrationManager>(),
        root.to_str(), agent).map_err(|_| LocalError::new("invalid_request"))?;
    if root.canonicalize().ok().filter(|p| Some(p) == context.project_root.canonicalize().ok().as_ref()).is_none() {
        return Err(LocalError::new("invalid_request"));
    }
    Ok(context.cwd)
}

pub(crate) fn emit_change(app: &tauri::AppHandle, root: &Path, agent: &str, revision: &str) {
    use tauri::Emitter;
    let _ = app.emit(&format!("local-rpc://agent/{agent}/configuration"),
        serde_json::json!({"projectRoot":root,"agentId":agent,"revision":revision}));
}

pub(crate) struct SavedModel {
    path: PathBuf,
    before: String,
    pub model: Option<String>,
    pub thinking: String,
    /// The agent's Auto Route group stays saved while a concrete model is selected.
    pub auto_route: Option<super::auto_route::Group>,
}
impl SavedModel {
    pub fn read(cwd: &Path) -> LocalResult<Self> {
        let path = cwd.join("SHELL.yaml");
        let before = fs::read_to_string(&path).map_err(|_| LocalError::new("local_config_read_failed"))?;
        let shell = crate::parse_shell_yaml(&before).map_err(|_| LocalError::new("local_config_invalid"))?;
        if shell.provider.as_deref().or(shell.command.as_deref()) != Some("kota-local") {
            return Err(LocalError::new("local_provider_changed"));
        }
        let thinking = models::saved_thinking(shell.effort.as_deref()).to_owned();
        Ok(Self { path, before, model: shell.model, thinking, auto_route: shell.auto_route.filter(|p| !p.is_empty()).map(|p| p.group()) })
    }
    pub fn check(&self, backend: &Backend) -> LocalResult<()> {
        let model = models::selected_model(self.model.as_deref())?;
        if super::auto_route::is_auto(&model) { return super::auto_route::gate(&backend.owned, self.auto_route.as_ref()).map(|_| ()); }
        models::resolve_model(backend, &model, &self.thinking).map(|_| ())
    }
    /// Caller holds the same metadata lock as startup/online configuration.
    /// Compare before replacing so concurrent saved changes are never lost.
    /// A `None` pair leaves the saved one in place (a concrete model keeps it for later).
    pub fn save(&self, model: &str, thinking: &str, auto_route: Option<&super::auto_route::Group>) -> LocalResult<String> {
        save_model(&self.path, &self.before, Some(model), Some(thinking), auto_route, false)
    }
}

/// Shared by the online owner and offline set_model; neither writes a profile. A `Some` pair
/// replaces the saved `auto-route` block; `None` never removes it. Detail also updates
/// its launch args here, avoiding a second selection write in the identity worker.
/// Ordinary Workbench/startup/offline controls keep their existing args behavior.
pub(crate) fn save_model(path: &Path, before: &str, model: Option<&str>, thinking: Option<&str>, auto_route: Option<&super::auto_route::Group>, detail: bool) -> LocalResult<String> {
        if fs::read_to_string(path).map_err(|_| LocalError::new("local_config_read_failed"))? != before {
            return Err(LocalError::new("local_config_changed"));
        }
        let mut mapping: serde_yaml::Mapping = serde_yaml::from_str(before)
            .map_err(|_| LocalError::new("local_config_invalid"))?;
        if let Some(model) = model { crate::yaml_set_string(&mut mapping, "model", model); }
        if let Some(thinking) = thinking { crate::yaml_set_string(&mut mapping, "effort", thinking); }
        if let Some(pair) = auto_route {
            crate::yaml_set_value(&mut mapping, "auto-route", &super::auto_route::ShellGroup::from(pair))
                .map_err(|_| LocalError::new("local_config_write_failed"))?;
        }
        let mut text = serde_yaml::to_string(&mapping).map_err(|_| LocalError::new("local_config_write_failed"))?;
        let mut shell = crate::parse_shell_yaml(&text).map_err(|_| LocalError::new("local_config_invalid"))?;
        if detail {
            crate::sync_shell_launch_args(&mut shell, crate::pty::agent::AgentCli::KotaLocal);
            crate::yaml_set_string(&mut mapping, "provider", "kota-local");
            crate::yaml_set_value(&mut mapping, "args", &shell.args).map_err(|_| LocalError::new("local_config_write_failed"))?;
            text = serde_yaml::to_string(&mapping).map_err(|_| LocalError::new("local_config_write_failed"))?;
        }
        if shell.provider.as_deref().or(shell.command.as_deref()) == Some("kota-local")
            && shell.auto_route.is_some() {
            text = format!("{}{text}", crate::puppeteer_setup::STRING_COMMENTS);
        }
        crate::adapter_sync::atomic_replace(path, text.as_bytes()).map_err(|_| LocalError::new("local_config_write_failed"))?;
        Ok(crate::puppeteer_setup::revision(Some(text.as_bytes())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_rpc::auth_fixture::Fixture;
    use serde_json::json;

    fn shell(f: &Fixture) -> PathBuf {
        let cwd = f.backend.owned.agent.join("fixture-workspace");
        fs::create_dir(&cwd).unwrap();
        fs::write(cwd.join("SHELL.yaml"), "provider: kota-local\nmodel: fable/reason\neffort: max\nskills: [untouched]\nargs: [--original]\nunknown: {keep: true}\n").unwrap();
        cwd
    }
    #[test]
    fn launch_check_is_fresh_and_does_not_change_saved_model_or_auth() {
        let f = Fixture::new(json!({}));
        let cwd = shell(&f);
        let saved = SavedModel::read(&cwd).unwrap();
        let before = fs::read(cwd.join("SHELL.yaml")).unwrap();
        assert_eq!(saved.check(&f.backend).unwrap_err(), LocalError::provider("provider_not_signed_in", "fable"));
        f.store(json!({"other":{"type":"api_key","key":"fake-other"}}));
        assert_eq!(saved.check(&f.backend).unwrap_err().code, "provider_not_signed_in");
        f.store(json!({"fable":{"type":"api_key","key":"fake-owned"}}));
        let auth = fs::read(f.backend.owned.auth_path()).unwrap();
        saved.check(&f.backend).unwrap();
        assert_eq!(fs::read(f.backend.owned.auth_path()).unwrap(), auth);
        assert_eq!(fs::read(cwd.join("SHELL.yaml")).unwrap(), before);
        f.store(json!({"fable":{"type":"api_key","key":""}}));
        assert_eq!(saved.check(&f.backend).unwrap_err().code, "credential_invalid");
    }
    #[test]
    fn offline_configuration_uses_the_same_gate_and_native_clamp_then_preserves_other_fields() {
        let f = Fixture::new(json!({"actualThinking":"off"}));
        let cwd = shell(&f);
        let saved = SavedModel::read(&cwd).unwrap();
        let model = models::ModelSelection { provider_id:"other".into(), model_id:"other-model".into() };
        let before = fs::read(cwd.join("SHELL.yaml")).unwrap();
        assert_eq!(models::resolve_model(&f.backend, &model, &saved.thinking).err().unwrap().code, "provider_not_signed_in");
        assert_eq!(fs::read(cwd.join("SHELL.yaml")).unwrap(), before);
        f.store(json!({"other":{"type":"api_key","key":"fake-owned"}}));
        let resolved = models::resolve_model(&f.backend, &model, &saved.thinking).unwrap();
        saved.save(&resolved.model_arg, &resolved.thinking_level, None).unwrap();
        let yaml: serde_yaml::Value = serde_yaml::from_str(&fs::read_to_string(cwd.join("SHELL.yaml")).unwrap()).unwrap();
        assert_eq!(yaml["model"].as_str(), Some("other/other-model"));
        assert_eq!(yaml["effort"].as_str(), Some("off"));
        assert_eq!(yaml["skills"][0].as_str(), Some("untouched"));
        assert_eq!(yaml["args"][0].as_str(), Some("--original"));
        assert_eq!(yaml["unknown"]["keep"].as_bool(), Some(true));
        SavedModel::read(&cwd).unwrap().check(&f.backend).unwrap();
    }
    #[test]
    fn an_auto_route_group_round_trips_and_stays_while_a_concrete_model_is_selected() {
        use crate::local_rpc::auto_route::fixture_group;
        let f = Fixture::new(json!({}));
        let cwd = shell(&f);
        let both = fixture_group();
        let mut one = both.clone(); one.default_model = "model-2".into(); one.threshold = 0.75;
        one.models[0].provider_id = "openrouter".into(); one.models[0].model_id = "anthropic/claude:beta".into();
        assert_eq!(SavedModel::read(&cwd).unwrap().auto_route, None);
        for pair in [&both, &one] {
            SavedModel::read(&cwd).unwrap().save("kota/auto", "off", Some(pair)).unwrap();
            assert!(fs::read_to_string(cwd.join("SHELL.yaml")).unwrap().starts_with(crate::puppeteer_setup::STRING_COMMENTS));
            let saved = SavedModel::read(&cwd).unwrap();
            assert_eq!((saved.model.as_deref(), saved.thinking.as_str(), saved.auto_route.as_ref()), (Some("kota/auto"), "off", Some(pair)));
        }
        // The release condition needs a ready Jev channel; no Pi helper runs for kota/auto.
        assert_eq!(SavedModel::read(&cwd).unwrap().check(&f.backend).unwrap_err().code, "jev_key_required");
        crate::local_rpc::jev::save_key_with(&f.backend, "r", "openrouter", "fixture-jev-key", |_, _, _, _| Ok("verified")).unwrap();
        SavedModel::read(&cwd).unwrap().check(&f.backend).unwrap();
        assert!(f.events().is_empty(), "the Auto Route check never starts a Pi helper");
        // A concrete model keeps the pair; switching back brings it out unchanged.
        SavedModel::read(&cwd).unwrap().save("fable/reason", "high", None).unwrap();
        let concrete = SavedModel::read(&cwd).unwrap();
        assert_eq!((concrete.model.as_deref(), concrete.auto_route.as_ref()), (Some("fable/reason"), Some(&one)));
        concrete.save("kota/auto", "off", concrete.auto_route.as_ref()).unwrap();
        assert_eq!(SavedModel::read(&cwd).unwrap().auto_route, Some(one.clone()));
        let yaml: serde_yaml::Value = serde_yaml::from_str(&fs::read_to_string(cwd.join("SHELL.yaml")).unwrap()).unwrap();
        assert_eq!(yaml["auto-route"]["models"][1]["model"].as_str(), Some("fable/plain"));
        assert_eq!(yaml["auto-route"]["default-model"].as_str(), Some("model-2"));
        assert_eq!(yaml["auto-route"]["threshold"].as_f64(), Some(0.75));
        assert_eq!(yaml["skills"][0].as_str(), Some("untouched"));
    }

    #[test]
    fn saving_a_concrete_model_retains_an_empty_string_group_and_its_instructions() {
        let f = Fixture::new(json!({}));
        let cwd = shell(&f); let path = cwd.join("SHELL.yaml");
        let before = format!("{}auto-route: {{threshold: 0.85, default-model: model-1, models: []}}\n", fs::read_to_string(&path).unwrap());
        fs::write(&path, before).unwrap();
        SavedModel::read(&cwd).unwrap().save("fable/plain", "off", None).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with(crate::puppeteer_setup::STRING_COMMENTS));
        let parsed = crate::parse_shell_yaml(&text).unwrap();
        assert!(parsed.auto_route.unwrap().is_empty());
        assert_eq!(parsed.model.as_deref(), Some("fable/plain"));
        let mapping: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
        assert!(mapping["auto-route"]["models"].as_sequence().unwrap().is_empty());
        assert_eq!(mapping["unknown"]["keep"].as_bool(), Some(true));
        assert_eq!(SavedModel::read(&cwd).unwrap().auto_route, None);
    }

    #[test]
    fn saved_changes_and_terminal_shells_cannot_be_overwritten_by_an_offline_local_control() {
        let f = Fixture::new(json!({}));
        let cwd = shell(&f);
        let saved = SavedModel::read(&cwd).unwrap();
        fs::write(cwd.join("SHELL.yaml"), "provider: pi\nmodel: terminal-only\neffort: high\n").unwrap();
        let before = fs::read(cwd.join("SHELL.yaml")).unwrap();
        assert_eq!(saved.save("other/other-model", "off", None).unwrap_err().code, "local_config_changed");
        assert_eq!(SavedModel::read(&cwd).err().unwrap().code, "local_provider_changed");
        assert_eq!(fs::read(cwd.join("SHELL.yaml")).unwrap(), before);
    }
}
