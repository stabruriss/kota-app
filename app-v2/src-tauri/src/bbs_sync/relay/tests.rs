use super::{
    wire::{Declarations, DECLARATION_MS},
    *,
};
use crate::bbs_sync::{
    control::{Member, Membership, Role},
    transport::PeerIdentity,
    DeviceIdentity,
};

pub(super) struct Fixture {
    pub(super) identities: [DeviceIdentity; 2],
    pub(super) contexts: [SessionContext; 2],
    pub(super) now: u64,
}
pub(super) fn fixture() -> Fixture {
    let seeds = [
        "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
        "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
    ];
    let identities = seeds.map(|seed| {
        let seed: Vec<u8> = (0..seed.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&seed[i..i + 2], 16).unwrap())
            .collect();
        DeviceIdentity::from_seed(seed.try_into().unwrap()).unwrap()
    });
    let members: Vec<Member> = identities
        .iter()
        .enumerate()
        .map(|(i, id)| Member {
            device_id: id.device_id().unwrap(),
            public_key: id.public_key.clone(),
            name: "Fixture".into(),
            role: Role::Member,
            membership_id: ["membership-alpha-0001", "membership-bravo-0002"][i].into(),
            last_seen_at: 0,
            online: true,
        })
        .collect();
    assert!(members[0].device_id < members[1].device_id);
    let group = "group-fixture-0001";
    let instances = ["instance-client-0001", "instance-server-0001"];
    let contexts = [0, 1].map(|i| SessionContext {
        peer: PeerIdentity::current(
            &Membership {
                group_id: group.into(),
                worker_url: "https://worker.example".into(),
                role: Role::Member,
                membership_id: members[i].membership_id.clone(),
            },
            &members,
            &identities[i],
            &members[1 - i].device_id,
        )
        .unwrap(),
        origin: "https://worker.example".into(),
        wake: "a".repeat(64),
        boot: "b".repeat(64),
        nonce: "session-nonce-fixture-0001".into(),
        local_instance: instances[i].into(),
        remote_instance: instances[1 - i].into(),
    });
    Fixture {
        identities,
        contexts,
        now: 1_789_488_000_000,
    }
}
pub(super) fn statements(f: &Fixture) -> [SignedStatement; 2] {
    let client = SignedStatement::create(
        &f.identities[0],
        &f.contexts[0],
        "c".repeat(64),
        None,
        f.now,
    )
    .unwrap();
    let server = SignedStatement::create(
        &f.identities[1],
        &f.contexts[1],
        "d".repeat(64),
        Some(client.digest().unwrap()),
        f.now,
    )
    .unwrap();
    [client, server]
}

#[test]
fn tls_statements_match_independent_node_golden_bytes_and_signatures() {
    let fixture_json: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../relays/laughing-man-cloudflare/tests/fixtures/relay-proof-v1.json"
    )))
    .unwrap();
    let f = fixture();
    let pair = statements(&f);
    for (i, label) in ["client", "server"].iter().enumerate() {
        let signed = SignedStatement::decode(
            &serde_json::to_vec(&fixture_json["tls"]["declarations"][label]).unwrap(),
        )
        .unwrap();
        assert_eq!(pair[i].encode().unwrap(), signed.encode().unwrap());
        assert_eq!(
            signed.statement.signing_bytes().unwrap(),
            fixture_json["tls"][format!("{label}Message")]
                .as_str()
                .unwrap()
                .as_bytes()
        );
    }
    f.contexts[1].verify_remote(&pair[0], None, f.now).unwrap();
    f.contexts[0]
        .verify_remote(&pair[1], Some(&pair[0].digest().unwrap()), f.now)
        .unwrap();
    let declarations = Declarations {
        client: pair[0].clone(),
        server: pair[1].clone(),
    };
    assert_eq!(declarations.session_id().unwrap().len(), 64);
}

#[test]
fn valid_signatures_cannot_change_context_role_expiry_or_reply_binding() {
    let f = fixture();
    let pair = statements(&f);
    let mutations: Vec<Box<dyn Fn(&mut wire::Statement)>> = vec![
        Box::new(|s| s.origin = "https://alias.example".into()),
        Box::new(|s| s.group = "different-group-0001".into()),
        Box::new(|s| std::mem::swap(&mut s.from, &mut s.to)),
        Box::new(|s| s.from_membership = "old-membership-0001".into()),
        Box::new(|s| s.to_membership = "old-membership-0002".into()),
        Box::new(|s| s.from_instance = "old-instance-000001".into()),
        Box::new(|s| s.to_instance = "old-instance-000002".into()),
        Box::new(|s| s.boot = "old-boot-00000001".into()),
        Box::new(|s| s.wake = "old-wake-00000001".into()),
        Box::new(|s| s.nonce = "another-nonce-0001".into()),
        Box::new(|s| s.role = wire::Role::Client),
        Box::new(|s| s.reply_to = None),
        Box::new(|s| s.issued_at += 1),
        Box::new(|s| s.expires_at = s.issued_at),
        Box::new(|s| s.expires_at += 1),
        Box::new(|s| s.peer_version = 3),
        Box::new(|s| s.relay_version = 2),
    ];
    for (index, mutate) in mutations.into_iter().enumerate() {
        let mut changed = pair[1].clone();
        mutate(&mut changed.statement);
        changed.signature = f.identities[1]
            .sign(&changed.statement.signing_bytes().unwrap())
            .unwrap();
        assert!(
            f.contexts[0]
                .verify_remote(&changed, Some(&pair[0].digest().unwrap()), f.now)
                .is_err(),
            "mutation {index}"
        );
    }
    assert!(matches!(
        f.contexts[1].verify_remote(&pair[0], None, f.now + DECLARATION_MS),
        Err(Error::StaleSignature)
    ));
    let mut forged = pair[0].clone();
    forged.signature = f.identities[1]
        .sign(&forged.statement.signing_bytes().unwrap())
        .unwrap();
    assert!(matches!(
        f.contexts[1].verify_remote(&forged, None, f.now),
        Err(Error::Unauthorized)
    ));
    assert!(f.contexts[0].verify_remote(&pair[0], None, f.now).is_err());
}

#[test]
fn statement_decode_and_origin_reject_ambiguous_or_oversized_values() {
    let f = fixture();
    let pair = statements(&f);
    let mut value = serde_json::to_value(&pair[0]).unwrap();
    value["statement"]["unknown"] = true.into();
    assert!(SignedStatement::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    let mut missing = serde_json::to_value(&pair[0]).unwrap();
    missing["statement"]
        .as_object_mut()
        .unwrap()
        .remove("replyTo");
    assert!(SignedStatement::decode(&serde_json::to_vec(&missing).unwrap()).is_err());
    assert!(SignedStatement::decode(&vec![b' '; 8193]).is_err());
    let valid = String::from_utf8(pair[0].encode().unwrap()).unwrap();
    let duplicate = valid.replacen(
        "\"relayVersion\":1",
        "\"relayVersion\":1,\"relayVersion\":1",
        1,
    );
    assert!(SignedStatement::decode(duplicate.as_bytes()).is_err());
    for origin in [
        "http://worker.example",
        "https://worker.example/",
        "https://worker.example:443",
        "https://WORKER.example",
        "https://worker.example/path",
        "https://user@worker.example",
    ] {
        let mut context = f.contexts[0].clone();
        context.origin = origin.into();
        assert!(context.validate().is_err(), "{origin}");
    }
    let mut context = f.contexts[0].clone();
    context.nonce = "short".into();
    assert!(context.validate().is_err());
    assert!(SignedStatement::create(
        &f.identities[0],
        &f.contexts[0],
        "c".repeat(64),
        None,
        wire::MAX_SAFE_INTEGER
    )
    .is_err());
}
