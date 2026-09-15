use super::*;
use crate::bbs_sync::transport::{FileIo, Limits};
use serde_json::{json, Value};
use std::os::unix::fs::MetadataExt;
mod unavailable_tests;

struct Board {
    directory: PathBuf,
    store: ContentStore,
    files: FileIo,
    fence: GroupFence,
}
impl Board {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("kota-content-test-{}", Uuid::new_v4()));
        let store = ContentStore::at(directory.join("bbs"), directory.join("account"));
        store.prepare_layout().unwrap();
        let result = Self {
            directory,
            store,
            files: FileIo::start(Limits::default()).unwrap(),
            fence: GroupFence {
                group_id: "group-test".into(),
                membership_id: "member-test".into(),
            },
        };
        result.set_group(Some(&result.fence));
        result
    }
    fn set_group(&self, fence: Option<&GroupFence>) {
        let _lock = self.store.state.content_lock().unwrap();
        fs::create_dir_all(self.store.state.control_path().parent().unwrap()).unwrap();
        write_small(&self.store.state.control_path(), &serde_json::to_vec(&json!({
            "schemaVersion":1,"deviceName":"test-device", "pending":null,"invitation":null,
            "current":fence.map(|f| json!({"groupId":f.group_id,"membershipId":f.membership_id,"workerUrl":"https://example.test","role":"member"}))
        })).unwrap()).unwrap();
    }
    async fn page(&self, phase: ManifestPhase, items: Vec<Value>) -> reconcile::Plan {
        let store = self.store.clone();
        let fence = self.fence.clone();
        self.files
            .run(&Cancellation::default(), move |io| {
                let mut reader = reconcile::ManifestReader::default();
                for earlier in [
                    ManifestPhase::Tombstones,
                    ManifestPhase::Threads,
                    ManifestPhase::Versions,
                ] {
                    if earlier == phase {
                        break;
                    }
                    store.process_page(
                        &fence,
                        &Manifest {
                            phase: earlier,
                            items: vec![],
                            next: None,
                        },
                        &mut reader,
                        io,
                    )?;
                }
                store.process_page(
                    &fence,
                    &Manifest {
                        phase,
                        items,
                        next: None,
                    },
                    &mut reader,
                    io,
                )
            })
            .await
            .unwrap()
            .unwrap()
    }
    async fn thread(&self) {
        self.page(
            ManifestPhase::Threads,
            vec![serde_json::to_value(record()).unwrap()],
        )
        .await;
    }
    async fn verified(&self, resource: Resource, bytes: &[u8]) -> VerifiedFile {
        let cancel = Cancellation::default();
        let mut writer = self
            .files
            .receive(
                resource.clone(),
                &self.store.prepare_layout().unwrap(),
                &cancel,
            )
            .await
            .unwrap();
        let mut offset = 0;
        for chunk in bytes.chunks(16_000) {
            offset = writer
                .write(offset, bytes::BytesMut::from(chunk))
                .await
                .unwrap();
        }
        writer.finish(resource.sha256).await.unwrap()
    }
    async fn receive_post(&self, post: &PostVersion, raw: &[u8]) -> Result<()> {
        let verified = self.verified(reconcile::post_resource(post), raw).await;
        self.store
            .install_received(
                verified,
                self.fence.clone(),
                Some(post.clone()),
                &Cancellation::default(),
            )
            .await
    }
    async fn refresh(&self) {
        let store = self.store.clone();
        let fence = self.fence.clone();
        let issues = self
            .files
            .run(&Cancellation::default(), move |io| {
                store.refresh(&fence, io)
            })
            .await
            .unwrap()
            .unwrap();
        assert!(issues.is_empty(), "{issues:?}");
    }
    fn state(&self) -> SyncState {
        self.store.state.load_state().unwrap().unwrap_or_default()
    }
}
impl Drop for Board {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}
fn record() -> ThreadRecordItem {
    reconcile::thread_item(ThreadRecord {
        schema: THREAD_SCHEMA.into(),
        thread_id: "thread-one".into(),
        status: "open".into(),
        visibility: "targeted".into(),
        project_tags: vec!["source".into()],
        created_by_project: "source".into(),
        created_by_agent: "author".into(),
        created_at: "2026-09-12T10:00:00Z".into(),
    })
    .unwrap()
}
fn metadata(post: &str, kind: &str, attachments: Vec<BbsAttachment>) -> BbsPostMeta {
    BbsPostMeta {
        schema: POST_SCHEMA.into(),
        post_id: post.into(),
        thread_id: "thread-one".into(),
        project_id: "source".into(),
        agent_id: "author".into(),
        agent_display_name: "Author".into(),
        agent_avatar: Some("codex".into()),
        project_display_name: "Source".into(),
        created_at: "2026-09-12T10:00:00Z".into(),
        kind: kind.into(),
        attachments,
        mentions: Vec::new(),
    }
}
fn raw_post(
    post: &str,
    kind: &str,
    body: &str,
    attachments: Vec<BbsAttachment>,
) -> (Vec<u8>, PostVersion) {
    let meta = metadata(post, kind, attachments);
    let mut raw = serialized_post(&meta, body).unwrap();
    // Deliberately preserve noncanonical YAML/comment/spacing in the version.
    raw = raw.replacen("schema:", "# immutable source comment\nschema:", 1);
    let raw = raw.into_bytes();
    let p = PostVersion {
        thread_id: meta.thread_id.clone(),
        post_id: meta.post_id.clone(),
        version_id: bbs_sync::raw_sha256(&raw),
        size_bytes: raw.len() as u64,
        kind: kind.into(),
        attachments: attachment_refs(&meta).unwrap(),
        avatar: Some(AvatarSidecar::none()),
    };
    (raw, p)
}
fn attachment(bytes: &[u8]) -> BbsAttachment {
    BbsAttachment {
        id: "att-one".into(),
        name: "file.txt".into(),
        path: "attachments/root/att-one.txt".into(),
        size_bytes: bytes.len() as u64,
        sha256: bbs_sync::raw_sha256(bytes),
    }
}
async fn settle(files: &FileIo) {
    for _ in 0..100 {
        if files.run(&Cancellation::default(), |_| ()).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("file cleanup did not settle");
}

#[tokio::test]
async fn raw_byte_forks_reuse_same_hash_and_copy_attachments_into_distinct_inodes() {
    let b = Board::new();
    b.thread().await;
    let bytes = b"registered attachment";
    let (raw, mut first) = raw_post("root", "topic", "First", vec![attachment(bytes)]);
    first.attachments[0].available = true;
    b.receive_post(&first, &raw).await.unwrap();
    let resource = reconcile::attachment_resource(&first, &first.attachments[0]);
    let file = b.verified(resource.clone(), bytes).await;
    b.store
        .install_received(file, b.fence.clone(), None, &Cancellation::default())
        .await
        .unwrap();
    let resident = b
        .store
        .source(&b.fence, &reconcile::post_resource(&first))
        .unwrap();
    let original_inode = fs::metadata(&resident).unwrap().ino();
    b.receive_post(&first, &raw).await.unwrap();
    assert_eq!(fs::metadata(&resident).unwrap().ino(), original_inode);
    let (fork_raw, fork) = raw_post("root", "topic", "Second", vec![attachment(bytes)]);
    b.receive_post(&fork, &fork_raw).await.unwrap();
    let target = reconcile::attachment_resource(&fork, &fork.attachments[0]);
    let store = b.store.clone();
    let fence = b.fence.clone();
    let copy_target = target.clone();
    assert!(b
        .files
        .run(&Cancellation::default(), move |io| store.copy_available(
            &fence,
            &copy_target,
            None,
            io
        ))
        .await
        .unwrap()
        .unwrap());
    let from = b.store.source(&b.fence, &resource).unwrap();
    let to = b.store.source(&b.fence, &target).unwrap();
    assert_ne!(
        fs::metadata(from).unwrap().ino(),
        fs::metadata(&to).unwrap().ino()
    );
    assert_eq!(fs::read(&resident).unwrap(), raw);
    assert_eq!(
        fs::read(
            b.store
                .source(&b.fence, &reconcile::post_resource(&fork))
                .unwrap()
        )
        .unwrap(),
        fork_raw
    );
    assert!(to
        .to_string_lossy()
        .contains(&format!("versions/root/{}/attachments", fork.version_id)));
    let snapshot = snapshot_at(&b.store, "receiver", None).unwrap();
    assert_eq!(snapshot.threads[0].posts.len(), 2);
    for post in &snapshot.threads[0].posts {
        assert!(post.attachments[0].available);
        assert_eq!(
            post.attachments[0].attachment.path,
            "attachments/root/att-one.txt"
        );
    }
    let rendered = render_thread_scoped(&b.store, "thread-one").unwrap();
    assert!(rendered.contains(&to.display().to_string()));
    b.refresh().await;
    assert_eq!(b.state().ledger.len(), 2);
}

#[tokio::test]
async fn exact_version_delete_promotes_survivor_and_never_becomes_logical_delete() {
    let b = Board::new();
    b.thread().await;
    let (raw, first) = raw_post("root", "topic", "First", vec![]);
    let (raw2, second) = raw_post("root", "topic", "Second", vec![]);
    b.receive_post(&first, &raw).await.unwrap();
    b.receive_post(&second, &raw2).await.unwrap();
    let (reply_raw, reply) = raw_post("reply-one", "reply", "Shared reply pool", vec![]);
    b.receive_post(&reply, &reply_raw).await.unwrap();
    let request: BbsDeleteRequest = serde_json::from_value(
        json!({"threadId":"thread-one", "postId":"root", "versionId":first.version_id}),
    )
    .unwrap();
    assert_eq!(request.version_id.as_ref(), Some(&first.version_id));
    b.store
        .delete_local(
            &request.thread_id,
            request.post_id.as_deref(),
            request.version_id.as_deref(),
        )
        .unwrap();
    assert_eq!(
        fs::read(b.store.root.join("threads/thread-one/posts/root.md")).unwrap(),
        raw2
    );
    let state = b.state();
    assert_eq!(state.tombstones.len(), 1);
    assert!(state
        .tombstones
        .contains_key(&format!("version:thread-one/root/{}", first.version_id)));
    assert!(b
        .store
        .delete_local("thread-one", Some("root"), Some("bad-version"))
        .is_err());
    assert!(b
        .store
        .delete_local("thread-one", Some("root"), Some(&first.version_id))
        .is_err());
    assert_eq!(
        snapshot_at(&b.store, "receiver", None).unwrap().threads[0]
            .posts
            .len(),
        2
    );
    assert!(b.receive_post(&first, &raw).await.is_err());
    settle(&b.files).await;
    // A stale local version action cannot silently target the last survivor.
    assert_eq!(
        b.store
            .delete_local("thread-one", Some("root"), Some(&second.version_id))
            .unwrap_err()
            .to_string(),
        "last_version_use_logical_delete"
    );
    // A valid remote tombstone still wins, even if it leaves no root to show.
    b.page(
        ManifestPhase::Tombstones,
        vec![serde_json::to_value(Tombstone {
            thread_id: "thread-one".into(),
            post_id: Some("root".into()),
            version_id: Some(second.version_id.clone()),
            deleted_at: now_iso(),
        })
        .unwrap()],
    )
    .await;
    assert!(b
        .store
        .root
        .join("threads/thread-one/posts/reply-one.md")
        .is_file());
    assert!(snapshot_at(&b.store, "receiver", None)
        .unwrap()
        .threads
        .is_empty());
    assert!(!b.state().tombstones.contains_key("thread:thread-one"));
}

#[tokio::test]
async fn invalid_frontmatter_role_second_root_and_late_membership_are_rejected_before_publish() {
    let b = Board::new();
    b.thread().await;
    let (raw, root) = raw_post("root", "topic", "valid", vec![]);
    b.receive_post(&root, &raw).await.unwrap();
    let (raw2, second) = raw_post(
        "another-root",
        "topic",
        "invalid second logical root",
        vec![],
    );
    assert!(b.receive_post(&second, &raw2).await.is_err());
    settle(&b.files).await;
    let mut bad = root.clone();
    bad.kind = "reply".into();
    assert!(b.receive_post(&bad, &raw).await.is_err());
    settle(&b.files).await;
    let changed = String::from_utf8(raw.clone())
        .unwrap()
        .replace("threadId: thread-one", "threadId: other-thread")
        .into_bytes();
    let mut bad = root.clone();
    bad.version_id = bbs_sync::raw_sha256(&changed);
    bad.size_bytes = changed.len() as u64;
    assert!(b.receive_post(&bad, &changed).await.is_err());
    settle(&b.files).await;
    let (later_raw, later) = raw_post("reply-late", "reply", "late", vec![]);
    let verified = b
        .verified(reconcile::post_resource(&later), &later_raw)
        .await;
    b.set_group(None);
    assert!(b
        .store
        .install_received(
            verified,
            b.fence.clone(),
            Some(later),
            &Cancellation::default()
        )
        .await
        .is_err());
    settle(&b.files).await;
    assert_eq!(
        load_thread(b.store.root(), "thread-one").unwrap().1.len(),
        1
    );
    assert!(fs::read_dir(b.store.prepare_layout().unwrap())
        .unwrap()
        .next()
        .is_none());
}

#[tokio::test]
async fn immutable_received_avatar_does_not_resample_local_hero_and_native_omits_field() {
    let b = Board::new();
    b.thread().await;
    let (raw, mut p) = raw_post("root", "topic", "source", vec![]);
    b.receive_post(&p, &raw).await.unwrap();
    let value = serde_json::to_value(snapshot_at(&b.store, "receiver", None).unwrap()).unwrap();
    let post = &value["threads"][0]["posts"][0];
    assert_eq!(post["syncAvatar"], json!({"kind":"none"}));
    assert!(post.get("agentAvatar").is_none());
    // A forwarding peer cannot replace an already captured sidecar.
    p.avatar = Some(AvatarSidecar {
        kind: "builtin".into(),
        id: Some("codex".into()),
        ..AvatarSidecar::none()
    });
    b.receive_post(&p, &raw).await.unwrap();
    assert!(matches!(
        b.store
            .avatar_view("thread-one", "root", &p.version_id, Some(&b.state())),
        Some(AvatarView::None)
    ));
    let id = human_identity("local-project", "Human".into(), Some("user-default".into()));
    let local =
        create_thread_scoped(&b.store, &id, vec![], true, "Local post".into(), &[]).unwrap();
    let value = serde_json::to_value(snapshot_at(&b.store, "receiver", None).unwrap()).unwrap();
    let local = value["threads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["threadId"] == local)
        .unwrap();
    assert!(local["posts"][0].get("syncAvatar").is_none());
    assert_eq!(local["posts"][0]["agentAvatar"], "user-default");
}

#[tokio::test]
async fn new_root_registration_is_membership_scoped_and_private_delete_reenters_only_on_peer_announcement(
) {
    let b = Board::new();
    b.set_group(None);
    let id = human_identity("local-project", "Human".into(), None);
    let old =
        create_thread_scoped(&b.store, &id, vec![], true, "Private history".into(), &[]).unwrap();
    b.store.delete_local(&old, None, None).unwrap();
    assert!(b.state().tombstones.contains_key(&format!("thread:{old}")));
    b.set_group(Some(&b.fence));
    let new = create_thread_scoped(&b.store, &id, vec![], true, "New root".into(), &[]).unwrap();
    assert_eq!(
        b.state().groups[&b.fence.group_id].shared_threads,
        BTreeSet::from([new])
    );
    let empty = b
        .store
        .catalog(&b.fence)
        .unwrap()
        .page(ManifestPhase::Tombstones, None)
        .unwrap()
        .0;
    assert!(empty.items.is_empty());
    let d = Tombstone {
        thread_id: old.clone(),
        deleted_at: "2099-01-01T00:00:00Z".into(),
        ..Default::default()
    };
    b.page(
        ManifestPhase::Tombstones,
        vec![serde_json::to_value(d).unwrap()],
    )
    .await;
    assert!(b.state().groups[&b.fence.group_id]
        .shared_threads
        .contains(&old));
    assert_eq!(
        b.store
            .catalog(&b.fence)
            .unwrap()
            .page(ManifestPhase::Tombstones, None)
            .unwrap()
            .0
            .items
            .len(),
        1
    );
    let other = GroupFence {
        group_id: "group-other".into(),
        membership_id: "other-member".into(),
    };
    b.set_group(Some(&other));
    for phase in [
        ManifestPhase::Tombstones,
        ManifestPhase::Threads,
        ManifestPhase::Versions,
    ] {
        assert!(b
            .store
            .catalog(&other)
            .unwrap()
            .page(phase, None)
            .unwrap()
            .0
            .items
            .is_empty());
    }
}

#[tokio::test]
async fn promotion_recovery_validates_moved_attachments_and_handles_missing_or_wrong_destination() {
    for destination in ["good", "missing", "wrong"] {
        let b = Board::new();
        b.thread().await;
        let bytes = b"verified";
        let (raw, first) = raw_post("root", "topic", "first", vec![]);
        let (raw2, second) = raw_post("root", "topic", "second", vec![attachment(bytes)]);
        b.receive_post(&first, &raw).await.unwrap();
        b.receive_post(&second, &raw2).await.unwrap();
        let file = b
            .verified(
                reconcile::attachment_resource(&second, &second.attachments[0]),
                bytes,
            )
            .await;
        b.store
            .install_received(file, b.fence.clone(), None, &Cancellation::default())
            .await
            .unwrap();
        let thread = b.store.root.join("threads/thread-one");
        let extra = thread.join(format!("versions/root/{}", second.version_id));
        // Simulate termination after persistent delete + attachment move but
        // before moving post.md. No invented journal or success marker.
        {
            let lock = acquire_write_lock(b.store.root()).unwrap();
            let mut state = b.store.state_locked(&lock).unwrap();
            let d = Tombstone {
                thread_id: "thread-one".into(),
                post_id: Some("root".into()),
                version_id: Some(first.version_id.clone()),
                deleted_at: now_iso(),
            };
            state.tombstones.insert(d.key().unwrap(), d);
            b.store.save_locked(&lock, &state).unwrap();
        }
        fs::remove_file(thread.join("posts/root.md")).unwrap();
        fs::create_dir_all(thread.join("attachments")).unwrap();
        fs::rename(extra.join("attachments"), thread.join("attachments/root")).unwrap();
        if destination == "missing" {
            fs::remove_dir_all(thread.join("attachments/root")).unwrap();
        }
        if destination == "wrong" {
            fs::write(thread.join("attachments/root/att-one.txt"), b"incorrect").unwrap();
        }
        let store = b.store.clone();
        let fence = b.fence.clone();
        b.files
            .run(&Cancellation::default(), move |io| {
                store.recover_deletions(Some(&fence), &BTreeSet::from(["thread-one".into()]), io)
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fs::read(thread.join("posts/root.md")).unwrap(), raw2);
        b.refresh().await;
        let view = snapshot_at(&b.store, "receiver", None).unwrap();
        assert_eq!(
            view.threads[0].posts[0].attachments[0].available,
            destination == "good"
        );
    }
}

#[tokio::test]
async fn file_worker_budget_cancellation_cleanup_and_owned_staging_recovery() {
    let b = Board::new();
    b.thread().await;
    let (raw, p) = raw_post("root", "topic", "bytes", vec![]);
    let verified = b.verified(reconcile::post_resource(&p), &raw).await;
    // VerifiedFile retains the account permit through installation; a scan
    // cannot start another file operation or deadlock by reacquiring it.
    assert!(b.files.run(&Cancellation::default(), |_| ()).await.is_err());
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(b
        .store
        .install_received(verified, b.fence.clone(), Some(p), &cancel)
        .await
        .is_err());
    settle(&b.files).await;
    let stage = b.store.prepare_layout().unwrap();
    let owned = stage.join(format!(".receive-{}.partial", Uuid::new_v4()));
    let unknown = stage.join("user-recovery.partial");
    fs::write(&owned, b"orphan").unwrap();
    fs::write(&unknown, b"preserve").unwrap();
    let store = b.store.clone();
    b.files
        .run(&Cancellation::default(), move |io| {
            store.recover_staging(io)
        })
        .await
        .unwrap()
        .unwrap();
    assert!(!owned.exists());
    assert_eq!(fs::read(unknown).unwrap(), b"preserve");
    assert!(load_thread(b.store.root(), "thread-one")
        .unwrap()
        .1
        .is_empty());
}

#[tokio::test]
async fn corrupt_state_and_symlink_destinations_fail_closed_without_touching_outside() {
    let b = Board::new();
    b.thread().await;
    let (raw, p) = raw_post("root", "topic", "bytes", vec![]);
    let outside = b.directory.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::create_dir_all(b.store.root.join("threads/thread-one")).unwrap();
    std::os::unix::fs::symlink(&outside, b.store.root.join("threads/thread-one/posts")).unwrap();
    assert!(b.receive_post(&p, &raw).await.is_err());
    settle(&b.files).await;
    assert!(fs::read_dir(&outside).unwrap().next().is_none());
    fs::remove_file(b.store.root.join("threads/thread-one/posts")).unwrap();
    let original = b"{broken-private-state";
    fs::write(b.store.state.state_path(), original).unwrap();
    assert!(b.receive_post(&p, &raw).await.is_err());
    settle(&b.files).await;
    assert_eq!(fs::read(b.store.state.state_path()).unwrap(), original);
    assert!(!b
        .store
        .root
        .join("threads/thread-one/posts/root.md")
        .exists());
}

#[tokio::test]
async fn unchanged_inventory_uses_file_proofs_and_manual_edit_keeps_raw_byte_versioning() {
    let b = Board::new();
    let source = b.directory.join("input.bin");
    fs::write(&source, vec![7u8; 128 * 1024]).unwrap();
    let identity = human_identity("local-project", "Human".into(), None);
    let thread = create_thread_scoped(
        &b.store,
        &identity,
        vec![],
        true,
        "original".into(),
        &[BbsAttachmentSource {
            path: source.display().to_string(),
            name: None,
        }],
    )
    .unwrap();
    let count = |store: ContentStore, fence: GroupFence, files: FileIo| async move {
        files
            .run(&Cancellation::default(), move |io| {
                let issues = store.refresh(&fence, io)?;
                assert!(issues.is_empty());
                Ok::<_, anyhow::Error>(io.charged_bytes())
            })
            .await
            .unwrap()
            .unwrap()
    };
    assert!(count(b.store.clone(), b.fence.clone(), b.files.clone()).await >= 128 * 1024);
    assert!(count(b.store.clone(), b.fence.clone(), b.files.clone()).await < 16 * 1024);
    let (_, posts) = load_thread(b.store.root(), &thread).unwrap();
    let old = posts[0].version_id.clone();
    let post = posts[0].meta.post_id.clone();
    let mut changed = fs::read(&posts[0].path).unwrap();
    changed.extend_from_slice(b"\nmanual change\n");
    fs::write(&posts[0].path, &changed).unwrap();
    b.refresh().await;
    let state = b.state();
    assert!(state.ledger[&format!("{post}/{old}")].body_stamp.is_none());
    let new = bbs_sync::raw_sha256(&changed);
    assert!(state.ledger[&format!("{post}/{new}")].body_stamp.is_some());
    assert_eq!(fs::read(&posts[0].path).unwrap(), changed);
}

#[tokio::test]
async fn a_new_global_delete_survives_a_concurrent_inventory_state_save() {
    let b = Board::new();
    let identity = human_identity("local-project", "Human".into(), None);
    let active =
        create_thread_scoped(&b.store, &identity, vec![], true, "Shared".into(), &[]).unwrap();
    for n in 0..8 {
        // Publish outside the current group so the background scan has no
        // reason to visit this thread, while sharing the same private state.
        b.set_group(None);
        let private = create_thread_scoped(
            &b.store,
            &identity,
            vec![],
            true,
            format!("private-{n}"),
            &[],
        )
        .unwrap();
        b.set_group(Some(&b.fence));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let peer = barrier.clone();
        let store = b.store.clone();
        let deleted = private.clone();
        let deletion = std::thread::spawn(move || {
            peer.wait();
            store.delete_local(&deleted, None, None).unwrap();
        });
        let store = b.store.clone();
        let fence = b.fence.clone();
        b.files
            .run(&Cancellation::default(), move |io| {
                barrier.wait();
                store.refresh(&fence, io)
            })
            .await
            .unwrap()
            .unwrap();
        deletion.join().unwrap();
        let state = b.state();
        assert!(state.tombstones.contains_key(&format!("thread:{private}")));
        assert!(state.groups[&b.fence.group_id]
            .shared_threads
            .contains(&active));
        assert!(!state.groups[&b.fence.group_id]
            .shared_threads
            .contains(&private));
    }
    assert_eq!(b.state().tombstones.len(), 8);
}

#[tokio::test]
async fn changed_extra_version_is_rehomed_and_captured_origin_is_retained() {
    let b = Board::new();
    b.thread().await;
    let (raw, first) = raw_post("root", "topic", "first", vec![]);
    let (raw2, second) = raw_post("root", "topic", "second", vec![]);
    b.receive_post(&first, &raw).await.unwrap();
    b.receive_post(&second, &raw2).await.unwrap();
    let old_path = b
        .store
        .source(&b.fence, &reconcile::post_resource(&second))
        .unwrap();
    let mut raw3 = raw2;
    raw3.extend_from_slice(b"\nmanual extra edit\n");
    fs::write(&old_path, &raw3).unwrap();
    // Even before the new hash is indexed, a received post cannot sample the
    // coincident local `codex` hero ID in its original frontmatter.
    let view = snapshot_at(&b.store, "receiver", None).unwrap();
    assert!(view.threads[0]
        .posts
        .iter()
        .all(|p| matches!(p.sync_avatar, Some(AvatarView::None))));
    b.refresh().await;
    let hash = bbs_sync::raw_sha256(&raw3);
    assert!(!old_path.exists());
    assert_eq!(
        fs::read(
            b.store
                .root
                .join(format!("threads/thread-one/versions/root/{hash}/post.md"))
        )
        .unwrap(),
        raw3
    );
    assert!(b.state().ledger[&format!("root/{}", second.version_id)]
        .body_stamp
        .is_none());
}

#[tokio::test]
async fn same_post_id_in_unindexed_private_legacy_thread_cannot_change_parent() {
    let b = Board::new();
    b.thread().await;
    let legacy = b.store.root.join("threads/private-legacy");
    fs::create_dir_all(legacy.join("posts")).unwrap();
    let mut thread = meta_from_record(&record().record);
    thread.thread_id = "private-legacy".into();
    write_thread_meta(&legacy.join("thread.yaml"), &thread).unwrap();
    let mut old = metadata("root", "topic", vec![]);
    old.thread_id = "private-legacy".into();
    write_post(&legacy, &old, "Private legacy body").unwrap();
    let before = fs::read(legacy.join("posts/root.md")).unwrap();
    let (raw, incoming) = raw_post("root", "topic", "wrong new parent", vec![]);
    assert!(b.receive_post(&incoming, &raw).await.is_err());
    settle(&b.files).await;
    assert!(!b
        .store
        .root
        .join("threads/thread-one/posts/root.md")
        .exists());
    assert_eq!(fs::read(legacy.join("posts/root.md")).unwrap(), before);
    assert!(!b.state().groups[&b.fence.group_id]
        .shared_threads
        .contains("private-legacy"));
}

#[tokio::test]
async fn snapshot_image_path_is_null_until_the_registered_bytes_are_verified() {
    let b = Board::new();
    b.thread().await;
    let png = b"\x89PNG\r\n\x1a\nsource-avatar";
    let (raw, mut p) = raw_post("root", "topic", "image source", vec![]);
    p.avatar = Some(AvatarSidecar {
        kind: "image".into(),
        id: None,
        sha256: Some(bbs_sync::raw_sha256(png)),
        ext: Some("png".into()),
        size_bytes: Some(png.len() as u64),
    });
    b.receive_post(&p, &raw).await.unwrap();
    let snapshot =
        || serde_json::to_value(snapshot_at(&b.store, "receiver", None).unwrap()).unwrap();
    let view = snapshot();
    let avatar = &view["threads"][0]["posts"][0]["syncAvatar"];
    assert_eq!(avatar["available"], false);
    assert!(avatar["localPath"].is_null());
    let r = reconcile::avatar_resource(p.avatar.as_ref().unwrap()).unwrap();
    assert!(b.store.read_verified_avatar(&r.sha256, "png").is_err());
    let verified = b.verified(r.clone(), png).await;
    b.store
        .install_received(verified, b.fence.clone(), None, &Cancellation::default())
        .await
        .unwrap();
    let view = snapshot();
    let avatar = &view["threads"][0]["posts"][0]["syncAvatar"];
    assert_eq!(avatar["available"], true);
    assert!(avatar["localPath"].as_str().is_some());
    use base64::Engine as _;
    let data = b.store.read_verified_avatar(&r.sha256, "png").unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(data.strip_prefix("data:image/png;base64,").unwrap())
            .unwrap(),
        png
    );
    assert!(b.store.read_verified_avatar(&r.sha256, "gif").is_err());
    assert!(b.store.read_verified_avatar("../escape", "png").is_err());
    let path = b.store.source(&b.fence, &r).unwrap();
    fs::write(&path, b"unverified modification").unwrap();
    assert!(b.store.read_verified_avatar(&r.sha256, "png").is_err());
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(b.store.root().join("thread.yaml"), &path).unwrap();
    assert!(b.store.read_verified_avatar(&r.sha256, "png").is_err());
    let view = snapshot();
    let avatar = &view["threads"][0]["posts"][0]["syncAvatar"];
    assert_eq!(avatar["available"], false);
    assert!(avatar["localPath"].is_null());
}

#[tokio::test]
async fn a_hash_match_does_not_copy_an_unrelated_posts_private_attachment() {
    let b = Board::new();
    b.thread().await;
    let bytes = b"same bytes, different logical post";
    let (raw, root) = raw_post("root", "topic", "existing", vec![attachment(bytes)]);
    b.receive_post(&root, &raw).await.unwrap();
    let file = b
        .verified(
            reconcile::attachment_resource(&root, &root.attachments[0]),
            bytes,
        )
        .await;
    b.store
        .install_received(file, b.fence.clone(), None, &Cancellation::default())
        .await
        .unwrap();
    let mut entry = attachment(bytes);
    entry.path = "attachments/unrelated/att-one.txt".into();
    let (raw, reply) = raw_post("unrelated", "reply", "another manifest", vec![entry]);
    b.receive_post(&reply, &raw).await.unwrap();
    let target = reconcile::attachment_resource(&reply, &reply.attachments[0]);
    let store = b.store.clone();
    let fence = b.fence.clone();
    assert!(!b
        .files
        .run(&Cancellation::default(), move |io| store
            .copy_available(&fence, &target, None, io))
        .await
        .unwrap()
        .unwrap());
}

#[tokio::test]
async fn same_size_partial_edit_after_fsync_invalidates_the_receive_proof() {
    let b = Board::new();
    b.thread().await;
    let (raw, root) = raw_post("root", "topic", "file owner", vec![attachment(b"abc")]);
    b.receive_post(&root, &raw).await.unwrap();
    let resource = reconcile::attachment_resource(&root, &root.attachments[0]);
    let verified = b.verified(resource, b"abc").await;
    fs::write(verified.path(), b"xyz").unwrap();
    assert!(b
        .store
        .install_received(verified, b.fence.clone(), None, &Cancellation::default())
        .await
        .is_err());
    settle(&b.files).await;
    let snapshot = snapshot_at(&b.store, "receiver", None).unwrap();
    assert!(!snapshot.threads[0].posts[0].attachments[0].available);
    assert!(!b
        .store
        .root
        .join("threads/thread-one/attachments/root/att-one.txt")
        .exists());
}

#[tokio::test]
async fn invalid_relationship_alone_cannot_enroll_private_root_or_hide_valid_page_neighbor() {
    let b = Board::new();
    b.thread().await;
    let (raw, original) = raw_post("root", "topic", "known root", vec![]);
    b.receive_post(&original, &raw).await.unwrap();
    let mut invalid = original.clone();
    invalid.thread_id = "private-root".into();
    let (_, valid) = raw_post("reply", "reply", "valid neighbor", vec![]);
    let mut items = vec![invalid, valid.clone()];
    items.sort_by_key(|p| (p.thread_id.clone(), p.post_id.clone(), p.version_id.clone()));
    let plan = b
        .page(
            ManifestPhase::Versions,
            items
                .into_iter()
                .map(|p| serde_json::to_value(p).unwrap())
                .collect(),
        )
        .await;
    assert!(plan
        .issues
        .iter()
        .any(|i| i.code == "invalid_post_relationship"));
    assert!(plan.versions.iter().any(|p| p == &valid));
    assert!(!b.state().groups[&b.fence.group_id]
        .shared_threads
        .contains("private-root"));
    let catalog = b.store.catalog(&b.fence).unwrap();
    for phase in [
        ManifestPhase::Tombstones,
        ManifestPhase::Threads,
        ManifestPhase::Versions,
    ] {
        let (page, _) = catalog.page(phase, None).unwrap();
        assert!(!serde_json::to_string(&page)
            .unwrap()
            .contains("private-root"));
    }
}

#[tokio::test]
async fn sync_eligibility_does_not_limit_local_posts_or_hide_other_valid_records() {
    let b = Board::new();
    let identity = human_identity("local-project", "Human".into(), None);
    let huge = "z".repeat(reconcile::MAX_BODY_BYTES as usize + 1);
    let huge_thread =
        create_thread_scoped(&b.store, &identity, vec![], true, huge.clone(), &[]).unwrap();
    assert_eq!(
        load_thread(b.store.root(), &huge_thread).unwrap().1[0].body,
        huge
    );
    let tags: Vec<_> = (0..32)
        .map(|n| format!("tag-{n:02}-{}", "x".repeat(54)))
        .collect();
    let bad_record = create_thread_scoped(
        &b.store,
        &identity,
        tags,
        false,
        "body remains local".into(),
        &[],
    )
    .unwrap();
    let good = create_thread_scoped(
        &b.store,
        &identity,
        vec![],
        true,
        "valid peer item".into(),
        &[],
    )
    .unwrap();
    let store = b.store.clone();
    let fence = b.fence.clone();
    let issues = b
        .files
        .run(&Cancellation::default(), move |io| {
            store.refresh(&fence, io)
        })
        .await
        .unwrap()
        .unwrap();
    assert!(!issues
        .iter()
        .any(|e| e.thread_id.as_ref() == Some(&huge_thread)));
    let unavailable = b
        .store
        .catalog(&b.fence)
        .unwrap()
        .page(ManifestPhase::Unavailable, None)
        .unwrap()
        .0;
    assert!(unavailable
        .items
        .iter()
        .any(|v| v["thread_id"] == huge_thread && v["reason"] == "too_large_to_sync"));
    assert!(issues
        .iter()
        .any(|e| e.code == "invalid_local_thread_record"
            && e.thread_id.as_ref() == Some(&bad_record)));
    let page = b
        .store
        .catalog(&b.fence)
        .unwrap()
        .page(ManifestPhase::Threads, None)
        .unwrap()
        .0;
    assert!(page.items.iter().any(|v| v["record"]["threadId"] == good));
    assert!(!page
        .items
        .iter()
        .any(|v| v["record"]["threadId"] == bad_record));
    assert_eq!(
        load_thread(b.store.root(), &huge_thread).unwrap().1[0]
            .body
            .len(),
        reconcile::MAX_BODY_BYTES as usize + 1
    );
}
