use super::*;
use crate::bbs_sync::relay::{tests::fixture, wire::Declarations};
use serde_json::{json, Value};

// Unmodified outputs captured from the real BbsGroup.fetch chain, with actual
// Ed25519 signatures from the public RFC seeds. No production account was used.
const RECORDED: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../relays/laughing-man-cloudflare/tests/fixtures/relay-discovery-v1.json"
));

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Recorded {
    now: u64,
    initial: CanonicalJson,
    offered: CanonicalJson,
    opened: CanonicalJson,
    open_response: CanonicalJson,
}

#[test]
fn actual_worker_fetch_pages_preserve_and_verify_both_declarations() {
    let f = fixture();
    let recorded: Recorded = serde_json::from_str(RECORDED).unwrap();
    assert_eq!(recorded.now, f.now);
    let decode = |raw: CanonicalJson, local: usize| {
        Page::decode(
            &raw.into_bytes(),
            &f.contexts[local].peer.group_id,
            &f.contexts[local].peer.local_device_id,
            None,
        )
        .unwrap()
    };
    let initial = decode(recorded.initial, 0);
    let offered = decode(recorded.offered, 1);
    let opened = decode(recorded.opened, 0);
    assert_eq!(initial.items.len(), 3);
    let before = initial
        .items
        .iter()
        .find(|i| i.device == f.contexts[0].peer.remote_device_id)
        .unwrap();
    assert!(before.handshake.as_ref().unwrap().client.is_none());
    assert!(before.handshake.as_ref().unwrap().ready.client);
    assert!(before.handshake.as_ref().unwrap().ready.server);
    let offer = offered
        .items
        .iter()
        .find(|i| i.device == f.contexts[1].peer.remote_device_id)
        .unwrap();
    let answer = opened
        .items
        .iter()
        .find(|i| i.device == f.contexts[0].peer.remote_device_id)
        .unwrap();
    let client = offer.handshake.as_ref().unwrap().client.as_ref().unwrap();
    let view = answer.handshake.as_ref().unwrap();
    let server = view.server.as_ref().unwrap();
    assert_eq!(view.client.as_ref().unwrap().bytes(), client.bytes());
    for (local, row, page) in [(0, answer, &opened), (1, offer, &offered)] {
        let old = &f.contexts[local];
        let context = row
            .wake
            .as_ref()
            .unwrap()
            .context(
                old.peer.clone(),
                old.origin.clone(),
                page.boot.clone(),
                old.nonce.clone(),
                &old.local_instance,
                &old.remote_instance,
                f.now,
            )
            .unwrap();
        if local == 0 {
            context
                .verify_remote(
                    &server.signed,
                    Some(&client.signed.digest().unwrap()),
                    f.now,
                )
                .unwrap();
        } else {
            context.verify_remote(&client.signed, None, f.now).unwrap();
        }
    }
    let id = Declarations {
        client: client.signed.clone(),
        server: server.signed.clone(),
    }
    .session_id()
    .unwrap();
    let response: Value = serde_json::from_slice(&recorded.open_response.into_bytes()).unwrap();
    assert_eq!(response["session"], id);
    assert_eq!(view.session.as_deref(), Some(id.as_str()));
    // Typed projections must not accidentally normalize/reorder the opaque echo.
    assert_eq!(client.bytes(), client.signed.encode().unwrap());
    assert!(offered
        .items
        .iter()
        .filter(|i| i.device != f.contexts[1].peer.remote_device_id)
        .all(|i| i.handshake.is_none()));
}

#[test]
fn poll_shape_cursor_and_pair_fail_closed_without_partial_projection() {
    let f = fixture();
    let good: Value = serde_json::from_str(RECORDED).unwrap();
    let decode = |value: &Value, after: Option<&str>| {
        Page::decode(
            &serde_json::to_vec(value).unwrap(),
            &f.contexts[0].peer.group_id,
            &f.contexts[0].peer.local_device_id,
            after,
        )
    };
    let mut mutations = Vec::new();
    let mut v = good["initial"].clone();
    v["extra"] = true.into();
    mutations.push(v);
    let mut v = good["initial"].clone();
    v.as_object_mut().unwrap().remove("next");
    mutations.push(v);
    let mut v = good["initial"].clone();
    v["items"][0].as_object_mut().unwrap().remove("handshake");
    mutations.push(v);
    let mut v = good["initial"].clone();
    v["items"][1]["handshake"]
        .as_object_mut()
        .unwrap()
        .remove("client");
    mutations.push(v);
    let mut v = good["initial"].clone();
    v["items"][1]["wake"]["server"] = f.contexts[0].peer.local_device_id.clone().into();
    mutations.push(v);
    let mut v = good["initial"].clone();
    v["items"][1]["currentWake"] = "f".repeat(64).into();
    mutations.push(v);
    let mut v = good["initial"].clone();
    v["items"][1]["wake"]["expiresAt"] = (f.now + 120_001).into();
    mutations.push(v);
    let mut v = good["initial"].clone();
    v["items"][1]["wake"] = Value::Null;
    mutations.push(v);
    let mut v = good["initial"].clone();
    v["items"].as_array_mut().unwrap().swap(0, 1);
    mutations.push(v);
    let mut v = good["initial"].clone();
    v["items"][2] = v["items"][1].clone();
    mutations.push(v);
    let mut v = good["initial"].clone();
    v["next"] = "f".repeat(64).into();
    mutations.push(v);
    let mut v = good["initial"].clone();
    v["items"] = json!([]);
    v["next"] = "f".repeat(64).into();
    mutations.push(v);
    let mut v = good["initial"].clone();
    v["items"] = Value::Array(vec![v["items"][0].clone(); 17]);
    mutations.push(v);
    let mut v = good["opened"].clone();
    v["items"][1]["handshake"]["closed"] = true.into();
    mutations.push(v);
    let mut v = good["opened"].clone();
    v["items"][1]["handshake"]["session"] = Value::Null;
    mutations.push(v);
    for (index, v) in mutations.iter().enumerate() {
        assert!(decode(v, None).is_err(), "mutation {index}");
    }
    assert!(decode(&good["initial"], Some(&f.contexts[0].peer.local_device_id)).is_err());
    let mut first = good["initial"].clone();
    first["items"].as_array_mut().unwrap().truncate(1);
    first["next"] = first["items"][0]["device"].clone();
    let first = decode(&first, None).unwrap();
    let mut last = good["initial"].clone();
    last["items"].as_array_mut().unwrap().remove(0);
    assert_eq!(decode(&last, first.next.as_deref()).unwrap().items.len(), 2);
    let raw = serde_json::to_string(&good["initial"]).unwrap();
    assert!(Page::decode(
        format!(" {raw}").as_bytes(),
        &f.contexts[0].peer.group_id,
        &f.contexts[0].peer.local_device_id,
        None
    )
    .is_err());
    let duplicate = raw.replacen("\"next\":null", "\"next\":null,\"next\":null", 1);
    assert!(Page::decode(
        duplicate.as_bytes(),
        &f.contexts[0].peer.group_id,
        &f.contexts[0].peer.local_device_id,
        None
    )
    .is_err());
    assert!(Page::decode(
        &vec![b' '; MAX_JSON + 1],
        &f.contexts[0].peer.group_id,
        &f.contexts[0].peer.local_device_id,
        None
    )
    .is_err());
}

#[test]
fn wake_metadata_cannot_replace_trusted_membership_instance_or_acceptance_time() {
    let f = fixture();
    let recorded: Recorded = serde_json::from_str(RECORDED).unwrap();
    let c = &f.contexts[0];
    let page = Page::decode(
        &recorded.initial.into_bytes(),
        &c.peer.group_id,
        &c.peer.local_device_id,
        None,
    )
    .unwrap();
    let wake = page.items[1].wake.as_ref().unwrap();
    let context = |w: &Wake, now| {
        w.context(
            c.peer.clone(),
            c.origin.clone(),
            page.boot.clone(),
            c.nonce.clone(),
            &c.local_instance,
            &c.remote_instance,
            now,
        )
    };
    assert!(context(wake, f.now).is_ok());
    let mut wrong = wake.clone();
    wrong.client_membership = "different-membership-0001".into();
    assert!(matches!(context(&wrong, f.now), Err(Error::Unauthorized)));
    let mut wrong = wake.clone();
    wrong.server_instance = "different-instance-00001".into();
    assert!(matches!(context(&wrong, f.now), Err(Error::Unauthorized)));
    for now in [f.now - 1, f.now + 120_000] {
        assert!(matches!(context(wake, now), Err(Error::StaleSignature)));
    }
}

#[test]
fn declaration_four_kib_limit_and_raw_field_order_are_shared_with_worker() {
    let f = fixture();
    let signed = crate::bbs_sync::relay::tests::statements(&f)[0].clone();
    let reordered = format!(
        "{{\"signature\":{},\"statement\":{}}}",
        serde_json::to_string(&signed.signature).unwrap(),
        serde_json::to_string(&signed.statement).unwrap()
    );
    compact_json(reordered.as_bytes(), MAX_DECLARATION).unwrap();
    let declaration: Declaration = serde_json::from_str(&reordered).unwrap();
    assert_eq!(declaration.bytes(), reordered.as_bytes());
    assert_ne!(declaration.bytes(), declaration.signed.encode().unwrap());
    f.contexts[1]
        .verify_remote(&declaration.signed, None, f.now)
        .unwrap();
    let mut too_large = signed;
    too_large.statement.origin = format!("https://{}.example", "a".repeat(4096));
    let raw = serde_json::to_vec(&too_large).unwrap();
    assert!(raw.len() > 4096 && raw.len() < 8192);
    assert!(too_large.encode().is_err());
    assert!(SignedStatement::decode(&raw).is_err());
    assert!(serde_json::from_slice::<Declaration>(&raw).is_err());
}
