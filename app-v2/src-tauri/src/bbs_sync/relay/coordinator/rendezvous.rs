//! Private scheduling permissions. No new protocol, retry formula or poller.
use super::*;
use std::sync::atomic::AtomicU64;

/// Explicitly classify every transport error. New variants must be reviewed;
/// an unknown/unsafe failure must never silently grant early network work.
pub(super) fn blocks_discovery(error: Error) -> bool {
    match error {
        Error::Timeout | Error::Closed | Error::RelaySessionLost => false,
        Error::InvalidSignal
        | Error::Unauthorized
        | Error::StaleSignature
        | Error::InvalidResource
        | Error::Protocol
        | Error::ProtocolVersion
        | Error::CloudflareResourceLimit
        | Error::Integrity
        | Error::Io
        | Error::Busy
        | Error::Cancelled
        | Error::Runtime => true,
    }
}

#[derive(Default)]
pub(super) struct PeerRecovery {
    pub(super) failure: Option<Error>,
    pub(super) natural_until: u64,
    pub(super) early: Early,
}
#[derive(Default)]
pub(super) struct Early {
    pub(super) segment: u64,
    pub(super) used: bool,
}
impl PeerRecovery {
    pub(super) fn active_started(&mut self, at: u64) {
        if at >= self.natural_until {
            self.early.segment = self.early.segment.saturating_add(1);
            self.early.used = false;
        }
    }
}

impl Actor {
    // Attach bounded scheduling facts to the existing started record. Do not
    // log device IDs, revisions, wake IDs or content to diagnose an idle loop.
    pub(super) fn round_reason(&self, id: &str, at: u64) -> serde_json::Value {
        let checkpoint = self.engine.checkpoint(id).ok().flatten();
        let item = self.item(id);
        let revision_mismatch = item
            .as_ref()
            .and_then(|i| i.announcement.as_ref())
            .map(|a| checkpoint.as_deref() != Some(a.revision.as_str()));
        let fresh_wake = item.as_ref().is_some_and(|i| {
            i.wake.as_ref().is_some_and(|w| {
                w.expires_at > at && !self.seen_wakes.contains(id, &w.id, at)
            }) && i.handshake.as_ref().is_some_and(|h| !h.closed)
        });
        serde_json::json!({
            "forced": self.forced.contains(id),
            "verification": self.verification.contains(id),
            "full_catalog_due": self.engine.full_catalog_due(id),
            "checkpoint_known": checkpoint.is_some(),
            "revision_mismatch": revision_mismatch,
            "fresh_wake": fresh_wake,
            "change_pending": self.schedule.changed_at.is_some(),
        })
    }
    /// Isolated actor harness: prepare real local catalogs and publish them via
    /// its in-process relay before injecting capped failures at a fixed phase.
    /// This is not compiled into an App or a public diagnostics surface.
    #[cfg(test)]
    pub(crate) async fn test_rendezvous_backoff(
        &mut self,
        phase_ms: u64,
        exchange: bool,
    ) -> Result<()> {
        // Fix the existing per-instance jitter too: no randomized clock phase
        // or Manual on the passive endpoint can rescue these test cases.
        self.instance = if self.local < self.schedule.peers.keys().next().unwrap().clone() {
            "rendezvous-fixed-client-0001".into()
        } else {
            "rendezvous-fixed-server-0001".into()
        };
        self.engine.refresh(&self.cancel).await?;
        if let Some(intent) = self
            .outbox
            .prepare(
                &self.engine.revision()?,
                &self.instance,
                now(),
                &self.cancel,
            )
            .await?
        {
            let reply = self
                .client(None, self.cancel.clone())?
                .announce(&intent, now(), Instant::now() + PROGRESS_TIMEOUT)?
                .execute()
                .await?;
            let receipt = Receipt::decode(reply.json()?)?;
            assert!(matches!(
                self.outbox
                    .complete(&intent, receipt, now(), &self.cancel)
                    .await?,
                Completion::Confirmed
            ));
        }
        let ids = self.schedule.peers.keys().cloned().collect::<Vec<_>>();
        for id in ids {
            for _ in 0..7 {
                self.fail(&id, Error::Timeout);
            }
            let until = now() + phase_ms;
            self.schedule.peers.get_mut(&id).unwrap().retry_at = until;
            self.recovery.get_mut(&id).unwrap().natural_until = until;
        }
        if exchange {
            for _ in 0..7 {
                self.service_error(ServiceTask::Exchange, Error::Timeout);
            }
        }
        self.needs_refresh = false;
        self.refresh_at = now() + HEARTBEAT_MS;
        self.poll_at = now();
        Ok(())
    }
    pub(super) fn discovery_retry(&self) -> u64 {
        self.service_backoff
            .iter()
            .map(|b| b.cross_until)
            .max()
            .unwrap_or(0)
            .max(self.service_backoff[ServiceTask::Poll as usize].until)
    }
    pub(super) fn passive_retry(&self) -> u64 {
        self.discovery_retry()
            .max(self.service_backoff[ServiceTask::Catalog as usize].until)
            .max(self.service_backoff[ServiceTask::Announce as usize].until)
    }
    pub(super) fn record_failure(&mut self, id: &str, error: Error) {
        let until = self
            .schedule
            .peers
            .get(id)
            .map_or(0, |p| p.retry_at)
            .max(self.service_retry());
        let recovery = self.recovery.entry(id.into()).or_default();
        recovery.failure = Some(error);
        recovery.natural_until = recovery.natural_until.max(until);
        // In particular, another early failure never renews early.used.
        Trace::backoff("peer", error, until.saturating_sub(now()));
    }
    pub(super) fn passive_candidate(&self, id: &str, at: u64) -> bool {
        if at < self.passive_retry()
            || self.discovery_stale
            || self.cancelled_until.is_some_and(|until| at < until)
            || self.attempts.contains_key(id)
            || self.connected.contains_key(id)
            || self.attempts.len() + self.connected.len() >= 4
            || !(self.authorized())()
        {
            return false;
        }
        let Some(p) = self.schedule.peers.get(id) else {
            return false;
        };
        let recovery = self.recovery.get(id);
        if !p.needed
            || p.busy
            || recovery.is_some_and(|r| r.early.used)
            || (at < p.retry_at
                && !recovery
                    .and_then(|r| r.failure)
                    .is_some_and(|e| !blocks_discovery(e)))
        {
            return false;
        }
        let Some(snapshot) = &self.snapshot else {
            return false;
        };
        let Some(item) = snapshot.items.get(id) else {
            return false;
        };
        let (Some(a), Some(w), Some(h)) = (&item.announcement, &item.wake, &item.handshake) else {
            return false;
        };
        if a.peer_version != super::super::PEER_VERSION
            || h.closed
            || item.current_wake.as_deref() != Some(&w.id)
            || self.seen_wakes.contains(id, &w.id, at)
            || !(if self.local.as_str() < id {
                h.ready.server
            } else {
                h.ready.client
            })
        {
            return false;
        }
        self.initial.peer(id).ok().is_some_and(|peer| {
            a.membership == peer.remote_membership_id
                && (self.peer_check(&peer))()
                && w.context(
                    peer,
                    self.initial.membership.worker_url.clone(),
                    snapshot.boot.clone(),
                    "candidate-context".into(),
                    &self.instance,
                    &a.instance,
                    at,
                )
                .is_ok()
        })
    }
    pub(super) fn start_attempt(&mut self, id: &str, at: u64) -> Result<()> {
        if self.attempts.contains_key(id)
            || self.connected.contains_key(id)
            || self.attempts.len() + self.connected.len() >= 4
        {
            return Err(Error::Busy);
        }
        let p = self.schedule.peers.get(id).ok_or(Error::Unauthorized)?;
        let passive = at < p.retry_at || at < self.service_retry();
        if passive && (!self.passive_candidate(id, at) || self.meta.is_some()) {
            return Err(Error::Busy);
        }
        let item = self.item(id).ok_or(Error::Busy)?;
        let a = item.announcement.as_ref().ok_or(Error::Busy)?;
        let peer = self
            .authority
            .read()
            .map_err(|_| Error::Runtime)?
            .peer(id)?;
        if a.membership != peer.remote_membership_id {
            // As before, a stale announcement is skipped, not promoted to an
            // authoritative membership failure visible to the user.
            return Err(Error::Busy);
        }
        if a.peer_version != super::super::PEER_VERSION {
            return Err(Error::ProtocolVersion);
        }
        let reason = self.round_reason(id, at);
        let mut attempt = Attempt {
            tag: uuid::Uuid::new_v4().to_string(),
            peer,
            instance: a.instance.clone(),
            until: at + ACTIVE_MS,
            polls: 0,
            wake: None,
            nonce: uuid::Uuid::new_v4().to_string(),
            ready_boot: None,
            handshake: None,
            offered: false,
            waiting_poll: false,
            tls: None,
            cancel: Cancellation::default(),
            passive_boot: None,
            trace: Trace::new(passive, self.manual_queued),
        };
        if passive {
            let boot = self.snapshot.as_ref().ok_or(Error::Busy)?.boot.clone();
            let wake = item.wake.as_ref().ok_or(Error::RelaySessionLost)?;
            let context = wake.context(
                attempt.peer.clone(),
                self.initial.membership.worker_url.clone(),
                boot.clone(),
                attempt.nonce.clone(),
                &self.instance,
                &attempt.instance,
                at,
            )?;
            // Check the bounded replay table before queueing, without spending
            // either the replay entry or early credit on local backpressure.
            let mut seen = self.seen_wakes.clone();
            if !seen.consume(id, &wake.id, wake.expires_at, at) {
                return Err(Error::Busy);
            }
            let until = Instant::now() + PROGRESS_TIMEOUT;
            let call = self
                .client(Some(&attempt.peer), attempt.cancel.clone())?
                .ready(&context, at, until)?;
            let queued = call.submit()?;
            // No await between real queue admission and this owner's commit.
            attempt.wake = Some(wake.id.clone());
            attempt.passive_boot = Some(boot.clone());
            self.seen_wakes = seen;
            self.recovery.entry(id.into()).or_default().early.used = true;
            self.metadata_submitted(
                Op::Ready(id.into(), attempt.tag.clone(), boot),
                call,
                until,
                Some(queued),
            );
        } else {
            self.recovery
                .entry(id.into())
                .or_default()
                .active_started(at);
        }
        attempt.trace.started(reason);
        self.attempts.insert(id.into(), attempt);
        self.schedule.busy(id, at);
        self.poll_at = self.poll_at.min(at + POLL_MS);
        self.emit(Notice::Connecting(id.into()));
        Ok(())
    }
    pub(super) fn due_poll(&mut self, at: u64) -> Result<()> {
        if self.meta.is_none() && at >= self.poll_at.max(self.discovery_retry()) {
            self.start_poll()?;
        }
        Ok(())
    }
    pub(super) fn next_work_deadline(&self, at: u64) -> u64 {
        let mut deadline = self
            .refresh_at
            .min(self.poll_at.max(self.discovery_retry()));
        if let Some(admission) = self.admission_at {
            deadline = deadline.min(admission);
        }
        // Independent reads must not move a normal active retry to their next
        // 30s slot. Retain its own finite due wake, without polling early.
        for peer in self.schedule.peers.values().filter(|p| p.needed && !p.busy) {
            let retry = peer.retry_at.max(self.service_retry());
            if retry > at {
                deadline = deadline.min(retry);
            }
        }
        deadline
    }
    pub(super) fn manual(&mut self) {
        let service_until = self.service_retry();
        for (id, peer) in &self.schedule.peers {
            let recovery = self.recovery.entry(id.clone()).or_default();
            recovery.natural_until = recovery.natural_until.max(peer.retry_at.max(service_until));
        }
        self.schedule.manual();
        self.forced.extend(self.schedule.peers.keys().cloned());
        self.engine.force_full_catalogs();
        self.cancelled_until = None;
        self.needs_refresh = true;
        for b in &mut self.service_backoff {
            b.until = 0;
            b.cross_until = 0;
        }
        self.poll_at = now();
        self.losses.clear();
        self.manual_until = Some(u64::MAX);
        self.manual_queued = Some(Instant::now());
        self.admission_at = None;
    }
}

static NEXT_TRACE: AtomicU64 = AtomicU64::new(1);
pub(super) struct Trace {
    id: u64,
    started: Instant,
    pub(super) started_at: u64,
    pub(super) passive: bool,
    queued_ms: Option<u128>,
}
impl Trace {
    pub(super) fn backoff(operation: &'static str, error: Error, retry_ms: u64) {
        emit_log(
            serde_json::json!({"pid":std::process::id(), "operation":operation,
            "stage":"backoff", "error":error.to_string(), "retry_ms":retry_ms}),
        );
    }
    pub(super) fn service_lapse(operation: &'static str) {
        emit_log(serde_json::json!({"pid":std::process::id(), "stage":"failed",
            "operation":operation, "error":Error::Unauthorized.to_string(), "cause":"lease_lapsed"}));
    }
    pub(super) fn new(passive: bool, queued: Option<Instant>) -> Self {
        Self {
            id: NEXT_TRACE.fetch_add(1, Ordering::Relaxed),
            started: Instant::now(),
            started_at: now(),
            passive,
            queued_ms: queued.map(|t| t.elapsed().as_millis()),
        }
    }
    pub(super) fn value(
        &self,
        stage: &'static str,
        error: Option<Error>,
        retry_ms: u64,
    ) -> serde_json::Value {
        serde_json::json!({"pid":std::process::id(), "attempt":self.id,
            "mode":if self.passive {"passive"} else {"active"}, "stage":stage,
            "elapsed_ms":self.started.elapsed().as_millis(), "manual_wait_ms":self.queued_ms,
            "error":error.map(|e| e.to_string()), "retry_ms":retry_ms})
    }
    pub(super) fn event(&self, stage: &'static str, error: Option<Error>, retry_ms: u64) {
        emit_log(self.value(stage, error, retry_ms));
    }
    pub(super) fn failure_value(&self, error: Error, cause: Option<&str>) -> serde_json::Value {
        let mut value = self.value("failed", Some(error), 0);
        value["cause"] = serde_json::json!(cause);
        value
    }
    pub(super) fn failed(&self, error: Error, cause: Option<&str>) {
        emit_log(self.failure_value(error, cause));
    }
    fn started(&self, reason: serde_json::Value) {
        let mut value = self.value("started", None, 0);
        value["reason"] = reason;
        emit_log(value);
    }
    pub(super) fn completed(&self, result: &exchange::Outcome) {
        let mut value = self.value("completed", None, 0);
        value["outcome"] = serde_json::json!({
            "completed": result.completed, "total": result.total,
            "failures": result.failures.len(), "omitted": result.omitted, "more": result.more,
        });
        emit_log(value);
    }
}
fn emit_log(value: serde_json::Value) {
    let line = format!("[bbs-relay-rendezvous] {value}");
    #[cfg(not(test))]
    crate::kota_debug_log(&line);
    #[cfg(test)]
    eprintln!("{line}");
}
