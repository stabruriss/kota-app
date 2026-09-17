//! Bounded account-side composition of the existing TLS, ciphertext windows and
//! application channels. No timer, HTTP retry policy, socket or public progress
//! is created here. The network owner drives these finite steps explicitly.
use super::{
    envelope::Packet,
    framing::{Channels, Framing},
    http::{HttpClient, SendAdmission},
    proof::{Direction, Fields, ReceiveMode, Route, SignedRequest, Target},
    upload::{Builder, Upload},
    window::{OutgoingBatch, ReceiveWindow, SendWindow},
    wire::{Role, MAX_SAFE_INTEGER},
    Error, Result, TlsStream,
};
use crate::bbs_sync::{
    transport::{Context, MAX_CONNECTIONS},
    DeviceIdentity,
};
use std::{collections::BTreeMap, sync::Arc, time::Instant};
use tokio::sync::Notify;

struct Peer {
    tls: TlsStream,
    receive: ReceiveWindow,
    send: SendWindow,
    framing: Option<Framing>,
    channels: Option<Channels>,
    acknowledged: u64,
    ack_sequence: u64,
    ack: Option<Acknowledgement>,
    receive_closed: bool,
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.tls.cancellation().cancel();
    }
}

/// The body is fixed and tiny. It is copied into the small HTTP partition only
/// when submitted, never into the ordinary file/ciphertext budget.
#[derive(Clone)]
pub(super) struct Acknowledgement {
    pub(super) sequence: u64,
    pub(super) through: u64,
    pub(super) request: SignedRequest,
    pub(super) body: String,
    final_flag: bool,
}

struct Pending {
    packet: Packet,
    positions: [usize; MAX_CONNECTIONS],
}

#[derive(Default, Debug)]
pub(super) struct Activity {
    /// Actual TLS input, including handshake bytes; not file completion.
    pub(super) tls_input: usize,
    /// Includes locally buffered output. NOT an endpoint progress clock.
    pub(super) framed: usize,
    pub(super) consumed_batches: u64,
    /// At most four entries. The owner reports the affected peer and can keep
    /// servicing the remaining sessions; one cancellation is not a group error.
    pub(super) retired: Vec<(String, Error)>,
}

pub(super) struct Pump {
    context: Context,
    identity: DeviceIdentity,
    peers: BTreeMap<String, Peer>,
    pending: Option<Pending>,
    first: usize,
    wake: Arc<Notify>,
}
impl Pump {
    pub(super) fn new(context: Context, identity: DeviceIdentity) -> Self {
        Self {
            context,
            identity,
            peers: BTreeMap::new(),
            pending: None,
            first: 0,
            wake: Arc::new(Notify::new()),
        }
    }
    /// `TlsStream` carries the context that passed PendingTls::accept. A caller
    /// cannot pair an authenticated stream with a different HTTP/group context.
    pub(super) fn add(&mut self, mut tls: TlsStream) -> Result<String> {
        tls.handshake_complete()?;
        let c = tls.context();
        if self.context.shutdown.is_cancelled() {
            return Err(Error::Closed);
        }
        if c.peer.local_device_id != self.identity.device_id().map_err(|_| Error::Unauthorized)? {
            return Err(Error::Unauthorized);
        }
        if self.peers.contains_key(&tls.session_id) {
            return Err(Error::Protocol);
        }
        if self.peers.len() == MAX_CONNECTIONS || self.pending.is_some() {
            return Err(Error::Busy);
        }
        for peer in self.peers.values() {
            let old = peer.tls.context();
            if old.origin != c.origin
                || old.boot != c.boot
                || old.peer.group_id != c.peer.group_id
                || old.local_instance != c.local_instance
                || old.peer.local_membership_id != c.peer.local_membership_id
                || old.peer.remote_device_id == c.peer.remote_device_id
            {
                return Err(Error::Unauthorized);
            }
        }
        let id = tls.session_id.clone();
        let receive = ReceiveWindow::new(id.clone(), tls.membership(), tls.cancellation())?;
        let send = SendWindow::new(c.clone(), id.clone(), tls.membership(), tls.cancellation())?;
        self.peers.insert(
            id.clone(),
            Peer {
                tls,
                receive,
                send,
                framing: None,
                channels: None,
                acknowledged: 0,
                ack_sequence: 0,
                ack: None,
                receive_closed: false,
            },
        );
        Ok(id)
    }
    pub(super) fn remove(&mut self, session: &str) {
        if self.peers.remove(session).is_none() {
            return;
        }
        // An old completion has no effect on newer peers. Even a current
        // retirement drops only its slice of an already accepted merged packet.
        if let Some(p) = &mut self.pending {
            if let Some(i) = p.packet.items().iter().position(|i| i.session() == session) {
                p.positions[i] = p.packet.items()[i].batches().len();
            }
        }
        self.release_packet();
    }
    fn check(&self) -> Result<()> {
        if self.context.shutdown.is_cancelled() {
            Err(Error::Closed)
        } else {
            Ok(())
        }
    }
    fn peer(&self, session: &str) -> Result<&Peer> {
        self.check()?;
        self.peers.get(session).ok_or(Error::Closed)
    }
    fn peer_mut(&mut self, session: &str) -> Result<&mut Peer> {
        self.check()?;
        self.peers.get_mut(session).ok_or(Error::Closed)
    }
    pub(super) fn channels(&mut self, session: &str) -> Result<Option<Channels>> {
        Ok(self.peer_mut(session)?.channels.take())
    }
    pub(super) fn data_ready(&self) -> bool {
        self.pending.is_none() && self.peers.values().all(|p| !p.receive.pending())
    }
    pub(super) fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }
    pub(super) fn can_add(&self) -> bool {
        self.peers.len() < MAX_CONNECTIONS && self.pending.is_none()
    }
    pub(super) fn wake(&self) -> Arc<Notify> {
        self.wake.clone()
    }
    pub(super) fn limits(&self) -> super::super::transport::Limits {
        self.context.limits.clone()
    }
    pub(super) fn sessions(&self) -> Vec<String> {
        self.peers.keys().cloned().collect()
    }
    pub(super) fn context(&self) -> Context {
        self.context.clone()
    }
    pub(super) fn byte_wait(&self) -> Option<usize> {
        self.peers
            .values()
            .filter_map(|p| p.framing.as_ref()?.byte_wait())
            .min()
    }
    pub(super) fn authority(
        &self,
        session: &str,
    ) -> Result<(
        super::super::transport::Cancellation,
        super::super::transport::MembershipCheck,
    )> {
        let peer = self.peer(session)?;
        if peer.tls.cancellation().is_cancelled() {
            return Err(Error::Cancelled);
        }
        if !(peer.tls.membership())() {
            return Err(Error::Unauthorized);
        }
        Ok((peer.tls.cancellation(), peer.tls.membership()))
    }
    pub(super) fn acknowledgement_pending(&self, session: &str) -> Result<bool> {
        let peer = self.peer(session)?;
        if peer.tls.cancellation().is_cancelled() {
            return Err(Error::Cancelled);
        }
        if !(peer.tls.membership())() {
            return Err(Error::Unauthorized);
        }
        Ok(!peer.receive_closed
            && (peer.ack.is_some() || peer.receive.cursor() > peer.acknowledged))
    }
    pub(super) fn unacknowledged(&self) -> Result<bool> {
        self.check()?;
        let mut pending = false;
        for peer in self.peers.values() {
            pending |= peer.send.pending()?;
        }
        Ok(pending)
    }
    /// A full data read is not admitted while its predecessor retains ciphertext.
    /// A receipts/probe read may still use the independent small HTTP lane.
    pub(super) fn read(&self, mode: ReceiveMode, now: u64) -> Result<SignedRequest> {
        self.check()?;
        if mode == ReceiveMode::Data && !self.data_ready() {
            return Err(Error::Busy);
        }
        let c = self
            .peers
            .values()
            .next()
            .ok_or(Error::Closed)?
            .tls
            .context();
        let mut cursors = Vec::with_capacity(self.peers.len());
        for (session, peer) in &self.peers {
            // A stale session must not hide behind another peer's membership.
            if peer.tls.cancellation().is_cancelled() {
                return Err(Error::Cancelled);
            }
            if !(peer.tls.membership())() {
                return Err(Error::Unauthorized);
            }
            cursors.push((session.clone(), peer.receive.cursor()));
        }
        SignedRequest::sign(
            &self.identity,
            &c.peer.local_membership_id,
            Target::receive(&c.origin, &c.peer.group_id, &c.boot, mode, &cursors)?,
            Fields::Read,
            &[],
            now,
        )
    }
    /// HTTP already checked the complete envelope against the signed request.
    /// Re-check current sessions before admitting any body. Server cursor and
    /// closed numbers are descriptive; only a peer signature releases uploads.
    pub(super) fn accept(&mut self, packet: Packet, now: u64) -> Result<Activity> {
        self.check()?;
        let has_data = packet.items().iter().any(|i| !i.batches().is_empty());
        if has_data && !self.data_ready() {
            return Err(Error::Busy);
        }
        for item in packet.items() {
            let peer = self.peer(item.session())?;
            if peer.tls.cancellation().is_cancelled() {
                return Err(Error::Cancelled);
            }
            if !(peer.tls.membership())() {
                return Err(Error::Unauthorized);
            }
            for (offset, batch) in item.batches().iter().enumerate() {
                if batch.sequence != peer.receive.cursor() + offset as u64 {
                    return Err(Error::Protocol);
                }
            }
        }
        let mut activity = Activity::default();
        for item in packet.items() {
            if let Some(ack) = item.unverified_ack() {
                let effect = self.peer_mut(item.session())?.send.acknowledge(ack, now)?;
                activity.consumed_batches += effect.consumed_batches;
                // A signed final closes only our sending direction. Preserve
                // this peer and any reverse ciphertext in this very envelope.
            }
        }
        if has_data {
            let mut positions = [0; MAX_CONNECTIONS];
            for (i, item) in packet.items().iter().enumerate() {
                if !self.peers.contains_key(item.session()) {
                    positions[i] = item.batches().len();
                }
            }
            self.pending = Some(Pending { packet, positions });
        }
        self.release_packet();
        Ok(activity)
    }
    /// At most 16 bounded passes per peer. Each input step is <=16 KiB. Rotation
    /// prevents one stalled peer from blocking plaintext work on another peer.
    pub(super) fn step(&mut self) -> Result<Activity> {
        let mut activity = Activity::default();
        let ids: Vec<_> = self.peers.keys().cloned().collect();
        if ids.is_empty() {
            self.release_packet();
            return Ok(activity);
        }
        if self.context.shutdown.is_cancelled() {
            return Err(Error::Closed);
        }
        for pass in 0..16 {
            for n in 0..ids.len() {
                let id = &ids[(self.first + n) % ids.len()];
                let Some(peer) = self.peers.get_mut(id) else {
                    continue;
                };
                match advance_peer(
                    peer,
                    self.pending.as_mut(),
                    &self.context,
                    self.wake.clone(),
                ) {
                    Ok(a) => {
                        activity.tls_input += a.tls_input;
                        activity.framed += a.framed;
                        activity.consumed_batches += a.consumed_batches;
                    }
                    Err(error) => {
                        self.remove(id);
                        activity.retired.push((id.clone(), error));
                    }
                }
            }
            if pass == 15 {
                self.first = (self.first + 1) % ids.len();
            }
        }
        for id in ids {
            let Some(peer) = self.peers.get_mut(&id) else {
                continue;
            };
            let payload_done = !peer.receive.pending()
                && self.pending.as_ref().is_none_or(|p| {
                    p.packet.items().iter().enumerate().all(|(i, item)| {
                        item.session() != id || p.positions[i] == item.batches().len()
                    })
                });
            if payload_done {
                let error = match peer.send.finish_close() {
                    Err(error) => Some(error),
                    Ok(()) if peer.send.closed() && peer.receive_closed => Some(Error::Closed),
                    Ok(()) => None,
                };
                if let Some(error) = error {
                    self.remove(&id);
                    activity.retired.push((id, error));
                }
            }
        }
        self.release_packet();
        Ok(activity)
    }
    fn release_packet(&mut self) {
        if self.pending.as_ref().is_some_and(|p| {
            p.packet
                .items()
                .iter()
                .enumerate()
                .all(|(i, item)| p.positions[i] == item.batches().len())
        }) {
            self.pending = None;
        }
    }
    /// The caller owns this admission until it submits or abandons the builder.
    /// Window capacity and response headroom precede any ciphertext allocation.
    pub(super) fn prepare_upload(
        &self,
        session: &str,
        http: &HttpClient,
        deadline: Instant,
    ) -> Result<SendAdmission> {
        let peer = self.peer(session)?;
        if !peer.send.writable()? {
            return Err(Error::Busy);
        }
        http.prepare_send(peer.tls.cancellation(), peer.tls.membership(), deadline)
    }
    pub(super) fn has_output(&mut self, session: &str) -> Result<bool> {
        let peer = self.peer_mut(session)?;
        Ok(peer.send.writable()? && peer.tls.wants_write()?)
    }
    pub(super) fn fill(
        &mut self,
        session: &str,
        builder: &mut Builder,
        admission: &SendAdmission,
    ) -> Result<usize> {
        admission.check()?;
        let peer = self.peer_mut(session)?;
        if !peer.send.writable()? {
            return Err(Error::Busy);
        }
        builder.fill(&mut peer.tls)
    }
    pub(super) fn enqueue(
        &mut self,
        session: &str,
        body: Upload,
        now: u64,
    ) -> Result<OutgoingBatch> {
        self.check()?;
        let identity = &self.identity;
        let peer = self.peers.get_mut(session).ok_or(Error::Closed)?;
        let sequence = peer.send.enqueue(identity, body, now)?;
        peer.send.batch(sequence)
    }
    pub(super) fn submitted(&mut self, session: &str, sequence: u64) -> Result<()> {
        self.peer_mut(session)?.send.submitted(sequence)
    }
    pub(super) fn retry(&self, session: &str, sequence: u64) -> Result<OutgoingBatch> {
        self.peer(session)?.send.batch(sequence)
    }
    pub(super) fn upload_pending(&self, session: &str, sequence: u64) -> Result<bool> {
        self.peer(session)?.send.contains(sequence)
    }
    pub(super) fn send_stopped(&self, session: &str) -> Result<bool> {
        Ok(self.peer(session)?.send.stopped())
    }
    /// One immutable, coalesced cumulative acknowledgement per peer. HTTP reply
    /// loss retries the same proof/body. Newer consumption waits behind it.
    pub(super) fn acknowledgement(
        &mut self,
        session: &str,
        now: u64,
    ) -> Result<Option<Acknowledgement>> {
        self.acknowledgement_kind(session, now, false)
    }
    /// Build the one normal close receipt for a direction.  The final bit is
    /// signed over the same cursor and operation sequence as an ordinary ACK;
    /// it is only emitted when all locally received bytes are consumed and no
    /// earlier ACK is in flight. Both independently closed directions are
    /// required to retire a peer; this never asserts content completion.
    pub(super) fn final_acknowledgement(
        &mut self,
        session: &str,
        now: u64,
    ) -> Result<Option<Acknowledgement>> {
        self.acknowledgement_kind(session, now, true)
    }
    fn acknowledgement_kind(
        &mut self,
        session: &str,
        now: u64,
        final_flag: bool,
    ) -> Result<Option<Acknowledgement>> {
        self.check()?;
        let peer = self.peers.get_mut(session).ok_or(Error::Closed)?;
        if peer.tls.cancellation().is_cancelled() {
            return Err(Error::Cancelled);
        }
        if !(peer.tls.membership())() {
            return Err(Error::Unauthorized);
        }
        if let Some(ack) = &peer.ack {
            return Ok(Some(ack.clone()));
        }
        if peer.receive_closed {
            return Ok(None);
        }
        if final_flag
            && (peer.receive.pending()
                || self.pending.as_ref().is_some_and(|p| {
                    p.packet.items().iter().enumerate().any(|(i, item)| {
                        item.session() == session && p.positions[i] != item.batches().len()
                    })
                }))
        {
            return Err(Error::Busy);
        }
        let through = peer.receive.cursor();
        if through == peer.acknowledged && !final_flag {
            return Ok(None);
        }
        if peer.ack_sequence >= MAX_SAFE_INTEGER {
            return Err(Error::Protocol);
        }
        let c = peer.tls.context();
        let body = format!("{{\"through\":\"{through}\",\"final\":{final_flag}}}");
        let request = SignedRequest::sign(
            &self.identity,
            &c.peer.local_membership_id,
            Target::post(&c.origin, &c.peer.group_id, Route::Ack)?,
            Fields::Frame {
                boot: &c.boot,
                session,
                direction: if c.role() == Role::Client {
                    Direction::ServerToClient
                } else {
                    Direction::ClientToServer
                },
                sequence: peer.ack_sequence,
                final_flag,
            },
            body.as_bytes(),
            now,
        )?;
        let ack = Acknowledgement {
            sequence: peer.ack_sequence,
            through,
            request,
            body,
            final_flag,
        };
        peer.ack = Some(ack.clone());
        Ok(Some(ack))
    }
    /// Called after this exact ACK mutation is confirmed; never a file or round
    /// success. A mismatched/late completion cannot discard a newer intent.
    pub(super) fn ack_submitted(&mut self, session: &str, sequence: u64) -> Result<()> {
        let peer = self.peer_mut(session)?;
        if peer.tls.cancellation().is_cancelled() {
            return Err(Error::Cancelled);
        }
        if !(peer.tls.membership())() {
            return Err(Error::Unauthorized);
        }
        let ack = peer
            .ack
            .as_ref()
            .filter(|a| a.sequence == sequence)
            .ok_or(Error::Protocol)?;
        peer.acknowledged = ack.through;
        peer.receive_closed = ack.final_flag;
        peer.ack_sequence += 1;
        peer.ack = None;
        Ok(())
    }
}

fn advance_peer(
    peer: &mut Peer,
    mut pending: Option<&mut Pending>,
    context: &Context,
    wake: Arc<Notify>,
) -> Result<Activity> {
    let mut activity = Activity::default();
    let id = &peer.tls.session_id;
    let item = pending
        .as_mut()
        .and_then(|p| p.packet.items().iter().position(|i| i.session() == id));
    if let (Some(p), Some(i)) = (pending.as_mut(), item) {
        if !peer.receive.pending() {
            if let Some(batch) = p.packet.items()[i].batches().get(p.positions[i]) {
                if peer.receive.offer(batch.clone())? != super::window::Offered::Accepted {
                    return Err(Error::Protocol);
                }
            }
        }
    }
    let before = peer.receive.cursor();
    activity.tls_input += peer.receive.feed(&mut peer.tls)?;
    if peer.receive.cursor() != before {
        activity.consumed_batches += 1;
        if let (Some(p), Some(i)) = (pending.as_mut(), item) {
            p.positions[i] += 1;
        }
    }
    if peer.framing.is_none() && peer.tls.handshake_complete()? {
        let cancel = peer.tls.cancellation();
        let membership = peer.tls.membership();
        let (channels, framing) =
            Framing::start_with_wake(&mut peer.tls, context, cancel, membership, wake)?;
        peer.channels = Some(channels);
        peer.framing = Some(framing);
    }
    if let Some(framing) = &mut peer.framing {
        activity.framed += framing.step_direction(&mut peer.tls, !peer.send.stopped())?;
    }
    Ok(activity)
}

#[cfg(test)]
mod tests;
