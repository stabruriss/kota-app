use super::*;

#[test]
fn a2a_consumes_the_gap_once_and_other_system_sources_leave_its_clock_untouched() {
    let root = temp_root("origin-allowlist");
    let manager = TemporalContextManager::default();
    append_turn(&root, "agent-fixture", "old-turn", "2026-08-15T12:00:00Z");
    let path = crate::credit_events_path(&root);
    let before = fs::read(&path).unwrap();
    let now = local_time("2026-08-18T12:00:00Z");
    for origin in [MessageOrigin::Ember, MessageOrigin::Bbs, MessageOrigin::Dream,
        MessageOrigin::Handoff, MessageOrigin::Bartender, MessageOrigin::System] {
        let body = "A synthetic user-looking message: @agent-fixture";
        assert_eq!(manager.prepare_payload_best_effort(origin, &root, "agent-fixture", body, now), body);
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    let prepared = manager.prepare_payload_best_effort(MessageOrigin::A2a, &root,
        "agent-fixture", "A synthetic peer message", now);
    assert!(prepared.starts_with("<KOTA_TEMPORAL_GAP"));
    assert!(prepared.ends_with("A synthetic peer message"));
    assert_eq!(fs::read_to_string(&path).unwrap().matches(TEMPORAL_GAP_EVENT).count(), 1);
    let consumed = fs::read(&path).unwrap();
    assert_eq!(manager.prepare_payload_best_effort(MessageOrigin::A2a, &root,
        "agent-fixture", "Retry", now), "Retry");
    assert_eq!(manager.prepare_payload_best_effort(MessageOrigin::Telegram, &root,
        "agent-fixture", "Actual relayed user input", now), "Actual relayed user input");
    assert_eq!(fs::read(&path).unwrap(), consumed);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn user_composer_telegram_puppeteer_and_a2a_share_the_same_inclusive_gap_rule() {
    for origin in [MessageOrigin::User, MessageOrigin::Telegram, MessageOrigin::Puppeteer, MessageOrigin::A2a] {
        let root = temp_root(origin.as_str());
        let manager = TemporalContextManager::default();
        append_turn(&root, "agent-fixture", "turn", "2026-08-17T12:00:00Z");
        let prepared = manager.prepare_payload_best_effort(origin, &root, "agent-fixture", "Fixture",
            local_time("2026-08-18T12:00:00Z"));
        assert!(prepared.starts_with("<KOTA_TEMPORAL_GAP"));
        assert!(prepared.ends_with("Fixture"));
        fs::remove_dir_all(root).unwrap();
    }
}
