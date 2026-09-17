use super::*;

fn headers() -> BTreeMap<String, Vec<String>> {
    BTreeMap::from([("content-type".into(), vec!["application/json".into()])])
}
#[test]
fn exact_application_status_and_body_pairs_have_one_closed_mapping() {
    let rows = [
        (429, "relay_backpressure", Error::Busy),
        (429, "rate_limited", Error::Busy),
        (409, "relay_not_ready", Error::Busy),
        (401, "stale_signature", Error::StaleSignature),
        (401, "unauthorized", Error::Unauthorized),
        (403, "unauthorized", Error::Unauthorized),
        (401, "invalid_key_or_signature", Error::Unauthorized),
        (409, "removed", Error::Unauthorized),
        (409, "replayed_signature", Error::Unauthorized),
        (409, "protocol_mismatch", Error::ProtocolVersion),
        (409, "relay_session_lost", Error::RelaySessionLost),
        (409, "relay_wake_changed", Error::RelaySessionLost),
        (409, "relay_wake_used", Error::RelaySessionLost),
        (409, "relay_wake_expired", Error::RelaySessionLost),
        (409, "relay_instance_changed", Error::RelaySessionLost),
        (400, "invalid_relay_query", Error::Protocol),
        (413, "invalid_relay_metadata", Error::Protocol),
        (413, "request_too_large", Error::Protocol),
        (409, "invalid_tls_statement", Error::Protocol),
        (409, "relay_sequence_conflict", Error::Protocol),
        (409, "relay_sequence_gap", Error::Protocol),
        (409, "invalid_relay_ack", Error::Protocol),
        (503, "relay_service_limit", Error::CloudflareResourceLimit),
        (500, "relay_backpressure", Error::CloudflareResourceLimit),
        (400, "unknown_new_error", Error::CloudflareResourceLimit),
        (
            409,
            "worker_update_required",
            Error::CloudflareResourceLimit,
        ),
        (
            429,
            "cloudflare_quota_exceeded",
            Error::CloudflareResourceLimit,
        ),
    ];
    for (status, code, error) in rows {
        let bytes = serde_json::to_vec(&serde_json::json!({"ok":false,"error":code})).unwrap();
        assert_eq!(
            rejected(status, &headers(), &bytes),
            error,
            "{status} {code}"
        );
    }
}

#[test]
fn platform_like_bodies_cannot_generate_daily_quota_reset_or_upgrade_codes() {
    let html = BTreeMap::from([(
        "content-type".into(),
        vec!["text/html; charset=UTF-8".into()],
    )]);
    let samples: &[&[u8]] = &[
        b"",
        b"<html>1027 quota 1102 resetAtUtc 00:00 UTC</html>",
        br#"{"error":"rate_limited"}"#,
        br#"{"ok":false}"#,
        br#"{"ok":true,"error":"relay_backpressure"}"#,
        br#"{"ok":false,"error":"relay_service_limit","resetAtUtc":1789603200000}"#,
        br#"{"ok":false,"error":"cloudflare_quota_exceeded"}"#,
        br#"{"ok":false,"error":"relay_backpressure","error":"relay_backpressure"}"#,
        br#"{ "ok":false,"error":"relay_backpressure"}"#,
        br#"{"ok":false,"error":"1102"}"#,
    ];
    for status in [400, 401, 409, 429, 500, 503] {
        for bytes in samples {
            assert_eq!(
                rejected(status, &headers(), bytes),
                Error::CloudflareResourceLimit
            );
            assert_eq!(
                rejected(status, &html, bytes),
                Error::CloudflareResourceLimit
            );
        }
    }
    assert_eq!(
        rejected(503, &headers(), &vec![b'x'; MAX_JSON + 1]),
        Error::CloudflareResourceLimit
    );
    assert_eq!(json(200, &html, b"<html>1027</html>"), Err(Error::Protocol));
    assert_eq!(json(201, &headers(), b"{}"), Err(Error::Protocol));
    assert_eq!(
        json(200, &headers(), br#"{"x":1,"x":2}"#),
        Err(Error::Protocol)
    );
    assert_eq!(
        json(200, &headers(), &vec![b'x'; MAX_JSON + 1]),
        Err(Error::Protocol)
    );
    let mut compressed = headers();
    compressed.insert("content-encoding".into(), vec!["gzip".into()]);
    assert_eq!(json(200, &compressed, b"{}"), Err(Error::Protocol));
    for code in [
        Error::CloudflareResourceLimit,
        Error::RelaySessionLost,
        Error::Protocol,
        Error::Busy,
    ] {
        assert_ne!(code.to_string(), "cloudflare_quota_exceeded");
    }
}

#[test]
fn upload_and_ack_receipts_cannot_smuggle_consumption_or_unknown_fields() {
    for bytes in [
        br#"{"accepted":true,"consumed":false}"#.as_slice(),
        br#"{"accepted":false,"consumed":true}"#,
    ] {
        mutation(bytes, true).unwrap();
    }
    mutation(br#"{"accepted":true}"#, false).unwrap();
    mutation(br#"{"accepted":false}"#, false).unwrap();
    for bytes in [
        br#"{"ok":true}"#.as_slice(),
        br#"{"accepted":true}"#,
        br#"{"accepted":true,"consumed":true}"#,
        br#"{"accepted":false,"consumed":false}"#,
        br#"{"accepted":true,"consumed":false,"through":"9999999"}"#,
        br#"{"accepted":true,"consumed":false,"resetAtUtc":123}"#,
    ] {
        assert_eq!(mutation(bytes, true), Err(Error::Protocol));
    }
    for bytes in [
        br#"{"ok":true}"#.as_slice(),
        br#"{"accepted":0}"#,
        br#"{"accepted":true,"through":"9999999"}"#,
        br#"{"accepted":true,"accepted":true}"#,
    ] {
        assert_eq!(mutation(bytes, false), Err(Error::Protocol));
    }
}
