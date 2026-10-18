//! Child of agent_bus: Local takes the exact rendered envelope before terminal framing.
use super::*;
use crate::{local_rpc::Injection, shell_switch::PromptSource};

pub(super) fn applies(pty: &PtyManager, message: &ActorMessage) -> bool {
    pty.agent_local_target(&message.target_agent_id, message.launch_request.as_ref())
}

fn source(message: &ActorMessage) -> &'static str {
    match (message.actor_id.as_str(), message.intent.as_str()) {
        (_, "dream") => "kota.dream",
        ("laughing-man", _) => "kota.lm",
        ("bbs", _) | (_, "bbs-thread") => "kota.bbs",
        ("ember", _) => "kota.ember",
        ("bartender", _) => "kota.bartender",
        _ => "kota.bus",
    }
}

pub(super) fn deliver(app: &AppHandle, pty: &PtyManager, message: &ActorMessage,
    payload: String, now: DateTime<Local>, original_id: &str, retry: bool) -> Result<()> {
    let input = Injection::text(original_id, &message.target_agent_id, now.to_rfc3339(), false,
        source(message), payload, Some((&message.actor_id, &message.intent)));
    // Reopening a failed attempt uses the original ledger payload, not this
    // attempt's time/ID. Active or consumed IDs are no-ops in the registry.
    pty.agent_submit_local(app, &message.target_agent_id, input,
        PromptSource::bus(&message.intent), retry, message.launch_request.clone(), soul_input(message, now))
}

pub(super) fn retry(app: &AppHandle, pty: &PtyManager, message: &ActorMessage, original_id: &str) -> Result<()> {
    let now = Local::now();
    let body = app.state::<crate::temporal_context::TemporalContextManager>()
        .prepare_payload_best_effort(message.origin, &message.project_root, &message.target_agent_id, &message.text, now);
    let payload = render_terminal_message_with_body_at(message, &body, now);
    deliver(app, pty, message, payload, now, original_id, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_actor_keeps_the_existing_envelope_bytes_without_terminal_markers() {
        for (actor, intent, expected) in [
            ("agent-fable-silver", "handoff", "kota.bus"), ("laughing-man", "telegram", "kota.lm"),
            ("bbs", "bbs-thread", "kota.bbs"), ("ember", "reminder", "kota.ember"),
            ("violet", "dream", "kota.dream"), ("bartender", "resolve-conflict", "kota.bartender"),
        ] {
            let message = ActorMessage { project_root: "/tmp/local-fixture".into(), actor_id: actor.into(),
                actor_name: "Fixture".into(), target_agent_id: "agent-fable-copper".into(), intent: intent.into(),
                text: "hello\n<unmodified> test".into(), event_id: "event-fixture-1".into(), dedupe_key: None,
                launch_request: None, terminal_timing: None, soul_eligibility: None,
                origin: MessageOrigin::for_actor(actor.starts_with("agent-"), actor, intent) };
            let now: DateTime<Local> = DateTime::parse_from_rfc3339("2026-10-01T12:00:00Z").unwrap().into();
            let payload = render_terminal_message_with_body_at(&message, &message.text, now);
            let input = Injection::text(&message.event_id, &message.target_agent_id, now.to_rfc3339(), false,
                source(&message), payload.clone(), Some((actor, intent)));
            input.validate(&message.target_agent_id).unwrap();
            assert_eq!(input.custom_type, expected);
            assert_eq!(input.content.as_str(), Some(payload.as_str()));
            assert!(!input.content.as_str().unwrap().contains('\x1b'));
        }
    }
}
