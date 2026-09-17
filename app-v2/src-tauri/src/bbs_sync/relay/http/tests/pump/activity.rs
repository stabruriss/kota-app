use super::*;
use crate::bbs_sync::relay::activity::{Activity, Event, Failure, Operation};
use std::{
    future::Future,
    task::{Context as TaskContext, Wake, Waker},
};
use tokio::{sync::mpsc, task::JoinHandle};

pub(super) fn take(node: &mut Node) -> Activity {
    take_with_parent(node, node.cancel.clone())
}
fn take_with_parent(node: &mut Node, parent: Cancellation) -> Activity {
    let empty = Pump::new(node.context.clone(), fixture().identities[0].clone());
    Activity::new(
        std::mem::replace(&mut node.pump, empty),
        node.http.clone(),
        parent,
        authorized(),
        fixture().now,
    )
    .unwrap()
}
pub(super) fn run(mut activity: Activity) -> (JoinHandle<Failure>, mpsc::Receiver<Channels>) {
    let (tx, rx) = mpsc::channel(4);
    (
        tokio::spawn(async move {
            loop {
                match activity.next().await {
                    Ok(Event::Ready { channels, .. }) => tx.send(channels).await.unwrap(),
                    Ok(Event::Retired { error, .. }) => return Failure::Transport(error),
                    Err(failure) => return failure,
                }
            }
        }),
        rx,
    )
}
pub(super) async fn ready(rx: &mut mpsc::Receiver<Channels>) -> Channels {
    tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("real TLS handshake")
        .expect("channels")
}

#[tokio::test]
async fn account_activity_handshakes_transfers_both_directions_and_records_every_request() {
    measured_transfer(2 * 1024 * 1024 + 29).await;
}
#[tokio::test]
#[ignore = "explicit request-budget measurement; real TLS/FileIo, no Cloudflare account"]
async fn ten_mib_files_measure_daily_request_model() {
    measured_transfer(10 * 1024 * 1024).await;
}
async fn measured_transfer(file_bytes: usize) {
    let mut pair = Pair::unstarted().await;
    let started = Instant::now();
    let (a, mut arx) = run(take(&mut pair.nodes[0]));
    let (b, mut brx) = run(take(&mut pair.nodes[1]));
    let channels = [ready(&mut arx).await, ready(&mut brx).await];
    let mut directions = Vec::new();
    for sender in [0, 1] {
        let root = Root::new();
        let (resource, path) = root.source(file_bytes);
        let before = pair.relay.lock().unwrap().sends_by_device.len();
        let incoming = channels[1 - sender]
            .data
            .expect_file(resource.clone(), &root.0.join("staging"))
            .await
            .unwrap();
        let data = channels[sender].data.clone();
        let r = resource.clone();
        let sent = tokio::spawn(async move { data.send_file(r, &path).await });
        let verified = tokio::time::timeout(Duration::from_secs(40), incoming.finish())
            .await
            .expect("bounded transfer")
            .unwrap();
        sent.await.unwrap().unwrap();
        assert_eq!(verified.resource, resource);
        assert_eq!(
            raw_sha256(&fs::read(verified.path()).unwrap()),
            resource.sha256
        );
        drop(verified);
        pair.nodes[1 - sender]
            .context
            .io
            .run_in_round(&pair.nodes[1 - sender].cancel, |_| ())
            .await
            .unwrap();
        let state = pair.relay.lock().unwrap();
        let device = fixture().identities[sender].device_id().unwrap();
        let mut bins: [Vec<usize>; 2] = Default::default();
        for (id, bytes) in &state.sends_by_device[before..] {
            bins[usize::from(*id != device)].push(*bytes);
        }
        // Begin/End each wait for a peer response and therefore cannot share
        // the intervening data batches. Keep them in the total request ledger.
        let data_bins = &bins[0][1..bins[0].len()-1];
        directions.push(json!({"source":sender,"fileBytes":resource.size_bytes,
            "dataBatchMeanBytes":data_bins.iter().sum::<usize>() as f64/data_bins.len().max(1) as f64,
            "bulkDirectionIncludingBeginEnd":bins[0],"reverseIncludingFileAcks":bins[1],
            "bulkMeanBytes":bins[0].iter().sum::<usize>() as f64 / bins[0].len().max(1) as f64}));
    }
    let elapsed = started.elapsed();
    for node in &pair.nodes {
        node.cancel.cancel();
    }
    for owner in [a, b] {
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), owner)
                .await
                .unwrap()
                .unwrap(),
            Failure::Transport(Error::Cancelled)
        ));
    }
    {
        let s = pair.relay.lock().unwrap();
        assert_eq!(s.created, 4);
        assert!(s.send_sizes.iter().all(|n| (1..=MAX_BATCH).contains(n)));
        println!(
            "BBS_RELAY_ACTIVITY_COUNTS {}",
            json!({
                "fixture":"two fixed HTTP workers per account, in-process relay, real mutual TLS/FileIo, activity owner",
                "elapsedMs":elapsed.as_millis(),"fileBytes":2*file_bytes,
                "uploads":s.send_sizes.len(),"receive":s.receives,"acks":s.acknowledgements,"small":s.small_reads,
                "totalRequests":s.send_sizes.len()+s.receives+s.acknowledgements+s.small_reads,
                "receiveUploadRatio":s.receives as f64/s.send_sizes.len().max(1) as f64,
                "directions":directions,"allBatchSizes":s.send_sizes,"includesHandshakeAndReverse":true
            })
        );
    }
    pair.stop().await;
}

#[tokio::test]
async fn lost_upload_and_ack_replies_retry_the_original_proofs_without_reencrypting() {
    let mut pair = reads::settled().await;
    pair.relay.lock().unwrap().lose_replies = [1, 1];
    let (a, _) = run(take(&mut pair.nodes[0]));
    let (b, _) = run(take(&mut pair.nodes[1]));
    let channel = pair.nodes[0].channels.as_ref().unwrap().control.clone();
    let sent = tokio::spawn(async move { channel.send(b"one authoritative message").await });
    let incoming = &mut pair.nodes[1].channels.as_mut().unwrap().incoming;
    let delivery = tokio::time::timeout(Duration::from_secs(5), incoming.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(delivery.bytes(), b"one authoritative message");
    delivery.acknowledge().unwrap();
    sent.await.unwrap().unwrap();
    until(|| {
        let state = pair.relay.lock().unwrap();
        [true, false].into_iter().all(|kind| {
            state
                .attempts
                .iter()
                .any(|x| x.0 == kind && state.attempts.iter().filter(|y| *y == x).count() >= 2)
        })
    })
    .await;
    assert!(
        pair.nodes[1]
            .channels
            .as_mut()
            .unwrap()
            .incoming
            .try_recv()
            .is_err(),
        "no replayed plaintext"
    );
    for node in &pair.nodes {
        node.cancel.cancel();
    }
    let _ = a.await.unwrap();
    let _ = b.await.unwrap();
    pair.stop().await;
}

struct CountWake(AtomicUsize);
impl Wake for CountWake {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn byte_blocked_activity_does_not_loop_without_an_event_and_real_release_resumes_it() {
    let mut pair = reads::settled().await;
    let mut held = Vec::new();
    while let Ok(p) = pair.nodes[0].context.limits.reserve(MAX_FRAME) {
        held.push(p);
    }
    let mut owner = take(&mut pair.nodes[0]);
    let turns = owner.turns.clone();
    let before = pair.relay.lock().unwrap().receives;
    let counter = Arc::new(CountWake(AtomicUsize::new(0)));
    let waker: Waker = counter.clone().into();
    let mut cx = TaskContext::from_waker(&waker);
    {
        let mut next = Box::pin(owner.next());
        assert!(next.as_mut().poll(&mut cx).is_pending());
        let n = turns.load(Ordering::SeqCst);
        for _ in 0..100 {
            assert!(next.as_mut().poll(&mut cx).is_pending());
        }
        assert_eq!(
            turns.load(Ordering::SeqCst),
            n,
            "unrelated executor polls do not rerun the activity loop"
        );
        assert_eq!(pair.relay.lock().unwrap().receives, before);
        let wakes = counter.0.load(Ordering::SeqCst);
        drop(held);
        assert!(
            counter.0.load(Ordering::SeqCst) > wakes,
            "actual byte release wakes the existing waiter"
        );
        assert!(next.as_mut().poll(&mut cx).is_pending());
        until(|| pair.relay.lock().unwrap().receives > before).await;
        pair.nodes[0].cancel.cancel();
        assert!(matches!(
            next.await,
            Err(Failure::Transport(Error::Cancelled))
        ));
    }
    pair.stop().await;
}

#[tokio::test]
async fn cancel_and_rebind_retire_activity_while_a_fixed_http_worker_is_blocked() {
    for rebound in [false, true] {
        let limits = Limits::default();
        let (http, state, release) = fake_pool(limits.clone()).await;
        let (pump, context) = reads::bare_pump(limits).await;
        let cancel = Cancellation::default();
        let mut owner = Activity::new(
            pump,
            http.clone(),
            cancel.clone(),
            authorized(),
            fixture().now,
        )
        .unwrap();
        let pending = tokio::spawn(async move { owner.next().await });
        until(|| state.calls.load(Ordering::SeqCst) != 0).await;
        if rebound {
            http.pool.bind(&fixture().contexts[0].origin).unwrap();
        } else {
            cancel.cancel();
        }
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), pending)
                .await
                .unwrap()
                .unwrap(),
            Err(Failure::Transport(Error::Cancelled))
        ));
        assert_eq!(state.created.load(Ordering::SeqCst), 2);
        assert!(
            !*state.release.lock().unwrap(),
            "cancel did not wait for the blocked worker"
        );
        release.0.release();
        http.pool.stop();
        context.io.stop_and_wait().await;
    }
}

#[tokio::test]
async fn platform_error_retires_activity_as_resource_without_guessing_quota_or_retrying() {
    let mut pair = reads::settled().await;
    pair.relay.lock().unwrap().reject = Some(429);
    let mut owner = take(&mut pair.nodes[0]);
    let failure = owner.next().await;
    match failure {
        Err(Failure::Response {
            operation: Operation::Receive,
            error,
        }) => {
            assert_eq!(error, Error::CloudflareResourceLimit);
        }
        _ => panic!("only a fixed resource-limit error may leave the decoder"),
    }
    assert!(matches!(
        owner.next().await,
        Err(Failure::Transport(Error::Cancelled))
    ));
    pair.stop().await;
}

#[tokio::test]
async fn application_backpressure_retries_same_upload_and_ack_without_replaying_plaintext() {
    let mut pair = reads::settled().await;
    let before = pair.relay.lock().unwrap().attempts.len();
    pair.relay.lock().unwrap().busy_replies = [3, 2];
    let (a, _) = run(take(&mut pair.nodes[0]));
    let (b, _) = run(take(&mut pair.nodes[1]));
    let channel = pair.nodes[0].channels.as_ref().unwrap().control.clone();
    let sent = tokio::spawn(async move { channel.send(b"bounded application backpressure").await });
    let delivery = tokio::time::timeout(
        Duration::from_secs(8),
        pair.nodes[1].channels.as_mut().unwrap().incoming.recv(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(delivery.bytes(), b"bounded application backpressure");
    delivery.acknowledge().unwrap();
    sent.await.unwrap().unwrap();
    until(|| pair.relay.lock().unwrap().busy_replies == [0, 0]).await;
    {
        let state = pair.relay.lock().unwrap();
        for (kind, minimum) in [(true, 4), (false, 3)] {
            let attempts: Vec<_> = state.attempts[before..]
                .iter()
                .filter(|x| x.0 == kind)
                .collect();
            assert!(attempts.len() >= minimum);
            assert!(
                attempts[..minimum].iter().all(|x| *x == attempts[0]),
                "same proof, sequence and ciphertext digest"
            );
        }
    }
    assert!(pair.nodes[1]
        .channels
        .as_mut()
        .unwrap()
        .incoming
        .try_recv()
        .is_err());
    for node in &pair.nodes {
        node.cancel.cancel();
    }
    let _ = a.await.unwrap();
    let _ = b.await.unwrap();
    pair.stop().await;
}

#[tokio::test]
async fn repeated_lost_ack_reply_expires_at_the_original_deadline_without_resigning() {
    failed_ack_deadline(false).await;
}

#[tokio::test]
async fn repeated_ack_backpressure_expires_at_the_original_deadline_without_resigning() {
    failed_ack_deadline(true).await;
}

async fn failed_ack_deadline(busy: bool) {
    let mut pair = reads::settled().await;
    let before = pair.relay.lock().unwrap().attempts.len();
    if busy {
        pair.relay.lock().unwrap().busy_replies[1] = usize::MAX;
    } else {
        pair.relay.lock().unwrap().lose_replies[1] = usize::MAX;
    }
    let started = Instant::now();
    // Only the receiving owner runs here; the original sender's finite cycle
    // uploads one control message, so there can be only one ACK intent.
    let channel = pair.nodes[0].channels.as_ref().unwrap().control.clone();
    let sent = tokio::spawn(async move { channel.send(b"expire this HTTP intent").await });
    for _ in 0..100 {
        pair.nodes[0].cycle().await.unwrap();
        if pair.nodes[0].pump.unacknowledged().unwrap() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(pair.nodes[0].pump.unacknowledged().unwrap());
    // An activity epoch and a peer cancellation are distinct in the owner:
    // timeout retires this peer, not the whole account's explicit Cancel token.
    let (owner, _) = run(take_with_parent(
        &mut pair.nodes[1],
        Cancellation::default(),
    ));
    let delivery = tokio::time::timeout(
        Duration::from_secs(3),
        pair.nodes[1].channels.as_mut().unwrap().incoming.recv(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(delivery.bytes(), b"expire this HTTP intent");
    // The content consumer deliberately holds its independent credit. HTTP
    // receipt success/loss cannot count as application consumption.
    let result = tokio::time::timeout(PROGRESS_TIMEOUT + Duration::from_secs(6), owner)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(result, Failure::Transport(Error::Timeout)),
        "expected original ACK deadline; got {}",
        match &result {
            Failure::Transport(e) => e.to_string(),
            Failure::Response { .. } => "HTTP rejection".into(),
        }
    );
    assert!(started.elapsed() >= PROGRESS_TIMEOUT);
    {
        let s = pair.relay.lock().unwrap();
        let attempts: Vec<_> = s.attempts[before..].iter().filter(|x| !x.0).collect();
        assert!(
            attempts.len() > 3 && attempts.len() <= 24,
            "bounded retry backoff inside one original 20s deadline"
        );
        assert!(
            attempts.iter().all(|x| *x == attempts[0]),
            "all ACK proof fields and body remain byte-identical"
        );
    }
    drop(delivery);
    pair.nodes[0].cancel.cancel();
    assert!(sent.await.unwrap().is_err());
    pair.stop().await;
}

#[tokio::test]
async fn activity_cancellation_during_a_real_file_removes_partial_without_stopping_file_worker() {
    let mut pair = reads::settled().await;
    let root = Root::new();
    let (resource, path) = root.source(16 * 1024 * 1024 + 29);
    let data = pair.nodes[0].channels.as_ref().unwrap().data.clone();
    let incoming = pair.nodes[1]
        .channels
        .as_ref()
        .unwrap()
        .data
        .expect_file(resource.clone(), &root.0.join("staging"))
        .await
        .unwrap();
    let sent = tokio::spawn(async move { data.send_file(resource, &path).await });
    let received = tokio::spawn(incoming.finish());
    let (a, _) = run(take(&mut pair.nodes[0]));
    let (b, _) = run(take(&mut pair.nodes[1]));
    until(|| {
        fs::read_dir(root.0.join("staging"))
            .unwrap()
            .flatten()
            .any(|e| e.metadata().is_ok_and(|m| m.len() > 0))
    })
    .await;
    for node in &pair.nodes {
        node.cancel.cancel();
    }
    for owner in [a, b] {
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), owner)
                .await
                .unwrap()
                .unwrap(),
            Failure::Transport(Error::Cancelled)
        ));
    }
    assert!(tokio::time::timeout(Duration::from_secs(2), sent)
        .await
        .unwrap()
        .unwrap()
        .is_err());
    assert!(tokio::time::timeout(Duration::from_secs(2), received)
        .await
        .unwrap()
        .unwrap()
        .is_err());
    until(|| fs::read_dir(root.0.join("staging")).unwrap().count() == 0).await;
    // Retiring a network activity does not shut down the shared roster/file IO.
    assert_eq!(
        pair.nodes[1]
            .context
            .io
            .run_when_available(&Cancellation::default(), |_| 17)
            .await
            .unwrap(),
        17
    );
    pair.stop().await;
}
