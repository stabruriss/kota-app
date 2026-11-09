use super::*;
use crate::{ProjectAgentDetail, ProjectAgentNameFieldsWire};
use std::path::PathBuf;
use std::sync::{Arc, Barrier};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("kota-tavern-invite-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn detail(root: &Path) -> ProjectAgentDetail {
    let shell_path = root.join("SHELL.yaml");
    fs::write(
        &shell_path,
        "provider: codex\nmodel: custom-model\n# keep exact shell\n",
    )
    .unwrap();
    ProjectAgentDetail {
        shell_revision: None,
        applied_to_session: None,
        agent_id: "agent-piner".into(),
        display_name: "Dr. 颦儿-绛珠 v. Kota".into(),
        name_fields: Some(ProjectAgentNameFieldsWire {
            title_id: Some("doctor".into()),
            given: "颦儿".into(),
            middle: "绛珠".into(),
            surname: "v. Kota".into(),
        }),
        source_hero_id: "hero-dex".into(),
        source_hero_name: "Dex".into(),
        project_id: "project-kota".into(),
        project_name: "Kota".into(),
        cli: crate::pty::agent::AgentCli::Codex,
        provider: "codex".into(),
        model: "custom-model".into(),
        effort: Some("high".into()), auto_route: None,
        avatar_id: Some("user:portrait".into()),
        skills: vec!["skill-one".into()],
        args: vec![],
        ghost: "A personal GHOST shared by several incarnations".into(),
        adapter_path: root.join("AGENTS.md").to_string_lossy().into_owned(),
        shell_path: shell_path.to_string_lossy().into_owned(),
        agent_yaml_path: root.join("agent.yaml").to_string_lossy().into_owned(),
        notes: crate::project_agent_notes(root),
        status: "active".into(),
        archived_at: None,
        invite_eligibility: eligibility(
            root,
            hero_id(Some("project-kota"), root, "agent-piner"),
            "Dr. 颦儿-绛珠 v. Kota",
            "persona",
        )
        .unwrap(),
        record: crate::ProjectAgentRecord {
            turns: 12,
            incarnations: 1,
            estimated_tokens: 123,
            commends: 2,
            last_active_at: None,
        },
        session_id: Some("old-session".into()),
        forkable: true,
        session_source: None,
        dirty: false,
        dirty_summary: String::new(),
    }
}

fn profile(root: &Path) -> TavernHeroProfileDraft {
    snapshot(&detail(root), None).unwrap()
}

#[test]
fn invitation_keeps_current_name_fields_and_profile_not_source_hero_defaults() {
    let temp = Temp::new();
    let source = detail(&temp.0);
    let profile = snapshot(&source, None).unwrap();
    let saved = save(&temp.0, profile).unwrap();
    let restored = existing(&temp.0, &saved.hero_id).unwrap().unwrap();
    assert_eq!(restored.name, source.display_name);
    assert!(!restored.name.contains("Dex"));
    assert_eq!(
        serde_json::to_value(&restored.name_fields).unwrap(),
        serde_json::to_value(&source.name_fields).unwrap()
    );
    assert_eq!(restored.ghost, source.ghost);
    assert_eq!(restored.provider, source.provider);
    assert_eq!(restored.model, source.model);
    assert_eq!(restored.effort, source.effort);
    assert_eq!(restored.avatar_id, source.avatar_id);
    assert_eq!(restored.skills, source.skills);
    assert_eq!(
        restored.shell,
        fs::read_to_string(&source.shell_path).unwrap()
    );
    assert_eq!(
        fs::read_to_string(temp.0.join(&saved.hero_id).join("GHOST.md")).unwrap(),
        restored.ghost
    );
    assert_eq!(
        fs::read_to_string(temp.0.join(&saved.hero_id).join("SHELL.yaml")).unwrap(),
        restored.shell
    );
    assert!(restored.source.is_some());
}

#[test]
fn concurrent_and_later_invites_reuse_one_hero_without_overwriting_it() {
    let temp = Temp::new();
    let draft = profile(&temp.0);
    let barrier = Arc::new(Barrier::new(8));
    let replies = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let barrier = barrier.clone();
                let draft = draft.clone();
                let root = &temp.0;
                scope.spawn(move || {
                    barrier.wait();
                    save(root, draft).unwrap()
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(
        replies
            .iter()
            .filter(|r| r.duplicate_hero_id.is_none())
            .count(),
        1
    );
    assert!(replies
        .iter()
        .all(|r| r.hero_id == draft.hero_id && r.display_name == draft.name));
    let path = temp.0.join(&draft.hero_id).join("hero.json");
    let bytes = fs::read(&path).unwrap();
    let mut changed = draft.clone();
    changed.name = "Renamed incarnation".into();
    changed.ghost = "Updated persona".into();
    let again = save(&temp.0, changed).unwrap();
    assert_eq!(again.hero_id, draft.hero_id);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    let reopened = eligibility(&temp.0, draft.hero_id, "Renamed incarnation", "").unwrap();
    assert!(!reopened.eligible);
    assert_eq!(
        reopened.duplicate_hero_id.as_deref(),
        Some(again.hero_id.as_str())
    );
    assert_eq!(
        fs::read_dir(&temp.0)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().join("hero.json").is_file())
            .count(),
        1
    );
}

#[test]
fn equal_ghosts_on_distinct_incarnations_are_not_duplicates_and_system_dirs_are_skipped() {
    let temp = Temp::new();
    fs::create_dir(temp.0.join("system-bbs")).unwrap();
    let first = profile(&temp.0);
    save(&temp.0, first.clone()).unwrap();
    for (project, agent) in [
        ("project-kota", "agent-other"),
        ("project-other", "agent-piner"),
    ] {
        let id = hero_id(Some(project), &temp.0, agent);
        assert!(
            eligibility(&temp.0, id.clone(), &first.name, &first.ghost)
                .unwrap()
                .eligible
        );
        let mut next = first.clone();
        next.hero_id = id;
        let saved = save(&temp.0, next).unwrap();
        assert_ne!(saved.hero_id, first.hero_id);
        assert_ne!(saved.display_name, first.name);
    }
    assert_eq!(
        hero_id(Some("project-kota"), Path::new("/moved"), "agent-piner"),
        first.hero_id
    );
    assert_ne!(
        hero_id(None, Path::new("/one"), "agent-piner"),
        hero_id(None, Path::new("/two"), "agent-piner")
    );
}

#[test]
fn failed_publication_leaves_no_visible_hero_and_retry_succeeds() {
    let temp = Temp::new();
    let draft = profile(&temp.0);
    let target = temp.0.join(&draft.hero_id);
    let staging = temp.0.join(".invite-staging").join(&draft.hero_id);
    // Fail after GHOST was written but before the complete profile could be published.
    fs::create_dir_all(staging.join("SHELL.yaml")).unwrap();
    assert!(save(&temp.0, draft.clone()).is_err());
    assert!(!target.exists());
    assert!(!staging.exists());
    save(&temp.0, draft.clone()).unwrap();
    assert!(existing(&temp.0, &draft.hero_id).unwrap().is_some());
}

#[test]
fn corrupt_existing_invitation_is_an_error_not_permission_to_make_another() {
    let temp = Temp::new();
    let draft = profile(&temp.0);
    save(&temp.0, draft.clone()).unwrap();
    let path = temp.0.join(&draft.hero_id).join("hero.json");
    fs::write(&path, "corrupt").unwrap();
    assert!(eligibility(&temp.0, draft.hero_id.clone(), &draft.name, &draft.ghost).is_err());
    assert!(save(&temp.0, draft).is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), "corrupt");
}

#[test]
fn reincarnation_writes_its_own_name_fields_without_changing_template_fields() {
    let temp = Temp::new();
    let profile = profile(&temp.0);
    let source_fields = serde_json::to_value(&profile.name_fields).unwrap();
    let mut fields = profile.name_fields.clone().unwrap();
    fields.given.push_str(" II");
    fields.surname = "v. Other".into();
    let request = crate::TavernIncarnateHeroRequest {
        agent_id: "agent-new".into(),
        template_id: profile.hero_id.clone(),
        display_name: "Dr. 颦儿 II-绛珠 v. Other".into(),
        name_fields: Some(fields),
        project_root: None,
        progress_id: None,
        profile,
    };
    let shell = crate::parse_shell_yaml(&request.profile.shell).unwrap();
    let yaml =
        crate::compile_agent_yaml(&request, &shell, crate::pty::agent::AgentCli::Codex, None, None);
    let parsed: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(
        parsed["display-name"].as_str(),
        Some(request.display_name.as_str())
    );
    assert_eq!(
        parsed["display-name-fields"]["given"].as_str(),
        Some("颦儿 II")
    );
    assert_eq!(
        parsed["display-name-fields"]["middle"].as_str(),
        Some("绛珠")
    );
    assert_eq!(
        parsed["display-name-fields"]["surname"].as_str(),
        Some("v. Other")
    );
    assert_eq!(
        serde_json::to_value(&request.profile.name_fields).unwrap(),
        source_fields
    );
    assert!(!yaml.contains("old-session"));
}
