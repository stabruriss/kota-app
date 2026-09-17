//! Account-charged immutable ciphertext and the two-batch relay window.
//! Upload replies never release credit; only the peer's authenticated receipt
//! can advance consumption. No timer, retry loop or content-completion signal.
use super::{
    proof::{Direction, Fields, Route, SignedAck, SignedRequest, Target, VerifiedAck},
    upload::Upload,
    wire::{hash, Role, MAX_SAFE_INTEGER},
    Error, Result, SessionContext, TlsStream, MAX_BATCH,
};
use crate::bbs_sync::transport::{BytePermit, Cancellation, Limits, MembershipCheck};
use crate::bbs_sync::DeviceIdentity;
use std::{collections::VecDeque, ops::Range, sync::Arc};

const WINDOW_BATCHES: usize = 2;

struct Allocation {
    bytes: Box<[u8]>,
    len: usize,
    // Shared ownership keeps the charge even if Cancel drops the window while
    // an already-blocking HTTP call still owns the same ciphertext.
    _permit: BytePermit,
}

#[derive(Clone)]
pub(super) struct Payload(Arc<Allocation>, Range<usize>);
impl Payload {
    pub(super) fn bytes(&self) -> &[u8] {
        &self.0.bytes[self.1.clone()]
    }
    /// A subview retains the entire original response charge. Never copy a
    /// multi-session reply into separately uncharged per-session buffers.
    pub(super) fn slice(&self, range: Range<usize>) -> Result<Self> {
        if range.start >= range.end || range.end > self.1.len() {
            return Err(Error::Protocol);
        }
        Ok(Self(
            self.0.clone(),
            self.1.start + range.start..self.1.start + range.end,
        ))
    }
}

/// Reserve before allocating, and never grow/reallocate behind the byte budget.
pub(super) struct Buffer(Allocation);
impl Buffer {
    pub(super) fn new(limits: &Limits, capacity: usize) -> Result<Self> {
        if capacity == 0 || capacity > MAX_BATCH + super::MAX_ENVELOPE {
            return Err(Error::Protocol);
        }
        let permit = limits.reserve(capacity)?;
        Ok(Self(Allocation {
            bytes: vec![0; capacity].into_boxed_slice(),
            len: 0,
            _permit: permit,
        }))
    }
    pub(super) fn spare_mut(&mut self) -> &mut [u8] {
        &mut self.0.bytes[self.0.len..]
    }
    pub(super) fn len(&self) -> usize {
        self.0.len
    }
    pub(super) fn advance(&mut self, count: usize) -> Result<()> {
        if count > self.0.bytes.len() - self.0.len {
            return Err(Error::Protocol);
        }
        self.0.len += count;
        Ok(())
    }
    pub(super) fn freeze(self) -> Result<Payload> {
        if self.0.len == 0 {
            return Err(Error::Protocol);
        }
        let len = self.0.len;
        Ok(Payload(Arc::new(self.0), 0..len))
    }
}

#[derive(Clone)]
pub(super) struct Batch {
    pub(super) sequence: u64,
    pub(super) payload: Payload,
}

#[derive(Clone)]
pub(super) struct OutgoingBatch {
    pub(super) sequence: u64,
    pub(super) payload: Upload,
    pub(super) request: SignedRequest,
}

pub(super) struct SendWindow {
    context: SessionContext,
    session: String,
    authorized: MembershipCheck,
    cancel: Cancellation,
    batches: VecDeque<OutgoingBatch>,
    next: u64,
    submitted: u64,
    consumed: u64,
    receipt: Option<(VerifiedAck, String)>,
    closed: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ReceiptEffect {
    /// Newly confirmed batches, never bytes uploaded or a file-completion count.
    pub(super) consumed_batches: u64,
    pub(super) closed: bool,
}
impl SendWindow {
    pub(super) fn new(
        context: SessionContext,
        session: String,
        authorized: MembershipCheck,
        cancel: Cancellation,
    ) -> Result<Self> {
        context.validate()?;
        hash(&session)?;
        check(&authorized, &cancel)?;
        Ok(Self {
            context,
            session,
            authorized,
            cancel,
            batches: VecDeque::with_capacity(WINDOW_BATCHES),
            next: 0,
            submitted: 0,
            consumed: 0,
            receipt: None,
            closed: false,
        })
    }
    pub(super) fn writable(&self) -> Result<bool> {
        check(&self.authorized, &self.cancel)?;
        Ok(!self.stopped() && self.batches.len() < WINDOW_BATCHES)
    }
    /// A verified final stops new sends immediately. Its terminal cleanup is
    /// separate so reverse ciphertext in the same envelope can enter TLS first.
    pub(super) fn stopped(&self) -> bool {
        self.receipt.as_ref().is_some_and(|(ack, _)| ack.final_flag)
    }
    pub(super) fn closed(&self) -> bool {
        self.closed
    }
    pub(super) fn finish_close(&mut self) -> Result<()> {
        check(&self.authorized, &self.cancel)?;
        if self.stopped() {
            self.closed = true;
            self.batches.clear();
        }
        Ok(())
    }
    pub(super) fn pending(&self) -> Result<bool> {
        check(&self.authorized, &self.cancel)?;
        Ok(!self.batches.is_empty())
    }
    pub(super) fn enqueue(
        &mut self,
        identity: &DeviceIdentity,
        payload: Upload,
        now: u64,
    ) -> Result<u64> {
        check(&self.authorized, &self.cancel)?;
        if self.stopped() {
            return Err(Error::Closed);
        }
        if self.batches.len() == WINDOW_BATCHES {
            return Err(Error::Busy);
        }
        if payload.len() == 0 || payload.len() > MAX_BATCH || self.next >= MAX_SAFE_INTEGER {
            return Err(Error::Protocol);
        }
        if identity.device_id().map_err(|_| Error::Unauthorized)?
            != self.context.peer.local_device_id
        {
            return Err(Error::Unauthorized);
        }
        let sequence = self.next;
        let request = SignedRequest::sign_upload(
            identity,
            &self.context.peer.local_membership_id,
            Target::post(
                &self.context.origin,
                &self.context.peer.group_id,
                Route::Send,
            )?,
            Fields::Frame {
                boot: &self.context.boot,
                session: &self.session,
                direction: match self.context.role() {
                    Role::Client => Direction::ClientToServer,
                    Role::Server => Direction::ServerToClient,
                },
                sequence,
                final_flag: false,
            },
            &payload,
            now,
        )?;
        check(&self.authorized, &self.cancel)?;
        self.next += 1;
        self.batches.push_back(OutgoingBatch {
            sequence,
            payload,
            request,
        });
        Ok(sequence)
    }
    /// Retries clone these exact bytes AND the original proof; no re-signing or
    /// re-encryption. The HTTP owner calls submitted()
    /// only once its bounded queue has accepted this exact batch.
    pub(super) fn batch(&self, sequence: u64) -> Result<OutgoingBatch> {
        check(&self.authorized, &self.cancel)?;
        self.batches
            .iter()
            .find(|b| b.sequence == sequence)
            .cloned()
            .ok_or(Error::Protocol)
    }
    pub(super) fn contains(&self, sequence: u64) -> Result<bool> {
        check(&self.authorized, &self.cancel)?;
        Ok(!self.stopped() && self.batches.iter().any(|b| b.sequence == sequence))
    }
    pub(super) fn submitted(&mut self, sequence: u64) -> Result<()> {
        check(&self.authorized, &self.cancel)?;
        if self.stopped()
            || !self.batches.iter().any(|b| b.sequence == sequence)
            || sequence > self.submitted
        {
            return Err(Error::Protocol);
        }
        if sequence == self.submitted {
            self.submitted += 1;
        }
        Ok(())
    }
    pub(super) fn acknowledge(&mut self, signed: &SignedAck, now: u64) -> Result<ReceiptEffect> {
        check(&self.authorized, &self.cancel)?;
        let ack = signed.verify(
            &self.context,
            &self.session,
            self.submitted,
            now,
            &self.authorized,
        )?;
        if let Some((old, body_hash)) = &self.receipt {
            if ack.sequence < old.sequence {
                return Ok(ReceiptEffect {
                    consumed_batches: 0,
                    closed: self.stopped(),
                });
            }
            if ack.sequence == old.sequence {
                if ack != *old || signed.body_hash() != *body_hash {
                    return Err(Error::Protocol);
                }
                return Ok(ReceiptEffect {
                    consumed_batches: 0,
                    closed: self.stopped(),
                });
            }
        }
        // A receive can forward the latest cumulative ACK and skip earlier ACK
        // operations. Consumption must still stay inside our submitted prefix.
        if self.stopped() || ack.through < self.consumed {
            return Err(Error::Protocol);
        }
        let consumed_batches = ack.through - self.consumed;
        self.consumed = ack.through;
        // Release the signed consumed prefix now, including under full byte
        // pressure. Unconsumed bytes are cancelled only by finish_close after
        // the reverse payload; they never count as consumed/progress.
        self.batches.retain(|b| b.sequence >= self.consumed);
        self.receipt = Some((ack, signed.body_hash()));
        Ok(ReceiptEffect {
            consumed_batches,
            closed: self.stopped(),
        })
    }
}

struct Incoming {
    batch: Batch,
    offset: usize,
}
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Offered {
    Accepted,
    Duplicate,
    Gap,
}
pub(super) struct ReceiveWindow {
    session: String,
    authorized: MembershipCheck,
    cancel: Cancellation,
    next: u64,
    incoming: Option<Incoming>,
}
impl ReceiveWindow {
    pub(super) fn new(
        session: String,
        authorized: MembershipCheck,
        cancel: Cancellation,
    ) -> Result<Self> {
        hash(&session)?;
        check(&authorized, &cancel)?;
        Ok(Self {
            session,
            authorized,
            cancel,
            next: 0,
            incoming: None,
        })
    }
    pub(super) fn cursor(&self) -> u64 {
        self.next
    }
    pub(super) fn pending(&self) -> bool {
        self.incoming.is_some()
    }
    pub(super) fn offer(&mut self, batch: Batch) -> Result<Offered> {
        check(&self.authorized, &self.cancel)?;
        if batch.payload.bytes().is_empty()
            || batch.payload.bytes().len() > MAX_BATCH
            || batch.sequence >= MAX_SAFE_INTEGER
        {
            return Err(Error::Protocol);
        }
        if batch.sequence < self.next {
            return Ok(Offered::Duplicate);
        }
        if batch.sequence > self.next {
            return Ok(Offered::Gap);
        }
        if let Some(current) = &self.incoming {
            if batch.payload.bytes() != current.batch.payload.bytes() {
                return Err(Error::Integrity);
            }
            return Ok(Offered::Duplicate);
        }
        self.incoming = Some(Incoming { batch, offset: 0 });
        Ok(Offered::Accepted)
    }
    /// Processes one bounded TLS input step. A partial TLS record may span
    /// batches. No out-of-order or duplicate byte is ever fed into rustls.
    /// Caller drains plaintext before trying again if rustls has backpressure.
    pub(super) fn feed(&mut self, tls: &mut TlsStream) -> Result<usize> {
        check(&self.authorized, &self.cancel)?;
        if tls.session_id != self.session {
            return Err(Error::Unauthorized);
        }
        let Some(current) = &mut self.incoming else {
            return Ok(0);
        };
        let bytes = current.batch.payload.bytes();
        let count = match tls.receive(&bytes[current.offset..]) {
            Err(Error::Busy) => return Ok(0),
            value => value?,
        };
        current.offset += count;
        if current.offset == bytes.len() {
            self.incoming = None;
            self.next += 1;
        }
        Ok(count)
    }
}

fn check(authorized: &MembershipCheck, cancel: &Cancellation) -> Result<()> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    if !authorized() {
        return Err(Error::Unauthorized);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
