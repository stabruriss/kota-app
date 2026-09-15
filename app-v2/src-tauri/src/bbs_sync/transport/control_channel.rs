//! Independent control traffic. The consumer explicitly returns credit only
//! after parsing/handling a message; retaining a delivery stalls its sender.
use super::{
    protocol::{self, Control, Frame},
    Cancellation, Error, Limits, MembershipCheck, Result, CONTROL_WINDOW, MAX_FRAME,
    PROGRESS_TIMEOUT,
};
use bytes::BytesMut;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex, OwnedSemaphorePermit, Semaphore};
use webrtc::data_channel::{DataChannel, DataChannelEvent};

pub(crate) struct Delivery {
    frame: Frame,
    seq: u64,
    consumed: mpsc::Sender<u64>,
    cancel: Cancellation,
}
impl Delivery {
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.frame.bytes[9..]
    }
    pub(crate) fn acknowledge(self) -> Result<()> {
        let seq = self.seq;
        let tx = self.consumed.clone();
        let cancel = self.cancel.clone();
        drop(self); // Release payload memory before allowing another message.
        tx.try_send(seq).map_err(|_| {
            cancel.cancel();
            Error::Closed
        })
    }
    /// Parsing may finish before a bounded repository operation. Retain its
    /// byte reservation until that operation consumes the parsed message.
    pub(crate) fn acknowledge_retaining(self) -> Result<RetainedDelivery> {
        self.consumed.try_send(self.seq).map_err(|_| {
            self.cancel.cancel();
            Error::Closed
        })?;
        Ok(RetainedDelivery { _frame: self.frame })
    }
    pub(crate) fn reject(self) {
        self.cancel.cancel();
    }
}
pub(crate) struct RetainedDelivery {
    _frame: Frame,
}
struct Pending {
    reply: oneshot::Sender<Result<()>>,
    _slot: OwnedSemaphorePermit,
}
struct Outgoing {
    next: u64,
    acked: u64,
    pending: BTreeMap<u64, Pending>,
}
pub(crate) struct ControlChannel {
    channel: Arc<dyn DataChannel>,
    send_lock: AsyncMutex<()>,
    outgoing: Mutex<Outgoing>,
    slots: Arc<Semaphore>,
    limits: Limits,
    cancel: Cancellation,
    authorized: MembershipCheck,
}
impl ControlChannel {
    /// Called on the dedicated network runtime after both channels opened and
    /// the signed remote description passed validation.
    pub(crate) fn start(
        channel: Arc<dyn DataChannel>,
        limits: Limits,
        cancel: Cancellation,
        authorized: MembershipCheck,
    ) -> (Arc<Self>, mpsc::Receiver<Delivery>) {
        let (deliver, incoming) = mpsc::channel(CONTROL_WINDOW);
        let (consumed, acknowledgements) = mpsc::channel(CONTROL_WINDOW);
        let this = Arc::new(Self {
            channel,
            send_lock: AsyncMutex::new(()),
            outgoing: Mutex::new(Outgoing {
                next: 1,
                acked: 0,
                pending: BTreeMap::new(),
            }),
            slots: Arc::new(Semaphore::new(CONTROL_WINDOW)),
            limits,
            cancel,
            authorized,
        });
        let driver = this.clone();
        tokio::spawn(async move {
            let result = driver.pump(deliver, consumed, acknowledgements).await;
            driver.cancel.cancel();
            if let Ok(mut state) = driver.outgoing.lock() {
                for (_, pending) in std::mem::take(&mut state.pending) {
                    let _ = pending
                        .reply
                        .send(Err(result.err().unwrap_or(Error::Closed)));
                }
            }
        });
        (this, incoming)
    }
    fn check(&self) -> Result<()> {
        if self.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if !(self.authorized)() {
            self.cancel.cancel();
            return Err(Error::Unauthorized);
        }
        Ok(())
    }
    /// Completion means the remote application returned credit, not merely
    /// that SCTP accepted the bytes. No buffered_amount-based assumptions.
    pub(crate) async fn send(&self, payload: &[u8]) -> Result<()> {
        self.check()?;
        if payload.len() > MAX_FRAME - 9 {
            return Err(Error::Protocol);
        }
        let task = async {
            let _serial = self.send_lock.lock().await;
            self.check()?;
            let slot = self
                .slots
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| Error::Closed)?;
            let (done, received) = oneshot::channel();
            let seq = {
                let mut state = self.outgoing.lock().map_err(|_| Error::Closed)?;
                let seq = state.next;
                state.next = state.next.checked_add(1).ok_or(Error::Protocol)?;
                state.pending.insert(
                    seq,
                    Pending {
                        reply: done,
                        _slot: slot,
                    },
                );
                seq
            };
            let frame = protocol::encode_control(
                Control::Message {
                    seq,
                    bytes: payload,
                },
                &self.limits,
            )?;
            send_frame(&self.channel, frame, &self.cancel).await?;
            drop(_serial);
            received.await.map_err(|_| Error::Closed)?
        };
        let result = tokio::select! {
            _ = self.cancel.cancelled() => Err(Error::Cancelled),
            value = tokio::time::timeout(PROGRESS_TIMEOUT, task) => value.unwrap_or(Err(Error::Timeout)),
        };
        if result.is_err() {
            self.cancel.cancel();
        }
        result
    }
    async fn pump(
        &self,
        deliver: mpsc::Sender<Delivery>,
        consumed: mpsc::Sender<u64>,
        mut acknowledgements: mpsc::Receiver<u64>,
    ) -> Result<()> {
        let mut received = 0u64;
        let mut handled = 0u64;
        loop {
            self.check()?;
            tokio::select! {
                _ = self.cancel.cancelled() => return Err(Error::Cancelled),
                ack = acknowledgements.recv() => {
                    self.check()?;
                    let ack = ack.ok_or(Error::Closed)?;
                    if ack != handled.checked_add(1).ok_or(Error::Protocol)? || ack > received { return Err(Error::Protocol); }
                    let frame = protocol::encode_control(Control::Credit(ack), &self.limits)?;
                    send_frame(&self.channel, frame, &self.cancel).await?;
                    handled = ack;
                }
                event = self.channel.poll() => {
                    self.check()?;
                    match event {
                        Some(DataChannelEvent::OnMessage(message)) => {
                            if message.is_string { return Err(Error::Protocol); }
                            let frame = Frame::received(message.data, &self.limits)?;
                            match protocol::decode_control(&frame.bytes)? {
                                Control::Credit(seq) => {
                                    let mut state = self.outgoing.lock().map_err(|_| Error::Closed)?;
                                    if seq != state.acked.checked_add(1).ok_or(Error::Protocol)? { return Err(Error::Protocol); }
                                    let pending = state.pending.remove(&seq).ok_or(Error::Protocol)?;
                                    state.acked = seq;
                                    let _ = pending.reply.send(Ok(()));
                                }
                                Control::Message { seq, .. } => {
                                    if seq != received.checked_add(1).ok_or(Error::Protocol)? || received - handled >= CONTROL_WINDOW as u64 {
                                        return Err(Error::Protocol);
                                    }
                                    received = seq;
                                    // A compliant sender has <=CONTROL_WINDOW unconsumed messages;
                                    // this cannot overflow even when the consumer stops completely.
                                    deliver.try_send(Delivery { frame, seq, consumed:consumed.clone(), cancel:self.cancel.clone() })
                                        .map_err(|_| Error::Closed)?;
                                }
                            }
                        }
                        Some(DataChannelEvent::OnClose | DataChannelEvent::OnError | DataChannelEvent::OnClosing) | None => return Err(Error::Closed),
                        _ => {},
                    }
                }
            }
        }
    }
}
pub(crate) async fn send_frame(
    channel: &Arc<dyn DataChannel>,
    mut frame: Frame,
    cancel: &Cancellation,
) -> Result<()> {
    let bytes = std::mem::replace(&mut frame.bytes, BytesMut::new());
    tokio::select! {
        _ = cancel.cancelled() => Err(Error::Cancelled),
        result = tokio::time::timeout(PROGRESS_TIMEOUT, channel.send(bytes)) => {
            result.map_err(|_| Error::Timeout)?.map_err(|_| Error::Closed)?;
            Ok(())
        }
    }
    // Frame's budget permit remains held until the send future resolves/drops.
}
