use super::*;
use crate::pty::agent::AgentCli;

fn prefix(id: &str) -> String {
    crate::shell_switch::handoff_text(
        id,
        "agent-fixture73",
        "Fixture Agent",
        "fixture-zenith-river-73",
        "Zenith River 73",
        Path::new("/tmp/fixture-zenith-river-73/project-memory"),
        AgentCli::Codex,
        AgentCli::Claude,
        "2026-09-20T12:00:00Z",
    )
}
fn agent(cwd: PathBuf) -> ProjectAgent {
    ProjectAgent {
        agent_id: "agent-fixture73".into(),
        shell: "claude".into(),
        cwd,
        session_id: None,
    }
}
fn source(path: PathBuf) -> NativeSource {
    NativeSource {
        kind: "claude-jsonl".into(),
        session_id: "new-session".into(),
        path,
        aux_path: None,
    }
}

#[test]
fn shell_handoff_is_split_before_truncation_and_bus_receipt_extraction() {
    let agent = agent(PathBuf::from("/tmp/unused"));
    let source = source(PathBuf::from("/tmp/source"));
    let id = uuid::Uuid::new_v4().to_string();
    let body = format!("<KOTA_MESSAGE id=\"agentbus-receipt-test\" from=\"bbs\" to=\"agent-fixture73\" intent=\"bbs-thread\">\n{}\n</KOTA_MESSAGE>", "a".repeat(12_000));
    let native = event(
        &agent,
        &source,
        "user",
        "message",
        "2026-09-20T12:00:01Z",
        "native-id",
        format!("{}{body}", prefix(&id)),
    );
    assert_eq!(native.text, truncate_chars(&body, MAX_EVENT_TEXT_CHARS));
    assert!(native.shell_handoff.is_some());
    let expanded = expand_shell_handoffs(vec![native]);
    assert_eq!(expanded.len(), 2);
    assert!(native_work_event(&expanded[0]).is_none());
    assert_eq!(native_work_event(&expanded[1]).unwrap().state, "working");
    assert_eq!(expanded[0].message_origin.as_deref(), Some("shell_handoff"));
    assert_eq!(
        expanded[0].native_event_id.as_deref(),
        Some(format!("kota-handoff:{id}").as_str())
    );
    assert!(agent_bus_receipt_from_event(&expanded[1]).is_some());
    assert_eq!(
        filter_internal_agent_bus_envelopes(expanded.clone()).len(),
        1
    );
    let mut later = expanded[0].clone();
    later.timestamp = "2026-09-21T12:00:01Z".into();
    later.session_id = "resumed".into();
    assert_eq!(
        event_to_message(expanded[0].clone()).id,
        event_to_message(later.clone()).id
    );
    assert_eq!(
        dedupe_native_events(vec![expanded[0].clone(), later]).len(),
        1
    );
}

#[test]
fn provider_attachment_markers_do_not_hide_handoff_or_change_original_message() {
    let agent = agent(PathBuf::from("/tmp/unused"));
    let source = source(PathBuf::from("/tmp/source"));
    let markers = "[Image #13]\n";
    let original = "<KOTA_MESSAGE id=\"agentbus-image-test\" from=\"a\" to=\"agent-fixture73\" intent=\"handoff\">\nimage task\n</KOTA_MESSAGE>\n\n";
    let parsed = parse_codex_line(
        &agent,
        &source,
        1,
        serde_json::json!({
            "type":"event_msg", "timestamp":"2026-09-20T12:00:01Z",
            "payload":{"type":"user_message", "message":format!("{markers}{}{original}", prefix(&uuid::Uuid::new_v4().to_string()))}
        }),
    );
    let expanded = expand_shell_handoffs(parsed);
    assert_eq!(expanded.len(), 2);
    assert_eq!(expanded[1].text, format!("{markers}{original}"));
    assert_eq!(expanded[0].message_origin.as_deref(), Some("shell_handoff"));
    assert!(agent_bus_receipt_from_event(&expanded[1]).is_some());
}

#[test]
fn six_native_user_parsers_preserve_handoff_and_original_and_privacy() {
    let root = std::env::temp_dir().join(format!("kota-switch-parser-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let id = "95a4a248-1585-4cae-84a8-baf64a929097";
    let input = format!("{}Do the next task.", prefix(id));
    let timestamp = "2026-09-20T12:00:01Z";
    for provider in ["claude", "codex", "antigravity", "kimi", "pi", "opencode"] {
        let mut agent = agent(root.clone());
        agent.shell = provider.into();
        // The parser's source metadata is deterministic for the public fixture;
        // only the SQLite parser below needs a real temporary file.
        let mut source = source(PathBuf::from("/tmp/fixture-zenith-river-73/source.jsonl"));
        let parsed = match provider {
            "claude" => parse_claude_line(
                &agent,
                &source,
                1,
                serde_json::json!({
                    "type":"user", "timestamp":timestamp, "uuid":"native-id", "message":{"role":"user", "content":input}
                }),
            ),
            "codex" => parse_codex_line(
                &agent,
                &source,
                1,
                serde_json::json!({
                    "type":"event_msg", "timestamp":timestamp, "payload":{"type":"user_message", "message":input}
                }),
            ),
            "antigravity" => parse_antigravity_line(
                &agent,
                &source,
                1,
                serde_json::json!({
                    "source":"USER_EXPLICIT", "type":"USER_INPUT", "timestamp":timestamp, "content":input
                }),
            ),
            "kimi" => parse_kimi_line(
                &agent,
                &source,
                1,
                serde_json::json!({
                    "type":"turn.prompt", "timestamp":timestamp, "input":input
                }),
            ),
            "pi" => parse_pi_message_entry(
                &agent,
                &source,
                &PiSessionEntry {
                    id: "entry".into(),
                    parent_id: None,
                    timestamp: timestamp.into(),
                    json: serde_json::json!({"message":{"role":"user", "content":[{"type":"text", "text":input}]}}),
                },
            ),
            _ => {
                source.path = root.join("opencode.db");
                let conn = Connection::open(&source.path).unwrap();
                conn.execute_batch("create table message(id text, session_id text, time_created integer, data text); create table part(id text, session_id text, message_id text, time_created integer, data text);").unwrap();
                conn.execute(
                    "insert into message values ('m', 'new-session', 1790000000000, ?1)",
                    [
                        serde_json::json!({"role":"user", "time":{"created":1790000000000i64}})
                            .to_string(),
                    ],
                )
                .unwrap();
                conn.execute(
                    "insert into part values ('p', 'new-session', 'm', 1790000000000, ?1)",
                    [serde_json::json!({"type":"text", "text":input}).to_string()],
                )
                .unwrap();
                drop(conn);
                parse_opencode_sqlite(&agent, &source).unwrap()
            }
        };
        let expanded = expand_shell_handoffs(parsed);
        assert_eq!(expanded.len(), 2, "{provider}");
        assert_eq!(
            expanded[0].message_origin.as_deref(),
            Some("shell_handoff"),
            "{provider}"
        );
        assert_eq!(expanded[1].text, "Do the next task.", "{provider}");
        assert!(native_work_event(&expanded[0]).is_none());
        let spans = vec![PrivacySpan {
            agent_id: "agent-fixture73".into(),
            started_at: "2026-01-01T00:00:00Z".into(),
            ended_at: Some("2027-01-01T00:00:00Z".into()),
        }];
        assert_eq!(
            partition_private(expanded.clone(), &spans).0.len(),
            0,
            "{provider}"
        );
        if provider == "claude" && std::env::var_os("KOTA_SHELL_SWITCH_FIXTURE").is_some() {
            let messages: Vec<_> = expanded.into_iter().map(event_to_message).collect();
            println!(
                "KOTA_SHELL_SWITCH_FIXTURE={}",
                serde_json::to_string(&messages).unwrap()
            );
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn paused_old_lookup_cannot_overwrite_a_later_generation_of_the_same_provider() {
    let cwd = std::env::temp_dir().join(format!("kota-switch-stale-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&cwd).unwrap();
    let path = cwd.join("agent.yaml");
    fs::write(
        &path,
        "id: agent-fixture73\nprovider: claude\nshell: claude\nshell-generation: old\n",
    )
    .unwrap();
    let old_agent = agent(cwd.clone());
    let old_source = source(cwd.join("old.jsonl"));
    fs::write(&old_source.path, "{}").unwrap();
    let expected = read_yaml_file(&path)
        .unwrap()
        .as_ref()
        .map(binding_fingerprint);
    let (ready, started) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        ready.send(()).unwrap();
        wait.recv().unwrap();
        finish_source_lookup(&old_agent, expected, Ok(Some(old_source))).unwrap()
    });
    started.recv().unwrap();
    let latest = "id: agent-fixture73\nprovider: claude\nshell: claude\nshell-generation: newer\nhandoff-pending: {id: preserve-me}\n";
    fs::write(&path, latest).unwrap();
    release.send(()).unwrap();
    assert!(thread.join().unwrap().is_none());
    assert_eq!(fs::read_to_string(&path).unwrap(), latest);
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn literal_handoff_tags_remain_user_text() {
    let agent = agent(PathBuf::from("/tmp/unused"));
    let source = source(PathBuf::from("/tmp/source"));
    let quoted = format!(
        "Please review this:\n{}hello",
        prefix(&uuid::Uuid::new_v4().to_string())
    );
    let parsed = event(
        &agent,
        &source,
        "user",
        "message",
        "2026-09-20T12:00:01Z",
        "native",
        quoted.clone(),
    );
    assert!(parsed.shell_handoff.is_none());
    assert_eq!(parsed.text, quoted);
}

#[test]
fn old_binding_cannot_replace_a_new_provider_or_write_during_startup() {
    let cwd = std::env::temp_dir().join(format!("kota-switch-binding-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&cwd).unwrap();
    let old = agent(cwd.clone());
    let source = source(cwd.join("old.jsonl"));
    fs::write(&source.path, "{}").unwrap();
    let path = cwd.join("agent.yaml");
    let bytes = "id: agent-fixture73\nprovider: codex\nshell: codex\nshell-generation: newer\nsession-id: current\n";
    fs::write(&path, bytes).unwrap();
    write_agent_session_binding(&old, &source).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), bytes);
    fs::write(
        &path,
        "id: agent-fixture73\nprovider: claude\nshell: claude\n",
    )
    .unwrap();
    let lock = crate::shell_switch::metadata_lock(&cwd);
    lock.lock().unwrap().starting = Some("starting".into());
    let before = fs::read(&path).unwrap();
    write_agent_session_binding(&old, &source).unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
    lock.lock().unwrap().starting = None;
    write_agent_session_binding(&old, &source).unwrap();
    assert!(fs::read_to_string(&path).unwrap().contains("new-session"));
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn late_old_log_append_does_not_pass_new_session_birth_fence() {
    let cwd = std::env::temp_dir().join(format!("kota-switch-birth-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&cwd).unwrap();
    let agent = agent(cwd.clone());
    let old = source(cwd.join("old.jsonl"));
    fs::write(&old.path, "{}").unwrap();
    let cutoff = chrono::Utc::now();
    fs::write(
        cwd.join("agent.yaml"),
        format!(
            "id: agent-fixture73\nprovider: claude\nshell-generation: new\nsession-reset-at: {}\n",
            cutoff.to_rfc3339()
        ),
    )
    .unwrap();
    fs::write(&old.path, "late old data").unwrap();
    assert!(!native_source_is_after_agent_session_reset(&agent, &old));
    let new = source(cwd.join("new.jsonl"));
    fs::write(&new.path, "{}").unwrap();
    assert!(native_source_is_after_agent_session_reset(&agent, &new));
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn shared_sqlite_database_uses_the_session_birth_not_database_mtime() {
    let cwd = std::env::temp_dir().join(format!("kota-switch-db-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&cwd).unwrap();
    let mut agent = agent(cwd.clone());
    agent.shell = "opencode".into();
    let cutoff = chrono::Utc::now().timestamp_millis();
    fs::write(
        cwd.join("agent.yaml"),
        format!(
            "id: agent-fixture73\nprovider: opencode\nshell-generation: new\nsession-reset-at: {}\n",
            chrono::DateTime::from_timestamp_millis(cutoff)
                .unwrap()
                .to_rfc3339()
        ),
    )
    .unwrap();
    let db = cwd.join("opencode.db");
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch("create table session (id text, time_created integer);")
        .unwrap();
    conn.execute(
        "insert into session values ('old', ?1), ('new', ?2)",
        params![cutoff - 1000, cutoff + 1000],
    )
    .unwrap();
    drop(conn);
    let mut source = source(db);
    source.kind = "opencode-sqlite".into();
    source.session_id = "old".into();
    assert!(!native_source_is_after_agent_session_reset(&agent, &source));
    source.session_id = "new".into();
    assert!(native_source_is_after_agent_session_reset(&agent, &source));
    fs::remove_dir_all(cwd).unwrap();
}
