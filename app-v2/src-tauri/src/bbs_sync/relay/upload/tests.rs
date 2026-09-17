use super::*;
use crate::bbs_sync::{
    raw_sha256,
    relay::{
        proof::{Direction, Fields, Route, SignedAck, SignedRequest, Target},
        tests::{fixture, Fixture},
        window::SendWindow,
        PendingTls,
    },
    transport::{Cancellation, MembershipCheck, APP_QUEUE_BYTES},
};

fn authorized() -> MembershipCheck {
    Arc::new(|| true)
}

pub(in crate::bbs_sync::relay) fn fragmented(limits: &Limits, bytes: &[u8]) -> Upload {
    assert!(!bytes.is_empty() && bytes.len() <= MAX_BATCH);
    let chunks = bytes
        .chunks(MAX_FRAME)
        .map(|part| {
            let mut b = Buffer::new(limits, MAX_FRAME).unwrap();
            b.spare_mut()[..part.len()].copy_from_slice(part);
            b.advance(part.len()).unwrap();
            b.freeze().unwrap()
        })
        .collect();
    Upload::from_parts(chunks, bytes.len())
}
pub(in crate::bbs_sync::relay) fn pair() -> (TlsStream, TlsStream) {
    let f = fixture();
    let c = PendingTls::client(
        &f.identities[0],
        f.contexts[0].clone(),
        authorized(),
        Cancellation::default(),
        f.now,
    )
    .unwrap();
    let s = PendingTls::server(
        &f.identities[1],
        f.contexts[1].clone(),
        authorized(),
        Cancellation::default(),
        c.statement(),
        f.now,
    )
    .unwrap();
    let cs = c.statement().clone();
    let ss = s.statement().clone();
    let mut c = c.accept(&ss, f.now).unwrap();
    let mut s = s.accept(&cs, f.now).unwrap();
    for _ in 0..32 {
        let transfer = |from: &mut TlsStream, to: &mut TlsStream| {
            let mut bytes = [0; 4096];
            let n = from.drain_tls(&mut bytes).unwrap();
            if n > 0 {
                assert_eq!(to.receive(&bytes[..n]).unwrap(), n);
            }
        };
        transfer(&mut c, &mut s);
        transfer(&mut s, &mut c);
        if c.handshake_complete().unwrap()
            && s.handshake_complete().unwrap()
            && !c.wants_write().unwrap()
            && !s.wants_write().unwrap()
        {
            return (c, s);
        }
    }
    panic!("handshake stalled");
}
fn send_window(f: &Fixture) -> SendWindow {
    SendWindow::new(
        f.contexts[0].clone(),
        "e".repeat(64),
        authorized(),
        Cancellation::default(),
    )
    .unwrap()
}
fn ack(f: &Fixture, through: u64) -> SignedAck {
    let c = &f.contexts[1];
    let body = format!("{{\"through\":\"{through}\",\"final\":false}}");
    let request = SignedRequest::sign(
        &f.identities[1],
        &c.peer.local_membership_id,
        Target::post(&c.origin, &c.peer.group_id, Route::Ack).unwrap(),
        Fields::Frame {
            boot: &c.boot,
            session: &"e".repeat(64),
            direction: Direction::ClientToServer,
            sequence: 0,
            final_flag: false,
        },
        body.as_bytes(),
        f.now,
    )
    .unwrap();
    SignedAck::decode(
        &serde_json::to_vec(&serde_json::json!({
            "proof":{"device":c.peer.local_device_id,"membership":c.peer.local_membership_id,
            "time":f.now,"nonce":"","boot":c.boot,"session":"e".repeat(64),
            "direction":"c2s","sequence":0,"final":false,
            "signature":request.headers().unwrap()["x-kota-relay-signature"]},"body":body,
        }))
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn block_reader_is_exact_finite_and_signs_the_existing_worker_golden_body() {
    let golden: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../relays/laughing-man-cloudflare/tests/fixtures/relay-proof-v1.json"
    )))
    .unwrap();
    let case = golden["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "send")
        .unwrap();
    let hex = case["bodyHex"].as_str().unwrap();
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    let limits = Limits::default();
    let f = fixture();
    // Split the independent vector even when it is smaller than one TLS block.
    let parts = bytes
        .chunks(3)
        .map(|chunk| {
            let mut b = Buffer::new(&limits, chunk.len()).unwrap();
            b.spare_mut().copy_from_slice(chunk);
            b.advance(chunk.len()).unwrap();
            b.freeze().unwrap()
        })
        .collect();
    let body = Upload::from_parts(parts, bytes.len());
    let mut window = send_window(&f);
    window.enqueue(&f.identities[0], body, f.now).unwrap();
    let batch = window.batch(0).unwrap();
    assert_eq!(
        serde_json::to_value(batch.request.headers().unwrap()).unwrap(),
        case["headers"]
    );
    assert_eq!(batch.payload.digest(), raw_sha256(&bytes));
    assert!(batch
        .request
        .matches_upload(Some(&Body::from(batch.payload.clone()))));
    assert!(!batch
        .request
        .matches_upload(Some(&fragmented(&limits, b"different").into())));
    let digest_ptr = batch.payload.digest().as_ptr();
    for size in [1, 2, 7, MAX_FRAME] {
        let body = Body::from(batch.payload.clone());
        let mut reader = body.reader();
        let mut output = Vec::new();
        let mut block = vec![0; size];
        assert_eq!(reader.read(&mut []).unwrap(), 0);
        loop {
            let n = reader.read(&mut block).unwrap();
            if n == 0 {
                break;
            }
            output.extend_from_slice(&block[..n]);
        }
        assert_eq!(reader.read(&mut block).unwrap(), 0);
        assert_eq!(output, bytes);
    }
    let retry = window.batch(0).unwrap();
    assert!(retry.payload.shares(&batch.payload));
    assert_eq!(retry.payload.digest().as_ptr(), digest_ptr);
    assert_eq!(
        retry.request.headers().unwrap(),
        batch.request.headers().unwrap()
    );
    assert!(matches!(
        window.enqueue(
            &f.identities[1],
            fragmented(&limits, b"wrong identity"),
            f.now
        ),
        Err(Error::Unauthorized)
    ));
    assert!(window.batch(1).is_err());
    let c = &f.contexts[0];
    let poll = SignedRequest::sign(
        &f.identities[0],
        &c.peer.local_membership_id,
        Target::poll(&c.origin, &c.peer.group_id, None).unwrap(),
        Fields::Read,
        &[],
        f.now,
    )
    .unwrap();
    assert!(!poll.matches_upload(Some(&Body::from(batch.payload))));
}

#[test]
fn tls_builder_fills_sixteen_blocks_without_a_body_copy_or_per_frame_flush() {
    let limits = Limits::default();
    let (mut sender, mut receiver) = pair();
    let mut builder = Builder::new(limits.clone());
    assert_eq!(builder.fill(&mut sender).unwrap(), 0);
    assert!(limits.reserve(APP_QUEUE_BYTES).is_ok());
    let plain = [19; MAX_FRAME];
    for _ in 0..16 {
        assert_eq!(sender.write_plaintext(&plain).unwrap(), plain.len());
        builder.fill(&mut sender).unwrap();
    }
    assert!(builder.full());
    let full = builder.finish().unwrap();
    assert_eq!(full.len(), MAX_BATCH);
    assert_eq!(full.parts.chunks.len(), 16);
    assert!(sender.wants_write().unwrap()); // TLS overhead continues next batch.
    let mut tail = Builder::new(limits.clone());
    assert!(tail.fill(&mut sender).unwrap() > 0);
    assert!(!sender.wants_write().unwrap());
    let tail = tail.finish().unwrap();
    assert_eq!(tail.parts.chunks.len(), 1);
    let charge = MAX_BATCH + MAX_FRAME;
    let rest = limits.reserve(APP_QUEUE_BYTES - charge).unwrap();
    assert!(matches!(limits.reserve(1), Err(Error::Busy)));
    let mut recovered = 0;
    for upload in [full, tail] {
        let body = Body::from(upload);
        let mut reader = body.reader();
        let mut input = [0; 997];
        loop {
            let n = reader.read(&mut input).unwrap();
            if n == 0 {
                break;
            }
            assert_eq!(receiver.receive(&input[..n]).unwrap(), n);
            let mut output = [0; MAX_FRAME];
            loop {
                let n = receiver.read_plaintext(&mut output).unwrap();
                if n == 0 {
                    break;
                }
                assert!(output[..n].iter().all(|b| *b == 19));
                recovered += n;
            }
        }
    }
    assert_eq!(recovered, 16 * MAX_FRAME);
    drop(rest);
    assert!(limits.reserve(APP_QUEUE_BYTES).is_ok());
}

#[test]
fn a_small_control_record_retains_one_block_and_busy_does_not_lose_tls_bytes() {
    let limits = Limits::default();
    let (mut sender, _receiver) = pair();
    let held = limits.reserve(APP_QUEUE_BYTES - MAX_FRAME).unwrap();
    let mut builder = Builder::new(limits.clone());
    sender.write_plaintext(b"small control").unwrap();
    assert!(builder.fill(&mut sender).unwrap() > 0);
    assert_eq!(builder.parts.len(), 0);
    assert!(builder.len() < MAX_FRAME);
    let before = builder.len();
    sender.write_plaintext(&[7; MAX_FRAME]).unwrap();
    assert!(builder.fill(&mut sender).unwrap() > 0);
    assert_eq!(builder.len(), MAX_FRAME);
    assert!(builder.len() > before);
    assert!(matches!(builder.fill(&mut sender), Err(Error::Busy)));
    assert!(sender.wants_write().unwrap());
    let body = builder.finish().unwrap();
    assert_eq!(body.len(), MAX_FRAME);
    drop(body);
    let mut remaining = Builder::new(limits.clone());
    assert!(remaining.fill(&mut sender).unwrap() > 0);
    assert!(!sender.wants_write().unwrap());
    assert!(remaining.len() < MAX_FRAME);
    drop(remaining);
    drop(held);
    assert!(limits.reserve(APP_QUEUE_BYTES).is_ok());
}

#[test]
fn signed_ack_releases_whole_batch_but_partial_http_reader_keeps_all_block_permits() {
    let limits = Limits::default();
    let f = fixture();
    let mut window = send_window(&f);
    let bytes = vec![31; MAX_BATCH];
    window
        .enqueue(&f.identities[0], fragmented(&limits, &bytes), f.now)
        .unwrap();
    window.submitted(0).unwrap();
    let retry = window.batch(0).unwrap();
    let body = Body::from(retry.payload);
    let rest = limits.reserve(APP_QUEUE_BYTES - MAX_BATCH).unwrap();
    let mut reader = body.reader();
    assert_eq!(reader.read(&mut [0; 13]).unwrap(), 13);
    assert!(matches!(limits.reserve(1), Err(Error::Busy)));
    let mut fake: serde_json::Value = serde_json::to_value(&ack(&f, 1)).unwrap();
    fake["body"] = "{\"through\":\"2\",\"final\":false}".into();
    let fake = SignedAck::decode(&serde_json::to_vec(&fake).unwrap()).unwrap();
    assert!(window.acknowledge(&fake, f.now).is_err());
    assert!(matches!(limits.reserve(1), Err(Error::Busy)));
    assert_eq!(
        window
            .acknowledge(&ack(&f, 1), f.now)
            .unwrap()
            .consumed_batches,
        1
    );
    assert!(matches!(limits.reserve(1), Err(Error::Busy)));
    drop(reader);
    drop(body);
    assert!(limits.reserve(MAX_BATCH).is_ok());
    drop(rest);
}
