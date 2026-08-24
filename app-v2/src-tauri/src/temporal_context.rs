use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Local, SecondsFormat, Utc};

const TEMPORAL_GAP_EVENT: &str = "temporal_gap_consumed";
const TEMPORAL_GAP_THRESHOLD_HOURS: i64 = 24;
const TEMPORAL_GAP_COPY: &str =
    "It has been over 24 hours since your last completed response in this room.";

#[derive(Clone, Default)]
pub struct TemporalContextManager {
    ledger_guard: Arc<Mutex<()>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CompletedTurn {
    event_id: String,
    occurred_at: DateTime<Utc>,
}

impl TemporalContextManager {
    pub fn prepare_payload_best_effort(
        &self,
        project_root: &Path,
        target_agent_id: &str,
        payload: &str,
        now: DateTime<Local>,
    ) -> String {
        match self.prepare_payload(project_root, target_agent_id, payload, now) {
            Ok(prepared) => prepared,
            Err(err) => {
                crate::kota_debug_log(&format!(
                    "[temporal-context] skipped for {target_agent_id}: {err}"
                ));
                payload.to_string()
            }
        }
    }

    fn prepare_payload(
        &self,
        project_root: &Path,
        target_agent_id: &str,
        payload: &str,
        now: DateTime<Local>,
    ) -> Result<String, String> {
        let target_agent_id = target_agent_id.trim();
        if target_agent_id.is_empty() || payload.trim().is_empty() {
            return Ok(payload.to_string());
        }

        let _guard = self
            .ledger_guard
            .lock()
            .map_err(|_| "temporal context ledger lock poisoned".to_string())?;
        let ledger = match fs::read_to_string(crate::credit_events_path(project_root)) {
            Ok(ledger) => ledger,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(payload.to_string())
            }
            Err(err) => {
                return Err(format!(
                    "read {}: {err}",
                    crate::credit_events_path(project_root).display()
                ))
            }
        };
        let (turn, consumed_turn_ids) = temporal_context_state(&ledger, target_agent_id)?;
        let Some(turn) = turn else {
            return Ok(payload.to_string());
        };
        if now
            .with_timezone(&Utc)
            .signed_duration_since(turn.occurred_at)
            < Duration::hours(TEMPORAL_GAP_THRESHOLD_HOURS)
        {
            return Ok(payload.to_string());
        }
        if consumed_turn_ids.contains(&turn.event_id) {
            return Ok(payload.to_string());
        }

        // Consume before delivery and never roll back. A failed send may miss one
        // reminder, but no prompt echo or provider lifecycle is needed as state.
        crate::append_project_credit_event(
            project_root,
            &serde_json::json!({
                "event": TEMPORAL_GAP_EVENT,
                "target_agent_id": target_agent_id,
                "baseline_turn_event_id": turn.event_id,
                "occurred_at": now.to_rfc3339_opts(SecondsFormat::Secs, false),
            }),
        )?;

        Ok(render_temporal_gap_payload(payload, now))
    }
}

fn temporal_context_state(
    ledger: &str,
    target_agent_id: &str,
) -> Result<(Option<CompletedTurn>, HashSet<String>), String> {
    let mut latest: Option<CompletedTurn> = None;
    let mut consumed_turn_ids = HashSet::new();
    for line in ledger
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match value.get("event").and_then(|item| item.as_str()) {
            Some("turn") if event_matches_agent(&value, target_agent_id) => {}
            Some(TEMPORAL_GAP_EVENT)
                if value
                    .get("target_agent_id")
                    .or_else(|| value.get("targetAgentId"))
                    .and_then(|item| item.as_str())
                    == Some(target_agent_id) =>
            {
                if let Some(turn_event_id) = value
                    .get("baseline_turn_event_id")
                    .or_else(|| value.get("baselineTurnEventId"))
                    .and_then(|item| item.as_str())
                {
                    consumed_turn_ids.insert(turn_event_id.to_string());
                }
                continue;
            }
            _ => continue,
        }
        let Some(event_id) = value
            .get("source_event_id")
            .or_else(|| value.get("sourceEventId"))
            .and_then(|item| item.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Err(format!(
                "latest turn for {target_agent_id} has no stable source event id"
            ));
        };
        let Some(occurred_at) = value
            .get("occurred_at")
            .or_else(|| value.get("occurredAt"))
            .and_then(|item| item.as_str())
        else {
            return Err(format!(
                "latest turn for {target_agent_id} has no occurrence time"
            ));
        };
        let occurred_at = DateTime::parse_from_rfc3339(occurred_at)
            .map_err(|_| {
                format!("latest turn for {target_agent_id} has an invalid occurrence time")
            })?
            .with_timezone(&Utc);
        let candidate = CompletedTurn {
            event_id: event_id.to_string(),
            occurred_at,
        };
        if latest
            .as_ref()
            .map_or(true, |current| candidate.occurred_at >= current.occurred_at)
        {
            latest = Some(candidate);
        }
    }
    Ok((latest, consumed_turn_ids))
}

fn event_matches_agent(value: &serde_json::Value, target_agent_id: &str) -> bool {
    ["agent_id", "agentId", "incarnation_id", "incarnationId"]
        .iter()
        .find_map(|key| value.get(*key).and_then(|item| item.as_str()))
        == Some(target_agent_id)
}

fn render_temporal_gap_payload(payload: &str, now: DateTime<Local>) -> String {
    format!(
        "<KOTA_TEMPORAL_GAP v=\"1\" current_time=\"{}\">\n{}\n</KOTA_TEMPORAL_GAP>\n{}",
        now.to_rfc3339_opts(SecondsFormat::Secs, false),
        TEMPORAL_GAP_COPY,
        payload
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "kota-temporal-context-{label}-{}",
            Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn local_time(value: &str) -> DateTime<Local> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Local)
    }

    fn append_turn(root: &Path, agent_id: &str, event_id: &str, occurred_at: &str) {
        crate::append_project_credit_event(
            root,
            &serde_json::json!({
                "event": "turn",
                "agent_id": agent_id,
                "source_event_id": event_id,
                "occurred_at": occurred_at,
            }),
        )
        .unwrap();
    }

    #[test]
    fn first_message_has_no_temporal_gap() {
        let root = temp_root("first-message");
        let manager = TemporalContextManager::default();

        let prepared = manager.prepare_payload_best_effort(
            &root,
            "agent-a",
            "hello",
            local_time("2026-08-18T12:00:00-07:00"),
        );

        assert_eq!(prepared, "hello");
        assert!(!crate::credit_events_path(&root).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn threshold_is_inclusive_and_each_end_turn_is_consumed_once() {
        let root = temp_root("consume-once");
        let manager = TemporalContextManager::default();
        append_turn(&root, "agent-a", "turn-a", "2026-08-17T12:00:00-07:00");

        let now = local_time("2026-08-18T12:00:00-07:00");
        let first = manager.prepare_payload_best_effort(&root, "agent-a", "hello", now);
        let second = manager.prepare_payload_best_effort(&root, "agent-a", "again", now);

        assert!(first
            .starts_with("<KOTA_TEMPORAL_GAP v=\"1\" current_time=\"2026-08-18T12:00:00-07:00\">"));
        assert!(first.ends_with("</KOTA_TEMPORAL_GAP>\nhello"));
        assert_eq!(second, "again");
        let ledger = fs::read_to_string(crate::credit_events_path(&root)).unwrap();
        assert_eq!(ledger.matches(TEMPORAL_GAP_EVENT).count(), 1);
        let record = crate::load_project_agent_credit_record(&root, "agent-a");
        assert_eq!(record.turns, 1);
        assert_eq!(
            record.last_active_at.as_deref(),
            Some("2026-08-17T12:00:00-07:00")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn newer_end_turn_starts_a_new_clock() {
        let root = temp_root("new-clock");
        let manager = TemporalContextManager::default();
        append_turn(&root, "agent-a", "turn-a", "2026-08-15T12:00:00Z");
        let first_now = local_time("2026-08-17T12:00:00Z");
        assert!(manager
            .prepare_payload_best_effort(&root, "agent-a", "first", first_now)
            .starts_with("<KOTA_TEMPORAL_GAP"));

        append_turn(&root, "agent-a", "turn-b", "2026-08-17T13:00:00Z");
        assert_eq!(
            manager.prepare_payload_best_effort(
                &root,
                "agent-a",
                "fresh",
                local_time("2026-08-18T12:59:59Z"),
            ),
            "fresh"
        );
        assert!(manager
            .prepare_payload_best_effort(
                &root,
                "agent-a",
                "stale again",
                local_time("2026-08-18T13:00:00Z"),
            )
            .starts_with("<KOTA_TEMPORAL_GAP"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn another_agents_turn_does_not_change_the_clock() {
        let root = temp_root("per-agent");
        let manager = TemporalContextManager::default();
        append_turn(&root, "agent-a", "turn-a", "2026-08-15T12:00:00Z");
        append_turn(&root, "agent-b", "turn-b", "2026-08-18T11:59:00Z");

        assert!(manager
            .prepare_payload_best_effort(
                &root,
                "agent-a",
                "hello a",
                local_time("2026-08-18T12:00:00Z"),
            )
            .starts_with("<KOTA_TEMPORAL_GAP"));
        assert_eq!(
            manager.prepare_payload_best_effort(
                &root,
                "agent-b",
                "hello b",
                local_time("2026-08-18T12:00:00Z"),
            ),
            "hello b"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
