use super::*;

fn attempt(r: &Rig) -> Attempt {
    Attempt {
        tag: "lease-fixture".into(), peer: r.actor.initial.peer(&r.peer).unwrap(),
        instance: "remote-fixture".into(), until: now() + ACTIVE_MS, polls: 0,
        wake: None, nonce: "fixture-nonce".into(), ready_boot: None, handshake: None,
        offered: false, waiting_poll: false, tls: None, cancel: Cancellation::default(),
        passive_boot: None, trace: Trace::new(false, None),
    }
}
fn observe_lapse(r: &Rig) -> u64 {
    r.actor.authority.write().unwrap().valid_until = 0;
    assert!(!(r.actor.authorized())());
    r.actor.lapse_at.load(Ordering::Acquire)
}
fn no_penalty(r: &Rig) {
    assert!(r.actor.verification.is_empty() && r.actor.recovery.is_empty());
    assert!(!r.notices.lock().unwrap().iter().any(|n| matches!(n, Notice::Error(_, _))));
}

#[tokio::test]
async fn expired_then_renewed_lease_does_not_charge_a_late_peer_refusal() {
    let mut r = Rig::new().await;
    r.actor.attempts.insert(r.peer.clone(), attempt(&r));
    r.actor.initial.valid_until = 0; // renewal is live before the Members mailbox is consumed
    let lapse = observe_lapse(&r);
    r.actor.authority.write().unwrap().valid_until = now() + 90_000;
    r.actor.poll_at = now() + IDLE_POLL_MS;
    r.actor.fail(&r.peer, Error::Unauthorized);
    no_penalty(&r);
    assert!(r.actor.attempts.is_empty());
    let peer = &r.actor.schedule.peers[&r.peer];
    assert!(peer.needed && peer.retry_at <= now() + POLL_MS);
    assert!(r.actor.poll_at <= now() + POLL_MS);
    assert_eq!(r.actor.lapse_at.load(Ordering::Acquire), lapse, "renewal never clears the evidence");
    assert_eq!(r.actor.schedule.candidates(now() + POLL_MS), vec![r.peer.clone()]);
}

#[tokio::test]
async fn an_expired_lease_waits_for_members_without_spinning_poll_or_penalizing_peers() {
    let mut r = Rig::new().await;
    r.actor.attempts.insert(r.peer.clone(), attempt(&r));
    observe_lapse(&r);
    let poll_at = now() + IDLE_POLL_MS;
    r.actor.poll_at = poll_at;
    r.actor.fail(&r.peer, Error::Unauthorized);
    for _ in 0..10 { r.actor.tick().unwrap(); }
    no_penalty(&r);
    assert_eq!(r.actor.poll_at, poll_at);
    assert!(r.actor.meta.is_none() && r.actor.file.is_none());
    assert_eq!(r.actor.schedule.peers[&r.peer].retry_at, 0);
    assert!(r.actor.schedule.peers[&r.peer].needed);
}

#[tokio::test]
async fn worker_rejection_revocation_and_new_attempts_cannot_borrow_a_prior_lapse() {
    for case in 0..4 {
        let mut r = Rig::new().await;
        r.actor.attempts.insert(r.peer.clone(), attempt(&r));
        let lapse = observe_lapse(&r);
        r.actor.authority.write().unwrap().valid_until = now() + 90_000;
        let cause = if case == 0 { Some("worker:unauthorized") } else { None };
        if case == 1 { r.actor.authority.write().unwrap().members.retain(|m| m.device_id != r.peer); }
        if case == 2 { r.actor.attempts.get_mut(&r.peer).unwrap().trace.started_at = lapse + 1; }
        let error = if case == 3 { Error::Timeout } else { Error::Unauthorized };
        r.actor.fail_with_cause(&r.peer, error, cause);
        assert!(r.actor.verification.contains(&r.peer));
        assert!(r.actor.schedule.peers[&r.peer].retry_at > now() + POLL_MS);
        assert!(r.notices.lock().unwrap().iter().any(|n| matches!(n, Notice::Error(_, _))));
    }
}

#[tokio::test]
async fn service_lease_refusal_keeps_backoffs_verification_and_manual_unchanged() {
    for task in [ServiceTask::Poll, ServiceTask::Announce, ServiceTask::Catalog, ServiceTask::Exchange] {
        let mut r = Rig::new().await;
        let start = now();
        observe_lapse(&r);
        let poll_at = now() + IDLE_POLL_MS;
        let manual = Some(now() + ACTIVE_MS);
        r.actor.poll_at = poll_at;
        r.actor.manual_until = manual;
        r.actor.service_backoff[task as usize] = ServiceBackoff { failures: 2, until: 123, cross_until: 456 };
        r.actor.service_error_at(task, Error::Unauthorized, Some(start), None);
        let b = r.actor.service_backoff[task as usize];
        assert_eq!((b.failures, b.until, b.cross_until), (2, 123, 456));
        assert_eq!(r.actor.manual_until, manual);
        assert_eq!(r.actor.poll_at, poll_at);
        no_penalty(&r);
    }
}

#[tokio::test]
async fn timed_poll_keeps_local_expiry_separate_from_worker_unauthorized() {
    for worker in [false, true] {
        let mut r = Rig::new().await;
        r.actor.start_poll().unwrap();
        assert!(r.actor.meta_started_at.is_some());
        observe_lapse(&r);
        let flight = r.actor.meta.take().unwrap();
        let done = if worker {
            // Keep the same Flight start while delivering a verified refusal.
            drop(flight);
            MetaDone::Poll(Err((Error::Unauthorized, Some("worker:unauthorized".into()))))
        } else {
            // The expired closure rejects before any HTTP submit/DNS. Exercise
            // the real Poll completion rather than synthesizing its error.
            flight.await
        };
        r.actor.meta_done(done).unwrap();
        if worker {
            assert_eq!(r.actor.service_backoff[ServiceTask::Poll as usize].failures, 1);
            assert!(r.actor.verification.contains(&r.peer));
        } else {
            assert_eq!(r.actor.service_backoff[ServiceTask::Poll as usize].failures, 0);
            no_penalty(&r);
        }
    }
}

#[tokio::test]
async fn members_after_observed_expiry_pull_one_poll_forward_but_routine_members_do_not() {
    let mut r = Rig::new().await;
    r.actor.members_at = now().saturating_sub(1);
    observe_lapse(&r);
    r.actor.poll_at = now() + IDLE_POLL_MS;
    let mut renewed = r.actor.initial.clone();
    renewed.valid_until = now() + 90_000;
    r.actor.members(renewed.clone()).unwrap();
    assert!(r.actor.poll_at <= now() + POLL_MS);
    // Advance only this scheduling deadline; the HTTP future is never polled.
    r.actor.poll_at = now();
    r.actor.needs_refresh = false;
    r.actor.refresh_at = now() + HEARTBEAT_MS;
    r.actor.tick().unwrap();
    assert!(r.actor.meta.is_some());
    let ordinary = r.actor.poll_at;
    r.actor.members(renewed).unwrap();
    assert_eq!(r.actor.poll_at, ordinary);
    no_penalty(&r);
}

#[tokio::test]
async fn service_errors_without_a_call_start_or_with_worker_proof_are_not_exempt() {
    for (started, worker) in [(false, false), (true, true)] {
        let mut r = Rig::new().await;
        let start = now();
        observe_lapse(&r);
        r.actor.service_error_at(ServiceTask::Announce, Error::Unauthorized,
            started.then_some(start), worker.then_some("worker:unauthorized"));
        assert!(r.actor.verification.contains(&r.peer));
        assert_eq!(r.actor.service_backoff[ServiceTask::Announce as usize].failures, 1);
    }
}

#[tokio::test]
async fn peer_guard_records_expiry_only_when_identity_membership_and_epoch_still_match() {
    for case in 0..5 {
        let r = Rig::new().await;
        let guard = r.actor.peer_check(&r.actor.initial.peer(&r.peer).unwrap());
        {
            let mut live = r.actor.authority.write().unwrap();
            live.valid_until = 0;
            match case {
                1 => live.membership.membership_id = "replaced-membership".into(),
                2 => live.identity.public_key = "replaced-identity".into(),
                3 => live.members.retain(|m| m.device_id != r.peer),
                _ => {},
            }
        }
        if case == 4 { r.actor.work.cancel(); }
        assert!(!guard());
        assert_eq!(r.actor.lapse_at.load(Ordering::Acquire) > 0, case == 0);
        if case == 0 {
            let lapse = r.actor.lapse_at.load(Ordering::Acquire);
            r.actor.authority.write().unwrap().valid_until = now() + 90_000;
            assert!(guard(), "renewal does not retire a valid capability");
            assert_eq!(r.actor.lapse_at.load(Ordering::Acquire), lapse);
        }
    }
}
