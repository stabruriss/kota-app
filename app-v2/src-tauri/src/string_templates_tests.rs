use super::*;
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self { Self(std::env::temp_dir().join(format!("kota-string-templates-{}", uuid::Uuid::new_v4()))) }
    fn put(&self, bytes: &[u8]) { fs::create_dir_all(path(&self.0).parent().unwrap()).unwrap(); fs::write(path(&self.0), bytes).unwrap(); }
}
impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
fn template(name: &str) -> Template {
    Template { id: uuid::Uuid::new_v4().to_string(), name: name.into(), saved_at: "2026-01-01T00:00:00Z".into(), group: crate::local_rpc::auto_route::fixture_group() }
}

#[test]
fn defaults_do_not_write_and_round_trip_preserves_groups_and_full_model_ids() {
    let f = Fixture::new(); let mut library = read(&f.0).unwrap();
    assert_eq!(library.revision, "missing"); assert!(!f.0.exists());
    let mut item = template("  Cafe\u{301}  ");
    item.group.models[0].provider_id = "magpie".into();
    item.group.models[0].model_id = "anthropic/model:beta".into();
    library.templates.push(item);
    let saved = save(&f.0, library).unwrap();
    assert_eq!(saved.templates[0].name, "Café");
    assert_eq!(read(&f.0).unwrap(), saved);
    assert_eq!(saved.templates[0].group.models[0].model_id, "anthropic/model:beta");
    assert_eq!(serde_json::to_value(&saved).unwrap()["templates"][0]["savedAt"], "2026-01-01T00:00:00Z");
}

#[test]
fn unicode_name_collisions_return_existing_id_and_do_not_write() {
    for (a,b) in [("Café", " CAFE\u{301} "), ("String 1", "string 1"), ("Σ", "σ")] {
        let f = Fixture::new(); let mut library = read(&f.0).unwrap();
        library.templates.push(template(a)); let mut saved = save(&f.0, library).unwrap();
        let before = fs::read(path(&f.0)).unwrap(); let id = saved.templates[0].id.clone();
        saved.templates.push(template(b));
        let failure = save(&f.0, saved).unwrap_err();
        assert_eq!(failure.code, "string_template_name_exists"); assert_eq!(failure.duplicate_id, Some(id));
        assert_eq!(fs::read(path(&f.0)).unwrap(), before);
    }
}

#[test]
fn overwrite_preserves_id_delete_and_stale_cross_app_cas() {
    let f = Fixture::new(); let mut initial = read(&f.0).unwrap();
    initial.templates.push(template("String 1")); let first = save(&f.0, initial).unwrap();
    let mut changed = first.clone(); changed.templates[0].group.threshold = 0.9;
    let second = save(&f.0, changed).unwrap();
    assert_eq!(second.templates[0].id, first.templates[0].id);
    assert_ne!(first.revision, second.revision);
    assert_eq!(save(&f.0, first).unwrap_err().code, "string_templates_conflict");
    let mut deleted = second; deleted.templates.clear();
    assert!(save(&f.0, deleted).unwrap().templates.is_empty());
}

#[test]
fn external_edit_without_revision_bump_still_invalidates_cas() {
    let f = Fixture::new(); let mut library = read(&f.0).unwrap(); library.templates.push(template("one"));
    let saved = save(&f.0, library).unwrap(); let mut external = saved.clone(); external.templates[0].name = "external".into();
    let bytes = serde_json::to_vec(&external).unwrap(); f.put(&bytes);
    assert_eq!(save(&f.0, saved).unwrap_err().code, "string_templates_conflict");
    assert_eq!(fs::read(path(&f.0)).unwrap(), bytes);
}

#[test]
fn limits_and_incomplete_groups_fail_without_overwriting() {
    let f = Fixture::new(); let mut library = read(&f.0).unwrap();
    library.templates = (1..=50).map(|n| template(&format!("String {n}"))).collect();
    let saved = save(&f.0, library).unwrap(); let before = fs::read(path(&f.0)).unwrap();
    let mut full = saved.clone(); full.templates.push(template("extra"));
    assert_eq!(save(&f.0, full).unwrap_err().code, "string_templates_full");
    for variant in 0..9 {
        let mut bad = saved.clone();
        match variant {
            0 => bad.schema = 2,
            1 => bad.templates[0].name = " ".into(),
            2 => bad.templates[0].name = "🧵".repeat(41),
            3 => bad.templates[0].group.models[0].intent = " ".into(),
            4 => bad.templates[0].group.models.truncate(1),
            5 => bad.templates[0].group.default_model = "missing".into(),
            6 => bad.templates[0].group.models[0].thinking_level = "invalid".into(),
            7 => bad.templates[0].id = bad.templates[1].id.clone(),
            _ => bad.templates[0].saved_at = "not a date".into(),
        }
        assert_eq!(save(&f.0, bad).unwrap_err().code, "string_templates_invalid");
        assert_eq!(fs::read(path(&f.0)).unwrap(), before);
    }
    let mut valid = saved; valid.templates[0].name = "🧵".repeat(40);
    assert!(save(&f.0, valid).is_ok());
}

#[test]
fn malformed_oversized_unknown_schema_files_are_not_repaired_or_overwritten() {
    for bytes in [b"{}".to_vec(), b"invalid".to_vec(), vec![b' '; MAX_BYTES as usize + 1]] {
        let f = Fixture::new(); f.put(&bytes);
        assert_eq!(read(&f.0).unwrap_err().code, "string_templates_invalid");
        assert_eq!(fs::read(path(&f.0)).unwrap(), bytes);
    }
}

#[test]
fn lock_rejects_second_writer_and_releases_without_lost_updates() {
    let f = Fixture::new(); let library = read(&f.0).unwrap();
    fs::create_dir_all(path(&f.0).parent().unwrap()).unwrap();
    let lock = FileLock::acquire(&path(&f.0).parent().unwrap().join(".string-templates.lock"), "busy").unwrap();
    assert_eq!(save(&f.0, library.clone()).unwrap_err().code, "string_templates_write_failed");
    drop(lock); assert!(save(&f.0, library).is_ok());
}

#[cfg(unix)]
#[test]
fn symlink_is_not_followed_or_replaced() {
    let f = Fixture::new(); fs::create_dir_all(path(&f.0).parent().unwrap()).unwrap();
    let target = f.0.join("other.json"); fs::write(&target, b"keep").unwrap();
    std::os::unix::fs::symlink(&target, path(&f.0)).unwrap();
    assert_eq!(read(&f.0).unwrap_err().code, "string_templates_invalid");
    assert_eq!(save(&f.0, Library { schema: 1, revision: "missing".into(), templates: vec![] }).unwrap_err().code, "string_templates_invalid");
    assert_eq!(fs::read(target).unwrap(), b"keep");
}
