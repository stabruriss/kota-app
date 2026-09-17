//! Bounded relay metadata, separate from endpoint authority. A valid HTTP page
//! only advertises a candidate; Handshake still verifies the peer declaration.
use super::{
    proof::{compact_json, CanonicalJson, MAX_JSON},
    wire::{hash, token, MAX_SAFE_INTEGER},
    Error, Result, SignedStatement,
};
use crate::bbs_sync::transport::PeerIdentity;
use serde::{Deserialize, Deserializer};

pub(super) const MAX_DECLARATION: usize = super::wire::MAX_STATEMENT_BYTES;

fn nullable<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
    input: D,
) -> std::result::Result<Option<T>, D::Error> {
    Option::<T>::deserialize(input)
}

/// Keep the exact JSON bytes: the dual Open must echo the immutable offered
/// client, not reorder its fields by deserializing through a sorted JSON map.
#[derive(Clone)]
pub(super) struct Declaration {
    pub(super) signed: SignedStatement,
    bytes: Vec<u8>,
}
impl Declaration {
    pub(super) fn local(signed: SignedStatement) -> Result<Self> {
        let bytes = signed.encode()?;
        if bytes.len() > MAX_DECLARATION {
            return Err(Error::Protocol);
        }
        Ok(Self { signed, bytes })
    }
    pub(super) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}
impl<'de> Deserialize<'de> for Declaration {
    fn deserialize<D: Deserializer<'de>>(input: D) -> std::result::Result<Self, D::Error> {
        use serde::de::Error as _;
        let bytes = CanonicalJson::deserialize(input)?.into_bytes();
        if bytes.len() > MAX_DECLARATION {
            return Err(D::Error::custom("oversize relay declaration"));
        }
        let signed = SignedStatement::decode(&bytes)
            .map_err(|_| D::Error::custom("invalid relay declaration"))?;
        Ok(Self { signed, bytes })
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Announcement {
    pub(super) device: String,
    pub(super) membership: String,
    pub(super) request_id: String,
    fingerprint: String,
    pub(super) instance: String,
    pub(super) revision: String,
    pub(super) peer_version: u32,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Wake {
    pub(super) id: String,
    group: String,
    client: String,
    server: String,
    client_membership: String,
    server_membership: String,
    client_instance: String,
    server_instance: String,
    created_at: u64,
    pub(super) expires_at: u64,
}
impl Wake {
    fn validate(&self, group: &str, local: &str, remote: &str) -> Result<()> {
        for t in [
            &self.id,
            &self.group,
            &self.client_membership,
            &self.server_membership,
            &self.client_instance,
            &self.server_instance,
        ] {
            token(t)?;
        }
        hash(&self.client)?;
        hash(&self.server)?;
        let (client, server) = if local < remote {
            (local, remote)
        } else {
            (remote, local)
        };
        if self.group != group
            || self.client != client
            || self.server != server
            || self.client >= self.server
            || self.expires_at <= self.created_at
            || self.expires_at > MAX_SAFE_INTEGER
            || self.expires_at - self.created_at > 120_000
        {
            return Err(Error::Protocol);
        }
        Ok(())
    }
    pub(super) fn context(
        &self,
        peer: PeerIdentity,
        origin: String,
        boot: String,
        nonce: String,
        local_instance: &str,
        remote_instance: &str,
        now: u64,
    ) -> Result<super::SessionContext> {
        self.validate(
            &peer.group_id,
            &peer.local_device_id,
            &peer.remote_device_id,
        )?;
        let client = peer.local_device_id < peer.remote_device_id;
        let (mine, theirs, mine_instance, their_instance) = if client {
            (
                &self.client_membership,
                &self.server_membership,
                &self.client_instance,
                &self.server_instance,
            )
        } else {
            (
                &self.server_membership,
                &self.client_membership,
                &self.server_instance,
                &self.client_instance,
            )
        };
        if mine != &peer.local_membership_id
            || theirs != &peer.remote_membership_id
            || mine_instance != local_instance
            || their_instance != remote_instance
        {
            return Err(Error::Unauthorized);
        }
        if now < self.created_at || now >= self.expires_at {
            return Err(Error::StaleSignature);
        }
        let context = super::SessionContext {
            peer,
            origin,
            boot,
            nonce,
            wake: self.id.clone(),
            local_instance: local_instance.into(),
            remote_instance: remote_instance.into(),
        };
        context.validate()?;
        Ok(context)
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Ready {
    pub(super) client: bool,
    pub(super) server: bool,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HandshakeView {
    pub(super) ready: Ready,
    #[serde(deserialize_with = "nullable")]
    pub(super) client: Option<Declaration>,
    #[serde(deserialize_with = "nullable")]
    pub(super) server: Option<Declaration>,
    #[serde(deserialize_with = "nullable")]
    pub(super) session: Option<String>,
    pub(super) closed: bool,
}
impl HandshakeView {
    fn validate(&self) -> Result<()> {
        if let Some(session) = &self.session {
            hash(session)?;
        }
        if self.server.is_some() && (self.client.is_none() || self.session.is_none())
            || self.closed
                && (self.session.is_none() || self.client.is_some() || self.server.is_some())
            || self.client.is_some() && (!self.ready.client || !self.ready.server)
        {
            return Err(Error::Protocol);
        }
        Ok(())
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Item {
    pub(super) device: String,
    #[serde(deserialize_with = "nullable")]
    pub(super) announcement: Option<Announcement>,
    #[serde(deserialize_with = "nullable")]
    pub(super) current_wake: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub(super) wake: Option<Wake>,
    #[serde(deserialize_with = "nullable")]
    pub(super) handshake: Option<HandshakeView>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Page {
    pub(super) boot: String,
    pub(super) items: Vec<Item>,
    #[serde(deserialize_with = "nullable")]
    pub(super) next: Option<String>,
}
impl Page {
    pub(super) fn decode(
        bytes: &[u8],
        group: &str,
        local: &str,
        after: Option<&str>,
    ) -> Result<Self> {
        compact_json(bytes, MAX_JSON)?;
        token(group)?;
        hash(local)?;
        if let Some(after) = after {
            hash(after)?;
        }
        let page: Self = serde_json::from_slice(bytes).map_err(|_| Error::Protocol)?;
        token(&page.boot)?;
        if page.items.len() > 16 {
            return Err(Error::Protocol);
        }
        let mut previous = after.unwrap_or("");
        for item in &page.items {
            hash(&item.device)?;
            if item.device.as_str() <= previous {
                return Err(Error::Protocol);
            }
            previous = &item.device;
            if let Some(a) = &item.announcement {
                if a.device != item.device {
                    return Err(Error::Protocol);
                }
                for t in [&a.membership, &a.request_id, &a.instance] {
                    token(t)?;
                }
                hash(&a.fingerprint)?;
                hash(&a.revision)?;
            }
            if let Some(id) = &item.current_wake {
                hash(id)?;
            }
            if let Some(w) = &item.wake {
                w.validate(group, local, &item.device)?;
                if item.current_wake.as_deref() != Some(&w.id) {
                    return Err(Error::Protocol);
                }
            }
            if let Some(h) = &item.handshake {
                if item.wake.is_none() {
                    return Err(Error::Protocol);
                }
                h.validate()?;
            } else if item.wake.is_some() {
                return Err(Error::Protocol);
            }
        }
        if let Some(next) = &page.next {
            hash(next)?;
            if page.items.last().map(|i| i.device.as_str()) != Some(next) {
                return Err(Error::Protocol);
            }
        }
        Ok(page)
    }
}

#[cfg(test)]
mod tests;
