use super::*;
use crate::bbs_sync::relay::{
    coalesce::{Coalescer, MAX_COALESCE},
    reads::{Completion, Demand, Outcome, Reads},
};
use tokio::task::{JoinHandle, JoinSet};

async fn queued(node: &mut Node, bytes: &'static [u8]) -> JoinHandle<Result<()>> {
    let channel = node.channels.as_ref().unwrap().control.clone();
    let sender = tokio::spawn(async move { channel.send(bytes).await });
    let wake = node.pump.wake();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let notified = wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            assert!(node.pump.step().unwrap().retired.is_empty());
            if node.pump.has_output(&node.session).unwrap() {
                return;
            }
            notified.await;
        }
    })
    .await
    .expect("actual queued frame wakes the owner without polling");
    sender
}

#[tokio::test]
async fn output_wakes_the_owner_and_more_control_bytes_do_not_extend_the_first_deadline() {
    let mut pair = reads::settled().await;
    let mut coalesce = Coalescer::default();
    let node = &mut pair.nodes[0];
    let base = Instant::now();
    assert!(coalesce.next_wake(true, true).is_none());
    assert!(coalesce
        .prepare(&mut node.pump, &node.http, base, fixture().now)
        .unwrap()
        .is_none());
    let first = queued(node, b"first control").await;
    assert!(coalesce
        .prepare(&mut node.pump, &node.http, base, fixture().now)
        .unwrap()
        .is_none());
    let second = queued(node, b"second control").await;
    let last = base + MAX_COALESCE - Duration::from_millis(1);
    assert!(coalesce
        .prepare(&mut node.pump, &node.http, last, fixture().now)
        .unwrap()
        .is_none());
    assert_eq!(coalesce.next_wake(true, false), Some(base + MAX_COALESCE));
    let ready = coalesce
        .prepare(
            &mut node.pump,
            &node.http,
            base + MAX_COALESCE,
            fixture().now,
        )
        .unwrap()
        .unwrap();
    let sequence = ready.batch.sequence;
    let proof = ready.batch.request.headers().unwrap();
    let digest = ready.batch.payload.digest().to_owned();
    let reply = ready.submit(&mut node.pump).unwrap().wait().await.unwrap();
    assert_eq!(reply.status, 200);
    drop(reply);
    let retry = node.pump.retry(&node.session, sequence).unwrap();
    assert_eq!(retry.request.headers().unwrap(), proof);
    assert_eq!(retry.payload.digest(), digest);
    drop(retry);
    assert!(coalesce.next_wake(true, true).is_none());
    pair.nodes[1].cycle().await.unwrap();
    for bytes in [b"first control".as_slice(), b"second control".as_slice()] {
        let delivered = tokio::time::timeout(
            Duration::from_secs(5),
            pair.nodes[1].channels.as_mut().unwrap().incoming.recv(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(delivered.bytes(), bytes);
        delivered.acknowledge().unwrap();
    }
    pair.until(|_| first.is_finished() && second.is_finished())
        .await;
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    pair.stop().await;
}

#[tokio::test]
async fn cumulative_receipt_is_signed_at_the_fixed_deadline_and_not_replaced_in_flight() {
    let mut pair = reads::settled().await;
    let mut coalesce = Coalescer::default();
    let mut producers = Vec::new();
    let base = Instant::now();
    for (i, text) in [b"one".as_slice(), b"two".as_slice()]
        .into_iter()
        .enumerate()
    {
        producers.push(queued(&mut pair.nodes[0], text).await);
        pair.nodes[0].cycle().await.unwrap();
        let n = &mut pair.nodes[1];
        let request = n.pump.read(ReceiveMode::Data, fixture().now).unwrap();
        let reply = n
            .http
            .execute(
                request,
                None,
                n.cancel.clone(),
                authorized(),
                Instant::now() + PROGRESS_TIMEOUT,
            )
            .await
            .unwrap();
        n.pump
            .accept(reply.into_receive().unwrap(), fixture().now)
            .unwrap();
        n.pump.step().unwrap();
        let at = base + Duration::from_millis(i as u64 * 49);
        assert!(coalesce
            .acknowledgement(&mut n.pump, at, fixture().now)
            .unwrap()
            .is_none());
        assert_eq!(coalesce.next_wake(false, true), Some(base + MAX_COALESCE));
    }
    let n = &mut pair.nodes[1];
    let (id, ack) = coalesce
        .acknowledgement(&mut n.pump, base + MAX_COALESCE, fixture().now)
        .unwrap()
        .unwrap();
    assert!(ack.through >= 2);
    let same = coalesce
        .acknowledgement(
            &mut n.pump,
            base + Duration::from_secs(1),
            fixture().now + 1000,
        )
        .unwrap()
        .unwrap()
        .1;
    assert_eq!(same.sequence, ack.sequence);
    assert_eq!(same.body, ack.body);
    assert_eq!(
        same.request.headers().unwrap(),
        ack.request.headers().unwrap()
    );
    assert!(
        coalesce.next_wake(false, false).is_none(),
        "in-flight receipts must not cause a timer spin"
    );
    n.pump.ack_submitted(&id, ack.sequence).unwrap();
    assert!(coalesce
        .acknowledgement(&mut n.pump, base + Duration::from_secs(1), fixture().now)
        .unwrap()
        .is_none());
    assert!(coalesce.next_wake(true, true).is_none());
    pair.stop().await;
    for producer in producers {
        let _ = producer.await.unwrap();
    }
}

#[tokio::test]
async fn cancel_or_same_origin_rebind_prevents_a_buffered_batch_from_being_submitted() {
    for rebind in [false, true] {
        let mut pair = reads::settled().await;
        let n = &mut pair.nodes[0];
        let pending = queued(n, b"not sent after retirement").await;
        let mut coalesce = Coalescer::default();
        let now = Instant::now();
        assert!(coalesce
            .prepare(&mut n.pump, &n.http, now, fixture().now)
            .unwrap()
            .is_none());
        let before = pair.relay.lock().unwrap().send_sizes.len();
        if rebind {
            n.pool.bind(&fixture().contexts[0].origin).unwrap();
        } else {
            n.cancel.cancel();
        }
        assert!(matches!(
            coalesce.prepare(&mut n.pump, &n.http, now + MAX_COALESCE, fixture().now),
            Err(Error::Cancelled)
        ));
        coalesce.clear();
        assert!(coalesce.next_wake(true, true).is_none());
        assert_eq!(pair.relay.lock().unwrap().send_sizes.len(), before);
        pair.stop().await;
        let _ = pending.await.unwrap();
    }
}

enum Finished {
    Read(Completion),
    Upload(Result<Reply>),
    Ack(String, u64, Result<Reply>),
}

/// Test owner, not the final production activity/coordinator: finite selectors
/// composed using real channel wakeups and their next deadlines. No 1 ms loop,
/// extra blocking workers, speculative quota decoder or file-progress clock.
struct Drive {
    node: Node,
    reads: Reads,
    coalesce: Coalescer,
    jobs: JoinSet<Finished>,
    upload: bool,
    ack: bool,
}
impl Drive {
    async fn run(mut self) -> Node {
        let base = Instant::now();
        self.reads.begin(base, self.node.cancel.clone()).unwrap();
        let wake = self.node.pump.wake();
        let mut file_work = self.node.context.io.work_status();
        loop {
            if self.node.cancel.is_cancelled() {
                break;
            }
            let notified = wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let now = Instant::now();
            let ms = fixture().now + base.elapsed().as_millis() as u64;
            let moved = self.node.pump.step().unwrap();
            assert!(moved.retired.is_empty());
            if !self.upload {
                if let Some(ready) = self
                    .coalesce
                    .prepare(&mut self.node.pump, &self.node.http, now, ms)
                    .unwrap()
                {
                    let pending = ready.submit(&mut self.node.pump).unwrap();
                    self.upload = true;
                    self.jobs
                        .spawn(async move { Finished::Upload(pending.wait().await) });
                }
            }
            if !self.ack {
                if let Some((session, ack)) = self
                    .coalesce
                    .acknowledgement(&mut self.node.pump, now, ms)
                    .unwrap()
                {
                    let body = self.node.http.small_body(ack.body.as_bytes()).unwrap();
                    let http = self.node.http.clone();
                    let cancel = self.node.cancel.clone();
                    self.ack = true;
                    self.jobs.spawn(async move {
                        Finished::Ack(
                            session,
                            ack.sequence,
                            http.execute(
                                ack.request,
                                Some(body.into()),
                                cancel,
                                authorized(),
                                Instant::now() + PROGRESS_TIMEOUT,
                            )
                            .await,
                        )
                    });
                }
            }
            let demand = Demand {
                receive: true,
                file_wait: *file_work.borrow_and_update() != 0,
            };
            // Reads itself permits only one request in each lane.
            for _ in 0..2 {
                if let Some(read) = self
                    .reads
                    .dispatch(
                        &self.node.pump,
                        &self.node.http,
                        demand,
                        authorized(),
                        now,
                        ms,
                    )
                    .unwrap()
                {
                    self.jobs
                        .spawn(async move { Finished::Read(read.wait().await) });
                }
            }
            assert!(self.jobs.len() <= 4);
            if moved.framed != 0 || moved.tls_input != 0 {
                // Run producers freed by plaintext admission before inspecting
                // the next already-available bytes. A bounded Pump step precedes
                // every yield; empty work sleeps on actual events below.
                tokio::task::yield_now().await;
                continue;
            }
            let at = self
                .coalesce
                .next_wake(!self.upload, !self.ack)
                .into_iter()
                .chain(self.reads.next_wake(&self.node.pump, demand, now).unwrap())
                .min();
            tokio::select! {
                biased;
                _ = self.node.cancel.cancelled() => break,
                result = self.jobs.join_next(), if !self.jobs.is_empty() => {
                    let now = Instant::now();
                    let ms = fixture().now + base.elapsed().as_millis() as u64;
                    match result.unwrap().unwrap() {
                        Finished::Read(value) => assert!(matches!(self.reads.finish(value, &mut self.node.pump, now, ms).unwrap(), Outcome::Applied(_))),
                        Finished::Upload(reply) => {
                            self.upload = false;
                            assert_eq!(reply.unwrap().status, 200);
                        },
                        Finished::Ack(session, seq, reply) => {
                            self.ack = false;
                            assert_eq!(reply.unwrap().status, 200);
                            self.node.pump.ack_submitted(&session, seq).unwrap();
                        },
                    }
                },
                _ = notified => {},
                _ = file_work.changed() => {},
                _ = async { match at {
                    Some(at) => tokio::time::sleep_until(at.into()).await,
                    None => std::future::pending::<()>().await,
                } } => {},
            }
        }
        self.reads.stop();
        self.coalesce.clear();
        self.jobs.abort_all();
        while self.jobs.join_next().await.is_some() {}
        self.node
    }
}

#[tokio::test]
async fn wake_driven_coalescing_transfers_real_files_both_ways_and_counts_reverse_ack_requests() {
    let pair = reads::settled().await;
    let channels = pair
        .nodes
        .each_ref()
        .map(|n| n.channels.as_ref().unwrap().data.clone());
    let cancels = pair.nodes.each_ref().map(|n| n.cancel.clone());
    let files = pair.nodes.each_ref().map(|n| n.context.io.clone());
    let relay = pair.relay.clone();
    let handles = pair.nodes.map(|node| {
        tokio::spawn(
            Drive {
                node,
                reads: Reads::default(),
                coalesce: Coalescer::default(),
                jobs: JoinSet::new(),
                upload: false,
                ack: false,
            }
            .run(),
        )
    });
    let started = Instant::now();
    for sender in [0, 1] {
        let root = Root::new();
        let (resource, path) = root.source(2 * 1024 * 1024 + 29);
        let incoming = channels[1 - sender]
            .expect_file(resource.clone(), &root.0.join("staging"))
            .await
            .unwrap();
        let data = channels[sender].clone();
        let r = resource.clone();
        let sent = tokio::spawn(async move { data.send_file(r, &path).await });
        let received = tokio::spawn(incoming.finish());
        let (sent, received) = tokio::time::timeout(Duration::from_secs(40), async {
            (sent.await.unwrap(), received.await.unwrap())
        })
        .await
        .expect("whole-file transfer with real wakes");
        sent.unwrap();
        let verified = received.unwrap();
        assert_eq!(verified.resource, resource);
        assert_eq!(
            raw_sha256(&fs::read(verified.path()).unwrap()),
            resource.sha256
        );
        // Release the completed incoming file permit before reversing direction.
        // Production hands VerifiedFile to the installer; this fixture checks it here.
        drop(verified);
        // Drop queues Cleanup; wait for the same worker/permit instead of
        // relying on the next source-file setup being slow enough for cleanup.
        files[1 - sender]
            .run_in_round(&cancels[1 - sender], |_| ())
            .await
            .unwrap();
    }
    let elapsed = started.elapsed();
    for c in cancels {
        c.cancel();
    }
    let mut nodes = Vec::new();
    for handle in handles {
        nodes.push(
            tokio::time::timeout(Duration::from_secs(5), handle)
                .await
                .unwrap()
                .unwrap(),
        );
    }
    let state = relay.lock().unwrap();
    assert_eq!(
        state.created, 4,
        "exactly two fixed HTTP workers per account"
    );
    println!(
        "BBS_RELAY_COALESCE_COUNTS {}",
        json!({
            "fixture":"in-process relay, real TLS/FileIo, test activity owner",
            "fileBytes":2*(2*1024*1024+29),"coalesceMs":MAX_COALESCE.as_millis(),
            "elapsedMs":elapsed.as_millis(),"uploads":state.send_sizes.len(),"receive":state.receives,
            "acks":state.acknowledgements,"smallReads":state.small_reads,
            "ciphertextBytes":state.send_sizes.iter().sum::<usize>(),"batchSizes":state.send_sizes,
            "includesSetupHandshake":true
        })
    );
    // A real background-QoS producer need not fill 256 KiB within 50 ms.
    // The deterministic capacity test separately forces and checks a full flush.
    // This run verifies framing/ownership/SHA and records, not blesses, request cost.
    assert!(state.send_sizes.iter().all(|n| (1..=MAX_BATCH).contains(n)));
    drop(state);
    for n in nodes {
        n.pool.stop();
        n.context.io.stop_and_wait().await;
    }
}
