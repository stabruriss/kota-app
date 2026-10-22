use super::configuration::agent_cwd_with;
use crate::integrations::WorkspaceProject;
use serde_json::json;
use std::{cell::Cell, fs, path::{Path, PathBuf}};

struct Projects {
    root: PathBuf,
}

impl Projects {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("kota-agent-cwd-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn project(&self, id: &str, archived: bool) -> WorkspaceProject {
        let local_root = self.root.join(id);
        fs::create_dir_all(local_root.join(".agent-workspaces")).unwrap();
        serde_json::from_value(json!({
            "projectId": id, "repoFullName": "", "remoteUrl": "", "defaultBranch": "main", "baseRef": "main",
            "localRoot": local_root.to_string_lossy(), "sourceDir": local_root.to_string_lossy(),
            "sharedDir": local_root.join("project-memory").to_string_lossy(),
            "rulesDir": local_root.join("project-rules").to_string_lossy(),
            "agents": [], "archived": archived,
        }))
        .unwrap()
    }
}

impl Drop for Projects {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn counted(list: Vec<WorkspaceProject>, calls: &Cell<usize>) -> impl FnOnce() -> super::error::LocalResult<Vec<WorkspaceProject>> + '_ {
    move || {
        calls.set(calls.get() + 1);
        Ok(list)
    }
}

#[test]
fn displayed_project_keeps_its_directory_without_listing_the_others() {
    let projects = Projects::new("displayed");
    let active = projects.project("shown", false);
    let calls = Cell::new(0);
    let cwd = agent_cwd_with(Path::new(&active.local_root), "agent-0123456789", Some(&active),
        counted(vec![projects.project("other", false)], &calls)).unwrap();
    assert_eq!(cwd, PathBuf::from(&active.local_root).join(".agent-workspaces").join("agent-0123456789"));
    assert_eq!(calls.get(), 0, "the displayed project must not trigger a registry scan");
}

#[test]
fn registered_undisplayed_project_resolves_to_its_own_agent_directory() {
    let projects = Projects::new("undisplayed");
    let active = projects.project("shown", false);
    let other = projects.project("other", false);
    let calls = Cell::new(0);
    let cwd = agent_cwd_with(Path::new(&other.local_root), "agent-abcdef0123", Some(&active),
        counted(vec![active.clone(), other.clone()], &calls)).unwrap();
    assert_eq!(cwd, Path::new(&other.local_root).canonicalize().unwrap().join(".agent-workspaces").join("agent-abcdef0123"));
    assert_eq!(calls.get(), 1);
    let calls = Cell::new(0);
    let without_active = agent_cwd_with(Path::new(&other.local_root), "agent-abcdef0123", None,
        counted(vec![other.clone()], &calls)).unwrap();
    assert_eq!(without_active, cwd);
}

#[test]
fn unregistered_archived_relative_missing_or_malformed_requests_are_refused() {
    let projects = Projects::new("refused");
    let active = projects.project("shown", false);
    let archived = projects.project("gone", true);
    let stranger = projects.project("stranger", false);
    let calls = Cell::new(0);
    let list = || counted(vec![active.clone(), archived.clone()], &calls);
    let refused = |root: &Path, agent: &str| {
        agent_cwd_with(root, agent, Some(&active), list()).err().map(|e| e.to_string())
    };
    assert_eq!(refused(Path::new(&stranger.local_root), "agent-0123456789"), Some("invalid_request".into()));
    assert_eq!(refused(Path::new(&archived.local_root), "agent-0123456789"), Some("invalid_request".into()));
    assert_eq!(refused(&projects.root.join("absent"), "agent-0123456789"), Some("invalid_request".into()));
    assert_eq!(refused(Path::new("Kota/Workspaces/shown"), "agent-0123456789"), Some("invalid_request".into()));
    assert_eq!(refused(Path::new(&active.local_root), ""), Some("invalid_request".into()));
    assert_eq!(refused(Path::new(&active.local_root), "agent/../x"), Some("invalid_request".into()));
    // An archived project that is somehow still the active entry is refused too.
    assert_eq!(agent_cwd_with(Path::new(&archived.local_root), "agent-0123456789", Some(&archived), list())
        .err().map(|e| e.to_string()), Some("invalid_request".into()));
}
