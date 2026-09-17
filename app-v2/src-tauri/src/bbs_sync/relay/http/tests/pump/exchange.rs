//! Real content RPCs over the existing two-worker HTTP harness and mutual TLS.
//! The relay is process-local; no Cloudflare or production entry is exercised.
use super::*;
use crate::bbs_sync::{
    control::{Member, Role},
    exchange::{self as content, Engine},
    roster,
    transport::NetworkHost,
};
use base64::Engine as _;

#[tokio::test]
async fn relay_carrier_preserves_real_two_way_forks_attachments_rosters_and_reuse() {
    let f = fixture();
    let (_a, sa, mut fa) = content::tests::board(&f.contexts[0].peer.local_membership_id);
    let (_b, sb, mut fb) = content::tests::board(&f.contexts[1].peer.local_membership_id);
    let raw = [
        content::tests::publish(&sa, "A byte-exact root"),
        content::tests::publish(&sb, "B distinct immutable fork"),
    ];
    fa.group_id = f.contexts[0].peer.group_id.clone();
    fb.group_id = fa.group_id.clone();
    let members: Vec<_> = (0..2)
        .map(|i| Member {
            device_id: f.identities[i].device_id().unwrap(),
            public_key: f.identities[i].public_key.clone(),
            name: format!("Device {i}"),
            role: if i == 0 { Role::Owner } else { Role::Member },
            membership_id: f.contexts[i].peer.local_membership_id.clone(),
            online: true,
            last_seen_at: f.now,
        })
        .collect();
    for (i, store) in [&sa, &sb].into_iter().enumerate() {
        let path = store.state.control_path();
        let mut control: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        control["current"]["groupId"] = json!(fa.group_id);
        control["current"]["workerUrl"] = json!(f.contexts[i].origin);
        control["current"]["role"] = json!(members[i].role);
        fs::write(&path, serde_json::to_vec(&control).unwrap()).unwrap();
        store.state.save_identity(&f.identities[i]).unwrap();
        let mut state = store.state.load_state().unwrap().unwrap();
        let shared = state.groups.remove("group-test").unwrap();
        state.groups.insert(fa.group_id.clone(), shared);
        store.state.save_state(&state).unwrap();
        let current = crate::bbs_sync::control::read_membership(&store.state).unwrap();
        roster::persist_members(&store.state, current.as_ref(), &members).unwrap();
    }
    let avatar = base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aX1sAAAAASUVORK5CYII=").unwrap();
    let sha = raw_sha256(&avatar);
    fs::create_dir_all(sa.root().join("avatars")).unwrap();
    fs::write(
        sa.root().join("avatars").join(format!("{sha}.png")),
        &avatar,
    )
    .unwrap();
    let ra = roster::runtime::Runtime::test_source(
        sa.clone(),
        vec![roster::Project {
            project_id: "source".into(),
            name: "Source".into(),
            agents: vec![roster::Agent {
                agent_id: "agent-source".into(),
                name: "Source agent".into(),
                avatar: roster::Avatar::Image {
                    sha256: sha.clone(),
                    ext: "png".into(),
                    size_bytes: avatar.len() as u64,
                },
            }],
        }],
    );
    let rb = roster::runtime::Runtime::test_source(sb.clone(), vec![]);
    // Roster, refresh, transport and installer share one FileIo per account.
    let files = [ra.files().unwrap(), rb.files().unwrap()];
    let host = NetworkHost::start_with_files(files[0].0.clone(), files[0].1.clone())
        .await
        .unwrap();
    let stores = [sa.clone(), sb.clone()];
    let fences = [fa.clone(), fb.clone()];
    let checked = host.execute(move |_| async move {
        let began = Instant::now();
        let mut pair = Pair::with_files(Some(files)).await;
        let mut engines = Vec::new();
        for (i, runtime) in [ra, rb].into_iter().enumerate() {
            let mut engine = Engine::new(stores[i].clone(), fences[i].clone(),
                pair.nodes[i].context.clone(), Arc::new(move |_, event| {
                    match event {
                        content::Event::Progress(_, done, total) => eprintln!("relay_content_progress device={i} elapsed_ms={} {done}/{total}", began.elapsed().as_millis()),
                        content::Event::Failed(_, error) => eprintln!("relay_content_failed device={i} elapsed_ms={} {error}", began.elapsed().as_millis()),
                        _ => {},
                    }
                }));
            Arc::get_mut(&mut engine).unwrap().roster = Some(runtime);
            engine.refresh(&Cancellation::default()).await?;
            engines.push(engine);
        }
        let (a, mut arx) = activity::run(activity::take(&mut pair.nodes[0]));
        let (b, mut brx) = activity::run(activity::take(&mut pair.nodes[1]));
        let channels = [activity::ready(&mut arx).await, activity::ready(&mut brx).await];
        let mut links = Vec::new();
        for (i, channels) in channels.into_iter().enumerate() {
            let connection = channels.into_connection()?;
            assert!(!connection.cancellation().is_cancelled(), "adoption must not run the old Channels drop guard");
            assert!(connection.diagnostics().await?.is_none(), "relay cannot invent ICE candidates or datagram counters");
            links.push(content::Link::start(connection, f.identities[1-i].device_id().unwrap(), i==0, engines[i].clone()));
        }
        // A round contains many sequential RPCs; twenty seconds is the
        // unchanged per-operation/no-progress deadline, not its total budget.
        let first = tokio::time::timeout(Duration::from_secs(60), links[0].round()).await.unwrap()?;
        eprintln!("relay_content_finished elapsed_ms={}", began.elapsed().as_millis());
        assert!(first.failures.is_empty(), "{}", serde_json::to_string(&first).unwrap());
        assert_eq!(first.completed, first.total);
        assert!(first.total >= 3);
        assert!(!first.more && first.omitted == 0);
        for (store, fence) in stores.iter().zip(&fences) {
            for bytes in &raw {
                let version = raw_sha256(bytes);
                let post = Resource { identity: ResourceKind::Post {
                    thread_id: "thread-one".into(), post_id: "root".into(), version_id: version.clone(),
                }, sha256: version.clone(), size_bytes: bytes.len() as u64 };
                assert_eq!(fs::read(store.source(fence, &post).unwrap()).unwrap(), *bytes);
                let bytes = b"immutable attachment bytes";
                let attachment = Resource { identity: ResourceKind::Attachment {
                    thread_id: "thread-one".into(), post_id: "root".into(), attachment_id: "file".into(), version_id: version,
                }, sha256: raw_sha256(bytes), size_bytes: bytes.len() as u64 };
                assert_eq!(fs::read(store.source(fence, &attachment).unwrap()).unwrap(), bytes);
            }
        }
        until(|| links.iter().all(|link| !link.active())).await;
        let second = tokio::time::timeout(Duration::from_secs(60), links[0].round()).await.unwrap()?;
        assert_eq!((second.completed, second.total), (0, 0));
        assert!(second.failures.is_empty() && !second.more);
        until(|| links.iter().all(|link| !link.active())).await;
        assert!(!links[0].is_stopped() && !links[1].is_stopped());
        for link in &links { link.cancel(); }
        for task in [a, b] {
            let _ = tokio::time::timeout(Duration::from_secs(2), task).await.unwrap().unwrap();
        }
        let requests = {
            let r = pair.relay.lock().unwrap();
            assert_eq!(r.created, 4, "exactly two existing HTTP workers per account");
            r.send_sizes.len()+r.receives+r.small_reads+r.acknowledgements
        };
        pair.stop().await;
        println!("BBS_RELAY_CONTENT_FIXTURE {}", json!({"first":first,"second":second,"requests":requests,"carrier":"relay","realTls":true,"realFileIo":true,"cloudflare":false}));
        Ok(())
    }).await;
    host.stop_and_wait().await;
    checked.unwrap();
    let empty = roster::read_peer(
        &sa.state,
        &roster::context(&sa.state).unwrap(),
        &fixture().identities[1].device_id().unwrap(),
    )
    .unwrap()
    .expect("the opposite direction installs a complete empty roster");
    assert!(empty.projects.is_empty());
    let cached = roster::read_peer(
        &sb.state,
        &roster::context(&sb.state).unwrap(),
        &fixture().identities[0].device_id().unwrap(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(cached.projects[0].agents[0].name, "Source agent");
    assert_eq!(
        fs::read(sb.root().join("avatars").join(format!("{sha}.png"))).unwrap(),
        avatar
    );
    assert!(
        sb.read_verified_avatar(&sha, "png").is_err(),
        "roster carrier does not grant post-avatar authority"
    );
}

#[tokio::test]
async fn rejected_off_runtime_channel_adoption_cancels_the_original_relay() {
    let mut pair = Pair::new().await;
    let channels = pair.nodes[0].channels.take().unwrap();
    assert!(matches!(channels.into_connection(), Err(Error::Runtime)));
    assert!(pair.nodes[0].cancel.is_cancelled());
    pair.stop().await;
}

/// Pause only the last application credit, after the responder really received
/// Finish/Ok. The relay receipt and reverse TLS payload then share one envelope.
async fn final_during_finish_credit(peer_lost: bool) {
    use crate::bbs_sync::relay::activity::{Event, Failure};
    use tokio::sync::{mpsc, oneshot};
    let f = fixture();
    let (_a, sa, fa) = content::tests::board(&f.contexts[0].peer.local_membership_id);
    let (_b, sb, fb) = content::tests::board(&f.contexts[1].peer.local_membership_id);
    let host = NetworkHost::start().await.unwrap();
    let checked = host.execute(move |_| async move {
        let mut pair = Pair::unstarted().await;
        let errors = Arc::new(Mutex::new(Vec::new()));
        let mut engines = Vec::new();
        for (i, (store, fence)) in [(sa, fa), (sb, fb)].into_iter().enumerate() {
            let errors = errors.clone();
            let engine = Engine::new(store, fence, pair.nodes[i].context.clone(), Arc::new(move |_, event| {
                if let content::Event::Failed(_, error) = event { errors.lock().unwrap().push(error); }
            }));
            engine.refresh(&Cancellation::default()).await?;
            engines.push(engine);
        }
        let (a, mut arx) = activity::run(activity::take(&mut pair.nodes[0]));
        let mut owner = activity::take(&mut pair.nodes[1]);
        let session = pair.nodes[1].session.clone();
        let (btx, mut brx) = mpsc::channel(1);
        let (close, mut commands) = mpsc::unbounded_channel::<oneshot::Sender<Result<bool>>>();
        let b = tokio::spawn(async move {
            loop {
                tokio::select! {
                    command = commands.recv() => {
                        if let Some(reply) = command {
                            let _ = reply.send(owner.final_ack(&session).map(|ack| ack.is_some_and(|ack| ack.body.contains("true"))));
                        }
                    }
                    result = owner.next() => match result {
                        Ok(Event::Ready { channels, .. }) => btx.send(channels).await.unwrap(),
                        Ok(Event::Retired { error, .. }) => return Failure::Transport(error),
                        Err(error) => return error,
                    }
                }
            }
        });
        let channels = [activity::ready(&mut arx).await, activity::ready(&mut brx).await];
        let links: Vec<_> = channels.into_iter().enumerate().map(|(i, channels)| {
            content::Link::start(channels.into_connection().unwrap(), f.identities[1-i].device_id().unwrap(), i==0, engines[i].clone())
        }).collect();
        let (reached, release) = links[1].defer_finish_credit();
        let initiator = links[0].clone();
        let began = Instant::now();
        let round = tokio::spawn(async move { initiator.round().await });
        tokio::time::timeout(Duration::from_secs(60), reached).await.unwrap().unwrap();
        until(|| !links[1].active()).await;
        assert!(links[0].active() && !round.is_finished());
        let waiting = Instant::now();
        let id = pair.nodes[0].session.clone();
        let reverse = {
            let mut relay = pair.relay.lock().unwrap();
            relay.hold_receive = Some(f.identities[0].device_id().unwrap());
            relay.sessions[&id].directions[1].next
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let (tx, rx) = oneshot::channel();
                close.send(tx).unwrap();
                if rx.await.unwrap().unwrap() { break; }
                tokio::task::yield_now().await;
            }
        }).await.unwrap();
        until(|| pair.relay.lock().unwrap().sessions[&id].directions[0].closed).await;
        if peer_lost {
            // The remote actually disappears before delivering the last credit.
            pair.nodes[1].cancel.cancel();
            drop(release);
        } else {
            release.send(()).unwrap();
            until(|| pair.relay.lock().unwrap().sessions[&id].directions[1].next > reverse).await;
        }
        pair.relay.lock().unwrap().hold_receive = None;
        let outcome = tokio::time::timeout(PROGRESS_TIMEOUT + Duration::from_secs(6), round).await.unwrap().unwrap();
        if peer_lost {
            assert!(outcome.is_err());
            // Turn's existing 20 s wait already started before the responder
            // reached Finish. Receiving final must not restart that clock.
            assert!(began.elapsed() >= PROGRESS_TIMEOUT);
            assert!(errors.lock().unwrap().contains(&Error::Timeout), "{:?}", errors.lock().unwrap());
        } else {
            let outcome = outcome.expect("final must not discard reverse Finish credit");
            assert_eq!((outcome.completed, outcome.total), (0, 0));
            assert!(outcome.failures.is_empty() && !outcome.more);
            assert!(errors.lock().unwrap().is_empty(), "{:?}", errors.lock().unwrap());
            assert!(pair.relay.lock().unwrap().final_with_payload > 0);
            assert!(!links[0].is_stopped());
        }
        println!("BBS_RELAY_FINAL_FIXTURE {}", json!({"peerLost":peer_lost,"roundElapsedMs":began.elapsed().as_millis(),
            "afterFinishMs":waiting.elapsed().as_millis(),"sameEnvelope":pair.relay.lock().unwrap().final_with_payload,
            "errors":errors.lock().unwrap().iter().map(ToString::to_string).collect::<Vec<_>>(),"realLink":true,"realTls":true,"cloudflare":false}));
        for link in &links { link.cancel(); }
        for task in [a, b] { let _ = tokio::time::timeout(Duration::from_secs(2), task).await.unwrap().unwrap(); }
        pair.stop().await;
        Ok(())
    }).await;
    host.stop_and_wait().await;
    checked.unwrap();
}

#[tokio::test]
async fn final_keeps_reverse_finish_credit_until_real_round_completes() {
    final_during_finish_credit(false).await;
}

#[tokio::test]
async fn final_without_reverse_finish_credit_still_times_out() {
    final_during_finish_credit(true).await;
}
