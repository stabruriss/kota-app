use super::{
    auth,
    auth_fixture::{until, Fixture},
    auth_protocol::{self, AuthReply, AuthStart, Dialog},
    credentials::AuthType,
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

fn request(provider: &str) -> AuthStart {
    serde_json::from_value(json!({"operation":"login","providerId":provider,"authType":"api_key"}))
        .unwrap()
}
type Events = Arc<Mutex<Vec<Value>>>;
fn begin(f: &Fixture, provider: &str) -> (String, Events, Arc<AtomicUsize>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let changes = Arc::new(AtomicUsize::new(0));
    let changed = changes.clone();
    let id = auth::start(
        f.backend.clone(),
        request(provider),
        Arc::new(move |e| capture.lock().unwrap().push(e)),
        Arc::new(move || {
            changed.fetch_add(1, Ordering::SeqCst);
        }),
    )
    .unwrap()
    .operation_id;
    (id, events, changes)
}
fn has(events: &Events, kind: &str) -> bool {
    events.lock().unwrap().iter().any(|e| e["event"] == kind)
}
fn terminal(events: &Events) -> Value {
    events
        .lock()
        .unwrap()
        .iter()
        .find(|e| e["event"] == "result")
        .unwrap()
        .clone()
}
fn reply(id: &str, value: &str) -> AuthReply {
    AuthReply {
        operation_id: id.into(),
        prompt_id: "prompt-fixture".into(),
        value: Some(value.into()),
        cancelled: None,
    }
}

#[test]
fn the_helper_has_an_exact_command_precheck_and_never_falls_into_a_model_prompt() {
    let f = Fixture::new(json!({"noCommand":true}));
    let error = f
        .backend
        .query(json!({"operation":"catalog"}))
        .err()
        .unwrap();
    assert_eq!(error.code, "local_auth_extension_unavailable");
    assert_eq!(f.requests("catalog"), 0);
    assert!(!f.events().is_empty());
}

#[test]
fn key_validation_keeps_the_dialog_pending_and_reply_uses_its_native_id() {
    let f = Fixture::new(json!({}));
    let (id, events, changes) = begin(&f, "fable");
    until(|| has(&events, "prompt"));
    assert_eq!(
        auth::respond(reply(&id, "!never-execute"))
            .unwrap_err()
            .code,
        "credential_invalid"
    );
    assert!(has(&events, "prompt"));
    assert!(!has(&events, "result"));
    assert!(f.events().iter().all(|e| e["stage"] != "reply"));
    assert!(auth::respond(AuthReply {
        prompt_id: "wrong-step".into(),
        ..reply(&id, "fake-literal")
    })
    .is_err());
    auth::respond(reply(&id, "fake-literal")).unwrap();
    until(|| has(&events, "result") && changes.load(Ordering::SeqCst) == 1);
    let result = terminal(&events);
    assert_eq!(result["commit"], "confirmed");
    assert_eq!(result["outcome"], "succeeded");
    assert!(result.get("credential").is_none() && result.get("statuses").is_none());
    assert!(!serde_json::to_string(&*events.lock().unwrap())
        .unwrap()
        .contains("NEVER_FORWARD"));
    assert!(f
        .events()
        .iter()
        .any(|e| e["nativeId"] == format!("dialog-{id}")));
    assert!(auth::respond(reply(&id, "fake-literal")).is_err());
}

#[test]
fn an_oauth_secret_is_not_treated_as_an_api_key_field() {
    let value = json!({"schema":1,"channel":"kota.auth","operationId":"operation","event":"prompt","providerId":"fable",
        "authType":"oauth","promptId":"step","prompt":{"type":"secret","message":"Native OAuth secret"}});
    let record =
        json!({"method":"input","id":"native","title":format!("{}{}",auth_protocol::CODEC,value)});
    let event =
        auth_protocol::decode(&record, "operation", Some("fable"), Some(AuthType::Oauth)).unwrap();
    Dialog {
        native_id: "native".into(),
        event,
    }
    .validate_reply(&AuthReply {
        operation_id: "operation".into(),
        prompt_id: "step".into(),
        value: Some("!opaque-${TOKEN}".into()),
        cancelled: None,
    })
    .unwrap();
}

#[test]
fn select_ids_and_operation_provider_type_are_bound_without_forwarding_unknown_fields() {
    let value = json!({"schema":1,"channel":"kota.auth","operationId":"operation","event":"prompt","providerId":"fable",
        "authType":"api_key","promptId":"step","credential":{"key":"NEVER_FORWARD"},"prompt":{"type":"select","message":"Choose a native option",
        "options":[{"id":"profile","label":"Profile"},{"id":"chain","label":"Chain"}]}});
    let record = json!({"method":"select","id":"native","title":format!("{}{}",auth_protocol::CODEC,value),"options":["profile","chain"]});
    let event =
        auth_protocol::decode(&record, "operation", Some("fable"), Some(AuthType::ApiKey)).unwrap();
    assert!(event.get("credential").is_none());
    assert!(auth_protocol::decode(
        &record,
        "different-operation",
        Some("fable"),
        Some(AuthType::ApiKey)
    )
    .is_err());
    assert!(auth_protocol::decode(
        &record,
        "operation",
        Some("different-provider"),
        Some(AuthType::ApiKey)
    )
    .is_err());
    assert!(
        auth_protocol::decode(&record, "operation", Some("fable"), Some(AuthType::Oauth)).is_err()
    );
    let dialog = Dialog {
        native_id: "native".into(),
        event,
    };
    let mut answer = reply("operation", "Profile");
    answer.prompt_id = "step".into();
    assert!(dialog.validate_reply(&answer).is_err());
    answer.value = Some("profile".into());
    dialog.validate_reply(&answer).unwrap();
}

#[test]
fn provider_ownership_is_exclusive_and_cancel_closes_the_exact_prompt() {
    let f = Fixture::new(json!({}));
    let (id, events, changes) = begin(&f, "fable");
    until(|| has(&events, "prompt"));
    assert_eq!(
        auth::start(
            f.backend.clone(),
            request("fable"),
            Arc::new(|_| {}),
            Arc::new(|| {})
        )
        .err()
        .unwrap()
        .code,
        "busy-auth"
    );
    auth::cancel(&id).unwrap();
    until(|| has(&events, "result") && changes.load(Ordering::SeqCst) == 1);
    let result = terminal(&events);
    assert_eq!(result["commit"], "not-written");
    assert_eq!(result["outcome"], "cancelled");
    assert!(has(&events, "prompt_closed"));
    assert!(f.backend.owned.provider_lock("fable").is_ok());
}

#[test]
fn cancellation_can_interrupt_startup_before_any_auth_command_is_submitted() {
    let f = Fixture::new(json!({"delayCommands":true}));
    let (id, events, changes) = begin(&f, "fable");
    until(|| !f.events().is_empty());
    let at = std::time::Instant::now();
    auth::cancel(&id).unwrap();
    until(|| has(&events, "result") && changes.load(Ordering::SeqCst) == 1);
    assert!(at.elapsed() < std::time::Duration::from_secs(1));
    assert_eq!(terminal(&events)["commit"], "not-written");
    assert_eq!(terminal(&events)["outcome"], "cancelled");
    assert_eq!(f.requests("login"), 0);
}

#[test]
fn helper_exit_after_submission_is_unknown_and_never_automatically_retried() {
    let f = Fixture::new(json!({"exitAfterCommand":true}));
    let (_, events, changes) = begin(&f, "fable");
    until(|| has(&events, "result") && changes.load(Ordering::SeqCst) == 1);
    assert_eq!(terminal(&events)["commit"], "unknown");
    assert_eq!(terminal(&events)["outcome"], "indeterminate");
    assert_eq!(f.requests("login"), 1);
}

#[test]
fn an_earlier_provider_expiry_uses_independent_auth_cancellation() {
    let f = Fixture::new(json!({"expiry":0.05}));
    let (_, events, changes) = begin(&f, "fable");
    until(|| has(&events, "result") && changes.load(Ordering::SeqCst) == 1);
    assert_eq!(terminal(&events)["commit"], "not-written");
    assert_eq!(terminal(&events)["outcome"], "timeout");
    assert_eq!(f.requests("cancel"), 1);
    assert!(has(&events, "prompt_closed"));
}

#[test]
fn azure_login_requires_its_controlled_url_and_does_not_accept_a_free_env_object() {
    assert!(serde_json::from_value::<AuthStart>(json!({"operation":"login","providerId":"azure","authType":"api_key","env":{"AZURE_OPENAI_BASE_URL":"http://127.0.0.1:1"}})).is_err());
    let mut azure = request("azure");
    assert_eq!(azure.validate().unwrap_err().code, "azure_resource_missing");
    azure.resource_url = Some("http://127.0.0.1:1/fixture".into());
    azure.validate().unwrap();
    let mut other = request("fable");
    other.resource_url = azure.resource_url;
    assert!(other.validate().is_err());
}

#[test]
fn query_timeout_names_get_commands_and_cancellation_releases_the_helper() {
    use super::query::Query;
    use std::time::{Duration, Instant};
    let f = Fixture::new(json!({"delayCommands":true}));
    let error = f.backend.query_observed(json!({"operation":"catalog"}), &Query::new(Duration::from_secs(2))).unwrap_err();
    assert_eq!(error.code, "auth_timeout");
    assert_eq!(error.stage.as_deref(), Some("helper_ready"));
    assert_eq!(f.requests("catalog"), 0);
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = cancel.clone();
    let query = Query::observed(Instant::now() + Duration::from_secs(30), Arc::new(move || flag.load(Ordering::SeqCst)), Arc::new(|_| {}));
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| f.backend.query_observed(json!({"operation":"catalog"}), &query));
        until(|| f.events().iter().filter(|e| e["stage"] == "commands").count() == 2);
        let cancelled_at = Instant::now(); cancel.store(true, Ordering::SeqCst);
        let error = worker.join().unwrap().unwrap_err();
        assert_eq!(error.code, "startup_cancelled");
        assert_eq!(error.stage.as_deref(), Some("helper_ready"));
        assert!(cancelled_at.elapsed() < Duration::from_secs(2));
    });
}

#[test]
fn configuration_lock_contention_is_distinct_and_can_be_cancelled_before_spawn() {
    use super::{owned::FileLock, query::Query};
    use std::time::{Duration, Instant};
    let f = Fixture::new(json!({}));
    let _lock = FileLock::acquire(&f.backend.owned.root.join(".config.lock"), "local_config_busy").unwrap();
    let error = f.backend.query_observed(json!({"operation":"catalog"}), &Query::new(Duration::from_millis(50))).unwrap_err();
    assert_eq!(error.code, "local_config_busy");
    assert_eq!(error.stage.as_deref(), Some("configuration_lock"));
    assert!(f.events().is_empty());
    let query = Query::observed(Instant::now() + Duration::from_secs(30), Arc::new(|| true), Arc::new(|_| {}));
    assert_eq!(f.backend.query_observed(json!({"operation":"catalog"}), &query).unwrap_err().code, "startup_cancelled");
    assert!(f.events().is_empty());
}

#[test]
fn queries_are_offline_but_login_keeps_native_networking() {
    let f = Fixture::new(json!({}));
    f.backend.query(json!({"operation":"catalog"})).unwrap();
    assert_eq!(f.events().iter().find(|e| e["stage"] == "spawn").unwrap()["offline"], true);
    let (id, events, _) = begin(&f, "fable");
    until(|| has(&events, "prompt"));
    assert_eq!(f.events().iter().filter(|e| e["stage"] == "spawn").last().unwrap()["offline"], false);
    super::auth::cancel(&id).unwrap();
    until(|| has(&events, "result"));
}

#[test]
fn a_pi_credential_lock_is_waited_on_and_only_reports_busy_after_readiness_expires() {
    let f = Fixture::new(json!({"delayCommands":true}));
    let lock = f.backend.owned.agent.join("auth.json.lock");
    std::fs::create_dir(&lock).unwrap();
    let started = std::time::Instant::now();
    let error = f.backend.query_observed(json!({"operation":"catalog"}), &super::query::Query::new(std::time::Duration::from_millis(500))).unwrap_err();
    assert_eq!(error.code, "auth_store_busy");
    assert_eq!(error.stage.as_deref(), Some("credential_store"));
    assert!(started.elapsed() >= std::time::Duration::from_millis(500));
    assert!(lock.is_dir());
}

#[test]
fn helper_readiness_and_command_each_receive_their_own_budget() {
    let f = Fixture::new(json!({"delayCommands":2.5,"delayCatalog":2.5}));
    f.backend.query_observed(json!({"operation":"catalog"}), &super::query::Query::new(std::time::Duration::from_secs(4))).unwrap();
}

#[test]
fn a_storage_failure_is_busy_only_when_the_pi_lock_is_present() {
    for held in [false, true] {
        let f = Fixture::new(json!({"storageFailed":true}));
        if held { std::fs::create_dir(f.backend.owned.agent.join("auth.json.lock")).unwrap(); }
        let error = f.backend.query(json!({"operation":"catalog"})).unwrap_err();
        assert_eq!(error.code, if held { "auth_store_busy" } else { "auth_store_read_failed" });
        assert_eq!(error.stage.as_deref(), Some("model_catalog"));
    }
}
