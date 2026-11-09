use super::*;
use std::sync::Arc;

struct Board {
    dir: PathBuf,
    store: sync::ContentStore,
}
impl Board {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("bbs-view-cache-{}", Uuid::new_v4()));
        let store = sync::ContentStore::at(dir.join("Workspaces/bbs"), dir.join("account"));
        store.prepare_layout().unwrap();
        Self { dir, store }
    }
    fn thread(&self) -> String {
        let identity = Identity {
            project_id: "source".into(),
            project_display_name: "Source".into(),
            agent_id: "author".into(),
            agent_display_name: "Author".into(),
            agent_avatar: None,
        };
        create_thread_scoped(&self.store, &identity, vec![], true, "body".into(), &[]).unwrap()
    }
    fn key(&self, project: &str) -> ProjectKey {
        (
            self.store.root().into(),
            self.store.account_dir().into(),
            project.into(),
        )
    }
    fn view(&self, project: &str) -> BbsSnapshot {
        load(&self.store, project, None).unwrap()
    }
    fn thread_cached(&self, thread: &str) -> cache::Cached<(BbsThreadMeta, Vec<LoadedPost>)> {
        THREADS
            .peek(&(self.store.root().into(), thread.into()))
            .unwrap()
    }
    fn sides_cached(&self, project: &str, thread: &str) -> cache::Cached<Sides> {
        SIDES.peek(&(self.key(project), thread.into())).unwrap()
    }
}
impl Drop for Board {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn unchanged_snapshot_hits_all_units_and_project_state_and_display_names_stay_separate() {
    let b = Board::new();
    let thread = b.thread();
    let first = b.view("target");
    let inputs = INPUTS.peek(&b.key("target")).unwrap();
    let raw = b.thread_cached(&thread);
    let sides = b.sides_cached("target", &thread);
    let again = b.view("target");
    assert_eq!(
        serde_json::to_value(&again).unwrap(),
        serde_json::to_value(&first).unwrap()
    );
    assert!(Arc::ptr_eq(
        &inputs.value,
        &INPUTS.peek(&b.key("target")).unwrap().value
    ));
    assert!(Arc::ptr_eq(&raw.value, &b.thread_cached(&thread).value));
    assert!(Arc::ptr_eq(
        &sides.value,
        &b.sides_cached("target", &thread).value
    ));
    let renamed = load(&b.store, "target", Some("Renamed")).unwrap();
    assert_eq!(renamed.project_display_name, "Renamed");
    let mut state = load_project_state(b.store.root(), "target").unwrap();
    state.processed_posts.insert(
        first.threads[0].posts[0].post_id.clone(),
        BbsSeenPost {
            thread_id: thread.clone(),
            at: now_iso(),
        },
    );
    save_project_state(b.store.root(), &state).unwrap();
    assert_eq!(b.view("target").threads[0].posts[0].state, "processed");
    assert!(
        Arc::ptr_eq(&sides.value, &b.sides_cached("target", &thread).value),
        "project read flags do not reread attachment/avatar projection"
    );
    assert_eq!(b.view("another").threads[0].posts[0].state, "new");
    assert!(Arc::ptr_eq(&raw.value, &b.thread_cached(&thread).value));
}

#[test]
fn external_post_replacement_rebuilds_only_its_thread_and_invalid_yaml_is_not_masked() {
    let b = Board::new();
    let a = b.thread();
    let other = b.thread();
    b.view("target");
    let raw = b.thread_cached(&a);
    let unchanged = b.thread_cached(&other);
    let post = &raw.value.1[0];
    let before_hash = post.version_id.clone();
    write_bytes_atomic(
        &post.path,
        serialized_post(&post.meta, "new body").unwrap().as_bytes(),
    )
    .unwrap();
    let changed = b.view("target");
    let changed_post = &changed
        .threads
        .iter()
        .find(|t| t.thread_id == a)
        .unwrap()
        .posts[0];
    assert_eq!(changed_post.body, "new body");
    assert_ne!(changed_post.version_id, before_hash);
    assert!(!Arc::ptr_eq(&raw.value, &b.thread_cached(&a).value));
    assert!(Arc::ptr_eq(
        &unchanged.value,
        &b.thread_cached(&other).value
    ));
    fs::write(&post.path, "invalid frontmatter").unwrap();
    assert!(!b.view("target").threads.iter().any(|t| t.thread_id == a));
    write_bytes_atomic(
        &post.path,
        serialized_post(&post.meta, "repaired").unwrap().as_bytes(),
    )
    .unwrap();
    assert!(b
        .view("target")
        .threads
        .iter()
        .any(|t| t.thread_id == a && t.posts[0].body == "repaired"));
}

#[test]
fn thread_metadata_and_legacy_avatar_dependencies_are_recorded_by_the_loader() {
    let b = Board::new();
    let thread = b.thread();
    let first = b.view("target");
    assert!(first.threads[0].posts[0].agent_avatar.is_none());
    let raw = b.thread_cached(&thread);
    let path = b
        .dir
        .join("Workspaces/source/.agent-workspaces/author/agent.yaml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "avatar-id: codex\n").unwrap();
    assert_eq!(
        b.view("target").threads[0].posts[0].agent_avatar.as_deref(),
        Some("codex")
    );
    assert!(Arc::ptr_eq(&raw.value, &b.thread_cached(&thread).value));
    fs::write(&path, "avatar-id: claude\n").unwrap();
    assert_eq!(
        b.view("target").threads[0].posts[0].agent_avatar.as_deref(),
        Some("claude")
    );
    let meta_path = thread_dir(b.store.root(), &thread).join("thread.yaml");
    let mut meta = read_thread_meta(&meta_path).unwrap();
    meta.visibility = "targeted".into();
    meta.project_tags = vec!["specific-project".into()];
    write_thread_meta(&meta_path, &meta).unwrap();
    let next = b.view("target");
    assert_eq!(next.threads[0].visibility, "targeted");
    assert_eq!(next.threads[0].project_tags, ["specific-project"]);
    assert!(!Arc::ptr_eq(&raw.value, &b.thread_cached(&thread).value));
}

#[test]
fn cached_thread_cannot_bypass_managed_path_symlink_rejection() {
    let b = Board::new();
    let thread = b.thread();
    assert_eq!(b.view("target").threads.len(), 1);
    let dir = thread_dir(b.store.root(), &thread);
    let posts = dir.join("posts");
    let moved = dir.join("moved-posts");
    fs::rename(&posts, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &posts).unwrap();
    assert!(b.view("target").threads.is_empty());
    fs::remove_file(&posts).unwrap();
    fs::rename(moved, posts).unwrap();
    assert_eq!(b.view("target").threads.len(), 1);
}
