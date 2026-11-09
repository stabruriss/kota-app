//! v5 attachment slices over the real loopback DataPipe, the same file path
//! relay TLS uses. Slices are 64 KiB under cfg(test).
use super::*;
use crate::bbs_sync::transport::{self, RESUME_SLICE as SLICE};

fn pattern(len: u64) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}
/// A text-only topic in its own thread. Post IDs are global, so the topic
/// is named after its thread.
fn publish_text(store: &ContentStore, thread: &str, body: &str) -> Vec<u8> {
    let post = format!("{thread}-root");
    let root = store.root().join("threads").join(thread);
    fs::create_dir_all(root.join("posts")).unwrap();
    let record = json!({"schema":"kota.bbs.thread.v1","threadId":thread,"status":"open","visibility":"targeted","projectTags":["source"],"createdByProject":"source","createdByAgent":"author","createdAt":"2026-09-12T10:00:00Z","updatedAt":"2026-09-12T10:00:00Z","latestPostId":post});
    fs::write(root.join("thread.yaml"), serde_yaml::to_string(&record).unwrap()).unwrap();
    let meta = json!({"schema":"kota.bbs.post.v1","threadId":thread,"postId":post,"projectId":"source","agentId":"author","agentDisplayName":"Author","projectDisplayName":"Source","createdAt":"2026-09-12T10:00:00Z","kind":"topic","attachments":[]});
    let bytes = format!("---\n{}---\n\n{body}\n", serde_yaml::to_string(&meta).unwrap()).into_bytes();
    fs::write(root.join("posts").join(format!("{post}.md")), &bytes).unwrap();
    let mut state = store.state.load_state().unwrap().unwrap_or_default();
    state.groups.entry("group-test".into()).or_insert_with(GroupState::default)
        .shared_threads.insert(thread.into());
    store.state.save_state(&state).unwrap();
    bytes
}
fn text_resource(thread: &str, post: &[u8]) -> Resource {
    Resource {
        identity: ResourceKind::Post {
            thread_id: thread.into(),
            post_id: format!("{thread}-root"),
            version_id: raw_sha256(post),
        },
        sha256: raw_sha256(post),
        size_bytes: post.len() as u64,
    }
}
async fn refresh(host: &NetworkHost, engine: &Arc<Engine>) {
    let engine = engine.clone();
    host.execute(move |_| async move { engine.refresh(&Cancellation::default()).await })
        .await
        .unwrap();
}
fn post_resource(post: &[u8]) -> Resource {
    Resource {
        identity: ResourceKind::Post {
            thread_id: "thread-one".into(),
            post_id: "root".into(),
            version_id: raw_sha256(post),
        },
        sha256: raw_sha256(post),
        size_bytes: post.len() as u64,
    }
}
fn attachment_resource(post: &[u8], bytes: &[u8]) -> Resource {
    Resource {
        identity: ResourceKind::Attachment {
            thread_id: "thread-one".into(),
            post_id: "root".into(),
            version_id: raw_sha256(post),
            attachment_id: "file".into(),
        },
        sha256: raw_sha256(bytes),
        size_bytes: bytes.len() as u64,
    }
}
fn partial(store: &ContentStore, bytes: &[u8]) -> PathBuf {
    store
        .root()
        .join(".sync-staging")
        .join(transport::resumable_partial_name(&raw_sha256(bytes), bytes.len() as u64))
}
fn ranges(link: &Link) -> Vec<(u64, u64)> {
    link.served.lock().unwrap().iter().flatten().copied().collect()
}
async fn round(pair: &Connected) -> Result<Outcome> {
    let a = pair.a.clone();
    let result = pair.ha.execute(move |_| async move { a.round().await }).await;
    if result.is_ok() {
        responder_finished(pair).await;
    }
    result
}

#[tokio::test]
async fn lost_link_resumes_at_the_last_fsynced_slice_without_resending_the_prefix() {
    let (_ra, sa, fa) = board("member-a");
    let (_rb, sb, fb) = board("member-b");
    let bytes = pattern(3 * SLICE + 1234);
    let post = publish_with(&sb, "large attachment", &bytes);
    {
        let pair = connect_v5(sa.clone(), fa.clone(), sb.clone(), fb.clone()).await;
        let stop = pair.a.connection.cancellation();
        *pair.a.after_slice.lock().unwrap() = Some(Box::new(move |at| {
            if at == 2 * SLICE {
                stop.cancel();
            }
        }));
        assert!(round(&pair).await.is_err());
        assert_eq!(ranges(&pair.b), vec![(0, SLICE), (SLICE, SLICE)]);
    }
    let staged = partial(&sa, &bytes);
    assert_eq!(fs::metadata(&staged).unwrap().len(), 2 * SLICE);
    // Bytes after the last fsync are not trusted, even if they look right.
    let mut file = fs::OpenOptions::new().append(true).open(&staged).unwrap();
    std::io::Write::write_all(&mut file, &bytes[(2 * SLICE) as usize..(2 * SLICE + 1000) as usize]).unwrap();
    drop(file);
    // A restarted account: new engines, connection and file worker, so the
    // prefix hash is re-read. Without pull history its catalog (which now
    // echoes B's body) counts as owed once: it yields at boundary 0.
    let pair = connect_v5(sa.clone(), fa.clone(), sb, fb).await;
    let yielded = round(&pair).await.unwrap();
    assert!(yielded.more && ranges(&pair.b).is_empty());
    let outcome = round(&pair).await.unwrap();
    assert!(outcome.settled() && outcome.failures.is_empty(), "{:?}", outcome.failures.len());
    assert_eq!(ranges(&pair.b), vec![(2 * SLICE, SLICE), (3 * SLICE, 1234)]);
    assert!(!staged.exists());
    assert!(sa.source(&fa, &attachment_resource(&post, &bytes)).is_ok());
}

#[tokio::test]
async fn the_peer_takes_our_body_before_we_take_any_slice_of_its_large_attachment() {
    let (_ra, sa, fa) = board("member-a");
    let (_rb, sb, fb) = board("member-b");
    let a_post = publish(&sa, "A body that must not wait for B's file");
    let bytes = pattern(3 * SLICE);
    let b_post = publish_with(&sb, "B body with a large file", &bytes);
    let pair = connect_v5(sa.clone(), fa.clone(), sb.clone(), fb.clone()).await;
    let first = round(&pair).await.unwrap();
    assert!(first.more, "A stopped at slice boundary 0, before any file byte");
    assert!(ranges(&pair.b).is_empty() && !partial(&sa, &bytes).exists());
    assert!(sa.source(&fa, &post_resource(&b_post)).is_ok(), "B's body came first");
    assert!(sb.source(&fb, &post_resource(&a_post)).is_ok(), "B already has A's body");
    // B pulled A's whole catalog. A's refreshed catalog now echoes B's own
    // version back; that does not stop the next round's slices again.
    refresh(&pair.ha, &pair.ea).await;
    let second = round(&pair).await.unwrap();
    assert!(second.settled() && second.failures.is_empty());
    assert_eq!(ranges(&pair.b), vec![(0, SLICE), (SLICE, SLICE), (2 * SLICE, SLICE)]);
    assert!(sa.source(&fa, &attachment_resource(&b_post, &bytes)).is_ok());
}

#[tokio::test]
async fn a_bad_final_hash_fails_only_that_item_and_still_confirms_the_round() {
    let (_ra, sa, fa) = board("member-a");
    let (_rb, sb, fb) = board("member-b");
    let bytes = pattern(2 * SLICE + 77);
    let post = publish_with(&sb, "B body survives a corrupt file", &bytes);
    let pair = connect_v5(sa.clone(), fa.clone(), sb, fb).await;
    pair.a.connection.corrupt_next_slice();
    let outcome = round(&pair).await.unwrap();
    assert!(outcome.settled());
    assert_eq!(outcome.failures.len(), 1);
    assert_eq!(outcome.completed + 1, outcome.total);
    assert!(sa.source(&fa, &post_resource(&post)).is_ok());
    let revision_b = pair.eb.snapshot.lock().unwrap().as_ref().unwrap().revision.clone();
    assert_eq!(pair.ea.checkpoint(&pair.a.remote).unwrap(), Some(revision_b));
    assert!(!partial(&sa, &bytes).exists(), "bytes that fail the offer hash are not progress");
    assert!(!pair.a.is_stopped() && !pair.b.is_stopped(), "the link stays usable");
    let resource = attachment_resource(&post, &bytes);
    let ResourceKind::Attachment { post_id, version_id, attachment_id, .. } = &resource.identity else {
        unreachable!()
    };
    assert!(crate::bbs::sync::attachment_paused(post_id, version_id, attachment_id, &resource.sha256));
    crate::bbs::sync::mark_attachment_paused(&resource, false);
}

#[tokio::test]
async fn a_v4_link_receives_a_large_attachment_as_one_whole_file_get() {
    let (_ra, sa, fa) = board("member-a");
    let (_rb, sb, fb) = board("member-b");
    let bytes = pattern(3 * SLICE);
    let post = publish_with(&sb, "v4 peer", &bytes);
    let pair = connect(sa.clone(), fa.clone(), sb, fb).await;
    let outcome = round(&pair).await.unwrap();
    assert!(outcome.settled() && outcome.failures.is_empty());
    let served = pair.b.served.lock().unwrap().clone();
    assert!(!served.is_empty() && served.iter().all(Option::is_none), "{served:?}");
    assert!(!partial(&sa, &bytes).exists());
    assert!(sa.source(&fa, &attachment_resource(&post, &bytes)).is_ok());
}

#[tokio::test]
async fn a_post_by_the_sender_mid_transfer_stops_the_pull_once_and_its_body_comes_first() {
    let (_ra, sa, fa) = board("member-a");
    let (_rb, sb, fb) = board("member-b");
    let bytes = pattern(5 * SLICE);
    let post = publish_with(&sb, "B large file", &bytes);
    let pair = connect_v5(sa.clone(), fa.clone(), sb.clone(), fb.clone()).await;
    // After A's first slice, hold its second slice after Ready (without
    // blocking A's network thread) while B posts and sends Changed.
    let (release, gate) = tokio::sync::oneshot::channel();
    let (reached, observed) = tokio::sync::oneshot::channel();
    let slots = std::sync::Mutex::new((Some(gate), Some(reached)));
    let connection = pair.a.connection.clone();
    *pair.a.after_slice.lock().unwrap() = Some(Box::new(move |at| {
        if at == SLICE {
            let mut slots = slots.lock().unwrap();
            connection.pause_next_file(slots.0.take().unwrap());
            let _ = slots.1.take().unwrap().send(());
        }
    }));
    let a = pair.a.clone();
    let first = pair.ha.execute(move |_| async move { a.round().await });
    let poster = async {
        observed.await.unwrap();
        let text = publish_text(&sb, "thread-two", "B posts while A downloads");
        let b = pair.b.clone();
        pair.hb.execute(move |_| async move { b.changed().await }).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        release.send(()).unwrap();
        text
    };
    let (first, text) = tokio::join!(first, poster);
    let first = first.unwrap();
    responder_finished(&pair).await;
    assert!(first.more, "the pull stopped at the end of the slice in flight");
    assert_eq!(fs::metadata(partial(&sa, &bytes)).unwrap().len(), 2 * SLICE);
    let before = pair.b.served.lock().unwrap().len();
    // B's coordinator refreshes after its local change; A opens the next round.
    refresh(&pair.hb, &pair.eb).await;
    let second = round(&pair).await.unwrap();
    assert!(second.settled() && second.failures.is_empty(), "exactly one stop");
    let served: Vec<_> = pair.b.served.lock().unwrap()[before..].to_vec();
    assert_eq!(served.first(), Some(&None), "the new body precedes the remaining slices: {served:?}");
    assert!(served[1..].iter().all(Option::is_some));
    assert!(sa.source(&fa, &text_resource("thread-two", &text)).is_ok());
    assert!(sa.source(&fa, &attachment_resource(&post, &bytes)).is_ok());
}

#[tokio::test]
async fn a_post_by_the_puller_mid_transfer_reaches_the_peer_before_its_next_slice() {
    let (_ra, sa, fa) = board("member-a");
    let (_rb, sb, fb) = board("member-b");
    let bytes = pattern(5 * SLICE);
    let post = publish_with(&sb, "B large file", &bytes);
    let pair = connect_v5(sa.clone(), fa.clone(), sb.clone(), fb.clone()).await;
    let link = Arc::downgrade(&pair.a);
    let store = sa.clone();
    let text = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let written = text.clone();
    *pair.a.after_slice.lock().unwrap() = Some(Box::new(move |at| {
        if at == SLICE {
            *written.lock().unwrap() = publish_text(&store, "thread-two", "A posts while it downloads");
            link.upgrade().unwrap().note_local_change();
        }
    }));
    // 1: A's own change ends this round; its Open catalog cannot carry the
    // new post. 2: the next Open does, and A yields before its next slice.
    let first = round(&pair).await.unwrap();
    assert!(first.more);
    assert_eq!(fs::metadata(partial(&sa, &bytes)).unwrap().len(), SLICE);
    refresh(&pair.ha, &pair.ea).await;
    let second = round(&pair).await.unwrap();
    assert!(second.more, "the next Open yields at slice boundary 0");
    assert_eq!(fs::metadata(partial(&sa, &bytes)).unwrap().len(), SLICE);
    assert_eq!(ranges(&pair.b), vec![(0, SLICE)], "no Get in the yielding round");
    let text = text.lock().unwrap().clone();
    assert!(sb.source(&fb, &text_resource("thread-two", &text)).is_ok(), "B has A's new body");
    refresh(&pair.ha, &pair.ea).await;
    let third = round(&pair).await.unwrap();
    assert!(third.settled() && third.failures.is_empty(), "the echo does not stop it again");
    assert!(sa.source(&fa, &attachment_resource(&post, &bytes)).is_ok());
}

#[tokio::test]
async fn an_idle_changed_packet_starts_nothing_and_does_not_stop_the_next_round() {
    let (_ra, sa, fa) = board("member-a");
    let (_rb, sb, fb) = board("member-b");
    let bytes = pattern(3 * SLICE);
    let post = publish_with(&sb, "B large file", &bytes);
    let pair = connect_v5(sa.clone(), fa.clone(), sb, fb).await;
    let calls = || pair.ea.rpc_requests() + pair.eb.rpc_requests();
    let b = pair.b.clone();
    pair.hb.execute(move |_| async move { b.changed().await }).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(calls(), 0, "a Changed packet is not a request and opens no round");
    let outcome = round(&pair).await.unwrap();
    assert!(outcome.settled() && outcome.failures.is_empty(), "a stale mark is dropped at Open");
    assert!(sa.source(&fa, &attachment_resource(&post, &bytes)).is_ok());
}

#[tokio::test]
async fn one_yield_per_local_serial_even_while_the_peer_pull_stays_partial() {
    let (_ra, sa, fa) = board("member-a");
    let (_rb, sb, fb) = board("member-b");
    publish(&sa, "A topic with a small file");
    publish_text(&sa, "thread-two", "A second topic");
    let bytes = pattern(3 * SLICE);
    let post = publish_with(&sb, "B large file", &bytes);
    let pair = connect_v5(sa.clone(), fa.clone(), sb, fb).await;
    // B yields between its jobs, so its half never settles in these rounds.
    pair.eb.yield_requested.store(true, Ordering::Relaxed);
    let first = round(&pair).await.unwrap();
    assert!(first.more);
    assert!(ranges(&pair.b).is_empty(), "the one yield, at slice boundary 0");
    // Same local serial, an echoing catalog, and a peer that is still partial.
    refresh(&pair.ha, &pair.ea).await;
    let second = round(&pair).await.unwrap();
    assert!(second.more, "B's half is still partial");
    assert_eq!(ranges(&pair.b), vec![(0, SLICE), (SLICE, SLICE), (2 * SLICE, SLICE)], "no second yield");
    assert!(sa.source(&fa, &attachment_resource(&post, &bytes)).is_ok());
}
