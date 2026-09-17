use super::*;
use crate::bbs_sync::{
    raw_sha256,
    relay::{tests::fixture, window::Buffer},
    transport::{Limits, MembershipCheck, APP_QUEUE_BYTES},
};
use serde_json::{json, Value};
use std::sync::Arc;

fn golden() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../relays/laughing-man-cloudflare/tests/fixtures/relay-envelope-v1.json"
    )))
    .unwrap()
}
fn unhex(value: &str) -> Vec<u8> {
    (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).unwrap())
        .collect()
}
fn expected(case: &Value) -> ReceiveRequest {
    ReceiveRequest {
        boot: case["boot"].as_str().unwrap().into(),
        mode: match case["mode"].as_str().unwrap() {
            "data" => ReceiveMode::Data,
            "probe" => ReceiveMode::Probe,
            "receipts" => ReceiveMode::Receipts,
            _ => panic!("fixture mode"),
        },
        cursors: case["cursors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (r[0].as_str().unwrap().into(), r[1].as_u64().unwrap()))
            .collect(),
    }
}
pub(in crate::bbs_sync::relay) fn encode_json(metadata: &Value, bytes: &[u8]) -> Vec<u8> {
    encode_raw(&serde_json::to_vec(metadata).unwrap(), bytes)
}
fn encode_raw(json: &[u8], bytes: &[u8]) -> Vec<u8> {
    let mut out = b"KBR1".to_vec();
    out.extend_from_slice(&(json.len() as u32).to_be_bytes());
    out.extend_from_slice(json);
    out.extend_from_slice(bytes);
    out
}
pub(in crate::bbs_sync::relay) fn empty(request: &ReceiveRequest) -> Vec<u8> {
    encode_json(
        &json!({"boot":request.boot,"items": request.cursors.iter().map(|(id,n)| json!({
        "session":id,"next":n.to_string(),"consumed":n.to_string(),"closed":false,"batches":[],"ack":null,
    })).collect::<Vec<_>>()}),
        &[],
    )
}
fn payload(limits: &Limits, bytes: &[u8]) -> Payload {
    let mut buffer = Buffer::new(limits, bytes.len()).unwrap();
    buffer.spare_mut().copy_from_slice(bytes);
    buffer.advance(bytes.len()).unwrap();
    buffer.freeze().unwrap()
}
fn parse(expected: &ReceiveRequest, bytes: &[u8]) -> Result<Metadata> {
    Metadata::parse(expected, &payload(&Limits::default(), bytes))
}

#[test]
fn independent_node_envelopes_bind_mode_cursors_and_preserve_peer_receipt_bytes() {
    let f = fixture();
    let authorized: MembershipCheck = Arc::new(|| true);
    for case in golden()["cases"].as_array().unwrap() {
        let wire = unhex(case["envelopeHex"].as_str().unwrap());
        assert_eq!(raw_sha256(&wire), case["sha256"].as_str().unwrap());
        assert_eq!(&wire[..4], b"KBR1");
        let limits = Limits::default();
        let body = payload(&limits, &wire);
        let metadata = Metadata::parse(&expected(case), &body).unwrap();
        let packet = metadata
            .into_packet(body, limits.reserve(2 * MAX_ENVELOPE).unwrap())
            .unwrap();
        for (item, source) in packet.items().iter().zip(case["items"].as_array().unwrap()) {
            assert_eq!(item.session(), source["session"].as_str().unwrap());
            assert_eq!(
                item.window(),
                (
                    source["next"].as_u64().unwrap(),
                    source["consumed"].as_u64().unwrap(),
                    source["closed"].as_bool().unwrap()
                )
            );
            assert_eq!(
                item.batches().len(),
                source["batches"].as_array().unwrap().len()
            );
            for (batch, raw) in item
                .batches()
                .iter()
                .zip(source["batches"].as_array().unwrap())
            {
                assert_eq!(batch.sequence, raw["sequence"].as_u64().unwrap());
                assert_eq!(batch.payload.bytes(), unhex(raw["hex"].as_str().unwrap()));
            }
            if let Some(ack) = item.unverified_ack() {
                assert_eq!(serde_json::to_value(ack).unwrap(), source["ack"]);
                assert_eq!(
                    ack.verify(&f.contexts[0], item.session(), 1, f.now, &authorized)
                        .unwrap()
                        .through,
                    1
                );
            } else {
                assert!(source["ack"].is_null());
            }
        }
        drop(packet);
        assert!(limits.reserve(APP_QUEUE_BYTES).is_ok());
    }
}

#[test]
fn envelope_rejects_bad_magic_lengths_truncation_trailing_and_noncanonical_json() {
    let case = golden()["cases"][0].clone();
    let expected = expected(&case);
    let wire = unhex(case["envelopeHex"].as_str().unwrap());
    for n in 1..wire.len() {
        assert!(matches!(parse(&expected, &wire[..n]), Err(Error::Protocol)));
    }
    for version in [b'0', b'2', 0] {
        let mut bad = wire.clone();
        bad[3] = version;
        assert!(matches!(parse(&expected, &bad), Err(Error::Protocol)));
    }
    for len in [0, (MAX_ENVELOPE - PREFIX + 1) as u32, u32::MAX] {
        let mut bad = wire.clone();
        bad[4..8].copy_from_slice(&len.to_be_bytes());
        assert!(matches!(parse(&expected, &bad), Err(Error::Protocol)));
    }
    let mut extra = wire.clone();
    extra.push(0);
    assert!(matches!(parse(&expected, &extra), Err(Error::Protocol)));
    let header: Value = serde_json::from_slice(&wire[8..]).unwrap();
    for raw in [
        serde_json::to_vec_pretty(&header).unwrap(),
        b"{\"boot\":\"b\",\"boot\":\"b\",\"items\":[]}".to_vec(),
        vec![0xff],
    ] {
        assert!(matches!(
            parse(&expected, &encode_raw(&raw, &[])),
            Err(Error::Protocol)
        ));
    }
}

#[test]
fn entire_packet_rejects_unknown_fields_reordered_sessions_gaps_and_inconsistent_ranges() {
    let case = golden()["cases"][4].clone();
    let expected = expected(&case);
    let wire = unhex(case["envelopeHex"].as_str().unwrap());
    let start = 8 + u32::from_be_bytes(wire[4..8].try_into().unwrap()) as usize;
    let valid: Value = serde_json::from_slice(&wire[8..start]).unwrap();
    let changes: Vec<Box<dyn Fn(&mut Value)>> = vec![
        Box::new(|v| {
            v["extra"] = true.into();
        }),
        Box::new(|v| {
            v["boot"] = "c".repeat(64).into();
        }),
        Box::new(|v| {
            v["items"][0]["extra"] = true.into();
        }),
        Box::new(|v| {
            v["items"][0]["batches"][0]["extra"] = true.into();
        }),
        Box::new(|v| {
            v["items"][0].as_object_mut().unwrap().remove("ack");
        }),
        Box::new(|v| {
            v["items"][0]["closed"] = "false".into();
        }),
        Box::new(|v| {
            v["items"][0]["next"] = "1".into();
        }),
        Box::new(|v| {
            v["items"][0]["consumed"] = "1".into();
        }),
        Box::new(|v| {
            v["items"][0]["next"] = "01".into();
        }),
        Box::new(|v| {
            v["items"][0]["next"] = "9007199254740992".into();
        }),
        Box::new(|v| {
            v["items"][0]["next"] = 2.into();
        }),
        Box::new(|v| {
            v["items"][0]["batches"][0]["sequence"] = "1".into();
        }),
        Box::new(|v| {
            v["items"][0]["batches"][1]["sequence"] = "0".into();
        }),
        Box::new(|v| {
            v["items"][0]["batches"][0]["length"] = 0.into();
        }),
        Box::new(|v| {
            v["items"][0]["batches"][0]["length"] = (MAX_BATCH + 1).into();
        }),
        Box::new(|v| {
            v["items"][0]["batches"][0]["length"] = 1.into();
        }),
        Box::new(|v| {
            v["items"].as_array_mut().unwrap().swap(0, 1);
        }),
        Box::new(|v| {
            let b = v["items"][0].clone();
            v["items"].as_array_mut().unwrap().push(b);
        }),
        Box::new(|v| {
            v["items"].as_array_mut().unwrap().pop();
        }),
        Box::new(|v| {
            let b = v["items"][0]["batches"][0].clone();
            v["items"][0]["batches"].as_array_mut().unwrap().push(b);
        }),
        Box::new(|v| {
            v["items"][3]["ack"]["proof"]["boot"] = "c".repeat(64).into();
        }),
        Box::new(|v| {
            v["items"][3]["ack"]["proof"]["session"] = "f".repeat(64).into();
        }),
        Box::new(|v| {
            v["items"][3]["ack"]["proof"]["signature"] = "bad".into();
        }),
        Box::new(|v| {
            v["items"][3]["ack"]["body"] = "{}".into();
        }),
    ];
    for (i, change) in changes.iter().enumerate() {
        let mut bad = valid.clone();
        change(&mut bad);
        assert!(
            matches!(
                parse(&expected, &encode_json(&bad, &wire[start..])),
                Err(Error::Protocol)
            ),
            "mutation {i}"
        );
    }
}

#[test]
fn short_modes_cannot_forward_payload_or_probe_ack_and_max_payload_is_exactly_bounded() {
    let data = golden()["cases"][3].clone();
    let wire = unhex(data["envelopeHex"].as_str().unwrap());
    for mode in [ReceiveMode::Probe, ReceiveMode::Receipts] {
        let mut request = expected(&data);
        request.mode = mode;
        assert!(matches!(parse(&request, &wire), Err(Error::Protocol)));
    }
    let receipt = golden()["cases"][2].clone();
    let mut request = expected(&receipt);
    request.mode = ReceiveMode::Probe;
    assert!(matches!(
        parse(&request, &unhex(receipt["envelopeHex"].as_str().unwrap())),
        Err(Error::Protocol)
    ));
    let request = ReceiveRequest {
        boot: "b".repeat(64),
        mode: ReceiveMode::Data,
        cursors: vec![("e".repeat(64), 0)],
    };
    let header = json!({"boot":request.boot,"items":[{"session":request.cursors[0].0,"next":"1","consumed":"0","closed":false,"ack":null,"batches":[{"sequence":"0","length":MAX_BATCH}]}]});
    let wire = encode_json(&header, &vec![71; MAX_BATCH]);
    assert!(parse(&request, &wire).is_ok());
    let mut over = wire.clone();
    over.push(0);
    assert!(matches!(parse(&request, &over), Err(Error::Protocol)));
    let raw = vec![b' '; MAX_ENVELOPE - PREFIX + 1];
    assert!(matches!(
        parse(&request, &encode_raw(&raw, &[])),
        Err(Error::Protocol)
    ));
}

#[test]
fn batches_are_shared_subviews_and_last_subview_retains_whole_response_charge() {
    let case = golden()["cases"][4].clone();
    let wire = unhex(case["envelopeHex"].as_str().unwrap());
    let limits = Limits::default();
    let body = payload(&limits, &wire);
    let base = body.bytes().as_ptr() as usize;
    let metadata = Metadata::parse(&expected(&case), &body).unwrap();
    let start = metadata.start;
    let packet = metadata
        .into_packet(body, limits.reserve(2 * MAX_ENVELOPE).unwrap())
        .unwrap();
    let first = packet.items()[0].batches()[0].payload.clone();
    assert_eq!(first.bytes().as_ptr() as usize, base + start);
    let second = first.slice(1..3).unwrap();
    let nested = second.slice(1..2).unwrap();
    assert_eq!(nested.bytes().as_ptr() as usize, base + start + 2);
    assert!(matches!(first.slice(0..0), Err(Error::Protocol)));
    assert!(matches!(first.slice(0..usize::MAX), Err(Error::Protocol)));
    drop(packet);
    drop(first);
    drop(second);
    let rest = limits.reserve(APP_QUEUE_BYTES - wire.len()).unwrap();
    assert!(matches!(limits.reserve(1), Err(Error::Busy)));
    drop(nested);
    drop(rest);
    assert!(limits.reserve(APP_QUEUE_BYTES).is_ok());
}
