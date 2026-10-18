use super::*;
use std::sync::atomic::AtomicUsize;

static PROBES: OnceLock<Mutex<HashMap<PathBuf, Arc<AtomicUsize>>>> = OnceLock::new();
pub(super) fn record_git_read(path: &Path) {
    if let Some(probes) = PROBES.get() {
        for (root, count) in probes.lock().unwrap().iter() {
            if path.starts_with(root) {
                count.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}
struct Probe {
    root: PathBuf,
    count: Arc<AtomicUsize>,
}
impl Probe {
    fn new(root: &Path) -> Self {
        let count = Arc::new(AtomicUsize::new(0));
        PROBES
            .get_or_init(Mutex::default)
            .lock()
            .unwrap()
            .insert(root.to_owned(), count.clone());
        Self {
            root: root.to_owned(),
            count,
        }
    }
    fn take(&self) -> usize {
        self.count.swap(0, Ordering::Relaxed)
    }
}
impl Drop for Probe {
    fn drop(&mut self) {
        PROBES.get().unwrap().lock().unwrap().remove(&self.root);
    }
}

struct Fixture {
    base: PathBuf,
    workspace: integrations::WorkspaceProject,
}
impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!("kota-name-index-{}", uuid::Uuid::new_v4()));
        let source = base.join("source");
        let local = base.join("workspace");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&local).unwrap();
        let workspace = integrations::WorkspaceProject {
            project_id: format!("fixture-{}", uuid::Uuid::new_v4()),
            repo_full_name: "fixture/example".into(),
            remote_url: String::new(),
            github_html_url: String::new(),
            default_branch: "main".into(),
            base_ref: "main".into(),
            local_root: local.display().to_string(),
            local_root_bytes: 0,
            source_dir: source.display().to_string(),
            source_dir_bytes: 0,
            shared_dir: local.join("project-memory").display().to_string(),
            rules_dir: local.join("rules").display().to_string(),
            agents: vec![],
            archived: false,
            archived_at: None,
        };
        Self { base, workspace }
    }
    fn source(&self) -> PathBuf {
        PathBuf::from(&self.workspace.source_dir)
    }
    fn local(&self) -> PathBuf {
        PathBuf::from(&self.workspace.local_root)
    }
    fn put(root: &Path, relative: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "fixture\n").unwrap();
    }
    fn git(root: &Path, args: &[&str]) {
        let out = Command::new("git")
            .current_dir(root)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgSign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    fn init_git(&self) {
        Self::git(&self.source(), &["init", "-b", "main"]);
        Self::put(&self.source(), "README.md");
        self.commit_source();
    }
    fn commit_source(&self) {
        Self::git(&self.source(), &["add", "-A"]);
        Self::git(
            &self.source(),
            &["commit", "--allow-empty", "-m", "fixture"],
        );
    }
    fn agent(&mut self) -> PathBuf {
        let cwd = self.local().join(".agent-workspaces/worker-amber");
        fs::create_dir_all(&cwd).unwrap();
        fs::write(
            cwd.join("agent.yaml"),
            "status: active\ndisplay_name: Amber Worker\naka: Amber\n",
        )
        .unwrap();
        let tree = cwd.join("project-files");
        Self::git(
            &self.source(),
            &[
                "worktree",
                "add",
                "-b",
                "fixture-worker",
                tree.to_str().unwrap(),
            ],
        );
        self.workspace.agents.push(integrations::AgentLaunchSpec {
            agent_id: "worker-amber".into(),
            cli: crate::pty::agent::AgentCli::Codex.into(),
            cwd: cwd.display().to_string(),
            project_root: self.workspace.local_root.clone(),
            worktree_root: tree.display().to_string(),
            shared_dir: self.workspace.shared_dir.clone(),
            rules_dir: self.workspace.rules_dir.clone(),
            adapter_path: cwd.join("AGENTS.md").display().to_string(),
            project_id: self.workspace.project_id.clone(),
            project_remote: String::new(),
            project_base_ref: "main".into(),
        });
        tree
    }
    fn request(&self, kind: WorkspaceTreeRootKind, query: &str) -> SearchRequest {
        SearchRequest {
            project_id: self.workspace.project_id.clone(),
            root_kind: kind,
            include_ignored: false,
            generation: 1,
            query_revision: 1,
            query: query.into(),
            cancel: false,
        }
    }
    fn build(&self, kind: WorkspaceTreeRootKind, ignored: bool, limit: usize) -> Catalogue {
        Catalogue::build(
            &self.workspace,
            kind,
            ignored,
            &AtomicBool::new(false),
            limit,
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}
fn paths(response: &SearchResponse) -> BTreeSet<&str> {
    response.entries.iter().map(|e| e.path.as_str()).collect()
}
fn query(catalogue: &Catalogue, request: &SearchRequest) -> SearchResponse {
    catalogue
        .query(request, &AtomicBool::new(false), MATCH_LIMIT)
        .unwrap()
}

#[test]
fn include_ignored_hidden_and_symlink_rules_are_native_and_explicit() {
    let f = Fixture::new();
    f.init_git();
    fs::write(f.source().join(".gitignore"), "node_modules/\ngenerated/\n").unwrap();
    Fixture::put(&f.source(), ".ordinary/HiddenNeedle.rs");
    Fixture::put(&f.source(), "node_modules/Needle.rs");
    Fixture::put(&f.source(), "generated/Needle.rs");
    Fixture::put(&f.base, "outside/TargetNeedle.rs");
    #[cfg(unix)]
    std::os::unix::fs::symlink(f.base.join("outside"), f.source().join("LinkedNeedle")).unwrap();
    let request = f.request(WorkspaceTreeRootKind::ProjectFiles, "  nEeDlE  ");
    let filtered = query(&f.build(request.root_kind, false, INDEX_LIMIT), &request);
    assert!(paths(&filtered).contains(".ordinary/HiddenNeedle.rs"));
    assert!(!paths(&filtered).contains("node_modules/Needle.rs"));
    assert!(!paths(&filtered).iter().any(|p| p.contains("TargetNeedle")));
    assert!(!filtered.git_failed && !filtered.read_failed && !filtered.index_truncated);
    let all = query(&f.build(request.root_kind, true, INDEX_LIMIT), &request);
    assert!(paths(&all).contains("node_modules/Needle.rs"));
    assert!(paths(&all).contains("generated/Needle.rs"));
    #[cfg(unix)]
    assert_eq!(
        all.entries
            .iter()
            .find(|e| e.path == "LinkedNeedle")
            .unwrap()
            .kind,
        "symlink"
    );
}

#[test]
fn matching_is_basename_literal_and_directory_matches_do_not_admit_children() {
    let f = Fixture::new();
    Fixture::put(&f.local(), "NeedleFolder/other.rs");
    Fixture::put(&f.local(), "src/[draft].md");
    Fixture::put(&f.local(), "src/规划.md");
    let index = f.build(WorkspaceTreeRootKind::ProjectWorkspace, true, INDEX_LIMIT);
    assert_eq!(
        paths(&query(&index, &f.request(index.root.kind, "needle"))),
        BTreeSet::from(["NeedleFolder"])
    );
    assert_eq!(
        query(&index, &f.request(index.root.kind, "NeedleFolder/other")).matched_count,
        0
    );
    assert_eq!(
        query(&index, &f.request(index.root.kind, ".*")).matched_count,
        0
    );
    assert_eq!(
        query(&index, &f.request(index.root.kind, "[draft]")).matched_count,
        1
    );
    assert_eq!(
        query(&index, &f.request(index.root.kind, "规划")).matched_count,
        1
    );
}

#[test]
fn catalogue_cap_distinguishes_exact_boundary_and_overflow() {
    let f = Fixture::new();
    Fixture::put(&f.local(), "one");
    Fixture::put(&f.local(), "two");
    let index = f.build(WorkspaceTreeRootKind::ProjectWorkspace, true, 2);
    assert_eq!(index.entries.len(), 2);
    assert!(!index.index_truncated);
    Fixture::put(&f.local(), "three");
    let limited = f.build(WorkspaceTreeRootKind::ProjectWorkspace, true, 2);
    assert_eq!(limited.entries.len(), 2);
    assert!(limited.index_truncated);
    assert_eq!(INDEX_LIMIT, 200_000);
    assert_eq!(MATCH_LIMIT, 2_000);
}

#[test]
fn more_than_3000_directories_are_searchable_without_a_directory_budget() {
    let f = Fixture::new();
    for n in 0..3010 {
        fs::create_dir(f.local().join(format!("dir-{n:04}"))).unwrap();
    }
    Fixture::put(&f.local(), "dir-3009/Needle.rs");
    let index = f.build(WorkspaceTreeRootKind::ProjectWorkspace, false, INDEX_LIMIT);
    let response = query(&index, &f.request(index.root.kind, "needle"));
    assert_eq!(index.entries.len(), 3011);
    assert!(!response.index_truncated && !response.truncated);
    assert!(paths(&response).contains("dir-3009/Needle.rs"));
}

#[test]
fn match_cap_is_separate_from_catalogue_cap_and_includes_ancestors() {
    let f = Fixture::new();
    for n in 0..2001 {
        Fixture::put(&f.local(), &format!("src/Needle-{n:04}.rs"));
    }
    let index = f.build(WorkspaceTreeRootKind::ProjectWorkspace, true, INDEX_LIMIT);
    let response = query(&index, &f.request(index.root.kind, "needle"));
    assert_eq!(response.matched_count, 2000);
    assert_eq!(response.entries.len(), 2001);
    assert!(paths(&response).contains("src"));
    assert!(response.truncated);
    assert!(!response.index_truncated);
    assert_eq!(response.indexed_count, 2002);
    println!(
        "NAME_SEARCH_MATCH_CAP_PAYLOAD_BYTES={}",
        serde_json::to_vec(&response).unwrap().len()
    );
    let exact = index
        .query(
            &f.request(index.root.kind, "needle"),
            &AtomicBool::new(false),
            2001,
        )
        .unwrap();
    assert_eq!(exact.matched_count, 2001);
    assert!(!exact.truncated);
}

#[cfg(unix)]
#[test]
fn read_failures_keep_partial_names_and_do_not_claim_a_complete_root() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    Fixture::put(&f.local(), "denied/Needle.rs");
    Fixture::put(&f.local(), "VisibleNeedle.rs");
    let denied = f.local().join("denied");
    let permissions = fs::metadata(&denied).unwrap().permissions();
    fs::set_permissions(&denied, fs::Permissions::from_mode(0)).unwrap();
    let built = Catalogue::build(
        &f.workspace,
        WorkspaceTreeRootKind::ProjectWorkspace,
        true,
        &AtomicBool::new(false),
        INDEX_LIMIT,
    );
    fs::set_permissions(&denied, permissions).unwrap();
    let index = built.unwrap();
    assert!(index.read_failed);
    assert!(
        paths(&query(&index, &f.request(index.root.kind, "needle"))).contains("VisibleNeedle.rs")
    );
    let permissions = fs::metadata(f.local()).unwrap().permissions();
    fs::set_permissions(f.local(), fs::Permissions::from_mode(0)).unwrap();
    let built = Catalogue::build(
        &f.workspace,
        index.root.kind,
        true,
        &AtomicBool::new(false),
        INDEX_LIMIT,
    );
    fs::set_permissions(f.local(), permissions).unwrap();
    assert!(built.unwrap().read_failed);
}

#[test]
fn committed_agent_only_files_are_ghosts_and_archived_agents_are_excluded() {
    let mut f = Fixture::new();
    f.init_git();
    let tree = f.agent();
    Fixture::put(&tree, "virtual/deep/GhostNeedle.rs");
    Fixture::git(&tree, &["add", "-A"]);
    Fixture::git(&tree, &["commit", "-m", "agent-only committed"]);
    let request = f.request(WorkspaceTreeRootKind::ProjectFiles, "ghostneedle");
    let response = query(&f.build(request.root_kind, false, INDEX_LIMIT), &request);
    assert_eq!(
        paths(&response),
        BTreeSet::from(["virtual", "virtual/deep", "virtual/deep/GhostNeedle.rs"])
    );
    assert!(response
        .entries
        .iter()
        .all(|e| e.is_ghost && e.tree_has_changes));
    let file = response.entries.iter().find(|e| e.kind == "file").unwrap();
    assert_eq!(
        file.file_change.as_ref().unwrap().participants[0].actor_id,
        "worker-amber"
    );
    assert_eq!(file.file_change.as_ref().unwrap().status, "added");
    fs::write(
        Path::new(&f.workspace.agents[0].cwd).join("agent.yaml"),
        "status: archived\n",
    )
    .unwrap();
    assert_eq!(
        query(&f.build(request.root_kind, false, INDEX_LIMIT), &request).matched_count,
        0
    );
}

#[test]
fn git_overlay_is_deduplicated_and_ignored_tracked_changes_remain_explicit_supplements() {
    let f = Fixture::new();
    f.init_git();
    Fixture::put(&f.source(), "generated/Needle.rs");
    f.commit_source();
    fs::write(f.source().join(".gitignore"), "generated/\n").unwrap();
    fs::write(f.source().join("generated/Needle.rs"), "changed\n").unwrap();
    let index = f.build(WorkspaceTreeRootKind::ProjectFiles, false, INDEX_LIMIT);
    let response = query(&index, &f.request(index.root.kind, "needle"));
    assert_eq!(response.matched_count, 1);
    assert_eq!(response.entries.len(), 2);
    let file = response.entries.iter().find(|e| e.kind == "file").unwrap();
    assert!(!file.is_ghost);
    assert!(file.file_change.is_some());
}

#[test]
fn git_failure_is_explicit_while_physical_partial_results_survive() {
    let mut f = Fixture::new();
    f.init_git();
    let tree = f.agent();
    Fixture::put(&f.source(), "PhysicalNeedle.rs");
    fs::write(tree.join(".git"), "gitdir: missing-repository\n").unwrap();
    let index = f.build(WorkspaceTreeRootKind::ProjectFiles, false, INDEX_LIMIT);
    let response = query(&index, &f.request(index.root.kind, "needle"));
    assert!(response.git_failed);
    assert!(paths(&response).contains("PhysicalNeedle.rs"));
}

fn legacy_inventory(
    workspace: &integrations::WorkspaceProject,
    kind: WorkspaceTreeRootKind,
) -> BTreeMap<String, WorkspaceTreeEntry> {
    let mut pending = vec![String::new()];
    let mut result = BTreeMap::new();
    let mut n = 0;
    while n < pending.len() {
        let listing = crate::workspace_list_tree_path_for_workspace(
            workspace,
            crate::WorkspaceTreePathRequest {
                project_id: workspace.project_id.clone(),
                root_kind: kind,
                relative_path: Some(pending[n].clone()),
            },
        )
        .unwrap();
        n += 1;
        for entry in listing.entries {
            if entry.kind == "folder"
                && ![".git", ".hg", ".svn", "node_modules"]
                    .contains(&entry.name.to_lowercase().as_str())
            {
                pending.push(entry.path.clone());
            }
            result.insert(entry.path.clone(), entry);
        }
    }
    result
}

#[test]
fn returned_metadata_matches_existing_listing_for_both_roots_including_ghosts_and_worktrees() {
    let mut f = Fixture::new();
    f.init_git();
    let tree = f.agent();
    Fixture::put(&f.source(), "physical/Needle.rs");
    Fixture::put(&tree, "ghost/Needle.rs");
    #[cfg(unix)]
    std::os::unix::fs::symlink(f.source(), f.local().join("NeedleLink")).unwrap();
    for kind in [
        WorkspaceTreeRootKind::ProjectFiles,
        WorkspaceTreeRootKind::ProjectWorkspace,
    ] {
        let old = legacy_inventory(&f.workspace, kind);
        let index = f.build(kind, false, INDEX_LIMIT);
        let response = query(&index, &f.request(kind, "needle"));
        for entry in &response.entries {
            assert_eq!(
                serde_json::to_value(entry).unwrap(),
                serde_json::to_value(&old[&entry.path]).unwrap(),
                "{}",
                entry.path
            );
        }
        assert!(response.matched_count > 0);
        assert!(!response.git_failed);
    }
}

#[test]
fn git_process_count_depends_on_repositories_not_directory_count_and_edits_use_zero_git() {
    let mut measured = Vec::new();
    for dirs in [10, 1000] {
        let mut f = Fixture::new();
        f.init_git();
        for n in 0..dirs {
            Fixture::put(&f.source(), &format!("dir-{n:04}/Needle.rs"));
        }
        f.commit_source();
        f.agent();
        let probe = Probe::new(&f.base);
        let index = f.build(WorkspaceTreeRootKind::ProjectFiles, false, INDEX_LIMIT);
        let builds = probe.take();
        for word in ["needle", "dir-0001", "absent"] {
            let _ = query(&index, &f.request(index.root.kind, word));
        }
        assert_eq!(probe.take(), 0);
        measured.push(builds);
    }
    assert_eq!(measured[0], measured[1]);
    assert!(measured[0] > 0 && measured[0] <= 12);
    println!("NAME_SEARCH_GIT_COUNTS_10_1000={measured:?}");
}

#[tokio::test]
async fn registry_reuses_snapshot_and_clear_new_generation_rebuilds_once() {
    let f = Fixture::new();
    Fixture::put(&f.local(), "OldNeedle.rs");
    let registry = Registry::default();
    let request = f.request(WorkspaceTreeRootKind::ProjectWorkspace, "needle");
    let workspace = f.workspace.clone();
    let first = registry
        .search("main", request.clone(), move || Ok(workspace))
        .await
        .unwrap();
    assert!(paths(&first).contains("OldNeedle.rs"));
    fs::remove_file(f.local().join("OldNeedle.rs")).unwrap();
    Fixture::put(&f.local(), "NewNeedle.rs");
    let mut next = request.clone();
    next.query_revision += 1;
    next.query = "newneedle".into();
    assert_eq!(
        registry
            .search("main", next.clone(), || panic!("must not prepare again"))
            .await
            .unwrap()
            .matched_count,
        0
    );
    let mut cancel = next.clone();
    cancel.cancel = true;
    assert!(
        registry
            .search("main", cancel, || panic!())
            .await
            .unwrap()
            .cancelled
    );
    assert!(
        registry
            .search("main", request.clone(), || panic!("late begin"))
            .await
            .unwrap()
            .cancelled
    );
    next.generation += 1;
    let workspace = f.workspace.clone();
    let response = registry
        .search("main", next.clone(), move || Ok(workspace))
        .await
        .unwrap();
    assert!(paths(&response).contains("NewNeedle.rs"));
    assert!(
        registry
            .search("main", request, || panic!("old generation"))
            .await
            .unwrap()
            .cancelled
    );
    next.include_ignored = true;
    assert!(registry
        .search("main", next, || panic!())
        .await
        .unwrap_err()
        .contains("scope mismatch"));
}

#[tokio::test]
async fn simultaneous_first_queries_share_one_initializer_and_old_keyword_is_discarded() {
    let f = Fixture::new();
    Fixture::put(&f.local(), "Needle.rs");
    let registry = Arc::new(Registry::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let (send, receive) = std::sync::mpsc::channel();
    let request = f.request(WorkspaceTreeRootKind::ProjectWorkspace, "needle");
    let first_registry = registry.clone();
    let count = calls.clone();
    let first_request = request.clone();
    let workspace = f.workspace.clone();
    let first = tokio::spawn(async move {
        first_registry
            .search("main", first_request, move || {
                count.fetch_add(1, Ordering::Relaxed);
                receive.recv_timeout(Duration::from_secs(5)).unwrap();
                Ok(workspace)
            })
            .await
    });
    while calls.load(Ordering::Relaxed) == 0 {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let second_registry = registry.clone();
    let mut second_request = request.clone();
    second_request.query_revision = 2;
    let second = tokio::spawn(async move {
        second_registry
            .search("main", second_request, || panic!("duplicate build"))
            .await
    });
    loop {
        if registry.owners.lock().unwrap()["main"]
            .slot
            .as_ref()
            .unwrap()
            .latest_query
            .load(Ordering::Acquire)
            == 2
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    send.send(()).unwrap();
    assert!(first.await.unwrap().unwrap().cancelled);
    assert_eq!(second.await.unwrap().unwrap().matched_count, 1);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn cancellation_during_initialization_releases_late_result_without_publishing() {
    let f = Fixture::new();
    let registry = Arc::new(Registry::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let (send, receive) = std::sync::mpsc::channel();
    let r = f.request(WorkspaceTreeRootKind::ProjectWorkspace, "needle");
    let task_registry = registry.clone();
    let task_request = r.clone();
    let count = calls.clone();
    let workspace = f.workspace.clone();
    let task = tokio::spawn(async move {
        task_registry
            .search("main", task_request, move || {
                count.fetch_add(1, Ordering::Relaxed);
                receive.recv_timeout(Duration::from_secs(5)).unwrap();
                Ok(workspace)
            })
            .await
    });
    while calls.load(Ordering::Relaxed) == 0 {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let mut cancel = r;
    cancel.cancel = true;
    assert!(
        registry
            .search("main", cancel, || panic!())
            .await
            .unwrap()
            .cancelled
    );
    send.send(()).unwrap();
    assert!(task.await.unwrap().unwrap().cancelled);
}

#[tokio::test]
async fn window_scope_capacity_expiry_and_close_release_indices_without_silent_rescans() {
    let f = Fixture::new();
    Fixture::put(&f.local(), "Needle.rs");
    let registry = Registry::default();
    let r = f.request(WorkspaceTreeRootKind::ProjectWorkspace, "needle");
    for window in ["one", "two", "three"] {
        let workspace = f.workspace.clone();
        assert_eq!(
            registry
                .search(window, r.clone(), move || Ok(workspace))
                .await
                .unwrap()
                .matched_count,
            1
        );
    }
    assert_eq!(
        registry
            .owners
            .lock()
            .unwrap()
            .values()
            .filter(|o| o.slot.is_some())
            .count(),
        MAX_SNAPSHOTS
    );
    assert_eq!(
        registry
            .search("one", r.clone(), || panic!())
            .await
            .unwrap_err(),
        EXPIRED
    );
    registry.prune(Instant::now() + IDLE_TTL + Duration::from_secs(1));
    assert_eq!(
        registry
            .search("three", r.clone(), || panic!())
            .await
            .unwrap_err(),
        EXPIRED
    );
    let mut fresh = r.clone();
    fresh.generation += 1;
    let workspace = f.workspace.clone();
    registry
        .search("three", fresh, move || Ok(workspace))
        .await
        .unwrap();
    let weak = {
        let owners = registry.owners.lock().unwrap();
        Arc::downgrade(
            owners["three"]
                .slot
                .as_ref()
                .unwrap()
                .value
                .get()
                .unwrap()
                .as_ref()
                .unwrap(),
        )
    };
    registry.release_window("three");
    assert!(weak.upgrade().is_none());
}

#[test]
fn cancel_before_begin_is_a_tombstone_and_cancelled_build_does_not_walk() {
    let f = Fixture::new();
    let registry = Registry::default();
    let mut r = f.request(WorkspaceTreeRootKind::ProjectWorkspace, "x");
    r.cancel = true;
    assert!(registry.checkout("main", &r).unwrap().is_none());
    r.cancel = false;
    assert!(registry.checkout("main", &r).unwrap().is_none());
    assert!(Catalogue::build(
        &f.workspace,
        r.root_kind,
        true,
        &AtomicBool::new(true),
        INDEX_LIMIT
    )
    .is_err());
}

#[test]
fn prefix_decoration_keeps_segment_boundaries_and_path_fence_rejects_escape() {
    let mut index = ProjectTreeDiffIndex::default();
    index
        .files
        .insert("foobar/child".into(), workspace_tree_file_change(vec![]));
    assert!(!index.has_descendant(Path::new("foo")));
    assert!(index.has_descendant(Path::new("foobar")));
    assert!(workspace_tree_relative_path(Some("../outside")).is_err());
    assert!(workspace_tree_relative_path(Some("/outside")).is_err());
}

#[test]
fn non_repository_workspace_uses_its_own_gitignore_and_node_modules_is_not_hardcoded() {
    let f = Fixture::new();
    Fixture::put(&f.local(), "node_modules/DependencyNeedle.rs");
    let kind = WorkspaceTreeRootKind::ProjectWorkspace;
    let request = f.request(kind, "needle");
    assert_eq!(
        query(&f.build(kind, false, INDEX_LIMIT), &request).matched_count,
        1
    );
    fs::write(f.local().join(".gitignore"), "node_modules/\n").unwrap();
    assert_eq!(
        query(&f.build(kind, false, INDEX_LIMIT), &request).matched_count,
        0
    );
    assert_eq!(
        query(&f.build(kind, true, INDEX_LIMIT), &request).matched_count,
        1
    );
}

#[test]
fn three_vcs_directory_nodes_remain_searchable_but_never_descend_with_either_switch() {
    let f = Fixture::new();
    let vendor = f.local().join("vendor");
    fs::create_dir_all(&vendor).unwrap();
    Fixture::git(&vendor, &["init", "-b", "main"]);
    for path in [".git/SecretNeedle", ".HG/SecretNeedle", ".SvN/SecretNeedle"] {
        Fixture::put(&vendor, path);
    }
    Fixture::put(&f.local(), "node_modules/DependencyNeedle");
    Fixture::put(&f.local(), ".git-extra/VisibleNeedle");
    for include_ignored in [false, true] {
        let index = f.build(
            WorkspaceTreeRootKind::ProjectWorkspace,
            include_ignored,
            INDEX_LIMIT,
        );
        for path in ["vendor/.git", "vendor/.HG", "vendor/.SvN"] {
            assert!(index.entries.contains_key(path));
            assert!(!index
                .entries
                .keys()
                .any(|p| p.starts_with(&format!("{path}/"))));
        }
        assert!(index.entries.contains_key("node_modules/DependencyNeedle"));
        assert!(index.entries.contains_key(".git-extra/VisibleNeedle"));
        assert_eq!(
            query(&index, &f.request(index.root.kind, "SecretNeedle")).matched_count,
            0
        );
        assert!(!index.git_failed && !index.read_failed);
    }
}

#[test]
fn case_only_agent_renames_do_not_masquerade_as_source_files_or_folders() {
    let mut f = Fixture::new();
    f.init_git();
    Fixture::put(&f.source(), "sample.rs");
    Fixture::put(&f.source(), "nest/Needle.rs");
    f.commit_source();
    let agent = f.agent();
    Fixture::git(&agent, &["mv", "sample.rs", "rename-file-temporary"]);
    Fixture::git(&agent, &["mv", "rename-file-temporary", "Sample.rs"]);
    Fixture::git(&agent, &["mv", "nest", "rename-folder-temporary"]);
    Fixture::git(&agent, &["mv", "rename-folder-temporary", "Nest"]);
    Fixture::git(&agent, &["commit", "-m", "case-only rename"]);
    let index = f.build(WorkspaceTreeRootKind::ProjectFiles, false, INDEX_LIMIT);
    assert!(!index.entries["sample.rs"].entry.is_ghost);
    assert!(index.entries["Sample.rs"].entry.is_ghost);
    assert!(!index.entries["nest/Needle.rs"].entry.is_ghost);
    assert!(index.entries["Nest"].entry.is_ghost);
    assert!(index.entries["Nest/Needle.rs"].entry.is_ghost);
    assert_eq!(
        index.entries["Sample.rs"]
            .entry
            .file_change
            .as_ref()
            .unwrap()
            .status,
        "added"
    );
}

fn rss_kib() -> u64 {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}
#[cfg(unix)]
fn peak_rss_bytes() -> u64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    // getrusage writes the supplied, initialized rusage and has no owned pointer.
    assert_eq!(unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) }, 0);
    let value = usage.ru_maxrss as u64;
    if cfg!(target_os = "macos") {
        value
    } else {
        value * 1024
    }
}
#[cfg(not(unix))]
fn peak_rss_bytes() -> u64 {
    0
}

fn percentiles(values: &[f64]) -> serde_json::Value {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    serde_json::json!({"p50Ms":sorted[sorted.len()/2],"p95Ms":sorted[(sorted.len()*95).div_ceil(100)-1]})
}

#[tokio::test]
#[ignore = "isolated native before/after performance receipt; run only with the resource window"]
async fn benchmark_native_index_and_small_payload_three_rounds() {
    let mut f = Fixture::new();
    f.init_git();
    for n in 0..64 {
        Fixture::put(&f.source(), &format!("dir-{n:03}/Needle.rs"));
    }
    f.commit_source();
    let tree = f.agent();
    Fixture::put(&tree, "ghost/Needle.rs");
    let probe = Probe::new(&f.base);
    let mut before = Vec::new();
    let mut after = Vec::new();
    let mut query_times = Vec::new();
    let mut rows = Vec::new();
    for round in 1..=3 {
        probe.take();
        let start = Instant::now();
        let old = legacy_inventory(&f.workspace, WorkspaceTreeRootKind::ProjectFiles);
        let before_ms = start.elapsed().as_secs_f64() * 1000.0;
        let before_git = probe.take();
        let expected = old
            .values()
            .filter(|e| e.name.to_lowercase().contains("needle"))
            .count();
        let registry = Registry::default();
        let request = f.request(WorkspaceTreeRootKind::ProjectFiles, "needle");
        let workspace = f.workspace.clone();
        let rss_before = rss_kib();
        let start = Instant::now();
        let result = registry
            .search("bench", request.clone(), move || Ok(workspace))
            .await
            .unwrap();
        let bytes = serde_json::to_vec(&result).unwrap().len();
        let after_ms = start.elapsed().as_secs_f64() * 1000.0;
        let after_git = probe.take();
        let rss_after = rss_kib();
        assert_eq!(result.matched_count, expected);
        assert!(
            !result.truncated
                && !result.index_truncated
                && !result.read_failed
                && !result.git_failed
        );
        let mut changed = request;
        changed.query_revision += 1;
        changed.query = "Needle.rs".into();
        let start = Instant::now();
        let result2 = registry
            .search("bench", changed, || {
                panic!("cached query prepared workspace")
            })
            .await
            .unwrap();
        let bytes2 = serde_json::to_vec(&result2).unwrap().len();
        let query_ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(probe.take(), 0);
        before.push(before_ms);
        after.push(after_ms);
        query_times.push(query_ms);
        rows.push(serde_json::json!({"round":round,"beforeMs":before_ms,"beforeGitProcesses":before_git,
            "afterMs":after_ms,"afterGitProcesses":after_git,"cachedQueryMs":query_ms,"cachedQueryGitProcesses":0,
            "indexedEntries":result.indexed_count,"matched":result.matched_count,"returnedWithAncestors":result.entries.len(),
            "payloadBytes":bytes,"cachedPayloadBytes":bytes2,"rssBeforeKiB":rss_before,"rssAfterKiB":rss_after}));
    }
    // A larger physical-only catalogue for measured memory and maximum-hit payload.
    let big = Fixture::new();
    let big_files: usize = std::env::var("KOTA_NAME_SEARCH_BENCH_FILES")
        .ok()
        .map(|s| s.parse().unwrap())
        .unwrap_or(20_000);
    for n in 0..big_files {
        Fixture::put(&big.local(), &format!("group-{}/Needle-{n:05}.rs", n / 100));
    }
    let rss_before = rss_kib();
    let registry = Registry::default();
    let workspace = big.workspace.clone();
    let start = Instant::now();
    let result = registry
        .search(
            "big",
            big.request(WorkspaceTreeRootKind::ProjectWorkspace, "needle"),
            move || Ok(workspace),
        )
        .await
        .unwrap();
    let payload = serde_json::to_vec(&result).unwrap().len();
    let build_ms = start.elapsed().as_secs_f64() * 1000.0;
    let rss_retained = rss_kib();
    assert_eq!(result.matched_count, MATCH_LIMIT);
    assert!(result.truncated);
    let physical_entries = big_files + big_files.div_ceil(100);
    assert_eq!(result.indexed_count, physical_entries.min(INDEX_LIMIT));
    assert_eq!(result.index_truncated, physical_entries > INDEX_LIMIT);
    let mut absent_request =
        big.request(WorkspaceTreeRootKind::ProjectWorkspace, "definitely-absent");
    absent_request.query_revision = 2;
    let start = Instant::now();
    let absent = registry
        .search("big", absent_request, || {
            panic!("cached query prepared workspace")
        })
        .await
        .unwrap();
    let absent_query_ms = start.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(absent.matched_count, 0);
    registry.release_window("big");
    let rss_released = rss_kib();
    println!(
        "NAME_SEARCH_BENCH={}",
        serde_json::json!({"profile":"dev opt-level=0; unchanged", "rounds":rows,
        "before":percentiles(&before),"after":percentiles(&after),"cachedQuery":percentiles(&query_times),
        "largeCatalogue":{"physicalEntries":physical_entries,"indexedEntries":result.indexed_count,
        "indexTruncated":result.index_truncated,"matched":result.matched_count,"buildAndSerializeMs":build_ms,
        "absentQueryMs":absent_query_ms,"returnedWithAncestors":result.entries.len(),"payloadBytes":payload,
        "rssBeforeKiB":rss_before,"rssRetainedKiB":rss_retained,"rssAfterReleaseKiB":rss_released,
        "processPeakRssBytes":peak_rss_bytes()},
        "boundary":"real FS/Git, preloaded isolated workspace; excludes manager preparation, IPC transport and React; n=3 p95 is maximum; OS cache not cleared"})
    );
}
