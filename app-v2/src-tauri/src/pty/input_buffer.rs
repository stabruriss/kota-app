//! Admission and completion state for one PTY incarnation. No lock from this
//! module is held while starting a process, writing a PTY, or waiting for I/O.
use std::collections::VecDeque;
use std::sync::{mpsc, Mutex};

const MAX_PENDING_BYTES: usize = 1024 * 1024;
const MAX_PENDING_ITEMS: usize = 1024;
pub(super) type Receipt = mpsc::SyncSender<Result<(), String>>;

pub(super) struct Input {
    pub text: String,
    receipt: Option<Receipt>,
}

impl Input {
    pub fn complete(self, result: Result<(), String>) {
        if let Some(receipt) = self.receipt {
            let _ = receipt.send(result);
        }
    }
}

pub(super) enum Admission {
    Direct(Input),
    Queued { start: bool },
}

pub(super) enum AfterFailure {
    Closed,
    RetryStart,
    KeepRunning,
}

#[derive(Default, PartialEq)]
enum Phase {
    #[default]
    Idle,
    Starting,
    Flushing,
    Ready,
    Closed,
}

#[derive(Default)]
struct State {
    phase: Phase,
    held: bool,
    bytes: usize,
    inputs: VecDeque<Input>,
}

#[derive(Default)]
pub(super) struct InputBuffer {
    state: Mutex<State>,
}

impl InputBuffer {
    pub fn hold_start(&self) {
        self.state.lock().unwrap().held = true;
    }
    pub fn release_start(&self) {
        self.state.lock().unwrap().held = false;
    }
    pub fn is_closed(&self) -> bool {
        self.state.lock().unwrap().phase == Phase::Closed
    }

    pub fn admit(
        &self,
        text: String,
        receipt: Option<Receipt>,
        running: bool,
    ) -> Result<Admission, String> {
        let mut state = self.state.lock().unwrap();
        if state.phase == Phase::Closed {
            return Err("agent closed or replaced; input not delivered".into());
        }
        let input = Input { text, receipt };
        if state.phase == Phase::Ready && running {
            return Ok(Admission::Direct(input));
        }
        if state.bytes.saturating_add(input.text.len()) > MAX_PENDING_BYTES
            || state.inputs.len() >= MAX_PENDING_ITEMS
        {
            return Err("agent startup input buffer is full; input not delivered".into());
        }
        if state.phase == Phase::Ready {
            state.phase = Phase::Idle;
        }
        state.bytes += input.text.len();
        state.inputs.push_back(input);
        let start = state.phase == Phase::Idle && !state.held;
        if start {
            state.phase = Phase::Starting;
        }
        Ok(Admission::Queued { start })
    }

    /// The closure must only install process state; no I/O or waiting.
    pub fn install_if_open(&self, install: impl FnOnce()) -> bool {
        let state = self.state.lock().unwrap();
        if state.phase == Phase::Closed {
            return false;
        }
        install();
        true
    }

    pub fn next(&self) -> Option<Input> {
        let mut state = self.state.lock().unwrap();
        if state.phase == Phase::Closed {
            return None;
        }
        if let Some(input) = state.inputs.pop_front() {
            state.phase = Phase::Flushing;
            state.bytes -= input.text.len();
            Some(input)
        } else {
            state.phase = Phase::Ready;
            None
        }
    }

    pub fn fail(&self, error: &str, after: AfterFailure) -> usize {
        let inputs = {
            let mut state = self.state.lock().unwrap();
            if state.phase != Phase::Closed {
                state.phase = match after {
                    AfterFailure::Closed => Phase::Closed,
                    AfterFailure::RetryStart => Phase::Idle,
                    AfterFailure::KeepRunning => Phase::Ready,
                };
            }
            state.bytes = 0;
            std::mem::take(&mut state.inputs)
        };
        let count = inputs.len();
        for input in inputs {
            input.complete(Err(error.to_string()));
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_start_and_flush_share_one_ordered_buffer() {
        let queue = InputBuffer::default();
        assert!(matches!(
            queue.admit("a".into(), None, false).unwrap(),
            Admission::Queued { start: true }
        ));
        assert!(matches!(
            queue.admit("b".into(), None, false).unwrap(),
            Admission::Queued { start: false }
        ));
        assert_eq!(queue.next().unwrap().text, "a");
        // The process now exists, but new writes must not bypass the flush.
        assert!(matches!(
            queue.admit("c".into(), None, true).unwrap(),
            Admission::Queued { start: false }
        ));
        assert_eq!(queue.next().unwrap().text, "b");
        assert_eq!(queue.next().unwrap().text, "c");
        assert!(queue.next().is_none());
        assert!(matches!(
            queue.admit("d".into(), None, true).unwrap(),
            Admission::Direct(_)
        ));
    }

    #[test]
    fn replacement_can_buffer_until_old_process_stops() {
        let queue = InputBuffer::default();
        queue.hold_start();
        assert!(matches!(
            queue.admit("a".into(), None, false).unwrap(),
            Admission::Queued { start: false }
        ));
        queue.release_start();
        assert!(matches!(
            queue.admit(String::new(), None, false).unwrap(),
            Admission::Queued { start: true }
        ));
        assert_eq!(queue.next().unwrap().text, "a");
    }

    #[test]
    fn closure_and_failure_report_undelivered_inputs() {
        let queue = InputBuffer::default();
        let (send, receive) = mpsc::sync_channel(1);
        queue.admit("prompt".into(), Some(send), false).unwrap();
        assert_eq!(queue.fail("closed", AfterFailure::Closed), 1);
        assert_eq!(receive.recv().unwrap(), Err("closed".into()));
        assert!(queue.next().is_none());
        assert!(queue.admit("later".into(), None, false).is_err());
        let replacement = InputBuffer::default();
        assert!(replacement.next().is_none());
    }

    #[test]
    fn pending_input_is_bounded_and_failed_start_can_retry() {
        let queue = InputBuffer::default();
        queue
            .admit("x".repeat(MAX_PENDING_BYTES), None, false)
            .unwrap();
        assert!(queue.admit("x".into(), None, false).is_err());
        queue.fail("startup failed", AfterFailure::RetryStart);
        assert!(matches!(
            queue.admit("retry".into(), None, false).unwrap(),
            Admission::Queued { start: true }
        ));
    }

    #[test]
    fn delivery_failure_does_not_restart_a_live_process_or_replay_pending_inputs() {
        let queue = InputBuffer::default();
        queue.admit("first".into(), None, false).unwrap();
        queue.next().unwrap().complete(Err("write failed".into()));
        let (send, receive) = mpsc::sync_channel(1);
        queue.admit("pending".into(), Some(send), true).unwrap();
        assert_eq!(queue.fail("write failed", AfterFailure::KeepRunning), 1);
        assert_eq!(receive.recv().unwrap(), Err("write failed".into()));
        assert!(matches!(
            queue.admit("new".into(), None, true).unwrap(),
            Admission::Direct(_)
        ));
        queue.fail("closed", AfterFailure::Closed);
        queue.fail("late failure", AfterFailure::KeepRunning);
        assert!(queue.admit("too late".into(), None, true).is_err());
    }

    #[test]
    fn concurrent_producers_keep_their_order_and_start_exactly_once() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Barrier,
        };
        let queue = Arc::new(InputBuffer::default());
        let barrier = Arc::new(Barrier::new(4));
        let starts = Arc::new(AtomicUsize::new(0));
        let mut producers = Vec::new();
        for producer in 0..4 {
            let queue = Arc::clone(&queue);
            let barrier = Arc::clone(&barrier);
            let starts = Arc::clone(&starts);
            producers.push(std::thread::spawn(move || {
                barrier.wait();
                for sequence in 0..50 {
                    if matches!(
                        queue
                            .admit(format!("{producer}:{sequence}"), None, false)
                            .unwrap(),
                        Admission::Queued { start: true }
                    ) {
                        starts.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }));
        }
        for producer in producers {
            producer.join().unwrap();
        }
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        let mut expected = [0; 4];
        while let Some(input) = queue.next() {
            let (producer, sequence) = input.text.split_once(':').unwrap();
            let producer: usize = producer.parse().unwrap();
            assert_eq!(sequence.parse::<usize>().unwrap(), expected[producer]);
            expected[producer] += 1;
        }
        assert_eq!(expected, [50; 4]);
    }
}
