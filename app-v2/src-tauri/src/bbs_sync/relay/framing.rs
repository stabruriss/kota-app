//! Two logical channels inside the authenticated TLS stream. The carrier owns
//! only framing: application credit, ACK after writing, fsync and SHA verification
//! stay in the same ControlChannel/DataPipe used by the existing transport.
use super::{Error, Result, TlsStream};
use crate::bbs_sync::transport::{
    channel::{Channel, Outbound, RelayPort},
    control_channel::ControlChannel,
    data_channel::DataPipe,
    protocol::Frame,
    Cancellation, Connection, Context, Delivery, Limits, MembershipCheck, CONTROL_WINDOW,
    DATA_WINDOW, MAX_FRAME,
};
use std::sync::Arc;
use tokio::sync::{mpsc, OwnedSemaphorePermit};

const HEADER: usize = 5;
#[derive(Clone, Copy)]
enum Lane {
    Control = 1,
    Data = 2,
}
struct Sending {
    message: Outbound,
    header: [u8; HEADER],
    offset: usize,
}

pub(super) struct Channels {
    pub(super) control: Arc<ControlChannel>,
    pub(super) data: Arc<DataPipe>,
    pub(super) incoming: mpsc::Receiver<Delivery>,
    cancel: Option<Cancellation>,
    authorized: MembershipCheck,
}
impl Drop for Channels {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
        }
    }
}
impl Channels {
    /// Transfer the single receive owner to the content connection. Only the
    /// successful transfer disarms Drop; any rejected adoption closes the TLS
    /// channels. Framing still owns their shared connection/byte permits.
    pub(super) fn into_connection(mut self) -> Result<Connection> {
        let cancel = self.cancel.as_ref().ok_or(Error::Closed)?.clone();
        let incoming = std::mem::replace(&mut self.incoming, mpsc::channel(1).1);
        let connection = Connection::from_relay(
            self.control.clone(),
            incoming,
            self.data.clone(),
            cancel,
            self.authorized.clone(),
        )?;
        self.cancel = None;
        Ok(connection)
    }
}
pub(super) struct Framing {
    control: RelayPort,
    data: RelayPort,
    sending: Option<Sending>,
    header: [u8; HEADER],
    header_read: usize,
    frame: Option<Frame>,
    frame_read: usize,
    limits: Limits,
    cancel: Cancellation,
    authorized: MembershipCheck,
    shutdown: Cancellation,
    session: String,
    byte_blocked: bool,
    _connection: OwnedSemaphorePermit,
}
impl Drop for Framing {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl Framing {
    #[cfg(test)]
    pub(super) fn start(
        tls: &mut TlsStream,
        context: &Context,
        cancel: Cancellation,
        authorized: MembershipCheck,
    ) -> Result<(Channels, Self)> {
        Self::start_with_wake(
            tls,
            context,
            cancel,
            authorized,
            Arc::new(tokio::sync::Notify::new()),
        )
    }
    /// Called only after both certificate possession checks completed. No
    /// socket, file worker or polling timer is created here.
    pub(super) fn start_with_wake(
        tls: &mut TlsStream,
        context: &Context,
        cancel: Cancellation,
        authorized: MembershipCheck,
        wake: Arc<tokio::sync::Notify>,
    ) -> Result<(Channels, Self)> {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if context.shutdown.is_cancelled() {
            return Err(Error::Closed);
        }
        if !authorized() {
            return Err(Error::Unauthorized);
        }
        if !tls.handshake_complete()? {
            return Err(Error::Busy);
        }
        let permit = context.limits.connection()?;
        let (control, control_port) = Channel::relay(CONTROL_WINDOW, wake.clone());
        let (data, data_port) = Channel::relay(DATA_WINDOW + 2, wake);
        let (control, incoming) = ControlChannel::start_on(
            control,
            context.limits.clone(),
            cancel.clone(),
            authorized.clone(),
        );
        let data = DataPipe::start_on(
            data,
            context.limits.clone(),
            context.io.clone(),
            cancel.clone(),
            authorized.clone(),
        );
        Ok((
            Channels {
                control,
                data,
                incoming,
                cancel: Some(cancel.clone()),
                authorized: authorized.clone(),
            },
            Self {
                control: control_port,
                data: data_port,
                sending: None,
                header: [0; HEADER],
                header_read: 0,
                frame: None,
                frame_read: 0,
                limits: context.limits.clone(),
                cancel,
                authorized,
                shutdown: context.shutdown.clone(),
                session: tls.session_id.clone(),
                byte_blocked: false,
                _connection: permit,
            },
        ))
    }
    /// A bounded synchronous step for the network owner. A zero result is
    /// backpressure/empty, never evidence of endpoint or file progress.
    pub(super) fn step(&mut self, tls: &mut TlsStream) -> Result<usize> {
        self.step_direction(tls, true)
    }
    /// A peer final stops outbound plaintext without closing reverse input.
    pub(super) fn step_direction(&mut self, tls: &mut TlsStream, send: bool) -> Result<usize> {
        self.byte_blocked = false;
        let result = self.advance(tls, send);
        if result.is_err() {
            self.cancel.cancel();
        }
        result
    }
    pub(super) fn byte_wait(&self) -> Option<usize> {
        self.byte_blocked.then_some(MAX_FRAME)
    }
    fn advance(&mut self, tls: &mut TlsStream, send: bool) -> Result<usize> {
        if self.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if self.shutdown.is_cancelled() {
            return Err(Error::Closed);
        }
        if !(self.authorized)() {
            return Err(Error::Unauthorized);
        }
        if tls.session_id != self.session {
            return Err(Error::Unauthorized);
        }
        if !tls.handshake_complete()? {
            return Err(Error::Protocol);
        }
        // Only unencrypted frames can be prioritized. Never reorder bytes after
        // they have entered TLS, including a partially written frame.
        let mut moved = if send { self.write(tls)? } else { 0 };
        moved += self.read(tls)?;
        Ok(moved)
    }
    fn take(port: &mut RelayPort) -> Result<Option<Outbound>> {
        match port.send.try_recv() {
            Ok(frame) => Ok(Some(frame)),
            Err(mpsc::error::TryRecvError::Empty) => Ok(None),
            Err(_) => Err(Error::Closed),
        }
    }
    fn write(&mut self, tls: &mut TlsStream) -> Result<usize> {
        if self.sending.is_none() {
            let next = if let Some(frame) = Self::take(&mut self.control)? {
                Some((Lane::Control, frame))
            } else {
                Self::take(&mut self.data)?.map(|frame| (Lane::Data, frame))
            };
            if let Some((lane, message)) = next {
                let len = message.frame.bytes.len();
                if len == 0 || len > MAX_FRAME {
                    return Err(Error::Protocol);
                }
                let mut header = [0; HEADER];
                header[0] = lane as u8;
                header[1..].copy_from_slice(&(len as u32).to_be_bytes());
                self.sending = Some(Sending {
                    message,
                    header,
                    offset: 0,
                });
            }
        }
        let Some(current) = &mut self.sending else {
            return Ok(0);
        };
        let bytes = if current.offset < HEADER {
            &current.header[current.offset..]
        } else {
            &current.message.frame.bytes[current.offset - HEADER..]
        };
        let count = tls.write_plaintext(bytes)?;
        current.offset += count;
        if current.offset == HEADER + current.message.frame.bytes.len() {
            self.sending
                .take()
                .expect("present until accepted")
                .message
                .accepted();
        }
        Ok(count)
    }
    fn read(&mut self, tls: &mut TlsStream) -> Result<usize> {
        let mut count = 0;
        if self.header_read < HEADER {
            let n = tls.read_plaintext(&mut self.header[self.header_read..])?;
            self.header_read += n;
            count += n;
            if self.header_read < HEADER {
                return Ok(count);
            }
        }
        let length =
            u32::from_be_bytes(self.header[1..].try_into().map_err(|_| Error::Protocol)?) as usize;
        let port = match self.header[0] {
            1 => &self.control,
            2 => &self.data,
            _ => return Err(Error::Protocol),
        };
        if length == 0 || length > MAX_FRAME {
            return Err(Error::Protocol);
        }
        if self.frame.is_none() {
            let mut frame = match Frame::allocate(&self.limits) {
                Ok(frame) => frame,
                // Keep the bounded header and rustls plaintext in place. Do not
                // acknowledge application consumption or allocate around credit.
                Err(Error::Busy) => {
                    self.byte_blocked = true;
                    return Ok(count);
                }
                Err(error) => return Err(error),
            };
            frame.bytes.resize(length, 0);
            self.frame = Some(frame);
        }
        let frame = self.frame.as_mut().ok_or(Error::Protocol)?;
        let n = tls.read_plaintext(&mut frame.bytes[self.frame_read..])?;
        self.frame_read += n;
        count += n;
        if self.frame_read == length {
            // Protocol credit bounds the sum of these queues and the consuming
            // state-machine queues. Overflow is a violating peer, not data loss.
            port.receive
                .try_send(self.frame.take().ok_or(Error::Protocol)?)
                .map_err(|_| Error::Protocol)?;
            self.header_read = 0;
            self.frame_read = 0;
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests;
