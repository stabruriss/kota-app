//! Finite declaration exchange. The coordinator schedules bounded HTTP work;
//! this owner creates no worker, timer, socket, identity or implicit retry.
use super::{
    discovery::{Declaration, HandshakeView},
    proof::{compact_json, Fields, Route, SignedRequest, Target},
    wire::{Declarations, Role},
    Error, PendingTls, Result, SessionContext, TlsStream,
};
use crate::bbs_sync::{
    transport::{Cancellation, MembershipCheck, PROGRESS_TIMEOUT},
    DeviceIdentity,
};
use serde::{Deserialize, Serialize};
use std::time::Instant;

const MAX_OPEN: usize = 2 * super::wire::MAX_STATEMENT_BYTES + 32;

/// Body bytes never change under this handshake. A lost HTTP reply may retry
/// the same body; the boot-domain proof can refresh its bounded wall-clock time.
struct Message(Vec<u8>);
impl Message {
    fn open(client: &Declaration, server: Option<&Declaration>) -> Result<Self> {
        let mut bytes = b"{\"client\":".to_vec();
        bytes.extend_from_slice(client.bytes());
        bytes.extend_from_slice(b",\"server\":");
        bytes.extend_from_slice(server.map(Declaration::bytes).unwrap_or(b"null"));
        bytes.push(b'}');
        compact_json(&bytes, MAX_OPEN)?;
        Ok(Self(bytes))
    }
}

pub(super) struct Handshake {
    context: SessionContext,
    pending: Option<PendingTls>,
    local: Declaration,
    remote: Option<Declaration>,
    message: Message,
    authorized: MembershipCheck,
    cancel: Cancellation,
    deadline: Instant,
    ended: bool,
}
impl Handshake {
    pub(super) fn context(&self) -> &SessionContext {
        &self.context
    }
    /// Call with the current pair view after both ready; None means keep waiting
    /// within the existing start window. No ephemeral key is created on None.
    pub(super) fn start(
        identity: &DeviceIdentity,
        context: SessionContext,
        view: &HandshakeView,
        authorized: MembershipCheck,
        cancel: Cancellation,
        now: u64,
        at: Instant,
    ) -> Result<Option<Self>> {
        context.validate()?;
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if !authorized() {
            return Err(Error::Unauthorized);
        }
        if view.closed || view.session.is_some() {
            return Err(Error::RelaySessionLost);
        }
        if !view.ready.client || !view.ready.server {
            return Ok(None);
        }
        let (pending, remote) = match context.role() {
            Role::Client => {
                // No attempt may adopt another attempt's ephemeral private key.
                if view.client.is_some() || view.server.is_some() {
                    return Err(Error::RelaySessionLost);
                }
                (
                    PendingTls::client(
                        identity,
                        context.clone(),
                        authorized.clone(),
                        cancel.clone(),
                        now,
                    )?,
                    None,
                )
            }
            Role::Server => {
                let Some(offer) = &view.client else {
                    return Ok(None);
                };
                if view.server.is_some() {
                    return Err(Error::RelaySessionLost);
                }
                (
                    PendingTls::server(
                        identity,
                        context.clone(),
                        authorized.clone(),
                        cancel.clone(),
                        &offer.signed,
                        now,
                    )?,
                    Some(offer.clone()),
                )
            }
        };
        let local = Declaration::local(pending.statement().clone())?;
        let message = if let Some(remote) = &remote {
            Message::open(remote, Some(&local))?
        } else {
            Message::open(&local, None)?
        };
        Ok(Some(Self {
            context,
            pending: Some(pending),
            local,
            remote,
            message,
            authorized,
            cancel,
            deadline: at + PROGRESS_TIMEOUT,
            ended: false,
        }))
    }
    pub(super) fn deadline(&self) -> Instant {
        self.deadline
    }
    fn check(&self, now: u64, at: Instant) -> Result<()> {
        if self.ended || self.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if !(self.authorized)() {
            return Err(Error::Unauthorized);
        }
        if at >= self.deadline {
            return Err(Error::Timeout);
        }
        self.local.signed.statement.validate_time(now)
    }
    /// Only the current identity may sign; callers account the borrowed body
    /// before copying it into the existing HTTP pool's queued Payload.
    pub(super) fn request(
        &mut self,
        identity: &DeviceIdentity,
        now: u64,
        at: Instant,
    ) -> Result<(SignedRequest, &[u8])> {
        let result = self
            .check(now, at)
            .and_then(|()| sign(identity, &self.context, Route::Open, &self.message.0, now));
        let request = self.end_on_error(result)?;
        Ok((request, &self.message.0))
    }
    fn end_on_error<T>(&mut self, result: Result<T>) -> Result<T> {
        if result.is_err() {
            self.ended = true;
            self.pending = None;
        }
        result
    }
    /// Decode only HTTP-success bodies here. The common bounded response
    /// classifier handles HTTP/platform/backpressure errors before this call.
    pub(super) fn response(
        &mut self,
        bytes: &[u8],
        now: u64,
        at: Instant,
    ) -> Result<Option<TlsStream>> {
        let result = self.response_inner(bytes, now, at);
        self.end_on_error(result)
    }
    fn response_inner(&mut self, bytes: &[u8], now: u64, at: Instant) -> Result<Option<TlsStream>> {
        self.check(now, at)?;
        compact_json(bytes, 1024)?;
        if self.context.role() == Role::Client {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Offered {
                boot: String,
                offered: bool,
            }
            let reply: Offered = serde_json::from_slice(bytes).map_err(|_| Error::Protocol)?;
            if reply.boot != self.context.boot {
                return Err(Error::RelaySessionLost);
            }
            if !reply.offered {
                return Err(Error::Protocol);
            }
            // A relay acceptance is neither endpoint progress nor TLS success.
            Ok(None)
        } else {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Opened {
                boot: String,
                session: String,
                closed: bool,
            }
            let reply: Opened = serde_json::from_slice(bytes).map_err(|_| Error::Protocol)?;
            if reply.boot != self.context.boot || reply.closed {
                return Err(Error::RelaySessionLost);
            }
            let remote = self.remote.as_ref().ok_or(Error::Protocol)?;
            let expected = Declarations {
                client: remote.signed.clone(),
                server: self.local.signed.clone(),
            }
            .session_id()?;
            if reply.session != expected {
                return Err(Error::Protocol);
            }
            self.finish(remote.signed.clone(), now).map(Some)
        }
    }
    /// The client learns the server statement/session through its pair's Poll.
    /// Empty reads do not reset deadline or recreate the one pending certificate.
    pub(super) fn observe(
        &mut self,
        boot: &str,
        wake: &str,
        view: &HandshakeView,
        now: u64,
        at: Instant,
    ) -> Result<Option<TlsStream>> {
        let result = self.observe_inner(boot, wake, view, now, at);
        self.end_on_error(result)
    }
    fn observe_inner(
        &mut self,
        boot: &str,
        wake: &str,
        view: &HandshakeView,
        now: u64,
        at: Instant,
    ) -> Result<Option<TlsStream>> {
        self.check(now, at)?;
        if boot != self.context.boot || wake != self.context.wake || view.closed {
            return Err(Error::RelaySessionLost);
        }
        if self.context.role() != Role::Client {
            return Err(Error::Protocol);
        }
        if let Some(client) = &view.client {
            if client.bytes() != self.local.bytes() {
                return Err(Error::Unauthorized);
            }
        }
        let Some(remote) = &view.server else {
            return Ok(None);
        };
        if !view.ready.client || !view.ready.server || view.client.is_none() {
            return Err(Error::Protocol);
        }
        // Verify the remote signature/binding before trusting the session hint.
        self.context
            .verify_remote(&remote.signed, Some(&self.local.signed.digest()?), now)?;
        let expected = Declarations {
            client: self.local.signed.clone(),
            server: remote.signed.clone(),
        }
        .session_id()?;
        if view.session.as_deref() != Some(expected.as_str()) {
            return Err(Error::Protocol);
        }
        self.finish(remote.signed.clone(), now).map(Some)
    }
    fn finish(&mut self, remote: super::SignedStatement, now: u64) -> Result<TlsStream> {
        let pending = self.pending.take().ok_or(Error::Cancelled)?;
        let tls = pending.accept(&remote, now)?;
        self.ended = true;
        // The returned stream still has to complete mutual TLS CertificateVerify.
        Ok(tls)
    }
}

fn sign(
    identity: &DeviceIdentity,
    context: &SessionContext,
    route: Route,
    bytes: &[u8],
    now: u64,
) -> Result<SignedRequest> {
    if identity.device_id().map_err(|_| Error::Unauthorized)? != context.peer.local_device_id {
        return Err(Error::Unauthorized);
    }
    SignedRequest::sign(
        identity,
        &context.peer.local_membership_id,
        Target::post(&context.origin, &context.peer.group_id, route)?,
        Fields::Boot {
            boot: &context.boot,
        },
        bytes,
        now,
    )
}

pub(super) fn ready(
    identity: &DeviceIdentity,
    context: &SessionContext,
    now: u64,
) -> Result<(SignedRequest, Vec<u8>)> {
    context.validate()?;
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Body<'a> {
        wake: &'a str,
        client_instance: &'a str,
        server_instance: &'a str,
    }
    let (client_instance, server_instance) = if context.role() == Role::Client {
        (&context.local_instance, &context.remote_instance)
    } else {
        (&context.remote_instance, &context.local_instance)
    };
    let bytes = serde_json::to_vec(&Body {
        wake: &context.wake,
        client_instance,
        server_instance,
    })
    .map_err(|_| Error::Protocol)?;
    let request = sign(identity, context, Route::Ready, &bytes, now)?;
    Ok((request, bytes))
}

#[cfg(test)]
mod tests;
