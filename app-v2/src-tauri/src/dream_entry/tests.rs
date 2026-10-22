use super::*;
use crate::local_rpc::{LocalScope, OnlineState, Snapshot};

const T1: &str = "2026-01-02T01:02:03.000Z";
const T2: &str = "2026-01-02T01:03:04.567Z";

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("dream-inbox-fixture-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("project/.agent-workspaces/agent-fixture")).unwrap();
        Self(root.canonicalize().unwrap())
    }
    fn home(&self) -> PathBuf {
        self.0.join("home")
    }
    fn root(&self) -> PathBuf {
        self.0.join("project")
    }
    fn env(&self) -> Env {
        Env {
            agent_cwd: self.root().join(".agent-workspaces/agent-fixture"),
            kota_home: self.home(),
            agent_id: "agent-fixture".into(),
            project: Some("project-fixture".into()),
            jev_ready: Some(false),
        }
    }
    fn entry(&self, ts: &str) -> Entry {
        Entry::new(
            &self.env(),
            &self.root(),
            Some("Fixture".into()),
            &["Ask for evidence.".into()],
            false,
            ts,
        )
        .unwrap()
    }
    fn participant(&self, ts: &str) -> Participant {
        Participant {
            agent_id: "agent-fixture".into(),
            project_root: self.root(),
            delivered_at: ts.into(),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn dream_entry_validates_unicode_items_and_preserves_literal_arguments() {
    let parse =
        |args: &[&str]| parse_items(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    assert_eq!(
        parse(&["--", "  ask\n for\tevidence ", "--none"]).unwrap(),
        (vec!["ask for evidence".into(), "--none".into()], false)
    );
    assert_eq!(parse(&["--none"]).unwrap(), (vec![], true));
    assert_eq!(
        parse(&["--none", "one"]).unwrap_err(),
        "pass insights or --none, not both."
    );
    assert_eq!(parse(&["a", "b", "c"]).unwrap_err(), "at most 2 insights.");
    assert_eq!(parse(&["one", " \n\t"]).unwrap_err(), "insight 2 is empty.");
    assert_eq!(
        parse(&["--"]).unwrap_err(),
        "nothing to record. Pass insights or --none."
    );
    assert!(parse(&[&"字".repeat(240)]).is_ok());
    assert_eq!(
        parse(&[&"字".repeat(301)]).unwrap_err(),
        "insight 1 is too long: 301 of 240 characters."
    );
    assert_eq!(success_reply(0), "Dream entry recorded (none).");
    assert_eq!(success_reply(1), "Dream entry recorded (1 item).");
    assert_eq!(success_reply(2), "Dream entry recorded (2 items).");
}

#[test]
fn dream_entry_id_is_normalized_daily_and_scoped_to_incarnation() {
    let f = Fixture::new();
    let first = f.entry(T1);
    let spaced = Entry::new(
        &f.env(),
        &f.root(),
        None,
        &[" Ask  for\nevidence. ".into()],
        false,
        T2,
    )
    .unwrap();
    assert_eq!(first.id, spaced.id);
    assert!(first.id.starts_with("de-2026-01-02-"));
    assert_eq!(first.id.len(), 22);
    assert_ne!(first.id, f.entry("2026-01-03T00:00:00Z").id);
    assert_eq!(first.id, f.entry("2026-01-01T17:02:03-08:00").id);
    let mut env = f.env();
    env.agent_id = "agent-other".into();
    assert_ne!(
        first.id,
        Entry::new(&env, &f.root(), None, &first.items, false, T1)
            .unwrap()
            .id
    );
    env = f.env();
    env.project = Some("another-fixture".into());
    assert_ne!(
        first.id,
        Entry::new(&env, &f.root(), None, &first.items, false, T1)
            .unwrap()
            .id
    );
}

#[test]
fn dream_entry_resubmission_refreshes_ts_and_consumed_ack_survives_removal() {
    let f = Fixture::new();
    submit(&f.home(), &f.entry(T1)).unwrap();
    let participant = f.participant(T2);
    assert!(!has_submitted(
        &f.home(),
        &participant,
        &read_inbox(&f.home(), false).unwrap()
    ));
    let second = f.entry(T2);
    submit(&f.home(), &second).unwrap();
    let entries = read_inbox(&f.home(), false).unwrap();
    assert_eq!(entries, vec![second.clone()]);
    assert!(has_submitted(&f.home(), &participant, &entries));
    remove_acknowledged(&f.home(), &HashSet::from([second.id.clone()])).unwrap();
    assert!(read_inbox(&f.home(), false).unwrap().is_empty());
    assert!(has_submitted(&f.home(), &participant, &[]));
    assert!(!has_submitted(
        &f.home(),
        &f.participant("2026-01-02T02:00:00Z"),
        &[]
    ));
    let mut other = participant.clone();
    other.agent_id = "agent-other".into();
    assert!(!has_submitted(&f.home(), &other, &[]));
    other = participant;
    other.project_root = f.0.join("another-project");
    assert!(!has_submitted(&f.home(), &other, &[]));
}

#[test]
fn dream_entry_none_counts_as_submitted_and_rejects_invalid_identity_or_payload() {
    let f = Fixture::new();
    let entry = Entry::new(&f.env(), &f.root(), None, &[], true, T1).unwrap();
    submit(&f.home(), &entry).unwrap();
    assert_eq!(entry.text(), "__KOTA_DREAM_NONE__");
    assert!(has_submitted(
        &f.home(),
        &f.participant(T1),
        &read_inbox(&f.home(), false).unwrap()
    ));
    let mut bad = entry.clone();
    bad.items.push("unexpected".into());
    assert!(submit(&f.home(), &bad).is_err());
    bad = entry;
    bad.id = "../escape".into();
    assert!(submit(&f.home(), &bad).is_err());
}

#[test]
fn dream_entry_ingestion_quarantines_bad_files_but_status_never_moves_them() {
    let f = Fixture::new();
    let entry = f.entry(T1);
    submit(&f.home(), &entry).unwrap();
    let directory = inbox(&f.home());
    fs::write(directory.join("broken.json"), "{").unwrap();
    let mut mismatch = entry.clone();
    mismatch.items[0] = "Changed content".into();
    fs::write(
        directory.join("mismatch.json"),
        serde_json::to_vec(&mismatch).unwrap(),
    )
    .unwrap();
    fs::write(directory.join(".writing.tmp"), "{").unwrap();
    assert_eq!(read_inbox(&f.home(), false).unwrap().len(), 1);
    assert!(directory.join("broken.json").exists());
    assert!(!directory.join("rejected").exists());
    assert_eq!(read_inbox(&f.home(), true).unwrap(), vec![entry]);
    assert!(!directory.join("broken.json").exists());
    assert_eq!(fs::read_dir(directory.join("rejected")).unwrap().count(), 2);
    assert!(directory.join(".writing.tmp").exists());
    let old = directory.join("rejected").join(format!(
        "{}-old.json",
        (Utc::now() - chrono::Duration::days(31)).timestamp_millis()
    ));
    fs::write(&old, "{").unwrap();
    read_inbox(&f.home(), true).unwrap();
    assert!(!old.exists());
    assert_eq!(fs::read_dir(directory.join("rejected")).unwrap().count(), 2);
}

#[test]
fn dream_entry_parallel_writers_leave_one_complete_submission() {
    let f = Fixture::new();
    let handles = (0..8)
        .map(|n| {
            let home = f.home();
            let entry = f.entry(&format!("2026-01-02T01:02:{n:02}.000Z"));
            std::thread::spawn(move || submit(&home, &entry).unwrap())
        })
        .collect::<Vec<_>>();
    for handle in handles {
        handle.join().unwrap();
    }
    let entries = read_inbox(&f.home(), true).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].text(), "- Ask for evidence.");
    assert_eq!(fs::read_dir(inbox(&f.home())).unwrap().count(), 1);
}

#[test]
fn dream_entry_local_work_status_uses_rpc_and_respects_project_scope() {
    let f = Fixture::new();
    let mut snapshot = Snapshot {
        scope: LocalScope {
            project_root: f.root(),
            agent_id: "agent-fixture".into(),
            session_id: "local-fixture".into(),
            epoch: 1,
        },
        state: OnlineState::Ready,
        queued: 0,
        compacting: false,
        child_pid: None,
        model: None,
        thinking_level: None,
        error: None,
    };
    assert!(!local_is_working(&snapshot, &f.root()));
    snapshot.queued = 1;
    assert!(local_is_working(&snapshot, &f.root()));
    snapshot.queued = 0;
    snapshot.compacting = true;
    assert!(local_is_working(&snapshot, &f.root()));
    snapshot.compacting = false;
    for state in [OnlineState::Busy, OnlineState::Starting] {
        snapshot.state = state;
        assert!(local_is_working(&snapshot, &f.root()));
    }
    snapshot.queued = 1;
    for state in [OnlineState::Offline, OnlineState::Error] {
        snapshot.state = state;
        assert!(!local_is_working(&snapshot, &f.root()));
    }
    snapshot.state = OnlineState::Busy;
    assert!(!local_is_working(&snapshot, &f.0.join("other-project")));
}

#[test]
fn dream_entry_external_binaries_match_startup_shims_and_build_lists() {
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
    let packaged = config["bundle"]["externalBin"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| {
            Path::new(name.as_str().unwrap())
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string()
        })
        .collect::<HashSet<_>>();
    let installed = crate::CLI_SHIMS
        .iter()
        .map(|(name, _)| name.to_string())
        .collect::<HashSet<_>>();
    assert_eq!(packaged, installed);
    assert_eq!(installed.len(), crate::CLI_SHIMS.len());
    let package: serde_json::Value =
        serde_json::from_str(include_str!("../../../package.json")).unwrap();
    for name in packaged {
        for script in ["build:sidecars", "build:sidecars:dev"] {
            assert!(package["scripts"][script]
                .as_str()
                .unwrap()
                .contains(&format!("--bin {name}")));
        }
    }
}

#[test]
fn dream_cli_uses_working_directory_identity_rejects_mismatch_and_falls_back() {
    let f = Fixture::new();
    let agent_cwd = f.root().join(".agent-workspaces/agent-fixture");
    let worktree = agent_cwd.join("project-files");
    let plain_dir = f.0.join("outside");
    fs::create_dir_all(&worktree).unwrap();
    fs::create_dir_all(&plain_dir).unwrap();
    fs::write(
        f.root().join("workspace.json"),
        r#"{"projectId":"cedar-project"}"#,
    )
    .unwrap();
    fs::write(
        agent_cwd.join("agent.yaml"),
        "id: agent-fixture\ndisplay-name: Birch v. cedar\nstatus: active\n",
    )
    .unwrap();
    let args = vec![
        "--kota-home".into(),
        f.home().display().to_string(),
        "--".into(),
        "Verify working directory identity.".into(),
    ];

    // 1) Directory matches environment -> records entry under agent-fixture@cedar-project.
    let matching = HashMap::from([
        ("KOTA_AGENT_ID", "agent-fixture".to_string()),
        ("KOTA_PROJECT_ID", "cedar-project".to_string()),
        ("KOTA_AGENT_CWD", agent_cwd.display().to_string()),
    ]);
    let reply = run_cli_with(&args, Some(&worktree), |k| matching.get(k).cloned(), T1).unwrap();
    assert_eq!(reply, "Dream entry recorded (1 item).");
    let entries = read_inbox(&f.home(), false).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].agent_id, "agent-fixture");
    assert_eq!(entries[0].agent_display_name.as_deref(), Some("Birch"));
    assert_eq!(entries[0].project_id, "cedar-project");
    remove_acknowledged(&f.home(), &HashSet::from([entries[0].id.clone()])).unwrap();

    // 2) Directory vs environment mismatch -> rejected with exact copy, inbox stays empty.
    let mismatched = HashMap::from([
        ("KOTA_AGENT_ID", "agent-maple".to_string()),
        ("KOTA_PROJECT_ID", "maple-project".to_string()),
    ]);
    let err = run_cli_with(&args, Some(&worktree), |k| mismatched.get(k).cloned(), T1).unwrap_err();
    assert_eq!(
        err,
        "identity mismatch: environment says agent-maple@maple-project, working directory says agent-fixture@cedar-project"
    );
    assert!(read_inbox(&f.home(), false).unwrap().is_empty());

    // 3) No directory evidence -> falls back to environment variables.
    let fallback_env = HashMap::from([
        ("KOTA_AGENT_CWD", agent_cwd.display().to_string()),
        ("KOTA_AGENT_ID", "agent-maple".to_string()),
        ("KOTA_PROJECT_ID", "maple-project".to_string()),
        ("KOTA_PROJECT_ROOT", f.root().display().to_string()),
    ]);
    let reply =
        run_cli_with(&args, Some(&plain_dir), |k| fallback_env.get(k).cloned(), T2).unwrap();
    assert_eq!(reply, "Dream entry recorded (1 item).");
    let entries = read_inbox(&f.home(), false).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].agent_id, "agent-maple");
    assert_eq!(entries[0].project_id, "maple-project");
}
