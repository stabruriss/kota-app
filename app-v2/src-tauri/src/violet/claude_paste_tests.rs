use super::*;
use serde_json::json;

fn fixture() -> JsonValue {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/claude-pasted-native.json"
    ))
    .unwrap()
}

fn context() -> (ProjectAgent, NativeSource) {
    let fixture = fixture();
    (
        ProjectAgent {
            agent_id: fixture["agentId"].as_str().unwrap().into(),
            shell: "claude".into(),
            cwd: PathBuf::from("/tmp/unused-claude-paste-test"),
            session_id: None,
        },
        NativeSource {
            kind: "claude-jsonl".into(),
            session_id: fixture["sessionId"].as_str().unwrap().into(),
            path: fixture["sourcePath"].as_str().unwrap().into(),
            aux_path: None,
        },
    )
}

fn wrapped(body: &str) -> String {
    format!("\n\n<pasted_content id=\"paste-1\">\n{body}\n</pasted_content id=\"paste-1\">\n")
}

fn record(content: JsonValue) -> JsonValue {
    json!({
        "type": "user", "uuid": "native-input", "timestamp": "2026-09-20T19:58:16.294Z",
        "origin": {"kind": "human"}, "promptSource": "typed",
        "message": {"role": "user", "content": content},
    })
}

fn parse(record: JsonValue) -> Vec<NativeEvent> {
    let (agent, source) = context();
    parse_claude_line(&agent, &source, 1, record)
}

#[test]
fn actual_claude_pastes_produce_one_handoff_and_two_original_messages() {
    let fixture = fixture();
    let (agent, source) = context();
    let mut rounds = Vec::new();
    for (index, record) in fixture["records"].as_array().unwrap().iter().enumerate() {
        let parsed = parse_claude_line(&agent, &source, index, record.clone());
        assert_eq!(parsed.len(), 1);
        assert_eq!(
            parsed[0].text,
            fixture["originals"][index].as_str().unwrap()
        );
        assert_eq!(
            parsed[0].native_event_id.as_deref(),
            record["uuid"].as_str()
        );
        assert_eq!(
            parsed[0].timestamp,
            record["timestamp"].as_str().unwrap().replace('Z', "+00:00")
        );
        assert_eq!(parsed[0].session_id, source.session_id);
        assert_eq!(parsed[0].source_path, source.path);
        assert_eq!(parsed[0].shell_handoff.is_some(), index == 0);
        let expanded = expand_shell_handoffs(parsed);
        if index == 0 {
            assert_eq!(expanded.len(), 2);
            assert_eq!(expanded[0].message_origin.as_deref(), Some("shell_handoff"));
            assert_eq!(
                expanded[0].native_event_id.as_deref(),
                Some("kota-handoff:0c00ed19-f5cc-4e27-ac44-8158c7c831d9")
            );
            assert!(native_work_event(&expanded[0]).is_none());
            assert!(!expanded[0].text.contains("UI-QA-20260920-CLAUDE-1"));
        } else {
            assert_eq!(expanded.len(), 1);
            assert_eq!(expanded[0].message_origin.as_deref(), Some("user"));
        }
        assert_eq!(
            native_work_event(expanded.last().unwrap()).unwrap().state,
            "working"
        );
        rounds.push(
            expanded
                .into_iter()
                .map(event_to_message)
                .collect::<Vec<_>>(),
        );
    }
    let actual = serde_json::to_string(&json!({"first": rounds[0], "second": rounds[1]})).unwrap();
    if std::env::var_os("KOTA_CLAUDE_PASTE_FIXTURE").is_some() {
        println!("KOTA_CLAUDE_PASTE_FIXTURE={actual}");
    } else {
        let expected = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../tests/fixtures/claude-pasted-projection.json"),
        )
        .unwrap();
        assert_eq!(format!("{actual}\n"), expected);
    }
}

#[test]
fn one_outer_layer_preserves_inner_bytes_without_decoding() {
    for body in [
        "ordinary input",
        " \n  snow 雪 &amp; <literal>\t\n ",
        "<pasted_content id=\"inner\">\ninner\n</pasted_content id=\"inner\">",
    ] {
        let input = wrapped(body);
        assert_eq!(claude_pasted_input(&input), Some(body));
        assert_eq!(parse(record(json!(input)))[0].text, body);
    }
    let mut top_level = record(JsonValue::Null);
    top_level["content"] = json!(wrapped("top-level string"));
    assert_eq!(parse(top_level)[0].text, "top-level string");
}

#[test]
fn malformed_quoted_empty_and_multiple_wrappers_are_not_normalized() {
    let valid = wrapped("text");
    let cases = [
        "ordinary unwrapped text".to_owned(),
        wrapped(""),
        "<pasted_content id=\"\">\ntext\n</pasted_content id=\"\">".into(),
        "<pasted_content id=\"bad id\">\ntext\n</pasted_content id=\"bad id\">".into(),
        "<pasted_content id=\"x\">\ntext\n</pasted_content id=\"y\">".into(),
        "<pasted_content id=\"x\">text</pasted_content id=\"x\">".into(),
        "<pasted_content id=\"x\">\ntext".into(),
        format!("Please review:{valid}"),
        format!("{valid} trailing prose"),
        format!("```xml\n{valid}\n```"),
        format!("{valid}between{valid}"),
        format!("{valid}\n<pasted_content id=\"x\">\nunterminated"),
    ];
    for input in cases {
        assert_eq!(claude_pasted_input(&input), None, "{input:?}");
        assert_eq!(parse(record(json!(input)))[0].text, input);
    }
}

#[test]
fn normalization_requires_typed_human_user_string_metadata() {
    let input = wrapped("text");
    for patch in [
        json!({"type": "assistant"}),
        json!({"type": "system"}),
        json!({"origin": {"kind": "system"}}),
        json!({"origin": null}),
        json!({"promptSource": "system"}),
        json!({"promptSource": null}),
    ] {
        let mut line = record(json!(input));
        for (key, value) in patch.as_object().unwrap() {
            line[key] = value.clone();
        }
        assert_eq!(parse(line)[0].text, input);
    }
    // Input submitted while Claude is mid-turn is stamped `queued`, not `typed`.
    let mut queued = record(json!(input));
    queued["promptSource"] = json!("queued");
    assert_eq!(parse(queued)[0].text, "text");
    // Native tool blocks retain their existing parser path.
    let tools = parse(record(json!([{"type": "tool_result", "content": input}])));
    assert_eq!(tools[0].kind, "tool");
    assert_eq!(tools[0].text, input);
    for patch in [
        json!({"isMeta": true}),
        json!({"isCompactSummary": true}),
        json!({"origin": {"kind": "task-notification"}, "promptSource": "system"}),
    ] {
        let mut line = record(json!(input));
        for (key, value) in patch.as_object().unwrap() {
            line[key] = value.clone();
        }
        assert!(parse(line).is_empty());
    }
}

#[test]
fn wrapped_bus_uses_existing_hidden_envelope_and_receipt_rules() {
    let bus = "<KOTA_MESSAGE id=\"agentbus-paste-test\" from=\"agent-a\" to=\"agent-fixture73\" intent=\"handoff\">\nA multiline message.\n</KOTA_MESSAGE>";
    let plain = parse(record(json!(bus)));
    let pasted = parse(record(json!(wrapped(bus))));
    assert_eq!(
        serde_json::to_value(&pasted).unwrap(),
        serde_json::to_value(&plain).unwrap()
    );
    assert_eq!(
        agent_bus_receipt_from_event(&pasted[0]).unwrap().event_id,
        "agentbus-paste-test"
    );
    assert!(filter_internal_agent_bus_envelopes(pasted).is_empty());
}

#[test]
fn queued_and_coalesced_bus_pastes_stay_hidden_with_one_receipt_each() {
    let bus_a = "<KOTA_MESSAGE id=\"agentbus-paste-a\" from=\"agent-a\" to=\"agent-fixture73\" intent=\"handoff\">\nfirst\n</KOTA_MESSAGE>";
    let bus_b = "<KOTA_MESSAGE id=\"agentbus-paste-b\" from=\"agent-b\" to=\"agent-fixture73\" intent=\"review\">\nsecond\n</KOTA_MESSAGE>";
    let plain = parse(record(json!(bus_a)));
    let mut queued = record(json!(wrapped(bus_a)));
    queued["promptSource"] = json!("queued");
    let queued = parse(queued);
    assert_eq!(
        serde_json::to_value(&queued).unwrap(),
        serde_json::to_value(&plain).unwrap()
    );
    assert!(filter_internal_agent_bus_envelopes(queued).is_empty());

    // Two bus deliveries can land in one Claude submission as consecutive wrappers.
    let coalesced = format!("{}{}", wrapped(bus_a), wrapped(bus_b));
    assert_eq!(
        claude_pasted_inputs(&coalesced),
        Some(vec![bus_a, bus_b])
    );
    let events = parse(record(json!(coalesced)));
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].text, bus_a);
    assert_eq!(events[1].text, bus_b);
    assert_eq!(events[0].native_event_id.as_deref(), Some("native-input:0"));
    assert_eq!(events[1].native_event_id.as_deref(), Some("native-input:1"));
    let receipts = events
        .iter()
        .filter_map(agent_bus_receipt_from_event)
        .map(|receipt| receipt.event_id)
        .collect::<Vec<_>>();
    assert_eq!(receipts, ["agentbus-paste-a", "agentbus-paste-b"]);
    assert!(filter_internal_agent_bus_envelopes(events).is_empty());

    // Coalesced ordinary pastes are still plain user messages, one per paste.
    let events = parse(record(json!(format!("{}{}", wrapped("one"), wrapped("two")))));
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].text, "one");
    assert_eq!(events[1].text, "two");
    assert!(events.iter().all(|event| event.role == "user" && event.kind == "message"));
}

#[test]
fn image_attachment_pastes_unwrap_the_text_block_and_keep_the_markers() {
    // Claude submits an attached image as blocks; the typed text follows the
    // image block with attachment markers before the wrapper.
    let image = json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AA=="}});
    let text = "[Image #7]\n\n<pasted_content id=\"aa0e\">\n是这个么，但是怎么还不统一呢？\n</pasted_content id=\"aa0e\">\n";
    let events = parse(record(json!([image, {"type": "text", "text": text}])));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].text, "[Image #7]\n\n是这个么，但是怎么还不统一呢？");
    assert_eq!(events[0].native_event_id.as_deref(), Some("native-input:1"));
    assert_eq!(events[0].kind, "message");

    // Non-paste text blocks and tool blocks keep their native bytes.
    let plain = parse(record(json!([image, {"type": "text", "text": "[Image #7]\nplain"}])));
    assert_eq!(plain[0].text, "[Image #7]\nplain");
    let mixed = "[Image #7]\n\n<pasted_content id=\"x\">\ntext\n</pasted_content id=\"x\"> more";
    let mixed_events = parse(record(json!([image, {"type": "text", "text": mixed}])));
    assert_eq!(mixed_events[0].text, mixed);
    let tools = parse(record(json!([{"type": "tool_result", "content": wrapped("text")}])));
    assert_eq!(tools[0].kind, "tool");
    assert_eq!(tools[0].text, wrapped("text"));
}

#[test]
fn attachment_handoff_long_bus_and_privacy_keep_existing_order() {
    let fixture = fixture();
    let first = fixture["records"][0]["message"]["content"]
        .as_str()
        .unwrap();
    let inner = claude_pasted_input(first).unwrap();
    let handoff = inner
        .strip_suffix(fixture["originals"][0].as_str().unwrap())
        .unwrap();
    let bus = format!("<KOTA_MESSAGE id=\"agentbus-long-paste\" from=\"agent-a\" to=\"agent-fixture73\" intent=\"handoff\">\n{}\n</KOTA_MESSAGE>", "a".repeat(12_000));
    let original = format!("[Image #13]\n{bus}");
    let pasted = parse(record(json!(wrapped(&format!(
        "[Image #13]\n{handoff}{bus}"
    )))));
    assert_eq!(
        pasted[0].text,
        truncate_chars(&original, MAX_EVENT_TEXT_CHARS)
    );
    let expanded = expand_shell_handoffs(pasted);
    assert_eq!(expanded.len(), 2);
    assert!(native_work_event(&expanded[0]).is_none());
    assert_eq!(
        agent_bus_receipt_from_event(&expanded[1]).unwrap().event_id,
        "agentbus-long-paste"
    );
    assert_eq!(
        filter_internal_agent_bus_envelopes(expanded.clone()).len(),
        1
    );
    let spans = [PrivacySpan {
        agent_id: fixture["agentId"].as_str().unwrap().into(),
        started_at: "2026-09-20T19:00:00Z".into(),
        ended_at: Some("2026-09-20T20:00:00Z".into()),
    }];
    assert!(partition_private(expanded, &spans).0.is_empty());
}

#[test]
fn temporal_gap_input_keeps_the_same_existing_projection() {
    let input = "<KOTA_TEMPORAL_GAP v=\"1\" current_time=\"2026-09-20T12:00:00-07:00\">\nIt has been over 24 hours since your last completed response in this room.\n</KOTA_TEMPORAL_GAP>\nContinue the task.";
    let plain = parse(record(json!(input)));
    let pasted = parse(record(json!(wrapped(input))));
    assert_eq!(
        serde_json::to_value(&pasted).unwrap(),
        serde_json::to_value(&plain).unwrap()
    );
    assert_eq!(pasted[0].text, input);
    assert!(pasted[0].shell_handoff.is_none());
}

#[test]
fn existing_cache_stays_wrapped_and_only_new_native_lines_are_normalized() {
    let root = std::env::temp_dir().join(format!("kota-claude-paste-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let (agent, mut source) = context();
    source.path = root.join("native.jsonl");
    let old = record(json!(wrapped("old cached input")));
    let mut new = record(json!(wrapped("new input")));
    new["uuid"] = json!("new-native-input");
    new["timestamp"] = json!("2026-09-20T19:59:00.000Z");
    let old_line = format!("{old}\n");
    let native_bytes = format!("{old_line}{new}\n");
    fs::write(&source.path, &native_bytes).unwrap();
    let old_event = event(
        &agent,
        &source,
        "user",
        "message",
        old["timestamp"].as_str().unwrap(),
        "native-input",
        old["message"]["content"].as_str().unwrap().into(),
    );
    assert_eq!(PARSED_EVENT_CACHE_VERSION, "8");
    let key = source_cache_key(&source);
    write_cached_native_events(
        &parsed_event_cache_path(&root, &key).unwrap(),
        &[old_event.clone()],
    )
    .unwrap();
    write_source_cursor(
        &source_cursor_path(&root, &key).unwrap(),
        &SourceCursor {
            incarnation: None,
            path: path_string(&source.path),
            offset: old_line.len() as u64,
            line_index: 1,
            updated_at: now_iso(),
            model: None,
            effort: None,
            codex_sandbox_policy: None,
        },
    )
    .unwrap();
    let parsed = parse_jsonl_source_incremental(&root, &agent, &source, parse_claude_line).unwrap();
    assert_eq!(parsed.len(), 2);
    assert_eq!(
        serde_json::to_value(&parsed[0]).unwrap(),
        serde_json::to_value(&old_event).unwrap()
    );
    assert_eq!(parsed[1].text, "new input");
    assert_eq!(fs::read_to_string(&source.path).unwrap(), native_bytes);
    let reread = parse_jsonl_source_incremental(&root, &agent, &source, parse_claude_line).unwrap();
    assert_eq!(
        serde_json::to_value(&reread).unwrap(),
        serde_json::to_value(&parsed).unwrap()
    );
    fs::remove_dir_all(root).unwrap();
}
