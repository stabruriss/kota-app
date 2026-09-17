//! Finite ciphertext/receipt coalescing. The activity owner supplies real wake
//! events and clocks; this creates no task, timer, HTTP retry or public progress.
use super::{
    http::{HttpClient, PendingHttp, SendAdmission, SEND_ADMISSION_BYTES},
    pump::{Acknowledgement, Pump},
    upload::Builder,
    window::OutgoingBatch,
    Error, Result,
};
use crate::bbs_sync::transport::{MAX_FRAME, PROGRESS_TIMEOUT};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

/// Fixed from the first available byte/consumption. Later work never extends
/// it. Full batches and unrelieved output-byte backpressure flush immediately.
pub(super) const MAX_COALESCE: Duration = Duration::from_millis(50);

struct Building {
    session: String,
    body: Builder,
    admission: SendAdmission,
    flush_at: Instant,
}

#[derive(Default)]
pub(super) struct Coalescer {
    building: Option<Building>,
    after: Option<String>,
    receipts: BTreeMap<String, Instant>,
    byte_wait: Option<usize>,
}

pub(super) struct Ready {
    pub(super) session: String,
    pub(super) batch: OutgoingBatch,
    admission: SendAdmission,
}
impl Ready {
    pub(super) fn deadline(&self) -> Instant {
        self.admission.deadline()
    }
    /// Queue admission, then submitted-prefix publication, before awaiting any
    /// HTTP result. Failure keeps the SAME frozen batch/proof in SendWindow;
    /// the owner retries via Pump::retry, never through another TLS encryption.
    pub(super) fn submit(self, pump: &mut Pump) -> Result<PendingHttp> {
        let pending = self
            .admission
            .submit(self.batch.request, Some(self.batch.payload.into()))?;
        pump.submitted(&self.session, self.batch.sequence)?;
        Ok(pending)
    }
}

impl Coalescer {
    pub(super) fn clear(&mut self) {
        self.building = None;
        self.receipts.clear();
        self.after = None;
        self.byte_wait = None;
    }
    pub(super) fn byte_wait(&self) -> Option<usize> {
        self.byte_wait
    }

    /// One unfinished builder per account, not one response reservation per
    /// peer. Service rotates after each emitted batch. It never drains TLS until
    /// both response/scratch headroom and its charged output blocks are owned.
    pub(super) fn prepare(
        &mut self,
        pump: &mut Pump,
        http: &HttpClient,
        now: Instant,
        unix_ms: u64,
    ) -> Result<Option<Ready>> {
        self.byte_wait = None;
        if let Err(error) = http.check_binding() {
            self.clear();
            return Err(error);
        }
        let ids = pump.sessions();
        if self.building.as_ref().is_some_and(|b| {
            !ids.contains(&b.session) || pump.send_stopped(&b.session).unwrap_or(true)
        }) {
            self.building = None;
        }
        if self.building.is_none() && !ids.is_empty() {
            let first = self
                .after
                .as_ref()
                .and_then(|last| ids.iter().position(|id| id > last))
                .unwrap_or(0);
            for n in 0..ids.len() {
                let session = &ids[(first + n) % ids.len()];
                if !pump.has_output(session)? {
                    continue;
                }
                let admission =
                    match pump.prepare_upload(session, http, Instant::now() + PROGRESS_TIMEOUT) {
                        Err(Error::Busy) => {
                            self.byte_wait = Some(SEND_ADMISSION_BYTES + MAX_FRAME);
                            continue;
                        }
                        other => other?,
                    };
                self.building = Some(Building {
                    session: session.clone(),
                    body: Builder::new(pump.limits()),
                    admission,
                    flush_at: now + MAX_COALESCE,
                });
                break;
            }
        }
        let Some(building) = &mut self.building else {
            return Ok(None);
        };
        let blocked = match pump.fill(&building.session, &mut building.body, &building.admission) {
            Ok(_) => false,
            Err(Error::Busy) => true,
            Err(error) => {
                self.building = None;
                return Err(error);
            }
        };
        if building.body.len() == 0 {
            // No byte means no coalescing window or headroom reservation. A
            // caller waits for a real credit/channel event, not an empty timer.
            self.building = None;
            if blocked {
                self.byte_wait = Some(SEND_ADMISSION_BYTES + MAX_FRAME);
            }
            return Ok(None);
        }
        // fill may make partial progress and internally stop on byte Busy. If
        // TLS still has output, leaving it blocked cannot improve this batch.
        let pressured = blocked || pump.has_output(&building.session)?;
        if !building.body.full() && !pressured && now < building.flush_at {
            return Ok(None);
        }
        let building = self.building.take().ok_or(Error::Runtime)?;
        let batch = pump.enqueue(&building.session, building.body.finish()?, unix_ms)?;
        self.after = Some(building.session.clone());
        Ok(Some(Ready {
            session: building.session,
            batch,
            admission: building.admission,
        }))
    }

    /// Observe consumption without signing yet, so the fixed window captures
    /// the latest cumulative cursor. The owner allows only one ACK flight per
    /// direction; until confirmed Pump returns the original immutable receipt.
    pub(super) fn acknowledgement(
        &mut self,
        pump: &mut Pump,
        now: Instant,
        unix_ms: u64,
    ) -> Result<Option<(String, Acknowledgement)>> {
        let ids = pump.sessions();
        self.receipts.retain(|id, _| ids.contains(id));
        let mut ready = None;
        for id in ids {
            if !pump.acknowledgement_pending(&id)? {
                self.receipts.remove(&id);
                continue;
            }
            let at = *self
                .receipts
                .entry(id.clone())
                .or_insert(now + MAX_COALESCE);
            if at <= now && ready.as_ref().is_none_or(|(_, old)| at < *old) {
                ready = Some((id, at));
            }
        }
        let Some((id, _)) = ready else {
            return Ok(None);
        };
        Ok(pump.acknowledgement(&id, unix_ms)?.map(|ack| (id, ack)))
    }

    /// Only active coalescing windows. No work means no deadline. This is not a
    /// periodic poll; the owner selects it with channel, HTTP and credit events.
    pub(super) fn next_wake(
        &self,
        upload_slot: bool,
        acknowledgement_slot: bool,
    ) -> Option<Instant> {
        self.building
            .as_ref()
            .filter(|_| upload_slot)
            .map(|b| b.flush_at)
            .into_iter()
            .chain(
                self.receipts
                    .values()
                    .filter(|_| acknowledgement_slot)
                    .copied(),
            )
            .min()
    }
}
