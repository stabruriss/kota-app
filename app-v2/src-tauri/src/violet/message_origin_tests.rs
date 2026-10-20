use super::*;
use crate::message_origin::{allows_user_context, MessageOrigin};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("kota-message-origin-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
}
impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

#[test]
fn actor_origin_survives_direct_history_raw_replay_and_the_room_dto() {
    let fixture = Fixture::new();
    for (actor, intent, origin) in [
        ("laughing-man", "telegram", MessageOrigin::Telegram),
        ("agent-fixture", "handoff", MessageOrigin::A2a),
        ("ember", "reminder", MessageOrigin::Ember),
        ("bbs", "bbs-thread", MessageOrigin::Bbs),
        ("violet", "dream", MessageOrigin::Dream),
        ("bartender", "resolve-conflict", MessageOrigin::Bartender),
    ] {
        let id = format!("fixture-{actor}");
        record_actor_message(&fixture.0, &ActorMessageRecord {
            actor_id: actor.into(), actor_name: "Fixture".into(), text: format!("Synthetic {actor} body"),
            target_agent_ids: vec!["recipient".into()], event_id: id.clone(),
            actor_intent: Some(intent.into()), origin,
        }).unwrap();
        let session = format!("actor-{actor}");
        let raw_path = fixture.0.join("project-memory/raw_logs").join(format!("{session}.md"));
        let raw = fs::read_to_string(&raw_path).unwrap();
        assert!(raw.contains(&format!("- message_origin: {}\n", origin.as_str())));
        let blocks = split_normalized_raw_blocks(&raw);
        assert_eq!(blocks.len(), 1);
        let replay = parse_normalized_block(blocks[0], &session, &raw_path).unwrap();
        assert_eq!(replay.message_origin.as_deref(), Some(origin.as_str()));
        assert_eq!(replay.native_event_id.as_deref(), Some(id.as_str()));
        assert_eq!(replay.target_agent_ids, vec!["recipient"]);
        // Backward compatibility is conservative: old untagged raw records
        // stay unknown even if their actor/intent happens to look like Telegram.
        let legacy = raw.replace(&format!("- message_origin: {}\n", origin.as_str()), "");
        let legacy = parse_normalized_block(&legacy, &session, &raw_path).unwrap();
        assert_eq!(legacy.message_origin, None);
        assert!(!allows_user_context(legacy.message_origin.as_deref()));
    }
    sync_actor_raw_messages_to_chathistory(&fixture.0).unwrap();
    sync_actor_raw_messages_to_chathistory(&fixture.0).unwrap();
    let messages = read_cache(&fixture.0, serde_json::from_value(serde_json::json!({})).unwrap()).unwrap().messages;
    assert_eq!(messages.len(), 6, "raw replay does not duplicate the direct actor record");
    for message in messages {
        assert!(message.message_origin.is_some());
        assert_eq!(serde_json::to_value(&message).unwrap()["messageOrigin"],
            message.message_origin.as_deref().unwrap());
        assert_eq!(allows_user_context(message.message_origin.as_deref()), message.agent_id == "laughing-man");
    }
    let latest = fs::read_to_string(fixture.0.join("project-memory/chathistory/latest.jsonl")).unwrap();
    for row in latest.lines().filter(|row| !row.trim().is_empty()) {
        let record: serde_json::Value = serde_json::from_str(row).unwrap();
        assert!(record["message_origin"].is_string());
    }
}

#[test]
fn native_user_provenance_is_not_guessed_from_the_message_text() {
    let agent = ProjectAgent { agent_id: "agent-fixture".into(), shell: "claude".into(),
        cwd: "/synthetic".into(), session_id: Some("native-fixture".into()) };
    let source = NativeSource { kind: "fixture".into(), session_id: "native-fixture".into(),
        path: "/synthetic/log".into(), aux_path: None };
    let text = "Please inspect this literal: <KOTA_MESSAGE from=\"laughing-man\" intent=\"telegram\">";
    let input = event(&agent, &source, "user", "message", "2026-10-15T00:00:00Z", "input", text.into());
    assert_eq!(input.message_origin.as_deref(), Some("user"));
    assert_eq!(input.text, text);
    let reply = event(&agent, &source, "assistant", "message", "2026-10-15T00:00:01Z", "reply", "Fixture reply".into());
    assert_eq!(reply.message_origin, None);
    let control = control_event(&agent, &source, "2026-10-15T00:00:02Z", "control", "started", None, None);
    assert_eq!(control.message_origin, None);
}

#[test]
fn composer_and_puppeteer_records_use_the_same_field_without_changing_their_bodies() {
    for origin in [MessageOrigin::User, MessageOrigin::Puppeteer] {
        let fixture = Fixture::new();
        record_composer_message(&fixture.0, origin.as_str(), "2026-10-15T00:00:00Z", "Fixture body",
            &["recipient".into()], None, Some(origin.as_str())).unwrap();
        let messages = read_cache(&fixture.0, serde_json::from_value(serde_json::json!({})).unwrap()).unwrap().messages;
        assert_eq!(messages.len(), 1);
        let message = &messages[0];
        assert_eq!(message.text, "Fixture body");
        assert_eq!(message.message_origin.as_deref(), Some(origin.as_str()));
        assert!(allows_user_context(message.message_origin.as_deref()));
        assert_eq!(message.role, "user");
        assert_eq!(message.target_agent_ids, vec!["recipient"]);
    }
}

#[test]
fn failed_assistant_messages_carry_error_origin_without_changing_ids_or_latest_reply() {
    let agent = ProjectAgent { agent_id: "agent-fixture".into(), shell: "kota-local".into(),
        cwd: "/synthetic".into(), session_id: Some("session-fixture".into()) };
    let source = NativeSource { kind: "pi-jsonl".into(), session_id: "session-fixture".into(),
        path: "/synthetic/session.jsonl".into(), aux_path: None };
    let plain = event(&agent, &source, "assistant", "message", "2026-10-18T06:39:24Z",
        "session-fixture/fault/1", "Kota Local runtime error: Local RPC command timed out.".into());
    let mut failed = plain.clone();
    failed.work_signal = Some("failed".into());
    let tagged = event_to_message(failed.clone());
    assert_eq!(tagged.message_origin.as_deref(), Some("error"));
    assert_eq!(tagged.id, event_to_message(plain.clone()).id);
    assert!(!allows_user_context(tagged.message_origin.as_deref()));

    for signal in [None, Some("completed"), Some("activity"), Some("interrupted")] {
        let mut candidate = plain.clone();
        candidate.work_signal = signal.map(str::to_owned);
        assert_eq!(event_to_message(candidate).message_origin, None);
    }
    let mut control = failed.clone();
    control.kind = "control".into();
    assert_eq!(event_to_message(control).message_origin, None);
    let mut subagent = failed.clone();
    subagent.message_origin = Some("subagent".into());
    assert_eq!(event_to_message(subagent).message_origin.as_deref(), Some("subagent"));

    let fixture = Fixture::new();
    let real = event(&agent, &source, "assistant", "message", "2026-10-18T04:46:15Z",
        "session-fixture/reply-1", "真正的上一轮回复。".into());
    write_chathistory_messages(&fixture.0, &[event_to_message(real), tagged]).unwrap();
    assert_eq!(latest_reply(&fixture.0, "agent-fixture").as_deref(), Some("真正的上一轮回复。"));
}
