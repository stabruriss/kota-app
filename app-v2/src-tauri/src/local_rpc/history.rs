//! Full-history display transport only. Incremental native entries keep their wire format.
use serde::Deserialize;
use serde_json::Value;

pub(super) const CODEC: &str = "kota.history.v1:";
// Independent of the queue codec's 32 KiB bound. The extension uses a 32 MiB
// soft limit with an envelope reserve; allow a small parsing margin here.
const MAX_BYTES: usize = 32 * 1024 * 1024 + 1024;
pub(super) const TOO_LARGE: &str = "History is too large to restore. Start a new session; the old session is preserved.";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Receipt {
    schema: u8,
    pub request_id: String,
    pub result: History,
}

#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum History {
    Ready { entries: Vec<Value>, #[serde(rename = "leafId")] leaf_id: Option<String> },
    TooLarge {},
}

pub(super) fn receipt(event: &Value) -> Option<Receipt> {
    if event["type"] != "extension_ui_request" || event["method"] != "notify" { return None; }
    let text = event["message"].as_str()?.strip_prefix(CODEC)?;
    if text.len() > MAX_BYTES { return None; }
    let value: Receipt = serde_json::from_str(text).ok()?;
    if value.schema != 1 || value.request_id.is_empty() || value.request_id.len() > 128
        || !value.request_id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')) { return None; }
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(payload: Value) -> Value {
        json!({"type":"extension_ui_request", "method":"notify", "message":format!("{CODEC}{payload}")})
    }

    #[test]
    fn history_codec_accepts_large_payloads_without_expanding_queue_codec() {
        let value = event(json!({"schema":1,"requestId":"local-1-2","result":{"status":"ready",
            "entries":[{"id":"one","text":"x".repeat(40*1024)}],"leafId":"one"}}));
        let result = receipt(&value).unwrap();
        assert!(matches!(result.result, History::Ready { entries, leaf_id } if entries.len()==1 && leaf_id.as_deref()==Some("one")));
        assert!(receipt(&event(json!({"schema":1,"requestId":"local-1-2","result":{"status":"too_large"}}))).is_some());
        let oversized = json!({"type":"extension_ui_request","method":"notify","message":format!("{CODEC}{}", " ".repeat(MAX_BYTES+1))});
        assert!(receipt(&oversized).is_none());
    }

    #[test]
    fn history_codec_rejects_wrong_schema_method_and_partial_success() {
        for payload in [json!({"schema":2,"requestId":"a","result":{"status":"too_large"}}),
            json!({"schema":1,"requestId":"a b","result":{"status":"too_large"}}),
            json!({"schema":1,"requestId":"a","result":{"status":"ready","leafId":null}}),
            json!({"schema":1,"requestId":"a","result":{"status":"too_large","entries":[]}})] {
            assert!(receipt(&event(payload)).is_none());
        }
        let mut value = event(json!({"schema":1,"requestId":"a","result":{"status":"too_large"}}));
        value["method"] = json!("input"); assert!(receipt(&value).is_none());
    }
}
