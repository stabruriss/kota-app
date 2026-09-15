use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};

#[test]
fn local_notice_accepts_a_real_post_filename_beyond_the_sync_id_budget() {
    let b = Board::new();
    let mut author = human_identity("source", "Author".into(), None);
    author.agent_id = "a".repeat(128);
    create_thread_mentions_scoped(
        &b.content,
        &author,
        vec!["source".into()],
        false,
        "Long existing local identity".into(),
        &[],
        &[b.target()],
    )
    .unwrap();
    let key = b.keys("pending")[0].clone();
    assert!(b.record(&key).destination.post_id.len() > 128);
    let sink = RecordingSink::default();
    delivery::dispatch(&b.queue, claim(&b.queue, &key, "run", now_ms()), &sink).unwrap();
    assert_eq!(sink.calls.lock().unwrap().len(), 1);
}

#[test]
fn queue_permissions_corruption_and_symlinks_fail_closed() {
    let b = Board::new();
    b.post("Original", &[b.target()]);
    let key = b.keys("pending")[0].clone();
    let path = b.queue.path("pending", &key).unwrap();
    for directory in [
        b.queue.root(),
        b.queue.root().join("pending"),
        b.queue.root().join("processing"),
        b.queue.root().join("processed"),
    ] {
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
    }
    b.queue.initialize().unwrap();
    for directory in [
        b.queue.root(),
        b.queue.root().join("pending"),
        b.queue.root().join("processing"),
        b.queue.root().join("processed"),
    ] {
        assert_eq!(
            fs::metadata(directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let bytes = fs::read(&path).unwrap();
    let mut corrupt: Value = serde_json::from_slice(&bytes).unwrap();
    corrupt["schemaVersion"] = json!(2);
    for raw in [serde_json::to_vec(&corrupt).unwrap(), b"{broken".to_vec()] {
        fs::write(&path, &raw).unwrap();
        assert!(b.queue.check(&key, "run", now_ms()).is_err());
        assert_eq!(fs::read(&path).unwrap(), raw);
    }
    fs::remove_file(&path).unwrap();
    let other = b.account.0.join("untouched.json");
    fs::write(&other, &bytes).unwrap();
    symlink(&other, &path).unwrap();
    assert!(b.queue.check(&key, "run", now_ms()).is_err());
    assert_eq!(fs::read(&other).unwrap(), bytes);
    fs::remove_file(&path).unwrap();
    fs::remove_dir(b.queue.root().join("pending")).unwrap();
    symlink(&b.account.0, b.queue.root().join("pending")).unwrap();
    assert!(b.queue.path("pending", &key).is_err());
    assert!(b.queue.initialize().is_err());
}

#[test]
fn only_one_processing_recovery_attempt_is_allowed() {
    let b = Board::new();
    b.post("Crash before submission", &[b.target()]);
    let key = b.keys("pending")[0].clone();
    drop(claim(&b.queue, &key, "first-app", now_ms()));
    drop(claim(&b.queue, &key, "recovery-app", now_ms()));
    assert!(matches!(
        b.queue.check(&key, "third-app", now_ms()).unwrap(),
        store::Check::Done
    ));
    let record: Value =
        serde_json::from_slice(&fs::read(b.queue.path("processed", &key).unwrap()).unwrap())
            .unwrap();
    assert_eq!(record["result"], "recovery_exhausted");
}

#[test]
fn awaiting_post_expiry_checks_body_and_never_turns_an_orphan_into_a_notice() {
    let b = Board::new();
    let thread = b.post("Uncommitted crash window", &[b.target()]);
    let key = b.keys("pending")[0].clone();
    let mut record = b.record(&key);
    record.ready = false;
    record.proof = None;
    fs::write(
        b.queue.path("pending", &key).unwrap(),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    let expiry = record.created_at_ms + ORPHAN_WAIT_MS;
    assert!(matches!(
        b.queue.check(&key, "recovery", expiry + 1).unwrap(),
        store::Check::Recover
    ));
    fs::remove_file(
        b.content
            .root()
            .join("threads")
            .join(&thread)
            .join("posts")
            .join(format!("{}.md", record.destination.post_id)),
    )
    .unwrap();
    assert!(matches!(
        b.queue.check(&key, "recovery", expiry - 1).unwrap(),
        store::Check::Wait { .. }
    ));
    assert!(matches!(
        b.queue.check(&key, "recovery", expiry + 1).unwrap(),
        store::Check::Done
    ));
    let settled: Value =
        serde_json::from_slice(&fs::read(b.queue.path("processed", &key).unwrap()).unwrap())
            .unwrap();
    assert_eq!(settled["result"], "unpublished");
}

#[test]
fn ready_notice_requires_its_real_thread_and_unmodified_body() {
    let b = Board::new();
    let thread = b.post("Actual body", &[b.target()]);
    let key = b.keys("pending")[0].clone();
    let body = b
        .content
        .root()
        .join("threads")
        .join(&thread)
        .join("posts")
        .join(format!("{}.md", b.record(&key).destination.post_id));
    fs::remove_file(&body).unwrap();
    let outside = b.account.0.join("outside.md");
    fs::write(&outside, "Do not read").unwrap();
    symlink(&outside, &body).unwrap();
    assert!(b.queue.check(&key, "run", now_ms()).is_err());
    assert_eq!(fs::read_to_string(&outside).unwrap(), "Do not read");
    let thread = b.post("Orphan body", &[b.target()]);
    let key = b
        .keys("pending")
        .into_iter()
        .find(|k| b.record(k).destination.thread_id == thread)
        .unwrap();
    fs::remove_file(
        b.content
            .root()
            .join("threads")
            .join(thread)
            .join("thread.yaml"),
    )
    .unwrap();
    assert!(matches!(
        b.queue.check(&key, "run", now_ms()).unwrap(),
        store::Check::Done
    ));
}
