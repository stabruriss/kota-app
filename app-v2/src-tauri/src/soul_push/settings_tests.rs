use super::*;
use std::cell::Cell;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self { Self(std::env::temp_dir().join(format!("kota-soul-settings-{}", uuid::Uuid::new_v4()))) }
    fn put(&self, bytes: &[u8]) {
        fs::create_dir_all(path(&self.0).parent().unwrap()).unwrap();
        fs::write(path(&self.0), bytes).unwrap();
    }
}
impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

#[test]
fn missing_read_is_read_only_and_default_enabled_is_effectively_off_without_jev() {
    let fixture = Fixture::new();
    for ready in [false, true] {
        let snapshot = read_snapshot(&fixture.0, ready).unwrap();
        assert_eq!(snapshot.settings, Settings::initial(true));
        assert_eq!(snapshot.source_revision, "missing");
        assert_eq!(snapshot.jev_ready, ready);
        assert_eq!(snapshot.settings.enabled && snapshot.jev_ready, ready);
        assert!(!fixture.0.exists());
    }
}

#[test]
fn save_round_trip_returns_truthful_camel_case_snapshot_and_never_resets_saved_toggle() {
    let fixture = Fixture::new();
    let saved = save_snapshot(&fixture.0, &Settings::initial(true), "missing", || Ok(true)).unwrap();
    assert!(saved.source_revision.starts_with("sha256:"));
    let before = fs::read(path(&fixture.0)).unwrap();
    let read = read_snapshot(&fixture.0, false).unwrap();
    assert_eq!(read.settings, saved.settings); // lost key isn't a preference write
    assert_eq!(read.source_revision, saved.source_revision);
    assert!(!read.jev_ready);
    assert_eq!(fs::read(path(&fixture.0)).unwrap(), before);
    let dto = serde_json::to_value(&saved).unwrap();
    assert_eq!(dto["jevReady"], true);
    assert_eq!(dto["sourceRevision"], saved.source_revision);
    assert_eq!(dto["settings"]["markThreshold"], 0.70);
    assert_eq!(dto["settings"]["dreamThreshold"], 0.80);
    assert_eq!(dto["settings"]["minimumMessageLength"], 60);
    assert_eq!(dto["settings"].as_object().unwrap().len(), 8);
}

#[test]
fn stale_cas_and_invalid_fields_do_not_invoke_jev_or_overwrite_bytes() {
    let fixture = Fixture::new();
    let saved = save_snapshot(&fixture.0, &Settings::initial(true), "missing", || Ok(true)).unwrap();
    let before = fs::read(path(&fixture.0)).unwrap();
    let called = Cell::new(false);
    let error = save_snapshot(&fixture.0, &Settings::initial(false), "missing", || { called.set(true); Ok(true) }).unwrap_err();
    assert_eq!(error.code, "soul_settings_conflict");
    let mut invalid = Settings::initial(true); invalid.max_marks = 0;
    let error = save_snapshot(&fixture.0, &invalid, &saved.source_revision, || { called.set(true); Ok(true) }).unwrap_err();
    assert_eq!(error.code, "soul_settings_invalid"); assert!(!called.get());
    assert_eq!(fs::read(path(&fixture.0)).unwrap(), before);
}

#[test]
fn external_edit_during_readiness_check_wins_instead_of_being_silently_overwritten() {
    let fixture = Fixture::new();
    let saved = save_snapshot(&fixture.0, &Settings::initial(true), "missing", || Ok(true)).unwrap();
    let mut external = Settings::initial(true); external.cooldown_replies = 25;
    let external_bytes = serde_json::to_vec(&external).unwrap();
    let error = save_snapshot(&fixture.0, &Settings::initial(false), &saved.source_revision, || {
        fs::write(path(&fixture.0), &external_bytes).unwrap(); Ok(true)
    }).unwrap_err();
    assert_eq!(error.code, "soul_settings_conflict");
    assert_eq!(fs::read(path(&fixture.0)).unwrap(), external_bytes);
}

#[test]
fn enabling_requires_ready_jev_but_turning_off_survives_missing_rejected_or_invalid_store() {
    for failure in ["jev_key_required", "jev_key_rejected", "jev_store_invalid"] {
        let fixture = Fixture::new();
        let error = save_snapshot(&fixture.0, &Settings::initial(true), "missing", || Err(LocalError::new(failure))).unwrap_err();
        assert_eq!(error.code, failure); assert!(!path(&fixture.0).exists());
        let saved = save_snapshot(&fixture.0, &Settings::initial(false), "missing", || Err(LocalError::new(failure))).unwrap();
        assert!(!saved.settings.enabled && !saved.jev_ready);
    }
    let fixture = Fixture::new();
    let error = save_snapshot(&fixture.0, &Settings::initial(true), "missing", || Ok(false)).unwrap_err();
    assert_eq!(error.code, "jev_key_required"); assert!(!path(&fixture.0).exists());
}

#[test]
fn malformed_or_unknown_settings_fail_read_without_replacing_them_with_defaults() {
    for bytes in [b"{}".to_vec(), b"[]".to_vec(), vec![0xff],
        serde_json::to_vec(&serde_json::json!({"schema":2})).unwrap(),
        serde_json::to_vec(&serde_json::json!({"schema":1,"enabled":true,"markThreshold":0.85,
            "dreamThreshold":0.90,"maxMarks":3,"maxDreams":2,"cooldownReplies":10,
            "minimumMessageLength":60,"unexpected":true})).unwrap()] {
        let fixture = Fixture::new(); fixture.put(&bytes);
        assert_eq!(read_snapshot(&fixture.0, true).unwrap_err().code, "soul_settings_invalid");
        assert_eq!(fs::read(path(&fixture.0)).unwrap(), bytes);
    }
    let fixture = Fixture::new(); fixture.put(&vec![b' '; 1024 * 1024 + 1]);
    assert_eq!(read_snapshot(&fixture.0, false).unwrap_err().code, "soul_settings_invalid");
}

#[test]
fn cross_app_lock_refuses_a_second_writer_without_claiming_a_false_cas_conflict() {
    let fixture = Fixture::new();
    let directory = path(&fixture.0).parent().unwrap().to_owned();
    fs::create_dir_all(&directory).unwrap();
    let lock = FileLock::acquire(&directory.join(".puppeteer-soul.lock"), "fixture_busy").unwrap();
    let error = save_snapshot(&fixture.0, &Settings::initial(false), "missing", || panic!("no readiness read under another writer")).unwrap_err();
    assert_eq!(error.code, "soul_settings_write_failed"); assert!(!path(&fixture.0).exists());
    drop(lock);
    assert!(save_snapshot(&fixture.0, &Settings::initial(false), "missing", || Ok(false)).is_ok());
}

#[cfg(unix)]
#[test]
fn symlink_is_not_read_or_replaced() {
    let fixture = Fixture::new();
    let directory = path(&fixture.0).parent().unwrap().to_owned();
    fs::create_dir_all(&directory).unwrap();
    let target = fixture.0.join("synthetic-other-file.json");
    fs::write(&target, b"synthetic content").unwrap();
    std::os::unix::fs::symlink(&target, path(&fixture.0)).unwrap();
    assert_eq!(read_snapshot(&fixture.0, true).unwrap_err().code, "soul_settings_invalid");
    assert_eq!(save_snapshot(&fixture.0, &Settings::initial(false), "missing", || Ok(false)).unwrap_err().code, "soul_settings_invalid");
    assert_eq!(fs::read(target).unwrap(), b"synthetic content");
    assert!(fs::symlink_metadata(path(&fixture.0)).unwrap().file_type().is_symlink());
}
