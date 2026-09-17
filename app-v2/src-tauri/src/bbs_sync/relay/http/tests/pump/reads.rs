use super::*;
use crate::bbs_sync::relay::reads::{Demand, Outcome, ReadJob, Reads};

const DATA: Demand = Demand {
    receive: true,
    file_wait: false,
};
const WAIT: Demand = Demand {
    receive: false,
    file_wait: true,
};

fn dispatch(
    reads: &mut Reads,
    node: &Node,
    demand: Demand,
    base: Instant,
    ms: u64,
) -> Option<ReadJob> {
    reads
        .dispatch(
            &node.pump,
            &node.http,
            demand,
            authorized(),
            base + Duration::from_millis(ms),
            fixture().now + ms,
        )
        .unwrap()
}
async fn complete(reads: &mut Reads, node: &mut Node, job: ReadJob, base: Instant, ms: u64) {
    let result = job.wait().await;
    let Outcome::Applied(activity) = reads
        .finish(
            result,
            &mut node.pump,
            base + Duration::from_millis(ms),
            fixture().now + ms,
        )
        .unwrap()
    else {
        panic!("successful fixture receive")
    };
    assert_eq!(
        activity.tls_input, 0,
        "a read does not run TLS or fabricate progress"
    );
    assert_eq!(activity.framed, 0);
    assert!(activity.retired.is_empty());
}
pub(super) async fn settled() -> Pair {
    let mut p = Pair::new().await;
    for _ in 0..4 {
        p.cycle().await;
    }
    assert!(p.nodes.iter().all(|n| !n.pump.unacknowledged().unwrap()));
    p
}

#[tokio::test]
async fn empty_activity_reads_back_off_once_per_account_and_stop_without_idle_polling() {
    let mut pair = settled().await;
    let before = pair.relay.lock().unwrap().receives;
    let base = Instant::now();
    let mut reads = Reads::default();
    assert!(dispatch(&mut reads, &pair.nodes[0], DATA, base, 0).is_none());
    reads.begin(base, pair.nodes[0].cancel.clone()).unwrap();
    for (at, next) in [
        (0, 250),
        (250, 750),
        (750, 1750),
        (1750, 2750),
        (2750, 3750),
    ] {
        let job = dispatch(&mut reads, &pair.nodes[0], DATA, base, at).unwrap();
        assert!(job.mode() == ReceiveMode::Data);
        assert!(
            dispatch(&mut reads, &pair.nodes[0], DATA, base, at).is_none(),
            "no duplicate in-flight read"
        );
        complete(&mut reads, &mut pair.nodes[0], job, base, at).await;
        assert!(dispatch(&mut reads, &pair.nodes[0], DATA, base, next - 1).is_none());
        assert_eq!(
            reads.next_wake(&pair.nodes[0].pump, DATA, base).unwrap(),
            Some(base + Duration::from_millis(next))
        );
    }
    assert_eq!(pair.relay.lock().unwrap().receives - before, 5);
    reads.stop();
    assert!(dispatch(&mut reads, &pair.nodes[0], WAIT, base, 86_400_000).is_none());
    assert!(reads
        .next_wake(&pair.nodes[0].pump, WAIT, base)
        .unwrap()
        .is_none());
    assert_eq!(pair.relay.lock().unwrap().receives - before, 5);
    pair.stop().await;
}

#[tokio::test]
async fn exhausted_data_credit_uses_receipts_in_the_same_four_second_slot_without_extra_polls() {
    let mut pair = settled().await;
    let channel = pair.nodes[0].channels.as_ref().unwrap().control.clone();
    let sender = tokio::spawn(async move { channel.send(b"waiting for peer consumption").await });
    for _ in 0..100 {
        pair.nodes[0].cycle().await.unwrap();
        if pair.nodes[0].pump.unacknowledged().unwrap() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(pair.nodes[0].pump.unacknowledged().unwrap());
    let mut held = Vec::new();
    while let Ok(p) = pair.nodes[0].context.limits.reserve(MAX_FRAME) {
        held.push(p);
    }
    let before = {
        let s = pair.relay.lock().unwrap();
        (s.receives, s.small_reads)
    };
    let base = Instant::now();
    let mut reads = Reads::default();
    reads.begin(base, pair.nodes[0].cancel.clone()).unwrap();
    for at in [0, 250, 1000, 3999] {
        assert!(dispatch(&mut reads, &pair.nodes[0], DATA, base, at).is_none());
    }
    // 3999's failed byte admission remains known during its 250 ms retry gap.
    for at in [4000, 8000] {
        let job = dispatch(&mut reads, &pair.nodes[0], DATA, base, at).unwrap();
        assert!(job.mode() == ReceiveMode::Receipts);
        assert!(dispatch(&mut reads, &pair.nodes[0], DATA, base, at).is_none());
        complete(&mut reads, &mut pair.nodes[0], job, base, at).await;
        assert!(dispatch(&mut reads, &pair.nodes[0], DATA, base, at + 1).is_none());
    }
    {
        let s = pair.relay.lock().unwrap();
        assert_eq!((s.receives - before.0, s.small_reads - before.1), (0, 2));
    }
    assert!(
        pair.nodes[0].pump.unacknowledged().unwrap(),
        "old signed receipt is not new consumption"
    );
    drop(held);
    let job = dispatch(&mut reads, &pair.nodes[0], DATA, base, 8251).unwrap();
    assert!(job.mode() == ReceiveMode::Data);
    complete(&mut reads, &mut pair.nodes[0], job, base, 8251).await;
    reads.stop();
    pair.stop().await;
    assert!(matches!(
        sender.await.unwrap(),
        Err(Error::Cancelled) | Err(Error::Closed)
    ));
}

#[tokio::test]
async fn replaced_activity_and_dropped_results_cannot_rearm_or_clear_the_new_read() {
    let mut pair = settled().await;
    let base = Instant::now();
    let mut reads = Reads::default();
    reads.begin(base, pair.nodes[0].cancel.clone()).unwrap();
    let old = dispatch(&mut reads, &pair.nodes[0], DATA, base, 0)
        .unwrap()
        .wait()
        .await;
    reads.begin(base, pair.nodes[0].cancel.clone()).unwrap();
    let fresh = dispatch(&mut reads, &pair.nodes[0], DATA, base, 0).unwrap();
    assert!(matches!(
        reads.finish(old, &mut pair.nodes[0].pump, base, fixture().now),
        Err(Error::Cancelled)
    ));
    assert!(dispatch(&mut reads, &pair.nodes[0], DATA, base, 0).is_none());
    complete(&mut reads, &mut pair.nodes[0], fresh, base, 0).await;
    let dropped = dispatch(&mut reads, &pair.nodes[0], DATA, base, 250)
        .unwrap()
        .wait()
        .await;
    drop(dropped);
    assert!(dispatch(&mut reads, &pair.nodes[0], DATA, base, 250).is_none());
    let next = dispatch(&mut reads, &pair.nodes[0], DATA, base, 500).unwrap();
    let rebound = next.wait().await;
    pair.nodes[0].http = pair.nodes[0]
        .pool
        .bind(&fixture().contexts[0].origin)
        .unwrap();
    assert!(
        matches!(
            reads.finish(rebound, &mut pair.nodes[0].pump, base, fixture().now),
            Err(Error::Cancelled)
        ),
        "same-origin rebind invalidates an already returned response too"
    );
    reads.begin(base, pair.nodes[0].cancel.clone()).unwrap();
    let next = dispatch(&mut reads, &pair.nodes[0], DATA, base, 0).unwrap();
    reads.stop();
    let old = next.wait().await;
    assert!(matches!(
        reads.finish(old, &mut pair.nodes[0].pump, base, fixture().now),
        Err(Error::Cancelled)
    ));
    assert!(dispatch(&mut reads, &pair.nodes[0], WAIT, base, 4000).is_none());
    pair.stop().await;
}

pub(super) async fn bare_pump(limits: Limits) -> (Pump, Context) {
    let f = fixture();
    let client = PendingTls::client(
        &f.identities[0],
        f.contexts[0].clone(),
        authorized(),
        Cancellation::default(),
        f.now,
    )
    .unwrap();
    let server = PendingTls::server(
        &f.identities[1],
        f.contexts[1].clone(),
        authorized(),
        Cancellation::default(),
        client.statement(),
        f.now,
    )
    .unwrap();
    let tls = client.accept(server.statement(), f.now).unwrap();
    let context = Context {
        io: FileIo::start(limits.clone()).unwrap(),
        limits,
        shutdown: Cancellation::default(),
    };
    let mut pump = Pump::new(context.clone(), f.identities[0].clone());
    pump.add(tls).unwrap();
    (pump, context)
}

#[tokio::test]
async fn blocked_data_http_still_allows_one_small_probe_and_stop_returns_before_dns_like_wait() {
    let limits = Limits::default();
    let (http, state, release) = fake_pool(limits.clone()).await;
    let (mut pump, context) = bare_pump(limits).await;
    let base = Instant::now();
    let parent = Cancellation::default();
    let mut reads = Reads::default();
    reads.begin(base, parent).unwrap();
    let data = reads
        .dispatch(&pump, &http, DATA, authorized(), base, fixture().now)
        .unwrap()
        .unwrap();
    let blocked = tokio::spawn(data.wait());
    until(|| state.calls.load(Ordering::SeqCst) == 1).await;
    assert!(reads
        .dispatch(
            &pump,
            &http,
            WAIT,
            authorized(),
            base + Duration::from_millis(3999),
            fixture().now
        )
        .unwrap()
        .is_none());
    let small = reads
        .dispatch(
            &pump,
            &http,
            WAIT,
            authorized(),
            base + Duration::from_secs(4),
            fixture().now + 4000,
        )
        .unwrap()
        .unwrap();
    assert!(small.mode() == ReceiveMode::Probe);
    assert!(
        reads
            .dispatch(
                &pump,
                &http,
                WAIT,
                authorized(),
                base + Duration::from_secs(8),
                fixture().now + 8000
            )
            .unwrap()
            .is_none(),
        "one small flight, no stacked keepalive"
    );
    let completion = small.wait().await;
    assert!(matches!(
        reads.finish(
            completion,
            &mut pump,
            base + Duration::from_secs(4),
            fixture().now + 4000
        ),
        Ok(Outcome::Applied(_))
    ));
    assert_eq!(state.created.load(Ordering::SeqCst), 2);
    assert_eq!(state.calls.load(Ordering::SeqCst), 2);
    reads.stop();
    let old = tokio::time::timeout(Duration::from_secs(1), blocked)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        reads.finish(old, &mut pump, base, fixture().now),
        Err(Error::Cancelled)
    ));
    assert_eq!(
        state.created.load(Ordering::SeqCst),
        2,
        "never replace a blocked HTTP worker"
    );
    release.0.release();
    http.pool.stop();
    context.io.stop_and_wait().await;
}

#[tokio::test]
async fn a_full_data_queue_cannot_suppress_the_due_small_probe() {
    let limits = Limits::default();
    let (http, state, release) = fake_pool(limits.clone()).await;
    let (mut pump, context) = bare_pump(limits).await;
    let mut jobs = Vec::new();
    for i in 0..5 {
        jobs.push(spawn(
            &http,
            request(false),
            None,
            Cancellation::default(),
            authorized(),
            Instant::now() + PROGRESS_TIMEOUT,
        ));
        if i == 0 {
            until(|| state.calls.load(Ordering::SeqCst) == 1).await;
        }
    }
    until(|| http.pool.0.data.state.lock().unwrap().jobs.len() == 4).await;
    let base = Instant::now();
    let mut reads = Reads::default();
    reads.begin(base, Cancellation::default()).unwrap();
    let demand = Demand {
        receive: true,
        file_wait: true,
    };
    let due = base + Duration::from_secs(4);
    let probe = reads
        .dispatch(
            &pump,
            &http,
            demand,
            authorized(),
            due,
            fixture().now + 4000,
        )
        .unwrap();
    // Release blocked fixtures even when the assertion below fails.
    if probe.is_none() {
        release.0.release();
    }
    let probe = probe.expect("full large queue must still permit the independent small lane");
    assert!(
        probe.mode() == ReceiveMode::Probe,
        "queue pressure is not consumer/byte pressure"
    );
    assert!(matches!(
        reads.finish(probe.wait().await, &mut pump, due, fixture().now + 4000),
        Ok(Outcome::Applied(_))
    ));
    assert_eq!(state.created.load(Ordering::SeqCst), 2);
    reads.stop();
    release.0.release();
    for job in jobs {
        job.await.unwrap().unwrap();
    }
    http.pool.stop();
    context.io.stop_and_wait().await;
}

struct Reject;
impl Backend for Reject {
    fn discard(&mut self) {}
    fn run(
        &mut self,
        _: &Endpoint,
        _: &SignedRequest,
        _: Option<&Body>,
        response: &mut Buffer,
        guard: &Guard,
    ) -> Result<Parts> {
        guard.check()?;
        let body = b"<html>1027 maybe, not a trusted quota sample</html>";
        response.spare_mut()[..body.len()].copy_from_slice(body);
        response.advance(body.len())?;
        Ok(Parts {
            status: 429,
            headers: BTreeMap::from([("content-type".into(), vec!["text/html".into()])]),
        })
    }
}
#[tokio::test]
async fn rejected_status_stays_bounded_and_unclassified_without_faking_read_progress() {
    let limits = Limits::default();
    let pool = HttpPool::start_with(limits.clone(), |_| Box::new(Reject))
        .await
        .unwrap();
    let http = pool.bind(&fixture().contexts[0].origin).unwrap();
    let (mut pump, context) = bare_pump(limits).await;
    let base = Instant::now();
    let mut reads = Reads::default();
    reads.begin(base, Cancellation::default()).unwrap();
    let job = reads
        .dispatch(&pump, &http, DATA, authorized(), base, fixture().now)
        .unwrap()
        .unwrap();
    let Outcome::Rejected(reply) = reads
        .finish(job.wait().await, &mut pump, base, fixture().now)
        .unwrap()
    else {
        panic!("not a quota decoder")
    };
    assert_eq!(reply.status, 429);
    assert_eq!(
        reply.body.unwrap().bytes(),
        b"<html>1027 maybe, not a trusted quota sample</html>"
    );
    assert_eq!(
        reads.next_wake(&pump, DATA, base).unwrap(),
        Some(base + Duration::from_millis(250))
    );
    assert!(!pump.is_empty());
    reads.stop();
    pool.stop();
    context.io.stop_and_wait().await;
}

#[tokio::test]
async fn real_file_wait_keeps_small_probes_but_does_not_extend_the_existing_twenty_second_timeout()
{
    let mut pair = settled().await;
    let root = Root::new();
    let (resource, _) = root.source(1024);
    let incoming = pair.nodes[0]
        .channels
        .as_ref()
        .unwrap()
        .data
        .expect_file(resource, &root.0.join("staging"))
        .await
        .unwrap();
    let file = tokio::spawn(incoming.finish());
    // The receiver keeps the account file permit while waiting for Begin. A
    // real local roster/metadata waiter grants probe eligibility; the idle file
    // handle itself does not. Neither signal extends the DataPipe deadline.
    let mut work = pair.nodes[0].context.io.work_status();
    let cancel_wait = Cancellation::default();
    let waiting = tokio::spawn({
        let files = pair.nodes[0].context.io.clone();
        let cancel = cancel_wait.clone();
        async move { files.run_when_available(&cancel, |_| ()).await }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while *work.borrow_and_update() == 0 {
            work.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let base = Instant::now();
    let mut reads = Reads::default();
    reads.begin(base, pair.nodes[0].cancel.clone()).unwrap();
    let before = {
        let s = pair.relay.lock().unwrap();
        (s.receives, s.small_reads)
    };
    for _ in 0..4 {
        let demand = Demand {
            receive: false,
            file_wait: *work.borrow_and_update() != 0,
        };
        assert!(demand.file_wait);
        let due = reads
            .next_wake(&pair.nodes[0].pump, demand, Instant::now())
            .unwrap()
            .unwrap();
        tokio::select! {
            _=tokio::time::sleep_until(tokio::time::Instant::from_std(due))=>{},
            _=pair.nodes[0].cancel.cancelled()=>break,
        }
        if file.is_finished() {
            break;
        }
        let ms = base.elapsed().as_millis() as u64;
        let job = dispatch(&mut reads, &pair.nodes[0], demand, base, ms).unwrap();
        assert!(job.mode() == ReceiveMode::Probe);
        complete(&mut reads, &mut pair.nodes[0], job, base, ms).await;
    }
    cancel_wait.cancel();
    assert_eq!(waiting.await.unwrap(), Err(Error::Cancelled));
    assert_eq!(*work.borrow(), 0);
    let idle_file = Demand {
        receive: false,
        file_wait: *work.borrow() != 0,
    };
    assert!(reads
        .next_wake(&pair.nodes[0].pump, idle_file, Instant::now())
        .unwrap()
        .is_none());
    let result = tokio::time::timeout(Duration::from_secs(10), file)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(Error::Timeout)));
    assert!(base.elapsed() >= PROGRESS_TIMEOUT);
    assert!(base.elapsed() < PROGRESS_TIMEOUT + Duration::from_secs(8));
    let s = pair.relay.lock().unwrap();
    assert_eq!(s.receives, before.0);
    assert_eq!(s.small_reads - before.1, 4);
    drop(s);
    reads.stop();
    assert!(dispatch(&mut reads, &pair.nodes[0], WAIT, base, 60_000).is_none());
    pair.stop().await;
    assert_eq!(
        fs::read_dir(root.0.join("staging")).unwrap().count(),
        0,
        "the timed-out file's private partial is removed"
    );
}
