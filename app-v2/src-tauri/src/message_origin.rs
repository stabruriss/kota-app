//! Prompt provenance assigned by the producer, not inferred from message text.
//! Room history keeps the wire value in `message_origin`; unknown/old values
//! are deliberately outside the shared user-input allowlist.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageOrigin {
    User,
    Telegram,
    Puppeteer,
    A2a,
    Ember,
    Bbs,
    Handoff,
    Dream,
    Bartender,
    System,
}

impl MessageOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Telegram => "telegram",
            Self::Puppeteer => "puppeteer",
            Self::A2a => "a2a",
            Self::Ember => "ember",
            Self::Bbs => "bbs",
            Self::Handoff => "handoff",
            Self::Dream => "dream",
            Self::Bartender => "bartender",
            Self::System => "system",
        }
    }

    pub fn allows_user_context(self) -> bool {
        allows_user_context(Some(self.as_str()))
    }

    /// Preserve A2A temporal gaps without admitting peers to Soul inputs.
    pub fn uses_temporal_gap(self) -> bool {
        self.allows_user_context() || self == Self::A2a
    }

    /// The bus resolves project membership before calling this. A project
    /// agent cannot turn its A2A message into user input by choosing an intent.
    pub(crate) fn for_actor(is_project_agent: bool, actor: &str, intent: &str) -> Self {
        if is_project_agent { return Self::A2a; }
        match (actor, intent) {
            ("laughing-man", "telegram") => Self::Telegram,
            (_, "dream") => Self::Dream,
            ("ember", _) => Self::Ember,
            ("bbs", _) => Self::Bbs,
            ("bartender", _) => Self::Bartender,
            (_, "handoff") => Self::Handoff,
            _ => Self::System,
        }
    }
}

/// Shared by Soul push eligibility and episode export.
/// No text/role/actor fallback here: absent legacy metadata stays unknown.
pub(crate) fn allows_user_context(origin: Option<&str>) -> bool {
    matches!(origin, Some("user" | "telegram" | "puppeteer"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_values_and_origin_policies_are_exact() {
        for origin in [MessageOrigin::User, MessageOrigin::Telegram, MessageOrigin::Puppeteer,
            MessageOrigin::A2a, MessageOrigin::Ember, MessageOrigin::Bbs, MessageOrigin::Handoff,
            MessageOrigin::Dream, MessageOrigin::Bartender, MessageOrigin::System] {
            assert_eq!(serde_json::to_value(origin).unwrap(), origin.as_str());
            assert_eq!(serde_json::from_value::<MessageOrigin>(origin.as_str().into()).unwrap(), origin);
            assert_eq!(origin.allows_user_context(), matches!(origin,
                MessageOrigin::User | MessageOrigin::Telegram | MessageOrigin::Puppeteer));
            assert_eq!(origin.uses_temporal_gap(), matches!(origin,
                MessageOrigin::User | MessageOrigin::Telegram | MessageOrigin::Puppeteer | MessageOrigin::A2a));
        }
        for legacy in [None, Some("native"), Some("subagent"), Some("shell_handoff"),
            Some("unknown"), Some("User"), Some(" user ")] {
            assert!(!allows_user_context(legacy));
        }
    }

    #[test]
    fn bus_origin_uses_resolved_sender_not_a_body_or_an_agent_chosen_intent() {
        for intent in ["message", "handoff", "telegram", "dream", "bbs-thread"] {
            assert_eq!(MessageOrigin::for_actor(true, "agent-fixture", intent), MessageOrigin::A2a);
        }
        for (actor, intent, origin) in [
            ("laughing-man", "telegram", MessageOrigin::Telegram),
            ("laughing-man", "status", MessageOrigin::System),
            ("ember", "reminder", MessageOrigin::Ember),
            ("bbs", "bbs-thread", MessageOrigin::Bbs),
            ("violet", "dream", MessageOrigin::Dream),
            ("bartender", "resolve-conflict", MessageOrigin::Bartender),
            ("system", "handoff", MessageOrigin::Handoff),
            ("unknown", "telegram", MessageOrigin::System),
        ] {
            assert_eq!(MessageOrigin::for_actor(false, actor, intent), origin);
        }
    }
}
