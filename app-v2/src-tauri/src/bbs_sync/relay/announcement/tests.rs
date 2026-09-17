use super::*;
use crate::bbs_sync::{control::Role, transport::Limits};
use std::{
    os::unix::fs::{symlink, PermissionsExt},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};

const NOW: u64 = 1_790_000_000_000;
const INSTANCE: &str = "announcement-instance-one";
const NEXT_INSTANCE: &str = "announcement-instance-two";

struct Fixture {
    root: PathBuf,
    store: StateStore,
    identity: DeviceIdentity,
    outbox: Outbox,
    alive: Arc<AtomicBool>,
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("bbs-relay-announcement-{}", uuid::Uuid::new_v4()));
        let store = StateStore::at(&root);
        let identity = DeviceIdentity::from_seed([43; 32]).unwrap();
        let scope = Scope::new(
            &Membership {
                group_id: "announcement-group-one".into(),
                worker_url: "https://relay.example.com".into(),
                membership_id: "announcement-membership-one".into(),
                role: Role::Owner,
            },
            &identity,
        )
        .unwrap();
        let alive = Arc::new(AtomicBool::new(true));
        let guard = alive.clone();
        let outbox = Outbox::new(
            &store,
            scope,
            FileIo::start(Limits::default()).unwrap(),
            Arc::new(move || guard.load(Ordering::Acquire)),
        );
        Self {
            root,
            store,
            identity,
            outbox,
            alive,
        }
    }
    fn restored(&self) -> Outbox {
        Outbox::new(
            &self.store,
            self.outbox.scope.clone(),
            self.outbox.files.clone(),
            self.outbox.current.clone(),
        )
    }
    fn bytes(&self) -> Vec<u8> {
        fs::read(&self.outbox.path).unwrap()
    }
    async fn prepare(&self, digit: char) -> Intent {
        self.outbox
            .prepare(
                &digit.to_string().repeat(64),
                INSTANCE,
                NOW,
                &Cancellation::default(),
            )
            .await
            .unwrap()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn success(sent: &Intent) -> Receipt {
    Receipt::decode(
        &serde_json::to_vec(&serde_json::json!({
            "ok":true, "current":sent.request_id(), "changed":true,
        }))
        .unwrap(),
    )
    .unwrap()
}
fn conflict(current: &str) -> Receipt {
    Receipt::decode(
        &serde_json::to_vec(&serde_json::json!({
            "ok":false, "current":current, "changed":false,
        }))
        .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn restart_recovers_exact_pending_and_new_instance_replaces_it_atomically() {
    let f = Fixture::new();
    let cancel = Cancellation::default();
    assert!(!f.root.exists());
    assert!(f.outbox.pending(&cancel).await.unwrap().is_none());
    assert!(
        !f.root.exists(),
        "read-only empty outbox must not create identity/cache"
    );
    let original = f.prepare('a').await;
    assert_eq!(
        fs::metadata(&f.outbox.path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(f.outbox.path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let bytes = f.bytes();
    let restore = f.restored();
    let pending = restore.pending(&cancel).await.unwrap().unwrap();
    assert!(pending == original);
    let after_restart = restore
        .prepare(original.revision(), INSTANCE, NOW + 86_400_000, &cancel)
        .await
        .unwrap()
        .unwrap();
    assert!(after_restart == original);
    assert_eq!(f.bytes(), bytes);
    let (first_proof, first_body) = original.signed(&f.identity, NOW).unwrap();
    let (new_proof, new_body) = after_restart.signed(&f.identity, NOW + 86_400_000).unwrap();
    assert_eq!(first_body, new_body);
    assert!(first_proof.matches_body(&new_body) && new_proof.matches_body(&first_body));
    assert_ne!(first_proof.headers().unwrap(), new_proof.headers().unwrap());
    let next = restore
        .prepare(
            original.revision(),
            NEXT_INSTANCE,
            NOW + 86_400_000,
            &cancel,
        )
        .await
        .unwrap()
        .unwrap();
    assert_ne!(next.request_id(), original.request_id());
    assert_eq!(next.instance(), NEXT_INSTANCE);
    assert!(next.body.previous.is_none());
    let replaced = f.bytes();
    assert!(matches!(
        restore
            .complete(&pending, success(&pending), NOW, &cancel)
            .await
            .unwrap(),
        Completion::Ignored
    ));
    assert_eq!(f.bytes(), replaced);
    assert!(matches!(
        restore
            .complete(&next, success(&next), NOW, &cancel)
            .await
            .unwrap(),
        Completion::Confirmed
    ));
    let confirmed = f.bytes();
    assert!(restore
        .prepare(
            original.revision(),
            NEXT_INSTANCE,
            NOW + 90_000_000,
            &cancel
        )
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        f.bytes(),
        confirmed,
        "stable confirmed revision must not write again"
    );
    assert!(!f.store.identity_path().exists() && !f.store.control_path().exists());
}

#[tokio::test]
async fn cas_miss_creates_new_immutable_request_for_same_desired_revision() {
    let f = Fixture::new();
    let cancel = Cancellation::default();
    let original = f.prepare('a').await;
    let original_bytes = original.signed(&f.identity, NOW).unwrap().1;
    let Completion::Rebased(retry) = f
        .outbox
        .complete(
            &original,
            conflict("worker-already-has-this-id"),
            NOW + 1,
            &cancel,
        )
        .await
        .unwrap()
    else {
        panic!("CAS conflict must retain a new pending intent")
    };
    assert_ne!(original.request_id(), retry.request_id());
    assert_eq!(retry.revision(), original.revision());
    assert_eq!(retry.instance(), original.instance());
    assert_eq!(
        retry.body.previous.as_deref(),
        Some("worker-already-has-this-id")
    );
    assert_eq!(
        original.signed(&f.identity, NOW + 2).unwrap().1,
        original_bytes
    );
    assert!(f.restored().pending(&cancel).await.unwrap().unwrap() == retry);
    let before = f.bytes();
    assert!(matches!(
        f.outbox
            .complete(&original, success(&original), NOW, &cancel)
            .await
            .unwrap(),
        Completion::Ignored
    ));
    assert_eq!(f.bytes(), before);
    assert!(matches!(
        f.outbox
            .complete(&retry, success(&retry), NOW, &cancel)
            .await
            .unwrap(),
        Completion::Confirmed
    ));
}

#[tokio::test]
async fn newest_revision_survives_late_receipt_and_return_to_previously_confirmed_revision() {
    let f = Fixture::new();
    let cancel = Cancellation::default();
    let a = f.prepare('a').await;
    f.outbox
        .complete(&a, success(&a), NOW, &cancel)
        .await
        .unwrap();
    let b = f.prepare('b').await;
    // B may already have reached Worker. Returning to A cannot simply drop B
    // and declare the previous A confirmation current.
    let a2 = f.prepare('a').await;
    assert_ne!(a2.request_id(), a.request_id());
    assert_ne!(a2.request_id(), b.request_id());
    let bytes = f.bytes();
    assert!(matches!(
        f.outbox
            .complete(&b, success(&b), NOW, &cancel)
            .await
            .unwrap(),
        Completion::Ignored
    ));
    assert_eq!(f.bytes(), bytes);
    let Completion::Rebased(a3) = f
        .outbox
        .complete(&a2, conflict(b.request_id()), NOW + 2, &cancel)
        .await
        .unwrap()
    else {
        panic!("latest A remains pending after an old B reached Worker")
    };
    assert_eq!(a3.revision(), a.revision());
    f.outbox
        .complete(&a3, success(&a3), NOW, &cancel)
        .await
        .unwrap();
    assert!(f
        .outbox
        .prepare(a.revision(), INSTANCE, NOW, &cancel)
        .await
        .unwrap()
        .is_none());
    let cache: Cache = serde_json::from_slice(&f.bytes()).unwrap();
    assert!(cache.pending.is_none());
    assert_eq!(cache.announced.unwrap().request_id, a3.request_id());
}

#[tokio::test]
async fn group_membership_origin_and_device_scope_cannot_replay_each_others_pending() {
    let f = Fixture::new();
    let cancel = Cancellation::default();
    let old = f.prepare('a').await;
    let original_scope = f.outbox.scope.clone();
    let mut prior = old.clone();
    for field in 0..4 {
        let mut scope = original_scope.clone();
        match field {
            0 => scope.group = "announcement-group-two".into(),
            1 => scope.membership = "announcement-membership-two".into(),
            2 => scope.origin = "https://next.example.com".into(),
            _ => scope.device = "a".repeat(64),
        }
        let next = Outbox::new(&f.store, scope, f.outbox.files.clone(), Arc::new(|| true));
        assert!(next.pending(&cancel).await.unwrap().is_none());
        let pending = next
            .prepare(&"b".repeat(64), NEXT_INSTANCE, NOW, &cancel)
            .await
            .unwrap()
            .unwrap();
        let bytes = f.bytes();
        assert!(matches!(
            next.complete(&prior, success(&prior), NOW, &cancel)
                .await
                .unwrap(),
            Completion::Ignored
        ));
        assert!(matches!(
            f.outbox
                .complete(&old, success(&old), NOW, &cancel)
                .await
                .unwrap(),
            Completion::Ignored
        ));
        assert_eq!(f.bytes(), bytes);
        prior = pending;
    }
    f.alive.store(false, Ordering::Release);
    assert!(matches!(
        f.outbox
            .prepare(&"c".repeat(64), INSTANCE, NOW, &cancel)
            .await,
        Err(Error::Unauthorized)
    ));
    assert_eq!(
        fs::read_dir(f.outbox.path.parent().unwrap())
            .unwrap()
            .count(),
        1
    );
}

#[tokio::test]
async fn failed_durable_write_returns_no_sendable_intent_and_cleans_temporary_file() {
    let f = Fixture::new();
    let count = Arc::new(AtomicUsize::new(0));
    let blocker = f.outbox.path.clone();
    let mut outbox = f.outbox.clone();
    // Force a real atomic-rename failure after initial read and serialization,
    // without platform-dependent permission tricks or a mocked writer.
    outbox.current = Arc::new(move || {
        if count.fetch_add(1, Ordering::AcqRel) == 1 {
            fs::create_dir_all(&blocker).unwrap();
        }
        true
    });
    assert!(matches!(
        outbox
            .prepare(&"a".repeat(64), INSTANCE, NOW, &Cancellation::default())
            .await,
        Err(Error::Io)
    ));
    assert!(f.outbox.path.is_dir());
    assert_eq!(
        fs::read_dir(f.outbox.path.parent().unwrap())
            .unwrap()
            .count(),
        1,
        "failed write removes create-new temp"
    );
    assert!(!f.store.identity_path().exists());
    fs::remove_dir(&f.outbox.path).unwrap();
    assert!(f
        .outbox
        .pending(&Cancellation::default())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn cancellation_or_unusable_receipt_keeps_pending_exactly_and_cache_loss_only_reannounces() {
    let f = Fixture::new();
    let pending = f.prepare('a').await;
    let before = f.bytes();
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert!(matches!(
        f.outbox
            .complete(&pending, success(&pending), NOW, &cancelled)
            .await,
        Err(Error::Cancelled)
    ));
    assert_eq!(f.bytes(), before);
    let cancel = Cancellation::default();
    let wrong =
        Receipt::decode(br#"{"ok":true,"current":"some-other-request-id","changed":false}"#)
            .unwrap();
    assert!(matches!(
        f.outbox.complete(&pending, wrong, NOW, &cancel).await,
        Err(Error::Protocol)
    ));
    assert_eq!(f.bytes(), before);
    for raw in [
        br#"{"ok":true,"changed":false}"#.as_slice(),
        br#"{"ok":true,"current":null,"changed":false}"#.as_slice(),
        br#"{"ok":false,"current":null,"changed":true}"#.as_slice(),
        br#"{"ok":true,"current":"some-other-request-id","changed":false,"resetAtUtc":0}"#
            .as_slice(),
    ] {
        assert!(Receipt::decode(raw).is_err());
    }
    assert!(f.restored().pending(&cancel).await.unwrap().unwrap() == pending);
    fs::remove_file(&f.outbox.path).unwrap();
    let replacement = f.prepare('a').await;
    assert_ne!(replacement.request_id(), pending.request_id());
    assert_eq!(replacement.revision(), pending.revision());
    assert!(
        replacement.body.previous.is_none(),
        "Worker CAS will recover its actual current ID"
    );
    assert!(
        !f.store.state_path().exists()
            && !f.store.identity_path().exists()
            && !f.store.control_path().exists()
    );
}

#[tokio::test]
async fn malformed_oversized_or_symlink_cache_is_rejected_without_replacement() {
    let f = Fixture::new();
    let _ = f.prepare('a').await;
    let cancel = Cancellation::default();
    for bytes in [
        b"broken json".to_vec(),
        vec![b' '; CACHE_BYTES + 1],
        br#"{"schema":99}"#.to_vec(),
    ] {
        fs::write(&f.outbox.path, &bytes).unwrap();
        assert!(f.outbox.pending(&cancel).await.is_err());
        assert!(f
            .outbox
            .prepare(&"b".repeat(64), INSTANCE, NOW, &cancel)
            .await
            .is_err());
        assert_eq!(f.bytes(), bytes);
    }
    let external = f.root.join("unrelated");
    fs::write(&external, b"must not read or overwrite this").unwrap();
    fs::remove_file(&f.outbox.path).unwrap();
    symlink(&external, &f.outbox.path).unwrap();
    assert!(matches!(f.outbox.pending(&cancel).await, Err(Error::Io)));
    assert_eq!(
        fs::read(external).unwrap(),
        b"must not read or overwrite this"
    );
}
