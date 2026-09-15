use super::*;

fn notice(post: &str, kind: Option<&str>) -> UnavailablePost {
    UnavailablePost {
        thread_id: "thread-one".into(),
        post_id: post.into(),
        reason: UnavailableReason::TooLargeToSync,
        kind: kind.map(str::to_owned),
    }
}
async fn mark(board: &Board, post: &str, kind: Option<&str>) -> reconcile::Plan {
    board
        .page(
            ManifestPhase::Unavailable,
            vec![serde_json::to_value(notice(post, kind)).unwrap()],
        )
        .await
}
fn snapshot(board: &Board) -> Value {
    serde_json::to_value(snapshot_at(&board.store, "target", Some("Target")).unwrap()).unwrap()
}
fn path(board: &Board, post: &str) -> PathBuf {
    board
        .store
        .root()
        .join(format!("threads/thread-one/placeholders/{post}.json"))
}

#[tokio::test]
async fn oversized_sources_use_bounded_headers_without_versions_hashes_or_failures() {
    let b = Board::new();
    b.thread().await;
    let posts = b.store.root().join("threads/thread-one/posts");
    fs::create_dir_all(&posts).unwrap();
    let (raw, _) = raw_post("huge-root", "topic", "source text stays here", vec![]);
    fs::write(posts.join("huge-root.md"), &raw).unwrap();
    OpenOptions::new()
        .write(true)
        .open(posts.join("huge-root.md"))
        .unwrap()
        .set_len(reconcile::MAX_BODY_BYTES + 1)
        .unwrap();
    let large_header = format!(
        "---\npadding: {}\n---\nbody",
        "x".repeat(reconcile::MAX_FRONTMATTER_BYTES + 100)
    );
    fs::write(posts.join("non-mint-id.md"), large_header).unwrap();
    let store = b.store.clone();
    let fence = b.fence.clone();
    let (issues, charged) = b
        .files
        .run(&Cancellation::default(), move |io| -> Result<_> {
            let before = io.charged_bytes();
            let issues = store.refresh(&fence, io)?;
            Ok((issues, io.charged_bytes() - before))
        })
        .await
        .unwrap()
        .unwrap();
    assert!(issues.is_empty(), "{issues:?}");
    assert!(
        charged < 128 * 1024,
        "only two bounded headers plus thread record: {charged}"
    );
    assert!(b.state().ledger.is_empty());
    let catalog = b.store.catalog(&b.fence).unwrap();
    assert!(catalog
        .page(ManifestPhase::Versions, None)
        .unwrap()
        .0
        .items
        .is_empty());
    let (page, issues) = catalog.page(ManifestPhase::Unavailable, None).unwrap();
    assert!(issues.is_empty());
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items[0]["kind"], "topic");
    assert!(page.items[1].get("kind").is_none());
    for row in page.items {
        assert!(row.get("version_id").is_none());
        assert!(row.get("body").is_none());
    }
    assert_eq!(
        fs::metadata(posts.join("huge-root.md")).unwrap().len(),
        reconcile::MAX_BODY_BYTES + 1
    );
}

#[tokio::test]
async fn unknown_root_is_visible_without_inventing_a_post_and_show_reuses_projection() {
    let b = Board::new();
    b.thread().await;
    assert!(mark(&b, "unavailable-non-mint", None)
        .await
        .issues
        .is_empty());
    let first = fs::metadata(path(&b, "unavailable-non-mint")).unwrap();
    assert!(mark(&b, "unavailable-non-mint", None)
        .await
        .issues
        .is_empty());
    assert_eq!(
        first.ino(),
        fs::metadata(path(&b, "unavailable-non-mint"))
            .unwrap()
            .ino()
    );
    assert_eq!(first.mode() & 0o777, 0o600);
    assert!(b.state().ledger.is_empty());
    let empty = snapshot(&b);
    assert_eq!(empty["threads"].as_array().unwrap().len(), 1);
    assert!(empty["threads"][0]["posts"].as_array().unwrap().is_empty());
    assert_eq!(
        empty["threads"][0]["unavailablePosts"],
        json!([
            {"postId":"unavailable-non-mint","reason":"too_large_to_sync"}
        ])
    );
    let (raw, p) = raw_post("normal-reply", "reply", "I remain a reply", vec![]);
    b.receive_post(&p, &raw).await.unwrap();
    let with_reply = snapshot(&b);
    assert_eq!(with_reply["threads"][0]["posts"][0]["kind"], "reply");
    assert_eq!(
        with_reply["threads"][0]["posts"].as_array().unwrap().len(),
        1
    );
    let rendered = render_thread_scoped(&b.store, "thread-one").unwrap();
    assert!(rendered.contains("post: unavailable-non-mint\n\nFile size too large to sync."));
    assert!(rendered.contains("I remain a reply"));
    println!(
        "KOTA_BBS_UNAVAILABLE_FIXTURE={}",
        json!({"emptyRoot":empty,"withReply":with_reply,"show":rendered})
    );
    // A stored received marker is not advertised by this device, even after reindexing.
    b.refresh().await;
    assert!(b
        .store
        .catalog(&b.fence)
        .unwrap()
        .page(ManifestPhase::Unavailable, None)
        .unwrap()
        .0
        .items
        .is_empty());
}

#[tokio::test]
async fn kinds_only_refine_and_absence_cancellation_or_leaving_cannot_withdraw() {
    let b = Board::new();
    b.thread().await;
    mark(&b, "missing", None).await;
    assert!(mark(&b, "missing", Some("reply")).await.issues.is_empty());
    let before = fs::read(path(&b, "missing")).unwrap();
    mark(&b, "missing", None).await;
    assert_eq!(fs::read(path(&b, "missing")).unwrap(), before);
    let conflict = mark(&b, "missing", Some("topic")).await;
    assert_eq!(conflict.issues[0].code, "unavailable_kind_mismatch");
    b.page(ManifestPhase::Unavailable, vec![]).await;
    b.refresh().await;
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(b.files.run(&cancel, |_| ()).await.is_err());
    b.set_group(None);
    assert_eq!(fs::read(path(&b, "missing")).unwrap(), before);
    let reopened = ContentStore::at(b.store.root.clone(), b.store.account.clone());
    let view = serde_json::to_value(snapshot_at(&reopened, "target", None).unwrap()).unwrap();
    assert_eq!(view["threads"][0]["unavailablePosts"][0]["kind"], "reply");
    assert_eq!(view["threads"][0]["sharingGroupId"], Value::Null);
}

#[tokio::test]
async fn real_content_takes_precedence_cleans_marker_and_last_version_count_is_real_only() {
    let b = Board::new();
    b.thread().await;
    mark(&b, "root", Some("topic")).await;
    let saved_marker = fs::read(path(&b, "root")).unwrap();
    let (raw, p) = raw_post("root", "topic", "now small enough", vec![]);
    b.receive_post(&p, &raw).await.unwrap();
    assert!(!path(&b, "root").exists());
    assert!(mark(&b, "root", Some("topic")).await.issues.is_empty());
    assert!(!path(&b, "root").exists());
    // A crash after body publication but before marker unlink cannot create a Fork.
    fs::write(path(&b, "root"), saved_marker).unwrap();
    let view = snapshot(&b);
    assert!(view["threads"][0]["unavailablePosts"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(view["threads"][0]["posts"].as_array().unwrap().len(), 1);
    assert!(b
        .store
        .delete_local("thread-one", Some("root"), Some(&p.version_id))
        .unwrap_err()
        .to_string()
        .contains("last_version_use_logical_delete"));
    b.refresh().await;
    assert!(!path(&b, "root").exists());
}

#[tokio::test]
async fn only_thread_and_post_tombstones_remove_markers_and_deleted_ids_stay_deleted() {
    let b = Board::new();
    b.thread().await;
    mark(&b, "missing", None).await;
    let version = Tombstone {
        thread_id: "thread-one".into(),
        post_id: Some("missing".into()),
        version_id: Some("a".repeat(64)),
        deleted_at: "2026-09-12T12:00:00Z".into(),
    };
    b.page(
        ManifestPhase::Tombstones,
        vec![serde_json::to_value(&version).unwrap()],
    )
    .await;
    assert!(path(&b, "missing").exists());
    let logical = Tombstone {
        version_id: None,
        ..version
    };
    b.page(
        ManifestPhase::Tombstones,
        vec![serde_json::to_value(&logical).unwrap()],
    )
    .await;
    assert!(!path(&b, "missing").exists());
    mark(&b, "missing", None).await;
    assert!(!path(&b, "missing").exists());
    mark(&b, "another", None).await;
    let thread = Tombstone {
        post_id: None,
        ..logical
    };
    b.page(
        ManifestPhase::Tombstones,
        vec![serde_json::to_value(thread).unwrap()],
    )
    .await;
    assert!(!path(&b, "another").exists());
    assert!(snapshot(&b)["threads"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn notices_cannot_enroll_private_roots_or_create_missing_thread_records() {
    let b = Board::new();
    let plan = mark(&b, "missing", None).await;
    assert_eq!(plan.issues[0].code, "missing_thread_record");
    assert!(!path(&b, "missing").exists());
    assert!(b
        .state()
        .groups
        .values()
        .all(|g| g.shared_threads.is_empty()));
    b.thread().await;
    let mut state = b.state();
    state
        .groups
        .get_mut("group-test")
        .unwrap()
        .shared_threads
        .clear();
    b.store.state.save_state(&state).unwrap();
    let plan = mark(&b, "missing", None).await;
    assert_eq!(plan.issues[0].code, "unshared_unavailable_post");
    assert!(!path(&b, "missing").exists());
    assert!(b.state().groups["group-test"].shared_threads.is_empty());
}

#[tokio::test]
async fn more_than_4096_markers_are_paged_and_durable_without_a_total_cutoff() {
    let b = Board::new();
    b.thread().await;
    let mut source = b.state();
    for n in 0..4097 {
        let p = notice(&format!("post-{n:05}"), None);
        source.local_unavailable.insert(p.key().unwrap(), p);
    }
    let catalog = reconcile::Catalog::from_state(&source, "group-test").unwrap();
    let store = b.store.clone();
    let fence = b.fence.clone();
    let count = b
        .files
        .run(&Cancellation::default(), move |io| -> Result<_> {
            let mut reader = reconcile::ManifestReader::default();
            for phase in [
                ManifestPhase::Tombstones,
                ManifestPhase::Threads,
                ManifestPhase::Versions,
            ] {
                store.process_page(
                    &fence,
                    &Manifest {
                        phase,
                        items: vec![],
                        next: None,
                    },
                    &mut reader,
                    io,
                )?;
            }
            let mut after = None;
            let mut count = 0;
            loop {
                let (page, issues) = catalog
                    .page(ManifestPhase::Unavailable, after.as_ref())
                    .unwrap();
                assert!(issues.is_empty());
                assert!(serde_json::to_vec(&page).unwrap().len() <= reconcile::MAX_PAGE_BYTES);
                assert!(page.items.len() <= reconcile::MAX_PAGE_ITEMS);
                count += page.items.len();
                after = page.next.clone();
                assert!(store
                    .process_page(&fence, &page, &mut reader, io)?
                    .issues
                    .is_empty());
                if after.is_none() {
                    break;
                }
            }
            assert!(reader.is_complete());
            Ok(count)
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(count, 4097);
    assert_eq!(
        fs::read_dir(path(&b, "post-00000").parent().unwrap())
            .unwrap()
            .count(),
        4097
    );
    let view = snapshot(&b);
    assert_eq!(
        view["threads"][0]["unavailablePosts"]
            .as_array()
            .unwrap()
            .len(),
        4097
    );
    assert!(b.state().ledger.is_empty());
}

#[tokio::test]
async fn notice_page_does_not_rehash_existing_bodies_when_thread_has_a_tombstone() {
    let b = Board::new();
    b.thread().await;
    let (raw, post) = raw_post("root", "topic", &"body".repeat(32_768), vec![]);
    b.receive_post(&post, &raw).await.unwrap();
    let deletion = Tombstone {
        thread_id: "thread-one".into(),
        post_id: Some("gone".into()),
        version_id: None,
        deleted_at: "2026-09-12T12:00:00Z".into(),
    };
    let store = b.store.clone();
    let fence = b.fence.clone();
    let charged = b
        .files
        .run(&Cancellation::default(), move |io| -> Result<u64> {
            let mut reader = reconcile::ManifestReader::default();
            for phase in [
                ManifestPhase::Tombstones,
                ManifestPhase::Threads,
                ManifestPhase::Versions,
            ] {
                store.process_page(
                    &fence,
                    &Manifest {
                        phase,
                        items: if phase == ManifestPhase::Tombstones {
                            vec![serde_json::to_value(&deletion)?]
                        } else {
                            vec![]
                        },
                        next: None,
                    },
                    &mut reader,
                    io,
                )?;
            }
            let before = io.charged_bytes();
            let plan = store.process_page(
                &fence,
                &Manifest {
                    phase: ManifestPhase::Unavailable,
                    items: vec![serde_json::to_value(notice("missing", None))?],
                    next: None,
                },
                &mut reader,
                io,
            )?;
            assert!(plan.issues.is_empty());
            assert!(reader.is_complete());
            Ok(io.charged_bytes() - before)
        })
        .await
        .unwrap()
        .unwrap();
    assert!(
        charged < 16 * 1024,
        "a notice must not re-read the existing 128 KiB body: {charged}"
    );
    assert!(path(&b, "missing").exists());
}
