use super::*;
mod coalesce;
use crate::bbs_sync::{
    control::{Member, Membership, Role as MemberRole},
    relay::{
        envelope::{self, Metadata},
        proof::SignedAck,
        tests::fixture,
        window::Buffer,
        PendingTls, SessionContext,
    },
    transport::{
        protocol::{encode_control, Control},
        Cancellation, FileIo, Limits, MembershipCheck, PeerIdentity, APP_QUEUE_BYTES,
    },
};
use serde_json::json;
use std::{
    io::Read,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

fn account_context() -> Context {
    let limits = Limits::default();
    Context {
        io: FileIo::start(limits.clone()).unwrap(),
        limits,
        shutdown: Cancellation::default(),
    }
}
fn pair(n: u8, active: Arc<AtomicBool>) -> (TlsStream, TlsStream) {
    pair_at(n, active, "https://worker.example")
}
fn pair_at(n: u8, active: Arc<AtomicBool>, origin: &str) -> (TlsStream, TlsStream) {
    let f = fixture();
    let identities = [
        f.identities[0].clone(),
        DeviceIdentity::from_seed([n; 32]).unwrap(),
    ];
    let members: Vec<_> = identities
        .iter()
        .enumerate()
        .map(|(i, d)| Member {
            device_id: d.device_id().unwrap(),
            public_key: d.public_key.clone(),
            name: "Fixture".into(),
            role: MemberRole::Member,
            membership_id: if i == 0 {
                "membership-account-0001".into()
            } else {
                format!("membership-remote-{n:04}")
            },
            last_seen_at: 0,
            online: true,
        })
        .collect();
    let instances = [
        "instance-account-0001".to_string(),
        format!("instance-remote-{n:04}"),
    ];
    let contexts = [0, 1].map(|i| SessionContext {
        peer: PeerIdentity::current(
            &Membership {
                group_id: f.contexts[0].peer.group_id.clone(),
                worker_url: f.contexts[0].origin.clone(),
                role: MemberRole::Member,
                membership_id: members[i].membership_id.clone(),
            },
            &members,
            &identities[i],
            &members[1 - i].device_id,
        )
        .unwrap(),
        origin: origin.into(),
        wake: format!("{n:064x}"),
        boot: f.contexts[0].boot.clone(),
        nonce: format!("session-nonce-{n:016}"),
        local_instance: instances[i].clone(),
        remote_instance: instances[1 - i].clone(),
    });
    let client = usize::from(contexts[0].role() == Role::Server);
    let auth: MembershipCheck = Arc::new(move || active.load(Ordering::Acquire));
    let c = PendingTls::client(
        &identities[client],
        contexts[client].clone(),
        auth.clone(),
        Cancellation::default(),
        f.now,
    )
    .unwrap();
    let s = PendingTls::server(
        &identities[1 - client],
        contexts[1 - client].clone(),
        auth,
        Cancellation::default(),
        c.statement(),
        f.now,
    )
    .unwrap();
    let cs = c.statement().clone();
    let ss = s.statement().clone();
    let mut c = c.accept(&ss, f.now).unwrap();
    let mut s = s.accept(&cs, f.now).unwrap();
    // Setup fixture only. Tested merged plaintext below is fed exclusively by
    // the charged response Packet and ReceiveWindow.
    for _ in 0..20 {
        let transfer = |from: &mut TlsStream, to: &mut TlsStream| {
            let mut bytes = [0; 4096];
            let n = from.drain_tls(&mut bytes).unwrap();
            if n > 0 {
                assert_eq!(to.receive(&bytes[..n]).unwrap(), n);
            }
        };
        transfer(&mut c, &mut s);
        transfer(&mut s, &mut c);
        if c.handshake_complete().unwrap() && s.handshake_complete().unwrap() {
            break;
        }
    }
    assert!(c.handshake_complete().unwrap() && s.handshake_complete().unwrap());
    if client == 0 {
        (c, s)
    } else {
        (s, c)
    }
}

#[tokio::test]
async fn merged_packet_services_other_peers_after_one_membership_is_revoked() {
    let f = fixture();
    let context = account_context();
    let mut pump = Pump::new(context.clone(), f.identities[0].clone());
    let mut remote = BTreeMap::new();
    let mut authorized = BTreeMap::new();
    for n in 1..=4 {
        let active = Arc::new(AtomicBool::new(true));
        let (local, mut other) = pair(n, active.clone());
        let id = pump.add(local).unwrap();
        let limits = Limits::default();
        let body = vec![n; 1024];
        let frame = encode_control(
            Control::Message {
                seq: 1,
                bytes: &body,
            },
            &limits,
        )
        .unwrap();
        let mut header = [0; 5];
        header[0] = 1;
        header[1..].copy_from_slice(&(frame.bytes.len() as u32).to_be_bytes());
        assert_eq!(other.write_plaintext(&header).unwrap(), 5);
        assert_eq!(
            other.write_plaintext(&frame.bytes).unwrap(),
            frame.bytes.len()
        );
        let mut builder = Builder::new(limits.clone());
        builder.fill(&mut other).unwrap();
        let body = super::super::upload::Body::from(builder.finish().unwrap());
        let mut bytes = Vec::new();
        body.reader().read_to_end(&mut bytes).unwrap();
        remote.insert(id.clone(), (n, bytes));
        authorized.insert(id, active);
    }
    let request = pump.read(ReceiveMode::Data, f.now).unwrap();
    let expected = request.receive_request().unwrap();
    let mut ciphertext = Vec::new();
    let mut items = Vec::new();
    for (id, cursor) in &expected.cursors {
        let bytes = &remote[id].1;
        items.push(
            json!({"session":id,"next":"1","consumed":"0","closed":false,
            "batches":[{"sequence":cursor.to_string(),"length":bytes.len()}],"ack":null}),
        );
        ciphertext.extend_from_slice(bytes);
    }
    let raw =
        envelope::tests::encode_json(&json!({"boot":expected.boot,"items":items}), &ciphertext);
    let mut buffer = Buffer::new(
        &context.limits,
        super::super::MAX_BATCH + super::super::MAX_ENVELOPE,
    )
    .unwrap();
    buffer.spare_mut()[..raw.len()].copy_from_slice(&raw);
    buffer.advance(raw.len()).unwrap();
    let body = buffer.freeze().unwrap();
    let packet = Metadata::parse(expected, &body)
        .unwrap()
        .into_packet(body, context.limits.reserve(16 * 1024).unwrap())
        .unwrap();
    pump.accept(packet, f.now).unwrap();
    let pending_before = pump.pending.as_ref().unwrap().packet.items().len();
    pump.remove(&"f".repeat(64));
    assert_eq!(
        pump.pending.as_ref().unwrap().packet.items().len(),
        pending_before,
        "late removal of an old session must not discard a new packet"
    );
    let bad = remote.keys().next().unwrap().clone();
    authorized[&bad].store(false, Ordering::Release);
    let activity = pump.step().unwrap();
    assert_eq!(activity.retired, vec![(bad.clone(), Error::Unauthorized)]);
    assert_eq!(activity.consumed_batches, 3);
    assert!(!pump.peers.contains_key(&bad));
    assert!(pump.data_ready());
    for (id, (byte, _)) in &remote {
        if id == &bad {
            continue;
        }
        let mut channels = pump.channels(id).unwrap().unwrap();
        let message = tokio::time::timeout(Duration::from_secs(2), channels.incoming.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(message.bytes(), vec![*byte; 1024]);
        message.acknowledge().unwrap();
        let ack = pump.acknowledgement(id, f.now).unwrap().unwrap();
        assert_eq!(ack.through, 1);
        // Keep remaining channels alive until their assertions complete.
        pump.peer_mut(id).unwrap().channels = Some(channels);
    }
    assert!(matches!(
        pump.acknowledgement(&bad, f.now),
        Err(Error::Closed)
    ));
    drop(pump);
    context.io.stop_and_wait().await;
}

#[tokio::test]
async fn account_context_and_four_slots_reject_aliases_and_retirement_prevents_late_reuse() {
    let f = fixture();
    let context = account_context();
    let mut pump = Pump::new(context.clone(), f.identities[0].clone());
    let active = || Arc::new(AtomicBool::new(true));
    let (first, _) = pair(1, active());
    let id = pump.add(first).unwrap();
    let (duplicate_remote, _) = pair(1, active());
    assert!(matches!(
        pump.add(duplicate_remote),
        Err(Error::Unauthorized)
    ));
    for n in 2..=4 {
        let (local, _) = pair(n, active());
        pump.add(local).unwrap();
    }
    let (extra, _) = pair(5, active());
    assert!(matches!(pump.add(extra), Err(Error::Busy)));
    assert_eq!(
        pump.read(ReceiveMode::Data, f.now)
            .unwrap()
            .receive_request()
            .unwrap()
            .cursors
            .len(),
        4
    );
    let cancel = pump.peer(&id).unwrap().tls.cancellation();
    pump.remove(&id);
    assert!(cancel.is_cancelled());
    assert!(matches!(pump.submitted(&id, 0), Err(Error::Closed)));
    assert!(matches!(pump.ack_submitted(&id, 0), Err(Error::Closed)));
    let (alias, _) = pair_at(5, active(), "https://alias.example");
    assert!(matches!(pump.add(alias), Err(Error::Unauthorized)));
    let (mut same_origin, _) = pair(5, active());
    // The authenticated context is private to TlsStream; group/origin cannot
    // be substituted at Pump::add. A different local identity is likewise rejected.
    assert_eq!(same_origin.context().origin, f.contexts[0].origin);
    let other_context = account_context();
    let mut other = Pump::new(
        other_context.clone(),
        DeviceIdentity::from_seed([99; 32]).unwrap(),
    );
    assert!(same_origin.handshake_complete().unwrap());
    assert!(matches!(other.add(same_origin), Err(Error::Unauthorized)));
    drop(other);
    other_context.io.stop_and_wait().await;
    context.shutdown.cancel();
    assert!(matches!(
        pump.read(ReceiveMode::Probe, f.now),
        Err(Error::Closed)
    ));
    assert!(matches!(
        pump.acknowledgement(&id, f.now),
        Err(Error::Closed)
    ));
    drop(pump);
    context.io.stop_and_wait().await;
    assert!(context.limits.reserve(APP_QUEUE_BYTES).is_ok());
}

#[tokio::test]
async fn only_two_closed_directions_retire_without_claiming_completion() {
    let f = fixture();
    let context = account_context();
    let mut pump = Pump::new(context.clone(), f.identities[0].clone());
    let (local, remote) = pair(1, Arc::new(AtomicBool::new(true)));
    let id = pump.add(local).unwrap();
    let cancel = pump.peer(&id).unwrap().tls.cancellation();
    let make_packet = |pump: &Pump, ack: Option<SignedAck>| {
        let request = pump.read(ReceiveMode::Receipts, f.now).unwrap();
        let expected = request.receive_request().unwrap();
        let raw = envelope::tests::encode_json(
            &json!({"boot":expected.boot,"items":[{
                "session":id,"next":"0","consumed":"0","closed":true,"batches":[],"ack":ack
            }]}),
            &[],
        );
        let mut buffer = Buffer::new(&context.limits, 8192).unwrap();
        buffer.spare_mut()[..raw.len()].copy_from_slice(&raw);
        buffer.advance(raw.len()).unwrap();
        let body = buffer.freeze().unwrap();
        Metadata::parse(expected, &body)
            .unwrap()
            .into_packet(body, context.limits.reserve(16384).unwrap())
            .unwrap()
    };
    let activity = pump.accept(make_packet(&pump, None), f.now).unwrap();
    assert!(activity.retired.is_empty());
    assert!(!cancel.is_cancelled());
    assert!(
        pump.peer(&id).unwrap().send.writable().unwrap(),
        "relay closed flag is not a peer receipt"
    );

    let c = remote.context();
    let direction = if c.role() == Role::Client {
        Direction::ServerToClient
    } else {
        Direction::ClientToServer
    };
    let body = r#"{"through":"0","final":true}"#;
    let request = SignedRequest::sign(
        &DeviceIdentity::from_seed([1; 32]).unwrap(),
        &c.peer.local_membership_id,
        Target::post(&c.origin, &c.peer.group_id, Route::Ack).unwrap(),
        Fields::Frame {
            boot: &c.boot,
            session: &id,
            direction,
            sequence: 0,
            final_flag: true,
        },
        body.as_bytes(),
        f.now,
    )
    .unwrap();
    let headers = request.headers().unwrap();
    let ack = SignedAck::decode(&serde_json::to_vec(&json!({"proof":{
        "device":c.peer.local_device_id,"membership":c.peer.local_membership_id,"time":f.now,"nonce":"",
        "boot":c.boot,"session":id,"direction":headers["x-kota-relay-direction"],"sequence":0,"final":true,
        "signature":headers["x-kota-relay-signature"]},"body":body})).unwrap()).unwrap();
    let mut forged = serde_json::to_value(&ack).unwrap();
    forged["proof"]["signature"] = f.identities[0].sign(b"not the recipient").unwrap().into();
    let forged = SignedAck::decode(&serde_json::to_vec(&forged).unwrap()).unwrap();
    assert!(matches!(
        pump.accept(make_packet(&pump, Some(forged)), f.now),
        Err(Error::Unauthorized)
    ));
    assert!(!cancel.is_cancelled());
    let packet = make_packet(&pump, Some(ack));
    let activity = pump.accept(packet, f.now).unwrap();
    assert!(activity.retired.is_empty());
    assert_eq!(activity.consumed_batches, 0);
    assert!(!cancel.is_cancelled());
    assert!(!pump.peer(&id).unwrap().send.writable().unwrap());
    assert!(!pump.peer(&id).unwrap().send.closed());
    assert!(pump.step().unwrap().retired.is_empty());
    assert!(pump.peer(&id).unwrap().send.closed());
    let final_ack = pump.final_acknowledgement(&id, f.now).unwrap().unwrap();
    assert!(
        pump.step().unwrap().retired.is_empty(),
        "unconfirmed local final is not closed"
    );
    pump.ack_submitted(&id, final_ack.sequence).unwrap();
    assert!(pump.final_acknowledgement(&id, f.now).unwrap().is_none());
    assert_eq!(
        pump.step().unwrap().retired,
        vec![(id.clone(), Error::Closed)]
    );
    assert!(cancel.is_cancelled());
    assert!(pump.peers.is_empty());
    assert!(pump.data_ready());
    assert!(matches!(pump.ack_submitted(&id, 0), Err(Error::Closed)));
    drop(pump);
    context.io.stop_and_wait().await;
}

#[tokio::test]
async fn normal_final_ack_is_signed_without_inventing_consumption() {
    let f = fixture();
    let context = account_context();
    let mut pump = Pump::new(context.clone(), f.identities[0].clone());
    let (local, _remote) = pair(1, Arc::new(AtomicBool::new(true)));
    let id = pump.add(local).unwrap();

    let ack = pump.final_acknowledgement(&id, f.now).unwrap().unwrap();
    assert_eq!(ack.through, 0);
    assert_eq!(ack.sequence, 0);
    assert_eq!(ack.body, r#"{"through":"0","final":true}"#);
    let headers = ack.request.headers().unwrap();
    assert_eq!(headers["x-kota-relay-final"], "1");
    assert!(!headers["x-kota-relay-signature"].is_empty());

    // A final close is one immutable operation; callers cannot manufacture a
    // second close with a new sequence while the first is outstanding.
    let again = pump.final_acknowledgement(&id, f.now).unwrap().unwrap();
    assert_eq!(again.sequence, ack.sequence);
    assert_eq!(again.body, ack.body);
    drop(pump);
    context.io.stop_and_wait().await;
}

#[tokio::test]
async fn final_releases_only_consumed_credit_before_reverse_payload_under_full_budget() {
    let f = fixture();
    let context = account_context();
    let mut pump = Pump::new(context.clone(), f.identities[0].clone());
    let (local, mut remote) = pair(1, Arc::new(AtomicBool::new(true)));
    let id = pump.add(local).unwrap();
    let mut b = Buffer::new(&context.limits, 64 * 1024).unwrap();
    b.spare_mut().fill(7);
    b.advance(64 * 1024).unwrap();
    let batch = pump
        .enqueue(&id, Upload::single(b.freeze().unwrap()).unwrap(), f.now)
        .unwrap();
    pump.submitted(&id, batch.sequence).unwrap();
    drop(batch);

    let remote_limits = Limits::default();
    let frame = encode_control(
        Control::Message {
            seq: 1,
            bytes: b"last reverse message",
        },
        &remote_limits,
    )
    .unwrap();
    let mut header = [0; 5];
    header[0] = 1;
    header[1..].copy_from_slice(&(frame.bytes.len() as u32).to_be_bytes());
    remote.write_plaintext(&header).unwrap();
    remote.write_plaintext(&frame.bytes).unwrap();
    let mut builder = Builder::new(remote_limits);
    builder.fill(&mut remote).unwrap();
    let mut ciphertext = Vec::new();
    super::super::upload::Body::from(builder.finish().unwrap())
        .reader()
        .read_to_end(&mut ciphertext)
        .unwrap();
    let c = remote.context();
    let body = r#"{"through":"1","final":true}"#;
    let ack = SignedRequest::sign(
        &DeviceIdentity::from_seed([1; 32]).unwrap(),
        &c.peer.local_membership_id,
        Target::post(&c.origin, &c.peer.group_id, Route::Ack).unwrap(),
        Fields::Frame {
            boot: &c.boot,
            session: &id,
            direction: if c.role() == Role::Client {
                Direction::ServerToClient
            } else {
                Direction::ClientToServer
            },
            sequence: 0,
            final_flag: true,
        },
        body.as_bytes(),
        f.now,
    )
    .unwrap();
    let h = ack.headers().unwrap();
    let ack = json!({"proof":{"device":c.peer.local_device_id,"membership":c.peer.local_membership_id,"time":f.now,"nonce":"",
        "boot":c.boot,"session":id,"direction":h["x-kota-relay-direction"],"sequence":0,"final":true,
        "signature":h["x-kota-relay-signature"]},"body":body});
    let request = pump.read(ReceiveMode::Data, f.now).unwrap();
    let expected = request.receive_request().unwrap();
    let raw = envelope::tests::encode_json(
        &json!({"boot":expected.boot,"items":[{
            "session":id,"next":"1","consumed":"0","closed":true,
            "batches":[{"sequence":"0","length":ciphertext.len()}],"ack":ack
        }]}),
        &ciphertext,
    );
    let mut b = Buffer::new(&context.limits, raw.len()).unwrap();
    b.spare_mut().copy_from_slice(&raw);
    b.advance(raw.len()).unwrap();
    let body = b.freeze().unwrap();
    let packet = Metadata::parse(expected, &body)
        .unwrap()
        .into_packet(body, context.limits.reserve(16384).unwrap())
        .unwrap();
    let mut held = Vec::new();
    for power in (0..20).rev() {
        while let Ok(p) = context.limits.reserve(1 << power) {
            held.push(p);
        }
    }
    assert!(matches!(context.limits.reserve(1), Err(Error::Busy)));
    let accepted = pump.accept(packet, f.now).unwrap();
    assert_eq!(accepted.consumed_batches, 1);
    assert!(accepted.retired.is_empty());
    assert!(
        !pump.peer(&id).unwrap().send.closed(),
        "cleanup must follow reverse TLS input"
    );
    assert!(
        context.limits.reserve(64 * 1024).is_ok(),
        "only the signed consumed prefix releases credit early"
    );
    let stepped = pump.step().unwrap();
    assert!(stepped.retired.is_empty());
    assert_eq!(stepped.tls_input, ciphertext.len());
    assert!(pump.peer(&id).unwrap().send.closed());
    let mut channels = pump.channels(&id).unwrap().unwrap();
    let delivery = tokio::time::timeout(Duration::from_secs(2), channels.incoming.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(delivery.bytes(), b"last reverse message");
    // Receiving a final is not local application completion or cancellation.
    assert!(!pump.peer(&id).unwrap().tls.cancellation().is_cancelled());
    drop(delivery);
    drop(channels);
    drop(pump);
    drop(held);
    context.io.stop_and_wait().await;
    assert!(context.limits.reserve(APP_QUEUE_BYTES).is_ok());
}
