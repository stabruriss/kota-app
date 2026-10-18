use super::*;
use crate::local_rpc::auth_fixture::Fixture;
use std::{io::{Read, Write}, net::TcpListener, sync::{Arc, Mutex}, thread};

// Loopback-only fake gateway; it has no upstream or model endpoint.
struct Gateway { port: u16, requests: Arc<Mutex<Vec<String>>>, worker: Option<thread::JoinHandle<()>> }
impl Gateway {
    fn new(replies: Vec<(u16, Value)>) -> Self { Self::with_header(replies, String::new()) }
    fn with_header(replies: Vec<(u16, Value)>, header: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new())); let seen = requests.clone();
        let worker = thread::spawn(move || {
            for (status, body) in replies {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() { Ok((s, _)) => break s, Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "fake gateway was not called"); thread::sleep(Duration::from_millis(2));
                    }, Err(e) => panic!("{e}") }
                };
                // macOS inherits the listener's nonblocking flag on accept.
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") { let mut byte = [0]; stream.read_exact(&mut byte).unwrap(); request.push(byte[0]); }
                seen.lock().unwrap().push(String::from_utf8(request).unwrap());
                let body = body.to_string();
                write!(stream, "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{header}\r\n{body}", body.len()).unwrap();
            }
        });
        Self { port, requests, worker: Some(worker) }
    }
    fn settings(&self) -> Settings { Settings { port:self.port, ..Settings::default() } }
    fn finish(mut self) -> Vec<String> { self.worker.take().unwrap().join().unwrap(); self.requests.lock().unwrap().clone() }
}
fn fake_key(backend: &Backend, key: &str) -> LocalResult<()> {
    // This writer is only a test substitute; production writes through native Pi login.
    let mut value = read_json(&backend.owned.auth_path(), "fixture")?.unwrap();
    value[PROVIDER] = json!({"type":"api_key","key":key});
    write_json(&backend.owned.auth_path(), &value, "fixture")
}
fn list(models: Value) -> Value { json!({"data":models}) }
fn mapped(v: Value) -> Value { model(serde_json::from_value(v).unwrap()) }

#[test]
fn defaults_are_local_one_port_and_read_does_not_initialize_or_discover() {
    let root = std::env::temp_dir().join(format!("magpie-read-only-{}", uuid::Uuid::new_v4()));
    let owned = OwnedDirectory::new(&root);
    let state = read(&owned).unwrap();
    assert_eq!(state.settings.address().unwrap(), "http://127.0.0.1:3425");
    assert_eq!(state.default_key, DEFAULT_KEY); assert!(!state.linked); assert!(!owned.root.exists());
    let mut settings = Settings { ip:"192.0.2.8".into(), port:3456, mode:Mode::Lan };
    assert_eq!(settings.address().unwrap(), "http://192.0.2.8:3456");
    settings.mode = Mode::Local; assert_eq!(settings.address().unwrap(), "http://127.0.0.1:3456");
    settings.mode = Mode::Lan; settings.ip = "::1".into(); assert_eq!(settings.address().unwrap(), "http://[::1]:3456");
}
#[test]
fn official_mapping_uses_native_apis_and_preserves_model_ids() {
    let m = mapped(json!({"id":"vendor/nested/gpt-5.5","display_name":"GPT-5.5","native_endpoints":["/v1/responses","/v1/messages"],
        "context_window":4096,"max_output_tokens":8192,"modalities":{"input":["text","image"]},
        "supported_reasoning_levels":[{"effort":"none"},{"effort":"high"},{"effort":"xhigh"}]}));
    assert_eq!(m["id"], "vendor/nested/gpt-5.5"); assert_eq!(m["api"], "openai-responses");
    assert_eq!(m["maxTokens"],4096); assert_eq!(m["contextWindow"],4096); assert_eq!(m["input"],json!(["text","image"]));
    assert_eq!(m["thinkingLevelMap"]["off"],"none"); assert_eq!(m["thinkingLevelMap"]["xhigh"],"xhigh");
    assert!(m["thinkingLevelMap"]["minimal"].is_null()); assert_eq!(m["promptCache"],json!({"short":300}));
    assert!(m.get("cost").is_none());
}
#[test]
fn missing_metadata_is_not_an_eligibility_filter() {
    let m = mapped(json!({"id":"bridge/claude-sonnet-4-6"}));
    assert_eq!(m["name"], "bridge/claude-sonnet-4-6"); assert_eq!(m["reasoning"],false);
    for field in ["api","baseUrl","contextWindow","maxTokens","cost","thinkingLevelMap","input","promptCache"] { assert!(m.get(field).is_none(), "{field}"); }
    assert_eq!(mapped(json!({"id":"unknown/plain","max_output_tokens":32000}))["maxTokens"],32000);
}
#[test]
fn claude_compatibility_matches_official_examples_without_model_rejection() {
    for id in ["anthropic/claude-opus-4-6","anthropic.claude-sonnet-4.6-v1","relay/opus-5.5","claude-5-sonnet"] { assert_eq!(claude_flags(id),(true,true),"{id}"); }
    for id in ["claude-3-7-sonnet","claude-sonnet-4-5","claude-haiku-4","claude-unversioned"] { assert_eq!(claude_flags(id),(true,false),"{id}"); }
    for id in ["zcode/glm-5.3","vendor/fakeopus-5.5","gpt-5.5"] { assert_eq!(claude_flags(id),(false,false),"{id}"); }
    let m = mapped(json!({"id":"relay/claude-sonnet-4-6","native_endpoints":["/v1/messages"],"supported_reasoning_levels":[{"effort":"high"}]}));
    assert_eq!(m["compat"]["forceAdaptiveThinking"],true); assert!(m["thinkingLevelMap"].get("off").is_none());
    assert_eq!(m["promptCache"],json!({"short":300,"long":3600}));
    let glm = mapped(json!({"id":"zcode/glm-5.3","native_endpoints":["/v1/messages"],"supported_reasoning_levels":[{"effort":"high"}]}));
    assert!(glm["thinkingLevelMap"]["off"].is_null()); assert!(glm.get("compat").is_none());
}
#[test]
fn probe_does_only_two_gets_and_updates_only_owned_magpie() {
    let fixture = Fixture::new(json!({})); fixture.store(json!({"other":{"type":"api_key","key":"fixture-other"}}));
    let gateway = Gateway::new(vec![(200,json!({"name":"magpie"})),(200,list(json!([
        {"id":"upstream/chat"},{"id":"group/route","owned_by":"upstream"},
        {"id":"claude/subscription"},{"id":"vendor/claude-sonnet-4-6","native_endpoints":["/v1/messages"]}])))]);
    write_json(&fixture.backend.owned.agent.join("models.json"), &json!({"unrelated":42,"providers":{"other":{"apiKey":"fixture-other","baseUrl":"http://127.0.0.1:1"}}}),"fixture").unwrap();
    write_provider(&fixture.backend.owned,&gateway.settings(),None).unwrap();
    let state = run(&fixture.backend,None,fake_key).unwrap(); assert!(state.linked); assert_eq!(state.model_count,3);
    let requests = gateway.finish(); assert_eq!(requests.len(),2);
    assert!(requests[0].starts_with("GET /api/hello ")); assert!(requests[1].starts_with("GET /v1/models "));
    assert!(requests.iter().all(|r| r.contains(&format!("Authorization: Bearer {DEFAULT_KEY}\r\n"))));
    let cfg = config(&fixture.backend.owned).unwrap(); assert_eq!(cfg["unrelated"],42);
    assert_eq!(cfg["providers"]["other"]["baseUrl"],"http://127.0.0.1:1");
    assert!(cfg["providers"][PROVIDER].get("apiKey").is_none());
    assert_eq!(cfg["providers"][PROVIDER]["models"][2]["baseUrl"],state.settings.address().unwrap());
    assert_eq!(AuthStore::load(&fixture.backend.owned).unwrap().api_key("other").unwrap(),Some("fixture-other"));
    assert!(!serde_json::to_string(&state).unwrap().contains("fixture-other"));
}
#[test]
fn failed_hello_short_circuits_and_saved_catalog_is_retained() {
    let fixture = Fixture::new(json!({}));
    let gateway = Gateway::new(vec![(200,json!({"name":"some-other-server"}))]);
    write_provider(&fixture.backend.owned,&gateway.settings(),Some(vec![json!({"id":"old/model"})])).unwrap();
    let before = config(&fixture.backend.owned).unwrap();
    let state = run(&fixture.backend,None,|_,_| panic!("must not store credentials on failed discovery")).unwrap();
    assert!(!state.linked); assert_eq!(state.error.as_deref(),Some("not_magpie")); assert_eq!(state.model_count,1);
    assert_eq!(config(&fixture.backend.owned).unwrap(),before); assert_eq!(gateway.finish().len(),1);
}
#[test]
fn gateway_key_rejection_keeps_list_and_never_echoes_key() {
    let fixture = Fixture::new(json!({})); let key = "fixture-sk-magpie-key-not-a-real-secret";
    let gateway = Gateway::new(vec![(200,json!({"name":"magpie"})),(403,json!({"error":key}))]);
    let settings = Settings { mode:Mode::Lan,ip:"127.0.0.1".into(),port:gateway.port };
    let state = run(&fixture.backend,Some(Save { settings:settings.clone(),key:Some(key.into()) }),fake_key).unwrap();
    assert_eq!(state.settings,settings); assert_eq!(state.error.as_deref(),Some("key_rejected")); assert!(!state.linked);
    assert!(!serde_json::to_string(&state).unwrap().contains(key));
    assert!(gateway.finish().iter().all(|r| r.contains(key)));
}
#[test]
fn malformed_model_list_keeps_old_catalog_but_empty_list_is_success() {
    let fixture = Fixture::new(json!({}));
    let bad = Gateway::new(vec![(200,json!({"name":"magpie"})),(200,json!({"data":"wrong"}))]);
    write_provider(&fixture.backend.owned,&bad.settings(),Some(vec![json!({"id":"old/model"})])).unwrap();
    let state = run(&fixture.backend,None,fake_key).unwrap(); assert_eq!(state.error.as_deref(),Some("catalog_failed")); assert_eq!(state.model_count,1); bad.finish();
    let empty = Gateway::new(vec![(200,json!({"name":"magpie"})),(200,list(json!([])))]);
    let calls = std::cell::Cell::new(0);
    let state = run(&fixture.backend,Some(Save { settings:empty.settings(),key:None }),|b,k| { calls.set(calls.get()+1); fake_key(b,k) }).unwrap();
    assert!(state.linked); assert_eq!(state.model_count,0); assert_eq!(calls.get(),1); empty.finish();
}
#[test]
fn settings_and_key_survive_unreachable_gateway_without_scanning_elsewhere() {
    let fixture = Fixture::new(json!({}));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap(); let port = listener.local_addr().unwrap().port(); drop(listener);
    let settings = Settings { mode:Mode::Lan,ip:"127.0.0.1".into(),port };
    let state = run(&fixture.backend,Some(Save {settings:settings.clone(),key:Some("fixture-lan-key".into())}),fake_key).unwrap();
    assert_eq!(state.error.as_deref(),Some("unreachable")); assert_eq!(read(&fixture.backend.owned).unwrap().settings,settings);
    assert_eq!(credential(&fixture.backend.owned).unwrap().as_deref(),Some("fixture-lan-key"));
}
#[test]
fn missing_lan_key_and_native_save_failure_are_controlled() {
    let fixture = Fixture::new(json!({}));
    let settings = Settings { mode:Mode::Lan,ip:"127.0.0.1".into(),port:1 };
    let state = run(&fixture.backend,Some(Save {settings,key:None}),|_,_| panic!("missing key")).unwrap();
    assert_eq!(state.error.as_deref(),Some("key_rejected"));
    let state = run(&fixture.backend,Some(Save {settings:Settings::default(),key:None}),|_,_| Err(error("save_failed"))).unwrap();
    assert_eq!(state.error.as_deref(),Some("save_failed")); assert!(!state.linked);
}
#[test]
fn changing_lan_port_does_not_reuse_a_key_from_a_different_target() {
    let fixture = Fixture::new(json!({})); fake_key(&fixture.backend,"fixture-old-lan-key").unwrap();
    let previous = Settings { mode:Mode::Lan, ip:"127.0.0.1".into(), port:1234 };
    write_provider(&fixture.backend.owned,&previous,None).unwrap();
    let next = Settings { port:4321, ..previous.clone() };
    let result = run(&fixture.backend,Some(Save { settings:next, key:None }),|_,_| panic!("no key write")).unwrap();
    assert_eq!(result.error.as_deref(),Some("key_rejected")); assert_eq!(result.settings,previous);
}
#[test]
fn discovery_does_not_follow_redirects_to_a_second_target() {
    let fixture = Fixture::new(json!({}));
    let other = TcpListener::bind("127.0.0.1:0").unwrap(); other.set_nonblocking(true).unwrap();
    let redirected = format!("http://127.0.0.1:{}/api/hello", other.local_addr().unwrap().port());
    let gateway = Gateway::with_header(vec![(302,json!({}))],format!("Location: {redirected}\r\n"));
    write_provider(&fixture.backend.owned,&gateway.settings(),None).unwrap();
    let state = run(&fixture.backend,None,|_,_| panic!("no key write")).unwrap();
    assert_eq!(state.error.as_deref(),Some("not_magpie")); assert_eq!(gateway.finish().len(),1);
    assert_eq!(other.accept().unwrap_err().kind(),std::io::ErrorKind::WouldBlock);
}
#[test]
fn automated_key_save_uses_the_existing_private_protocol_offline() {
    let fixture = Fixture::new(json!({}));
    auth::save_magpie_key(&fixture.backend,"fixture-no-real-key").unwrap();
    assert_eq!(fixture.requests("login"),1);
    assert!(fixture.events().iter().any(|e| e["stage"] == "reply"));
    assert!(fixture.events().iter().any(|e| e["stage"] == "spawn" && e["offline"] == true));
    assert!(!serde_json::to_string(&fixture.events()).unwrap().contains("fixture-no-real-key"));
}
