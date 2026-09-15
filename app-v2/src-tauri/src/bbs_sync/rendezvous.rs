//! Signed preflight before allocating ICE/DTLS resources. The smaller device ID
//! owns the offer nonce, including when both devices initiate at once.
use super::{
    scheduler::{ActiveWindow, Nonces, ACTIVE_MS},
    transport::{Error, PeerIdentity, Result},
    DeviceIdentity,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Kind {
    Wake,
    Ready,
    Busy,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Message {
    protocol_version: u32,
    group_id: String,
    from: String,
    to: String,
    from_membership: String,
    to_membership: String,
    pub(crate) kind: Kind,
    pub(crate) nonce: String,
    issued_at: u64,
    expires_at: u64,
}
impl Message {
    fn bytes(&self) -> Result<Vec<u8>> {
        let mut bytes = b"kota-bbs-wake-v1\n".to_vec();
        bytes.extend(serde_json::to_vec(self).map_err(|_| Error::InvalidSignal)?);
        Ok(bytes)
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SignedWake {
    message: Message,
    signature: String,
}
impl SignedWake {
    pub(crate) fn create(
        own: &DeviceIdentity,
        peer: &PeerIdentity,
        kind: Kind,
        nonce: &str,
        now: u64,
    ) -> Result<Self> {
        if own.device_id().map_err(|_| Error::Unauthorized)? != peer.local_device_id
            || !valid_nonce(nonce)
        {
            return Err(Error::InvalidSignal);
        }
        let message = Message {
            protocol_version: 1,
            group_id: peer.group_id.clone(),
            from: peer.local_device_id.clone(),
            to: peer.remote_device_id.clone(),
            from_membership: peer.local_membership_id.clone(),
            to_membership: peer.remote_membership_id.clone(),
            kind,
            nonce: nonce.into(),
            issued_at: now,
            expires_at: now.saturating_add(ACTIVE_MS),
        };
        let signature = own
            .sign(&message.bytes()?)
            .map_err(|_| Error::Unauthorized)?;
        Ok(Self { message, signature })
    }
    pub(crate) fn verify(&self, peer: &PeerIdentity, now: u64) -> Result<&Message> {
        let m = &self.message;
        if m.protocol_version != 1
            || m.group_id != peer.group_id
            || m.from != peer.remote_device_id
            || m.to != peer.local_device_id
            || m.from_membership != peer.remote_membership_id
            || m.to_membership != peer.local_membership_id
            || !valid_nonce(&m.nonce)
        {
            return Err(Error::Unauthorized);
        }
        if m.expires_at <= now
            || m.issued_at > now.saturating_add(5_000)
            || m.expires_at != m.issued_at.saturating_add(ACTIVE_MS)
        {
            return Err(Error::StaleSignature);
        }
        peer.verify_bytes(&m.bytes()?, &self.signature)?;
        Ok(m)
    }
}
fn valid_nonce(s: &str) -> bool {
    s.len() == 32
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Waking,
    WaitingOffer,
    Offering,
    Answering,
}
#[derive(Debug)]
struct Attempt {
    nonce: String,
    window: ActiveWindow,
    stage: Stage,
}
#[derive(Default)]
pub(crate) struct Rendezvous {
    attempts: BTreeMap<String, Attempt>,
    consumed: Nonces,
    retired: Nonces,
}
impl Rendezvous {
    pub(crate) fn cancel_current(&mut self, now: u64) {
        for (peer, attempt) in std::mem::take(&mut self.attempts) {
            self.consumed
                .consume(&peer, &attempt.nonce, now.saturating_add(ACTIVE_MS), now);
        }
        // Includes completed handshakes, so a delayed ready read by a future
        // heartbeat cannot penalize or restart a user-cancelled attempt.
        self.retired = self.consumed.clone();
    }
    pub(crate) fn was_cancelled(&self, peer: &str, nonce: &str, now: u64) -> bool {
        self.retired.contains(peer, nonce, now)
    }
    pub(crate) fn begin(&mut self, remote: &str, now: u64) -> Option<String> {
        if self.attempts.contains_key(remote) || self.attempts.len() >= 32 {
            return None;
        }
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        self.attempts.insert(
            remote.into(),
            Attempt {
                nonce: nonce.clone(),
                window: ActiveWindow::new(now),
                stage: Stage::Waking,
            },
        );
        Some(nonce)
    }
    pub(crate) fn wake(
        &mut self,
        own: &str,
        remote: &str,
        nonce: &str,
        now: u64,
    ) -> Result<(Kind, String)> {
        if !valid_nonce(nonce) {
            return Err(Error::InvalidSignal);
        }
        if own < remote {
            // The lexicographically larger initiator drops its own competing
            // nonce when it receives this wake. Repeated wake never extends a window.
            let n = self
                .attempts
                .get(remote)
                .map(|a| a.nonce.clone())
                .or_else(|| self.begin(remote, now))
                .ok_or(Error::Busy)?;
            return Ok((Kind::Wake, n));
        }
        if !self
            .consumed
            .consume(remote, nonce, now.saturating_add(ACTIVE_MS), now)
        {
            return Err(Error::InvalidSignal);
        }
        if self
            .attempts
            .get(remote)
            .is_some_and(|a| a.stage != Stage::Waking)
        {
            return Err(Error::InvalidSignal);
        }
        if !self.attempts.contains_key(remote) && self.attempts.len() >= 32 {
            return Err(Error::Busy);
        }
        let window = self
            .attempts
            .remove(remote)
            .map(|a| a.window)
            .unwrap_or_else(|| ActiveWindow::new(now));
        if window.until <= now {
            return Err(Error::Timeout);
        }
        self.attempts.insert(
            remote.into(),
            Attempt {
                nonce: nonce.into(),
                window,
                stage: Stage::WaitingOffer,
            },
        );
        Ok((Kind::Ready, nonce.into()))
    }
    pub(crate) fn ready(&mut self, own: &str, remote: &str, nonce: &str, now: u64) -> Result<u64> {
        if own >= remote {
            return Err(Error::InvalidSignal);
        }
        let a = self.attempts.get_mut(remote).ok_or(Error::InvalidSignal)?;
        if a.stage != Stage::Waking
            || a.nonce != nonce
            || a.window.until <= now
            || !self.consumed.consume(remote, nonce, a.window.until, now)
        {
            return Err(Error::InvalidSignal);
        }
        a.stage = Stage::Offering;
        Ok(a.window.until)
    }
    pub(crate) fn description(
        &mut self,
        own: &str,
        remote: &str,
        nonce: &str,
        now: u64,
    ) -> Result<u64> {
        let a = self.attempts.get_mut(remote).ok_or(Error::InvalidSignal)?;
        let expected = if own < remote {
            Stage::Offering
        } else {
            Stage::WaitingOffer
        };
        if a.stage != expected || a.nonce != nonce || now >= a.window.until {
            return Err(Error::InvalidSignal);
        }
        a.stage = Stage::Answering;
        Ok(a.window.until)
    }
    pub(crate) fn poll_due(&mut self, now: u64) -> bool {
        self.attempts
            .values_mut()
            .fold(false, |due, a| a.window.poll(now) || due)
    }
    pub(crate) fn expires(&mut self, now: u64) -> Vec<String> {
        let expired = self
            .attempts
            .iter()
            .filter(|(_, a)| now >= a.window.until)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in &expired {
            self.attempts.remove(id);
        }
        expired
    }
    pub(crate) fn remove(&mut self, peer: &str) {
        self.attempts.remove(peer);
    }
    pub(crate) fn len(&self) -> usize {
        self.attempts.len()
    }
    pub(crate) fn wake_seen(&self, peer: &str, nonce: &str) -> bool {
        self.attempts
            .get(peer)
            .is_some_and(|a| a.nonce == nonce && a.stage != Stage::Waking)
    }
    pub(crate) fn matches(&self, peer: &str, nonce: &str) -> bool {
        self.attempts.get(peer).is_some_and(|a| a.nonce == nonce)
    }
    pub(crate) fn contains(&self, peer: &str) -> bool {
        self.attempts.contains_key(peer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn staggered_heartbeat_and_simultaneous_initiation_choose_only_smaller_offer() {
        let (mut a, mut b) = (Rendezvous::default(), Rendezvous::default());
        let an = a.begin("b", 0).unwrap();
        let bn = b.begin("a", 0).unwrap();
        assert_eq!(
            a.wake("a", "b", &bn, 23_000).unwrap(),
            (Kind::Wake, an.clone())
        );
        assert_eq!(
            b.wake("b", "a", &an, 23_000).unwrap(),
            (Kind::Ready, an.clone())
        );
        assert!(a.ready("a", "b", &an, 25_000).is_ok());
        assert!(b.ready("b", "a", &bn, 25_000).is_err());
        assert!(b.description("b", "a", &an, 28_000).is_ok());
        assert!(a.description("a", "b", &an, 29_000).is_ok());
    }
    #[test]
    fn cancelled_handshake_nonce_is_retired_without_blocking_the_next_nonce() {
        let mut r = Rendezvous::default();
        let old = r.begin("peer", 1).unwrap();
        r.cancel_current(2);
        assert!(r.was_cancelled("peer", &old, 3));
        let new = r.begin("peer", 4).unwrap();
        assert!(!r.was_cancelled("peer", &new, 5));
        assert!(r.ready("a", "peer", &new, 6).is_ok());
    }
    #[test]
    fn lost_or_replayed_ready_does_not_restart_a_nonce_or_extend_activity() {
        let mut r = Rendezvous::default();
        let nonce = r.begin("b", 0).unwrap();
        assert_eq!(r.expires(60_000), vec!["b"]);
        assert!(r.ready("a", "b", &nonce, 60_000).is_err());
        let new = r.begin("b", 65_000).unwrap();
        assert_ne!(new, nonce);
        assert!(r.ready("a", "b", &nonce, 65_001).is_err());
        assert!(r.ready("a", "b", &new, 65_002).is_ok());
        assert!(r.ready("a", "b", &new, 65_003).is_err());
        assert!(r.description("a", "b", &new, 66_000).is_ok());
        assert!(r.description("a", "b", &new, 66_001).is_err());
    }
}
