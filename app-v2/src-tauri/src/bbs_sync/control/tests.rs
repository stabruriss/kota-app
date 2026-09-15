use super::*;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use ring::signature::{UnparsedPublicKey, ED25519};
use std::{
    collections::{BTreeSet, VecDeque},
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
struct FakeTransport(Arc<Mutex<Fake>>);
#[derive(Default)]
struct Fake {
    calls: Vec<(String, Value, BTreeMap<String, String>)>,
    failures_after: VecDeque<String>,
    failures_before: VecDeque<String>,
    group: String,
    invite: Option<InviteStatus>,
    joined: BTreeMap<String, String>,
    removed: BTreeSet<String>,
    store: Option<PathBuf>,
    protocol: u32,
    legacy_worker: bool,
}
fn response(value: Value) -> RemoteResponse {
    serde_json::from_value(value).unwrap()
}
impl Transport for FakeTransport {
    fn send(
        &self,
        method: &str,
        url: &str,
        headers: &BTreeMap<String, String>,
        raw: &str,
    ) -> Result<RemoteResponse, String> {
        let body: Value = serde_json::from_str(raw).unwrap();
        let url = Url::parse(url).unwrap();
        let path = url.path().to_string();
        let origin = url.origin().ascii_serialization();
        let mut fake = self.0.lock().unwrap();
        if let Some(path) = &fake.store {
            let stored: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            if body.get("requestId").is_some() && !path.to_string_lossy().contains("signal") {
                assert!(
                    stored["pending"].is_object(),
                    "intent must be durable before sending"
                );
            }
        }
        if !path.starts_with("/bbs/owner/") {
            let device = headers.get("x-kota-bbs-device").unwrap();
            let public = BASE64
                .decode(headers.get("x-kota-bbs-public-key").unwrap())
                .unwrap();
            assert_eq!(*device, raw_sha256(&public));
            let group = body["expectedGroupId"].as_str().unwrap_or("");
            let message = [
                "kota-bbs-control.v1",
                &origin,
                method,
                &path,
                group,
                device,
                &headers["x-kota-bbs-time"],
                &headers["x-kota-bbs-nonce"],
                &raw_sha256(raw.as_bytes()),
            ]
            .join("\n");
            UnparsedPublicKey::new(&ED25519, public)
                .verify(
                    message.as_bytes(),
                    &BASE64.decode(&headers["x-kota-bbs-signature"]).unwrap(),
                )
                .unwrap();
        } else {
            assert_eq!(
                headers.get("x-kota-standby-secret").map(String::as_str),
                Some("paired-secret")
            );
        }
        fake.calls
            .push((path.clone(), body.clone(), headers.clone()));
        if fake.legacy_worker {
            return decode_control_response(
                404,
                br#"{"ok":false,"error":"not found"}"#,
                &url,
                Instant::now() + REQUEST_BUDGET,
                |_, _| {
                    Ok((200, br#"{"ok":true,"relayVersion":"0.1.2","protocolVersion":"kota-lm-standby.v1"}"#.to_vec()))
                },
            );
        }
        if fake.failures_before.front() == Some(&path) {
            fake.failures_before.pop_front();
            return Err("worker_unreachable".into());
        }
        let mut result = json!({"protocolVersion": if fake.protocol == 0 { 1 } else { fake.protocol }, "ok":true});
        let id = body["requestId"].as_str().unwrap_or("").to_string();
        match path.as_str() {
            "/bbs/owner/create" => {
                fake.group = body["groupId"].as_str().unwrap().into();
                result["groupId"] = json!(fake.group);
                result["role"] = json!("owner");
                result["membershipId"] = json!(id);
            }
            "/bbs/owner/status" => {
                result["groupId"] = json!(fake.group);
                result["invite"] =
                    serde_json::to_value(fake.invite.clone().unwrap_or(InviteStatus {
                        gen: 0,
                        has_code: false,
                        expires_at: 0,
                    }))
                    .unwrap();
                result["members"] = json!([{"deviceId":"owner","publicKey":"key","name":"Owner","role":"owner","membershipId":"id","lastSeenAt":0,"online":true}]);
            }
            "/bbs/owner/invitation" => {
                let gen = body["gen"].as_u64().unwrap();
                if fake.invite.as_ref().is_some_and(|invite| invite.gen > gen) {
                    result["ok"] = json!(false);
                    result["error"] = json!("generation_conflict");
                } else {
                    fake.invite = Some(InviteStatus {
                        gen,
                        has_code: true,
                        expires_at: now_ms().unwrap() + 900_000,
                    });
                    result["invite"] = serde_json::to_value(&fake.invite).unwrap();
                }
            }
            "/bbs/join" => {
                if fake.removed.contains(&id) {
                    result["ok"] = json!(false);
                    result["error"] = json!("removed");
                } else {
                    fake.joined
                        .entry(id.clone())
                        .or_insert_with(|| headers["x-kota-bbs-public-key"].clone());
                    result["groupId"] = json!("remote-group-000001");
                    result["role"] = json!("member");
                    result["membershipId"] = json!(id);
                }
            }
            "/bbs/owner/dissolve" => {}
            _ if path.ends_with("/leave") => {}
            _ if path.ends_with("/heartbeat") => {
                result["groupId"] = json!(fake.group);
                result["invite"] = serde_json::to_value(&fake.invite).unwrap();
                result["signals"] = json!([]);
            }
            _ if path.ends_with("/rename") => {}
            _ => panic!("unexpected fake request path: {path}"),
        }
        if fake.failures_after.front() == Some(&path) {
            fake.failures_after.pop_front();
            return Err("worker_unreachable".into());
        }
        Ok(response(result))
    }
}
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("bbs-control-{}", request_id())))
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn open(root: &Root, transport: FakeTransport, owner: bool) -> ControlClient<FakeTransport> {
    let store = StateStore::at(&root.0);
    transport.0.lock().unwrap().store = Some(store.control_path());
    ControlClient::open(
        store,
        transport,
        owner.then(|| OwnerConnection {
            worker_url: "https://own.example".into(),
            desktop_secret: "paired-secret".into(),
        }),
        "This device",
    )
    .unwrap()
}
fn invitation() -> String {
    format!("kota-bbs://other.example/join#{}", "a".repeat(64))
}

#[test]
fn legacy_route_requires_same_origin_health_proof_of_older_lm_release() {
    let url = Url::parse("https://worker.example:8443/bbs/owner/create").unwrap();
    for (version, protocol, ok, status, upgrade) in [
        ("0.1.2", "kota-lm-standby.v1", true, 200, true),
        ("0.1.1", "kota-lm-standby.v1", true, 200, true),
        ("0.1.3", "kota-lm-standby.v1", true, 200, false),
        ("0.1.4", "kota-lm-standby.v1", true, 200, false),
        ("0.2.0", "kota-lm-standby.v1", true, 200, false),
        ("0.1.12", "kota-lm-standby.v1", true, 200, false),
        ("0.1.2", "unknown", true, 200, false),
        ("0.1.2", "kota-lm-standby.v1", false, 200, false),
        ("0.1.2", "kota-lm-standby.v1", true, 404, false),
        ("v0.1.2", "kota-lm-standby.v1", true, 200, false),
        ("0.01.2", "kota-lm-standby.v1", true, 200, false),
        ("0.1.3-rc.1", "kota-lm-standby.v1", true, 200, false),
        ("", "kota-lm-standby.v1", true, 200, false),
    ] {
        let result = decode_control_response(
            404,
            br#"{ "error": "not found", "ok": false }"#,
            &url,
            Instant::now() + Duration::from_secs(3),
            |health, remaining| {
                assert_eq!(health, "https://worker.example:8443/health");
                assert!(remaining > Duration::ZERO && remaining <= Duration::from_secs(3));
                Ok((
                    status,
                    serde_json::to_vec(
                        &json!({"ok":ok,"relayVersion":version,"protocolVersion":protocol}),
                    )
                    .unwrap(),
                ))
            },
        );
        assert_eq!(
            result.unwrap_err(),
            if upgrade {
                "worker_update_required"
            } else {
                "invalid_worker_response"
            },
            "version {version}, protocol {protocol}, ok {ok}, status {status}"
        );
    }
    for health in [
        br#"{}"#.as_slice(),
        br#"{"ok":true,"relayVersion":"0.1.2"}"#,
        b"not JSON",
    ] {
        assert_eq!(
            decode_control_response(
                404,
                br#"{"ok":false,"error":"not found"}"#,
                &url,
                Instant::now() + REQUEST_BUDGET,
                |_, _| Ok((200, health.to_vec()))
            )
            .unwrap_err(),
            "invalid_worker_response"
        );
    }
    assert!(
        relay_version(crate::laughing_man::STANDBY_RECOMMENDED_VERSION).unwrap()
            >= BBS_MIN_RELAY_VERSION
    );
}

#[test]
fn normal_control_responses_never_probe_health_or_become_upgrade_errors() {
    let url = Url::parse("https://worker.example/bbs/owner/create").unwrap();
    for (status, bytes, parses) in [
        (200, br#"{"protocolVersion":1,"ok":true}"#.as_slice(), true),
        (
            404,
            br#"{"protocolVersion":1,"ok":false,"error":"not_found"}"#,
            true,
        ),
        (
            404,
            br#"{"protocolVersion":2,"ok":false,"error":"not found"}"#,
            true,
        ),
        (200, br#"{"ok":false,"error":"not found"}"#, false),
        (500, br#"{"ok":false,"error":"not found"}"#, false),
        (404, br#"{"ok":false,"error":"not_found"}"#, false),
        (
            404,
            br#"{"ok":false,"error":"not found","extra":"proxy"}"#,
            false,
        ),
        (
            404,
            br#"{"ok":false,"error":"not found","protocolVersion":null}"#,
            false,
        ),
        (404, b"<html>not found</html>", false),
        (404, b"{", false),
    ] {
        let result = decode_control_response(
            status,
            bytes,
            &url,
            Instant::now() + REQUEST_BUDGET,
            |_, _| panic!("not an exact legacy route response"),
        );
        assert_eq!(result.is_ok(), parses);
        if !parses {
            assert_eq!(result.unwrap_err(), "invalid_worker_response");
        }
    }
}

#[test]
fn legacy_health_probe_keeps_one_deadline_and_transport_errors() {
    let url = Url::parse("https://worker.example/bbs/owner/create").unwrap();
    let bytes = br#"{"ok":false,"error":"not found"}"#;
    let error = decode_control_response(404, bytes, &url, Instant::now(), |_, _| {
        panic!("budget exhausted before health")
    });
    assert_eq!(error.unwrap_err(), "worker_unreachable");
    for code in [
        "worker_unreachable",
        "worker_redirect_rejected",
        "worker_response_too_large",
        "worker_response_failed",
    ] {
        let error =
            decode_control_response(404, bytes, &url, Instant::now() + REQUEST_BUDGET, |_, _| {
                Err(code.into())
            });
        assert_eq!(error.unwrap_err(), code);
    }
}

#[test]
fn http_response_reader_keeps_status_redirect_and_size_limits() {
    let reply =
        ureq::Response::new(404, "Not Found", r#"{"ok":false,"error":"not found"}"#).unwrap();
    assert_eq!(
        read_http_response(Err(ureq::Error::Status(404, reply)), MAX_RESPONSE_BYTES)
            .unwrap()
            .0,
        404
    );
    for limit in [MAX_RESPONSE_BYTES, MAX_HEALTH_BYTES] {
        let reply = ureq::Response::new(200, "OK", &"x".repeat(limit)).unwrap();
        assert_eq!(read_http_response(Ok(reply), limit).unwrap().1.len(), limit);
        let reply = ureq::Response::new(200, "OK", &"x".repeat(limit + 1)).unwrap();
        assert_eq!(
            read_http_response(Ok(reply), limit).unwrap_err(),
            "worker_response_too_large"
        );
    }
    let redirect = ureq::Response::new(302, "Found", "").unwrap();
    assert_eq!(
        read_http_response(Ok(redirect), MAX_HEALTH_BYTES).unwrap_err(),
        "worker_redirect_rejected"
    );
    assert_eq!(
        read_http_response(
            Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "not exposed").into()),
            MAX_RESPONSE_BYTES
        )
        .unwrap_err(),
        "worker_unreachable"
    );
    assert_eq!(
        HttpsTransport::default()
            .send(
                "POST",
                "http://worker.example/bbs/owner/create",
                &BTreeMap::new(),
                "{}"
            )
            .unwrap_err(),
        "https_required"
    );
}

#[test]
fn create_on_old_worker_keeps_request_and_identity_until_upgrade() {
    let root = Root::new();
    let transport = FakeTransport::default();
    transport.0.lock().unwrap().legacy_worker = true;
    let mut client = open(&root, transport.clone(), true);
    let identity = fs::read(client.store.identity_path()).unwrap();
    assert_eq!(
        client.invitation(None, false).unwrap_err(),
        "worker_update_required"
    );
    let pending = fs::read(client.store.control_path()).unwrap();
    let parsed: Value = serde_json::from_slice(&pending).unwrap();
    assert_eq!(parsed["pending"]["kind"], "create");
    assert!(parsed["current"].is_null());
    assert!(parsed["invitation"].is_null());
    assert_eq!(
        client.resume_pending().unwrap_err(),
        "worker_update_required"
    );
    assert_eq!(fs::read(client.store.control_path()).unwrap(), pending);
    drop(client);
    let mut client = open(&root, transport.clone(), true);
    assert_eq!(fs::read(client.store.control_path()).unwrap(), pending);
    transport.0.lock().unwrap().legacy_worker = false;
    let result = client.invitation(None, false).unwrap();
    assert_eq!(result.group_id, parsed["pending"]["groupId"]);
    assert_eq!(fs::read(client.store.identity_path()).unwrap(), identity);
    assert!(!client.has_pending());
    assert_eq!(client.membership().unwrap().role, Role::Owner);
    let fake = transport.0.lock().unwrap();
    let creates = fake
        .calls
        .iter()
        .filter(|call| call.0 == "/bbs/owner/create")
        .collect::<Vec<_>>();
    assert_eq!(creates.len(), 3);
    for request in creates {
        assert_eq!(request.1["requestId"], parsed["pending"]["requestId"]);
        assert_eq!(request.1["groupId"], parsed["pending"]["groupId"]);
    }
}

#[test]
fn remote_error_string_cannot_forge_a_local_upgrade_diagnosis() {
    struct Unproved;
    impl Transport for Unproved {
        fn send(
            &self,
            _: &str,
            _: &str,
            _: &BTreeMap<String, String>,
            _: &str,
        ) -> Result<RemoteResponse, String> {
            Ok(response(
                json!({"protocolVersion":1,"ok":false,"error":"worker_update_required"}),
            ))
        }
    }
    let root = Root::new();
    let client = ControlClient::open(
        StateStore::at(&root.0),
        Unproved,
        Some(OwnerConnection {
            worker_url: "https://own.example".into(),
            desktop_secret: "fixture-secret".into(),
        }),
        "This device",
    )
    .unwrap();
    let error = client
        .request(
            "https://own.example",
            "/bbs/owner/create",
            "",
            json!({}),
            true,
        )
        .unwrap_err();
    assert_eq!(error, "control_rejected");
    assert_eq!(
        super::super::public::Error::from_code(&error).code,
        "sync_unavailable"
    );
}

#[test]
fn status_and_reopen_are_local_and_keep_device_identity() {
    let root = Root::new();
    let transport = FakeTransport::default();
    let mut first = open(&root, transport.clone(), false);
    let id = first.device_id().unwrap();
    for _ in 0..20 {
        assert!(first.status_memory().is_none());
    }
    first.rename(None, "New name").unwrap();
    drop(first);
    let second = open(&root, transport.clone(), false);
    assert_eq!(second.device_id().unwrap(), id);
    assert_eq!(second.device_name(), "New name");
    assert!(transport.0.lock().unwrap().calls.is_empty());
}
#[test]
fn successful_join_with_lost_response_recovers_same_key_and_request_after_restart() {
    let root = Root::new();
    let transport = FakeTransport::default();
    transport
        .0
        .lock()
        .unwrap()
        .failures_after
        .push_back("/bbs/join".into());
    let mut client = open(&root, transport.clone(), false);
    let device = client.device_id().unwrap();
    assert_eq!(
        client.join(None, &invitation()).unwrap_err(),
        "worker_unreachable"
    );
    assert!(client.membership().is_none());
    assert!(client.has_pending());
    drop(client);
    let mut client = open(&root, transport.clone(), false);
    client.resume_pending().unwrap();
    assert_eq!(client.device_id().unwrap(), device);
    assert_eq!(client.membership().unwrap().group_id, "remote-group-000001");
    assert!(!client.has_pending());
    let fake = transport.0.lock().unwrap();
    assert_eq!(fake.joined.len(), 1);
    assert_eq!(fake.calls[0].1["requestId"], fake.calls[1].1["requestId"]);
    assert_eq!(
        fake.calls[0].2["x-kota-bbs-public-key"],
        fake.calls[1].2["x-kota-bbs-public-key"]
    );
    assert_ne!(
        fake.calls[0].2["x-kota-bbs-nonce"],
        fake.calls[1].2["x-kota-bbs-nonce"]
    );
}
#[test]
fn removed_join_receipt_cannot_restore_membership_and_new_join_remains_possible() {
    let root = Root::new();
    let transport = FakeTransport::default();
    transport
        .0
        .lock()
        .unwrap()
        .failures_after
        .push_back("/bbs/join".into());
    let mut client = open(&root, transport.clone(), false);
    client.join(None, &invitation()).unwrap_err();
    let id = transport.0.lock().unwrap().calls[0].1["requestId"]
        .as_str()
        .unwrap()
        .to_string();
    transport.0.lock().unwrap().removed.insert(id);
    assert_eq!(client.resume_pending().unwrap_err(), "removed");
    assert!(client.membership().is_none());
    assert!(!client.has_pending());
    client.join(None, &invitation()).unwrap();
    assert!(client.membership().is_some());
}
#[test]
fn own_empty_group_is_revoked_before_foreign_invite_is_consumed() {
    let root = Root::new();
    let transport = FakeTransport::default();
    let mut client = open(&root, transport.clone(), true);
    let created = client.invitation(None, false).unwrap();
    transport
        .0
        .lock()
        .unwrap()
        .failures_before
        .push_back("/bbs/owner/dissolve".into());
    assert_eq!(
        client
            .join(Some(&created.group_id), &invitation())
            .unwrap_err(),
        "worker_unreachable"
    );
    assert!(!transport
        .0
        .lock()
        .unwrap()
        .calls
        .iter()
        .any(|call| call.0 == "/bbs/join"));
    client.resume_pending().unwrap();
    assert_eq!(client.membership().unwrap().role, Role::Member);
    let calls = &transport.0.lock().unwrap().calls;
    let join = calls.iter().position(|call| call.0 == "/bbs/join").unwrap();
    assert_eq!(calls[join - 1].0, "/bbs/owner/dissolve");
}
#[test]
fn registration_loss_reuses_material_and_generation_conflict_discards_pending() {
    let root = Root::new();
    let transport = FakeTransport::default();
    let mut client = open(&root, transport.clone(), true);
    transport
        .0
        .lock()
        .unwrap()
        .failures_after
        .push_back("/bbs/owner/invitation".into());
    assert_eq!(
        client.invitation(None, false).unwrap_err(),
        "worker_unreachable"
    );
    let group = client.membership().unwrap().group_id.clone();
    assert!(client.has_pending());
    drop(client);
    let mut client = open(&root, transport.clone(), true);
    let current = client.invitation(Some(&group), false).unwrap();
    let fake = transport.0.lock().unwrap();
    let registrations: Vec<_> = fake
        .calls
        .iter()
        .filter(|call| call.0 == "/bbs/owner/invitation")
        .collect();
    assert_eq!(registrations[0].1, registrations[1].1);
    drop(fake);
    assert_eq!(current.generation, "1");
    transport
        .0
        .lock()
        .unwrap()
        .failures_before
        .push_back("/bbs/owner/invitation".into());
    client.invitation(Some(&group), true).unwrap_err();
    transport.0.lock().unwrap().invite.as_mut().unwrap().gen = 99;
    assert_eq!(client.resume_pending().unwrap_err(), "generation_conflict");
    assert!(!client.has_pending());
    assert!(client.data.invitation.is_none());
}
#[test]
fn consumed_invitation_rotates_on_explicit_background_refresh_not_status_reads() {
    let root = Root::new();
    let transport = FakeTransport::default();
    let mut client = open(&root, transport.clone(), true);
    let first = client.invitation(None, false).unwrap();
    transport
        .0
        .lock()
        .unwrap()
        .invite
        .as_mut()
        .unwrap()
        .has_code = false;
    let before = transport.0.lock().unwrap().calls.len();
    client.status_memory();
    assert_eq!(transport.0.lock().unwrap().calls.len(), before);
    client.refresh_members(Some(&first.group_id)).unwrap();
    let second = client.invitation(Some(&first.group_id), false).unwrap();
    assert_eq!(second.generation, "2");
    assert_ne!(first.invitation, second.invitation);
}

#[test]
fn normal_heartbeat_and_on_demand_code_use_one_request_without_hash_response() {
    let root = Root::new();
    let transport = FakeTransport::default();
    let mut client = open(&root, transport.clone(), true);
    let initial = client.invitation(None, false).unwrap();
    let before = transport.0.lock().unwrap().calls.len();
    client.refresh_members(Some(&initial.group_id)).unwrap();
    let copy = client.invitation(Some(&initial.group_id), false).unwrap();
    assert_eq!(initial.invitation, copy.invitation);
    assert_eq!(transport.0.lock().unwrap().calls.len(), before + 1);
    let remote = serde_json::to_value(&client.status_memory().unwrap().invite).unwrap();
    assert!(remote.get("hash").is_none());
    assert_eq!(remote["hasCode"], true);
}

#[test]
fn startup_inspection_never_creates_and_existing_recovery_never_generates_replacement() {
    let root = Root::new();
    let store = StateStore::at(&root.0);
    for _ in 0..20 {
        assert!(inspect_existing(&store).unwrap().is_none());
    }
    assert!(!root.0.exists());
    assert!(ControlClient::open_existing(store.clone(), FakeTransport::default(), None).is_err());
    assert!(!store.identity_path().exists());
    let transport = FakeTransport::default();
    let mut client = open(&root, transport.clone(), false);
    transport
        .0
        .lock()
        .unwrap()
        .failures_after
        .push_back("/bbs/join".into());
    assert!(client.join(None, &invitation()).is_err());
    let bytes = fs::read(store.identity_path()).unwrap();
    let pending = fs::read(store.control_path()).unwrap();
    drop(client);
    assert!(inspect_existing(&store).unwrap().unwrap().3);
    let mut recovered =
        ControlClient::open_existing(store.clone(), transport.clone(), None).unwrap();
    recovered.resume_pending().unwrap();
    drop(recovered);
    assert_eq!(fs::read(store.identity_path()).unwrap(), bytes);
    let joins = transport
        .0
        .lock()
        .unwrap()
        .calls
        .iter()
        .filter(|(p, _, _)| p == "/bbs/join")
        .map(|(_, b, _)| b["requestId"].clone())
        .collect::<Vec<_>>();
    assert_eq!(joins.len(), 2);
    assert_eq!(joins[0], joins[1]);
    fs::write(store.identity_path(), b"broken").unwrap();
    assert!(inspect_existing(&store).is_err());
    assert!(ControlClient::open_existing(store.clone(), transport, None).is_err());
    assert_eq!(fs::read(store.identity_path()).unwrap(), b"broken");
    assert_ne!(fs::read(store.control_path()).unwrap(), pending);
}
#[test]
fn stale_group_actions_send_nothing_and_protocol_error_preserves_pending_bytes() {
    let root = Root::new();
    let transport = FakeTransport::default();
    let mut client = open(&root, transport.clone(), false);
    client.join(None, &invitation()).unwrap();
    let before = transport.0.lock().unwrap().calls.len();
    assert_eq!(
        client.disconnect(Some("stale-group")).unwrap_err(),
        "group_context_changed"
    );
    assert_eq!(transport.0.lock().unwrap().calls.len(), before);
    transport.0.lock().unwrap().protocol = 99;
    client
        .rename(Some("remote-group-000001"), "Changed")
        .unwrap_err();
    let saved = fs::read(client.store.control_path()).unwrap();
    assert_eq!(client.resume_pending().unwrap_err(), "protocol_mismatch");
    assert_eq!(fs::read(client.store.control_path()).unwrap(), saved);
}
#[test]
fn invalid_credentials_and_invitation_urls_are_not_silently_replaced() {
    let root = Root::new();
    let transport = FakeTransport::default();
    let client = open(&root, transport.clone(), false);
    let identity = fs::read(client.store.identity_path()).unwrap();
    fs::write(client.store.control_path(), b"broken-private-state").unwrap();
    let store = client.store.clone();
    drop(client);
    assert!(
        matches!(ControlClient::open(store.clone(), transport, None, "Device"), Err(error) if error == "invalid private state JSON")
    );
    assert_eq!(fs::read(store.identity_path()).unwrap(), identity);
    for value in [
        "https://worker.example/join#secret",
        "kota-bbs://user:pass@worker.example/join#secret",
        "kota-bbs://worker.example/path#secret",
        "kota-bbs://worker.example/join?secret#secret",
    ] {
        assert!(parse_invitation(value).is_err());
    }
    assert!(worker_origin("http://worker.example").is_err());
    assert!(worker_origin("https://user:secret@worker.example").is_err());
}

#[test]
fn signing_bytes_match_the_workers_ed25519_fixture() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../relays/laughing-man-cloudflare/tests/fixtures/control-proof-v1.json"
    )))
    .unwrap();
    let identity = DeviceIdentity::from_seed([7; 32]).unwrap();
    let headers = signed_headers(
        &identity,
        fixture["origin"].as_str().unwrap(),
        fixture["method"].as_str().unwrap(),
        fixture["path"].as_str().unwrap(),
        fixture["groupId"].as_str().unwrap(),
        fixture["body"].as_str().unwrap(),
        fixture["timestamp"].as_u64().unwrap(),
        fixture["nonce"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(headers["x-kota-bbs-device"], fixture["deviceId"]);
    assert_eq!(headers["x-kota-bbs-public-key"], fixture["publicKey"]);
    assert_eq!(headers["x-kota-bbs-signature"], fixture["signature"]);
}

#[test]
fn a_second_controller_cannot_replace_the_first_controllers_identity() {
    let root = Root::new();
    let transport = FakeTransport::default();
    let client = open(&root, transport.clone(), false);
    let before = fs::read(client.store.identity_path()).unwrap();
    assert!(
        matches!(ControlClient::open(client.store.clone(), transport.clone(), None, "Other"), Err(error) if error == "control_in_use")
    );
    assert_eq!(fs::read(client.store.identity_path()).unwrap(), before);
    let id = client.device_id().unwrap();
    drop(client);
    assert_eq!(open(&root, transport, false).device_id().unwrap(), id);
}
