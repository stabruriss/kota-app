use super::*;
use public::Indicator;

fn event(manager: &Manager, fence: &GroupFence, at: Instant, notice: Notice) {
    manager.notice_at(
        fence,
        0,
        0,
        notice,
        at,
        chrono::DateTime::parse_from_rfc3339("2026-09-17T08:00:00Z")
            .unwrap()
            .to_utc(),
    );
}
fn failed(peer: &str, error: transport::Error) -> Notice {
    Notice::Exchange(exchange::Event::Failed(peer.into(), error))
}
fn finished(peer: &str, outcome: exchange::Outcome) -> Notice {
    Notice::Exchange(exchange::Event::Finished(peer.into(), outcome))
}

#[tokio::test]
async fn recovery_window_survives_reads_manual_progress_and_healthy_control() {
    let (_root, manager, stub, fence) = grouped_fixture();
    let now = Instant::now();
    manager.0.booted.store(true, Ordering::Release);
    let (tx, _rx) = mpsc::sync_channel(1);
    *manager.0.worker.lock().unwrap() = Some(Worker { generation: 1, tx });
    let original = manager.status_at(now);
    assert_eq!(original.sync.indicator, Indicator::Healthy);
    event(
        &manager,
        &fence,
        now,
        failed("peer", transport::Error::Timeout),
    );
    let first = manager.status_at(now);
    assert_eq!(first.sync.indicator, Indicator::Connecting);
    let deadline = now + RECOVERY_DISPLAY_WINDOW;
    assert_eq!(
        manager
            .status_at(deadline - Duration::from_nanos(1))
            .sync
            .indicator,
        Indicator::Connecting
    );
    assert_eq!(
        manager.status_at(deadline).sync.indicator,
        Indicator::ReachingPeers
    );

    for offset in [10, 30, 59, 61, 90] {
        let at = now + Duration::from_secs(offset);
        event(&manager, &fence, at, Notice::Connecting("peer".into()));
        event(
            &manager,
            &fence,
            at,
            failed("peer", transport::Error::Timeout),
        );
        manager
            .start(public::Command {
                expected_group_id: Some(fence.group_id.clone()),
            })
            .await
            .unwrap();
        manager.control_recovered(0);
        let before = manager.status_at(at);
        for _ in 0..100 {
            assert_eq!(manager.status_at(at), before, "card/status reads are pure");
            let _ = manager.diagnostics();
        }
        assert_eq!(
            before.sync.indicator,
            if offset < 60 {
                Indicator::Connecting
            } else {
                Indicator::ReachingPeers
            }
        );
        assert_eq!(
            before.sync.last_successful_at,
            original.sync.last_successful_at
        );
    }
    let at = deadline + Duration::from_secs(31);
    event(
        &manager,
        &fence,
        at,
        Notice::Exchange(exchange::Event::Started("peer".into())),
    );
    let started = manager.status_at(at);
    event(
        &manager,
        &fence,
        at,
        Notice::Exchange(exchange::Event::Progress("peer".into(), 1, 3)),
    );
    let progress = manager.status_at(at);
    assert_eq!(started.sync.indicator, Indicator::ReachingPeers);
    assert_eq!(started.sync.phase, public::Phase::Syncing);
    assert_eq!(started.sync.error, first.sync.error);
    assert_eq!(
        (progress.sync.completed, progress.sync.total),
        (Some(1), Some(3))
    );
    assert_eq!(progress.sync.indicator, Indicator::ReachingPeers);
    assert_eq!(
        progress.sync.last_successful_at,
        original.sync.last_successful_at
    );
    event(
        &manager,
        &fence,
        at,
        finished(
            "peer",
            exchange::Outcome {
                completed: 3,
                total: 3,
                ..Default::default()
            },
        ),
    );
    let recovered = manager.status_at(at);
    assert_eq!(recovered.sync.indicator, Indicator::Healthy);
    assert_eq!(recovered.sync.phase, public::Phase::Idle);
    assert!(recovered.sync.error.is_none());
    assert_eq!(
        recovered.sync.last_successful_at.as_deref(),
        Some("2026-09-17T08:00:00+00:00")
    );
    assert_eq!(stub.calls.load(Ordering::Relaxed), 0);
    manager.shutdown();
}

#[test]
fn incomplete_outcomes_and_other_peers_do_not_discharge_recovery() {
    let (_root, manager, stub, fence) = grouped_fixture();
    let now = Instant::now();
    let mut other = manager.0.view.lock().unwrap().status.group.members[0].clone();
    other.id = "other".into();
    manager
        .0
        .view
        .lock()
        .unwrap()
        .status
        .group
        .members
        .push(other);
    event(
        &manager,
        &fence,
        now,
        failed("peer", transport::Error::RelaySessionLost),
    );
    let at = now + RECOVERY_DISPLAY_WINDOW;
    event(
        &manager,
        &fence,
        at,
        finished("other", exchange::Outcome::default()),
    );
    assert_eq!(
        manager.status_at(at).sync.indicator,
        Indicator::FetchingSession
    );
    assert!(manager.status_at(at).sync.error.is_some());
    let last_success = manager.status_at(at).sync.last_successful_at;
    for outcome in [
        exchange::Outcome {
            more: true,
            ..Default::default()
        },
        exchange::Outcome {
            omitted: 1,
            ..Default::default()
        },
        exchange::Outcome {
            completed: 1,
            total: 2,
            ..Default::default()
        },
        exchange::Outcome {
            failures: vec![public::Failure::checked(
                "peer",
                "source_unavailable",
                None,
                None,
            )],
            ..Default::default()
        },
    ] {
        event(&manager, &fence, at, finished("peer", outcome));
        assert_eq!(
            manager.status_at(at).sync.indicator,
            Indicator::FinishingSync
        );
        assert_eq!(manager.status_at(at).sync.last_successful_at, last_success);
        assert_eq!(
            manager.0.view.lock().unwrap().recovery[&RecoverySource::Peer("peer".into())].since,
            now
        );
    }
    event(
        &manager,
        &fence,
        at,
        finished(
            "peer",
            exchange::Outcome {
                failures: vec![public::Failure::checked(
                    "peer",
                    "sync_integrity_error",
                    None,
                    None,
                )],
                ..Default::default()
            },
        ),
    );
    assert_eq!(
        manager.status_at(at).sync.indicator,
        Indicator::RetryingFiles
    );
    // A healthy heartbeat or a stale/cancelled completion cannot turn this green.
    manager.control_recovered(0);
    manager.0.work.cancel();
    event(
        &manager,
        &fence,
        at,
        finished("peer", exchange::Outcome::default()),
    );
    assert_eq!(
        manager.status_at(at).sync.indicator,
        Indicator::RetryingFiles
    );
    assert_eq!(stub.calls.load(Ordering::Relaxed), 0);
}

#[test]
fn only_evidenced_blockers_bypass_the_recovery_window() {
    for (code, expected) in [
        ("control_in_use", Indicator::OtherInstance),
        ("incomplete_sync_identity", Indicator::DeviceIdentityError),
        ("missing_device_identity", Indicator::DeviceIdentityError),
        ("worker_update_required", Indicator::UpdateWorker),
        ("protocol_mismatch", Indicator::UpdateKota),
        ("cloudflare_resource_limit", Indicator::CloudflareLimit),
    ] {
        let (_root, manager, stub, _) = grouped_fixture();
        manager.error(code);
        assert_eq!(manager.status().sync.indicator, expected, "{code}");
        assert!(!manager.status().sync.service_recoverable);
        assert_eq!(stub.calls.load(Ordering::Relaxed), 0);
    }
    for code in [
        "control_busy",
        "sync_busy",
        "unauthorized",
        "removed",
        "sync_file_io_failed",
    ] {
        let (_root, manager, _, _) = grouped_fixture();
        manager.error(code);
        assert_eq!(
            manager.status().sync.indicator,
            Indicator::Connecting,
            "{code}"
        );
        let since = manager.0.view.lock().unwrap().recovery[&RecoverySource::Control].since;
        assert_eq!(
            manager
                .status_at(since + RECOVERY_DISPLAY_WINDOW)
                .sync
                .indicator,
            Indicator::Reconnecting,
            "{code}"
        );
        assert!(!manager
            .status()
            .sync
            .error
            .unwrap()
            .contains("Another Kota"));
    }
    let (_root, manager, _) = fixture();
    manager.error("not_joined");
    assert_eq!(manager.status().sync.indicator, Indicator::Healthy);
    assert_eq!(manager.status().sync.phase, public::Phase::Idle);
    assert!(manager.status().sync.error.is_none());
}

#[test]
fn service_recovery_survives_control_heartbeat_but_not_completed_business_work() {
    let (_root, manager, _, fence) = grouped_fixture();
    let now = Instant::now();
    event(
        &manager,
        &fence,
        now,
        Notice::Error(String::new(), transport::Error::CloudflareResourceLimit),
    );
    manager.error("control_busy");
    assert_eq!(manager.status().sync.indicator, Indicator::CloudflareLimit);
    manager.control_recovered(0);
    assert_eq!(manager.status().sync.indicator, Indicator::CloudflareLimit);
    event(&manager, &fence, now, Notice::Connecting("peer".into()));
    assert_eq!(manager.status().sync.indicator, Indicator::CloudflareLimit);
    event(
        &manager,
        &fence,
        now,
        finished("peer", exchange::Outcome::default()),
    );
    assert_eq!(manager.status().sync.indicator, Indicator::Healthy);
    assert!(!manager.status().sync.service_recoverable);
}

#[test]
fn control_overlay_does_not_hide_real_sync_progress() {
    let (_root, manager, _, fence) = grouped_fixture();
    let now = Instant::now();
    manager.error("worker_unreachable");
    event(
        &manager,
        &fence,
        now,
        Notice::Exchange(exchange::Event::Started("peer".into())),
    );
    event(
        &manager,
        &fence,
        now,
        Notice::Exchange(exchange::Event::Progress("peer".into(), 2, 3)),
    );
    let status = manager.status_at(Instant::now() + RECOVERY_DISPLAY_WINDOW);
    assert_eq!(status.sync.phase, public::Phase::Syncing);
    assert_eq!(status.sync.indicator, Indicator::ReachingService);
    assert_eq!(
        (status.sync.completed, status.sync.total),
        (Some(2), Some(3))
    );
    assert_eq!(
        status.sync.last_successful_at.as_deref(),
        Some("2000-01-01T00:00:00Z")
    );
    manager.control_recovered(0);
    assert_eq!(manager.status().sync.indicator, Indicator::Healthy);
    assert_eq!(manager.status().sync.phase, public::Phase::Syncing);
}

#[test]
fn recovery_scope_uses_confirmed_membership_not_an_unloaded_snapshot() {
    let (_root, manager, worker) = recovery_fixture();
    let mut client =
        ControlClient::open_existing(manager.0.store.state.clone(), worker.clone(), None).unwrap();
    client.refresh_members(Some("group-test")).unwrap();
    manager.project(&client);
    let fence = GroupFence {
        group_id: "group-test".into(),
        membership_id: "member-test".into(),
    };
    let now = Instant::now();
    event(
        &manager,
        &fence,
        now,
        failed(&worker.members[1].device_id, transport::Error::Timeout),
    );
    let before = manager.status_at(now + RECOVERY_DISPLAY_WINDOW);
    drop(client);
    let mut client =
        ControlClient::open_existing(manager.0.store.state.clone(), worker.clone(), None).unwrap();
    manager.project(&client); // On-disk membership, no remote members response yet.
    assert!(manager.status().group.members.is_empty());
    assert_eq!(
        manager
            .status_at(now + RECOVERY_DISPLAY_WINDOW)
            .sync
            .indicator,
        Indicator::ReachingPeers
    );
    client.refresh_members(Some("group-test")).unwrap();
    manager.project(&client); // Offline presence is not departure or completion.
    assert_eq!(
        manager.status_at(now + RECOVERY_DISPLAY_WINDOW).sync.error,
        before.sync.error
    );
    drop(client);
    let mut departed = worker;
    departed.members.truncate(1);
    let mut client =
        ControlClient::open_existing(manager.0.store.state.clone(), departed.clone(), None)
            .unwrap();
    client.refresh_members(Some("group-test")).unwrap();
    manager.project(&client);
    assert_eq!(manager.status().sync.indicator, Indicator::Healthy);
    assert!(manager.status().sync.error.is_none());
    assert_eq!(
        manager.status().sync.last_successful_at,
        before.sync.last_successful_at
    );
    manager.error("worker_unreachable");
    let emitted = Arc::new(AtomicUsize::new(0));
    let count = emitted.clone();
    *manager.0.emit.lock().unwrap() = Some(Arc::new(move || {
        count.fetch_add(1, Ordering::AcqRel);
    }));
    drop(client);
    let mut saved: Value =
        serde_json::from_slice(&fs::read(manager.0.store.state.control_path()).unwrap()).unwrap();
    saved["current"]["membershipId"] = "new-membership".into();
    crate::bbs_sync::write(&manager.0.store.state.control_path(), &saved).unwrap();
    let client =
        ControlClient::open_existing(manager.0.store.state.clone(), departed, None).unwrap();
    manager.project(&client);
    assert_eq!(manager.status().sync.indicator, Indicator::Healthy);
    assert!(manager.0.view.lock().unwrap().recovery.is_empty());
    assert_eq!(emitted.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn valid_membership_rejection_is_distinct_from_unattributed_unauthorized() {
    #[derive(Clone)]
    struct Rejected;
    impl Transport for Rejected {
        fn send(
            &self,
            _: &str,
            url: &str,
            _: &BTreeMap<String, String>,
            _: &str,
        ) -> Result<RemoteResponse, String> {
            assert!(url.ends_with("/heartbeat"));
            Err("unauthorized".into())
        }
    }
    let (_root, original, _) = recovery_fixture();
    let manager = Manager::services(
        original.0.store.clone(),
        Arc::new(|| Box::new(Rejected)),
        Arc::new(|| None),
    );
    manager.bootstrap();
    wait_until(|| manager.status().sync.indicator == Indicator::GroupAccessDenied).await;
    assert!(
        manager.status().group.id.is_none(),
        "the control client actually retired the denied membership"
    );
    assert!(!manager.status().sync.control_recoverable);
    assert!(manager.status().sync.last_successful_at.is_none());
    println!(
        "KOTA_BBS_MEMBERSHIP_REJECTION_FIXTURE={}",
        serde_json::to_string(&manager.status()).unwrap()
    );
    let saved: Value =
        serde_json::from_slice(&fs::read(manager.0.store.state.control_path()).unwrap()).unwrap();
    assert!(saved["current"].is_null());
    manager.shutdown();
    wait_until(|| manager.0.worker.lock().unwrap().is_none()).await;

    let (_root, manager, _, fence) = grouped_fixture();
    let now = Instant::now();
    event(
        &manager,
        &fence,
        now,
        failed("peer", transport::Error::Unauthorized),
    );
    assert_eq!(manager.status_at(now).sync.indicator, Indicator::Connecting);
    assert_eq!(
        manager
            .status_at(now + RECOVERY_DISPLAY_WINDOW)
            .sync
            .indicator,
        Indicator::Reconnecting
    );
    assert!(manager.status().group.id.is_some());
}

#[tokio::test]
async fn indicator_hint_is_one_passive_wakeup_and_is_cancelled_on_recovery() {
    let (_root, manager, stub, fence) = grouped_fixture();
    let emitted = Arc::new(AtomicUsize::new(0));
    let count = emitted.clone();
    *manager.0.emit.lock().unwrap() = Some(Arc::new(move || {
        count.fetch_add(1, Ordering::AcqRel);
    }));
    let now = Instant::now();
    // Advance only the presentation clock; the real threshold task still runs.
    let since = now - RECOVERY_DISPLAY_WINDOW + Duration::from_millis(150);
    event(
        &manager,
        &fence,
        since,
        failed("peer", transport::Error::Timeout),
    );
    let deadline = since + RECOVERY_DISPLAY_WINDOW;
    for _ in 0..100 {
        let _ = manager.status();
    }
    if let Some(timer) = manager.0.indicator_timer.lock().unwrap().as_ref() {
        assert_eq!(timer.deadline, deadline);
    } // Under load the threshold may already have fired; neither path resets it.
    wait_until(|| emitted.load(Ordering::Acquire) == 2).await;
    assert!(manager.0.indicator_timer.lock().unwrap().is_none());
    assert_eq!(manager.status().sync.indicator, Indicator::ReachingPeers);
    assert_eq!(stub.calls.load(Ordering::Relaxed), 0);
    let last_sync = manager.status().sync.last_successful_at;
    assert_eq!(last_sync.as_deref(), Some("2000-01-01T00:00:00Z"));
    event(
        &manager,
        &fence,
        Instant::now(),
        finished("peer", exchange::Outcome::default()),
    );
    event(
        &manager,
        &fence,
        Instant::now(),
        failed("peer", transport::Error::Timeout),
    );
    assert!(manager.0.indicator_timer.lock().unwrap().is_some());
    event(
        &manager,
        &fence,
        Instant::now(),
        finished("peer", exchange::Outcome::default()),
    );
    assert!(manager.0.indicator_timer.lock().unwrap().is_none());
    event(
        &manager,
        &fence,
        Instant::now(),
        failed("peer", transport::Error::Timeout),
    );
    manager.shutdown();
    assert!(manager.0.indicator_timer.lock().unwrap().is_none());
}

#[test]
fn manager_indicator_fixture_uses_real_projection_with_a_controlled_clock() {
    let (_root, manager, stub, fence) = grouped_fixture();
    let now = Instant::now();
    let healthy = manager.status_at(now);
    event(&manager, &fence, now, Notice::Connecting("peer".into()));
    let connecting = manager.status_at(now);
    event(
        &manager,
        &fence,
        now,
        Notice::Exchange(exchange::Event::Started("peer".into())),
    );
    event(
        &manager,
        &fence,
        now,
        Notice::Exchange(exchange::Event::Progress("peer".into(), 1, 3)),
    );
    let syncing = manager.status_at(now);
    event(
        &manager,
        &fence,
        now,
        failed("peer", transport::Error::RelaySessionLost),
    );
    let recovering = manager.status_at(now);
    let after_window = manager.status_at(now + RECOVERY_DISPLAY_WINDOW);
    event(
        &manager,
        &fence,
        now + RECOVERY_DISPLAY_WINDOW,
        Notice::Exchange(exchange::Event::Started("peer".into())),
    );
    let started_with_debt = manager.status_at(now + RECOVERY_DISPLAY_WINDOW);
    event(
        &manager,
        &fence,
        now + RECOVERY_DISPLAY_WINDOW,
        finished("peer", exchange::Outcome::default()),
    );
    let completed = manager.status_at(now + RECOVERY_DISPLAY_WINDOW);
    let (_root, multiple, _, multiple_fence) = grouped_fixture();
    let mut other = multiple.0.view.lock().unwrap().status.group.members[0].clone();
    other.id = "other".into();
    multiple
        .0
        .view
        .lock()
        .unwrap()
        .status
        .group
        .members
        .push(other);
    event(
        &multiple,
        &multiple_fence,
        now,
        failed("peer", transport::Error::Timeout),
    );
    let multiple_before = multiple.status_at(now + RECOVERY_DISPLAY_WINDOW);
    event(
        &multiple,
        &multiple_fence,
        now + RECOVERY_DISPLAY_WINDOW,
        finished("other", exchange::Outcome::default()),
    );
    let multiple_after = multiple.status_at(now + RECOVERY_DISPLAY_WINDOW);
    assert_eq!(multiple_after.sync.indicator, Indicator::ReachingPeers);
    let mut causes = BTreeMap::new();
    for (name, peer, error) in [
        ("reaching_service", "", transport::Error::Timeout),
        ("reaching_peers", "peer", transport::Error::Closed),
        (
            "fetching_session",
            "peer",
            transport::Error::RelaySessionLost,
        ),
        ("retrying_files", "peer", transport::Error::Integrity),
        ("checking_protocol", "peer", transport::Error::Protocol),
        ("reconnecting", "peer", transport::Error::Io),
        (
            "cloudflare_limit",
            "",
            transport::Error::CloudflareResourceLimit,
        ),
        ("update_kota", "peer", transport::Error::ProtocolVersion),
    ] {
        let (_root, source, _, fence) = grouped_fixture();
        event(&source, &fence, now, Notice::Error(peer.into(), error));
        causes.insert(name, source.status_at(now + RECOVERY_DISPLAY_WINDOW));
    }
    let (_root, partial, _, partial_fence) = grouped_fixture();
    event(
        &partial,
        &partial_fence,
        now,
        finished(
            "peer",
            exchange::Outcome {
                more: true,
                ..Default::default()
            },
        ),
    );
    causes.insert(
        "finishing_sync",
        partial.status_at(now + RECOVERY_DISPLAY_WINDOW),
    );
    for (name, code) in [
        ("other_instance", "control_in_use"),
        ("update_worker", "worker_update_required"),
        ("device_identity_error", "incomplete_sync_identity"),
    ] {
        let (_root, source, _) = fixture();
        source.error(code);
        causes.insert(name, source.status());
    }
    let encoded = serde_json::to_string(&json!({
        "healthy": healthy, "connecting": connecting, "syncing": syncing,
        "recovering": recovering, "afterWindow": after_window,
        "startedWithDebt": started_with_debt, "completed": completed, "causes": causes,
        "multiplePeersBefore": multiple_before, "multiplePeersAfter": multiple_after,
        "busyError": public::Error::from_code("control_busy"),
        "busyDetail": public::display_error("control_busy"),
    }))
    .unwrap();
    for forbidden in [
        "privateKey",
        "requestId",
        "workerUrl",
        "nonce",
        "resetAtUtc",
        "errorScope",
        "errorCode",
    ] {
        assert!(!encoded.contains(forbidden), "{forbidden}");
    }
    for status in causes.values() {
        assert!(!status.sync.service_recoverable);
        assert!(!status.sync.control_recoverable || !status.sync.service_recoverable);
    }
    assert_eq!(stub.calls.load(Ordering::Relaxed), 0);
    println!("KOTA_BBS_INDICATOR_FIXTURE={encoded}");
}
