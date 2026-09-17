use super::*;
use crate::bbs_sync::{
    raw_sha256,
    relay::{
        upload::{
            tests::{fragmented, pair},
            Builder,
        },
        window::SendWindow,
    },
};

const HEADROOM: usize = super::super::super::proof::MAX_JSON + 2 * MAX_ENVELOPE + SEND_SCRATCH;

type Captured = (usize, String, BTreeMap<String, String>);
struct Capture(Arc<Mutex<Vec<Captured>>>);
impl Backend for Capture {
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
        let mut reader = body.ok_or(Error::Protocol)?.reader();
        let mut block = [0; 4093];
        let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
        let mut size = 0;
        loop {
            let n = reader.read(&mut block).unwrap();
            if n == 0 {
                break;
            }
            digest.update(&block[..n]);
            size += n;
        }
        let hash: String = digest
            .finish()
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let mut calls = self.0.lock().unwrap();
        calls.push((size, hash, request.headers()?));
        if calls.len() == 1 {
            return Err(Error::Closed);
        } // lost first upload reply
        response.spare_mut()[..2].copy_from_slice(b"{}");
        response.advance(2)?;
        Ok(Parts {
            status: 200,
            headers: BTreeMap::new(),
        })
    }
}

#[tokio::test]
async fn cancelling_a_partial_block_upload_does_not_release_an_unreturned_http_owner() {
    struct Partial(Arc<Harness>);
    impl Backend for Partial {
        fn discard(&mut self) {}
        fn run(
            &mut self,
            _: &Endpoint,
            _: &SignedRequest,
            body: Option<&Body>,
            _: &mut Buffer,
            guard: &Guard,
        ) -> Result<Parts> {
            let mut reader = GuardedBody {
                reader: body.unwrap().reader(),
                guard,
            };
            let mut prefix = [0; 13];
            assert_eq!(reader.read(&mut prefix).unwrap(), prefix.len());
            self.0.calls.fetch_add(1, Ordering::SeqCst);
            let mut released = self.0.release.lock().unwrap();
            while !*released {
                released = self.0.wake.wait(released).unwrap();
            }
            assert!(reader.read(&mut prefix).is_err());
            guard.check()?;
            unreachable!("cancelled body must not resume")
        }
    }
    let limits = Limits::default();
    let state = Arc::new(Harness::default());
    let _release = ReleaseOnDrop(state.clone());
    let factory = state.clone();
    let pool = HttpPool::start_with(limits.clone(), move |_| {
        factory.created.fetch_add(1, Ordering::SeqCst);
        Box::new(Partial(factory.clone()))
    })
    .await
    .unwrap();
    let f = fixture();
    let client = pool.bind(&f.contexts[0].origin).unwrap();
    let cancel = Cancellation::default();
    let admission = client
        .prepare_send(
            cancel.clone(),
            authorized(),
            Instant::now() + PROGRESS_TIMEOUT,
        )
        .unwrap();
    let mut window = SendWindow::new(
        f.contexts[0].clone(),
        "e".repeat(64),
        authorized(),
        cancel.clone(),
    )
    .unwrap();
    window
        .enqueue(
            &f.identities[0],
            fragmented(&limits, &vec![51; MAX_BATCH]),
            f.now,
        )
        .unwrap();
    let batch = window.batch(0).unwrap();
    let rest = limits
        .reserve(APP_QUEUE_BYTES - SMALL_RESERVE - HEADROOM - MAX_BATCH)
        .unwrap();
    let job = tokio::spawn(admission.execute(batch.request, Some(batch.payload.into())));
    until(|| state.calls.load(Ordering::SeqCst) == 1).await;
    assert!(matches!(limits.reserve(1), Err(Error::Busy)));
    cancel.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), job)
            .await
            .unwrap()
            .unwrap(),
        Err(Error::Cancelled)
    ));
    drop(window); // HTTP is now the sole owner of all sixteen blocks.
    assert!(matches!(limits.reserve(1), Err(Error::Busy)));
    assert_eq!(state.created.load(Ordering::SeqCst), 2);
    state.release();
    until(|| limits.reserve(HEADROOM + MAX_BATCH).is_ok()).await;
    drop(rest);
    pool.stop();
}

#[tokio::test]
async fn reserve_before_filling_keeps_one_request_admissible_at_the_account_limit() {
    let limits = Limits::default();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let pool = HttpPool::start_with(limits.clone(), {
        let calls = calls.clone();
        move |_| Box::new(Capture(calls.clone()))
    })
    .await
    .unwrap();
    let f = fixture();
    let client = pool.bind(&f.contexts[0].origin).unwrap();
    let (mut tls, _) = pair();
    tls.write_plaintext(b"pending control").unwrap();
    let blocked = limits.reserve(APP_QUEUE_BYTES - SMALL_RESERVE).unwrap();
    assert!(matches!(
        client.prepare_send(
            Cancellation::default(),
            authorized(),
            Instant::now() + PROGRESS_TIMEOUT
        ),
        Err(Error::Busy)
    ));
    assert!(tls.wants_write().unwrap()); // no builder/encryption drain on failed admission
    assert!(calls.lock().unwrap().is_empty());
    drop(blocked);
    let admission = client
        .prepare_send(
            Cancellation::default(),
            authorized(),
            Instant::now() + PROGRESS_TIMEOUT,
        )
        .unwrap();
    let mut builder = Builder::new(limits.clone());
    builder.fill(&mut tls).unwrap();
    for _ in 0..16 {
        tls.write_plaintext(&[41; MAX_FRAME]).unwrap();
        builder.fill(&mut tls).unwrap();
        if builder.full() {
            break;
        }
    }
    assert!(builder.full());
    let mut window = SendWindow::new(
        f.contexts[0].clone(),
        "e".repeat(64),
        authorized(),
        Cancellation::default(),
    )
    .unwrap();
    window
        .enqueue(&f.identities[0], builder.finish().unwrap(), f.now)
        .unwrap();
    let batch = window.batch(0).unwrap();
    let expected = (
        MAX_BATCH,
        batch.payload.digest().into(),
        batch.request.headers().unwrap(),
    );
    let rest = limits
        .reserve(APP_QUEUE_BYTES - SMALL_RESERVE - HEADROOM - MAX_BATCH)
        .unwrap();
    assert!(matches!(limits.reserve(1), Err(Error::Busy)));
    assert!(matches!(
        admission
            .execute(batch.request, Some(batch.payload.into()))
            .await,
        Err(Error::Closed)
    ));
    // The reply can wake this task before the worker drops its last Job fields.
    // Wait for real release; failure must never free the batch retained for retry.
    until(|| limits.reserve(HEADROOM).is_ok()).await;
    assert!(limits.reserve(HEADROOM).is_ok());
    assert!(matches!(limits.reserve(HEADROOM + 1), Err(Error::Busy)));
    let retry = window.batch(0).unwrap();
    assert_eq!(retry.request.headers().unwrap(), expected.2);
    let second = client
        .prepare_send(
            Cancellation::default(),
            authorized(),
            Instant::now() + PROGRESS_TIMEOUT,
        )
        .unwrap();
    let response = second
        .execute(retry.request, Some(retry.payload.into()))
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(*calls.lock().unwrap(), vec![expected.clone(), expected]);
    assert!(window.batch(0).is_ok()); // HTTP 200 still does not free ciphertext
    drop(response);
    drop(window);
    drop(rest);
    until(|| limits.reserve(APP_QUEUE_BYTES - SMALL_RESERVE).is_ok()).await;
    assert!(limits.reserve(APP_QUEUE_BYTES - SMALL_RESERVE).is_ok());
    pool.stop();
}

#[tokio::test]
async fn prepared_upload_is_fenced_before_fill_and_before_enqueue_without_any_http() {
    let limits = Limits::default();
    let (client, state, _release) = fake_pool(limits.clone()).await;
    let cancel = Cancellation::default();
    let first = client
        .prepare_send(
            cancel.clone(),
            authorized(),
            Instant::now() + PROGRESS_TIMEOUT,
        )
        .unwrap();
    cancel.cancel();
    assert_eq!(first.check(), Err(Error::Cancelled));
    assert!(matches!(
        first
            .execute(upload(b"x"), Some(fragmented(&limits, b"x").into()))
            .await,
        Err(Error::Cancelled)
    ));
    let member = Arc::new(AtomicBool::new(true));
    let fence = member.clone();
    let auth: MembershipCheck = Arc::new(move || fence.load(Ordering::SeqCst));
    let revoked = client
        .prepare_send(
            Cancellation::default(),
            auth,
            Instant::now() + PROGRESS_TIMEOUT,
        )
        .unwrap();
    member.store(false, Ordering::SeqCst);
    assert_eq!(revoked.check(), Err(Error::Unauthorized));
    drop(revoked);
    let expired = client
        .prepare_send(
            Cancellation::default(),
            authorized(),
            Instant::now() + Duration::from_millis(25),
        )
        .unwrap();
    tokio::time::sleep(Duration::from_millis(35)).await;
    assert_eq!(expired.check(), Err(Error::Timeout));
    assert!(matches!(
        expired
            .execute(upload(b"x"), Some(fragmented(&limits, b"x").into()))
            .await,
        Err(Error::Timeout)
    ));
    let wrong = client
        .prepare_send(
            Cancellation::default(),
            authorized(),
            Instant::now() + PROGRESS_TIMEOUT,
        )
        .unwrap();
    assert!(matches!(
        wrong.execute(request(false), None).await,
        Err(Error::Protocol)
    ));
    let wrong_body = client
        .prepare_send(
            Cancellation::default(),
            authorized(),
            Instant::now() + PROGRESS_TIMEOUT,
        )
        .unwrap();
    assert!(matches!(
        wrong_body
            .execute(upload(b"x"), Some(fragmented(&limits, b"y").into()))
            .await,
        Err(Error::Protocol)
    ));
    let retired = client
        .prepare_send(
            Cancellation::default(),
            authorized(),
            Instant::now() + PROGRESS_TIMEOUT,
        )
        .unwrap();
    let _new = client.pool.bind(&fixture().contexts[0].origin).unwrap();
    assert_eq!(retired.check(), Err(Error::Cancelled));
    assert!(matches!(
        retired
            .execute(upload(b"x"), Some(fragmented(&limits, b"x").into()))
            .await,
        Err(Error::Cancelled)
    ));
    assert_eq!(state.calls.load(Ordering::SeqCst), 0);
    assert!(limits.reserve(APP_QUEUE_BYTES - SMALL_RESERVE).is_ok());
    client.pool.stop();
    until(|| state.dropped.load(Ordering::SeqCst) == 2).await;
}

#[tokio::test]
async fn real_https_streams_one_fixed_length_request_per_batch_and_retries_identically() {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
    };
    let certificate = rcgen::generate_simple_self_signed(vec!["worker.example".into()]).unwrap();
    let der = CertificateDer::from(certificate.cert.der().to_vec());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
        certificate.signing_key.serialize_der(),
    ));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let server_config = Arc::new(
        rustls::ServerConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![der.clone()], key)
            .unwrap(),
    );
    let mut roots = rustls::RootCertStore::empty();
    roots.add(der).unwrap();
    let client_config = Arc::new(
        rustls::ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let source: Vec<_> = (0..MAX_BATCH).map(|i| (i % 239) as u8).collect();
    let expected_hash = raw_sha256(&source);
    let server = std::thread::spawn(move || {
        let mut observations = Vec::new();
        for attempt in 0..2 {
            let until = Instant::now() + Duration::from_secs(10);
            let socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < until =>
                    {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("isolated upload accept: {e}"),
                }
            };
            socket.set_nonblocking(false).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut tls = rustls::StreamOwned::new(
                rustls::ServerConnection::new(server_config.clone()).unwrap(),
                socket,
            );
            let mut reader = BufReader::new(&mut tls);
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            assert!(first.starts_with("POST /bbs/relay/send?group="));
            let mut headers = BTreeMap::new();
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
                let (name, value) = line.trim().split_once(':').unwrap();
                assert!(headers
                    .insert(name.to_ascii_lowercase(), value.trim().to_owned())
                    .is_none());
            }
            assert_eq!(headers["content-length"], MAX_BATCH.to_string());
            assert!(!headers.contains_key("transfer-encoding"));
            let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
            let mut remaining = MAX_BATCH;
            let mut block = [0; 4093];
            while remaining > 0 {
                let n = remaining.min(block.len());
                reader.read_exact(&mut block[..n]).unwrap();
                hash.update(&block[..n]);
                remaining -= n;
            }
            assert!(reader.buffer().is_empty()); // no trailing chunk delimiter or extra bytes
            let hash: String = hash
                .finish()
                .as_ref()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            assert_eq!(hash, expected_hash);
            observations.push(
                headers
                    .into_iter()
                    .filter(|(k, _)| k.starts_with("x-kota-relay-"))
                    .collect::<BTreeMap<_, _>>(),
            );
            drop(reader);
            if attempt == 1 {
                tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
                tls.flush().unwrap();
            }
            // First connection deliberately closes after body, before response.
        }
        assert_eq!(observations[0], observations[1]);
        observations.len()
    });
    let origin = format!("https://worker.example:{}", address.port());
    let limits = Limits::default();
    let pool = HttpPool::start_with(limits.clone(), move |_| {
        let config = client_config.clone();
        Box::new(Https {
            test_agent: Some(Arc::new(move |_| {
                ureq::AgentBuilder::new()
                    .https_only(true)
                    .try_proxy_from_env(false)
                    .redirects(0)
                    .timeout(Duration::from_secs(10))
                    .tls_config(config.clone())
                    .resolver(move |_: &str| Ok(vec![address]))
                    .build()
            })),
            ..Https::default()
        })
    })
    .await
    .unwrap();
    let client = pool.bind(&origin).unwrap();
    let mut f = fixture();
    f.contexts[0].origin = origin;
    let mut window = SendWindow::new(
        f.contexts[0].clone(),
        "e".repeat(64),
        authorized(),
        Cancellation::default(),
    )
    .unwrap();
    // Reserve before materializing the first immutable batch.
    let permit = client
        .prepare_send(
            Cancellation::default(),
            authorized(),
            Instant::now() + PROGRESS_TIMEOUT,
        )
        .unwrap();
    window
        .enqueue(&f.identities[0], fragmented(&limits, &source), f.now)
        .unwrap();
    let first = window.batch(0).unwrap();
    assert!(matches!(
        permit
            .execute(first.request, Some(first.payload.into()))
            .await,
        Err(Error::Closed)
    ));
    let retry = window.batch(0).unwrap();
    let permit = client
        .prepare_send(
            Cancellation::default(),
            authorized(),
            Instant::now() + PROGRESS_TIMEOUT,
        )
        .unwrap();
    assert_eq!(
        permit
            .execute(retry.request, Some(retry.payload.into()))
            .await
            .unwrap()
            .status,
        200
    );
    assert!(window.batch(0).is_ok());
    assert_eq!(
        tokio::task::spawn_blocking(move || server.join().unwrap())
            .await
            .unwrap(),
        2
    );
    pool.stop();
}
