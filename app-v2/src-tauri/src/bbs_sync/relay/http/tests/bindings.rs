use super::*;

const A: &str = "https://worker.example";
const B: &str = "https://other.example";

fn read_at(origin: &str, small: bool) -> SignedRequest {
    let f = fixture();
    let c = &f.contexts[0];
    let target = if small {
        Target::receive(
            origin,
            &c.peer.group_id,
            &c.boot,
            ReceiveMode::Probe,
            &[("e".repeat(64), 0)],
        )
        .unwrap()
    } else {
        Target::poll(origin, &c.peer.group_id, None).unwrap()
    };
    SignedRequest::sign(
        &f.identities[0],
        &c.peer.local_membership_id,
        target,
        Fields::Read,
        &[],
        f.now,
    )
    .unwrap()
}

#[derive(Default)]
struct Blocked {
    created: AtomicUsize,
    dropped: AtomicUsize,
    discards: AtomicUsize,
    calls: Mutex<Vec<(u64, std::thread::ThreadId)>>,
    release: Mutex<bool>,
    wake: Condvar,
}
impl Blocked {
    fn release(&self) {
        *self.release.lock().unwrap() = true;
        self.wake.notify_all();
    }
    fn count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}
struct Release(Arc<Blocked>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}
struct BlockedBackend(Arc<Blocked>);
impl Drop for BlockedBackend {
    fn drop(&mut self) {
        self.0.dropped.fetch_add(1, Ordering::SeqCst);
    }
}
impl Backend for BlockedBackend {
    fn discard(&mut self) {
        self.0.discards.fetch_add(1, Ordering::SeqCst);
    }
    fn run(
        &mut self,
        endpoint: &Endpoint,
        request: &SignedRequest,
        _: Option<&Body>,
        response: &mut Buffer,
        _: &Guard,
    ) -> Result<Parts> {
        endpoint.destination.validate_url(&request.url())?;
        self.0
            .calls
            .lock()
            .unwrap()
            .push((endpoint.generation, std::thread::current().id()));
        // Deliberately ignores cancellation while blocked, like system DNS.
        // Only the original worker can leave this call after the gate opens.
        if endpoint.generation == 1 {
            let mut release = self.0.release.lock().unwrap();
            while !*release {
                release = self.0.wake.wait(release).unwrap();
            }
        }
        let bytes = request
            .receive_request()
            .map(envelope::tests::empty)
            .unwrap_or_else(|| b"{}".to_vec());
        response.spare_mut()[..bytes.len()].copy_from_slice(&bytes);
        response.advance(bytes.len())?;
        Ok(Parts {
            status: 200,
            headers: BTreeMap::from([("content-type".into(), vec![envelope::CONTENT_TYPE.into()])]),
        })
    }
}
async fn blocked_pool() -> (HttpPool, Limits, Arc<Blocked>, Release) {
    let limits = Limits::default();
    let state = Arc::new(Blocked::default());
    let factory = state.clone();
    let pool = HttpPool::start_with(limits.clone(), move |_| {
        factory.created.fetch_add(1, Ordering::SeqCst);
        Box::new(BlockedBackend(factory.clone()))
    })
    .await
    .unwrap();
    (pool, limits, state.clone(), Release(state))
}
async fn call(client: &HttpClient, origin: &str, small: bool) -> Result<Reply> {
    client
        .execute(
            read_at(origin, small),
            None,
            Cancellation::default(),
            authorized(),
            Instant::now() + PROGRESS_TIMEOUT,
        )
        .await
}

#[tokio::test]
async fn many_origin_rebinds_retire_queues_but_keep_inflight_bytes_partitions_and_two_threads() {
    let (pool, limits, state, _release) = blocked_pool().await;
    let old = pool.bind(A).unwrap();
    assert_eq!(old.endpoint.generation, 1);
    let large = vec![9; MAX_BATCH];
    let old_data = spawn(
        &old,
        upload(&large),
        Some(payload(&limits, &large)),
        Cancellation::default(),
        authorized(),
        Instant::now() + PROGRESS_TIMEOUT,
    );
    let old_small = spawn(
        &old,
        read_at(A, true),
        None,
        Cancellation::default(),
        authorized(),
        Instant::now() + PROGRESS_TIMEOUT,
    );
    until(|| state.count() == 2).await;
    let queued_data = spawn(
        &old,
        read_at(A, false),
        None,
        Cancellation::default(),
        authorized(),
        Instant::now() + PROGRESS_TIMEOUT,
    );
    let queued_small = spawn(
        &old,
        read_at(A, true),
        None,
        Cancellation::default(),
        authorized(),
        Instant::now() + PROGRESS_TIMEOUT,
    );
    until(|| queued(&old) == 1 && pool.0.small.state.lock().unwrap().jobs.len() == 1).await;
    let in_flight_data = MAX_BATCH + 65536 + 2 * MAX_ENVELOPE + SEND_SCRATCH;
    let remaining = APP_QUEUE_BYTES - SMALL_RESERVE - in_flight_data;
    assert!(matches!(limits.reserve(remaining), Err(Error::Busy)));
    let other = pool.bind(B).unwrap();
    for task in [old_data, old_small, queued_data, queued_small] {
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap(),
            Err(Error::Cancelled)
        ));
    }
    assert_eq!(queued(&old), 0);
    assert_eq!(pool.0.small.state.lock().unwrap().jobs.len(), 0);
    // Queued ownership is gone immediately; each blocked call still owns all
    // its original request, response and header reservations.
    let rest = limits.reserve(remaining).unwrap();
    assert!(matches!(limits.reserve(1), Err(Error::Busy)));
    let small_rest = pool
        .0
        .small_limits
        .reserve(SMALL_RESERVE - 3 * MAX_ENVELOPE)
        .unwrap();
    assert!(matches!(pool.0.small_limits.reserve(1), Err(Error::Busy)));
    assert!(matches!(old.small_body(b"{}"), Err(Error::Cancelled)));
    assert_eq!(state.count(), 2);

    let mut current = other.clone();
    for i in 0..16 {
        let previous = current.endpoint.generation;
        current = pool.bind(if i % 2 == 0 { B } else { A }).unwrap();
        assert_eq!(current.endpoint.generation, previous + 1);
    }
    assert_eq!(current.endpoint.generation, 18);
    // Same origin again does not revive the old A proof/handle (or old B).
    assert!(matches!(call(&old, A, true).await, Err(Error::Cancelled)));
    assert!(matches!(call(&other, B, true).await, Err(Error::Cancelled)));
    assert!(matches!(
        pool.bind("https://worker.local"),
        Err(Error::InvalidSignal)
    ));
    assert_eq!(pool.0.generation.load(Ordering::Acquire), 18);
    assert!(!current.endpoint.retired.is_cancelled());
    assert_eq!(state.created.load(Ordering::SeqCst), 2);
    drop(rest);
    drop(small_rest);

    // A fresh queued request still uses its original absolute deadline. No
    // replacement worker is created to escape either blocked original call.
    assert!(matches!(
        current
            .execute(
                read_at(A, false),
                None,
                Cancellation::default(),
                authorized(),
                Instant::now() + Duration::from_millis(80)
            )
            .await,
        Err(Error::Timeout)
    ));
    assert_eq!(queued(&current), 0);
    assert_eq!(state.count(), 2);
    let data = spawn(
        &current,
        read_at(A, false),
        None,
        Cancellation::default(),
        authorized(),
        Instant::now() + PROGRESS_TIMEOUT,
    );
    let small = spawn(
        &current,
        read_at(A, true),
        None,
        Cancellation::default(),
        authorized(),
        Instant::now() + PROGRESS_TIMEOUT,
    );
    until(|| queued(&current) == 1 && pool.0.small.state.lock().unwrap().jobs.len() == 1).await;
    state.release();
    assert_eq!(data.await.unwrap().unwrap().status, 200);
    assert_eq!(
        small
            .await
            .unwrap()
            .unwrap()
            .into_receive()
            .unwrap()
            .items()
            .len(),
        1
    );
    assert_eq!(state.created.load(Ordering::SeqCst), 2);
    let calls = state.calls.lock().unwrap().clone();
    assert_eq!(
        calls.iter().map(|c| c.0).collect::<Vec<_>>(),
        vec![1, 1, 18, 18]
    );
    assert_eq!(
        calls
            .iter()
            .map(|c| format!("{:?}", c.1))
            .collect::<BTreeSet<_>>()
            .len(),
        2
    );
    assert!(state.discards.load(Ordering::SeqCst) >= 2);
    pool.stop();
    until(|| state.dropped.load(Ordering::SeqCst) == 2).await;
    drop(old);
    drop(other);
    drop(current);
    drop(pool);
    assert!(limits.reserve(APP_QUEUE_BYTES).is_ok());
}

#[tokio::test]
async fn invalid_bind_is_nonmutating_foreign_requests_never_run_and_unbind_never_reauthorizes() {
    let (pool, _, state, _release) = blocked_pool().await;
    state.release();
    assert!(pool.0.active.lock().unwrap().is_none());
    for origin in [
        "https://worker.local",
        "http://worker.example",
        "https://127.0.0.1",
    ] {
        assert!(pool.bind(origin).is_err());
    }
    assert_eq!(pool.0.generation.load(Ordering::Acquire), 0);
    assert_eq!(state.count(), 0);
    let first = pool.bind(A).unwrap();
    assert!(matches!(
        call(&first, B, true).await,
        Err(Error::Unauthorized)
    ));
    assert_eq!(state.count(), 0);
    assert!(pool.bind("https://worker.local").is_err());
    assert!(!first.endpoint.retired.is_cancelled());
    assert_eq!(call(&first, A, true).await.unwrap().status, 200);
    let next = pool.bind(A).unwrap();
    assert_eq!(next.endpoint.generation, first.endpoint.generation + 1);
    assert!(matches!(call(&first, A, true).await, Err(Error::Cancelled)));
    assert_eq!(state.count(), 1);
    pool.unbind();
    assert!(pool.0.active.lock().unwrap().is_none());
    assert!(matches!(call(&next, A, true).await, Err(Error::Cancelled)));
    let newest = pool.bind(A).unwrap();
    assert_eq!(newest.endpoint.generation, 3);
    assert_eq!(call(&newest, A, true).await.unwrap().status, 200);
    assert_eq!(state.count(), 2);
    pool.stop();
    assert!(matches!(pool.bind(A), Err(Error::Closed)));
    assert!(matches!(call(&newest, A, true).await, Err(Error::Closed)));
    until(|| state.dropped.load(Ordering::SeqCst) == 2).await;
}

#[tokio::test]
async fn binding_generation_fences_independently_and_never_wraps() {
    let g = guard();
    assert!(g.check().is_ok());
    g.generation.store(1, Ordering::Release);
    assert!(!g.binding.is_cancelled());
    assert_eq!(g.check(), Err(Error::Cancelled));
    let (pool, _, state, _release) = blocked_pool().await;
    pool.0.generation.store(u64::MAX, Ordering::Release);
    assert!(matches!(pool.bind(A), Err(Error::Runtime)));
    assert_eq!(pool.0.generation.load(Ordering::Acquire), u64::MAX);
    assert!(pool.0.active.lock().unwrap().is_none());
    assert_eq!(state.count(), 0);
    pool.stop();
    until(|| state.dropped.load(Ordering::SeqCst) == 2).await;
}

#[tokio::test]
async fn real_https_rebind_closes_idle_agents_and_uses_new_origin_on_the_same_worker() {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
    };
    let cert =
        rcgen::generate_simple_self_signed(vec!["worker.example".into(), "other.example".into()])
            .unwrap();
    let der = CertificateDer::from(cert.cert.der().to_vec());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let server = Arc::new(
        rustls::ServerConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![der.clone()], key)
            .unwrap(),
    );
    let mut roots = rustls::RootCertStore::empty();
    roots.add(der).unwrap();
    let client = Arc::new(
        rustls::ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let origins = [
        format!("{A}:{}", address.port()),
        format!("{B}:{}", address.port()),
        format!("{A}:{}", address.port()),
    ];
    let body = envelope::tests::empty(read_at(&origins[0], true).receive_request().unwrap());
    let (closed, mut closed_events) = tokio::sync::mpsc::channel(3);
    let server = std::thread::spawn(move || {
        for host in ["worker.example", "other.example", "worker.example"] {
            let deadline = Instant::now() + Duration::from_secs(5);
            let socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(e) => panic!("isolated binding server accept: {e}"),
                }
            };
            socket.set_nonblocking(false).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut tls = rustls::StreamOwned::new(
                rustls::ServerConnection::new(server.clone()).unwrap(),
                socket,
            );
            let mut reader = BufReader::new(&mut tls);
            let mut line = String::new();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            assert!(line.starts_with("GET /bbs/relay/receive?"));
            let mut found_host = false;
            loop {
                line.clear();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
                if line.to_ascii_lowercase().starts_with("host:") {
                    assert_eq!(line.trim(), format!("Host: {host}:{}", address.port()));
                    found_host = true;
                }
            }
            assert!(found_host);
            drop(reader);
            write!(tls, "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n", body.len()).unwrap();
            tls.write_all(&body).unwrap();
            tls.flush().unwrap();
            let result = tls.read(&mut [0u8; 1]);
            let ended = matches!(result, Ok(0))
                || result.as_ref().is_err_and(|e| {
                    matches!(
                        e.kind(),
                        std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset
                    )
                });
            closed.blocking_send(ended).unwrap();
            assert!(ended, "idle old origin must close without another request");
        }
    });
    let made_workers = Arc::new(AtomicUsize::new(0));
    let made_agents = Arc::new(Mutex::new(Vec::new()));
    let factory: Arc<dyn Fn(Destination) -> ureq::Agent + Send + Sync> = {
        let made_agents = made_agents.clone();
        Arc::new(move |destination| {
            made_agents
                .lock()
                .unwrap()
                .push(std::thread::current().id());
            // Only the test build supplies the isolated CA/DNS override; the
            // production Agent selector and generation comparison still run.
            let expected =
                if destination == Destination::new(&format!("{A}:{}", address.port())).unwrap() {
                    format!("worker.example:{}", address.port())
                } else {
                    format!("other.example:{}", address.port())
                };
            ureq::AgentBuilder::new()
                .https_only(true)
                .try_proxy_from_env(false)
                .redirects(0)
                .timeout(Duration::from_secs(5))
                .tls_config(client.clone())
                .resolver(move |netloc: &str| {
                    assert_eq!(netloc, expected);
                    Ok(vec![address])
                })
                .build()
        })
    };
    let count = made_workers.clone();
    let pool = HttpPool::start_with(Limits::default(), move |_| {
        count.fetch_add(1, Ordering::SeqCst);
        Box::new(Https {
            test_agent: Some(factory.clone()),
            ..Https::default()
        })
    })
    .await
    .unwrap();
    let mut handle = pool.bind(&origins[0]).unwrap();
    for i in 0..3 {
        assert_eq!(handle.endpoint.generation, i as u64 + 1);
        let packet = call(&handle, &origins[i], true)
            .await
            .unwrap()
            .into_receive()
            .unwrap();
        assert_eq!(packet.items()[0].window(), (0, 0, false));
        if i == 2 {
            pool.unbind();
        } else {
            handle = pool.bind(&origins[i + 1]).unwrap();
        }
        // Observe close before sending any request to the next origin.
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(6), closed_events.recv())
                .await
                .unwrap(),
            Some(true)
        );
    }
    let threads = made_agents.lock().unwrap();
    assert_eq!(threads.len(), 3);
    assert!(threads.iter().all(|id| id == &threads[0]));
    assert_eq!(made_workers.load(Ordering::SeqCst), 2);
    drop(threads);
    pool.stop();
    server.join().unwrap();
}
