//! Finite activity-read selection. Clock inputs are explicit; this owns no
//! timer/thread, identity, progress deadline, file permit or public sync phase.
use super::{
    http::{HttpClient, PendingHttp, Reply, RECEIVE_ADMISSION_BYTES},
    proof::ReceiveMode,
    pump::{Activity, Pump},
    Error, Result,
};
use crate::bbs_sync::transport::{Cancellation, MembershipCheck, PROGRESS_TIMEOUT};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

const PROBE_INTERVAL: Duration = Duration::from_secs(4);
const RETRY_ADMISSION: Duration = Duration::from_millis(250);

/// Trusted local work facts, not peer-provided claims. `file_wait` grants only
/// a small liveness read; it does not assert disk progress or extend a timeout.
#[derive(Clone, Copy)]
pub(super) struct Demand {
    pub(super) receive: bool,
    pub(super) file_wait: bool,
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    generation: u64,
    id: u64,
    mode: ReceiveMode,
}
impl Stamp {
    fn lane(self) -> usize {
        usize::from(self.mode != ReceiveMode::Data)
    }
}
struct Flight {
    stamp: Stamp,
    abandoned: Arc<AtomicBool>,
}
struct Lease(Arc<AtomicBool>);
impl Drop for Lease {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
struct Active {
    generation: u64,
    parent: Cancellation,
    cancel: Cancellation,
    flights: [Option<Flight>; 2],
    data_at: Instant,
    status_at: Instant,
    empty: u8,
    byte_blocked: bool,
}
impl Drop for Active {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
#[derive(Default)]
pub(super) struct Reads {
    generation: u64,
    next_id: u64,
    active: Option<Active>,
}
pub(super) struct ReadJob {
    stamp: Stamp,
    binding: HttpClient,
    parent: Cancellation,
    pending: PendingHttp,
    lease: Lease,
}
pub(super) struct Completion {
    stamp: Stamp,
    binding: HttpClient,
    result: Result<Reply>,
    _lease: Lease,
}
pub(super) enum Outcome {
    Applied(Activity),
    /// Preserve the bounded response for the later authoritative decoder. No
    /// HTML/status-code guess is made here about quota, boot or App versions.
    Rejected(Reply),
}
impl ReadJob {
    pub(super) fn mode(&self) -> ReceiveMode {
        self.stamp.mode
    }
    pub(super) async fn wait(self) -> Completion {
        let Self {
            stamp,
            binding,
            parent,
            pending,
            lease,
        } = self;
        let result = tokio::select! {
            _ = parent.cancelled() => Err(Error::Cancelled),
            value = pending.wait() => value,
        };
        Completion {
            stamp,
            binding,
            result,
            _lease: lease,
        }
    }
}
impl Reads {
    pub(super) fn byte_wait(&self, pump: &Pump) -> Option<usize> {
        self.active
            .as_ref()
            .filter(|a| a.byte_blocked && a.flights[0].is_none() && pump.data_ready())
            .map(|_| RECEIVE_ADMISSION_BYTES)
    }
    pub(super) fn credit_ready(&mut self, now: Instant) {
        if let Some(a) = &mut self.active {
            if a.byte_blocked {
                a.byte_blocked = false;
                a.data_at = now;
            }
        }
    }
    /// Called only by the explicit active-work owner. Replacing work cancels
    /// old requests, even if a new operation uses identical origin/session IDs.
    pub(super) fn begin(&mut self, now: Instant, parent: Cancellation) -> Result<()> {
        if parent.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let generation = self.generation.checked_add(1).ok_or(Error::Runtime)?;
        self.generation = generation;
        self.active = Some(Active {
            generation,
            parent,
            cancel: Cancellation::default(),
            flights: [None, None],
            data_at: now,
            status_at: now + PROBE_INTERVAL,
            empty: 0,
            byte_blocked: false,
        });
        Ok(())
    }
    pub(super) fn stop(&mut self) {
        self.active = None;
    }
    fn current(&mut self, now: Instant) -> Option<&mut Active> {
        if self
            .active
            .as_ref()
            .is_some_and(|a| a.parent.is_cancelled())
        {
            self.stop();
        }
        let a = self.active.as_mut()?;
        for (lane, flight) in a.flights.iter_mut().enumerate() {
            if flight
                .as_ref()
                .is_some_and(|f| f.abandoned.load(Ordering::Acquire))
            {
                *flight = None;
                if lane == 0 {
                    a.data_at = a.data_at.max(now + RETRY_ADMISSION);
                } else {
                    a.status_at = a.status_at.max(now + RETRY_ADMISSION);
                }
            }
        }
        Some(a)
    }
    /// At most one data read and one small status read per activity. Actual
    /// uploads/ACKs still use the same two fixed HTTP workers and bounded queues.
    /// A small read is never queued behind a second large receive in this owner.
    pub(super) fn dispatch(
        &mut self,
        pump: &Pump,
        http: &HttpClient,
        demand: Demand,
        authorized: MembershipCheck,
        now: Instant,
        unix_ms: u64,
    ) -> Result<Option<ReadJob>> {
        if self.current(now).is_none() || pump.is_empty() {
            return Ok(None);
        }
        let consumer_blocked = !demand.receive || !pump.data_ready();
        let a = self.active.as_ref().expect("current activity");
        if !consumer_blocked && a.flights[0].is_none() && now >= a.data_at {
            // Busy BEFORE queue admission means the full response budget is
            // unavailable. Only that (or consumer backpressure) permits receipts.
            match self.enqueue(
                pump,
                http,
                ReceiveMode::Data,
                authorized.clone(),
                now,
                unix_ms,
            ) {
                Ok(Some(job)) => {
                    self.active.as_mut().unwrap().byte_blocked = false;
                    return Ok(Some(job));
                }
                Ok(None) => self.active.as_mut().unwrap().byte_blocked = true,
                Err(Error::Busy) => {
                    // Large-queue pressure must not starve the independent
                    // small lane. It is NOT byte/consumer backpressure.
                    self.active.as_mut().unwrap().byte_blocked = false;
                }
                Err(error) => return Err(error),
            }
            self.active.as_mut().unwrap().data_at = now + RETRY_ADMISSION;
        }
        let a = self.active.as_ref().expect("current activity");
        if a.flights[1].is_some() || now < a.status_at {
            return Ok(None);
        }
        let unacknowledged = pump.unacknowledged()?;
        if !unacknowledged && !demand.file_wait {
            return Ok(None);
        }
        let mode = if (consumer_blocked || a.byte_blocked) && unacknowledged {
            ReceiveMode::Receipts
        } else {
            ReceiveMode::Probe
        };
        match self.enqueue(pump, http, mode, authorized, now, unix_ms) {
            Ok(Some(job)) => Ok(Some(job)),
            Ok(None) | Err(Error::Busy) => {
                self.active.as_mut().unwrap().status_at = now + RETRY_ADMISSION;
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
    /// None is byte-admission Busy; Err(Busy) is bounded HTTP-queue Busy. No
    /// accepted request or status slot is recorded until submit has succeeded.
    fn enqueue(
        &mut self,
        pump: &Pump,
        http: &HttpClient,
        mode: ReceiveMode,
        authorized: MembershipCheck,
        now: Instant,
        unix_ms: u64,
    ) -> Result<Option<ReadJob>> {
        let request = pump.read(mode, unix_ms)?;
        let a = self.active.as_mut().ok_or(Error::Cancelled)?;
        let admission = match http.prepare_receive(
            &request,
            a.cancel.clone(),
            authorized,
            Instant::now() + PROGRESS_TIMEOUT,
        ) {
            Err(Error::Busy) => return Ok(None),
            other => other?,
        };
        let id = self.next_id.checked_add(1).ok_or(Error::Runtime)?;
        let pending = admission.submit(request, None)?;
        self.next_id = id;
        let stamp = Stamp {
            generation: a.generation,
            id,
            mode,
        };
        let abandoned = Arc::new(AtomicBool::new(false));
        a.flights[stamp.lane()] = Some(Flight {
            stamp,
            abandoned: abandoned.clone(),
        });
        a.status_at = now + PROBE_INTERVAL;
        Ok(Some(ReadJob {
            stamp,
            binding: http.clone(),
            parent: a.parent.clone(),
            pending,
            lease: Lease(abandoned),
        }))
    }
    /// One result can update only its own current activity and lane. Non-200
    /// responses remain bounded and are handed to the future shared decoder.
    pub(super) fn finish(
        &mut self,
        completion: Completion,
        pump: &mut Pump,
        now: Instant,
        unix_ms: u64,
    ) -> Result<Outcome> {
        // A successful HTTP result can sit in a completion queue during rebind.
        // Recheck its capability again at the last mutation boundary.
        completion.binding.check_binding()?;
        let a = self.current(now).ok_or(Error::Cancelled)?;
        let slot = &mut a.flights[completion.stamp.lane()];
        if a.generation != completion.stamp.generation
            || !slot.as_ref().is_some_and(|f| f.stamp == completion.stamp)
        {
            return Err(Error::Cancelled);
        }
        *slot = None;
        if completion.stamp.mode == ReceiveMode::Data {
            a.data_at = now + RETRY_ADMISSION;
        }
        let reply = completion.result?;
        if reply.status != 200 {
            return Ok(Outcome::Rejected(reply));
        }
        let packet = reply.into_receive()?;
        let had_data = packet.items().iter().any(|i| !i.batches().is_empty());
        let queued_data = packet.items().iter().any(|i| {
            i.batches()
                .last()
                .is_some_and(|b| b.sequence + 1 < i.window().0)
        });
        let activity = pump.accept(packet, unix_ms)?;
        if completion.stamp.mode == ReceiveMode::Data {
            if had_data {
                a.empty = 0;
                // Drain an already queued prefix immediately. At the observed
                // tail, give the peer's bounded upload window one opportunity
                // to flush instead of issuing a predictably empty extra read.
                // This deadline never slides on other local work.
                a.data_at = if queued_data {
                    now
                } else {
                    now + super::coalesce::MAX_COALESCE
                };
            } else {
                a.empty = (a.empty + 1).min(3);
                a.data_at = now + Duration::from_millis(250 << (a.empty - 1));
            }
        } else if activity.consumed_batches > 0 {
            // A verified peer receipt may have freed the missing byte credit.
            // This schedules an opportunity, not file progress or sync success.
            a.data_at = now;
            a.byte_blocked = false;
        }
        Ok(Outcome::Applied(activity))
    }
    /// The outer activity task may sleep/select until this deadline or a real
    /// channel/credit/work event. No busy loop or timer exists in this value.
    pub(super) fn next_wake(
        &mut self,
        pump: &Pump,
        demand: Demand,
        now: Instant,
    ) -> Result<Option<Instant>> {
        let Some(a) = self.current(now) else {
            return Ok(None);
        };
        if pump.is_empty() {
            return Ok(None);
        }
        let data =
            (demand.receive && pump.data_ready() && a.flights[0].is_none() && !a.byte_blocked)
                .then_some(a.data_at);
        let small = ((pump.unacknowledged()? || demand.file_wait) && a.flights[1].is_none())
            .then_some(a.status_at);
        Ok(data.into_iter().chain(small).min())
    }
}
