use super::*;

fn requests(root: &std::path::Path) -> Vec<Value> {
    fs::read_to_string(root.join("history-requests")).unwrap_or_default().lines()
        .map(|line| serde_json::from_str(line).unwrap()).collect()
}

#[test]
fn full_history_ack_orders_reconcile_empty_tool_rows_then_use_native_deltas_and_full_fallback() {
    for ack_first in [false, true] {
        let root = root(); let sink = capture();
        if ack_first { fs::write(root.join("ack-first"), "").unwrap(); }
        let input = fixture("history-receipt");
        {
            let mut ledger = Ledger::open(&root.join("ledger.jsonl")).unwrap();
            ledger.hold("session-fixture", input.clone()).unwrap();
            ledger.record("session-fixture", &input.kota_id, DeliveryState::Accepted).unwrap();
        }
        let mut details = serde_json::to_value(&input.details).unwrap(); details["kotaId"] = json!(input.kota_id);
        let entries = json!([
            {"id":"empty-tool","type":"message","parentId":null,"message":{"role":"toolResult","toolName":"read","content":[]}},
            {"id":"receipt","parentId":"empty-tool","type":"custom_message","customType":input.custom_type,"content":input.content,"details":details}
        ]);
        fs::write(root.join("entries.json"), entries.to_string()).unwrap();
        let runtime = LocalRuntime::start(config(&root, 1, true), sink.clone()).unwrap();
        assert!(seen(&sink, &input.kota_id, DeliveryState::Durable));
        assert!(!seen(&sink, &input.kota_id, DeliveryState::Abandoned));
        assert!(sink.notices.lock().unwrap().iter().any(|n| matches!(n,
            LocalNotice::Entries { entries: rows, leaf_id, since:None, recovery:true, .. }
            if rows == entries.as_array().unwrap() && leaf_id.as_deref() == Some("receipt"))));
        runtime.rpc(json!({"type":"abort"})).unwrap();
        until(|| requests(&root).iter().any(|r| r["type"] == "get_entries"));
        assert!(requests(&root).iter().filter(|r| r["type"] == "get_entries").all(|r| r["since"] == "receipt"));
        fs::write(root.join("invalid-cursor"), "").unwrap();
        runtime.rpc(json!({"type":"abort"})).unwrap();
        until(|| sink.notices.lock().unwrap().iter().any(|n| matches!(n, LocalNotice::Entries { since:None, recovery:false, .. })));
        runtime.close().unwrap();
        let requests = requests(&root);
        assert_eq!(requests.iter().filter(|r| r["type"] == "prompt").count(), 2);
        assert!(requests.iter().filter(|r| r["type"] == "prompt").all(|r| r["message"].as_str().unwrap().starts_with("/kota-history ")));
        assert!(!sink.notices.lock().unwrap().iter().any(|n| matches!(n, LocalNotice::Event { event, .. }
            if event["message"].as_str().is_some_and(|s| s.starts_with(crate::local_rpc::history::CODEC)))));
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn oversized_history_fails_before_reconciliation_without_restart_or_native_full_retry() {
    for ack_first in [false, true] {
        let root = root(); let sink = capture();
        if ack_first { fs::write(root.join("ack-first"), "").unwrap(); }
        fs::write(root.join("history-large"), "").unwrap();
        let original = b"[]"; fs::write(root.join("entries.json"), original).unwrap();
        let error = LocalRuntime::start(config(&root, 1, true), sink.clone()).err().unwrap().to_string();
        assert_eq!(error, crate::local_rpc::history::TOO_LARGE);
        assert!(!sink.notices.lock().unwrap().iter().any(|n| matches!(n, LocalNotice::Entries {..})));
        assert!(sink.notices.lock().unwrap().iter().any(|n| matches!(n, LocalNotice::Status { state:OnlineState::Error, .. })));
        assert_eq!(requests(&root).len(), 1);
        assert_eq!(fs::read_to_string(root.join("launches")).unwrap(), "started\n");
        assert_eq!(fs::read(root.join("entries.json")).unwrap(), original);
        assert_eq!(received(&root), 0);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn malformed_or_failed_history_command_is_not_successful_empty_history() {
    for marker in ["history-error", "history-bad"] {
        let root = root(); let sink = capture(); fs::write(root.join(marker), "").unwrap();
        let error = LocalRuntime::start(config(&root, 1, true), sink.clone()).err().unwrap().to_string();
        assert!(!error.contains("sensitive"));
        assert!(!sink.notices.lock().unwrap().iter().any(|n| matches!(n, LocalNotice::Entries {..})));
        assert_eq!(requests(&root).len(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn oversized_cursor_fallback_stays_in_error_when_the_workbench_is_polled() {
    let root = root(); let sink = capture();
    fs::write(root.join("entries.json"), json!([{"id":"existing","type":"custom","data":{}}]).to_string()).unwrap();
    let runtime = LocalRuntime::start(config(&root, 1, true), sink.clone()).unwrap();
    fs::write(root.join("history-large"), "").unwrap();
    fs::write(root.join("invalid-cursor"), "").unwrap();
    runtime.rpc(json!({"type":"abort"})).unwrap();
    until(|| runtime.snapshot().state == OnlineState::Error && runtime.inner_join_finished_for_test());
    for _ in 0..100 {
        assert_eq!(runtime.snapshot().error.as_deref(), Some(crate::local_rpc::history::TOO_LARGE));
        assert!(runtime.snapshot().child_pid.is_none());
        let _ = runtime.workbench(None);
    }
    assert_eq!(fs::read_to_string(root.join("launches")).unwrap(), "started\n");
    assert_eq!(requests(&root).len(), 3); // initial full, invalid delta, one full fallback
    runtime.close().unwrap(); fs::remove_dir_all(root).unwrap();
}
