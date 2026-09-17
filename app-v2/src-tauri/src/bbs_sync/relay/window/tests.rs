use super::*;
use crate::bbs_sync::{
    relay::{
        proof::{Direction, Fields, Route, SignedRequest, Target},
        tests::{fixture, Fixture},
        PendingTls,
    },
    transport::{APP_QUEUE_BYTES, MAX_FRAME},
};

fn guard() -> MembershipCheck {
    Arc::new(|| true)
}
fn payload(limits: &Limits, bytes: &[u8]) -> Payload {
    let mut buffer = Buffer::new(limits, bytes.len()).unwrap();
    buffer.spare_mut().copy_from_slice(bytes);
    buffer.advance(bytes.len()).unwrap();
    buffer.freeze().unwrap()
}
fn sender(f: &Fixture, session: &str) -> SendWindow {
    SendWindow::new(
        f.contexts[0].clone(),
        session.into(),
        guard(),
        Cancellation::default(),
    )
    .unwrap()
}
fn ack(f: &Fixture, session: &str, operation: u64, through: u64, final_flag: bool) -> SignedAck {
    let body = format!("{{\"through\":\"{through}\",\"final\":{final_flag}}}");
    ack_bytes(f, session, operation, final_flag, &body)
}
fn ack_bytes(
    f: &Fixture,
    session: &str,
    operation: u64,
    final_flag: bool,
    body: &str,
) -> SignedAck {
    let context = &f.contexts[1];
    let request = SignedRequest::sign(
        &f.identities[1],
        &context.peer.local_membership_id,
        Target::post(&context.origin, &context.peer.group_id, Route::Ack).unwrap(),
        Fields::Frame {
            boot: &context.boot,
            session,
            direction: Direction::ClientToServer,
            sequence: operation,
            final_flag,
        },
        body.as_bytes(),
        f.now,
    )
    .unwrap();
    let h = request.headers().unwrap();
    SignedAck::decode(&serde_json::to_vec(&serde_json::json!({
        "proof": {"device":context.peer.local_device_id,"membership":context.peer.local_membership_id,
        "time":f.now,"nonce":"","boot":context.boot,"session":session,"direction":"c2s",
        "sequence":operation,"final":final_flag,"signature":h["x-kota-relay-signature"]},
        "body":body,
    })).unwrap()).unwrap()
}

#[test]
fn retained_batches_share_one_charge_and_http_ownership_survives_cancel() {
    let limits = Limits::default();
    let f = fixture();
    let session = "e".repeat(64);
    let mut a = sender(&f, &session);
    let mut b = sender(&f, &session);
    for window in [&mut a, &mut b] {
        for seq in 0..2 {
            let mut buf = Buffer::new(&limits, MAX_BATCH).unwrap();
            buf.advance(MAX_BATCH).unwrap();
            assert_eq!(
                window
                    .enqueue(
                        &f.identities[0],
                        Upload::single(buf.freeze().unwrap()).unwrap(),
                        f.now
                    )
                    .unwrap(),
                seq
            );
        }
        assert!(!window.writable().unwrap());
    }
    assert!(matches!(limits.reserve(1), Err(Error::Busy)));
    let in_http = a.batch(0).unwrap();
    let retry = a.batch(0).unwrap();
    assert!(in_http.payload.shares(&retry.payload));
    a.cancel.cancel();
    assert!(matches!(a.batch(0), Err(Error::Cancelled)));
    drop(a);
    drop(b);
    let remainder = limits.reserve(APP_QUEUE_BYTES - MAX_BATCH).unwrap();
    assert!(matches!(limits.reserve(1), Err(Error::Busy)));
    drop(in_http);
    assert!(matches!(limits.reserve(1), Err(Error::Busy)));
    drop(retry);
    drop(remainder);
    assert!(limits.reserve(APP_QUEUE_BYTES).is_ok());
    assert!(matches!(
        Buffer::new(&limits, MAX_BATCH + super::super::MAX_ENVELOPE + 1),
        Err(Error::Protocol)
    ));
    let mut buf = Buffer::new(&limits, 1).unwrap();
    assert!(matches!(buf.advance(2), Err(Error::Protocol)));
    assert!(matches!(buf.freeze(), Err(Error::Protocol)));
    assert!(limits.reserve(APP_QUEUE_BYTES).is_ok());
}

#[test]
fn uploads_and_lost_replies_do_not_release_credit_only_signed_contiguous_receipts_do() {
    let limits = Limits::default();
    let f = fixture();
    let session = "e".repeat(64);
    let mut a = sender(&f, &session);
    a.enqueue(
        &f.identities[0],
        Upload::single(payload(&limits, b"first immutable ciphertext")).unwrap(),
        f.now,
    )
    .unwrap();
    a.enqueue(
        &f.identities[0],
        Upload::single(payload(&limits, b"second immutable ciphertext")).unwrap(),
        f.now,
    )
    .unwrap();
    let retry = a.batch(0).unwrap();
    assert!(matches!(a.submitted(1), Err(Error::Protocol)));
    assert!(matches!(
        a.acknowledge(&ack(&f, &session, 0, 1, false), f.now),
        Err(Error::Protocol)
    ));
    a.submitted(0).unwrap();
    // An HTTP timeout or even a successful upload response changes no credit.
    for receipt in [
        br#"{"accepted":true,"consumed":false}"#.as_slice(),
        br#"{"accepted":false,"consumed":true}"#,
    ] {
        super::super::response::mutation(receipt, true).unwrap();
        assert_eq!(a.consumed, 0);
        assert_eq!(a.batches.len(), 2);
        assert!(a.batch(0).unwrap().payload.shares(&retry.payload));
    }
    assert_eq!(
        super::super::response::mutation(
            br#"{"accepted":true,"consumed":false,"through":"2"}"#,
            true
        ),
        Err(Error::Protocol)
    );
    assert!(!a.writable().unwrap());
    assert_eq!(a.consumed, 0);
    a.submitted(0).unwrap();
    assert!(a.batch(0).unwrap().payload.shares(&retry.payload));
    a.submitted(1).unwrap();
    assert!(matches!(
        a.acknowledge(&ack(&f, &session, 0, 3, false), f.now),
        Err(Error::Protocol)
    ));
    let first = ack(&f, &session, 0, 1, false);
    assert_eq!(
        a.acknowledge(&first, f.now).unwrap(),
        ReceiptEffect {
            consumed_batches: 1,
            closed: false
        }
    );
    assert_eq!(a.acknowledge(&first, f.now).unwrap().consumed_batches, 0);
    // Same operation and semantic values, but different compact signed bytes.
    // Worker rejects this as a sequence conflict; the endpoint must too.
    assert!(matches!(
        a.acknowledge(
            &ack_bytes(&f, &session, 0, false, r#"{"final":false,"through":"1"}"#),
            f.now
        ),
        Err(Error::Protocol)
    ));
    assert!(matches!(
        a.acknowledge(&ack(&f, &session, 0, 2, false), f.now),
        Err(Error::Protocol)
    ));
    // Relay may forward a later cumulative operation without the intermediate one.
    assert_eq!(
        a.acknowledge(&ack(&f, &session, 2, 2, false), f.now)
            .unwrap()
            .consumed_batches,
        1
    );
    assert_eq!(a.acknowledge(&first, f.now).unwrap().consumed_batches, 0);
    assert!(matches!(
        a.acknowledge(&ack(&f, &session, 3, 1, false), f.now),
        Err(Error::Protocol)
    ));
    assert!(a.batches.is_empty());
    assert!(a.writable().unwrap());
}

#[test]
fn signed_final_can_stop_without_acknowledging_or_completing_unsent_content() {
    let limits = Limits::default();
    let f = fixture();
    let session = "e".repeat(64);
    let mut a = sender(&f, &session);
    a.enqueue(
        &f.identities[0],
        Upload::single(payload(&limits, b"unconsumed")).unwrap(),
        f.now,
    )
    .unwrap();
    a.submitted(0).unwrap();
    let final_ack = ack(&f, &session, 0, 0, true);
    assert_eq!(
        a.acknowledge(&final_ack, f.now).unwrap(),
        ReceiptEffect {
            consumed_batches: 0,
            closed: true
        }
    );
    assert_eq!(a.consumed, 0);
    assert_eq!(
        a.batches.len(),
        1,
        "terminal discard waits for reverse payload"
    );
    assert!(!a.closed());
    assert!(!a.writable().unwrap());
    a.finish_close().unwrap();
    assert!(a.batches.is_empty());
    assert!(!a.writable().unwrap());
    assert_eq!(
        a.acknowledge(&final_ack, f.now).unwrap().consumed_batches,
        0
    );
    assert!(matches!(
        a.enqueue(
            &f.identities[0],
            Upload::single(payload(&limits, b"cannot reopen")).unwrap(),
            f.now
        ),
        Err(Error::Closed)
    ));
    assert!(matches!(
        a.acknowledge(&ack(&f, &session, 1, 1, false), f.now),
        Err(Error::Protocol)
    ));
    assert!(limits.reserve(APP_QUEUE_BYTES).is_ok());
}

fn pair() -> (TlsStream, TlsStream) {
    let f = fixture();
    let c = PendingTls::client(
        &f.identities[0],
        f.contexts[0].clone(),
        guard(),
        Cancellation::default(),
        f.now,
    )
    .unwrap();
    let s = PendingTls::server(
        &f.identities[1],
        f.contexts[1].clone(),
        guard(),
        Cancellation::default(),
        c.statement(),
        f.now,
    )
    .unwrap();
    let cs = c.statement().clone();
    let ss = s.statement().clone();
    let mut c = c.accept(&ss, f.now).unwrap();
    let mut s = s.accept(&cs, f.now).unwrap();
    let transfer = |from: &mut TlsStream, to: &mut TlsStream| {
        let mut out = [0; 4096];
        let len = from.drain_tls(&mut out).unwrap();
        if len > 0 {
            assert_eq!(to.receive(&out[..len]).unwrap(), len);
        }
    };
    for _ in 0..16 {
        transfer(&mut c, &mut s);
        transfer(&mut s, &mut c);
        if c.handshake_complete().unwrap() && s.handshake_complete().unwrap() {
            return (c, s);
        }
    }
    panic!("handshake stalled");
}

#[test]
fn cancelling_partial_tls_from_an_envelope_keeps_parent_charge_until_last_owner_drops() {
    use crate::bbs_sync::relay::{
        envelope::{tests::encode_json, Metadata},
        proof::{ReceiveMode, ReceiveRequest},
        MAX_ENVELOPE,
    };
    let limits = Limits::default();
    let (mut client, mut server) = pair();
    for _ in 0..3 {
        client.write_plaintext(&[19; MAX_FRAME]).unwrap();
    }
    let mut ciphertext = vec![0; MAX_BATCH];
    let n = client.drain_tls(&mut ciphertext).unwrap();
    ciphertext.truncate(n);
    let request = ReceiveRequest {
        boot: "b".repeat(64),
        mode: ReceiveMode::Data,
        cursors: vec![(server.session_id.clone(), 0)],
    };
    let bytes = encode_json(
        &serde_json::json!({"boot":request.boot,"items":[{
            "session":server.session_id,"next":"1","consumed":"0","closed":false,"ack":null,
            "batches":[{"sequence":"0","length":n}],
        }]}),
        &ciphertext,
    );
    let capacity = MAX_BATCH + MAX_ENVELOPE;
    let mut buffer = Buffer::new(&limits, capacity).unwrap();
    buffer.spare_mut()[..bytes.len()].copy_from_slice(&bytes);
    buffer.advance(bytes.len()).unwrap();
    let body = buffer.freeze().unwrap();
    let packet = Metadata::parse(&request, &body)
        .unwrap()
        .into_packet(body, limits.reserve(2 * MAX_ENVELOPE).unwrap())
        .unwrap();
    let rest = limits
        .reserve(APP_QUEUE_BYTES - capacity - 2 * MAX_ENVELOPE)
        .unwrap();
    let cancel = Cancellation::default();
    let mut receiver =
        ReceiveWindow::new(server.session_id.clone(), guard(), cancel.clone()).unwrap();
    receiver
        .offer(packet.items()[0].batches()[0].clone())
        .unwrap();
    let mut fed = 0;
    for _ in 0..8 {
        let n = receiver.feed(&mut server).unwrap();
        fed += n;
        if n == 0 {
            break;
        }
    }
    assert!(fed > 0 && fed < ciphertext.len());
    assert_eq!(receiver.cursor(), 0);
    drop(packet); // Drop decoded metadata, not the still-in-use HTTP allocation.
    let metadata_free = limits.reserve(2 * MAX_ENVELOPE).unwrap();
    cancel.cancel();
    assert!(matches!(receiver.feed(&mut server), Err(Error::Cancelled)));
    assert!(matches!(limits.reserve(1), Err(Error::Busy)));
    drop(receiver);
    assert!(limits.reserve(capacity).is_ok());
    drop(rest);
    drop(metadata_free);
    assert!(limits.reserve(APP_QUEUE_BYTES).is_ok());
}

#[test]
fn receive_retries_gaps_and_half_records_never_replay_bytes_into_tls() {
    let limits = Limits::default();
    let (mut c, mut s) = pair();
    let cancel = Cancellation::default();
    let mut receiver = ReceiveWindow::new(s.session_id.clone(), guard(), cancel.clone()).unwrap();
    let plain = b"a record split across two independent HTTP batches";
    assert_eq!(c.write_plaintext(plain).unwrap(), plain.len());
    let mut bytes = [0; 1024];
    let n = c.drain_tls(&mut bytes).unwrap();
    let first = Batch {
        sequence: 0,
        payload: payload(&limits, &bytes[..7]),
    };
    let second = Batch {
        sequence: 1,
        payload: payload(&limits, &bytes[7..n]),
    };
    assert_eq!(receiver.offer(second.clone()).unwrap(), Offered::Gap);
    assert_eq!(receiver.cursor(), 0);
    assert_eq!(receiver.offer(first.clone()).unwrap(), Offered::Accepted);
    assert_eq!(receiver.offer(first.clone()).unwrap(), Offered::Duplicate);
    assert_eq!(receiver.feed(&mut s).unwrap(), 7);
    assert_eq!(receiver.cursor(), 1); // TLS holds only the unfinished record.
    assert_eq!(receiver.offer(first).unwrap(), Offered::Duplicate);
    assert_eq!(receiver.offer(second.clone()).unwrap(), Offered::Accepted);
    assert_eq!(receiver.feed(&mut s).unwrap(), n - 7);
    assert_eq!(receiver.cursor(), 2);
    let mut out = [0; 1024];
    let count = s.read_plaintext(&mut out).unwrap();
    assert_eq!(&out[..count], plain);
    assert_eq!(receiver.offer(second).unwrap(), Offered::Duplicate);
    assert_eq!(receiver.feed(&mut s).unwrap(), 0);
    assert_eq!(s.read_plaintext(&mut out).unwrap(), 0);
    cancel.cancel();
    assert!(matches!(receiver.feed(&mut s), Err(Error::Cancelled)));
}

#[test]
fn receive_keeps_partial_batch_credit_while_tls_plaintext_consumer_is_stopped() {
    let limits = Limits::default();
    let (mut c, mut s) = pair();
    for _ in 0..3 {
        assert_eq!(c.write_plaintext(&[42; MAX_FRAME]).unwrap(), MAX_FRAME);
    }
    let mut buffer = Buffer::new(&limits, MAX_BATCH).unwrap();
    let n = c.drain_tls(buffer.spare_mut()).unwrap();
    buffer.advance(n).unwrap();
    let batch = Batch {
        sequence: 0,
        payload: buffer.freeze().unwrap(),
    };
    let mut receiver =
        ReceiveWindow::new(s.session_id.clone(), guard(), Cancellation::default()).unwrap();
    receiver.offer(batch.clone()).unwrap();
    let mut fed = 0;
    for _ in 0..8 {
        let n = receiver.feed(&mut s).unwrap();
        fed += n;
        if n == 0 {
            break;
        }
    }
    assert!(fed > 0 && fed < batch.payload.bytes().len());
    assert_eq!(receiver.cursor(), 0);
    assert_eq!(receiver.offer(batch.clone()).unwrap(), Offered::Duplicate);
    let bad = Batch {
        sequence: 0,
        payload: payload(&limits, b"different retained sequence"),
    };
    assert!(matches!(receiver.offer(bad), Err(Error::Integrity)));
    let mut received = 0;
    let mut out = [0; MAX_FRAME];
    for _ in 0..12 {
        received += s.read_plaintext(&mut out).unwrap();
        receiver.feed(&mut s).unwrap();
        if receiver.cursor() == 1 {
            received += s.read_plaintext(&mut out).unwrap();
            break;
        }
    }
    assert_eq!(receiver.cursor(), 1);
    assert_eq!(received, 3 * MAX_FRAME);
}
