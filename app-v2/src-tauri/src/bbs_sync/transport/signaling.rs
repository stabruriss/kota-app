//! Signed, session-only signaling. No manifest/content or persistent ICE state.
use super::{Error, Result};
use crate::bbs_sync::{
    control::{Member, Membership},
    raw_sha256, safe_id, DeviceIdentity,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use ring::signature::{UnparsedPublicKey, ED25519};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fmt,
    net::{IpAddr, SocketAddr},
};
use webrtc::peer_connection::{RTCSdpType, RTCSessionDescription};

const DOMAIN: &str = "kota-bbs-session-v1";
const MAX_SIGNAL: usize = 64 * 1024;
const MAX_SDP: usize = 48 * 1024;
const MAX_AGE_MS: u64 = 120_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PeerIdentity {
    pub(crate) group_id: String,
    pub(crate) local_device_id: String,
    pub(crate) remote_device_id: String,
    pub(crate) local_membership_id: String,
    pub(crate) remote_membership_id: String,
    remote_public_key: String,
}
impl PeerIdentity {
    pub(crate) fn verify_bytes(&self, bytes: &[u8], signature: &str) -> Result<()> {
        let key = BASE64
            .decode(&self.remote_public_key)
            .map_err(|_| Error::Unauthorized)?;
        let signature_bytes = BASE64.decode(signature).map_err(|_| Error::Unauthorized)?;
        if signature_bytes.len() != 64 || BASE64.encode(&signature_bytes) != signature {
            return Err(Error::Unauthorized);
        }
        UnparsedPublicKey::new(&ED25519, key)
            .verify(bytes, &signature_bytes)
            .map_err(|_| Error::Unauthorized)
    }
    /// The caller supplies the current authenticated Worker membership snapshot.
    /// Session code checks it again through the live membership callback.
    pub(crate) fn current(
        group: &Membership,
        members: &[Member],
        own: &DeviceIdentity,
        remote: &str,
    ) -> Result<Self> {
        let local = own.device_id().map_err(|_| Error::Unauthorized)?;
        if remote == local {
            return Err(Error::Unauthorized);
        }
        let me = members
            .iter()
            .find(|m| m.device_id == local)
            .ok_or(Error::Unauthorized)?;
        let peer = members
            .iter()
            .find(|m| m.device_id == remote)
            .ok_or(Error::Unauthorized)?;
        if me.public_key != own.public_key || me.membership_id != group.membership_id {
            return Err(Error::Unauthorized);
        }
        for member in [me, peer] {
            let key = BASE64
                .decode(&member.public_key)
                .map_err(|_| Error::Unauthorized)?;
            if key.len() != 32
                || BASE64.encode(&key) != member.public_key
                || raw_sha256(&key) != member.device_id
            {
                return Err(Error::Unauthorized);
            }
            safe_id(&member.membership_id).map_err(|_| Error::Unauthorized)?;
        }
        safe_id(&group.group_id).map_err(|_| Error::Unauthorized)?;
        Ok(Self {
            group_id: group.group_id.clone(),
            local_device_id: local,
            remote_device_id: remote.to_owned(),
            local_membership_id: me.membership_id.clone(),
            remote_membership_id: peer.membership_id.clone(),
            remote_public_key: peer.public_key.clone(),
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SessionRole {
    Offer,
    Answer,
}
impl SessionRole {
    fn matches(self, description: &RTCSessionDescription) -> bool {
        matches!(
            (self, description.sdp_type),
            (Self::Offer, RTCSdpType::Offer) | (Self::Answer, RTCSdpType::Answer)
        )
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Body {
    protocol_version: u32,
    group_id: String,
    from: String,
    to: String,
    from_membership: String,
    to_membership: String,
    role: SessionRole,
    nonce: String,
    issued_at: u64,
    expires_at: u64,
    fingerprint: String,
    sdp: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SignedDescription {
    body: Body,
    signature: String,
}
impl fmt::Debug for SignedDescription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignedDescription")
            .field("role", &self.body.role)
            .field("description", &"<redacted>")
            .finish()
    }
}
impl SignedDescription {
    pub(crate) fn create(
        identity: &DeviceIdentity,
        peer: &PeerIdentity,
        role: SessionRole,
        nonce: &str,
        description: &RTCSessionDescription,
        now_ms: u64,
    ) -> Result<Self> {
        if !role.matches(description)
            || identity.device_id().map_err(|_| Error::Unauthorized)? != peer.local_device_id
        {
            return Err(Error::Unauthorized);
        }
        validate_nonce(nonce)?;
        let parsed = inspect_sdp(&description.sdp)?;
        let body = Body {
            protocol_version: 1,
            group_id: peer.group_id.clone(),
            from: peer.local_device_id.clone(),
            to: peer.remote_device_id.clone(),
            from_membership: peer.local_membership_id.clone(),
            to_membership: peer.remote_membership_id.clone(),
            role,
            nonce: nonce.into(),
            issued_at: now_ms,
            expires_at: now_ms.checked_add(MAX_AGE_MS).ok_or(Error::InvalidSignal)?,
            fingerprint: parsed.fingerprint,
            sdp: description.sdp.clone(),
        };
        let signature = identity
            .sign(&signed_bytes(&body)?)
            .map_err(|_| Error::Unauthorized)?;
        Ok(Self { body, signature })
    }
    pub(crate) fn encode(&self) -> Result<String> {
        let wire = serde_json::to_string(self).map_err(|_| Error::InvalidSignal)?;
        if wire.len() > MAX_SIGNAL {
            return Err(Error::InvalidSignal);
        }
        Ok(wire)
    }
    pub(crate) fn decode(wire: &str) -> Result<Self> {
        if wire.len() > MAX_SIGNAL {
            return Err(Error::InvalidSignal);
        }
        serde_json::from_str(wire).map_err(|_| Error::InvalidSignal)
    }
    pub(crate) fn nonce(&self) -> &str {
        &self.body.nonce
    }
    /// Must be called before set_remote_description, with the expected pending
    /// nonce/role and a freshly authorized peer. A valid signature is not membership.
    pub(crate) fn verify(
        &self,
        peer: &PeerIdentity,
        role: SessionRole,
        nonce: &str,
        now_ms: u64,
    ) -> Result<RTCSessionDescription> {
        let b = &self.body;
        validate_nonce(&b.nonce)?;
        if b.protocol_version != 1
            || b.group_id != peer.group_id
            || b.from != peer.remote_device_id
            || b.to != peer.local_device_id
            || b.from_membership != peer.remote_membership_id
            || b.to_membership != peer.local_membership_id
            || b.role != role
            || b.nonce != nonce
        {
            return Err(Error::Unauthorized);
        }
        if b.expires_at <= b.issued_at || b.expires_at - b.issued_at > MAX_AGE_MS {
            return Err(Error::InvalidSignal);
        }
        if now_ms >= b.expires_at || b.issued_at > now_ms.saturating_add(300_000) {
            return Err(Error::StaleSignature);
        }
        let public = BASE64
            .decode(&peer.remote_public_key)
            .map_err(|_| Error::Unauthorized)?;
        let sig = BASE64
            .decode(&self.signature)
            .map_err(|_| Error::Unauthorized)?;
        if BASE64.encode(&sig) != self.signature {
            return Err(Error::Unauthorized);
        }
        UnparsedPublicKey::new(&ED25519, public)
            .verify(&signed_bytes(b)?, &sig)
            .map_err(|_| Error::Unauthorized)?;
        let parsed = inspect_sdp(&b.sdp)?;
        if parsed.fingerprint != b.fingerprint {
            return Err(Error::Unauthorized);
        }
        let value = serde_json::json!({"type": b.role, "sdp": b.sdp});
        serde_json::from_value(value).map_err(|_| Error::InvalidSignal)
    }
}
fn signed_bytes(body: &Body) -> Result<Vec<u8>> {
    serde_json::to_vec(&(DOMAIN, body)).map_err(|_| Error::InvalidSignal)
}
pub(crate) fn validate_nonce(value: &str) -> Result<()> {
    if value.len() != 32
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::InvalidSignal);
    }
    Ok(())
}

pub(crate) struct DescriptionInfo {
    pub(crate) fingerprint: String,
    pub(crate) candidates: BTreeSet<SocketAddr>,
    pub(crate) candidate_types: std::collections::BTreeMap<SocketAddr, Option<String>>,
}
pub(crate) fn inspect_sdp(sdp: &str) -> Result<DescriptionInfo> {
    if sdp.len() > MAX_SDP || sdp.contains('\0') {
        return Err(Error::InvalidSignal);
    }
    let mut fingerprint: Option<String> = None;
    let mut candidates = BTreeSet::new();
    let mut candidate_types = std::collections::BTreeMap::<SocketAddr, Option<String>>::new();
    let mut component_two = BTreeSet::new();
    let mut media = 0;
    let mut candidate_lines = 0;
    for line in sdp.lines() {
        if line.starts_with("m=") {
            let f: Vec<_> = line.split_ascii_whitespace().collect();
            if f.len() != 4
                || f[0] != "m=application"
                || f[2] != "UDP/DTLS/SCTP"
                || f[3] != "webrtc-datachannel"
            {
                return Err(Error::InvalidSignal);
            }
            media += 1;
        }
        if let Some(value) = line.strip_prefix("a=fingerprint:") {
            let value = value.strip_prefix("sha-256 ").ok_or(Error::InvalidSignal)?;
            let parts: Vec<_> = value.split(':').collect();
            if parts.len() != 32
                || parts
                    .iter()
                    .any(|p| p.len() != 2 || !p.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err(Error::InvalidSignal);
            }
            let next = parts.join(":").to_ascii_lowercase();
            if fingerprint.as_ref().is_some_and(|old| old != &next) {
                return Err(Error::InvalidSignal);
            }
            fingerprint = Some(next);
        }
        if let Some(value) = line.strip_prefix("a=candidate:") {
            candidate_lines += 1;
            if candidate_lines > 32 {
                return Err(Error::InvalidSignal);
            }
            let f: Vec<_> = value.split_ascii_whitespace().collect();
            if f.len() < 8
                || !matches!(f[1], "1" | "2")
                || !f[2].eq_ignore_ascii_case("udp")
                || f[6] != "typ"
                || !matches!(f[7], "host" | "srflx")
            {
                return Err(Error::InvalidSignal);
            }
            let ip: IpAddr = f[4].parse().map_err(|_| Error::InvalidSignal)?;
            let port: u16 = f[5].parse().map_err(|_| Error::InvalidSignal)?;
            if port == 0 || ip.is_unspecified() || ip.is_multicast() {
                return Err(Error::InvalidSignal);
            }
            if f[8..].len() % 2 != 0 {
                return Err(Error::InvalidSignal);
            }
            for pair in f[8..].chunks_exact(2) {
                match pair[0] {
                    "raddr" => {
                        pair[1]
                            .parse::<IpAddr>()
                            .map_err(|_| Error::InvalidSignal)?;
                    }
                    "rport" => {
                        pair[1].parse::<u16>().map_err(|_| Error::InvalidSignal)?;
                    }
                    "generation" | "network-id" | "network-cost" => {
                        pair[1].parse::<u32>().map_err(|_| Error::InvalidSignal)?;
                    }
                    "ufrag"
                        if pair[1].len() <= 256
                            && pair[1]
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b"+/".contains(&b)) => {}
                    _ => return Err(Error::InvalidSignal),
                }
            }
            let address = SocketAddr::new(ip, port);
            if f[1] == "1" {
                candidates.insert(address);
                candidate_types
                    .entry(address)
                    .and_modify(|kind| {
                        if kind.as_deref() != Some(f[7]) {
                            *kind = None;
                        }
                    })
                    .or_insert_with(|| Some(f[7].to_string()));
            } else {
                component_two.insert(address);
            }
        }
        if line.starts_with("a=ice-server:") || line.starts_with("a=remote-candidates:") {
            return Err(Error::InvalidSignal);
        }
    }
    // rtc 0.20.5 add_candidates_to_media_descriptions emits the same candidate
    // twice (components 1 and 2), including data-only SDP. Component 2 must be
    // an exact address duplicate, so it never authorizes another destination.
    if media != 1 || candidates.is_empty() || !component_two.is_subset(&candidates) {
        return Err(Error::InvalidSignal);
    }
    Ok(DescriptionInfo {
        fingerprint: fingerprint.ok_or(Error::InvalidSignal)?,
        candidates,
        candidate_types,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bbs_sync::control::Role;
    fn identities() -> (DeviceIdentity, DeviceIdentity, PeerIdentity, PeerIdentity) {
        let a = DeviceIdentity::from_seed([1; 32]).unwrap();
        let b = DeviceIdentity::from_seed([2; 32]).unwrap();
        let members: Vec<_> = [&a, &b]
            .iter()
            .enumerate()
            .map(|(i, key)| Member {
                device_id: key.device_id().unwrap(),
                public_key: key.public_key.clone(),
                name: "test".into(),
                role: if i == 0 { Role::Owner } else { Role::Member },
                membership_id: format!("member-{i}"),
                last_seen_at: 0,
                online: true,
            })
            .collect();
        let mut m = Membership {
            group_id: "group-test".into(),
            worker_url: "https://example.invalid".into(),
            role: Role::Owner,
            membership_id: "member-0".into(),
        };
        let ab = PeerIdentity::current(&m, &members, &a, &b.device_id().unwrap()).unwrap();
        m.membership_id = "member-1".into();
        m.role = Role::Member;
        let ba = PeerIdentity::current(&m, &members, &b, &a.device_id().unwrap()).unwrap();
        (a, b, ab, ba)
    }
    fn description() -> RTCSessionDescription {
        serde_json::from_value(serde_json::json!({"type":"offer","sdp":format!("v=0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\na=fingerprint:sha-256 {}\r\na=candidate:1 1 udp 123 127.0.0.1 12345 typ host\r\n", ["AB";32].join(":"))})).unwrap()
    }
    #[test]
    fn signed_description_binds_identity_group_recipient_membership_role_nonce_and_expiry() {
        let (a, _, ab, ba) = identities();
        let nonce = "a".repeat(32);
        let signed =
            SignedDescription::create(&a, &ab, SessionRole::Offer, &nonce, &description(), 1000)
                .unwrap();
        let verify = |s: &SignedDescription| s.verify(&ba, SessionRole::Offer, &nonce, 1001);
        assert!(verify(&SignedDescription::decode(&signed.encode().unwrap()).unwrap()).is_ok());
        for field in 0..8 {
            let mut invalid = signed.clone();
            match field {
                0 => invalid.body.group_id.push('x'),
                1 => invalid.body.to.push('x'),
                2 => invalid.body.from.push('x'),
                3 => invalid.body.from_membership.push('x'),
                4 => invalid.body.to_membership.push('x'),
                5 => invalid.body.role = SessionRole::Answer,
                6 => invalid.body.nonce = "b".repeat(32),
                _ => invalid.body.fingerprint = "00".repeat(32),
            }
            assert!(verify(&invalid).is_err(), "field {field}");
        }
        assert!(matches!(
            signed.verify(&ba, SessionRole::Offer, &nonce, 121000),
            Err(Error::StaleSignature)
        ));
        let mut revoked = ba.clone();
        revoked.remote_membership_id = "rejoined-membership".into();
        assert!(signed
            .verify(&revoked, SessionRole::Offer, &nonce, 1001)
            .is_err());
        let mut bad = signed.clone();
        bad.signature = BASE64.encode([0; 64]);
        assert!(verify(&bad).is_err());
        assert!(!format!("{signed:?}").contains("candidate"));
    }
    #[test]
    fn candidate_policy_rejects_relay_tcp_mdns_servers_and_signaled_prflx() {
        let sdp = description().sdp;
        assert!(inspect_sdp(&sdp).is_ok());
        assert!(
            inspect_sdp(&sdp.replace("typ host", "typ srflx raddr 192.168.1.1 rport 123")).is_ok()
        );
        for text in [
            sdp.replace("typ host", "typ relay"),
            sdp.replace("typ host", "typ prflx"),
            sdp.replace(" udp ", " tcp "),
            sdp.replace("127.0.0.1", "peer.local"),
            sdp.clone() + "a=ice-server:turn:evil.invalid\r\n",
            sdp.replace("127.0.0.1", "239.1.1.1"),
            sdp.replace("12345 typ", "0 typ"),
            sdp.replace("m=application", "m=audio"),
        ] {
            assert!(inspect_sdp(&text).is_err());
        }
    }
}
