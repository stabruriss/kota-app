use super::*;
use crate::violet::NoteChange;
use serde_json::Value;

fn setup(f: &Fixture, shell: &str) -> Env {
    let mut env = f.env(false);
    let root = f.root.join("project");
    fs::create_dir_all(&root).unwrap();
    fs::write(f.agent().join("SHELL.yaml"), format!("provider: {shell}\n")).unwrap();
    fs::write(f.agent().join("agent.yaml"), "id: agent-fixture\n").unwrap();
    env.project_root = Some(root);
    env
}

fn events(env: &Env) -> Vec<Value> {
    fs::read_to_string(env.project_root.as_ref().unwrap().join("project-memory/chathistory/latest.jsonl"))
        .unwrap_or_default().lines().map(|line| serde_json::from_str(line).unwrap()).collect()
}

fn run(env: &Env, args: &[&str], stdin: &str) -> Result<String, String> {
    cli::run(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>(), env, stdin, T1)
}

#[test]
fn notes_cli_and_local_tool_use_saved_text_and_provider_without_dispatching() {
    for shell in ["codex", "kota-local"] {
        let f = Fixture::new();
        let env = setup(&f, shell);
        // The Local tool supplies the same CLI command and a -- separator.
        let reply = run(&env, &["add-mark", "--", "  When editing\n  `file.rs`,  verify it.  "], "").unwrap();
        let saved = marks::read(&f.paths()).unwrap();
        let rows = events(&env);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["text"], saved.marks[0].text);
        assert_eq!(rows[0]["text"], "When editing `file.rs`, verify it.");
        assert_eq!(rows[0]["kind"], "commentary");
        assert_eq!(rows[0]["role"], "assistant");
        assert_eq!(rows[0]["shell"], shell);
        assert_eq!(rows[0]["message_origin"], "note");
        assert_eq!(rows[0]["agent_id"], "agent-fixture");
        assert_eq!(rows[0]["agent_visible"], false);
        assert!(rows[0].get("target_agent_ids").is_none_or(|v| v.as_array().is_some_and(Vec::is_empty)));
        assert!(crate::violet::latest_reply(env.project_root.as_ref().unwrap(), "agent-fixture").is_none());
        assert_eq!(reply, format!("Added {} (revision 1). 1 of 20 marks in use.\n{}  2026-10-14  {}",
            saved.marks[0].id, saved.marks[0].id, saved.marks[0].text));
    }
}

#[test]
fn note_progress_is_deduplicated_but_a_reused_mark_id_with_new_revision_is_not() {
    let f = Fixture::new();
    let env = setup(&f, "codex");
    run(&env, &["add-mark", "Keep the saved text."], "").unwrap();
    let saved = marks::read(&f.paths()).unwrap();
    let root = env.project_root.as_ref().unwrap();
    crate::violet::record_note_change(root, "agent-fixture", "codex", NoteChange::Added, &saved.marks[0], 1, T1).unwrap();
    assert_eq!(events(&env).len(), 1);
    let mut replacement = saved.marks[0].clone();
    replacement.text = "A later note with a recycled id.".into();
    crate::violet::record_note_change(root, "agent-fixture", "codex", NoteChange::Added, &replacement, 3, T1).unwrap();
    assert_eq!(events(&env).len(), 2);
    assert_ne!(events(&env)[0]["id"], events(&env)[1]["id"]);
}

#[test]
fn edits_and_deletes_report_the_mark_text_at_the_time_of_the_change() {
    for shell in ["codex", "kota-local"] {
        let f = Fixture::new();
        let env = setup(&f, shell);
        run(&env, &["add-mark", "First wording."], "").unwrap();
        let id = marks::read(&f.paths()).unwrap().marks[0].id.clone();
        let at = |ts: &str, args: &[&str]| cli::run(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>(), &env, "", ts).unwrap();
        // The Local tool supplies the same CLI command and a -- separator.
        at(T2, &["edit-mark", &id, "--", "  Second\n  wording.  "]);
        assert_eq!(marks::read(&f.paths()).unwrap().marks[0].text, "Second wording.");
        at(T3, &["delete-mark", &id]);
        assert!(marks::read(&f.paths()).unwrap().marks.is_empty());
        let rows = events(&env);
        assert_eq!(rows.len(), 3);
        // One row per saved change: what it did, the text as saved (for a delete, as it was), dated when it changed.
        for (row, (origin, text, ts, revision)) in rows.iter().zip([("note", "First wording.", T1, 1),
            ("note_edit", "Second wording.", T2, 2), ("note_delete", "Second wording.", T3, 3)]) {
            assert_eq!(row["message_origin"], origin);
            assert_eq!(row["text"], text);
            assert_eq!(row["ts"], ts);
            assert_eq!(row["source"]["native_event_id"], format!("note:{id}:{revision}"));
            assert_eq!(row["kind"], "commentary");
            assert_eq!(row["role"], "assistant");
            assert_eq!(row["shell"], shell);
            assert_eq!(row["agent_id"], "agent-fixture");
            assert_eq!(row["agent_visible"], false);
        }
        assert!(crate::violet::latest_reply(env.project_root.as_ref().unwrap(), "agent-fixture").is_none());
    }
}

#[test]
fn refusals_and_a_missing_project_do_not_report_a_note() {
    let f = Fixture::new();
    let env = setup(&f, "codex");
    run(&env, &["add-mark"], "  ").unwrap();
    run(&env, &["add-mark", &"中".repeat(201)], "").unwrap();
    assert!(events(&env).is_empty());
    for index in 0..20 { marks::add(&f.paths(), &format!("Note {index}"), None, T1).unwrap(); }
    run(&env, &["add-mark", "Full."], "").unwrap();
    let id = marks::read(&f.paths()).unwrap().marks[0].id.clone();
    run(&env, &["edit-mark", &id], "  ").unwrap();
    run(&env, &["edit-mark", &id, &"中".repeat(201)], "").unwrap();
    run(&env, &["edit-mark", "m-missing", "Changed."], "").unwrap();
    run(&env, &["delete-mark", "m-missing"], "").unwrap();
    assert_eq!(marks::read(&f.paths()).unwrap().revision, 20);
    assert!(events(&env).is_empty());
    let mut outside = env;
    outside.project_root = None;
    run(&outside, &["edit-mark", &id, "Changed outside the project."], "").unwrap();
    run(&outside, &["delete-mark", &id], "").unwrap();
    run(&outside, &["add-mark", "Outside the project."], "").unwrap();
    assert_eq!(marks::read(&f.paths()).unwrap().revision, 23);
    assert!(!f.root.join("project/project-memory").exists());
}

#[test]
fn notification_io_failure_preserves_success_and_note_bytes() {
    let f = Fixture::new();
    let env = setup(&f, "codex");
    fs::write(env.project_root.as_ref().unwrap().join("project-memory"), "blocked").unwrap();
    let reply = run(&env, &["add-mark", "The save must still succeed."], "").unwrap();
    let saved = marks::read(&f.paths()).unwrap();
    assert_eq!(saved.marks.len(), 1);
    assert_eq!(saved.marks[0].text, "The save must still succeed.");
    assert_eq!(reply, format!("Added {} (revision 1). 1 of 20 marks in use.\n{}  2026-10-14  {}",
        saved.marks[0].id, saved.marks[0].id, saved.marks[0].text));
    assert!(events(&env).is_empty());
}

#[test]
fn mark_write_failure_has_no_progress() {
    let f = Fixture::new();
    let env = setup(&f, "codex");
    fs::create_dir_all(f.paths().marks()).unwrap();
    assert!(run(&env, &["add-mark", "Cannot save."], "").is_err());
    assert!(events(&env).is_empty());
}
