//! Local recruitment derives concrete settings without changing the Hero template.
use crate::local_rpc::{
    error::{LocalError, LocalResult},
    models::{self, RecruitSelection, ResolvedRecruit},
};
use crate::{
    cli_from_shell, parse_shell_yaml, sync_shell_launch_args, ShellYaml, TavernHeroProfileDraft,
};

pub(crate) fn prepare(
    profile: &TavernHeroProfileDraft,
    resolve: impl FnOnce(RecruitSelection) -> LocalResult<ResolvedRecruit>,
) -> Result<Option<ShellYaml>, String> {
    let mut shell = match parse_shell_yaml(&profile.shell) {
        Ok(shell) => shell,
        Err(error) if profile.provider == "kota-local" => return Err(error),
        // Leave non-Local validation and its existing save/materialize order alone.
        Err(_) => return Ok(None),
    };
    if cli_from_shell(&shell, profile).ok() != Some(crate::pty::agent::AgentCli::KotaLocal) {
        return Ok(None);
    }
    let model_arg = shell.model.as_deref().unwrap_or_default().trim();
    let describe_error = |error: LocalError| {
        if error.code == "model_not_found"
            && model_arg != crate::local_rpc::auto_route::MODEL_ARG
            && model_arg
                .split_once('/')
                .is_some_and(|(provider, id)| models::valid_model_identifiers(provider, id))
        {
            format!("Model \"{model_arg}\" was not found.")
        } else {
            error.to_string()
        }
    };
    let model = models::selected_model(Some(model_arg)).map_err(describe_error)?;
    let resolved = resolve(RecruitSelection {
        schema: 1,
        provider_id: model.provider_id,
        model_id: model.model_id,
        thinking_level: Some(models::saved_thinking(shell.effort.as_deref()).into()),
        auto_route: shell.auto_route.as_ref().filter(|pair| !pair.is_empty()).map(|pair| pair.group()),
    }).map_err(describe_error)?;
    shell.model = Some(resolved.model_arg);
    shell.effort = Some(resolved.thinking_level);
    // The new agent gets its own copy of the template's pair; later template edits never reach it.
    if let Some(pair) = &resolved.auto_route { shell.auto_route = Some(pair.into()); }
    sync_shell_launch_args(&mut shell, crate::pty::agent::AgentCli::KotaLocal);
    Ok(Some(shell))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(shell: &str) -> TavernHeroProfileDraft {
        serde_json::from_value(serde_json::json!({
            "heroId":"local-fixture", "name":"Local fixture", "provider":"kota-local",
            "model":"default", "skills":[], "ghost":"Fixture", "shell":shell,
            "archived":false, "dismissed":false
        }))
        .unwrap()
    }
    fn resolved(thinking: &str) -> ResolvedRecruit {
        ResolvedRecruit {
            schema: 1,
            provider_id: "fable".into(),
            model_id: "family/model".into(),
            model_arg: "fable/family/model".into(),
            thinking_level: thinking.into(),
            auto_route: None,
        }
    }

    #[test]
    fn concrete_models_keep_off_and_native_clamp_without_changing_template() {
        for (effort, requested) in [(None, "medium"), (Some("default"), "medium"), (Some("off"), "off"), (Some("max"), "max")] {
            let mut input = "provider: kota-local\nmodel: fable/family/model\nargs: [--model, stale, --thinking, high, --approve]\n".to_owned();
            if let Some(effort) = effort { input.push_str(&format!("effort: {effort:?}\n")); }
            let hero = profile(&input);
            let before = serde_json::to_value(&hero).unwrap();
            let mut calls = 0;
            let shell = prepare(&hero, |selection| {
                calls += 1;
                assert_eq!(selection.schema, 1);
                assert_eq!(selection.provider_id, "fable");
                assert_eq!(selection.model_id, "family/model");
                assert_eq!(selection.thinking_level.as_deref(), Some(requested));
                Ok(resolved("off"))
            }).unwrap().unwrap();
            assert_eq!(calls, 1);
            assert_eq!(serde_json::to_value(&hero).unwrap(), before);
            assert_eq!(shell.provider.as_deref(), Some("kota-local"));
            assert_eq!(shell.model.as_deref(), Some("fable/family/model"));
            assert_eq!(shell.effort.as_deref(), Some("off"));
            assert_eq!(shell.args, ["--model", "fable/family/model", "--thinking", "off", "--approve"]);
            let written = crate::parse_shell_yaml(&crate::compile_shell_yaml_text(&shell)).unwrap();
            assert_eq!(written.command.as_deref(), Some("kota-local"));
            assert_eq!(written.model, shell.model);
            assert_eq!(written.effort, shell.effort);
            assert_eq!(written.args, shell.args);
        }
    }

    #[test]
    fn an_auto_route_template_gives_the_new_agent_its_own_copy_of_the_group() {
        let pair = crate::local_rpc::auto_route::fixture_group();
        let shell = ShellYaml { provider: Some("kota-local".into()), model: Some("kota/auto".into()),
            effort: Some("off".into()), auto_route: Some((&pair).into()), ..ShellYaml::default() };
        let hero = profile(&crate::compile_shell_yaml_text(&shell));
        let before = serde_json::to_value(&hero).unwrap();
        let shell = prepare(&hero, |selection| {
            assert_eq!((selection.provider_id.as_str(), selection.model_id.as_str(), selection.auto_route.as_ref()), ("kota", "auto", Some(&pair)));
            Ok(ResolvedRecruit { schema: 1, provider_id: "kota".into(), model_id: "auto".into(), model_arg: "kota/auto".into(),
                thinking_level: "off".into(), auto_route: Some(pair.clone()) })
        }).unwrap().unwrap();
        assert_eq!(serde_json::to_value(&hero).unwrap(), before, "the template itself is unchanged");
        assert_eq!((shell.model.as_deref(), shell.effort.as_deref()), (Some("kota/auto"), Some("off")));
        let written = crate::parse_shell_yaml(&crate::compile_shell_yaml_text(&shell)).unwrap();
        assert_eq!(written.auto_route.unwrap().group(), pair);
        // A concrete-model template still hands over the pair it keeps for later.
        let kept = profile(&crate::compile_shell_yaml_text(&ShellYaml { model: Some("fable/family/model".into()), ..shell.clone() }));
        let shell = prepare(&kept, |selection| { assert!(selection.auto_route.is_some()); Ok(resolved("off")) }).unwrap().unwrap();
        assert_eq!(shell.auto_route.unwrap().group(), pair);
        let blocked = profile("provider: kota-local\nmodel: kota/auto\n");
        assert_eq!(prepare(&blocked, |_| Err(LocalError::new("auto_route_models_required"))).unwrap_err(), "auto_route_models_required");
        assert_eq!(prepare(&blocked, |_| Err(LocalError::new("model_not_found"))).unwrap_err(), "model_not_found");
    }

    #[test]
    fn a_legacy_local_template_requires_a_model_before_any_resolver_or_save() {
        for model in [None, Some(""), Some("default"), Some(" Default ")] {
            let mut input = "provider: kota-local\n".to_owned();
            if let Some(model) = model { input.push_str(&format!("model: {model:?}\n")); }
            let hero = profile(&input);
            let before = serde_json::to_value(&hero).unwrap();
            assert_eq!(prepare(&hero, |_| panic!("missing model cannot reach Pi or save")).unwrap_err(), "model_required");
            assert_eq!(serde_json::to_value(&hero).unwrap(), before);
        }
    }

    #[test]
    fn missing_catalog_model_names_the_full_selection_from_the_shell() {
        use crate::local_rpc::auth_fixture::Fixture;
        use serde_json::json;

        let fixture = Fixture::new(json!({"providers": [{
            "providerId": "magpie", "providerLabel": "Magpie", "canApiKey": true, "canOAuth": false,
            "models": [{"modelId": "agy-cli/gemini-4-argon", "name": "Argon", "thinkingLevels": ["off"]}]
        }]}));
        fixture.store(json!({"magpie": {"type": "api_key", "key": "fixture-key"}}));
        let hero = profile("provider: kota-local\nmodel: magpie/antigravity/gemini-4-argon\n");
        let error = prepare(&hero, |selection| {
            models::resolve_local_recruit_config(&fixture.backend, selection)
        }).unwrap_err();

        assert_eq!(error, "Model \"magpie/antigravity/gemini-4-argon\" was not found.");
        assert_eq!(fixture.requests("resolve"), 1);
    }

    #[test]
    fn retired_provider_names_the_model_rejected_before_resolution() {
        let hero = profile("provider: kota-local\nmodel: azure-openai-responses/old-model\n");
        let error = prepare(&hero, |_| panic!("retired provider must not resolve")).unwrap_err();
        assert_eq!(error, "Model \"azure-openai-responses/old-model\" was not found.");
    }

    #[test]
    fn rejected_recruit_has_no_derived_shell_and_keeps_the_original_request() {
        for code in [
            "provider_not_signed_in",
            "auth_store_read_failed",
            "auth_store_invalid",
            "credential_invalid",
            "azure_resource_missing",
            "auth_handler_unavailable",
            "thinking_invalid",
        ] {
            let hero = profile("provider: kota-local\nmodel: fable/family/model\n");
            let before = serde_json::to_value(&hero).unwrap();
            let error = prepare(&hero, |_| Err(LocalError::new(code))).unwrap_err();
            assert_eq!(error, code);
            assert_eq!(serde_json::to_value(&hero).unwrap(), before);
        }
    }

    #[test]
    fn non_local_recruit_never_resolves_local_configuration() {
        for provider in ["claude", "codex", "pi", "opencode", "kimi", "antigravity"] {
            let mut hero = profile(&format!("provider: {provider}\nmodel: default\n"));
            hero.provider = provider.into();
            assert!(prepare(&hero, |_| panic!(
                "non-Local must not access Local auth"
            ))
            .unwrap()
            .is_none());
        }
    }

    #[test]
    fn invalid_model_and_reserved_route_do_not_reach_the_resolver() {
        for model in [
            "auto route",
            "auto-route",
            "bare-model",
            "provider/",
            "/model",
        ] {
            let hero = profile(&format!("provider: kota-local\nmodel: {model:?}\n"));
            assert_eq!(
                prepare(&hero, |_| panic!("invalid selection must not resolve")).unwrap_err(),
                "model_not_found"
            );
        }
    }
}
