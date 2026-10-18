use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Local, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use crate::message_origin::MessageOrigin;

/// The exact cross-day context attached to one composer send. The user text
/// stays separate, and only the recipients listed here received this context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposerTemporalGap {
    pub current_time: String,
    pub prompt: String,
    pub target_agent_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub elapsed_days_by_target: BTreeMap<String, i64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedComposerPrompt {
    pub target_agent_id: String,
    pub payload: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temporal_gap: Option<ComposerTemporalGap>,
}

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
    pub fn prepare_composer(
        &self,
        project_root: &Path,
        message_id: &str,
        timestamp: &str,
        target_agent_ids: &[String],
        payload: &str,
        now: DateTime<Local>,
    ) -> Result<(Vec<PreparedComposerPrompt>, Vec<std::path::PathBuf>), String> {
        if message_id.trim().is_empty() || DateTime::parse_from_rfc3339(timestamp).is_err() {
            return Err("invalid composer message identity".into());
        }
        let mut seen = HashSet::new();
        let mut elapsed_days_by_target = BTreeMap::new();
        let mut prepared = target_agent_ids
            .iter()
            .map(|id| id.trim())
            .filter(|id| !id.is_empty() && seen.insert((*id).to_string()))
            .map(|id| {
                let (payload, elapsed_days) =
                    self.prepare_with_elapsed_days_best_effort(MessageOrigin::User, project_root, id, payload, now);
                if let Some(days) = elapsed_days {
                    elapsed_days_by_target.insert(id.to_string(), days);
                }
                PreparedComposerPrompt {
                    target_agent_id: id.to_string(),
                    payload,
                    temporal_gap: None,
                }
            })
            .collect::<Vec<_>>();
        let temporal_targets = prepared
            .iter()
            .filter(|item| item.payload != payload)
            .map(|item| item.target_agent_id.clone())
            .collect::<Vec<_>>();
        if temporal_targets.is_empty() {
            return Ok((prepared, Vec::new()));
        }
        let gap = ComposerTemporalGap {
            current_time: now.to_rfc3339_opts(SecondsFormat::Secs, false),
            prompt: render_temporal_gap_payload("", now),
            target_agent_ids: temporal_targets,
            elapsed_days_by_target,
        };
        // Do not hand a wrapped prompt to the caller until its clean original
        // and context are durable. On failure the caller sends the plain prompt.
        let recipients = prepared
            .iter()
            .map(|item| item.target_agent_id.clone())
            .collect::<Vec<_>>();
        let changed = crate::violet::record_composer_temporal_message(
            project_root,
            message_id,
            timestamp,
            payload,
            &recipients,
            &gap,
        )?;
        for item in &mut prepared {
            if gap.target_agent_ids.contains(&item.target_agent_id) {
                item.temporal_gap = Some(gap.clone());
            }
        }
        Ok((prepared, changed))
    }

    /// Dance calls this only after its actual recipient is fixed. The returned payload,
    /// including any existing temporal wrapper, is reused verbatim for delivery retries.
    /// The ordinary composer entry point above is intentionally unchanged.
    pub(crate) fn prepare_puppeteer_composer(
        &self, project_root: &Path, message_id: &str, timestamp: &str,
        target_agent_id: &str, payload: &str, now: DateTime<Local>,
    ) -> Result<(PreparedComposerPrompt, Vec<std::path::PathBuf>), String> {
        if message_id.trim().is_empty() || target_agent_id.trim().is_empty()
            || DateTime::parse_from_rfc3339(timestamp).is_err() {
            return Err("invalid composer message identity".into());
        }
        let (prepared_payload, elapsed_days) = self.prepare_with_elapsed_days_best_effort(
            MessageOrigin::Puppeteer, project_root, target_agent_id, payload, now);
        let temporal_gap = (prepared_payload != payload).then(|| ComposerTemporalGap {
            current_time: now.to_rfc3339_opts(SecondsFormat::Secs, false),
            prompt: render_temporal_gap_payload("", now),
            target_agent_ids: vec![target_agent_id.to_string()],
            elapsed_days_by_target: elapsed_days.map(|days| BTreeMap::from([(target_agent_id.to_string(),days)])).unwrap_or_default(),
        });
        let changed = crate::violet::record_puppeteer_composer_message(
            project_root, message_id, timestamp, payload, target_agent_id, temporal_gap.as_ref())?;
        Ok((PreparedComposerPrompt {target_agent_id:target_agent_id.to_string(),payload:prepared_payload,temporal_gap}, changed))
    }

    pub fn prepare_payload_best_effort(
        &self,
        origin: MessageOrigin,
        project_root: &Path,
        target_agent_id: &str,
        payload: &str,
        now: DateTime<Local>,
    ) -> String {
        self.prepare_with_elapsed_days_best_effort(origin, project_root, target_agent_id, payload, now)
            .0
    }

    fn prepare_with_elapsed_days_best_effort(
        &self,
        origin: MessageOrigin,
        project_root: &Path,
        target_agent_id: &str,
        payload: &str,
        now: DateTime<Local>,
    ) -> (String, Option<i64>) {
        if !origin.uses_temporal_gap() { return (payload.to_string(), None); }
        match self.prepare_payload(project_root, target_agent_id, payload, now) {
            Ok(prepared) => prepared,
            Err(err) => {
                crate::kota_debug_log(&format!(
                    "[temporal-context] skipped for {target_agent_id}: {err}"
                ));
                (payload.to_string(), None)
            }
        }
    }

    fn prepare_payload(
        &self,
        project_root: &Path,
        target_agent_id: &str,
        payload: &str,
        now: DateTime<Local>,
    ) -> Result<(String, Option<i64>), String> {
        let target_agent_id = target_agent_id.trim();
        if target_agent_id.is_empty() || payload.trim().is_empty() {
            return Ok((payload.to_string(), None));
        }

        let _guard = self
            .ledger_guard
            .lock()
            .map_err(|_| "temporal context ledger lock poisoned".to_string())?;
        let ledger = match fs::read_to_string(crate::credit_events_path(project_root)) {
            Ok(ledger) => ledger,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok((payload.to_string(), None))
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
            return Ok((payload.to_string(), None));
        };
        let elapsed = now
            .with_timezone(&Utc)
            .signed_duration_since(turn.occurred_at);
        if elapsed < Duration::hours(TEMPORAL_GAP_THRESHOLD_HOURS) {
            return Ok((payload.to_string(), None));
        }
        if consumed_turn_ids.contains(&turn.event_id) {
            return Ok((payload.to_string(), None));
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

        Ok((
            render_temporal_gap_payload(payload, now),
            Some(elapsed.num_days()),
        ))
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

pub(crate) fn render_temporal_gap_payload(payload: &str, now: DateTime<Local>) -> String {
    format!(
        "<KOTA_TEMPORAL_GAP v=\"1\" current_time=\"{}\">\n{}\n</KOTA_TEMPORAL_GAP>\n{}",
        now.to_rfc3339_opts(SecondsFormat::Secs, false),
        TEMPORAL_GAP_COPY,
        payload
    )
}

pub(crate) fn is_temporal_gap_prompt(text: &str) -> bool {
    let mut lines = text.lines();
    let Some(time) = lines
        .next()
        .and_then(|line| line.strip_prefix("<KOTA_TEMPORAL_GAP v=\"1\" current_time=\""))
        .and_then(|value| value.strip_suffix("\">"))
    else {
        return false;
    };
    !time.is_empty()
        && time.len() <= 64
        && !time.contains('"')
        && lines.next() == Some(TEMPORAL_GAP_COPY)
        && lines.next() == Some("</KOTA_TEMPORAL_GAP>")
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

    mod puppeteer_tests { include!("temporal_context_puppeteer_tests.rs"); }
    mod origin_tests { include!("temporal_context_origin_tests.rs"); }

    #[test]
    fn first_message_has_no_temporal_gap() {
        let root = temp_root("first-message");
        let manager = TemporalContextManager::default();

        let prepared = manager.prepare_payload_best_effort(MessageOrigin::User,
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
        let first = manager.prepare_payload_best_effort(MessageOrigin::User, &root, "agent-a", "hello", now);
        let second = manager.prepare_payload_best_effort(MessageOrigin::User, &root, "agent-a", "again", now);

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
            .prepare_payload_best_effort(MessageOrigin::User, &root, "agent-a", "first", first_now)
            .starts_with("<KOTA_TEMPORAL_GAP"));

        append_turn(&root, "agent-a", "turn-b", "2026-08-17T13:00:00Z");
        assert_eq!(
            manager.prepare_payload_best_effort(MessageOrigin::User,
                &root,
                "agent-a",
                "fresh",
                local_time("2026-08-18T12:59:59Z"),
            ),
            "fresh"
        );
        assert!(manager
            .prepare_payload_best_effort(MessageOrigin::User,
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
            .prepare_payload_best_effort(MessageOrigin::User,
                &root,
                "agent-a",
                "hello a",
                local_time("2026-08-18T12:00:00Z"),
            )
            .starts_with("<KOTA_TEMPORAL_GAP"));
        assert_eq!(
            manager.prepare_payload_best_effort(MessageOrigin::User,
                &root,
                "agent-b",
                "hello b",
                local_time("2026-08-18T12:00:00Z"),
            ),
            "hello b"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn composer_temporal_original_and_context_survive_without_provider_logs() {
        let root = temp_root("fable-composer-history");
        let now = local_time("2026-08-18T12:00:00-07:00");
        append_turn(
            &root,
            "agent-fable-amber",
            "fable-turn-old",
            "2026-08-15T12:00:00Z",
        );
        append_turn(
            &root,
            "agent-fable-birch",
            "fable-turn-new",
            "2026-08-18T18:59:00Z",
        );
        append_turn(
            &root,
            "agent-fable-cedar",
            "fable-turn-previous-day",
            "2026-08-17T19:00:00Z",
        );
        let targets = vec![
            "agent-fable-amber".into(),
            "agent-fable-birch".into(),
            "agent-fable-cedar".into(),
        ];
        let text =
            "Check this file\n/synthetic/fable/project-memory/attachments/composer/image.png";
        let (prepared, changed) = TemporalContextManager::default()
            .prepare_composer(
                &root,
                "fable-composer-one",
                "2026-08-18T18:59:59Z",
                &targets,
                text,
                now,
            )
            .unwrap();
        assert!(!changed.is_empty());
        let gap = prepared[0].temporal_gap.as_ref().unwrap();
        assert_eq!(
            gap.target_agent_ids,
            vec!["agent-fable-amber", "agent-fable-cedar"]
        );
        assert_eq!(
            gap.elapsed_days_by_target,
            BTreeMap::from([
                ("agent-fable-amber".into(), 3),
                ("agent-fable-cedar".into(), 1),
            ])
        );
        assert_eq!(prepared[0].payload, format!("{}{}", gap.prompt, text));
        assert_eq!(prepared[1].payload, text);
        assert!(prepared[1].temporal_gap.is_none());
        assert_eq!(prepared[2].temporal_gap.as_ref(), Some(gap));
        // No source file, provider process, source cursor, or frontend memory is
        // supplied: exercise the same reader used after an App restart.
        let request = serde_json::from_value(serde_json::json!({})).unwrap();
        let state = crate::violet::read_cache(&root, request).unwrap();
        assert_eq!(state.messages.len(), 1);
        let saved = &state.messages[0];
        assert_eq!(saved.id, "fable-composer-one");
        assert_eq!(saved.role, "user");
        assert_eq!(saved.agent_id, "user");
        assert_eq!(saved.text, text);
        assert_eq!(saved.timestamp, "2026-08-18T18:59:59Z");
        assert_eq!(saved.target_agent_ids, targets);
        assert_eq!(saved.temporal_gap.as_ref(), Some(gap));

        // Re-writing the same event is idempotent. A separate user send with
        // identical text is not hidden by the legacy two-minute text bucket.
        crate::violet::record_composer_temporal_message(
            &root,
            &saved.id,
            &saved.timestamp,
            text,
            &targets,
            gap,
        )
        .unwrap();
        crate::violet::record_composer_temporal_message(
            &root,
            "fable-composer-two",
            &saved.timestamp,
            text,
            &targets,
            gap,
        )
        .unwrap();
        let request = serde_json::from_value(serde_json::json!({})).unwrap();
        let reread = crate::violet::read_cache(&root, request).unwrap();
        assert_eq!(reread.messages.len(), 2);
        assert!(reread.messages.iter().all(|message| message.text == text));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn old_composer_context_without_elapsed_days_still_loads() {
        let gap: ComposerTemporalGap = serde_json::from_value(serde_json::json!({
            "currentTime": "2026-08-18T19:00:00Z",
            "prompt": "saved reminder",
            "targetAgentIds": ["agent-fable-amber"]
        }))
        .unwrap();
        assert!(gap.elapsed_days_by_target.is_empty());
    }

    #[test]
    fn composer_without_temporal_context_keeps_the_existing_path() {
        let root = temp_root("fable-composer-plain");
        let (prepared, changed) = TemporalContextManager::default()
            .prepare_composer(
                &root,
                "fable-composer-plain",
                "2026-08-18T19:00:00Z",
                &["agent-fable-amber".into()],
                "ordinary prompt",
                local_time("2026-08-18T19:00:00Z"),
            )
            .unwrap();
        assert_eq!(prepared[0].payload, "ordinary prompt");
        assert!(prepared[0].temporal_gap.is_none());
        assert!(changed.is_empty());
        assert!(!root.join("project-memory/chathistory").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn composer_history_failure_does_not_return_a_wrapped_prompt() {
        let root = temp_root("fable-composer-failure");
        append_turn(
            &root,
            "agent-fable-amber",
            "fable-turn",
            "2026-08-15T12:00:00Z",
        );
        fs::create_dir_all(root.join("project-memory")).unwrap();
        fs::write(root.join("project-memory/chathistory"), "not a directory").unwrap();
        assert!(TemporalContextManager::default()
            .prepare_composer(
                &root,
                "fable-composer-failed",
                "2026-08-18T19:00:00Z",
                &["agent-fable-amber".into()],
                "ordinary prompt",
                local_time("2026-08-18T19:00:00Z"),
            )
            .is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn temporal_echo_recognition_does_not_treat_actor_envelopes_as_composer_echoes() {
        let temporal = render_temporal_gap_payload("hello", local_time("2026-08-18T19:00:00Z"));
        assert!(is_temporal_gap_prompt(&temporal));
        assert!(is_temporal_gap_prompt(&temporal.replace('\n', "\r\n")));
        assert!(!is_temporal_gap_prompt(
            &temporal.replace("v=\"1\"", "v=\"2\"")
        ));
        assert!(!is_temporal_gap_prompt(&format!(
            "<KOTA_MESSAGE id=\"fable-bus\" from=\"laughing-man\" to=\"agent-fable\" intent=\"telegram\">\n{temporal}\n</KOTA_MESSAGE>"
        )));
        assert!(!is_temporal_gap_prompt(&format!(
            "Quoted example:\n{temporal}"
        )));
    }
}
