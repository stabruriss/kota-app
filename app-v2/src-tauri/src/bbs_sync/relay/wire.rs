//! The statement domain is shared with bbs_relay_session.ts. JSON is compact;
//! timestamps are bounded integers, serialized as decimal strings in proofs.
use super::{Error, Result, PEER_VERSION, RELAY_VERSION};
use crate::bbs_sync::{raw_sha256, transport::PeerIdentity, valid_hash, DeviceIdentity};
use serde::{Deserialize, Serialize};
use std::fmt;

pub(super) const DECLARATION_MS: u64 = 120_000;
pub(super) const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;
pub(super) const MAX_STATEMENT_BYTES: usize = 4 * 1024;

#[derive(Clone)]
pub(crate) struct SessionContext {
    pub(crate) peer: PeerIdentity,
    /// One canonical configured HTTPS origin for every member of this group.
    pub(crate) origin: String,
    pub(crate) wake: String,
    pub(crate) boot: String,
    pub(crate) nonce: String,
    pub(crate) local_instance: String,
    pub(crate) remote_instance: String,
}
impl SessionContext {
    pub(super) fn role(&self) -> Role {
        if self.peer.local_device_id < self.peer.remote_device_id {
            Role::Client
        } else {
            Role::Server
        }
    }
    pub(super) fn validate(&self) -> Result<()> {
        canonical_origin(&self.origin)?;
        for value in [
            &self.peer.group_id,
            &self.peer.local_membership_id,
            &self.peer.remote_membership_id,
            &self.wake,
            &self.boot,
            &self.nonce,
            &self.local_instance,
            &self.remote_instance,
        ] {
            token(value)?;
        }
        hash(&self.peer.local_device_id)?;
        hash(&self.peer.remote_device_id)?;
        if self.peer.local_device_id == self.peer.remote_device_id {
            return Err(Error::Unauthorized);
        }
        Ok(())
    }
    pub(super) fn verify_remote(
        &self,
        signed: &SignedStatement,
        reply_to: Option<&str>,
        now: u64,
    ) -> Result<()> {
        self.validate()?;
        let s = &signed.statement;
        s.validate_time(now)?;
        hash(&s.certificate_sha256)?;
        if s.relay_version != RELAY_VERSION || s.peer_version != PEER_VERSION {
            return Err(Error::ProtocolVersion);
        }
        if s.origin != self.origin
            || s.group != self.peer.group_id
            || s.from != self.peer.remote_device_id
            || s.to != self.peer.local_device_id
            || s.from_membership != self.peer.remote_membership_id
            || s.to_membership != self.peer.local_membership_id
            || s.from_instance != self.remote_instance
            || s.to_instance != self.local_instance
            || s.role == self.role()
            || s.wake != self.wake
            || s.boot != self.boot
            || s.nonce != self.nonce
            || s.reply_to.as_deref() != reply_to
        {
            return Err(Error::Unauthorized);
        }
        self.peer
            .verify_bytes(&s.signing_bytes()?, &signed.signature)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Role {
    Client,
    Server,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Statement {
    pub(super) relay_version: u32,
    pub(super) peer_version: u32,
    pub(super) origin: String,
    pub(super) group: String,
    pub(super) from: String,
    pub(super) to: String,
    pub(super) from_membership: String,
    pub(super) to_membership: String,
    pub(super) from_instance: String,
    pub(super) to_instance: String,
    pub(super) role: Role,
    pub(super) wake: String,
    pub(super) boot: String,
    pub(super) nonce: String,
    pub(super) issued_at: u64,
    pub(super) expires_at: u64,
    pub(super) certificate_sha256: String,
    #[serde(deserialize_with = "required_nullable")]
    pub(super) reply_to: Option<String>,
}

// `null` is part of the signed shape; an absent replyTo is not a second spelling.
fn required_nullable<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<String>, D::Error> {
    Option::<String>::deserialize(d)
}
impl Statement {
    pub(super) fn validate_time(&self, now: u64) -> Result<()> {
        if now > MAX_SAFE_INTEGER
            || self.expires_at > MAX_SAFE_INTEGER
            || self.expires_at <= self.issued_at
            || self.expires_at - self.issued_at > DECLARATION_MS
        {
            return Err(Error::InvalidSignal);
        }
        if self.issued_at > now || self.expires_at <= now {
            return Err(Error::StaleSignature);
        }
        Ok(())
    }
    pub(super) fn signing_bytes(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(&serde_json::json!([
            "kota-bbs-relay.tls.v1",
            self.relay_version,
            self.peer_version,
            self.origin,
            self.group,
            self.from,
            self.to,
            self.from_membership,
            self.to_membership,
            self.from_instance,
            self.to_instance,
            self.role,
            self.wake,
            self.boot,
            self.nonce,
            self.issued_at.to_string(),
            self.expires_at.to_string(),
            self.certificate_sha256,
            self.reply_to,
        ]))
        .map_err(|_| Error::InvalidSignal)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SignedStatement {
    pub(super) statement: Statement,
    pub(super) signature: String,
}
impl fmt::Debug for SignedStatement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignedStatement")
            .field("role", &self.statement.role)
            .field("statement", &"<redacted>")
            .finish()
    }
}
impl SignedStatement {
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_STATEMENT_BYTES {
            return Err(Error::InvalidSignal);
        }
        serde_json::from_slice(bytes).map_err(|_| Error::InvalidSignal)
    }
    pub(crate) fn encode(&self) -> Result<Vec<u8>> {
        let bytes = serde_json::to_vec(self).map_err(|_| Error::InvalidSignal)?;
        if bytes.len() > MAX_STATEMENT_BYTES {
            return Err(Error::InvalidSignal);
        }
        Ok(bytes)
    }
    pub(super) fn digest(&self) -> Result<String> {
        Ok(raw_sha256(&self.statement.signing_bytes()?))
    }
    pub(super) fn create(
        identity: &DeviceIdentity,
        context: &SessionContext,
        certificate_sha256: String,
        reply_to: Option<String>,
        now: u64,
    ) -> Result<Self> {
        context.validate()?;
        if identity.device_id().map_err(|_| Error::Unauthorized)? != context.peer.local_device_id {
            return Err(Error::Unauthorized);
        }
        hash(&certificate_sha256)?;
        let statement = Statement {
            relay_version: RELAY_VERSION,
            peer_version: PEER_VERSION,
            origin: context.origin.clone(),
            group: context.peer.group_id.clone(),
            from: context.peer.local_device_id.clone(),
            to: context.peer.remote_device_id.clone(),
            from_membership: context.peer.local_membership_id.clone(),
            to_membership: context.peer.remote_membership_id.clone(),
            from_instance: context.local_instance.clone(),
            to_instance: context.remote_instance.clone(),
            role: context.role(),
            wake: context.wake.clone(),
            boot: context.boot.clone(),
            nonce: context.nonce.clone(),
            issued_at: now,
            expires_at: now
                .checked_add(DECLARATION_MS)
                .ok_or(Error::InvalidSignal)?,
            certificate_sha256,
            reply_to,
        };
        statement.validate_time(now)?;
        let signature = identity
            .sign(&statement.signing_bytes()?)
            .map_err(|_| Error::Unauthorized)?;
        Ok(Self {
            statement,
            signature,
        })
    }
}

#[derive(Clone, Serialize)]
pub(crate) struct Declarations {
    pub(super) client: SignedStatement,
    pub(super) server: SignedStatement,
}
impl Declarations {
    pub(super) fn session_id(&self) -> Result<String> {
        let bytes = serde_json::to_vec(&[
            "kota-bbs-relay.session-id.v1",
            &self.client.digest()?,
            &self.server.digest()?,
        ])
        .map_err(|_| Error::InvalidSignal)?;
        Ok(raw_sha256(&bytes))
    }
}

pub(super) fn canonical_origin(value: &str) -> Result<()> {
    let u = url::Url::parse(value).map_err(|_| Error::InvalidSignal)?;
    if u.scheme() != "https"
        || u.origin().ascii_serialization() != value
        || !u.username().is_empty()
        || u.password().is_some()
    {
        return Err(Error::InvalidSignal);
    }
    Ok(())
}
pub(super) fn hash(value: &str) -> Result<()> {
    valid_hash(value).map_err(|_| Error::InvalidSignal)
}
pub(super) fn token(value: &str) -> Result<()> {
    if (16..=80).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        Ok(())
    } else {
        Err(Error::InvalidSignal)
    }
}
