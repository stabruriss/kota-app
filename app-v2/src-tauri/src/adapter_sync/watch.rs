//! Native events are restricted to rules leaf directories. Healthy watches
//! need no periodic filesystem scan; only missing/failed subscriptions retry.
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Source {
    AccountRules,
    ProjectRules(PathBuf),
    SkillPool,
    PoolSkill(String),
}

#[derive(Default)]
pub(super) struct Changes {
    pub sources: BTreeSet<Source>,
    pub recheck: BTreeSet<PathBuf>,
    pub error: Option<String>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty() && self.recheck.is_empty() && self.error.is_none()
    }
    pub fn merge(&mut self, other: Self) {
        self.sources.extend(other.sources);
        self.recheck.extend(other.recheck);
        if other.error.is_some() {
            self.error = other.error;
        }
    }
}

type Index = BTreeMap<PathBuf, BTreeSet<Source>>;
type Sink = Arc<dyn Fn(Changes) + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq)]
struct DirectoryIdentity {
    device: u64,
    inode: u64,
}

fn resolve_directory(path: &Path) -> Result<(PathBuf, DirectoryIdentity), String> {
    use std::os::unix::fs::MetadataExt;
    let path =
        std::fs::canonicalize(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let metadata = std::fs::metadata(&path).map_err(|error| error.to_string())?;
    if !metadata.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    Ok((
        path,
        DirectoryIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        },
    ))
}

struct Directory {
    source: Source,
    current: Option<(PathBuf, DirectoryIdentity)>,
    initialized: bool,
    recheck: bool,
    failures: u8,
    last_error: Option<String>,
}

struct WatchSet {
    watcher: notify::RecommendedWatcher,
    index: Arc<RwLock<Index>>,
    directories: BTreeMap<PathBuf, Directory>,
    attached: BTreeMap<PathBuf, DirectoryIdentity>,
    attach_failures: BTreeMap<PathBuf, u8>,
    sink: Sink,
    #[cfg(test)]
    callbacks: Arc<std::sync::atomic::AtomicUsize>,
}

fn classify(event: &Event, index: &Index) -> Changes {
    let mut changes = Changes::default();
    if matches!(event.kind, EventKind::Access(_)) {
        return changes;
    }
    for (directory, sources) in index {
        if event.need_rescan() {
            changes.sources.extend(sources.iter().cloned());
            changes.recheck.insert(directory.clone());
            continue;
        }
        if event.paths.iter().any(|path| path == directory) {
            // Directory metadata can change for an irrelevant temporary file.
            // Only refresh decides whether the root's identity actually changed.
            changes.recheck.insert(directory.clone());
        }
        for path in &event.paths {
            if sources.contains(&Source::SkillPool) {
                let Ok(relative) = path.strip_prefix(directory) else {
                    continue;
                };
                let parts: Vec<_> = relative.components().collect();
                if parts.len() == 1 || (parts.len() == 2 && parts[1].as_os_str() == "SKILL.md") {
                    if let Some(id) = parts
                        .first()
                        .and_then(|part| part.as_os_str().to_str())
                        .filter(|id| !id.starts_with('.'))
                    {
                        changes.sources.insert(Source::PoolSkill(id.to_string()));
                    }
                }
            }
            if path.parent() == Some(directory.as_path())
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            {
                changes.sources.extend(
                    sources
                        .iter()
                        .filter(|source| !matches!(source, Source::SkillPool))
                        .cloned(),
                );
            }
        }
    }
    changes
}

impl WatchSet {
    fn new(sink: Sink) -> Result<Self, String> {
        let index = Arc::new(RwLock::new(Index::new()));
        let callback_index = Arc::clone(&index);
        let callback_sink = Arc::clone(&sink);
        #[cfg(test)]
        let callbacks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        #[cfg(test)]
        let callback_count = Arc::clone(&callbacks);
        let watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
            // Only bounded source/path classification and marking here. No
            // config reads, directory traversal, disk logging or compilation.
            #[cfg(test)]
            callback_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let changes = match event {
                Ok(event) => classify(&event, &callback_index.read().unwrap()),
                Err(error) => Changes {
                    error: Some(error.to_string()),
                    ..Changes::default()
                },
            };
            if !changes.is_empty() {
                callback_sink(changes);
            }
        })
        .map_err(|error| error.to_string())?;
        Ok(Self {
            watcher,
            index,
            directories: BTreeMap::new(),
            attached: BTreeMap::new(),
            attach_failures: BTreeMap::new(),
            sink,
            #[cfg(test)]
            callbacks,
        })
    }

    fn add(&mut self, directory: PathBuf, source: Source) {
        self.directories
            .entry(directory)
            .and_modify(|entry| {
                entry.recheck = true;
                entry.failures = 0;
            })
            .or_insert(Directory {
                source,
                current: None,
                initialized: false,
                recheck: true,
                failures: 0,
                last_error: None,
            });
        // Explicit open/start is also a retry opportunity after exhausted retries.
        self.attach_failures.clear();
    }

    fn invalidate(&mut self, paths: &BTreeSet<PathBuf>, all: bool) {
        for (requested, entry) in &mut self.directories {
            if all
                || paths.contains(requested)
                || entry
                    .current
                    .as_ref()
                    .is_some_and(|(path, _)| paths.contains(path))
            {
                entry.recheck = true;
                entry.failures = 0;
            }
        }
        self.attach_failures.clear();
    }

    fn refresh(&mut self) {
        let mut changes = Changes::default();
        for (requested, entry) in &mut self.directories {
            if !entry.recheck && (entry.current.is_some() || entry.failures >= 3) {
                continue;
            }
            entry.recheck = false;
            match resolve_directory(requested) {
                Ok(current) => {
                    if entry.initialized && entry.current.as_ref() != Some(&current) {
                        changes.sources.insert(entry.source.clone());
                    }
                    entry.current = Some(current);
                    entry.failures = 0;
                    entry.last_error = None;
                }
                Err(error) => {
                    if entry.current.take().is_some() {
                        changes.sources.insert(entry.source.clone());
                    }
                    entry.failures += 1;
                    if entry.last_error.as_deref() != Some(&error) {
                        crate::kota_debug_log(&format!(
                            "[adapter-sync] rules watch unavailable: {error}"
                        ));
                        entry.last_error = Some(error);
                    }
                }
            }
            entry.initialized = true;
        }
        let mut desired = BTreeMap::<PathBuf, (DirectoryIdentity, BTreeSet<Source>)>::new();
        for entry in self.directories.values() {
            if let Some((path, identity)) = &entry.current {
                desired
                    .entry(path.clone())
                    .or_insert_with(|| (identity.clone(), BTreeSet::new()))
                    .1
                    .insert(entry.source.clone());
            }
        }
        // Install the classifier before starting new streams. Canonical paths
        // agree with notify on macOS (/var versus /private/var, symlinks).
        *self.index.write().unwrap() = desired
            .iter()
            .map(|(path, (_, sources))| (path.clone(), sources.clone()))
            .collect();
        let stale = self
            .attached
            .iter()
            .filter(|(path, identity)| desired.get(*path).map(|(next, _)| next) != Some(*identity))
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        for path in stale {
            let _ = self.watcher.unwatch(&path);
            self.attached.remove(&path);
        }
        for (path, (identity, sources)) in desired {
            if self.attached.get(&path) == Some(&identity)
                || self.attach_failures.get(&path).copied().unwrap_or(0) >= 3
            {
                continue;
            }
            let mode = if sources.contains(&Source::SkillPool) {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };
            match self.watcher.watch(&path, mode) {
                Ok(()) => {
                    if self.attach_failures.remove(&path).is_some() {
                        changes.sources.extend(sources);
                    }
                    self.attached.insert(path, identity);
                }
                Err(error) => {
                    let attempts = self.attach_failures.entry(path.clone()).or_default();
                    *attempts += 1;
                    if *attempts == 1 {
                        crate::kota_debug_log(&format!(
                            "[adapter-sync] watch {}: {error}",
                            path.display()
                        ));
                    }
                }
            }
        }
        if !changes.sources.is_empty() {
            (self.sink)(changes);
        }
    }
}

thread_local! { static WATCHES: RefCell<Option<WatchSet>> = const { RefCell::new(None) }; }

pub(super) fn start() -> Result<(), String> {
    WATCHES.with(|slot| {
        if slot.borrow().is_none() {
            let mut watches = WatchSet::new(Arc::new(super::mark_rule_events))?;
            watches.add(crate::account_rules_dir(), Source::AccountRules);
            watches.add(crate::account_skills_dir(), Source::SkillPool);
            for root in crate::registered_adapter_projects()? {
                watches.add(crate::project_rules_dir(&root), Source::ProjectRules(root));
            }
            watches.refresh();
            *slot.borrow_mut() = Some(watches);
        }
        Ok(())
    })
}

pub(super) fn register_project(root: &Path) {
    WATCHES.with(|slot| {
        if let Some(watches) = slot.borrow_mut().as_mut() {
            watches.add(crate::account_rules_dir(), Source::AccountRules);
            watches.add(crate::account_skills_dir(), Source::SkillPool);
            watches.add(
                crate::project_rules_dir(root),
                Source::ProjectRules(root.to_path_buf()),
            );
            watches.refresh();
        }
    });
}

pub(super) fn forget_project(root: &Path) {
    WATCHES.with(|slot| {
        if let Some(watches) = slot.borrow_mut().as_mut() {
            watches.directories.retain(|_, entry| !matches!(&entry.source, Source::ProjectRules(project) if project == root));
            watches.refresh();
        }
    });
}

pub(super) fn register_skill_pool() {
    WATCHES.with(|slot| {
        if let Some(watches) = slot.borrow_mut().as_mut() {
            watches.add(crate::account_skills_dir(), Source::SkillPool);
            watches.refresh();
        }
    });
}

pub(super) fn maintenance() {
    WATCHES.with(|slot| {
        if let Some(watches) = slot.borrow_mut().as_mut() {
            watches.refresh();
        }
    });
}

pub(super) fn process(changes: Changes) {
    WATCHES.with(|slot| {
        if let Some(watches) = slot.borrow_mut().as_mut() {
            if changes.error.is_some() || !changes.recheck.is_empty() {
                watches.invalidate(&changes.recheck, changes.error.is_some());
                watches.refresh();
            }
        }
    });
    if let Some(error) = changes.error {
        crate::kota_debug_log(&format!("[adapter-sync] rules event error: {error}"));
    }
    let mut skills = BTreeSet::new();
    let mut all_skills = false;
    for source in changes.sources {
        match source {
            Source::SkillPool => all_skills = true,
            Source::PoolSkill(id) => {
                skills.insert(id);
            }
            Source::AccountRules => match super::project_catalog() {
                Ok(roots) => {
                    for root in roots {
                        super::mark_project(root, "account rules file change");
                    }
                }
                Err(error) => crate::kota_debug_log(&format!(
                    "[adapter-sync] account rules broadcast: {error}"
                )),
            },
            Source::ProjectRules(root) => super::mark_project(root, "project rules file change"),
        }
    }
    if all_skills || !skills.is_empty() {
        super::pool_changed(if all_skills { None } else { Some(skills) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    #[test]
    fn only_rule_files_and_root_changes_trigger_sync() {
        let index = BTreeMap::from([(
            PathBuf::from("/rules"),
            BTreeSet::from([Source::AccountRules]),
        )]);
        let event = |path| Event::new(EventKind::Any).add_path(PathBuf::from(path));
        assert!(classify(&event("/rules/readme.MD"), &index)
            .sources
            .contains(&Source::AccountRules));
        for ignored in [
            "/rules/tmp.txt",
            "/rules/nested/a.md",
            "/project/src/main.rs",
            "/rules/a.md.tmp",
        ] {
            assert!(classify(&event(ignored), &index).sources.is_empty());
        }
        assert!(classify(&event("/rules"), &index)
            .recheck
            .contains(Path::new("/rules")));
    }

    #[test]
    fn account_and_project_directories_keep_their_source_scope_when_events_merge() {
        let account = PathBuf::from("/account/rules");
        let first = PathBuf::from("/first/project-rules");
        let second = PathBuf::from("/second/project-rules");
        let first_source = Source::ProjectRules(PathBuf::from("/first"));
        let second_source = Source::ProjectRules(PathBuf::from("/second"));
        let index = BTreeMap::from([
            (account.clone(), BTreeSet::from([Source::AccountRules])),
            (first.clone(), BTreeSet::from([first_source.clone()])),
            (second, BTreeSet::from([second_source.clone()])),
        ]);
        let mut changes = classify(
            &Event::new(EventKind::Any).add_path(first.join("rule.md")),
            &index,
        );
        assert_eq!(changes.sources, BTreeSet::from([first_source.clone()]));
        changes.merge(classify(
            &Event::new(EventKind::Any).add_path(account.join("rule.md")),
            &index,
        ));
        assert_eq!(
            changes.sources,
            BTreeSet::from([first_source, Source::AccountRules])
        );
        assert!(!changes.sources.contains(&second_source));
        let mut rescan = Event::new(EventKind::Any);
        rescan.attrs.set_flag(notify::event::Flag::Rescan);
        assert_eq!(classify(&rescan, &index).sources.len(), 3);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_rules_events_survive_rename_and_directory_replacement_without_build_events() {
        let root = std::env::temp_dir().join(format!("kota-rules-watch-{}", uuid::Uuid::new_v4()));
        let rules = root.join("project-rules");
        std::fs::create_dir_all(&rules).unwrap();
        let (send, receive) = mpsc::channel();
        let mut watches = WatchSet::new(Arc::new(move |changes| {
            let _ = send.send(changes);
        }))
        .unwrap();
        watches.add(rules.clone(), Source::ProjectRules(root.clone()));
        watches.refresh();
        std::fs::write(rules.join("first.MD"), "first").unwrap();
        let await_change = || {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let change = receive
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .unwrap();
                if change.sources.contains(&Source::ProjectRules(root.clone())) {
                    return change;
                }
            }
        };
        await_change();
        std::fs::rename(rules.join("first.MD"), rules.join("second.md")).unwrap();
        await_change();
        while receive.recv_timeout(Duration::from_millis(250)).is_ok() {}
        std::fs::write(rules.join("x.tmp"), "temporary").unwrap();
        while let Ok(changes) = receive.recv_timeout(Duration::from_millis(400)) {
            assert!(
                changes.sources.is_empty(),
                "temporary file must not dirty the project"
            );
        }
        // Drain native batching before the unrelated build workload.
        while receive.recv_timeout(Duration::from_millis(250)).is_ok() {}
        let before_build = watches.callbacks.load(std::sync::atomic::Ordering::Relaxed);
        let build = root.join(".agent-workspaces/test/project-files/target");
        std::fs::create_dir_all(&build).unwrap();
        for n in 0..1000 {
            std::fs::write(build.join(format!("object-{n}")), "build").unwrap();
        }
        let source = build.parent().unwrap().join("watcher-build.rs");
        std::fs::write(&source, "pub fn fixture() -> u64 { 42 }").unwrap();
        let compile = std::process::Command::new("rustc")
            .arg("--crate-type=lib")
            .arg("--emit=obj")
            .arg(&source)
            .arg("-o")
            .arg(build.join("fixture.o"))
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        assert!(receive.recv_timeout(Duration::from_millis(500)).is_err());
        assert_eq!(
            watches.callbacks.load(std::sync::atomic::Ordering::Relaxed),
            before_build,
            "unrelated build must not enter the native callback"
        );
        std::fs::remove_dir_all(&rules).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let removed = loop {
            let change = receive
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap();
            if !change.recheck.is_empty() {
                break change;
            }
        };
        watches.invalidate(&removed.recheck, false);
        watches.refresh();
        assert!(watches.attached.is_empty());
        std::fs::create_dir(&rules).unwrap();
        std::fs::write(rules.join("restored.md"), "restored").unwrap();
        watches.refresh();
        assert_eq!(watches.attached.len(), 1);
        await_change();
        println!(
            "{}",
            serde_json::json!({"engine":"macOS FSEvents", "rule_save":"PASS", "rename":"PASS", "directory_restore":"PASS", "worktree_file_writes":1000, "rustc_build":"PASS", "build_callbacks":0})
        );
        drop(watches);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn pool_events_select_member_ids_and_ignore_unrelated_skill_contents() {
        let index = BTreeMap::from([(
            PathBuf::from("/skills"),
            BTreeSet::from([Source::SkillPool]),
        )]);
        let event = |path| Event::new(EventKind::Any).add_path(PathBuf::from(path));
        for path in ["/skills/A", "/skills/A/SKILL.md"] {
            assert_eq!(
                classify(&event(path), &index).sources,
                BTreeSet::from([Source::PoolSkill("A".into())])
            );
        }
        for path in ["/skills/A/scripts/run.py", "/skills/.importing/SKILL.md"] {
            assert!(classify(&event(path), &index).sources.is_empty());
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_skill_pool_detects_members_and_skill_entry_restoration() {
        let root = std::env::temp_dir().join(format!("kota-skills-watch-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("A")).unwrap();
        let (send, receive) = mpsc::channel();
        let mut watches = WatchSet::new(Arc::new(move |changes| {
            let _ = send.send(changes);
        }))
        .unwrap();
        watches.add(root.clone(), Source::SkillPool);
        watches.refresh();
        let await_id = |id: &str| {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let changes = receive
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .unwrap();
                if changes.sources.contains(&Source::PoolSkill(id.to_string())) {
                    break;
                }
            }
        };
        std::fs::write(root.join("A/SKILL.md"), "created").unwrap();
        await_id("A");
        std::fs::remove_file(root.join("A/SKILL.md")).unwrap();
        await_id("A");
        std::fs::write(root.join("A/SKILL.md"), "restored").unwrap();
        await_id("A");
        std::fs::rename(root.join("A"), root.join("B")).unwrap();
        await_id("B");
        println!(
            "{}",
            serde_json::json!({"engine":"macOS FSEvents", "skill_entry_create":"PASS", "skill_entry_delete":"PASS", "skill_entry_restore":"PASS", "skill_directory_rename":"PASS"})
        );
        drop(watches);
        std::fs::remove_dir_all(root).unwrap();
    }
}
