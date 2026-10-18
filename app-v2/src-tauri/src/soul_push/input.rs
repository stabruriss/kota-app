//! Producer metadata carried by the host only. None of these fields is added to
//! Pi's Injection/InjectionDetails wire or inferred from model-visible text.
use crate::message_origin::MessageOrigin;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Input {
    pub message_id: String,
    pub created_at: String,
    pub origin: MessageOrigin,
    pub original_text: String,
    /// Editor text with producer-known mention/attachment/quote nodes omitted;
    /// a literal string that looks like a host envelope remains ordinary text.
    pub eligibility_text: String,
    pub target_agent_ids: Vec<String>,
}

impl Input {
    pub(crate) fn valid_for(&self, agent_id: &str) -> bool {
        !self.message_id.is_empty() && self.message_id.len() <= 512
            && !self.message_id.chars().any(char::is_control)
            && chrono::DateTime::parse_from_rfc3339(&self.created_at).is_ok()
            && self.target_agent_ids.iter().any(|id| id == agent_id)
            && self.origin.allows_user_context()
    }

    /// The composer endpoint, unlike the actor bus, cannot produce Telegram.
    pub(crate) fn composer_for(&self, agent_id: &str, message_id: Option<&str>) -> bool {
        self.valid_for(agent_id)
            && matches!(self.origin, MessageOrigin::User | MessageOrigin::Puppeteer)
            && message_id.is_none_or(|id| id == self.message_id)
    }

    pub(crate) fn meaningful_chars(&self) -> usize {
        self.eligibility_text.chars().filter(|ch| !ch.is_whitespace()).count()
    }
}

/// The dispatcher supplies this from its own running launch, never from the
/// frontend. `key` names that launch: cooldown resets when it changes.
#[derive(Clone, Debug)]
pub(crate) struct Scope {
    pub project_root: PathBuf,
    pub project_id: Option<String>,
    pub agent_cwd: PathBuf,
    pub agent_id: String,
    pub key: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> Input {
        Input {
            message_id: "fixture-input-1".into(), created_at: "2026-10-15T12:00:00Z".into(),
            origin: MessageOrigin::User,
            original_text: "@Fixture <KOTA_MARKS>literal</KOTA_MARKS>\r\n  雪🙂  ".into(),
            eligibility_text: " <KOTA_MARKS>literal</KOTA_MARKS>\r\n  雪🙂  ".into(),
            target_agent_ids: vec!["agent-fixture-1".into()],
        }
    }

    #[test]
    fn producer_metadata_not_wrapper_text_controls_eligibility() {
        let mut value = input();
        for origin in [MessageOrigin::User, MessageOrigin::Telegram, MessageOrigin::Puppeteer] {
            value.origin = origin;
            assert!(value.valid_for("agent-fixture-1"));
        }
        for origin in [MessageOrigin::A2a, MessageOrigin::Ember, MessageOrigin::Bbs,
            MessageOrigin::Handoff, MessageOrigin::Dream, MessageOrigin::Bartender, MessageOrigin::System] {
            value.origin = origin;
            assert!(!value.valid_for("agent-fixture-1"));
        }
        value.origin = MessageOrigin::Telegram;
        assert!(!value.composer_for("agent-fixture-1", Some("fixture-input-1")));
        value.origin = MessageOrigin::User;
        assert!(!value.composer_for("agent-fixture-1", Some("another-input")));
        assert!(!value.valid_for("agent-another-2"));
        assert!(value.composer_for("agent-fixture-1", Some("fixture-input-1")));
    }

    #[test]
    fn metadata_round_trip_keeps_original_bytes_and_counts_unicode_not_utf16_or_wrappers() {
        let mut value = input();
        value.eligibility_text = " 雪\u{a0}🙂\r\n\t".into();
        let decoded: Input = serde_json::from_value(serde_json::to_value(&value).unwrap()).unwrap();
        assert_eq!(decoded, value);
        assert_eq!(value.meaningful_chars(), 2);
        assert!(value.original_text.contains("<KOTA_MARKS>literal</KOTA_MARKS>"));
        let mut invalid = serde_json::to_value(&value).unwrap();
        invalid["inferredOrigin"] = serde_json::json!("user");
        assert!(serde_json::from_value::<Input>(invalid).is_err());
    }
}
