//! Opt-in room round trip through the real Pi CLI, the Kota Pi extension and
//! the loopback fake provider. No account HOME, credentials or model API:
//!   KOTA_LOCAL_TEST_EXTENSION=<repo>/app-v2/kota-ext/index.ts \
//!     cargo test --lib kota_local::pi_tests -- --ignored
use super::*;
use crate::local_rpc::{launch, Config, Injection, InjectionDetails, LocalRuntime, OnlineState, Sink};
use serde_json::json;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const AGENT: &str = "agent-fable-copper";
const PEER: &str = "agent-fixture-peer";
const SESSION: &str = "33333333-3333-4333-8333-333333333333";
const BUS_EVENT: &str = "agentbus-agent-fixture-peer-agent-fable-copper-0001";
const BUS_BODY: &str = "Please review the gate.";
const BUS_REPLY: &str = "Synthetic bus reply.";
const COMPOSER_ID: &str = "msg-roundtrip-composer";
const COMPOSER_TEXT: &str = "Summarize the gate status.";
const COMPOSER_REPLY: &str = "Synthetic composer reply.";

struct RoundTrip {
    root: PathBuf,
    extension: PathBuf,
    provider: Child,
}

impl Drop for RoundTrip {
    fn drop(&mut self) {
        let _ = self.provider.kill();
        let _ = self.provider.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}

impl RoundTrip {
    fn new(replies: &[&str]) -> Self {
        let extension = PathBuf::from(
            std::env::var("KOTA_LOCAL_TEST_EXTENSION").expect("explicit test extension path"),
        );
        assert!(extension.is_file());
        let root = std::env::temp_dir().join(format!("kota-local-r3-{}", uuid::Uuid::new_v4()));
        for directory in ["home", "tmp", "config", "sessions", "workspace/.agents/skills", "project"] {
            launch::storage_directory(&root.join(directory)).unwrap();
        }
        let agent_dir = root.join("project").join(".agent-workspaces").join(AGENT);
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(
            agent_dir.join("agent.yaml"),
            format!("id: {AGENT}\nprovider: {KOTA_LOCAL_SHELL}\nstatus: active\n"),
        )
        .unwrap();

        let script = root.join("script.json");
        let steps = replies
            .iter()
            .map(|text| json!({ "type": "text", "text": text }))
            .collect::<Vec<_>>();
        fs::write(&script, serde_json::to_vec(&steps).unwrap()).unwrap();
        let mut provider = Command::new("node")
            .arg(extension.parent().unwrap().join("test/fake-provider/server.mjs"))
            .args(["--port", "0", "--script"])
            .arg(&script)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pipe = provider.stdout.take().unwrap();
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(pipe).read_line(&mut line);
            let _ = sender.send(line);
        });
        let ready: JsonValue =
            serde_json::from_str(&receiver.recv_timeout(Duration::from_secs(10)).unwrap()).unwrap();
        let base_url = ready["baseUrl"].as_str().unwrap().to_string();
        assert!(base_url.starts_with("http://127.0.0.1:"));

        let models = json!({ "providers": { "kota-fake": {
            "baseUrl": base_url, "api": "openai-completions", "apiKey": "fixture-not-a-secret",
            "models": [{ "id": "fixture", "name": "Synthetic fixture", "reasoning": false, "input": ["text"],
                "contextWindow": 16384, "maxTokens": 2048,
                "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 } }]
        } } });
        fs::write(root.join("config/models.json"), serde_json::to_vec(&models).unwrap()).unwrap();
        let settings = json!({
            "steeringMode": "all", "followUpMode": "all", "cacheWarming": "off",
            "enableInstallTelemetry": false, "enableAnalytics": false, "defaultTools": ["read"],
            "retry": { "enabled": false }, "compaction": { "enabled": false }
        });
        fs::write(root.join("config/settings.json"), serde_json::to_vec(&settings).unwrap()).unwrap();
        Self { root, extension, provider }
    }

    fn project_root(&self) -> PathBuf {
        self.root.join("project")
    }

    fn config(&self) -> Config {
        let root = PathBuf::from(std::env::var_os("KOTA_LOCAL_TEST_PI_RUNTIME_ROOT").expect("explicit pinned Pi runtime root"));
        let binary = crate::local_rpc::bundle::BundledPi::from_root(&root,&self.extension).unwrap().executable;
        let cwd = self.root.join("workspace");
        let mut command = Command::new(binary);
        command
            .current_dir(&cwd)
            .args(launch::arguments(
                &cwd,
                &self.extension,
                &self.root.join("sessions"),
                SESSION,
                Some("kota-fake/fixture"),
                Some("off"),
                None,
            ))
            .args(["--offline", "--no-context-files"])
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.root.join("home"))
            .env("PI_CODING_AGENT_DIR", self.root.join("config"))
            .env("PI_CODING_AGENT_SESSION_DIR", self.root.join("sessions"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("PI_OFFLINE", "1")
            .env("PI_SKIP_VERSION_CHECK", "1")
            .env("PI_TELEMETRY", "0")
            .env("NO_COLOR", "1");
        Config {
            scope: KotaLocalScope {
                project_root: self.project_root(),
                agent_id: AGENT.into(),
                session_id: SESSION.into(),
                epoch: 1,
            },
            ledger: self.root.join("ledger.jsonl"),
            command,
            expected_model:Some(crate::local_rpc::models::ModelSelection { provider_id:"kota-fake".into(),model_id:"fixture".into() }),
            current: Arc::new(|| true),
        }
    }
}

fn injection(message_id: &str, custom_type: &str, content: &str, actor: Option<(&str, &str)>) -> Injection {
    Injection {
        kota_id: format!("{message_id}#{AGENT}"),
        custom_type: custom_type.into(),
        content: JsonValue::String(content.into()),
        details: InjectionDetails {
            schema: 1,
            message_id: message_id.into(),
            target_agent_id: AGENT.into(),
            created_at: Utc::now().to_rfc3339(),
            private: false,
            source_actor_id: actor.map(|(id, _)| id.to_string()),
            source_intent: actor.map(|(_, intent)| intent.to_string()),
            route_model: None,
        },
    }
}

fn until(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done() {
        assert!(Instant::now() < deadline, "real Pi room round trip timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn request() -> VioletRoomRequest {
    VioletRoomRequest {
        project_root: None,
        limit: Some(500),
        before: None,
        around: None,
        agent_ids: None,
        watch_agent_ids: None,
    }
}

fn room(project_root: &Path) -> Vec<VioletChatMessage> {
    read_room_messages(project_root, &request()).unwrap()
}

fn settled_with(runtime: &LocalRuntime, project_root: &Path, reply: &str) -> bool {
    runtime.snapshot().state == OnlineState::Ready
        && room(project_root).iter().any(|message| message.text == reply)
}

#[test]
#[ignore = "requires real Pi and KOTA_LOCAL_TEST_EXTENSION; loopback fixture only"]
fn real_pi_room_round_trip_projects_composer_and_bus_once() {
    let fixture = RoundTrip::new(&[BUS_REPLY, COMPOSER_REPLY]);
    let project_root = fixture.project_root();
    let sink: Arc<dyn Sink> = Arc::new(VioletLocalSink { app: None });
    let runtime = LocalRuntime::start(fixture.config(), sink).unwrap();
    runtime.activate(false).unwrap();

    // Agent Bus: the actor bubble is recorded at send time, exactly as today;
    // the Local input carries the rendered envelope with the same event id.
    record_actor_message(
        &project_root,
        &ActorMessageRecord {
            actor_id: PEER.into(),
            actor_name: "Fixture Peer".into(),
            text: BUS_BODY.into(),
            target_agent_ids: vec![AGENT.into()],
            event_id: BUS_EVENT.into(),
            actor_intent: Some("handoff".into()),
            origin: crate::message_origin::MessageOrigin::A2a,
        },
    )
    .unwrap();
    let envelope = format!(
        "<KOTA_MESSAGE id=\"{BUS_EVENT}\" from=\"{PEER}\" to=\"{AGENT}\" intent=\"handoff\">\n{BUS_BODY}\n</KOTA_MESSAGE>"
    );
    runtime
        .submit(injection(BUS_EVENT, "kota.bus", &envelope, Some((PEER, "handoff"))))
        .unwrap();
    until(|| settled_with(&runtime, &project_root, BUS_REPLY));

    let composer = injection(COMPOSER_ID, "kota.user", COMPOSER_TEXT, None);
    let created_at = normalize_timestamp(&composer.details.created_at);
    runtime.submit(composer).unwrap();
    until(|| settled_with(&runtime, &project_root, COMPOSER_REPLY));
    runtime.close().unwrap();

    let messages = room(&project_root);
    let count = |text: &str| messages.iter().filter(|message| message.text == text).count();
    assert_eq!(count(COMPOSER_TEXT), 1, "one composer bubble");
    assert_eq!(count(BUS_BODY), 1, "one Agent Bus bubble");
    assert_eq!(count(BUS_REPLY), 1);
    assert_eq!(count(COMPOSER_REPLY), 1);
    assert!(messages.iter().all(|message| !message.text.contains("KOTA_MESSAGE")));
    let user = messages.iter().find(|message| message.text == COMPOSER_TEXT).unwrap();
    assert_eq!(
        (user.role.as_str(), user.agent_id.as_str(), user.shell.as_str()),
        ("user", AGENT, KOTA_LOCAL_SHELL)
    );
    assert_eq!(user.timestamp, created_at);
    assert_eq!(user.native_event_id.as_deref(), Some(format!("{COMPOSER_ID}#{AGENT}").as_str()));
    let bus = messages.iter().find(|message| message.text == BUS_BODY).unwrap();
    assert_eq!(bus.native_event_id.as_deref(), Some(BUS_EVENT));

    let state = sync_project(&project_root, request()).unwrap();
    assert!(state
        .agent_bus_receipts
        .iter()
        .any(|receipt| receipt.event_id == BUS_EVENT && receipt.agent_id == AGENT));
    assert!(state.local_pending_deliveries.is_empty());
    let source = state.sources.iter().find(|source| source.agent_id == AGENT).unwrap();
    assert_eq!(source.source_kind, KOTA_LOCAL_SOURCE_KIND);
    assert!(source.error.is_none());
}
