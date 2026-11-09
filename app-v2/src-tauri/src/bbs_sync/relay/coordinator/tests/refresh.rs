use super::*;
use serde_json::json;

async fn inspected(r: &mut Rig, expected_change: bool) {
    let done = r.actor.file.take().expect("periodic input check queued").await;
    assert!(matches!(&done, FileDone::Inspected(Ok(changed)) if *changed == expected_change));
    r.actor.file_done(done);
}

#[tokio::test]
async fn periodic_unchanged_inputs_do_not_rebuild_catalog_or_announce() {
    let mut r = synced().await;
    let before = r.actor.engine.revision().unwrap();
    let path = r.actor.engine.store.state.state_path();
    use std::os::unix::fs::MetadataExt;
    let stamp = || std::fs::metadata(&path).ok().map(|m| (m.ino(), m.len(), m.mtime(), m.mtime_nsec()));
    let original = stamp();
    for _ in 0..3 {
        r.actor.refresh_at = now();
        r.actor.tick().unwrap();
        inspected(&mut r, false).await;
        assert!(!r.actor.needs_refresh);
        assert_eq!(r.actor.engine.revision().unwrap(), before);
        assert_eq!(stamp(), original);
        assert_idle(&mut r);
        assert!(r.actor.file.is_none());
    }
}

#[tokio::test]
async fn periodic_stat_fallback_sees_cli_post_even_without_marker_or_state_write() {
    let mut r = synced().await;
    let raw = exchange::tests::publish(&r.actor.engine.store, "initial shared post");
    let mut state = r.actor.engine.store.state.load_state().unwrap().unwrap();
    let group = state.groups.remove("group-test").unwrap();
    state.groups.insert(r.actor.engine.fence.group_id.clone(), group);
    r.actor.engine.store.state.save_state(&state).unwrap();
    r.actor.prepare();
    prepared(&mut r).await;
    confirm_announcement(&mut r).await;
    r.actor.finished(&r.peer, &exchange::Outcome::default());
    r.actor.prepare();
    prepared(&mut r).await;
    assert_idle(&mut r);
    let old_revision = r.actor.engine.revision().unwrap();
    let state_path = r.actor.engine.store.state.state_path();
    let old_state = std::fs::read(&state_path).unwrap();
    let serial = r.actor.work.serial();
    let reply = String::from_utf8(raw).unwrap()
        .replace("postId: root", "postId: reply")
        .replace("kind: topic", "kind: reply")
        .replace("attachments/root/file.txt", "attachments/reply/file.txt")
        .replace("initial shared post", "external CLI reply");
    std::fs::write(r.actor.engine.store.root().join("threads/thread-one/posts/reply.md"), reply).unwrap();
    assert_eq!(std::fs::read(state_path).unwrap(), old_state);
    assert_eq!(r.actor.work.serial(), serial);
    r.actor.refresh_at = now();
    r.actor.tick().unwrap();
    inspected(&mut r, true).await;
    assert!(r.actor.needs_refresh);
    r.actor.tick().unwrap();
    prepared(&mut r).await;
    assert_ne!(r.actor.engine.revision().unwrap(), old_revision);
    assert!(r.actor.pending.is_some(), "external post gets an announcement");
}

#[tokio::test]
async fn periodic_inspection_checks_roster_version_and_cancel_still_resumes() {
    let mut r = synced().await;
    r.actor.engine.roster.as_ref().unwrap().test_publish_source(vec![roster::Project {
        project_id: "fixture-project".into(), name: "External roster".into(), agents: vec![],
    }]);
    r.actor.refresh_at = now();
    r.actor.tick().unwrap();
    inspected(&mut r, true).await;
    r.actor.work.cancel();
    r.actor.sync_epoch();
    assert!(!r.actor.needs_refresh && !r.actor.needs_inspection);
    r.actor.refresh_at = now();
    r.actor.tick().unwrap();
    prepared(&mut r).await;
    assert!(r.actor.pending.is_some(), "new instance publishes after the cancel quiet period");
}

async fn prepared(r: &mut Rig) {
    let done = r.actor.file.take().expect("catalog refresh queued").await;
    assert!(matches!(&done, FileDone::Prepared(Ok(_))));
    r.actor.file_done(done);
}

async fn confirm_announcement(r: &mut Rig) {
    let intent = r.actor.pending.take().expect("new publication");
    let receipt = Receipt::decode(
        &serde_json::to_vec(&json!({
            "ok": true, "current": intent.request_id(), "changed": true,
        }))
        .unwrap(),
    )
    .unwrap();
    let completion = r
        .actor
        .outbox
        .complete(&intent, receipt, now(), &r.actor.cancel)
        .await;
    assert!(matches!(&completion, Ok(Completion::Confirmed)));
    r.actor.file_done(FileDone::Confirmed(completion));
    r.actor.poll_at = now() + IDLE_POLL_MS;
}

async fn synced() -> Rig {
    let mut r = Rig::new().await;
    r.actor.prepare();
    prepared(&mut r).await;
    confirm_announcement(&mut r).await;
    let revision = r.actor.engine.revision().unwrap();
    exchange::tests::confirm_peer_catalog(&r.actor.engine, &r.peer, &"a".repeat(64));
    r.actor.finished(&r.peer, &exchange::Outcome::default());
    r.actor.prepare();
    prepared(&mut r).await;
    assert!(r.actor.pending.is_none());

    let mut snapshot = rendezvous::ready_snapshot(&r, now(), &"b".repeat(64), false, false);
    let remote = snapshot.items.get_mut(&r.peer).unwrap();
    remote.current_wake = None;
    remote.wake = None;
    remote.handshake = None;
    let mut local = remote.clone();
    local.device = r.actor.local.clone();
    let announcement = local.announcement.as_mut().unwrap();
    announcement.device = r.actor.local.clone();
    announcement.membership = r.actor.initial.membership.membership_id.clone();
    announcement.instance = r.actor.instance.clone();
    announcement.revision = revision;
    snapshot.items.insert(r.actor.local.clone(), local);
    r.actor.adopt_snapshot(snapshot);
    r.actor.refresh_at = now() + HEARTBEAT_MS;
    assert!(!r.actor.schedule.peers[&r.peer].needed);
    assert!(!r.actor.engine.full_catalog_due(&r.peer));
    r
}

async fn changed(r: &mut Rig) {
    r.actor.work.changed();
    r.actor.tick().unwrap();
    assert!(r.actor.schedule.changed_at.is_some());
    // Advance the existing debounce, without sleeping or polling HTTP futures.
    r.actor.schedule.changed_at = Some(now());
    r.actor.tick().unwrap();
    assert!(
        r.actor.attempts.is_empty(),
        "refresh must precede a new round"
    );
    prepared(r).await;
}

fn assert_idle(r: &mut Rig) {
    assert!(r.actor.pending.is_none());
    assert!(r.actor.forced.is_empty());
    assert!(!r.actor.schedule.peers[&r.peer].needed);
    r.actor.tick().unwrap();
    assert!(
        r.actor.attempts.is_empty() && r.actor.meta.is_none(),
        "unchanged hints must not start a wake or accelerate idle polling"
    );
}

async fn assert_aged_snapshot_refreshes_before_starting(age_ms: u64) {
    let mut r = synced().await;
    r.actor.forced.insert(r.peer.clone());
    r.actor.refresh_needed();
    r.actor.snapshot_at = now() - age_ms;
    // Exercise the jitter/phase gap before the next ordinary idle read.
    r.actor.poll_at = now() + 2_000;
    let began = Instant::now();
    r.actor.tick().unwrap();
    assert!(r.actor.meta.is_some() && r.actor.discovery_stale);
    assert!(r.actor.attempts.is_empty());
    assert!(r.actor.losses.is_empty() && r.actor.verification.is_empty());

    // Supply the next discovery response, as if the idle relay boot changed.
    // This is a scheduling test, not an HTTP/TLS latency measurement.
    r.actor.meta.take();
    let fresh = Snapshot {
        boot: "fresh-after-idle-boot".into(),
        items: r.actor.snapshot.as_ref().unwrap().items.clone(),
    };
    r.actor.meta_done(MetaDone::Poll(Ok(fresh))).unwrap();
    r.actor.tick().unwrap();
    assert!(r.actor.attempts.contains_key(&r.peer));
    assert!(!r.actor.discovery_stale);
    assert!(r.actor.losses.is_empty() && r.actor.verification.is_empty());
    assert_eq!(r.actor.schedule.peers[&r.peer].retry_at, 0);
    assert!(!r.notices.lock().unwrap().iter().any(|n| matches!(n, Notice::Error(_, _))));
    assert!(began.elapsed() < Duration::from_secs(2), "no idle-period delay before starting");
    println!("BBS_FRESH_START_GATE elapsed_ms={} old_age_ms={} peer_version=4 simulated_discovery=true",
        began.elapsed().as_millis(), age_ms);
}

#[tokio::test]
async fn aged_snapshot_polls_before_starting_without_charging_a_boot_loss() {
    assert_aged_snapshot_refreshes_before_starting(POLL_MS + 1).await;
}

#[tokio::test]
async fn fifteen_second_snapshot_refreshes_before_a_sleeping_worker_can_cost_an_attempt() {
    assert_aged_snapshot_refreshes_before_starting(15_000).await;
}

#[tokio::test]
async fn aged_idle_snapshot_does_not_accelerate_polling_without_work() {
    let mut r = synced().await;
    r.actor.snapshot_at = now() - IDLE_POLL_MS - 1;
    let poll_at = now() + 2_000;
    r.actor.poll_at = poll_at;
    assert_idle(&mut r);
    assert_eq!(r.actor.poll_at, poll_at);
    assert!(!r.actor.discovery_stale);
}

#[tokio::test]
async fn unshared_content_markers_do_not_open_relay_rounds() {
    let mut r = synced().await;
    let before = r.actor.engine.revision().unwrap();
    exchange::tests::publish(&r.actor.engine.store, "unshared local post");
    let mut state = r.actor.engine.store.state.load_state().unwrap().unwrap();
    state.groups.clear();
    r.actor.engine.store.state.save_state(&state).unwrap();
    r.actor.engine.store.state.mark_content_changed();
    for _ in 0..3 {
        changed(&mut r).await;
        assert_eq!(r.actor.engine.revision().unwrap(), before);
        assert_idle(&mut r);
    }
}

#[tokio::test]
async fn unchanged_roster_and_rescan_hints_do_not_open_relay_rounds() {
    let mut r = synced().await;
    let before = r.actor.engine.revision().unwrap();
    r.actor
        .engine
        .roster
        .as_ref()
        .unwrap()
        .test_publish_source(vec![]);
    // Roster callbacks and need_rescan both converge on this same Work hint.
    for _ in 0..2 {
        changed(&mut r).await;
        assert_eq!(r.actor.engine.revision().unwrap(), before);
        assert_idle(&mut r);
    }
}

#[tokio::test]
async fn shared_content_and_real_roster_changes_still_require_rounds() {
    for roster_change in [false, true] {
        let mut r = synced().await;
        let before = r.actor.engine.revision().unwrap();
        if roster_change {
            r.actor
                .engine
                .roster
                .as_ref()
                .unwrap()
                .test_publish_source(vec![roster::Project {
                    project_id: "synthetic-project".into(),
                    name: "Fixture".into(),
                    agents: vec![],
                }]);
        } else {
            exchange::tests::publish(&r.actor.engine.store, "shared post");
            let mut state = r.actor.engine.store.state.load_state().unwrap().unwrap();
            let shared = state.groups.remove("group-test").unwrap();
            state
                .groups
                .insert(r.actor.engine.fence.group_id.clone(), shared);
            r.actor.engine.store.state.save_state(&state).unwrap();
        }
        changed(&mut r).await;
        assert_ne!(r.actor.engine.revision().unwrap(), before);
        assert!(r.actor.pending.is_some());
        assert!(r.actor.forced.contains(&r.peer));
        assert!(r.actor.schedule.peers[&r.peer].needed);
        // A duplicate hint during publication must not lose outstanding work.
        r.actor.prepare();
        prepared(&mut r).await;
        assert!(r.actor.pending.is_some() && r.actor.forced.contains(&r.peer));
        confirm_announcement(&mut r).await;
        assert!(r.actor.candidates(now()).contains(&r.peer));
        r.actor.finished(&r.peer, &exchange::Outcome::default());
        r.actor.prepare();
        prepared(&mut r).await;
        assert_idle(&mut r);
    }
}

#[tokio::test]
async fn unchanged_refresh_preserves_manual_recovery_remote_wake_and_full_repair() {
    for cause in ["manual", "recovery", "remote", "wake", "full"] {
        let mut r = synced().await;
        match cause {
            "manual" => {
                r.actor.manual();
                r.actor.poll_at = now() + IDLE_POLL_MS;
                r.actor.prepare();
                prepared(&mut r).await;
            }
            "recovery" => r.actor.fail(&r.peer, Error::Timeout),
            "remote" => {
                let mut snapshot = r.actor.snapshot.take().unwrap();
                snapshot
                    .items
                    .get_mut(&r.peer)
                    .unwrap()
                    .announcement
                    .as_mut()
                    .unwrap()
                    .revision = "c".repeat(64);
                r.actor.adopt_snapshot(snapshot);
            }
            "wake" => {
                let fresh = rendezvous::ready_snapshot(&r, now(), &"d".repeat(64), false, true);
                let mut snapshot = r.actor.snapshot.take().unwrap();
                snapshot
                    .items
                    .insert(r.peer.clone(), fresh.items[&r.peer].clone());
                r.actor.adopt_snapshot(snapshot);
            }
            "full" => r.actor.engine.force_full_catalogs(),
            _ => unreachable!(),
        }
        let retry = r.actor.schedule.peers[&r.peer].retry_at;
        // Keep the actor from opening until the unchanged refresh has finished.
        r.actor.work.changed();
        r.actor.serial = r.actor.work.serial();
        r.actor.schedule.changed_at = Some(now());
        r.actor.tick().unwrap();
        prepared(&mut r).await;
        assert!(r.actor.pending.is_none(), "{cause}: no new publication");
        assert!(
            r.actor.schedule.peers[&r.peer].needed,
            "{cause}: still owes a round"
        );
        assert_eq!(r.actor.schedule.peers[&r.peer].retry_at, retry);
        let reason = r.actor.round_reason(&r.peer, now());
        let flag = match cause {
            "manual" => "forced",
            "recovery" => "verification",
            "remote" => "revision_mismatch",
            "wake" => "fresh_wake",
            "full" => "full_catalog_due",
            _ => unreachable!(),
        };
        assert_eq!(reason[flag], true, "{cause}: explain the admitted work");
        assert!(reason.as_object().unwrap().values().all(|v| v.is_boolean()));
        if cause == "recovery" {
            assert!(r.actor.verification.contains(&r.peer));
            assert!(
                r.actor.candidates(now()).is_empty(),
                "backoff still applies"
            );
        } else {
            assert!(r.actor.candidates(now()).contains(&r.peer), "{cause}");
        }
    }
}

#[tokio::test]
async fn completed_wake_stays_consumed_during_close_and_refresh() {
    for retire_first in [false, true] {
        let mut r = synced().await;
        let wake = "d".repeat(64);
        let session = "e".repeat(64);
        let fresh = rendezvous::ready_snapshot(&r, now(), &wake, true, true);
        let mut snapshot = r.actor.snapshot.take().unwrap();
        snapshot
            .items
            .insert(r.peer.clone(), fresh.items[&r.peer].clone());
        r.actor.adopt_snapshot(snapshot);
        r.actor.connected.insert(
            r.peer.clone(),
            Connected {
                peer: r.actor.initial.peer(&r.peer).unwrap(),
                instance: "rendezvous-remote-0001".into(),
                session: session.clone(),
                wake: wake.clone(),
                link: None,
                finished: false,
                closing_at: None,
                trace: Trace::new(false, None),
            },
        );
        r.actor.schedule.connected(&r.peer, now());
        r.actor.verification.insert(r.peer.clone());
        r.actor.finished(&r.peer, &exchange::Outcome::default());
        assert!(r.actor.seen_wakes.contains(&r.peer, &wake, now()));
        assert!(!r.actor.verification.contains(&r.peer));
        if retire_first {
            r.actor
                .activity_event(Ok(ActivityEvent::Retired {
                    session: session.clone(),
                    error: Error::Closed,
                }))
                .unwrap();
        }

        // Exercise both completion paths before a new discovery result arrives:
        // the old snapshot still advertises an open, unexpired wake.
        r.actor.prepare();
        prepared(&mut r).await;
        let snapshot = r.actor.snapshot.take().unwrap();
        r.actor.adopt_snapshot(snapshot);
        assert_idle(&mut r);
        r.actor
            .activity_event(Ok(ActivityEvent::Retired {
                session,
                error: Error::Closed,
            }))
            .unwrap();
        r.actor.event(
            u64::MAX,
            exchange::Event::Failed(r.peer.clone(), Error::Closed),
        );
        assert_idle(&mut r);
        assert!(r.actor.verification.is_empty() && r.actor.losses.is_empty());

        let fresh = rendezvous::ready_snapshot(&r, now(), &"f".repeat(64), false, true);
        let mut snapshot = r.actor.snapshot.take().unwrap();
        snapshot
            .items
            .insert(r.peer.clone(), fresh.items[&r.peer].clone());
        r.actor.adopt_snapshot(snapshot);
        assert!(
            r.actor.schedule.peers[&r.peer].needed,
            "a different wake still needs a round"
        );
        assert_eq!(r.actor.round_reason(&r.peer, now())["fresh_wake"], true);
    }
}

#[tokio::test]
async fn successful_install_hint_does_not_repeat_an_unchanged_catalog() {
    let mut r = synced().await;
    r.actor.finished(
        &r.peer,
        &exchange::Outcome {
            completed: 1,
            total: 1,
            ..Default::default()
        },
    );
    assert!(r.actor.schedule.changed_at.is_some());
    r.actor.schedule.changed_at = Some(now());
    r.actor.tick().unwrap();
    prepared(&mut r).await;
    assert_idle(&mut r);
}

#[tokio::test]
async fn incomplete_round_survives_an_unchanged_refresh_and_keeps_backoff() {
    for outcome in [
        exchange::Outcome {
            more: true,
            ..Default::default()
        },
        // Reported item failures settle the round (isolation); a result that
        // accounts for fewer items than it planned does not.
        exchange::Outcome {
            total: 1,
            ..Default::default()
        },
    ] {
        let mut r = synced().await;
        r.actor.finished(&r.peer, &outcome);
        let retry = r.actor.schedule.peers[&r.peer].retry_at;
        r.actor.prepare();
        prepared(&mut r).await;
        assert!(r.actor.pending.is_none());
        assert!(r.actor.verification.contains(&r.peer));
        assert!(r.actor.schedule.peers[&r.peer].needed);
        assert_eq!(r.actor.schedule.peers[&r.peer].retry_at, retry);
        assert_eq!(r.actor.round_reason(&r.peer, now())["verification"], true);
        r.actor.finished(&r.peer, &exchange::Outcome::default());
        r.actor.prepare();
        prepared(&mut r).await;
        assert_idle(&mut r);
    }
}
