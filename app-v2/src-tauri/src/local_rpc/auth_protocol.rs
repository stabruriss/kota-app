use super::{
    credentials::{validate_literal_key, AuthType, Capabilities},
    error::{identifier, LocalError, LocalResult},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

pub(super) const CODEC: &str = "kota.auth.v1:";
pub(crate) const THINKING_LEVELS: &[&str] =
    &["off", "minimal", "low", "medium", "high", "xhigh", "max"];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AuthStart {
    pub operation: String,
    pub provider_id: String,
    pub auth_type: Option<AuthType>,
    pub resource_url: Option<String>,
}
impl AuthStart {
    pub fn validate(&self) -> LocalResult<()> {
        if !identifier(&self.provider_id, 128)
            || !matches!(self.operation.as_str(), "login" | "logout")
        {
            return Err(LocalError::new("invalid_request"));
        }
        if self.operation == "login" {
            let kind = self
                .auth_type
                .ok_or_else(|| LocalError::new("invalid_request"))?;
            if self.provider_id == "azure" && kind == AuthType::ApiKey {
                if !self
                    .resource_url
                    .as_deref()
                    .is_some_and(super::credentials::valid_resource_url)
                {
                    return Err(LocalError::provider(
                        "azure_resource_missing",
                        &self.provider_id,
                    ));
                }
            } else if self.resource_url.is_some() {
                return Err(LocalError::new("invalid_request"));
            }
        } else if self.auth_type.is_some() || self.resource_url.is_some() {
            return Err(LocalError::new("invalid_request"));
        }
        Ok(())
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthStarted {
    pub operation_id: String,
}

// No Debug on a reply: value can be a key, token, or OAuth redirect URL.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AuthReply {
    pub operation_id: String,
    pub prompt_id: String,
    pub value: Option<String>,
    pub cancelled: Option<bool>,
}

#[derive(Clone)]
pub(super) struct Dialog {
    pub native_id: String,
    pub event: Value,
}
impl Dialog {
    pub fn validate_reply(&self, reply: &AuthReply) -> LocalResult<()> {
        if reply.cancelled == Some(true) {
            if reply.value.is_some() {
                return Err(LocalError::new("invalid_request"));
            }
            return Ok(());
        }
        let value = reply
            .value
            .as_deref()
            .ok_or_else(|| LocalError::new("invalid_request"))?;
        if value.len() > 64 * 1024 {
            return Err(LocalError::new("invalid_request"));
        }
        if self.event["prompt"]["type"] == "select" {
            if !self.event["prompt"]["options"]
                .as_array()
                .is_some_and(|options| options.iter().any(|o| o["id"] == value))
            {
                return Err(LocalError::new("invalid_request"));
            }
        }
        if self.event["credentialField"] == "api_key.key" {
            validate_literal_key(value, self.event["providerId"].as_str().unwrap())?;
        }
        Ok(())
    }
}

/// Decode and rebuild a whitelist, rather than forwarding arbitrary provider
/// errors or credential objects. Prompt content is allowed only in UI events.
pub(super) fn decode(
    record: &Value,
    operation: &str,
    provider: Option<&str>,
    auth_type: Option<AuthType>,
) -> LocalResult<Value> {
    let fail = || LocalError::new("auth_protocol_invalid");
    let method = record["method"].as_str().ok_or_else(fail)?;
    let carrier = if method == "notify" {
        &record["message"]
    } else {
        &record["title"]
    };
    let text = carrier
        .as_str()
        .and_then(|s| s.strip_prefix(CODEC))
        .ok_or_else(fail)?;
    if text.len() > 4 * 1024 * 1024 {
        return Err(fail());
    }
    let value: Value = serde_json::from_str(text).map_err(|_| fail())?;
    if value["schema"] != 1
        || value["channel"] != "kota.auth"
        || value["operationId"] != operation
        || provider.is_some_and(|p| value.get("providerId").is_some_and(|id| id != p))
    {
        return Err(fail());
    }
    let mut clean =
        json!({"schema":1,"channel":"kota.auth","operationId":operation,"event":value["event"]});
    if let Some(p) = value.get("providerId") {
        if !p.as_str().is_some_and(|s| identifier(s, 128)) {
            return Err(fail());
        }
        clean["providerId"] = p.clone();
    }
    match value["event"].as_str().ok_or_else(fail)? {
        "started" if method == "notify" => {
            if !value["operation"].as_str().is_some_and(valid_operation) {
                return Err(fail());
            }
            clean["operation"] = value["operation"].clone();
        }
        "prompt" if matches!(method, "input" | "select") => {
            if provider.is_none()
                || value["providerId"] != provider.unwrap()
                || !value["promptId"]
                    .as_str()
                    .is_some_and(|s| identifier(s, 128))
                || serde_json::from_value::<AuthType>(value["authType"].clone()).ok() != auth_type
            {
                return Err(fail());
            }
            let p = &value["prompt"];
            if !p["message"].as_str().is_some_and(|s| s.len() <= 16 * 1024) {
                return Err(fail());
            }
            let kind = p["type"].as_str().ok_or_else(fail)?;
            if !matches!(kind, "text" | "secret" | "manual_code" | "select")
                || (kind == "select") != (method == "select")
            {
                return Err(fail());
            }
            let mut prompt = json!({"type":kind,"message":p["message"]});
            for key in ["placeholder"] {
                if let Some(v) = p.get(key) {
                    if !v.as_str().is_some_and(|s| s.len() <= 16 * 1024) {
                        return Err(fail());
                    }
                    prompt[key] = v.clone();
                }
            }
            if kind == "select" {
                let options = p["options"]
                    .as_array()
                    .filter(|a| a.len() <= 128)
                    .ok_or_else(fail)?;
                let mut ids = std::collections::BTreeSet::new();
                let mut clean_options = Vec::new();
                for option in options {
                    let id = option["id"]
                        .as_str()
                        .filter(|s| identifier(s, 128))
                        .ok_or_else(fail)?;
                    if !ids.insert(id)
                        || !option["label"]
                            .as_str()
                            .is_some_and(|s| s.len() <= 16 * 1024)
                    {
                        return Err(fail());
                    }
                    let mut option_clean = json!({"id":id,"label":option["label"]});
                    if let Some(description) = option.get("description") {
                        if !description.as_str().is_some_and(|s| s.len() <= 16 * 1024) {
                            return Err(fail());
                        }
                        option_clean["description"] = description.clone();
                    }
                    clean_options.push(option_clean);
                }
                if record["options"] != json!(ids.iter().copied().collect::<Vec<_>>()) {
                    // Compare sets because native option order remains the
                    // order given by the extension, not this sorted set.
                    let native = record["options"].as_array().ok_or_else(fail)?;
                    let native_ids: std::collections::BTreeSet<_> =
                        native.iter().filter_map(Value::as_str).collect();
                    if native_ids != ids || native.len() != ids.len() {
                        return Err(fail());
                    }
                }
                prompt["options"] = json!(clean_options);
            }
            if let Some(role) = value.get("credentialField") {
                if role != "api_key.key" || kind != "secret" || auth_type != Some(AuthType::ApiKey)
                {
                    return Err(fail());
                }
                clean["credentialField"] = role.clone();
            }
            clean["promptId"] = value["promptId"].clone();
            clean["authType"] = value["authType"].clone();
            clean["prompt"] = prompt;
        }
        "prompt_closed" if method == "notify" => {
            if !value["promptId"]
                .as_str()
                .is_some_and(|s| identifier(s, 128))
                || !value["reason"].as_str().is_some_and(|s| {
                    matches!(
                        s,
                        "answered" | "cancelled" | "timeout" | "step-aborted" | "interrupted"
                    )
                })
            {
                return Err(fail());
            }
            clean["promptId"] = value["promptId"].clone();
            clean["reason"] = value["reason"].clone();
        }
        "auth_event" if method == "notify" => {
            clean["notification"] = notification(&value["notification"])?;
        }
        "result" if method == "notify" => {
            if !value["operation"].as_str().is_some_and(valid_operation)
                || !value["outcome"].as_str().is_some_and(|s| {
                    matches!(
                        s,
                        "succeeded"
                            | "failed"
                            | "cancelled"
                            | "timeout"
                            | "interrupted"
                            | "committed-unsynchronized"
                            | "indeterminate"
                    )
                })
                || !value["commit"]
                    .as_str()
                    .is_some_and(|s| matches!(s, "confirmed" | "not-written" | "unknown"))
            {
                return Err(fail());
            }
            for key in ["operation", "outcome", "commit"] {
                clean[key] = value[key].clone();
            }
            if let Some(code) = value.get("code") {
                if !code.as_str().is_some_and(|s| {
                    matches!(
                        s,
                        "invalid-request"
                            | "unsupported-provider"
                            | "unsupported-auth"
                            | "busy-auth"
                            | "auth-failed"
                            | "storage-failed"
                            | "credential-sync-failed"
                            | "runtime-exited"
                            | "source-policy"
                            | "model-not-found"
                            | "thinking-invalid"
                    )
                }) {
                    return Err(fail());
                }
                clean["code"] = code.clone();
            }
            // These are private, structured read-only results. Deserialize
            // each at the consumer before exposing any of it to WebView.
            for key in ["status", "statuses", "catalog", "resolved"] {
                if let Some(v) = value.get(key) {
                    clean[key] = v.clone();
                }
            }
        }
        _ => return Err(fail()),
    }
    Ok(clean)
}
fn valid_operation(value: &str) -> bool {
    matches!(
        value,
        "login" | "logout" | "status" | "cancel" | "refresh" | "catalog" | "resolve"
    )
}
fn notification(value: &Value) -> LocalResult<Value> {
    let fail = || LocalError::new("auth_protocol_invalid");
    let kind = value["type"].as_str().ok_or_else(fail)?;
    let mut clean = Map::new();
    clean.insert("type".into(), json!(kind));
    let keys: &[&str] = match kind {
        "info" => &["message"],
        "auth_url" => &["url", "instructions"],
        "device_code" => &["userCode", "verificationUri"],
        "progress" => &["message"],
        _ => return Err(fail()),
    };
    for key in keys {
        if let Some(v) = value.get(key) {
            if !v.as_str().is_some_and(|s| s.len() <= 16 * 1024) {
                return Err(fail());
            }
            clean.insert((*key).into(), v.clone());
        } else if *key != "instructions" {
            return Err(fail());
        }
    }
    for key in ["intervalSeconds", "expiresInSeconds"] {
        if let Some(v) = value.get(key) {
            if !v.as_f64().is_some_and(|n| n.is_finite() && n > 0.0) {
                return Err(fail());
            }
            clean.insert(key.into(), v.clone());
        }
    }
    if let Some(links) = value.get("links") {
        let links = links
            .as_array()
            .filter(|a| a.len() <= 64)
            .ok_or_else(fail)?;
        let mut safe = Vec::new();
        for link in links {
            if !link["url"].as_str().is_some_and(|s| s.len() <= 16 * 1024) {
                return Err(fail());
            }
            let mut entry = json!({"url":link["url"]});
            if let Some(label) = link.get("label") {
                if !label.as_str().is_some_and(|s| s.len() <= 16 * 1024) {
                    return Err(fail());
                }
                entry["label"] = label.clone();
            }
            safe.push(entry);
        }
        clean.insert("links".into(), json!(safe));
    }
    Ok(Value::Object(clean))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Status {
    #[serde(flatten)]
    pub capabilities: Capabilities,
    pub stored_type: Option<AuthType>,
    pub configured: bool,
    pub policy: String,
    pub validity: String,
}
