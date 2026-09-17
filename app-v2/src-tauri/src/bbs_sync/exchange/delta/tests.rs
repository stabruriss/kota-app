use super::*;
use crate::bbs_sync::{
    control::{Member, Membership, Role},
    transport::{Cancellation, NetworkHost},
    DeviceIdentity,
};
use std::fs;

fn large_catalog(count: usize) -> Catalog {
    use crate::bbs_sync::{raw_sha256, GroupState, LedgerEntry, PostVersion, SyncState};
    let mut state = SyncState::new();
    state.groups.insert(
        "group".into(),
        GroupState {
            shared_threads: std::collections::BTreeSet::from(["thread".into()]),
            ..Default::default()
        },
    );
    for i in 0..count {
        let id = format!("post-{i:08}");
        let version = raw_sha256(id.as_bytes());
        let row = LedgerEntry {
            thread_id: "thread".into(),
            post_id: id.clone(),
            version_id: version.clone(),
            body_stamp: Some(FileStamp {
                len: 100,
                modified_ns: 1,
                changed_ns: 1,
                dev: 1,
                ino: 1,
            }),
            descriptor: Some(PostVersion {
                thread_id: "thread".into(),
                post_id: id,
                version_id: version,
                size_bytes: 100,
                kind: "reply".into(),
                attachments: vec![],
                avatar: Some(crate::bbs_sync::AvatarSidecar::none()),
            }),
            ..Default::default()
        };
        state.ledger.insert(row.key().unwrap(), row);
    }
    Catalog::from_state(&state, "group").unwrap()
}

#[test]
fn oversized_cache_falls_back_without_truncating_full_catalog_pages() {
    let catalog = large_catalog(50_000);
    assert!(cache_bytes(&catalog).is_none());
    let mut after = None;
    let mut count = 0;
    loop {
        let (page, issues) = catalog
            .page(crate::bbs_sync::ManifestPhase::Versions, after.as_ref())
            .unwrap();
        assert!(issues.is_empty());
        count += page.items.len();
        after = page.next;
        if after.is_none() {
            break;
        }
    }
    assert_eq!(count, 50_000);
}

#[tokio::test]
async fn cancelled_streaming_cache_write_cleans_temp_and_releases_real_file_worker() {
    let r = root();
    let store = StateStore::at(&r.0);
    let host = NetworkHost::start().await.unwrap();
    let peers = (20..23)
        .map(|seed| peer(seed, "member"))
        .collect::<Vec<_>>();
    let mut c = Cache::default();
    c.members(peers.clone());
    let catalog = large_catalog(20_000);
    let s = Snapshot {
        revision: catalog.revision(),
        cache_bytes: cache_bytes(&catalog),
        catalog: Arc::new(catalog),
        roster: None,
        issues: vec![],
        omitted: 0,
    };
    assert!(s.cache_bytes.unwrap() > 2 * 1024 * 1024);
    for p in peers {
        let start = c
            .start(&p.remote_device_id, Some("7".repeat(64)), Instant::now())
            .unwrap();
        c.complete(&start, s.clone(), true, Instant::now());
    }
    let flush = c.flush(&store);
    let root = store.root.clone();
    host.execute(move |ctx| async move {
        let cancel = Cancellation::default();
        let stop = cancel.clone();
        let files = ctx.io.clone();
        let task = tokio::spawn(async move {
            files
                .run_when_available(&stop, move |io| flush.run(io))
                .await
        });
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let writing = fs::read_dir(&root)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .any(|e| {
                        e.file_name().to_string_lossy().starts_with(".state-")
                            && e.metadata().is_ok_and(|m| m.len() > 0)
                    });
                if writing {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        cancel.cancel();
        let result = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            result,
            Err(crate::bbs_sync::transport::Error::Cancelled)
                | Ok(Err(crate::bbs_sync::transport::Error::Cancelled))
        ));
        // Await cancellation alone can precede the blocking worker returning.
        // Real admission of the next job proves the worker and permit retired.
        tokio::time::timeout(
            Duration::from_secs(2),
            ctx.io
                .run_when_available(&Cancellation::default(), move |_| {
                    assert!(!root.join("relay-catalogs.json").exists());
                    assert!(!fs::read_dir(root).unwrap().any(|e| e
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".state-")));
                }),
        )
        .await
        .unwrap()?;
        Ok(())
    })
    .await
    .unwrap();
    host.stop();
}

fn peer(seed: u8, membership: &str) -> PeerIdentity {
    let own = DeviceIdentity::from_seed([19; 32]).unwrap();
    let other = DeviceIdentity::from_seed([seed; 32]).unwrap();
    let group = Membership {
        group_id: "group-delta".into(),
        worker_url: "https://example.test".into(),
        membership_id: "local-member".into(),
        role: Role::Owner,
    };
    let members = [&own, &other]
        .into_iter()
        .enumerate()
        .map(|(n, key)| Member {
            device_id: key.device_id().unwrap(),
            public_key: key.public_key.clone(),
            name: "test".into(),
            role: Role::Member,
            membership_id: if n == 0 { "local-member" } else { membership }.into(),
            online: true,
            last_seen_at: 0,
        })
        .collect::<Vec<_>>();
    PeerIdentity::current(&group, &members, &own, &other.device_id().unwrap()).unwrap()
}
fn snapshot() -> Snapshot {
    let catalog = Catalog::default();
    Snapshot {
        revision: catalog.revision(),
        cache_bytes: cache_bytes(&catalog),
        catalog: Arc::new(catalog),
        roster: None,
        issues: vec![],
        omitted: 0,
    }
}
async fn flush(cache: &mut Cache, store: &StateStore, host: &NetworkHost) {
    let job = cache.flush(store);
    let generation = job.generation;
    let stamp = host
        .execute(move |ctx| async move {
            ctx.io
                .run_when_available(&Cancellation::default(), move |io| job.run(io))
                .await?
        })
        .await
        .unwrap();
    cache.flushed(generation, stamp);
}
struct Root(PathBuf);
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn root() -> Root {
    Root(std::env::temp_dir().join(format!("bbs-delta-{}", uuid::Uuid::new_v4())))
}

#[tokio::test]
async fn acknowledged_snapshot_requires_durable_cache_exact_pair_and_revision() {
    let r = root();
    let store = StateStore::at(&r.0);
    let host = NetworkHost::start().await.unwrap();
    let p = peer(20, "remote-member");
    let other = peer(21, "third-member");
    let now = Instant::now();
    let mut c = Cache::default();
    c.members([p.clone(), other.clone()]);
    let s = snapshot();
    let received = "7".repeat(64);
    let start = c
        .start(&p.remote_device_id, Some(received.clone()), now)
        .unwrap();
    assert!(start.checkpoint.is_none());
    c.complete(&start, s.clone(), true, now);
    assert!(c
        .start(&p.remote_device_id, Some(received.clone()), now)
        .unwrap()
        .checkpoint
        .is_none());
    flush(&mut c, &store, &host).await;
    let path = store.root.join("relay-catalogs.json");
    assert!(path.exists());
    let start = c
        .start(&p.remote_device_id, Some(received.clone()), now)
        .unwrap();
    assert_eq!(start.checkpoint.as_deref(), Some(received.as_str()));
    assert!(start.select(&s, Some(&s.revision)).1);
    assert!(!start.select(&s, Some(&"0".repeat(64))).1);
    assert!(!start.select(&s, None).1);
    assert!(c
        .start(&other.remote_device_id, Some(received.clone()), now)
        .unwrap()
        .checkpoint
        .is_none());
    // Restart ignores previous boot's cache even when its bytes are intact.
    let mut restarted = Cache::default();
    restarted.members([p.clone()]);
    assert!(restarted
        .start(&p.remote_device_id, Some(received.clone()), now)
        .unwrap()
        .checkpoint
        .is_none());
    // An externally lost/corrupt cache is detected only on round-external IO.
    fs::write(&path, b"broken").unwrap();
    flush(&mut c, &store, &host).await;
    assert!(c
        .start(&p.remote_device_id, Some(received), now)
        .unwrap()
        .checkpoint
        .is_none());
    host.stop();
}

#[tokio::test]
async fn manual_daily_membership_and_late_completion_cannot_bypass_full_repair() {
    let r = root();
    let store = StateStore::at(&r.0);
    let host = NetworkHost::start().await.unwrap();
    let p = peer(20, "remote-member");
    let now = Instant::now();
    let mut c = Cache::default();
    c.members([p.clone()]);
    let s = snapshot();
    let checkpoint = Some("7".repeat(64));
    let initial = c
        .start(&p.remote_device_id, checkpoint.clone(), now)
        .unwrap();
    c.complete(&initial, s.clone(), true, now);
    flush(&mut c, &store, &host).await;
    assert!(!c.due(
        &p.remote_device_id,
        now + FULL_INTERVAL - Duration::from_secs(1)
    ));
    assert!(c.due(&p.remote_device_id, now + FULL_INTERVAL));
    assert!(c
        .start(&p.remote_device_id, checkpoint.clone(), now + FULL_INTERVAL)
        .unwrap()
        .checkpoint
        .is_none());
    let in_flight = c
        .start(&p.remote_device_id, checkpoint.clone(), now)
        .unwrap();
    c.force_full();
    c.complete(&in_flight, s.clone(), true, now);
    flush(&mut c, &store, &host).await;
    assert!(c.due(&p.remote_device_id, now)); // late old full cannot consume Manual
    let manual = c
        .start(&p.remote_device_id, checkpoint.clone(), now)
        .unwrap();
    c.complete(&manual, s.clone(), true, now);
    flush(&mut c, &store, &host).await;
    assert!(!c.due(&p.remote_device_id, now));
    let replaced = peer(20, "rejoined-member");
    c.members([replaced.clone()]);
    c.complete(&manual, s, true, now);
    flush(&mut c, &store, &host).await;
    assert!(c.due(&p.remote_device_id, now));
    assert!(c
        .start(&p.remote_device_id, checkpoint, now)
        .unwrap()
        .checkpoint
        .is_none());
    host.stop();
}

#[tokio::test]
async fn cache_write_failure_and_budget_eviction_only_cost_full_manifests() {
    let r = root();
    let store = StateStore::at(&r.0);
    let host = NetworkHost::start().await.unwrap();
    let now = Instant::now();
    let peers = (20..25)
        .map(|seed| peer(seed, "member"))
        .collect::<Vec<_>>();
    let mut c = Cache::default();
    c.members(peers.clone());
    for p in &peers {
        let start = c
            .start(&p.remote_device_id, Some("7".repeat(64)), now)
            .unwrap();
        let mut s = snapshot();
        s.cache_bytes = Some(PAIR_BYTES); // budget policy, not a fake IO measurement
        c.complete(&start, s, true, now);
    }
    assert!(
        c.peers
            .values()
            .filter_map(|p| p.candidate.as_ref())
            .map(|c| c.snapshot.bytes)
            .sum::<usize>()
            < TOTAL_BYTES
    );
    assert_eq!(
        c.peers.values().filter(|p| p.candidate.is_some()).count(),
        3
    );
    fs::create_dir_all(store.root.join("relay-catalogs.json")).unwrap(); // atomic rename must fail
    flush(&mut c, &store, &host).await;
    for p in &peers {
        assert!(c
            .start(&p.remote_device_id, Some("7".repeat(64)), now)
            .unwrap()
            .checkpoint
            .is_none());
        assert!(!c.due(&p.remote_device_id, now)); // no immediate retry storm on cache failure
    }
    host.stop();
}
