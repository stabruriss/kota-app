use crate::*;

#[test]
fn six_terminal_adapters_match_frozen_baseline() {
    let root = PathBuf::from("/Users/example/Kota/Workspaces/fable-project");
    let cwd = root.join(".agent-workspaces/agent-fable-copper");
    let ctx = IncarnationContext {
        project_root: root.clone(), source_dir: root.join("source"), cwd: cwd.clone(),
        worktree_root: cwd.join("project-files"), shared_dir: root.join("project-memory"),
        rules_dir: root.join("project-rules"), project_id: Some("fable-project".into()),
        project_remote: None, project_base_ref: "HEAD".into(),
    };
    let request = TavernIncarnateHeroRequest {
        agent_id: "agent-fable-copper".into(), template_id: "Hero-FABLE123".into(), display_name: "Copper v. Fable".into(),
        name_fields: None, project_root: Some(path_string(&root)), progress_id: None,
        profile: TavernHeroProfileDraft { hero_id: "Hero-FABLE123".into(), name: "Copper".into(), name_fields: None,
            provider: "codex".into(), model: "default".into(), effort: None, auto_route: None, avatar_id: None, skills: Vec::new(),
            ghost: "Keep evidence and preserve unrelated work.".into(), shell: "provider: codex\n".into(),
            archived: false, dismissed: false, kind: None, source: None },
    };
    let inputs = AdapterInputs {
        account_user: AccountUserIdentity { name: "Alex".into(), avatar_id: None },
        teammates: vec![TeammateInfo { agent_id: "agent-fable-silver".into(), display_name: "Silver v. Fable".into(), aka: "Silver".into() }],
        account_rules: vec![RuleFile { title: "Fixture rule".into(), file_name: "fixture-rule.md".into(),
            content: "Preserve the original input.".into(), always: true, task_trigger: None }],
        project_rules: vec![RuleFile { title: "Fixture build".into(), file_name: "fixture-build.md".into(),
            content: "Use the isolated fixture.".into(), always: false, task_trigger: Some("build".into()) }],
    };
    let baseline = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/local_rpc/fixtures/adapters");
    for cli in [pty::agent::AgentCli::Claude, pty::agent::AgentCli::Codex, pty::agent::AgentCli::Antigravity,
        pty::agent::AgentCli::Opencode, pty::agent::AgentCli::Pi, pty::agent::AgentCli::Kimi, pty::agent::AgentCli::KotaLocal] {
        let text = compile_provider_adapter_with_inputs(&request, &ctx, cli, &inputs).unwrap();
        assert_eq!(text.as_bytes(), fs::read(baseline.join(format!("{}.md", shell_name_for_cli(cli)))).unwrap(),
            "terminal adapter changed for {}", shell_name_for_cli(cli));
    }
}
