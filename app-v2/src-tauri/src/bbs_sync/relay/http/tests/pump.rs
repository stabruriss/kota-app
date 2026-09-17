//! In-process relay storage behind the REAL two blocking HTTP workers. This is
//! not a Cloudflare/HTTP-on-the-wire timing or production polling-policy claim.
use super::*;
use crate::bbs_sync::{
    raw_sha256,
    relay::{framing::Channels, proof::SignedAck, pump::Pump, upload::Builder, PendingTls},
    transport::{Context, FileIo, Resource, ResourceKind, DATA_WINDOW, MAX_FRAME},
};
use serde_json::{json, Value};
use std::{collections::VecDeque, fs, io::Read, path::PathBuf};
mod activity;
mod coalesce;
mod coordinator;
mod exchange;
mod reads;

#[derive(Default)]
struct DirectionState {
    next: u64,
    consumed: u64,
    batches: VecDeque<(u64, Vec<u8>)>,
    ack: Option<SignedAck>,
    closed: bool,
}
struct Session {
    devices: [String; 2],
    directions: [DirectionState; 2],
}
#[derive(Default)]
struct RelayState {
    sessions: BTreeMap<String, Session>,
    send_sizes: Vec<usize>,
    receives: usize,
    small_reads: usize,
    acknowledgements: usize,
    created: usize,
    sends_by_device: Vec<(String, usize)>,
    attempts: Vec<(bool, BTreeMap<String, String>, String)>,
    lose_replies: [usize; 2],
    busy_replies: [usize; 2],
    reject: Option<u16>,
    hold_receive: Option<String>,
    final_with_payload: usize,
}
struct Relay(Arc<Mutex<RelayState>>);
impl Backend for Relay {
    fn discard(&mut self) {}
    fn run(
        &mut self,
        _: &Endpoint,
        request: &SignedRequest,
        body: Option<&Body>,
        response: &mut Buffer,
        guard: &Guard,
    ) -> Result<Parts> {
        guard.check()?;
        let mut state = self.0.lock().unwrap();
        let headers = request.headers()?;
        if let Some(status) = state.reject {
            let bytes = b"<html>429 1027 not an authoritative quota sample</html>";
            response.spare_mut()[..bytes.len()].copy_from_slice(bytes);
            response.advance(bytes.len())?;
            return Ok(Parts {
                status,
                headers: BTreeMap::new(),
            });
        }
        let mut content_type = "application/json";
        let bytes = if let Some(read) = request.receive_request() {
            content_type = envelope::CONTENT_TYPE;
            if read.mode == ReceiveMode::Data {
                state.receives += 1;
            } else {
                state.small_reads += 1;
            }
            let mut items = Vec::new();
            let mut bytes = Vec::new();
            let held = state.hold_receive.as_ref() == Some(&headers["x-kota-relay-device"]);
            for (id, cursor) in &read.cursors {
                let session = state.sessions.get(id).ok_or(Error::Closed)?;
                let outgoing = session
                    .devices
                    .iter()
                    .position(|d| d == &headers["x-kota-relay-device"])
                    .ok_or(Error::Unauthorized)?;
                let incoming = &session.directions[1 - outgoing];
                let mut batches = Vec::new();
                if read.mode == ReceiveMode::Data && !held {
                    for (seq, data) in incoming.batches.iter().filter(|(n, _)| n >= cursor) {
                        if bytes.len() + data.len() > MAX_BATCH {
                            break;
                        }
                        batches.push(json!({"sequence":seq.to_string(),"length":data.len()}));
                        bytes.extend_from_slice(data);
                    }
                }
                let ack = if read.mode == ReceiveMode::Probe || held {
                    None
                } else {
                    session.directions[outgoing].ack.clone()
                };
                let terminal_with_data = !batches.is_empty() && session.directions[outgoing].closed;
                items.push(json!({"session":id,"next":incoming.next.to_string(),
                    "consumed":incoming.consumed.to_string(),"closed":incoming.closed || session.directions[outgoing].closed,"batches":batches,
                    "ack":ack}));
                state.final_with_payload += usize::from(terminal_with_data);
            }
            envelope::tests::encode_json(&json!({"boot":read.boot,"items":items}), &bytes)
        } else {
            let id = &headers["x-kota-relay-session"];
            let direction = match headers["x-kota-relay-direction"].as_str() {
                "c2s" => 0,
                "s2c" => 1,
                _ => return Err(Error::Protocol),
            };
            let sequence: u64 = headers["x-kota-relay-sequence"].parse().unwrap();
            let mut bytes = Vec::new();
            body.ok_or(Error::Protocol)?
                .reader()
                .read_to_end(&mut bytes)
                .unwrap();
            state
                .attempts
                .push((request.is_send(), headers.clone(), raw_sha256(&bytes)));
            let kind = usize::from(!request.is_send());
            if state.busy_replies[kind] > 0 {
                state.busy_replies[kind] -= 1;
                let body = b"{\"ok\":false,\"error\":\"relay_backpressure\"}";
                response.spare_mut()[..body.len()].copy_from_slice(body);
                response.advance(body.len())?;
                return Ok(Parts {
                    status: 429,
                    headers: BTreeMap::from([(
                        "content-type".into(),
                        vec!["application/json".into()],
                    )]),
                });
            }
            let mut consumed = false;
            if request.is_send() {
                let target =
                    &mut state.sessions.get_mut(id).ok_or(Error::Closed)?.directions[direction];
                if sequence == target.next {
                    assert!(target.batches.len() < 2);
                    target.batches.push_back((sequence, bytes.clone()));
                    target.next += 1;
                } else if sequence >= target.consumed {
                    assert_eq!(
                        target
                            .batches
                            .iter()
                            .find(|(n, _)| *n == sequence)
                            .unwrap()
                            .1,
                        bytes
                    );
                } else {
                    consumed = true;
                }
                state.send_sizes.push(bytes.len());
                state
                    .sends_by_device
                    .push((headers["x-kota-relay-device"].clone(), bytes.len()));
            } else {
                let text = String::from_utf8(bytes).unwrap();
                let ack = SignedAck::decode(&serde_json::to_vec(&json!({
                    "proof": {"device":headers["x-kota-relay-device"],"membership":headers["x-kota-relay-membership"],
                    "time":headers["x-kota-relay-time"].parse::<u64>().unwrap(),"nonce":"",
                    "boot":headers["x-kota-relay-boot"],"session":id,"direction":headers["x-kota-relay-direction"],
                    "sequence":sequence,"final":headers["x-kota-relay-final"] == "1","signature":headers["x-kota-relay-signature"]},"body":text
                })).unwrap())?;
                let through = serde_json::from_str::<Value>(&text).unwrap()["through"]
                    .as_str()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap();
                let target =
                    &mut state.sessions.get_mut(id).ok_or(Error::Closed)?.directions[direction];
                assert!(through <= target.next && through >= target.consumed);
                target.batches.retain(|(n, _)| *n >= through);
                target.consumed = through;
                target.ack = Some(ack);
                target.closed = headers["x-kota-relay-final"] == "1";
                if target.closed {
                    target.batches.clear();
                }
                state.acknowledgements += 1;
            }
            let kind = usize::from(!request.is_send());
            if state.lose_replies[kind] > 0 {
                state.lose_replies[kind] -= 1;
                return Err(Error::Closed); // Applied remotely; only HTTP reply lost.
            }
            serde_json::to_vec(&if request.is_send() {
                json!({"accepted":!consumed,"consumed":consumed})
            } else {
                json!({"accepted":true})
            })
            .unwrap()
        };
        response.spare_mut()[..bytes.len()].copy_from_slice(&bytes);
        response.advance(bytes.len())?;
        Ok(Parts {
            status: 200,
            headers: BTreeMap::from([("content-type".into(), vec![content_type.into()])]),
        })
    }
}

struct Node {
    pump: Pump,
    pool: HttpPool,
    http: HttpClient,
    context: Context,
    session: String,
    cancel: Cancellation,
    channels: Option<Channels>,
}
impl Node {
    async fn cycle(&mut self) -> Result<()> {
        assert!(self.pump.step()?.retired.is_empty());
        if self.channels.is_none() {
            self.channels = self.pump.channels(&self.session)?;
        }
        if let Some(ack) = self.pump.acknowledgement(&self.session, fixture().now)? {
            let body = self.http.small_body(ack.body.as_bytes())?;
            let reply = self
                .http
                .execute(
                    ack.request,
                    Some(body.into()),
                    self.cancel.clone(),
                    authorized(),
                    Instant::now() + PROGRESS_TIMEOUT,
                )
                .await?;
            if reply.status != 200 {
                return Err(Error::Protocol);
            }
            drop(reply);
            self.pump.ack_submitted(&self.session, ack.sequence)?;
        }
        if self.pump.has_output(&self.session)? {
            let admission = self.pump.prepare_upload(
                &self.session,
                &self.http,
                Instant::now() + PROGRESS_TIMEOUT,
            )?;
            let mut builder = Builder::new(self.context.limits.clone());
            self.pump.fill(&self.session, &mut builder, &admission)?;
            if builder.len() != 0 {
                let batch = self
                    .pump
                    .enqueue(&self.session, builder.finish()?, fixture().now)?;
                let pending = admission.submit(batch.request, Some(batch.payload.into()))?;
                self.pump.submitted(&self.session, batch.sequence)?;
                let reply = pending.wait().await?;
                if reply.status != 200 {
                    return Err(Error::Protocol);
                }
                drop(reply);
            }
        }
        let mode = if self.pump.data_ready() {
            ReceiveMode::Data
        } else {
            ReceiveMode::Receipts
        };
        let request = self.pump.read(mode, fixture().now)?;
        let reply = self
            .http
            .execute(
                request,
                None,
                self.cancel.clone(),
                authorized(),
                Instant::now() + PROGRESS_TIMEOUT,
            )
            .await?;
        self.pump.accept(reply.into_receive()?, fixture().now)?;
        assert!(self.pump.step()?.retired.is_empty());
        if self.channels.is_none() {
            self.channels = self.pump.channels(&self.session)?;
        }
        Ok(())
    }
}
impl Drop for Node {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.pool.stop();
        self.context.shutdown.cancel();
    }
}
struct Pair {
    nodes: [Node; 2],
    relay: Arc<Mutex<RelayState>>,
}
impl Pair {
    async fn new() -> Self {
        let mut pair = Self::unstarted().await;
        for _ in 0..30 {
            pair.cycle().await;
            if pair.nodes.iter().all(|n| n.channels.is_some()) {
                return pair;
            }
        }
        panic!("real mutual TLS handshake did not finish through envelopes");
    }
    async fn unstarted() -> Self {
        Self::with_files(None).await
    }
    async fn with_files(files: Option<[(FileIo, Limits); 2]>) -> Self {
        let f = fixture();
        let cancel = [Cancellation::default(), Cancellation::default()];
        let client = PendingTls::client(
            &f.identities[0],
            f.contexts[0].clone(),
            authorized(),
            cancel[0].clone(),
            f.now,
        )
        .unwrap();
        let server = PendingTls::server(
            &f.identities[1],
            f.contexts[1].clone(),
            authorized(),
            cancel[1].clone(),
            client.statement(),
            f.now,
        )
        .unwrap();
        let cs = client.statement().clone();
        let ss = server.statement().clone();
        let tls = [
            client.accept(&ss, f.now).unwrap(),
            server.accept(&cs, f.now).unwrap(),
        ];
        let relay = Arc::new(Mutex::new(RelayState::default()));
        relay.lock().unwrap().sessions.insert(
            tls[0].session_id.clone(),
            Session {
                devices: f.identities.each_ref().map(|d| d.device_id().unwrap()),
                directions: Default::default(),
            },
        );
        let mut nodes = Vec::new();
        for (i, tls) in tls.into_iter().enumerate() {
            let (io, limits) = files
                .as_ref()
                .map(|files| files[i].clone())
                .unwrap_or_else(|| {
                    let limits = Limits::default();
                    (FileIo::start(limits.clone()).unwrap(), limits)
                });
            let context = Context {
                limits: limits.clone(),
                io,
                shutdown: Cancellation::default(),
            };
            let state = relay.clone();
            let pool = HttpPool::start_with(limits, move |_| {
                state.lock().unwrap().created += 1;
                Box::new(Relay(state.clone()))
            })
            .await
            .unwrap();
            let http = pool.bind(&f.contexts[i].origin).unwrap();
            let mut pump = Pump::new(context.clone(), f.identities[i].clone());
            let session = pump.add(tls).unwrap();
            nodes.push(Node {
                pump,
                pool,
                http,
                context,
                session,
                cancel: cancel[i].clone(),
                channels: None,
            });
        }
        Self {
            nodes: nodes.try_into().ok().unwrap(),
            relay,
        }
    }
    async fn cycle(&mut self) {
        for node in &mut self.nodes {
            node.cycle().await.unwrap();
        }
        tokio::task::yield_now().await;
    }
    async fn until(&mut self, done: impl Fn(&Self) -> bool) {
        tokio::time::timeout(Duration::from_secs(15), async {
            while !done(self) {
                self.cycle().await;
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("bounded real file/envelope progress");
    }
    async fn stop(self) {
        for node in &self.nodes {
            node.cancel.cancel();
            node.pool.stop();
        }
        for node in &self.nodes {
            node.context.io.stop_and_wait().await;
        }
    }
}
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("bbs-relay-pump-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(p.join("staging")).unwrap();
        Self(p)
    }
    fn source(&self, len: usize) -> (Resource, PathBuf) {
        let bytes: Vec<_> = (0..len).map(|n| (n % 241) as u8).collect();
        let hash = raw_sha256(&bytes);
        let path = self.0.join("source");
        fs::write(&path, bytes).unwrap();
        (
            Resource {
                identity: ResourceKind::Attachment {
                    thread_id: "thread-pump".into(),
                    post_id: "post-pump".into(),
                    attachment_id: "att-pump".into(),
                    version_id: hash.clone(),
                },
                sha256: hash,
                size_bytes: len as u64,
            },
            path,
        )
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn merged_envelopes_and_real_tls_transfer_verified_files_with_one_account_budget() {
    let mut pair = Pair::new().await;
    assert_eq!(
        pair.relay.lock().unwrap().created,
        4,
        "two HTTP workers per account"
    );
    for sender in [0, 1] {
        let root = Root::new();
        let (resource, path) = root.source(1024 * 1024 + 29);
        let incoming = pair.nodes[1 - sender]
            .channels
            .as_ref()
            .unwrap()
            .data
            .expect_file(resource.clone(), &root.0.join("staging"))
            .await
            .unwrap();
        let data = pair.nodes[sender].channels.as_ref().unwrap().data.clone();
        let r = resource.clone();
        let sent = tokio::spawn(async move { data.send_file(r, &path).await });
        let received = tokio::spawn(incoming.finish());
        pair.until(|_| sent.is_finished() && received.is_finished())
            .await;
        sent.await.unwrap().unwrap();
        let verified = received.await.unwrap().unwrap();
        assert_eq!(verified.resource, resource);
        assert_eq!(
            raw_sha256(&fs::read(verified.path()).unwrap()),
            resource.sha256
        );
        drop(verified);
    }
    // This fixture flushes available output immediately. Its request count is
    // recorded, not presented as the still-unimplemented batching policy/budget.
    let state = pair.relay.lock().unwrap();
    println!(
        "BBS_RELAY_PUMP_COUNTS {}",
        json!({"uploads":state.send_sizes.len(),"receive":state.receives,
        "acks":state.acknowledgements,"smallReads":state.small_reads,
        "ciphertextBytes":state.send_sizes.iter().sum::<usize>(),"maxBatch":state.send_sizes.iter().max()})
    );
    drop(state);
    pair.stop().await;
}

#[tokio::test]
async fn full_file_receive_window_keeps_control_and_signed_receipts_live_without_losing_frames() {
    let mut pair = Pair::new().await;
    let root = Root::new();
    let (resource, path) = root.source(1024 * 1024);
    let (resume, pause) = oneshot::channel();
    *pair.nodes[1]
        .channels
        .as_ref()
        .unwrap()
        .data
        .pause_after_ready
        .lock()
        .await = Some(pause);
    let incoming = pair.nodes[1]
        .channels
        .as_ref()
        .unwrap()
        .data
        .expect_file(resource.clone(), &root.0.join("staging"))
        .await
        .unwrap();
    let receiver = tokio::spawn(incoming.finish());
    let data = pair.nodes[0].channels.as_ref().unwrap().data.clone();
    let r = resource.clone();
    let sender = tokio::spawn(async move { data.send_file(r, &path).await });
    // debug_queued takes the same mutex as next(). Keep driving the network
    // while the observer waits for Begin; otherwise the fixture itself stalls
    // the only task that can deliver Begin and release that mutex.
    let data = pair.nodes[1].channels.as_ref().unwrap().data.clone();
    let full = tokio::spawn(async move {
        while data.debug_queued().await != DATA_WINDOW {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    });
    pair.until(|_| full.is_finished()).await;
    full.await.unwrap();
    assert!(!sender.is_finished());
    assert_eq!(
        fs::read_dir(root.0.join("staging"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .metadata()
            .unwrap()
            .len(),
        0
    );
    let control = pair.nodes[0].channels.as_ref().unwrap().control.clone();
    let sent_control = tokio::spawn(async move {
        control
            .send(b"control while file consumption is stopped")
            .await
    });
    let mut incoming_control = None;
    for _ in 0..100 {
        pair.cycle().await;
        if let Ok(delivery) = pair.nodes[1].channels.as_mut().unwrap().incoming.try_recv() {
            incoming_control = Some(delivery);
            break;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let delivery = incoming_control.expect("control is independent of file consumption");
    assert_eq!(
        delivery.bytes(),
        b"control while file consumption is stopped"
    );
    assert!(!sent_control.is_finished());
    delivery.acknowledge().unwrap();
    pair.until(|_| sent_control.is_finished()).await;
    sent_control.await.unwrap().unwrap();
    assert!(!sender.is_finished());
    resume.send(()).unwrap();
    pair.until(|_| sender.is_finished() && receiver.is_finished())
        .await;
    sender.await.unwrap().unwrap();
    let verified = receiver.await.unwrap().unwrap();
    assert_eq!(
        raw_sha256(&fs::read(verified.path()).unwrap()),
        resource.sha256
    );
    drop(verified);
    pair.stop().await;
}

#[tokio::test]
async fn partial_merged_ciphertext_keeps_its_parent_credit_and_small_receipts_can_still_run() {
    let mut pair = Pair::new().await;
    for _ in 0..4 {
        pair.cycle().await;
    }
    let mut calls = Vec::new();
    for byte in [17, 29] {
        let channel = pair.nodes[0].channels.as_ref().unwrap().control.clone();
        calls.push(tokio::spawn(async move {
            channel.send(&vec![byte; MAX_FRAME - 9]).await
        }));
    }
    let source = &mut pair.nodes[0];
    let admission = source
        .pump
        .prepare_upload(
            &source.session,
            &source.http,
            Instant::now() + PROGRESS_TIMEOUT,
        )
        .unwrap();
    let mut builder = Builder::new(source.context.limits.clone());
    for _ in 0..100 {
        source.pump.step().unwrap();
        source
            .pump
            .fill(&source.session, &mut builder, &admission)
            .unwrap();
        if builder.len() > 2 * MAX_FRAME {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(builder.len() > 2 * MAX_FRAME);
    let batch = source
        .pump
        .enqueue(&source.session, builder.finish().unwrap(), fixture().now)
        .unwrap();
    let retry = source.pump.retry(&source.session, batch.sequence).unwrap();
    assert!(batch.payload.shares(&retry.payload));
    let pending = admission
        .submit(batch.request, Some(batch.payload.into()))
        .unwrap();
    source
        .pump
        .submitted(&source.session, batch.sequence)
        .unwrap();
    drop(pending.wait().await.unwrap());

    let destination = &mut pair.nodes[1];
    // Model other legitimate retained account frames. Only the exact maximum
    // response+header charge remains; no extra plaintext frame can be allocated.
    let held = destination
        .context
        .limits
        .reserve(APP_QUEUE_BYTES - SMALL_RESERVE - MAX_BATCH - 3 * MAX_ENVELOPE)
        .unwrap();
    let before = destination
        .pump
        .read(ReceiveMode::Data, fixture().now)
        .unwrap();
    let cursor = before.receive_request().unwrap().cursors[0].1;
    let reply = destination
        .http
        .execute(
            before,
            None,
            destination.cancel.clone(),
            authorized(),
            Instant::now() + PROGRESS_TIMEOUT,
        )
        .await
        .unwrap();
    destination
        .pump
        .accept(reply.into_receive().unwrap(), fixture().now)
        .unwrap();
    let activity = destination.pump.step().unwrap();
    assert!(activity.tls_input > 0);
    assert_eq!(
        activity.consumed_batches, 0,
        "partial TLS input cannot ACK the batch"
    );
    assert!(!destination.pump.data_ready());
    assert!(matches!(
        destination.pump.read(ReceiveMode::Data, fixture().now),
        Err(Error::Busy)
    ));
    assert!(destination
        .pump
        .acknowledgement(&destination.session, fixture().now)
        .unwrap()
        .is_none());
    assert!(matches!(
        destination.context.limits.reserve(1),
        Err(Error::Busy)
    ));
    let read = destination
        .pump
        .read(ReceiveMode::Receipts, fixture().now)
        .unwrap();
    assert_eq!(read.receive_request().unwrap().cursors[0].1, cursor);
    let reply = destination
        .http
        .execute(
            read,
            None,
            destination.cancel.clone(),
            authorized(),
            Instant::now() + PROGRESS_TIMEOUT,
        )
        .await
        .unwrap();
    destination
        .pump
        .accept(reply.into_receive().unwrap(), fixture().now)
        .unwrap();
    assert!(
        !destination.pump.data_ready(),
        "status-only reply cannot replace the held data packet"
    );
    assert!(matches!(
        destination.context.limits.reserve(1),
        Err(Error::Busy)
    ));

    drop(held);
    let activity = destination.pump.step().unwrap();
    assert_eq!(activity.consumed_batches, 1);
    assert!(destination.pump.data_ready());
    let original = destination
        .pump
        .acknowledgement(&destination.session, fixture().now)
        .unwrap()
        .unwrap();
    let retry_ack = destination
        .pump
        .acknowledgement(&destination.session, fixture().now + 1000)
        .unwrap()
        .unwrap();
    assert_eq!(original.body, retry_ack.body);
    assert_eq!(
        original.request.headers().unwrap(),
        retry_ack.request.headers().unwrap(),
        "ACK retries keep their original signature/time"
    );
    assert!(matches!(
        destination
            .pump
            .ack_submitted(&destination.session, original.sequence + 1),
        Err(Error::Protocol)
    ));
    assert_eq!(
        destination
            .pump
            .acknowledgement(&destination.session, fixture().now)
            .unwrap()
            .unwrap()
            .sequence,
        original.sequence
    );
    for expected in [17, 29] {
        let delivery = tokio::time::timeout(
            Duration::from_secs(2),
            destination.channels.as_mut().unwrap().incoming.recv(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(delivery.bytes().iter().all(|b| *b == expected));
        assert_eq!(delivery.bytes().len(), MAX_FRAME - 9);
        delivery.acknowledge().unwrap();
    }
    drop(retry);
    pair.until(|_| calls.iter().all(|c| c.is_finished())).await;
    for call in calls {
        call.await.unwrap().unwrap();
    }
    assert!(pair.relay.lock().unwrap().small_reads > 0);
    pair.stop().await;
}
