use super::*;
use crate::local_rpc::auth_fixture::Fixture;
// Existing cases exercise validation/roster gates with a freshly read fixture baseline.
fn save(manager: &IntegrationManager, root: &str, settings: &Settings, owned: &OwnedDirectory) -> LocalResult<()> {
    let before = read_source(Path::new(root))?;
    save_checked(manager, root, settings, owned, &crate::puppeteer_setup::revision(before.as_deref()), |_| {}).map(|_| ())
}
fn gate(settings: &Settings, seats: &[Seat], owned: &OwnedDirectory) -> LocalResult<()> { super::gate(settings, seats, owned, true) }
// Fixtures explicitly use the bundled snapshot; production supplies the once-read account snapshot.
fn request_body(request: &RouteRequest, settings: &Settings, roster: &[Seat]) -> Value {
    super::request_body(request, settings, roster, prompts())
}
fn choose_target(
    request: &RouteRequest,
    settings: &Settings,
    before: &[Seat],
    after: impl FnOnce() -> LocalResult<Vec<Seat>>,
    classify: impl FnOnce(Value) -> LocalResult<Value>,
) -> LocalResult<(String, bool)> {
    super::choose_target(request, settings, before, prompts(), after, classify)
        .map(|s| (s.target.agent_id, s.outcome == Outcome::Sentinel))
}
fn pick(reply: &Value, body: &Value, threshold: f64) -> LocalResult<Option<String>> {
    super::pick(reply, body).map(|(id, p)| (p > threshold).then_some(id))
}
mod prompt_parity {
    include!("puppeteer_prompt_parity_tests.rs");
}
fn roster() -> Vec<Seat> {
    ["agent-a", "agent-b", "agent-c"]
        .into_iter()
        .map(|id| Seat {
            id: id.into(),
            name: format!("Name {id}"),
        })
        .collect()
}
fn request() -> RouteRequest {
    RouteRequest {
        message_id: "message-fixture".into(),
        timestamp: "2026-10-09T12:00:00Z".into(),
        payload: "Original user message".into(),
        text: None,
        attachments: vec![],
        links: vec![],
        paths_mentioned: vec![],
        roster: vec![RosterItem {
            id: "agent-a".into(),
            name: "Name agent-a".into(),
            last_reply: Some(TimedText {
                at: "2026-10-09T11:00:00Z".into(),
                text: "Latest final reply".into(),
            }),
            last_reply_order: None,
            recent_user_messages: vec![TimedText {
                at: "2026-10-09T10:00:00Z".into(),
                text: "Previous direct input".into(),
            }],
        }],
    }
}
fn settings() -> Settings {
    Settings {
        enabled: true,
        sentinel: Some("agent-c".into()),
        threshold: 0.75, // Explicit existing project setting; new projects default to 0.50.
        ..Settings::default()
    }
}
fn response(a: f64, b: f64, c: f64) -> Value {
    json!({"answers":{"target":{"type":"choice","choice":"agent-a","probabilities":{"agent-a":a,"agent-b":b,"agent-c":c},"confidence":1}}})
}
#[test]
fn omitted_text_strips_attachment_paths_and_markers_without_changing_delivery_payload() {
    let mut req = request();
    req.payload =
        "[Image #1] \"/tmp/picture 1.png\" /home/fixture/project-memory/attachments/composer/a.png"
            .into();
    req.attachments = vec![Attachment {
        name: "picture 1.png".into(),
        path: Some("/tmp/picture 1.png".into()),
        mime_type: None,
        size: None,
    }];
    assert_eq!(classification_text(&req), "");
    assert_eq!(
        choose_target(
            &req,
            &settings(),
            &roster(),
            || Ok(roster()),
            |_| panic!("attachment-only must bypass Jev")
        )
        .unwrap(),
        ("agent-c".into(), true)
    );
    req.payload.push_str("\n\nClean  message");
    assert_eq!(classification_text(&req), "Clean message");
    assert_eq!(
        request_body(&req, &settings(), &roster())["state"]["new_message"]["text"],
        "Clean message"
    );
    assert!(req.payload.starts_with("[Image #1]"));
    req.text = Some("Raw  draft\ntext".into());
    assert_eq!(classification_text(&req), "Raw  draft\ntext");
}

#[test]
fn defaults_are_local_and_disabled_with_no_sentinel() {
    let s = Settings::default();
    assert_eq!(s.schema, 1);
    assert!(!s.enabled);
    assert_eq!(s.sentinel, None);
    assert_eq!(s.threshold, 0.5);
    assert_eq!(s.strategy, Strategy::Context);
    s.validate().unwrap();
    let root = std::env::temp_dir().join(format!("kota-dance-default-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    assert_eq!(read(&root).unwrap(), s);
    assert!(!settings_path(&root).exists());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn settings_require_valid_threshold_unicode_limits_and_two_nonempty_rules() {
    for step in 1..=19 {
        let s = Settings { threshold: f64::from(step) / 20.0, ..Settings::default() };
        s.validate().unwrap();
        assert_eq!(decode(Some(&serialize(&s))).unwrap(), s);
    }
    for t in [-0.05, 0.0, 0.025, 0.049, 0.051, 0.49, 0.76, 0.96, 1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(Settings {
            threshold: t,
            ..Settings::default()
        }
        .validate()
        .is_err());
    }
    let mut s = settings();
    s.strategy = Strategy::Rule;
    s.conditions.insert("agent-a".into(), "😀".repeat(200));
    s.conditions.insert("agent-b".into(), " \n".into());
    assert_eq!(s.validate().unwrap_err().code, "dance_rules_required");
    s.conditions.insert("agent-b".into(), "condition".into());
    s.validate().unwrap();
    s.conditions.insert("agent-a".into(), "😀".repeat(201));
    assert_eq!(s.validate().unwrap_err().code, "dance_settings_invalid");
    assert!(Settings {
        sentinel: Some("../escape".into()),
        ..settings()
    }
    .validate()
    .is_err());
}
#[test]
fn settings_roundtrip_and_malformed_owned_files_fail_closed() {
    let root = std::env::temp_dir().join(format!("kota-dance-settings-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(crate::project_memory_dir(&root)).unwrap();
    let s = settings();
    fs::write(settings_path(&root), serde_json::to_vec(&s).unwrap()).unwrap();
    assert_eq!(read(&root).unwrap(), s);
    fs::write(settings_path(&root), "not json").unwrap();
    assert_eq!(read(&root).unwrap_err().code, "dance_settings_invalid");
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn enabling_requires_a_seated_sentinel_and_shared_ready_key_but_disabling_needs_neither() {
    let f = Fixture::new(json!({}));
    let seats = roster();
    gate(&Settings::default(), &[], &f.backend.owned).unwrap();
    assert_eq!(
        gate(
            &Settings {
                sentinel: None,
                ..settings()
            },
            &seats,
            &f.backend.owned
        )
        .unwrap_err()
        .code,
        "dance_sentinel_required"
    );
    assert_eq!(
        gate(&settings(), &seats, &f.backend.owned)
            .unwrap_err()
            .code,
        "jev_key_required"
    );
    jev::save_key_with(
        &f.backend,
        "ready",
        "typesafe",
        "fixture-key",
        |_, _, _, _| Ok("verified"),
    )
    .unwrap();
    jev::select(&f.backend, "typesafe").unwrap();
    gate(&settings(), &seats, &f.backend.owned).unwrap();
    jev::mark_rejected(
        &f.backend.owned,
        &jev::current(&f.backend.owned).unwrap().unwrap(),
    )
    .unwrap();
    gate(
        &Settings {
            enabled: false,
            ..settings()
        },
        &[],
        &f.backend.owned,
    )
    .unwrap();
}
#[test]
fn context_request_uses_bounded_recent_inputs_final_reply_and_empty_seats_with_name() {
    let mut req = request();
    req.payload = "😀".repeat(5000);
    req.roster[0].last_reply.as_mut().unwrap().text = "😀".repeat(250);
    req.roster[0].recent_user_messages = (0..5)
        .map(|i| TimedText {
            at: i.to_string(),
            text: "😀".repeat(240),
        })
        .collect();
    let body = request_body(&req, &settings(), &roster());
    let state = &body["state"];
    assert_eq!(
        state["new_message"]["text"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        4096
    );
    assert_eq!(
        state["roster"][0]["last_reply"]["text"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        200
    );
    assert_eq!(
        state["roster"][0]["recent_user_messages"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(state["roster"][0]["recent_user_messages"][0]["at"], "3");
    assert_eq!(state["roster"][0]["recent_user_messages"][1]["at"], "4");
    assert_eq!(state["roster"][1]["last_reply"], Value::Null);
    assert_eq!(state["roster"][1]["recent_user_messages"], json!([]));
    assert_eq!(state["roster"][0]["name"], "Name agent-a");
    assert!(state["roster"][0].get("agent").is_none());
    assert_eq!(
        body["questions"]["target"]["criteria"]["agent-a"],
        "Route to agent-a in state.roster."
    );
    for forbidden in ["threshold", "sentinel", "confidence", "thread"] {
        assert!(state.get(forbidden).is_none());
    }
}
#[test]
fn rule_omits_empty_conditions_and_history_but_keeps_sentinel_as_fallback() {
    let mut s = settings();
    s.strategy = Strategy::Rule;
    s.conditions = BTreeMap::from([
        ("agent-a".into(), "Original A".into()),
        ("agent-b".into(), "Original B".into()),
        ("agent-c".into(), " \n".into()),
    ]);
    let body = request_body(&request(), &s, &roster());
    assert_eq!(body["state"]["roster"].as_array().unwrap().len(), 2);
    assert_eq!(
        body["state"]["roster"][0],
        json!({"id":"agent-a","name":"Name agent-a","condition":"Original A"})
    );
    assert_eq!(
        body["questions"]["target"]["criteria"],
        json!({"agent-a":"Original A","agent-b":"Original B"})
    );
    let (target, fallback) = choose_target(
        &request(),
        &s,
        &roster(),
        || Ok(roster()),
        |_| Ok(response(0.5, 0.5, 0.0)),
    )
    .unwrap();
    assert_eq!((target.as_str(), fallback), ("agent-c", true));
}
#[test]
fn only_strict_probability_over_threshold_routes_else_sentinel_no_confidence_override() {
    for (a, expected, fallback) in [(0.75, "agent-c", true), (0.751, "agent-a", false)] {
        let result = choose_target(
            &request(),
            &settings(),
            &roster(),
            || Ok(roster()),
            |_| Ok(response(a, 1.0 - a, 0.0)),
        )
        .unwrap();
        assert_eq!(result, (expected.into(), fallback));
    }
    let body = request_body(&request(), &settings(), &roster());
    assert!(pick(
        &json!({"answers":{"target":{"type":"choice","choice":"agent-x","probabilities":{}}}}),
        &body,
        0.75
    )
    .is_err());
    assert!(pick(&response(2.0, -1.0, 0.0), &body, 0.75).is_err());
}
#[test]
fn only_one_seat_or_attachment_only_input_does_not_call_jev() {
    let seats = vec![roster()[2].clone()];
    assert_eq!(
        choose_target(
            &request(),
            &settings(),
            &seats,
            || Ok(seats.clone()),
            |_| panic!("no HTTP")
        )
        .unwrap(),
        ("agent-c".into(), false)
    );
    let mut req = request();
    req.payload = "project-memory/attachments/composer/fixture.png".into();
    req.text = Some(String::new());
    assert_eq!(
        choose_target(
            &req,
            &settings(),
            &roster(),
            || Ok(roster()),
            |_| panic!("no HTTP")
        )
        .unwrap(),
        ("agent-c".into(), true)
    );
}
#[test]
fn a_departed_match_falls_back_but_missing_sentinel_only_blocks_uncertain_sends() {
    let after = vec![roster()[1].clone(), roster()[2].clone()];
    let result = choose_target(
        &request(),
        &settings(),
        &roster(),
        || Ok(after),
        |_| Ok(response(0.9, 0.1, 0.0)),
    )
    .unwrap();
    assert_eq!(result, ("agent-c".into(), true));
    let no_sentinel = vec![roster()[0].clone(), roster()[1].clone()];
    let result = choose_target(
        &request(),
        &settings(),
        &roster(),
        || Ok(no_sentinel.clone()),
        |_| Ok(response(0.9, 0.1, 0.0)),
    )
    .unwrap();
    assert_eq!(result, ("agent-a".into(), false));
    assert_eq!(
        choose_target(
            &request(),
            &settings(),
            &roster(),
            || Ok(no_sentinel),
            |_| Ok(response(0.7, 0.3, 0.0))
        )
        .unwrap_err()
        .code,
        "dance_sentinel_required"
    );
}
#[test]
fn jev_errors_never_fall_back_to_sentinel() {
    for code in ["jev_auth", "jev_timeout", "jev_unreachable"] {
        assert_eq!(
            choose_target(
                &request(),
                &settings(),
                &roster(),
                || panic!("never prepare delivery"),
                |_| Err(LocalError::new(code))
            )
            .unwrap_err()
            .code,
            code
        );
    }
}
#[test]
fn context_metadata_lists_are_capped_and_do_not_include_attachment_contents() {
    let mut req = request();
    req.links = vec!["x".repeat(2000); 10];
    req.paths_mentioned = vec!["path".into(); 10];
    req.attachments = (0..10)
        .map(|i| Attachment {
            name: i.to_string(),
            path: Some("path".into()),
            mime_type: Some("image/png".into()),
            size: Some(123),
        })
        .collect();
    let body = request_body(&req, &settings(), &roster());
    let msg = &body["state"]["new_message"];
    for key in ["attachments", "links", "paths_mentioned"] {
        assert_eq!(msg[key].as_array().unwrap().len(), 8);
    }
    assert_eq!(msg["links"][0].as_str().unwrap().len(), 1024);
    assert!(msg["attachments"][0].get("content").is_none());
}
fn project(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("kota-dance-{label}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    root
}
fn seat_project(manager: &IntegrationManager, root: &str) {
    for seat in roster() {
        let path = Path::new(root).join(".agent-workspaces").join(&seat.id);
        fs::create_dir_all(&path).unwrap();
        fs::write(
            path.join("agent.yaml"),
            format!("status: active\ndisplay-name: {}\n", seat.name),
        )
        .unwrap();
    }
    crate::save_project_agent_layout(
        manager,
        Some(root),
        &roster().into_iter().map(|s| Some(s.id)).collect::<Vec<_>>(),
    )
    .unwrap();
}
fn verified_key(f: &Fixture) {
    jev::save_key_with(
        &f.backend,
        "ready",
        "typesafe",
        "fixture-key",
        |_, _, _, _| Ok("verified"),
    )
    .unwrap();
    jev::select(&f.backend, "typesafe").unwrap();
}
#[cfg(unix)]
#[test]
fn save_and_route_read_the_same_ui_spelled_symlink_layout() {
    let root = project("symlink-layout");
    let physical = root.join("physical");
    let alias = root.join("ui-alias");
    fs::create_dir(&physical).unwrap();
    std::os::unix::fs::symlink(&physical, &alias).unwrap();
    let ui_root = alias.to_str().unwrap();
    let manager = IntegrationManager::default();
    let f = Fixture::new(json!({}));
    verified_key(&f);
    seat_project(&manager, ui_root);
    let canonical = project_root(ui_root).unwrap();
    assert_ne!(canonical, alias);
    assert!(
        crate::load_project_agent_layout(&manager, canonical.to_str())
            .unwrap()
            .is_none(),
        "the old canonical-root call ignored this valid UI layout"
    );
    save(&manager, ui_root, &settings(), &f.backend.owned).unwrap();
    assert_eq!(read(&canonical).unwrap(), settings());
    let before = seats(&manager, ui_root).unwrap();
    assert_eq!(
        before.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        ["agent-a", "agent-b", "agent-c"]
    );
    assert_eq!(
        choose_target(
            &request(),
            &settings(),
            &before,
            || seats(&manager, ui_root),
            |_| Ok(response(0.9, 0.1, 0.0))
        )
        .unwrap(),
        ("agent-a".into(), false)
    );
    assert!(Arc::ptr_eq(
        &routing_lock(&canonical),
        &routing_lock(&project_root(physical.to_str().unwrap()).unwrap())
    ));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn missing_or_copied_layout_is_roster_unavailable_for_save_and_route_not_client_ids() {
    let root = project("missing-layout");
    let ui_root = root.to_str().unwrap();
    let manager = IntegrationManager::default();
    let f = Fixture::new(json!({}));
    for copied in [false, true] {
        if copied {
            seat_project(&manager, ui_root);
            let path = crate::project_agent_layout_path(&root);
            let mut layout: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            layout["projectRoot"] = json!("/another/project");
            fs::write(path, serde_json::to_vec(&layout).unwrap()).unwrap();
        }
        assert_eq!(
            seats(&manager, ui_root).unwrap_err().code,
            "dance_roster_unavailable"
        );
        assert_eq!(
            save(&manager, ui_root, &settings(), &f.backend.owned)
                .unwrap_err()
                .code,
            "dance_roster_unavailable"
        );
    }
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn malformed_settings_can_be_disabled_or_replaced_but_enable_still_checks_roster_and_key() {
    let root = project("repair-settings");
    let ui_root = root.to_str().unwrap();
    let manager = IntegrationManager::default();
    let f = Fixture::new(json!({}));
    fs::create_dir_all(crate::project_memory_dir(&root)).unwrap();
    let mut unknown = serde_json::to_value(Settings::default()).unwrap();
    unknown["futureKey"] = json!(true);
    for bad in ["not json".to_owned(), unknown.to_string()] {
        fs::write(settings_path(&root), &bad).unwrap();
        assert_eq!(read(&root).unwrap_err().code, "dance_settings_invalid");
        save(&manager, ui_root, &Settings::default(), &f.backend.owned).unwrap();
        assert_eq!(read(&root).unwrap(), Settings::default());
        fs::write(settings_path(&root), &bad).unwrap();
        let disabled = Settings {
            enabled: false,
            ..settings()
        };
        save(&manager, ui_root, &disabled, &f.backend.owned).unwrap();
        assert_eq!(read(&root).unwrap(), disabled);
    }
    seat_project(&manager, ui_root);
    fs::write(settings_path(&root), "not json").unwrap();
    assert_eq!(
        save(&manager, ui_root, &settings(), &f.backend.owned)
            .unwrap_err()
            .code,
        "jev_key_required"
    );
    verified_key(&f);
    save(&manager, ui_root, &settings(), &f.backend.owned).unwrap();
    assert_eq!(read(&root).unwrap(), settings());
    // A rejected key and a lost seating file cannot prevent a disable-only edit.
    jev::mark_rejected(
        &f.backend.owned,
        &jev::current(&f.backend.owned).unwrap().unwrap(),
    )
    .unwrap();
    fs::remove_file(crate::project_agent_layout_path(&root)).unwrap();
    let disabled = Settings {
        enabled: false,
        ..settings()
    };
    save(&manager, ui_root, &disabled, &f.backend.owned).unwrap();
    assert_eq!(read(&root).unwrap(), disabled);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn the_only_remaining_rule_candidate_bypasses_jev_but_is_rechecked_before_delivery() {
    let s = Settings {
        strategy: Strategy::Rule,
        conditions: BTreeMap::from([
            ("agent-a".into(), "A condition".into()),
            ("agent-b".into(), "B condition".into()),
        ]),
        ..settings()
    };
    let before = vec![roster()[0].clone(), roster()[2].clone()];
    assert_eq!(
        choose_target(
            &request(),
            &s,
            &before,
            || Ok(before.clone()),
            |_| panic!("one candidate must not call Jev")
        )
        .unwrap(),
        ("agent-a".into(), false)
    );
    assert_eq!(
        choose_target(
            &request(),
            &s,
            &before,
            || Ok(vec![roster()[2].clone()]),
            |_| panic!("one candidate must not call Jev")
        )
        .unwrap(),
        ("agent-c".into(), true)
    );
}
#[test]
fn settings_changed_after_classification_aborts_without_a_target() {
    let root = project("changed-settings");
    fs::create_dir_all(crate::project_memory_dir(&root)).unwrap();
    fs::write(
        settings_path(&root),
        serde_json::to_vec(&settings()).unwrap(),
    )
    .unwrap();
    let result = choose_target(
        &request(),
        &settings(),
        &roster(),
        || {
            if read(&root)? != settings() {
                return Err(LocalError::new("dance_settings_changed"));
            }
            Ok(roster())
        },
        |_| {
            fs::write(
                settings_path(&root),
                serde_json::to_vec(&Settings {
                    threshold: 0.8,
                    ..settings()
                })
                .unwrap(),
            )
            .unwrap();
            Ok(response(0.9, 0.1, 0.0))
        },
    );
    assert_eq!(result.unwrap_err().code, "dance_settings_changed");
    fs::remove_dir_all(root).unwrap();
}
fn routed(req: &RouteRequest) -> Routed {
    Routed {
        message_id: req.message_id.clone(),
        target_agent_id: "agent-a".into(),
        fallback: false,
        recorded: true,
        prepared: Some(crate::temporal_context::PreparedComposerPrompt {
            target_agent_id: "agent-a".into(),
            payload: "the exact first prepared payload".into(),
            temporal_gap: None,
        }),
    }
}
#[test]
fn retries_reuse_the_cached_prepared_result_but_changed_identity_is_invalid() {
    let mut routing = Routing::default();
    let req = request();
    assert!(routing.cached(&req).unwrap().is_none());
    let first = routed(&req);
    routing.remember(&req, first.clone());
    assert_eq!(
        serde_json::to_value(routing.cached(&req).unwrap()).unwrap(),
        serde_json::to_value(first).unwrap()
    );
    let mut changed = req.clone();
    changed.payload.push_str("changed");
    assert_eq!(
        routing.cached(&changed).unwrap_err().code,
        "invalid_request"
    );
    changed = req.clone();
    changed.timestamp = "2026-10-09T12:00:01Z".into();
    assert_eq!(
        routing.cached(&changed).unwrap_err().code,
        "invalid_request"
    );
    let mut returned = routing.cached(&req).unwrap().unwrap();
    returned.prepared.as_mut().unwrap().payload.clear();
    assert_eq!(
        routing
            .cached(&req)
            .unwrap()
            .unwrap()
            .prepared
            .unwrap()
            .payload,
        "the exact first prepared payload"
    );
}
#[test]
fn the_process_local_routing_cache_evicts_only_the_oldest_after_128_results() {
    let mut routing = Routing::default();
    let first = request();
    routing.remember(&first, routed(&first));
    for i in 1..=128 {
        let req = RouteRequest {
            message_id: format!("message-{i}"),
            ..request()
        };
        routing.remember(&req, routed(&req));
    }
    assert_eq!(routing.completed.len(), 128);
    assert!(routing.cached(&first).unwrap().is_none());
    for i in [1, 128] {
        let req = RouteRequest {
            message_id: format!("message-{i}"),
            ..request()
        };
        assert_eq!(
            routing.cached(&req).unwrap().unwrap().message_id,
            req.message_id
        );
    }
}
#[test]
fn one_project_has_one_inflight_routing_slot_and_read_only_prompts_are_bundled() {
    let root = Path::new("/fixture/dance");
    let first = routing_lock(root);
    let second = routing_lock(root);
    let _held = first.lock().unwrap();
    assert!(second.try_lock().is_err());
    assert_eq!(prompts()["schema"], 1);
    for (key, q) in [
        ("stringFirst", "model"),
        ("stringLater", "model"),
        ("danceContext", "target"),
        ("danceRule", "target"),
    ] {
        assert_eq!(prompts()[key]["questions"][q]["type"], "choice");
        assert!(
            prompts()[key]["questions"][q]["instructions"]
                .as_str()
                .unwrap()
                .len()
                <= 512
        );
    }
}

#[test]
fn events_snapshot_returns_busy_without_waiting_for_the_route_then_reads_the_same_log() {
    let root = project("events-busy").canonicalize().unwrap();
    let lock = routing_lock(&root);
    let mut held = lock.lock().unwrap();
    held.events.record(EventData::error("jev_auth").unwrap()).unwrap();
    let before = serde_json::to_value(held.events.snapshot()).unwrap();
    let path = root.to_string_lossy().into_owned();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn({
        let path = path.clone();
        move || {
            let result = tauri::async_runtime::block_on(puppeteer_dance_events(path));
            tx.send(result).unwrap();
        }
    });
    let while_held = rx.recv_timeout(std::time::Duration::from_secs(2));
    // Release and join even on the old blocking implementation, so a regression
    // fails this test instead of leaving a blocked worker behind.
    drop(held);
    worker.join().unwrap();
    let after = tauri::async_runtime::block_on(puppeteer_dance_events(path)).unwrap();
    fs::remove_dir_all(root).unwrap();
    assert_eq!(while_held.expect("snapshot must return while the route still holds its lock").unwrap_err().code, "dance_busy");
    assert_eq!(serde_json::to_value(after).unwrap(), before, "busy reads neither erase nor append log entries");
}

#[test]
fn rule_path_prepare_defaults_comments_and_raw_revision_protect_manual_edits() {
    let root = project("rule-path");
    let path = prepare(&root).unwrap();
    let initial = fs::read_to_string(&path).unwrap();
    assert!(initial.starts_with(crate::puppeteer_setup::DANCE_COMMENTS));
    assert_eq!(read(&root).unwrap(), Settings::default());
    prepare(&root).unwrap(); assert_eq!(fs::read_to_string(&path).unwrap(), initial);
    let snapshot = read_snapshot(&root).unwrap();
    let edited = format!("// Manual edit, including comments, must invalidate a stale save.\n{initial}");
    fs::write(&path, &edited).unwrap();
    let manager = IntegrationManager::default();
    let owned = OwnedDirectory::new(&root.join("unused-owned"));
    assert_eq!(save_checked(&manager, root.to_str().unwrap(), &Settings::default(), &owned, &snapshot.source_revision, |_| {}).unwrap_err().code, "dance_settings_conflict");
    assert_eq!(fs::read_to_string(&path).unwrap(), edited);
    let current = read_snapshot(&root).unwrap();
    let saved = save_checked(&manager, root.to_str().unwrap(), &Settings::default(), &owned, &current.source_revision, |_| {}).unwrap();
    assert_eq!(saved.source_revision, read_snapshot(&root).unwrap().source_revision);
    assert!(fs::read_to_string(&path).unwrap().starts_with(crate::puppeteer_setup::DANCE_COMMENTS));
    assert!(!root.join("unused-owned").exists());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn dance_prepare_preserves_existing_rule_values_but_invalid_jsonc_and_unknown_fields_fail_closed() {
    let root = project("jsonc-reader");
    fs::create_dir_all(crate::project_memory_dir(&root)).unwrap();
    let path = settings_path(&root);
    let text = "/* user note */ {\"schema\":1,\"enabled\":false,\"sentinel\":\"agent-a\",\"strategy\":\"rule\",\"threshold\":0.8,\"conditions\":{\"agent-a\":\"https://example/* literal */\",\"agent-b\":\"review\"}}";
    fs::write(&path, text).unwrap(); prepare(&root).unwrap();
    assert!(fs::read_to_string(&path).unwrap().ends_with(text));
    assert_eq!(read(&root).unwrap().conditions["agent-a"], "https://example/* literal */");
    for bad in ["/*bad", "{\"unknown\":1}", "{\"schema\":1,\"enabled\":false,\"sentinel\":null,\"strategy\":\"rule\",\"conditions\":{}}"] {
        fs::write(&path, bad).unwrap(); assert!(read(&root).is_err()); assert!(prepare(&root).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), bad);
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn led_selection_carries_real_probability_or_a_precise_bypass_reason() {
    let decide = |req: &RouteRequest, s: &Settings, seats: &[Seat], reply: Value| {
        super::choose_target(req, s, seats, prompts(), || Ok(seats.to_vec()), |_| Ok(reply)).unwrap()
    };
    let selected = decide(&request(), &settings(), &roster(), response(0.82, 0.1, 0.08));
    let value = serde_json::to_value(EventData::Selection(selected)).unwrap();
    assert_eq!(value["kind"], "selection");
    assert_eq!(value["outcome"], "selected");
    assert_eq!(value["target"]["agentId"], "agent-a");
    assert_eq!(value["candidate"]["probability"], 0.82);
    assert!(value.get("reason").is_none());
    let low = decide(&request(), &settings(), &roster(), response(0.61, 0.2, 0.19));
    assert_eq!(low.target.agent_id, "agent-c");
    assert_eq!(low.candidate.unwrap().agent.agent_id, "agent-a");
    assert_eq!(low.reason, Some(Reason::BelowThreshold));
    let direct = decide(&request(), &settings(), &roster()[..1], Value::Null);
    assert_eq!(direct.outcome, Outcome::Direct); assert!(direct.candidate.is_none());
    assert_eq!(direct.reason, Some(Reason::OnlySeat));
    let empty = decide(&RouteRequest { text: Some(String::new()), ..request() }, &settings(), &roster(), Value::Null);
    assert_eq!(empty.reason, Some(Reason::EmptyText)); assert!(empty.candidate.is_none());
    let no_candidates = decide(&request(), &Settings { strategy: Strategy::Rule, ..settings() }, &roster(), Value::Null);
    assert_eq!(no_candidates.reason, Some(Reason::NoCandidates));
    let s = Settings { strategy: Strategy::Rule, conditions: BTreeMap::from([("agent-a".into(), "A".into())]), ..settings() };
    let single = decide(&request(), &s, &roster(), Value::Null);
    assert_eq!(single.reason, Some(Reason::OnlyCandidate)); assert!(single.candidate.is_none());
    let left = super::choose_target(&request(), &settings(), &roster(), prompts(),
        || Ok(vec![roster()[2].clone()]), |_| Ok(response(0.9,0.05,0.05))).unwrap();
    assert_eq!(left.target.agent_id, "agent-c"); assert_eq!(left.reason, Some(Reason::TargetLeft));
    assert_eq!(left.candidate.unwrap().probability, 0.9);
}

#[test]
fn led_runtime_retains_fifteen_newest_with_stable_time_and_monotonic_ids() {
    let mut routing = Routing::default();
    let first = routing.events.record(EventData::error("jev_timeout").unwrap()).unwrap();
    assert!(chrono::DateTime::parse_from_rfc3339(&first.occurred_at).is_ok());
    let json = serde_json::to_value(&first).unwrap();
    assert!(json["occurredAt"].as_str().unwrap().contains('.'));
    for _ in 0..18 { routing.events.record(EventData::error("dance_record_failed").unwrap()); }
    let before = serde_json::to_value(routing.events.snapshot()).unwrap();
    assert_eq!(before.as_array().unwrap().len(), 15);
    assert_eq!(before[0]["id"], 19); assert_eq!(before[14]["id"], 5);
    assert_eq!(before, serde_json::to_value(routing.events.snapshot()).unwrap());
    let other = Routing::default(); assert!(other.events.snapshot().is_empty());
    assert_eq!(before[0]["kind"], "dance_error");
    assert_eq!(serde_json::to_value(first).unwrap(), json);
}

#[test]
fn led_invalid_entries_do_not_consume_ids_and_guards_do_not_create_errors() {
    let mut log = events::Log::default();
    assert!(EventData::error("dance_busy").is_none()); assert!(EventData::error("invalid_request").is_none());
    for code in ["jev_auth", "jev_timeout", "jev_unreachable", "jev_key_required", "jev_key_rejected",
        "dance_sentinel_required", "dance_settings_changed", "dance_disabled", "dance_record_failed"] {
        assert!(EventData::error(code).is_some());
    }
    let valid = super::choose_target(&request(), &settings(), &roster(), prompts(), || Ok(roster()), |_| Ok(response(0.82,0.1,0.08))).unwrap();
    let mut bad = valid.clone(); bad.candidate.as_mut().unwrap().probability = f64::NAN;
    assert!(log.record(EventData::Selection(bad)).is_none());
    let mut bad = valid.clone(); bad.target.agent_id = "../outside".into();
    assert!(log.record(EventData::Selection(bad)).is_none());
    let mut bad = valid.clone(); bad.target.name = "\0".into();
    assert!(log.record(EventData::Selection(bad)).is_none());
    let mut bad = valid.clone(); bad.threshold = 2.0;
    assert!(log.record(EventData::Selection(bad)).is_none());
    assert!(log.record(EventData::JevError { code: "dance_busy".into() }).is_none());
    assert_eq!(log.record(EventData::Selection(valid)).unwrap().id, 1);
}

#[test]
fn led_production_route_boundary_skips_cached_retry_but_keeps_real_repeat_errors() {
    use std::cell::RefCell;
    let mut routing = Routing::default(); let req = request(); let emitted = RefCell::new(Vec::new());
    let emit = |event: events::Event| emitted.borrow_mut().push(event.id);
    let route = |routing: &mut Routing| {
        let selection = super::choose_target(&req, &settings(), &roster(), prompts(), || Ok(roster()), |_| Ok(response(0.82,0.1,0.08)))?;
        record_event(routing, EventData::Selection(selection), &emit);
        Ok(routed(&req))
    };
    routing.route(&req, &emit, route).unwrap();
    routing.route(&req, &emit, |_| panic!("Retry must not call Jev, prepare, or emit")).unwrap();
    assert_eq!(*emitted.borrow(), vec![1]);
    let later = RouteRequest { message_id: "later".into(), ..request() };
    for _ in 0..2 {
        assert_eq!(routing.route(&later, &emit, |_| Err(LocalError::new("jev_auth"))).unwrap_err().code, "jev_auth");
    }
    assert_eq!(*emitted.borrow(), vec![1,2,3]);
    assert_eq!(routing.events.snapshot().len(), 3);
    let failure = RouteRequest { message_id: "record-failure".into(), ..request() };
    routing.route(&failure, &emit, |routing| { route(routing)?; Err(LocalError::new("dance_record_failed")) }).unwrap_err();
    let snapshot = serde_json::to_value(routing.events.snapshot()).unwrap();
    assert_eq!(snapshot[0]["code"], "dance_record_failed"); assert_eq!(snapshot[1]["kind"], "selection");
}

fn reply(id: &str, at: &str, order: Option<u64>) -> RosterItem {
    RosterItem { id: id.into(), name: id.into(), last_reply: Some(TimedText { at: at.into(), text: format!("Reply from {id}") }),
        last_reply_order: order, recent_user_messages: vec![] }
}

#[test]
fn latest_reply_uses_time_across_pages_and_private_order_only_for_coarse_same_second() {
    let mut req = request();
    req.roster = vec![reply("agent-b", "2026-10-09T11:59:59.789Z", Some(1)), reply("agent-a", "2026-10-09T11:59:59Z", Some(2))];
    assert_eq!(latest_reply_message(&req, &roster()).map(|(id, _)| id), Some("agent-a"));
    let state = request_body(&req, &settings(), &roster())["state"].clone();
    assert_eq!(state["latest_reply"], json!({"agent_id":"agent-a","at":"2026-10-09T11:59:59Z","text":"Reply from agent-a"}));
    assert!(!state.to_string().contains("Order")); assert!(!state.to_string().contains("order"));
    req.roster[1].last_reply.as_mut().unwrap().at = "2026-10-09T11:59:59.100Z".into();
    assert_eq!(latest_reply_message(&req, &roster()).map(|(id, _)| id), Some("agent-b"), "full precision orders by time");
    req.roster[1].last_reply.as_mut().unwrap().at = "2026-10-09T11:59:58Z".into();
    assert_eq!(latest_reply_message(&req, &roster()).map(|(id, _)| id), Some("agent-b"), "later page must not beat a later second");
    req.roster[1].last_reply.as_mut().unwrap().at = "2026-10-09T11:59:59Z".into();
    req.roster[1].last_reply_order = None;
    assert_eq!(latest_reply_message(&req, &roster()).map(|(id, _)| id), Some("agent-b"), "legacy caller falls back to timestamps");
    req.roster[0].last_reply.as_mut().unwrap().at = "2026-10-09T04:59:59-07:00".into();
    assert_eq!(latest_reply_message(&req, &roster()).map(|(id, _)| id), Some("agent-b"), "equal legacy timestamps use stable request order");
}

#[test]
fn latest_reply_excludes_future_equal_invalid_and_departed_agents_and_omits_absent_key() {
    let mut req = request();
    req.roster = vec![reply("agent-a", &req.timestamp, Some(4)), reply("agent-b", "not a date", Some(5)),
        reply("agent-c", "2026-10-09T12:00:01Z", Some(6)), reply("agent-gone", "2026-10-09T11:59:59Z", Some(7))];
    assert_eq!(latest_reply_message(&req, &roster()).map(|(id, _)| id), None);
    for strategy in [Strategy::Context, Strategy::Rule] {
        let s = Settings { strategy, ..settings() };
        assert!(request_body(&req, &s, &roster())["state"].get("latest_reply").is_none());
    }
    req.roster.push(reply("agent-a", "2026-10-09T11:00:00Z", None));
    assert_eq!(latest_reply_message(&req, &roster()).map(|(id, _)| id), Some("agent-a"));
    for strategy in [Strategy::Context, Strategy::Rule] {
        let s = Settings { strategy, conditions: BTreeMap::from([("agent-a".into(), "A".into()), ("agent-b".into(), "B".into())]), ..settings() };
        let body = request_body(&req, &s, &roster());
        assert_eq!(body["state"]["latest_reply"], json!({"agent_id":"agent-a","at":"2026-10-09T11:00:00Z","text":"Reply from agent-a"}), "content belongs to the valid reply, not the first matching id");
        if s.strategy == Strategy::Rule {
            assert!(!body["state"]["roster"].to_string().contains("last_reply"));
        }
    }
}

#[test]
fn latest_reply_content_is_bounded_in_both_strategies_without_extra_rule_history() {
    let mut req = request();
    let at = format!("2026-10-09T11:59:59.{}Z", "0".repeat(80));
    req.roster = vec![reply("agent-b", "2026-10-09T11:00:00Z", Some(2)), reply("agent-a", &at, Some(3))];
    req.roster[1].last_reply.as_mut().unwrap().text = "好".repeat(201);
    req.roster[1].recent_user_messages = request().roster.remove(0).recent_user_messages;
    for strategy in [Strategy::Context, Strategy::Rule] {
        let s = Settings { strategy, conditions: BTreeMap::from([("agent-a".into(), "A".into()), ("agent-b".into(), "B".into())]), ..settings() };
        let state = request_body(&req, &s, &roster())["state"].clone();
        let expected = json!({"agent_id":"agent-a","at":format!("2026-10-09T11:59:59.{}", "0".repeat(44)),"text":"好".repeat(200)});
        assert_eq!(state["latest_reply"], expected);
        assert_eq!(state["latest_reply"]["at"].as_str().unwrap().chars().count(), 64);
        assert!(!state.to_string().contains("order")); assert!(!state.to_string().contains("Order"));
        let candidates = state["roster"].as_array().unwrap();
        if s.strategy == Strategy::Rule {
            assert_eq!(candidates.len(), 2);
            for candidate in candidates {
                assert_eq!(candidate.as_object().unwrap().len(), 3);
                for key in ["id", "name", "condition"] { assert!(candidate.get(key).is_some()); }
            }
        } else {
            assert_eq!(candidates[0]["last_reply"]["text"], "好".repeat(200));
            assert_eq!(candidates[0]["recent_user_messages"].as_array().unwrap().len(), 1);
        }
    }
}

#[test]
fn sentinel_starts_at_seed_follows_the_recipient_and_resets_on_disable_reenable_or_restart() {
    let mut s = Sentinel::default(); let settings = settings();
    assert!(s.for_round(&settings)); assert_eq!(s.current, settings.sentinel);
    let target = choose_target(&request(), &settings, &roster(), || Ok(roster()), |_| Ok(response(0.1,0.9,0.0))).unwrap().0;
    assert!(s.follow(&target)); assert_eq!(s.current.as_deref(), Some("agent-b"));
    assert!(!s.follow(&target), "the same recipient needs no sentinel push");
    assert!(!s.for_round(&settings)); assert_eq!(s.current.as_deref(), Some("agent-b"));
    assert!(!s.sync(&Settings { threshold: 0.5, ..settings.clone() }, false));
    assert_eq!(s.current.as_deref(), Some("agent-b"), "editing the threshold keeps the recipient");
    assert!(s.sync(&Settings { enabled: false, ..settings.clone() }, false)); assert!(s.current.is_none());
    assert!(s.sync(&settings, true)); assert_eq!(s.current, settings.sentinel);
    let mut restarted = Sentinel::default(); restarted.sync(&settings, false); assert_eq!(restarted.current, settings.sentinel);
}

#[test]
fn low_probability_uses_the_seed_until_a_recipient_is_decided_in_both_strategies() {
    for strategy in [Strategy::Context, Strategy::Rule] {
        let settings = Settings { strategy, conditions: BTreeMap::from([("agent-a".into(), "A".into()), ("agent-b".into(), "B".into())]), ..settings() };
        let mut sentinel = Sentinel::default(); sentinel.for_round(&settings);
        let effective = Settings { sentinel: sentinel.current.clone(), ..settings.clone() };
        let selected = super::choose_target(&request(), &effective, &roster(), prompts(), || Ok(roster()), |_| Ok(response(0.5,0.5,0.0))).unwrap();
        assert_eq!(selected.target.agent_id, "agent-c"); assert_eq!(selected.outcome, Outcome::Sentinel);
        assert_eq!(settings.sentinel.as_deref(), Some("agent-c"));
    }
}

#[test]
fn fallback_uses_the_last_recipient_even_when_another_agent_replied_most_recently() {
    for strategy in [Strategy::Context, Strategy::Rule] {
        let settings = Settings { strategy, conditions: BTreeMap::from([("agent-a".into(), "A".into()), ("agent-b".into(), "B".into())]), ..settings() };
        let mut sentinel = Sentinel::default(); sentinel.for_round(&settings);
        let target = choose_target(&request(), &settings, &roster(), || Ok(roster()), |_| Ok(response(0.1,0.9,0.0))).unwrap().0;
        assert!(sentinel.follow(&target));
        let req = RouteRequest { message_id: "next-message".into(), ..request() };
        assert!(!sentinel.for_round(&settings));
        let effective = Settings { sentinel: sentinel.current.clone(), ..settings.clone() };
        let selected = super::choose_target(&req, &effective, &roster(), prompts(), || Ok(roster()), |body| {
            assert_eq!(body["state"]["latest_reply"]["agent_id"], "agent-a", "Jev still sees the most recent reply");
            Ok(response(0.5,0.5,0.0))
        }).unwrap();
        assert_eq!(selected.target.agent_id, "agent-b"); assert_eq!(selected.outcome, Outcome::Sentinel);
        assert!(!sentinel.follow(&selected.target.agent_id));
        assert_eq!(settings.sentinel.as_deref(), Some("agent-c"), "runtime following does not rewrite the seed");
    }
}

#[test]
fn departed_sentinel_errors_only_if_fallback_is_needed_without_silently_selecting_another() {
    let settings = Settings { sentinel: Some("agent-b".into()), ..settings() };
    let mut s = Sentinel::default(); s.sync(&settings, false); s.follow("agent-c");
    let req = request();
    let seats = roster()[..2].to_vec(); s.for_round(&settings);
    let effective = Settings { sentinel: s.current, ..settings };
    assert_eq!(choose_target(&req, &effective, &seats, || Ok(seats.clone()), |_| Ok(response(0.5,0.5,0.0))).unwrap_err().code, "dance_sentinel_required");
    assert_eq!(choose_target(&req, &effective, &seats, || Ok(seats.clone()), |_| Ok(response(0.9,0.1,0.0))).unwrap().0, "agent-a");
}

#[test]
fn current_sentinel_read_does_not_write_settings_or_revision_and_cached_retry_does_not_advance_it() {
    let root = project("runtime-sentinel").canonicalize().unwrap(); fs::create_dir_all(crate::project_memory_dir(&root)).unwrap();
    fs::write(settings_path(&root), serialize(&settings())).unwrap();
    let before = fs::read(settings_path(&root)).unwrap();
    let first = read_current_snapshot(&root).unwrap(); assert_eq!(first.current_sentinel.as_deref(), Some("agent-c"));
    {
        let lock = routing_lock(&root); let mut routing = lock.lock().unwrap();
        let pushes = std::cell::RefCell::new(Vec::new());
        for (req, probabilities, expected) in [
            (request(), response(0.9,0.1,0.0), "agent-a"),
            (RouteRequest { message_id: "second-message".into(), ..request() }, response(0.1,0.9,0.0), "agent-b"),
        ] {
            let result = routing.route(&req, &|_| {}, |routing| {
                routing.sentinel.for_round(&settings());
                let effective = Settings { sentinel: routing.sentinel.current.clone(), ..settings() };
                let selection = super::choose_target(&req, &effective, &roster(), prompts(), || Ok(roster()), |_| Ok(probabilities)).unwrap();
                let target = selection.target.agent_id.clone();
                record_event(routing, EventData::Selection(selection), &|_| {});
                if routing.sentinel.follow(&target) { pushes.borrow_mut().push(target.clone()); }
                Ok(Routed { target_agent_id: target, ..routed(&req) })
            }).unwrap();
            assert_eq!(result.target_agent_id, expected); assert_eq!(routing.sentinel.current.as_deref(), Some(expected));
        }
        let retry = routing.route(&request(), &|_| panic!("no log for cached retry"), |_| panic!("must not update sentinel")).unwrap();
        assert_eq!(retry.target_agent_id, "agent-a");
        assert_eq!(routing.sentinel.current.as_deref(), Some("agent-b"), "retrying an older message never rolls the sentinel back");
        assert_eq!(*pushes.borrow(), vec!["agent-a", "agent-b"]); assert_eq!(routing.events.snapshot().len(), 2);
    }
    let next = read_current_snapshot(&root).unwrap();
    assert_eq!(next.current_sentinel.as_deref(), Some("agent-b")); assert_eq!(next.source_revision, first.source_revision);
    assert_eq!(next.settings, first.settings); assert_eq!(fs::read(settings_path(&root)).unwrap(), before);
    let disabled = Settings { enabled: false, ..settings() };
    let changes = std::cell::RefCell::new(Vec::new());
    let saved = save_checked(&IntegrationManager::default(), root.to_str().unwrap(), &disabled, &OwnedDirectory::new(&root.join("unused")), &next.source_revision,
        |current| changes.borrow_mut().push(current)).unwrap();
    assert!(saved.current_sentinel.is_none()); assert_eq!(*changes.borrow(), vec![None]);
    assert_eq!(saved.settings.sentinel.as_deref(), Some("agent-c"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn new_enable_requires_seated_seed_but_existing_enabled_edits_do_not_and_defaults_do_not_migrate_explicit_values() {
    let f = Fixture::new(json!({})); verified_key(&f);
    let orphaned = Settings { sentinel: Some("agent-left".into()), ..settings() };
    assert_eq!(super::gate(&orphaned, &roster(), &f.backend.owned, true).unwrap_err().code, "dance_sentinel_required");
    super::gate(&orphaned, &roster(), &f.backend.owned, false).unwrap();
    super::gate(&Settings { enabled: false, sentinel: None, ..settings() }, &[], &f.backend.owned, true).unwrap();
    let missing = br#"{"schema":1,"enabled":false,"sentinel":null,"strategy":"context"}"#;
    assert_eq!(decode(Some(missing)).unwrap().threshold, 0.5); assert_eq!(decode(None).unwrap().threshold, 0.5);
    assert_eq!(decode(Some(&serialize(&settings()))).unwrap().threshold, 0.75);
    let root = project("threshold-default"); let path = prepare(&root).unwrap();
    assert_eq!(read(&root).unwrap().threshold, 0.5);
    assert!(fs::read_to_string(&path).unwrap().contains("// threshold: 0.05-0.95 in steps of 0.05; the default is 0.50."));
    let existing = String::from_utf8(serialize(&settings())).unwrap().replace("threshold: 0.05-0.95", "threshold: 0.50-0.95").replace("the default is 0.50", "the default is 0.85");
    fs::write(&path, &existing).unwrap(); assert_eq!(read(&root).unwrap().threshold, 0.75);
    assert_eq!(fs::read_to_string(&path).unwrap(), existing, "reading does not migrate saved values or comments");
    prepare(&root).unwrap();
    let prepared = fs::read_to_string(&path).unwrap();
    assert_eq!(prepared, format!("// threshold: 0.05-0.95 in steps of 0.05; the default is 0.50.\n{existing}"));
    assert_eq!(read(&root).unwrap().threshold, 0.75);
    prepare(&root).unwrap(); assert_eq!(fs::read_to_string(&path).unwrap(), prepared);
    fs::remove_dir_all(root).unwrap();
}
