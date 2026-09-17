//! Coordinator metadata over the account's existing two HTTP workers.
//! Each call is explicit and finite. This module neither retries nor opens an
//! activity window; the coordinator owns those scheduling decisions.
use super::{
    announcement::{Intent, Scope},
    discovery::{Item, Page, Ready},
    handshake::{self, Handshake},
    http::{HttpClient, Reply},
    proof::{compact_json, Fields, Route, SignedRequest, Target},
    upload::Body,
    window::Payload,
    wire::{hash, token, MAX_SAFE_INTEGER},
    Error, Result, SessionContext,
};
use crate::bbs_sync::{
    control::Membership,
    raw_sha256,
    transport::{Cancellation, MembershipCheck, PeerIdentity, PROGRESS_TIMEOUT},
    DeviceIdentity,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::Instant};

// The existing control-plane membership limit, not a catalog/roster limit.
const MAX_MEMBERS: usize = 32;
const MAX_MUTATION: usize = 1024;

#[derive(Clone)]
pub(super) struct Client {
    http: HttpClient,
    identity: DeviceIdentity,
    membership: Membership,
    scope: Scope,
    local: String,
    cancel: Cancellation,
    authorized: MembershipCheck,
}

/// A retry keeps these exact proof/body bytes and the first absolute deadline.
/// Dropping the waiting future abandons its ticket; no blocking thread is added.
pub(super) struct Call {
    client: Client,
    signed: SignedRequest,
    body: Option<Payload>,
    deadline: Instant,
}
impl Call {
    pub(super) async fn execute(&self) -> Result<Reply> {
        self.client.check(self.deadline)?;
        let reply = self
            .client
            .http
            .execute(
                self.signed.clone(),
                self.body.clone().map(Body::from),
                self.client.cancel.clone(),
                self.client.authorized.clone(),
                self.deadline,
            )
            .await?;
        self.client.check(self.deadline)?;
        if reply.status != 200 {
            return Err(reply.error());
        }
        self.client.http.checked(reply.json())?;
        Ok(reply)
    }
}

pub(super) struct Snapshot {
    pub(super) boot: String,
    pub(super) items: BTreeMap<String, Item>,
}
impl Client {
    pub(super) fn new(
        http: HttpClient,
        identity: DeviceIdentity,
        membership: Membership,
        cancel: Cancellation,
        authorized: MembershipCheck,
    ) -> Result<Self> {
        let scope = Scope::new(&membership, &identity)?;
        let local = identity.device_id().map_err(|_| Error::Unauthorized)?;
        let value = Self {
            http,
            identity,
            membership,
            scope,
            local,
            cancel,
            authorized,
        };
        value.check(Instant::now() + PROGRESS_TIMEOUT)?;
        Ok(value)
    }
    fn check(&self, deadline: Instant) -> Result<()> {
        self.http.check_binding()?;
        if self.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if !(self.authorized)() {
            return Err(Error::Unauthorized);
        }
        if Instant::now() >= deadline {
            return Err(Error::Timeout);
        }
        Ok(())
    }
    fn peer(&self, peer: &PeerIdentity) -> Result<()> {
        if peer.group_id != self.membership.group_id
            || peer.local_device_id != self.local
            || peer.local_membership_id != self.membership.membership_id
        {
            return Err(Error::Unauthorized);
        }
        Ok(())
    }
    fn context(&self, context: &SessionContext) -> Result<()> {
        self.peer(&context.peer)?;
        if context.origin != self.membership.worker_url {
            return Err(Error::Unauthorized);
        }
        context.validate()
    }
    fn call(&self, signed: SignedRequest, bytes: &[u8], deadline: Instant) -> Result<Call> {
        let deadline = deadline.min(Instant::now() + PROGRESS_TIMEOUT);
        self.check(deadline)?;
        let body = if bytes.is_empty() {
            None
        } else {
            Some(self.http.metadata_body(bytes)?)
        };
        self.check(deadline)?;
        Ok(Call {
            client: self.clone(),
            signed,
            body,
            deadline,
        })
    }
    /// One whole group read, never one independent polling loop per peer.
    /// Partial pages remain private and all pages share one boot/deadline.
    pub(super) async fn poll(&self, now: u64, deadline: Instant) -> Result<Snapshot> {
        let started = Instant::now();
        let deadline = deadline.min(started + PROGRESS_TIMEOUT);
        self.check(deadline)?;
        let mut after: Option<String> = None;
        let mut boot: Option<String> = None;
        let mut items = BTreeMap::new();
        // Even one item per page has a finite bound, including a terminal page.
        for _ in 0..=MAX_MEMBERS {
            let time = now
                .checked_add(
                    started
                        .elapsed()
                        .as_millis()
                        .try_into()
                        .map_err(|_| Error::Runtime)?,
                )
                .ok_or(Error::Runtime)?;
            let signed = SignedRequest::sign(
                &self.identity,
                &self.membership.membership_id,
                Target::poll(
                    &self.membership.worker_url,
                    &self.membership.group_id,
                    after.as_deref(),
                )?,
                Fields::Read,
                &[],
                time,
            )?;
            let reply = self.call(signed, &[], deadline)?.execute().await?;
            let page = self.http.checked(Page::decode(
                reply.json()?,
                &self.membership.group_id,
                &self.local,
                after.as_deref(),
            ))?;
            drop(reply); // No HTTP payload allocation retained across pages.
            self.check(deadline)?;
            if boot.as_ref().is_some_and(|b| b != &page.boot) {
                return Err(Error::RelaySessionLost);
            }
            boot = Some(page.boot);
            if items.len() + page.items.len() > MAX_MEMBERS {
                return self.http.checked(Err(Error::Protocol));
            }
            for item in page.items {
                items.insert(item.device.clone(), item);
            }
            after = page.next;
            if after.is_none() {
                return Ok(Snapshot {
                    boot: boot.ok_or(Error::Protocol)?,
                    items,
                });
            }
        }
        self.http.checked(Err(Error::Protocol))
    }
    /// The caller must have durably prepared this intent before making a call,
    /// and completes it through Outbox only outside accepted content rounds.
    pub(super) fn announce(&self, intent: &Intent, now: u64, deadline: Instant) -> Result<Call> {
        if !intent.in_scope(&self.scope) {
            return Err(Error::Unauthorized);
        }
        let (signed, bytes) = intent.signed(&self.identity, now)?;
        self.call(signed, &bytes, deadline)
    }
    pub(super) fn wake(&self, intent: &WakeIntent, now: u64, deadline: Instant) -> Result<Call> {
        if intent.scope != self.scope {
            return Err(Error::Unauthorized);
        }
        let signed = SignedRequest::sign(
            &self.identity,
            &self.membership.membership_id,
            Target::post(
                &self.membership.worker_url,
                &self.membership.group_id,
                Route::Wake,
            )?,
            Fields::Mutation {
                nonce: &uuid::Uuid::new_v4().to_string(),
            },
            &intent.bytes,
            now,
        )?;
        self.call(signed, &intent.bytes, deadline)
    }
    pub(super) fn ready(
        &self,
        context: &SessionContext,
        now: u64,
        deadline: Instant,
    ) -> Result<Call> {
        self.context(context)?;
        let (signed, bytes) = handshake::ready(&self.identity, context, now)?;
        self.call(signed, &bytes, deadline)
    }
    pub(super) fn open(&self, handshake: &mut Handshake, now: u64, at: Instant) -> Result<Call> {
        self.context(handshake.context())?;
        let deadline = handshake.deadline();
        let (signed, bytes) = handshake.request(&self.identity, now, at)?;
        self.call(signed, bytes, deadline)
    }
}

/// Unlike announcements, wake intents expire with the active attempt. A stale
/// CAS result never changes this intent or silently starts another attempt.
pub(super) struct WakeIntent {
    scope: Scope,
    bytes: Vec<u8>,
    expected: String,
    previous: Option<String>,
}
pub(super) enum WakeResult {
    Accepted(String),
    Current(Option<String>),
}
impl WakeIntent {
    pub(super) fn new(
        identity: &DeviceIdentity,
        membership: &Membership,
        peer: &PeerIdentity,
        instance: &str,
        peer_instance: &str,
        previous: Option<&str>,
        now: u64,
    ) -> Result<Self> {
        let scope = Scope::new(membership, identity)?;
        if peer.group_id != membership.group_id
            || peer.local_membership_id != membership.membership_id
            || peer.local_device_id != identity.device_id().map_err(|_| Error::Unauthorized)?
            || peer.local_device_id == peer.remote_device_id
        {
            return Err(Error::Unauthorized);
        }
        hash(&peer.remote_device_id)?;
        token(instance)?;
        token(peer_instance)?;
        if let Some(previous) = previous {
            hash(previous)?;
        }
        if now > MAX_SAFE_INTEGER {
            return Err(Error::Protocol);
        }
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Body<'a> {
            request_id: &'a str,
            previous: Option<&'a str>,
            created_at: u64,
            instance: &'a str,
            peer: &'a str,
            peer_instance: &'a str,
        }
        let request_id = uuid::Uuid::new_v4().to_string();
        let bytes = serde_json::to_vec(&Body {
            request_id: &request_id,
            previous,
            created_at: now,
            instance,
            peer: &peer.remote_device_id,
            peer_instance,
        })
        .map_err(|_| Error::Protocol)?;
        compact_json(&bytes, MAX_MUTATION)?;
        let expected = raw_sha256(
            &serde_json::to_vec(&[
                "kota-bbs-relay.wake-id.v1",
                &membership.group_id,
                &peer.local_device_id,
                &membership.membership_id,
                &request_id,
            ])
            .map_err(|_| Error::Protocol)?,
        );
        Ok(Self {
            scope,
            bytes,
            expected,
            previous: previous.map(str::to_owned),
        })
    }
    pub(super) fn response(&self, bytes: &[u8]) -> Result<WakeResult> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Reply {
            ok: bool,
            #[serde(deserialize_with = "required_nullable")]
            current: Option<String>,
            changed: bool,
            // Absent on no replacement. Null is not a replacement ID either.
            #[serde(default)]
            replaced_wake: Option<String>,
        }
        compact_json(bytes, MAX_MUTATION)?;
        let reply: Reply = serde_json::from_slice(bytes).map_err(|_| Error::Protocol)?;
        for id in reply.current.iter().chain(reply.replaced_wake.iter()) {
            hash(id)?;
        }
        if reply.ok {
            if reply.current.as_deref() != Some(&self.expected)
                || reply
                    .replaced_wake
                    .as_ref()
                    .is_some_and(|id| Some(id) != self.previous.as_ref())
            {
                return Err(Error::Protocol);
            }
            Ok(WakeResult::Accepted(self.expected.clone()))
        } else {
            if reply.changed || reply.replaced_wake.is_some() {
                return Err(Error::Protocol);
            }
            Ok(WakeResult::Current(reply.current))
        }
    }
}
fn required_nullable<'de, T: Deserialize<'de>, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}

pub(super) fn ready_response(bytes: &[u8], context: &SessionContext) -> Result<Ready> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Reply {
        boot: String,
        client: bool,
        server: bool,
    }
    compact_json(bytes, MAX_MUTATION)?;
    let reply: Reply = serde_json::from_slice(bytes).map_err(|_| Error::Protocol)?;
    if reply.boot != context.boot {
        return Err(Error::RelaySessionLost);
    }
    let mine = if context.peer.local_device_id < context.peer.remote_device_id {
        reply.client
    } else {
        reply.server
    };
    if !mine {
        return Err(Error::Protocol);
    }
    Ok(Ready {
        client: reply.client,
        server: reply.server,
    })
}
