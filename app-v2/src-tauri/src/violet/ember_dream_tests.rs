use super::*;

struct DreamFixture(PathBuf);

impl DreamFixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("kota-dream-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn path(&self) -> PathBuf {
        self.0.join("dreams.md")
    }
}

impl Drop for DreamFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn inbox_entry(home: &Path, root: &Path, agent: &str, text: Option<&str>) -> crate::dream_entry::Entry {
    fs::create_dir_all(root).unwrap();
    let env = crate::soul::cli::Env {
        agent_cwd: root.join(".agent-workspaces").join(agent), kota_home: home.to_path_buf(),
        agent_id: agent.into(), project: None, jev_ready: Some(false),
    };
    let items = text.into_iter().map(str::to_string).collect::<Vec<_>>();
    let entry = crate::dream_entry::Entry::new(&env, root, None, &items, text.is_none(), "2026-01-02T01:02:03.456Z").unwrap();
    crate::dream_entry::submit(home, &entry).unwrap();
    entry
}

fn pending_consolidation(
    fixture: &DreamFixture,
    current: &[String],
    candidate: &str,
) -> (PathBuf, Vec<EmberDreamEntryRecord>) {
    let home = fixture.0.join("account");
    inbox_entry(
        &home,
        &fixture.0.join("project"),
        "agent-fixture",
        Some(candidate),
    );
    write_active_dreams_markdown(&home.join("dreams/dreams.md"), current, "fixture", 0).unwrap();
    let records = collect_unprocessed_dream_entries(&home, &HashSet::new())
        .unwrap()
        .into_iter()
        .flatten()
        .collect();
    (home, records)
}

#[test]
fn ember_dream_full_list_keeps_active_and_discards_unselected_candidates() {
    let fixture = DreamFixture::new();
    let current = (0..10)
        .map(|index| format!("Durable observation {index}."))
        .collect::<Vec<_>>();
    let (home, records) = pending_consolidation(&fixture, &current, "Unselected candidate.");
    let rows = current
        .iter()
        .enumerate()
        .map(
            |(index, text)| serde_json::json!({"from": ember_active_dream_id(index), "text": text}),
        )
        .collect::<Vec<_>>();
    let output = serde_json::json!({"dreams": rows}).to_string();
    assert_eq!(
        finish_ember_dream_consolidation(&home, &current, &output, "fixture", &records).unwrap(),
        0
    );
    assert_eq!(
        read_active_dream_entries_at(&home.join("dreams/dreams.md")).unwrap(),
        current
    );
    assert!(!home.join("dreams/old_dreams.md").exists());
    assert!(crate::dream_entry::read_inbox(&home, false)
        .unwrap()
        .is_empty());
    let ledger = fs::read_dir(home.join("dreams/entries"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(fs::read_to_string(ledger)
        .unwrap()
        .contains(&records[0].event_id));
}

#[test]
fn ember_dream_final_list_replaces_an_active_and_persists_model_order() {
    let fixture = DreamFixture::new();
    let current = vec![
        "Keep this.".into(),
        "Retire this.".into(),
        "Keep that.".into(),
    ];
    let (home, records) = pending_consolidation(&fixture, &current, "New insight.");
    let output = r#"{"dreams":[
        {"from":"active-3","text":"Keep that."},
        {"from":null,"text":"New insight."},
        {"from":"active-1","text":"Keep this."}
    ]}"#;
    assert_eq!(
        finish_ember_dream_consolidation(&home, &current, output, "fixture", &records).unwrap(),
        1
    );
    let active = read_active_dreams_at(&home.join("dreams/dreams.md")).unwrap();
    assert_eq!(
        active
            .iter()
            .map(|item| item.text.as_str())
            .collect::<Vec<_>>(),
        ["Keep that.", "New insight.", "Keep this."]
    );
    assert_eq!(
        active.iter().map(|item| item.position).collect::<Vec<_>>(),
        [0, 1, 2]
    );
    let archive = fs::read_to_string(home.join("dreams/old_dreams.md")).unwrap();
    assert!(archive.starts_with("# Old Dreams\n\n## "));
    assert_eq!(
        archive
            .lines()
            .filter(|line| line.starts_with("- "))
            .collect::<Vec<_>>(),
        ["- Retire this."]
    );
}

#[test]
fn ember_dream_rewrite_archives_old_identity_without_migrating_usage() {
    use crate::soul::dream_usage;
    let fixture = DreamFixture::new();
    let current = vec!["Original insight.".into(), "Keep this.".into()];
    let (home, records) = pending_consolidation(&fixture, &current, "Additional evidence.");
    let old_id = dream_id(&current[0]);
    let keep_id = dream_id(&current[1]);
    dream_usage::append_pushes(
        &home,
        "agent-fixture",
        "fixture-message",
        "2026-01-02T00:00:00Z",
        &[(&old_id, 0.95), (&keep_id, 0.95)],
    )
    .unwrap();
    let ledger = home.join("dreams/usage.jsonl");
    let usage_before = fs::read(&ledger).unwrap();
    let output = r#"{"dreams":[
        {"from":"active-1","text":"A more durable insight."},
        {"from":"active-2","text":"* KEEP  this!"}
    ]}"#;
    assert_eq!(
        finish_ember_dream_consolidation(&home, &current, output, "fixture", &records).unwrap(),
        1
    );
    let active = read_active_dreams_at(&home.join("dreams/dreams.md")).unwrap();
    assert_ne!(active[0].id, old_id);
    assert_eq!(active[0].id, dream_id("A more durable insight."));
    assert_eq!(
        active[1].id, keep_id,
        "formatting changes preserve identity"
    );
    assert_eq!(fs::read(&ledger).unwrap(), usage_before);
    let usage = dream_usage::summaries(&home).unwrap();
    assert_eq!(usage[&old_id].pushed, 1);
    let items = build_ember_dream_consolidation_items(
        &active
            .iter()
            .map(|item| item.text.clone())
            .collect::<Vec<_>>(),
        &[],
        Some(&usage),
    );
    assert_eq!(items[0].usage.as_ref().unwrap().pushed, 0);
    assert_eq!(items[1].usage.as_ref().unwrap().pushed, 1);
    let archive = fs::read_to_string(home.join("dreams/old_dreams.md")).unwrap();
    assert_eq!(
        archive
            .lines()
            .filter(|line| line.starts_with("- "))
            .collect::<Vec<_>>(),
        ["- Original insight."]
    );
}

#[test]
fn ember_dream_duplicates_keep_the_first_text_before_applying_capacity() {
    let current = vec!["Keep this.".into(), "Merge this.".into()];
    let rows = parse_ember_dream_cli_output(
        r#"{"dreams":[
        {"from":"active-1","text":"Keep this."},
        {"from":"active-2","text":"KEEP this!"},
        {"from":null,"text":"Fresh insight."},
        {"from":null,"text":" fresh insight! "},
        {"from":null,"text":"Another insight."}
    ]}"#,
    )
    .unwrap();
    let result = apply_ember_dream_final_list(&current, rows, 3).unwrap();
    assert_eq!(
        result.active,
        ["Keep this.", "Fresh insight.", "Another insight."]
    );
    assert_eq!(result.archived, ["Merge this."]);
}

#[test]
fn ember_dream_capacity_trims_the_model_tail_and_archives_each_text_once() {
    assert_eq!(EMBER_DREAM_MAX_ACTIVE_ENTRIES, 10);
    let fixture = DreamFixture::new();
    let current = (0..15)
        .map(|index| format!("Observation {index}."))
        .collect::<Vec<_>>();
    let (home, records) = pending_consolidation(&fixture, &current, "Overflow proposal.");
    let mut rows = current
        .iter()
        .enumerate()
        .rev()
        .map(
            |(index, text)| serde_json::json!({"from": ember_active_dream_id(index), "text": text}),
        )
        .collect::<Vec<_>>();
    rows.push(serde_json::json!({"from": null, "text": "Overflow proposal."}));
    let output = serde_json::json!({"dreams": rows}).to_string();
    assert_eq!(
        finish_ember_dream_consolidation(&home, &current, &output, "fixture", &records).unwrap(),
        6
    );
    let active = read_active_dream_entries_at(&home.join("dreams/dreams.md")).unwrap();
    assert_eq!(
        active,
        current.iter().rev().take(10).cloned().collect::<Vec<_>>()
    );
    let archive = fs::read_to_string(home.join("dreams/old_dreams.md")).unwrap();
    let expected = current[..5]
        .iter()
        .map(|text| format!("- {text}"))
        .chain(std::iter::once("- Overflow proposal.".into()))
        .collect::<Vec<_>>();
    assert_eq!(
        archive
            .lines()
            .filter(|line| line.starts_with("- "))
            .collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn ember_dream_cli_output_ignores_other_json_events() {
    let output = r#"{"type":"status","message":"running"}
{"dreams":[{"from":"active-1","text":"Keep this."},{"from":null,"text":"New insight."}]}
done"#;
    let dreams = parse_ember_dream_cli_output(output).unwrap();
    assert_eq!(dreams.len(), 2);
    assert_eq!(dreams[0].from.as_deref(), Some("active-1"));
    assert_eq!(dreams[0].text, "Keep this.");
    assert_eq!(dreams[1].from, None);
    assert_eq!(dreams[1].text, "New insight.");
}

#[test]
fn ember_dream_null_from_preserves_unreferenced_active_identity() {
    let fixture = DreamFixture::new();
    let current = vec![
        "Keep this.".into(),
        "Keep that.".into(),
        "Retire this.".into(),
    ];
    let (home, records) = pending_consolidation(&fixture, &current, "New insight.");
    let output = r#"{"dreams":[
        {"from":null,"text":"Keep this."},
        {"from":null,"text":"* KEEP  that!"},
        {"from":null,"text":"New insight."}
    ]}"#;
    assert_eq!(
        finish_ember_dream_consolidation(&home, &current, output, "fixture", &records).unwrap(),
        1
    );
    let active = read_active_dreams_at(&home.join("dreams/dreams.md")).unwrap();
    assert_eq!(active.len(), 3);
    assert_eq!(active[0].id, dream_id(&current[0]));
    assert_eq!(active[1].id, dream_id(&current[1]));
    assert_eq!(active[2].text, "New insight.");
    let archive = fs::read_to_string(home.join("dreams/old_dreams.md")).unwrap();
    assert_eq!(
        archive
            .lines()
            .filter(|line| line.starts_with("- "))
            .collect::<Vec<_>>(),
        ["- Retire this."]
    );
}

#[test]
fn ember_dream_invalid_final_lists_preserve_outputs_and_unconsumed_inbox() {
    let mut cases = vec![
        "{bad json".to_string(),
        r#"{"decisions":[]}"#.into(),
        r#"{"dreams":[{"from":null,"text":"Incomplete."}"#.into(),
        r#"{"dreams":[{"from":"candidate-1","text":"Insight."}]}"#.into(),
        r#"{"dreams":[{"from":"active-2","text":"Insight."}]}"#.into(),
        r#"{"dreams":[{"from":"active-01","text":"Insight."}]}"#.into(),
        r#"{"dreams":[{"from":" active-1 ","text":"Insight."}]}"#.into(),
        r#"{"dreams":[{"from":4,"text":"Insight."}]}"#.into(),
        r#"{"dreams":[{"text":"Insight."}]}"#.into(),
        r#"{"dreams":[{"from":null}]}"#.into(),
        r#"{"dreams":[{"from":null,"text":" - \n "}]}"#.into(),
        r#"{"dreams":[{"from":null,"text":"__KOTA_DREAM_NONE__"}]}"#.into(),
        r#"{"dreams":[{"from":"active-1","text":"Keep."},{"from":"active-1","text":"KEEP!"}]}"#
            .into(),
        serde_json::json!({"dreams":[{"from":null,"text":"界".repeat(241)}]}).to_string(),
    ];
    // Neither deduplication nor truncation may hide an invalid trailing row.
    for bad in [
        serde_json::json!({"from":"active-1","text":"Keep."}),
        serde_json::json!({"from":"unknown","text":"Keep."}),
        serde_json::json!({"from":null,"text":"x".repeat(241)}),
    ] {
        let mut rows = vec![serde_json::json!({"from":"active-1","text":"Keep."})];
        rows.extend(
            (0..10).map(|index| serde_json::json!({"from":null,"text":format!("New {index}.")})),
        );
        rows.push(bad);
        cases.push(serde_json::json!({"dreams":rows}).to_string());
    }
    for output in cases {
        let fixture = DreamFixture::new();
        let current = vec!["Keep.".into()];
        let (home, records) = pending_consolidation(&fixture, &current, "Candidate insight.");
        let active_path = home.join("dreams/dreams.md");
        let old_path = home.join("dreams/old_dreams.md");
        fs::write(
            &old_path,
            "# Old Dreams\n\n## 2026-01-01T00:00:00Z\n- Previous archive.\n",
        )
        .unwrap();
        let inbox_path = home
            .join("dreams/inbox")
            .join(format!("{}.json", records[0].event_id));
        let before = [&active_path, &old_path, &inbox_path].map(|path| fs::read(path).unwrap());
        assert!(
            finish_ember_dream_consolidation(&home, &current, &output, "fixture", &records)
                .is_err(),
            "{output}"
        );
        assert_eq!(
            [&active_path, &old_path, &inbox_path].map(|path| fs::read(path).unwrap()),
            before,
            "{output}"
        );
        assert!(!home.join("dreams/entries").exists(), "{output}");
    }
}

#[test]
fn ember_dream_final_text_is_cleaned_and_measured_in_unicode_characters() {
    let text = "界".repeat(240);
    let output = serde_json::json!({"dreams":[
        {"from":null,"text":format!("  - \"{text}\"\n")},
        {"from":null,"text":"First\n second\tthird."}
    ]})
    .to_string();
    let result =
        apply_ember_dream_final_list(&[], parse_ember_dream_cli_output(&output).unwrap(), 10)
            .unwrap();
    assert_eq!(result.active, [text, "First second third.".into()]);
    assert!(result.archived.is_empty());
}

#[test]
fn ember_dream_empty_final_list_archives_active_and_consumes_the_batch() {
    let fixture = DreamFixture::new();
    let current = vec!["Outdated observation.".into()];
    let (home, records) = pending_consolidation(&fixture, &current, "Rejected candidate.");
    assert_eq!(
        finish_ember_dream_consolidation(&home, &current, r#"{"dreams":[]}"#, "fixture", &records)
            .unwrap(),
        1
    );
    assert!(read_active_dream_entries_at(&home.join("dreams/dreams.md"))
        .unwrap()
        .is_empty());
    assert!(crate::dream_entry::read_inbox(&home, false)
        .unwrap()
        .is_empty());
    let archive = fs::read_to_string(home.join("dreams/old_dreams.md")).unwrap();
    assert!(archive.contains("- Outdated observation."));
    assert!(!archive.contains("Rejected candidate"));
}

#[test]
fn ember_dream_prompt_exposes_active_count_and_capacity_without_replacing_item_text() {
    let current = vec![
        "Current observation.".into(),
        "Literal {{capacity}}.".into(),
    ];
    let projects = vec![vec![EmberDreamEntryRecord {
        event_id: "event-fixture".into(),
        ts: "2026-01-02T00:00:00Z".into(),
        agent_id: "agent-fixture".into(),
        agent_display_name: None,
        text: "- Candidate observation.".into(),
    }]];
    let items = build_ember_dream_consolidation_items(&current, &projects, None);
    let prompt =
        render_ember_dream_consolidation_prompt(&items, EMBER_DREAM_CONSOLIDATE_PROMPT_TEMPLATE)
            .unwrap();
    assert!(prompt.contains("Current Dreams (2 of at most 10)"));
    assert!(prompt.contains("Return the new list of Dreams, at most 10 entries."));
    assert!(prompt.contains("\"text\": \"Literal {{capacity}}.\""));
    assert!(prompt.contains("\"id\": \"candidate-1\""));
    assert!(!prompt.contains("{{items_json}}"));
    assert!(!prompt.contains("{{active_count}}"));
}

#[test]
fn ember_dream_collection_reads_all_inbox_projects_and_never_chat_envelopes() {
    let f = DreamFixture::new();
    let home = f.0.join("home");
    let a = f.0.join("project-a"); let b = f.0.join("project-b");
    let first = inbox_entry(&home, &a, "agent-a", Some("First insight."));
    inbox_entry(&home, &a, "agent-b", None);
    let other = inbox_entry(&home, &b, "agent-c", Some("Other project insight."));
    fs::create_dir_all(a.join("project-memory/chathistory/events")).unwrap();
    fs::write(a.join("project-memory/chathistory/events/2026-01-02.jsonl"),
        "{\"id\":\"chat-only\",\"role\":\"assistant\",\"kind\":\"message\",\"text\":\"<KOTA_DREAM_ENTRY>done</KOTA_DREAM_ENTRY>\"}\n").unwrap();
    let groups = collect_unprocessed_dream_entries(&home, &HashSet::new()).unwrap();
    assert_eq!(groups.len(), 2);
    assert_eq!(groups.iter().map(Vec::len).collect::<Vec<_>>(), vec![2, 1]);
    let entries = groups.iter().flatten().collect::<Vec<_>>();
    assert!(entries.iter().any(|entry| entry.event_id == first.id && entry.text == "- First insight."));
    assert!(entries.iter().any(|entry| entry.event_id == other.id && entry.text == "- Other project insight."));
    assert!(entries.iter().any(|entry| entry.text == "__KOTA_DREAM_NONE__"));
    assert!(entries.iter().all(|entry| entry.event_id != "chat-only"));
    // Collection alone is not consumption; a failed model call can retry this batch.
    assert_eq!(crate::dream_entry::read_inbox(&home, false).unwrap().len(), 3);
}

#[test]
fn ember_dream_already_processed_resubmission_is_removed_but_still_acknowledged() {
    let f = DreamFixture::new();
    let home = f.0.join("home");
    let entry = inbox_entry(&home, &f.0.join("project"), "agent-fixture", None);
    let participant = crate::dream_entry::Participant {
        agent_id: entry.agent_id.clone(), project_root: entry.project_root.clone(), delivered_at: entry.ts.clone(),
    };
    assert!(collect_unprocessed_dream_entries(&home, &HashSet::from([entry.id])).unwrap().is_empty());
    assert!(crate::dream_entry::read_inbox(&home, false).unwrap().is_empty());
    assert!(crate::dream_entry::has_submitted(&home, &participant, &[]));
    assert!(!home.join("dreams/entries").exists());
}

#[test]
fn ember_dream_consumption_waits_for_both_outputs_and_the_processed_ledger() {
    for blocked in ["old_dreams.md", "dreams.md", "entries"] {
        let f = DreamFixture::new();
        let home = f.0.join("home");
        let entry = inbox_entry(&home, &f.0.join("project"), "agent-fixture", Some("Keep this insight."));
        let records = collect_unprocessed_dream_entries(&home, &HashSet::new()).unwrap().pop().unwrap();
        let result = EmberDreamApplyResult { active: vec!["Keep this insight.".into()], archived: vec!["Archived insight.".into()] };
        let obstacle = home.join("dreams").join(blocked);
        if blocked == "entries" { fs::write(&obstacle, "fixture obstacle").unwrap(); }
        else { fs::create_dir(&obstacle).unwrap(); }
        assert!(persist_ember_dream_result(&home, &result, "fixture", &records).is_err());
        assert_eq!(crate::dream_entry::read_inbox(&home, false).unwrap(), vec![entry.clone()]);
        if blocked == "entries" {
            // Documented retry window: the output landed, but nothing is acknowledged.
            assert!(fs::read_to_string(home.join("dreams/dreams.md")).unwrap().contains("- Keep this insight."));
            fs::remove_file(&obstacle).unwrap();
        } else {
            assert!(!home.join("dreams/entries").exists());
            fs::remove_dir(&obstacle).unwrap();
        }
        persist_ember_dream_result(&home, &result, "fixture", &records).unwrap();
        assert!(crate::dream_entry::read_inbox(&home, false).unwrap().is_empty());
        let ledger = fs::read_dir(home.join("dreams/entries")).unwrap().next().unwrap().unwrap().path();
        let saved = fs::read_to_string(ledger).unwrap();
        let line: EmberDreamEntryRecord = serde_json::from_str(saved.trim()).unwrap();
        assert_eq!(line.event_id, entry.id);
        assert!(crate::dream_entry::has_submitted(&home, &crate::dream_entry::Participant {
            agent_id: entry.agent_id, project_root: entry.project_root, delivered_at: entry.ts,
        }, &[]));
    }
}

#[test]
fn ember_dream_none_is_journaled_and_consumed_without_model_or_output_changes() {
    let f = DreamFixture::new();
    let home = f.0.join("home");
    inbox_entry(&home, &f.0.join("project"), "agent-fixture", None);
    let records = collect_unprocessed_dream_entries(&home, &HashSet::new()).unwrap().pop().unwrap();
    let items = build_ember_dream_consolidation_items(&[], &[records.clone()], None);
    assert!(items.is_empty());
    append_dream_entry_records_at(&home, &records).unwrap();
    assert!(crate::dream_entry::read_inbox(&home, false).unwrap().is_empty());
    assert!(!home.join("dreams/dreams.md").exists());
}

#[test]
fn ember_dream_acknowledgements_reach_room_but_not_summary_counts() {
    let event = |text: &str| -> ChathistoryEvent {
        serde_json::from_value(serde_json::json!({
            "id": "dream-fixture", "ts": "2026-01-02T01:02:03Z", "role": "assistant",
            "agent_id": "agent-fixture", "shell": "codex", "kind": "message",
            "display": true, "agent_visible": true, "text": text,
            "source": {"session_id": "session-fixture"}
        })).unwrap()
    };
    for body in ["done", "unexpected words\n- literal text", ""] {
        let text = format!(" \n<KOTA_DREAM_ENTRY>\n{body}\n</KOTA_DREAM_ENTRY>\n");
        let row = event(&text);
        assert!(is_ember_dream_entry_event(&row));
        assert!(!is_summary_count_event(&row));
        let messages = chathistory_events_to_room_messages(vec![row], None);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, text);
    }
    let quoted = event("Discuss <KOTA_DREAM_ENTRY>done</KOTA_DREAM_ENTRY> as a format.");
    assert!(!is_ember_dream_entry_event(&quoted));
    assert!(is_summary_count_event(&quoted));
    assert_eq!(chathistory_events_to_room_messages(vec![quoted], None).len(), 1);
    let legacy = event("Dream entry\n- An older portrait.");
    assert!(!is_summary_count_event(&legacy));
    assert!(chathistory_events_to_room_messages(vec![legacy], None).is_empty());
}

#[test]
fn ember_dream_id_uses_the_existing_normalization_and_sha256_prefix() {
    assert_eq!(dream_id("Review first."), "d-f7e35e12");
    assert_eq!(dream_id("  REVIEW\t first! \n"), "d-f7e35e12");
    assert_eq!(dream_id("先看证据， 再做决定。"), "d-d9a0a311");
    assert_ne!(dream_id("Review first."), dream_id("Review after editing."));
}

#[test]
fn ember_dream_reader_exposes_identity_text_and_list_position_without_writing() {
    let fixture = DreamFixture::new();
    let content = "# Dreams\n\n## Active Dreams\n- Review first.\n- Ask for evidence.\n\n## Last Dream\n- 2026-01-02, provider: fixture\n";
    fs::write(fixture.path(), content).unwrap();

    assert_eq!(
        read_active_dreams_at(&fixture.path()).unwrap(),
        vec![
            ActiveDream {
                id: "d-f7e35e12".into(),
                text: "Review first.".into(),
                position: 0,
            },
            ActiveDream {
                id: dream_id("Ask for evidence."),
                text: "Ask for evidence.".into(),
                position: 1,
            },
        ]
    );
    assert_eq!(fs::read_to_string(fixture.path()).unwrap(), content);
}

#[test]
fn ember_dream_reader_supports_missing_empty_and_legacy_files() {
    let fixture = DreamFixture::new();
    assert!(read_active_dreams_at(&fixture.path()).unwrap().is_empty());
    fs::write(
        fixture.path(),
        "## Active Dreams\n- _No dream entries yet._\n",
    )
    .unwrap();
    assert!(read_active_dreams_at(&fixture.path()).unwrap().is_empty());
    fs::write(fixture.path(), "# Dreams\n- Review first.\n").unwrap();
    assert_eq!(
        read_active_dreams_at(&fixture.path()).unwrap()[0].id,
        "d-f7e35e12"
    );
}

#[test]
fn ember_dream_empty_active_section_never_reads_last_dream_metadata() {
    let fixture = DreamFixture::new();
    for active in ["", "- _No dream entries yet._\n", "- __KOTA_DREAM_NONE__\n"] {
        let content = format!(
            "# Dreams\n\n## Active Dreams\n{active}\n## Last Dream\n- 2026-01-02, provider: fixture, processed entries: 4\n"
        );
        fs::write(fixture.path(), &content).unwrap();
        assert!(read_active_dream_entries_at(&fixture.path())
            .unwrap()
            .is_empty());
        assert!(read_active_dreams_at(&fixture.path()).unwrap().is_empty());
        assert_eq!(fs::read_to_string(fixture.path()).unwrap(), content);
    }
}

#[test]
fn ember_dream_reader_leaves_capacity_changes_to_consolidation() {
    let fixture = DreamFixture::new();
    let content = format!(
        "## Active Dreams\n{}",
        (0..15)
            .map(|n| format!("- Observation {n}.\n"))
            .collect::<String>()
    );
    fs::write(fixture.path(), &content).unwrap();
    let active = read_active_dreams_at(&fixture.path()).unwrap();
    assert_eq!(active.len(), 15);
    assert_eq!(active.last().unwrap().position, 14);
    assert_eq!(fs::read_to_string(fixture.path()).unwrap(), content);
}

#[test]
fn ember_dream_usage_follows_the_active_text_identity_and_stays_off_candidates() {
    use crate::soul::dream_usage;

    let fixture = DreamFixture::new();
    let current = vec!["Review first.".into(), "A revised insight.".into()];
    let id = dream_id("  REVIEW first! ");
    for (agent, message, ts) in [
        ("fixture-agent-a", "fixture-1", "2026-01-01T00:00:00Z"),
        ("fixture-agent-a", "fixture-2", "2026-01-02T00:00:00Z"),
        ("fixture-agent-b", "fixture-3", "2026-01-02T12:00:00Z"),
    ] {
        dream_usage::append_pushes(&fixture.0, agent, message, ts, &[(&id, 0.95)]).unwrap();
    }
    dream_usage::append_pushes(
        &fixture.0,
        "fixture-agent-a",
        "fixture-old",
        "2026-01-01T00:00:00Z",
        &[(&dream_id("An obsolete insight."), 0.95)],
    )
    .unwrap();
    dream_usage::append_clear(&fixture.0, "fixture-agent-a", &id, "2026-01-03T00:00:00Z").unwrap();
    let usage = dream_usage::summaries(&fixture.0).unwrap();
    let projects = vec![vec![EmberDreamEntryRecord {
        event_id: "fixture-event".into(),
        ts: "2026-01-03T00:00:00Z".into(),
        agent_id: "fixture-agent".into(),
        agent_display_name: None,
        text: "- Review first.".into(),
    }]];

    let items = build_ember_dream_consolidation_items(&current, &projects, Some(&usage));
    let value = serde_json::to_value(&items).unwrap();
    assert_eq!(value[0]["id"], "active-1");
    assert_eq!(
        value[0]["usage"],
        serde_json::json!({
            "pushed": 3, "agents": 2, "last": "2026-01-02"
        })
    );
    assert_eq!(
        value[1]["usage"],
        serde_json::json!({
            "pushed": 0, "agents": 0, "last": null
        })
    );
    assert_eq!(value[2]["kind"], "candidate");
    assert!(value[2].get("usage").is_none());
    assert!(!serde_json::to_string(&items)
        .unwrap()
        .contains("fixture-agent"));
}

#[test]
fn ember_dream_missing_usage_ledger_reports_zero_usage_for_active_items() {
    let fixture = DreamFixture::new();
    let usage = crate::soul::dream_usage::summaries(&fixture.0).unwrap();
    let items =
        build_ember_dream_consolidation_items(&["Review first.".into()], &[], Some(&usage));
    let value = serde_json::to_value(&items).unwrap();
    assert_eq!(
        value[0]["usage"],
        serde_json::json!({"pushed": 0, "agents": 0, "last": null})
    );
    assert!(!fixture.0.join("dreams").exists());
}

#[test]
fn ember_dream_unavailable_usage_is_omitted_rather_than_reported_as_zero() {
    let items = build_ember_dream_consolidation_items(&["Review first.".into()], &[], None);
    let value = serde_json::to_value(&items).unwrap();
    assert_eq!(value[0]["kind"], "active");
    assert!(value[0].get("usage").is_none());
}

#[test]
fn ember_dream_export_isolates_failures_and_leaves_archives_out_of_episodes() {
    let fixture = DreamFixture::new();
    let kota_home = fixture.0.join("account");
    fs::create_dir_all(kota_home.join("dreams")).unwrap();
    let insight = "Keep a record of decisions.";
    fs::write(
        kota_home.join("dreams/old_dreams.md"),
        format!("# Old Dreams\n\n## 2026-01-02T00:00:00Z\n- {insight}\n"),
    )
    .unwrap();
    let invalid_project = fixture.0.join("not-a-project-directory");
    fs::write(&invalid_project, "fixture").unwrap();
    let project = fixture.0.join("project");
    let agent_dir = |id: &str| project.join(".agent-workspaces").join(id);
    for (id, status) in [
        ("agent-fixture-good", "active"),
        ("agent-fixture-broken", "active"),
        ("agent-fixture-archived", "archived"),
    ] {
        let cwd = agent_dir(id);
        fs::create_dir_all(&cwd).unwrap();
        fs::write(
            cwd.join("agent.yaml"),
            format!("agent-id: {id}\nstatus: {status}\n"),
        )
        .unwrap();
    }
    let blocked = agent_dir("agent-fixture-broken").join("under-table");
    fs::write(&blocked, "not-a-directory").unwrap();
    // Use the real exporter with fixture identity/time instead of the account
    // configuration read by its production entry point.
    let export = |root: &Path, home: &Path| {
        crate::soul::export::export_with(root, home, "Fixture User", "2026-01-03T00:00:00Z")
    };
    let projects = [invalid_project, project.clone()];
    let mut visited = Vec::new();
    let errors = sync_and_export_episodes(
        &projects,
        &kota_home,
        |_| {},
        |root, home| {
            visited.push(root.to_path_buf());
            export(root, home)
        },
    );
    assert_eq!(visited, projects);
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(errors[0].contains("not-a-project-directory"));
    assert!(errors[1].contains("agent-fixture-broken"));
    let episodes = agent_dir("agent-fixture-good").join("under-table/episodes/episodes.jsonl");
    assert!(!episodes.exists());
    assert!(!agent_dir("agent-fixture-archived")
        .join("under-table")
        .exists());

    let errors = sync_and_export_episodes(&[project], &kota_home, |_| {}, export);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(!episodes.exists());
}

#[test]
fn ember_episode_export_syncs_each_project_right_before_moving_it() {
    let projects = [
        PathBuf::from("/tmp/kota-dream-a"),
        PathBuf::from("/tmp/kota-dream-b"),
    ];
    let calls = std::cell::RefCell::new(Vec::new());
    let errors = sync_and_export_episodes(
        &projects,
        Path::new("/tmp/kota-dream-home"),
        |root| calls.borrow_mut().push(format!("sync {}", root.display())),
        |root, _| {
            calls
                .borrow_mut()
                .push(format!("export {}", root.display()));
            Ok(crate::soul::export::ExportReport::default())
        },
    );
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        calls.into_inner(),
        [
            "sync /tmp/kota-dream-a",
            "export /tmp/kota-dream-a",
            "sync /tmp/kota-dream-b",
            "export /tmp/kota-dream-b",
        ]
    );
}
