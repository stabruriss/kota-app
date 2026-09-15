use super::*;
use crate::{
    agent_directory::tests::Account,
    bbs_sync::{
        roster::{self, Avatar},
        transport::{Cancellation, Limits},
    },
};

fn projects(count: usize) -> Vec<Project> {
    vec![Project {
        project_id: "p".into(),
        name: "Project".into(),
        agents: (0..count)
            .map(|i| Agent {
                agent_id: format!("a-{i:05}"),
                name: format!("Agent {i}"),
                avatar: Avatar::Builtin { id: "codex".into() },
            })
            .collect(),
    }]
}

#[tokio::test]
async fn multi_page_roster_streams_complete_snapshot_without_a_total_cutoff() {
    let account = Account::new();
    let (store, _, peer) = roster::tests::joined(&account);
    let local = Local::new(projects(4101)).unwrap();
    let expected = local.version.clone();
    let file_io = FileIo::start(Limits::default()).unwrap();
    let handle = file_io.clone();
    file_io
        .run(&Cancellation::default(), move |io| {
            let mut receive = Receiver::begin(&store, &peer, &local.version, handle).unwrap();
            let mut cursor = None;
            let mut pages = 0;
            loop {
                let page = local.page(cursor.as_deref()).unwrap();
                assert!(page.items.len() <= 64);
                assert!(serde_json::to_vec(&page).unwrap().len() <= PAGE_BUDGET);
                let roundtrip: Page =
                    serde_json::from_slice(&serde_json::to_vec(&page).unwrap()).unwrap();
                let complete = receive.append(&store, &roundtrip, io, || true).unwrap();
                pages += 1;
                cursor = page.next;
                if complete {
                    break;
                }
            }
            assert!(pages > 64);
            let cached = roster::read_peer(&store, &context(&store).unwrap(), &peer)
                .unwrap()
                .unwrap();
            assert_eq!(cached.version, expected);
            assert_eq!(cached.projects[0].agents.len(), 4101);
        })
        .await
        .unwrap();
    file_io.stop_and_wait().await;
}

#[tokio::test]
async fn partial_corrupt_and_revoked_rosters_keep_old_cache_complete_empty_replaces() {
    let account = Account::new();
    let (store, _, peer) = roster::tests::joined(&account);
    let file_io = FileIo::start(Limits::default()).unwrap();
    let handle = file_io.clone();
    file_io
        .run(&Cancellation::default(), move |io| {
            let old = Local::new(projects(1)).unwrap();
            let mut receiver =
                Receiver::begin(&store, &peer, &old.version, handle.clone()).unwrap();
            receiver
                .append(&store, &old.page(None).unwrap(), io, || true)
                .unwrap();
            let cache = store.root.join(format!("rosters/{peer}.json"));
            let original = fs::read(&cache).unwrap();
            let source = Local::new(projects(70)).unwrap();
            let mut partial =
                Receiver::begin(&store, &peer, &source.version, handle.clone()).unwrap();
            let first = source.page(None).unwrap();
            assert!(!partial.append(&store, &first, io, || true).unwrap());
            assert_eq!(fs::read(&cache).unwrap(), original);
            let mut tail = source.page(first.next.as_deref()).unwrap();
            tail.version = "e".repeat(64);
            assert!(partial.append(&store, &tail, io, || true).is_err());
            assert_eq!(fs::read(&cache).unwrap(), original);
            let empty = Local::new(vec![]).unwrap();
            let mut changed =
                Receiver::begin(&store, &peer, &empty.version, handle.clone()).unwrap();
            assert!(changed
                .append(&store, &empty.page(None).unwrap(), io, || false)
                .is_err());
            assert_eq!(fs::read(&cache).unwrap(), original);
            let mut changed = Receiver::begin(&store, &peer, &empty.version, handle).unwrap();
            assert!(changed
                .append(&store, &empty.page(None).unwrap(), io, || true)
                .unwrap());
            assert!(roster::read_peer(&store, &context(&store).unwrap(), &peer)
                .unwrap()
                .unwrap()
                .projects
                .is_empty());
            // Remove our own partials here on the file thread; Drop stays nonblocking.
            for item in fs::read_dir(cache.parent().unwrap()).unwrap().flatten() {
                if item.file_name().to_string_lossy().starts_with(".roster-") {
                    fs::remove_file(item.path()).unwrap();
                }
            }
        })
        .await
        .unwrap();
    file_io.stop_and_wait().await;
}

#[test]
fn public_pages_use_one_immutable_hash_and_reject_stale_or_noncanonical_offsets() {
    let mut view = roster::DirectoryView {
        devices: vec![roster::DeviceView {
            device_id: None,
            name: "Local".into(),
            local: true,
            online: true,
            roster_status: "synced",
            received_at: None,
            projects: Some(super::super::project_views(&projects(130), "local", true)),
        }],
    };
    let first = public::Snapshot::new(view.clone()).unwrap();
    let mut response = first
        .page(public::ReadRequest {
            version: None,
            after: None,
        })
        .unwrap();
    let hash = response.version.clone();
    let mut count = response.items.len();
    while let Some(next) = response.next {
        response = first
            .page(public::ReadRequest {
                version: Some(hash.clone()),
                after: Some(next),
            })
            .unwrap();
        assert!(serde_json::to_vec(&response).unwrap().len() <= public::PAGE_BYTES);
        count += response.items.len();
    }
    assert_eq!(count, 132);
    view.devices[0].online = false;
    let changed = public::Snapshot::new(view).unwrap();
    assert_eq!(
        serde_json::to_value(
            changed
                .page(public::ReadRequest {
                    version: Some(hash.clone()),
                    after: Some("64".into())
                })
                .unwrap_err()
        )
        .unwrap(),
        serde_json::json!({"code":"roster_changed"})
    );
    for bad in [
        "00",
        "01",
        "-1",
        "+1",
        "1.0",
        "99999999999999999999999999999",
        "132",
    ] {
        assert!(first
            .page(public::ReadRequest {
                version: Some(hash.clone()),
                after: Some(bad.into())
            })
            .is_err());
    }
}

#[tokio::test]
async fn member_removal_retires_cache_and_rejects_late_pages_but_offline_keeps_it() {
    let account = Account::new();
    let (store, _, peer) = roster::tests::joined(&account);
    let file_io = FileIo::start(Limits::default()).unwrap();
    let files = file_io.clone();
    file_io
        .run(&Cancellation::default(), move |io| {
            let local = Local::new(projects(1)).unwrap();
            let mut receiver =
                Receiver::begin(&store, &peer, &local.version, files.clone()).unwrap();
            receiver
                .append(&store, &local.page(None).unwrap(), io, || true)
                .unwrap();
            let path = store.root.join(format!("rosters/{peer}.json"));
            let bytes = fs::read(&path).unwrap();
            let current = context(&store).unwrap().membership.unwrap();
            let mut member = crate::bbs_sync::control::Member {
                device_id: peer.clone(),
                membership_id: "peer-membership".into(),
                name: "Renamed while offline".into(),
                online: false,
                public_key: "unused-public-key".into(),
                role: crate::bbs_sync::control::Role::Member,
                last_seen_at: 0,
            };
            roster::persist_members(&store, Some(&current), &[member.clone()]).unwrap();
            assert_eq!(fs::read(&path).unwrap(), bytes);
            let proof = roster::reference::Peer::load(&store, &peer)
                .unwrap()
                .unwrap();
            let source = Local::new(projects(70)).unwrap();
            let mut late = Receiver::begin(&store, &peer, &source.version, files).unwrap();
            let first = source.page(None).unwrap();
            late.append(&store, &first, io, || true).unwrap();
            roster::persist_members(&store, Some(&current), &[]).unwrap();
            assert!(!path.exists());
            assert!(proof.check(&store).is_err());
            let tail = source.page(first.next.as_deref()).unwrap();
            assert!(late.append(&store, &tail, io, || true).is_err());
            assert!(!path.exists());
            member.membership_id = "new-peer-incarnation".into();
            roster::persist_members(&store, Some(&current), &[member]).unwrap();
            assert!(late.append(&store, &tail, io, || true).is_err());
            assert!(!path.exists());
        })
        .await
        .unwrap();
    file_io.stop_and_wait().await;
}
