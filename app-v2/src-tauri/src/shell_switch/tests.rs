use super::*;

struct Fixture {
    root: PathBuf,
    cwd: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("kota-shell-switch-{}", Uuid::new_v4()));
        let cwd = root.join(".agent-workspaces/agent-fixture73");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(root.join("project-memory/chathistory")).unwrap();
        fs::create_dir_all(cwd.join("project-files/.git")).unwrap();
        fs::write(cwd.join("AGENTS.md"), "<!-- kota:adapter:AGENTS.md -->\n<!-- kota:ghost:start -->\nIncarnation Ghost.\n<!-- kota:ghost:end -->\n").unwrap();
        fs::write(cwd.join("agent.yaml"), "id: agent-fixture73\ndisplay-name: Tester\nprovider: codex\nshell: codex\nsession-id: old-session\nsession-source: native\nstatus: active\n").unwrap();
        fs::write(
            cwd.join("SHELL.yaml"),
            "provider: codex\nmodel: default\nargs: []\n",
        )
        .unwrap();
        Self { root, cwd }
    }
    fn target(&self, provider: &str) {
        fs::write(
            self.cwd.join("SHELL.yaml"),
            format!("provider: {provider}\nmodel: default\nargs: []\n"),
        )
        .unwrap();
    }
    fn request(&self, fresh: bool) -> AgentSpawnRequest {
        serde_json::from_value(serde_json::json!({
            "agentId":"agent-fixture73", "cli":"codex", "cwd":self.cwd,
            "projectRoot":self.root, "projectId":"fixture-zenith-river-73", "freshSession":fresh,
            "sessionId":"old-session", "args":["resume", "old-session"]
        }))
        .unwrap()
    }
    fn yaml(&self) -> serde_yaml::Mapping {
        read_yaml_mapping(&self.cwd.join("agent.yaml")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let root = self.root.clone();
        let _ = adapter_sync::call(move || {
            adapter_sync::take_project(&root);
            Ok(())
        });
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn resumed_pty_commits_verified_id_instead_of_stale_saved_hint() {
    let f = Fixture::new();
    let mut request = f.request(false);
    let mut launch = Launch::prepare(&mut request, None).unwrap().unwrap();
    assert!(!launch.is_fresh());
    assert_eq!(request.session_id.as_deref(), Some("old-session"));
    // Ownership preflight replaces a stale hint before process creation.
    request.session_id = Some("verified-session".into());
    launch.commit(request.session_id.as_deref()).unwrap();
    assert_eq!(yaml_string(&f.yaml(), "session-id").as_deref(), Some("verified-session"));
    assert!(yaml_string(&f.yaml(), "session-reset-at").is_none());
}

#[test]
fn actual_save_and_reopen_keep_applied_shell_ghost_and_session_until_start() {
    let f = Fixture::new();
    let manager = IntegrationManager::default();
    let save = |provider: &str| ProjectAgentSaveRequest {
        shell_revision: Some(crate::puppeteer_setup::revision(Some(&fs::read(f.cwd.join("SHELL.yaml")).unwrap()))),
        agent_id: "agent-fixture73".into(),
        project_root: Some(path_string(&f.root)),
        provider: Some(provider.into()),
        display_name: "Tester".into(),
        name_fields: None,
        model: "default".into(),
        effort: None,
        auto_route: None,
        avatar_id: None,
        skills: Vec::new(),
        ghost: "Incarnation Ghost.".into(),
    };
    let initial =
        load_project_agent_detail(&manager, Some(&path_string(&f.root)), "agent-fixture73")
            .unwrap();
    let saved = save_project_agent_detail(&manager, save("claude")).unwrap();
    assert_eq!(saved.cli, AgentCli::Claude);
    assert_eq!(saved.ghost, initial.ghost);
    assert_eq!(saved.session_id, initial.session_id);
    assert_eq!(
        effective_provider(&f.yaml()).unwrap(),
        Some(AgentCli::Codex)
    );
    assert!(yaml_value::<Handoff>(&f.yaml(), "handoff-pending").is_none());
    assert!(f.cwd.join("AGENTS.md").is_file());
    assert!(!f.cwd.join("CLAUDE.md").exists());
    // A rule-change refresh must still use the applied provider.
    regenerate_project_adapters_in_root(&f.root).unwrap();
    assert!(!f.cwd.join("CLAUDE.md").exists());
    save_project_agent_detail(&manager, save("codex")).unwrap();
    let mut request = f.request(false);
    assert!(!Launch::prepare(&mut request, None).unwrap().unwrap().fresh);
    save_project_agent_detail(&manager, save("claude")).unwrap();
    save_project_agent_detail(&manager, save("kimi")).unwrap();
    let mut request = f.request(false);
    let launch = Launch::prepare(&mut request, None).unwrap().unwrap();
    assert_eq!(request.cli, AgentCli::Kimi);
    assert!(launch
        .handoff
        .as_ref()
        .unwrap()
        .text
        .contains("from codex to kimi"));
}

#[test]
fn a_legacy_default_local_agent_can_save_identity_ghost_and_skills_without_pi() {
    let f = Fixture::new();
    f.target("kota-local");
    let manager = IntegrationManager::default();
    let request = |name: &str, ghost: &str, skills: Vec<String>| ProjectAgentSaveRequest {
        shell_revision: Some(crate::puppeteer_setup::revision(Some(&fs::read(f.cwd.join("SHELL.yaml")).unwrap()))),
        agent_id: "agent-fixture73".into(), project_root: Some(path_string(&f.root)),
        provider: Some("kota-local".into()), display_name: name.into(), name_fields: None,
        model: "default".into(), effort: None, auto_route: None, avatar_id: None, skills, ghost: ghost.into(),
    };
    // This uses the ordinary save entry point, with no Backend, helper or App.
    // Missing credentials/defaults cannot obstruct these non-SHELL edits.
    save_project_agent_detail(&manager, request("Renamed fixture", "Incarnation Ghost.", vec![])).unwrap();
    save_project_agent_detail(&manager, request("Renamed fixture", "Updated fixture Ghost.", vec![])).unwrap();
    let saved = save_project_agent_detail(&manager, request("Renamed fixture", "Updated fixture Ghost.", vec!["fixture-missing-skill".into()])).unwrap();
    assert_eq!(saved.model, "default");
    assert_eq!(saved.effort, None);
    assert_eq!(saved.skills, ["fixture-missing-skill"]);
    assert_eq!(saved.ghost, "Updated fixture Ghost.");
    let yaml = read_yaml_mapping(&f.cwd.join("SHELL.yaml")).unwrap();
    assert_eq!(yaml_string(&yaml, "model").as_deref(), Some("default"));
    assert!(yaml_value::<Vec<String>>(&yaml, "args").unwrap().is_empty());
}

#[test]
fn changing_a_terminal_shell_to_local_requires_a_concrete_model_and_thinking_before_any_write() {
    let f = Fixture::new();
    let manager = IntegrationManager::default();
    let agent_before = fs::read(f.cwd.join("agent.yaml")).unwrap();
    let shell_before = fs::read(f.cwd.join("SHELL.yaml")).unwrap();
    let request = |model: &str, effort: Option<&str>| ProjectAgentSaveRequest {
        shell_revision: Some(crate::puppeteer_setup::revision(Some(&fs::read(f.cwd.join("SHELL.yaml")).unwrap()))),
        agent_id: "agent-fixture73".into(), project_root: Some(path_string(&f.root)), provider: Some("kota-local".into()),
        display_name: "Tester".into(), name_fields: None, model: model.into(), effort: effort.map(str::to_owned),
        auto_route: None, avatar_id: None, skills: vec![], ghost: "Incarnation Ghost.".into(),
    };
    for (model, effort, expected) in [("default", None, "model_required"), ("fable/plain", None, "thinking_invalid"), ("fable/plain", Some("bogus"), "thinking_invalid")] {
        assert_eq!(save_project_agent_detail(&manager, request(model, effort)).err().unwrap(), expected);
        assert_eq!(fs::read(f.cwd.join("agent.yaml")).unwrap(), agent_before);
        assert_eq!(fs::read(f.cwd.join("SHELL.yaml")).unwrap(), shell_before);
    }
    let saved = save_project_agent_detail(&manager, request("fable/plain", Some("off"))).unwrap();
    assert_eq!(saved.model, "fable/plain");
    assert_eq!(saved.effort.as_deref(), Some("off"));
}

#[test]
fn resolving_a_saved_provider_target_does_not_check_the_old_session_in_the_new_provider() {
    let f = Fixture::new();
    f.target("claude");
    let before = fs::read(f.cwd.join("agent.yaml")).unwrap();
    let request = resolve_project_agent_launch(
        &IntegrationManager::default(),
        Some(&path_string(&f.root)),
        "agent-fixture73",
    )
    .unwrap();
    assert_eq!(request.cli, AgentCli::Claude);
    assert_eq!(request.session_id.as_deref(), Some("old-session"));
    assert_eq!(
        claude_resume_target_availability(&request),
        ResumeTargetAvailability::Unknown
    );
    assert_eq!(fs::read(f.cwd.join("agent.yaml")).unwrap(), before);
    assert!(yaml_value::<Handoff>(&f.yaml(), "handoff-pending").is_none());
}

#[test]
fn ordinary_fresh_without_pending_keeps_identity_ghost_and_does_not_inject() {
    let f = Fixture::new();
    regenerate_project_adapters_in_root(&f.root).unwrap();
    let shell_before = fs::read(f.cwd.join("SHELL.yaml")).unwrap();
    let ghost_before = fs::read(f.cwd.join("AGENTS.md")).unwrap();
    let mut request = f.request(true);
    let mut launch = Launch::prepare(&mut request, None).unwrap().unwrap();
    assert!(launch.fresh && launch.handoff.is_none());
    assert_eq!(request.session_id, None);
    assert_eq!(
        request.args,
        fresh_project_agent_launch_args(AgentCli::Codex, &[])
    );
    launch.prepare_adapter().unwrap();
    launch.commit(None).unwrap();
    drop(launch);
    assert_eq!(
        effective_provider(&f.yaml()).unwrap(),
        Some(AgentCli::Codex)
    );
    assert_eq!(
        yaml_string(&f.yaml(), "id").as_deref(),
        Some("agent-fixture73")
    );
    assert_eq!(
        yaml_string(&f.yaml(), "display-name").as_deref(),
        Some("Tester")
    );
    assert!(yaml_string(&f.yaml(), "session-id").is_none());
    assert!(yaml_string(&f.yaml(), "session-reset-at").is_some());
    assert_eq!(fs::read(f.cwd.join("SHELL.yaml")).unwrap(), shell_before);
    assert_eq!(fs::read(f.cwd.join("AGENTS.md")).unwrap(), ghost_before);
    assert_eq!(
        compose(
            &f.cwd,
            AgentCli::Codex,
            "next input".into(),
            PromptSource::Composer
        )
        .unwrap(),
        ("next input".into(), None)
    );
}

#[test]
fn target_adapter_preserves_ghost_and_rollback_preserves_later_user_edits() {
    let f = Fixture::new();
    let mut yaml = f.yaml();
    yaml_remove(&mut yaml, "display-name");
    yaml_set_string(&mut yaml, "displayName", "Legacy Name");
    write_yaml_mapping(&f.cwd.join("agent.yaml"), &yaml).unwrap();
    f.target("claude");
    let before = fs::read(f.cwd.join("AGENTS.md")).unwrap();
    let mut req = f.request(false);
    let mut launch = Launch::prepare(&mut req, None).unwrap().unwrap();
    launch.prepare_adapter().unwrap();
    assert!(fs::read_to_string(f.cwd.join("CLAUDE.md"))
        .unwrap()
        .contains("Incarnation Ghost."));
    assert!(fs::read_to_string(f.cwd.join("CLAUDE.md"))
        .unwrap()
        .contains("You are Legacy Name, a Kota project agent."));
    assert_eq!(fs::read(f.cwd.join("AGENTS.md")).unwrap(), before);
    launch.rollback().unwrap();
    assert!(!f.cwd.join("CLAUDE.md").exists());
    let mut next = Launch::prepare(&mut f.request(false), None)
        .unwrap()
        .unwrap();
    next.prepare_adapter().unwrap();
    fs::write(f.cwd.join("CLAUDE.md"), "user changed this during startup").unwrap();
    next.rollback().unwrap();
    assert_eq!(
        fs::read_to_string(f.cwd.join("CLAUDE.md")).unwrap(),
        "user changed this during startup"
    );
    assert_eq!(fs::read(f.cwd.join("AGENTS.md")).unwrap(), before);
}

#[test]
fn late_profile_edit_is_carried_forward_and_lifecycle_retirement_aborts_commit() {
    let f = Fixture::new();
    f.target("claude");
    let mut launch = Launch::prepare(&mut f.request(false), None)
        .unwrap()
        .unwrap();
    launch.prepare_adapter().unwrap();
    let old = fs::read_to_string(f.cwd.join("AGENTS.md")).unwrap();
    fs::write(
        f.cwd.join("AGENTS.md"),
        replace_adapter_ghost(&old, "Latest Ghost").unwrap(),
    )
    .unwrap();
    launch.commit(None).unwrap();
    assert!(fs::read_to_string(f.cwd.join("CLAUDE.md"))
        .unwrap()
        .contains("Latest Ghost"));
    drop(launch);
    f.target("codex");
    let mut launch = Launch::prepare(&mut f.request(false), None)
        .unwrap()
        .unwrap();
    let mut yaml = f.yaml();
    yaml_set_string(&mut yaml, "status", "archived");
    write_yaml_mapping(&f.cwd.join("agent.yaml"), &yaml).unwrap();
    assert!(launch.commit(None).is_err());
    assert_eq!(
        effective_provider(&f.yaml()).unwrap(),
        Some(AgentCli::Claude)
    );
}

#[test]
fn preparing_target_skills_keeps_applied_links_and_owned_cleanup_keeps_user_files() {
    let f = Fixture::new();
    let pool = f.root.join("skill-pool");
    fs::create_dir_all(pool.join("s")).unwrap();
    fs::write(pool.join("s/SKILL.md"), "skill").unwrap();
    project_account_skills_from_pool(&f.cwd, AgentCli::Codex, &["s".into()], &pool).unwrap();
    project_account_skills_from_pool(&f.cwd, AgentCli::Claude, &["s".into()], &pool).unwrap();
    assert!(f.cwd.join(".agents/skills/s/SKILL.md").is_file());
    assert!(f.cwd.join(".claude/skills/s/SKILL.md").is_file());
    fs::create_dir_all(f.cwd.join(".agents/skills/user")).unwrap();
    fs::write(f.cwd.join(".agents/skills/user/SKILL.md"), "user content").unwrap();
    let mut yaml = f.yaml();
    set_provider(&mut yaml, AgentCli::Claude);
    write_yaml_mapping(&f.cwd.join("agent.yaml"), &yaml).unwrap();
    project_account_skills_from_pool(&f.cwd, AgentCli::Claude, &["s".into()], &pool).unwrap();
    assert!(!f.cwd.join(".agents/skills/s").exists());
    assert!(f.cwd.join(".agents/skills/user/SKILL.md").is_file());
}

#[test]
fn latest_target_is_pinned_only_at_start_and_completion_does_not_overwrite_later_save() {
    let f = Fixture::new();
    let mut req = f.request(false); // Stale bus fallback is resolved before Save.
    f.target("claude");
    let mut launch = Launch::prepare(&mut req, None).unwrap().unwrap();
    assert_eq!(req.cli, AgentCli::Claude);
    assert_eq!(req.session_id, None);
    assert!(!req
        .args
        .iter()
        .any(|a| a.contains("old-session") || a == "resume"));
    assert!(launch.handoff.is_some());
    assert_eq!(
        effective_provider(&f.yaml()).unwrap(),
        Some(AgentCli::Codex)
    );
    f.target("kimi");
    launch.commit(None).unwrap();
    assert_eq!(
        effective_provider(&f.yaml()).unwrap(),
        Some(AgentCli::Claude)
    );
    assert!(fs::read_to_string(f.cwd.join("SHELL.yaml"))
        .unwrap()
        .contains("kimi"));
    let mut next = f.request(false);
    let next = Launch::prepare(&mut next, None).unwrap().unwrap();
    assert!(next
        .handoff
        .as_ref()
        .unwrap()
        .text
        .contains("from claude to kimi"));
}

#[test]
fn saving_back_to_applied_provider_does_not_switch_or_inject() {
    let f = Fixture::new();
    f.target("claude");
    f.target("codex");
    let mut req = f.request(false);
    let launch = Launch::prepare(&mut req, None).unwrap().unwrap();
    assert!(!launch.fresh);
    assert!(launch.handoff.is_none());
    assert_eq!(req.session_id.as_deref(), Some("old-session"));
}

#[test]
fn failed_start_preserves_applied_metadata_and_latest_target() {
    let f = Fixture::new();
    f.target("claude");
    let before = fs::read(f.cwd.join("agent.yaml")).unwrap();
    let mut req = f.request(false);
    {
        let launch = Launch::prepare(&mut req, None).unwrap().unwrap();
        assert!(launch.handoff.is_some());
    }
    assert_eq!(fs::read(f.cwd.join("agent.yaml")).unwrap(), before);
    let mut retry = f.request(false);
    assert!(Launch::prepare(&mut retry, None).unwrap().is_some());
    assert_eq!(retry.cli, AgentCli::Claude);
}

#[test]
fn first_start_is_not_a_fictional_provider_handoff() {
    let f = Fixture::new();
    fs::write(
        f.cwd.join("agent.yaml"),
        "id: agent-fixture73\nstatus: active\n",
    )
    .unwrap();
    f.target("claude");
    let mut req = f.request(false);
    let launch = Launch::prepare(&mut req, None).unwrap().unwrap();
    assert!(launch.handoff.is_none());
    assert_eq!(req.cli, AgentCli::Claude);
}

#[test]
fn aliases_are_equal_but_conflicting_applied_markers_fail_closed() {
    let yaml = serde_yaml::from_str("provider: claude\nshell: cc\n").unwrap();
    assert_eq!(effective_provider(&yaml).unwrap(), Some(AgentCli::Claude));
    let yaml = serde_yaml::from_str("provider: codex\nshell: cc\n").unwrap();
    assert!(effective_provider(&yaml).is_err());
}

#[test]
fn legacy_baseline_uses_configuration_before_save() {
    let mut yaml =
        serde_yaml::from_str("id: agent-fixture73\nsession-source: native\nsession-id: old\n")
            .unwrap();
    let previous = ShellYaml {
        provider: Some("claude".into()),
        ..Default::default()
    };
    preserve_legacy_provider(Path::new("/nonexistent"), &mut yaml, &previous, None).unwrap();
    assert_eq!(effective_provider(&yaml).unwrap(), Some(AgentCli::Claude));
    let next = ShellYaml {
        provider: Some("codex".into()),
        ..Default::default()
    };
    preserve_legacy_provider(Path::new("/nonexistent"), &mut yaml, &next, None).unwrap();
    assert_eq!(effective_provider(&yaml).unwrap(), Some(AgentCli::Claude));
}

#[test]
fn pending_survives_resume_excluded_channels_and_only_matching_success_clears() {
    let f = Fixture::new();
    f.target("claude");
    let mut req = f.request(false);
    let mut launch = Launch::prepare(&mut req, None).unwrap().unwrap();
    launch.commit(None).unwrap();
    drop(launch);
    let pending: Handoff = yaml_value(&f.yaml(), "handoff-pending").unwrap();
    let mut resumed = f.request(false);
    let mut resumed = Launch::prepare(&mut resumed, None).unwrap().unwrap();
    resumed.commit(None).unwrap();
    drop(resumed);
    let original = "\x1b[200~<KOTA_MESSAGE>original\nmessage</KOTA_MESSAGE>\x1b[201~".to_string();
    assert_eq!(
        compose(
            &f.cwd,
            AgentCli::Claude,
            original.clone(),
            PromptSource::bus("dream")
        )
        .unwrap()
        .0,
        original
    );
    let (injected, sent) = compose(
        &f.cwd,
        AgentCli::Claude,
        original.clone(),
        PromptSource::Bus,
    )
    .unwrap();
    assert_eq!(injected.matches("\x1b[200~").count(), 1);
    assert!(injected.ends_with(&original[6..]));
    assert_eq!(sent.as_ref(), Some(&pending));
    // Simulate a failed submit: no clearing takes place.
    assert!(compose(
        &f.cwd,
        AgentCli::Claude,
        original.clone(),
        PromptSource::Composer
    )
    .unwrap()
    .1
    .is_some());
    let mut newer = pending.clone();
    newer.id = Uuid::new_v4().to_string();
    newer.generation = Uuid::new_v4().to_string();
    let mut yaml = f.yaml();
    yaml_set_value(&mut yaml, "handoff-pending", &newer).unwrap();
    write_yaml_mapping(&f.cwd.join("agent.yaml"), &yaml).unwrap();
    submitted(&f.cwd, &pending).unwrap();
    assert_eq!(
        yaml_value::<Handoff>(&f.yaml(), "handoff-pending"),
        Some(newer.clone())
    );
    submitted(&f.cwd, &newer).unwrap();
    assert_eq!(
        compose(
            &f.cwd,
            AgentCli::Claude,
            original.clone(),
            PromptSource::Composer
        )
        .unwrap()
        .0,
        original
    );
}

#[test]
fn explicit_same_provider_fresh_clears_pending_but_switch_fresh_keeps_new_pending() {
    let f = Fixture::new();
    f.target("claude");
    let mut req = f.request(true);
    let mut launch = Launch::prepare(&mut req, None).unwrap().unwrap();
    launch.commit(None).unwrap();
    drop(launch);
    assert!(yaml_value::<Handoff>(&f.yaml(), "handoff-pending").is_some());
    let mut req = f.request(true);
    let mut fresh = Launch::prepare(&mut req, None).unwrap().unwrap();
    fresh.commit(None).unwrap();
    assert!(yaml_value::<Handoff>(&f.yaml(), "handoff-pending").is_none());
}

#[test]
fn inline_prompt_uses_target_paths_and_strict_split_preserves_original_bytes() {
    let f = Fixture::new();
    let id = Uuid::new_v4().to_string();
    let make = || {
        handoff_text(
            &id,
            "agent-fixture73",
            "Fixture Agent",
            "fixture-zenith-river-73",
            "Zenith River 73",
            &f.root.join("project-memory"),
            AgentCli::Codex,
            AgentCli::Claude,
            "2026-09-20T12:34:56Z",
        )
    };
    let prefix = make();
    assert!(!prefix.contains("Relevant summary:"));
    assert!(prefix.contains(&f.root.display().to_string()));
    let input = "<KOTA_MESSAGE id=\"abc\">\n你好\n</KOTA_MESSAGE>\n";
    let full = format!("{prefix}{input}");
    let (handoff, original) = split_inline(&full, "agent-fixture73").unwrap();
    assert_eq!(original, input);
    assert_eq!(handoff.id, id);
    assert!(split_inline(&format!("Quoted:\n{full}"), "agent-fixture73").is_none());
    assert!(split_inline(&full, "agent-other").is_none());
    assert!(split_inline(
        &full.replace("The switch happened at:", "Date:"),
        "agent-fixture73"
    )
    .is_none());
    fs::create_dir_all(f.root.join("project-memory/chathistory/summaries")).unwrap();
    fs::write(
        f.root
            .join("project-memory/chathistory/summaries/recent.json"),
        "{}",
    )
    .unwrap();
    assert!(split_inline(&format!("{}{input}", make()), "agent-fixture73").is_some());
}

#[test]
fn six_default_shells_do_not_send_default_or_invent_effort() {
    for cli in [
        AgentCli::Claude,
        AgentCli::Codex,
        AgentCli::Antigravity,
        AgentCli::Opencode,
        AgentCli::Pi,
        AgentCli::Kimi,
    ] {
        let f = Fixture::new();
        let mut yaml = f.yaml();
        set_provider(
            &mut yaml,
            if cli == AgentCli::Codex {
                AgentCli::Claude
            } else {
                AgentCli::Codex
            },
        );
        write_yaml_mapping(&f.cwd.join("agent.yaml"), &yaml).unwrap();
        f.target(shell_name_for_cli(cli));
        let mut request = f.request(false);
        let launch = Launch::prepare(&mut request, None).unwrap().unwrap();
        assert!(launch.fresh && launch.handoff.is_some());
        assert_eq!(request.cli, cli);
        let args = crate::pty::agent::args_for_spawn(
            cli,
            &request.args,
            Path::new(&request.cwd),
            request.session_id.as_deref(),
        );
        assert!(
            !args.iter().any(|a| a == "default"
                || a == "--model"
                || a == "--effort"
                || a == "--thinking"
                || a.contains("reasoning_effort")
                || a == "old-session"
                || a == "--resume"
                || a == "resume"),
            "{cli:?}: {args:?}"
        );
    }
}

#[test]
fn switching_a_local_agent_with_an_auto_route_pair_to_another_provider_writes_the_same_bytes() {
    let manager = IntegrationManager::default();
    let local = "provider: kota-local\nmodel: fable/plain\neffort: \"off\"\nargs: []\n";
    let carrying = Fixture::new();
    let group = crate::local_rpc::auto_route::fixture_group();
    let block = serde_yaml::to_string(&crate::local_rpc::auto_route::ShellGroup::from(&group)).unwrap();
    fs::write(carrying.cwd.join("SHELL.yaml"), format!("{local}auto-route:\n{}", block.lines().map(|l| format!("  {l}\n")).collect::<String>())).unwrap();
    let plain = Fixture::new();
    fs::write(plain.cwd.join("SHELL.yaml"), local).unwrap();
    let save = |f: &Fixture, provider: &str| save_project_agent_detail(&manager, ProjectAgentSaveRequest {
        shell_revision: Some(crate::puppeteer_setup::revision(Some(&fs::read(f.cwd.join("SHELL.yaml")).unwrap()))),
        agent_id: "agent-fixture73".into(), project_root: Some(path_string(&f.root)), provider: Some(provider.into()),
        display_name: "Tester".into(), name_fields: None, model: "default".into(), effort: None,
        auto_route: None, avatar_id: None, skills: Vec::new(), ghost: "Incarnation Ghost.".into(),
    }).unwrap();
    for provider in ["codex", "claude"] {
        let detail = save(&carrying, provider);
        save(&plain, provider);
        assert_eq!(detail.auto_route, None);
        let shell = fs::read_to_string(carrying.cwd.join("SHELL.yaml")).unwrap();
        assert_eq!(shell, fs::read_to_string(plain.cwd.join("SHELL.yaml")).unwrap(), "{provider}");
        assert!(!shell.contains("auto-route"), "{provider}");
        regenerate_project_adapters_in_root(&carrying.root).unwrap();
        regenerate_project_adapters_in_root(&plain.root).unwrap();
        for adapter in ["AGENTS.md", "CLAUDE.md"] {
            let read = |f: &Fixture| fs::read_to_string(f.cwd.join(adapter)).ok().map(|text| text.replace(&path_string(&f.root), "<root>")
                .replace(&f.root.file_name().unwrap().to_string_lossy().to_string(), "<project>"));
            assert_eq!(read(&carrying), read(&plain), "{provider} {adapter}");
        }
    }
}

#[test]
fn a_local_agent_keeps_its_auto_route_pair_when_a_concrete_model_is_saved() {
    let f = Fixture::new();
    let mut group = crate::local_rpc::auto_route::fixture_group();
    group.models[1].provider_id = "ollama".into(); group.models[1].model_id = "qwen3:32b".into();
    let shell = ShellYaml { provider: Some("kota-local".into()), model: Some("kota/auto".into()), effort: Some("off".into()), auto_route: Some((&group).into()), ..ShellYaml::default() };
    fs::write(f.cwd.join("SHELL.yaml"), compile_shell_yaml_text(&shell)).unwrap();
    let manager = IntegrationManager::default();
    let saved = save_project_agent_detail(&manager, ProjectAgentSaveRequest {
        shell_revision: Some(crate::puppeteer_setup::revision(Some(&fs::read(f.cwd.join("SHELL.yaml")).unwrap()))),
        agent_id: "agent-fixture73".into(), project_root: Some(path_string(&f.root)), provider: Some("kota-local".into()),
        display_name: "Tester".into(), name_fields: None, model: "fable/plain".into(), effort: Some("high".into()),
        auto_route: None, avatar_id: None, skills: Vec::new(), ghost: "Incarnation Ghost.".into(),
    }).unwrap();
    let pair = saved.auto_route.expect("the pair stays for the next switch back");
    assert_eq!(pair, group);
    let shell = parse_shell_yaml(&fs::read_to_string(f.cwd.join("SHELL.yaml")).unwrap()).unwrap();
    assert_eq!(shell.model.as_deref(), Some("fable/plain"));
    assert_eq!(shell.auto_route.unwrap().models[1].model, "ollama/qwen3:32b");
}

#[test]
fn local_profile_rejects_a_stale_raw_shell_before_identity_skills_or_ghost_writes() {
    let f = Fixture::new(); f.target("kota-local");
    let manager = IntegrationManager::default();
    let before = fs::read(f.cwd.join("SHELL.yaml")).unwrap();
    let identity = fs::read(f.cwd.join("agent.yaml")).unwrap();
    let adapter = fs::read(f.cwd.join("AGENTS.md")).unwrap();
    let mut changed = b"# Agent changed this file\n".to_vec(); changed.extend(&before);
    fs::write(f.cwd.join("SHELL.yaml"), &changed).unwrap();
    let request = ProjectAgentSaveRequest {
        agent_id: "agent-fixture73".into(), project_root: Some(path_string(&f.root)), provider: Some("kota-local".into()),
        display_name: "Renamed".into(), name_fields: None, model: "default".into(), effort: None,
        auto_route: None, avatar_id: None, skills: vec!["new-skill".into()], ghost: "Changed ghost".into(),
        shell_revision: Some(crate::puppeteer_setup::revision(Some(&before))),
    };
    assert_eq!(save_project_agent_detail(&manager, request).err().unwrap(), "local_config_changed");
    assert_eq!(fs::read(f.cwd.join("SHELL.yaml")).unwrap(), changed);
    assert_eq!(fs::read(f.cwd.join("agent.yaml")).unwrap(), identity);
    assert_eq!(fs::read(f.cwd.join("AGENTS.md")).unwrap(), adapter);
}

#[test]
fn local_detail_carries_the_exact_shell_revision_after_normalization() {
    let f = Fixture::new(); f.target("kota-local");
    let manager = IntegrationManager::default();
    let detail = load_project_agent_detail(&manager, Some(&path_string(&f.root)), "agent-fixture73").unwrap();
    assert_eq!(detail.shell_revision, Some(crate::puppeteer_setup::revision(Some(&fs::read(f.cwd.join("SHELL.yaml")).unwrap()))));
    f.target("codex");
    let terminal = load_project_agent_detail(&manager, Some(&path_string(&f.root)), "agent-fixture73").unwrap();
    assert_eq!(terminal.shell_revision, None);
}

#[test]
fn readonly_local_rule_detail_keeps_prepared_empty_group_and_comments_byte_for_byte() {
    let f = Fixture::new();
    let text = format!("{}# Personal formatting stays\nprovider: kota-local\nmodel: fable/plain\neffort: high\nargs: []\nauto-route: {{threshold: 0.85, default-model: model-1, models: []}}\n", crate::puppeteer_setup::STRING_COMMENTS);
    fs::write(f.cwd.join("SHELL.yaml"), &text).unwrap();
    let identity = fs::read(f.cwd.join("agent.yaml")).unwrap();
    let adapter = fs::read(f.cwd.join("AGENTS.md")).unwrap();
    let manager = IntegrationManager::default();
    let detail = load_project_agent_detail_with_repairs(&manager, Some(&path_string(&f.root)), "agent-fixture73", false).unwrap();
    assert_eq!(detail.shell_revision, Some(crate::puppeteer_setup::revision(Some(text.as_bytes()))));
    assert_eq!(detail.model, "fable/plain");
    assert_eq!(detail.auto_route, None);
    assert_eq!(fs::read(f.cwd.join("SHELL.yaml")).unwrap(), text.as_bytes());
    assert_eq!(fs::read(f.cwd.join("agent.yaml")).unwrap(), identity);
    assert_eq!(fs::read(f.cwd.join("AGENTS.md")).unwrap(), adapter);
}

#[test]
fn repairing_local_rule_detail_retains_prepared_empty_group_and_instructions() {
    let f = Fixture::new(); let path = f.cwd.join("SHELL.yaml");
    fs::write(&path, format!("{}provider: kota-local\nmodel: fable/plain\neffort: high\nargs: []\nauto-route: {{threshold: 0.85, default-model: model-1, models: []}}\n", crate::puppeteer_setup::STRING_COMMENTS)).unwrap();
    let manager = IntegrationManager::default();
    let detail = load_project_agent_detail(&manager, Some(&path_string(&f.root)), "agent-fixture73").unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains(crate::puppeteer_setup::STRING_COMMENTS));
    assert!(text.contains("  models: []\n"));
    assert!(parse_shell_yaml(&text).unwrap().auto_route.unwrap().is_empty());
    assert_eq!(detail.model, "fable/plain");
    assert_eq!(detail.auto_route, None);
    assert_eq!(detail.shell_revision, Some(crate::puppeteer_setup::revision(Some(text.as_bytes()))));
    load_project_agent_detail(&manager, Some(&path_string(&f.root)), "agent-fixture73").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
}
