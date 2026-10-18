use super::{auth_fixture::Fixture, credentials::AuthType, error::LocalError, models::*};
use serde_json::json;

fn selection(model: &str, thinking: Option<&str>) -> RecruitSelection {
    serde_json::from_value(
        json!({"schema":1,"providerId":"fable","modelId":model,
        "thinkingLevel":thinking}),
    )
    .unwrap()
}

#[test]
fn retired_azure_selection_is_blocked_without_reading_helpers_or_migrating_credentials() {
    let f = Fixture::new(json!({}));
    f.store(json!({"azure-openai-responses":{"type":"api_key","key":"fake-legacy-key",
        "env":{"AZURE_OPENAI_BASE_URL":"http://127.0.0.1:1/legacy"}}}));
    let auth = std::fs::read(f.backend.owned.auth_path()).unwrap();
    let shell = f.backend.owned.root.join("SHELL.yaml");
    let original = "provider: kota-local\nmodel: azure-openai-responses/legacy\neffort: high\n";
    std::fs::write(&shell, original).unwrap();
    let legacy = ModelSelection { provider_id: "azure-openai-responses".into(), model_id: "legacy".into() };
    let errors = [
        selected_model(Some("azure-openai-responses/legacy")).err().unwrap(),
        resolve_model(&f.backend, &legacy, "high").err().unwrap(),
        resolve_local_recruit_config(&f.backend, RecruitSelection { schema: 1,
            provider_id: legacy.provider_id.clone(), model_id: legacy.model_id.clone(),
            thinking_level: Some("high".into()), auto_route: None }).err().unwrap(),
        gate_resolved(&f.backend, &ResolvedRecruit { schema: 1,
            provider_id: legacy.provider_id.clone(), model_id: legacy.model_id.clone(),
            model_arg: "azure-openai-responses/legacy".into(), thinking_level: "high".into(),
            auto_route: None }, &super::query::Query::standard()).err().unwrap(),
    ];
    for error in errors {
        assert_eq!(error.code, "model_not_found");
        assert_eq!(error.provider_id.as_deref(), Some("azure-openai-responses"));
    }
    assert!(f.events().is_empty());
    assert_eq!(std::fs::read(f.backend.owned.auth_path()).unwrap(), auth);
    assert_eq!(std::fs::read_to_string(shell).unwrap(), original);
}

#[test]
fn a_cold_catalog_keeps_unlinked_names_but_only_owned_credentials_are_linked() {
    let f = Fixture::new(json!({}));
    let empty = model_catalog(&f.backend).unwrap();
    assert_eq!(empty.providers.len(), 2);
    assert!(empty.providers.iter().all(|p| !p.linked));
    f.store(json!({"fable":{"type":"api_key","key":"fake-literal"}}));
    let catalog = model_catalog(&f.backend).unwrap();
    assert!(catalog.providers[0].linked);
    assert!(!catalog.providers[1].linked);
    assert_eq!(catalog.providers[1].models[0].name, "Unlinked display name");
    let auth = availability(&f.backend);
    assert_eq!(auth.summary, "ready");
    assert_eq!(auth.providers[0].auth_type, Some(AuthType::ApiKey));
    // The helper claims every provider is configured. It cannot make env-only
    // providers Ready or selected through a direct resolver call.
    assert_eq!(auth.providers[1].status, "not_signed_in");
    let result = resolve_model(
        &f.backend,
        &ModelSelection {
            provider_id: "other".into(),
            model_id: "other-model".into(),
        },
        "off",
    );
    assert_eq!(result.err().unwrap().code, "provider_not_signed_in");
    assert_eq!(f.requests("resolve"), 0);
}

#[test]
fn a_retired_defaults_file_never_selects_a_provider_or_thinking() {
    let f = Fixture::new(json!({"actualThinking":"off"}));
    let retired = f.backend.owned.root.join("defaults.json");
    std::fs::write(&retired, "{broken").unwrap();
    f.store(json!({"fable":{"type":"api_key","key":"fake-literal"}}));
    let result = resolve_local_recruit_config(&f.backend, selection("plain", None)).unwrap();
    assert_eq!(result.model_arg, "fable/plain");
    assert_eq!(result.thinking_level, "off");
    assert!(f.events().iter().any(|event| event["operation"] == "resolve" && event["thinkingLevel"] == "medium"));
    assert_eq!(std::fs::read_to_string(&retired).unwrap(), "{broken");
    f.store(json!({"other":{"type":"api_key","key":"another-fake-literal"}}));
    assert_eq!(resolve_local_recruit_config(&f.backend, selection("plain", Some("off"))).err().unwrap().code, "provider_not_signed_in");
    assert_eq!(std::fs::read_to_string(&retired).unwrap(), "{broken");
    let catalog = model_catalog(&f.backend).unwrap();
    assert_eq!(catalog.providers[0].models[0].name, "Plain display name");
    assert!(!catalog.providers[0].linked);
}

#[test]
fn concrete_thinking_keeps_off_and_native_clamp_is_the_written_result() {
    let f = Fixture::new(json!({"actualThinking":"off"}));
    f.store(json!({"fable":{"type":"api_key","key":"fake-literal"}}));
    let result = resolve_local_recruit_config(&f.backend, selection("plain", Some("max"))).unwrap();
    assert_eq!(result.model_arg, "fable/plain");
    assert_eq!(result.thinking_level, "off");
    assert_eq!(resolve_local_recruit_config(&f.backend, selection("plain", Some("off"))).unwrap().thinking_level, "off");
    assert_eq!(resolve_local_recruit_config(&f.backend, selection("plain", Some("bogus"))).err().unwrap().code, "thinking_invalid");
    for value in [None, Some(""), Some("default")] {
        assert_eq!(selected_model(value).err().unwrap().code, "model_required");
    }
}

#[test]
fn the_gate_is_read_again_after_model_resolution() {
    let f = Fixture::new(json!({"removeBeforeResolve":true}));
    f.store(json!({"fable":{"type":"api_key","key":"fake-literal"}}));
    assert_eq!(
        resolve_local_recruit_config(&f.backend, selection("plain", Some("off")))
            .err()
            .unwrap()
            .code,
        "provider_not_signed_in"
    );
}

#[test]
fn fast_availability_failures_retry_once_but_a_timeout_never_retries() {
    let f = Fixture::new(json!({"failFirstCatalog":true}));
    f.store(json!({"fable":{"type":"api_key","key":"fake-literal"}}));
    assert_eq!(availability(&f.backend).summary, "ready");
    assert_eq!(f.requests("catalog"), 2);
    let mut attempts = 0;
    let auth = availability_retry(|| {
        attempts += 1;
        Err(LocalError::new("auth_timeout"))
    });
    assert_eq!(attempts, 1);
    assert_eq!(auth.summary, "error");
    assert!(auth.providers.is_empty());
    let mut attempts = 0;
    let auth = availability_retry(|| {
        attempts += 1;
        Err(LocalError::new("auth_store_invalid"))
    });
    assert_eq!(attempts, 2);
    assert_eq!(auth.summary, "error");
    assert!(auth.providers.is_empty());
}

#[test]
fn unsafe_or_broken_auth_never_reaches_a_helper_or_becomes_not_signed_in() {
    let f = Fixture::new(json!({}));
    f.store(json!({"fable":{"type":"api_key","key":"!never-run"}}));
    assert_eq!(availability(&f.backend).summary, "error");
    assert!(f.events().is_empty());
    std::fs::write(f.backend.owned.auth_path(), "{broken").unwrap();
    assert_eq!(
        super::credentials::AuthStore::load(&f.backend.owned)
            .err()
            .unwrap()
            .code,
        "auth_store_invalid"
    );
    assert_eq!(availability(&f.backend).summary, "error");
    assert!(f.events().is_empty());
}

#[test]
fn concurrent_metadata_is_one_query_but_sign_out_is_always_fresh() {
    let f = Fixture::new(json!({"delayCatalog":0.2}));
    f.store(json!({"fable":{"type":"api_key","key":"fake-owned"}}));
    std::thread::scope(|scope| {
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(6));
        let workers = (0..6).map(|_| {
            let barrier = barrier.clone(); let backend = &f.backend;
            scope.spawn(move || { barrier.wait(); assert_eq!(availability(backend).summary, "ready"); })
        }).collect::<Vec<_>>();
        for worker in workers { worker.join().unwrap(); }
    });
    assert_eq!(f.requests("catalog"), 1);
    let model = ModelSelection { provider_id:"fable".into(), model_id:"plain".into() };
    resolve_model(&f.backend, &model, "medium").unwrap();
    resolve_model(&f.backend, &model, "medium").unwrap();
    assert_eq!(f.requests("catalog"), 1);
    assert_eq!(f.requests("resolve"), 1);
    f.store(json!({}));
    assert_eq!(availability(&f.backend).summary, "not_ready");
    assert_eq!(resolve_model(&f.backend, &model, "medium").err().unwrap().code, "provider_not_signed_in");
    assert!(model_catalog(&f.backend).unwrap().providers.iter().all(|p| !p.linked));
    assert_eq!(f.requests("catalog"), 1);
    assert_eq!(f.requests("resolve"), 1);
}

#[test]
fn pi_extension_and_model_config_identity_each_invalidate_metadata() {
    use std::io::Write;
    let f = Fixture::new(json!({}));
    model_catalog(&f.backend).unwrap();
    assert_eq!(f.requests("catalog"), 1);
    std::fs::OpenOptions::new().append(true).open(&f.backend.bundled.executable).unwrap().write_all(b"\n# upgraded fixture Pi\n").unwrap();
    model_catalog(&f.backend).unwrap();
    assert_eq!(f.requests("catalog"), 2);
    std::fs::write(&f.backend.bundled.extension, "// changed extension identity\n").unwrap();
    model_catalog(&f.backend).unwrap();
    assert_eq!(f.requests("catalog"), 3);
    std::fs::write(f.backend.owned.agent.join("models.json"), "{}\n").unwrap();
    model_catalog(&f.backend).unwrap();
    assert_eq!(f.requests("catalog"), 4);
}

#[test]
fn a_startup_joining_a_background_catalog_observes_its_credential_store_wait() {
    use std::sync::{Arc, Mutex};
    let f = Fixture::new(json!({"delayCommands":1}));
    f.store(json!({"fable":{"type":"api_key","key":"fake-owned"}}));
    std::fs::create_dir(f.backend.owned.agent.join("auth.json.lock")).unwrap();
    let stages = Arc::new(Mutex::new(Vec::new()));
    let capture = stages.clone();
    let query = super::query::Query::standard().observe(Arc::new(move |stage| capture.lock().unwrap().push(stage)));
    std::thread::scope(|scope| {
        let background = scope.spawn(|| model_catalog(&f.backend).unwrap());
        super::auth_fixture::until(|| f.events().iter().any(|e| e["stage"] == "commands"));
        let model = ModelSelection { provider_id: "fable".into(), model_id: "plain".into() };
        resolve_model_observed(&f.backend, &model, "medium", &query).unwrap();
        background.join().unwrap();
    });
    assert!(stages.lock().unwrap().contains(&"credential_store"));
    assert_eq!(f.requests("catalog"), 1);
}
