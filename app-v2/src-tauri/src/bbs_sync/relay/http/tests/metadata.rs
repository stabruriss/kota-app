//! Real HTTP worker queues with isolated, scripted relay replies. The recorded
//! poll bytes are from BbsGroup.fetch; this is not a live Cloudflare test.
use super::*;
use crate::bbs_sync::{
    control::{Membership, Role},
    relay::{
        announcement::{Completion, Outbox, Receipt, Scope},
        discovery::Page,
        handshake::Handshake,
        metadata::{self, Client, WakeIntent, WakeResult},
        proof::CanonicalJson,
        wire::Declarations,
    },
    transport::FileIo,
    StateStore,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::VecDeque, io::Read};

#[derive(Clone, PartialEq, Eq, Debug)]
struct Observed {
    method: String,
    url: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}
#[derive(Default)]
struct Script {
    replies: Mutex<VecDeque<(u16, Vec<u8>)>>,
    calls: Mutex<Vec<Observed>>,
    created: AtomicUsize,
    discarded: AtomicUsize,
    blocked: Mutex<bool>,
    wake: Condvar,
}
impl Script {
    fn push(&self, status: u16, value: Value) {
        self.raw(status, serde_json::to_vec(&value).unwrap());
    }
    fn raw(&self, status: u16, bytes: Vec<u8>) {
        self.replies.lock().unwrap().push_back((status, bytes));
    }
    fn release(&self) {
        *self.blocked.lock().unwrap() = false;
        self.wake.notify_all();
    }
}
struct ScriptBackend(Arc<Script>);
impl Backend for ScriptBackend {
    fn discard(&mut self) {
        self.0.discarded.fetch_add(1, Ordering::SeqCst);
    }
    fn run(
        &mut self,
        _: &Endpoint,
        request: &SignedRequest,
        body: Option<&Body>,
        response: &mut Buffer,
        _: &Guard,
    ) -> Result<Parts> {
        let mut bytes = Vec::new();
        if let Some(body) = body {
            body.reader().read_to_end(&mut bytes).unwrap();
        }
        assert!(request.matches_body(&bytes));
        self.0.calls.lock().unwrap().push(Observed {
            method: request.method().into(),
            url: request.url(),
            headers: request.headers().unwrap(),
            body: bytes,
        });
        let mut blocked = self.0.blocked.lock().unwrap();
        while *blocked {
            blocked = self.0.wake.wait(blocked).unwrap();
        }
        let (status, bytes) = self
            .0
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("explicit reply per request");
        assert!(bytes.len() <= response.spare_mut().len());
        response.spare_mut()[..bytes.len()].copy_from_slice(&bytes);
        response.advance(bytes.len())?;
        Ok(Parts {
            status,
            headers: BTreeMap::from([("content-type".into(), vec!["application/json".into()])]),
        })
    }
}
struct Rig {
    pool: HttpPool,
    client: Client,
    script: Arc<Script>,
    limits: Limits,
    cancel: Cancellation,
    allowed: Arc<AtomicBool>,
}
impl Drop for Rig {
    fn drop(&mut self) {
        self.script.release();
        self.pool.stop();
    }
}
fn membership(index: usize) -> Membership {
    let f = fixture();
    Membership {
        group_id: f.contexts[index].peer.group_id.clone(),
        worker_url: f.contexts[index].origin.clone(),
        membership_id: f.contexts[index].peer.local_membership_id.clone(),
        role: Role::Member,
    }
}
impl Rig {
    async fn new(index: usize) -> Self {
        let limits = Limits::default();
        let script = Arc::new(Script::default());
        let s = script.clone();
        let pool = HttpPool::start_with(limits.clone(), move |_| {
            s.created.fetch_add(1, Ordering::SeqCst);
            Box::new(ScriptBackend(s.clone()))
        })
        .await
        .unwrap();
        let cancel = Cancellation::default();
        let allowed = Arc::new(AtomicBool::new(true));
        let a = allowed.clone();
        let client = Client::new(
            pool.bind(&membership(index).worker_url).unwrap(),
            fixture().identities[index].clone(),
            membership(index),
            cancel.clone(),
            Arc::new(move || a.load(Ordering::Acquire)),
        )
        .unwrap();
        Self {
            pool,
            client,
            script,
            limits,
            cancel,
            allowed,
        }
    }
}
fn deadline() -> Instant {
    Instant::now() + PROGRESS_TIMEOUT
}
fn recorded(key: &str) -> Vec<u8> {
    let mut data: BTreeMap<String, CanonicalJson> = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../relays/laughing-man-cloudflare/tests/fixtures/relay-discovery-v1.json"
    )))
    .unwrap();
    data.remove(key).unwrap().into_bytes()
}
fn page(items: Vec<Value>, next: Option<&str>, boot: &str) -> Value {
    json!({"boot":boot,"items":items,"next":next})
}
fn item(id: &str) -> Value {
    json!({"device":id,"announcement":null,"currentWake":null,"wake":null,"handshake":null})
}

#[tokio::test]
async fn malformed_successful_metadata_discards_agents_before_the_next_request() {
    let rig = Rig::new(0).await;
    rig.script.push(200, json!({"boot":"not-a-valid-page"}));
    assert!(matches!(
        rig.client.poll(fixture().now, deadline()).await,
        Err(Error::Protocol)
    ));
    until(|| rig.script.discarded.load(Ordering::SeqCst) > 0).await;
    rig.script
        .push(200, page(vec![], None, &fixture().contexts[0].boot));
    assert!(rig.client.poll(fixture().now, deadline()).await.is_ok());
    assert_eq!(
        rig.script.created.load(Ordering::SeqCst),
        2,
        "discard replaces agents, never the account workers"
    );
}

#[tokio::test]
async fn group_poll_uses_one_paged_read_and_preserves_actual_worker_declarations() {
    let rig = Rig::new(0).await;
    let f = fixture();
    rig.script.raw(200, recorded("opened"));
    let read = rig.client.poll(f.now, deadline()).await.unwrap();
    let direct = Page::decode(
        &recorded("opened"),
        &membership(0).group_id,
        &f.contexts[0].peer.local_device_id,
        None,
    )
    .unwrap();
    let id = &f.contexts[0].peer.remote_device_id;
    assert_eq!(read.boot, direct.boot);
    assert_eq!(
        read.items[id]
            .handshake
            .as_ref()
            .unwrap()
            .client
            .as_ref()
            .unwrap()
            .bytes(),
        direct
            .items
            .iter()
            .find(|v| &v.device == id)
            .unwrap()
            .handshake
            .as_ref()
            .unwrap()
            .client
            .as_ref()
            .unwrap()
            .bytes()
    );
    assert_eq!(rig.script.created.load(Ordering::SeqCst), 2);
    let ids: Vec<_> = (1..=32).map(|n| format!("{n:064x}")).collect();
    for (i, chunk) in ids.chunks(16).enumerate() {
        rig.script.push(
            200,
            page(
                chunk.iter().map(|v| item(v)).collect(),
                if i == 0 { Some(&ids[15]) } else { None },
                &f.contexts[0].boot,
            ),
        );
    }
    let read = rig.client.poll(f.now, deadline()).await.unwrap();
    assert_eq!(
        read.items.len(),
        32,
        "membership bound must not truncate a legal full group"
    );
    let calls = rig.script.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert!(calls.iter().all(|r| r.method == "GET" && r.body.is_empty()));
    assert!(calls[2].url.ends_with(&format!("&after={}", ids[15])));
    assert_eq!(rig.script.created.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn mixed_boot_repeated_cursor_and_oversize_group_never_return_partial_pages() {
    let f = fixture();
    let ids: Vec<_> = (1..=33).map(|n| format!("{n:064x}")).collect();
    for case in 0..3 {
        let rig = Rig::new(0).await;
        rig.script.push(
            200,
            page(vec![item(&ids[0])], Some(&ids[0]), &f.contexts[0].boot),
        );
        match case {
            0 => rig
                .script
                .push(200, page(vec![item(&ids[1])], None, "new-boot-incarnation")),
            1 => rig
                .script
                .push(200, page(vec![item(&ids[0])], None, &f.contexts[0].boot)),
            _ => {
                rig.script.push(
                    200,
                    page(
                        ids[1..17].iter().map(|v| item(v)).collect(),
                        Some(&ids[16]),
                        &f.contexts[0].boot,
                    ),
                );
                rig.script.push(
                    200,
                    page(
                        ids[17..33].iter().map(|v| item(v)).collect(),
                        None,
                        &f.contexts[0].boot,
                    ),
                );
            }
        }
        assert!(
            matches!(rig.client.poll(f.now, deadline()).await, Err(e) if e == if case == 0 { Error::RelaySessionLost } else { Error::Protocol })
        );
    }
}

#[tokio::test]
async fn wake_busy_retry_keeps_exact_proof_body_and_cas_cannot_ack_another_wake() {
    let rig = Rig::new(0).await;
    let f = fixture();
    let c = &f.contexts[0];
    let intent = WakeIntent::new(
        &f.identities[0],
        &membership(0),
        &c.peer,
        &c.local_instance,
        &c.remote_instance,
        Some(&c.wake),
        f.now,
    )
    .unwrap();
    let call = rig.client.wake(&intent, f.now, deadline()).unwrap();
    rig.script
        .push(429, json!({"ok":false,"error":"relay_backpressure"}));
    assert!(matches!(call.execute().await, Err(Error::Busy)));
    assert_eq!(
        rig.script.calls.lock().unwrap().len(),
        1,
        "no hidden automatic retry"
    );
    let first = rig.script.calls.lock().unwrap()[0].clone();
    let body: Value = serde_json::from_slice(&first.body).unwrap();
    let expected = crate::bbs_sync::raw_sha256(
        &serde_json::to_vec(&json!([
            "kota-bbs-relay.wake-id.v1",
            c.peer.group_id,
            c.peer.local_device_id,
            c.peer.local_membership_id,
            body["requestId"]
        ]))
        .unwrap(),
    );
    rig.script.push(
        200,
        json!({"ok":true,"current":expected,"changed":true,"replacedWake":c.wake}),
    );
    let reply = call.execute().await.unwrap();
    assert!(
        matches!(intent.response(reply.json().unwrap()).unwrap(), WakeResult::Accepted(ref id) if id == &expected)
    );
    assert_eq!(rig.script.calls.lock().unwrap()[1], first);
    assert!(matches!(
        intent.response(
            &serde_json::to_vec(&json!({"ok":true,"current":"f".repeat(64),"changed":true}))
                .unwrap()
        ),
        Err(Error::Protocol)
    ));
    assert!(matches!(
        intent
            .response(
                &serde_json::to_vec(&json!({"ok":false,"current":"f".repeat(64),"changed":false}))
                    .unwrap()
            )
            .unwrap(),
        WakeResult::Current(Some(_))
    ));
    assert_eq!(
        rig.script.calls.lock().unwrap().len(),
        2,
        "CAS conflict itself cannot start another wake"
    );
    rig.cancel.cancel();
    assert!(matches!(call.execute().await, Err(Error::Cancelled)));
    assert_eq!(rig.script.calls.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn blocked_metadata_cancel_and_rebind_discard_old_result_without_more_workers() {
    let rig = Rig::new(0).await;
    *rig.script.blocked.lock().unwrap() = true;
    rig.script.raw(200, recorded("initial"));
    let client = rig.client.clone();
    let task = tokio::spawn(async move { client.poll(fixture().now, deadline()).await });
    until(|| rig.script.calls.lock().unwrap().len() == 1).await;
    rig.cancel.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_millis(200), task)
            .await
            .unwrap()
            .unwrap(),
        Err(Error::Cancelled)
    ));
    // A -> B -> A is still a new capability. Releasing a late A reply never
    // gives the old metadata reader authority over the new A binding.
    rig.pool.bind("https://other.example").unwrap();
    rig.pool.bind(&membership(0).worker_url).unwrap();
    rig.script.release();
    assert!(matches!(
        rig.client.poll(fixture().now, deadline()).await,
        Err(Error::Cancelled)
    ));
    assert_eq!(rig.script.calls.lock().unwrap().len(), 1);
    assert_eq!(rig.script.created.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn revoke_during_metadata_http_cannot_publish_a_snapshot() {
    let rig = Rig::new(0).await;
    *rig.script.blocked.lock().unwrap() = true;
    rig.script.raw(200, recorded("initial"));
    let client = rig.client.clone();
    let task = tokio::spawn(async move { client.poll(fixture().now, deadline()).await });
    until(|| rig.script.calls.lock().unwrap().len() == 1).await;
    rig.allowed.store(false, Ordering::Release);
    rig.script.release();
    assert!(matches!(task.await.unwrap(), Err(Error::Unauthorized)));
    assert_eq!(rig.script.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn announce_uses_persisted_intent_and_only_explicit_outbox_confirmation_retires_it() {
    let rig = Rig::new(0).await;
    let f = fixture();
    let root = std::env::temp_dir().join(format!("bbs-relay-meta-{}", uuid::Uuid::new_v4()));
    let store = StateStore::at(&root);
    let files = FileIo::start(rig.limits.clone()).unwrap();
    let scope = Scope::new(&membership(0), &f.identities[0]).unwrap();
    let outbox = Outbox::new(&store, scope.clone(), files.clone(), authorized());
    let intent = outbox
        .prepare(
            &"d".repeat(64),
            &f.contexts[0].local_instance,
            f.now,
            &rig.cancel,
        )
        .await
        .unwrap()
        .unwrap();
    let call = rig.client.announce(&intent, f.now, deadline()).unwrap();
    rig.script
        .push(503, json!({"ok":false,"error":"relay_service_limit"}));
    assert!(matches!(
        call.execute().await,
        Err(Error::CloudflareResourceLimit)
    ));
    let restored = Outbox::new(&store, scope, files, authorized());
    let pending = restored.pending(&rig.cancel).await.unwrap().unwrap();
    assert_eq!(pending.request_id(), intent.request_id());
    let later = rig
        .client
        .announce(&pending, f.now + 10_000, deadline())
        .unwrap();
    rig.script.push(
        200,
        json!({"ok":true,"current":pending.request_id(),"changed":false}),
    );
    let receipt = Receipt::decode(later.execute().await.unwrap().json().unwrap()).unwrap();
    assert!(
        restored.pending(&rig.cancel).await.unwrap().is_some(),
        "HTTP does not clear durable state by itself"
    );
    assert!(matches!(
        restored
            .complete(&pending, receipt, f.now + 10_000, &rig.cancel)
            .await
            .unwrap(),
        Completion::Confirmed
    ));
    assert!(restored.pending(&rig.cancel).await.unwrap().is_none());
    let calls = rig.script.calls.lock().unwrap();
    assert_eq!(calls[0].body, calls[1].body);
    assert_ne!(
        calls[0].headers, calls[1].headers,
        "restart refreshes only proof time/nonce"
    );
    drop(calls);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn ready_and_dual_open_use_http_queues_before_real_mutual_tls() {
    let a = Rig::new(0).await;
    let b = Rig::new(1).await;
    let f = fixture();
    let mut view = crate::bbs_sync::relay::discovery::HandshakeView {
        ready: crate::bbs_sync::relay::discovery::Ready {
            client: true,
            server: true,
        },
        client: None,
        server: None,
        session: None,
        closed: false,
    };
    for (rig, context) in [(&a, &f.contexts[0]), (&b, &f.contexts[1])] {
        rig.script.push(
            200,
            json!({"boot":context.boot,"client":true,"server":true}),
        );
        let call = rig.client.ready(context, f.now, deadline()).unwrap();
        let r = metadata::ready_response(call.execute().await.unwrap().json().unwrap(), context)
            .unwrap();
        assert!(r.client && r.server);
        assert!(matches!(
            metadata::ready_response(
                br#"{"boot":"old-boot-incarnation","client":true,"server":true}"#,
                context
            ),
            Err(Error::RelaySessionLost)
        ));
    }
    let mut client = Handshake::start(
        &f.identities[0],
        f.contexts[0].clone(),
        &view,
        authorized(),
        a.cancel.clone(),
        f.now,
        Instant::now(),
    )
    .unwrap()
    .unwrap();
    let offer = a.client.open(&mut client, f.now, Instant::now()).unwrap();
    a.script
        .push(200, json!({"boot":f.contexts[0].boot,"offered":true}));
    assert!(client
        .response(
            offer.execute().await.unwrap().json().unwrap(),
            f.now,
            Instant::now()
        )
        .unwrap()
        .is_none());
    #[derive(Deserialize)]
    struct Open {
        client: crate::bbs_sync::relay::discovery::Declaration,
        server: Option<crate::bbs_sync::relay::discovery::Declaration>,
    }
    let offered: Open =
        serde_json::from_slice(&a.script.calls.lock().unwrap().last().unwrap().body).unwrap();
    view.client = Some(offered.client);
    let mut server = Handshake::start(
        &f.identities[1],
        f.contexts[1].clone(),
        &view,
        authorized(),
        b.cancel.clone(),
        f.now,
        Instant::now(),
    )
    .unwrap()
    .unwrap();
    let opened = b.client.open(&mut server, f.now, Instant::now()).unwrap();
    // Extract the exact generated declaration via the signed request body;
    // the scripted relay derives only the deterministic public session ID.
    b.script
        .push(429, json!({"ok":false,"error":"relay_backpressure"}));
    assert!(matches!(opened.execute().await, Err(Error::Busy)));
    let raw = b.script.calls.lock().unwrap().last().unwrap().body.clone();
    let dual: Open = serde_json::from_slice(&raw).unwrap();
    assert_eq!(dual.client.bytes(), view.client.as_ref().unwrap().bytes());
    view.server = dual.server;
    view.session = Some(
        Declarations {
            client: view.client.as_ref().unwrap().signed.clone(),
            server: view.server.as_ref().unwrap().signed.clone(),
        }
        .session_id()
        .unwrap(),
    );
    b.script.push(
        200,
        json!({"boot":f.contexts[1].boot,"session":view.session,"closed":false}),
    );
    let mut ts = server
        .response(
            opened.execute().await.unwrap().json().unwrap(),
            f.now,
            Instant::now(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(b.script.calls.lock().unwrap().last().unwrap().body, raw);
    let mut tc = client
        .observe(
            &f.contexts[0].boot,
            &f.contexts[0].wake,
            &view,
            f.now,
            Instant::now(),
        )
        .unwrap()
        .unwrap();
    fn transfer(
        from: &mut crate::bbs_sync::relay::TlsStream,
        to: &mut crate::bbs_sync::relay::TlsStream,
    ) {
        let mut bytes = [0; 16384];
        let n = from.drain_tls(&mut bytes).unwrap();
        let mut pos = 0;
        while pos < n {
            pos += to.receive(&bytes[pos..n.min(pos + 7)]).unwrap();
        }
    }
    for _ in 0..20 {
        transfer(&mut tc, &mut ts);
        transfer(&mut ts, &mut tc);
        if tc.handshake_complete().unwrap() && ts.handshake_complete().unwrap() {
            break;
        }
    }
    assert!(tc.handshake_complete().unwrap() && ts.handshake_complete().unwrap());
    assert_eq!(
        a.script.created.load(Ordering::SeqCst) + b.script.created.load(Ordering::SeqCst),
        4
    );
}
