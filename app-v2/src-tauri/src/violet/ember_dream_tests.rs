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
fn ember_dream_capacity_is_ten_and_overflow_still_archives_from_the_front() {
    let current = (0..15)
        .map(|n| format!("Observation {n}."))
        .collect::<Vec<_>>();
    let decisions = (0..current.len())
        .map(|index| (ember_active_dream_id(index), EmberDreamDecisionAction::Keep))
        .collect();
    let result =
        apply_ember_dream_decisions(&current, &[], &decisions, EMBER_DREAM_MAX_ACTIVE_ENTRIES)
            .unwrap();
    assert_eq!(result.active, current[5..]);
    assert_eq!(result.archived, current[..5]);
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

    let (items, _) = build_ember_dream_consolidation_items(&current, &projects, Some(&usage));
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
    let (items, _) =
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
    let (items, _) = build_ember_dream_consolidation_items(&["Review first.".into()], &[], None);
    let value = serde_json::to_value(&items).unwrap();
    assert_eq!(value[0]["kind"], "active");
    assert!(value[0].get("usage").is_none());
}

#[test]
fn ember_dream_export_isolates_failures_and_imports_archives_without_room_messages() {
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
    let before = fs::read_to_string(&episodes).unwrap();
    let records = before
        .lines()
        .map(|line| serde_json::from_str::<JsonValue>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["id"], dream_id(insight));
    assert_eq!(records[0]["kind"], "old_dream");
    assert_eq!(records[0]["project"], JsonValue::Null);
    assert_eq!(records[0]["w0"], 0.5);
    assert!(!agent_dir("agent-fixture-archived")
        .join("under-table")
        .exists());

    let errors = sync_and_export_episodes(&[project], &kota_home, |_| {}, export);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(fs::read_to_string(&episodes).unwrap(), before);
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
