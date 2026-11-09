use super::*;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("kota-agent-notes-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn agent(&self, project: &str, id: &str, status: &str) -> PathBuf {
        let cwd = self.0.join(project).join(".agent-workspaces").join(id);
        fs::create_dir_all(&cwd).unwrap();
        fs::write(cwd.join("agent.yaml"), format!("id: {id}\nstatus: {status}\nrecruited-from: source-hero\n")).unwrap();
        cwd
    }
    fn notes(&self, cwd: &Path, text: &str) -> PathBuf {
        let path = soul::SoulPaths::new(cwd).marks();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

#[test]
fn notes_status_is_read_only_and_empty_or_unparseable_files_are_still_openable() {
    let f = Fixture::new();
    let cwd = f.agent("one", "agent-one", "active");
    let status = project_agent_notes(&cwd);
    assert!(!status.exists);
    assert!(!cwd.join("under-table").exists());
    for content in [r#"{"version":1,"revision":0,"marks":[]}"#, "broken JSON"] {
        let path = f.notes(&cwd, content);
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        let status = serde_json::to_value(project_agent_notes(&cwd)).unwrap();
        assert_eq!(status["exists"], true);
        assert_eq!(status["path"], path_string(&path));
        assert_eq!(project_agent_notes_path(&cwd).unwrap(), path);
        assert_eq!(fs::read_to_string(&path).unwrap(), content);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
        assert!(!cwd.join("under-table/soul/.lock").exists());
    }
}

#[test]
fn notes_target_is_the_registered_incarnation_even_if_archived_or_other_project_is_active() {
    let f = Fixture::new();
    let a = f.agent("one", "agent-one", "archived");
    let b = f.agent("two", "agent-one", "active");
    let c = f.agent("one", "agent-two", "active");
    let paths = [f.notes(&a, "archived"), f.notes(&b, "other project"), f.notes(&c, "other agent")];
    f.notes(&f.0.join("heroes/source-hero"), "source hero");
    assert_eq!(project_agent_notes_path(&a).unwrap(), paths[0]);
    assert_eq!(project_agent_notes_path(&b).unwrap(), paths[1]);
    assert_eq!(project_agent_notes_path(&c).unwrap(), paths[2]);
}

#[test]
fn notes_open_rechecks_file_presence() {
    let f = Fixture::new();
    let cwd = f.agent("one", "agent-one", "active");
    let path = f.notes(&cwd, "notes");
    assert!(project_agent_notes(&cwd).exists);
    fs::remove_file(path).unwrap();
    assert_eq!(project_agent_notes_path(&cwd).unwrap_err(), "Notes file not found.");
}
