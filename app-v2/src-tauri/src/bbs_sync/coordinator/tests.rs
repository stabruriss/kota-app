use super::super::control::Role;
use super::super::exchange::tests::{board, publish};
use super::super::transport::NetworkHost;
use super::*;
use std::sync::{atomic::AtomicUsize, Mutex};

#[tokio::test]
async fn manual_wait_ends_at_active_window_without_bypassing_backoff() {
    let (_root, store, fence) = board("member-a");
    let own = DeviceIdentity::from_seed([91; 32]).unwrap();
    let other = DeviceIdentity::from_seed([92; 32]).unwrap();
    let other_id = other.device_id().unwrap();
    let members = vec![
        Member { device_id: own.device_id().unwrap(), public_key: own.public_key.clone(),
            name: "A".into(), role: Role::Member, membership_id: fence.membership_id.clone(), online: true, last_seen_at: 0 },
        Member { device_id: other_id.clone(), public_key: other.public_key,
            name: "B".into(), role: Role::Owner, membership_id: "member-b".into(), online: true, last_seen_at: 0 },
    ];
    let authority = Authority {
        membership: Membership { group_id: fence.group_id.clone(), membership_id: fence.membership_id.clone(),
            worker_url: "https://fixture.invalid".into(), role: Role::Member },
        identity: own, members, valid_until: now() + 90_000,
    };
    let (tx, rx) = std::sync::mpsc::sync_channel(16);
    let host = NetworkHost::start().await.unwrap();
    host.execute(move |ctx| async move {
        let errors = Arc::new(Mutex::new(Vec::new()));
        let recorded = errors.clone();
        let mut actor = Actor::new(ctx, store, authority, Port { tx, fence, epoch: 0 }, Arc::new(move |_, event| {
            if let Notice::Error(_, error) = event { recorded.lock().unwrap().push(error); }
        }))?;
        let deadline = now() + super::super::scheduler::ACTIVE_MS;
        actor.manual_deadline.store(u64::MAX, Ordering::Relaxed);
        actor.expire_manual_wait(deadline + 60_000);
        assert!(errors.lock().unwrap().is_empty(), "preparation is not a total scan-duration limit");
        actor.manual_deadline.store(deadline, Ordering::Relaxed);
        actor.expire_manual_wait(deadline - 1);
        assert!(errors.lock().unwrap().is_empty());
        actor.expire_manual_wait(deadline);
        assert_eq!(*errors.lock().unwrap(), vec![Error::Timeout]);
        assert!(actor.schedule.peers[&other_id].retry_at > deadline);
        assert!(actor.schedule.candidates(deadline).is_empty());
        assert!(rx.try_recv().is_err(), "this deadline must not issue HTTP or create a peer");
        actor.manual_deadline.store(deadline, Ordering::Relaxed);
        actor.emit(Notice::Connecting(other_id.clone()));
        assert_eq!(actor.manual_deadline.load(Ordering::Relaxed), 0);
        actor.manual_deadline.store(deadline, Ordering::Relaxed);
        actor.emit(Notice::Exchange(exchange::Event::Started(other_id)));
        assert_eq!(actor.manual_deadline.load(Ordering::Relaxed), 0);
        actor.manual_deadline.store(deadline, Ordering::Relaxed);
        actor.work.cancel();
        actor.sync_epoch();
        assert_eq!(actor.manual_deadline.load(Ordering::Relaxed), 0);
        Ok(())
    }).await.unwrap();
    host.stop_and_wait().await;
}

#[tokio::test]
async fn actual_actors_wake_glare_exchange_and_cancel_without_stun_or_fast_idle_poll() {
    let (_ra, sa, fa) = board("member-a");
    let (_rb, sb, fb) = board("member-b");
    let raw = publish(&sa, "Actor-owned round");
    let mut identities = [
        DeviceIdentity::from_seed([91; 32]).unwrap(),
        DeviceIdentity::from_seed([92; 32]).unwrap(),
    ];
    identities.sort_by_key(|k| k.device_id().unwrap());
    let [ka, kb] = identities;
    let aid = ka.device_id().unwrap();
    let bid = kb.device_id().unwrap();
    let members = vec![
        Member {
            device_id: aid.clone(),
            public_key: ka.public_key.clone(),
            name: "a".into(),
            role: Role::Owner,
            membership_id: fa.membership_id.clone(),
            last_seen_at: now(),
            online: true,
        },
        Member {
            device_id: bid.clone(),
            public_key: kb.public_key.clone(),
            name: "b".into(),
            role: Role::Member,
            membership_id: fb.membership_id.clone(),
            last_seen_at: now(),
            online: true,
        },
    ];
    let ma = Membership {
        group_id: fa.group_id.clone(),
        membership_id: fa.membership_id.clone(),
        worker_url: "https://example.test".into(),
        role: Role::Owner,
    };
    let mut mb = ma.clone();
    mb.membership_id = fb.membership_id.clone();
    mb.role = Role::Member;
    let a = Authority {
        membership: ma,
        identity: ka,
        members: members.clone(),
        valid_until: now() + 90_000,
    };
    let b = Authority {
        membership: mb,
        identity: kb,
        members,
        valid_until: now() + 90_000,
    };
    // Production control projection persists these identities/members before
    // constructing actors. The v3 fixture must exercise that same prerequisite.
    for (store, authority) in [(&sa, &a), (&sb, &b)] {
        store.state.save_identity(&authority.identity).unwrap();
        let current = super::super::control::read_membership(&store.state).unwrap();
        super::super::roster::persist_members(
            &store.state,
            current.as_ref(),
            &authority.members,
        )
        .unwrap();
    }
    let roster_a = super::super::roster::runtime::Runtime::test_source(sa.clone(), vec![]);
    let roster_b = super::super::roster::runtime::Runtime::test_source(sb.clone(), vec![]);
    // Preflight authentication is exercised before allocating any PC/socket.
    let ab = a.peer(&bid).unwrap();
    let ba = b.peer(&aid).unwrap();
    let issued = now();
    let signed = SignedWake::create(&a.identity, &ab, Kind::Wake, &"a".repeat(32), issued).unwrap();
    assert!(signed.verify(&ba, issued).is_ok());
    assert!(signed.verify(&ba, issued + 60_000).is_err());
    let mut rejoined = ba.clone();
    rejoined.local_membership_id = "new-membership".into();
    assert!(signed.verify(&rejoined, issued).is_err());
    let mut wire = serde_json::to_value(&signed).unwrap();
    wire["message"]["kind"] = serde_json::json!("ready");
    let tampered: SignedWake = serde_json::from_value(wire).unwrap();
    assert!(tampered.verify(&ba, issued).is_err());
    let authority_a = Arc::new(RwLock::new(a.clone()));
    let connections_a = Arc::new(Mutex::new(Vec::new()));
    let live_a = authority_a.clone();
    let registered_a = connections_a.clone();
    let hub = Arc::new(Mutex::new(BTreeMap::<String, Vec<Signal>>::new()));
    let polls = Arc::new(AtomicUsize::new(0));
    fn port(
        id: String,
        fence: GroupFence,
        hub: Arc<Mutex<BTreeMap<String, Vec<Signal>>>>,
        polls: Arc<AtomicUsize>,
    ) -> Port {
        let (tx, rx) = std::sync::mpsc::sync_channel::<ControlWork>(16);
        std::thread::spawn(move || {
            while let Ok(work) = rx.recv() {
                let result = match work.kind {
                    ControlKind::Send { to, payload } => {
                        hub.lock()
                            .unwrap()
                            .entry(to.clone())
                            .or_default()
                            .push(Signal {
                                id: uuid::Uuid::new_v4().to_string(),
                                from: id.clone(),
                                to,
                                payload,
                                expires_at: now() + 120_000,
                            });
                        vec![]
                    }
                    ControlKind::Pull => {
                        polls.fetch_add(1, Ordering::Relaxed);
                        hub.lock().unwrap().remove(&id).unwrap_or_default()
                    }
                };
                let _ = work.reply.send(Ok(result));
            }
        });
        Port {
            tx,
            fence,
            epoch: 0,
        }
    }
    let pa = port(aid.clone(), fa.clone(), hub.clone(), polls.clone());
    let pb = port(bid.clone(), fb.clone(), hub.clone(), polls.clone());
    // Match App production: directory publication and transfer share the same
    // account file service, including for Manual requests from either role.
    let (files_a, limits_a) = roster_a.files().unwrap();
    let (files_b, limits_b) = roster_b.files().unwrap();
    let ha = NetworkHost::start_with_files(files_a, limits_a).await.unwrap();
    let hb = NetworkHost::start_with_files(files_b, limits_b).await.unwrap();
    let (ta, ra) = mpsc::channel(16);
    let (tb, rb) = mpsc::channel(16);
    let outcomes = Arc::new(AtomicUsize::new(0));
    let errors = Arc::new(Mutex::new(Vec::new()));
    fn observer(outcomes: Arc<AtomicUsize>, errors: Arc<Mutex<Vec<Error>>>) -> Observe {
        Arc::new(move |_, notice| match notice {
            Notice::Exchange(exchange::Event::Finished(_, r)) => {
                assert!(r.failures.is_empty());
                outcomes.fetch_add(1, Ordering::Relaxed);
            }
            Notice::Error(_, e) => {
                eprintln!("actor_diagnostic={e}");
                errors.lock().unwrap().push(e);
            }
            _ => {}
        })
    }
    let oa = observer(outcomes.clone(), errors.clone());
    let ob = observer(outcomes.clone(), errors.clone());
    let store_b = sb.clone();
    let store_a = sa.clone();
    let b_after = b.clone();
    let work_a = Arc::new(Work::default());
    let work_b = Arc::new(Work::default());
    let actor_work_a = work_a.clone();
    let actor_work_b = work_b.clone();
    let ah = ha.clone();
    let bh = hb.clone();
    let (inspection, internal) = oneshot::channel();
    let ja = tokio::spawn(async move {
        ah.execute(move |ctx| async move {
            let actor = Actor::new(ctx, sa, a, pa, oa)?
                .roster(roster_a)
                .live_authority(live_a, registered_a)
                .work_control(actor_work_a)
                .loopback();
            let _ = inspection.send(actor.tx.clone());
            actor.run(ra).await
        })
        .await
    });
    let jb = tokio::spawn(async move {
        bh.execute(move |ctx| async move {
            Actor::new(ctx, store_b, b, pb, ob)?
                .roster(roster_b)
                .work_control(actor_work_b)
                .loopback()
                .run(rb)
                .await
        })
        .await
    });
    let end = std::time::Instant::now() + Duration::from_secs(35);
    while outcomes.load(Ordering::Relaxed) < 2 {
        if std::time::Instant::now() >= end {
            ha.stop();
            hb.stop();
            panic!("actors did not complete: {:?}", *errors.lock().unwrap());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        std::fs::read(sb.root().join("threads/thread-one/posts/root.md")).unwrap(),
        raw
    );
    let directory = super::super::roster::context(&sb.state).unwrap();
    let cached = super::super::roster::read_peer(&sb.state, &directory, &aid)
        .unwrap()
        .expect("the actor exchanged the authenticated peer roster");
    assert!(cached.projects.is_empty());
    let polls_after = polls.load(Ordering::Relaxed);
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        polls.load(Ordering::Relaxed),
        polls_after,
        "connected actors must stop signaling polling"
    );
    // Exercise Manual after the original round, from both protocol roles. The
    // larger-ID side must actually wake the offerer's round via Changed, not
    // merely acknowledge the local command or flash its status.
    for commands in [&ta, &tb] {
        let before = outcomes.load(Ordering::Relaxed);
        commands.send(Command::Manual(0)).await.unwrap();
        tokio::time::timeout(Duration::from_secs(12), async {
            while outcomes.load(Ordering::Relaxed) < before + 2 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.expect("manual request did not complete a real bidirectional round");
    }
    // Shared authority changes fence idle connections immediately, before the
    // actor can consume its Members command or issue a later HTTP heartbeat.
    let idle = connections_a
        .lock()
        .unwrap()
        .iter()
        .filter_map(Weak::upgrade)
        .collect::<Vec<_>>();
    assert_eq!(idle.len(), 1);
    assert!(idle[0].recheck_membership().is_ok());
    let internal = internal.await.unwrap();
    internal
        .send(Internal::Round(
            bid.clone(),
            u64::MAX,
            Err(Error::Unauthorized),
        ))
        .await
        .unwrap();
    internal
        .send(Internal::Exchange(
            u64::MAX,
            exchange::Event::Failed(bid.clone(), Error::Integrity),
        ))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        idle[0].recheck_membership().is_ok(),
        "old link results must not close the current peer"
    );
    // Cancel clears only old work. No Manual/Restart is used below.
    work_a.changed(); // Old queued marker must be swallowed by the cutoff.
    work_a.cancel();
    work_b.cancel();
    tokio::time::sleep(Duration::from_millis(600)).await;
    let before = outcomes.load(Ordering::Relaxed);
    ta.send(Command::Manual(0)).await.unwrap(); // queued before Cancel
    internal
        .send(Internal::Signals(0, Ok(vec![])))
        .await
        .unwrap();
    internal
        .send(Internal::Refreshed(0, Err(Error::Io)))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(outcomes.load(Ordering::Relaxed), before);
    assert!(idle[0].recheck_membership().is_err());
    let updated = publish(&store_a, "New change after Cancel");
    work_a.changed();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while hub.lock().unwrap().get(&bid).is_none_or(|s| s.is_empty()) {
        assert!(
            std::time::Instant::now() < deadline,
            "fresh marker failed to create new wake"
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    // Simulates B's next normally phased heartbeat, not an extra fast poll.
    let signals = hub.lock().unwrap().remove(&bid).unwrap_or_default();
    tb.send(Command::Members(b_after, signals, work_b.epoch()))
        .await
        .unwrap();
    while outcomes.load(Ordering::Relaxed) < before + 2 {
        assert!(
            std::time::Instant::now() < deadline,
            "automatic post-Cancel round failed: {:?}",
            *errors.lock().unwrap()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(sb
        .state
        .load_state()
        .unwrap()
        .unwrap()
        .ledger
        .values()
        .any(|p| p.version_id == crate::bbs_sync::raw_sha256(&updated)));
    let idle = connections_a
        .lock()
        .unwrap()
        .iter()
        .filter_map(Weak::upgrade)
        .collect::<Vec<_>>();
    assert_eq!(idle.len(), 1);
    authority_a
        .write()
        .unwrap()
        .members
        .retain(|m| m.device_id != bid);
    assert_eq!(idle[0].recheck_membership(), Err(Error::Unauthorized));
    ta.send(Command::Cancel).await.unwrap();
    tb.send(Command::Cancel).await.unwrap();
    ha.stop();
    hb.stop();
    let _ = ja.await;
    let _ = jb.await;
    ha.stop_and_wait().await;
    hb.stop_and_wait().await;
}

#[test]
fn cancel_marker_cutoff_preserves_only_genuinely_new_changes() {
    let work = Work::default();
    work.changed();
    work.changed();
    work.cancel();
    let cancelled = work.epoch();
    assert_eq!(work.cutoff.load(Ordering::Acquire), work.serial());
    work.changed();
    assert!(work.serial() > work.cutoff.load(Ordering::Acquire));
    assert_eq!(work.epoch(), cancelled);
    work.cancel();
    assert_eq!(work.cutoff.load(Ordering::Acquire), work.serial());
}
