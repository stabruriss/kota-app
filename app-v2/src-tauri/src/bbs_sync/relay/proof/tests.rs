use super::*;
use crate::bbs_sync::relay::tests::{fixture, Fixture};
use std::sync::Arc;

fn bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}
fn post(f: &Fixture, route: Route) -> Target {
    Target::post(&f.contexts[0].origin, &f.contexts[0].peer.group_id, route).unwrap()
}
fn sign_ack(f: &Fixture, body: &str, sequence: u64, final_flag: bool) -> SignedAck {
    let request = SignedRequest::sign(
        &f.identities[1],
        &f.contexts[1].peer.local_membership_id,
        post(f, Route::Ack),
        Fields::Frame {
            boot: &f.contexts[0].boot,
            session: &"e".repeat(64),
            direction: Direction::ClientToServer,
            sequence,
            final_flag,
        },
        body.as_bytes(),
        f.now,
    )
    .unwrap();
    SignedAck {
        proof: request.proof,
        body: body.into(),
    }
}

#[test]
fn every_rust_request_domain_matches_independent_node_headers_bytes_and_signatures() {
    let golden: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../relays/laughing-man-cloudflare/tests/fixtures/relay-proof-v1.json"
    )))
    .unwrap();
    let f = fixture();
    let boot = &f.contexts[0].boot;
    let session = "e".repeat(64);
    for case in golden["requests"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let (target, fields, actor) = match name {
            "poll" => (
                Target::poll(&f.contexts[0].origin, &f.contexts[0].peer.group_id, None).unwrap(),
                Fields::Read,
                0,
            ),
            "receive" | "receive-data" | "receive-receipts" => (
                Target::receive(
                    &f.contexts[0].origin,
                    &f.contexts[0].peer.group_id,
                    boot,
                    match name {
                        "receive" => ReceiveMode::Probe,
                        "receive-data" => ReceiveMode::Data,
                        _ => ReceiveMode::Receipts,
                    },
                    &[(session.clone(), 0)],
                )
                .unwrap(),
                Fields::Read,
                usize::from(name != "receive-receipts"),
            ),
            "announce" | "wake" => (
                post(
                    &f,
                    if name == "announce" {
                        Route::Announce
                    } else {
                        Route::Wake
                    },
                ),
                Fields::Mutation {
                    nonce: "nonce-fixture-0001",
                },
                0,
            ),
            "ready" | "open" | "open-offer" => (
                post(
                    &f,
                    if name == "ready" {
                        Route::Ready
                    } else {
                        Route::Open
                    },
                ),
                Fields::Boot { boot },
                0,
            ),
            "send" | "ack" | "ack-final" => (
                post(
                    &f,
                    if name == "send" {
                        Route::Send
                    } else {
                        Route::Ack
                    },
                ),
                Fields::Frame {
                    boot,
                    session: &session,
                    direction: Direction::ClientToServer,
                    sequence: u64::from(name == "ack-final"),
                    final_flag: name == "ack-final",
                },
                usize::from(name != "send"),
            ),
            _ => panic!("unexpected vector"),
        };
        let body = bytes(case["bodyHex"].as_str().unwrap());
        let request = SignedRequest::sign(
            &f.identities[actor],
            &f.contexts[actor].peer.local_membership_id,
            target,
            fields,
            &body,
            f.now,
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(request.method(), case["method"].as_str().unwrap(), "{name}");
        assert_eq!(
            request.url(),
            format!(
                "{}{}",
                case["origin"].as_str().unwrap(),
                case["path"].as_str().unwrap()
            )
        );
        assert_eq!(
            request.proof.message(&request.target, &body).unwrap(),
            case["message"].as_str().unwrap().as_bytes(),
            "{name}"
        );
        assert_eq!(
            request.proof.signature,
            case["signature"].as_str().unwrap(),
            "{name}"
        );
        assert_eq!(
            serde_json::to_value(request.headers().unwrap()).unwrap(),
            case["headers"],
            "{name}"
        );
        if name.starts_with("ack") {
            let ack = SignedAck {
                proof: request.proof,
                body: String::from_utf8(body).unwrap(),
            };
            let encoded = serde_json::to_vec(&ack).unwrap();
            let ack = SignedAck::decode(&encoded).unwrap();
            let active: MembershipCheck = Arc::new(|| true);
            assert_eq!(
                ack.verify(&f.contexts[0], &session, 1, f.now, &active)
                    .unwrap(),
                VerifiedAck {
                    through: 1,
                    sequence: u64::from(name == "ack-final"),
                    final_flag: name == "ack-final",
                }
            );
        }
    }
}

#[test]
fn receive_modes_bind_canonical_query_and_use_only_the_appropriate_response_budget() {
    let golden: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../relays/laughing-man-cloudflare/tests/fixtures/relay-proof-v1.json"
    )))
    .unwrap();
    let f = fixture();
    let c = &f.contexts[0];
    let make = |mode| {
        Target::receive(
            &c.origin,
            &c.peer.group_id,
            &c.boot,
            mode,
            &[("e".repeat(64), 0)],
        )
        .unwrap()
    };
    for mode in [ReceiveMode::Data, ReceiveMode::Probe, ReceiveMode::Receipts] {
        let request = SignedRequest::sign(
            &f.identities[0],
            &c.peer.local_membership_id,
            make(mode),
            Fields::Read,
            b"",
            f.now,
        )
        .unwrap();
        assert_eq!(request.small_request(), mode != ReceiveMode::Data);
        assert_eq!(
            request.response_limit(),
            super::super::MAX_ENVELOPE
                + if mode == ReceiveMode::Data {
                    MAX_BATCH
                } else {
                    0
                }
        );
        assert!(request.proof.nonce.is_empty());
        let check = |target: &Target| {
            f.contexts[1].peer.verify_bytes(
                &request.proof.message(target, b"").unwrap(),
                &request.proof.signature,
            )
        };
        assert!(check(&request.target).is_ok());
        for other in [ReceiveMode::Data, ReceiveMode::Probe, ReceiveMode::Receipts] {
            assert_eq!(check(&make(other)).is_ok(), mode == other);
        }
        assert!(SignedRequest::sign(
            &f.identities[0],
            &c.peer.local_membership_id,
            make(mode),
            Fields::Read,
            b"{}",
            f.now
        )
        .is_err());
        if mode == ReceiveMode::Receipts {
            // Rust exposes only the typed constructor, not an arbitrary URL
            // input. These shared Worker rejection vectors cannot reuse the
            // canonical request's proof (case, duplicates, order or aliases).
            for path in golden["receiveQueryRejections"].as_array().unwrap() {
                let mut changed = make(mode);
                changed.path = path.as_str().unwrap().into();
                assert_ne!(changed.path, request.target.path);
                assert!(check(&changed).is_err());
            }
        }
    }
}

#[test]
fn recipient_receipt_rejects_server_claims_altered_body_scope_revocation_and_overshoot() {
    let f = fixture();
    let session = "e".repeat(64);
    let active: MembershipCheck = Arc::new(|| true);
    let absent: MembershipCheck = Arc::new(|| false);
    let ack = sign_ack(&f, r#"{"through":"1","final":false}"#, 0, false);
    let verify = |a: &SignedAck| a.verify(&f.contexts[0], &session, 1, f.now, &active);
    assert!(verify(&ack).is_ok());
    assert!(SignedAck::decode(br#"{"through":1}"#).is_err()); // Worker number has no authority.
    for mutated in [
        r#"{"through":"0","final":false}"#,
        r#"{"through":"2","final":false}"#,
        r#"{"through":"1","final":true}"#,
    ] {
        let mut forged = ack.clone();
        forged.body = mutated.into();
        assert!(matches!(verify(&forged), Err(Error::Unauthorized)));
    }
    let overshoot = sign_ack(&f, r#"{"through":"2","final":false}"#, 1, false);
    assert!(matches!(verify(&overshoot), Err(Error::Protocol)));
    let wrong_final = sign_ack(&f, r#"{"through":"1","final":true}"#, 1, false);
    assert!(matches!(verify(&wrong_final), Err(Error::Protocol)));
    for alternate in ["01", "+1", "1e0", "9007199254740992"] {
        let a = sign_ack(
            &f,
            &format!(r#"{{"through":"{alternate}","final":false}}"#),
            1,
            false,
        );
        assert!(matches!(verify(&a), Err(Error::Protocol)));
    }
    for field in [
        "origin",
        "group",
        "membership",
        "boot",
        "direction",
        "device",
    ] {
        let mut ctx = f.contexts[0].clone();
        let mut a = ack.clone();
        match field {
            "origin" => ctx.origin = "https://other.example".into(),
            "group" => ctx.peer.group_id = "other-group-0001".into(),
            "membership" => ctx.peer.remote_membership_id = "new-membership-0001".into(),
            "boot" => ctx.boot = "new-boot-00000001".into(),
            "direction" => a.proof.direction = "s2c".into(),
            "device" => a.proof.device = f.contexts[0].peer.local_device_id.clone(),
            _ => unreachable!(),
        }
        assert!(
            matches!(
                a.verify(&ctx, &session, 1, f.now, &active),
                Err(Error::Unauthorized)
            ),
            "{field}"
        );
    }
    assert!(matches!(
        ack.verify(&f.contexts[0], &"f".repeat(64), 1, f.now, &active),
        Err(Error::Unauthorized)
    ));
    assert!(matches!(
        ack.verify(&f.contexts[0], &session, 1, f.now, &absent),
        Err(Error::Unauthorized)
    ));
    assert!(matches!(
        ack.verify(
            &f.contexts[0],
            &session,
            1,
            f.now + AUTH_WINDOW_MS + 1,
            &active
        ),
        Err(Error::StaleSignature)
    ));
    let final_ack = sign_ack(&f, r#"{"through":"1","final":true}"#, 1, true);
    let result = verify(&final_ack).unwrap();
    assert_eq!(result.through, 1); // Final does not fabricate additional consumption.
    assert!(result.final_flag);
}

#[test]
fn canonical_targets_domain_fields_body_and_integer_limits_fail_closed() {
    let f = fixture();
    let c = &f.contexts[0];
    let membership = &c.peer.local_membership_id;
    let boot = &c.boot;
    let session = "e".repeat(64);
    let get = || Target::poll(&c.origin, &c.peer.group_id, None).unwrap();
    assert!(SignedRequest::sign(
        &f.identities[0],
        membership,
        get(),
        Fields::Read,
        b"{}",
        f.now
    )
    .is_err());
    assert!(SignedRequest::sign(
        &f.identities[0],
        membership,
        get(),
        Fields::Boot { boot },
        b"",
        f.now
    )
    .is_err());
    assert!(SignedRequest::sign(
        &f.identities[0],
        membership,
        post(&f, Route::Open),
        Fields::Read,
        b"{}",
        f.now
    )
    .is_err());
    assert!(SignedRequest::sign(
        &f.identities[0],
        membership,
        post(&f, Route::Send),
        Fields::Frame {
            boot,
            session: &session,
            direction: Direction::ClientToServer,
            sequence: 0,
            final_flag: true,
        },
        b"x",
        f.now
    )
    .is_err());
    for body in [
        br#"{ "a":1}"#.as_slice(),
        br#"{"a":1,"a":1}"#,
        br#"{"a":1e0}"#,
        br#"{"a":9007199254740992}"#,
        br#"{"a":-1}"#,
    ] {
        assert!(SignedRequest::sign(
            &f.identities[0],
            membership,
            post(&f, Route::Ready),
            Fields::Boot { boot },
            body,
            f.now
        )
        .is_err());
    }
    assert!(SignedRequest::sign(
        &f.identities[0],
        membership,
        post(&f, Route::Send),
        Fields::Frame {
            boot,
            session: &session,
            direction: Direction::ClientToServer,
            sequence: 0,
            final_flag: false,
        },
        &vec![0; MAX_BATCH + 1],
        f.now
    )
    .is_err());
    for cursors in [
        vec![],
        vec![(session.clone(), 0); 5],
        vec![(session.clone(), 0), (session.clone(), 1)],
        vec![(session.clone(), MAX_SAFE_INTEGER + 1)],
        vec![("f".repeat(64), 0), (session.clone(), 0)],
    ] {
        for mode in [ReceiveMode::Data, ReceiveMode::Probe, ReceiveMode::Receipts] {
            assert!(Target::receive(&c.origin, &c.peer.group_id, boot, mode, &cursors).is_err());
        }
    }
    assert!(Target::poll("https://user@worker.example", &c.peer.group_id, None).is_err());
    assert!(Target::poll(&c.origin, "group-fixture-0001&after=evil", None).is_err());
    assert!(Target::post(&c.origin, &c.peer.group_id, Route::Receive).is_err());
    let target = Target::receive(
        &c.origin,
        &c.peer.group_id,
        boot,
        ReceiveMode::Data,
        &[(session, 7)],
    )
    .unwrap();
    let request = SignedRequest::sign(
        &f.identities[0],
        membership,
        target,
        Fields::Read,
        b"",
        f.now,
    )
    .unwrap();
    let headers = request.headers().unwrap();
    assert!(!headers.contains_key("x-kota-relay-nonce"));
    assert!(!headers.contains_key("x-kota-relay-boot")); // Boot is signed in this GET's URL only.
}
