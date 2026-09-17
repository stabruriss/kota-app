//! The two concrete frame carriers. File/control credit remains in its original
//! state machine; the relay carrier only moves already-accounted frames.
use super::{protocol::Frame, Cancellation, Error, Limits, Result, MAX_FRAME, PROGRESS_TIMEOUT};
use bytes::BytesMut;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex, Notify};
use webrtc::data_channel::{DataChannel, DataChannelEvent};

#[derive(Clone)]
pub(crate) enum Channel {
    Rtc(Arc<dyn DataChannel>, Limits),
    Relay(Arc<RelayChannel>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bbs_sync::transport::{protocol, APP_QUEUE_BYTES};
    use std::time::Duration;

    #[tokio::test]
    async fn relay_frame_encoding_waits_for_real_credit_and_carrier_acceptance() {
        let limits = Limits::default();
        let held = limits.reserve(APP_QUEUE_BYTES).unwrap();
        let cancel = Cancellation::default();
        let (channel, mut port) = Channel::relay(1, Arc::new(Notify::new()));
        let send = channel.send_encoded(&limits, &cancel, MAX_FRAME, || {
            protocol::encode_control(protocol::Control::Credit(7), &limits)
        });
        tokio::pin!(send);
        assert!(tokio::time::timeout(Duration::from_millis(10), &mut send)
            .await
            .is_err());
        assert!(port.send.try_recv().is_err());
        assert!(!cancel.is_cancelled());
        drop(held);
        let frame = tokio::select! {
            value = &mut send => panic!("frame not accepted yet: {value:?}"),
            value = port.send.recv() => value.unwrap(),
        };
        assert!(matches!(
            protocol::decode_control(&frame.frame.bytes),
            Ok(protocol::Control::Credit(7))
        ));
        assert!(matches!(limits.reserve(APP_QUEUE_BYTES), Err(Error::Busy)));
        frame.accepted();
        assert_eq!(send.await, Ok(()));
        assert!(limits.reserve(APP_QUEUE_BYTES).is_ok());
    }

    #[tokio::test]
    async fn cancellation_interrupts_relay_encoding_without_consuming_credit() {
        let limits = Limits::default();
        let held = limits.reserve(APP_QUEUE_BYTES).unwrap();
        let cancel = Cancellation::default();
        let (channel, mut port) = Channel::relay(1, Arc::new(Notify::new()));
        let send = channel.send_encoded(&limits, &cancel, MAX_FRAME, || {
            protocol::encode_control(protocol::Control::Credit(1), &limits)
        });
        tokio::pin!(send);
        assert!(tokio::time::timeout(Duration::from_millis(10), &mut send)
            .await
            .is_err());
        cancel.cancel();
        assert_eq!(send.await, Err(Error::Cancelled));
        assert!(port.send.try_recv().is_err());
        drop(held);
        assert!(limits.reserve(APP_QUEUE_BYTES).is_ok());
    }

    #[tokio::test]
    async fn relay_file_ack_uses_its_exact_bytes_when_bulk_frame_credit_is_exhausted() {
        let limits = Limits::default();
        let held = limits.reserve(APP_QUEUE_BYTES - 8192).unwrap();
        assert!(matches!(Frame::allocate(&limits), Err(Error::Busy)));
        let cancel = Cancellation::default();
        let (channel, mut port) = Channel::relay(1, Arc::new(Notify::new()));
        let ack = protocol::Data::Ack {
            id: protocol::TransferId([7; 16]),
            offset: 16359,
        };
        let capacity = channel.encoding_capacity(ack.compact_capacity());
        assert_eq!(capacity, 25);
        let send = channel.send_encoded(&limits, &cancel, capacity, || {
            ack.encode_sized(&limits, capacity)
        });
        tokio::pin!(send);
        let frame = tokio::select! {
            value = &mut send => panic!("frame not accepted yet: {value:?}"),
            value = port.send.recv() => value.unwrap(),
        };
        assert_eq!(frame.frame.bytes.capacity(), 25);
        assert!(matches!(
            protocol::decode_data(&frame.frame.bytes),
            Ok(protocol::Data::Ack { offset: 16359, .. })
        ));
        assert!(limits.reserve(8192 - 25).is_ok());
        assert!(matches!(limits.reserve(8192), Err(Error::Busy)));
        frame.accepted();
        assert_eq!(send.await, Ok(()));
        assert!(limits.reserve(8192).is_ok());
        drop(held);
        assert!(limits.reserve(APP_QUEUE_BYTES).is_ok());
    }
}
pub(crate) struct RelayChannel {
    send: mpsc::Sender<Outbound>,
    receive: Mutex<mpsc::Receiver<Frame>>,
    wake: Arc<Notify>,
}
pub(crate) struct Outbound {
    pub(crate) frame: Frame,
    accepted: oneshot::Sender<()>,
}
impl Outbound {
    pub(crate) fn accepted(self) {
        // Release plaintext before waking the next file read/control producer.
        drop(self.frame);
        let _ = self.accepted.send(());
    }
}
pub(crate) struct RelayPort {
    pub(crate) send: mpsc::Receiver<Outbound>,
    pub(crate) receive: mpsc::Sender<Frame>,
}
impl Channel {
    pub(crate) fn encoding_capacity(&self, compact: usize) -> usize {
        match self {
            Self::Rtc(..) => MAX_FRAME,
            Self::Relay(_) => compact,
        }
    }
    /// Relay HTTP windows share the same byte budget as these frames. A full
    /// window is backpressure, not loss of the authenticated connection. RTC
    /// retains its original immediate admission; relay waits for real credit.
    pub(crate) async fn send_encoded(
        &self,
        limits: &Limits,
        cancel: &Cancellation,
        capacity: usize,
        encode: impl Fn() -> Result<Frame>,
    ) -> Result<()> {
        let operation = async {
            let frame = loop {
                match encode() {
                    Err(Error::Busy) if matches!(self, Self::Relay(_)) => {
                        limits.wait_for_bytes(capacity).await?;
                    }
                    result => break result?,
                }
            };
            self.send(frame, cancel).await
        };
        tokio::select! {
            _ = cancel.cancelled() => Err(Error::Cancelled),
            result = tokio::time::timeout(PROGRESS_TIMEOUT, operation) => result.map_err(|_| Error::Timeout)?,
        }
    }
    pub(crate) fn relay(capacity: usize, wake: Arc<Notify>) -> (Self, RelayPort) {
        let (send, outgoing) = mpsc::channel(1);
        let (receive, incoming) = mpsc::channel(capacity);
        (
            Self::Relay(Arc::new(RelayChannel {
                send,
                receive: Mutex::new(incoming),
                wake,
            })),
            RelayPort {
                send: outgoing,
                receive,
            },
        )
    }
    pub(crate) async fn receive(&self) -> Result<Frame> {
        match self {
            Self::Relay(channel) => {
                let frame = channel
                    .receive
                    .lock()
                    .await
                    .recv()
                    .await
                    .ok_or(Error::Closed)?;
                // A hint to revisit buffered plaintext, never proof that the
                // application consumed/wrote this frame or returned credit.
                channel.wake.notify_one();
                Ok(frame)
            }
            Self::Rtc(channel, limits) => loop {
                match channel.poll().await {
                    Some(DataChannelEvent::OnMessage(message)) if !message.is_string => {
                        return Frame::received(message.data, limits)
                    }
                    Some(DataChannelEvent::OnMessage(_)) => return Err(Error::Protocol),
                    None
                    | Some(
                        DataChannelEvent::OnClose
                        | DataChannelEvent::OnClosing
                        | DataChannelEvent::OnError,
                    ) => return Err(Error::Closed),
                    _ => {}
                }
            },
        }
    }
    pub(crate) async fn send(&self, mut frame: Frame, cancel: &Cancellation) -> Result<()> {
        let operation = async {
            match self {
                Self::Rtc(channel, _) => {
                    let bytes = std::mem::replace(&mut frame.bytes, BytesMut::new());
                    channel.send(bytes).await.map_err(|_| Error::Closed)?;
                    // Keep the frame's permit while the carrier owns its bytes.
                    drop(frame);
                }
                Self::Relay(channel) => {
                    let (accepted, done) = oneshot::channel();
                    channel
                        .send
                        .send(Outbound { frame, accepted })
                        .await
                        .map_err(|_| Error::Closed)?;
                    // Store one coalesced permit even if the owner has not yet
                    // polled its notification future. No per-frame task/timer.
                    channel.wake.notify_one();
                    done.await.map_err(|_| Error::Closed)?;
                }
            }
            Ok(())
        };
        tokio::select! {
            _ = cancel.cancelled() => Err(Error::Cancelled),
            result = tokio::time::timeout(PROGRESS_TIMEOUT, operation) => result.map_err(|_| Error::Timeout)?,
        }
    }
}
