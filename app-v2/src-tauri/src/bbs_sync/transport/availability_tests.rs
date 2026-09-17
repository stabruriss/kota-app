use super::*;
use std::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll, Wake, Waker},
};

#[derive(Default)]
struct Wakes(AtomicUsize);
impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}
fn poll<T>(future: Pin<&mut impl Future<Output = T>>, wake: &Arc<Wakes>) -> Poll<T> {
    future.poll(&mut Context::from_waker(&Waker::from(wake.clone())))
}

#[test]
fn byte_credit_waiters_wake_after_release_without_reserving_or_starving_small_frames() {
    let limits = Limits::default();
    let rest = limits.reserve(APP_QUEUE_BYTES - 2 * MAX_FRAME).unwrap();
    let first = limits.reserve(MAX_FRAME).unwrap();
    let second = limits.reserve(MAX_FRAME).unwrap();
    let wake = Arc::new(Wakes::default());
    let mut large = Box::pin(limits.wait_for_bytes(2 * MAX_FRAME));
    let mut small = Box::pin(limits.wait_for_bytes(MAX_FRAME));
    assert!(poll(large.as_mut(), &wake).is_pending());
    assert!(poll(small.as_mut(), &wake).is_pending());
    drop(limits.reserve(0).unwrap());
    assert_eq!(
        wake.0.load(Ordering::Relaxed),
        0,
        "zero bytes did not return credit"
    );
    drop(first);
    assert!(wake.0.load(Ordering::Relaxed) >= 2);
    assert!(poll(large.as_mut(), &wake).is_pending());
    assert_eq!(poll(small.as_mut(), &wake), Poll::Ready(Ok(())));
    // Readiness did not take bytes, and the older large waiter has not joined
    // semaphore admission ahead of this small application frame.
    let frame = limits.reserve(MAX_FRAME).unwrap();
    assert!(poll(large.as_mut(), &wake).is_pending());
    drop(frame);
    assert!(poll(large.as_mut(), &wake).is_pending());
    drop(second);
    assert_eq!(poll(large.as_mut(), &wake), Poll::Ready(Ok(())));
    let all = limits.reserve(2 * MAX_FRAME).unwrap();
    drop(all);
    drop(rest);
    assert!(limits.reserve(APP_QUEUE_BYTES).is_ok());
}

#[test]
fn byte_credit_release_before_first_await_and_abandoned_waits_cannot_lose_or_own_credit() {
    let limits = Limits::default();
    let held = limits.reserve(APP_QUEUE_BYTES).unwrap();
    let wake = Arc::new(Wakes::default());
    let mut abandoned = Box::pin(limits.wait_for_bytes(MAX_FRAME));
    assert!(poll(abandoned.as_mut(), &wake).is_pending());
    drop(abandoned);
    let mut before_poll = Box::pin(limits.wait_for_bytes(APP_QUEUE_BYTES));
    drop(held); // No first poll/registered waiter is needed for this release.
    assert_eq!(poll(before_poll.as_mut(), &wake), Poll::Ready(Ok(())));
    let held = limits.reserve(APP_QUEUE_BYTES).unwrap();
    let mut recheck = Box::pin(limits.wait_for_bytes(MAX_FRAME));
    assert!(poll(recheck.as_mut(), &wake).is_pending());
    drop(held);
    assert_eq!(poll(recheck.as_mut(), &wake), Poll::Ready(Ok(())));
}

#[test]
fn partition_credit_returns_to_parent_only_after_the_last_shared_owner() {
    let limits = Limits::default();
    let child = limits.partition(64 * 1024).unwrap();
    let held = Arc::new(child.reserve(64 * 1024).unwrap());
    let view = held.clone();
    let wake = Arc::new(Wakes::default());
    let mut parent = Box::pin(limits.wait_for_bytes(APP_QUEUE_BYTES));
    let mut invalid = Box::pin(child.wait_for_bytes(64 * 1024 + 1));
    assert_eq!(
        poll(invalid.as_mut(), &wake),
        Poll::Ready(Err(Error::Protocol))
    );
    drop(invalid);
    assert!(poll(parent.as_mut(), &wake).is_pending());
    drop(child);
    drop(held);
    assert!(poll(parent.as_mut(), &wake).is_pending());
    assert!(limits.reserve(APP_QUEUE_BYTES).is_err());
    drop(view);
    assert_eq!(poll(parent.as_mut(), &wake), Poll::Ready(Ok(())));
    assert!(limits.reserve(APP_QUEUE_BYTES).is_ok());
}
