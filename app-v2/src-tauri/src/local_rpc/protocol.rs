use anyhow::{bail, Result};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

pub(super) const MAX_INJECTION: usize = 16 * 1024 * 1024;
pub(super) const MAX_RECORD: usize = 64 * 1024 * 1024;
pub(super) const QUEUE_CONTROL_CODEC: &str = "kota.queue.v1:";
/// Private Auto Route facts for the workbench. Never model input, history or a room message.
pub(crate) const ROUTE_CODEC: &str = "kota.route.v1:";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum HoldAction { Pause, Resume }
impl HoldAction {
    pub fn command(self) -> &'static str {
        match self { Self::Pause => "kota-hold-pause", Self::Resume => "kota-hold-resume" }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct HoldReceipt {
    schema: u8,
    pub request_id: String,
    pub action: HoldAction,
    pub kota_ids: Vec<String>,
}

pub(super) fn hold_receipt(event: &Value, target: &str) -> Result<HoldReceipt> {
    let raw = event["message"].as_str().and_then(|s| s.strip_prefix(QUEUE_CONTROL_CODEC));
    if event["type"] != "extension_ui_request" || event["method"] != "notify"
        || raw.is_none_or(|s| s.len() > 32 * 1024) { bail!("Invalid Kota queue control receipt"); }
    let receipt: HoldReceipt = serde_json::from_str(raw.unwrap())?;
    let mut unique = std::collections::BTreeSet::new();
    if receipt.schema != 1 || !valid_id(&receipt.request_id, 128) || receipt.kota_ids.len() > 64
        || receipt.kota_ids.iter().any(|id| !unique.insert(id) || !id.split_once('#')
            .is_some_and(|(message, agent)| valid_id(message, 256) && agent == target)) {
        bail!("Invalid Kota queue control receipt");
    }
    Ok(receipt)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalScope {
    pub project_root: PathBuf,
    pub agent_id: String,
    pub session_id: String,
    pub epoch: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DeliveryState {
    Held,
    Sent,
    Accepted,
    Consumed,
    Durable,
    Failed,
    ClearRequested,
    Cleared,
    Abandoned,
}

impl DeliveryState {
    pub(super) fn terminal(self) -> bool {
        matches!(self, Self::Durable | Self::Failed | Self::Cleared | Self::Abandoned)
    }

    pub(super) fn queued(self) -> bool {
        matches!(self, Self::Held | Self::Sent | Self::Accepted | Self::ClearRequested)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum OnlineState { Offline, Starting, Ready, Busy, Error }

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub(crate) enum LocalNotice {
    Event { scope: LocalScope, event: Value },
    Entries {
        scope: LocalScope,
        entries: Vec<Value>,
        #[serde(rename = "leafId")]
        leaf_id: Option<String>,
        since: Option<String>,
        recovery: bool,
    },
    Delivery {
        scope: LocalScope,
        #[serde(rename = "kotaId")]
        kota_id: String,
        #[serde(rename = "messageId")]
        message_id: String,
        state: DeliveryState,
        at: String,
        #[serde(rename = "customType")]
        custom_type: String,
    },
    Status {
        scope: LocalScope,
        state: OnlineState,
        queued: usize,
        compacting: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

/// Called on a separate bounded sink worker, never on the pipe reader.
pub(crate) trait Sink: Send + Sync {
    fn notify(&self, notice: &LocalNotice) -> Result<(), String>;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Injection {
    pub kota_id: String,
    pub custom_type: String,
    pub content: Value,
    pub details: InjectionDetails,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct InjectionDetails {
    pub schema: u8,
    pub message_id: String,
    pub target_agent_id: String,
    pub created_at: String,
    pub private: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_actor_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_intent: Option<String>,
}

pub(super) fn valid_id(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && value.bytes().all(|c|
        c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b':' | b'-'))
}

pub(super) fn known_custom_type(value: &str) -> bool {
    matches!(value, "kota.user" | "kota.lm" | "kota.bus" | "kota.bbs" |
        "kota.ember" | "kota.dream" | "kota.bartender" | "kota.shadow_clone")
}

impl Injection {
    pub(crate) fn text(message_id: &str, target: &str, created_at: String, private: bool,
        custom_type: &str, content: String, source: Option<(&str, &str)>) -> Self {
        Self {
            kota_id: format!("{message_id}#{target}"), custom_type: custom_type.into(),
            content: Value::String(content),
            details: InjectionDetails {
                schema: 1, message_id: message_id.into(), target_agent_id: target.into(), created_at, private,
                source_actor_id: source.map(|s| s.0.into()), source_intent: source.map(|s| s.1.into()),
            },
        }
    }

    pub(crate) fn validate(&self, target: &str) -> Result<()> {
        let d = &self.details;
        if d.schema != 1 || !valid_id(&d.message_id, 256) || !valid_id(&d.target_agent_id, 128)
            || d.target_agent_id != target || self.kota_id != format!("{}#{}", d.message_id, target)
            || chrono::DateTime::parse_from_rfc3339(&d.created_at).is_err()
            || !known_custom_type(&self.custom_type) {
            bail!("invalid Local injection identity or source");
        }
        match &self.content {
            Value::String(text) if !text.is_empty() => {},
            Value::Array(blocks) if !blocks.is_empty() => {
                for block in blocks {
                    let Some(obj) = block.as_object() else { bail!("invalid Local content block") };
                    match obj.get("type").and_then(Value::as_str) {
                        Some("text") if obj.len() == 2 && obj.get("text").is_some_and(Value::is_string) => {},
                        Some("image") if obj.len() == 3
                            && obj.get("mimeType").and_then(Value::as_str).is_some_and(|mime|
                                matches!(mime, "image/png" | "image/jpeg" | "image/webp" | "image/gif")) => {
                            let data = obj.get("data").and_then(Value::as_str)
                                .ok_or_else(|| anyhow::anyhow!("invalid Local image data"))?;
                            if data.is_empty() || base64::engine::general_purpose::STANDARD.decode(data).is_err() {
                                bail!("invalid Local image data");
                            }
                        },
                        _ => bail!("invalid Local content block"),
                    }
                }
            },
            _ => bail!("invalid Local content"),
        }
        if serde_json::to_vec(self)?.len() > MAX_INJECTION { bail!("Local injection exceeds 16 MiB"); }
        Ok(())
    }

    pub(super) fn prompt(&self) -> Result<String> {
        Ok(format!("/kota-inject {}", serde_json::to_string(self)?))
    }
}

/// Only these exact, ID-bearing command errors can affect a delivery.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Rejection { Failed, TooLate }

pub(super) fn rejection(event: &Value, active_id: &str) -> Option<Rejection> {
    command_rejection(event, "kota-inject", active_id)
}

pub(super) fn command_error<'a>(event: &'a Value, command: &str) -> Option<&'a str> {
    if event.get("type")?.as_str()? != "extension_error"
        || event.get("extensionPath")?.as_str()? != format!("command:{command}")
        || event.get("event")?.as_str()? != "command" { return None; }
    event.get("error")?.as_str()
}

pub(super) fn command_rejection(event: &Value, command: &str, active_id: &str) -> Option<Rejection> {
    let error = command_error(event, command)?;
    let (kind, id) = if command == "kota-cancel" {
        if let Some(id) = error.strip_prefix("kota-cancel-too-late kotaId=") { (Rejection::TooLate, id) }
        else {
            let rest = error.strip_prefix("kota-cancel-failed kotaId=")?;
            let (id, code) = rest.split_once(" code=")?;
            if code != "invalid-id" { return None; }
            (Rejection::Failed, id)
        }
    } else if command == "kota-inject" {
        let rest = error.strip_prefix("kota-inject-failed kotaId=")?;
        let (id, code) = rest.split_once(" code=")?;
        if !matches!(code, "invalid-json" | "invalid-envelope" | "invalid-id" |
            "invalid-details" | "invalid-content" | "too-large" | "duplicate" | "queue-limit") { return None; }
        (Rejection::Failed, id)
    } else { return None; };
    if id == "unknown" || id.chars().any(char::is_whitespace) { return None; }
    (urlencoding::decode(id).ok()?.as_ref() == active_id).then_some(kind)
}

#[derive(Clone, Debug)]
pub(crate) struct CustomReceipt {
    pub kota_id: String,
    pub custom_type: String,
    pub details: InjectionDetails,
}

/// Both Pi's custom_message entries and custom message events use this parser.
pub(crate) fn custom_receipt(value: &Value) -> Option<CustomReceipt> {
    let message = if value.get("type").and_then(Value::as_str) == Some("message_end") {
        value.get("message")?
    } else { value };
    if message.get("role").and_then(Value::as_str) != Some("custom")
        && message.get("type").and_then(Value::as_str) != Some("custom_message") { return None; }
    let custom_type = message.get("customType")?.as_str()?;
    if !known_custom_type(custom_type) { return None; }
    let mut details = message.get("details")?.as_object()?.clone();
    let kota_id = details.remove("kotaId")?.as_str()?.to_owned();
    let details: InjectionDetails = serde_json::from_value(Value::Object(details)).ok()?;
    if details.schema != 1 || !valid_id(&details.message_id, 256)
        || !valid_id(&details.target_agent_id, 128)
        || kota_id != format!("{}#{}", details.message_id, details.target_agent_id)
        || chrono::DateTime::parse_from_rfc3339(&details.created_at).is_err() { return None; }
    Some(CustomReceipt { kota_id, custom_type: custom_type.into(), details })
}

#[cfg(test)]
pub(super) fn fixture(id: &str) -> Injection {
    Injection {
        kota_id: format!("{id}#agent-fable-copper"), custom_type: "kota.user".into(),
        content: Value::String("Original text\n  stays intact.".into()),
        details: InjectionDetails { schema: 1, message_id: id.into(),
            target_agent_id: "agent-fable-copper".into(), created_at: "2026-10-02T00:00:00Z".into(),
            private: false, source_actor_id: None, source_intent: None },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn queue_receipts_require_exact_shape_unique_ids_and_current_target() {
        let valid = serde_json::json!({"schema":1,"requestId":"local-7-4","action":"pause","kotaIds":["a#agent-fable-copper"]});
        let event = |body:Value| serde_json::json!({"type":"extension_ui_request","method":"notify","message":format!("{QUEUE_CONTROL_CODEC}{body}")});
        assert_eq!(hold_receipt(&event(valid.clone()),"agent-fable-copper").unwrap().action,HoldAction::Pause);
        let mut extra=valid.clone();extra["extra"]=serde_json::json!(1);
        let mut duplicate=valid.clone();duplicate["kotaIds"]=serde_json::json!(["a#agent-fable-copper","a#agent-fable-copper"]);
        let mut target=valid.clone();target["kotaIds"]=serde_json::json!(["a#another-agent"]);
        let mut oversized=valid.clone();oversized["kotaIds"]=serde_json::json!((0..65).map(|i|format!("{i}#agent-fable-copper")).collect::<Vec<_>>());
        for invalid in [extra,duplicate,target,oversized] { assert!(hold_receipt(&event(invalid),"agent-fable-copper").is_err()); }
    }

    #[test]
    fn preserves_content_and_rejects_untyped_receipts_and_unknown_errors() {
        let input = fixture("msg-1");
        input.validate("agent-fable-copper").unwrap();
        let decoded: Injection = serde_json::from_str(input.prompt().unwrap().strip_prefix("/kota-inject ").unwrap()).unwrap();
        assert_eq!(decoded, input);
        let mut event = json!({"type":"extension_error", "extensionPath":"command:kota-inject", "event":"command",
            "error":"kota-inject-busy-compacting kotaId=msg-1%23agent-fable-copper"});
        assert_eq!(rejection(&event, &input.kota_id), None);
        event["error"] = json!("kota-inject-failed kotaId=unknown code=invalid-json");
        assert_eq!(rejection(&event, &input.kota_id), None);
        event["error"] = json!("kota-inject-failed kotaId=msg-1%23agent-fable-copper code=invalid-json");
        assert_eq!(rejection(&event, &input.kota_id), Some(Rejection::Failed));
        event["extensionPath"] = json!("<runtime>/send_message");
        assert_eq!(rejection(&event, &input.kota_id), None);
        assert!(custom_receipt(&json!({"role":"custom","customType":"untrusted","details":{"kotaId":input.kota_id}})).is_none());
    }
}
