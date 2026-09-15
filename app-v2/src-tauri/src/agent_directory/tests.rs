use super::*;
use serde_json::json;

pub(crate) struct Account(pub(crate) PathBuf);
impl Account {
    pub(crate) fn new() -> Self {
        let path = std::env::temp_dir().join(format!("bbs-directory-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    pub(crate) fn project(&self, id: &str, archived: bool) -> PathBuf {
        let root = self.0.join("Workspaces").join(id);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("workspace.json"), serde_json::to_vec(&json!({
            "projectId":id,"repoFullName":"org/Same Name","archived":archived,
            "localRoot":"/private/never-used","agents":[{"agentId":"stale", "cwd":"/private/no"}],
            "secret":"not-public"
        })).unwrap()).unwrap();
        root
    }
    pub(crate) fn agent(&self, project: &str, id: &str, yaml: &str) {
        let cwd = self
            .0
            .join("Workspaces")
            .join(project)
            .join(".agent-workspaces")
            .join(id);
        fs::create_dir_all(&cwd).unwrap();
        fs::write(cwd.join("agent.yaml"), yaml).unwrap();
    }
}
impl Drop for Account {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn collector_is_read_only_and_excludes_only_inactive_registrations() {
    let account = Account::new();
    assert!(collect(&account.0).unwrap().complete().unwrap().is_empty());
    assert_eq!(fs::read_dir(&account.0).unwrap().count(), 0);
    let p = account.project("project-a", false);
    let before = fs::read(p.join("workspace.json")).unwrap();
    account.project("project-b", true);
    account.agent(
        "project-a",
        "agent-a",
        "displayName: Same\nstatus: active\nsession-id: private\n",
    );
    account.agent("project-a", "agent-b", "display-name: Same\nstatus: idle\n");
    account.agent(
        "project-a",
        "agent-closed",
        "display-name: Gone\nstatus: ARCHIVED\n",
    );
    account.agent("project-b", "agent-a", "display-name: Hidden\n");
    let directory = collect(&account.0).unwrap().complete().unwrap();
    assert_eq!(directory.len(), 1);
    assert_eq!(directory[0].agents.len(), 2);
    assert_eq!(directory[0].agents[0].name, "Same");
    assert_eq!(directory[0].root, p);
    assert_eq!(fs::read(p.join("workspace.json")).unwrap(), before);
    assert!(!account.0.join("bbs-sync").exists());
    assert!(!p.join("project-memory").exists());
    for status in ["archived", "deleted", "dismissed", "removed"] {
        assert!(!visible(status));
    }
    for status in ["active", "offline", "idle", "", "unknown"] {
        assert!(visible(status));
    }
}

#[test]
fn collector_has_no_agent_total_cap_and_detects_rename_removal_and_bad_partial_input() {
    let account = Account::new();
    account.project("project-a", false);
    for i in 0..130 {
        account.agent(
            "project-a",
            &format!("a-{i:03}"),
            "display-name: Original\n",
        );
    }
    assert_eq!(
        collect(&account.0).unwrap().complete().unwrap()[0]
            .agents
            .len(),
        130
    );
    account.agent("project-a", "a-129", "display-name: New name\n");
    assert_eq!(
        collect(&account.0).unwrap().complete().unwrap()[0].agents[129].name,
        "New name"
    );
    fs::remove_dir_all(
        account
            .0
            .join("Workspaces/project-a/.agent-workspaces/a-129"),
    )
    .unwrap();
    assert_eq!(
        collect(&account.0).unwrap().complete().unwrap()[0]
            .agents
            .len(),
        129
    );
    account.agent("project-a", "a-001", "[invalid yaml");
    assert!(collect(&account.0).unwrap().complete().is_err());
    account.agent("project-a", "a-001", "display-name: Recovered\n");
    assert_eq!(
        collect(&account.0).unwrap().complete().unwrap()[0]
            .agents
            .len(),
        129
    );
}
