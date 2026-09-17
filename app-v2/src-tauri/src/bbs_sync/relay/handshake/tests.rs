use super::*;
use crate::bbs_sync::relay::{
    discovery::Ready,
    proof::CanonicalJson,
    tests::{fixture, Fixture},
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

fn empty() -> HandshakeView {
    HandshakeView {
        ready: Ready {
            client: true,
            server: true,
        },
        client: None,
        server: None,
        session: None,
        closed: false,
    }
}
fn begin(f: &Fixture, index: usize, view: &HandshakeView, at: Instant) -> Handshake {
    Handshake::start(
        &f.identities[index],
        f.contexts[index].clone(),
        view,
        Arc::new(|| true),
        Cancellation::default(),
        f.now,
        at,
    )
    .unwrap()
    .unwrap()
}
fn pair(f: &Fixture, at: Instant) -> (Handshake, Handshake, HandshakeView) {
    let a = begin(f, 0, &empty(), at);
    let mut view = empty();
    view.client = Some(a.local.clone());
    let b = begin(f, 1, &view, at);
    view.server = Some(b.local.clone());
    view.session = Some(
        Declarations {
            client: a.local.signed.clone(),
            server: b.local.signed.clone(),
        }
        .session_id()
        .unwrap(),
    );
    (a, b, view)
}
fn opened(f: &Fixture, view: &HandshakeView) -> Vec<u8> {
    serde_json::to_vec(
        &serde_json::json!({"boot":f.contexts[0].boot,"session":view.session,"closed":false}),
    )
    .unwrap()
}
fn transfer(from: &mut TlsStream, to: &mut TlsStream) {
    let mut cipher = [0; 65_536 + 1024];
    let n = from.drain_tls(&mut cipher).unwrap();
    let mut offset = 0;
    while offset < n {
        offset += to.receive(&cipher[offset..n.min(offset + 7)]).unwrap();
    }
}

#[test]
fn declaration_discovery_consumes_ephemeral_keys_then_requires_real_mutual_tls() {
    let f = fixture();
    let at = Instant::now();
    let (mut a, mut b, view) = pair(&f, at);
    #[derive(Deserialize)]
    struct Body {
        client: CanonicalJson,
        server: Option<CanonicalJson>,
    }
    let (proof, raw) = a.request(&f.identities[0], f.now, at).unwrap();
    assert!(proof.matches_body(raw));
    let body: Body = serde_json::from_slice(raw).unwrap();
    assert_eq!(body.client.into_bytes(), a.local.bytes());
    assert!(body.server.is_none());
    let (proof, raw) = b.request(&f.identities[1], f.now, at).unwrap();
    assert!(proof.matches_body(raw));
    let body: Body = serde_json::from_slice(raw).unwrap();
    assert_eq!(body.client.into_bytes(), a.local.bytes());
    assert_eq!(body.server.unwrap().into_bytes(), b.local.bytes());
    let accepted =
        serde_json::to_vec(&serde_json::json!({"boot":f.contexts[0].boot,"offered":true})).unwrap();
    assert!(a.response(&accepted, f.now, at).unwrap().is_none());
    assert!(
        a.pending.is_some(),
        "relay acceptance cannot consume the endpoint key"
    );
    let mut server = b.response(&opened(&f, &view), f.now, at).unwrap().unwrap();
    let mut client = a
        .observe(&f.contexts[0].boot, &f.contexts[0].wake, &view, f.now, at)
        .unwrap()
        .unwrap();
    assert!(a.pending.is_none() && b.pending.is_none());
    assert!(!client.handshake_complete().unwrap() && !server.handshake_complete().unwrap());
    assert!(matches!(
        client.write_plaintext(b"no early payload"),
        Err(Error::Busy)
    ));
    for _ in 0..32 {
        transfer(&mut client, &mut server);
        transfer(&mut server, &mut client);
        if client.handshake_complete().unwrap() && server.handshake_complete().unwrap() {
            break;
        }
    }
    assert!(client.handshake_complete().unwrap() && server.handshake_complete().unwrap());
    assert_eq!(client.session_id, server.session_id);
    for forward in [true, false] {
        let (from, to) = if forward {
            (&mut client, &mut server)
        } else {
            (&mut server, &mut client)
        };
        assert_eq!(
            from.write_plaintext(b"verified endpoint payload").unwrap(),
            25
        );
        transfer(from, to);
        let mut plain = [0; 64];
        let n = to.read_plaintext(&mut plain).unwrap();
        assert_eq!(&plain[..n], b"verified endpoint payload");
    }
    assert!(matches!(
        a.observe(&f.contexts[0].boot, &f.contexts[0].wake, &view, f.now, at),
        Err(Error::Cancelled)
    ));
    assert!(matches!(
        b.response(&opened(&f, &view), f.now, at),
        Err(Error::Cancelled)
    ));
}

#[test]
fn server_echoes_original_client_order_instead_of_reserializing_it() {
    let f = fixture();
    let at = Instant::now();
    let a = begin(&f, 0, &empty(), at);
    let raw = format!(
        "{{\"signature\":{},\"statement\":{}}}",
        serde_json::to_string(&a.local.signed.signature).unwrap(),
        serde_json::to_string(&a.local.signed.statement).unwrap()
    );
    let mut view = empty();
    view.client = Some(serde_json::from_str(&raw).unwrap());
    let mut b = begin(&f, 1, &view, at);
    let (_, body) = b.request(&f.identities[1], f.now, at).unwrap();
    assert!(body.starts_with(format!("{{\"client\":{raw},\"server\":").as_bytes()));
    assert_ne!(view.client.as_ref().unwrap().bytes(), a.local.bytes());
}

#[test]
fn ready_gate_and_single_owner_do_not_adopt_old_offers_or_sessions() {
    let f = fixture();
    let at = Instant::now();
    for index in [0, 1] {
        let mut view = empty();
        view.ready.server = false;
        assert!(Handshake::start(
            &f.identities[index],
            f.contexts[index].clone(),
            &view,
            Arc::new(|| true),
            Cancellation::default(),
            f.now,
            at
        )
        .unwrap()
        .is_none());
    }
    assert!(Handshake::start(
        &f.identities[1],
        f.contexts[1].clone(),
        &empty(),
        Arc::new(|| true),
        Cancellation::default(),
        f.now,
        at
    )
    .unwrap()
    .is_none());
    let (a, _, mut view) = pair(&f, at);
    for index in [0, 1] {
        assert!(matches!(
            Handshake::start(
                &f.identities[index],
                f.contexts[index].clone(),
                &view,
                Arc::new(|| true),
                Cancellation::default(),
                f.now,
                at
            ),
            Err(Error::RelaySessionLost)
        ));
    }
    view = empty();
    view.client = Some(a.local);
    assert!(matches!(
        Handshake::start(
            &f.identities[0],
            f.contexts[0].clone(),
            &view,
            Arc::new(|| true),
            Cancellation::default(),
            f.now,
            at
        ),
        Err(Error::RelaySessionLost)
    ));
}

#[test]
fn poll_or_open_acceptance_does_not_renew_deadline_or_change_retry_body() {
    let f = fixture();
    let at = Instant::now();
    let mut a = begin(&f, 0, &empty(), at);
    let deadline = a.deadline();
    let first = a.request(&f.identities[0], f.now, at).unwrap().1.to_vec();
    let accepted =
        serde_json::to_vec(&serde_json::json!({"boot":f.contexts[0].boot,"offered":true})).unwrap();
    for seconds in [1, 5, 10, 19] {
        let later = at + Duration::from_secs(seconds);
        assert!(a
            .response(&accepted, f.now + seconds * 1000, later)
            .unwrap()
            .is_none());
        assert!(a
            .observe(
                &f.contexts[0].boot,
                &f.contexts[0].wake,
                &empty(),
                f.now + seconds * 1000,
                later
            )
            .unwrap()
            .is_none());
        let (proof, bytes) = a
            .request(&f.identities[0], f.now + seconds * 1000, later)
            .unwrap();
        assert_eq!(bytes, first);
        assert!(proof.matches_body(bytes));
        assert_eq!(a.deadline(), deadline);
    }
    assert!(matches!(
        a.request(&f.identities[0], f.now + 20_000, deadline),
        Err(Error::Timeout)
    ));
    assert!(a.pending.is_none());
    assert!(
        matches!(
            a.request(&f.identities[0], f.now, at),
            Err(Error::Cancelled)
        ),
        "earlier timestamp cannot revive failed attempt"
    );
}

#[test]
fn cancellation_and_membership_revocation_are_terminal_before_poll_or_request() {
    let f = fixture();
    let at = Instant::now();
    for cancelled in [true, false] {
        let active = Arc::new(AtomicBool::new(true));
        let check = active.clone();
        let cancel = Cancellation::default();
        let mut a = Handshake::start(
            &f.identities[0],
            f.contexts[0].clone(),
            &empty(),
            Arc::new(move || check.load(Ordering::SeqCst)),
            cancel.clone(),
            f.now,
            at,
        )
        .unwrap()
        .unwrap();
        if cancelled {
            cancel.cancel();
        } else {
            active.store(false, Ordering::SeqCst);
        }
        let error = a.request(&f.identities[0], f.now, at).err().unwrap();
        assert_eq!(
            error,
            if cancelled {
                Error::Cancelled
            } else {
                Error::Unauthorized
            }
        );
        active.store(true, Ordering::SeqCst);
        assert!(a.pending.is_none());
        assert!(matches!(
            a.observe(
                &f.contexts[0].boot,
                &f.contexts[0].wake,
                &empty(),
                f.now,
                at
            ),
            Err(Error::Cancelled)
        ));
    }
}

#[test]
fn valid_relay_hints_cannot_replace_signature_bindings_or_final_session() {
    let f = fixture();
    let at = Instant::now();
    for case in 0..9 {
        let (mut a, _, mut view) = pair(&f, at);
        match case {
            0 => view.server.as_mut().unwrap().signed.signature = "AA==".into(),
            1 => view.server.as_mut().unwrap().signed.statement.group = "other-group-0001".into(),
            2 => {
                view.server
                    .as_mut()
                    .unwrap()
                    .signed
                    .statement
                    .from_membership = "other-membership-0001".into()
            }
            3 => view.server.as_mut().unwrap().signed.statement.role = Role::Client,
            4 => view.server.as_mut().unwrap().signed.statement.boot = "c".repeat(64),
            5 => view.server.as_mut().unwrap().signed.statement.reply_to = Some("c".repeat(64)),
            6 => view.session = Some("c".repeat(64)),
            7 => view.client = Some(begin(&f, 0, &empty(), at).local),
            8 => view.server.as_mut().unwrap().signed.statement.expires_at = f.now,
            _ => unreachable!(),
        }
        if (1..=5).contains(&case) {
            let s = &mut view.server.as_mut().unwrap().signed;
            s.signature = f.identities[1]
                .sign(&s.statement.signing_bytes().unwrap())
                .unwrap();
        }
        assert!(
            a.observe(&f.contexts[0].boot, &f.contexts[0].wake, &view, f.now, at)
                .is_err(),
            "case {case}"
        );
        assert!(a.pending.is_none());
        assert!(matches!(
            a.request(&f.identities[0], f.now, at),
            Err(Error::Cancelled)
        ));
    }
    let (a, _, mut view) = pair(&f, at);
    view.session = None;
    view.server = None;
    view.client.as_mut().unwrap().signed.signature = "AA==".into();
    assert!(matches!(
        Handshake::start(
            &f.identities[1],
            f.contexts[1].clone(),
            &view,
            Arc::new(|| true),
            Cancellation::default(),
            f.now,
            at
        ),
        Err(Error::Unauthorized)
    ));
    assert!(!a.ended);
}

#[test]
fn changed_boot_wake_closed_or_malformed_receipt_cannot_revive_an_attempt() {
    let f = fixture();
    let at = Instant::now();
    for case in 0..3 {
        let (mut a, _, mut view) = pair(&f, at);
        let boot = if case == 0 {
            "different-boot-0001"
        } else {
            &f.contexts[0].boot
        };
        let wake = if case == 1 {
            "different-wake-0001"
        } else {
            &f.contexts[0].wake
        };
        view.closed = case == 2;
        assert!(matches!(
            a.observe(boot, wake, &view, f.now, at),
            Err(Error::RelaySessionLost)
        ));
        assert!(matches!(
            a.observe(&f.contexts[0].boot, &f.contexts[0].wake, &view, f.now, at),
            Err(Error::Cancelled)
        ));
    }
    for reply in [
        serde_json::json!({"boot":f.contexts[0].boot,"offered":true,"resetAtUtc":123}),
        serde_json::json!({"boot":f.contexts[0].boot,"offered":false}),
        serde_json::json!({"boot":"other-boot-0001","offered":true}),
    ] {
        let mut a = begin(&f, 0, &empty(), at);
        assert!(a
            .response(&serde_json::to_vec(&reply).unwrap(), f.now, at)
            .is_err());
        assert!(a.ended && a.pending.is_none());
    }
    let (_, mut b, mut view) = pair(&f, at);
    view.session = Some("e".repeat(64));
    assert!(matches!(
        b.response(&opened(&f, &view), f.now, at),
        Err(Error::Protocol)
    ));
    assert!(b.pending.is_none());
}

#[test]
fn ready_body_binds_ordered_instances_and_only_current_identity_can_sign() {
    let f = fixture();
    let (a, a_bytes) = ready(&f.identities[0], &f.contexts[0], f.now).unwrap();
    let (b, b_bytes) = ready(&f.identities[1], &f.contexts[1], f.now).unwrap();
    assert_eq!(a_bytes, b_bytes);
    assert!(a.matches_body(&a_bytes) && b.matches_body(&b_bytes));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&a_bytes).unwrap(),
        serde_json::json!({"wake":f.contexts[0].wake,"clientInstance":"instance-client-0001","serverInstance":"instance-server-0001"})
    );
    assert!(matches!(
        ready(&f.identities[1], &f.contexts[0], f.now),
        Err(Error::Unauthorized)
    ));
}
