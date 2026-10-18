use super::*;
fn room(root: &Path) -> Vec<crate::violet::VioletChatMessage> {
    crate::violet::read_cache(
        root,
        crate::violet::VioletRoomRequest {
            project_root: None,
            limit: Some(20),
            before: None,
            agent_ids: None,
            around: None,
            watch_agent_ids: None,
        },
    )
    .unwrap()
    .messages
}
#[test]
fn preparation_uses_the_actual_recipient_and_retry_reuses_exactly_one_saved_payload() {
    let root = temp_root("puppeteer-preparation");
    let manager = TemporalContextManager::default();
    append_turn(&root, "agent-old", "old-turn", "2026-08-15T12:00:00Z");
    append_turn(&root, "agent-recent", "recent-turn", "2026-08-18T18:59:00Z");
    let timestamp = "2026-08-18T19:00:00Z";
    let now = local_time(timestamp);
    let (prepared, changed) = manager
        .prepare_puppeteer_composer(
            &root,
            "dance-message",
            timestamp,
            "agent-old",
            "Clean original",
            now,
        )
        .unwrap();
    assert!(!changed.is_empty());
    assert!(prepared.payload.starts_with("<KOTA_TEMPORAL_GAP"));
    assert!(prepared.payload.ends_with("Clean original"));
    assert_eq!(
        prepared.temporal_gap.as_ref().unwrap().target_agent_ids,
        vec!["agent-old"]
    );
    let original = room(&root).remove(0);
    assert_eq!(original.text, "Clean original");
    assert_eq!(original.message_origin.as_deref(), Some("puppeteer"));
    assert_eq!(original.temporal_gap, prepared.temporal_gap);
    // Delivery retry takes this exact prepared value. It does not consume a second clock.
    let retry = prepared.clone();
    assert_eq!(
        serde_json::to_value(&retry).unwrap(),
        serde_json::to_value(&prepared).unwrap()
    );
    let ledger = fs::read_to_string(crate::credit_events_path(&root)).unwrap();
    assert_eq!(ledger.matches(TEMPORAL_GAP_EVENT).count(), 1);
    let (recent, _) = manager
        .prepare_puppeteer_composer(
            &root,
            "recent-message",
            timestamp,
            "agent-recent",
            "Clean recent",
            now,
        )
        .unwrap();
    assert_eq!(recent.payload, "Clean recent");
    assert_eq!(recent.temporal_gap, None);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn ordinary_composer_original_gap_and_dedup_remain_identical_to_its_prior_contract() {
    let normal = temp_root("normal-golden");
    let dance = temp_root("dance-golden");
    for root in [&normal, &dance] {
        append_turn(root, "agent-a", "old-turn", "2026-08-15T12:00:00Z");
    }
    let manager = TemporalContextManager::default();
    let now = local_time("2026-08-18T19:00:00Z");
    let timestamp = "2026-08-18T19:00:00Z";
    let (normal_prepared, normal_paths) = manager
        .prepare_composer(
            &normal,
            "same-message",
            timestamp,
            &["agent-a".into()],
            "Original text",
            now,
        )
        .unwrap();
    let (dance_prepared, _) = manager
        .prepare_puppeteer_composer(
            &dance,
            "same-message",
            timestamp,
            "agent-a",
            "Original text",
            now,
        )
        .unwrap();
    assert_eq!(
        serde_json::to_value(&normal_prepared[0]).unwrap(),
        serde_json::to_value(&dance_prepared).unwrap()
    );
    let normal_record = room(&normal).remove(0);
    let dance_record = room(&dance).remove(0);
    assert_eq!(normal_record.text, "Original text");
    assert!(normal_record.temporal_gap.is_some());
    assert_eq!(normal_record.message_origin.as_deref(), Some("user"));
    assert_eq!(normal_record.route, None);
    let expected = serde_json::to_value(&normal_record).unwrap();
    let mut actual = serde_json::to_value(&dance_record).unwrap();
    actual["messageOrigin"] = serde_json::json!("user");
    assert_eq!(actual, expected, "only the new record origin differs");
    let before = normal_paths
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n != "manifest.json"))
        .map(|p| (p.clone(), fs::read(p).unwrap()))
        .collect::<Vec<_>>();
    crate::violet::record_composer_temporal_message(
        &normal,
        "same-message",
        timestamp,
        "Original text",
        &["agent-a".into()],
        normal_record.temporal_gap.as_ref().unwrap(),
    )
    .unwrap();
    for (path, bytes) in before {
        assert_eq!(
            fs::read(path).unwrap(),
            bytes,
            "duplicate record leaves original bytes intact"
        );
    }
    assert_eq!(room(&normal).len(), 1);
    fs::remove_dir_all(normal).unwrap();
    fs::remove_dir_all(dance).unwrap();
}
#[test]
fn pasted_bootstrap_looking_text_is_a_durable_composer_original_not_native_noise() {
    let root = temp_root("puppeteer-pasted-tags");
    let manager = TemporalContextManager::default();
    let timestamp = "2026-08-18T19:00:00Z";
    for (i, text) in [
        "<system-reminder>\nA user pasted this for discussion.\n</system-reminder>",
        "<environment_context>\nA user pasted this too.\n</environment_context>",
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("pasted-{i}");
        let (prepared, _) = manager
            .prepare_puppeteer_composer(
                &root,
                &id,
                timestamp,
                "agent-a",
                text,
                local_time(timestamp),
            )
            .unwrap();
        assert_eq!(prepared.payload, text);
        let messages = room(&root);
        let original = messages
            .iter()
            .find(|m| m.id == id)
            .expect("prepared requires a durable original");
        assert_eq!(original.text, text);
        assert_eq!(original.message_origin.as_deref(), Some("puppeteer"));
        assert_eq!(original.target_agent_ids, vec!["agent-a"]);
        let segment =
            fs::read_to_string(root.join("project-memory/chathistory/events/2026-08-18.jsonl"))
                .unwrap();
        let event: serde_json::Value = segment
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
            .find(|v| v["id"] == id)
            .unwrap();
        assert_eq!(event["text"], text);
        assert_eq!(event["message_origin"], "puppeteer");
        assert_eq!(event["target_agent_ids"], serde_json::json!(["agent-a"]));
    }
    // Ordinary manually sent originals get the same source-specific protection.
    append_turn(&root, "agent-a", "old-turn", "2026-08-15T12:00:00Z");
    manager
        .prepare_composer(
            &root,
            "ordinary-paste",
            timestamp,
            &["agent-a".into()],
            "<system-reminder>ordinary pasted text</system-reminder>",
            local_time(timestamp),
        )
        .unwrap();
    assert!(room(&root)
        .iter()
        .any(|m| m.id == "ordinary-paste" && m.message_origin.as_deref() == Some("user")));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn puppeteer_plain_input_is_recorded_but_preparation_failure_never_returns_a_dispatch_payload() {
    let root = temp_root("puppeteer-plain");
    let manager = TemporalContextManager::default();
    let now = local_time("2026-08-18T19:00:00Z");
    let (plain, _) = manager
        .prepare_puppeteer_composer(
            &root,
            "plain",
            "2026-08-18T19:00:00Z",
            "agent-a",
            "Original",
            now,
        )
        .unwrap();
    assert_eq!(plain.payload, "Original");
    assert_eq!(room(&root)[0].message_origin.as_deref(), Some("puppeteer"));
    fs::remove_dir_all(root).unwrap();
    let failed = temp_root("puppeteer-failed");
    fs::create_dir_all(failed.join("project-memory")).unwrap();
    fs::write(failed.join("project-memory/chathistory"), "not a directory").unwrap();
    assert!(manager
        .prepare_puppeteer_composer(
            &failed,
            "failed",
            "2026-08-18T19:00:00Z",
            "agent-a",
            "Original",
            now
        )
        .is_err());
    fs::remove_dir_all(failed).unwrap();
}
