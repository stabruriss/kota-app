use super::*;
use crate::bbs_sync::transport::{
    NetworkHost, NetworkMode, PeerIdentity, PendingConnection, SessionRole,
};
use crate::bbs_sync::{
    control::{Member, Membership, Role},
    raw_sha256, DeviceIdentity, GroupState,
};
use serde_json::json;
use std::{fs, path::PathBuf};

#[test]
fn finish_response_preserves_transport_errors_and_version_check_is_specific() {
    for error in [
        Error::Timeout, Error::Closed, Error::Cancelled, Error::Io,
        Error::Unauthorized, Error::Busy, Error::Protocol,
    ] {
        assert_eq!(expect_ok(Err(error)), Err(error));
    }
    assert_eq!(expect_ok(Ok(Answer::Ok)), Ok(()));
    assert_eq!(expect_ok(Ok(Answer::Busy)), Err(Error::Protocol));
    assert_eq!(check_protocol_version(EXCHANGE_PROTOCOL_VERSION), Ok(()));
    for version in [EXCHANGE_PROTOCOL_VERSION - 1, EXCHANGE_PROTOCOL_VERSION + 1] {
        assert_eq!(check_protocol_version(version), Err(Error::ProtocolVersion));
    }
    assert_eq!(Error::ProtocolVersion.to_string(), "protocol_mismatch");
    assert_eq!(Error::Protocol.to_string(), "sync_protocol_error");
}

#[test]
fn stage_diagnostics_do_not_retain_packet_material() {
    let request = Request::Finish { round: "not-for-logging".into(), result: Outcome::default() };
    let trace = Trace { outgoing: Some(request.stage()), outgoing_id: 3, remote_protocol: Some(3), ..Trace::default() };
    let value = serde_json::to_string(&trace).unwrap();
    assert_eq!(value, "{\"outgoing\":\"finish\",\"outgoing_id\":3,\"incoming\":null,\"incoming_id\":0,\"remote_protocol\":3}");
    assert!(!value.contains("not-for-logging"));
}
pub(crate) struct Root(PathBuf);
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
pub(crate) fn board(membership: &str) -> (Root, ContentStore, GroupFence) {
    let root = Root(std::env::temp_dir().join(format!("bbs-exchange-{}", uuid::Uuid::new_v4())));
    let store = ContentStore::at(root.0.join("bbs"), root.0.join("account"));
    store.prepare_layout().unwrap();
    let fence = GroupFence {
        group_id: "group-test".into(),
        membership_id: membership.into(),
    };
    let control = json!({"schemaVersion":1,"deviceName":"test","current":{"groupId":fence.group_id,"membershipId":fence.membership_id,"workerUrl":"https://example.test","role":"member"},"pending":null,"invitation":null});
    fs::create_dir_all(store.state.control_path().parent().unwrap()).unwrap();
    fs::write(
        store.state.control_path(),
        serde_json::to_vec(&control).unwrap(),
    )
    .unwrap();
    (root, store, fence)
}
pub(crate) fn publish(store: &ContentStore, body: &str) -> Vec<u8> {
    let root = store.root().join("threads/thread-one");
    fs::create_dir_all(root.join("posts")).unwrap();
    let record = json!({"schema":"kota.bbs.thread.v1","threadId":"thread-one","status":"open","visibility":"targeted","projectTags":["source"],"createdByProject":"source","createdByAgent":"author","createdAt":"2026-09-12T10:00:00Z","updatedAt":"2026-09-12T10:00:00Z","latestPostId":"root"});
    fs::write(
        root.join("thread.yaml"),
        serde_yaml::to_string(&record).unwrap(),
    )
    .unwrap();
    let attachment = b"immutable attachment bytes";
    fs::create_dir_all(root.join("attachments/root")).unwrap();
    fs::write(root.join("attachments/root/file.txt"), attachment).unwrap();
    let meta = json!({"schema":"kota.bbs.post.v1","threadId":"thread-one","postId":"root","projectId":"source","agentId":"author","agentDisplayName":"Author","projectDisplayName":"Source","createdAt":"2026-09-12T10:00:00Z","kind":"topic","attachments":[{"id":"file","name":"file.txt","path":"attachments/root/file.txt","sha256":raw_sha256(attachment),"sizeBytes":attachment.len()}]});
    let bytes = format!(
        "---\n# original bytes\n{}---\n\n{body}\n",
        serde_yaml::to_string(&meta).unwrap()
    )
    .into_bytes();
    fs::write(root.join("posts/root.md"), &bytes).unwrap();
    let mut state = store.state.load_state().unwrap().unwrap_or_default();
    state
        .groups
        .entry("group-test".into())
        .or_insert_with(GroupState::default)
        .shared_threads
        .insert("thread-one".into());
    store.state.save_state(&state).unwrap();
    bytes
}
struct Connected {
    a: Arc<Link>,
    b: Arc<Link>,
    ha: NetworkHost,
    hb: NetworkHost,
    ea: Arc<Engine>,
    eb: Arc<Engine>,
}
impl Drop for Connected {
    fn drop(&mut self) {
        self.a.cancel();
        self.b.cancel();
        self.ha.stop();
        self.hb.stop();
    }
}
async fn connect(sa: ContentStore, fa: GroupFence, sb: ContentStore, fb: GroupFence) -> Connected {
    connect_rosters(sa, fa, sb, fb, vec![], vec![]).await
}
async fn connect_rosters(
    sa: ContentStore,
    fa: GroupFence,
    sb: ContentStore,
    fb: GroupFence,
    ra: Vec<roster::Project>,
    rb: Vec<roster::Project>,
) -> Connected {
    connect_sources(sa, fa, sb, fb, Some(ra), Some(rb)).await
}
async fn connect_sources(
    sa: ContentStore, fa: GroupFence, sb: ContentStore, fb: GroupFence,
    ra: Option<Vec<roster::Project>>, rb: Option<Vec<roster::Project>>,
) -> Connected {
    connect_wiring(sa, fa, sb, fb, ra, rb, true).await
}
async fn connect_wiring(
    sa: ContentStore, fa: GroupFence, sb: ContentStore, fb: GroupFence,
    ra: Option<Vec<roster::Project>>, rb: Option<Vec<roster::Project>>,
    shared_files: bool,
) -> Connected {
    let mut keys = [
        DeviceIdentity::from_seed([61; 32]).unwrap(),
        DeviceIdentity::from_seed([62; 32]).unwrap(),
    ];
    keys.sort_by_key(|k| k.device_id().unwrap());
    let [ka, kb] = keys;
    let a_id = ka.device_id().unwrap();
    let b_id = kb.device_id().unwrap();
    let members = vec![
        Member {
            device_id: ka.device_id().unwrap(),
            public_key: ka.public_key.clone(),
            name: "a".into(),
            role: Role::Owner,
            membership_id: fa.membership_id.clone(),
            online: true,
            last_seen_at: 0,
        },
        Member {
            device_id: kb.device_id().unwrap(),
            public_key: kb.public_key.clone(),
            name: "b".into(),
            role: Role::Member,
            membership_id: fb.membership_id.clone(),
            online: true,
            last_seen_at: 0,
        },
    ];
    let ma = Membership {
        group_id: fa.group_id.clone(),
        worker_url: "https://example.test".into(),
        role: Role::Owner,
        membership_id: fa.membership_id.clone(),
    };
    let mut mb = ma.clone();
    mb.role = Role::Member;
    mb.membership_id = fb.membership_id.clone();
    let ab = PeerIdentity::current(&ma, &members, &ka, &kb.device_id().unwrap()).unwrap();
    let ba = PeerIdentity::current(&mb, &members, &kb, &ka.device_id().unwrap()).unwrap();
    let ab_catalog = ab.clone();
    let ba_catalog = ba.clone();
    // Content-only scenarios still speak the real v3 Roster RPC, with a
    // complete empty local roster and the actual authenticated device IDs.
    sa.state.save_identity(&ka).unwrap();
    sb.state.save_identity(&kb).unwrap();
    for store in [&sa, &sb] {
        let current = crate::bbs_sync::control::read_membership(&store.state).unwrap();
        roster::persist_members(&store.state, current.as_ref(), &members).unwrap();
    }
    let ra = ra.map(|p| roster::runtime::Runtime::test_source(sa.clone(), p))
        .unwrap_or_else(|| roster::runtime::Runtime::new(sa.clone()));
    let rb = rb.map(|p| roster::runtime::Runtime::test_source(sb.clone(), p))
        .unwrap_or_else(|| roster::runtime::Runtime::new(sb.clone()));
    // Use the production wiring: roster publication and exchange share one
    // account file worker/permit. The old helper silently used two workers.
    let host = |runtime: Arc<roster::runtime::Runtime>| async move {
        if shared_files {
            let (files, limits) = runtime.files().unwrap();
            NetworkHost::start_with_files(files, limits).await.unwrap()
        } else {
            NetworkHost::start().await.unwrap()
        }
    };
    let ha = host(ra.clone()).await;
    let hb = host(rb.clone()).await;
    let ea = ha
        .execute(move |ctx| async move {
            let mut e = Engine::new(
                sa,
                fa,
                ctx,
                Arc::new(|_, event| {
                    if let Event::Failed(_, error) = event {
                        eprintln!("exchange_a_error={error}")
                    }
                }),
            );
            Arc::get_mut(&mut e).unwrap().roster = Some(ra);
            e.catalog_members([ab_catalog]);
            e.refresh(&Cancellation::default()).await?;
            Ok(e)
        })
        .await
        .unwrap();
    let eb = hb
        .execute(move |ctx| async move {
            let mut e = Engine::new(
                sb,
                fb,
                ctx,
                Arc::new(|_, event| {
                    if let Event::Failed(_, error) = event {
                        eprintln!("exchange_b_error={error}")
                    }
                }),
            );
            Arc::get_mut(&mut e).unwrap().roster = Some(rb);
            e.catalog_members([ba_catalog]);
            e.refresh(&Cancellation::default()).await?;
            Ok(e)
        })
        .await
        .unwrap();
    let (mut a, offer) = ha
        .execute(move |ctx| async move {
            let mut p = PendingConnection::new(
                ctx,
                ab,
                SessionRole::Offer,
                "a".repeat(32),
                Arc::new(|| true),
                NetworkMode::Loopback,
            )
            .await?;
            let s = p.local_description(&ka).await?;
            Ok((p, s))
        })
        .await
        .unwrap();
    let (b, answer) = hb
        .execute(move |ctx| async move {
            let mut p = PendingConnection::new(
                ctx,
                ba,
                SessionRole::Answer,
                "a".repeat(32),
                Arc::new(|| true),
                NetworkMode::Loopback,
            )
            .await?;
            p.remote_description(&offer).await?;
            let s = p.local_description(&kb).await?;
            Ok((p, s))
        })
        .await
        .unwrap();
    let ae = ea.clone();
    let be = eb.clone();
    let (a, b) = tokio::join!(
        ha.execute(move |_| async move {
            a.remote_description(&answer).await?;
            Ok(Link::start(a.connect().await?, b_id, true, ae))
        }),
        hb.execute(move |_| async move { Ok(Link::start(b.connect().await?, a_id, false, be)) })
    );
    Connected {
        a: a.unwrap(),
        b: b.unwrap(),
        ha,
        hb,
        ea,
        eb,
    }
}

#[tokio::test]
async fn real_roster_pages_and_unchanged_rounds_continue_missing_avatars_with_the_same_job_budget()
{
    let (_ra, sa, fa) = board("member-a");
    let (_rb, sb, fb) = board("member-b");
    let mut agents = Vec::new();
    let mut pictures = Vec::new();
    fs::create_dir_all(sa.root().join("avatars")).unwrap();
    for n in 0..75 {
        let avatar = if n < 35 {
            let bytes = format!("roster-only-image-{n}").into_bytes();
            let sha = raw_sha256(&bytes);
            fs::write(sa.root().join("avatars").join(format!("{sha}.png")), &bytes).unwrap();
            pictures.push((sha.clone(), bytes.clone()));
            roster::Avatar::Image {
                sha256: sha,
                ext: "png".into(),
                size_bytes: bytes.len() as u64,
            }
        } else {
            // Repeated hash is shared by several agents, never another job.
            roster::Avatar::Image {
                sha256: pictures[0].0.clone(),
                ext: "png".into(),
                size_bytes: pictures[0].1.len() as u64,
            }
        };
        agents.push(roster::Agent {
            agent_id: format!("a-{n:03}"),
            name: format!("Agent {n}"),
            avatar,
        });
    }
    let source = vec![roster::Project {
        project_id: "project".into(),
        name: "Source project".into(),
        agents,
    }];
    let pair = connect_rosters(sa.clone(), fa.clone(), sb.clone(), fb, source, vec![]).await;
    let link = pair.a.clone();
    let first = pair
        .ha
        .execute(move |_| async move { link.round().await })
        .await
        .unwrap();
    assert!(first.failures.is_empty());
    assert_eq!(first.completed, MAX_JOBS as u64);
    assert!(first.more);
    let peer = pair.b.remote.clone();
    let cache = sb.state.root.join(format!("rosters/{peer}.json"));
    let bytes = fs::read(&cache).unwrap();
    let cached = roster::read_peer(&sb.state, &roster::context(&sb.state).unwrap(), &peer)
        .unwrap()
        .unwrap();
    assert_eq!(cached.projects[0].agents.len(), 75);
    assert_eq!(
        fs::read_dir(sb.root().join("avatars")).unwrap().count(),
        MAX_JOBS
    );
    // Same roster version must still fetch missing pictures in the next round.
    let link = pair.a.clone();
    let second = pair
        .ha
        .execute(move |_| async move { link.round().await })
        .await
        .unwrap();
    assert!(second.failures.is_empty());
    assert!(!second.more);
    assert_eq!(second.completed, 3);
    assert_eq!(
        fs::read(&cache).unwrap(),
        bytes,
        "RosterUnchanged must not rewrite the cache"
    );
    for (sha, bytes) in &pictures {
        assert_eq!(
            fs::read(sb.root().join("avatars").join(format!("{sha}.png"))).unwrap(),
            *bytes
        );
    }
    let link = pair.a.clone();
    let third = pair
        .ha
        .execute(move |_| async move { link.round().await })
        .await
        .unwrap();
    assert_eq!(third.total, 0);
    assert!(third.failures.is_empty());
    // b only offers its own empty roster, despite caching a's complete list.
    let b = pair.a.remote.clone();
    assert!(
        roster::read_peer(&sa.state, &roster::context(&sa.state).unwrap(), &b)
            .unwrap()
            .unwrap()
            .projects
            .is_empty()
    );
    let first_resource = cached.projects[0].agents[0].avatar.resource().unwrap();
    assert!(sa
        .source_for_exchange(&fa, &first_resource, &Catalog::default(), None)
        .is_err());
    assert!(
        sb.read_verified_avatar(&pictures[0].0, "png").is_err(),
        "roster download must not grant old post-reader authority"
    );
    pair.ha.stop_and_wait().await;
    pair.hb.stop_and_wait().await;
}
#[tokio::test]
async fn unavailable_agent_directory_does_not_turn_successful_content_into_partial() {
    let (_ra, sa, fa) = board("member-a");
    let (_rb, sb, fb) = board("member-b");
    let raw = publish(&sa, "content remains available while the local directory is broken");
    let pair = connect_sources(sa, fa, sb.clone(), fb, None, Some(vec![])).await;
    for _ in 0..2 {
        let link = pair.a.clone();
        let outcome = pair.ha.execute(move |_| async move { link.round().await }).await.unwrap();
        assert!(outcome.failures.is_empty());
        assert_eq!(outcome.omitted, 0); assert!(!outcome.more);
        assert_eq!(outcome.completed, outcome.total);
    }
    assert_eq!(fs::read(sb.root().join("threads/thread-one/posts/root.md")).unwrap(), raw);
    assert!(pair.b.roster_unavailable.load(Ordering::Acquire));
    pair.ha.stop_and_wait().await; pair.hb.stop_and_wait().await;
}

#[tokio::test]
async fn actual_two_direction_round_preserves_raw_forks_attachments_and_reuses_connection() {
    let (_ra, sa, fa) = board("member-a");
    let (_rb, sb, fb) = board("member-b");
    let raw_a = publish(&sa, "A original");
    let raw_b = publish(&sb, "B original fork");
    let pair = connect(sa.clone(), fa.clone(), sb.clone(), fb.clone()).await;
    let a = pair.a.clone();
    let result = pair
        .ha
        .execute(move |_| async move { a.round().await })
        .await
        .unwrap();
    assert!(
        result.failures.is_empty(),
        "{}",
        serde_json::to_string(&result).unwrap()
    );
    assert_eq!(result.completed, result.total);
    assert!(result.total >= 2);
    for (store, fence) in [(&sa, &fa), (&sb, &fb)] {
        for raw in [&raw_a, &raw_b] {
            let resource = Resource {
                identity: ResourceKind::Post {
                    thread_id: "thread-one".into(),
                    post_id: "root".into(),
                    version_id: raw_sha256(raw),
                },
                sha256: raw_sha256(raw),
                size_bytes: raw.len() as u64,
            };
            assert_eq!(
                fs::read(store.source(fence, &resource).unwrap()).unwrap(),
                *raw
            );
        }
    }
    let a = pair.a.clone();
    let result = pair
        .ha
        .execute(move |_| async move { a.round().await })
        .await
        .unwrap();
    assert_eq!(result.completed, 0);
    assert_eq!(result.total, 0);
    assert!(result.failures.is_empty());
    assert!(!pair.a.is_stopped());
    assert!(!pair.b.is_stopped());
}
#[tokio::test]
async fn roster_file_work_during_admitted_round_does_not_desynchronize_the_peers() {
    for shared_files in [false, true] {
        let (_ra, sa, fa) = board("member-a");
        let (_rb, sb, fb) = board("member-b");
        let pair = connect_wiring(sa, fa, sb, fb, Some(vec![]), Some(vec![]), shared_files).await;
        let (io, _) = pair.ea.roster.as_ref().unwrap().files().unwrap();
        let (entered, started) = oneshot::channel();
        let (release, held) = std::sync::mpsc::channel::<()>();
        let file_work = tokio::spawn(async move {
            io.run_when_available(&Cancellation::default(), move |_| {
                let _ = entered.send(());
                // No external file or account data; emulate the roster's use of
                // its real account file service. A failed test cannot hang it.
                let _ = held.recv_timeout(Duration::from_secs(5));
            }).await
        });
        started.await.unwrap();
        let host = pair.ha.clone();
        let a = pair.a.clone();
        let round = tokio::spawn(async move { host.execute(move |_| async move { a.round().await }).await });
        tokio::time::sleep(Duration::from_millis(300)).await;
        let premature = round.is_finished();
        drop(release);
        file_work.await.unwrap().unwrap();
        let result = tokio::time::timeout(Duration::from_secs(8), round).await.unwrap().unwrap();
        eprintln!("shared_files={shared_files} finished_while_file_held={premature} result={:?} peer_active={}",
            result.as_ref().map(|o| (o.completed, o.total)), pair.b.active());
        if result.as_ref().is_err_and(|e| *e == Error::Busy) {
            let a = pair.a.clone();
            let second = pair.ha.execute(move |_| async move { a.round().await }).await;
            eprintln!("abandoned_round_retry={:?}", second.as_ref().map(|o| (o.completed, o.total)));
        }
        assert!(result.is_ok(), "temporary roster work must not abandon an accepted round");
        assert!(!pair.a.is_stopped());
        assert!(!pair.b.is_stopped());
        // The same connection must complete another full round, not provoke a
        // second Open while the responder still has the previous round active.
        let a = pair.a.clone();
        assert!(pair.ha.execute(move |_| async move { a.round().await }).await.is_ok());
    }
}

async fn assert_link_retired(link: &Link) {
    // A remote close may fall back to the existing idle-progress deadline.
    // Poll state, not a fixed sleep; the extra time is scheduling tolerance.
    let retired = || {
        link.is_stopped() && !link.active() && link.pending.lock().unwrap().is_none()
    };
    let settled = tokio::time::timeout(transport::PROGRESS_TIMEOUT + Duration::from_secs(5), async {
        while !retired() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await;
    assert!(settled.is_ok(), "link did not retire: stopped={} active={} pending={} round_permits={}",
        link.is_stopped(), link.active(), link.pending.lock().unwrap().is_some(),
        link.engine.rounds.available_permits());
}

#[tokio::test]
async fn resource_refusal_after_open_retires_the_link_instead_of_reusing_a_half_round() {
    let (_ra, sa, fa) = board("member-a");
    let (_rb, sb, fb) = board("member-b");
    let pair = connect(sa, fa, sb, fb).await;
    let _busy = pair.ea.context.limits.reserve(transport::APP_QUEUE_BYTES - 128 * 1024).unwrap();
    let a = pair.a.clone();
    let result = pair.ha.execute(move |_| async move { a.round().await }).await;
    assert_eq!(result.err(), Some(Error::Busy));
    // The initiating side must retire before returning, not after a timeout.
    assert!(pair.a.is_stopped());
    assert!(!pair.a.active());
    assert!(pair.ea.rounds.clone().try_acquire_owned().is_ok());
    assert_link_retired(&pair.b).await;
    assert!(!pair.b.active());
    assert!(pair.eb.rounds.clone().try_acquire_owned().is_ok());
}

#[tokio::test]
async fn transport_stop_retires_active_round_without_coordinator() {
    let (_ra, sa, fa) = board("member-a");
    let (_rb, sb, fb) = board("member-b");
    let pair = connect(sa, fa, sb, fb).await;
    let a = pair.a.clone();
    pair.ha.execute(move |_| async move {
        let response = a.rpc(Request::Open {
            protocol_version: EXCHANGE_PROTOCOL_VERSION,
            round: uuid::Uuid::new_v4().to_string(),
            revision: "0".repeat(64),
            checkpoint: None,
        }).await?;
        assert!(matches!(response.answer, Answer::Open { .. }));
        Ok(())
    }).await.unwrap();
    let b = pair.b.clone();
    pair.hb.execute(move |_| async move {
        // Drain response credits, then stop only the transport. Neither this
        // test nor the bare-Link harness invokes coordinator/Link cleanup.
        b.send(Packet::Working { counter: 1 }).await?;
        assert!(b.active());
        assert_eq!(b.engine.rounds.available_permits(), 0);
        b.connection.cancel();
        Ok(())
    }).await.unwrap();
    assert_link_retired(&pair.b).await;
    assert!(pair.eb.rounds.clone().try_acquire_owned().is_ok());
    // A later coordinator removal may retire the same link again safely.
    pair.b.cancel();
    assert!(!pair.b.active());
    assert_eq!(pair.eb.rounds.available_permits(), 1);
}

#[tokio::test]
async fn occupied_account_round_is_busy_without_changing_membership_or_losing_connection() {
    let (_ra, sa, fa) = board("member-a");
    let (_rb, sb, fb) = board("member-b");
    let pair = connect(sa, fa, sb, fb).await;
    let permit = pair.eb.rounds.clone().try_acquire_owned().unwrap();
    let a = pair.a.clone();
    assert_eq!(
        pair.ha
            .execute(move |_| async move { a.round().await })
            .await
            .err(),
        Some(Error::Busy)
    );
    assert!(!pair.a.is_stopped());
    assert!(!pair.b.is_stopped());
    assert_eq!(pair.ea.checkpoint(&pair.a.remote).unwrap(), None);
    assert_eq!(pair.eb.checkpoint(&pair.b.remote).unwrap(), None);
    drop(permit);
    let a = pair.a.clone();
    assert!(pair
        .ha
        .execute(move |_| async move { a.round().await })
        .await
        .is_ok());
}

#[tokio::test]
async fn cancelled_written_partial_is_removed_and_next_round_restarts_the_whole_file() {
    let (_ra, sa, fa) = board("member-a");
    let (_rb, sb, fb) = board("member-b");
    let raw = publish(&sb, &"transfer bytes\n".repeat(150_000));
    let resource = Resource {
        identity: ResourceKind::Post {
            thread_id: "thread-one".into(),
            post_id: "root".into(),
            version_id: raw_sha256(&raw),
        },
        sha256: raw_sha256(&raw),
        size_bytes: raw.len() as u64,
    };
    let partials = || {
        fs::read_dir(sa.root().join(".sync-staging"))
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(".receive-"))
            .map(|entry| entry.path())
            .collect::<Vec<_>>()
    };
    let mut first_written = None;
    for cancel_round in [true, false] {
        let pair = connect(sa.clone(), fa.clone(), sb.clone(), fb.clone()).await;
        let (started, written) = oneshot::channel();
        let (release, gate) = oneshot::channel();
        let connection = pair.a.connection.clone();
        pair.ha
            .execute(move |_| async move {
                connection.pause_after_first_file_write(started, gate).await;
                Ok(())
            })
            .await
            .unwrap();
        let a = pair.a.clone();
        let host = pair.ha.clone();
        let round =
            tokio::spawn(
                async move { host.execute(move |_| async move { a.round().await }).await },
            );
        let written = tokio::time::timeout(Duration::from_secs(10), written)
            .await
            .expect("file did not start")
            .unwrap();
        assert!(written > 0 && written < resource.size_bytes);
        let paths = partials();
        assert_eq!(paths.len(), 1);
        assert_eq!(fs::metadata(&paths[0]).unwrap().len(), written);
        assert!(
            sa.source(&fa, &resource).is_err(),
            "unverified file was installed"
        );
        if cancel_round {
            first_written = Some(written);
            pair.a.cancel();
            assert!(tokio::time::timeout(Duration::from_secs(2), round)
                .await
                .expect("cancel waited for transfer")
                .unwrap()
                .is_err());
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while !partials().is_empty() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "partial survived cancellation"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert!(sa.source(&fa, &resource).is_err());
        } else {
            // The new transfer starts at zero, rather than reusing the cancelled
            // output or acknowledging an offset from its previous incarnation.
            assert_eq!(Some(written), first_written);
            release.send(()).unwrap();
            let outcome = tokio::time::timeout(Duration::from_secs(15), round)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert!(outcome.failures.is_empty());
            assert_eq!(outcome.completed, 2); // full post and its attachment
            assert_eq!(fs::read(sa.source(&fa, &resource).unwrap()).unwrap(), raw);
            assert!(partials().is_empty());
        }
        pair.ha.stop_and_wait().await;
        pair.hb.stop_and_wait().await;
    }
}

#[tokio::test]
async fn silent_open_round_times_out_and_releases_account_permit() {
    let (_ra, sa, fa) = board("member-a");
    let (_rb, sb, fb) = board("member-b");
    let pair = connect(sa, fa, sb, fb).await;
    let a = pair.a.clone();
    pair.ha
        .execute(move |_| async move {
            let response = a
                .rpc(Request::Open {
                    protocol_version: EXCHANGE_PROTOCOL_VERSION,
                    round: uuid::Uuid::new_v4().to_string(),
                    revision: "0".repeat(64),
                    checkpoint: None,
                })
                .await?;
            assert!(matches!(response.answer, Answer::Open { .. }));
            Ok(())
        })
        .await
        .unwrap();
    let end = std::time::Instant::now() + Duration::from_secs(27);
    while !pair.b.is_stopped() {
        assert!(
            std::time::Instant::now() < end,
            "silent peer kept account permit forever"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(pair.eb.rounds.clone().try_acquire_owned().is_ok());
}

#[tokio::test]
async fn real_exchange_keeps_unavailable_out_of_failures_and_recovers_when_source_shrinks() {
    let (_ra, sa, fa) = board("member-a");
    let (_rb, sb, fb) = board("member-b");
    publish(&sa, "large source remains local");
    let root_path = sa.root().join("threads/thread-one/posts/root.md");
    fs::OpenOptions::new()
        .write(true)
        .open(&root_path)
        .unwrap()
        .set_len(reconcile::MAX_BODY_BYTES + 1)
        .unwrap();
    let reply = json!({"schema":"kota.bbs.post.v1","threadId":"thread-one","postId":"reply","projectId":"source","agentId":"author","agentDisplayName":"Author","projectDisplayName":"Source","createdAt":"2026-09-12T11:00:00Z","kind":"reply","attachments":[]});
    let reply = format!(
        "---\n{}---\n\nA real reply without a downloaded root\n",
        serde_yaml::to_string(&reply).unwrap()
    );
    fs::write(sa.root().join("threads/thread-one/posts/reply.md"), &reply).unwrap();
    let pair = connect(sa.clone(), fa, sb.clone(), fb.clone()).await;
    let marker = sb.root().join("threads/thread-one/placeholders/root.json");
    for round in 0..2 {
        let a = pair.a.clone();
        let outcome = pair
            .ha
            .execute(move |_| async move { a.round().await })
            .await
            .unwrap();
        assert!(
            outcome.failures.is_empty(),
            "{}",
            serde_json::to_string(&outcome).unwrap()
        );
        assert_eq!(outcome.omitted, 0);
        assert!(!outcome.more);
        assert_eq!(outcome.completed, outcome.total);
        assert_eq!(outcome.total, if round == 0 { 1 } else { 0 });
        assert!(marker.exists());
        assert!(!sb.root().join("threads/thread-one/posts/root.md").exists());
        assert_eq!(
            fs::read_to_string(sb.root().join("threads/thread-one/posts/reply.md")).unwrap(),
            reply
        );
        assert!(sb
            .catalog(&fb)
            .unwrap()
            .page(ManifestPhase::Unavailable, None)
            .unwrap()
            .0
            .items
            .is_empty());
    }
    assert!(pair.ea.checkpoint(&pair.a.remote).unwrap().is_some());
    assert!(pair.eb.checkpoint(&pair.b.remote).unwrap().is_some());
    let small = publish(&sa, "now a normal root");
    let ea = pair.ea.clone();
    pair.ha
        .execute(move |_| async move { ea.refresh(&Cancellation::default()).await })
        .await
        .unwrap();
    let a = pair.a.clone();
    let outcome = pair
        .ha
        .execute(move |_| async move { a.round().await })
        .await
        .unwrap();
    assert!(
        outcome.failures.is_empty(),
        "{}",
        serde_json::to_string(&outcome).unwrap()
    );
    assert_eq!(outcome.completed, 2); // actual body plus its registered attachment
    assert_eq!(outcome.total, 2);
    assert!(!marker.exists());
    assert_eq!(
        fs::read(sb.root().join("threads/thread-one/posts/root.md")).unwrap(),
        small
    );
    assert_eq!(
        sb.state
            .load_state()
            .unwrap()
            .unwrap()
            .ledger
            .values()
            .filter(|r| r.post_id == "root")
            .count(),
        1
    );
}

#[tokio::test]
async fn previous_exchange_version_fails_before_a_round_or_any_content_install() {
    assert!(serde_json::from_value::<Request>(
        json!({"type":"open","round":uuid::Uuid::new_v4().to_string()})
    )
    .is_err());
    let (_ra, sa, fa) = board("member-a");
    let (_rb, sb, fb) = board("member-b");
    publish(&sa, "must not reach the old peer");
    let pair = connect(sa, fa, sb.clone(), fb).await;
    let a = pair.a.clone();
    let result = pair
        .ha
        .execute(move |_| async move {
            a.rpc(Request::Open {
                protocol_version: 2,
                round: uuid::Uuid::new_v4().to_string(),
                revision: "0".repeat(64),
                checkpoint: None,
            })
            .await
            .map(|_| ())
        })
        .await;
    assert!(result.is_err());
    assert!(pair.b.is_stopped());
    assert!(pair.eb.rounds.clone().try_acquire_owned().is_ok());
    assert!(!sb.root().join("threads/thread-one").exists());
}

async fn responder_finished(pair: &Connected) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while pair.b.active() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("responder did not finish the accepted round");
}

#[tokio::test]
async fn incomplete_initiator_pull_keeps_only_the_completed_direction_checkpoint() {
    let (_ra, sa, fa) = board("member-a");
    let (_rb, sb, fb) = board("member-b");
    publish(&sb, "the initiator yields between the post and its attachment");
    let pair = connect(sa, fa, sb, fb).await;
    let revision_a = pair.ea.snapshot.lock().unwrap().as_ref().unwrap().revision.clone();
    pair.ea.yield_requested.store(true, Ordering::Relaxed);
    let a = pair.a.clone();
    let outcome = pair.ha.execute(move |_| async move { a.round().await }).await.unwrap();
    assert!(outcome.more);
    assert!(outcome.completed < outcome.total);
    responder_finished(&pair).await;
    assert_eq!(pair.ea.checkpoint(&pair.a.remote).unwrap(), None);
    // B did absorb A's fixed snapshot. This directional hint cannot prevent
    // A (the only initiator) from finishing its own outstanding pull next time.
    assert_eq!(pair.eb.checkpoint(&pair.b.remote).unwrap(), Some(revision_a));
}

#[tokio::test]
async fn incomplete_responder_pull_does_not_confirm_either_checkpoint() {
    let (_ra, sa, fa) = board("member-a");
    let (_rb, sb, fb) = board("member-b");
    publish(&sa, "the responder yields between the post and its attachment");
    let pair = connect(sa, fa, sb, fb).await;
    pair.eb.yield_requested.store(true, Ordering::Relaxed);
    let a = pair.a.clone();
    let outcome = pair.ha.execute(move |_| async move { a.round().await }).await.unwrap();
    assert!(outcome.more);
    assert!(outcome.completed < outcome.total);
    responder_finished(&pair).await;
    assert_eq!(pair.ea.checkpoint(&pair.a.remote).unwrap(), None);
    assert_eq!(pair.eb.checkpoint(&pair.b.remote).unwrap(), None);
}

#[tokio::test]
async fn checkpoints_name_the_pinned_catalog_and_roster_and_next_open_carries_them() {
    let (_ra, sa, fa) = board("member-a");
    let (_rb, sb, fb) = board("member-b");
    publish(&sa, "A original version");
    publish(&sb, "B independent version");
    let projects = |name: &str| vec![roster::Project {
        project_id: "project".into(), name: name.into(), agents: vec![],
    }];
    let pair = connect_rosters(sa, fa, sb.clone(), fb, projects("Pinned"), vec![]).await;
    let snapshot_a = pair.ea.snapshot.lock().unwrap().clone().unwrap();
    let snapshot_b = pair.eb.snapshot.lock().unwrap().clone().unwrap();
    assert_ne!(snapshot_a.revision, snapshot_b.revision);
    let roster = pair.ea.roster.as_ref().unwrap();
    // Publication between refresh and Open must not silently change this round.
    roster.test_publish_source(projects("Published before Open"));
    let (started, written) = oneshot::channel();
    let (release, gate) = oneshot::channel();
    let connection = pair.b.connection.clone();
    pair.hb.execute(move |_| async move {
        connection.pause_after_first_file_write(started, gate).await;
        Ok(())
    }).await.unwrap();
    let a = pair.a.clone();
    let host = pair.ha.clone();
    let round = tokio::spawn(async move {
        host.execute(move |_| async move { a.round().await }).await
    });
    tokio::time::timeout(Duration::from_secs(10), written).await.unwrap().unwrap();
    // B is pulling A after Turn, so A already installed B's different post.
    // Its advertised catalog must nevertheless still be the original snapshot.
    roster.test_publish_source(projects("Published after Open"));
    {
        let session = pair.a.session.lock().unwrap();
        assert_eq!(session.stage, 2);
        let catalog = session.catalog.as_ref().unwrap();
        let local = session.roster.as_ref().unwrap();
        assert!(Arc::ptr_eq(catalog, &snapshot_a.catalog));
        assert!(Arc::ptr_eq(local, snapshot_a.roster.as_ref().unwrap()));
        assert_eq!(catalog.revision_with_roster(Some(&local.version)), snapshot_a.revision);
    }
    assert_eq!(pair.b.trace.lock().unwrap().last_open,
        Some((snapshot_a.revision.clone(), None)));
    assert_eq!(pair.a.trace.lock().unwrap().last_open,
        Some((snapshot_b.revision.clone(), None)));
    let cached = roster::read_peer(&sb.state, &roster::context(&sb.state).unwrap(), &pair.b.remote)
        .unwrap().unwrap();
    assert_eq!(cached.projects[0].name, "Pinned");
    release.send(()).unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(10), round).await.unwrap().unwrap().unwrap();
    assert!(outcome.failures.is_empty());
    assert_eq!(outcome.completed, outcome.total);
    assert!(!outcome.more);
    responder_finished(&pair).await;
    assert_eq!(pair.ea.checkpoint(&pair.a.remote).unwrap(), Some(snapshot_b.revision.clone()));
    assert_eq!(pair.eb.checkpoint(&pair.b.remote).unwrap(), Some(snapshot_a.revision.clone()));
    let first_counts = [pair.a.trace.lock().unwrap().manifest_items,
        pair.b.trace.lock().unwrap().manifest_items];

    // Only an explicit round-external refresh publishes the newly installed
    // versions and the latest roster. No partial admission is taken after Open.
    for engine in [&pair.ea, &pair.eb] {
        engine.refresh(&Cancellation::default()).await.unwrap();
    }
    let next_a = pair.ea.snapshot.lock().unwrap().as_ref().unwrap().revision.clone();
    let next_b = pair.eb.snapshot.lock().unwrap().as_ref().unwrap().revision.clone();
    assert_ne!(next_a, snapshot_a.revision);
    assert_ne!(next_b, snapshot_b.revision);
    let a = pair.a.clone();
    let outcome = pair.ha.execute(move |_| async move { a.round().await }).await.unwrap();
    responder_finished(&pair).await;
    assert!(outcome.failures.is_empty());
    assert_eq!((outcome.completed, outcome.total, outcome.more), (0, 0, false));
    assert_eq!(pair.b.trace.lock().unwrap().last_open,
        Some((next_a.clone(), Some(snapshot_b.revision))));
    assert_eq!(pair.a.trace.lock().unwrap().last_open,
        Some((next_b.clone(), Some(snapshot_a.revision))));
    assert_eq!(pair.ea.checkpoint(&pair.a.remote).unwrap(), Some(next_b));
    assert_eq!(pair.eb.checkpoint(&pair.b.remote).unwrap(), Some(next_a));
    // Only the newly learned opposite Fork is offered back, not both old
    // versions. It is already present there, hence zero duplicate resources.
    for (link, first) in [(&pair.a,first_counts[0]),(&pair.b,first_counts[1])] {
        let counts=link.trace.lock().unwrap().manifest_items;
        assert_eq!(counts[2]-first[2],1);
        assert_eq!(counts[1]-first[1],1);
    }
    let cached = roster::read_peer(&sb.state, &roster::context(&sb.state).unwrap(), &pair.b.remote)
        .unwrap().unwrap();
    assert_eq!(cached.projects[0].name, "Published after Open");

    for engine in [&pair.ea, &pair.eb] {
        engine.refresh(&Cancellation::default()).await.unwrap();
    }
    let old=[pair.a.trace.lock().unwrap().manifest_items,pair.b.trace.lock().unwrap().manifest_items];
    let a=pair.a.clone();
    let empty=pair.ha.execute(move |_| async move { a.round().await }).await.unwrap();
    responder_finished(&pair).await;
    assert_eq!((empty.completed,empty.total),(0,0));
    for (link,before) in [(&pair.a,old[0]),(&pair.b,old[1])] {
        assert_eq!(link.trace.lock().unwrap().manifest_items,before);
    }
    // A Manual on the larger device must request full from the initiator too.
    pair.eb.force_full_catalogs();
    let a=pair.a.clone();
    let manual=pair.ha.execute(move |_| async move { a.round().await }).await.unwrap();
    responder_finished(&pair).await;
    assert!(manual.failures.is_empty());
    assert_eq!(pair.a.trace.lock().unwrap().last_open.as_ref().unwrap().1,None);
    for (link,before) in [(&pair.a,old[0]),(&pair.b,old[1])] {
        assert_eq!(link.trace.lock().unwrap().manifest_items[2]-before[2],2);
    }
}

#[tokio::test]
async fn peer_inventory_failures_or_omitted_issues_do_not_confirm_the_initiator_checkpoint() {
    for omitted in [false, true] {
        let (_ra, sa, fa) = board("member-a");
        let (_rb, sb, fb) = board("member-b");
        let pair = connect(sa, fa, sb, fb).await;
        {
            let mut snapshot = pair.eb.snapshot.lock().unwrap();
            let snapshot = snapshot.as_mut().unwrap();
            if omitted {
                snapshot.omitted = 1;
            } else {
                snapshot.issues.push(Failure::checked("local", "invalid_local_thread_record", Some("broken-thread"), None));
            }
        }
        let a = pair.a.clone();
        let outcome = pair.ha.execute(move |_| async move { a.round().await }).await.unwrap();
        responder_finished(&pair).await;
        assert_eq!(outcome.omitted, u64::from(omitted));
        assert_eq!(outcome.failures.is_empty(), omitted);
        assert_eq!(pair.ea.checkpoint(&pair.a.remote).unwrap(), None);
    }
}
