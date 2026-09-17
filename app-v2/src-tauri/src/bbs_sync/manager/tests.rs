use super::*;
use crate::bbs_sync::{
    control::{RemoteResponse, Transport},
    raw_sha256, DeviceIdentity,
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, path::PathBuf, sync::atomic::AtomicUsize};
mod indicator;
struct Root(PathBuf);
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[derive(Clone)]
struct Stub {
    calls: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<Value>>>,
}
impl Transport for Stub {
    fn send(
        &self,
        _: &str,
        url: &str,
        _: &BTreeMap<String, String>,
        body: &str,
    ) -> Result<RemoteResponse, String> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let value: Value = serde_json::from_str(body).unwrap();
        self.requests.lock().unwrap().push(value.clone());
        if url.ends_with("/bbs/join") {
            return serde_json::from_value(json!({"protocolVersion":1,"ok":true,"groupId":"group-test","role":"member","membershipId":value["requestId"]})).map_err(|_|"bad_stub".into());
        }
        Err("worker_unreachable".into())
    }
}
fn fixture() -> (Root, Manager, Stub) {
    let root = Root(std::env::temp_dir().join(format!("bbs-manager-{}", uuid::Uuid::new_v4())));
    let store = ContentStore::at(root.0.join("bbs"), root.0.join("account"));
    let stub = Stub {
        calls: Arc::new(AtomicUsize::new(0)),
        requests: Arc::new(Mutex::new(vec![])),
    };
    let factory = stub.clone();
    let m = Manager::services(
        store,
        Arc::new(move || Box::new(factory.clone())),
        Arc::new(|| None),
    );
    (root, m, stub)
}
async fn wait_until(test: impl Fn() -> bool) {
    let end = std::time::Instant::now() + Duration::from_secs(4);
    while !test() {
        assert!(std::time::Instant::now() < end);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
fn grouped_fixture() -> (Root, Manager, Stub, GroupFence) {
    let (root, manager, stub) = fixture();
    let fence = GroupFence {
        group_id: "group-one".into(),
        membership_id: "membership-one".into(),
    };
    {
        let mut view = manager.0.view.lock().unwrap();
        view.fence = Some((fence.group_id.clone(), fence.membership_id.clone()));
        view.status.device.id = "device-one".into();
        view.status.group.id = Some(fence.group_id.clone());
        view.status.group.role = Some(control::Role::Member);
        view.status.group.members.push(public::PublicMember {
            id: "peer".into(),
            name: "Peer".into(),
            role: control::Role::Owner,
            online: true,
            public_key: "public-only".into(),
        });
        view.status.sync.last_successful_at = Some("2000-01-01T00:00:00Z".into());
    }
    manager.notice(&fence, 0, 0, Notice::Connected("peer".into()));
    (root, manager, stub, fence)
}

#[tokio::test]
async fn invitation_reports_safe_worker_upgrade_code_and_preserves_pending() {
    struct OldWorker;
    impl Transport for OldWorker {
        fn send(
            &self,
            _: &str,
            _: &str,
            _: &BTreeMap<String, String>,
            _: &str,
        ) -> Result<RemoteResponse, String> {
            Err("worker_update_required".into())
        }
    }
    let (_root, original, _) = fixture();
    let manager = Manager::services(
        original.0.store.clone(),
        Arc::new(|| Box::new(OldWorker)),
        Arc::new(|| {
            Some(
                OwnerConnection::from_paired(&crate::laughing_man::LmStandbyConfig {
                    worker_url: "https://fixture.invalid".into(),
                    desktop_secret: "DO-NOT-EXPOSE".into(),
                    paired_at: "2026-09-12T00:00:00Z".into(),
                    ..Default::default()
                })
                .unwrap(),
            )
        }),
    );
    manager.bootstrap();
    let error = manager
        .invitation(public::InvitationRequest {
            expected_group_id: None,
            refresh: false,
        })
        .await
        .unwrap_err();
    let safe = serde_json::to_string(&error).unwrap();
    assert_eq!(safe, r#"{"code":"worker_update_required"}"#);
    assert_eq!(
        manager.diagnostics().last_error.as_deref(),
        Some("worker_update_required")
    );
    let status = manager.status();
    assert_eq!(status.sync.phase, public::Phase::Failed);
    assert_eq!(
        status.sync.error.as_deref(),
        Some("Worker is outdated; update it from the Laughing Man card and retry.")
    );
    assert!(status.group.id.is_none());
    let saved: Value =
        serde_json::from_slice(&fs::read(manager.0.store.state.control_path()).unwrap()).unwrap();
    assert_eq!(saved["pending"]["kind"], "create");
    assert!(saved["current"].is_null());
    assert!(saved["pending"]["requestId"].is_string());
    let fixture = json!({"error":error,"status":status});
    let bytes = serde_json::to_string(&fixture).unwrap();
    assert!(!bytes.contains("DO-NOT-EXPOSE"));
    assert!(!bytes.contains("fixture.invalid"));
    assert!(!bytes.contains("requestId"));
    println!("KOTA_BBS_WORKER_UPGRADE_FIXTURE={bytes}");
    manager.shutdown();
    wait_until(|| manager.0.worker.lock().unwrap().is_none()).await;
}

#[test]
fn finished_distinguishes_empty_success_all_failed_incomplete_and_warning_only() {
    let failed_item = public::Failure::checked(
        "local",
        "resource_install_failed",
        Some("thread"),
        Some("post"),
    );
    let cases = [
        (
            "nothing to transfer",
            exchange::Outcome::default(),
            public::Phase::Idle,
        ),
        (
            "all attempted tasks failed",
            exchange::Outcome {
                total: 2,
                failures: vec![failed_item.clone()],
                ..Default::default()
            },
            public::Phase::Failed,
        ),
        (
            "yielded before any success",
            exchange::Outcome {
                total: 2,
                more: true,
                ..Default::default()
            },
            public::Phase::Partial,
        ),
        (
            "creation-record warning only",
            exchange::Outcome {
                failures: vec![public::Failure::checked(
                    "peer",
                    "thread_record_mismatch",
                    Some("thread"),
                    None,
                )],
                ..Default::default()
            },
            public::Phase::Partial,
        ),
        (
            "success and failure",
            exchange::Outcome {
                completed: 1,
                total: 2,
                failures: vec![failed_item],
                ..Default::default()
            },
            public::Phase::Partial,
        ),
        (
            "all succeeded",
            exchange::Outcome {
                completed: 2,
                total: 2,
                ..Default::default()
            },
            public::Phase::Idle,
        ),
    ];
    for (label, outcome, expected) in cases {
        let (_root, manager, stub, fence) = grouped_fixture();
        manager.notice(
            &fence,
            0,
            0,
            Notice::Exchange(exchange::Event::Finished("peer".into(), outcome.clone())),
        );
        let status = manager.status();
        assert_eq!(status.sync.phase, expected, "{label}");
        assert_eq!(
            (status.sync.completed, status.sync.total),
            (Some(outcome.completed), Some(outcome.total)),
            "{label}"
        );
        assert_eq!(manager.diagnostics().failures, outcome.failures, "{label}");
        assert_eq!(
            manager.diagnostics().connections[0].state,
            "connected",
            "task failure does not break a healthy connection: {label}"
        );
        if expected == public::Phase::Idle {
            let time = status.sync.last_successful_at.as_deref().unwrap();
            assert_ne!(time, "2000-01-01T00:00:00Z", "{label}");
            assert!(chrono::DateTime::parse_from_rfc3339(time).is_ok());
        } else {
            assert_eq!(
                status.sync.last_successful_at.as_deref(),
                Some("2000-01-01T00:00:00Z"),
                "{label}"
            );
        }
        if expected == public::Phase::Failed {
            assert_eq!(
                status.sync.error,
                Some(public::display_error("sync_item_failed"))
            );
        }
        assert_eq!(stub.calls.load(Ordering::Relaxed), 0);
    }
}

#[test]
fn progress_projects_directional_counts_before_finished_summary() {
    let (_root, manager, stub, fence) = grouped_fixture();
    manager.notice(
        &fence,
        0,
        0,
        Notice::Exchange(exchange::Event::Started("peer".into())),
    );
    assert_eq!(
        (manager.status().sync.completed, manager.status().sync.total),
        (None, None)
    );
    let mut progress_states = Vec::new();
    for completed in [0, 1, 3] {
        manager.notice(
            &fence,
            0,
            0,
            Notice::Exchange(exchange::Event::Progress("peer".into(), completed, 3)),
        );
        let status = manager.status();
        assert_eq!(status.sync.phase, public::Phase::Syncing);
        assert_eq!(
            (status.sync.completed, status.sync.total),
            (Some(completed), Some(3))
        );
        assert_eq!(
            status.sync.last_successful_at.as_deref(),
            Some("2000-01-01T00:00:00Z")
        );
        progress_states.push(status);
    }
    println!(
        "KOTA_BBS_SYNC_MANAGER_PROGRESS_FIXTURE={}",
        serde_json::to_string(&progress_states).unwrap()
    );
    manager.notice(
        &fence,
        0,
        0,
        Notice::Exchange(exchange::Event::Finished(
            "peer".into(),
            exchange::Outcome {
                completed: 5,
                total: 5,
                ..Default::default()
            },
        )),
    );
    assert_eq!(
        (manager.status().sync.completed, manager.status().sync.total),
        (Some(5), Some(5))
    );
    assert_eq!(manager.status().sync.phase, public::Phase::Idle);
    assert_eq!(stub.calls.load(Ordering::Relaxed), 0);
}

#[test]
fn diagnostic_states_and_late_group_results_stay_bounded_and_memory_only() {
    let (_root, m, stub) = fixture();
    let fence = GroupFence {
        group_id: "group-one".into(),
        membership_id: "membership-one".into(),
    };
    {
        let mut v = m.0.view.lock().unwrap();
        v.fence = Some((fence.group_id.clone(), fence.membership_id.clone()));
        v.status.group.members.push(public::PublicMember {
            id: "peer".into(),
            name: "Peer".into(),
            role: control::Role::Member,
            online: true,
            public_key: "public-only".into(),
        });
    }
    m.notice(&fence, 0, 0, Notice::Connecting("peer".into()));
    assert_eq!(m.diagnostics().connections[0].state, "connecting");
    m.notice(&fence, 0, 0, Notice::Connected("peer".into()));
    assert_eq!(m.diagnostics().connections[0].state, "connected");
    m.notice(
        &fence,
        0,
        0,
        Notice::Error("peer".into(), transport::Error::Busy),
    );
    assert_ne!(m.status().sync.phase, public::Phase::Failed);
    m.notice(&fence, 0, 0, Notice::Disconnected("peer".into()));
    assert_eq!(m.diagnostics().connections[0].state, "disconnected");
    m.0.network_epoch.store(1, Ordering::Release);
    m.notice(&fence, 0, 0, Notice::Connected("peer".into()));
    assert_eq!(m.diagnostics().connections[0].state, "disconnected");
    let wrong = GroupFence {
        membership_id: "new-membership".into(),
        ..fence.clone()
    };
    m.notice(
        &wrong,
        1,
        0,
        Notice::Error("peer".into(), transport::Error::Timeout),
    );
    assert!(m.diagnostics().last_error.is_none());
    for n in 0..100 {
        m.notice(&fence, 1, 0, Notice::Connecting(format!("unknown-{n}")));
    }
    assert_eq!(m.diagnostics().connections.len(), 1);
    assert_eq!(stub.calls.load(Ordering::Relaxed), 0);
    assert!(!m.0.store.state.identity_path().parent().unwrap().exists());
}
#[tokio::test]
async fn fresh_status_panel_reads_and_bootstrap_create_no_sync_files_or_owned_workers() {
    let (_root, m, stub) = fixture();
    for _ in 0..100 {
        assert!(m.status().device.id.is_empty());
        let _ = m.diagnostics();
    }
    m.bootstrap();
    for _ in 0..100 {
        let _ = m.status();
    }
    assert!(!m.0.store.state.identity_path().parent().unwrap().exists());
    assert!(m.0.worker.lock().unwrap().is_none());
    assert!(m.0.network.lock().unwrap().is_none());
    assert!(m.0.relay.lock().unwrap().is_none());
    assert_eq!(stub.calls.load(Ordering::Relaxed), 0);
    let error = m
        .start(public::Command {
            expected_group_id: None,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, "not_joined");
    assert!(!m.0.store.state.identity_path().parent().unwrap().exists());
}

#[tokio::test]
async fn actual_roster_ipc_is_memory_only_and_avatar_read_is_device_scoped_with_shared_admission() {
    let (_root, manager, stub) = fixture();
    let account =
        crate::agent_directory::tests::Account(manager.0.store.account_dir().to_path_buf());
    account.project("p", false);
    account.agent("p", "a", "display-name: Picture\navatar-id: user:pic\n");
    let images = account.0.join("avatars");
    fs::create_dir_all(&images).unwrap();
    let bytes = b"bounded roster image";
    let sha = raw_sha256(bytes);
    fs::write(images.join("pic.png"), bytes).unwrap();
    fs::write(images.join("avatars.json"), br#"[{"id":"user:pic","label":"Picture","fileName":"pic.png","mime":"image/png","createdAt":"now"}]"#).unwrap();
    manager.0.store.prepare_layout().unwrap();
    assert!(manager
        .roster_page(roster::public::ReadRequest {
            version: None,
            after: None
        })
        .is_err());
    manager.setup(Arc::new(|| {}), Arc::new(|| {}));
    wait_until(|| {
        manager
            .roster_page(roster::public::ReadRequest {
                version: None,
                after: None,
            })
            .is_ok()
    })
    .await;
    let page = manager
        .roster_page(roster::public::ReadRequest {
            version: None,
            after: None,
        })
        .unwrap();
    assert_eq!(
        serde_json::to_value(&page).unwrap()["items"][2]["avatar"]["available"],
        true
    );
    let url = manager
        .roster_avatar(roster::public::AvatarRequest {
            device_id: "local".into(),
            sha256: sha.clone(),
        })
        .await
        .unwrap();
    assert!(url.starts_with("data:image/png;base64,"));
    assert!(manager
        .roster_avatar(roster::public::AvatarRequest {
            device_id: "f".repeat(64),
            sha256: sha.clone()
        })
        .await
        .is_err());
    assert!(manager
        .avatar(public::AvatarRequest {
            sha256: sha.clone(),
            ext: "png".into()
        })
        .await
        .is_err());
    let first = manager.0.avatars.clone().try_acquire_owned().unwrap();
    let second = manager.0.avatars.clone().try_acquire_owned().unwrap();
    assert!(manager
        .roster_avatar(roster::public::AvatarRequest {
            device_id: "local".into(),
            sha256: sha.clone()
        })
        .await
        .is_err());
    assert!(manager
        .avatar(public::AvatarRequest {
            sha256: sha,
            ext: "png".into()
        })
        .await
        .is_err());
    drop((first, second));
    for _ in 0..100 {
        assert_eq!(
            manager
                .roster_page(roster::public::ReadRequest {
                    version: None,
                    after: None
                })
                .unwrap()
                .version,
            page.version
        );
    }
    assert_eq!(stub.calls.load(Ordering::Relaxed), 0);
    assert!(
        manager.0.worker.lock().unwrap().is_none() && manager.0.network.lock().unwrap().is_none()
    );
    assert!(manager.0.relay.lock().unwrap().is_none());
    assert!(
        !manager.0.store.state.identity_path().exists()
            && !manager.0.store.state.control_path().exists()
    );
    manager.shutdown();
    manager.0.roster.files().unwrap().0.stop_and_wait().await;
}

#[tokio::test]
async fn roster_avatar_rechecks_durable_membership_even_before_the_memory_hint_arrives() {
    let (_root, manager, _) = fixture();
    let account =
        crate::agent_directory::tests::Account(manager.0.store.account_dir().to_path_buf());
    let (state, _, peer) = roster::tests::joined(&account);
    let _other_owner = state.control_lease().unwrap();
    manager.0.store.prepare_layout().unwrap();
    let bytes = b"remote avatar";
    let sha = raw_sha256(bytes);
    let projects = vec![roster::Project {
        project_id: "p".into(),
        name: "Remote".into(),
        agents: vec![roster::Agent {
            agent_id: "a".into(),
            name: "Remote agent".into(),
            avatar: roster::Avatar::Image {
                sha256: sha.clone(),
                ext: "png".into(),
                size_bytes: bytes.len() as u64,
            },
        }],
    }];
    let remote = roster::PeerRoster {
        schema_version: 1,
        group_id: "group-a".into(),
        device_id: peer.clone(),
        membership_id: "peer-membership".into(),
        version: roster::version(&projects).unwrap(),
        received_at: "2026-09-13T00:00:00Z".into(),
        projects,
    };
    crate::bbs_sync::write(&state.root.join(format!("rosters/{peer}.json")), &remote).unwrap();
    fs::create_dir_all(manager.0.store.root().join("avatars")).unwrap();
    fs::write(
        manager.0.store.root().join(format!("avatars/{sha}.png")),
        bytes,
    )
    .unwrap();
    manager.setup(Arc::new(|| {}), Arc::new(|| {}));
    wait_until(|| {
        manager
            .roster_page(roster::public::ReadRequest {
                version: None,
                after: None,
            })
            .is_ok()
    })
    .await;
    assert!(manager
        .roster_avatar(roster::public::AvatarRequest {
            device_id: peer.clone(),
            sha256: sha.clone()
        })
        .await
        .is_ok());
    let current = control::read_membership(&manager.0.store.state).unwrap();
    roster::persist_members(&manager.0.store.state, current.as_ref(), &[]).unwrap();
    // Intentionally no memory update: the file/authority proof is independent
    // of event delivery and another App's stale view.
    assert!(manager
        .roster_avatar(roster::public::AvatarRequest {
            device_id: peer,
            sha256: sha
        })
        .await
        .is_err());
    manager.shutdown();
    manager.0.roster.files().unwrap().0.stop_and_wait().await;
}

#[test]
fn old_work_callbacks_waiting_for_snapshot_lock_cannot_overwrite_cancelled_state() {
    let (_root, manager, _) = fixture();
    let fence = GroupFence {
        group_id: "group-one".into(),
        membership_id: "membership-one".into(),
    };
    let mut view = manager.0.view.lock().unwrap();
    view.fence = Some((fence.group_id.clone(), fence.membership_id.clone()));
    view.status.sync.phase = public::Phase::Syncing;
    view.status.sync.last_successful_at = Some("2026-09-12T10:00:00Z".into());
    let late = manager.clone();
    let (entered, waiting) = std::sync::mpsc::channel();
    let task = std::thread::spawn(move || {
        entered.send(()).unwrap();
        late.notice(&fence, 0, 0, Notice::Connecting("peer".into()));
        late.work_error(0, "worker_unreachable");
    });
    waiting.recv_timeout(Duration::from_secs(1)).unwrap();
    manager.0.work.cancel();
    view.status.sync.phase = public::Phase::Idle;
    drop(view);
    task.join().unwrap();
    assert_eq!(manager.status().sync.phase, public::Phase::Idle);
    assert!(manager.status().sync.error.is_none());
    assert_eq!(
        manager.status().sync.last_successful_at.as_deref(),
        Some("2026-09-12T10:00:00Z")
    );
    // Durable management failures are independent of the cancelled work epoch.
    manager.error("worker_unreachable");
    assert_eq!(manager.status().sync.phase, public::Phase::Failed);
}

#[test]
fn stale_manual_admission_cannot_set_a_trigger_for_the_new_membership() {
    let (_root, manager, stub) = fixture();
    {
        let mut view = manager.0.view.lock().unwrap();
        view.status.group.id = Some("group-one".into());
        view.fence = Some(("group-one".into(), "new-membership".into()));
    }
    assert_eq!(
        manager.admit_start(Some("group-one"), Some("old-membership")),
        Err(public::Error::from_code("group_context_changed"))
    );
    assert_eq!(
        manager.admit_start(Some("old-group"), Some("new-membership")),
        Err(public::Error::from_code("group_context_changed"))
    );
    assert!(!manager.0.manual.load(Ordering::Acquire));
    assert_eq!(stub.calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn admitted_manual_stays_visible_after_ack_until_real_work_or_cancel() {
    let (_root, manager, stub, fence) = grouped_fixture();
    manager.0.booted.store(true, Ordering::Release);
    // Keep only the real control mailbox present; no HTTP or peer allocation
    // is needed to observe the formerly invisible admission/wake interval.
    let (tx, _rx) = mpsc::sync_channel(1);
    *manager.0.worker.lock().unwrap() = Some(Worker { generation: 1, tx });
    manager.notice(&fence, 0, 0, Notice::Exchange(exchange::Event::Failed("peer".into(), transport::Error::Timeout)));
    let before = manager.status();
    let command = public::Command { expected_group_id: Some(fence.group_id.clone()) };
    manager.start(command.clone()).await.unwrap();
    let accepted = manager.status();
    assert_eq!(accepted.sync.phase, public::Phase::Connecting);
    assert_eq!(accepted.sync.error, before.sync.error, "admission is not recovery");
    assert!(accepted.sync.completed.is_none() && accepted.sync.total.is_none());
    assert!(manager.0.manual.load(Ordering::Acquire), "work really was queued");
    tokio::time::sleep(Duration::from_millis(600)).await;
    let waiting = manager.status();
    assert_eq!(waiting, accepted, "ACK alone cannot restore the idle/Retry button");
    assert_eq!(stub.calls.load(Ordering::Relaxed), 0);
    manager.notice(&fence, 0, 0, Notice::Exchange(exchange::Event::Started("peer".into())));
    let started = manager.status();
    assert_eq!(started.sync.phase, public::Phase::Syncing);
    manager.notice(&fence, 0, 0, Notice::Exchange(exchange::Event::Progress("peer".into(), 0, 3)));
    let progress = manager.status();
    assert_eq!(progress.sync.completed, Some(0));
    assert_eq!(progress.sync.total, Some(3));
    manager.cancel(command).await.unwrap();
    let cancelled = manager.status();
    assert_eq!(cancelled.sync.phase, public::Phase::Idle);
    assert!(!manager.0.manual.load(Ordering::Acquire));
    manager.notice(&fence, 0, 0, Notice::Exchange(exchange::Event::Started("peer".into())));
    assert_eq!(manager.status(), cancelled, "old-epoch work must not restart the spinner");
    assert_eq!(cancelled.sync.last_successful_at, accepted.sync.last_successful_at);
    println!("KOTA_BBS_MANUAL_ADMISSION_FIXTURE={}", json!({
        "before":before,"accepted":accepted,"waiting":waiting,"started":started,"progress":progress,"cancelled":cancelled,
    }));
}

#[test]
fn a_refreshed_offline_peer_settles_manual_without_fabricating_sync_success() {
    let (_root, manager, _, fence) = grouped_fixture();
    let (tx, _rx) = mpsc::sync_channel(1);
    *manager.0.worker.lock().unwrap() = Some(Worker { generation: 1, tx });
    manager.notice(&fence, 0, 0, Notice::Exchange(exchange::Event::Failed("peer".into(), transport::Error::Timeout)));
    let before = manager.status();
    assert_eq!(manager.admit_start(Some(&fence.group_id), Some(&fence.membership_id)), Ok(true));
    assert_eq!(manager.status().sync.phase, public::Phase::Connecting);
    manager.0.view.lock().unwrap().status.group.members[0].online = false;
    manager.control_recovered(0);
    let settled = manager.status();
    assert_eq!(settled.sync.phase, public::Phase::Failed);
    assert_eq!(settled.sync.error, before.sync.error);
    assert_eq!(settled.sync.last_successful_at, before.sync.last_successful_at);
}
#[test]
#[cfg(target_os = "macos")]
fn native_change_watcher_only_observes_its_owned_marker_leaf() {
    let (root, m, _) = fixture();
    let changed = Arc::new(coordinator::Work::default());
    let watch = m.change_watcher(changed.clone()).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let baseline = changed.serial();
    let unrelated = root.0.join("project-files/target");
    fs::create_dir_all(&unrelated).unwrap();
    for n in 0..200 {
        fs::write(
            unrelated.join(format!("build-{n}")),
            b"unrelated build output",
        )
        .unwrap();
    }
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(changed.serial(), baseline);
    super::super::write(&m.0.store.state.changes_dir().join("content"), &1).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while changed.serial() == baseline {
        assert!(
            std::time::Instant::now() < deadline,
            "marker rename not delivered"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(watch);
}
#[tokio::test]
async fn startup_recovers_durable_pending_with_same_identity_and_request_without_status_side_effects(
) {
    let (_root, m, stub) = fixture();
    let identity = DeviceIdentity::from_seed([83; 32]).unwrap();
    m.0.store.state.save_identity(&identity).unwrap();
    let request = "persistent-request";
    let token = "a".repeat(64);
    super::super::write(&m.0.store.state.control_path(),&json!({"schemaVersion":1,"deviceName":"saved","current":null,"invitation":null,"pending":{"kind":"join","workerUrl":"https://example.test","token":token,"requestId":request,"name":"saved","revokeOwn":null}})).unwrap();
    m.bootstrap();
    wait_until(|| m.status().group.id.as_deref() == Some("group-test")).await;
    assert_eq!(m.status().device.id, identity.device_id().unwrap());
    assert_eq!(stub.requests.lock().unwrap()[0]["requestId"], request);
    assert_eq!(
        m.0.store
            .state
            .load_identity()
            .unwrap()
            .unwrap()
            .private_key,
        identity.private_key
    );
    let calls = stub.calls.load(Ordering::Relaxed);
    for _ in 0..100 {
        let _ = m.status();
    }
    assert_eq!(stub.calls.load(Ordering::Relaxed), calls);
    m.shutdown();
    wait_until(|| m.0.worker.lock().unwrap().is_none()).await;
}
#[tokio::test]
async fn corrupt_or_missing_identity_never_replaces_keys_or_clears_pending() {
    let (_root, m, stub) = fixture();
    fs::create_dir_all(m.0.store.state.control_path().parent().unwrap()).unwrap();
    let control=br#"{"schemaVersion":1,"deviceName":"saved","current":null,"invitation":null,"pending":{"kind":"join","workerUrl":"https://example.test","token":"secret","requestId":"intent-one","name":"saved","revokeOwn":null}}"#;
    fs::write(m.0.store.state.control_path(), control).unwrap();
    fs::write(m.0.store.state.identity_path(), b"corrupt").unwrap();
    m.bootstrap();
    assert_eq!(m.status().sync.phase, public::Phase::Failed);
    assert_eq!(
        fs::read(m.0.store.state.identity_path()).unwrap(),
        b"corrupt"
    );
    assert_eq!(fs::read(m.0.store.state.control_path()).unwrap(), control);
    assert_eq!(stub.calls.load(Ordering::Relaxed), 0);
    assert!(m.0.worker.lock().unwrap().is_none());
}
#[tokio::test]
async fn stale_group_and_private_errors_fail_before_starting_management_work() {
    let (_root, m, stub) = fixture();
    m.bootstrap();
    let result = m
        .remove(public::RemoveRequest {
            expected_group_id: Some("old-group".into()),
            device_id: raw_sha256(b"peer"),
        })
        .await;
    assert_eq!(result.unwrap_err().code, "group_context_changed");
    assert_eq!(stub.calls.load(Ordering::Relaxed), 0);
    assert!(!m.0.store.state.identity_path().exists());
    m.error("token=DO_NOT_LEAK");
    let public = serde_json::to_string(&m.status()).unwrap();
    assert!(!public.contains("DO_NOT_LEAK"));
    assert!(!serde_json::to_string(&m.diagnostics())
        .unwrap()
        .contains("DO_NOT_LEAK"));
}
#[tokio::test]
async fn explicit_action_is_processed_and_new_unjoined_identity_does_not_leave_a_loop() {
    let (_root, m, stub) = fixture();
    m.bootstrap();
    let error = m
        .invitation(public::InvitationRequest {
            expected_group_id: None,
            refresh: false,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, "worker_not_paired");
    wait_until(|| m.0.worker.lock().unwrap().is_none()).await;
    assert_eq!(stub.calls.load(Ordering::Relaxed), 0);
    assert!(m.0.store.state.identity_path().exists());
    // A second action must be queued atomically with creation of its new owner.
    m.rename(public::RenameRequest {
        expected_group_id: None,
        name: "renamed".into(),
    })
    .await
    .unwrap();
    assert_eq!(m.status().device.name, "renamed");
    wait_until(|| m.0.worker.lock().unwrap().is_none()).await;
}

#[tokio::test]
async fn cancellation_does_not_wait_for_blocked_https_and_keeps_membership() {
    #[derive(Clone)]
    struct Blocked {
        entered: Arc<AtomicBool>,
        release: Arc<(Mutex<bool>, std::sync::Condvar)>,
    }
    impl Transport for Blocked {
        fn send(
            &self,
            _: &str,
            _: &str,
            _: &BTreeMap<String, String>,
            _: &str,
        ) -> Result<RemoteResponse, String> {
            self.entered.store(true, Ordering::Release);
            let (lock, wake) = &*self.release;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = wake.wait(released).unwrap();
            }
            Err("worker_unreachable".into())
        }
    }
    for pending_join in [false, true] {
        let (root, original, _) = fixture();
        let store = original.0.store.clone();
        let identity = DeviceIdentity::from_seed([85; 32]).unwrap();
        store.state.save_identity(&identity).unwrap();
        let current = if pending_join {
            Value::Null
        } else {
            json!({"groupId":"group-test","membershipId":"member-test","workerUrl":"https://example.test","role":"member"})
        };
        let pending = if pending_join {
            json!({"kind":"join","workerUrl":"https://example.test","token":"a".repeat(64),"requestId":"same-durable-request","name":"saved","revokeOwn":null})
        } else {
            Value::Null
        };
        super::super::write(&store.state.control_path(), &json!({"schemaVersion":1,"deviceName":"saved","current":current,"invitation":null,"pending":pending})).unwrap();
        let blocked = Blocked {
            entered: Arc::new(AtomicBool::new(false)),
            release: Arc::new((Mutex::new(false), std::sync::Condvar::new())),
        };
        let source = blocked.clone();
        let manager = Manager::services(
            store,
            Arc::new(move || Box::new(source.clone())),
            Arc::new(|| None),
        );
        manager.bootstrap();
        wait_until(|| blocked.entered.load(Ordering::Acquire)).await;
        let saved = fs::read(manager.0.store.state.control_path()).unwrap();
        let result = tokio::time::timeout(
            Duration::from_millis(100),
            manager.cancel(public::Command {
                expected_group_id: if pending_join {
                    None
                } else {
                    Some("group-test".into())
                },
            }),
        )
        .await;
        // Always unblock the worker even when an assertion fails.
        {
            let (lock, wake) = &*blocked.release;
            *lock.lock().unwrap() = true;
            wake.notify_all();
        }
        result.expect("cancel blocked behind HTTPS").unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(
            manager.status().group.id.as_deref(),
            if pending_join {
                None
            } else {
                Some("group-test")
            }
        );
        if pending_join {
            assert_eq!(
                fs::read(manager.0.store.state.control_path()).unwrap(),
                saved,
                "Cancel cannot discard an authorized durable join intent"
            );
        } else {
            assert_eq!(
                manager.status().sync.phase,
                public::Phase::Idle,
                "old heartbeat failure overwrote cancellation"
            );
        }
        manager.shutdown();
        wait_until(|| manager.0.worker.lock().unwrap().is_none()).await;
        drop(root);
    }
}

#[tokio::test]
async fn second_instance_keeps_persisted_group_but_never_takes_control_lease() {
    let (_root, m, stub) = fixture();
    let identity = DeviceIdentity::from_seed([86; 32]).unwrap();
    m.0.store.state.save_identity(&identity).unwrap();
    super::super::write(&m.0.store.state.control_path(),&json!({"schemaVersion":1,"deviceName":"saved","current":{"groupId":"group-test","membershipId":"member-test","workerUrl":"https://example.test","role":"member"},"invitation":null,"pending":null})).unwrap();
    let _lease = m.0.store.state.control_lease().unwrap();
    let before = fs::read(m.0.store.state.identity_path()).unwrap();
    m.bootstrap();
    wait_until(|| m.status().sync.phase == public::Phase::Failed).await;
    assert_eq!(m.status().group.id.as_deref(), Some("group-test"));
    assert_eq!(fs::read(m.0.store.state.identity_path()).unwrap(), before);
    assert_eq!(stub.calls.load(Ordering::Relaxed), 0);
    m.shutdown();
    wait_until(|| m.0.worker.lock().unwrap().is_none()).await;
}

#[derive(Clone)]
struct RecoveryWorker {
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    members: Vec<control::Member>,
}
impl Transport for RecoveryWorker {
    fn send(
        &self,
        _: &str,
        url: &str,
        _: &BTreeMap<String, String>,
        body: &str,
    ) -> Result<RemoteResponse, String> {
        let body: Value = serde_json::from_str(body).unwrap();
        self.requests.lock().unwrap().push((url.into(), body));
        let members = url.ends_with("/heartbeat").then_some(&self.members);
        serde_json::from_value(json!({
            "protocolVersion": 1, "ok": true, "groupId": "group-test",
            "members": members, "signals": [], "memberVersion": 1
        }))
        .map_err(|_| "bad_fixture".into())
    }
}
fn recovery_fixture() -> (Root, Manager, RecoveryWorker) {
    let (root, original, _) = fixture();
    let identity = DeviceIdentity::from_seed([86; 32]).unwrap();
    let peer = DeviceIdentity::from_seed([87; 32]).unwrap();
    let worker = RecoveryWorker {
        requests: Arc::new(Mutex::new(vec![])),
        members: vec![
            control::Member {
                device_id: identity.device_id().unwrap(),
                public_key: identity.public_key.clone(),
                name: "This Mac".into(),
                role: control::Role::Member,
                membership_id: "member-test".into(),
                last_seen_at: 0,
                online: true,
            },
            control::Member {
                device_id: peer.device_id().unwrap(),
                public_key: peer.public_key,
                name: "Other Mac".into(),
                role: control::Role::Owner,
                membership_id: "owner-test".into(),
                last_seen_at: 0,
                // Keep the integration test HTTP-only: no real peer, STUN or
                // network runtime is needed to prove control recovery.
                online: false,
            },
        ],
    };
    let factory = worker.clone();
    let manager = Manager::services(
        original.0.store.clone(),
        Arc::new(move || Box::new(factory.clone())),
        Arc::new(|| None),
    );
    manager.0.store.state.save_identity(&identity).unwrap();
    super::super::write(
        &manager.0.store.state.control_path(),
        &json!({
            "schemaVersion": 1, "deviceName": "saved", "current": {
                "groupId": "group-test", "membershipId": "member-test",
                "workerUrl": "https://fixture.invalid", "role": "member"
            }, "invitation": null, "pending": null
        }),
    )
    .unwrap();
    // Start with the existing client's durable encoding: JSON Value field
    // order differs from ControlState, even when their values are identical.
    drop(
        ControlClient::open_existing(manager.0.store.state.clone(), worker.clone(), None).unwrap(),
    );
    (root, manager, worker)
}
fn retry_command() -> public::Command {
    public::Command {
        expected_group_id: Some("group-test".into()),
    }
}
async fn boot_while_leased(manager: &Manager) -> super::super::ControlLease {
    let lease = manager.0.store.state.control_lease().unwrap();
    manager.bootstrap();
    wait_until(|| manager.status().sync.control_recoverable).await;
    assert_eq!(manager.status().sync.phase, public::Phase::Failed);
    lease
}

#[tokio::test]
async fn retry_takes_over_released_lease_and_projects_real_control_fixture() {
    let (_root, manager, worker) = recovery_fixture();
    let control = fs::read(manager.0.store.state.control_path()).unwrap();
    let identity = fs::read(manager.0.store.state.identity_path()).unwrap();
    let lease = boot_while_leased(&manager).await;
    let busy_status = manager.status();
    assert_eq!(busy_status.group.id.as_deref(), Some("group-test"));
    assert!(busy_status.group.members.is_empty());
    assert_eq!(
        busy_status.sync.error,
        Some(public::display_error("control_in_use"))
    );
    assert_eq!(
        manager.diagnostics().last_error.as_deref(),
        Some("sync_busy")
    );

    let busy_error = manager.start(retry_command()).await.unwrap_err();
    assert_eq!(busy_error.code, "sync_busy");
    wait_until(|| manager.status().sync.control_recoverable).await;
    for action in [Action::Invitation(false), Action::Join("not-a-code".into())] {
        assert_eq!(
            manager
                .action(Some("group-test".into()), action)
                .await
                .unwrap_err()
                .code,
            "sync_busy"
        );
        wait_until(|| manager.status().sync.control_recoverable).await;
    }
    assert!(worker.requests.lock().unwrap().is_empty());
    assert_eq!(
        fs::read(manager.0.store.state.control_path()).unwrap(),
        control
    );
    assert_eq!(
        fs::read(manager.0.store.state.identity_path()).unwrap(),
        identity
    );
    drop(lease);
    let attempts = manager.0.generation.load(Ordering::Relaxed);
    // A released lease, status/diagnostic reads and panel reads do not retry.
    for _ in 0..5 {
        assert!(manager.status().sync.control_recoverable);
        let _ = manager.diagnostics();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(manager.0.generation.load(Ordering::Relaxed), attempts);
    assert!(worker.requests.lock().unwrap().is_empty());

    manager.start(retry_command()).await.unwrap();
    wait_until(|| {
        let status = manager.status();
        // Only a real heartbeat settles control recovery, not Retry admission.
        status.group.members.len() == 2 && status.sync.error.is_none()
            && status.sync.phase == public::Phase::Idle
    })
    .await;
    let recovered = manager.status();
    assert!(!recovered.sync.control_recoverable);
    assert_eq!(recovered.sync.phase, public::Phase::Idle);
    assert!(
        recovered.sync.last_successful_at.is_none(),
        "heartbeat is not a completed content round"
    );
    assert!(recovered.group.members[0].online);
    assert!(!recovered.group.members[1].online);
    assert!(manager.0.worker.lock().unwrap().is_some());
    assert!(manager.0.network.lock().unwrap().is_none());
    assert_eq!(worker.requests.lock().unwrap().len(), 1);
    assert!(worker.requests.lock().unwrap()[0].0.ends_with("/heartbeat"));
    assert_eq!(
        fs::read(manager.0.store.state.identity_path()).unwrap(),
        identity
    );
    assert_eq!(
        fs::read(manager.0.store.state.control_path()).unwrap(),
        control
    );

    let fence = GroupFence {
        group_id: "group-test".into(),
        membership_id: "member-test".into(),
    };
    manager.notice(
        &fence,
        0,
        0,
        Notice::Exchange(exchange::Event::Failed(
            worker.members[1].device_id.clone(),
            transport::Error::Timeout,
        )),
    );
    manager.error("worker_unreachable");
    let control_over_exchange = manager.status();
    // Use the same healthy-heartbeat path, not a hand-built public state.
    manager.start(retry_command()).await.unwrap();
    wait_until(|| {
        worker.requests.lock().unwrap().len() == 2
            && manager.0.view.lock().unwrap().control_error.is_none()
    })
    .await;
    let retained_exchange = manager.status();
    assert_eq!(retained_exchange.sync.phase, public::Phase::Failed);
    assert_eq!(
        retained_exchange.sync.error,
        Some(public::display_error("sync_timeout"))
    );
    assert!(!retained_exchange.sync.control_recoverable);
    assert_eq!(
        manager.diagnostics().last_error.as_deref(),
        Some("sync_timeout")
    );
    manager.notice(
        &fence,
        0,
        0,
        Notice::Exchange(exchange::Event::Started(
            worker.members[1].device_id.clone(),
        )),
    );
    let started = manager.status();
    assert_eq!(started.sync.phase, public::Phase::Syncing);
    assert_eq!(started.sync.error, retained_exchange.sync.error);

    let details = [
        "sync_unavailable",
        "control_in_use",
        "incomplete_sync_identity",
        "sync_protocol_error",
        "sync_timeout",
        "stale_signature",
        "worker_unreachable",
        "worker_update_required",
        "removed",
        "sync_integrity_error",
        "sync_file_io_failed",
    ]
    .map(|code| (code, public::display_error(code)))
    .into_iter()
    .collect::<BTreeMap<_, _>>();
    let fixture = json!({ "busy": busy_status, "busyError": busy_error, "recovered": recovered,
        "controlOverExchange": control_over_exchange, "retainedExchange": retained_exchange,
        "started": started, "details": details });
    let encoded = serde_json::to_string(&fixture).unwrap();
    for forbidden in [
        "fixture.invalid",
        "requestId",
        "privateKey",
        "nonce",
        "errorScope",
        "errorCode",
    ] {
        assert!(!encoded.contains(forbidden));
    }
    println!("KOTA_BBS_RETRY_RECOVERY_FIXTURE={encoded}");
    manager.shutdown();
    wait_until(|| manager.0.worker.lock().unwrap().is_none()).await;
}

#[tokio::test]
async fn retry_rereads_left_or_replaced_membership_instead_of_reviving_snapshot() {
    for left in [true, false] {
        let (_root, manager, worker) = recovery_fixture();
        let lease = boot_while_leased(&manager).await;
        let identity = fs::read(manager.0.store.state.identity_path()).unwrap();
        let mut saved: Value =
            serde_json::from_slice(&fs::read(manager.0.store.state.control_path()).unwrap())
                .unwrap();
        if left {
            saved["current"] = Value::Null;
        } else {
            saved["current"]["membershipId"] = json!("replacement-membership");
        }
        // Simulate the old authorized owner committing its last state before
        // releasing the lease. The new instance's UI still has the old group.
        super::super::write(&manager.0.store.state.control_path(), &saved).unwrap();
        drop(lease);
        let error = manager.start(retry_command()).await.unwrap_err();
        assert_eq!(error.code, "group_context_changed");
        if left {
            wait_until(|| manager.0.worker.lock().unwrap().is_none()).await;
            assert!(manager.status().group.id.is_none());
            assert!(!manager.status().sync.control_recoverable);
            assert!(worker.requests.lock().unwrap().is_empty());
        } else {
            assert_eq!(
                manager.0.view.lock().unwrap().fence.as_ref().unwrap().1,
                "replacement-membership"
            );
        }
        assert_eq!(
            fs::read(manager.0.store.state.identity_path()).unwrap(),
            identity
        );
        assert_eq!(
            serde_json::from_slice::<Value>(
                &fs::read(manager.0.store.state.control_path()).unwrap()
            )
            .unwrap(),
            saved
        );
        manager.shutdown();
        wait_until(|| manager.0.worker.lock().unwrap().is_none()).await;
    }
}

#[tokio::test]
async fn retry_preserves_and_resumes_original_durable_management_request() {
    let (_root, manager, worker) = recovery_fixture();
    let lease = boot_while_leased(&manager).await;
    let mut saved: Value =
        serde_json::from_slice(&fs::read(manager.0.store.state.control_path()).unwrap()).unwrap();
    saved["pending"] = json!({ "kind": "mutation", "membership": saved["current"],
        "action": {"kind":"rename","name":"Restored name"}, "requestId":"original-durable-request" });
    super::super::write(&manager.0.store.state.control_path(), &saved).unwrap();
    assert_eq!(
        manager.start(retry_command()).await.unwrap_err().code,
        "sync_busy"
    );
    wait_until(|| manager.status().sync.control_recoverable).await;
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(manager.0.store.state.control_path()).unwrap())
            .unwrap(),
        saved
    );
    assert!(worker.requests.lock().unwrap().is_empty());
    drop(lease);
    manager.start(retry_command()).await.unwrap();
    wait_until(|| {
        manager.status().group.members.len() == 2 && manager.status().sync.error.is_none()
    })
    .await;
    let requests = worker.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].0.ends_with("/rename"));
    assert_eq!(requests[0].1["requestId"], "original-durable-request");
    assert!(requests[1].0.ends_with("/heartbeat"));
    let completed: Value =
        serde_json::from_slice(&fs::read(manager.0.store.state.control_path()).unwrap()).unwrap();
    assert!(completed["pending"].is_null());
    assert_eq!(completed["deviceName"], "Restored name");
    manager.shutdown();
    wait_until(|| manager.0.worker.lock().unwrap().is_none()).await;
}

#[tokio::test]
async fn retry_open_errors_fail_closed_without_replacing_identity_or_retrying_in_background() {
    for fault in ["missing", "corrupt", "lock_path"] {
        let (_root, manager, worker) = recovery_fixture();
        let lease = boot_while_leased(&manager).await;
        drop(lease);
        let identity_path = manager.0.store.state.identity_path();
        match fault {
            "missing" => fs::remove_file(&identity_path).unwrap(),
            "corrupt" => fs::write(&identity_path, b"{broken").unwrap(),
            _ => {
                let lock = identity_path.parent().unwrap().join(".control.lock");
                fs::remove_file(&lock).unwrap();
                std::os::unix::fs::symlink("absent-test-target", lock).unwrap();
            }
        }
        let identity = fs::read(&identity_path).ok();
        let control = fs::read(manager.0.store.state.control_path()).unwrap();
        let error = manager.start(retry_command()).await.unwrap_err();
        assert_eq!(
            error.code, "sync_unavailable",
            "only exact contention maps to sync_busy"
        );
        wait_until(|| manager.status().sync.control_recoverable).await;
        let generation = manager.0.generation.load(Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(manager.0.generation.load(Ordering::Relaxed), generation);
        assert_eq!(fs::read(&identity_path).ok(), identity);
        assert_eq!(
            fs::read(manager.0.store.state.control_path()).unwrap(),
            control
        );
        assert!(worker.requests.lock().unwrap().is_empty());
        assert_eq!(manager.status().sync.phase, public::Phase::Failed);
        manager.shutdown();
    }
}

#[tokio::test]
async fn healthy_control_clears_only_control_errors_and_obeys_cancel_epoch() {
    let (_root, manager, _, fence) = grouped_fixture();
    manager.error("worker_unreachable");
    manager.control_recovered(1); // stale/future replies cannot clear an error
    assert_eq!(manager.status().sync.phase, public::Phase::Failed);
    manager.control_recovered(0);
    assert_eq!(manager.status().sync.phase, public::Phase::Idle);
    assert!(manager.status().sync.error.is_none());
    assert_eq!(
        manager.status().sync.last_successful_at.as_deref(),
        Some("2000-01-01T00:00:00Z")
    );
    manager.error("worker_unreachable");
    manager.notice(
        &fence,
        0,
        0,
        Notice::Exchange(exchange::Event::Failed(
            "peer".into(),
            transport::Error::Protocol,
        )),
    );
    manager.control_recovered(0);
    assert_eq!(
        manager.status().sync.error,
        Some(public::display_error("sync_protocol_error"))
    );
    assert_eq!(manager.status().sync.phase, public::Phase::Failed);
    manager.notice(
        &fence,
        0,
        0,
        Notice::Exchange(exchange::Event::Started("peer".into())),
    );
    assert_eq!(manager.status().sync.error, Some(public::display_error("sync_protocol_error")));
    assert_eq!(manager.status().sync.phase, public::Phase::Syncing);
}

#[test]
fn idle_recovery_status_reads_never_restart_work_or_restore_an_old_failure() {
    let (_root, manager, stub, fence) = grouped_fixture();
    let (tx, _rx) = mpsc::sync_channel(1);
    *manager.0.worker.lock().unwrap() = Some(Worker { generation: 1, tx });
    manager.notice(&fence, 0, 0, Notice::Error("peer".into(), transport::Error::Timeout));
    let failed = manager.status();
    manager.notice(&fence, 0, 0, Notice::Connecting("peer".into()));
    let connecting = manager.status();
    // A new attempt or Started is not completion: only a complete Finished
    // clears this peer's debt, and neither admission nor memory reads advance Last sync.
    assert_eq!(connecting.sync.phase, public::Phase::Connecting);
    assert_eq!(connecting.sync.indicator, public::Indicator::Connecting);
    assert_eq!(connecting.sync.error, failed.sync.error);
    assert_eq!(connecting.sync.last_successful_at, failed.sync.last_successful_at);
    manager.notice(&fence, 0, 0, Notice::Exchange(exchange::Event::Started("peer".into())));
    let started = manager.status();
    for _ in 0..3 { assert_eq!(manager.status(), started, "closing/reopening the UI just rereads memory"); }
    assert_eq!(started.sync.error, failed.sync.error);
    assert_eq!(started.sync.indicator, public::Indicator::Connecting);
    assert_eq!(started.sync.last_successful_at, failed.sync.last_successful_at);
    manager.notice(&fence, 0, 0, Notice::Exchange(exchange::Event::Finished("peer".into(), exchange::Outcome::default())));
    let recovered = manager.status();
    assert_eq!(recovered.sync.phase, public::Phase::Idle);
    assert_eq!(recovered.sync.indicator, public::Indicator::Healthy);
    assert!(recovered.sync.error.is_none());
    assert_ne!(recovered.sync.last_successful_at, failed.sync.last_successful_at);
    assert_eq!(manager.status(), recovered);
    assert!(!manager.0.manual.load(Ordering::Acquire));
    assert_eq!(stub.calls.load(Ordering::Relaxed), 0);
    println!("KOTA_BBS_IDLE_RECOVERY_FIXTURE={}", json!({"failed":failed,"connecting":connecting,"started":started,"recovered":recovered}));
}

#[tokio::test]
async fn cancelled_retry_does_not_clear_durable_intent_or_start_a_stale_heartbeat() {
    #[derive(Clone)]
    struct Blocked {
        calls: Arc<AtomicUsize>,
        release: Arc<(Mutex<bool>, std::sync::Condvar)>,
    }
    impl Transport for Blocked {
        fn send(
            &self,
            _: &str,
            url: &str,
            _: &BTreeMap<String, String>,
            body: &str,
        ) -> Result<RemoteResponse, String> {
            assert!(
                url.ends_with("/rename"),
                "cancelled Retry must not schedule a heartbeat"
            );
            assert_eq!(
                serde_json::from_str::<Value>(body).unwrap()["requestId"],
                "original-durable-request"
            );
            self.calls.fetch_add(1, Ordering::Release);
            let (lock, wake) = &*self.release;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = wake.wait(released).unwrap();
            }
            Err("worker_unreachable".into())
        }
    }
    let (_root, original, _) = recovery_fixture();
    let blocked = Blocked {
        calls: Arc::new(AtomicUsize::new(0)),
        release: Arc::new((Mutex::new(false), std::sync::Condvar::new())),
    };
    let factory = blocked.clone();
    let manager = Manager::services(
        original.0.store.clone(),
        Arc::new(move || Box::new(factory.clone())),
        Arc::new(|| None),
    );
    let lease = boot_while_leased(&manager).await;
    let mut saved: Value =
        serde_json::from_slice(&fs::read(manager.0.store.state.control_path()).unwrap()).unwrap();
    saved["pending"] = json!({ "kind":"mutation", "membership":saved["current"],
        "action":{"kind":"rename","name":"Restored name"}, "requestId":"original-durable-request" });
    super::super::write(&manager.0.store.state.control_path(), &saved).unwrap();
    drop(lease);
    let retrying = manager.clone();
    let retry = tokio::spawn(async move { retrying.start(retry_command()).await });
    wait_until(|| blocked.calls.load(Ordering::Acquire) == 1).await;
    let cancel =
        tokio::time::timeout(Duration::from_millis(100), manager.cancel(retry_command())).await;
    // Always release the artificial HTTP block before asserting the hot path.
    *blocked.release.0.lock().unwrap() = true;
    blocked.release.1.notify_all();
    cancel
        .expect("Cancel waited for Retry's HTTPS request")
        .unwrap();
    retry.await.unwrap().unwrap_err();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(blocked.calls.load(Ordering::Acquire), 1);
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(manager.0.store.state.control_path()).unwrap())
            .unwrap(),
        saved
    );
    // The late network failure cannot replace the pre-existing busy detail.
    assert_eq!(
        manager.status().sync.error,
        Some(public::display_error("control_in_use"))
    );
    assert_eq!(
        manager.0.view.lock().unwrap().status.sync.phase,
        public::Phase::Idle
    );
    manager.shutdown();
    wait_until(|| manager.0.worker.lock().unwrap().is_none()).await;
}
