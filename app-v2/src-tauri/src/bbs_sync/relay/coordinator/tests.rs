use super::*;
use crate::bbs_sync::{
    control::{Member, Membership, Role},
    relay::tests::fixture,
    relay::HttpPool,
    transport::{FileIo, Limits},
};

struct Rig {
    _root: exchange::tests::Root,
    actor: Actor,
    pool: HttpPool,
    notices: Arc<Mutex<Vec<Notice>>>,
    peer: String,
}
impl Drop for Rig {
    fn drop(&mut self) {
        self.pool.stop();
    }
}
impl Rig {
    async fn new() -> Self {
        let f = fixture();
        let (root, store, _) = exchange::tests::board(&f.contexts[0].peer.local_membership_id);
        store.state.save_identity(&f.identities[0]).unwrap();
        let path = store.state.control_path();
        let mut control: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        control["current"]["groupId"] = serde_json::json!(f.contexts[0].peer.group_id);
        control["current"]["workerUrl"] = serde_json::json!(f.contexts[0].origin);
        control["current"]["role"] = serde_json::json!("owner");
        std::fs::write(path, serde_json::to_vec(&control).unwrap()).unwrap();
        let members: Vec<_> = (0..2)
            .map(|i| Member {
                device_id: f.identities[i].device_id().unwrap(),
                public_key: f.identities[i].public_key.clone(),
                name: format!("Fixture {i}"),
                role: if i == 0 { Role::Owner } else { Role::Member },
                membership_id: f.contexts[i].peer.local_membership_id.clone(),
                last_seen_at: now(),
                online: true,
            })
            .collect();
        let authority = Authority {
            membership: Membership {
                group_id: f.contexts[0].peer.group_id.clone(),
                worker_url: f.contexts[0].origin.clone(),
                role: Role::Owner,
                membership_id: members[0].membership_id.clone(),
            },
            identity: f.identities[0].clone(),
            members,
            valid_until: now() + 90000,
        };
        let limits = Limits::default();
        let io = FileIo::start(limits.clone()).unwrap();
        // Constructing these explicitly authorized workers performs no DNS or
        // HTTP. Tests below never poll a metadata future or start a TLS stream.
        let pool = HttpPool::start(limits.clone()).await.unwrap();
        let context = Context {
            limits,
            io,
            shutdown: Cancellation::default(),
        };
        let roster = roster::runtime::Runtime::test_source(store.clone(), vec![]);
        let notices: Arc<Mutex<Vec<Notice>>> = Default::default();
        let n = notices.clone();
        let actor = Actor::new(
            context,
            store,
            authority.clone(),
            pool.bind(&authority.membership.worker_url).unwrap(),
            Arc::new(move |_, e| n.lock().unwrap().push(e)),
            Arc::new(Work::default()),
            0,
            roster,
            Arc::new(RwLock::new(authority)),
            Arc::new(Mutex::new(Vec::new())),
        )
        .unwrap();
        Self {
            _root: root,
            actor,
            pool,
            notices,
            peer: f.contexts[0].peer.remote_device_id.clone(),
        }
    }
}

#[tokio::test]
async fn three_losses_without_endpoint_progress_end_the_attempt_and_keep_backoff() {
    let mut r = Rig::new().await;
    for n in 1..=3 {
        r.actor.fail(&r.peer, Error::RelaySessionLost);
        assert!(r.actor.discovery_stale, "new attempts require a fresh poll");
        assert_eq!(r.actor.losses[&r.peer], n);
        assert_eq!(
            r.notices
                .lock()
                .unwrap()
                .iter()
                .filter(|e| matches!(e, Notice::Error(_, Error::RelaySessionLost)))
                .count(),
            usize::from(n == 3)
        );
    }
    let retry = r.actor.schedule.peers[&r.peer].retry_at;
    assert!(retry >= now() + 4900);
    r.actor.record_progress(&r.peer, 0);
    assert_eq!(
        r.actor.losses[&r.peer], 3,
        "0/0, probes and successful HTTP are not endpoint progress"
    );
    r.actor.record_progress(&r.peer, 1);
    assert!(!r.actor.losses.contains_key(&r.peer));
    r.actor.fail(&r.peer, Error::RelaySessionLost);
    assert_eq!(r.actor.losses[&r.peer], 1);
    assert!(
        r.actor.schedule.peers[&r.peer].retry_at >= retry,
        "progress does not silently clear a prior retryAt"
    );
    let count = r.actor.losses[&r.peer];
    r.actor.service_error(ServiceTask::Poll, Error::CloudflareResourceLimit);
    assert_eq!(
        r.actor.losses[&r.peer], count,
        "platform resource errors are not a boot-loss loop"
    );
    assert!(
        !r.actor.cancel.is_cancelled(),
        "resource failure cannot turn off control membership/heartbeat"
    );
    r.actor.adopt_snapshot(Snapshot {
        boot: uuid::Uuid::new_v4().to_string(),
        items: BTreeMap::new(),
    });
    assert!(!r.actor.discovery_stale);
    assert_eq!(
        r.actor.losses[&r.peer], count,
        "discovery is not content progress"
    );
    r.actor.record_progress(&r.peer, 1);
    assert!(
        !r.actor.losses.contains_key(&r.peer),
        "a new Link's first real install resets losses even if the old round also reached one"
    );
}

#[tokio::test]
async fn cancel_retires_owned_futures_and_old_instance_without_pausing_future_work() {
    let mut r = Rig::new().await;
    r.actor.schedule.failed(&r.peer, now());
    let retry = r.actor.schedule.peers[&r.peer].retry_at;
    r.actor.schedule.changed(now());
    let instance = r.actor.instance.clone();
    let old = r.actor.authorized();
    r.actor.meta = Some(Box::pin(pending()));
    r.actor.file = Some(Box::pin(pending()));
    r.actor.work.cancel();
    r.actor.sync_epoch();
    assert!(!old());
    assert!(r.actor.meta.is_none() && r.actor.file.is_none() && r.actor.round.is_none());
    assert!(!r.actor.needs_refresh && r.actor.forced.is_empty());
    assert!(r.actor.schedule.changed_at.is_none());
    assert_eq!(r.actor.schedule.peers[&r.peer].retry_at, retry);
    assert_ne!(
        r.actor.instance, instance,
        "unknown late wake receipts still bind the retired instance"
    );
    assert!(r.actor.authorized()());
    r.actor.work.changed();
    r.actor.tick().unwrap();
    assert!(r.actor.schedule.changed_at.is_some());
    r.actor.schedule.changed_at = Some(now());
    r.actor.tick().unwrap();
    assert!(
        r.actor.file.is_some(),
        "a real later change prepares a fresh snapshot without Manual/restart"
    );
    r.actor.work.cancel();
    r.actor.sync_epoch();
    r.actor.refresh_at = now();
    r.actor.tick().unwrap();
    assert!(
        r.actor.file.is_some(),
        "normal future metadata fallback also resumes"
    );
}

#[test]
fn day_of_unchanged_discovery_needs_no_handshake_but_first_wake_and_manual_do() {
    let revision = "a".repeat(64);
    let changed = "b".repeat(64);
    let mut handshakes = 0;
    for _ in 0..24 * 60 * 2 {
        handshakes += usize::from(needs_round(Some(&revision), &revision, false, false));
    }
    assert_eq!(handshakes, 0);
    assert!(needs_round(None, &revision, false, false));
    assert!(needs_round(Some(&revision), &changed, false, false));
    assert!(needs_round(Some(&revision), &revision, true, false));
    assert!(needs_round(Some(&revision), &revision, false, true));
}

#[tokio::test]
async fn member_replacement_retires_attempt_and_current_authority_never_trusts_old_peer() {
    let mut r = Rig::new().await;
    let peer = r.actor.initial.peer(&r.peer).unwrap();
    let auth = r.actor.peer_check(&peer);
    assert!(auth());
    let cancel = Cancellation::default();
    r.actor.attempts.insert(
        r.peer.clone(),
        Attempt {
            tag: "test".into(),
            peer,
            instance: "remote-instance-123".into(),
            until: now() + ACTIVE_MS,
            polls: 0,
            wake: Some("c".repeat(64)),
            nonce: "nonce-instance-123".into(),
            ready_boot: None,
            handshake: None,
            offered: false,
            waiting_poll: false,
            tls: None,
            cancel: cancel.clone(),
        },
    );
    let mut next = r.actor.initial.clone();
    next.members[1].membership_id = "new-incarnation-123".into();
    r.actor.members(next).unwrap();
    assert!(!auth());
    assert!(cancel.is_cancelled());
    assert!(r.actor.attempts.is_empty());
    assert!(r.actor.forced.contains(&r.peer));
    assert!(r.actor.engine.checkpoint(&r.peer).unwrap().is_none());
}

#[tokio::test]
async fn authenticated_wake_can_use_stale_presence_but_never_bypass_retry_at() {
    use super::super::discovery::Page;
    use serde_json::json;
    let mut r = Rig::new().await;
    let f = fixture();
    let at = now();
    let remote_instance = "remote-process-123";
    let local = r.actor.own();
    let peer = r.actor.initial.peer(&r.peer).unwrap();
    let bytes = serde_json::to_vec(&json!({"boot":f.contexts[0].boot,"items":[{
        "device":r.peer,"announcement":{"device":r.peer,"membership":peer.remote_membership_id,
            "requestId":"request-fixture-123","fingerprint":"f".repeat(64),"instance":remote_instance,"revision":"a".repeat(64),"peerVersion":4},
        "currentWake":"b".repeat(64),"wake":{"id":"b".repeat(64),"group":peer.group_id,
            "client":local,"server":r.peer,"clientMembership":peer.local_membership_id,"serverMembership":peer.remote_membership_id,
            "clientInstance":r.actor.instance,"serverInstance":remote_instance,"createdAt":at,"expiresAt":at+120000},
        "handshake":{"ready":{"client":false,"server":false},"client":null,"server":null,"session":null,"closed":false}
    }],"next":null})).unwrap();
    let page = Page::decode(&bytes, &peer.group_id, &local, None).unwrap();
    r.actor.adopt_snapshot(Snapshot {
        boot: page.boot,
        items: page
            .items
            .into_iter()
            .map(|i| (i.device.clone(), i))
            .collect(),
    });
    r.actor.schedule.peers.get_mut(&r.peer).unwrap().online = false;
    assert_eq!(r.actor.candidates(at), vec![r.peer.clone()]);
    assert!(!r.actor.schedule.peers[&r.peer].online);
    r.actor.schedule.peers.get_mut(&r.peer).unwrap().retry_at = at + 5000;
    assert!(r.actor.candidates(at).is_empty());
    r.actor.retired(&r.peer, &"b".repeat(64));
    assert!(r.actor.candidates(at + 5000).is_empty());
}

#[tokio::test]
async fn delayed_old_owner_cannot_rebind_the_new_accounts_http_capability() {
    let r = Rig::new().await;
    let newer = r.pool.bind(&r.actor.initial.membership.worker_url).unwrap();
    let old = Actor::new(
        r.actor.context.clone(),
        r.actor.engine.store.clone(),
        r.actor.initial.clone(),
        r.actor.http.clone(),
        r.actor.observe.clone(),
        r.actor.work.clone(),
        r.actor.epoch,
        r.actor.engine.roster.as_ref().unwrap().clone(),
        r.actor.authority.clone(),
        r.actor.connections.clone(),
    );
    assert!(matches!(old, Err(Error::Cancelled)));
    assert!(newer.check_binding().is_ok());
}

#[tokio::test]
async fn live_capability_checks_identity_membership_expiry_and_epoch_on_every_use() {
    let r = Rig::new().await;
    let original = r.actor.initial.clone();
    let peer = original.peer(&r.peer).unwrap();
    let check = r.actor.peer_check(&peer);
    assert!(check());
    let mut presence = original.clone();
    for member in &mut presence.members {
        member.online = false;
        member.name.push_str(" renamed");
    }
    *r.actor.authority.write().unwrap() = presence;
    assert!(check(), "display and presence changes are not revocation");
    for local in [false, true] {
        for key in [false, true] {
            let mut changed = original.clone();
            let id = if local {
                &peer.local_device_id
            } else {
                &peer.remote_device_id
            };
            let member = changed
                .members
                .iter_mut()
                .find(|m| &m.device_id == id)
                .unwrap();
            if key {
                member.public_key.push('x');
            } else {
                member.membership_id.push('x');
            }
            *r.actor.authority.write().unwrap() = changed;
            assert!(
                !check(),
                "changed member tuple cannot use the old capability"
            );
        }
    }
    let mut removed = original.clone();
    removed.members.retain(|m| m.device_id != r.peer);
    *r.actor.authority.write().unwrap() = removed;
    assert!(!check());
    let mut changed = original.clone();
    changed.identity.private_key.push('x');
    *r.actor.authority.write().unwrap() = changed;
    assert!(!check());
    let mut expired = original.clone();
    expired.valid_until = now();
    *r.actor.authority.write().unwrap() = expired;
    assert!(!check());
    *r.actor.authority.write().unwrap() = original;
    assert!(check());
    r.actor.work.cancel();
    assert!(!check());
}

#[tokio::test]
async fn heartbeat_refreshes_authority_deadline_without_restarting_the_owner() {
    let mut r = Rig::new().await;
    let instance = r.actor.instance.clone();
    let epoch = r.actor.epoch;
    r.actor.initial.valid_until = 0;
    let mut renewed = r.actor.initial.clone();
    renewed.valid_until = now() + 90_000;
    r.actor.members(renewed.clone()).unwrap();
    assert_eq!(r.actor.initial.valid_until, renewed.valid_until);
    let peer = r.actor.initial.peer(&r.peer).unwrap();
    assert!(r.actor.peer_check(&peer)());
    assert_eq!(r.actor.instance, instance);
    assert_eq!(r.actor.epoch, epoch);
}

#[tokio::test]
async fn idle_service_failure_retains_a_verification_round_without_manual() {
    let mut r = Rig::new().await;
    r.actor.schedule.completed(&r.peer, now());
    r.actor.poll_at = now() + IDLE_POLL_MS;
    r.actor.service_error(ServiceTask::Poll, Error::Timeout);
    assert!(r.actor.verification.contains(&r.peer),
        "an unchanged checkpoint must not strand the visible failure forever");
    assert!(r.actor.schedule.peers[&r.peer].needed);
    assert!(r.actor.service_retry() > now(), "ordinary backoff still applies");
    assert_eq!(r.actor.poll_at, r.actor.service_retry(),
        "the five-second recovery must wake before the old idle poll");
    assert!(!r.actor.cancel.is_cancelled());
}

#[tokio::test]
async fn healthy_idle_polls_break_the_failure_streak_instead_of_accumulating_minutes() {
    let mut r = Rig::new().await;
    for _ in 0..7 {
        // Separate incidents with successful polls, not seven consecutive
        // failures. No socket or clock sleep is involved in this state test.
        r.actor.meta_done(MetaDone::Poll(Err(Error::Timeout))).unwrap();
        assert!(r.actor.service_retry() <= now() + 5000,
            "healthy intervals must not accumulate to the 300-second cap");
        r.actor.meta_done(MetaDone::Poll(Ok(Snapshot {
            boot: "same-idle-boot-123".into(), items: BTreeMap::new(),
        }))).unwrap();
    }
    assert_eq!(r.actor.service_backoff[ServiceTask::Poll as usize].failures, 0);
}

#[tokio::test]
async fn healthy_idle_polling_keeps_its_thirty_second_cadence() {
    let mut r = Rig::new().await;
    r.actor.schedule.completed(&r.peer, now());
    for _ in 0..2 {
        let before = now();
        r.actor.start_poll().unwrap(); // No future is polled: zero DNS/HTTP.
        assert!(r.actor.poll_at >= before + IDLE_POLL_MS);
        assert!(r.actor.poll_at <= now() + IDLE_POLL_MS + 2000);
        r.actor.meta = None;
        r.actor.meta_done(MetaDone::Poll(Ok(Snapshot {
            boot: "healthy-idle-boot-123".into(), items: BTreeMap::new(),
        }))).unwrap();
    }
    assert_eq!(r.actor.service_retry(), 0);
    assert!(r.actor.verification.is_empty());
}

#[tokio::test]
async fn consecutive_failures_keep_exponential_deadlines_and_other_operation_guards() {
    let mut r = Rig::new().await;
    for delay in [5000, 10000, 20000, 40000, 80000, 160000, 300000] {
        r.actor.poll_at = now() + IDLE_POLL_MS;
        let before = now();
        r.actor.meta_done(MetaDone::Poll(Err(Error::Timeout))).unwrap();
        let wake = r.actor.poll_at.max(r.actor.service_retry());
        assert!(wake >= before + delay && wake <= now() + delay,
            "failure wake still obeys the existing exponential deadline");
    }
    let service_until = r.actor.service_retry();
    r.actor.poll_at = now() + 600000;
    r.actor.fail(&r.peer, Error::Timeout);
    assert_eq!(r.actor.poll_at, service_until,
        "short peer backoff cannot bypass a longer service backoff");
}

#[tokio::test]
async fn successful_poll_does_not_clear_an_announcement_failure_or_fake_completion() {
    let mut r = Rig::new().await;
    r.actor.service_error(ServiceTask::Announce, Error::CloudflareResourceLimit);
    let write_until = r.actor.service_retry();
    r.actor.service_error(ServiceTask::Poll, Error::Timeout);
    r.actor.meta_done(MetaDone::Poll(Ok(Snapshot {
        boot: "same-idle-boot-123".into(), items: BTreeMap::new(),
    }))).unwrap();
    assert_eq!(r.actor.service_backoff[ServiceTask::Poll as usize].failures, 0);
    assert_eq!(r.actor.service_backoff[ServiceTask::Announce as usize].failures, 1);
    assert_eq!(r.actor.service_retry(), write_until);
    assert!(r.actor.verification.contains(&r.peer));
    assert!(!r.notices.lock().unwrap().iter().any(|e| matches!(e,
        Notice::Exchange(exchange::Event::Finished(..)))));
    r.actor.file_done(FileDone::Confirmed(Ok(Completion::Confirmed)));
    assert_eq!(r.actor.service_retry(), 0);
    assert!(r.actor.verification.contains(&r.peer), "an announcement is still not peer content verification");
}

#[tokio::test]
async fn peer_failure_retains_a_verification_round_until_real_completion() {
    let mut r = Rig::new().await;
    r.actor.poll_at = now() + IDLE_POLL_MS;
    r.actor.fail(&r.peer, Error::Timeout);
    assert!(r.actor.verification.contains(&r.peer),
        "failed recovery must survive a later matching revision poll");
    assert!(r.actor.schedule.peers[&r.peer].retry_at > now());
    assert_eq!(r.actor.poll_at, r.actor.schedule.peers[&r.peer].retry_at,
        "an idle peer failure must wake at its actual backoff deadline");
    r.actor.finished(&r.peer, &exchange::Outcome::default());
    assert!(!r.actor.verification.contains(&r.peer));
}

#[tokio::test]
async fn partial_round_does_not_discharge_pending_verification() {
    let mut r = Rig::new().await;
    r.actor.verification.insert(r.peer.clone());
    r.actor.finished(&r.peer, &exchange::Outcome { more: true, ..Default::default() });
    assert!(r.actor.verification.contains(&r.peer));
    r.actor.finished(&r.peer, &exchange::Outcome { omitted: 1, ..Default::default() });
    assert!(r.actor.verification.contains(&r.peer));
    r.actor.finished(&r.peer, &exchange::Outcome::default());
    assert!(!r.actor.verification.contains(&r.peer));
}

#[tokio::test]
async fn cancel_keeps_verification_debt_but_does_not_immediately_retry() {
    let mut r = Rig::new().await;
    r.actor.fail(&r.peer, Error::Timeout);
    let until = r.actor.schedule.peers[&r.peer].retry_at;
    r.actor.work.cancel();
    r.actor.sync_epoch();
    assert!(r.actor.verification.contains(&r.peer));
    assert!(r.actor.forced.is_empty());
    assert_eq!(r.actor.schedule.peers[&r.peer].retry_at, until);
    assert!(!r.actor.schedule.peers[&r.peer].needed);
    assert!(r.actor.attempts.is_empty());
    assert!(r.actor.cancelled_until.is_some_and(|t| t > now()));
}

#[tokio::test]
async fn a_replaced_wake_is_retired_after_fresh_poll_not_after_sixty_seconds() {
    use super::super::discovery::Page;
    use serde_json::json;
    let mut r = Rig::new().await;
    let f = fixture();
    let peer = r.actor.initial.peer(&r.peer).unwrap();
    let instance = "remote-process-123";
    let at = now();
    let bytes = serde_json::to_vec(&json!({"boot":f.contexts[0].boot,"items":[{
        "device":r.peer,"announcement":{"device":r.peer,"membership":peer.remote_membership_id,
            "requestId":"request-fixture-123","fingerprint":"f".repeat(64),"instance":instance,"revision":"a".repeat(64),"peerVersion":4},
        "currentWake":"b".repeat(64),"wake":{"id":"b".repeat(64),"group":peer.group_id,
            "client":r.actor.own(),"server":r.peer,"clientMembership":peer.local_membership_id,"serverMembership":peer.remote_membership_id,
            "clientInstance":r.actor.instance,"serverInstance":instance,"createdAt":at,"expiresAt":at+120000},
        "handshake":{"ready":{"client":false,"server":false},"client":null,"server":null,"session":null,"closed":false}
    }],"next":null})).unwrap();
    let page = Page::decode(&bytes, &peer.group_id, &r.actor.own(), None).unwrap();
    r.actor.attempts.insert(r.peer.clone(), Attempt {
        tag: "old-attempt".into(), peer, instance: instance.into(), until: at + ACTIVE_MS,
        polls: 0, wake: Some("c".repeat(64)), nonce: "old-nonce-123".into(),
        ready_boot: None, handshake: None, offered: false, waiting_poll: true,
        tls: None, cancel: Cancellation::default(),
    });
    r.actor.snapshot = Some(Snapshot { boot: page.boot.clone(),
        items: page.items.iter().cloned().map(|i| (i.device.clone(), i)).collect() });
    assert_eq!(r.actor.drive_attempt(&r.peer), Ok(()),
        "a pre-ACK local snapshot is still fenced by waiting_poll");
    r.actor.adopt_snapshot(Snapshot { boot: page.boot,
        items: page.items.into_iter().map(|i| (i.device.clone(), i)).collect() });
    assert_eq!(r.actor.drive_attempt(&r.peer), Err(Error::RelaySessionLost),
        "fresh poll proves the accepted wake was replaced; do not idle to ACTIVE_MS");
}

#[tokio::test]
async fn idle_boot_change_after_completion_does_not_create_new_verification_work() {
    let mut r = Rig::new().await;
    r.actor.snapshot = Some(Snapshot { boot: "previous-boot-123".into(), items: BTreeMap::new() });
    r.actor.connected.insert(r.peer.clone(), Connected {
        peer: r.actor.initial.peer(&r.peer).unwrap(), instance: "remote-instance-123".into(),
        session: "d".repeat(64), wake: "e".repeat(64), link: None,
        finished: true, closing_at: Some(now()),
    });
    r.actor.adopt_snapshot(Snapshot { boot: "new-idle-boot-123".into(), items: BTreeMap::new() });
    assert!(r.actor.connected.is_empty());
    assert!(r.actor.verification.is_empty());
    assert!(r.actor.losses.is_empty());
    assert!(!r.notices.lock().unwrap().iter().any(|e| matches!(e, Notice::Error(..))));
}
