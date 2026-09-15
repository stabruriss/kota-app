//! Pure account scheduling. Time is injected; no timers, files or sockets here.
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const HEARTBEAT_MS: u64 = 60_000;
pub(crate) const CHANGED_MS: u64 = 2_000;
pub(crate) const IDLE_YIELD_MS: u64 = 30_000;
pub(crate) const ACTIVE_MS: u64 = 60_000;
pub(crate) const POLL_MS: u64 = 2_000;
pub(crate) const MAX_POLLS: u32 = 30;

#[derive(Clone, Debug, Default)]
pub(crate) struct Peer {
    pub(crate) online: bool,
    pub(crate) needed: bool,
    pub(crate) connected: bool,
    pub(crate) busy: bool,
    pub(crate) last_activity: u64,
    pub(crate) connected_at: u64,
    pub(crate) last_served: u64,
    pub(crate) retry_at: u64,
    failures: u32,
}

#[derive(Default, Debug)]
pub(crate) struct Schedule {
    pub(crate) peers: BTreeMap<String, Peer>,
    pub(crate) changed_at: Option<u64>,
    pub(crate) heartbeat_at: Option<u64>,
    joined: bool,
}
impl Schedule {
    pub(crate) fn join(&mut self, now: u64) {
        self.joined = true;
        self.heartbeat_at = Some(now);
    }
    pub(crate) fn leave(&mut self) {
        *self = Self::default();
    }
    pub(crate) fn members(
        &mut self,
        values: impl IntoIterator<Item = (String, bool)>,
    ) -> Vec<String> {
        let values: BTreeMap<_, _> = values.into_iter().take(32).collect();
        let removed: Vec<_> = self
            .peers
            .keys()
            .filter(|id| !values.contains_key(*id))
            .cloned()
            .collect();
        self.peers.retain(|id, _| values.contains_key(id));
        for (id, online) in values {
            let peer = self.peers.entry(id).or_insert_with(|| Peer {
                needed: true,
                ..Peer::default()
            });
            peer.online = online;
        }
        removed
    }
    pub(crate) fn changed(&mut self, now: u64) {
        if self.joined && self.changed_at.is_none() {
            self.changed_at = Some(now.saturating_add(CHANGED_MS));
        }
    }
    pub(crate) fn take_changed(&mut self, now: u64) -> bool {
        if self.changed_at.is_some_and(|at| now >= at) {
            self.changed_at = None;
            for p in self.peers.values_mut() {
                p.needed = true;
            }
            true
        } else {
            false
        }
    }
    pub(crate) fn cancel_current(&mut self, now: u64) {
        self.changed_at = None;
        if self.heartbeat_at.is_some_and(|at| at <= now) {
            self.heartbeat_at = Some(now.saturating_add(HEARTBEAT_MS));
        }
        for peer in self.peers.values_mut() {
            peer.needed = false;
            peer.busy = false;
            peer.connected = false;
        }
    }
    pub(crate) fn manual(&mut self) {
        if !self.joined {
            return;
        }
        for p in self.peers.values_mut() {
            p.needed = true;
            p.retry_at = 0;
        }
    }
    pub(crate) fn heartbeat_due(&mut self, now: u64) -> bool {
        if self.joined && self.heartbeat_at.is_some_and(|at| now >= at) {
            self.heartbeat_at = Some(now.saturating_add(HEARTBEAT_MS));
            // One metadata fallback target plus existing connections. Selection
            // never clears retryAt or invents a new membership.
            let oldest = self
                .peers
                .iter()
                .filter(|(_, p)| p.online)
                .min_by_key(|(id, p)| (p.last_served, *id))
                .map(|(id, _)| id.clone());
            for (id, p) in &mut self.peers {
                if p.connected || Some(id) == oldest.as_ref() {
                    p.needed = true;
                }
            }
            true
        } else {
            false
        }
    }
    pub(crate) fn candidates(&self, now: u64) -> Vec<String> {
        let mut values: Vec<_> = self
            .peers
            .iter()
            .filter(|(_, p)| p.online && p.needed && !p.busy && now >= p.retry_at)
            .collect();
        values.sort_by_key(|(id, p)| (p.last_served, *id));
        values.into_iter().map(|(id, _)| id.clone()).collect()
    }
    pub(crate) fn yield_idle(&self, now: u64) -> Option<String> {
        if !self
            .peers
            .values()
            .any(|p| p.online && p.needed && !p.connected && now >= p.retry_at)
        {
            return None;
        }
        self.peers
            .iter()
            .filter(|(_, p)| {
                p.connected
                    && !p.busy
                    && (now.saturating_sub(p.last_activity) >= IDLE_YIELD_MS
                        || now.saturating_sub(p.connected_at) >= IDLE_YIELD_MS)
            })
            .max_by_key(|(id, p)| (p.last_served, *id))
            .map(|(id, _)| id.clone())
    }
    pub(crate) fn busy(&mut self, id: &str, now: u64) {
        if let Some(p) = self.peers.get_mut(id) {
            p.busy = true;
            p.last_activity = now;
        }
    }
    pub(crate) fn completed(&mut self, id: &str, now: u64) {
        if let Some(p) = self.peers.get_mut(id) {
            p.busy = false;
            p.needed = false;
            p.last_served = now;
            p.last_activity = now;
            p.failures = 0;
            p.retry_at = 0;
        }
    }
    pub(crate) fn connected(&mut self, id: &str, now: u64) {
        if let Some(p) = self.peers.get_mut(id) {
            if !p.connected {
                p.connected_at = now;
            }
            p.connected = true;
            p.last_activity = now;
        }
    }
    pub(crate) fn released(&mut self, id: &str) {
        if let Some(p) = self.peers.get_mut(id) {
            p.connected = false;
            p.busy = false;
        }
    }
    pub(crate) fn failed(&mut self, id: &str, now: u64) {
        if let Some(p) = self.peers.get_mut(id) {
            p.busy = false;
            p.failures = p.failures.saturating_add(1);
            let delay = 5_000u64
                .saturating_mul(1u64 << p.failures.saturating_sub(1).min(6))
                .min(300_000);
            p.retry_at = now.saturating_add(delay);
            p.needed = true;
        }
    }
    pub(crate) fn penalize(&mut self, id: &str, now: u64) {
        let busy = self.peers.get(id).is_some_and(|p| p.busy);
        self.failed(id, now);
        if let Some(p) = self.peers.get_mut(id) {
            p.busy = busy;
        }
    }
    /// Resource occupancy is not a failed sync or a membership change.
    pub(crate) fn occupied(&mut self, id: &str, now: u64) {
        if let Some(p) = self.peers.get_mut(id) {
            p.busy = false;
            p.retry_at = p.retry_at.max(now.saturating_add(2_000));
        }
    }
}

#[derive(Debug)]
pub(crate) struct ActiveWindow {
    pub(crate) until: u64,
    next: u64,
    polls: u32,
}
impl ActiveWindow {
    pub(crate) fn new(now: u64) -> Self {
        Self {
            until: now.saturating_add(ACTIVE_MS),
            next: now,
            polls: 0,
        }
    }
    pub(crate) fn poll(&mut self, now: u64) -> bool {
        if now >= self.until || self.polls >= MAX_POLLS || now < self.next {
            return false;
        }
        self.polls += 1;
        self.next = now.saturating_add(POLL_MS);
        true
    }
}

/// Expiring replay memory is bounded by the member set and the short activity
/// lifetime. At capacity reject; never evict an unexpired consumed nonce.
#[derive(Default, Clone)]
pub(crate) struct Nonces(BTreeMap<String, u64>);
impl Nonces {
    pub(crate) fn contains(&self, peer: &str, nonce: &str, now: u64) -> bool {
        self.0
            .get(&format!("{peer}/{nonce}"))
            .is_some_and(|expiry| *expiry > now)
    }
    pub(crate) fn consume(&mut self, peer: &str, nonce: &str, expires: u64, now: u64) -> bool {
        self.0.retain(|_, expiry| *expiry > now);
        let key = format!("{peer}/{nonce}");
        if expires <= now || self.0.len() >= 256 || self.0.contains_key(&key) {
            return false;
        }
        self.0.insert(key, expires);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unjoined_has_no_schedule_and_changed_is_fixed_window_not_trailing() {
        let mut s = Schedule::default();
        s.changed(1);
        assert!(!s.heartbeat_due(1_000_000));
        assert!(!s.take_changed(1_000_000));
        s.join(0);
        s.changed(0);
        s.changed(1_999);
        assert!(!s.take_changed(1_999));
        assert!(s.take_changed(2_000));
        s.leave();
        assert!(s.peers.is_empty());
        assert!(s.heartbeat_at.is_none());
    }
    #[test]
    fn heartbeat_never_bypasses_backoff_and_busy_does_not_remove_member() {
        let mut s = Schedule::default();
        s.join(0);
        s.members([("peer".into(), true)]);
        for _ in 0..8 {
            s.failed("peer", 100);
        }
        assert_eq!(s.peers["peer"].retry_at, 300_100);
        s.heartbeat_due(60_000);
        assert!(s.candidates(60_000).is_empty());
        s.occupied("peer", 60_000);
        assert_eq!(s.peers.len(), 1);
        assert_eq!(s.peers["peer"].retry_at, 300_100);
        s.manual();
        assert_eq!(s.candidates(60_000), vec!["peer"]);
    }
    #[test]
    fn six_peers_progress_with_four_live_slots_and_active_file_is_not_preempted() {
        let mut s = Schedule::default();
        s.join(0);
        s.members((0..6).map(|i| (format!("peer{i}"), true)));
        let mut served = BTreeSet::new();
        for now in (0..200_000).step_by(1_000) {
            let live = s.peers.values().filter(|p| p.connected).count();
            if live >= 4 {
                if let Some(id) = s.yield_idle(now) {
                    s.released(&id);
                }
            }
            for id in s.candidates(now) {
                if !s.peers[&id].connected && s.peers.values().filter(|p| p.connected).count() >= 4
                {
                    continue;
                }
                s.connected(&id, now);
                s.busy(&id, now);
                if id == "peer0" && now == 0 {
                    continue;
                }
                s.completed(&id, now + 1);
                served.insert(id);
            }
            if now == 50_000 {
                assert!(s.peers["peer0"].busy);
                s.completed("peer0", now);
                served.insert("peer0".into());
            }
            assert!(s.peers.values().filter(|p| p.connected).count() <= 4);
        }
        assert_eq!(served.len(), 6);
    }
    #[test]
    fn continuous_new_tasks_cannot_keep_first_four_slots_forever() {
        let mut s = Schedule::default();
        s.join(0);
        s.members((0..6).map(|n| (n.to_string(), true)));
        for n in 0..4 {
            s.connected(&n.to_string(), 0);
            s.completed(&n.to_string(), 0);
        }
        for now in (1000..=30000).step_by(1000) {
            for n in 0..4 {
                s.busy(&n.to_string(), now);
                s.completed(&n.to_string(), now);
            }
        }
        let released = s
            .yield_idle(30000)
            .expect("hot connection must yield between files");
        s.released(&released);
        assert!(s.candidates(30000).iter().any(|p| p == "4"));
        s.connected("4", 30000);
        s.completed("4", 30001);
        let released = s
            .yield_idle(31000)
            .expect("second waiter must also progress");
        s.released(&released);
        s.connected("5", 31000);
        s.completed("5", 31001);
        assert!(s.peers["4"].last_served > 0 && s.peers["5"].last_served > 0);
        assert!(s.peers.values().filter(|p| p.connected).count() <= 4);
    }
    #[test]
    fn active_polls_and_consumed_nonces_are_bounded_without_window_extension() {
        let mut w = ActiveWindow::new(0);
        let mut count = 0;
        for now in 0..120_000 {
            if w.poll(now) {
                count += 1;
            }
        }
        assert_eq!(count, 30);
        assert_eq!(w.until, 60_000);
        let mut seen = Nonces::default();
        assert!(seen.consume("peer", "one", 60_000, 0));
        assert!(!seen.consume("peer", "one", 60_000, 1));
        for i in 0..255 {
            assert!(seen.consume("peer", &i.to_string(), 60_000, 1));
        }
        assert!(!seen.consume("peer", "overflow", 60_000, 2));
        assert!(seen.consume("peer", "new", 70_000, 60_001));
    }
}

#[cfg(test)]
mod cancel_tests {
    use super::*;
    #[test]
    fn cancel_preserves_backoff_and_future_automatic_eligibility() {
        let mut schedule = Schedule::default();
        schedule.join(0);
        schedule.members([("peer".into(), true)]);
        schedule.heartbeat_due(0);
        schedule.failed("peer", 1);
        let retry = schedule.peers["peer"].retry_at;
        schedule.changed(10);
        schedule.cancel_current(20);
        assert!(schedule.candidates(59_999).is_empty());
        assert!(!schedule.take_changed(3_000));
        assert_eq!(schedule.peers["peer"].retry_at, retry);
        assert_eq!(schedule.heartbeat_at, Some(60_000));
        schedule.changed(30_000);
        assert!(!schedule.take_changed(31_999));
        assert!(schedule.take_changed(32_000));
        assert_eq!(schedule.candidates(32_000), vec!["peer"]);
        schedule.cancel_current(33_000);
        assert!(!schedule.heartbeat_due(59_999));
        assert!(schedule.heartbeat_due(60_000));
        assert_eq!(schedule.candidates(60_000), vec!["peer"]);
        schedule.leave();
        schedule.changed(70_000);
        assert!(!schedule.heartbeat_due(120_000));
        assert!(schedule.candidates(120_000).is_empty());
    }
}
