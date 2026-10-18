//! Opt-in integration: the real Pi CLI talks only to the scripted loopback API.
//! No account HOME, credentials, ancestors' prompts, telemetry, or model API.
use super::{protocol::fixture, runtime::{Config, LocalRuntime}, *};
use serde_json::{json, Value};
use std::{fs, io::{BufRead, BufReader}, path::{Path, PathBuf}, process::{Child, Command, Stdio},
    sync::{Arc, Mutex, mpsc}, thread, time::{Duration, Instant}};

const SESSION: &str = "11111111-1111-4111-8111-111111111111";
#[path = "pi_burst_tests.rs"]
mod burst_tests;
#[path = "shadow_clone_pi_tests.rs"]
mod shadow_clones;
#[derive(Default)]
struct Capture(Mutex<Vec<LocalNotice>>);
impl Sink for Capture {
    fn notify(&self, notice: &LocalNotice) -> Result<(), String> {
        self.0.lock().unwrap().push(notice.clone()); Ok(())
    }
}
impl Capture {
    fn has(&self, input: &Injection, state: DeliveryState) -> bool {
        self.0.lock().unwrap().iter().any(|n| matches!(n, LocalNotice::Delivery { kota_id, state: s, .. }
            if kota_id == &input.kota_id && *s == state))
    }
    fn event(&self, kind: &str) -> bool {
        self.0.lock().unwrap().iter().any(|n| matches!(n, LocalNotice::Event { event, .. } if event["type"] == kind))
    }
    fn entries_contain(&self, input: &Injection) -> bool {
        self.0.lock().unwrap().iter().any(|n| matches!(n, LocalNotice::Entries { entries, .. }
            if entries.iter().filter_map(custom_receipt).any(|r| r.kota_id == input.kota_id)))
    }
}

struct Fixture { root: PathBuf, extension: PathBuf, api: String, child: Child, http: ureq::Agent }
impl Drop for Fixture {
    fn drop(&mut self) { let _ = self.child.kill(); let _ = self.child.wait(); let _ = fs::remove_dir_all(&self.root); }
}
impl Fixture {
    fn new(steps: Value, compact: bool) -> Self {
        let extension = PathBuf::from(std::env::var("KOTA_LOCAL_TEST_EXTENSION").expect("explicit test extension path"));
        assert!(extension.is_file());
        let package = extension.parent().unwrap();
        let root = std::env::temp_dir().join(format!("kota-local-pi-{}", uuid::Uuid::new_v4()));
        for directory in ["home", "tmp", "config", "workspace/.agents/skills", "sessions"] {
            launch::storage_directory(&root.join(directory)).unwrap();
        }
        fs::write(root.join("workspace/fixture.txt"), "Harmless read-only fixture.\n").unwrap();
        let script = root.join("script.json");
        fs::write(&script, serde_json::to_vec(&steps).unwrap()).unwrap();
        let mut child = Command::new("node").arg(package.join("test/fake-provider/server.mjs"))
            .args(["--port", "0", "--script"]).arg(script)
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let pipe = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::sync_channel(1);
        thread::spawn(move || { let mut line = String::new(); let result = BufReader::new(pipe).read_line(&mut line); let _ = tx.send((result, line)); });
        let mut f = Self { root, extension, api: String::new(), child, http: ureq::AgentBuilder::new().timeout(Duration::from_secs(5)).build() };
        let (result, line) = rx.recv_timeout(Duration::from_secs(10)).unwrap(); result.unwrap();
        let ready: Value = serde_json::from_str(&line).unwrap();
        f.api = ready["baseUrl"].as_str().unwrap().trim_end_matches("/v1").to_string();
        assert!(f.api.starts_with("http://127.0.0.1:"));
        let models = json!({"providers":{"kota-fake":{
            "baseUrl":format!("{}/v1",f.api), "api":"openai-completions", "apiKey":"fixture-not-a-secret",
            "models":[{"id":"fixture","name":"Synthetic fixture","reasoning":false,"input":["text","image"],
                "contextWindow":16384,"maxTokens":2048,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0}}]
        }}});
        fs::write(f.root.join("config/models.json"), serde_json::to_vec(&models).unwrap()).unwrap();
        fs::write(f.root.join("config/settings.json"), serde_json::to_vec(&json!({
            "steeringMode":"all","followUpMode":"all","cacheWarming":"off",
            "enableInstallTelemetry":false,"enableAnalytics":false,"defaultTools":["read","bash"],"retry":{"enabled":false},
            "compaction":{"enabled":compact,"keepRecentTokens":if compact {0} else {256},"reserveTokens":if compact {16300} else {1024}}
        })).unwrap()).unwrap();
        f
    }
    fn config(&self, epoch: u64, session: &str) -> Config { self.config_for(epoch, session, ("kota-fake", "fixture"), None) }
    fn config_for(&self, epoch: u64, session: &str, model: (&str, &str), route: Option<&str>) -> Config {
        let root = PathBuf::from(std::env::var_os("KOTA_LOCAL_TEST_PI_RUNTIME_ROOT").expect("explicit pinned Pi runtime root"));
        let binary = bundle::BundledPi::from_root(&root,&self.extension).unwrap().executable;
        let cwd = self.root.join("workspace");
        let mut command = Command::new(binary);
        command.current_dir(&cwd).args(launch::arguments(&cwd, &self.extension, &self.root.join("sessions"), session,
            Some(&format!("{}/{}", model.0, model.1)), Some("off"), route)).args(["--offline", "--no-context-files"])
            .env_clear().env("PATH", std::env::var_os("PATH").unwrap_or_default()).env("HOME", self.root.join("home"))
            .env("PI_CODING_AGENT_DIR", self.root.join("config")).env("PI_CODING_AGENT_SESSION_DIR", self.root.join("sessions"))
            .env("TMPDIR", self.root.join("tmp")).env("PI_OFFLINE", "1").env("PI_SKIP_VERSION_CHECK", "1")
            .env("PI_TELEMETRY", "0").env("NO_COLOR", "1");
        Config { scope: LocalScope { project_root: cwd, agent_id: "agent-fable-copper".into(), session_id: session.into(), epoch },
            ledger: self.root.join("ledger.jsonl"), command, expected_model:Some(models::ModelSelection { provider_id:model.0.into(),model_id:model.1.into() }), current: Arc::new(|| true) }
    }
    fn requests(&self) -> usize {
        let state: Value = self.http.get(&format!("{}/__test/state",self.api)).call().unwrap().into_json().unwrap();
        state["requests"].as_array().unwrap().len()
    }
    fn release(&self, index: usize) {
        let value: Value = self.http.post(&format!("{}/__test/release/{index}",self.api)).call().unwrap().into_json().unwrap();
        assert_eq!(value["released"], true);
    }
}
fn until(mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(25);
    while !f() { assert!(Instant::now() < end, "real Pi fixture timed out"); thread::sleep(Duration::from_millis(5)); }
}
fn settle(runtime: &LocalRuntime, sink: &Capture, input: &Injection) {
    until(|| sink.has(input, DeliveryState::Consumed) && sink.entries_contain(input));
    until(|| runtime.snapshot().state == OnlineState::Ready && sink.event("agent_settled"));
}

#[test]
#[ignore = "requires real Pi and KOTA_LOCAL_TEST_EXTENSION; loopback fixture only"]
fn real_pi_sources_reload_idempotency_and_fresh_are_owned_by_the_rust_ledger() {
    let f = Fixture::new(json!((0..9).map(|_| json!({"type":"text","text":"Synthetic final reply."})).collect::<Vec<_>>()), false);
    let sink = Arc::new(Capture::default());
    let runtime = LocalRuntime::start(f.config(1, SESSION), sink.clone()).unwrap();
    runtime.activate(false).unwrap();
    let mut inputs = Vec::new();
    for (index, source) in ["kota.user","kota.lm","kota.bus","kota.bbs","kota.ember","kota.dream","kota.bartender"].iter().enumerate() {
        let mut input = fixture(&format!("input-{index}")); input.custom_type = source.to_string();
        runtime.submit(input.clone()).unwrap(); settle(&runtime, &sink, &input); inputs.push(input);
    }
    assert_eq!(f.requests(), 7);
    assert!(!sink.has(&inputs[0], DeliveryState::Durable), "live entries are not disk reload evidence");
    if let Ok(directory) = std::env::var("KOTA_LOCAL_FIXTURE_DIR") {
        let directory = Path::new(&directory); fs::create_dir_all(directory).unwrap();
        let entries: Vec<_> = sink.0.lock().unwrap().iter().filter_map(|notice| {
            if let LocalNotice::Entries { entries, .. } = notice { Some(entries.clone()) } else { None }
        }).flatten().collect();
        fs::write(directory.join("real-pi-entries.json"), serde_json::to_vec_pretty(&entries).unwrap()).unwrap();
    }
    runtime.close().unwrap();
    let restored = LocalRuntime::start(f.config(2, SESSION), sink.clone()).unwrap();
    restored.activate(false).unwrap();
    for input in &inputs { until(|| sink.has(input, DeliveryState::Durable)); restored.submit(input.clone()).unwrap(); }
    restored.rpc(json!({"type":"get_state"})).unwrap();
    assert_eq!(f.requests(), 7, "a repeated identity must not call the model again");
    restored.close().unwrap();
    let fresh_id = "22222222-2222-4222-8222-222222222222";
    let fresh = LocalRuntime::start(f.config(3, fresh_id), sink.clone()).unwrap();
    fresh.activate(true).unwrap();
    let input = fixture("fresh-input"); fresh.submit(input.clone()).unwrap(); settle(&fresh, &sink, &input);
    assert_eq!(fresh.snapshot().scope.session_id, fresh_id); assert_eq!(f.requests(), 8);
    fresh.close().unwrap();
}

#[test]
#[ignore = "requires real Pi and KOTA_LOCAL_TEST_EXTENSION; loopback fixture only"]
fn real_pi_crash_before_first_assistant_persistence_abandons_missing_input_without_replay() {
    let f = Fixture::new(json!([
        {"type":"text","text":"lost response","holdBeforeHeaders":true},
        {"type":"text","text":"Synthetic recovered response."}
    ]), false);
    let sink = Arc::new(Capture::default());
    let runtime = LocalRuntime::start(f.config(1, SESSION), sink.clone()).unwrap(); runtime.activate(false).unwrap();
    let input = fixture("lost-first-turn"); runtime.submit(input.clone()).unwrap();
    until(|| sink.has(&input, DeliveryState::Consumed) && f.requests() == 1);
    assert!(!sink.has(&input, DeliveryState::Durable));
    let pid = runtime.snapshot().child_pid.unwrap();
    unsafe { assert_eq!(libc::kill(pid as i32, libc::SIGKILL), 0); }
    until(|| runtime.snapshot().state == OnlineState::Error);
    // Retain the failed registry handle: an exited owner must release its
    // file lock without requiring the old App to close its error snapshot.
    until(|| runtime.inner_join_finished_for_test());
    let recovered = LocalRuntime::start(f.config(2, SESSION), sink.clone()).unwrap();
    assert_eq!(f.requests(), 1); recovered.activate(false).unwrap();
    assert!(sink.has(&input, DeliveryState::Abandoned));
    recovered.rpc(json!({"type":"get_state"})).unwrap(); assert_eq!(f.requests(), 1);
    let next = fixture("after-crash-new-input"); recovered.submit(next.clone()).unwrap();
    until(|| f.requests() == 2); settle(&recovered, &sink, &next);
    recovered.close().unwrap(); runtime.close().unwrap();
}

#[test]
#[ignore = "requires real Pi and KOTA_LOCAL_TEST_EXTENSION; loopback fixture only"]
fn real_pi_stop_during_compaction_retains_held_and_consumed_facts_then_accepts_new_input() {
    let f = Fixture::new(json!([
        {"type":"tool","name":"read","arguments":{"path":"fixture.txt"},"chunkSize":8,"holdAfterChunks":1},
        {"type":"text","text":"Synthetic compaction summary.","holdBeforeHeaders":true},
        {"type":"text","text":"Response to the retained input."},
        {"type":"text","text":"Later summary."},
        {"type":"text","text":"Response to the new input."},
        {"type":"text","text":"Final response."}
    ]), true);
    let sink = Arc::new(Capture::default());
    let runtime = LocalRuntime::start(f.config(1, SESSION), sink.clone()).unwrap(); runtime.activate(false).unwrap();
    runtime.submit(fixture("seed-stop")).unwrap(); until(|| f.requests() == 1);
    let drained = fixture("stop-drained"); runtime.submit(drained.clone()).unwrap();
    until(|| sink.has(&drained, DeliveryState::Accepted)); f.release(1);
    until(|| f.requests() == 2 && runtime.snapshot().compacting);
    let held = fixture("stop-held"); runtime.submit(held.clone()).unwrap();
    until(|| sink.has(&held, DeliveryState::Accepted));
    assert!(!sink.has(&held, DeliveryState::Consumed));
    runtime.rpc(json!({"type":"abort"})).unwrap();
    until(|| runtime.snapshot().queued == 0 && runtime.snapshot().state == OnlineState::Ready);
    assert!(!sink.has(&held, DeliveryState::Cleared));
    assert!(sink.has(&held, DeliveryState::Consumed));
    assert!(sink.has(&drained, DeliveryState::Consumed), "compaction abort can consume an already handed input before clear_queue");
    let next = fixture("after-stop"); runtime.submit(next.clone()).unwrap(); settle(&runtime, &sink, &next);
    assert!(!sink.has(&next, DeliveryState::Cleared));
    let events=sink.0.lock().unwrap();
    assert!(!events.iter().any(|n| matches!(n,LocalNotice::Event {event,..} if event["message"].as_str().is_some_and(|s|s.starts_with(protocol::QUEUE_CONTROL_CODEC)))));
    drop(events);
    runtime.close().unwrap();
}

#[test]
#[ignore = "requires real Pi and KOTA_LOCAL_TEST_EXTENSION; loopback fixture only"]
fn real_pi_precise_edit_requeues_at_tail_and_consumes_the_next_batch_once() {
    let f = Fixture::new(json!([
        {"type":"text","text":"First response.","holdBeforeHeaders":true},
        {"type":"text","text":"Response to the waiting batch."}
    ]), false);
    let sink = Arc::new(Capture::default());
    let runtime = LocalRuntime::start(f.config(1, SESSION), sink.clone()).unwrap(); runtime.activate(false).unwrap();
    runtime.submit(fixture("edit-seed")).unwrap(); until(|| f.requests() == 1);
    let edited = fixture("edit-original"); let other = fixture("edit-other");
    runtime.submit(edited.clone()).unwrap(); runtime.submit(other.clone()).unwrap();
    until(|| sink.has(&edited, DeliveryState::Accepted) && sink.has(&other, DeliveryState::Accepted));
    let removed = runtime.queue_action(json!({"type":"queue_cancel","kotaId":edited.kota_id,"edit":true})).unwrap();
    assert_eq!(removed["cancelled"], true);
    assert!(runtime.retry(edited.kota_id.clone()).is_err());
    runtime.queue_action(json!({"type":"queue_requeue","kotaId":edited.kota_id,"editToken":removed["editToken"]})).unwrap();
    until(|| sink.0.lock().unwrap().iter().filter(|n| matches!(n, LocalNotice::Delivery { kota_id, state: DeliveryState::Accepted, .. } if kota_id == &edited.kota_id)).count() == 2);
    f.release(1);
    settle(&runtime, &sink, &edited); settle(&runtime, &sink, &other);
    assert_eq!(f.requests(), 2);
    let ids: Vec<_> = sink.0.lock().unwrap().iter().filter_map(|notice| {
        if let LocalNotice::Event { event, .. } = notice { custom_receipt(event).map(|r| r.kota_id) } else { None }
    }).filter(|id| id == &edited.kota_id || id == &other.kota_id).collect();
    assert_eq!(ids, vec![other.kota_id.clone(), edited.kota_id.clone()]);
    assert_eq!(runtime.queue_action(json!({"type":"queue_cancel","kotaId":edited.kota_id})).unwrap()["reason"], "too_late");
    runtime.close().unwrap();
}

#[test]
#[ignore = "requires real Pi and KOTA_LOCAL_TEST_EXTENSION; loopback fixture only"]
fn real_pi_workbench_retains_completed_run_without_changing_projection_or_delivery() {
    let f = Fixture::new(json!([
        {"type":"tool","name":"read","arguments":{"path":"fixture.txt"}},
        {"type":"long","text":"Synthetic streaming reply. ","repeat":500,"chunkSize":32,"holdAfterChunks":5},
        {"type":"text","text":"Next turn.","holdBeforeHeaders":true}
    ]), false);
    let sink = Arc::new(Capture::default());
    let runtime = LocalRuntime::start(f.config(1, SESSION), sink.clone()).unwrap();
    runtime.activate(false).unwrap();
    let first = fixture("workbench-first"); runtime.submit(first.clone()).unwrap();
    until(|| f.requests() == 2 && runtime.workbench(None).blocks.as_ref().is_some_and(|b|
        b.iter().any(|b| b.kind == "progress" && !b.text.is_empty())));
    let view = runtime.workbench(None);
    let blocks = view.blocks.unwrap();
    assert_eq!(blocks.iter().filter(|b| b.kind == "tool").count(), 1);
    assert!(blocks.iter().any(|b| b.kind == "tool" && !b.active && b.text.contains("Harmless")));
    let ids: std::collections::BTreeSet<_> = blocks.iter().map(|b| &b.id).collect();
    assert_eq!(ids.len(), blocks.len());
    assert!(view.started_at.is_some());
    assert!(sink.event("message_update"), "original Violet sink still sees native events");
    let cursor = super::WorkbenchCursor { scope: view.snapshot.scope, revision: view.revision, queue_revision: Some(view.queue_revision) };
    assert!(runtime.workbench(Some(&cursor)).queue.is_none());
    f.release(2);
    until(|| {
        let snapshot = runtime.snapshot();
        assert_ne!(snapshot.state, OnlineState::Error, "workbench stream failed: {:?}", snapshot.error);
        snapshot.state == OnlineState::Ready && sink.event("agent_settled")
    });
    assert!(sink.has(&first, DeliveryState::Consumed));
    let completed = runtime.workbench(None);
    let completed_run = completed.run_id;
    assert!(completed.started_at.is_none());
    let completed = completed.blocks.unwrap();
    assert!(!completed.is_empty());
    assert!(completed.iter().all(|block| !block.active));
    assert!(completed.iter().any(|block| block.kind == "end" && block.text.contains("Synthetic streaming reply")));
    let second = fixture("workbench-second"); runtime.submit(second.clone()).unwrap();
    until(|| f.requests() == 3);
    // The HTTP request can arrive before the owner processes agent_start.
    until(|| {
        let view = runtime.workbench(None);
        view.run_id > completed_run && view.blocks.unwrap().iter().all(|b| !b.text.contains("Synthetic streaming reply"))
    });
    runtime.rpc(json!({"type":"abort"})).unwrap();
    until(|| runtime.workbench(None).started_at.is_none());
    assert!(sink.has(&first, DeliveryState::Consumed));
    runtime.close().unwrap();
}

#[test]
#[ignore = "requires real Pi and KOTA_LOCAL_TEST_EXTENSION; loopback fixture and fake Jev only"]
fn real_pi_auto_route_facts_reach_only_the_workbench() {
    let f = Fixture::new(json!([{"type":"error","status":503,"message":"Fixture temporary failure"},
        {"type":"text","text":"Routed reply."}]), false);
    let settings_path = f.root.join("config/settings.json");
    let mut settings: Value = serde_json::from_slice(&fs::read(&settings_path).unwrap()).unwrap();
    settings["retry"] = json!({"enabled":true,"maxRetries":1,"baseDelayMs":1});
    fs::write(&settings_path, serde_json::to_vec(&settings).unwrap()).unwrap();
    let package = f.extension.parent().unwrap().to_path_buf();
    let script = f.root.join("jev.json");
    fs::write(&script, serde_json::to_vec(&json!([{"serious":0.9,"casual":0.1}])).unwrap()).unwrap();
    let mut jev = Command::new("node").arg(package.join("test/fake-provider/jev.mjs")).arg("--script").arg(&script)
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut line = String::new();
    BufReader::new(jev.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let jev_url = serde_json::from_str::<Value>(&line).unwrap()["baseUrl"].as_str().unwrap().to_owned();
    // Same layout as production: the router's files sit next to the owned agent directory.
    let models = f.root.join("config/models.json");
    let mut config: Value = serde_json::from_slice(&fs::read(&models).unwrap()).unwrap();
    config["providers"]["typesafe"] = json!({"baseUrl": jev_url});
    fs::write(&models, serde_json::to_vec(&config).unwrap()).unwrap();
    fs::create_dir_all(f.root.join("jev")).unwrap();
    fs::write(f.root.join("jev/keys.json"), serde_json::to_vec(&json!({"schema":1,"provider":"typesafe",
        "keys":{"typesafe":{"key":"fake-jev-key","keyId":"key-1"}}})).unwrap()).unwrap();
    let mut pair = auto_route::fixture_group();
    for model in &mut pair.models { model.provider_id = "kota-fake".into(); model.model_id = "fixture".into(); }
    let config = f.config_for(1, SESSION, ("kota", "auto"), Some(&pair.launch_json()));
    let sink = Arc::new(Capture::default());
    let runtime = LocalRuntime::start(config, sink.clone()).unwrap();
    runtime.activate(false).unwrap();
    let input = fixture("routed-input");
    runtime.submit(input.clone()).unwrap();
    settle(&runtime, &sink, &input);
    let view = runtime.workbench(None);
    let route = view.auto_route.expect("kota/auto has a route view");
    assert_eq!(route.current_model.as_deref(), Some("model-1"));
    let turn = route.turn.unwrap();
    assert_eq!((turn.selected_model.as_str(), turn.route_model.as_str(), turn.fallback, turn.provider.as_str(), turn.model.as_str()), ("model-1", "model-1", false, "kota-fake", "fixture"));
    assert!(!sink.0.lock().unwrap().iter().any(|n| matches!(n, LocalNotice::Event { event, .. }
        if event["message"].as_str().is_some_and(|s| s.starts_with(protocol::ROUTE_CODEC)))), "route facts are never forwarded");
    assert_eq!(f.requests(), 2, "native retry recovers on the same physical model");
    assert_eq!(route.events.len(), 2, "one Jev result and one physical failure; retry adds no selection");
    assert!(matches!(&route.events[0].data, workbench::RouteEventData::ModelError { message, .. } if message.contains("Fixture temporary failure")));
    assert!(matches!(&route.events[1].data, workbench::RouteEventData::Selection { .. }));
    assert!(chrono::DateTime::parse_from_rfc3339(&route.events[0].occurred_at).is_ok());
    assert!(route.error.is_none(), "a recovered attempt is not a terminal route error");
    runtime.close().unwrap();
    let _ = jev.kill(); let _ = jev.wait();
}
