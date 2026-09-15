use super::*;
use crate::agent_directory::tests::Account;
use serde_json::json;

#[test]
fn public_directory_is_identity_and_lease_free_and_excludes_private_fields() {
    let account = Account::new();
    account.project("p", false);
    account.agent("p", "a", "display-name: Target\navatar-id: codex\ncwd: /private/source\napi-key: secret-value\nsession-id: hidden\n");
    let store = StateStore::at(&account.0);
    let value = serde_json::to_value(directory(&account.0, &store).unwrap()).unwrap();
    assert!(value["devices"][0]["deviceId"].is_null());
    assert_eq!(
        value["devices"][0]["projects"][0]["agents"][0]["targetRef"],
        "local/p/a"
    );
    assert_eq!(
        value["devices"][0]["projects"][0]["agents"][0]["avatar"],
        json!({"kind":"builtin","id":"codex"})
    );
    let raw = value.to_string();
    for banned in [
        "cwd",
        "private",
        "session",
        "secret",
        "api-key",
        "workerUrl",
    ] {
        assert!(!raw.contains(banned));
    }
    assert!(!account.0.join("bbs-sync").exists());
    let first = version(&collect_local(&account.0).unwrap()).unwrap();
    account.agent(
        "p",
        "a",
        "display-name: Target\navatar-id: codex\nsession-id: updated\nlast-active-at: later\n",
    );
    assert_eq!(version(&collect_local(&account.0).unwrap()).unwrap(), first);
    account.agent("p", "a", "display-name: Renamed\navatar-id: codex\n");
    assert_ne!(version(&collect_local(&account.0).unwrap()).unwrap(), first);
}

pub(crate) fn joined(account: &Account) -> (StateStore, String, String) {
    let store = StateStore::at(&account.0);
    let identity = crate::bbs_sync::DeviceIdentity::from_seed([21; 32]).unwrap();
    let device = identity.device_id().unwrap();
    let peer = crate::bbs_sync::DeviceIdentity::from_seed([22; 32])
        .unwrap()
        .device_id()
        .unwrap();
    store.save_identity(&identity).unwrap();
    super::super::write(&store.control_path(), &json!({
        "schemaVersion":1,"deviceName":"Local","current":{"groupId":"group-a", "workerUrl":"https://unit.test", "role":"owner", "membershipId":"own-membership"},
        "pending":null,"invitation":null
    })).unwrap();
    fs::create_dir_all(account.0.join("Workspaces/bbs")).unwrap();
    super::super::write(
        &store.root.join("rosters/members.json"),
        &Members {
            schema_version: 1,
            group_id: "group-a".into(),
            membership_id: "own-membership".into(),
            members: vec![Member {
                device_id: peer.clone(),
                membership_id: "peer-membership".into(),
                name: "Remote".into(),
                online: false,
            }],
        },
    )
    .unwrap();
    (store, device, peer)
}

#[test]
fn peer_not_synced_and_complete_empty_are_distinct_and_membership_fences_cache_reads() {
    let account = Account::new();
    let (store, device, peer) = joined(&account);
    let lease = store.control_lease().unwrap();
    let v = serde_json::to_value(directory(&account.0, &store).unwrap()).unwrap();
    assert_eq!(v["devices"][0]["deviceId"], device);
    assert_eq!(v["devices"][1]["rosterStatus"], "not_synced");
    assert!(v["devices"][1]["projects"].is_null());
    let mut roster = PeerRoster {
        schema_version: 1,
        group_id: "group-a".into(),
        device_id: peer.clone(),
        membership_id: "peer-membership".into(),
        version: version(&[]).unwrap(),
        received_at: "2026-09-13T10:00:00Z".into(),
        projects: vec![],
    };
    super::super::write(
        &store.root.join("rosters").join(format!("{peer}.json")),
        &roster,
    )
    .unwrap();
    let v = serde_json::to_value(directory(&account.0, &store).unwrap()).unwrap();
    assert_eq!(v["devices"][1]["rosterStatus"], "synced");
    assert_eq!(v["devices"][1]["projects"], json!([]));
    assert_eq!(v["devices"][1]["online"], false);
    roster.membership_id = "previous-membership".into();
    super::super::write(
        &store.root.join("rosters").join(format!("{peer}.json")),
        &roster,
    )
    .unwrap();
    assert!(read_peer(&store, &context(&store).unwrap(), &peer)
        .unwrap()
        .is_none());
    drop(lease);
    fs::write(store.identity_path(), b"{broken").unwrap();
    assert!(directory(&account.0, &store).is_err());
    assert_eq!(fs::read(store.identity_path()).unwrap(), b"{broken");
}

#[test]
fn registered_images_preserve_full_bytes_and_never_serialize_paths_or_encoded_images() {
    let account = Account::new();
    account.project("p", false);
    account.agent("p", "a", "display-name: Picture\navatar-id: user:pic\n");
    let dir = account.0.join("avatars");
    fs::create_dir_all(&dir).unwrap();
    let bytes = vec![42; 599_999];
    fs::write(dir.join("pic.png"), &bytes).unwrap();
    fs::write(dir.join("avatars.json"), serde_json::to_vec(&json!([{
        "id":"user:pic","label":"private-label","fileName":"pic.png","mime":"image/png","createdAt":"now"
    }])).unwrap()).unwrap();
    let projects = collect_local(&account.0).unwrap();
    assert_eq!(
        projects[0].agents[0].avatar,
        Avatar::Image {
            sha256: raw_sha256(&bytes),
            ext: "png".into(),
            size_bytes: 599_999
        }
    );
    let raw = serde_json::to_string(&projects).unwrap();
    assert!(!raw.contains("fileName") && !raw.contains("private-label") && !raw.contains("data:"));
    fs::remove_file(dir.join("pic.png")).unwrap();
    assert_eq!(
        collect_local(&account.0).unwrap()[0].agents[0].avatar,
        Avatar::None
    );
    assert_eq!(
        collect_local(&account.0).unwrap()[0].agents[0].name,
        "Picture"
    );
}

#[test]
fn roster_avatar_reuses_image_limits_but_retains_unknown_builtin_for_text_fallback() {
    let future = Avatar::Builtin {
        id: "future-avatar".into(),
    };
    assert!(future.validate().is_ok());
    assert_eq!(
        serde_json::to_value(PublicAvatar::new(&future, false)).unwrap(),
        json!({"kind":"builtin", "id":"future-avatar"})
    );
    assert!(Avatar::Builtin {
        id: "../../image".into()
    }
    .validate()
    .is_err());
    for (ext, size, valid) in [
        ("png", 600000, true),
        ("jpg", 1, true),
        ("webp", 1, true),
        ("gif", 1, false),
        ("png", 600001, false),
        ("png", 0, false),
    ] {
        assert_eq!(
            Avatar::Image {
                sha256: "a".repeat(64),
                ext: ext.into(),
                size_bytes: size
            }
            .validate()
            .is_ok(),
            valid
        );
    }
}
