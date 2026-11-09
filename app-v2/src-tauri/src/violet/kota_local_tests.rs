use super::*;
use serde_json::json;

#[path = "kota_local_route_tests.rs"]
mod route_tests;

const SESSION_FIXTURE: &str = include_str!("fixtures/kota_local/session-basic.json");
const AGENT: &str = "agent-fixture-local";
const SESSION: &str = "sess-fixture-local";
const BUS_MESSAGE: &str = "agentbus-agent-fixture-peer-agent-fixture-local-0001";

struct LocalProject {
    root: PathBuf,
}

impl LocalProject {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "kota-local-violet-{label}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn scope(&self, epoch: u64) -> KotaLocalScope {
        self.scope_in(SESSION, epoch)
    }

    fn scope_in(&self, session_id: &str, epoch: u64) -> KotaLocalScope {
        KotaLocalScope {
            project_root: self.root.clone(),
            agent_id: AGENT.into(),
            session_id: session_id.into(),
            epoch,
        }
    }

    fn messages(&self) -> Vec<VioletChatMessage> {
        read_room_messages(&self.root, &room_request(None)).unwrap()
    }

    fn texts(&self) -> Vec<String> {
        self.messages().into_iter().map(|message| message.text).collect()
    }

    fn agent(&self) -> ProjectAgent {
        ProjectAgent {
            agent_id: AGENT.into(),
            shell: KOTA_LOCAL_SHELL.into(),
            cwd: self.root.join(".agent-workspaces").join(AGENT),
            session_id: None,
        }
    }

    fn synced(&self) -> (Vec<VioletSourceStatus>, Vec<AgentBusReceipt>, Vec<LocalPendingDelivery>) {
        let mut sources = Vec::new();
        let mut receipts = Vec::new();
        let mut pending = Vec::new();
        assert!(sync_agent(
            &self.root,
            &self.agent(),
            &mut sources,
            &mut receipts,
            &mut pending
        ));
        (sources, receipts, pending)
    }

    fn write_agent_yaml(&self) {
        let dir = self.root.join(".agent-workspaces").join(AGENT);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("agent.yaml"),
            format!("id: {AGENT}\nprovider: {KOTA_LOCAL_SHELL}\nstatus: active\n"),
        )
        .unwrap();
    }
}

impl Drop for LocalProject {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn room_request(agent_ids: Option<Vec<String>>) -> VioletRoomRequest {
    VioletRoomRequest {
        project_root: None,
        limit: Some(500),
        before: None,
        around: None,
        agent_ids,
        watch_agent_ids: None,
    }
}

fn fixture_entries() -> (Vec<JsonValue>, String) {
    let data: JsonValue = serde_json::from_str(SESSION_FIXTURE).unwrap();
    (
        data["entries"].as_array().unwrap().clone(),
        data["leafId"].as_str().unwrap().to_string(),
    )
}

fn full_read<'a>(entries: &'a [JsonValue], leaf: &'a str) -> KotaLocalEntriesRead<'a> {
    KotaLocalEntriesRead {
        entries,
        leaf_id: Some(leaf),
        since: None,
        recovery: false,
    }
}

fn since_read<'a>(entries: &'a [JsonValue], leaf: &'a str, since: &'a str) -> KotaLocalEntriesRead<'a> {
    KotaLocalEntriesRead {
        entries,
        leaf_id: Some(leaf),
        since: Some(since),
        recovery: false,
    }
}

fn kota_input_details(message_id: &str, created_at: &str, private: bool) -> JsonValue {
    json!({
        "schema": 1,
        "messageId": message_id,
        "targetAgentId": AGENT,
        "createdAt": created_at,
        "private": private,
        "kotaId": format!("{message_id}#{AGENT}"),
    })
}

fn custom_entry(id: &str, parent: Option<&str>, custom_type: &str, content: JsonValue, details: JsonValue) -> JsonValue {
    json!({
        "type": "custom_message",
        "id": id,
        "parentId": parent,
        "timestamp": "2026-10-02T00:05:00.000Z",
        "customType": custom_type,
        "content": content,
        "display": true,
        "details": details,
    })
}

fn assistant_entry(id: &str, parent: &str, timestamp: &str, text: &str) -> JsonValue {
    json!({
        "type": "message",
        "id": id,
        "parentId": parent,
        "timestamp": timestamp,
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": text}],
            "provider": "kota-fake",
            "model": "fixture",
            "stopReason": "stop",
            "timestamp": 1790899260000i64,
        },
    })
}

#[test]
fn history_image_only_tool_row_remains_a_cursor_and_parent_after_projection() {
    let project = LocalProject::new("history-empty-tool");
    let input = custom_entry("input", None, "kota.user", json!("Visible input"),
        kota_input_details("image-history", "2026-10-02T00:05:00Z", false));
    let entries = vec![input, json!({"id":"empty-tool","parentId":"input","type":"message",
        "timestamp":"2026-10-02T00:05:01Z","message":{"role":"toolResult","toolName":"read","content":[]}})];
    let mut read = full_read(&entries, "empty-tool"); read.recovery = true;
    let outcome = ingest_entries(&project.scope(1), &read).unwrap();
    assert_eq!(outcome.examined, 2);
    assert_eq!(outcome.cursor.as_deref(), Some("empty-tool"));
    assert_eq!(entries[1]["message"]["content"], json!([]));
    let reply = vec![assistant_entry("reply", "empty-tool", "2026-10-02T00:05:02Z", "Reply after image")];
    ingest_entries(&project.scope(1), &since_read(&reply, "reply", "empty-tool")).unwrap();
    assert!(project.texts().contains(&"Reply after image".into()));
    ingest_entries(&project.scope(1), &since_read(&reply, "reply", "empty-tool")).unwrap();
    assert_eq!(project.texts().iter().filter(|text| text.as_str() == "Reply after image").count(), 1);
}

#[test]
fn long_local_endturn_saves_full_markdown_once_with_bounded_history() {
    let project = LocalProject::new("overflow");
    let full = format!("{}LOCAL_FULL_TAIL", "文🙂".repeat(11_000));
    let entry = assistant_entry("long-reply", "root", "2026-10-02T00:05:00Z", &full);
    let entries = vec![entry];
    ingest_entries(&project.scope(1), &full_read(&entries, "long-reply")).unwrap();
    let messages = project.messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].text.chars().count(), MAX_EVENT_TEXT_CHARS);
    let path = project.root.join(messages[0].overflow_path.as_ref().unwrap());
    assert_eq!(fs::read_to_string(&path).unwrap(), full);
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    ingest_entries(&project.scope(1), &full_read(&entries, "long-reply")).unwrap();
    assert_eq!(project.messages().len(), 1);
    assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), modified);
    let history = fs::read_to_string(chathistory_events_dir(&project.root).join("2026-10-02.jsonl")).unwrap();
    assert!(!history.contains("LOCAL_FULL_TAIL"));
}

fn delivery<'a>(
    message_id: &'a str,
    kota_id: &'a str,
    state: KotaLocalDeliveryState,
    custom_type: &'a str,
) -> KotaLocalDelivery<'a> {
    KotaLocalDelivery {
        kota_id,
        message_id,
        state,
        at: "2026-10-02T00:00:03Z",
        custom_type,
    }
}

#[test]
fn clone_reports_and_private_control_never_become_room_messages_or_delivery_rows() {
    let project = LocalProject::new("clone-report");
    let entry = custom_entry("clone-report", None, "kota.shadow_clone", json!("Clone #1 finished:\nSynthetic reply"),
        kota_input_details("clone-report", "2026-10-09T00:00:00Z", true));
    let entries = vec![entry, assistant_entry("parent-reply", "clone-report", "2026-10-09T00:00:01Z", "Parent reply")];
    ingest_entries(&project.scope(1), &full_read(&entries, "parent-reply")).unwrap();
    record_delivery(&project.scope(1), &delivery("clone-report", "clone-report#agent-fixture-local",
        KotaLocalDeliveryState::Consumed, "kota.shadow_clone")).unwrap();
    assert_eq!(project.texts(), ["Parent reply"]);
    assert!(!serde_json::to_string(&project.messages()).unwrap().contains("kota.clone.v1:"));
    assert_eq!(crate::violet::PARSED_EVENT_CACHE_VERSION, "8");
}

#[test]
fn full_read_projects_only_the_active_path() {
    let project = LocalProject::new("full");
    let (entries, leaf) = fixture_entries();

    let outcome = ingest_entries(&project.scope(1), &full_read(&entries, &leaf)).unwrap();

    let messages = project.messages();
    let shape = messages
        .iter()
        .map(|message| (message.role.as_str(), message.kind.as_str(), message.text.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        shape,
        vec![
            ("user", "message", "Read the current task."),
            ("assistant", "commentary", "Reading the task file first."),
            ("assistant", "message", "The task asks for a summary; tests look fine."),
            ("system", "compaction", "Summarized the task review."),
            ("assistant", "message", "Kota Local error from kota-fake/fixture: 529 overloaded."),
        ]
    );
    assert!(messages
        .iter()
        .all(|message| message.agent_id == AGENT && message.shell == KOTA_LOCAL_SHELL));

    let user = &messages[0];
    assert_eq!(user.timestamp, "2026-10-02T00:00:01+00:00");
    assert_eq!(user.native_event_id.as_deref(), Some("msg-fixture-user-1#agent-fixture-local"));
    let reply = &messages[2];
    assert_eq!(reply.native_event_id.as_deref(), Some("sess-fixture-local/e08:0"));
    assert_eq!(reply.model.as_deref(), Some("fixture"));
    assert_eq!(reply.effort.as_deref(), Some("high"));
    assert_eq!(messages[4].native_event_id.as_deref(), Some("sess-fixture-local/e11:error"));

    assert_eq!(outcome.examined, 14);
    assert_eq!(outcome.off_path, 1);
    assert_eq!(outcome.cursor.as_deref(), Some("e14"));
    let text = project.texts().join("\n");
    for hidden in [
        "Abandoned branch reply.",
        "KOTA_MESSAGE",
        "/kota-inject",
        "Task: summarize the review.",
        "Plan the read.",
        "You are a coding assistant.",
    ] {
        assert!(!text.contains(hidden), "{hidden} reached the room");
    }
}

#[test]
fn repeated_and_overlapping_reads_do_not_duplicate_room_records() {
    let project = LocalProject::new("idempotent");
    let (entries, leaf) = fixture_entries();
    let scope = project.scope(1);
    ingest_entries(&scope, &full_read(&entries, &leaf)).unwrap();
    let first = project.messages();

    let recovery = KotaLocalEntriesRead {
        recovery: true,
        ..full_read(&entries, &leaf)
    };
    ingest_entries(&scope, &recovery).unwrap();
    ingest_entries(&scope, &since_read(&entries[9..], &leaf, "e09")).unwrap();
    // A brand-new state file must still dedupe against existing room records.
    fs::remove_dir_all(local_state_dir(&project.root)).unwrap();
    ingest_entries(&scope, &full_read(&entries, &leaf)).unwrap();
    assert_eq!(project.messages().len(), first.len());

    let next = [assistant_entry("e15", "e14", "2026-10-02T00:01:00.000Z", "Retry succeeded.")];
    let outcome = ingest_entries(&scope, &since_read(&next, "e15", "e14")).unwrap();
    assert_eq!(outcome.written, 1);
    assert_eq!(outcome.cursor.as_deref(), Some("e15"));
    let texts = project.texts();
    assert_eq!(texts.len(), first.len() + 1);
    assert_eq!(texts.last().map(String::as_str), Some("Retry succeeded."));
}

#[test]
fn since_read_skips_entries_off_the_active_path() {
    let project = LocalProject::new("branch");
    let (entries, leaf) = fixture_entries();
    let scope = project.scope(1);
    ingest_entries(&scope, &full_read(&entries, &leaf)).unwrap();
    let before = project.messages().len();

    let batch = [
        assistant_entry("e15", "e08", "2026-10-02T00:01:00.000Z", "Sibling branch reply."),
        assistant_entry("e16", "e14", "2026-10-02T00:01:01.000Z", "Main branch reply."),
    ];
    let outcome = ingest_entries(&scope, &since_read(&batch, "e16", "e14")).unwrap();

    assert_eq!(outcome.off_path, 1);
    let texts = project.texts();
    assert_eq!(texts.len(), before + 1);
    assert!(texts.iter().any(|text| text == "Main branch reply."));
    assert!(!texts.iter().any(|text| text == "Sibling branch reply."));
}

#[test]
fn live_kota_user_and_its_entry_collapse_to_one_record() {
    let project = LocalProject::new("live-user");
    let scope = project.scope(1);
    let (entries, leaf) = fixture_entries();
    let custom = entries.iter().find(|entry| entry["id"] == "e04").unwrap();
    let live = json!({
        "type": "message_end",
        "message": {
            "role": "custom",
            "customType": "kota.user",
            "content": custom["content"],
            "display": true,
            "details": custom["details"],
            "timestamp": 1790899205000i64,
        },
    });

    record_live_event(&scope, &live).unwrap();
    let live_records = project.messages();
    assert_eq!(live_records.len(), 1);
    assert_eq!(live_records[0].role, "user");
    assert_eq!(live_records[0].timestamp, "2026-10-02T00:00:01+00:00");

    ingest_entries(&scope, &full_read(&entries, &leaf)).unwrap();
    let users = project
        .messages()
        .into_iter()
        .filter(|message| message.role == "user")
        .collect::<Vec<_>>();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].id, live_records[0].id);
}

#[test]
fn private_inputs_and_privacy_spans_stay_out_of_the_room() {
    let project = LocalProject::new("private");
    let scope = project.scope(1);
    let private_input = [custom_entry(
        "p1",
        None,
        "kota.user",
        json!("secret plan"),
        kota_input_details("msg-private-1", "2026-10-02T00:00:01Z", true),
    )];
    let outcome = ingest_entries(&scope, &full_read(&private_input, "p1")).unwrap();
    assert_eq!(outcome.skipped_private, 1);
    assert!(project.messages().is_empty());

    let violet_dir = project.root.join("project-memory").join(".violet");
    fs::create_dir_all(&violet_dir).unwrap();
    fs::write(
        violet_dir.join("privacy-spans.jsonl"),
        format!(
            "{}\n",
            json!({"agentId": AGENT, "startedAt": "2026-10-02T00:00:00Z", "endedAt": "2026-10-02T00:30:00Z"})
        ),
    )
    .unwrap();
    let reply = [assistant_entry("p2", "p1", "2026-10-02T00:00:09.000Z", "private reply")];
    let outcome = ingest_entries(&scope, &since_read(&reply, "p2", "p1")).unwrap();
    assert_eq!(outcome.skipped_private, 1);
    assert!(project.messages().is_empty());
}

#[test]
fn bus_inputs_confirm_receipts_without_bubbles() {
    let project = LocalProject::new("bus");
    let (entries, leaf) = fixture_entries();

    let outcome = ingest_entries(&project.scope(1), &full_read(&entries, &leaf)).unwrap();

    assert_eq!(outcome.receipts, 1);
    assert!(project.texts().iter().all(|text| !text.contains("Please also check the tests.")));
    let (sources, receipts, pending) = project.synced();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].source_kind, KOTA_LOCAL_SOURCE_KIND);
    assert_eq!(sources[0].status, "synced");
    assert_eq!(sources[0].session_id.as_deref(), Some(SESSION));
    assert!(sources[0].error.is_none());
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].event_id, BUS_MESSAGE);
    assert_eq!(receipts[0].agent_id, AGENT);
    assert!(pending.is_empty());
}

#[test]
fn delivery_receipts_come_only_from_consumed_or_durable_facts() {
    let project = LocalProject::new("receipts");
    let scope = project.scope(1);
    let kota_id = format!("agentbus-x#{AGENT}");
    use crate::local_rpc::DeliveryState::*;

    for state in [Held, Sent, Accepted, ClearRequested] {
        record_delivery(&scope, &delivery("agentbus-x", &kota_id, state, "kota.bus")).unwrap();
    }
    assert!(project.synced().1.is_empty());

    record_delivery(&scope, &delivery("agentbus-x", &kota_id, Consumed, "kota.bus")).unwrap();
    record_delivery(&scope, &delivery("agentbus-x", &kota_id, Durable, "kota.bus")).unwrap();
    let receipts = project.synced().1;
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].event_id, "agentbus-x");
    assert_eq!(receipts[0].timestamp, "2026-10-02T00:00:03+00:00");

    let composer_id = format!("msg-c#{AGENT}");
    record_delivery(&scope, &delivery("msg-c", &composer_id, Consumed, "kota.user")).unwrap();
    let unknown_id = format!("agentbus-y#{AGENT}");
    record_delivery(&scope, &delivery("agentbus-y", &unknown_id, Consumed, "other.extension")).unwrap();
    record_delivery(&scope, &delivery("agentbus-z", "agentbus-z#agent-other", Consumed, "kota.bus")).unwrap();
    assert_eq!(project.synced().1.len(), 1);
}

#[test]
fn pending_deliveries_follow_the_current_session_until_settled() {
    let project = LocalProject::new("pending");
    project.write_agent_yaml();
    let scope = project.scope(1);
    let composer_id = format!("msg-composer#{AGENT}");
    let bus_id = format!("agentbus-pending#{AGENT}");
    use crate::local_rpc::DeliveryState::*;

    record_delivery(&scope, &delivery("msg-composer", &composer_id, Held, "kota.user")).unwrap();
    record_delivery(&scope, &delivery("agentbus-pending", &bus_id, Accepted, "kota.bus")).unwrap();
    let pending = project.synced().2;
    assert_eq!(
        pending,
        vec![
            LocalPendingDelivery { message_id: "msg-composer".into(), target_agent_id: AGENT.into() },
            LocalPendingDelivery { message_id: "agentbus-pending".into(), target_agent_id: AGENT.into() },
        ]
    );
    assert_eq!(cached_pending_deliveries(&project.root, None), pending);
    let other_agent = vec!["agent-other".to_string()];
    assert!(cached_pending_deliveries(&project.root, Some(other_agent.as_slice())).is_empty());

    // A late lower fact never reopens a consumed item.
    record_delivery(&scope, &delivery("msg-composer", &composer_id, Consumed, "kota.user")).unwrap();
    record_delivery(&scope, &delivery("msg-composer", &composer_id, Accepted, "kota.user")).unwrap();
    // A failed item leaves the list and writes one content-free error line;
    // a repeated notice of the same fact writes nothing.
    let wrote_room = |paths: &[PathBuf]| paths.iter().any(|path| path.ends_with("latest.jsonl"));
    let first = record_delivery(&scope, &delivery("agentbus-pending", &bus_id, Failed, "kota.bus")).unwrap();
    let repeat = record_delivery(&scope, &delivery("agentbus-pending", &bus_id, Failed, "kota.bus")).unwrap();
    assert!(wrote_room(&first));
    assert!(repeat.is_empty());
    assert!(project.synced().2.is_empty());
    assert_eq!(
        project.texts(),
        vec!["Kota Local could not deliver Agent Bus message agentbus-pending.".to_string()]
    );

    // A manual retry reopens a failed item, and failing again writes another
    // line (the room view folds identical lines within two minutes).
    record_delivery(&scope, &delivery("agentbus-pending", &bus_id, Held, "kota.bus")).unwrap();
    assert_eq!(project.synced().2.len(), 1);
    let again = record_delivery(
        &scope,
        &KotaLocalDelivery { at: "2026-10-02T00:09:00Z", ..delivery("agentbus-pending", &bus_id, Failed, "kota.bus") },
    )
    .unwrap();
    assert!(wrote_room(&again));
    assert!(project.synced().2.is_empty());
    record_delivery(&scope, &delivery("agentbus-pending", &bus_id, Held, "kota.bus")).unwrap();
    assert_eq!(project.synced().2.len(), 1);

    // Fresh moves to a new session: old-session items stop holding the room.
    let fresh = project.scope_in("sess-fixture-fresh", 2);
    let fresh_id = format!("msg-fresh#{AGENT}");
    record_delivery(&fresh, &delivery("msg-fresh", &fresh_id, Sent, "kota.user")).unwrap();
    assert_eq!(
        project.synced().2,
        vec![LocalPendingDelivery { message_id: "msg-fresh".into(), target_agent_id: AGENT.into() }]
    );
}

#[test]
fn live_notices_from_an_older_epoch_are_ignored() {
    let project = LocalProject::new("epoch");
    let kota_id = format!("agentbus-old#{AGENT}");
    use crate::local_rpc::DeliveryState::*;
    record_delivery(&project.scope(2), &delivery("agentbus-new", &format!("agentbus-new#{AGENT}"), Sent, "kota.bus")).unwrap();

    record_delivery(&project.scope(1), &delivery("agentbus-old", &kota_id, Consumed, "kota.bus")).unwrap();
    record_runtime_status(
        &project.scope(1),
        &KotaLocalRuntimeStatus { state: "error", queued: 0, compacting: false, error: Some("old child exited") },
    )
    .unwrap();
    assert!(project.synced().1.is_empty());
    assert!(project.messages().is_empty());

    // Durable entries from an older child are still recorded.
    let (entries, leaf) = fixture_entries();
    ingest_entries(&project.scope(1), &full_read(&entries, &leaf)).unwrap();
    assert!(!project.messages().is_empty());
    assert_eq!(project.synced().2.len(), 1);
}

#[test]
fn runtime_faults_render_content_free_error_lines() {
    let project = LocalProject::new("faults");
    let scope = project.scope(1);
    let error_status = KotaLocalRuntimeStatus {
        state: "error",
        queued: 1,
        compacting: false,
        error: Some("pi exited with status 1"),
    };

    for event in [
        json!({"type": "message_update", "message": {"role": "assistant"}}),
        json!({"type": "auto_retry_end", "success": true, "attempt": 2}),
        json!({"type": "auto_retry_end", "success": false, "attempt": 3, "finalError": "529 overloaded"}),
        json!({"type": "compaction_end", "reason": "manual", "aborted": true, "willRetry": false}),
        json!({"type": "compaction_end", "reason": "threshold", "aborted": false, "willRetry": false, "errorMessage": "summary request failed"}),
        json!({"type": "extension_error", "extensionPath": "command:kota-inject", "event": "command", "error": "kota-inject-failed kotaId=unknown code=invalid-json"}),
        json!({"type": "extension_error", "extensionPath": "/fixture/workspace/kota-ext/index.ts", "event": "tool_call", "error": "secret stack trace"}),
    ] {
        record_live_event(&scope, &event).unwrap();
    }
    record_runtime_status(&scope, &error_status).unwrap();
    record_runtime_status(&scope, &error_status).unwrap();
    record_runtime_status(
        &scope,
        &KotaLocalRuntimeStatus { state: "ready", queued: 0, compacting: false, error: None },
    )
    .unwrap();

    // auto_retry_end(false) repeats the final failed attempt, which Pi
    // persists and the entries path already renders.
    let mut texts = project.texts();
    texts.sort();
    assert_eq!(
        texts,
        vec![
            "Kota Local compaction failed: summary request failed.".to_string(),
            "Kota Local extension error in the tool_call handler.".to_string(),
            "Kota Local runtime error: pi exited with status 1.".to_string(),
        ]
    );
    assert!(project.messages().iter().all(|message| message.role == "assistant"
        && message.agent_id == AGENT
        && !message.text.contains("secret")
        && !message.text.contains("/fixture/workspace")));
}

#[test]
fn malformed_or_foreign_kota_inputs_are_ignored() {
    let project = LocalProject::new("malformed");
    let mut mismatched = kota_input_details("msg-a", "2026-10-02T00:00:01Z", false);
    mismatched["kotaId"] = json!("msg-other#agent-fixture-local");
    let mut extra_field = kota_input_details("msg-b", "2026-10-02T00:00:01Z", false);
    extra_field["note"] = json!("not in schema 1");
    let mut wrong_schema = kota_input_details("msg-c", "2026-10-02T00:00:01Z", false);
    wrong_schema["schema"] = json!(2);
    let mut foreign_target = kota_input_details("msg-d", "2026-10-02T00:00:01Z", false);
    foreign_target["targetAgentId"] = json!("agent-other");
    foreign_target["kotaId"] = json!("msg-d#agent-other");
    let entries = [
        custom_entry("m1", None, "kota.user", json!("one"), mismatched),
        custom_entry("m2", Some("m1"), "kota.user", json!("two"), extra_field),
        custom_entry("m3", Some("m2"), "kota.user", json!("three"), wrong_schema),
        custom_entry("m4", Some("m3"), "kota.user", json!("four"), foreign_target.clone()),
        custom_entry("m5", Some("m4"), "other.extension", json!("five"), kota_input_details("msg-e", "2026-10-02T00:00:01Z", false)),
        custom_entry("m6", Some("m5"), "kota.bus", json!("six"), foreign_target),
    ];

    let outcome = ingest_entries(&project.scope(1), &full_read(&entries, "m6")).unwrap();

    assert_eq!(outcome.receipts, 0);
    assert!(project.messages().is_empty());
    assert!(project.synced().1.is_empty());
}

#[test]
fn sync_project_routes_kota_local_agents_without_file_discovery() {
    let project = LocalProject::new("sync");
    project.write_agent_yaml();
    let (entries, leaf) = fixture_entries();
    let scope = project.scope(1);
    ingest_entries(&scope, &full_read(&entries, &leaf)).unwrap();
    let pending_id = format!("msg-pending#{AGENT}");
    record_delivery(
        &scope,
        &delivery("msg-pending", &pending_id, KotaLocalDeliveryState::Accepted, "kota.user"),
    )
    .unwrap();

    let state = sync_project(&project.root, room_request(None)).unwrap();

    let source = state.sources.iter().find(|source| source.agent_id == AGENT).unwrap();
    assert_eq!(source.source_kind, KOTA_LOCAL_SOURCE_KIND);
    assert_eq!(source.status, "synced");
    assert!(source.error.is_none());
    assert!(state
        .agent_bus_receipts
        .iter()
        .any(|receipt| receipt.agent_id == AGENT && receipt.event_id == BUS_MESSAGE));
    assert_eq!(
        state.local_pending_deliveries,
        vec![LocalPendingDelivery { message_id: "msg-pending".into(), target_agent_id: AGENT.into() }]
    );
    let json = serde_json::to_value(&state).unwrap();
    assert_eq!(
        json["localPendingDeliveries"],
        json!([{"messageId": "msg-pending", "targetAgentId": AGENT}])
    );
    let cached = read_cache(&project.root, room_request(None)).unwrap();
    assert_eq!(cached.local_pending_deliveries, state.local_pending_deliveries);
    assert!(!state.messages.is_empty());
}

#[test]
fn room_state_json_has_no_local_field_without_local_agents() {
    let project = LocalProject::new("no-local");

    let synced = serde_json::to_value(sync_project(&project.root, room_request(None)).unwrap()).unwrap();
    let cached = serde_json::to_value(read_cache(&project.root, room_request(None)).unwrap()).unwrap();

    assert!(synced.get("localPendingDeliveries").is_none());
    assert!(cached.get("localPendingDeliveries").is_none());
}

// Recorded with real Pi 1.0.0 `--mode rpc` against the loopback fake provider
// (see fixtures/kota_local/recorded/RECEIPT.json). Identities are synthetic.
const RECORDED_FULL: &str = include_str!("fixtures/kota_local/recorded/full.json");
const RECORDED_SINCE: &str = include_str!("fixtures/kota_local/recorded/since.json");
const RECORDED_EVENTS: &str = include_str!("fixtures/kota_local/recorded/events.jsonl");
const RECORDED_AGENT: &str = "agent-fixture";
const RECORDED_SESSION: &str = "sess-recorded";

fn recorded_scope(project: &LocalProject) -> KotaLocalScope {
    KotaLocalScope {
        project_root: project.root.clone(),
        agent_id: RECORDED_AGENT.into(),
        session_id: RECORDED_SESSION.into(),
        epoch: 1,
    }
}

fn recorded(data: &str) -> (Vec<JsonValue>, String) {
    let data: JsonValue = serde_json::from_str(data).unwrap();
    (
        data["entries"].as_array().unwrap().clone(),
        data["leafId"].as_str().unwrap().to_string(),
    )
}

fn recorded_index(entries: &[JsonValue], id: &str) -> usize {
    entries.iter().position(|entry| entry["id"] == id).unwrap()
}

fn texts_with(project: &LocalProject, role: &str, kind: &str) -> Vec<String> {
    project
        .messages()
        .into_iter()
        .filter(|message| message.role == role && message.kind == kind)
        .map(|message| message.text)
        .collect()
}

const RECORDED_400_LINE: &str =
    "Kota Local error from kota-fake/fixture-alternate: Synthetic permanent fixture error (HTTP 400).";
const RECORDED_500_LINE: &str =
    "Kota Local error from kota-fake/fixture-alternate: 500 synthetic transient fixture failure (HTTP 500).";

#[test]
fn recorded_session_full_read_projects_one_record_per_outcome() {
    let project = LocalProject::new("recorded-full");
    let (entries, leaf) = recorded(RECORDED_FULL);

    ingest_entries(&recorded_scope(&project), &full_read(&entries, &leaf)).unwrap();

    let mut users = texts_with(&project, "user", "message");
    users.sort();
    assert_eq!(
        users,
        vec![
            "Synthetic image input.",
            "Synthetic permanent provider error task.",
            "Synthetic plain user text.",
            "Synthetic read task.",
            "Synthetic retry exhaustion task.",
        ]
    );
    let replies = texts_with(&project, "assistant", "message");
    for expected in [
        "Synthetic plain reply.",
        "Synthetic image reply.",
        "Synthetic bus reply.",
        "Synthetic Telegram reply.",
        "Synthetic reminder reply.",
        "Synthetic read completed.",
    ] {
        assert_eq!(replies.iter().filter(|text| *text == expected).count(), 1, "{expected}");
    }
    // The retried 500 attempt is dropped by its context_edit; the final
    // failure and the permanent 400 each leave exactly one line.
    assert_eq!(replies.iter().filter(|text| *text == RECORDED_400_LINE).count(), 1);
    assert_eq!(replies.iter().filter(|text| *text == RECORDED_500_LINE).count(), 1);
    assert_eq!(texts_with(&project, "system", "compaction").len(), 1);

    let all = project.texts().join("\n");
    for hidden in [
        "Synthetic private text.",
        "KOTA_MESSAGE",
        "Synthetic handoff body.",
        "Harmless Kota Local read fixture.",
        "Synthetic reasoning fixture",
        "You are an expert coding",
    ] {
        assert!(!all.contains(hidden), "{hidden} reached the room");
    }
    let reply = project
        .messages()
        .into_iter()
        .find(|message| message.text == "Synthetic read completed.")
        .unwrap();
    assert_eq!(reply.model.as_deref(), Some("fixture"));
    assert_eq!(reply.effort.as_deref(), Some("off"));

    let mut receipts = sync_receipts(&project);
    receipts.sort();
    assert_eq!(receipts, vec!["msg-bus", "msg-ember", "msg-lm"]);
}

fn sync_receipts(project: &LocalProject) -> Vec<String> {
    let agent = ProjectAgent {
        agent_id: RECORDED_AGENT.into(),
        shell: KOTA_LOCAL_SHELL.into(),
        cwd: project.root.clone(),
        session_id: None,
    };
    let (mut sources, mut receipts, mut pending) = (Vec::new(), Vec::new(), Vec::new());
    assert!(sync_agent(&project.root, &agent, &mut sources, &mut receipts, &mut pending));
    receipts.into_iter().map(|receipt| receipt.event_id).collect()
}

fn recorded_errors(project: &LocalProject) -> Vec<String> {
    texts_with(project, "assistant", "message")
        .into_iter()
        .filter(|text| text.starts_with("Kota Local error"))
        .collect()
}

fn live(scope: &KotaLocalScope, event_type: &str) {
    record_live_event(scope, &json!({ "type": event_type })).unwrap();
}

#[test]
fn recorded_live_reads_hold_a_failed_leaf_until_the_run_settles() {
    let project = LocalProject::new("recorded-held");
    let scope = recorded_scope(&project);
    let (entries, _) = recorded(RECORDED_FULL);
    let task_400 = recorded_index(&entries, "a9aa3971");
    let error_400 = recorded_index(&entries, "13a99d67");
    let attempt_500 = recorded_index(&entries, "07d87710");
    let final_500 = recorded_index(&entries, "a868a9a8");

    // New owner: full read up to the 400 task.
    ingest_entries(&scope, &full_read(&entries[..=task_400], "a9aa3971")).unwrap();
    live(&scope, "agent_start");
    // turn_end of the 400 attempt: the failed leaf is held.
    ingest_entries(&scope, &since_read(&entries[error_400..=error_400], "13a99d67", "a9aa3971")).unwrap();
    assert!(recorded_errors(&project).is_empty());
    // agent_settled without a retry: the failure was final.
    live(&scope, "agent_settled");
    assert_eq!(recorded_errors(&project), vec![RECORDED_400_LINE.to_string()]);

    live(&scope, "agent_start");
    // turn_end of the first 500 attempt: held.
    ingest_entries(&scope, &since_read(&entries[error_400 + 1..=attempt_500], "07d87710", "13a99d67")).unwrap();
    live(&scope, "auto_retry_start");
    // An empty read during the retry delay (e.g. a reconnect) proves nothing.
    ingest_entries(&scope, &since_read(&[], "07d87710", "07d87710")).unwrap();
    assert_eq!(recorded_errors(&project).len(), 1);
    live(&scope, "agent_start");
    // The retry's turn_end carries the context_edit that drops the attempt.
    ingest_entries(&scope, &since_read(&entries[attempt_500 + 1..=final_500], "a868a9a8", "07d87710")).unwrap();
    assert_eq!(recorded_errors(&project).len(), 1);
    // The exhausted retry settles with exactly one more line.
    live(&scope, "agent_settled");
    assert_eq!(
        recorded_errors(&project),
        vec![RECORDED_400_LINE.to_string(), RECORDED_500_LINE.to_string()]
    );
    // A later read does not repeat it.
    ingest_entries(&scope, &since_read(&[], "a868a9a8", "a868a9a8")).unwrap();
    assert_eq!(recorded_errors(&project).len(), 2);
}

#[test]
fn a_failed_leaf_read_after_the_run_settled_is_written_at_once() {
    // A get_entries answer can reach the sink after agent_settled did.
    let project = LocalProject::new("recorded-late-read");
    let scope = recorded_scope(&project);
    let (entries, _) = recorded(RECORDED_FULL);
    let task_400 = recorded_index(&entries, "a9aa3971");
    let error_400 = recorded_index(&entries, "13a99d67");
    ingest_entries(&scope, &full_read(&entries[..=task_400], "a9aa3971")).unwrap();
    live(&scope, "agent_start");
    live(&scope, "agent_settled");

    ingest_entries(&scope, &since_read(&entries[error_400..=error_400], "13a99d67", "a9aa3971")).unwrap();

    assert_eq!(recorded_errors(&project), vec![RECORDED_400_LINE.to_string()]);
}

#[test]
fn recorded_since_read_appends_the_next_turn() {
    let project = LocalProject::new("recorded-since");
    let scope = recorded_scope(&project);
    let (entries, leaf) = recorded(RECORDED_FULL);
    let (next, next_leaf) = recorded(RECORDED_SINCE);
    ingest_entries(&scope, &full_read(&entries, &leaf)).unwrap();
    let before = project.messages().len();

    let outcome = ingest_entries(&scope, &since_read(&next, &next_leaf, &leaf)).unwrap();

    assert_eq!(outcome.written, 2);
    assert_eq!(outcome.cursor.as_deref(), Some(next_leaf.as_str()));
    assert_eq!(project.messages().len(), before + 2);
    assert!(project.texts().iter().any(|text| text == "Synthetic incremental task."));
    assert!(project.texts().iter().any(|text| text == "Synthetic incremental reply."));
}

#[test]
fn recorded_live_events_collapse_with_entries_and_render_only_faults() {
    let project = LocalProject::new("recorded-events");
    let scope = recorded_scope(&project);
    for line in RECORDED_EVENTS.lines().filter(|line| !line.trim().is_empty()) {
        let event: JsonValue = serde_json::from_str(line).unwrap();
        record_live_event(&scope, &event).unwrap();
    }

    let users_live = texts_with(&project, "user", "message");
    assert!(users_live.iter().any(|text| text == "Synthetic plain user text."));
    assert!(!users_live.iter().any(|text| text == "Synthetic private text."));
    assert!(texts_with(&project, "assistant", "message").iter().all(|text| !text.starts_with("Kota Local error")));
    assert_eq!(
        project
            .texts()
            .into_iter()
            .filter(|text| text.starts_with("Kota Local extension error"))
            .collect::<Vec<_>>(),
        vec!["Kota Local extension error in the command handler.".to_string()]
    );

    // The durable read finds the same inputs: one record each, same ids.
    let ids_live = project
        .messages()
        .into_iter()
        .filter(|message| message.role == "user")
        .map(|message| message.id)
        .collect::<HashSet<_>>();
    let (entries, leaf) = recorded(RECORDED_FULL);
    ingest_entries(&scope, &full_read(&entries, &leaf)).unwrap();
    let ids_after = project
        .messages()
        .into_iter()
        .filter(|message| message.role == "user")
        .map(|message| message.id)
        .collect::<HashSet<_>>();
    // Six non-private kota.user inputs were consumed live (the recording also
    // covers the incremental turn); the full read adds no second record.
    assert_eq!(ids_live.len(), 6);
    assert_eq!(ids_after, ids_live);
}

// 颦儿's real Pi run through the bridge (all seven sources; createdAt / kotaId
// produced by the extension). Every reply reads the same and every input has
// the same text, which makes it an idempotency and batching probe.
const SEVEN_SOURCES: &str = include_str!("fixtures/kota_local/recorded/seven-sources.json");
// The same scenario as kept for the gate-one run (颦儿, sha256 b140ee38…).
const SEVEN_SOURCES_GATE1: &str =
    include_str!("fixtures/kota_local/recorded/seven-sources-gate1.json");
const SEVEN_AGENT: &str = "agent-fable-copper";

fn seven_scope(project: &LocalProject) -> KotaLocalScope {
    KotaLocalScope {
        project_root: project.root.clone(),
        agent_id: SEVEN_AGENT.into(),
        session_id: "sess-seven".into(),
        epoch: 1,
    }
}

fn seven_entries_from(data: &str) -> (Vec<JsonValue>, String) {
    let entries = serde_json::from_str::<Vec<JsonValue>>(data).unwrap();
    let leaf = entries.last().unwrap()["id"].as_str().unwrap().to_string();
    (entries, leaf)
}

fn chathistory_records(project: &LocalProject) -> Vec<ChathistoryEvent> {
    let mut records = Vec::new();
    for entry in fs::read_dir(chathistory_events_dir(&project.root)).unwrap().flatten() {
        records.extend(read_chathistory_event_file(&entry.path()).unwrap());
    }
    records
}

fn turn_credits(project: &LocalProject) -> usize {
    fs::read_to_string(crate::credit_events_path(&project.root))
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
}

#[test]
fn seven_source_read_projects_one_user_record_and_six_receipts() {
    for data in [SEVEN_SOURCES, SEVEN_SOURCES_GATE1] {
        assert_seven_source_projection(data);
    }
}

fn assert_seven_source_projection(data: &str) {
    let project = LocalProject::new("seven");
    let (entries, leaf) = seven_entries_from(data);

    ingest_entries(&seven_scope(&project), &full_read(&entries, &leaf)).unwrap();

    let records = chathistory_records(&project);
    let users = records.iter().filter(|record| record.role == "user").collect::<Vec<_>>();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].text, "Original text\n  stays intact.");
    assert_eq!(
        records
            .iter()
            .filter(|record| record.role == "assistant" && record.text == "Synthetic final reply.")
            .count(),
        7
    );
    let agent = ProjectAgent {
        agent_id: SEVEN_AGENT.into(),
        shell: KOTA_LOCAL_SHELL.into(),
        cwd: project.root.clone(),
        session_id: None,
    };
    let (mut sources, mut receipts, mut pending) = (Vec::new(), Vec::new(), Vec::new());
    assert!(sync_agent(&project.root, &agent, &mut sources, &mut receipts, &mut pending));
    let mut receipt_ids = receipts.into_iter().map(|receipt| receipt.event_id).collect::<Vec<_>>();
    receipt_ids.sort();
    assert_eq!(receipt_ids, vec!["input-1", "input-2", "input-3", "input-4", "input-5", "input-6"]);
}

#[test]
fn each_reply_is_one_record_and_one_turn_whatever_the_read_batching() {
    for data in [SEVEN_SOURCES, SEVEN_SOURCES_GATE1] {
        assert_batching_independent(data);
    }
}

fn assert_batching_independent(data: &str) {
    let (entries, leaf) = seven_entries_from(data);
    let whole = LocalProject::new("seven-whole");
    ingest_entries(&seven_scope(&whole), &full_read(&entries, &leaf)).unwrap();
    let recovery = KotaLocalEntriesRead {
        recovery: true,
        ..full_read(&entries, &leaf)
    };
    ingest_entries(&seven_scope(&whole), &recovery).unwrap();

    // The same session read turn by turn, as turn_end reads would see it.
    let chunked = LocalProject::new("seven-chunked");
    let scope = seven_scope(&chunked);
    let first_leaf = entries[2]["id"].as_str().unwrap().to_string();
    ingest_entries(&scope, &full_read(&entries[..3], &first_leaf)).unwrap();
    let mut since = first_leaf;
    for turn in entries[3..].chunks(2) {
        let turn_leaf = turn.last().unwrap()["id"].as_str().unwrap().to_string();
        ingest_entries(&scope, &since_read(turn, &turn_leaf, &since)).unwrap();
        since = turn_leaf;
    }

    let ids = |project: &LocalProject| {
        let mut ids = chathistory_records(project)
            .into_iter()
            .map(|record| record.id)
            .collect::<Vec<_>>();
        ids.sort();
        ids
    };
    assert_eq!(ids(&whole), ids(&chunked));
    assert_eq!(
        chathistory_records(&whole)
            .iter()
            .filter(|record| record.role == "assistant")
            .count(),
        7
    );
    assert_eq!(turn_credits(&whole), 7);
    assert_eq!(turn_credits(&chunked), 7);
}

#[test]
fn a_new_app_process_accepts_its_restarted_epochs() {
    let project = LocalProject::new("epoch-restart");
    use crate::local_rpc::DeliveryState::*;
    let before = format!("agentbus-before#{AGENT}");
    record_delivery(&project.scope(7), &delivery("agentbus-before", &before, Consumed, "kota.bus")).unwrap();

    // App restart: child epochs count from the start again.
    forget_process_epochs(&project.root, AGENT);
    let after = format!("agentbus-after#{AGENT}");
    record_delivery(&project.scope(1), &delivery("agentbus-after", &after, Consumed, "kota.bus")).unwrap();

    let mut receipts = project.synced().1.into_iter().map(|receipt| receipt.event_id).collect::<Vec<_>>();
    receipts.sort();
    assert_eq!(receipts, vec!["agentbus-after", "agentbus-before"]);
}

#[test]
fn a_recovery_read_rebuilds_the_pending_set_from_re_reported_deliveries() {
    let project = LocalProject::new("pending-recovery");
    project.write_agent_yaml();
    use crate::local_rpc::DeliveryState::*;
    let settled = format!("agentbus-settled#{AGENT}");
    let open = format!("msg-open#{AGENT}");
    record_delivery(&project.scope(3), &delivery("agentbus-settled", &settled, Accepted, "kota.bus")).unwrap();
    record_delivery(&project.scope(3), &delivery("msg-open", &open, Held, "kota.user")).unwrap();
    assert_eq!(project.synced().2.len(), 2);

    // The App exited before the settled item's terminal notice was persisted.
    forget_process_epochs(&project.root, AGENT);
    let (entries, leaf) = fixture_entries();
    let recovery = KotaLocalEntriesRead {
        recovery: true,
        ..full_read(&entries, &leaf)
    };
    ingest_entries(&project.scope(1), &recovery).unwrap();
    assert!(project.synced().2.is_empty());
    // The recovered owner re-reports what is still open, unchanged or not.
    record_delivery(&project.scope(1), &delivery("msg-open", &open, Sent, "kota.user")).unwrap();

    assert_eq!(
        project.synced().2,
        vec![LocalPendingDelivery { message_id: "msg-open".into(), target_agent_id: AGENT.into() }]
    );
    // A plain (non-recovery) read never drops pending items.
    ingest_entries(&project.scope(1), &full_read(&entries, &leaf)).unwrap();
    assert_eq!(project.synced().2.len(), 1);
}

#[test]
fn a_failed_recovery_read_keeps_the_pending_set() {
    let project = LocalProject::new("pending-recovery-fail");
    project.write_agent_yaml();
    use crate::local_rpc::DeliveryState::*;
    let open = format!("msg-open#{AGENT}");
    record_delivery(&project.scope(3), &delivery("msg-open", &open, Held, "kota.user")).unwrap();
    // Make the room write fail: a file where the events directory belongs.
    let events_dir = chathistory_events_dir(&project.root);
    if events_dir.exists() {
        fs::remove_dir_all(&events_dir).unwrap();
    }
    fs::create_dir_all(events_dir.parent().unwrap()).unwrap();
    fs::write(&events_dir, "not a directory").unwrap();

    forget_process_epochs(&project.root, AGENT);
    let (entries, leaf) = fixture_entries();
    let recovery = KotaLocalEntriesRead {
        recovery: true,
        ..full_read(&entries, &leaf)
    };

    assert!(ingest_entries(&project.scope(1), &recovery).is_err());
    assert_eq!(
        project.synced().2,
        vec![LocalPendingDelivery { message_id: "msg-open".into(), target_agent_id: AGENT.into() }]
    );
    assert_eq!(project.synced().0[0].status, "error");
}

#[test]
fn cache_reads_ignore_pending_items_of_an_agent_switched_off_kota_local() {
    let project = LocalProject::new("pending-switched");
    project.write_agent_yaml();
    use crate::local_rpc::DeliveryState::*;
    let open = format!("msg-open#{AGENT}");
    record_delivery(&project.scope(1), &delivery("msg-open", &open, Held, "kota.user")).unwrap();
    assert_eq!(cached_pending_deliveries(&project.root, None).len(), 1);

    // Same agent, now saved on a terminal provider (v6.4 carrier switch).
    fs::write(
        project.root.join(".agent-workspaces").join(AGENT).join("agent.yaml"),
        format!("id: {AGENT}\nprovider: codex\nstatus: active\n"),
    )
    .unwrap();
    assert!(cached_pending_deliveries(&project.root, None).is_empty());
    let state = read_cache(&project.root, room_request(None)).unwrap();
    assert!(state.local_pending_deliveries.is_empty());
}

#[test]
fn runtime_authored_error_sentences_reach_the_room_verbatim() {
    let project = LocalProject::new("runtime-sentence");
    let sentence = "Kota Local stopped because it could not accept messages. It will restart and resend queued messages when the next message arrives.";
    record_runtime_status(
        &project.scope(1),
        &KotaLocalRuntimeStatus { state: "error", queued: 1, compacting: false, error: Some(sentence) },
    )
    .unwrap();

    assert_eq!(project.texts(), vec![sentence.to_string()]);
}

#[test]
fn queue_command_errors_stay_with_the_runtime_but_other_extension_errors_render() {
    let project = LocalProject::new("queue-command-errors"); let scope = project.scope(1);
    for command in ["kota-inject", "kota-cancel", "kota-clear-held", "kota-hold-pause", "kota-hold-resume", "kota-reorder", "kota-mcp"] {
        record_live_event(&scope, &json!({"type":"extension_error","extensionPath":format!("command:{command}"),
            "event":"command","error":"synthetic unexpected command failure"})).unwrap();
    }
    for action in ["pause", "resume"] {
        record_live_event(&scope, &json!({"type":"extension_ui_request","method":"notify","message":format!("kota.queue.v1:{}", json!({"schema":1,"requestId":"local-1-4","action":action,"kotaIds":[format!("{BUS_MESSAGE}#{AGENT}")]}))})).unwrap();
    }
    record_live_event(&scope, &json!({"type":"extension_ui_request","method":"notify",
        "message":"kota.mcp.v1:{\"schema\":1,\"requestId\":\"local-1-2\",\"available\":true,\"servers\":[{\"name\":\"fake-server\",\"status\":\"connected\"}]}"})).unwrap();
    // Auto Route facts and its route command belong to the runtime and workbench, never the room.
    for fact in [json!({"schema":1,"event":"decision","selectedModel":"model-1","routeModel":"model-2","fallback":true}),
        json!({"schema":1,"event":"model_error","provider":"fable","model":"reason","message":"quota"}),
        json!({"schema":1,"event":"error","code":"jev_auth"})] {
        record_live_event(&scope, &json!({"type":"extension_ui_request","method":"notify","message":format!("kota.route.v1:{fact}")})).unwrap();
    }
    record_live_event(&scope, &json!({"type":"extension_error","extensionPath":"command:kota-auto-route",
        "event":"command","error":"kota-auto-route-failed code=invalid"})).unwrap();
    assert!(project.messages().is_empty());
    assert_eq!(crate::violet::PARSED_EVENT_CACHE_VERSION, "8");
    record_live_event(&scope, &json!({"type":"extension_error","extensionPath":"command:other-extension",
        "event":"command","error":"synthetic other failure"})).unwrap();
    assert_eq!(project.texts(), vec!["Kota Local extension error in the command handler."]);
}

#[test]
fn late_consumption_after_soft_clear_produces_the_original_bus_receipt() {
    let project = LocalProject::new("soft-clear-receipt"); let scope = project.scope(1);
    let kota_id = format!("{BUS_MESSAGE}#{AGENT}");
    use crate::local_rpc::DeliveryState::*;
    for state in [Accepted, Cleared] {
        record_delivery(&scope, &delivery(BUS_MESSAGE, &kota_id, state, "kota.bus")).unwrap();
    }
    assert!(project.synced().1.is_empty()); assert!(project.synced().2.is_empty());
    for state in [Consumed, Durable, Abandoned] {
        record_delivery(&scope, &delivery(BUS_MESSAGE, &kota_id, state, "kota.bus")).unwrap();
    }
    let receipts = project.synced().1;
    assert_eq!(receipts.len(), 1); assert_eq!(receipts[0].event_id, BUS_MESSAGE);
    assert!(project.synced().2.is_empty());
}

#[test]
fn pending_from_an_earlier_app_is_ignored_without_rewriting_room_history() {
    let project = LocalProject::new("process-pending"); project.write_agent_yaml();
    let (entries, leaf) = fixture_entries();
    ingest_entries(&project.scope(1), &full_read(&entries, &leaf)).unwrap();
    let before = serde_json::to_value(project.messages()).unwrap();
    let kota_id = format!("msg-stale#{AGENT}");
    record_delivery(&project.scope(1), &delivery("msg-stale", &kota_id, crate::local_rpc::DeliveryState::Accepted, "kota.user")).unwrap();
    assert_eq!(cached_pending_deliveries(&project.root, None).len(), 1);
    with_state(&project.root, AGENT, |state| { state.process_start = Some("earlier-app-process".into()); Ok(()) }).unwrap();
    assert!(cached_pending_deliveries(&project.root, None).is_empty());
    assert_eq!(serde_json::to_value(project.messages()).unwrap(), before);
    assert!(!project.synced().1.is_empty(), "existing receipts remain durable");
}
