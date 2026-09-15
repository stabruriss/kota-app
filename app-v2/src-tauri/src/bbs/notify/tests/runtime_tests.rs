use super::*;
use remote::{attachment, Receiver};

#[tokio::test]
async fn reuse_of_a_verified_attachment_notifies_without_waiting_for_the_deadline() {
    let r = Receiver::new().await;
    let bytes = b"one verified copy";
    let (raw, first) = r.bytes(
        "root",
        "topic",
        "First",
        vec![],
        vec![attachment("root", "image", bytes)],
    );
    r.post(&raw, &first).await;
    r.resource(
        reconcile::attachment_resource(&first, &first.attachments[0]),
        bytes,
        None,
    )
    .await;
    let (raw, fork) = r.bytes(
        "root",
        "topic",
        "Fork",
        vec![],
        vec![attachment("root", "image", bytes)],
    );
    r.post(&raw, &fork).await;
    let (raw, reply) = r.bytes(
        "reply",
        "reply",
        "Use the available copy",
        vec![r.target()],
        vec![],
    );
    r.post(&raw, &reply).await;
    let key = r.b.keys("pending")[0].clone();
    let record = r.b.record(&key);
    assert_eq!(record.waiting.len(), 1);
    assert_eq!(record.waiting[0].version_id, fork.version_id);
    let service = Service::at(r.b.content.clone());
    let sink = Arc::new(RecordingSink::default());
    service.start_with(r.files.clone(), sink.clone());
    let store = r.b.content.clone();
    let fence = r.fence.clone();
    let target = reconcile::attachment_resource(&fork, &fork.attachments[0]);
    assert!(r
        .files
        .run_when_available(&Cancellation::default(), move |io| {
            store.copy_available(&fence, &target, None, io)
        })
        .await
        .unwrap()
        .unwrap());
    wait_calls(&sink, 1).await;
    assert!(now_ms() < record.deadline_ms);
    assert!(!sink.calls.lock().unwrap()[0]["text"]
        .as_str()
        .unwrap()
        .contains(ATTACHMENT_WARNING));
    service.close().await;
    r.files.stop_and_wait().await;
}

async fn wait_calls(sink: &RecordingSink, n: usize) {
    wait_calls_with_budget(sink, n, Duration::from_secs(20)).await;
}
async fn wait_calls_with_budget(sink: &RecordingSink, n: usize, budget: Duration) {
    let result = tokio::time::timeout(budget, async {
        loop {
            if sink.calls.lock().unwrap().len() == n {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(
        result.is_ok(),
        "notification delivery: {}/{}",
        sink.calls.lock().unwrap().len(),
        n
    );
}

#[tokio::test]
async fn app_bridge_starts_without_identity_room_or_lm_and_leaf_events_deliver_after_initial_scan()
{
    let b = Board::new();
    let files = FileIo::start(Limits::default()).unwrap();
    let sink = Arc::new(RecordingSink::default());
    let service = Service::at(b.content.clone());
    service.start_with(files.clone(), sink.clone());
    tokio::time::timeout(Duration::from_secs(5), async {
        while service.scan_count() == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(!b.content.state.identity_path().exists());
    assert!(!b.content.state.control_path().exists());
    assert!(!b
        .account
        .0
        .join("Workspaces/target/project-memory")
        .exists());
    b.post(
        "CLI-style late publication without an open room",
        &[b.target()],
    );
    wait_calls(&sink, 1).await;
    assert!(b
        .account
        .0
        .join("Workspaces/target/project-memory")
        .exists());
    let scans = service.scan_count();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        service.scan_count(),
        scans,
        "healthy idle has no polling/rescan loop"
    );
    assert!(!b.content.state.identity_path().exists());
    assert!(!b.content.state.control_path().exists());
    service.close().await;
    files.stop_and_wait().await;
}

#[tokio::test]
async fn startup_streams_more_than_hint_budget_without_truncating_pending_targets() {
    let b = Board::new();
    let targets = (0..273)
        .map(|n| mentions::Mention::parse(&format!("local/target/gone-{n}")).unwrap())
        .collect::<Vec<_>>();
    b.post("all explicit recipients remain reachable", &targets);
    assert_eq!(b.keys("pending").len(), targets.len());
    let sink = Arc::new(RecordingSink::default());
    let service = Service::at(b.content.clone());
    let files = FileIo::start(Limits::default()).unwrap();
    service.start_with(files.clone(), sink.clone());
    // This is a many-recipient durability/streaming test, not the ten-second
    // attachment deadline. Each target intentionally persists a real Bus room
    // receipt. Keep the normal/deadline tests' twenty-second watchdog separate.
    let start = Instant::now();
    let mut last = (0, Instant::now());
    loop {
        let done = sink.calls.lock().unwrap().len();
        if done == targets.len() {
            break;
        }
        if done != last.0 {
            last = (done, Instant::now());
        }
        assert!(
            last.1.elapsed() < Duration::from_secs(10),
            "bulk progress stalled at {done}; pending={}, processing={}, processed={}",
            b.keys("pending").len(),
            b.keys("processing").len(),
            b.keys("processed").len()
        );
        assert!(
            start.elapsed() < Duration::from_secs(120),
            "bulk absolute watchdog at {done}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    println!(
        "BBS_NOTIFY_BULK count={} elapsed_ms={}",
        targets.len(),
        start.elapsed().as_millis()
    );
    service.close().await;
    assert_eq!(b.keys("processed").len(), targets.len());
    assert!(sink
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|c| c["wake"] == false));
    files.stop_and_wait().await;
}

#[tokio::test]
async fn real_ten_second_deadline_progresses_while_account_file_worker_is_blocked_and_late_file_does_not_repeat(
) {
    let r = Receiver::new().await;
    let bytes = b"root attachment";
    let (raw, root) = r.bytes(
        "root",
        "topic",
        "Root",
        vec![],
        vec![attachment("root", "image", bytes)],
    );
    r.post(&raw, &root).await;
    let (raw, reply) = r.bytes(
        "reply",
        "reply",
        "Notify before file worker becomes free",
        vec![r.target()],
        vec![],
    );
    let started = Instant::now();
    r.post(&raw, &reply).await;
    let key = r.b.keys("pending")[0].clone();
    let record = r.b.record(&key);
    assert!(record.deadline_ms - now_ms() <= ATTACHMENT_WAIT_MS);
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let files = r.files.clone();
    let busy = tokio::spawn(async move {
        files
            .run_when_available(&Cancellation::default(), move |_| {
                let _ = entered_tx.send(());
                release_rx.recv_timeout(Duration::from_secs(25)).unwrap();
            })
            .await
    });
    entered_rx.await.unwrap();
    let service = Service::at(r.b.content.clone());
    let sink = Arc::new(RecordingSink::default());
    service.start_with(r.files.clone(), sink.clone());
    wait_calls(&sink, 1).await;
    assert!(
        !busy.is_finished(),
        "deadline cannot queue behind account file I/O"
    );
    assert!(started.elapsed() >= Duration::from_secs(9));
    assert!(sink.calls.lock().unwrap()[0]["text"]
        .as_str()
        .unwrap()
        .contains(ATTACHMENT_WARNING));
    release_tx.send(()).unwrap();
    busy.await.unwrap().unwrap();
    r.resource(
        reconcile::attachment_resource(&root, &root.attachments[0]),
        bytes,
        None,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(sink.calls.lock().unwrap().len(), 1);
    service.close().await;
    r.files.stop_and_wait().await;
}

#[tokio::test]
async fn attachment_saved_callback_delivers_early_and_restart_does_not_reset_expired_wait() {
    let r = Receiver::new().await;
    let bytes = b"root attachment";
    let (raw, root) = r.bytes(
        "root",
        "topic",
        "Root",
        vec![],
        vec![attachment("root", "image", bytes)],
    );
    r.post(&raw, &root).await;
    let (raw, reply) = r.bytes("reply", "reply", "Early", vec![r.target()], vec![]);
    r.post(&raw, &reply).await;
    let key = r.b.keys("pending")[0].clone();
    let deadline = r.b.record(&key).deadline_ms;
    let service = Service::at(r.b.content.clone());
    let sink = Arc::new(RecordingSink::default());
    service.start_with(r.files.clone(), sink.clone());
    // This deliberately does not wait for the service's waiting subscription.
    r.resource(
        reconcile::attachment_resource(&root, &root.attachments[0]),
        bytes,
        None,
    )
    .await;
    wait_calls(&sink, 1).await;
    assert!(
        now_ms() < deadline,
        "complete ledger should notify before the deadline"
    );
    assert!(!sink.calls.lock().unwrap()[0]["text"]
        .as_str()
        .unwrap()
        .contains(ATTACHMENT_WARNING));
    service.close().await;
    let (raw, next) = r.bytes(
        "next",
        "reply",
        "Expired on restart",
        vec![r.target()],
        vec![attachment("next", "not-here", b"missing")],
    );
    r.post(&raw, &next).await;
    let key = r.b.keys("pending")[0].clone();
    let mut record = r.b.record(&key);
    record.created_at_ms = now_ms() - 15_000;
    record.deadline_ms = now_ms() - 5_000;
    fs::write(
        r.b.queue.path("pending", &key).unwrap(),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    let restart = Service::at(r.b.content.clone());
    restart.start_with(r.files.clone(), sink.clone());
    let start = Instant::now();
    wait_calls(&sink, 2).await;
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(sink.calls.lock().unwrap()[1]["text"]
        .as_str()
        .unwrap()
        .contains(ATTACHMENT_WARNING));
    restart.close().await;
    r.files.stop_and_wait().await;
}
