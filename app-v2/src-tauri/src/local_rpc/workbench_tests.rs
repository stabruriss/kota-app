use super::*;
use crate::local_rpc::{protocol::fixture, OnlineState};
use serde_json::json;

fn snapshot() -> Snapshot {
    Snapshot { scope: LocalScope { project_root: "/synthetic/project".into(), agent_id: "agent-copper".into(), session_id: "test-session".into(), epoch: 1 },
        state: OnlineState::Busy, queued: 0, compacting: false, child_pid: None, model: None, thinking_level: None, error: None }
}

#[test]
fn stream_final_and_cumulative_tool_output_replace_in_place_and_remain_until_next_run() {
    let mut view = Workbench::default();
    view.event(&json!({"type":"agent_start"}));
    view.event(&json!({"type":"message_start","message":{"role":"assistant"}}));
    for delta in ["First", " answer"] {
        view.event(&json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":delta}}));
    }
    view.event(&json!({"type":"message_end","message":{"role":"assistant","stopReason":"toolUse","content":[{"type":"text","text":"First answer"}]}}));
    for (kind, text) in [("tool_execution_start", ""), ("tool_execution_update", "one"), ("tool_execution_update", "one two"), ("tool_execution_end", "one two three")] {
        view.event(&json!({"type":kind,"toolCallId":"t1","toolName":"read", "partialResult":{"content":[{"type":"text","text":text}]},
            "result":{"content":[{"type":"text","text":text},{"type":"image","data":"NOT_FOR_WEBVIEW"}]}}));
    }
    assert_eq!(view.blocks.len(), 2);
    assert_eq!(view.blocks[0].kind, "progress");
    assert_eq!(view.blocks[1].text, "one two three");
    assert!(!view.blocks[1].active);
    for kind in ["auto_retry_start", "agent_end", "compaction_start", "compaction_end", "agent_start"] { view.event(&json!({"type":kind})); }
    assert_eq!(view.run, 1);
    assert_eq!(view.blocks.len(), 2);
    view.event(&json!({"type":"message_start","message":{"role":"assistant"}}));
    view.event(&json!({"type":"message_end","message":{"role":"assistant","stopReason":"stop","content":[{"type":"text","text":"Done"}]}}));
    assert_eq!(view.blocks[2].kind, "end");
    let serialized = serde_json::to_string(&view.read(snapshot(), None)).unwrap();
    assert!(!serialized.contains("NOT_FOR_WEBVIEW"));
    view.event(&json!({"type":"agent_settled"}));
    assert_eq!(view.blocks.len(), 3);
    assert_eq!(view.blocks[2].kind, "end");
    assert_eq!(view.blocks[2].text, "Done");
    assert!(view.blocks.iter().all(|block| !block.active));
    assert!(view.started_at.is_none()); assert!(view.phase.is_empty());
    view.event(&json!({"type":"agent_start"}));
    assert_eq!(view.run, 2);
    assert!(view.blocks.is_empty()); assert_eq!(view.dropped, 0);
    assert!(view.started_at.is_some());
}

#[test]
fn settled_finishes_partial_blocks_without_losing_their_text_or_clock_origin() {
    let mut view = Workbench::default();
    view.event(&json!({"type":"agent_start"}));
    view.event(&json!({"type":"message_start","message":{"role":"assistant"}}));
    view.event(&json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","contentIndex":0,"delta":"Visible thought"}}));
    view.event(&json!({"type":"tool_execution_update","toolCallId":"partial","toolName":"read",
        "partialResult":{"content":[{"type":"text","text":"Partial tool output"}]}}));
    let started = view.blocks[0].started_at.clone();
    assert!(view.blocks.iter().all(|block| block.active));
    view.event(&json!({"type":"agent_settled"}));
    assert_eq!(view.blocks[0].text, "Visible thought");
    assert_eq!(view.blocks[0].started_at, started);
    assert_eq!(view.blocks[1].text, "Partial tool output");
    assert!(view.blocks.iter().all(|block| !block.active));
    let retained = view.read(snapshot(), None);
    assert_eq!(retained.blocks.unwrap().len(), 2);
    assert!(Workbench::default().blocks.is_empty(), "a new owner cannot inherit display blocks");
}

#[test]
fn thinking_start_is_recorded_once_and_preserved_across_deltas_and_completion() {
    let mut view = Workbench::default();
    view.event(&json!({"type":"agent_start"}));
    view.event(&json!({"type":"message_start","message":{"role":"assistant"}}));
    view.event(&json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_start","contentIndex":0}}));
    let first = view.blocks[0].started_at.as_deref().unwrap();
    assert!(chrono::DateTime::parse_from_rfc3339(first).is_ok());
    assert_eq!(view.blocks[0].text, "");
    // Make a reset distinguishable even on clocks with millisecond resolution.
    let first = "2026-10-04T12:00:00+00:00";
    view.blocks[0].started_at = Some(first.into());
    for event in [
        json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","contentIndex":0,"delta":"Visible summary"}}),
        json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_end","contentIndex":0,"content":"Visible summary"}}),
        json!({"type":"message_end","message":{"role":"assistant","stopReason":"toolUse","content":[{"type":"thinking","thinking":"Visible summary"}]}}),
    ] {
        view.event(&event);
        assert_eq!(view.blocks[0].started_at.as_deref(), Some(first));
    }
    assert_eq!(view.blocks[0].text, "Visible summary");
    assert!(!view.blocks[0].active);
    let revision = view.revision;
    let cursor = Cursor { scope: snapshot().scope, revision, queue_revision: Some(view.queue_revision) };
    for _ in 0..3 {
        assert!(view.read(snapshot(), Some(&cursor)).blocks.is_none());
        assert_eq!(view.revision, revision, "display reads must not advance the stream");
    }
    view.event(&json!({"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":1}}));
    view.event(&json!({"type":"tool_execution_start","toolCallId":"tool-1","toolName":"read"}));
    let blocks = serde_json::to_value(view.read(snapshot(), None)).unwrap()["blocks"].as_array().unwrap().clone();
    assert_eq!(blocks[0]["startedAt"], first);
    assert!(blocks[1].get("startedAt").is_none());
    assert!(blocks[2].get("startedAt").is_none());
}

#[test]
fn high_rate_long_unicode_output_has_a_fixed_budget_and_unchanged_reads_omit_bodies() {
    let mut view = Workbench::default();
    for i in 0..600 {
        view.event(&json!({"type":"tool_execution_update","toolCallId":format!("tool-{i}"),"toolName":"read",
            "partialResult":{"content":[{"type":"text","text":"界".repeat(12000)}]}}));
    }
    assert!(view.blocks.len() <= BLOCKS);
    assert!(view.blocks.iter().all(|b| b.text.len() <= BLOCK_LIMIT));
    assert!(view.blocks.iter().map(|b| b.text.len()).sum::<usize>() <= TEXT_LIMIT);
    assert!(view.dropped > 0);
    let cursor = Cursor { scope: snapshot().scope, revision: view.revision, queue_revision: Some(view.queue_revision) };
    let same = view.read(snapshot(), Some(&cursor));
    assert!(same.blocks.is_none()); assert!(same.queue.is_none());
    let mut replaced = snapshot(); replaced.scope.epoch += 1;
    assert!(view.read(replaced, Some(&cursor)).blocks.is_some(), "new scope cannot reuse an old revision");
    view.clear_run();
    assert!(view.read(snapshot(), Some(&cursor)).blocks.unwrap().is_empty());
}

#[test]
fn queue_is_ledger_ordered_bounded_text_only_and_consumed_items_disappear() {
    let root = std::env::temp_dir().join(format!("local-workbench-{}", uuid::Uuid::new_v4()));
    let mut ledger = Ledger::open(&root.join("ledger.jsonl")).unwrap();
    let mut input = fixture("queue-message");
    input.content = json!([{"type":"text","text":"a".repeat(2000)},{"type":"image","data":"secret-image","mimeType":"image/png"}]);
    // Text-only projection is independent of any attachment metadata shape.
    let (text, truncated) = plain_text(&input.content, 384);
    assert_eq!(text.len(), 384); assert!(truncated); assert!(!text.contains("secret-image"));
    input.content = json!("a".repeat(2000));
    ledger.hold("test-session", input.clone()).unwrap();
    let mut view = Workbench::default(); view.refresh_queue(&ledger, "test-session");
    assert_eq!(view.queue_total, 1); assert!(view.queue[0].truncated);
    ledger.record("test-session", &input.kota_id, DeliveryState::Consumed).unwrap();
    view.refresh_queue(&ledger, "test-session");
    assert!(view.queue.is_empty());
    drop(ledger); std::fs::remove_dir_all(root).unwrap();
}

fn render_input(source: Option<(&str, &str, &str)>, body: &str,
    timing: Option<crate::agent_bus::AgentBusTerminalTiming>) -> Injection {
    let now: chrono::DateTime<chrono::Local> = chrono::DateTime::parse_from_rfc3339("2026-10-04T00:00:00Z").unwrap().into();
    let (content, custom_type, source) = if let Some((actor, intent, custom_type)) = source {
        let message = crate::agent_bus::ActorMessage { project_root: "/synthetic/project".into(), actor_id: actor.into(),
            actor_name: "Fixture".into(), target_agent_id: "agent-fable-copper".into(), intent: intent.into(),
            text: body.into(), event_id: "event-fixture-1".into(), dedupe_key: None, launch_request: None,
            terminal_timing: timing, soul_eligibility: None,
            origin: crate::message_origin::MessageOrigin::for_actor(actor.starts_with("agent-"), actor, intent) };
        (crate::agent_bus::render_terminal_message_with_body_at(&message, body, now), custom_type, Some((actor, intent)))
    } else { (body.into(), "kota.user", None) };
    let input = Injection::text("event-fixture-1", "agent-fable-copper", now.to_rfc3339(), false, custom_type, content, source);
    input.validate("agent-fable-copper").unwrap();
    input
}

fn temporal(body: &str) -> String {
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-04T00:00:00Z").unwrap().into();
    crate::temporal_context::render_temporal_gap_payload(body, now)
}

fn handoff(body: &str) -> String {
    use crate::pty::agent::AgentCli;
    let prefix = crate::shell_switch::handoff_text("db4e41c6-ff8a-4c2d-96b5-a6c0caa8c991", "agent-fable-copper", "Copper",
        "fixture-project", "Fixture", std::path::Path::new("/synthetic/project-memory"), AgentCli::Claude, AgentCli::KotaLocal,
        "2026-10-04T00:00:00Z");
    format!("{prefix}{body}")
}

#[test]
fn queue_projection_uses_real_renderers_for_all_seven_sources_and_never_changes_input() {
    let sources = [None, Some(("agent-fable-silver", "handoff", "kota.bus")),
        Some(("laughing-man", "telegram", "kota.lm")), Some(("bbs", "bbs-thread", "kota.bbs")),
        Some(("ember", "reminder", "kota.ember")), Some(("violet", "dream", "kota.dream")),
        Some(("bartender", "resolve-conflict", "kota.bartender"))];
    let root = std::env::temp_dir().join(format!("local-queue-projection-{}", uuid::Uuid::new_v4()));
    for (i, source) in sources.into_iter().enumerate() {
        let input = render_input(source, "真实正文 · 中日韩 👩‍💻\nNo wrappers here.", None);
        let original = input.clone();
        let ledger_path = root.join(format!("{i}.jsonl"));
        let mut ledger = Ledger::open(&ledger_path).unwrap();
        ledger.hold("test-session", input.clone()).unwrap();
        let original_bytes = std::fs::read(&ledger_path).unwrap();
        let mut view = Workbench::default(); view.refresh_queue(&ledger, "test-session");
        assert_eq!(view.queue[0].summary, "真实正文 · 中日韩 👩‍💻\nNo wrappers here.");
        assert_eq!(queue_text(&input, 16 * 1024), (view.queue[0].summary.clone(), false));
        assert_eq!(view.queue[0].sender_id, input.details.source_actor_id);
        assert_eq!(view.queue[0].custom_type, input.custom_type);
        assert_eq!(view.queue[0].created_at, input.details.created_at);
        assert_eq!(ledger.get("test-session", &input.kota_id).unwrap().injection, original);
        assert_eq!(std::fs::read(&ledger_path).unwrap(), original_bytes);
        assert_eq!(input, original);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn complete_handoff_and_temporal_prefixes_are_projected_in_the_actual_injection_order() {
    for source in [None, Some(("laughing-man", "telegram", "kota.lm")), Some(("agent-fable-silver", "handoff", "kota.bus"))] {
        let mut input = render_input(source, &temporal("跨天正文 👩‍💻\nnext line"), None);
        assert_eq!(queue_text(&input, 384), ("跨天正文 👩‍💻\nnext line".into(), false));
        input.content = json!(handoff(input.content.as_str().unwrap()));
        let original = input.clone();
        assert_eq!(queue_text(&input, 16 * 1024), ("跨天正文 👩‍💻\nnext line".into(), false));
        assert_eq!(input, original);
    }
    let input = render_input(None, &handoff("plain composer text"), None);
    assert_eq!(queue_text(&input, 384), ("plain composer text".into(), false));
}

#[test]
fn a_soul_envelope_is_peeled_after_the_handoff_and_before_the_gap() {
    let marks = "<KOTA_MARKS v=\"1\" revision=\"3\">\n- [m-a] Note.\n</KOTA_MARKS>\n";
    let mut input = render_input(None, &format!("{marks}{}", temporal("正文")), None);
    assert_eq!(queue_text(&input, 384), ("正文".into(), false));
    input.content = json!(handoff(input.content.as_str().unwrap()));
    assert_eq!(queue_text(&input, 384), ("正文".into(), false));
    let unclosed = "<KOTA_MARKS v=\"1\" revision=\"3\">\nnever closed";
    assert_eq!(queue_text(&render_input(None, unclosed, None), 384), (unclosed.into(), false));
}

#[test]
fn literal_tags_and_complete_quoted_envelopes_are_never_recursively_removed() {
    let literal_envelope = render_input(Some(("agent-fable-silver", "handoff", "kota.bus")), "quoted message", None)
        .content.as_str().unwrap().to_owned();
    for body in ["<KOTA_MESSAGE>\nLiteral example.\n</KOTA_MESSAGE>".to_owned(),
        format!("Example:\n{literal_envelope}"), literal_envelope.clone(),
        "Code: `<KOTA_MESSAGE>` and `<KOTA_TEMPORAL_GAP>` remain text.".into(),
        format!("Quoted prefix:\n{}", temporal("literal temporal example")),
        handoff("literal handoff inside the Actor body")] {
        let input = render_input(Some(("agent-fable-silver", "handoff", "kota.bus")), &body, None);
        assert_eq!(queue_text(&input, 16 * 1024), (body, false));
    }
    // Even matching IDs in Composer content do not establish Actor provenance.
    let input = render_input(None, &literal_envelope, None);
    assert_eq!(queue_text(&input, 16 * 1024), (literal_envelope, false));
    let quoted = format!("Example:\n{}", handoff("quoted text"));
    assert_eq!(queue_text(&render_input(None, &quoted, None), 16 * 1024), (quoted, false));
}

#[test]
fn malformed_multiple_or_mismatched_actor_envelopes_remain_entirely_unchanged() {
    let original = render_input(Some(("agent-fable-silver", "handoff", "kota.bus")), "body", None);
    let envelope = original.content.as_str().unwrap();
    let variants = [format!("leading text\n{envelope}"), format!("{envelope}\nextra text"),
        format!("\n{envelope}"), format!("{envelope}\n"), format!("{envelope}\n{envelope}"),
        envelope.replace("\n</KOTA_MESSAGE>", ""), envelope.replace("id=\"event-fixture-1\"", "id=\"another-id\""),
        envelope.replace("from=\"agent-fable-silver\"", "from=\"another-actor\""),
        envelope.replace("to=\"agent-fable-copper\"", "to=\"another-target\""),
        envelope.replace("intent=\"handoff\"", "intent=\"another-intent\""),
        envelope.replace("intent=\"handoff\">", "intent=\"handoff\" unknown=\"1\">"),
        "<KOTA_MESSAGE id=\"incomplete\">\nbody\n</KOTA_MESSAGE>".into()];
    for text in variants {
        let mut input = original.clone(); input.content = json!(text);
        assert_eq!(queue_text(&input, 16 * 1024), (text, false));
    }
    let mut input = original.clone(); input.details.source_actor_id = None;
    assert_eq!(queue_text(&input, 16 * 1024), (envelope.into(), false));
    input = original; input.custom_type = "future-source".into();
    assert_eq!(queue_text(&input, 16 * 1024), (input.content.as_str().unwrap().into(), false));
}

#[test]
fn a_failed_inner_prefix_keeps_all_outer_wrappers_instead_of_partially_stripping() {
    let gap = temporal("body");
    let bad_gaps = [gap.replace("</KOTA_TEMPORAL_GAP>\n", ""), gap.replace("It has been over 24 hours", "unknown copy"),
        gap.replace("v=\"1\"", "v=\"2\""), gap.replace("current_time=\"", "current_time=\"not-a-time"),
        "<KOTA_TEMPORAL_GAP>\nliteral or incomplete".into()];
    for gap in bad_gaps {
        for source in [None, Some(("laughing-man", "telegram", "kota.lm"))] {
            let mut input = render_input(source, &gap, None);
            input.content = json!(handoff(input.content.as_str().unwrap()));
            let text = input.content.as_str().unwrap().to_owned();
            assert_eq!(queue_text(&input, 16 * 1024), (text, false));
        }
    }
    for bad in [handoff("body").replace("</KOTA_HANDOFF>", ""),
        handoff("body").replace("agent-fable-copper", "another-agent"),
        handoff("body").replace("db4e41c6-ff8a-4c2d-96b5-a6c0caa8c991", "not-a-uuid")] {
        let input = render_input(None, &bad, None);
        assert_eq!(queue_text(&input, 16 * 1024), (bad, false));
    }
}

#[test]
fn generated_ember_timing_attributes_and_escaped_source_attributes_are_supported() {
    use crate::agent_bus::{AgentBusTerminalTiming, AgentBusTimingTrigger};
    let now: chrono::DateTime<chrono::Local> = chrono::DateTime::parse_from_rfc3339("2026-10-04T00:00:00Z").unwrap().into();
    for trigger in [AgentBusTimingTrigger::Scheduled, AgentBusTimingTrigger::Idle, AgentBusTimingTrigger::Manual] {
        let timing = AgentBusTerminalTiming { scheduled_for: (trigger == AgentBusTimingTrigger::Scheduled)
            .then(|| now.to_rfc3339_opts(chrono::SecondsFormat::Secs, false)), trigger };
        let input = render_input(Some(("ember", "reminder", "kota.ember")), "reminder body", Some(timing));
        assert_eq!(queue_text(&input, 384), ("reminder body".into(), false));
        let text = input.content.as_str().unwrap().replace(" current_time=\"", " extra=\"unknown\" current_time=\"");
        let mut bad = input.clone(); bad.content = json!(text);
        assert_eq!(queue_text(&bad, 16 * 1024), (text, false));
    }
    let input = render_input(Some(("actor<&\"", "intent>&\"", "kota.bus")), "escaped header body", None);
    assert_eq!(queue_text(&input, 384), ("escaped header body".into(), false));
}

#[test]
fn both_display_limits_apply_after_peeling_and_clip_on_utf8_boundaries() {
    let body = format!("{}👩‍💻{}", "界".repeat(127), "日本語한글👩‍💻".repeat(2000));
    let mut input = render_input(Some(("laughing-man", "telegram", "kota.lm")), &temporal(&body), None);
    input.content = json!(handoff(input.content.as_str().unwrap()));
    for limit in [0, 1, 2, 3, 383, 384, 385, 16 * 1024] {
        let (text, truncated) = queue_text(&input, limit);
        assert_eq!((text.clone(), truncated), plain_text(&json!(body), limit));
        assert!(text.len() <= limit && body.is_char_boundary(text.len()));
        assert!(!text.contains("KOTA_")); assert!(truncated);
    }
    let short = render_input(Some(("bbs", "bbs-thread", "kota.bbs")), &"a".repeat(384), None);
    assert_eq!(queue_text(&short, 384), ("a".repeat(384), false), "wrapper bytes must not consume the summary budget");
}

#[test]
fn mixed_media_never_crosses_display_and_ambiguous_multi_text_content_stays_plain() {
    let mut input = render_input(Some(("agent-fable-silver", "handoff", "kota.bus")), "body 👩‍💻", None);
    let envelope = input.content.clone();
    input.content = json!([{"type":"image","mimeType":"image/png","data":"c2VjcmV0LWltYWdl"},
        {"type":"text","text":envelope}]);
    let original = input.clone();
    assert_eq!(queue_text(&input, 384), ("body 👩‍💻".into(), false));
    assert_eq!(input, original);
    input.content.as_array_mut().unwrap().push(json!({"type":"text","text":"More text; no reliable single wrapper boundary."}));
    assert_eq!(queue_text(&input, 16 * 1024), plain_text(&input.content, 16 * 1024));
}

#[test]
fn successful_compaction_estimate_survives_settled_but_not_a_new_attempt_or_owner() {
    for reason in ["manual", "threshold", "overflow"] {
        let mut view = Workbench::default();
        view.event(&json!({"type":"compaction_start","reason":reason}));
        assert!(view.read(snapshot(), None).context_estimate.is_none());
        view.event(&json!({"type":"compaction_end","reason":reason,"aborted":false,"willRetry":false,
            "result":{"estimatedTokensAfter":400,"summary":"NOT_FOR_WEBVIEW","details":{"readFiles":["PRIVATE_PATH"]}}}));
        assert_eq!(view.read(snapshot(), None).context_estimate, Some(400.0));
        view.event(&json!({"type":"agent_settled"}));
        let cursor = Cursor { scope: snapshot().scope, revision: view.revision, queue_revision: Some(view.queue_revision) };
        let serialized = serde_json::to_value(view.read(snapshot(), Some(&cursor))).unwrap();
        assert_eq!(serialized["contextEstimate"], 400.0);
        assert!(serialized.get("blocks").is_none());
        assert!(!serialized.to_string().contains("NOT_FOR_WEBVIEW"));
        assert!(!serialized.to_string().contains("PRIVATE_PATH"));
        let mut replacement = snapshot(); replacement.scope.epoch += 1; replacement.scope.session_id = "next-session".into();
        // Runtime creates a fresh Workbench for each immutable scope/owner.
        assert!(Workbench::default().read(replacement, None).context_estimate.is_none());
        view.event(&json!({"type":"compaction_start","reason":reason}));
        assert!(view.read(snapshot(), None).context_estimate.is_none());
    }
}

#[test]
fn failed_aborted_missing_or_invalid_compaction_estimates_are_never_filled_in() {
    let variants = [
        json!({"type":"compaction_end","aborted":true,"result":{"estimatedTokensAfter":400}}),
        json!({"type":"compaction_end","aborted":false,"errorMessage":"summary failed","result":{"estimatedTokensAfter":400}}),
        json!({"type":"compaction_end","aborted":false,"result":{}}),
        json!({"type":"compaction_end","aborted":false,"result":{"estimatedTokensAfter":null}}),
        json!({"type":"compaction_end","aborted":false,"result":{"estimatedTokensAfter":-1}}),
        json!({"type":"compaction_end","aborted":false,"result":{"estimatedTokensAfter":"400"}}),
        json!({"type":"compaction_end","result":{"estimatedTokensAfter":400}}),
    ];
    for event in variants {
        let mut view = Workbench::default();
        view.event(&json!({"type":"compaction_end","aborted":false,"result":{"estimatedTokensAfter":400}}));
        view.event(&json!({"type":"compaction_start"}));
        view.event(&event);
        assert!(view.read(snapshot(), None).context_estimate.is_none());
    }
    for estimate in [0.0, 42.5] {
        let mut view = Workbench::default();
        view.event(&json!({"type":"compaction_end","aborted":false,"result":{"estimatedTokensAfter":estimate}}));
        assert_eq!(view.read(snapshot(), None).context_estimate, Some(estimate));
    }
}

#[test]
fn sixth_model_facts_are_kept_but_a_seventh_model_is_rejected() {
    let mut view = Workbench::default();
    let mut s = snapshot(); s.model = Some(json!({"provider":"kota","id":"auto"}));
    view.route_fact(&json!({"schema":1,"event":"decision","selectedModel":"model-6","routeModel":"model-6","provider":"fable","model":"sixth","thinkingLevel":"high"}));
    view.route_fact(&json!({"schema":1,"event":"model_error","routeModel":"model-6","provider":"fable","model":"sixth","message":"failed"}));
    view.route_fact(&json!({"schema":1,"event":"decision","selectedModel":"model-7","routeModel":"model-7"}));
    let route = view.read(s, None).auto_route.unwrap();
    assert_eq!(route.current_model.as_deref(), Some("model-6"));
    assert_eq!(route.turn.unwrap().route_model, "model-6");
    assert!(matches!(&route.events[0].data, RouteEventData::ModelError { model, .. } if model == "sixth"));
}
#[test]
fn prompt_fallback_is_a_bounded_workbench_notice_not_a_routing_error_and_recovers() {
    let mut view = Workbench::default();
    let mut auto = snapshot(); auto.model = Some(json!({"provider":"kota","id":"auto"}));
    let path = "/fixture/Kota/heroes/system-puppeteer/puppeteer-prompts.json";
    view.route_fact(&json!({"schema":1,"event":"prompt_status","path":path,"warning":crate::puppeteer_prompts::INVALID}));
    view.event(&json!({"type":"agent_start"}));
    let result = view.read(auto.clone(), None);
    let blocks = result.blocks.unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].text, format!("{}\n{path}", crate::puppeteer_prompts::INVALID));
    assert!(!blocks[0].error);
    assert!(result.auto_route.unwrap().error.is_none());
    assert!(view.read(snapshot(), None).blocks.unwrap().is_empty());
    view.route_fact(&json!({"schema":1,"event":"prompt_status","path":path,"warning":"unapproved arbitrary text"}));
    view.route_fact(&json!({"schema":1,"event":"prompt_status","path":path}));
    assert_eq!(view.read(auto.clone(), None).blocks.unwrap().len(), 1);
    view.route_fact(&json!({"schema":1,"event":"prompt_status","path":path,"warning":null}));
    assert!(view.read(auto, None).blocks.unwrap().is_empty());
}
fn selected_fact(outcome: &str) -> Value {
    json!({"event":"selection","outcome":outcome,"selected":{"provider":"fable","model":"reason","thinkingLevel":"high"},
        "candidate":{"routeModel":"model-6","provider":"fable","model":"candidate","probability":0.72},"threshold":0.85})
}
fn model_error(message: &str) -> Value { json!({"event":"model_error","provider":"fable","model":"reason","message":message}) }

#[test]
fn auto_string_events_survive_turns_but_errors_reference_only_this_run() {
    let mut view = Workbench::default();
    let auto = || { let mut s = snapshot(); s.model = Some(json!({"provider":"kota","id":"auto"})); s };
    view.event(&json!({"type":"agent_start"}));
    view.route_fact(&selected_fact("retained"));
    view.route_fact(&json!({"event":"decision","selectedModel":"model-1","routeModel":"model-1","fallback":false,"provider":"fable","model":"reason","thinkingLevel":"high"}));
    view.route_fact(&model_error("Connection error."));
    view.route_fact(&json!({"event":"error","code":"route_exhausted"}));
    let route = view.read(auto(), None).auto_route.unwrap();
    assert_eq!(route.events.len(), 2, "terminal projection is not another error row");
    assert_eq!(route.error, Some(RouteError { code:"route_exhausted".into(),event_id:Some(2) }));
    let occurred_at = route.events[0].occurred_at.clone();
    let wire = serde_json::to_value(&route).unwrap();
    assert_eq!(wire["events"][0]["occurredAt"], occurred_at);
    assert_eq!(wire["events"][1]["selected"]["thinkingLevel"], "high");
    assert_eq!(wire["events"][1]["candidate"]["probability"], 0.72);
    assert_eq!(wire["error"]["eventId"], 2);
    assert!(wire.get("attempts").is_none());
    assert!(view.read(snapshot(), None).auto_route.is_none());
    // Response facts can update the physical thinking without rewriting the event snapshot.
    view.event(&json!({"type":"message_end","message":{"role":"assistant","stopReason":"stop","provider":"fable","model":"reason","thinkingLevel":"low","content":[]}}));
    assert_eq!(view.route.turn.as_ref().unwrap().thinking_level, "low");
    assert_eq!(view.route.events, route.events);
    view.event(&json!({"type":"agent_settled"}));
    view.event(&json!({"type":"agent_start"}));
    assert!(view.route.turn.is_none()); assert!(view.route.error.is_none());
    assert_eq!(view.route.events, route.events);
    view.route_fact(&json!({"event":"error","code":"route_exhausted"}));
    assert_eq!(view.route.error.as_ref().unwrap().event_id, None, "never attach a previous run's error");
    view.route_fact(&model_error("Connection error."));
    view.route_fact(&json!({"event":"error","code":"route_exhausted"}));
    assert_eq!(view.route.error.as_ref().unwrap().event_id, Some(3));
    assert!(Workbench::default().route.events.is_empty());
}

#[test]
fn route_events_stamp_once_order_by_intake_and_evict_the_oldest_of_fifteen() {
    let mut view = Workbench::default();
    let before = chrono::Utc::now();
    for index in 0..18 {
        let mut fact = model_error(&format!("failure {index}"));
        fact["occurredAt"] = json!("1900-01-01T00:00:00Z"); view.route_fact(&fact);
    }
    let after = chrono::Utc::now();
    let events = view.route.events.clone();
    assert_eq!(events.len(), 15);
    assert_eq!((events[0].id, events[14].id), (18, 4));
    for event in &events {
        let time = chrono::DateTime::parse_from_rfc3339(&event.occurred_at).unwrap();
        assert!(time >= before && time <= after);
    }
    let mut s = snapshot(); s.model = Some(json!({"provider":"kota","id":"auto"}));
    let initial = view.read(s.clone(), None);
    assert_eq!(initial.auto_route.as_ref().unwrap().events, events);
    let cursor = Cursor { scope:s.scope.clone(),revision:initial.revision,queue_revision:Some(initial.queue_revision) };
    let wire = serde_json::to_value(view.read(s.clone(), Some(&cursor))).unwrap();
    assert!(wire.get("autoRoute").is_none()); assert!(wire.get("blocks").is_none());
    view.route_fact(&selected_fact("default"));
    assert_eq!(view.read(s, Some(&cursor)).auto_route.unwrap().events[0].id, 19);
    assert_eq!(view.route.events[1].occurred_at, events[0].occurred_at);
}

#[test]
fn route_errors_log_and_link_once_without_merging_repeated_failures() {
    let mut view = Workbench::default();
    for (kind, code) in [("jev_error","jev_auth"),("jev_error","jev_timeout"),("jev_error","jev_unreachable"),("string_error","route_empty"),("string_error","route_exhausted")] {
        for _ in 0..2 {
            view.route_fact(&json!({"event":"error","kind":kind,"code":code,"message":"same failure"}));
            assert_eq!(view.route.error.as_ref().unwrap().event_id, Some(view.route.events[0].id));
        }
    }
    assert_eq!(view.route.events.len(), 10);
    view.route_fact(&json!({"event":"error","kind":"string_error","code":"route_exhausted","provider":"fable","model":"missing","message":"No credentials."}));
    assert!(matches!(&view.route.events[0].data, RouteEventData::StringError { provider:Some(provider), .. } if provider=="fable"));
    view.route_fact(&model_error(&"x".repeat(BLOCK_LIMIT + 1)));
    assert!(matches!(&view.route.events[0].data, RouteEventData::ModelError { message, .. } if message.len()==BLOCK_LIMIT));
}

#[test]
fn malformed_route_events_are_rejected_whole_without_advancing_ids_or_revision() {
    let mut view = Workbench::default();
    let good = selected_fact("retained");
    for (path, value) in [
        ("/selected/provider",json!("x".repeat(129))), ("/selected/model",json!("x".repeat(1025))),
        ("/selected/thinkingLevel",json!("x".repeat(17))), ("/candidate/routeModel",json!("model-7")),
        ("/candidate/probability",json!(-0.1)), ("/candidate/probability",json!(1.01)),
        ("/candidate/probability",Value::Null), ("/candidate/model",json!(false)),
        ("/candidate/provider",json!("")), ("/outcome",json!("fallback")), ("/threshold",json!("0.85")),
    ] { let mut bad=good.clone(); *bad.pointer_mut(path).unwrap()=value; view.route_fact(&bad); }
    for bad in [
        json!({"event":"model_error","provider":"p","message":"bad"}),
        json!({"event":"error","kind":"jev_error","code":"route_exhausted","message":"bad"}),
        json!({"event":"error","kind":"string_error","code":"route_empty","message":null}),
        json!({"event":"error","kind":"string_error","code":"route_empty","message":"bad","provider":"p"}),
        json!({"event":"error","kind":"jev_error","code":"jev_auth","message":"bad","provider":42}),
        json!({"event":"error","code":"unknown_code"}),
    ] { view.route_fact(&bad); }
    assert_eq!((view.revision,view.route_sequence), (0,0)); assert!(view.route.events.is_empty());
    for outcome in ["selected","retained","default"] { view.route_fact(&selected_fact(outcome)); }
    assert_eq!(view.route.events.len(), 3);
}
