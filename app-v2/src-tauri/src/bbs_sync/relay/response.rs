//! Conservative error boundary. Without authoritative platform quota samples
//! there is no daily-quota variant, reset time, or inferred upgrade instruction.
use super::{
    proof::{compact_json, MAX_JSON},
    Error, Result,
};
use serde::Deserialize;
use std::collections::BTreeMap;

pub(super) fn singleton(
    headers: &BTreeMap<String, Vec<String>>,
    key: &str,
    expected: &str,
) -> bool {
    headers
        .get(key)
        .is_some_and(|v| v.len() == 1 && v[0] == expected)
}
fn identity(headers: &BTreeMap<String, Vec<String>>) -> bool {
    !headers.contains_key("content-encoding") || singleton(headers, "content-encoding", "identity")
}

/// Exact application codes take precedence over the generic 429/503 fallback.
/// No raw upstream text, guessed platform number or resetAtUtc leaves here.
pub(super) fn rejected(
    status: u16,
    headers: &BTreeMap<String, Vec<String>>,
    bytes: &[u8],
) -> Error {
    if (200..400).contains(&status) {
        return Error::Protocol;
    }
    if bytes.len() >= MAX_JSON {
        return Error::CloudflareResourceLimit;
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Rejection {
        ok: bool,
        error: String,
    }
    let json = singleton(headers, "content-type", "application/json") && identity(headers);
    if json && compact_json(bytes, MAX_JSON).is_ok() {
        if let Ok(body) = serde_json::from_slice::<Rejection>(bytes) {
            if !body.ok {
                match (status, body.error.as_str()) {
                    (429, "relay_backpressure" | "rate_limited") | (409, "relay_not_ready") => {
                        return Error::Busy
                    }
                    (401, "stale_signature") => return Error::StaleSignature,
                    (401 | 403, "unauthorized")
                    | (401, "invalid_key_or_signature")
                    | (403 | 409, "removed")
                    | (409, "replayed_signature") => return Error::Unauthorized,
                    (409, "protocol_mismatch") => return Error::ProtocolVersion,
                    (
                        409,
                        "relay_session_lost"
                        | "relay_wake_changed"
                        | "relay_wake_expired"
                        | "relay_wake_used"
                        | "relay_instance_changed",
                    ) => return Error::RelaySessionLost,
                    (503, "relay_service_limit") => return Error::CloudflareResourceLimit,
                    (
                        400 | 404 | 405 | 409 | 413,
                        "invalid_relay_integer"
                        | "invalid_relay_json"
                        | "invalid_relay_fields"
                        | "invalid_relay_metadata"
                        | "invalid_relay_direction"
                        | "invalid_relay_origin"
                        | "invalid_relay_url"
                        | "invalid_relay_method"
                        | "invalid_relay_query"
                        | "invalid_relay_cursor"
                        | "invalid_relay_header"
                        | "invalid_relay_final"
                        | "invalid_relay_payload"
                        | "invalid_relay_route"
                        | "invalid_relay_response"
                        | "invalid_relay_ack"
                        | "invalid_tls_statement"
                        | "relay_sequence_conflict"
                        | "relay_sequence_gap"
                        | "relay_response_too_large"
                        | "request_conflict"
                        | "request_too_large"
                        | "invalid_id"
                        | "invalid_hash"
                        | "not_found",
                    ) => return Error::Protocol,
                    _ => {}
                }
            }
        }
    }
    Error::CloudflareResourceLimit
}

/// Success is route-specific. In particular HTML, even with HTTP 200, is not
/// an application receipt. Body checks occur after the bounded HTTP reader.
pub(super) fn json<'a>(
    status: u16,
    headers: &BTreeMap<String, Vec<String>>,
    bytes: &'a [u8],
) -> Result<&'a [u8]> {
    if status != 200 {
        return Err(rejected(status, headers, bytes));
    }
    if !identity(headers) {
        return Err(Error::Protocol);
    }
    if !singleton(headers, "content-type", "application/json") {
        return Err(Error::Protocol);
    }
    compact_json(bytes, MAX_JSON)?;
    Ok(bytes)
}

pub(super) fn mutation(bytes: &[u8], upload: bool) -> Result<()> {
    compact_json(bytes, 1024)?;
    if upload {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Sent {
            accepted: bool,
            consumed: bool,
        }
        let sent: Sent = serde_json::from_slice(bytes).map_err(|_| Error::Protocol)?;
        if sent.accepted == sent.consumed {
            return Err(Error::Protocol);
        }
        // Either accepted or already consumed is only an HTTP receipt. Neither
        // releases ciphertext: that still requires the recipient's signed ACK.
    } else {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Acked {
            accepted: bool,
        }
        let ack: Acked = serde_json::from_slice(bytes).map_err(|_| Error::Protocol)?;
        // False is the Worker's exact response to an older cumulative ACK.
        // Either value completes only this HTTP operation, not consumption.
        let _ = ack.accepted;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
