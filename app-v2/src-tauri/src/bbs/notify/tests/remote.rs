use super::*;
use crate::bbs_sync::{
    AttachmentRef, AvatarSidecar, Manifest, ManifestPhase, PostVersion, ThreadRecord,
};

pub(super) struct Receiver {
    pub b: Board,
    pub files: FileIo,
    pub fence: sync::GroupFence,
    pub id: String,
    pub via: String,
    content: sync::ContentStore,
}
impl Receiver {
    pub async fn new() -> Self {
        let b = Board::new();
        let (_, id, via) = crate::bbs_sync::roster::tests::joined(&b.account);
        let content = b.content.receiving_from(&via).unwrap();
        let result = Self {
            b,
            files: FileIo::start(Limits::default()).unwrap(),
            fence: sync::GroupFence {
                group_id: "group-a".into(),
                membership_id: "own-membership".into(),
            },
            id,
            via,
            content,
        };
        let record = reconcile::thread_item(ThreadRecord {
            schema: THREAD_SCHEMA.into(),
            thread_id: "thread-received".into(),
            status: "open".into(),
            visibility: "targeted".into(),
            project_tags: vec!["source".into()],
            created_by_project: "source".into(),
            created_by_agent: "remote-agent".into(),
            created_at: "2026-09-13T10:00:00Z".into(),
        })
        .unwrap();
        let store = result.content.clone();
        let fence = result.fence.clone();
        result
            .files
            .run_when_available(&Cancellation::default(), move |io| {
                let mut reader = reconcile::ManifestReader::default();
                store.process_page(
                    &fence,
                    &Manifest {
                        phase: ManifestPhase::Tombstones,
                        items: vec![],
                        next: None,
                    },
                    &mut reader,
                    io,
                )?;
                store.process_page(
                    &fence,
                    &Manifest {
                        phase: ManifestPhase::Threads,
                        items: vec![serde_json::to_value(record).unwrap()],
                        next: None,
                    },
                    &mut reader,
                    io,
                )
            })
            .await
            .unwrap()
            .unwrap();
        result
    }
    pub fn target(&self) -> mentions::Mention {
        mentions::Mention {
            device_id: self.id.clone(),
            project_id: "target".into(),
            agent_id: "agent".into(),
        }
    }
    pub fn bytes(
        &self,
        post: &str,
        kind: &str,
        body: &str,
        mentions: Vec<mentions::Mention>,
        attachments: Vec<BbsAttachment>,
    ) -> (Vec<u8>, PostVersion) {
        let meta = BbsPostMeta {
            schema: POST_SCHEMA.into(),
            post_id: post.into(),
            thread_id: "thread-received".into(),
            project_id: "source".into(),
            project_display_name: "Remote Source".into(),
            agent_id: "remote-agent".into(),
            agent_display_name: "Remote Author".into(),
            agent_avatar: None,
            created_at: "2026-09-13T10:00:00Z".into(),
            kind: kind.into(),
            attachments,
            mentions,
        };
        let raw = serialized_post(&meta, body).unwrap().into_bytes();
        let p = PostVersion {
            thread_id: meta.thread_id.clone(),
            post_id: meta.post_id.clone(),
            version_id: bbs_sync::raw_sha256(&raw),
            size_bytes: raw.len() as u64,
            kind: kind.into(),
            attachments: meta
                .attachments
                .iter()
                .map(|a| AttachmentRef {
                    id: a.id.clone(),
                    sha256: a.sha256.clone(),
                    size_bytes: a.size_bytes,
                    ext: "txt".into(),
                    available: true,
                })
                .collect(),
            avatar: Some(AvatarSidecar::none()),
        };
        (raw, p)
    }
    pub async fn resource(
        &self,
        resource: crate::bbs_sync::transport::Resource,
        raw: &[u8],
        offered: Option<PostVersion>,
    ) {
        let cancel = Cancellation::default();
        let mut writer = self
            .files
            .receive(
                resource.clone(),
                &self.content.prepare_layout().unwrap(),
                &cancel,
            )
            .await
            .unwrap();
        let mut offset = 0;
        for bytes in raw.chunks(16_000) {
            offset = writer
                .write(offset, bytes::BytesMut::from(bytes))
                .await
                .unwrap();
        }
        let verified = writer.finish(resource.sha256.clone()).await.unwrap();
        self.content
            .install_received(verified, self.fence.clone(), offered, &cancel)
            .await
            .unwrap();
    }
    pub async fn post(&self, raw: &[u8], post: &PostVersion) {
        self.resource(reconcile::post_resource(post), raw, Some(post.clone()))
            .await;
    }
}
pub(super) fn attachment(post: &str, id: &str, bytes: &[u8]) -> BbsAttachment {
    BbsAttachment {
        id: id.into(),
        name: format!("{id}.txt"),
        path: format!("attachments/{post}/{id}.txt"),
        sha256: bbs_sync::raw_sha256(bytes),
        size_bytes: bytes.len() as u64,
    }
}

#[tokio::test]
async fn remote_text_reply_waits_for_existing_root_attachment_and_freezes_before_future_posts() {
    let r = Receiver::new().await;
    let bytes = b"verified root image bytes";
    let (raw, root) = r.bytes(
        "root",
        "topic",
        "Root",
        vec![],
        vec![attachment("root", "image", bytes)],
    );
    r.post(&raw, &root).await;
    assert!(
        !r.b.queue.root().exists(),
        "no metadata target means no notification"
    );
    let (raw, reply) = r.bytes(
        "reply",
        "reply",
        "Original remote instruction",
        vec![r.target(), r.b.target()],
        vec![],
    );
    r.post(&raw, &reply).await;
    let key = r.b.keys("pending")[0].clone();
    let before = r.b.record(&key);
    assert_eq!(
        r.b.keys("pending").len(),
        1,
        "remote local is never rebound"
    );
    assert_eq!(before.waiting.len(), 1);
    assert_eq!(before.waiting[0].post_id, "root");
    assert!(matches!(
        r.b.queue
            .check(&key, "run", before.deadline_ms - 1)
            .unwrap(),
        store::Check::Wait { .. }
    ));
    let (raw, future) = r.bytes(
        "future",
        "reply",
        "Later",
        vec![],
        vec![attachment("future", "new", b"future")],
    );
    r.post(&raw, &future).await;
    assert_eq!(r.b.record(&key).waiting, before.waiting);
    assert_eq!(r.b.record(&key).deadline_ms, before.deadline_ms);
    let resource = reconcile::attachment_resource(&root, &root.attachments[0]);
    r.resource(resource, bytes, None).await;
    let claimed = claim(&r.b.queue, &key, "run", before.deadline_ms - 1);
    assert!(!claimed.attachment_warning);
    let sink = RecordingSink::default();
    delivery::dispatch(&r.b.queue, claimed, &sink).unwrap();
    let calls = sink.calls.lock().unwrap();
    let text = calls[0]["text"].as_str().unwrap();
    assert!(text.contains(&r.via));
    assert!(text.contains("Remote Author"));
    assert!(text.contains("Original remote instruction"));
    assert!(!text.contains("User instruction:"));
    assert!(text.contains("not a human instruction"));
    drop(calls);
    // A valid same-ID different-body Fork is still the same notification key.
    let (fork, fork_post) = r.bytes("reply", "reply", "Fork", vec![r.target()], vec![]);
    r.post(&fork, &fork_post).await;
    r.post(&raw, &future).await;
    assert!(r.b.keys("pending").is_empty());
    assert_eq!(sink.calls.lock().unwrap().len(), 1);
    r.files.stop_and_wait().await;
}

#[tokio::test]
async fn deleted_body_and_invalid_partial_cannot_create_or_resurrect_a_notification() {
    let r = Receiver::new().await;
    let (raw, post) = r.bytes("root", "topic", "Valid body", vec![r.target()], vec![]);
    let resource = reconcile::post_resource(&post);
    let cancel = Cancellation::default();
    let mut bad = r
        .files
        .receive(
            resource.clone(),
            &r.content.prepare_layout().unwrap(),
            &cancel,
        )
        .await
        .unwrap();
    bad.write(0, bytes::BytesMut::from(vec![b'x'; raw.len()].as_slice()))
        .await
        .unwrap();
    assert!(bad.finish(resource.sha256).await.is_err());
    assert!(!r.b.queue.root().exists());
    // Wait for failed writer cleanup to release the single account file lease.
    r.files
        .run_when_available(&Cancellation::default(), |_| ())
        .await
        .unwrap();
    r.post(&raw, &post).await;
    let key = r.b.keys("pending")[0].clone();
    r.b.content
        .delete_local("thread-received", None, None)
        .unwrap();
    assert!(matches!(
        r.b.queue.check(&key, "run", now_ms()).unwrap(),
        store::Check::Done
    ));
    assert_eq!(r.b.keys("processed"), vec![key]);
    r.files.stop_and_wait().await;
}
