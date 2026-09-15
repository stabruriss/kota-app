//! A fixed set of configuration files, not directory polling. Metadata follows
//! symlinks just like read(), and failed/unstable reads never advance the stamp.
use super::AgentKey;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Source {
    Shell(AgentKey),
    Identity(AgentKey),
    AccountIdentity,
    RosterProject(PathBuf),
    AvatarIndex,
    AvatarFile,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    modified: (i64, i64),
    changed: (i64, i64),
    len: u64,
    device: u64,
    inode: u64,
}

fn stamp(path: &Path) -> Result<Stamp, String> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    Ok(Stamp {
        modified: (meta.mtime(), meta.mtime_nsec()),
        changed: (meta.ctime(), meta.ctime_nsec()),
        len: meta.len(),
        device: meta.dev(),
        inode: meta.ino(),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Fields {
    Identity(crate::AdapterIdentity),
    AccountName(String),
    Shell {
        provider: Option<crate::pty::agent::AgentCli>,
        skills: Vec<String>,
    },
    Roster(String),
    Image(Stamp),
}

fn fields(source: &Source, text: &str) -> Result<Fields, String> {
    match source {
        Source::RosterProject(_) => {
            #[derive(serde::Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct Registration {
                project_id: String,
                repo_full_name: String,
                #[serde(default)]
                archived: bool,
            }
            let r: Registration = serde_json::from_str(text).map_err(|e| e.to_string())?;
            Ok(Fields::Roster(
                serde_json::to_string(&(r.project_id, r.repo_full_name, r.archived)).unwrap(),
            ))
        }
        Source::AvatarIndex => {
            let rows: Vec<crate::StoredUserHeroAvatar> =
                serde_json::from_str(text).map_err(|e| e.to_string())?;
            let relevant: Vec<_> = rows
                .into_iter()
                .map(|r| (r.id, r.file_name, r.mime))
                .collect();
            Ok(Fields::Roster(serde_json::to_string(&relevant).unwrap()))
        }
        Source::AvatarFile => Err("image metadata is inspected separately".into()),
        Source::Identity(key) => {
            let value: serde_yaml::Value =
                serde_yaml::from_str(text).map_err(|error| error.to_string())?;
            let mapping = value
                .as_mapping()
                .filter(|mapping| !mapping.is_empty())
                .ok_or("agent.yaml must contain an identity mapping")?;
            Ok(Fields::Identity(crate::adapter_identity_fields(
                mapping, &key.agent,
            )))
        }
        Source::AccountIdentity => {
            let identity: crate::AccountUserIdentity =
                serde_json::from_str(text).map_err(|error| error.to_string())?;
            Ok(Fields::AccountName(
                crate::normalize_account_user_identity(identity).name,
            ))
        }
        Source::Shell(_) => {
            let shell = crate::parse_shell_yaml(text)?;
            let provider = shell
                .provider
                .as_deref()
                .or(shell.command.as_deref())
                .map(crate::cli_from_shell_name)
                .transpose()?;
            Ok(Fields::Shell {
                provider,
                skills: crate::normalized_skill_names(&shell.skills)
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
            })
        }
    }
}

struct Entry {
    source: Source,
    handled: Option<Stamp>,
    value: Option<Fields>,
    error: Option<String>,
    roster_value: Option<String>,
    roster_changed: bool,
}

#[derive(Default)]
struct Counts {
    stats: usize,
    reads: usize,
}

impl Entry {
    fn inspect(&mut self, path: &Path, counts: &mut Counts) -> bool {
        self.roster_changed = false;
        counts.stats += 1;
        let result = (|| {
            let before = stamp(path)?;
            if self.handled.as_ref() == Some(&before) && self.error.is_none() {
                return Ok(false);
            }
            let (next, roster) = if matches!(self.source, Source::AvatarFile) {
                (Fields::Image(before.clone()), None)
            } else {
                counts.reads += 1;
                let text = std::fs::read_to_string(path)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                let next = fields(&self.source, &text)?;
                let roster = if let Source::Identity(key) = &self.source {
                    let yaml: serde_yaml::Value =
                        serde_yaml::from_str(&text).map_err(|e| e.to_string())?;
                    let a = crate::agent_directory::agent_fields(
                        &key.agent,
                        yaml.as_mapping().ok_or("invalid agent mapping")?,
                    );
                    Some(serde_json::to_string(&a.map(|a| (a.id, a.name, a.avatar_id))).unwrap())
                } else {
                    None
                };
                (next, roster)
            };
            counts.stats += 1;
            if stamp(path)? != before {
                return Err(format!(
                    "{} changed while reading; retry later",
                    path.display()
                ));
            }
            // Establish initial fields without broadcasting all inactive projects
            // at app startup. Recovery after an initial failure must still sync.
            let changed =
                self.value.as_ref().is_some_and(|old| old != &next) || self.error.is_some();
            self.roster_changed = roster.is_some()
                && (self
                    .roster_value
                    .as_ref()
                    .is_some_and(|old| Some(old) != roster.as_ref())
                    || self.error.is_some());
            self.roster_value = roster;
            self.value = Some(next);
            self.handled = Some(before);
            self.error = None;
            Ok(changed)
        })();
        match result {
            Ok(changed) => changed,
            Err(error) => {
                if self.error.as_ref() != Some(&error) {
                    self.roster_changed = matches!(
                        self.source,
                        Source::Identity(_)
                            | Source::RosterProject(_)
                            | Source::AvatarIndex
                            | Source::AvatarFile
                    );
                    crate::kota_debug_log(&format!("[adapter-sync] config check: {error}"));
                    self.error = Some(error);
                }
                false
            }
        }
    }
}

#[derive(Default)]
struct FileSet {
    entries: BTreeMap<PathBuf, Entry>,
    projects: BTreeSet<PathBuf>,
    roster_changed: bool,
}

impl FileSet {
    fn register_roster_images(&mut self, paths: BTreeSet<PathBuf>) -> bool {
        let added = paths.iter().any(|path| !self.entries.contains_key(path));
        self.entries
            .retain(|path, e| !matches!(e.source, Source::AvatarFile) || paths.contains(path));
        for path in paths {
            self.add(path, Source::AvatarFile);
        }
        added
    }

    fn add(&mut self, path: PathBuf, source: Source) {
        if self.entries.contains_key(&path) {
            return;
        }
        let mut entry = Entry {
            source,
            handled: None,
            value: None,
            error: None,
            roster_value: None,
            roster_changed: false,
        };
        entry.inspect(&path, &mut Counts::default());
        self.entries.insert(path, entry);
    }

    fn check(&mut self) -> (Vec<Source>, Counts) {
        self.roster_changed = false;
        let mut counts = Counts::default();
        let mut changes = Vec::new();
        for (path, entry) in &mut self.entries {
            if entry.inspect(path, &mut counts) {
                changes.push(entry.source.clone());
            }
            self.roster_changed |= entry.roster_changed;
        }
        (changes, counts)
    }

    fn register_project(&mut self, root: &Path) -> Result<(), String> {
        self.projects.insert(root.to_path_buf());
        self.add(
            root.join("workspace.json"),
            Source::RosterProject(root.to_path_buf()),
        );
        let directory = root.join(".agent-workspaces");
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("read {}: {error}", directory.display())),
        };
        let mut present = BTreeSet::new();
        for entry in entries {
            let path = entry.map_err(|error| error.to_string())?.path();
            if !path.is_dir() {
                continue;
            }
            let Some(id) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let key = AgentKey {
                project: root.to_path_buf(),
                agent: id.to_string(),
            };
            present.insert(key.clone());
            if path.join("agent.yaml").is_file() || path.join("SHELL.yaml").is_file() {
                self.add(path.join("SHELL.yaml"), Source::Shell(key.clone()));
                self.add(path.join("agent.yaml"), Source::Identity(key));
            }
        }
        self.entries.retain(|_, entry| match &entry.source {
            Source::Shell(key) | Source::Identity(key) => {
                key.project != root || present.contains(key)
            }
            Source::AccountIdentity
            | Source::RosterProject(_)
            | Source::AvatarIndex
            | Source::AvatarFile => true,
        });
        Ok(())
    }
}

thread_local! { static FILES: RefCell<Option<FileSet>> = const { RefCell::new(None) }; }

pub(super) fn start() -> Result<(), String> {
    // Keep the catalog available even if enumerating one project fails;
    // subsequent open/start hooks can restore its entries.
    FILES.with(|slot| {
        let mut files = FileSet::default();
        files.add(crate::account_user_identity_path(), Source::AccountIdentity);
        files.add(
            crate::kota_home_dir().join("avatars/avatars.json"),
            Source::AvatarIndex,
        );
        *slot.borrow_mut() = Some(files);
    });
    for root in crate::registered_adapter_projects()? {
        register_project(&root);
    }
    Ok(())
}

pub(super) fn register_project(root: &Path) {
    FILES.with(|slot| {
        if let Some(files) = slot.borrow_mut().as_mut() {
            let before = files.entries.keys().cloned().collect::<BTreeSet<_>>();
            if let Err(error) = files.register_project(root) {
                crate::kota_debug_log(&format!("[adapter-sync] register config: {error}"));
            }
            if before != files.entries.keys().cloned().collect() {
                crate::bbs_sync::roster::runtime::local_changed();
            }
        }
    });
}

pub(super) fn projects() -> Option<Vec<PathBuf>> {
    FILES.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|files| files.projects.iter().cloned().collect())
    })
}

pub(super) fn account_identity_was_present() -> bool {
    FILES.with(|slot| {
        slot.borrow().as_ref().is_some_and(|files| {
            files
                .entries
                .get(&crate::account_user_identity_path())
                .is_some_and(|entry| entry.value.is_some())
        })
    })
}

pub(super) fn forget_agent(removed: &AgentKey) {
    FILES.with(|slot| {
        if let Some(files) = slot.borrow_mut().as_mut() {
            files.entries.retain(|_, entry| match &entry.source {
                Source::Shell(key) | Source::Identity(key) => key != removed,
                Source::AccountIdentity
                | Source::RosterProject(_)
                | Source::AvatarIndex
                | Source::AvatarFile => true,
            });
        }
    });
    crate::bbs_sync::roster::runtime::local_changed();
}

pub(super) fn forget_project(root: &Path) {
    FILES.with(|slot| {
        if let Some(files) = slot.borrow_mut().as_mut() {
            files.projects.remove(root);
            files.entries.retain(|_, entry| match &entry.source {
                Source::Shell(key) | Source::Identity(key) => key.project != root,
                Source::RosterProject(project) => project != root,
                Source::AccountIdentity | Source::AvatarIndex | Source::AvatarFile => true,
            });
        }
    });
    crate::bbs_sync::roster::runtime::local_changed();
}

pub(super) fn register_roster_images(paths: Vec<PathBuf>) {
    let added = FILES.with(|slot| {
        if let Some(files) = slot.borrow_mut().as_mut() {
            files.register_roster_images(paths.into_iter().collect())
        } else {
            false
        }
    });
    // Close capture→registration: add() establishes a baseline at registration,
    // which may already differ from the image that the collector captured.
    // Rebuild once after adding paths, never for the unchanged watched set.
    if added {
        crate::bbs_sync::roster::runtime::local_changed();
    }
}

pub(super) fn check() {
    let changes = FILES.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(files) = slot.as_mut() else {
            return Vec::new();
        };
        let (changes, counts) = files.check();
        if files.roster_changed {
            crate::bbs_sync::roster::runtime::local_changed();
        }
        if !changes.is_empty() {
            crate::kota_debug_log(&format!(
                "[adapter-sync] config changes={} metadata={} reads={}",
                changes.len(),
                counts.stats,
                counts.reads
            ));
        }
        changes
    });
    // Release the catalog borrow before account broadcasts read its projects.
    for source in changes {
        match source {
            Source::Shell(key) => super::mark_skills(key, "SHELL file change"),
            Source::Identity(key) => super::mark_project(key.project, "agent identity file change"),
            Source::RosterProject(_) | Source::AvatarIndex | Source::AvatarFile => {
                crate::bbs_sync::roster::runtime::local_changed()
            }
            Source::AccountIdentity => match super::project_catalog() {
                Ok(roots) => {
                    for root in roots {
                        super::mark_project(root, "account identity file change");
                    }
                }
                Err(error) => crate::kota_debug_log(&format!(
                    "[adapter-sync] account identity broadcast: {error}"
                )),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (PathBuf, FileSet, Source) {
        let root = std::env::temp_dir().join(format!("kota-config-check-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let source = Source::Shell(AgentKey {
            project: root.clone(),
            agent: "alice".into(),
        });
        (root, FileSet::default(), source)
    }

    #[test]
    fn fixed_files_ignore_unrelated_fields_and_retry_invalid_or_absent_inputs() {
        let (root, mut files, source) = fixture();
        let path = root.join("SHELL.yaml");
        std::fs::write(&path, "provider: codex\nskills: [A]\nmodel: first\n").unwrap();
        files.add(path.clone(), source.clone());
        let (changes, counts) = files.check();
        assert!(changes.is_empty());
        assert_eq!(counts.stats, 1);
        assert_eq!(counts.reads, 0);
        std::fs::write(&path, "provider: codex\nskills: [A]\nmodel: other\n").unwrap();
        let (changes, counts) = files.check();
        assert!(changes.is_empty());
        assert_eq!(counts.reads, 1);
        let good = files.entries[&path].handled.clone();
        std::fs::write(&path, "skills: [").unwrap();
        assert!(files.check().0.is_empty());
        assert_eq!(files.entries[&path].handled, good);
        assert_eq!(files.check().1.reads, 1);
        std::fs::remove_file(&path).unwrap();
        assert!(files.check().0.is_empty());
        std::fs::write(&path, "provider: codex\nskills: [B]\n").unwrap();
        assert_eq!(files.check().0, vec![source]);
        assert_eq!(files.check().1.reads, 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn initially_missing_file_is_retained_and_creation_triggers_sync() {
        let (root, mut files, source) = fixture();
        let path = root.join("SHELL.yaml");
        files.add(path.clone(), source.clone());
        assert_eq!(files.entries.len(), 1);
        assert!(files.check().0.is_empty());
        std::fs::write(&path, "provider: codex\nskills: [A]\n").unwrap();
        assert_eq!(files.check().0, vec![source]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn precision_handles_same_second_same_size_rename_restored_mtime_and_symlink_targets() {
        use std::os::unix::fs::symlink;
        let (root, mut files, source) = fixture();
        let path = root.join("SHELL.yaml");
        let target = root.join("target.yaml");
        std::fs::write(&target, "provider: codex\nskills: [A]\n").unwrap();
        symlink(&target, &path).unwrap();
        files.add(path.clone(), source.clone());
        for name in ["B", "C", "D"] {
            let modified = std::fs::metadata(&target).unwrap().modified().unwrap();
            std::fs::write(&target, format!("provider: codex\nskills: [{name}]\n")).unwrap();
            std::fs::File::options()
                .write(true)
                .open(&target)
                .unwrap()
                .set_modified(modified)
                .unwrap();
            assert_eq!(files.check().0, vec![source.clone()]);
        }
        let next = root.join("next.yaml");
        std::fs::write(&next, "provider: codex\nskills: [E]\n").unwrap();
        std::fs::rename(&next, &target).unwrap();
        assert_eq!(files.check().0, vec![source.clone()]);
        std::fs::write(&next, "provider: codex\nskills: [F]\n").unwrap();
        std::fs::remove_file(&path).unwrap();
        symlink(&next, &path).unwrap();
        assert_eq!(files.check().0, vec![source]);
        assert_eq!(files.check().1.reads, 0);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn identity_filters_session_avatar_and_time_but_tracks_compiler_inputs() {
        let (root, mut files, _) = fixture();
        let path = root.join("agent.yaml");
        let source = Source::Identity(AgentKey {
            project: root.clone(),
            agent: "alice".into(),
        });
        let original = "display-name: Alice\nstatus: active\nrecruited-from: Hero-A\n";
        std::fs::write(&path, original).unwrap();
        files.add(path.clone(), source.clone());
        std::fs::write(
            &path,
            format!("{original}session-id: new\nsession-updated-at: later\navatar-id: new-face\n"),
        )
        .unwrap();
        let (changes, counts) = files.check();
        assert!(changes.is_empty());
        assert_eq!(counts.reads, 1);
        assert_eq!(files.check().1.reads, 0);
        for text in [
            "displayName: Alicia\nstatus: active\nsource: {hero-id: Hero-A}\n",
            "display-name: Alicia\nstatus: archived\nrecruited-from: Hero-A\n",
            "display-name: Alicia\nstatus: archived\nrecruited-from: Hero-B\n",
        ] {
            std::fs::write(&path, text).unwrap();
            assert_eq!(files.check().0, vec![source.clone()]);
        }
        let previous = files.entries[&path].handled.clone();
        std::fs::write(&path, "").unwrap();
        assert!(files.check().0.is_empty());
        assert_eq!(files.entries[&path].handled, previous);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn account_name_changes_broadcast_but_avatar_changes_do_not() {
        let (root, mut files, _) = fixture();
        let path = root.join("account-user.json");
        std::fs::write(&path, r#"{"name":" Alice ","avatarId":"one"}"#).unwrap();
        files.add(path.clone(), Source::AccountIdentity);
        std::fs::write(&path, r#"{"name":"Alice","avatarId":"two"}"#).unwrap();
        assert!(files.check().0.is_empty());
        std::fs::write(&path, r#"{"name":"Bob"}"#).unwrap();
        assert_eq!(files.check().0, vec![Source::AccountIdentity]);
        std::fs::write(&path, "{").unwrap();
        assert!(files.check().0.is_empty());
        std::fs::remove_file(&path).unwrap();
        assert!(files.check().0.is_empty());
        std::fs::write(&path, r#"{"name":"Bob"}"#).unwrap();
        assert_eq!(files.check().0, vec![Source::AccountIdentity]);
        assert_eq!(files.check().1.reads, 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn image_registration_invalidates_once_after_the_initial_capture() {
        let (root, mut files, _) = fixture();
        let path = root.join("picture.png");
        std::fs::write(&path, b"captured").unwrap();
        let captured = stamp(&path).unwrap();
        std::fs::write(&path, b"replaced before watch").unwrap();
        let paths = BTreeSet::from([path.clone()]);
        assert!(files.register_roster_images(paths.clone()));
        assert_ne!(files.entries[&path].handled.as_ref(), Some(&captured));
        assert!(files.check().0.is_empty(), "the first check has no older baseline");
        assert!(!files.register_roster_images(paths), "unchanged paths must not loop");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn roster_fields_are_independent_from_adapter_and_session_changes() {
        let (root, mut files, _) = fixture();
        files.entries.clear();
        let path = root.join("agent.yaml");
        let key = AgentKey {
            project: root.clone(),
            agent: "a".into(),
        };
        std::fs::write(
            &path,
            "display-name: A\nstatus: active\navatar-id: codex\nsession-id: old\n",
        )
        .unwrap();
        files.add(path.clone(), Source::Identity(key.clone()));
        std::fs::write(
            &path,
            "display-name: A\nstatus: active\navatar-id: codex\nsession-id: new\n",
        )
        .unwrap();
        let (changes, counts) = files.check();
        assert!(changes.is_empty() && !files.roster_changed);
        assert_eq!(counts.reads, 1);
        std::fs::write(
            &path,
            "display-name: A\nstatus: active\navatar-id: user:pic\nsession-id: new\n",
        )
        .unwrap();
        assert!(files.check().0.is_empty());
        assert!(files.roster_changed);
        assert_eq!(files.check().1.reads, 0);
        assert!(!files.roster_changed);
        std::fs::write(
            &path,
            "display-name: B\nstatus: active\navatar-id: user:pic\n",
        )
        .unwrap();
        assert_eq!(files.check().0, vec![Source::Identity(key)]);
        assert!(files.roster_changed);
        let image = root.join("picture.png");
        std::fs::write(&image, [0xff; 20]).unwrap();
        files.add(image.clone(), Source::AvatarFile);
        std::fs::write(&image, [0xfe; 20]).unwrap();
        let (changes, counts) = files.check();
        assert_eq!(changes, vec![Source::AvatarFile]);
        assert_eq!(counts.reads, 0, "binary images use metadata only");
        let registration = root.join("workspace.json");
        std::fs::write(&registration, r#"{"projectId":"p","repoFullName":"P"}"#).unwrap();
        files.add(registration.clone(), Source::RosterProject(root.clone()));
        std::fs::write(
            &registration,
            r#"{"projectId":"p","repoFullName":"P","archived":true}"#,
        )
        .unwrap();
        assert_eq!(files.check().0, vec![Source::RosterProject(root.clone())]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn catalog_reconciles_lifecycle_members_without_scanning_directories_on_ticks() {
        let (root, mut files, _) = fixture();
        let agents = root.join(".agent-workspaces");
        let create = |id: &str| {
            let cwd = agents.join(id);
            std::fs::create_dir_all(&cwd).unwrap();
            std::fs::write(
                cwd.join("agent.yaml"),
                "display-name: Test\nstatus: archived\n",
            )
            .unwrap();
            std::fs::write(cwd.join("SHELL.yaml"), "provider: codex\nskills: []\n").unwrap();
            cwd
        };
        std::fs::write(
            root.join("workspace.json"),
            r#"{"projectId":"p","repoFullName":"P"}"#,
        )
        .unwrap();
        let alice = create("alice");
        files.register_project(&root).unwrap();
        assert_eq!(files.entries.len(), 3);
        let bob = create("bob");
        assert_eq!(files.check().1.stats, 3);
        assert_eq!(files.entries.len(), 3);
        files.register_project(&root).unwrap();
        assert_eq!(files.entries.len(), 5);
        std::fs::remove_file(bob.join("agent.yaml")).unwrap();
        files.register_project(&root).unwrap();
        assert_eq!(files.entries.len(), 5);
        std::fs::remove_dir_all(alice).unwrap();
        files.register_project(&root).unwrap();
        assert_eq!(files.entries.len(), 3);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "manual read-only local account benchmark; waits for 2-second periods"]
    fn configured_account_poll_performance() {
        use std::time::{Duration, Instant};
        let mut files = FileSet::default();
        files.add(crate::account_user_identity_path(), Source::AccountIdentity);
        for root in crate::registered_adapter_projects().unwrap() {
            files.register_project(&root).unwrap();
        }
        let mut samples = Vec::new();
        for _ in 0..12 {
            std::thread::sleep(Duration::from_secs(2));
            let start_cpu = super::super::performance::cpu_ms();
            let start = Instant::now();
            let (changes, counts) = files.check();
            samples.push(serde_json::json!({"milliseconds":start.elapsed().as_secs_f64()*1000.0,"cpu_ms":super::super::performance::cpu_ms()-start_cpu,"metadata":counts.stats,"reads":counts.reads,"changes":changes.len()}));
        }
        println!(
            "{}",
            serde_json::json!({"benchmark":"fixed-config-worker-path", "files":files.entries.len(), "projects":files.projects.len(), "interval_seconds":2, "samples":samples})
        );
    }
}
