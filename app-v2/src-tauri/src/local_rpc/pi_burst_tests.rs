use super::*;

#[test]
#[ignore = "requires real Pi and KOTA_LOCAL_TEST_EXTENSION; loopback fixture only"]
fn real_pi_network_and_bash_bursts_keep_the_owner_alive_and_records_complete() {
    for (name, steps, expected) in [
        ("network", json!([{"type":"long","text":"Synthetic streaming reply. ","repeat":500,"chunkSize":32}]),
            "Synthetic streaming reply. ".repeat(500)),
        ("bash", json!([
            {"type":"tool","name":"bash","arguments":{"command":"seq 1 20000"}},
            {"type":"text","text":"Tool burst complete."}
        ]), "Tool burst complete.".to_string()),
    ] {
        let f = Fixture::new(steps, false);
        let sink = Arc::new(Capture::default());
        let runtime = LocalRuntime::start(f.config(1, SESSION), sink.clone()).unwrap();
        runtime.activate(false).unwrap();
        let input = fixture(name);
        runtime.submit(input.clone()).unwrap();
        until(|| {
            let snapshot = runtime.snapshot();
            assert_ne!(snapshot.state, OnlineState::Error, "{name}: {:?}", snapshot.error);
            snapshot.state == OnlineState::Ready && sink.event("agent_settled")
                && sink.entries_contain(&input)
        });
        assert!(sink.has(&input, DeliveryState::Consumed));
        let notices = sink.0.lock().unwrap();
        let events: Vec<_> = notices.iter().filter_map(|n| match n {
            LocalNotice::Event { event, .. } => Some(event), _ => None,
        }).collect();
        let updates = events.iter().filter(|e| e["type"] == "message_update").count();
        let tool_updates = events.iter().filter(|e| e["type"] == "tool_execution_update").count();
        assert!(events.iter().any(|e| e["type"] == "message_end"
            && e["message"]["content"].as_array().is_some_and(|parts|
                parts.iter().any(|p| p["type"] == "text" && p["text"].as_str() == Some(&expected)))));
        if name == "network" { assert!(updates > 400, "fixture must exceed the former event-count bound"); }
        else {
            assert!(tool_updates > 0);
            assert!(events.iter().any(|e| e["type"] == "tool_execution_end"
                && e["isError"] == false && e["result"]["content"].as_array().is_some_and(|parts|
                    parts.iter().any(|p| p["text"].as_str().is_some_and(|text| text.contains("20000"))))));
        }
        println!("burst={name}, message_update={updates}, tool_execution_update={tool_updates}, final bytes={}", expected.len());
        drop(notices);
        runtime.close().unwrap();
    }
}
