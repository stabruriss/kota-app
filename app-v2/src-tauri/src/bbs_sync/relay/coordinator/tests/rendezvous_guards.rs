use super::rendezvous::ready_snapshot;
use super::*;
use crate::bbs_sync::transport::APP_QUEUE_BYTES;

fn pending_attempt(r: &Rig, wake: &str, passive: bool) -> Attempt {
    Attempt {
        tag: "synthetic-attempt".into(),
        peer: r.actor.initial.peer(&r.peer).unwrap(),
        instance: "rendezvous-remote-0001".into(),
        until: now() + ACTIVE_MS,
        polls: 0,
        wake: Some(wake.into()),
        nonce: "synthetic-nonce-0001".into(),
        ready_boot: Some("rendezvous-boot-0001".into()),
        handshake: None,
        offered: false,
        waiting_poll: false,
        tls: None,
        cancel: Cancellation::default(),
        passive_boot: passive.then(|| "rendezvous-boot-0001".into()),
        trace: Trace::new(passive, None),
    }
}
async fn recovering() -> Rig {
    let mut r = Rig::new().await;
    for _ in 0..7 {
        r.actor.fail(&r.peer, Error::Timeout);
    }
    r.actor
        .adopt_snapshot(ready_snapshot(&r, now(), &"b".repeat(64), false, true));
    r
}

fn announced_snapshot(r: &Rig) -> Snapshot {
    let mut snapshot = ready_snapshot(r, now(), &"b".repeat(64), false, true);
    let mut own = snapshot.items[&r.peer].clone();
    own.device = r.actor.local.clone();
    own.current_wake = None;
    own.wake = None;
    own.handshake = None;
    let a = own.announcement.as_mut().unwrap();
    a.device = r.actor.local.clone();
    a.membership = r.actor.initial.membership.membership_id.clone();
    a.instance = r.actor.instance.clone();
    snapshot.items.insert(r.actor.local.clone(), own);
    snapshot
}

#[tokio::test]
async fn every_error_and_service_slot_has_an_explicit_discovery_permission() {
    // Production's exhaustive match makes a new enum variant a compile error;
    // this table independently specifies all current variants, including Busy.
    let variants = [
        (Error::InvalidSignal, true),
        (Error::Unauthorized, true),
        (Error::StaleSignature, true),
        (Error::InvalidResource, true),
        (Error::Protocol, true),
        (Error::ProtocolVersion, true),
        (Error::RelaySessionLost, false),
        (Error::CloudflareResourceLimit, true),
        (Error::Integrity, true),
        (Error::Io, true),
        (Error::Busy, true),
        (Error::Closed, false),
        (Error::Cancelled, true),
        (Error::Timeout, false),
        (Error::Runtime, true),
    ];
    let mut r = Rig::new().await;
    for task in [
        ServiceTask::Poll,
        ServiceTask::Announce,
        ServiceTask::Catalog,
        ServiceTask::Exchange,
    ] {
        for (error, blocks) in variants {
            assert_eq!(blocks_discovery(error), blocks);
            r.actor.service_backoff = [ServiceBackoff::default(); 4];
            r.actor.service_error(task, error);
            let until = r.actor.service_retry();
            if error == Error::Cancelled {
                assert_eq!(until, 0);
                continue;
            }
            assert!(until > now());
            assert_eq!(
                r.actor.discovery_retry() > now(),
                blocks || matches!(task, ServiceTask::Poll)
            );
            assert_eq!(
                r.actor.passive_retry() > now(),
                blocks || !matches!(task, ServiceTask::Exchange)
            );
            if error == Error::Busy {
                assert!(until <= now() + POLL_MS);
            }
        }
    }
}

#[tokio::test]
async fn late_ordinary_error_and_healthy_poll_do_not_wash_out_a_platform_deadline() {
    let mut r = Rig::new().await;
    for _ in 0..7 {
        r.actor
            .service_error(ServiceTask::Exchange, Error::CloudflareResourceLimit);
    }
    let platform_until = r.actor.discovery_retry();
    r.actor.service_error(ServiceTask::Exchange, Error::Busy); // old per-slot until shrinks to 2s
    r.actor.service_ok(ServiceTask::Poll);
    assert_eq!(r.actor.discovery_retry(), platform_until);
    r.actor.service_error(ServiceTask::Exchange, Error::Timeout);
    assert!(r.actor.discovery_retry() >= platform_until);
    r.actor.service_ok(ServiceTask::Exchange);
    assert_eq!(r.actor.discovery_retry(), 0);
}

#[tokio::test]
async fn discovery_progresses_during_file_work_but_poll_and_write_failures_keep_their_own_debts() {
    let mut r = Rig::new().await;
    r.actor.file = Some(Box::pin(pending()));
    r.actor.service_error(ServiceTask::Catalog, Error::Timeout);
    let catalog_until = r.actor.service_retry();
    r.actor.poll_at = now();
    r.actor.tick().unwrap();
    assert!(r.actor.meta.is_some() && r.actor.file.is_some());
    r.actor.meta = None; // never polled: no HTTP
    r.actor.service_ok(ServiceTask::Poll);
    assert_eq!(r.actor.service_retry(), catalog_until);
    assert_eq!(r.actor.discovery_retry(), 0);
    r.actor.service_error(ServiceTask::Poll, Error::Timeout);
    r.actor.poll_at = now();
    r.actor.tick().unwrap();
    assert!(r.actor.meta.is_none(), "Poll itself still obeys backoff");
}

#[tokio::test]
async fn remote_ready_not_presence_bare_wake_or_own_ready_is_required() {
    let mut r = recovering().await;
    let wake = "b".repeat(64);
    for (own, remote, allowed) in [
        (false, false, false),
        (true, false, false),
        (false, true, true),
        (true, true, true),
    ] {
        r.actor
            .adopt_snapshot(ready_snapshot(&r, now(), &wake, own, remote));
        assert_eq!(r.actor.candidates(now()).contains(&r.peer), allowed);
    }
    // Offline in the control snapshot is only stale presence, not authority.
    r.actor.schedule.peers.get_mut(&r.peer).unwrap().online = false;
    assert!(r.actor.passive_candidate(&r.peer, now()));
    r.actor.schedule.peers.get_mut(&r.peer).unwrap().needed = false;
    assert!(!r.actor.passive_candidate(&r.peer, now()));
}

#[tokio::test]
async fn invalid_expired_closed_seen_and_wrong_generation_ready_are_rejected() {
    let mut r = recovering().await;
    let at = now();
    let wake = "b".repeat(64);
    for case in 0..7 {
        let mut snapshot = ready_snapshot(&r, at, &wake, false, true);
        let i = snapshot.items.get_mut(&r.peer).unwrap();
        match case {
            0 => i.current_wake = Some("c".repeat(64)),
            1 => i.handshake.as_mut().unwrap().closed = true,
            2 => i.announcement.as_mut().unwrap().instance = "another-process".into(),
            3 => i.announcement.as_mut().unwrap().membership = "another-member".into(),
            4 => i.announcement.as_mut().unwrap().peer_version = 3,
            5 => i.wake = None,
            6 => i.handshake = None,
            _ => unreachable!(),
        }
        r.actor.adopt_snapshot(snapshot);
        assert!(!r.actor.passive_candidate(&r.peer, at), "case {case}");
    }
    r.actor
        .adopt_snapshot(ready_snapshot(&r, at, &wake, false, true));
    assert!(!r.actor.passive_candidate(&r.peer, at + 120_000));
    assert!(r.actor.seen_wakes.consume(&r.peer, &wake, at + 120_000, at));
    assert!(!r.actor.passive_candidate(&r.peer, at));
    r.actor.seen_wakes = Nonces::default();
    r.actor.work.cancel();
    assert!(!r.actor.passive_candidate(&r.peer, at));
    r.actor.sync_epoch();
    assert!(!r.actor.passive_candidate(&r.peer, at));
}

#[tokio::test]
async fn only_recoverable_peer_errors_and_no_service_safety_limit_allow_passive_work() {
    let mut r = recovering().await;
    for error in [
        Error::Busy,
        Error::Io,
        Error::Runtime,
        Error::Unauthorized,
        Error::Protocol,
        Error::Integrity,
        Error::ProtocolVersion,
        Error::CloudflareResourceLimit,
    ] {
        r.actor.recovery.get_mut(&r.peer).unwrap().failure = Some(error);
        assert!(!r.actor.passive_candidate(&r.peer, now()), "{error}");
    }
    r.actor.recovery.get_mut(&r.peer).unwrap().failure = Some(Error::Closed);
    assert!(r.actor.passive_candidate(&r.peer, now()));
    r.actor
        .service_error(ServiceTask::Exchange, Error::CloudflareResourceLimit);
    assert!(!r.actor.passive_candidate(&r.peer, now()));
    r.actor.service_ok(ServiceTask::Exchange);
    r.actor.service_error(ServiceTask::Announce, Error::Timeout);
    assert!(
        !r.actor.passive_candidate(&r.peer, now()),
        "no bypass of unconfirmed writes"
    );
}

#[tokio::test]
async fn a_seen_wake_is_usable_only_by_its_admitted_passive_owner() {
    let mut r = recovering().await;
    let wake = "b".repeat(64);
    assert!(r
        .actor
        .seen_wakes
        .consume(&r.peer, &wake, now() + 120_000, now()));
    r.actor
        .attempts
        .insert(r.peer.clone(), pending_attempt(&r, &wake, true));
    // Both Ready are not yet true: drive returns without HTTP, but must not
    // retire this attempt simply because admission already marked it seen.
    assert_eq!(r.actor.drive_attempt(&r.peer), Ok(()));
    r.actor.attempts.get_mut(&r.peer).unwrap().passive_boot = None;
    assert_eq!(r.actor.drive_attempt(&r.peer), Err(Error::RelaySessionLost));
    r.actor.attempts.get_mut(&r.peer).unwrap().passive_boot = Some("other-boot".into());
    assert_eq!(r.actor.drive_attempt(&r.peer), Err(Error::RelaySessionLost));
    r.actor.attempts.get_mut(&r.peer).unwrap().passive_boot = Some("rendezvous-boot-0001".into());
    r.actor.attempts.get_mut(&r.peer).unwrap().wake = Some("c".repeat(64));
    assert_eq!(r.actor.drive_attempt(&r.peer), Err(Error::RelaySessionLost));
    assert!(r.actor.meta.is_none(), "never make a replacement wake");
}

#[tokio::test]
async fn full_bytes_and_live_attempts_do_not_spend_early_credit() {
    let mut r = recovering().await;
    let hold = r
        .actor
        .context
        .limits
        .reserve(APP_QUEUE_BYTES - 64 * 1024)
        .unwrap();
    assert_eq!(r.actor.start_attempt(&r.peer, now()), Err(Error::Busy));
    assert!(!r.actor.recovery[&r.peer].early.used);
    assert!(!r.actor.seen_wakes.contains(&r.peer, &"b".repeat(64), now()));
    assert!(r.actor.attempts.is_empty() && r.actor.meta.is_none());
    drop(hold);
    for n in 0..4 {
        r.actor.attempts.insert(
            format!("synthetic-slot-{n}"),
            pending_attempt(&r, &"b".repeat(64), false),
        );
    }
    assert_eq!(r.actor.start_attempt(&r.peer, now()), Err(Error::Busy));
    assert!(!r.actor.recovery[&r.peer].early.used);
}

#[tokio::test]
async fn early_failure_new_wake_boot_and_manual_do_not_renew_credit_but_natural_start_does() {
    let mut r = recovering().await;
    r.actor.recovery.get_mut(&r.peer).unwrap().early.used = true;
    let before = r.actor.recovery[&r.peer].early.segment;
    for n in 0..3 {
        r.actor.fail(&r.peer, Error::Timeout);
        r.actor
            .adopt_snapshot(ready_snapshot(&r, now(), &format!("{n:064x}"), false, true));
        assert!(!r.actor.passive_candidate(&r.peer, now()));
        assert_eq!(r.actor.recovery[&r.peer].early.segment, before);
    }
    let mut next_boot = ready_snapshot(&r, now(), &"e".repeat(64), false, true);
    next_boot.boot = "new-rendezvous-boot-0002".into();
    r.actor.adopt_snapshot(next_boot);
    assert!(!r.actor.passive_candidate(&r.peer, now()));
    r.actor
        .adopt_snapshot(ready_snapshot(&r, now(), &"e".repeat(64), false, true));
    let natural = r.actor.recovery[&r.peer].natural_until;
    r.actor.manual();
    r.actor.start_attempt(&r.peer, now()).unwrap(); // no future polled, no HTTP
    assert!(r.actor.recovery[&r.peer].early.used);
    assert_eq!(r.actor.recovery[&r.peer].early.segment, before);
    r.actor.remove(&r.peer);
    r.actor.start_attempt(&r.peer, natural).unwrap();
    assert!(!r.actor.recovery[&r.peer].early.used);
    assert_eq!(r.actor.recovery[&r.peer].early.segment, before + 1);
    r.actor.remove(&r.peer);
    r.actor.fail(&r.peer, Error::Timeout);
    r.actor
        .adopt_snapshot(ready_snapshot(&r, now(), &"d".repeat(64), false, true));
    assert!(r.actor.passive_candidate(&r.peer, now()));
    r.actor.finished(
        &r.peer,
        &exchange::Outcome {
            more: true,
            ..Default::default()
        },
    );
    assert!(
        r.actor.recovery.contains_key(&r.peer),
        "Partial is not a verified success"
    );
    r.actor.finished(&r.peer, &exchange::Outcome::default());
    assert!(!r.actor.recovery.contains_key(&r.peer));
}

#[test]
fn credit_cannot_replenish_itself_across_multiple_virtual_backoff_cycles() {
    let mut r = PeerRecovery::default();
    for cycle in 0..4 {
        let due = (cycle + 1) * 300_000;
        r.natural_until = due;
        r.early.used = true;
        // New wake IDs, boot changes, polls, and elapsed time alone never call
        // active_started. Only an actual normal-start admission does so.
        r.active_started(due - 1); // even explicit Manual just before the due time
        assert!(r.early.used);
        assert_eq!(r.early.segment, cycle);
        r.active_started(due);
        assert!(!r.early.used);
        assert_eq!(r.early.segment, cycle + 1);
    }
}

#[test]
fn transition_log_contains_only_bounded_safe_fields() {
    let trace = Trace::new(true, None);
    let value = trace.value("tls_connected", Some(Error::Timeout), 300_000);
    let object = value.as_object().unwrap();
    let keys = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    assert_eq!(
        keys,
        BTreeSet::from([
            "pid",
            "attempt",
            "mode",
            "stage",
            "elapsed_ms",
            "manual_wait_ms",
            "error",
            "retry_ms"
        ])
    );
    assert_eq!(value["mode"], "passive");
    assert_eq!(value["error"], "sync_timeout");
    assert!(serde_json::to_vec(&value).unwrap().len() < 256);
}

#[tokio::test]
async fn due_active_retry_and_local_admission_do_not_wait_for_or_accelerate_discovery() {
    let mut r = recovering().await;
    let at = now();
    r.actor.poll_at = at + IDLE_POLL_MS;
    r.actor.schedule.peers.get_mut(&r.peer).unwrap().retry_at = at + 5000;
    assert_eq!(r.actor.next_work_deadline(at), at + 5000);
    r.actor.service_error(ServiceTask::Exchange, Error::Timeout);
    r.actor.service_backoff[ServiceTask::Exchange as usize].until = at + 20_000;
    r.actor.poll_at = at + IDLE_POLL_MS;
    assert_eq!(r.actor.next_work_deadline(at), at + 20_000);
    r.actor.admission_at = Some(at + POLL_MS);
    assert_eq!(r.actor.next_work_deadline(at), at + POLL_MS);
    assert_eq!(
        r.actor.poll_at,
        at + IDLE_POLL_MS,
        "local admission wake is not a network poll"
    );
}

#[tokio::test]
async fn passive_admission_byte_backpressure_schedules_only_a_local_retry_and_spends_nothing() {
    let mut r = recovering().await;
    r.actor.adopt_snapshot(announced_snapshot(&r));
    r.actor.needs_refresh = false;
    r.actor.poll_at = now() + IDLE_POLL_MS;
    let poll_at = r.actor.poll_at;
    let _hold = r
        .actor
        .context
        .limits
        .reserve(APP_QUEUE_BYTES - 64 * 1024)
        .unwrap();
    r.actor.tick().unwrap();
    assert!(r.actor.meta.is_none() && r.actor.attempts.is_empty());
    assert!(r.actor.admission_at.is_some_and(|t| t <= now() + POLL_MS));
    assert_eq!(r.actor.poll_at, poll_at);
    assert!(!r.actor.recovery[&r.peer].early.used);
    r.actor.tick().unwrap();
    assert!(r.actor.meta.is_none());
    assert_eq!(r.actor.poll_at, poll_at);
}

#[tokio::test]
async fn healthy_catalog_can_prepare_in_exchange_backoff_but_unconfirmed_outbox_blocks_admission() {
    let mut r = recovering().await;
    for _ in 0..7 {
        r.actor.service_error(ServiceTask::Exchange, Error::Timeout);
    }
    r.actor.adopt_snapshot(announced_snapshot(&r));
    r.actor.poll_at = now() + IDLE_POLL_MS;
    r.actor.tick().unwrap();
    assert!(
        r.actor.file.is_some(),
        "existing FileIo preparation is allowed"
    );
    assert!(r.actor.attempts.is_empty());
    let done = r.actor.file.take().unwrap().await;
    r.actor.file_done(done);
    assert!(r.actor.pending.is_some());
    r.actor.tick().unwrap();
    assert!(r.actor.announcing && r.actor.meta.is_some());
    assert!(
        r.actor.attempts.is_empty(),
        "announcement is not confirmed yet"
    );
    assert!(!r.actor.recovery[&r.peer].early.used);
    // Metadata future is deliberately not polled: no real DNS/HTTP.
}

#[tokio::test]
async fn local_loss_causes_distinguish_boot_instance_and_wake_without_ids() {
    let mut r = Rig::new().await;
    let wake = "b".repeat(64);
    for (case, cause) in [(0, "boot_changed"), (1, "instance_changed"), (2, "wake_replaced")] {
        let mut snapshot = ready_snapshot(&r, now(), &wake, false, true);
        let item = snapshot.items.get_mut(&r.peer).unwrap();
        match case {
            0 => snapshot.boot = "replacement-boot".into(),
            1 => item.announcement.as_mut().unwrap().instance = "replacement-instance".into(),
            _ => item.current_wake = Some("c".repeat(64)),
        }
        r.actor.snapshot = Some(snapshot);
        r.actor.attempts.insert(r.peer.clone(), pending_attempt(&r, &wake, true));
        assert_eq!(r.actor.drive_attempt(&r.peer), Err(Error::RelaySessionLost));
        assert_eq!(r.actor.failure_cause(&r.peer, Error::RelaySessionLost), Some(cause));
        let trace = &r.actor.attempts[&r.peer].trace;
        let value = trace.failure_value(Error::RelaySessionLost, Some(cause));
        assert_eq!(value["cause"], cause);
        let text = value.to_string();
        for private in [r.peer.as_str(), wake.as_str(), "replacement-boot", "replacement-instance"] {
            assert!(!text.contains(private));
        }
    }
}
