use crate::*;
use crate::pty::agent::{AgentCli, AgentSpawnRequest};
use crate::shell_switch::{Launch, PromptSource};

struct Fixture { root: PathBuf, cwd: PathBuf }
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("local-launch-{}", Uuid::new_v4()));
        let cwd = root.join(".agent-workspaces/agent-fable-copper");
        fs::create_dir_all(cwd.join("project-files/.git")).unwrap();
        fs::create_dir_all(root.join("project-memory/chathistory")).unwrap();
        fs::write(cwd.join("agent.yaml"), "id: agent-fable-copper\nstatus: active\n").unwrap();
        let f = Self { root, cwd }; f.target("kota-local", "default", "default"); f
    }
    fn target(&self, provider: &str, model: &str, effort: &str) {
        fs::write(self.cwd.join("SHELL.yaml"), format!("provider: {provider}\nmodel: {model}\neffort: {effort}\nargs: []\n")).unwrap();
    }
    fn request(&self, fresh: bool) -> AgentSpawnRequest {
        serde_json::from_value(serde_json::json!({"agentId":"agent-fable-copper", "cli":"kota-local",
            "projectRoot":self.root, "projectId":"fable-local", "cwd":self.cwd, "freshSession":fresh})).unwrap()
    }
    fn yaml(&self) -> serde_yaml::Mapping { read_yaml_mapping(&self.cwd.join("agent.yaml")).unwrap() }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let root = self.root.clone();
        let _ = adapter_sync::call(move || { adapter_sync::take_project(&root); Ok(()) });
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn local_session_identity_is_committed_only_after_start_and_reloads_saved_model() {
    let f = Fixture::new();
    let before = fs::read(f.cwd.join("agent.yaml")).unwrap();
    let mut request = f.request(false);
    let launch = Launch::prepare(&mut request, None).unwrap().unwrap();
    assert_eq!(request.cli, AgentCli::KotaLocal);
    assert!(request.args.is_empty()); assert!(request.session_id.is_none());
    drop(launch); // Failed/cancelled startup cannot publish an effective session.
    assert_eq!(fs::read(f.cwd.join("agent.yaml")).unwrap(), before);
    let mut launch = Launch::prepare(&mut request, None).unwrap().unwrap();
    let session = "11111111-1111-4111-8111-111111111111";
    launch.commit(Some(session)).unwrap(); drop(launch);
    assert_eq!(yaml_string(&f.yaml(), "session-id").as_deref(), Some(session));
    assert_eq!(shell_switch::effective_provider(&f.yaml()).unwrap(), Some(AgentCli::KotaLocal));
    assert!(yaml_value::<shell_switch::Handoff>(&f.yaml(), "handoff-pending").is_none());
    f.target("kota-local", "kota-fake/fixture", "off");
    let mut stale_request = f.request(false);
    let launch = Launch::prepare(&mut stale_request, None).unwrap().unwrap();
    assert_eq!(stale_request.session_id.as_deref(), Some(session));
    assert_eq!(stale_request.args, ["--model", "kota-fake/fixture", "--thinking", "off"]);
    drop(launch);
    let mut fresh = f.request(true);
    let mut launch = Launch::prepare(&mut fresh, Some(AgentCli::KotaLocal)).unwrap().unwrap();
    let new_id = fresh.session_id.as_deref().unwrap();
    assert_ne!(new_id, session); assert!(Uuid::parse_str(new_id).is_ok());
    launch.commit(Some(new_id)).unwrap();
    assert!(yaml_value::<shell_switch::Handoff>(&f.yaml(), "handoff-pending").is_none());
}

#[test]
fn terminal_to_local_and_back_uses_existing_handoff_and_target_pinning() {
    for cli in [AgentCli::Claude, AgentCli::Codex, AgentCli::Antigravity, AgentCli::Opencode, AgentCli::Pi, AgentCli::Kimi] {
        let f = Fixture::new(); let provider = shell_name_for_cli(cli);
        fs::write(f.cwd.join("agent.yaml"), format!("id: agent-fable-copper\nstatus: active\nprovider: {provider}\nshell: {provider}\nsession-id: previous-session\n")).unwrap();
        let mut request = f.request(false);
        let mut launch = Launch::prepare(&mut request, Some(cli)).unwrap().unwrap();
        let local_session = request.session_id.as_deref().unwrap();
        assert!(Uuid::parse_str(local_session).is_ok());
        // A later Save stays the next target; finishing this launch must not erase it.
        f.target(provider, "default", "default");
        launch.commit(Some(local_session)).unwrap(); drop(launch);
        assert_eq!(shell_switch::effective_provider(&f.yaml()).unwrap(), Some(AgentCli::KotaLocal));
        let (text, handoff) = shell_switch::compose(&f.cwd, AgentCli::KotaLocal, "Original input".into(), PromptSource::Composer).unwrap();
        assert!(text.contains(&format!("from {provider} to kota-local")));
        assert!(text.ends_with("Original input"));
        shell_switch::submitted(&f.cwd, &handoff.unwrap()).unwrap();
        assert_eq!(shell_switch::compose(&f.cwd, AgentCli::KotaLocal, "Second input".into(), PromptSource::Composer).unwrap().0, "Second input");
        let mut back = f.request(false);
        let mut launch = Launch::prepare(&mut back, Some(AgentCli::KotaLocal)).unwrap().unwrap();
        assert_eq!(back.cli, cli);
        launch.commit(back.session_id.as_deref()).unwrap();
        let pending: shell_switch::Handoff = yaml_value(&f.yaml(), "handoff-pending").unwrap();
        assert!(pending.text.contains(&format!("from kota-local to {provider}")));
    }
}
