use super::*;
use crate::bbs_sync::relay::{
    coalesce::{Coalescer, MAX_COALESCE},
    http::HttpPool,
    MAX_BATCH,
};
use crate::bbs_sync::transport::MAX_FRAME;

#[tokio::test]
async fn membership_revocation_discards_unsubmitted_coalescing_without_waiting_for_its_timer() {
    let context = account_context();
    let pool = HttpPool::start(context.limits.clone()).await.unwrap();
    let http = pool.bind(&fixture().contexts[0].origin).unwrap();
    let active = Arc::new(AtomicBool::new(true));
    let (mut tls, _) = pair(1, active.clone());
    tls.write_plaintext(b"buffered before revocation").unwrap();
    let mut pump = Pump::new(context.clone(), fixture().identities[0].clone());
    pump.add(tls).unwrap();
    let mut coalesce = Coalescer::default();
    let now = Instant::now();
    assert!(coalesce
        .prepare(&mut pump, &http, now, fixture().now)
        .unwrap()
        .is_none());
    assert_eq!(coalesce.next_wake(true, true), Some(now + MAX_COALESCE));
    active.store(false, Ordering::Release);
    assert!(matches!(
        coalesce.prepare(&mut pump, &http, now, fixture().now),
        Err(Error::Unauthorized)
    ));
    assert!(coalesce.next_wake(true, true).is_none());
    drop(pump);
    pool.stop();
    drop(http);
    drop(pool);
    context.io.stop_and_wait().await;
    assert!(context.limits.reserve(APP_QUEUE_BYTES).is_ok());
}

#[tokio::test]
async fn full_ciphertext_batch_flushes_before_the_non_sliding_deadline_without_a_second_copy() {
    let context = account_context();
    let pool = HttpPool::start(context.limits.clone()).await.unwrap();
    let http = pool.bind(&fixture().contexts[0].origin).unwrap();
    let mut pump = Pump::new(context.clone(), fixture().identities[0].clone());
    let (tls, _) = pair(1, Arc::new(AtomicBool::new(true)));
    let id = pump.add(tls).unwrap();
    let mut coalesce = Coalescer::default();
    let at = Instant::now();
    let mut ready = None;
    for _ in 0..32 {
        assert_eq!(
            pump.peer_mut(&id)
                .unwrap()
                .tls
                .write_plaintext(&[7; MAX_FRAME])
                .unwrap(),
            MAX_FRAME
        );
        ready = coalesce
            .prepare(&mut pump, &http, at, fixture().now)
            .unwrap();
        if ready.is_some() {
            break;
        }
        assert_eq!(coalesce.next_wake(true, false), Some(at + MAX_COALESCE));
    }
    let ready = ready.expect("full batch flushes even while clock has not advanced");
    assert_eq!(ready.batch.payload.len(), MAX_BATCH);
    let held = pump.retry(&id, ready.batch.sequence).unwrap();
    assert!(held.payload.shares(&ready.batch.payload));
    drop(held);
    drop(ready);
    coalesce.clear();
    drop(pump);
    pool.stop();
    drop(http);
    drop(pool);
    context.io.stop_and_wait().await;
    assert!(context.limits.reserve(APP_QUEUE_BYTES).is_ok());
}

#[tokio::test]
async fn missing_headroom_does_not_drain_tls_and_later_block_exhaustion_flushes_the_owned_prefix() {
    let context = account_context();
    let pool = HttpPool::start(context.limits.clone()).await.unwrap();
    let http = pool.bind(&fixture().contexts[0].origin).unwrap();
    let mut pump = Pump::new(context.clone(), fixture().identities[0].clone());
    let (tls, _) = pair(1, Arc::new(AtomicBool::new(true)));
    let id = pump.add(tls).unwrap();
    let at = Instant::now();
    let mut coalesce = Coalescer::default();
    let mut held = Vec::new();
    while let Ok(p) = context.limits.reserve(MAX_FRAME) {
        held.push(p);
    }
    pump.peer_mut(&id)
        .unwrap()
        .tls
        .write_plaintext(&[9; MAX_FRAME])
        .unwrap();
    assert!(coalesce
        .prepare(&mut pump, &http, at, fixture().now)
        .unwrap()
        .is_none());
    assert!(coalesce.next_wake(true, true).is_none());
    assert!(
        pump.has_output(&id).unwrap(),
        "no speculative TLS drain before HTTP headroom"
    );
    // Six headroom blocks + one ciphertext block, less than a TLS record here.
    for _ in 0..7 {
        held.pop().unwrap();
    }
    let ready = coalesce
        .prepare(&mut pump, &http, at, fixture().now)
        .unwrap()
        .unwrap();
    assert_eq!(ready.batch.payload.len(), MAX_FRAME);
    assert!(pump.peer_mut(&id).unwrap().tls.wants_write().unwrap());
    assert!(coalesce.next_wake(true, true).is_none());
    drop(ready);
    drop(held);
    drop(pump);
    pool.stop();
    drop(http);
    drop(pool);
    context.io.stop_and_wait().await;
}

#[tokio::test]
async fn four_output_peers_rotate_without_four_headroom_reservations_or_idle_timers() {
    let context = account_context();
    let pool = HttpPool::start(context.limits.clone()).await.unwrap();
    let http = pool.bind(&fixture().contexts[0].origin).unwrap();
    let mut pump = Pump::new(context.clone(), fixture().identities[0].clone());
    for n in 1..=4 {
        let (mut tls, _) = pair(n, Arc::new(AtomicBool::new(true)));
        tls.write_plaintext(&[n; 64]).unwrap();
        pump.add(tls).unwrap();
    }
    let ids = pump.sessions();
    let mut seen = Vec::new();
    let mut coalesce = Coalescer::default();
    let at = Instant::now();
    for i in 0..4 {
        let now = at + MAX_COALESCE * (i * 2);
        assert!(coalesce
            .prepare(&mut pump, &http, now, fixture().now)
            .unwrap()
            .is_none());
        let ready = coalesce
            .prepare(&mut pump, &http, now + MAX_COALESCE, fixture().now)
            .unwrap()
            .unwrap();
        seen.push(ready.session.clone());
        // Keep the previously served peer eligible; others must still rotate.
        pump.peer_mut(&ready.session)
            .unwrap()
            .tls
            .write_plaintext(b"more")
            .unwrap();
        drop(ready);
    }
    assert_eq!(seen, ids);
    coalesce.clear();
    assert!(coalesce.next_wake(true, true).is_none());
    drop(pump);
    pool.stop();
    drop(http);
    drop(pool);
    context.io.stop_and_wait().await;
}
