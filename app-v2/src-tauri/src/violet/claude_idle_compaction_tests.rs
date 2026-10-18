use super::*;
use serde_json::json;

const NOTICE: &str = "Compacted while idle, before the prompt cache expired";

fn context() -> (ProjectAgent, NativeSource) {
    (
        ProjectAgent {
            agent_id: "claude-idle-test".into(),
            shell: "claude".into(),
            cwd: PathBuf::from("/tmp/unused-claude-idle-test"),
            session_id: Some("idle-session".into()),
        },
        NativeSource {
            kind: "claude-jsonl".into(),
            session_id: "idle-session".into(),
            path: PathBuf::from("/tmp/unused-claude-idle-test/session.jsonl"),
            aux_path: None,
        },
    )
}

fn notice() -> JsonValue {
    // Native shape observed in Claude Code 2.1.293 and 2.1.296. Only identifying
    // values are synthetic; isMeta=false is why the generic meta guard misses it.
    json!({
        "type": "system", "subtype": "informational", "level": "notice",
        "content": NOTICE, "isMeta": false, "version": "2.1.296",
        "uuid": "idle-notice", "sessionId": "idle-session",
        "timestamp": "2026-10-13T21:30:32.513Z"
    })
}

fn parse(record: JsonValue) -> Vec<NativeEvent> {
    let (agent, source) = context();
    parse_claude_line(&agent, &source, 0, record)
}

#[test]
fn native_notice_produces_no_message_or_work_signal() {
    for version in ["2.1.293", "2.1.296"] {
        let mut record = notice();
        record["version"] = json!(version);
        assert!(parse(record).is_empty());
    }
}

#[test]
fn every_envelope_condition_is_required() {
    for (field, replacement) in [
        ("type", "user"),
        ("type", "assistant"),
        ("subtype", "other"),
        ("level", "warning"),
    ] {
        let mut record = notice();
        record[field] = json!(replacement);
        let events = parse(record);
        assert_eq!(events.len(), 1, "{field}={replacement}");
        assert_eq!(events[0].text, NOTICE);
    }
    for field in ["type", "subtype", "level"] {
        let mut record = notice();
        record.as_object_mut().unwrap().remove(field);
        assert_eq!(parse(record).len(), 1, "missing {field}");
        let mut record = notice();
        record[field] = JsonValue::Null;
        assert_eq!(parse(record).len(), 1, "null {field}");
    }
}

#[test]
fn content_requires_exact_top_level_scalar_equality() {
    for content in [
        format!(" {NOTICE}"),
        format!("{NOTICE}\n"),
        format!("> {NOTICE}"),
        format!("```text\n{NOTICE}\n```"),
        format!("{NOTICE} — quoted by a person"),
        "A different informational notice".into(),
    ] {
        let mut record = notice();
        record["content"] = json!(content);
        assert_eq!(parse(record).len(), 1, "{content:?}");
    }
    let mut array_content = notice();
    array_content["content"] = json!([{ "type": "text", "text": NOTICE }]);
    assert_eq!(parse(array_content).len(), 1);

    let mut nested_content = notice();
    nested_content.as_object_mut().unwrap().remove("content");
    nested_content["message"] = json!({ "role": "system", "content": NOTICE });
    assert_eq!(parse(nested_content).len(), 1);
}

#[test]
fn human_and_assistant_can_quote_the_notice_or_its_entire_envelope() {
    for role in ["user", "assistant"] {
        for text in [NOTICE.to_string(), notice().to_string()] {
            let record = json!({
                "type": role, "uuid": "ordinary-chat",
                "timestamp": "2026-10-13T21:31:00Z",
                "origin": { "kind": "human" }, "promptSource": "typed",
                "message": { "role": role, "content": [{ "type": "text", "text": text }] }
            });
            let events = parse(record);
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].role, role);
            assert_eq!(events[0].text, text);
            let (room, shared) = split_for_violet_outputs(events, Path::new("/tmp/kota"));
            assert_eq!(room.len(), 1);
            assert_eq!(shared.len(), 1);
        }
    }
}

#[test]
fn codex_message_with_the_same_text_is_unchanged() {
    let (mut agent, mut source) = context();
    agent.shell = "codex".into();
    source.kind = "codex-jsonl".into();
    let record = json!({
        "type": "event_msg", "timestamp": "2026-10-13T21:31:00Z",
        "payload": { "type": "agent_message", "message": NOTICE }
    });
    let events = parse_codex_line(&agent, &source, 0, record);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].text, NOTICE);
}

#[test]
fn incremental_cache_keeps_chat_and_compaction_without_reintroducing_notice() {
    let root = std::env::temp_dir().join(format!("kota-claude-idle-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let (mut agent, mut source) = context();
    agent.cwd = root.clone();
    source.path = root.join("session.jsonl");
    let boundary = json!({
        "type": "system", "subtype": "compact_boundary", "uuid": "boundary",
        "timestamp": "2026-10-13T21:30:31Z", "content": "Conversation compacted",
        "compactMetadata": { "trigger": "auto", "preTokens": 200000, "postTokens": 4000 }
    });
    fs::write(&source.path, format!("{boundary}\n{}\n", notice())).unwrap();
    let first = parse_jsonl_source_incremental(&root, &agent, &source, parse_claude_line).unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].kind, "compaction");
    assert_eq!(native_work_event(&first[0]).unwrap().state, "idle");

    let reply = json!({
        "type": "assistant", "uuid": "ordinary-reply", "timestamp": "2026-10-13T21:31:00Z",
        "message": { "role": "assistant", "stop_reason": "end_turn", "content": NOTICE }
    });
    fs::OpenOptions::new()
        .append(true)
        .open(&source.path)
        .unwrap()
        .write_all(format!("{reply}\n").as_bytes())
        .unwrap();
    for _ in 0..2 {
        let events =
            parse_jsonl_source_incremental(&root, &agent, &source, parse_claude_line).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, "compaction");
        assert_eq!(events[1].role, "assistant");
        assert_eq!(events[1].text, NOTICE);
        assert_eq!(native_work_event(&events[1]).unwrap().state, "idle");
    }
    fs::remove_dir_all(root).unwrap();
}
