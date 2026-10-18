//! Heroes that follow an incarnation (SOUL-PLAN v4 §17): states, hero.json, saves,
//! relics on dismissal, provenance and carrying a soul. Synthetic directories only.
use super::*;
use std::path::PathBuf;

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!("kota-tavern-soul-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn draft(hero_id: &str, source: Option<TavernHeroSource>) -> TavernHeroProfileDraft {
    TavernHeroProfileDraft {
        hero_id: hero_id.into(),
        name: "Hero".into(),
        name_fields: None,
        provider: "codex".into(),
        model: "default".into(),
        effort: None,
        auto_route: None,
        avatar_id: None,
        skills: Vec::new(),
        ghost: "ghost".into(),
        shell: "provider: codex\n".into(),
        archived: false,
        dismissed: false,
        kind: Some("invited".into()),
        source,
    }
}

fn source(cwd: &Path) -> TavernHeroSource {
    TavernHeroSource {
        project_id: Some("project-fixture".into()),
        agent_id: "agent-one".into(),
        agent_cwd: cwd.to_string_lossy().into_owned(),
        dismissed_at: None,
    }
}

fn incarnation(root: &Path, cwd: &Path) -> IncarnationContext {
    IncarnationContext {
        project_root: root.to_path_buf(),
        source_dir: root.to_path_buf(),
        cwd: cwd.to_path_buf(),
        worktree_root: cwd.join("project-files"),
        shared_dir: root.join("project-memory"),
        rules_dir: root.join("project-rules"),
        project_id: Some("project-fixture".into()),
        project_remote: None,
        project_base_ref: "HEAD".into(),
    }
}

fn snapshot(cwd: &Path) -> SourceSnapshot {
    SourceSnapshot {
        agent_id: "agent-one".into(),
        display_name: "Final Name".into(),
        name_fields: None,
        provider: "codex".into(),
        model: "final-model".into(),
        effort: Some("high".into()),
        auto_route: None,
        avatar_id: Some("user:portrait".into()),
        skills: vec!["skill-z".into()],
        ghost: "final ghost".into(),
        shell_path: cwd.join("SHELL.yaml").to_string_lossy().into_owned(),
    }
}

fn soul_fixture(cwd: &Path) {
    fs::create_dir_all(cwd.join("under-table/soul")).unwrap();
    fs::write(
        cwd.join("under-table/soul/marks.json"),
        r#"{"version":1,"revision":1,"marks":[{"id":"m-20261015-0001","text":"a mark","created_at":"2026-10-15T00:00:00Z","updated_at":"2026-10-15T00:00:00Z","project":"project-fixture","used":0}]}"#,
    )
    .unwrap();
    fs::create_dir_all(cwd.join("under-table/episodes")).unwrap();
    // The second line is torn: a writer was mid-append.
    fs::write(
        cwd.join("under-table/episodes/episodes.jsonl"),
        "{\"id\":\"violet-1\",\"kind\":\"message\",\"project\":\"project-fixture\",\"ts\":\"2026-10-15T00:00:00Z\",\"text\":\"hello\",\"indexed\":true,\"w0\":0.5}\n{\"id\":\"violet-2\",\"kind\":\"mes",
    )
    .unwrap();
    fs::write(cwd.join("under-table/episodes/index.sqlite"), b"cache").unwrap();
    fs::write(cwd.join("under-table/episodes/.lock"), b"").unwrap();
    fs::write(
        cwd.join("under-table/episodes/state.json"),
        r#"{"version":1,"current_project":"project-fixture","projects":{"project-fixture":{"name":"project","root":"/old","stints":[{"from":"2026-09-01T00:00:00Z","to":null}],"cursor":{"segment":"2026-10-14.jsonl","violet_seq":9},"last_export":"2026-10-14T03:00:00Z","episodes":1}},"old_dreams":{"lines":4}}"#,
    )
    .unwrap();
    fs::write(cwd.join("SHELL.yaml"), "provider: codex\nmodel: final-model\n").unwrap();
}

#[test]
fn hero_source_state_follows_the_source_directory() {
    let t = Temp::new("state");
    let cwd = t.0.join("project/.agent-workspaces/agent-one");
    assert_eq!(tavern_hero_source_state(&draft("h", None)), TavernHeroSourceState::Native);
    assert_eq!(tavern_hero_source_state(&draft("h", Some(source(&cwd)))), TavernHeroSourceState::Dangling);
    fs::create_dir_all(&cwd).unwrap();
    fs::write(cwd.join("agent.yaml"), "id: agent-one\nstatus: archived\n").unwrap();
    assert_eq!(tavern_hero_source_state(&draft("h", Some(source(&cwd)))), TavernHeroSourceState::Linked);
    let mut relic = source(&cwd);
    relic.dismissed_at = Some("2026-10-15T20:00:00Z".into());
    assert_eq!(tavern_hero_source_state(&draft("h", Some(relic))), TavernHeroSourceState::Relic);
    assert_eq!(tavern_source_project_name(&source(&cwd)), "project-fixture");
    let mut unregistered = source(&cwd);
    unregistered.project_id = None;
    assert_eq!(tavern_source_project_name(&unregistered), "project");
    assert_eq!(tavern_source_project_root(&source(&cwd)).unwrap(), t.0.join("project"));
    assert_eq!(TavernHeroSourceState::Dangling.as_str(), "dangling");
}

#[test]
fn hero_json_round_trips_source_and_ignores_derived_and_legacy_fields() {
    let t = Temp::new("json");
    let profile = draft("hero-invited-x", Some(source(&t.0)));
    save_tavern_hero_profile_in(&t.0, &profile).unwrap();
    let text = fs::read_to_string(t.0.join("hero-invited-x/hero.json")).unwrap();
    assert!(text.contains("\"source\": {") && text.contains("\"agentCwd\"") && !text.contains("record"), "{text}");
    let loaded = load_tavern_hero_profile(&t.0.join("hero-invited-x")).unwrap().unwrap();
    assert_eq!(loaded.source, profile.source);
    let legacy = r#"{"heroId":"hero-invited-y","name":"Y","provider":"codex","model":"default","skills":[],"ghost":"g","shell":"provider: codex\n","record":{"turns":1,"incarnations":1,"estimatedTokens":0,"commends":0},"sourceState":"linked","sourceProjectName":"X"}"#;
    fs::create_dir_all(t.0.join("hero-invited-y")).unwrap();
    fs::write(t.0.join("hero-invited-y/hero.json"), legacy).unwrap();
    let loaded = load_tavern_hero_profile(&t.0.join("hero-invited-y")).unwrap().unwrap();
    assert!(loaded.source.is_none());
    let listing = serde_json::to_value(TavernHeroListing {
        profile: loaded,
        source_state: "native".into(),
        source_project_name: None,
        source_reason: None,
    })
    .unwrap();
    assert_eq!(listing["sourceState"], "native");
    assert_eq!(listing["heroId"], "hero-invited-y");
    assert!(listing.get("record").is_none() && listing.get("sourceReason").is_none());
}

#[test]
fn save_keeps_only_tavern_flags_for_a_followed_hero_and_everything_for_a_relic() {
    let t = Temp::new("save");
    let cwd = t.0.join("project/.agent-workspaces/agent-one");
    fs::create_dir_all(&cwd).unwrap();
    fs::write(cwd.join("agent.yaml"), "id: agent-one\nstatus: active\n").unwrap();
    save_tavern_hero_profile_in(&t.0, &draft("hero-linked", Some(source(&cwd)))).unwrap();
    let mut incoming = draft("hero-linked", None);
    incoming.name = "Edited".into();
    incoming.ghost = "edited ghost".into();
    incoming.archived = true;
    let merged = tavern_profile_for_save_in(&t.0, incoming).unwrap();
    assert_eq!((merged.name.as_str(), merged.ghost.as_str(), merged.archived, merged.source.is_some()), ("Hero", "ghost", true, true));
    // Dangling behaves the same: the saved snapshot stands, only the flags move.
    fs::remove_dir_all(&cwd).unwrap();
    let mut incoming = draft("hero-linked", None);
    incoming.dismissed = true;
    let merged = tavern_profile_for_save_in(&t.0, incoming).unwrap();
    assert!(merged.dismissed && !merged.archived && merged.name == "Hero");
    let mut relic_source = source(&cwd);
    relic_source.dismissed_at = Some("2026-10-15T20:00:00Z".into());
    save_tavern_hero_profile_in(&t.0, &draft("hero-relic", Some(relic_source.clone()))).unwrap();
    let mut incoming = draft("hero-relic", None);
    incoming.name = "Edited".into();
    let merged = tavern_profile_for_save_in(&t.0, incoming).unwrap();
    assert_eq!((merged.name.as_str(), merged.source), ("Edited", Some(relic_source)));
    let native = tavern_profile_for_save_in(&t.0, draft("hero-new", None)).unwrap();
    assert!(native.source.is_none());
}

#[test]
fn dismissal_keeps_the_soul_and_final_files_as_a_relic() {
    let t = Temp::new("relic");
    let root = t.0.join("project");
    let cwd = root.join(".agent-workspaces/agent-one");
    soul_fixture(&cwd);
    let ctx = incarnation(&root, &cwd);
    let heroes = t.0.join("heroes");
    // Not in Tavern: nothing to keep, the dismissal goes on.
    keep_relic_before_dismiss_in(&heroes, &ctx, &snapshot(&cwd), false).unwrap();
    assert!(!heroes.exists());
    let hero_id = tavern_invite::hero_id(Some("project-fixture"), &root, "agent-one");
    let mut profile = draft(&hero_id, Some(source(&cwd)));
    profile.name = "Old Name".into();
    save_tavern_hero_profile_in(&heroes, &profile).unwrap();
    keep_relic_before_dismiss_in(&heroes, &ctx, &snapshot(&cwd), false).unwrap();
    let dir = heroes.join(sanitize_tavern_hero_id(&hero_id));
    let relic = load_tavern_hero_profile(&dir).unwrap().unwrap();
    assert_eq!(
        (relic.name.as_str(), relic.model.as_str(), relic.effort.as_deref(), relic.skills.as_slice(), relic.ghost.as_str()),
        ("Final Name", "final-model", Some("high"), &["skill-z".to_string()][..], "final ghost")
    );
    assert_eq!(relic.shell, "provider: codex\nmodel: final-model\n");
    assert!(relic.source.as_ref().unwrap().dismissed_at.is_some());
    assert_eq!(tavern_hero_source_state(&relic), TavernHeroSourceState::Relic);
    assert_eq!(fs::read_to_string(dir.join("GHOST.md")).unwrap(), "final ghost");
    assert!(dir.join("under-table/soul/marks.json").is_file());
    assert_eq!(fs::read_to_string(dir.join("under-table/episodes/episodes.jsonl")).unwrap().lines().count(), 1);
    assert!(!dir.join("under-table/episodes/index.sqlite").exists() && !dir.join("under-table/episodes/.lock").exists());
    assert_eq!(soul::carry::counts(&dir), soul::carry::SoulCounts { marks: 1, episodes: 1 });
    let state = soul::export::read_state(&soul::SoulPaths::new(&dir)).unwrap();
    assert_eq!(state.projects["project-fixture"].cursor.violet_seq, 9);
    // Already a relic: dismissing the same id again changes nothing.
    let before = fs::read_to_string(dir.join("hero.json")).unwrap();
    keep_relic_before_dismiss_in(&heroes, &ctx, &snapshot(&cwd), false).unwrap();
    assert_eq!(fs::read_to_string(dir.join("hero.json")).unwrap(), before);
}

#[test]
fn a_relic_copy_that_cannot_be_written_blocks_the_dismissal() {
    let t = Temp::new("blocked");
    let root = t.0.join("project");
    let cwd = root.join(".agent-workspaces/agent-one");
    soul_fixture(&cwd);
    let heroes = t.0.join("heroes");
    let hero_id = tavern_invite::hero_id(Some("project-fixture"), &root, "agent-one");
    save_tavern_hero_profile_in(&heroes, &draft(&hero_id, Some(source(&cwd)))).unwrap();
    let dir = heroes.join(sanitize_tavern_hero_id(&hero_id));
    fs::write(dir.join("under-table"), b"not a directory").unwrap();
    let error = keep_relic_before_dismiss_in(&heroes, &incarnation(&root, &cwd), &snapshot(&cwd), false).unwrap_err();
    assert!(error.starts_with("Could not save this incarnation's soul to the Tavern. (") && error.ends_with(')'), "{error}");
    let untouched = load_tavern_hero_profile(&dir).unwrap().unwrap();
    assert!(untouched.source.clone().unwrap().dismissed_at.is_none() && untouched.name == "Hero");
    // Dismiss anyway: the soul is given up, but the hero still becomes a relic with its
    // final files, never a link to a directory that is about to vanish.
    keep_relic_before_dismiss_in(&heroes, &incarnation(&root, &cwd), &snapshot(&cwd), true).unwrap();
    let relic = load_tavern_hero_profile(&dir).unwrap().unwrap();
    assert_eq!(tavern_hero_source_state(&relic), TavernHeroSourceState::Relic);
    assert_eq!((relic.name.as_str(), relic.model.as_str(), relic.ghost.as_str()), ("Final Name", "final-model", "final ghost"));
    assert!(!soul::carry::has_soul(&dir));
    assert_eq!(soul::carry::counts(&dir), soul::carry::SoulCounts::default());
}

#[test]
fn a_forced_dismissal_never_blocks_even_when_the_relic_cannot_be_written() {
    let t = Temp::new("forced");
    let root = t.0.join("project");
    let cwd = root.join(".agent-workspaces/agent-one");
    soul_fixture(&cwd);
    let heroes = t.0.join("heroes");
    let hero_id = tavern_invite::hero_id(Some("project-fixture"), &root, "agent-one");
    save_tavern_hero_profile_in(&heroes, &draft(&hero_id, Some(source(&cwd)))).unwrap();
    let dir = heroes.join(sanitize_tavern_hero_id(&hero_id));
    // The hero directory cannot take the final files any more (hero.json is now a directory).
    fs::remove_file(dir.join("hero.json")).unwrap();
    fs::create_dir_all(dir.join("hero.json")).unwrap();
    // Without force the lookup failure blocks (whether a hero exists is unknown); with force nothing blocks.
    assert!(keep_relic_before_dismiss_in(&heroes, &incarnation(&root, &cwd), &snapshot(&cwd), false).unwrap_err().starts_with("Could not save this incarnation's soul to the Tavern. ("));
    assert!(keep_relic_before_dismiss_in(&heroes, &incarnation(&root, &cwd), &snapshot(&cwd), true).is_ok());
}

#[test]
fn agent_yaml_records_the_source_incarnation_only_when_it_has_one() {
    let shell = parse_shell_yaml("provider: codex\n").unwrap();
    let request = TavernIncarnateHeroRequest {
        agent_id: "agent-new".into(),
        template_id: "incarnation".into(),
        display_name: "Copy".into(),
        name_fields: None,
        project_root: None,
        progress_id: None,
        profile: draft("hero-x", None),
    };
    let lineage = IncarnationLineage { soul: None, project_id: Some("project-fixture".into()), agent_id: "agent-one".into() };
    let yaml = compile_agent_yaml(&request, &shell, pty::agent::AgentCli::Codex, None, Some(&lineage));
    assert!(yaml.contains("recruited-from: incarnation\n"), "{yaml}");
    assert!(yaml.contains("source-project-id: \"project-fixture\"\nsource-agent-id: \"agent-one\"\nsource:\n"), "{yaml}");
    let unregistered = IncarnationLineage { soul: None, project_id: None, agent_id: "agent-one".into() };
    assert!(compile_agent_yaml(&request, &shell, pty::agent::AgentCli::Codex, None, Some(&unregistered)).contains("source-project-id: null\n"));
    let plain = compile_agent_yaml(&request, &shell, pty::agent::AgentCli::Codex, None, None);
    assert!(!plain.contains("source-project-id") && !plain.contains("source-agent-id"));
}

#[test]
fn carrying_a_soul_opens_a_stint_in_the_new_project_and_rollback_leaves_nothing() {
    let t = Temp::new("carry");
    let old = t.0.join("old/.agent-workspaces/agent-one");
    soul_fixture(&old);
    let root = t.0.join("new");
    let cwd = root.join(".agent-workspaces/agent-two");
    fs::create_dir_all(&cwd).unwrap();
    let mut ctx = incarnation(&root, &cwd);
    ctx.project_id = Some("project-new".into());
    carry_soul_into(&ctx, &old).unwrap();
    let state = soul::export::read_state(&soul::SoulPaths::new(&cwd)).unwrap();
    assert_eq!(state.current_project.as_deref(), Some("project-new"));
    assert!(state.projects["project-fixture"].stints[0].to.is_some());
    let fresh = &state.projects["project-new"];
    assert_eq!((fresh.name.as_str(), fresh.stints.len(), fresh.stints[0].to.is_none(), fresh.cursor.violet_seq), ("new", 1, true, 0));
    assert_eq!(soul::carry::counts(&cwd), soul::carry::SoulCounts { marks: 1, episodes: 1 });
    // Carrying into a place that already has a soul is refused, never merged.
    assert!(carry_soul_into(&ctx, &old).unwrap_err().contains("already has an under-table"));
    // Rollback removes the half-made incarnation; no git here, so only the directory goes.
    roll_back_new_incarnation(&ctx, "agent-two");
    assert!(!cwd.exists());
}
