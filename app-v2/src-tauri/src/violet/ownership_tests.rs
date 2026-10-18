use super::*;

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    kota: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("kota-owner-test-{}", uuid::Uuid::new_v4()));
        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();
        let kota = home.join("Kota");
        Self { root, home, kota }
    }
    fn agent(&self, project: &str, id: &str, shell: &str) -> ProjectAgent {
        let cwd = self.root.join(project).join(".agent-workspaces").join(id);
        fs::create_dir_all(&cwd).unwrap();
        fs::write(
            cwd.join("agent.yaml"),
            format!("id: {id}\nprovider: {shell}\n"),
        )
        .unwrap();
        ProjectAgent {
            agent_id: id.into(),
            shell: shell.into(),
            cwd,
            session_id: None,
        }
    }
    fn agy(&self, id: &str) -> NativeSource {
        let path = antigravity_full_transcript_path(&antigravity_app_dir(&self.home), id);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{}\n").unwrap();
        NativeSource {
            kind: "antigravity-jsonl".into(),
            session_id: id.into(),
            path,
            aux_path: None,
        }
    }
    fn index(&self, agent: &ProjectAgent, id: &str) {
        let path = antigravity_app_dir(&self.home).join("cache/last_conversations.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            serde_json::to_vec(&serde_json::json!({ path_string(&agent.cwd): id })).unwrap(),
        )
        .unwrap();
    }
    fn verify(&self, agent: &ProjectAgent, source: &NativeSource) -> anyhow::Result<()> {
        verify_source_in(&self.home, &self.kota, agent, source)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn global_sole_candidate_and_body_path_do_not_claim_a_session() {
    let f = Fixture::new();
    let agent = f.agent("a", "agent-a", "antigravity");
    let source = f.agy("session-1111");
    fs::write(
        &source.path,
        format!("{{\"text\":\"{}\"}}\n", agent.cwd.display()),
    )
    .unwrap();
    assert!(
        locate_antigravity_source_in(&antigravity_app_dir(&f.home), &agent)
            .unwrap()
            .is_none()
    );
    assert!(f.verify(&agent, &source).is_err());
    assert!(!Store::new(&f.kota)
        .path(&key(&f.home, &agent, &source.session_id))
        .unwrap()
        .exists());
}

#[test]
fn same_store_cannot_be_claimed_by_two_agents_or_projects() {
    let f = Fixture::new();
    let owner = f.agent("a", "agent-a", "antigravity");
    let other = f.agent("b", "agent-b", "antigravity");
    let source = f.agy("session-1111");
    f.index(&owner, &source.session_id);
    // The foreign agent cannot win merely by being the first to poll.
    assert!(f.verify(&other, &source).is_err());
    f.verify(&owner, &source).unwrap();
    f.index(&other, &source.session_id);
    assert!(f.verify(&other, &source).is_err());
    f.verify(&owner, &source).unwrap();
}

#[test]
fn native_receipt_survives_pruned_index_and_transcript() {
    let f = Fixture::new();
    let owner = f.agent("a", "agent-a", "antigravity");
    let source = f.agy("session-1111");
    f.index(&owner, &source.session_id);
    f.verify(&owner, &source).unwrap();
    fs::remove_file(antigravity_app_dir(&f.home).join("cache/last_conversations.json")).unwrap();
    f.verify(&owner, &source).unwrap();
    fs::remove_file(&source.path).unwrap();
    verify_id_in(&f.home, &f.kota, &owner, &source.session_id).unwrap();
}

#[test]
fn resume_echo_is_not_first_claim_but_created_log_is() {
    let f = Fixture::new();
    let owner = f.agent("a", "agent-a", "antigravity");
    let source = f.agy("session-1111");
    let log = antigravity_app_dir(&f.home).join("log/cli-test.log");
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    fs::write(&log, format!("Initializing CLI store manager for workspace {}\nStreaming conversation session-1111\n", owner.cwd.display())).unwrap();
    assert!(f.verify(&owner, &source).is_err());
    fs::write(
        &log,
        format!(
            "Initializing CLI store manager for workspace {}\nCreated conversation session-1111\n",
            owner.cwd.display()
        ),
    )
    .unwrap();
    f.verify(&owner, &source).unwrap();
}

#[test]
fn move_preserves_owner_but_two_existing_identities_are_ambiguous() {
    let f = Fixture::new();
    let owner = f.agent("a", "agent-a", "antigravity");
    let source = f.agy("session-1111");
    f.index(&owner, &source.session_id);
    f.verify(&owner, &source).unwrap();
    let moved = f.agent("b", "agent-a", "antigravity");
    assert!(f.verify(&moved, &source).is_err());
    fs::remove_dir_all(&owner.cwd).unwrap();
    f.verify(&moved, &source).unwrap();
}

#[test]
fn corrupt_receipt_is_local_to_one_session_and_never_becomes_empty() {
    let f = Fixture::new();
    let owner = f.agent("a", "agent-a", "antigravity");
    let source = f.agy("session-1111");
    f.index(&owner, &source.session_id);
    f.verify(&owner, &source).unwrap();
    fs::write(
        Store::new(&f.kota)
            .path(&key(&f.home, &owner, &source.session_id))
            .unwrap(),
        b"{",
    )
    .unwrap();
    assert!(f
        .verify(&owner, &source)
        .unwrap_err()
        .to_string()
        .contains("damaged"));
    let fresh = f.agy("session-2222");
    f.index(&owner, &fresh.session_id);
    f.verify(&owner, &fresh).unwrap();
}

#[test]
fn replacement_requires_native_proof_and_append_does_not() {
    let f = Fixture::new();
    let owner = f.agent("a", "agent-a", "antigravity");
    let source = f.agy("session-1111");
    f.index(&owner, &source.session_id);
    f.verify(&owner, &source).unwrap();
    let stamp = incarnation(&source).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&source.path)
        .unwrap()
        .write_all(b"{}\n")
        .unwrap();
    assert_eq!(stamp, incarnation(&source).unwrap());
    fs::remove_file(antigravity_app_dir(&f.home).join("cache/last_conversations.json")).unwrap();
    let conversation = source
        .path
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    fs::rename(conversation, conversation.with_extension("old")).unwrap();
    let replacement = f.agy("session-1111");
    assert_ne!(stamp, incarnation(&replacement).unwrap());
    assert!(f.verify(&owner, &replacement).is_err());
    f.index(&owner, &source.session_id);
    f.verify(&owner, &replacement).unwrap();
}

#[test]
fn concurrent_claims_serialize_to_one_owner() {
    let f = Fixture::new();
    let a = f.agent("a", "agent-a", "antigravity");
    let b = f.agent("b", "agent-b", "antigravity");
    let key = key(&f.home, &a, "session-1111");
    let results = std::thread::scope(|scope| {
        let handles: Vec<_> = [&a, &b]
            .into_iter()
            .map(|agent| {
                scope.spawn(|| {
                    Store::new(&f.kota).verify(
                        &key,
                        agent,
                        Some((Path::new("test"), "incarnation-1")),
                        || Ok(Some("fixture".into())),
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
}

#[test]
fn provider_native_cwd_evidence_is_preserved() {
    let f = Fixture::new();
    for provider in ["claude", "codex", "pi", "kimi", "opencode"] {
        let agent = f.agent(provider, "agent-a", provider);
        let id = "session-1111";
        let path = match provider {
            "claude" => claude_project_dir(&f.home, &agent.cwd).join(format!("{id}.jsonl")),
            "kimi" => f
                .home
                .join(".kimi-code/sessions/workspace")
                .join(id)
                .join("agents/main/wire.jsonl"),
            "opencode" => opencode_db_path(&f.home),
            _ => f.home.join(provider).join(format!("{id}.jsonl")),
        };
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        match provider {
            "codex" => fs::write(&path, format!("{}\n", serde_json::json!({"type":"session_meta", "payload":{"id":id,"cwd":path_string(&agent.cwd)}}))).unwrap(),
            "pi" => fs::write(&path, format!("{}\n", serde_json::json!({"type":"session","id":id,"cwd":path_string(&agent.cwd)}))).unwrap(),
            "kimi" => {
                fs::write(&path, "{}\n").unwrap();
                fs::write(path.parent().unwrap().parent().unwrap().parent().unwrap().join("state.json"),
                    serde_json::to_vec(&serde_json::json!({"cwd":path_string(&agent.cwd)})).unwrap()).unwrap();
            },
            "opencode" => {
                let db = Connection::open(&path).unwrap();
                db.execute_batch("create table session(id text, directory text, time_created integer);").unwrap();
                db.execute("insert into session values (?1, ?2, 1)", [id, &path_string(&agent.cwd)]).unwrap();
            },
            _ => fs::write(&path, "{}\n").unwrap(),
        }
        let source = NativeSource {
            kind: native_source_kind_for_shell(provider),
            session_id: id.into(),
            path,
            aux_path: None,
        };
        f.verify(&agent, &source)
            .unwrap_or_else(|e| panic!("{provider}: {e}"));
        let foreign = f.agent("foreign", "agent-b", provider);
        assert!(f.verify(&foreign, &source).is_err(), "{provider}");
    }
}

#[test]
fn fresh_strips_all_agy_continuation_selectors() {
    let args = [
        "--continue",
        "--conversation=old-session",
        "--dangerously-skip-permissions",
    ]
    .map(String::from);
    assert_eq!(
        crate::fresh_project_agent_launch_args(AgentCli::Antigravity, &args),
        vec!["--dangerously-skip-permissions"]
    );
}

#[test]
fn selectors_do_not_confuse_codex_config_with_continue() {
    assert_eq!(
        selector(AgentCli::Codex, &["-c".into(), "model=test".into()]),
        (None, false)
    );
    assert_eq!(
        selector(AgentCli::Codex, &["resume".into(), "--last".into()]),
        (None, true)
    );
    assert_eq!(
        selector(
            AgentCli::Antigravity,
            &["--conversation=session-1111".into()]
        ),
        (Some("session-1111".into()), true)
    );
}

#[test]
fn unknown_session_denial_keeps_its_type_through_anyhow_context() {
    let error = denied("fixture").context("submit failed");
    assert!(error.is::<Unavailable>());
    assert!(error
        .chain()
        .any(|e| e.to_string().starts_with(UNAVAILABLE)));
}

fn launch_request(agent: &ProjectAgent, cli: AgentCli) -> AgentSpawnRequest {
    AgentSpawnRequest {
        agent_id: agent.agent_id.clone(),
        cli,
        cwd: path_string(&agent.cwd),
        project_root: path_string(&project_root(agent)),
        worktree_root: None,
        shared_dir: None,
        rules_dir: None,
        adapter_path: None,
        args: vec![],
        session_id: None,
        project_id: None,
        project_remote: None,
        project_base_ref: None,
        takeover: false,
        fresh_session: false,
    }
}

#[test]
fn old_bare_id_cannot_reach_spawn_without_proof() {
    let f = Fixture::new();
    let agent = f.agent("a", "agent-a", "antigravity");
    let mut req = launch_request(&agent, AgentCli::Antigravity);
    req.session_id = Some("session-1111".into());
    let result = prepare_launch_with(
        &mut req,
        |agent, id| verify_id_in(&f.home, &f.kota, agent, id),
        |_| panic!("not a latest lookup"),
    );
    assert!(result.is_err());
}

#[test]
fn latest_resume_is_pinned_to_the_verified_id() {
    let f = Fixture::new();
    let agent = f.agent("a", "agent-a", "codex");
    let mut req = launch_request(&agent, AgentCli::Codex);
    req.args = vec!["resume".into(), "--last".into()];
    let guard = prepare_launch_with(
        &mut req,
        |_, _| panic!("not an explicit ID"),
        |a| {
            assert_eq!(a.agent_id, agent.agent_id);
            Ok(Some("session-owned".into()))
        },
    )
    .unwrap();
    assert_eq!(req.session_id.as_deref(), Some("session-owned"));
    assert_eq!(guard.bound, req.session_id);
    assert!(!req.args.iter().any(|a| a == "resume" || a == "--last"));
}

#[test]
fn fresh_accepts_first_prompt_then_binds_created_id_without_using_old_metadata() {
    let f = Fixture::new();
    let agent = f.agent("a", "agent-a", "antigravity");
    let mut req = launch_request(&agent, AgentCli::Antigravity);
    req.session_id = Some("session-foreign".into());
    req.args = vec!["--continue".into(), "--conversation=session-foreign".into()];
    req.fresh_session = true;
    let mut guard = prepare_launch_with(
        &mut req,
        |_, _| panic!("Fresh never resumes"),
        |_| panic!("Fresh never selects latest"),
    )
    .unwrap();
    assert!(req.session_id.is_none());
    assert_eq!(selector(req.cli, &req.args), (None, false));
    guard
        .check_with(|_, _| panic!("no native ID yet"), |_, _| Ok(None))
        .unwrap();
    let source = f.agy("session-created");
    f.index(&agent, &source.session_id);
    guard
        .check_with(
            |_, _| panic!("still bootstrapping"),
            |a, _| {
                f.verify(a, &source)?;
                Ok(Some(source.session_id.clone()))
            },
        )
        .unwrap();
    assert_eq!(guard.bound.as_deref(), Some("session-created"));
    guard
        .check_with(
            |a, id| verify_id_in(&f.home, &f.kota, a, id),
            |_, _| panic!("bound ID does not drift with latest"),
        )
        .unwrap();
}

#[test]
fn fork_checks_parent_ownership_and_keeps_child_bootstrap_independent() {
    let f = Fixture::new();
    let parent = f.agent("a", "agent-parent", "claude");
    let child = f.agent("a", "agent-child", "claude");
    fs::write(
        child.cwd.join("agent.yaml"),
        "id: agent-child\nprovider: claude\nbunshin-from: agent-parent\n",
    )
    .unwrap();
    let mut req = launch_request(&child, AgentCli::Claude);
    req.args = vec![
        "--resume".into(),
        "session-parent".into(),
        "--fork-session".into(),
    ];
    let guard = prepare_launch_with(
        &mut req,
        |a, id| {
            assert_eq!(a.agent_id, parent.agent_id);
            assert_eq!(a.cwd, parent.cwd);
            assert_eq!(id, "session-parent");
            Ok(())
        },
        |_| panic!("no latest lookup"),
    )
    .unwrap();
    assert!(guard.new_session);
    assert!(guard.bound.is_none());
    assert_eq!(guard.agent.unwrap().agent_id, child.agent_id);
}

#[test]
fn native_file_identity_replacement_resets_only_its_incremental_cursor() {
    let f = Fixture::new();
    let agent = f.agent("a", "agent-a", "claude");
    let path = f.root.join("session.jsonl");
    let source = NativeSource {
        kind: "claude-jsonl".into(),
        session_id: "session-fixture".into(),
        path: path.clone(),
        aux_path: None,
    };
    let line = |id: &str, text: &str| {
        format!(
            "{}\n",
            serde_json::json!({
                "type":"assistant", "uuid":id, "timestamp":"2026-01-01T00:00:00Z",
                "message":{"role":"assistant", "content":[{"type":"text", "text":text}]}
            })
        )
    };
    fs::write(&path, line("before", "old output")).unwrap();
    let first =
        parse_jsonl_source_incremental(&f.root, &agent, &source, parse_claude_line).unwrap();
    assert!(first.iter().any(|event| event.text == "old output"));
    fs::rename(&path, path.with_extension("old")).unwrap();
    fs::write(&path, line("after", "new output longer than the old file")).unwrap();
    let next = parse_jsonl_source_incremental(&f.root, &agent, &source, parse_claude_line).unwrap();
    assert!(next
        .iter()
        .any(|event| event.text == "new output longer than the old file"));
    assert!(!next.iter().any(|event| event.text == "old output"));
    assert_eq!(PARSED_EVENT_CACHE_VERSION, "8");
}

#[test]
fn explicit_resume_and_metadata_commit_use_the_same_verified_id() {
    let f = Fixture::new();
    let agent = f.agent("a", "agent-a", "antigravity");
    let mut req = launch_request(&agent, AgentCli::Antigravity);
    req.session_id = Some("session-stale-hint".into());
    req.args = vec!["--conversation=session-owned".into()];
    let guard = prepare_launch_with(
        &mut req,
        |_, id| {
            assert_eq!(id, "session-owned");
            Ok(())
        },
        |_| panic!("explicit resume"),
    )
    .unwrap();
    assert_eq!(req.session_id.as_deref(), Some("session-owned"));
    assert_eq!(guard.bound, req.session_id);
}

fn opencode_fixture(f: &Fixture, owner: &ProjectAgent) -> (NativeSource, Connection) {
    let path = opencode_db_path(&f.home);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let db = Connection::open(&path).unwrap();
    db.execute_batch(
        "create table session(id text primary key, directory text, time_created integer);",
    )
    .unwrap();
    db.execute(
        "insert into session values ('session-db', ?1, 1)",
        [&path_string(&owner.cwd)],
    )
    .unwrap();
    (
        NativeSource {
            kind: "opencode-sqlite".into(),
            session_id: "session-db".into(),
            path,
            aux_path: None,
        },
        db,
    )
}

#[test]
fn opencode_pruned_row_uses_receipt_but_never_grants_a_foreign_owner() {
    let f = Fixture::new();
    let owner = f.agent("a", "agent-a", "opencode");
    let other = f.agent("b", "agent-b", "opencode");
    let (source, db) = opencode_fixture(&f, &owner);
    f.verify(&owner, &source).unwrap();
    db.execute("delete from session", []).unwrap();
    assert!(source.path.is_file());
    verify_id_in(&f.home, &f.kota, &owner, &source.session_id).unwrap();
    f.verify(&owner, &source).unwrap();
    assert!(verify_id_in(&f.home, &f.kota, &other, &source.session_id)
        .unwrap_err()
        .is::<Unavailable>());
    let missing = NativeSource {
        session_id: "never-registered".into(),
        ..source.clone()
    };
    assert!(f.verify(&owner, &missing).unwrap_err().is::<Unavailable>());
    // Reusing the ID for a different native row must require proof again.
    db.execute(
        "insert into session values ('session-db', ?1, 2)",
        [&path_string(&other.cwd)],
    )
    .unwrap();
    assert!(f.verify(&owner, &source).unwrap_err().is::<Unavailable>());
    f.verify(&other, &source).unwrap();
}

#[test]
fn opencode_busy_is_transient_and_recovers_without_fresh() {
    let f = Fixture::new();
    let owner = f.agent("a", "agent-a", "opencode");
    let (source, db) = opencode_fixture(&f, &owner);
    f.verify(&owner, &source).unwrap();
    db.execute_batch("BEGIN EXCLUSIVE;").unwrap();
    let error = verify_id_in(&f.home, &f.kota, &owner, &source.session_id).unwrap_err();
    assert!(!error.is::<Unavailable>());
    assert!(availability(Err(error)).is_err());
    db.execute_batch("ROLLBACK;").unwrap();
    assert_eq!(
        availability(verify_id_in(&f.home, &f.kota, &owner, &source.session_id)).unwrap(),
        true
    );
}

#[test]
fn store_and_evidence_io_errors_do_not_request_fresh() {
    let f = Fixture::new();
    let owner = f.agent("a", "agent-a", "antigravity");
    let store = Store::new(&f.kota);
    let key = key(&f.home, &owner, "session-io");
    let path = store.path(&key).unwrap();
    fs::create_dir_all(path.with_extension("lock")).unwrap();
    assert!(!store
        .verify(&key, &owner, None, || Ok(None))
        .unwrap_err()
        .is::<Unavailable>());
    fs::remove_dir(path.with_extension("lock")).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(!store.read(&key).unwrap_err().is::<Unavailable>());
    fs::remove_dir(&path).unwrap();
    let error = store
        .verify(&key, &owner, None, || Err("fixture read failed".into()))
        .unwrap_err();
    assert!(!error.is::<Unavailable>());
    assert!(availability(Err(error)).is_err());
    assert!(!availability(Err(denied("fixture unowned"))).unwrap());
    assert_eq!(
        display_error(&denied("session belongs to another agent")),
        RECOVERY_MESSAGE
    );
}

#[test]
fn first_claim_rejects_duplicate_id_in_two_registered_projects() {
    let f = Fixture::new();
    let owner = f.agent(
        "home/Kota/Workspaces/project-one",
        "agent-fixture",
        "antigravity",
    );
    let duplicate = f.agent(
        "home/Kota/Workspaces/project-two",
        "agent-fixture",
        "antigravity",
    );
    for (agent, id) in [(&owner, "fixture-one"), (&duplicate, "fixture-two")] {
        fs::write(
            project_root(agent).join("workspace.json"),
            serde_json::to_vec(&serde_json::json!({
                "projectId":id,"repoFullName":format!("fixture/{id}")
            }))
            .unwrap(),
        )
        .unwrap();
    }
    let source = f.agy("session-fixture");
    f.index(&owner, &source.session_id);
    let error = f.verify(&owner, &source).unwrap_err();
    assert!(error.is::<Unavailable>());
    assert!(error.to_string().contains("two registered projects"));
    assert!(!Store::new(&f.kota)
        .path(&key(&f.home, &owner, &source.session_id))
        .unwrap()
        .exists());
}

#[test]
fn timestamp_identity_is_epoch_seconds_and_nanoseconds() {
    assert_eq!(
        timestamp_identity(SystemTime::UNIX_EPOCH + StdDuration::new(123, 456)).unwrap(),
        "123:456"
    );
}

#[cfg(unix)]
#[test]
fn non_utf8_scope_returns_an_error_instead_of_panicking() {
    use std::os::unix::ffi::OsStringExt;
    let f = Fixture::new();
    let key = Key {
        provider: "codex".into(),
        scope: PathBuf::from(std::ffi::OsString::from_vec(vec![b'/', 0xff])),
        session: "fixture".into(),
    };
    let error = Store::new(&f.kota).path(&key).unwrap_err();
    assert!(!error.is::<Unavailable>());
}

#[test]
fn violet_denial_is_blocked_but_stale_startup_denial_is_discarded() {
    let f = Fixture::new();
    let owner = f.agent("a", "agent-a", "claude");
    let expected = read_yaml_file(&owner.cwd.join("agent.yaml"))
        .unwrap()
        .as_ref()
        .map(binding_fingerprint);
    let denial = || Err(denied("fixture missing proof").to_string());
    let error = finish_source_lookup(&owner, expected.clone(), denial()).unwrap_err();
    assert_eq!(source_error_status(&error), "blocked");
    assert_eq!(
        source_error_status(&transient("fixture I/O").to_string()),
        "error"
    );
    let lock = crate::shell_switch::metadata_lock(&owner.cwd);
    lock.lock().unwrap().starting = Some("fixture-starting".into());
    assert!(!source_binding_is_current(&owner).unwrap());
    assert!(finish_source_lookup(&owner, expected.clone(), denial())
        .unwrap()
        .is_none());
    lock.lock().unwrap().starting = None;
    fs::write(
        owner.cwd.join("agent.yaml"),
        "id: agent-a\nprovider: claude\nshell-generation: new\n",
    )
    .unwrap();
    assert!(finish_source_lookup(&owner, expected, denial())
        .unwrap()
        .is_none());
}
