//! Explicit, finite activity owner. Polling `next` drives the existing two HTTP
//! workers; no per-request tasks or additional blocking workers are created.
//! The account coordinator owns this activity. The common response decoder owns
//! platform/boot classification; the exchange alone owns content progress.
use super::{
    coalesce::Coalescer,
    framing::Channels,
    http::{HttpClient, Reply, SEND_ADMISSION_BYTES},
    pump::Pump,
    reads::{Completion, Demand, Outcome, Reads},
    Error, Result, TlsStream, MAX_ENVELOPE,
};
use crate::bbs_sync::transport::{Cancellation, MembershipCheck, PROGRESS_TIMEOUT};
use std::{
    collections::{BTreeMap, VecDeque},
    future::{pending, poll_fn, Future},
    pin::Pin,
    task::Poll,
    time::{Duration, Instant},
};

const RETRY_MIN: Duration = Duration::from_millis(250);
const UPLOAD: usize = 2;
const ACK: usize = 3;
type Flight = Pin<Box<dyn Future<Output = Finished> + Send>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Operation {
    Receive,
    Upload,
    Ack,
}
pub(super) enum Failure {
    Transport(Error),
    /// Only a fixed classified error leaves the relay response boundary.
    Response {
        operation: Operation,
        error: Error,
    },
}
impl From<Error> for Failure {
    fn from(value: Error) -> Self {
        Self::Transport(value)
    }
}
pub(super) enum Event {
    Ready { session: String, channels: Channels },
    Retired { session: String, error: Error },
}
enum Finished {
    Read(Completion),
    Mutation {
        slot: usize,
        session: String,
        sequence: u64,
        result: Result<Reply>,
    },
}
struct Mutation {
    session: String,
    sequence: u64,
    deadline: Instant,
    at: Instant,
    failures: u8,
}
impl Mutation {
    fn retry_at(&mut self, now: Instant) {
        self.failures = (self.failures + 1).min(3);
        self.at = now + RETRY_MIN * (1 << (self.failures - 1));
    }
}

pub(super) struct Activity {
    pump: Pump,
    http: HttpClient,
    cancel: Cancellation,
    authorized: MembershipCheck,
    reads: Reads,
    coalesce: Coalescer,
    flights: [Option<Flight>; 4],
    mutations: [Option<Mutation>; 2],
    retired: VecDeque<(String, Error)>,
    closing: BTreeMap<String, Instant>,
    started: Instant,
    unix_ms: u64,
    stopped: bool,
    #[cfg(test)]
    pub(super) turns: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
impl Drop for Activity {
    fn drop(&mut self) {
        self.stop();
    }
}
impl Activity {
    /// Construction starts no request or task. The caller explicitly polls
    /// next only for a real active exchange, and drops/stops it when work ends.
    pub(super) fn new(
        pump: Pump,
        http: HttpClient,
        cancel: Cancellation,
        authorized: MembershipCheck,
        unix_ms: u64,
    ) -> Result<Self> {
        http.check_binding()?;
        if !authorized() {
            return Err(Error::Unauthorized);
        }
        let started = Instant::now();
        let mut reads = Reads::default();
        reads.begin(started, cancel.clone())?;
        Ok(Self {
            pump,
            http,
            cancel,
            authorized,
            reads,
            coalesce: Coalescer::default(),
            flights: std::array::from_fn(|_| None),
            mutations: [None, None],
            retired: VecDeque::new(),
            closing: BTreeMap::new(),
            started,
            unix_ms,
            stopped: false,
            #[cfg(test)]
            turns: Default::default(),
        })
    }
    pub(super) fn add(&mut self, tls: TlsStream) -> Result<String> {
        self.check()?;
        self.pump.add(tls)
    }
    pub(super) fn can_add(&self) -> bool {
        self.pump.can_add()
    }
    pub(super) fn remove(&mut self, session: &str) {
        self.pump.remove(session);
        self.closing.remove(session);
        for slot in [UPLOAD, ACK] {
            if self.mutations[slot - 2]
                .as_ref()
                .is_some_and(|m| m.session == session)
            {
                self.mutations[slot - 2] = None;
                self.flights[slot] = None;
            }
        }
    }
    /// Create a final receiver receipt after the content owner has decided
    /// this direction is complete.  This is deliberately an explicit call:
    /// an empty read or a peer disconnect never proves application completion.
    pub(super) fn final_ack(
        &mut self,
        session: &str,
    ) -> Result<Option<super::pump::Acknowledgement>> {
        self.check()?;
        self.pump.final_acknowledgement(session, self.time()?)
    }
    /// Remember one application close intent while an ordinary cumulative ACK
    /// is in flight. Completion wakes this same owner; no final is lost because
    /// the caller happened to request it between two HTTP receipts.
    pub(super) fn close(&mut self, session: &str) -> Result<()> {
        self.check()?;
        self.pump.authority(session)?;
        self.closing
            .entry(session.into())
            .or_insert_with(|| Instant::now() + PROGRESS_TIMEOUT);
        self.pump.wake().notify_one();
        Ok(())
    }
    pub(super) fn stop(&mut self) {
        self.stopped = true;
        self.reads.stop();
        self.flights = std::array::from_fn(|_| None);
        self.mutations = [None, None];
        self.coalesce.clear();
        for id in self.pump.sessions() {
            self.pump.remove(&id);
        }
        self.retired.clear();
        self.closing.clear();
    }
    fn check(&self) -> Result<()> {
        if self.stopped || self.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if self.pump.context().shutdown.is_cancelled() {
            return Err(Error::Closed);
        }
        self.http.check_binding()?;
        if !(self.authorized)() {
            return Err(Error::Unauthorized);
        }
        Ok(())
    }
    fn time(&self) -> Result<u64> {
        self.unix_ms
            .checked_add(
                self.started
                    .elapsed()
                    .as_millis()
                    .try_into()
                    .map_err(|_| Error::Runtime)?,
            )
            .ok_or(Error::Runtime)
    }
    fn retire(&mut self, session: String, error: Error) {
        self.remove(&session);
        // Drained before the next pump step; at most four peers can retire.
        self.retired.push_back((session, error));
    }
    fn mutation_flight(&mut self, slot: usize, pending: super::http::PendingHttp) {
        let m = self.mutations[slot - 2]
            .as_ref()
            .expect("intent before admission");
        let session = m.session.clone();
        let sequence = m.sequence;
        self.flights[slot] = Some(Box::pin(async move {
            Finished::Mutation {
                slot,
                session,
                sequence,
                result: pending.wait().await,
            }
        }));
    }
    fn finish(
        &mut self,
        value: Finished,
        now: Instant,
        ms: u64,
    ) -> std::result::Result<(), Failure> {
        self.check()?; // Includes A -> B -> A, even after HTTP already returned.
        match value {
            Finished::Read(value) => match self.reads.finish(value, &mut self.pump, now, ms)? {
                Outcome::Applied(a) => {
                    for (id, error) in a.retired {
                        self.retire(id, error);
                    }
                }
                Outcome::Rejected(reply) => {
                    let error = reply.error();
                    if error != Error::Busy {
                        return Err(Failure::Response {
                            operation: Operation::Receive,
                            error,
                        });
                    }
                    // Reads already scheduled its next bounded opportunity.
                    // Backpressure does not count as content progress.
                }
            },
            Finished::Mutation {
                slot,
                session,
                sequence,
                result,
            } => {
                let Some(m) = self.mutations[slot - 2]
                    .as_mut()
                    .filter(|m| m.session == session && m.sequence == sequence)
                else {
                    return Ok(()); // Retired completion never applies to a new peer.
                };
                if slot == UPLOAD && self.pump.send_stopped(&session)? {
                    // The authenticated peer already stopped this direction.
                    // A late upload reply cannot retire the reverse direction.
                    self.mutations[0] = None;
                    return Ok(());
                }
                match result {
                    Ok(reply) if reply.status != 200 => {
                        let error = reply.error();
                        if error == Error::Busy {
                            if now < m.deadline {
                                m.retry_at(now);
                            } else {
                                self.retire(session, Error::Timeout);
                            }
                        } else {
                            return Err(Failure::Response {
                                operation: if slot == UPLOAD {
                                    Operation::Upload
                                } else {
                                    Operation::Ack
                                },
                                error,
                            });
                        }
                    }
                    Ok(reply) => {
                        self.http.checked(
                            reply
                                .json()
                                .and_then(|body| super::response::mutation(body, slot == UPLOAD)),
                        )?;
                        drop(reply);
                        if slot == ACK {
                            self.pump.ack_submitted(&session, sequence)?;
                        }
                        // This retires an HTTP intent, never ciphertext or a file.
                        self.mutations[slot - 2] = None;
                    }
                    Err(Error::Closed) if now < m.deadline => m.retry_at(now),
                    Err(error) => self.retire(session, error),
                }
            }
        }
        Ok(())
    }
    /// Cancel-safe: dropping this future leaves bounded work in the owner;
    /// dropping/stopping the owner retires queued/in-flight jobs immediately.
    pub(super) async fn next(&mut self) -> std::result::Result<Event, Failure> {
        let result = self.drive().await;
        if result.is_err() {
            self.stop();
        }
        result
    }
    async fn drive(&mut self) -> std::result::Result<Event, Failure> {
        let context = self.pump.context();
        let wake = self.pump.wake();
        let mut file_work = context.io.work_status();
        loop {
            self.check()?;
            if let Some((session, error)) = self.retired.pop_front() {
                return Ok(Event::Retired { session, error });
            }
            if self.pump.is_empty() {
                return Err(Error::Closed.into());
            }
            #[cfg(test)]
            self.turns
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let notified = wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let now = Instant::now();
            let ms = self.time()?;
            for (id, error) in self.pump.step()?.retired {
                self.retire(id, error);
            }
            for (id, deadline) in self.closing.clone() {
                if now >= deadline {
                    self.retire(id, Error::Timeout);
                } else {
                    match self.pump.final_acknowledgement(&id, ms) {
                        Ok(_) | Err(Error::Busy) => {}
                        Err(error) => self.retire(id, error),
                    }
                }
            }
            if !self.retired.is_empty() {
                continue;
            }
            for session in self.pump.sessions() {
                if let Some(channels) = self.pump.channels(&session)? {
                    return Ok(Event::Ready { session, channels });
                }
            }
            if let Some(m) = &self.mutations[0] {
                if self.pump.send_stopped(&m.session)? {
                    self.flights[UPLOAD] = None;
                    self.mutations[0] = None;
                }
            }
            // Reserve response headroom BEFORE draining TLS. An accepted batch
            // remains immutable in Pump if bounded queue admission is Busy.
            if self.mutations[0].is_none() {
                if let Some(ready) = self.coalesce.prepare(&mut self.pump, &self.http, now, ms)? {
                    let m = Mutation {
                        session: ready.session.clone(),
                        sequence: ready.batch.sequence,
                        deadline: ready.deadline(),
                        at: now,
                        failures: 0,
                    };
                    self.mutations[0] = Some(m);
                    match ready.submit(&mut self.pump) {
                        Ok(pending) => self.mutation_flight(UPLOAD, pending),
                        Err(Error::Busy) => {
                            self.mutations[0].as_mut().unwrap().at = now + RETRY_MIN
                        }
                        Err(error) => {
                            let id = self.mutations[0].as_ref().unwrap().session.clone();
                            self.retire(id, error);
                        }
                    }
                }
            }
            if self.mutations[1].is_none() {
                if let Some((session, ack)) =
                    self.coalesce.acknowledgement(&mut self.pump, now, ms)?
                {
                    self.mutations[1] = Some(Mutation {
                        session,
                        sequence: ack.sequence,
                        deadline: now + PROGRESS_TIMEOUT,
                        at: now,
                        failures: 0,
                    });
                }
            }
            let mut bytes = self
                .pump
                .byte_wait()
                .into_iter()
                .chain(self.coalesce.byte_wait())
                .min();
            let mut small_bytes = None;
            for slot in [UPLOAD, ACK] {
                if self.flights[slot].is_some() {
                    continue;
                }
                let Some(m) = &self.mutations[slot - 2] else {
                    continue;
                };
                let (id, sequence, deadline, at) =
                    (m.session.clone(), m.sequence, m.deadline, m.at);
                if now >= deadline {
                    self.retire(id, Error::Timeout);
                    continue;
                }
                if now < at {
                    continue;
                }
                let (cancel, authorized) = match self.pump.authority(&id) {
                    Ok(pair) => pair,
                    Err(error) => {
                        self.retire(id, error);
                        continue;
                    }
                };
                let admission = if slot == UPLOAD {
                    if !self.pump.upload_pending(&id, sequence)? {
                        self.mutations[0] = None; // Authenticated ACK already covered it.
                        continue;
                    }
                    let batch = self.pump.retry(&id, sequence)?;
                    match self.http.prepare_send(cancel, authorized, deadline) {
                        Ok(a) => a.submit(batch.request, Some(batch.payload.into())),
                        Err(Error::Busy) => {
                            bytes = minimum(bytes, SEND_ADMISSION_BYTES);
                            continue;
                        }
                        Err(error) => Err(error),
                    }
                } else {
                    let ack = self.pump.acknowledgement(&id, ms)?.ok_or(Error::Protocol)?;
                    let needed = ack.body.len() + ack.request.response_limit() + 2 * MAX_ENVELOPE;
                    let body = match self.http.small_body(ack.body.as_bytes()) {
                        Ok(body) => body,
                        Err(Error::Busy) => {
                            small_bytes = Some(needed);
                            continue;
                        }
                        Err(error) => return Err(error.into()),
                    };
                    match self
                        .http
                        .prepare_ack(&ack.request, cancel, authorized, deadline)
                    {
                        Ok(a) => a.submit(ack.request, Some(body.into())),
                        Err(Error::Busy) => {
                            small_bytes = Some(needed);
                            continue;
                        }
                        Err(error) => Err(error),
                    }
                };
                match admission {
                    Ok(pending) => {
                        if slot == UPLOAD {
                            self.pump.submitted(&id, sequence)?;
                        }
                        self.mutation_flight(slot, pending);
                    }
                    Err(Error::Busy) => {
                        self.mutations[slot - 2].as_mut().unwrap().at = now + RETRY_MIN
                    }
                    Err(error) => self.retire(id, error),
                }
            }
            if !self.retired.is_empty() {
                continue;
            }
            let demand = Demand {
                receive: true,
                file_wait: *file_work.borrow_and_update() != 0,
            };
            for _ in 0..2 {
                if let Some(read) = self.reads.dispatch(
                    &self.pump,
                    &self.http,
                    demand,
                    self.authorized.clone(),
                    now,
                    ms,
                )? {
                    let lane = usize::from(read.mode() != super::proof::ReceiveMode::Data);
                    if self.flights[lane].is_some() {
                        return Err(Error::Runtime.into());
                    }
                    self.flights[lane] =
                        Some(Box::pin(async move { Finished::Read(read.wait().await) }));
                }
            }
            bytes = bytes
                .into_iter()
                .chain(self.reads.byte_wait(&self.pump))
                .min();
            let at = self
                .coalesce
                .next_wake(self.mutations[0].is_none(), self.mutations[1].is_none())
                .into_iter()
                .chain(self.reads.next_wake(&self.pump, demand, now)?)
                .chain(self.closing.values().copied())
                .chain(self.mutations.iter().enumerate().filter_map(|(i, m)| {
                    m.as_ref()
                        .filter(|_| self.flights[i + 2].is_none())
                        .map(|m| {
                            if m.at > now {
                                m.at.min(m.deadline)
                            } else {
                                m.deadline
                            }
                        })
                }))
                .min();
            let mut peer_cancels: Vec<Pin<Box<dyn Future<Output = ()> + Send>>> = self
                .pump
                .sessions()
                .iter()
                .map(|id| {
                    self.pump
                        .authority(id)
                        .map(|(c, _)| Box::pin(async move { c.cancelled().await }) as _)
                })
                .collect::<Result<_>>()?;
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => return Err(Error::Cancelled.into()),
                _ = context.shutdown.cancelled() => return Err(Error::Closed.into()),
                _ = self.http.retired() => return Err(self.http.check_binding().err().unwrap_or(Error::Cancelled).into()),
                value = completion(&mut self.flights) => self.finish(value, Instant::now(), self.time()?)?,
                _ = notified => {},
                _ = poll_fn(|cx| { for c in &mut peer_cancels { if c.as_mut().poll(cx).is_ready() { return Poll::Ready(()); } } Poll::Pending }) => {},
                _ = file_work.changed() => {},
                result = async { match bytes { Some(n) => context.limits.wait_for_bytes(n).await, None => pending().await } } => {
                    result?; self.reads.credit_ready(Instant::now());
                },
                result = async { match small_bytes { Some(n) => self.http.wait_for_small_bytes(n).await, None => pending().await } } => { result?; },
                _ = async { match at { Some(at) => tokio::time::sleep_until(at.into()).await, None => pending().await } } => {},
            }
        }
    }
}
fn minimum(old: Option<usize>, value: usize) -> Option<usize> {
    Some(old.map_or(value, |old| old.min(value)))
}
async fn completion(flights: &mut [Option<Flight>; 4]) -> Finished {
    poll_fn(|cx| {
        for flight in flights.iter_mut() {
            if let Some(f) = flight {
                if let Poll::Ready(value) = f.as_mut().poll(cx) {
                    *flight = None;
                    return Poll::Ready(value);
                }
            }
        }
        Poll::Pending
    })
    .await
}
