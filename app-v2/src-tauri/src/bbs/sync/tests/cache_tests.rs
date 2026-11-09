use super::*;

#[tokio::test]
async fn unchanged_inventory_refresh_keeps_state_stamp_and_row_timestamp() {
    let b = Board::new();
    b.thread().await;
    let dir = b.store.root().join("threads/thread-one/posts");
    fs::create_dir_all(&dir).unwrap();
    let (raw, _) = raw_post("root", "topic", "unchanged", vec![]);
    fs::write(dir.join("root.md"), raw).unwrap();
    b.refresh().await;
    let before = current_stamp(&b.store.state.state_path()).unwrap();
    let state = b.state();
    let writes = b.store.state_cache.lock().unwrap().writes;
    for _ in 0..2 {
        b.refresh().await;
        assert_eq!(current_stamp(&b.store.state.state_path()).unwrap(), before);
        assert_eq!(b.state(), state);
    }
    assert_eq!(b.store.state_cache.lock().unwrap().writes, writes);
}

#[tokio::test]
async fn changed_inventory_row_advances_timestamp_and_writes_once() {
    let b = Board::new();
    b.thread().await;
    let dir = b.store.root().join("threads/thread-one");
    fs::create_dir_all(dir.join("posts")).unwrap();
    fs::create_dir_all(dir.join("attachments/root")).unwrap();
    let bytes = b"fixture attachment";
    let a = attachment(bytes);
    let path = dir.join(&a.path);
    fs::write(&path, bytes).unwrap();
    let (raw, _) = raw_post("root", "topic", "with attachment", vec![a]);
    fs::write(dir.join("posts/root.md"), raw).unwrap();
    b.refresh().await;
    let mut before = b.state();
    let key = before.ledger.keys().next().unwrap().clone();
    assert!(before.ledger[&key].attachments["att-one"].verified);
    before.ledger.get_mut(&key).unwrap().updated_at = "2000-01-01T00:00:00Z".into();
    b.store.state.save_state(&before).unwrap();
    let writes = b.store.state_cache.lock().unwrap().writes;
    fs::remove_file(path).unwrap();
    b.refresh().await;
    let after = b.state();
    assert!(!after.ledger[&key].attachments["att-one"].verified);
    assert!(after.ledger[&key].updated_at > before.ledger[&key].updated_at);
    assert_eq!(b.store.state_cache.lock().unwrap().writes, writes + 1);
    b.refresh().await;
    assert_eq!(b.store.state_cache.lock().unwrap().writes, writes + 1);
    assert_eq!(b.state(), after);
}

#[tokio::test]
async fn snapshot_sees_external_cli_post_without_app_generation_or_state_write() {
    let b = Board::new();
    b.thread().await;
    let dir = b.store.root().join("threads/thread-one/posts");
    fs::create_dir_all(&dir).unwrap();
    let (raw, _) = raw_post("root", "topic", "topic", vec![]);
    fs::write(dir.join("root.md"), raw).unwrap();
    let first = snapshot_at(&b.store, "target", Some("Target")).unwrap();
    assert_eq!(first.threads[0].posts.len(), 1);
    let state_before = fs::read(b.store.state.state_path()).unwrap();
    let root_before = fs::metadata(b.store.root().join("threads"))
        .unwrap()
        .modified()
        .unwrap();

    // Write as a separate CLI would: no call into an App's cache/generation.
    let (raw, _) = raw_post("reply", "reply", "external reply", vec![]);
    fs::write(dir.join("reply.md"), raw).unwrap();
    assert_eq!(state_before, fs::read(b.store.state.state_path()).unwrap());
    assert_eq!(
        root_before,
        fs::metadata(b.store.root().join("threads"))
            .unwrap()
            .modified()
            .unwrap()
    );
    let next = snapshot_at(&b.store, "target", Some("Target")).unwrap();
    assert_eq!(next.threads[0].posts.len(), 2);
    assert!(next.threads[0]
        .posts
        .iter()
        .any(|p| p.body == "external reply"));
}

#[tokio::test]
async fn snapshot_sees_other_store_placeholder_without_state_rewrite() {
    let b = Board::new();
    b.thread().await;
    assert!(snapshot_at(&b.store, "target", None)
        .unwrap()
        .threads
        .is_empty());
    let before = fs::read(b.store.state.state_path()).unwrap();
    let other = ContentStore::at(b.store.root.clone(), b.store.account.clone());
    {
        let lock = other.state.content_lock().unwrap();
        let state = other.state_locked(&lock).unwrap();
        other
            .mark_unavailable_locked(
                &lock,
                &state,
                &UnavailablePost {
                    thread_id: "thread-one".into(),
                    post_id: "missing".into(),
                    reason: UnavailableReason::TooLargeToSync,
                    kind: Some("topic".into()),
                },
            )
            .unwrap();
        other.save_locked(&lock, &state).unwrap();
    }
    assert_eq!(before, fs::read(b.store.state.state_path()).unwrap());
    let next = snapshot_at(&b.store, "target", None).unwrap();
    assert_eq!(next.threads[0].unavailable_posts[0].post_id, "missing");
    {
        let lock = other.state.content_lock().unwrap();
        other
            .clear_unavailable_locked(&lock, "thread-one", "missing")
            .unwrap();
    }
    assert!(snapshot_at(&b.store, "target", None)
        .unwrap()
        .threads
        .is_empty());
}

#[tokio::test]
async fn unchanged_state_keeps_file_and_external_replace_is_reloaded() {
    let b = Board::new();
    b.thread().await;
    {
        let lock = b.store.state.content_lock().unwrap();
        let state = b.store.state_locked(&lock).unwrap();
        let before = current_stamp(&b.store.state.state_path()).unwrap();
        b.store.save_locked(&lock, &state).unwrap();
        assert_eq!(before, current_stamp(&b.store.state.state_path()).unwrap());
    }
    let other = ContentStore::at(b.store.root.clone(), b.store.account.clone());
    let expected = {
        let lock = other.state.content_lock().unwrap();
        let mut state = other.state_locked(&lock).unwrap();
        state
            .groups
            .get_mut(&b.fence.group_id)
            .unwrap()
            .shared_threads
            .insert("thread-external".into());
        other.save_locked(&lock, &state).unwrap();
        state
    };
    let lock = b.store.state.content_lock().unwrap();
    assert_eq!(b.store.state_locked(&lock).unwrap(), expected);
}

#[tokio::test]
async fn external_corruption_is_an_error_not_a_stale_cached_state_or_snapshot() {
    let b = Board::new();
    b.thread().await;
    let before = fs::read(b.store.state.state_path()).unwrap();
    {
        let lock = b.store.state.content_lock().unwrap();
        b.store.state_locked(&lock).unwrap();
    }
    snapshot_at(&b.store, "target", None).unwrap();
    fs::write(b.store.state.state_path(), b"{malformed").unwrap();
    {
        let lock = b.store.state.content_lock().unwrap();
        assert!(b.store.state_locked(&lock).is_err());
    }
    assert!(snapshot_at(&b.store, "target", None).is_err());
    write_small(&b.store.state.state_path(), &before).unwrap();
    let lock = b.store.state.content_lock().unwrap();
    assert_eq!(b.store.state_locked(&lock).unwrap(), b.state());
    snapshot_at(&b.store, "target", None).unwrap();
}

#[tokio::test]
async fn other_store_publication_and_control_change_are_visible_without_reopening_app() {
    let b = Board::new();
    b.thread().await;
    let (raw, post) = raw_post("root", "topic", "topic", vec![]);
    b.receive_post(&post, &raw).await.unwrap();
    let first = snapshot_at(&b.store, "target", None).unwrap();
    assert_eq!(
        first.threads[0].sharing_group_id.as_deref(),
        Some("group-test")
    );
    let other = ContentStore::at(b.store.root.clone(), b.store.account.clone());
    let id = reply_scoped(
        &other,
        &human_identity("target", "User".into(), None),
        "thread-one",
        "reply from another process".into(),
        &[],
    )
    .unwrap();
    assert!(snapshot_at(&b.store, "target", None).unwrap().threads[0]
        .posts
        .iter()
        .any(|p| p.post_id == id));
    let state_stamp = current_stamp(&b.store.state.state_path()).unwrap();
    b.set_group(None);
    assert_eq!(
        current_stamp(&b.store.state.state_path()).unwrap(),
        state_stamp
    );
    assert!(snapshot_at(&b.store, "target", None).unwrap().threads[0]
        .sharing_group_id
        .is_none());
}
