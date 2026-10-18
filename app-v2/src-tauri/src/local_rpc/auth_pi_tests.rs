//! Opt-in Rust + bundled Bun Pi authentication. Only fake values and loopback
//! HTTP; the sandbox blocks personal Pi reads/writes and all other networking.
use super::{
    auth::{self, Backend},
    auth_protocol::{AuthReply, AuthStart},
    bundle::BundledPi,
    credentials::AuthStore,
    models::{self, ModelSelection},
    owned::OwnedDirectory,
    protocol::{fixture as input_fixture, LocalNotice, LocalScope, Sink},
    runtime::{Config, LocalRuntime},
};
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

struct Fixture {
    root: PathBuf,
    backend: Backend,
}
type Events = Arc<Mutex<Vec<Value>>>;
impl Fixture {
    fn new(port: Option<u16>) -> Self {
        let runtime = PathBuf::from(
            std::env::var_os("KOTA_LOCAL_TEST_PI_RUNTIME_ROOT")
                .expect("explicit pinned Pi staging"),
        );
        let extension = PathBuf::from(
            std::env::var_os("KOTA_LOCAL_TEST_EXTENSION").expect("explicit product extension"),
        );
        let binary = BundledPi::from_root(&runtime, &extension)
            .unwrap()
            .executable;
        let root = std::env::temp_dir().join(format!("local-native-auth-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        for name in ["home/.pi/agent", "tmp", "workspace", "sessions"] {
            fs::create_dir_all(root.join(name)).unwrap();
        }
        fs::write(
            root.join("home/.pi/agent/auth.json"),
            "synthetic-personal-sentinel\n",
        )
        .unwrap();
        let personal = PathBuf::from(std::env::var_os("HOME").unwrap()).join(".pi");
        let mut policy = format!("(version 1)(allow default)(deny network*)\n(deny file-read* file-write* (subpath {}))\n(deny file-read* file-write* (subpath {}))\n(deny file-write* (subpath {}))",
            json!(personal), json!(root.join("home/.pi")), json!(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()));
        if let Some(port) = port {
            policy.push_str(&format!(
                "\n(allow network-outbound (remote ip \"localhost:{port}\"))"
            ));
        }
        let executable = root.join("sandbox-pi");
        // exec keeps the native process in the Rust-owned process group. No
        // shell expansion and no global environment changes in the test host.
        fs::write(
            &executable,
            format!(
                r#"#!/usr/bin/python3
import os,sys,json
root={root}
env={{'PATH':'/usr/bin:/bin:/opt/homebrew/bin','HOME':root+'/home','USERPROFILE':root+'/home',
 'TMPDIR':root+'/tmp','PI_CODING_AGENT_DIR':os.environ['PI_CODING_AGENT_DIR'],
 'PI_SKIP_VERSION_CHECK':'1','NO_COLOR':'1'}}
for key in ['KOTA_LOCAL_AUTH_HELPER','KOTA_LOCAL_INSTALLATION_UUID']:
 if key in os.environ: env[key]=os.environ[key]
with open(root+'/spawns.jsonl','a') as log: log.write(json.dumps({{'offline':'--offline' in sys.argv}})+'\n')
os.execve('/usr/bin/sandbox-exec',['sandbox-exec','-p',{policy},{binary},*sys.argv[1:]],env)
"#,
                root = json!(root),
                policy = json!(policy),
                binary = json!(binary)
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let owned = OwnedDirectory::new(&root.join("kota-home"));
        owned.initialize().unwrap();
        Self {
            root,
            backend: Backend {
                owned,
                bundled: BundledPi {
                    executable,
                    extension,
                },
            },
        }
    }
    fn store(&self) -> Value {
        serde_json::from_slice(&fs::read(self.backend.owned.auth_path()).unwrap()).unwrap()
    }
    fn begin(&self, request: Value) -> (String, Events) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let capture = events.clone();
        let id = auth::start(
            self.backend.clone(),
            serde_json::from_value(request).unwrap(),
            Arc::new(move |event| capture.lock().unwrap().push(event)),
            Arc::new(|| {}),
        )
        .unwrap()
        .operation_id;
        (id, events)
    }
    fn login(&self, provider: &str, url: Option<&str>, key: &str) -> Value {
        let mut request = json!({"operation":"login","providerId":provider,"authType":"api_key"});
        if let Some(url) = url {
            request["resourceUrl"] = json!(url);
        }
        let (id, events) = self.begin(request);
        let prompt = event(&events, "prompt");
        respond(&id, &prompt, key);
        let result = event(&events, "result");
        assert_eq!(result["outcome"], "succeeded");
        assert_eq!(result["commit"], "confirmed");
        let wire = serde_json::to_string(&*events.lock().unwrap()).unwrap();
        assert!(!wire.contains(key));
        result
    }
}

#[test]
#[ignore = "requires pinned bundled Pi; synthetic Magpie streaming/tools/thinking/abort on loopback only"]
fn real_pi_magpie_native_transports_stream_tool_think_and_abort() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("kota-ext/test/fixtures/magpie-native-streams.mjs");
    let output = Command::new("node").arg(script).output().unwrap();
    assert!(output.status.success(),"synthetic Magpie transport: {}",String::from_utf8_lossy(&output.stderr));
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    let results = receipt["results"].as_array().unwrap(); assert_eq!(results.len(),3);
    assert!(results.iter().all(|result| result["passed"] == true && result["syntheticRequests"] == 3));
    assert_eq!(receipt["realUpstreamRequests"],0); assert_eq!(receipt["providerFactories"],0);
    println!("{}",String::from_utf8_lossy(&output.stdout));
}

#[test]
#[ignore = "requires pinned bundled Pi; Magpie native key save/catalog, only a closed loopback port"]
fn real_pi_magpie_uses_owned_configuration_and_native_secret_protocol() {
    let f = Fixture::new(None);
    // Extra argv witness only for this fixture; never log an IPC reply or a real credential.
    let wrapper = fs::read_to_string(&f.backend.bundled.executable).unwrap();
    fs::write(&f.backend.bundled.executable,wrapper.replace("with open(root+'/spawns.jsonl'",
        "with open(root+'/args.jsonl','a') as log: log.write(json.dumps(sys.argv[1:])+'\\n')\nwith open(root+'/spawns.jsonl'" )).unwrap();
    let key = "fixture-native-magpie-key";
    fs::write(f.backend.owned.auth_path(),serde_json::to_vec(&json!({"other":{"type":"api_key","key":"fixture-other"}})).unwrap()).unwrap();
    let closed = TcpListener::bind("127.0.0.1:0").unwrap(); let port = closed.local_addr().unwrap().port(); drop(closed);
    let settings = super::magpie::Settings { mode:super::magpie::Mode::Lan, ip:"127.0.0.1".into(), port };
    let state = super::magpie::save(&f.backend,super::magpie::Save { settings:settings.clone(),key:Some(key.into()) }).unwrap();
    assert_eq!(state.error.as_deref(),Some("unreachable")); assert_eq!(state.settings,settings);
    assert_eq!(f.store()["magpie"],json!({"type":"api_key","key":key}));
    assert_eq!(f.store()["other"]["key"],"fixture-other");
    assert!(!serde_json::to_string(&state).unwrap().contains(key));
    assert!(!fs::read_to_string(f.root.join("args.jsonl")).unwrap().contains(key));
    let path = f.backend.owned.agent.join("models.json");
    let mut config: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(config["kotaMagpie"]["port"],port); assert!(config["providers"]["magpie"].get("apiKey").is_none());
    config["providers"]["magpie"]["models"] = json!([{"id":"upstream/with/slashes","name":"Synthetic Magpie","reasoning":false}]);
    fs::write(&path,serde_json::to_vec(&config).unwrap()).unwrap();
    let catalog = models::model_catalog(&f.backend).unwrap();
    let provider = catalog.providers.iter().find(|p| p.provider_id == "magpie").unwrap();
    assert!(provider.linked); assert_eq!(provider.models[0].model_id,"upstream/with/slashes");
    let selection = ModelSelection { provider_id:"magpie".into(),model_id:"upstream/with/slashes".into() };
    models::resolve_model(&f.backend,&selection,"off").unwrap();
    fs::write(f.backend.owned.auth_path(),serde_json::to_vec(&json!({"other":{"type":"api_key","key":"fixture-other"}})).unwrap()).unwrap();
    assert_eq!(models::resolve_model(&f.backend,&selection,"off").err().unwrap().code,"provider_not_signed_in");
    assert!(fs::read_to_string(f.root.join("spawns.jsonl")).unwrap().lines().all(|line| serde_json::from_str::<Value>(line).unwrap()["offline"] == true));
}

fn expired_fake_oauth(f: &Fixture) {
    fs::write(f.backend.owned.auth_path(), serde_json::to_vec(&json!({"openai":{
        "type":"oauth","access":"synthetic-expired-access","refresh":"synthetic-refresh",
        "expires":1,"clientId":"synthetic-client","scopes":[]}})).unwrap()).unwrap();
}

#[test]
#[ignore = "requires pinned bundled Pi; legacy Azure fixture, no network"]
fn real_pi_1_1_new_azure_is_not_linked_by_a_legacy_owned_entry() {
    let f = Fixture::new(None);
    let path = f.backend.owned.auth_path();
    fs::write(&path, serde_json::to_vec(&json!({"azure-openai-responses":{
        "type":"api_key","key":"fake-legacy-key",
        "env":{"AZURE_OPENAI_BASE_URL":"http://127.0.0.1:1/legacy"}}})).unwrap()).unwrap();
    let auth = fs::read(&path).unwrap();
    let overlay = f.backend.owned.agent.join("models.json");
    let old_overlay = serde_json::to_vec(&json!({"providers":{"azure-openai-responses":{
        "baseUrl":"http://127.0.0.1:1/legacy","api":"azure-openai-responses","models":[{
            "id":"legacy","name":"Legacy Azure fixture","reasoning":false,"input":["text"],
            "contextWindow":16000,"maxTokens":1000,
            "cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0}}]}}})).unwrap();
    fs::write(&overlay, &old_overlay).unwrap();
    let shell = f.root.join("workspace/SHELL.yaml");
    let original = b"provider: kota-local\nmodel: azure-openai-responses/legacy\neffort: high\n";
    fs::write(&shell, original).unwrap();
    let catalog = models::model_catalog(&f.backend).unwrap();
    assert!(!catalog.providers.iter().any(|p| p.provider_id == "azure-openai-responses"));
    let azure = catalog.providers.iter().find(|p| p.provider_id == "azure").unwrap();
    assert!(!azure.linked);
    assert!(!azure.models.is_empty());
    let availability = models::availability(&f.backend);
    assert_eq!(availability.summary, "not_ready");
    assert!(!availability.providers.iter().any(|p| p.provider_id == "azure-openai-responses"));
    let azure_status = availability.providers.iter().find(|p| p.provider_id == "azure").unwrap();
    assert_eq!(azure_status.status, "not_signed_in");
    let model = ModelSelection { provider_id: "azure".into(), model_id: azure.models[0].model_id.clone() };
    assert_eq!(models::resolve_model(&f.backend, &model, "off").err().unwrap().code, "provider_not_signed_in");
    assert_eq!(models::selected_model(Some("azure-openai-responses/legacy")).err().unwrap().code, "model_not_found");
    assert_eq!(fs::read(path).unwrap(), auth);
    assert_eq!(fs::read(overlay).unwrap(), old_overlay);
    assert_eq!(fs::read(shell).unwrap(), original);
}

#[test]
#[ignore = "requires pinned bundled Pi; synthetic expired OAuth, no network"]
fn real_pi_metadata_is_single_flight_offline_freshly_gated_and_recovers_busy_storage() {
    let f = Fixture::new(None);
    expired_fake_oauth(&f);
    let original = fs::read(f.backend.owned.auth_path()).unwrap();
    let began = Instant::now();
    let barrier = std::sync::Barrier::new(6);
    thread::scope(|scope| {
        let workers = (0..6).map(|_| scope.spawn(|| {
            barrier.wait(); models::model_catalog(&f.backend).unwrap();
        })).collect::<Vec<_>>();
        for worker in workers { worker.join().unwrap(); }
    });
    let cold = began.elapsed().as_millis();
    let records = || fs::read_to_string(f.root.join("spawns.jsonl")).unwrap().lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap()).collect::<Vec<_>>();
    assert_eq!(records(), vec![json!({"offline":true})]);
    let model = ModelSelection { provider_id:"openai".into(), model_id:"gpt-6.1-sol".into() };
    let first = Instant::now(); models::resolve_model(&f.backend, &model, "max").unwrap();
    let first_resolve = first.elapsed().as_millis();
    let warm = Instant::now(); models::model_catalog(&f.backend).unwrap();
    models::resolve_model(&f.backend, &model, "max").unwrap();
    let warm_ms = warm.elapsed().as_millis();
    // The first native startup materializes models-store.json. Its new
    // identity correctly invalidates the pre-creation catalog once.
    assert_eq!(records().len(), 3);
    assert_eq!(fs::read(f.backend.owned.auth_path()).unwrap(), original);
    let lock = f.backend.owned.agent.join("auth.json.lock");
    fs::create_dir(&lock).unwrap();
    let waiting = Instant::now();
    thread::scope(|scope| {
        scope.spawn(|| {
            while waiting.elapsed() < Duration::from_secs(20) {
                fs::File::open(&lock).unwrap().set_times(fs::FileTimes::new().set_modified(std::time::SystemTime::now())).unwrap();
                thread::sleep(Duration::from_millis(250));
            }
            fs::remove_dir(&lock).unwrap();
        });
        f.backend.query(json!({"operation":"status"})).unwrap();
    }); // synthetic lock holder releases; the same query recovers without Retry
    assert!(waiting.elapsed() >= Duration::from_secs(20));
    assert_eq!(records().len(), 4);
    fs::write(f.backend.owned.auth_path(), "{}").unwrap();
    assert_eq!(models::resolve_model(&f.backend, &model, "max").err().unwrap().code, "provider_not_signed_in");
    assert_eq!(records().len(), 4);
    use std::io::Write;
    writeln!(fs::OpenOptions::new().append(true).open(&f.backend.bundled.executable).unwrap(), "# new bundled identity").unwrap();
    models::model_catalog(&f.backend).unwrap(); assert_eq!(records().len(), 5);
    println!("startup-timings: six callers cold_catalog={cold}ms, native_resolve={first_resolve}ms, warm_catalog_and_resolve={warm_ms}ms; cold-helpers=1, new-models-store-invalidation=1, native-clamp-helpers=1, lock-release=recovered, logout=blocked, new-identity=refreshed");
}

#[test]
#[ignore = "requires pinned bundled Pi; synthetic active lock, real 40 s readiness deadline"]
fn real_pi_a_persistently_held_lock_reports_busy_at_readiness_deadline() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let f = Fixture::new(None); expired_fake_oauth(&f);
    let lock = f.backend.owned.agent.join("auth.json.lock"); fs::create_dir(&lock).unwrap();
    let done = AtomicBool::new(false);
    let began = Instant::now();
    let error = thread::scope(|scope| {
        scope.spawn(|| {
            while !done.load(Ordering::SeqCst) {
                if let Ok(file) = fs::File::open(&lock) { let _ = file.set_times(fs::FileTimes::new().set_modified(std::time::SystemTime::now())); }
                thread::sleep(Duration::from_millis(250));
            }
        });
        let result = f.backend.query(json!({"operation":"status"}));
        done.store(true, Ordering::SeqCst); result.unwrap_err()
    });
    assert_eq!(error.code, "auth_store_busy");
    assert_eq!(error.stage.as_deref(), Some("credential_store"));
    assert!(began.elapsed() >= Duration::from_secs(40));
    assert!(began.elapsed() < Duration::from_secs(45));
    assert!(lock.is_dir()); // Kota did not erase someone else's lock
    println!("startup-busy: {}ms; diagnosed by lock presence at deadline, no ELOCKED on wire", began.elapsed().as_millis());
}

#[test]
#[ignore = "requires pinned bundled Pi; expired fake OAuth and loopback blackhole proxy"]
fn real_pi_term_releases_the_credential_lock_before_forced_cleanup() {
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    server.set_nonblocking(true).unwrap();
    let port = server.local_addr().unwrap().port();
    let f = Fixture::new(Some(port)); expired_fake_oauth(&f);
    fs::write(f.backend.owned.agent.join("settings.json"), serde_json::to_vec(&json!({"httpProxy":format!("http://127.0.0.1:{port}")})).unwrap()).unwrap();
    let mut command = Command::new(&f.backend.bundled.executable);
    command.current_dir(f.root.join("workspace")).env("PI_CODING_AGENT_DIR", &f.backend.owned.agent)
        .args(["--mode","rpc","--no-session","--no-approve","-ne","-ns","-np","--no-tools","--no-context-files","--no-themes"]);
    let mut process = super::process::Process::spawn(command).unwrap();
    let lock = f.backend.owned.agent.join("auth.json.lock");
    let end = Instant::now() + Duration::from_secs(20);
    let connection = loop {
        if let Ok((connection, _)) = server.accept() { break connection; }
        assert!(Instant::now() < end, "synthetic OAuth did not reach the loopback proxy");
        thread::sleep(Duration::from_millis(10));
    };
    assert!(lock.is_dir());
    let stopped = Instant::now(); process.stop().unwrap();
    assert!(!lock.exists());
    assert!(process.try_wait().unwrap().is_some());
    println!("startup-term: {}ms; Pi-owned credential lock removed", stopped.elapsed().as_millis());
    drop(connection);
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert_eq!(
            fs::read_to_string(self.root.join("home/.pi/agent/auth.json")).unwrap(),
            "synthetic-personal-sentinel\n"
        );
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn event(events: &Events, kind: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(35);
    loop {
        if let Some(value) = events
            .lock()
            .unwrap()
            .iter()
            .find(|e| e["event"] == kind)
            .cloned()
        {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "native auth event did not arrive"
        );
        thread::sleep(Duration::from_millis(5));
    }
}
fn respond(id: &str, prompt: &Value, value: &str) {
    auth::respond(AuthReply {
        operation_id: id.into(),
        prompt_id: prompt["promptId"].as_str().unwrap().into(),
        value: Some(value.into()),
        cancelled: None,
    })
    .unwrap();
}

#[test]
#[ignore = "requires pinned bundled Pi and explicit test extension; sandboxed fake credentials only"]
fn real_pi_auth_rust_binds_prompts_cancels_and_applies_fresh_link_gates() {
    let f = Fixture::new(None);
    let (id, events) =
        f.begin(json!({"operation":"login","providerId":"anthropic","authType":"api_key"}));
    let prompt = event(&events, "prompt");
    assert_eq!(prompt["credentialField"], "api_key.key");
    assert_eq!(prompt["prompt"]["type"], "secret");
    let competing: AuthStart = serde_json::from_value(
        json!({"operation":"login","providerId":"anthropic","authType":"api_key"}),
    )
    .unwrap();
    assert_eq!(
        auth::start(
            f.backend.clone(),
            competing,
            Arc::new(|_| {}),
            Arc::new(|| {})
        )
        .err()
        .unwrap()
        .code,
        "busy-auth"
    );
    assert_eq!(
        auth::respond(AuthReply {
            operation_id: id.clone(),
            prompt_id: prompt["promptId"].as_str().unwrap().into(),
            value: Some("!never-executed".into()),
            cancelled: None
        })
        .unwrap_err()
        .code,
        "credential_invalid"
    );
    assert!(f.store().get("anthropic").is_none());
    assert!(auth::respond(AuthReply {
        operation_id: id.clone(),
        prompt_id: "wrong-step".into(),
        value: Some("fake-unused".into()),
        cancelled: None
    })
    .is_err());
    respond(&id, &prompt, "fake-anthropic-owned");
    let result = event(&events, "result");
    assert_eq!(result["outcome"], "succeeded");
    assert_eq!(result["commit"], "confirmed");
    assert!(!serde_json::to_string(&*events.lock().unwrap())
        .unwrap()
        .contains("fake-anthropic-owned"));
    assert!(auth::respond(AuthReply {
        operation_id: id,
        prompt_id: prompt["promptId"].as_str().unwrap().into(),
        value: Some("fake-unused".into()),
        cancelled: None
    })
    .is_err());

    let availability = models::availability(&f.backend);
    assert_eq!(availability.summary, "ready");
    assert_eq!(
        availability
            .providers
            .iter()
            .find(|p| p.provider_id == "anthropic")
            .unwrap()
            .status,
        "signed_in"
    );
    let catalog = models::model_catalog(&f.backend).unwrap();
    let provider = catalog
        .providers
        .iter()
        .find(|p| p.provider_id == "anthropic")
        .unwrap();
    assert!(provider.linked);
    assert!(catalog
        .providers
        .iter()
        .any(|p| !p.linked && !p.models.is_empty()));
    let model = ModelSelection {
        provider_id: "anthropic".into(),
        model_id: provider.models[0].model_id.clone(),
    };
    let resolved = models::resolve_model(&f.backend, &model, "max").unwrap();
    assert!(provider.models[0]
        .thinking_levels
        .contains(&resolved.thinking_level));
    let before = fs::read(f.backend.owned.auth_path()).unwrap();
    let (cancel_id, cancelled) =
        f.begin(json!({"operation":"login","providerId":"anthropic","authType":"api_key"}));
    event(&cancelled, "prompt");
    auth::cancel(&cancel_id).unwrap();
    let outcome = event(&cancelled, "result");
    assert_eq!(outcome["outcome"], "cancelled");
    assert_eq!(outcome["commit"], "not-written");
    assert!(fs::read(f.backend.owned.auth_path()).unwrap() == before);

    let (_, logout) = f.begin(json!({"operation":"logout","providerId":"anthropic"}));
    assert_eq!(event(&logout, "result")["commit"], "confirmed");
    assert_eq!(
        models::resolve_model(&f.backend, &model, "off")
            .err()
            .unwrap()
            .code,
        "provider_not_signed_in"
    );
    assert!(
        !models::model_catalog(&f.backend)
            .unwrap()
            .providers
            .iter()
            .find(|p| p.provider_id == "anthropic")
            .unwrap()
            .linked
    );
}

struct Quiet;
impl Sink for Quiet {
    fn notify(&self, _: &LocalNotice) -> Result<(), String> {
        Ok(())
    }
}
#[test]
#[ignore = "requires pinned bundled Pi and explicit test extension; no model calls"]
fn real_pi_explicit_model_without_effort_starts_with_native_medium_clamp_without_writing_shell() {
    let f = Fixture::new(None);
    fs::write(f.backend.owned.auth_path(), serde_json::to_vec(&json!({
        "anthropic":{"type":"api_key","key":"synthetic-anthropic"},
        "openai":{"type":"api_key","key":"synthetic-openai"}
    })).unwrap()).unwrap();
    let catalog = models::model_catalog(&f.backend).unwrap();
    for (provider_id, reasoning) in [("anthropic", true), ("openai", false)] {
        let provider = catalog.providers.iter().find(|p| p.provider_id == provider_id).unwrap();
        let model = provider.models.iter().find(|m| if reasoning {m.thinking_levels.iter().any(|s| s == "medium")} else {m.thinking_levels == ["off"]}).unwrap();
        let workspace = f.root.join("workspace");
        let shell = format!("provider: kota-local\nmodel: {provider_id}/{}\nargs: []\nunknown: {{keep: true}}\n", model.model_id);
        fs::write(workspace.join("SHELL.yaml"), &shell).unwrap();
        let saved = super::configuration::SavedModel::read(&workspace).unwrap();
        assert_eq!(saved.thinking, "medium");
        let selected = models::selected_model(saved.model.as_deref()).unwrap();
        let resolved = models::resolve_model(&f.backend, &selected, &saved.thinking).unwrap();
        assert_eq!(resolved.thinking_level, if reasoning {"medium"} else {"off"});
        let session = uuid::Uuid::new_v4().to_string();
        let mut command = Command::new(&f.backend.bundled.executable);
        command.current_dir(&workspace).args(super::launch::arguments(&workspace, &f.backend.bundled.extension,
            &f.root.join("sessions"), &session, Some(&resolved.model_arg), Some(&resolved.thinking_level), None))
            .env_clear().env("PI_CODING_AGENT_DIR", &f.backend.owned.agent).args(["--offline", "--no-context-files"]);
        let runtime = LocalRuntime::start(Config { command, ledger:f.root.join(format!("{provider_id}-ledger.jsonl")),
            scope:LocalScope {project_root:workspace.clone(), agent_id:"agent-native-medium".into(), session_id:session, epoch:1},
            expected_model:Some(selected), current:Arc::new(||true)}, Arc::new(Quiet)).unwrap();
        let state = runtime.rpc(json!({"type":"get_state"})).unwrap();
        assert_eq!(state["model"]["provider"], provider_id);
        assert_eq!(state["model"]["id"], model.model_id);
        assert_eq!(state["thinkingLevel"], resolved.thinking_level);
        runtime.close().unwrap();
        assert_eq!(fs::read_to_string(workspace.join("SHELL.yaml")).unwrap(), shell);
    }
}

#[test]
#[ignore = "requires pinned bundled Pi and explicit test extension; loopback HTTP rejects all model calls"]
fn real_pi_auth_azure_saved_pair_reaches_the_unwrapped_agent_request() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::sync_channel(1);
    listener.set_nonblocking(true).unwrap();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(90);
        while Instant::now() < deadline {
            if let Ok((mut stream, _)) = listener.accept() {
                // Darwin inherits O_NONBLOCK from the listener. Read the HTTP
                // header under its timeout instead of treating WouldBlock as EOF.
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut byte = [0];
                while bytes.len() < 64 * 1024 && stream.read(&mut byte).unwrap_or(0) == 1 {
                    bytes.push(byte[0]);
                    if bytes.ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
                // An empty connection is not an HTTP saved-pair witness.
                if bytes.is_empty() { continue; }
                let body = br#"{"error":{"message":"Synthetic rejection","type":"invalid_request_error"}}"#;
                let _ = write!(stream, "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(body);
                let _ = tx.send(bytes);
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
    });
    let f = Fixture::new(Some(port));
    let base = format!("http://127.0.0.1:{port}/saved-B");
    f.login(
        "azure",
        Some(&format!("http://127.0.0.1:{port}/saved-A")),
        "fake-azure-key-A",
    );
    f.login("azure", Some(&base), "fake-azure-key-B");
    let stored = f.store();
    assert!(stored["azure"]["key"] == "fake-azure-key-B");
    assert!(stored["azure"]["env"]["AZURE_OPENAI_BASE_URL"] == base);
    let catalog = models::model_catalog(&f.backend).unwrap();
    let provider = catalog
        .providers
        .iter()
        .find(|p| p.provider_id == "azure")
        .unwrap();
    let model = ModelSelection {
        provider_id: provider.provider_id.clone(),
        model_id: provider.models[0].model_id.clone(),
    };
    models::resolve_model(&f.backend, &model, "off").unwrap();
    let workspace = f.root.join("workspace");
    let mut command = Command::new(&f.backend.bundled.executable);
    command
        .current_dir(&workspace)
        .args(super::launch::arguments(
            &workspace,
            &f.backend.bundled.extension,
            &f.root.join("sessions"),
            "22222222-2222-4222-8222-222222222222",
            Some(&format!("{}/{}", model.provider_id, model.model_id)),
            Some("off"),
            None,
        ))
        .env_clear()
        .env("PI_CODING_AGENT_DIR", &f.backend.owned.agent)
        .args(["--offline", "--no-context-files"]);
    let runtime = LocalRuntime::start(
        Config {
            command,
            ledger: f.root.join("ledger.jsonl"),
            scope: LocalScope {
                project_root: workspace,
                agent_id: "agent-fable-copper".into(),
                session_id: "22222222-2222-4222-8222-222222222222".into(),
                epoch: 1,
            },
            expected_model: Some(model),
            current: Arc::new(|| true),
        },
        Arc::new(Quiet),
    )
    .unwrap();
    runtime.activate(false).unwrap();
    runtime
        .submit(input_fixture("azure-request-fixture"))
        .unwrap();
    let request = String::from_utf8(rx.recv_timeout(Duration::from_secs(35)).unwrap()).unwrap();
    assert!(request.lines().next().unwrap().contains("/saved-B/"));
    assert!(request
        .lines()
        .any(|l| l.eq_ignore_ascii_case("api-key: fake-azure-key-B")));
    assert!(!request.contains("fake-azure-key-A"));
    runtime.close().unwrap();
    server.join().unwrap();
    AuthStore::load(&f.backend.owned)
        .unwrap()
        .preflight()
        .unwrap();
}

/// Loopback fake Jev (System One wire). Each connection gets the next status; 200 answers `mode`.
fn fake_jev(statuses: Vec<u16>) -> (u16, mpsc::Receiver<String>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let (tx, rx) = mpsc::sync_channel(statuses.len());
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(120);
        for status in statuses {
            let mut stream = loop {
                if let Ok((stream, _)) = listener.accept() { break stream; }
                if Instant::now() >= deadline { return; }
                thread::sleep(Duration::from_millis(5));
            };
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut bytes = Vec::new();
            let mut byte = [0];
            while bytes.len() < 64 * 1024 && stream.read(&mut byte).unwrap_or(0) == 1 {
                bytes.push(byte[0]);
                if bytes.ends_with(b"\r\n\r\n") { break; }
            }
            let head = String::from_utf8_lossy(&bytes).to_string();
            let length = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap())).unwrap_or(0);
            let mut body = vec![0; length];
            stream.read_exact(&mut body).unwrap();
            let reply = if status == 200 {
                json!({"answers":{"model":{"type":"choice","choice":"model-1","probabilities":{"model-1":0.7,"model-2":0.3},"confidence":0.7}}}).to_string()
            } else { json!({"error":{"message":"Synthetic Jev failure."}}).to_string() };
            let _ = write!(stream, "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len());
            let _ = tx.send(format!("{head}{}", String::from_utf8_lossy(&body)));
        }
    });
    (port, rx, server)
}

#[test]
#[ignore = "requires pinned bundled Pi and explicit test extension; loopback fake Jev only"]
fn real_pi_jev_key_check_saves_only_a_key_that_jev_accepts() {
    let (port, requests, server) = fake_jev(vec![200, 401, 500]);
    let f = Fixture::new(Some(port));
    // The classifier's base URL points at the loopback fake; the key is only a request option.
    fs::write(f.backend.owned.agent.join("models.json"), serde_json::to_vec(&json!({"providers":{"typesafe":{
        "baseUrl":format!("http://127.0.0.1:{port}/v1")}}})).unwrap()).unwrap();
    super::jev::select(&f.backend, "typesafe").unwrap();
    let settings = super::jev::save_key(&f.backend, "check-1", "typesafe", "fake-jev-key-1").unwrap();
    assert!(settings.ready);
    let first = requests.recv_timeout(Duration::from_secs(30)).unwrap();
    assert!(first.starts_with("POST /v1/systemone "));
    assert!(first.lines().any(|l| l.eq_ignore_ascii_case("authorization: Bearer fake-jev-key-1")));
    let body: Value = serde_json::from_str(&first[first.find("\r\n\r\n").unwrap() + 4..]).unwrap();
    assert_eq!(body["model"], "jev-latest");
    assert_eq!(body["state"], json!({"new_message":"ok"}));
    for (request, key) in [("check-401", "fake-jev-key-2"), ("check-500", "fake-jev-key-3")] {
        assert_eq!(super::jev::save_key(&f.backend, request, "typesafe", key).unwrap_err().code, "jev_verification_failed");
        requests.recv_timeout(Duration::from_secs(30)).unwrap();
        assert_eq!(super::jev::current(&f.backend.owned).unwrap().unwrap().key, "fake-jev-key-1", "a failed check keeps the old key");
    }
    let spawns = fs::read_to_string(f.root.join("spawns.jsonl")).unwrap();
    assert!(spawns.lines().all(|line| line.contains("\"offline\": true")), "the check helper never refreshes catalogs or OAuth");
    server.join().unwrap();
}
