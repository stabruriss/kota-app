use super::*;
use crate::{message_origin::MessageOrigin, soul::marks::{Mark, Marks}};
use serde_json::json;
use std::time::Instant;

const AGENT: &str = "agent-fixture";

struct Fixture {
    root: PathBuf,
    asked: Arc<Mutex<Vec<Value>>>,
}

impl Fixture {
    fn new() -> Self {
        let fixture = Self {
            root: std::env::temp_dir().join(format!("kota-soul-service-{}", uuid::Uuid::new_v4())),
            asked: Arc::default(),
        };
        let mark = |id: &str, text: &str| Mark { id: id.into(), text: text.into(), created_at: "2026-10-15T00:00:00Z".into(),
            updated_at: "2026-10-15T00:00:00Z".into(), project: None, used: 0 };
        soul::write_json_atomic(&fixture.paths().marks(), &Marks { version: 1, revision: 3,
            marks: vec![mark("m-a", "Check the fixture before changing a rule."), mark("m-b", "Keep synthetic receipts.")] }).unwrap();
        fixture
    }
    fn project(&self) -> PathBuf { self.root.join("project") }
    fn scope(&self, key: &str) -> Scope {
        Scope { project_root: self.project(), project_id: None, agent_cwd: self.project().join(".agent-workspaces").join(AGENT),
            agent_id: AGENT.into(), key: key.into() }
    }
    fn paths(&self) -> SoulPaths { SoulPaths::new(&self.scope("").agent_cwd) }
    /// Jev answers every question with `answer`, or fails with it.
    fn service(&self, settings: settings::Settings, jev_ready: bool, answer: Result<f64, &'static str>) -> Service {
        let snapshot = settings::Snapshot { settings, source_revision: "missing".into(), jev_ready };
        let asked = self.asked.clone();
        Service::with(self.root.join("home"), Box::new(move |_| Ok(snapshot.clone())),
            Box::new(|_| Ok(vec![ActiveDream { id: "d-a".into(), text: "The user reads the result first.".into(), position: 0 }])),
            Box::new(move |_, body: Value| {
                let answers = body["questions"].as_object().unwrap().keys()
                    .map(|id| (id.clone(), json!({"type": "noul", "noul": answer.unwrap_or(0.0)})))
                    .collect::<serde_json::Map<_, _>>();
                asked.lock().unwrap().push(body);
                answer.map(|_| json!({"answers": answers})).map_err(str::to_owned)
            }))
    }
    fn asked(&self) -> usize { self.asked.lock().unwrap().len() }
    fn last_question_ids(&self) -> Vec<String> {
        self.asked.lock().unwrap().last().unwrap()["questions"].as_object().unwrap().keys().cloned().collect()
    }
    fn set_turns(&self, count: usize) {
        let row = json!({"event": "turn", "agent_id": AGENT, "occurred_at": "2026-10-16T00:00:00Z"}).to_string();
        std::fs::create_dir_all(self.project().join("credits")).unwrap();
        std::fs::write(crate::credit_events_path(&self.project()), format!("{row}\n").repeat(count)).unwrap();
    }
    fn record(&self, key: &str, id: &str, ts: &str, turns: Option<u64>) {
        pushes::record(&self.paths(), &self.root.join("home"), &pushes::Delivery { agent_id: AGENT, message: &format!("seed-{key}"),
            session: key, project: None, ts, turns }, &[pushes::Pushed { id: id.into(), p: 0.9 }]).unwrap();
    }
    fn chathistory_rows(&self) -> Vec<Value> {
        std::fs::read_to_string(self.project().join("project-memory/chathistory/latest.jsonl")).unwrap_or_default()
            .lines().map(|line| serde_json::from_str(line).unwrap()).collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.root); }
}

fn input(id: &str) -> Input {
    let text = "fixture ".repeat(10);
    Input { message_id: id.into(), created_at: "2026-10-16T08:00:00Z".into(), origin: MessageOrigin::User,
        original_text: text.clone(), eligibility_text: text, target_agent_ids: vec![AGENT.into()] }
}

fn confirmed(service: &Service, delivery: Delivery) {
    let (done, wait) = mpsc::channel();
    service.confirm(delivery, move |_, _| { let _ = done.send(()); });
    wait.recv_timeout(Duration::from_secs(10)).unwrap();
}

#[test]
fn without_an_envelope_the_original_goes_out_and_nothing_is_written() {
    let fixture = Fixture::new();
    let on = settings::Settings::initial(true);
    let cases = [
        (fixture.service(on.clone(), false, Ok(0.99)), input("m-ready")),
        (fixture.service(settings::Settings::initial(false), true, Ok(0.99)), input("m-off")),
        (fixture.service(on.clone(), true, Ok(0.99)), Input { eligibility_text: "short".into(), ..input("m-short") }),
        (fixture.service(on.clone(), true, Ok(0.99)), Input { origin: MessageOrigin::A2a, ..input("m-bus") }),
        (fixture.service(on.clone(), true, Ok(0.99)), Input { target_agent_ids: vec!["agent-other".into()], ..input("m-other") }),
        (fixture.service(on.clone(), true, Err("jev_timeout")), input("m-error")),
        (fixture.service(on.clone(), true, Ok(0.1)), input("m-miss")),
    ];
    for (service, input) in &cases {
        assert!(service.prepare(fixture.scope("k"), input).is_none(), "{}", input.message_id);
        assert!(service.frozen.lock().unwrap().is_empty());
    }
    assert_eq!(fixture.asked(), 2); // Only the Jev error and the miss reached Jev.
    std::fs::remove_file(fixture.paths().marks()).unwrap();
    let no_dreams = settings::Settings { max_dreams: 0, ..on };
    assert!(fixture.service(no_dreams, true, Ok(0.99)).prepare(fixture.scope("k"), &input("m-none")).is_none());
    assert_eq!(fixture.asked(), 2);
    assert!(!fixture.paths().pushes().exists());
    assert!(!fixture.project().join("project-memory").exists());
}

#[test]
fn the_first_choice_is_reused_for_the_same_agent_and_message_without_asking_again() {
    let fixture = Fixture::new();
    let service = fixture.service(settings::Settings::initial(true), true, Ok(0.95));
    let first = service.prepare(fixture.scope("k"), &input("m-1")).unwrap();
    assert!(first.choice.envelope.starts_with("<KOTA_MARKS v=\"1\" revision=\"3\">\n"));
    assert!(first.choice.envelope.contains("- [m-a] ") && first.choice.envelope.contains("- [d-a] "));
    let again = service.prepare(fixture.scope("another-launch"), &input("m-1")).unwrap();
    assert!(Arc::ptr_eq(&first.choice, &again.choice));
    assert_eq!(fixture.asked(), 1);
    service.prepare(fixture.scope("k"), &input("m-2")).unwrap();
    assert_eq!(fixture.asked(), 2);
}

#[test]
fn confirmation_records_turns_and_one_context_row_and_a_repeat_changes_nothing() {
    let fixture = Fixture::new();
    let service = fixture.service(settings::Settings::initial(true), true, Ok(0.95));
    fixture.set_turns(4);
    let delivery = service.prepare(fixture.scope("k"), &input("m-1")).unwrap();
    let body = delivery.choice.display_body.clone();
    confirmed(&service, delivery);
    let rows = pushes::read_all(&fixture.paths()).unwrap();
    let mut ids = rows.iter().map(|push| push.id()).collect::<Vec<_>>();
    ids.sort();
    assert_eq!(ids, ["d-a", "m-a", "m-b"]);
    assert!(rows.iter().all(|push| push.message == "m-1" && push.session == "k" && push.turns == Some(4)));
    let context = |rows: Vec<Value>| rows.into_iter().filter(|row| row["kind"] == "context").collect::<Vec<_>>();
    let shown = context(fixture.chathistory_rows());
    assert_eq!(shown.len(), 1);
    assert_eq!((&shown[0]["role"], &shown[0]["agent_id"], &shown[0]["display"]), (&json!("system"), &json!(AGENT), &json!(true)));
    assert_eq!(shown[0]["text"], body);
    assert!(!body.contains("KOTA_MARKS"));
    fixture.set_turns(6);
    confirmed(&service, service.prepare(fixture.scope("k"), &input("m-1")).unwrap());
    assert_eq!(pushes::read_all(&fixture.paths()).unwrap(), rows);
    assert_eq!(context(fixture.chathistory_rows()), shown);
}

#[test]
fn a_held_local_delivery_confirms_on_its_first_accepted_or_consumed_notice_only() {
    let fixture = Fixture::new();
    let service = fixture.service(settings::Settings::initial(true), true, Ok(0.95));
    for id in ["m-1", "m-2"] {
        service.hold(format!("{id}#{AGENT}"), service.prepare(fixture.scope("k"), &input(id)).unwrap());
    }
    let (done, wait) = mpsc::channel();
    let notice = |kota: &str, state| { let done = done.clone(); service.local_delivery(kota, state, move |_, _| { let _ = done.send(()); }) };
    notice("m-1#agent-fixture", DeliveryState::Cleared); // An edited input may still be requeued as is.
    notice("m-2#agent-fixture", DeliveryState::Abandoned); // A closed session drops it unconfirmed.
    notice("m-1#agent-fixture", DeliveryState::Accepted);
    wait.recv_timeout(Duration::from_secs(10)).unwrap();
    notice("m-1#agent-fixture", DeliveryState::Consumed);
    assert!(service.held.lock().unwrap().is_empty());
    assert!(wait.recv_timeout(Duration::from_millis(200)).is_err());
    let rows = pushes::read_all(&fixture.paths()).unwrap();
    assert!(!rows.is_empty() && rows.iter().all(|push| push.message == "m-1"));
}

#[test]
fn cooldown_counts_replies_within_one_launch_and_utc_day() {
    let fixture = Fixture::new();
    let mut policy = settings::Settings::initial(true);
    policy.cooldown_replies = 2;
    let service = fixture.service(policy.clone(), true, Ok(0.0));
    let now = soul::now_ts();
    let yesterday = (Utc::now() - chrono::Duration::days(1)).to_rfc3339();
    fixture.record("k", "m-a", &now, Some(4));
    fixture.record("k-yesterday", "m-a", &yesterday, Some(4));
    fixture.record("k-unknown", "m-a", &now, None);
    fixture.record("k-carried", "m-a", &now, Some(9));
    let asks_m_a = |key: &str, turns: usize, id: &str| {
        fixture.set_turns(turns);
        assert!(service.prepare(fixture.scope(key), &input(id)).is_none());
        fixture.last_question_ids().contains(&"m-a".to_string())
    };
    assert!(!asks_m_a("k", 5, "c-1"));
    assert!(asks_m_a("k", 6, "c-2"));
    assert!(asks_m_a("k-new-launch", 5, "c-3"));
    assert!(asks_m_a("k-yesterday", 5, "c-4"));
    assert!(!asks_m_a("k-unknown", 50, "c-5")); // No Turns: launch and day alone.
    assert!(!asks_m_a("k-carried", 5, "c-6")); // A carried row may exceed this project's count.
    policy.cooldown_replies = 0;
    fixture.service(policy, true, Ok(0.0)).prepare(fixture.scope("k"), &input("c-7"));
    assert!(fixture.last_question_ids().contains(&"m-a".to_string()));
}

#[test]
fn the_previous_reply_is_this_agents_latest_own_reply() {
    let fixture = Fixture::new();
    let row = |id: &str, role: &str, agent: &str, text: &str, origin: Option<&str>| json!({"id": id, "ts": "2026-10-16T07:00:00Z",
        "role": role, "agent_id": agent, "shell": "claude", "kind": "message", "display": true, "agent_visible": true,
        "text": text, "source": {"session_id": "s"}, "message_origin": origin}).to_string();
    let latest = fixture.project().join("project-memory/chathistory/latest.jsonl");
    std::fs::create_dir_all(latest.parent().unwrap()).unwrap();
    std::fs::write(&latest, [row("r1", "assistant", AGENT, "older reply", None), row("r2", "assistant", AGENT, "latest reply", None),
        row("r3", "assistant", "agent-other", "another agent", None), row("r4", "assistant", AGENT, "handoff copy", Some("shell_handoff")),
        row("r5", "user", AGENT, "a user message", Some("user"))].join("\n")).unwrap();
    fixture.service(settings::Settings::initial(true), true, Ok(0.0)).prepare(fixture.scope("k"), &input("m-1"));
    assert_eq!(fixture.asked.lock().unwrap()[0]["state"]["previous_reply"], "latest reply");
}

#[test]
fn selection_waits_for_the_previous_confirmation_but_only_for_a_bounded_time() {
    let fixture = Fixture::new();
    let mut policy = settings::Settings::initial(true);
    policy.cooldown_replies = 2;
    let service = fixture.service(policy, true, Ok(0.0));
    fixture.set_turns(1);
    let (recorded, wait) = mpsc::channel::<()>();
    service.recording.lock().unwrap().insert(AGENT.into(), wait);
    let paths = fixture.paths();
    let home = fixture.root.join("home");
    let late = thread::spawn(move || {
        thread::sleep(Duration::from_millis(300));
        pushes::record(&paths, &home, &pushes::Delivery { agent_id: AGENT, message: "m-0", session: "k", project: None,
            ts: &soul::now_ts(), turns: Some(1) }, &[pushes::Pushed { id: "m-a".into(), p: 0.9 }]).unwrap();
        drop(recorded);
    });
    service.prepare(fixture.scope("k"), &input("m-1"));
    late.join().unwrap();
    assert!(!fixture.last_question_ids().contains(&"m-a".to_string()));

    let (stuck, wait) = mpsc::channel::<()>();
    service.recording.lock().unwrap().insert(AGENT.into(), wait);
    let started = Instant::now();
    service.prepare(fixture.scope("k"), &input("m-2"));
    assert!(started.elapsed() >= LEDGER_WAIT);
    assert_eq!(fixture.asked(), 2);
    drop(stuck);
}
