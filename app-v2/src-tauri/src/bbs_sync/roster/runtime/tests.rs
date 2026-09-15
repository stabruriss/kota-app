use super::*;
use crate::agent_directory::tests::Account;
use serde_json::json;
use std::{fs, sync::atomic::AtomicUsize, time::Duration};

async fn page(runtime: &Runtime) -> public::Page {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(p) = runtime.page(public::ReadRequest {
                version: None,
                after: None,
            }) {
                return p;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("roster publication")
}
async fn changed(runtime: &Runtime, old: &str) -> public::Page {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let p = page(runtime).await;
            if p.version != old {
                return p;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("roster version change")
}
fn store(account: &Account) -> ContentStore {
    ContentStore::at(account.0.join("Workspaces/bbs"), account.0.clone())
}
async fn close(runtime: &Runtime) {
    runtime.stop();
    if let Some(Ok((files, _))) = runtime.files.get() {
        files.stop_and_wait().await;
    }
}

#[tokio::test]
async fn memory_pages_and_identity_free_local_avatar_use_actual_background_projection() {
    let account = Account::new();
    account.project("p", false);
    account.agent(
        "p",
        "a",
        "display-name: Photo\navatar-id: user:pic\nsession-id: DO_NOT_EXPOSE\n",
    );
    let dir = account.0.join("avatars");
    fs::create_dir_all(&dir).unwrap();
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aX1sAAAAASUVORK5CYII=").unwrap();
    let sha = crate::bbs_sync::raw_sha256(&png);
    fs::write(dir.join("pic.png"), &png).unwrap();
    fs::write(dir.join("avatars.json"), br#"[{"id":"user:pic","label":"private-label","fileName":"pic.png","mime":"image/png","createdAt":"now"}]"#).unwrap();
    let content = store(&account);
    content.prepare_layout().unwrap();
    let runtime = Runtime::new(content.clone());
    for _ in 0..100 {
        assert!(runtime
            .page(public::ReadRequest {
                version: None,
                after: None
            })
            .is_err());
    }
    assert!(runtime.files.get().is_none());
    assert!(!content.state.root.exists());
    let hints = Arc::new(AtomicUsize::new(0));
    let emitted = hints.clone();
    let content_changes = Arc::new(AtomicUsize::new(0));
    let c = content_changes.clone();
    runtime.start(
        Arc::new(move || {
            emitted.fetch_add(1, Ordering::Relaxed);
        }),
        Arc::new(move || {
            c.fetch_add(1, Ordering::Relaxed);
        }),
    );
    let first = page(&runtime).await;
    let picture = runtime.avatar_resource("local", &sha).unwrap();
    let url = content.read_roster_avatar(&picture).unwrap();
    assert!(
        content.read_verified_avatar(&sha, "png").is_err(),
        "post reader must remain ledger restricted"
    );
    let fixture = json!({"local":first,"avatar":{"deviceId":"local","sha256":sha,"dataUrl":url},
        "staleError":public::Error::changed()});
    let serialized = serde_json::to_string(&fixture).unwrap();
    assert!(
        !serialized.contains("DO_NOT_EXPOSE")
            && !serialized.contains("private-label")
            && !serialized.contains("fileName")
    );
    assert_eq!(
        fixture["local"]["items"][0]["deviceId"],
        serde_json::Value::Null
    );
    assert_eq!(fixture["local"]["items"][2]["avatar"]["available"], true);
    println!("BBS_ROSTER_FIXTURE={serialized}");
    // Remove all registration files. Repeated IPC reads still return the full
    // published version until the background dirty hook accepts a new one.
    fs::remove_dir_all(account.0.join("Workspaces/p")).unwrap();
    for _ in 0..100 {
        assert_eq!(page(&runtime).await.version, first.version);
    }
    assert!(runtime.avatar_resource(&"f".repeat(64), &sha).is_err());
    runtime.mark(LOCAL);
    let empty = changed(&runtime, &first.version).await;
    assert_eq!(empty.items.len(), 1);
    assert!(runtime.avatar_resource("local", &sha).is_err());
    assert!(
        runtime
            .page(public::ReadRequest {
                version: Some(first.version),
                after: Some("1".into())
            })
            .unwrap_err()
            == public::Error::changed()
    );
    assert!(!content.state.identity_path().exists() && !content.state.control_path().exists());
    assert!(hints.load(Ordering::Relaxed) >= 2 && content_changes.load(Ordering::Relaxed) >= 2);
    close(&runtime).await;
}

#[tokio::test]
async fn presence_and_avatar_availability_change_public_version_without_changing_roster_content() {
    let account = Account::new();
    account.project("p", false);
    let (state, own, peer) = super::super::tests::joined(&account);
    let bytes = b"small photo";
    let sha = crate::bbs_sync::raw_sha256(bytes);
    let projects = vec![Project {
        project_id: "remote-p".into(),
        name: "Remote project".into(),
        agents: vec![super::super::Agent {
            agent_id: "a".into(),
            name: "Remote agent".into(),
            avatar: Avatar::Image {
                sha256: sha.clone(),
                ext: "png".into(),
                size_bytes: bytes.len() as u64,
            },
        }],
    }];
    let roster = PeerRoster {
        schema_version: 1,
        group_id: "group-a".into(),
        device_id: peer.clone(),
        membership_id: "peer-membership".into(),
        version: super::super::version(&projects).unwrap(),
        received_at: "2026-09-13T00:00:00Z".into(),
        projects,
    };
    crate::bbs_sync::write(&state.root.join(format!("rosters/{peer}.json")), &roster).unwrap();
    let content = store(&account);
    content.prepare_layout().unwrap();
    let runtime = Runtime::new(content.clone());
    runtime.start(Arc::new(|| {}), Arc::new(|| {}));
    let initial = page(&runtime).await;
    let source_version = runtime.source().unwrap().version.clone();
    let members = context(&state).unwrap().members;
    let mut next = context(&state).unwrap();
    next.members[0].online = true;
    next.members[0].name = "Renamed device".into();
    runtime.update_context(next);
    let online = changed(&runtime, &initial.version).await;
    assert_eq!(runtime.source().unwrap().version, source_version);
    let dir = content.root().join("avatars");
    fs::create_dir_all(&dir).unwrap();
    let blob = dir.join(format!("{sha}.png"));
    fs::write(&blob, bytes).unwrap();
    runtime.avatar_changed(&sha);
    let available = changed(&runtime, &online.version).await;
    assert!(serde_json::to_string(&available)
        .unwrap()
        .contains("\"available\":true"));
    let resource = runtime.avatar_resource(&peer, &sha).unwrap();
    assert!(content.read_roster_avatar(&resource).is_ok());
    assert!(runtime.avatar_resource("local", &sha).is_err());
    assert!(runtime.avatar_resource(&own, &sha).is_err());
    assert_eq!(members.len(), 1);
    let mut removed = context(&state).unwrap();
    removed.members.clear();
    runtime.update_context(removed);
    assert!(runtime.avatar_resource(&peer, &sha).is_err());
    let withdrawn = changed(&runtime, &available.version).await;
    assert!(!serde_json::to_string(&withdrawn).unwrap().contains(&peer));
    close(&runtime).await;
}

#[tokio::test]
async fn invalid_existing_identity_is_not_recreated_by_roster_start_or_reads() {
    let account = Account::new();
    let (state, _, _) = super::super::tests::joined(&account);
    fs::write(state.identity_path(), b"broken identity").unwrap();
    let before = fs::read(state.control_path()).unwrap();
    let runtime = Runtime::new(store(&account));
    runtime.start(Arc::new(|| {}), Arc::new(|| {}));
    runtime.wait_initialized().await;
    assert!(runtime
        .page(public::ReadRequest {
            version: None,
            after: None
        })
        .is_err());
    assert_eq!(fs::read(state.identity_path()).unwrap(), b"broken identity");
    assert_eq!(fs::read(state.control_path()).unwrap(), before);
    close(&runtime).await;
}

#[tokio::test]
async fn local_projection_does_not_clean_another_owners_stage_and_network_retirement_keeps_file_service(
) {
    let account = Account::new();
    let (state, _, _) = super::super::tests::joined(&account);
    let _owner = state.control_lease().unwrap();
    let stage = state.root.join("rosters/.roster-owner.partial");
    fs::write(&stage, b"in progress").unwrap();
    let runtime = Runtime::new(store(&account));
    runtime.start(Arc::new(|| {}), Arc::new(|| {}));
    let _ = page(&runtime).await;
    assert_eq!(
        fs::read(&stage).unwrap(),
        b"in progress",
        "local projection has no ControlLease"
    );
    // This call represents the already admitted control owner, not the reader.
    runtime.recover_staging().await.unwrap();
    assert!(!stage.exists());
    let (files, limits) = runtime.files().unwrap();
    let host = crate::bbs_sync::transport::NetworkHost::start_with_files(files.clone(), limits)
        .await
        .unwrap();
    host.stop_and_wait().await;
    assert_eq!(files.run(&Cancellation::default(), |_| 7).await.unwrap(), 7);
    assert!(runtime
        .page(public::ReadRequest {
            version: None,
            after: None
        })
        .is_ok());
    close(&runtime).await;
}
