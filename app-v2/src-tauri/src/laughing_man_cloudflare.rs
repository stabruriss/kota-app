//! Cloudflare errors observed by Laughing Man's standby client. No BBS state.
use super::{log_preview, LmStandbyConfig};
use chrono::{DateTime, Local, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::OnceLock;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LmCloudflareError {
    pub code: Option<u32>,
    pub message: String,
    pub limit: Option<String>,
    // Persist the first observed reset even after expiry so a continuing fault
    // cannot move it forward. Status/notification projections hide it when stale.
    pub reset_at: Option<String>,
    pub since: String,
    #[serde(default)]
    pub notified_for: Option<String>,
}

impl std::fmt::Display for LmCloudflareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.code {
            Some(code) if self.message.is_empty() => write!(f, "Cloudflare error {code}"),
            Some(code) => write!(f, "Cloudflare error {code}: {}", self.message),
            None => write!(f, "Cloudflare: {}", self.message),
        }
    }
}
impl std::error::Error for LmCloudflareError {}

// Meanings checked against the Workers error table, 2026-10-24:
// https://developers.cloudflare.com/workers/observability/errors/
fn meaning(code: u32) -> &'static str {
    match code {
        1101 => "Worker raised a JavaScript exception.",
        1102 => "Worker exceeded CPU time limit.",
        1103 => "The Worker owner needs to contact Cloudflare Support.",
        1019 => "Worker exceeded the loop limit.",
        1021 => "Worker tried to reach a host it cannot access.",
        1022 => "Cloudflare could not route this request to the Worker.",
        1024 => "Worker tried to make a subrequest to a Cloudflare-owned IP address.",
        1027 => "Worker exceeded the free tier's daily request limit.",
        1042 => "Worker-to-Worker fetch is not allowed, or no Worker serves this address.",
        _ => "",
    }
}

fn next_midnight(now: DateTime<Utc>) -> String {
    (now + chrono::Duration::days(1))
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("UTC midnight")
        .and_utc()
        .to_rfc3339()
}

pub(super) fn classify(text: &str, now: DateTime<Utc>) -> Option<LmCloudflareError> {
    let mut error = LmCloudflareError {
        code: None,
        message: String::new(),
        limit: None,
        reset_at: None,
        since: now.to_rfc3339(),
        notified_for: None,
    };
    if let Ok(body) = serde_json::from_str::<Value>(text) {
        // Never reinterpret another Worker JSON error's text as an edge code.
        if body.get("ok")?.as_bool()?
            || body.get("error")?.as_str()? != "cloudflare_limit"
            || body.get("source")?.as_str()? != "durable_objects"
        {
            return None;
        }
        let message = body.get("message")?.as_str()?;
        if message.trim().is_empty() {
            return None;
        }
        error.message = message.chars().take(200).collect();
        error.limit = body
            .get("limit")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(ToOwned::to_owned);
        // Only the Worker can identify a DO daily quota, from the DO exception.
        error.reset_at = error
            .limit
            .as_ref()
            .and_then(|_| body.get("resetAt"))
            .and_then(Value::as_str)
            .filter(|value| DateTime::parse_from_rfc3339(value).is_ok())
            .map(ToOwned::to_owned);
    } else {
        static TAGS: OnceLock<Regex> = OnceLock::new();
        static CODE: OnceLock<Regex> = OnceLock::new();
        let plain = TAGS
            .get_or_init(|| Regex::new(r"<[^>]*>").unwrap())
            .replace_all(text, " ")
            .replace("&nbsp;", " ");
        let captures = CODE
            .get_or_init(|| Regex::new(r"(?i)\berror(?:\s+code)?\s*:?\s*([0-9]{4,5})\b").unwrap())
            .captures(&plain)?;
        let code = captures[1].parse().ok()?;
        error.code = Some(code);
        error.message = meaning(code).into();
        if code == 1027 {
            error.limit = Some("daily requests".into());
            error.reset_at = Some(next_midnight(now));
        }
    }
    Some(error)
}

impl LmCloudflareError {
    pub(super) fn expire_reset(&mut self, now: DateTime<Utc>) {
        if self.limit.is_none()
            || self
                .reset_at
                .as_deref()
                .is_some_and(|at| DateTime::parse_from_rfc3339(at).map_or(true, |at| at <= now))
        {
            self.reset_at = None;
        }
    }

    fn notification(&self) -> String {
        let mut text = format!(
            "Laughing Man standby is unavailable. Telegram messages cannot reach Kota.\n\n{self}"
        );
        if let Some(at) = self
            .reset_at
            .as_deref()
            .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
        {
            text.push_str(&format!(
                "\n\nResets at {} ({} UTC).",
                at.with_timezone(&Local).format("%H:%M"),
                at.with_timezone(&Utc).format("%H:%M")
            ));
        }
        text
    }
}

/// State and the notification claim are saved together before attempting sendMessage.
/// A failed notification send is not repeated on every poll.
pub(super) fn record_failure(
    config: &mut LmStandbyConfig,
    failure: &anyhow::Error,
    now: DateTime<Utc>,
    owner_chat_id: Option<i64>,
) -> Option<(i64, String)> {
    config.live = false;
    config.last_error = Some(log_preview(&failure.to_string()));
    let Some(incoming) = failure.downcast_ref::<LmCloudflareError>() else {
        // A network failure is not proof that the prior Cloudflare fault recovered.
        return None;
    };
    let mut next = incoming.clone();
    if let Some(current) = config.cloudflare.as_ref() {
        next.since = current.since.clone();
        next.notified_for = current.notified_for.clone();
        // Pin the first observed reset. Neither a new edge parse nor a new Worker
        // response can move an unresolved fault's reset to the following day.
        if current.reset_at.is_some() {
            next.reset_at = current.reset_at.clone();
        }
    }
    let notification = owner_chat_id
        .filter(|_| next.notified_for.is_none())
        .map(|chat_id| {
            next.notified_for = Some(next.since.clone());
            let mut visible = next.clone();
            visible.expire_reset(now);
            (chat_id, visible.notification())
        });
    config.cloudflare = Some(next);
    notification
}

pub(super) fn recovered(config: &mut LmStandbyConfig) {
    config.live = true;
    config.last_error = None;
    config.cloudflare = None;
}

#[derive(Default)]
pub(super) struct Backoff(u32);
impl Backoff {
    pub(super) fn failed(&mut self, cloudflare: bool) -> u64 {
        if !cloudflare {
            return 30;
        }
        let delay = (30u64 << self.0.min(4)).min(300);
        self.0 = (self.0 + 1).min(4);
        delay
    }

    pub(super) fn reset(&mut self) {
        self.0 = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn at(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }
    fn now() -> DateTime<Utc> {
        at("2026-10-24T20:00:00Z")
    }
    fn quota() -> String {
        json!({"ok":false,"error":"cloudflare_limit","source":"durable_objects",
            "limit":"volume of requests","message":"Exceeded allowed volume of requests in Durable Objects free tier.",
            "resetAt":"2026-10-25T00:00:00Z","retryable":false,"overloaded":true}).to_string()
    }

    fn visible(config: &LmStandbyConfig, now: DateTime<Utc>) -> LmCloudflareError {
        let mut error = config.cloudflare.clone().unwrap();
        error.expire_reset(now);
        error
    }

    #[test]
    fn actual_edge_shapes_and_html_do_not_claim_a_daily_quota() {
        for (text, code) in [
            ("error code: 1101\n", 1101),
            ("error code: 1042\n", 1042),
            (
                "<!doctype html><html><h1>Error <span>1102</span></h1></html>",
                1102,
            ),
        ] {
            let error = classify(text, now()).unwrap();
            assert_eq!(error.code, Some(code));
            assert!(error.limit.is_none());
            assert!(error.reset_at.is_none());
            assert!(!error.to_string().contains('<'));
        }
        assert_eq!("error code: 1101\n".len(), 17);
    }

    #[test]
    fn only_the_approved_official_codes_have_meanings() {
        for code in [1101, 1102, 1103, 1019, 1021, 1022, 1024, 1027, 1042] {
            let error = classify(&format!("Error {code}"), now()).unwrap();
            assert!(!error.message.is_empty());
            assert_eq!(error.reset_at.is_some(), code == 1027);
        }
        for code in [1010, 1015, 1999, 10162] {
            let error = classify(&format!("error code: {code}"), now()).unwrap();
            assert_eq!(error.to_string(), format!("Cloudflare error {code}"));
            assert!(error.reset_at.is_none());
        }
        assert!(meaning(1042).contains("Worker-to-Worker"));
        assert!(meaning(1102).contains("CPU"));
    }

    #[test]
    fn worker_quota_json_preserves_original_message_and_reset() {
        let error = classify(&quota(), now()).unwrap();
        assert_eq!(error.code, None);
        assert_eq!(error.limit.as_deref(), Some("volume of requests"));
        assert_eq!(
            error.message,
            "Exceeded allowed volume of requests in Durable Objects free tier."
        );
        assert_eq!(error.reset_at.as_deref(), Some("2026-10-25T00:00:00Z"));
        assert!(error.to_string().contains(&error.message));
    }

    #[test]
    fn other_worker_exceptions_have_no_inferred_reset_and_messages_are_bounded() {
        let text = json!({"ok":false,"error":"cloudflare_limit","source":"durable_objects",
            "message":"🙂".repeat(201),"limit":null,"resetAt":"2026-10-25T00:00:00Z"})
        .to_string();
        let error = classify(&text, now()).unwrap();
        assert_eq!(error.message, "🙂".repeat(200));
        assert!(error.reset_at.is_none());
    }

    #[test]
    fn non_cloudflare_and_malformed_json_keep_the_original_error_path() {
        for text in [
            r#"{"ok":false,"error":"unauthorized","message":"Error 1027"}"#,
            r#"{"ok":false,"error":"cloudflare_limit"}"#,
            "upstream temporarily unavailable",
            "connection timed out",
            "HTTP 503",
            "Error 1101000",
        ] {
            assert!(classify(text, now()).is_none(), "{text}");
        }
    }

    #[test]
    fn backoff_caps_and_resets_without_delaying_ordinary_errors() {
        let mut backoff = Backoff::default();
        assert_eq!(
            (0..7).map(|_| backoff.failed(true)).collect::<Vec<_>>(),
            [30, 60, 120, 240, 300, 300, 300]
        );
        assert_eq!(backoff.failed(false), 30);
        // An ordinary error uses the original interval, but is not recovery.
        assert_eq!(backoff.failed(true), 300);
        backoff.reset();
        assert_eq!(backoff.failed(true), 30);
    }

    #[test]
    fn one_notification_per_fault_persists_until_heartbeat_recovery() {
        let mut config = LmStandbyConfig::default();
        let failure = anyhow::Error::new(classify(&quota(), now()).unwrap());
        let notice = record_failure(&mut config, &failure, now(), Some(42)).unwrap();
        assert_eq!(notice.0, 42);
        assert!(notice.1.contains("00:00 UTC"));
        assert!(!notice.1.contains("retries automatically"));
        let saved = serde_json::to_string(&config).unwrap();
        config = serde_json::from_str(&saved).unwrap();
        assert!(record_failure(&mut config, &failure, now(), Some(42)).is_none());
        assert!(!config.live);
        assert!(!config.last_error.as_ref().unwrap().contains('<'));
        recovered(&mut config);
        assert!(config.live && config.cloudflare.is_none() && config.last_error.is_none());
        assert!(record_failure(&mut config, &failure, now(), Some(42)).is_some());
    }

    #[test]
    fn missing_owner_skips_send_and_does_not_spend_notification_claim() {
        let mut config = LmStandbyConfig::default();
        let failure = anyhow::Error::new(classify("Error 1999", now()).unwrap());
        assert!(record_failure(&mut config, &failure, now(), None).is_none());
        assert!(config.cloudflare.as_ref().unwrap().notified_for.is_none());
        let (_, text) = record_failure(&mut config, &failure, now(), Some(42)).unwrap();
        assert!(text.ends_with("Cloudflare error 1999"));
        assert!(!text.contains("Resets"));
    }

    #[test]
    fn changing_error_codes_does_not_start_a_new_cycle_before_heartbeat_success() {
        let mut config = LmStandbyConfig::default();
        let quota = anyhow::Error::new(classify(&quota(), now()).unwrap());
        assert!(record_failure(&mut config, &quota, now(), Some(42)).is_some());
        let changed = anyhow::Error::new(classify("Error 1102", now()).unwrap());
        assert!(record_failure(&mut config, &changed, now(), Some(42)).is_none());
        assert_eq!(config.cloudflare.as_ref().unwrap().code, Some(1102));
        assert!(visible(&config, now()).reset_at.is_none());
        assert!(record_failure(&mut config, &quota, now(), Some(42)).is_none());
        assert_eq!(
            visible(&config, now()).reset_at.as_deref(),
            Some("2026-10-25T00:00:00Z")
        );
    }

    #[test]
    fn first_quota_evidence_after_1101_can_supply_a_reset_without_renotifying() {
        let mut config = LmStandbyConfig::default();
        let edge = anyhow::Error::new(classify("error code: 1101", now()).unwrap());
        assert!(record_failure(&mut config, &edge, now(), Some(42)).is_some());
        let quota = anyhow::Error::new(classify(&quota(), now()).unwrap());
        assert!(record_failure(&mut config, &quota, now(), Some(42)).is_none());
        assert_eq!(
            visible(&config, now()).reset_at.as_deref(),
            Some("2026-10-25T00:00:00Z")
        );
    }

    #[test]
    fn expired_reset_is_removed_and_never_rolls_forward_while_fault_continues() {
        for sample in [quota(), "error code: 1027".into()] {
            let mut config = LmStandbyConfig::default();
            record_failure(
                &mut config,
                &anyhow::Error::new(classify(&sample, now()).unwrap()),
                now(),
                Some(42),
            );
            let tomorrow = at("2026-10-25T00:01:00Z");
            let fresh_sample = sample.replace("2026-10-25T00:00:00Z", "2026-10-26T00:00:00Z");
            let fresh = anyhow::Error::new(classify(&fresh_sample, tomorrow).unwrap());
            assert!(record_failure(&mut config, &fresh, tomorrow, Some(42)).is_none());
            assert!(visible(&config, tomorrow).reset_at.is_none());
            // Retain the pin across saved state and ordinary network failures too.
            config = serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
            record_failure(
                &mut config,
                &anyhow::anyhow!("network unavailable"),
                tomorrow,
                Some(42),
            );
            assert!(record_failure(&mut config, &fresh, tomorrow, Some(42)).is_none());
            assert!(visible(&config, tomorrow).reset_at.is_none());
        }
    }

    #[test]
    fn already_expired_first_reset_is_hidden_in_a_delayed_owner_notification() {
        let tomorrow = at("2026-10-25T00:01:00Z");
        let mut config = LmStandbyConfig::default();
        let error = anyhow::Error::new(classify(&quota(), tomorrow).unwrap());
        assert!(record_failure(&mut config, &error, tomorrow, None).is_none());
        let (_, text) = record_failure(&mut config, &error, tomorrow, Some(42)).unwrap();
        assert!(!text.contains("Resets"));
        let fresh = anyhow::Error::new(
            classify(&quota().replace("2026-10-25", "2026-10-26"), tomorrow).unwrap(),
        );
        assert!(record_failure(&mut config, &fresh, tomorrow, Some(42)).is_none());
        assert!(visible(&config, tomorrow).reset_at.is_none());
    }

    #[test]
    fn old_config_deserializes_without_cloudflare_and_other_errors_stay_ordinary() {
        let mut value = serde_json::to_value(LmStandbyConfig::default()).unwrap();
        value.as_object_mut().unwrap().remove("cloudflare");
        let mut config: LmStandbyConfig = serde_json::from_value(value).unwrap();
        assert!(config.cloudflare.is_none());
        assert!(record_failure(
            &mut config,
            &anyhow::anyhow!("standby: connection failed"),
            now(),
            Some(42)
        )
        .is_none());
        assert_eq!(
            config.last_error.as_deref(),
            Some("standby: connection failed")
        );
    }
}
