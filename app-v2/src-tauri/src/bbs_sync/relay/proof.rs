//! HTTP proofs and recipient-authenticated receipts. No IO and no implicit
//! nonce storage; the HTTPS owner fences the current group/epoch before use.
use super::{
    upload::{Body, Upload},
    wire::{canonical_origin, hash, token, Role, MAX_SAFE_INTEGER},
    Error, Result, SessionContext, MAX_BATCH,
};
use crate::bbs_sync::{raw_sha256, transport::MembershipCheck, DeviceIdentity};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub(super) const MAX_JSON: usize = 64 * 1024;
const MAX_HEADERS: usize = 8 * 1024;
const AUTH_WINDOW_MS: u64 = 300_000;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Route {
    Poll,
    Receive,
    Announce,
    Wake,
    Ready,
    Open,
    Send,
    Ack,
}
impl Route {
    fn name(self) -> &'static str {
        match self {
            Self::Poll => "poll",
            Self::Receive => "receive",
            Self::Announce => "announce",
            Self::Wake => "wake",
            Self::Ready => "ready",
            Self::Open => "open",
            Self::Send => "send",
            Self::Ack => "ack",
        }
    }
    fn method(self) -> &'static str {
        if matches!(self, Self::Poll | Self::Receive) {
            "GET"
        } else {
            "POST"
        }
    }
    fn domain(self) -> &'static str {
        match self {
            Self::Poll | Self::Receive => "kota-bbs-relay.read.v1",
            Self::Announce | Self::Wake => "kota-bbs-relay.mutation.v1",
            Self::Ready | Self::Open => "kota-bbs-relay.boot.v1",
            Self::Send | Self::Ack => "kota-bbs-relay.frame.v1",
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Direction {
    ClientToServer,
    ServerToClient,
}
impl Direction {
    fn text(self) -> &'static str {
        match self {
            Self::ClientToServer => "c2s",
            Self::ServerToClient => "s2c",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReceiveMode {
    Data,
    Probe,
    Receipts,
}
impl ReceiveMode {
    fn text(self) -> &'static str {
        match self {
            Self::Data => "data",
            Self::Probe => "probe",
            Self::Receipts => "receipts",
        }
    }
}

#[derive(Clone)]
pub(super) struct Target {
    origin: String,
    group: String,
    route: Route,
    path: String,
    receive: Option<ReceiveRequest>,
}
#[derive(Clone)]
pub(super) struct ReceiveRequest {
    pub(super) boot: String,
    pub(super) mode: ReceiveMode,
    pub(super) cursors: Vec<(String, u64)>,
}
impl Target {
    fn new(origin: &str, group: &str, route: Route) -> Result<Self> {
        canonical_origin(origin)?;
        token(group)?;
        Ok(Self {
            origin: origin.into(),
            group: group.into(),
            route,
            path: format!("/bbs/relay/{}?group={group}", route.name()),
            receive: None,
        })
    }
    pub(super) fn poll(origin: &str, group: &str, after: Option<&str>) -> Result<Self> {
        let mut target = Self::new(origin, group, Route::Poll)?;
        if let Some(after) = after {
            hash(after)?;
            target.path.push_str(&format!("&after={after}"));
        }
        Ok(target)
    }
    pub(super) fn receive(
        origin: &str,
        group: &str,
        boot: &str,
        mode: ReceiveMode,
        cursors: &[(String, u64)],
    ) -> Result<Self> {
        token(boot)?;
        if cursors.is_empty() || cursors.len() > 4 {
            return Err(Error::Protocol);
        }
        let mut target = Self::new(origin, group, Route::Receive)?;
        target.receive = Some(ReceiveRequest {
            boot: boot.into(),
            mode,
            cursors: cursors.to_vec(),
        });
        target
            .path
            .push_str(&format!("&boot={boot}&mode={}&cursors=", mode.text()));
        let mut previous: Option<&str> = None;
        for (session, next) in cursors {
            hash(session)?;
            integer(*next)?;
            if previous.is_some_and(|p| p >= session.as_str()) {
                return Err(Error::Protocol);
            }
            if previous.is_some() {
                target.path.push(',');
            }
            target.path.push_str(&format!("{session}.{next}"));
            previous = Some(session);
        }
        Ok(target)
    }
    pub(super) fn post(origin: &str, group: &str, route: Route) -> Result<Self> {
        if route.method() != "POST" {
            return Err(Error::Protocol);
        }
        Self::new(origin, group, route)
    }
}

pub(super) enum Fields<'a> {
    Read,
    Mutation {
        nonce: &'a str,
    },
    Boot {
        boot: &'a str,
    },
    Frame {
        boot: &'a str,
        session: &'a str,
        direction: Direction,
        sequence: u64,
        final_flag: bool,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Proof {
    device: String,
    membership: String,
    time: u64,
    nonce: String,
    boot: String,
    session: String,
    direction: String,
    sequence: u64,
    #[serde(rename = "final")]
    final_flag: bool,
    signature: String,
}
impl Proof {
    fn message(&self, target: &Target, bytes: &[u8]) -> Result<Vec<u8>> {
        self.message_digest(target, &raw_sha256(bytes))
    }
    fn message_digest(&self, target: &Target, body_hash: &str) -> Result<Vec<u8>> {
        serde_json::to_vec(&serde_json::json!([
            target.route.domain(),
            target.origin,
            target.route.method(),
            target.path,
            target.group,
            self.device,
            self.membership,
            self.time.to_string(),
            self.nonce,
            self.boot,
            self.session,
            self.direction,
            self.sequence.to_string(),
            self.final_flag,
            body_hash,
        ]))
        .map_err(|_| Error::Protocol)
    }
}

#[derive(Clone)]
pub(super) struct SignedRequest {
    target: Target,
    proof: Proof,
    body_hash: String,
}
impl SignedRequest {
    /// `bytes` remain owned/charged by the caller. The proof hashes exactly
    /// those bytes, so retransmission never reserializes or reencrypts a batch.
    pub(super) fn sign(
        identity: &DeviceIdentity,
        membership: &str,
        target: Target,
        fields: Fields<'_>,
        bytes: &[u8],
        now: u64,
    ) -> Result<Self> {
        if target.route == Route::Send {
            if bytes.is_empty() || bytes.len() > MAX_BATCH {
                return Err(Error::Protocol);
            }
        } else if target.route.method() == "POST" {
            compact_json(bytes, MAX_JSON)?;
        } else if !bytes.is_empty() {
            return Err(Error::Protocol);
        }
        Self::sign_digest(identity, membership, target, fields, raw_sha256(bytes), now)
    }
    pub(super) fn sign_upload(
        identity: &DeviceIdentity,
        membership: &str,
        target: Target,
        fields: Fields<'_>,
        body: &Upload,
        now: u64,
    ) -> Result<Self> {
        if target.route != Route::Send || body.len() == 0 || body.len() > MAX_BATCH {
            return Err(Error::Protocol);
        }
        Self::sign_digest(
            identity,
            membership,
            target,
            fields,
            body.digest().into(),
            now,
        )
    }
    fn sign_digest(
        identity: &DeviceIdentity,
        membership: &str,
        target: Target,
        fields: Fields<'_>,
        body_hash: String,
        now: u64,
    ) -> Result<Self> {
        token(membership)?;
        integer(now)?;
        let mut proof = Proof {
            device: identity.device_id().map_err(|_| Error::Unauthorized)?,
            membership: membership.into(),
            time: now,
            nonce: String::new(),
            boot: String::new(),
            session: String::new(),
            direction: String::new(),
            sequence: 0,
            final_flag: false,
            signature: String::new(),
        };
        match (target.route, fields) {
            (Route::Poll | Route::Receive, Fields::Read) => {}
            (Route::Announce | Route::Wake, Fields::Mutation { nonce }) => {
                token(nonce)?;
                proof.nonce = nonce.into();
            }
            (Route::Ready | Route::Open, Fields::Boot { boot }) => {
                token(boot)?;
                proof.boot = boot.into();
            }
            (
                Route::Send | Route::Ack,
                Fields::Frame {
                    boot,
                    session,
                    direction,
                    sequence,
                    final_flag,
                },
            ) => {
                token(boot)?;
                hash(session)?;
                integer(sequence)?;
                if final_flag && target.route != Route::Ack {
                    return Err(Error::Protocol);
                }
                proof.boot = boot.into();
                proof.session = session.into();
                proof.direction = direction.text().into();
                proof.sequence = sequence;
                proof.final_flag = final_flag;
            }
            _ => return Err(Error::Protocol),
        }
        proof.signature = identity
            .sign(&proof.message_digest(&target, &body_hash)?)
            .map_err(|_| Error::Unauthorized)?;
        Ok(Self {
            target,
            proof,
            body_hash,
        })
    }
    pub(super) fn matches_body(&self, bytes: &[u8]) -> bool {
        self.body_hash == raw_sha256(bytes)
    }
    pub(super) fn matches_upload(&self, body: Option<&Body>) -> bool {
        match body {
            Some(Body::Ciphertext(_)) if self.target.route != Route::Send => false,
            Some(body) => self.body_hash == body.digest(),
            None => self.matches_body(&[]),
        }
    }
    pub(super) fn method(&self) -> &'static str {
        self.target.route.method()
    }
    pub(super) fn is_send(&self) -> bool {
        self.target.route == Route::Send
    }
    pub(super) fn small_request(&self) -> bool {
        self.target.route == Route::Ack
            || matches!(
                self.target.receive.as_ref().map(|r| r.mode),
                Some(ReceiveMode::Probe | ReceiveMode::Receipts)
            )
    }
    pub(super) fn receive_request(&self) -> Option<&ReceiveRequest> {
        self.target.receive.as_ref()
    }
    pub(super) fn response_limit(&self) -> usize {
        if self.small_request() {
            super::MAX_ENVELOPE
        } else if self.target.route == Route::Receive {
            MAX_BATCH + super::MAX_ENVELOPE
        } else {
            MAX_JSON
        }
    }
    pub(super) fn content_type(&self) -> &'static str {
        if self.target.route == Route::Send {
            "application/octet-stream"
        } else {
            "application/json"
        }
    }
    pub(super) fn url(&self) -> String {
        format!("{}{}", self.target.origin, self.target.path)
    }
    pub(super) fn headers(&self) -> Result<BTreeMap<String, String>> {
        let p = &self.proof;
        let mut headers = BTreeMap::from([
            ("version", "1".into()),
            ("device", p.device.clone()),
            ("membership", p.membership.clone()),
            ("time", p.time.to_string()),
            ("signature", p.signature.clone()),
        ]);
        if !p.nonce.is_empty() {
            headers.insert("nonce", p.nonce.clone());
        }
        if !p.boot.is_empty() {
            headers.insert("boot", p.boot.clone());
        }
        if !p.session.is_empty() {
            headers.insert("session", p.session.clone());
            headers.insert("direction", p.direction.clone());
            headers.insert("sequence", p.sequence.to_string());
            headers.insert("final", if p.final_flag { "1" } else { "0" }.into());
        }
        let headers: BTreeMap<String, String> = headers
            .into_iter()
            .map(|(k, v)| (format!("x-kota-relay-{k}"), v))
            .collect();
        // Leave room for Host/Content-Type/Content-Length in the HTTP adapter.
        if headers
            .iter()
            .map(|(k, v)| k.len() + v.len() + 4)
            .sum::<usize>()
            > MAX_HEADERS - 1024
        {
            return Err(Error::Protocol);
        }
        Ok(headers)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SignedAck {
    proof: Proof,
    body: String,
}
#[derive(Debug, PartialEq, Eq)]
pub(super) struct VerifiedAck {
    pub(super) through: u64,
    pub(super) sequence: u64,
    pub(super) final_flag: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AckBody {
    through: String,
    #[serde(rename = "final")]
    final_flag: bool,
}
impl SignedAck {
    /// Structural bounds only. Never frees credit or substitutes for verify().
    pub(super) fn envelope_shape(&self, boot: &str, session: &str) -> Result<()> {
        use base64::Engine as _;
        let p = &self.proof;
        hash(&p.device)?;
        token(&p.membership)?;
        integer(p.time)?;
        integer(p.sequence)?;
        if p.boot != boot
            || p.session != session
            || !p.nonce.is_empty()
            || !matches!(p.direction.as_str(), "c2s" | "s2c")
        {
            return Err(Error::Protocol);
        }
        let engine = base64::engine::general_purpose::STANDARD;
        let signature = engine.decode(&p.signature).map_err(|_| Error::Protocol)?;
        if signature.len() != 64 || engine.encode(signature) != p.signature {
            return Err(Error::Protocol);
        }
        compact_json(self.body.as_bytes(), 1024)?;
        let body: AckBody = serde_json::from_str(&self.body).map_err(|_| Error::Protocol)?;
        decimal(&body.through)?;
        if body.final_flag != p.final_flag {
            return Err(Error::Protocol);
        }
        Ok(())
    }
    pub(super) fn body_hash(&self) -> String {
        raw_sha256(self.body.as_bytes())
    }
    pub(super) fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_HEADERS {
            return Err(Error::Protocol);
        }
        serde_json::from_slice(bytes).map_err(|_| Error::Protocol)
    }
    /// Only the recipient's signature can free retained ciphertext. Current
    /// membership/work epoch is checked separately from the frozen statement.
    pub(super) fn verify(
        &self,
        context: &SessionContext,
        session: &str,
        sent: u64,
        now: u64,
        authorized: &MembershipCheck,
    ) -> Result<VerifiedAck> {
        if !authorized() {
            return Err(Error::Unauthorized);
        }
        context.validate()?;
        hash(session)?;
        integer(sent)?;
        integer(now)?;
        let p = &self.proof;
        integer(p.time)?;
        integer(p.sequence)?;
        let direction = if context.role() == Role::Client {
            Direction::ClientToServer
        } else {
            Direction::ServerToClient
        };
        if p.device != context.peer.remote_device_id
            || p.membership != context.peer.remote_membership_id
            || p.boot != context.boot
            || p.session != session
            || p.direction != direction.text()
            || !p.nonce.is_empty()
        {
            return Err(Error::Unauthorized);
        }
        if p.time.abs_diff(now) > AUTH_WINDOW_MS {
            return Err(Error::StaleSignature);
        }
        compact_json(self.body.as_bytes(), 1024)?;
        let target = Target::post(&context.origin, &context.peer.group_id, Route::Ack)?;
        context
            .peer
            .verify_bytes(&p.message(&target, self.body.as_bytes())?, &p.signature)?;
        let body: AckBody = serde_json::from_str(&self.body).map_err(|_| Error::Protocol)?;
        let through = decimal(&body.through)?;
        if body.final_flag != p.final_flag || through > sent {
            return Err(Error::Protocol);
        }
        if !authorized() {
            return Err(Error::Unauthorized);
        }
        Ok(VerifiedAck {
            through,
            sequence: p.sequence,
            final_flag: p.final_flag,
        })
    }
}
fn integer(n: u64) -> Result<()> {
    if n <= MAX_SAFE_INTEGER {
        Ok(())
    } else {
        Err(Error::Protocol)
    }
}
pub(super) fn decimal(text: &str) -> Result<u64> {
    if text.is_empty()
        || (text.len() > 1 && text.starts_with('0'))
        || !text.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(Error::Protocol);
    }
    let n = text.parse().map_err(|_| Error::Protocol)?;
    integer(n)?;
    Ok(n)
}
pub(super) fn compact_json(bytes: &[u8], max: usize) -> Result<()> {
    if bytes.len() > max {
        return Err(Error::Protocol);
    }
    let canonical: CanonicalJson = serde_json::from_slice(bytes).map_err(|_| Error::Protocol)?;
    if canonical.0 == bytes {
        Ok(())
    } else {
        Err(Error::Protocol)
    }
}

// serde_json::Value uses a sorted map in this lockfile. Re-encoding it would
// change valid signed JS object bytes. This bounded validator preserves string
// key insertion order, with JS's integer-index ordering, without changing a
// globally unified serde_json feature or accepting duplicate object keys.
pub(super) struct CanonicalJson(Vec<u8>);
impl CanonicalJson {
    pub(super) fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}
impl<'de> Deserialize<'de> for CanonicalJson {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        use serde::de::{Error as _, MapAccess, SeqAccess, Visitor};
        struct JsonVisitor;
        impl<'de> Visitor<'de> for JsonVisitor {
            type Value = CanonicalJson;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("canonical relay JSON")
            }
            fn visit_u64<E: serde::de::Error>(self, n: u64) -> std::result::Result<Self::Value, E> {
                integer(n).map_err(|_| E::custom("noncanonical number"))?;
                Ok(CanonicalJson(n.to_string().into_bytes()))
            }
            fn visit_i64<E: serde::de::Error>(self, n: i64) -> std::result::Result<Self::Value, E> {
                self.visit_u64(n.try_into().map_err(|_| E::custom("negative number"))?)
            }
            fn visit_bool<E: serde::de::Error>(
                self,
                b: bool,
            ) -> std::result::Result<Self::Value, E> {
                Ok(CanonicalJson(if b {
                    b"true".to_vec()
                } else {
                    b"false".to_vec()
                }))
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(CanonicalJson(b"null".to_vec()))
            }
            fn visit_str<E: serde::de::Error>(
                self,
                s: &str,
            ) -> std::result::Result<Self::Value, E> {
                Ok(CanonicalJson(serde_json::to_vec(s).map_err(E::custom)?))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut bytes = vec![b'['];
                while let Some(value) = a.next_element::<CanonicalJson>()? {
                    if bytes.len() > 1 {
                        bytes.push(b',');
                    }
                    bytes.extend(value.0);
                }
                bytes.push(b']');
                Ok(CanonicalJson(bytes))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut entries = Vec::new();
                let mut keys = std::collections::BTreeSet::new();
                while let Some((key, value)) = a.next_entry::<String, CanonicalJson>()? {
                    if !keys.insert(key.clone()) {
                        return Err(A::Error::custom("duplicate key"));
                    }
                    entries.push((key, value));
                }
                // JSON.stringify enumerates array-index keys before other keys.
                // Stable sort preserves insertion order among all string keys.
                entries.sort_by_key(|(k, _)| {
                    k.parse::<u32>()
                        .ok()
                        .filter(|n| *n != u32::MAX && n.to_string() == *k)
                        .unwrap_or(u32::MAX)
                });
                let mut bytes = vec![b'{'];
                for (key, value) in entries {
                    if bytes.len() > 1 {
                        bytes.push(b',');
                    }
                    bytes.extend(serde_json::to_vec(&key).map_err(A::Error::custom)?);
                    bytes.push(b':');
                    bytes.extend(value.0);
                }
                bytes.push(b'}');
                Ok(CanonicalJson(bytes))
            }
        }
        // Floats/exponents have no visitor: wire metadata uses safe unsigned
        // integers and canonical decimal strings, never floating-point values.
        d.deserialize_any(JsonVisitor)
    }
}

#[cfg(test)]
mod tests;
