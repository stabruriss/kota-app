use super::*;
fn selection(id: &str, parent: Option<&str>, auto: bool) -> JsonValue {
    json!({"type":"model_change","id":id,"parentId":parent,"timestamp":"2026-10-09T00:00:00Z",
        "provider":if auto{"kota"}else{"kota-fake"},"modelId":if auto{"auto"}else{"fixture"}})
}
fn failed(id: &str, parent: &str, text: &str) -> JsonValue {
    let mut e = assistant_entry(id, parent, "2026-10-09T00:01:00Z", "");
    e["message"]["content"] = json!([]);
    e["message"]["stopReason"] = json!("error");
    e["message"]["errorMessage"] = json!(text);
    e
}
#[test]
fn auto_string_empty_configuration_keeps_the_fixed_copy_without_a_provider_prefix() {
    let project = LocalProject::new("auto-string-copy");
    let text = "No models are configured for this Auto String.";
    assert_eq!(model_error_text(&json!({"errorMessage":"No models are configured for this Auto Route."}), true), "No models are configured for this Auto Route.");
    ingest_entries(
        &project.scope(1),
        &full_read(
            &[selection("auto", None, true), failed("empty", "auto", text)],
            "empty",
        ),
    )
    .unwrap();
    let messages = project.messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].text, text);
    assert_eq!(messages[0].route.as_deref(), Some("auto"));
    assert_eq!(PARSED_EVENT_CACHE_VERSION, "8");
}
#[test]
fn selected_route_is_historical_but_caption_stays_the_actual_reply_model_and_effort() {
    let project = LocalProject::new("route-caption");
    let mut first = assistant_entry("first", "auto", "2026-10-09T00:01:00Z", "First auto reply");
    first["message"]["model"] = json!("actual-fallback");
    first["message"]["thinkingLevel"] = json!("high");
    let second = assistant_entry(
        "second",
        "concrete",
        "2026-10-09T00:02:00Z",
        "Concrete reply",
    );
    let entries = vec![
        selection("auto", None, true),
        first,
        selection("concrete", Some("first"), false),
        second,
    ];
    ingest_entries(&project.scope(1), &full_read(&entries, "second")).unwrap();
    let messages = project.messages();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].route.as_deref(), Some("auto"));
    assert_eq!(messages[0].model.as_deref(), Some("actual-fallback"));
    assert_eq!(messages[0].effort.as_deref(), Some("high"));
    assert_eq!(messages[1].route, None);
    let search = crate::violet::room_search::search_room(
        &project.root,
        crate::violet::room_search::VioletRoomSearchRequest {
            project_root: project.root.to_string_lossy().into(),
            query: "First auto reply".into(),
            human_only: false,
            limit: None,
            cursor: None,
        },
    )
    .unwrap();
    assert_eq!(search.hits[0].route.as_deref(), Some("auto"));
    assert_eq!(search.hits[0].model.as_deref(), Some("actual-fallback"));
    assert_eq!(PARSED_EVENT_CACHE_VERSION, "8");
}
#[test]
fn metadata_before_cursor_hydrates_new_replies_without_backfilling_an_old_record() {
    let project = LocalProject::new("route-forward-only");
    let scope = project.scope(1);
    let old = assistant_entry("old", "auto", "2026-10-09T00:01:00Z", "Old cached reply");
    ingest_entries(&scope, &full_read(&[old.clone()], "old")).unwrap();
    assert_eq!(project.messages()[0].route, None);
    with_state(&project.root, AGENT, |s| {
        s.sessions.get_mut(SESSION).unwrap().route_selected = false;
        Ok(())
    })
    .unwrap();
    let new = assistant_entry("new", "old", "2026-10-09T00:02:00Z", "New reply");
    let entries = vec![selection("auto", None, true), old, new];
    ingest_entries(&scope, &full_read(&entries, "new")).unwrap();
    let messages = project.messages();
    assert_eq!(messages[0].route, None);
    assert_eq!(messages[1].route.as_deref(), Some("auto"));
    assert!(serde_json::to_value(&messages[0])
        .unwrap()
        .get("route")
        .is_none());
    assert_eq!(PARSED_EVENT_CACHE_VERSION, "8");
}
#[test]
fn a_live_held_failure_remembers_its_original_route_after_switching_to_concrete() {
    let project = LocalProject::new("route-held");
    let scope = project.scope(1);
    let entries = vec![
        selection("auto", None, true),
        failed(
            "failure",
            "auto",
            "All configured models failed for this turn.",
        ),
    ];
    ingest_entries(&scope, &since_read(&entries, "failure", "missing-cursor")).unwrap();
    assert!(project.messages().is_empty());
    let concrete = selection("concrete", Some("failure"), false);
    ingest_entries(&scope, &since_read(&[concrete], "concrete", "failure")).unwrap();
    record_live_event(&scope, &json!({"type":"agent_settled"})).unwrap();
    let messages = project.messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].route.as_deref(), Some("auto"));
    assert_eq!(
        messages[0].text,
        "All configured models failed for this turn."
    );
}
#[test]
fn retried_bridge_is_retracted_by_context_edit_and_only_final_error_reaches_history() {
    let project = LocalProject::new("route-bridge");
    let scope = project.scope(1);
    let bridge = "500: Model 1: provider returned error. Trying another configured model.";
    let head = vec![
        selection("auto", None, true),
        failed("bridge", "auto", bridge),
    ];
    ingest_entries(&scope, &since_read(&head, "bridge", "missing-cursor")).unwrap();
    record_live_event(&scope, &json!({"type":"auto_retry_start","attempt":1})).unwrap();
    assert!(project.messages().is_empty());
    let tail = vec![
        json!({"type":"context_edit","id":"retract","parentId":"bridge","timestamp":"2026-10-09T00:01:01Z","targetId":"bridge","replacement":null}),
        failed(
            "final",
            "retract",
            "All configured models failed for this turn.",
        ),
    ];
    ingest_entries(&scope, &since_read(&tail, "final", "bridge")).unwrap();
    record_live_event(&scope, &json!({"type":"agent_settled"})).unwrap();
    let messages = project.messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].text,
        "All configured models failed for this turn."
    );
    assert_eq!(messages[0].route.as_deref(), Some("auto"));
    ingest_entries(&scope, &full_read(&[head, tail].concat(), "final")).unwrap();
    assert_eq!(project.messages().len(), 1);
}
#[test]
fn a_non_retried_bridge_remains_an_honest_final_error_instead_of_disappearing() {
    let project = LocalProject::new("route-final-bridge");
    let scope = project.scope(1);
    let bridge = "500: Model 1: provider returned error. Trying another configured model.";
    let head = vec![
        selection("auto", None, true),
        failed("bridge", "auto", bridge),
    ];
    ingest_entries(&scope, &since_read(&head, "bridge", "missing-cursor")).unwrap();
    record_live_event(&scope, &json!({"type":"agent_settled"})).unwrap();
    assert_eq!(project.messages().len(), 1);
    assert!(project.messages()[0].text.contains(bridge));
    assert_eq!(project.messages()[0].route.as_deref(), Some("auto"));
}
#[test]
fn pasted_composer_source_is_visible_but_native_harness_records_stay_filtered() {
    let project = LocalProject::new("dance-pasted-source");
    crate::violet::record_puppeteer_composer_message(
        &project.root,
        "pasted",
        "2026-10-09T00:00:00Z",
        "<system-reminder>pasted by a human</system-reminder>",
        AGENT,
        None,
    )
    .unwrap();
    let original = project.messages().remove(0);
    let event = crate::violet::chathistory_event_from_message(&original, None);
    assert!(crate::violet::is_summary_count_event(&event));
    for (role, agent, shell, session) in [
        ("assistant", "user", "composer", "composer"),
        ("user", AGENT, "composer", "composer"),
        ("user", "user", "kota-local", "composer"),
        ("user", "user", "composer", "native-session"),
    ] {
        let mut native = original.clone();
        native.id = format!("native-{role}-{agent}-{shell}-{session}");
        native.role = role.into();
        native.agent_id = agent.into();
        native.shell = shell.into();
        native.session_id = session.into();
        native.message_origin = None;
        let event = crate::violet::chathistory_event_from_message(&native, None);
        assert!(!crate::violet::is_summary_count_event(&event));
        assert!(crate::violet::chathistory_events_to_room_messages(vec![event], None).is_empty());
        crate::violet::write_chathistory_messages(&project.root, &[native]).unwrap();
    }
    assert_eq!(project.messages().len(), 1);
    assert_eq!(
        crate::violet::read_chathistory_event_segments(&project.root)
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn puppeteer_record_is_durable_before_any_dispatch_and_duplicate_ids_do_not_duplicate_it() {
    let project = LocalProject::new("dance-record");
    let timestamp = "2026-10-09T00:00:00Z";
    let changed = crate::violet::record_puppeteer_composer_message(
        &project.root,
        "draft",
        timestamp,
        "Original",
        AGENT,
        None,
    )
    .unwrap();
    assert!(!changed.is_empty());
    let messages = project.messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].text, "Original");
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].message_origin.as_deref(), Some("puppeteer"));
    assert_eq!(messages[0].target_agent_ids, vec![AGENT]);
    crate::violet::record_puppeteer_composer_message(
        &project.root,
        "draft",
        timestamp,
        "Original",
        AGENT,
        None,
    )
    .unwrap();
    assert_eq!(project.messages().len(), 1);
    assert_eq!(PARSED_EVENT_CACHE_VERSION, "8");
}
