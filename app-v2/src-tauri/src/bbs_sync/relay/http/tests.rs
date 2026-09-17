use super::*;
use crate::bbs_sync::{
    relay::{
        proof::{Direction, Fields, ReceiveMode, Route, Target},
        tests::fixture,
        MAX_BATCH,
    },
    transport::APP_QUEUE_BYTES,
};
use std::{
    sync::atomic::{AtomicBool, AtomicUsize},
    time::Duration,
};
mod pump;
mod metadata;

fn authorized() -> MembershipCheck {
    Arc::new(|| true)
}
fn guard() -> Guard {
    Guard {
        cancel: Cancellation::default(),
        binding: Cancellation::default(),
        generation: Arc::new(AtomicU64::new(0)),
        expected_generation: 0,
        abandoned: Cancellation::default(),
        shutdown: Cancellation::default(),
        authorized: authorized(),
        deadline: Instant::now() + PROGRESS_TIMEOUT,
    }
}
fn request(probe: bool) -> SignedRequest {
    let f = fixture();
    let c = &f.contexts[0];
    let target = if probe {
        Target::receive(
            &c.origin,
            &c.peer.group_id,
            &c.boot,
            ReceiveMode::Probe,
            &[("e".repeat(64), 0)],
        )
        .unwrap()
    } else {
        Target::poll(&c.origin, &c.peer.group_id, None).unwrap()
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
fn upload(bytes: &[u8]) -> SignedRequest {
    let f = fixture();
    let c = &f.contexts[0];
    SignedRequest::sign(
        &f.identities[0],
        &c.peer.local_membership_id,
        Target::post(&c.origin, &c.peer.group_id, Route::Send).unwrap(),
        Fields::Frame {
            boot: &c.boot,
            session: &"e".repeat(64),
            direction: Direction::ClientToServer,
            sequence: 0,
            final_flag: false,
        },
        bytes,
        f.now,
    )
    .unwrap()
}
fn payload(limits: &Limits, bytes: &[u8]) -> Payload {
    let mut b = Buffer::new(limits, bytes.len()).unwrap();
    b.spare_mut().copy_from_slice(bytes);
    b.advance(bytes.len()).unwrap();
    b.freeze().unwrap()
}
async fn until(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("bounded state transition");
}

#[derive(Default)]
struct Harness {
    created: AtomicUsize,
    dropped: AtomicUsize,
    calls: AtomicUsize,
    release: Mutex<bool>,
    wake: Condvar,
}
impl Harness {
    fn release(&self) {
        *self.release.lock().unwrap() = true;
        self.wake.notify_all();
    }
}
struct ReleaseOnDrop(Arc<Harness>);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}
struct Fake {
    lane: Lane,
    state: Arc<Harness>,
}
impl Drop for Fake {
    fn drop(&mut self) {
        self.state.dropped.fetch_add(1, Ordering::SeqCst);
    }
}
impl Backend for Fake {
    fn discard(&mut self) {}
    fn run(
        &mut self,
        _: &Endpoint,
        request: &SignedRequest,
        _: Option<&Body>,
        response: &mut Buffer,
        _: &Guard,
    ) -> Result<Parts> {
        self.state.calls.fetch_add(1, Ordering::SeqCst);
        if matches!(self.lane, Lane::Data) {
            let mut released = self.state.release.lock().unwrap();
            while !*released {
                released = self.state.wake.wait(released).unwrap();
            }
        }
        let bytes = if let Some(receive) = request.receive_request() {
            crate::bbs_sync::relay::envelope::tests::empty(receive)
        } else {
            b"{}".to_vec()
        };
        response.spare_mut()[..bytes.len()].copy_from_slice(&bytes);
        response.advance(bytes.len())?;
        Ok(Parts {
            status: 200,
            headers: BTreeMap::from([("content-type".into(), vec![envelope::CONTENT_TYPE.into()])]),
        })
    }
}
async fn fake_pool(limits: Limits) -> (HttpClient, Arc<Harness>, ReleaseOnDrop) {
    let state = Arc::new(Harness::default());
    let factory = state.clone();
    let pool = HttpPool::start_with(limits, move |lane| {
        factory.created.fetch_add(1, Ordering::SeqCst);
        Box::new(Fake {
            lane,
            state: factory.clone(),
        })
    })
    .await
    .unwrap();
    (
        pool.bind(&fixture().contexts[0].origin).unwrap(),
        state.clone(),
        ReleaseOnDrop(state),
    )
}
fn spawn(
    pool: &HttpClient,
    request: SignedRequest,
    body: Option<Payload>,
    cancel: Cancellation,
    check: MembershipCheck,
    deadline: Instant,
) -> tokio::task::JoinHandle<Result<Reply>> {
    let pool = pool.clone();
    tokio::spawn(async move {
        pool.execute(request, body.map(Body::from), cancel, check, deadline)
            .await
    })
}
fn queued(pool: &HttpClient) -> usize {
    pool.pool.0.data.state.lock().unwrap().jobs.len()
}

#[tokio::test]
async fn two_fixed_workers_allow_probe_during_upload_and_cancel_retains_inflight_charge() {
    let limits = Limits::default();
    let (pool, state, _release) = fake_pool(limits.clone()).await;
    assert_eq!(state.created.load(Ordering::SeqCst), 2);
    let bytes = vec![23; MAX_BATCH];
    let cancel = Cancellation::default();
    let pending = spawn(
        &pool,
        upload(&bytes),
        Some(payload(&limits, &bytes)),
        cancel.clone(),
        authorized(),
        Instant::now() + PROGRESS_TIMEOUT,
    );
    until(|| state.calls.load(Ordering::SeqCst) == 1).await;
    let probe = pool
        .execute(
            request(true),
            None,
            Cancellation::default(),
            authorized(),
            Instant::now() + PROGRESS_TIMEOUT,
        )
        .await
        .unwrap();
    assert_eq!(probe.status, 200);
    assert!(probe.body.as_ref().unwrap().bytes().starts_with(b"KBR1"));
    drop(probe);
    let f = fixture();
    let c = &f.contexts[1];
    let body = b"{\"through\":\"0\",\"final\":false}";
    let ack = SignedRequest::sign(
        &f.identities[1],
        &c.peer.local_membership_id,
        Target::post(&c.origin, &c.peer.group_id, Route::Ack).unwrap(),
        Fields::Frame {
            boot: &c.boot,
            session: &"e".repeat(64),
            direction: Direction::ClientToServer,
            sequence: 0,
            final_flag: false,
        },
        body,
        f.now,
    )
    .unwrap();
    let reply = pool
        .execute(
            ack,
            Some(payload(&limits, body).into()),
            Cancellation::default(),
            authorized(),
            Instant::now() + PROGRESS_TIMEOUT,
        )
        .await
        .unwrap();
    assert_eq!(reply.status, 200);
    drop(reply);
    cancel.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), pending)
            .await
            .unwrap()
            .unwrap(),
        Err(Error::Cancelled)
    ));
    // The fake network is still blocked. Cancellation does not free its owned
    // upload, response reservation or the conservative two-header charge.
    let remainder = limits
        .reserve(
            APP_QUEUE_BYTES - SMALL_RESERVE - MAX_BATCH - 65536 - 2 * MAX_ENVELOPE - SEND_SCRATCH,
        )
        .unwrap();
    assert!(matches!(limits.reserve(1), Err(Error::Busy)));
    drop(remainder);
    assert_eq!(state.created.load(Ordering::SeqCst), 2);
    println!(
        "BBS_RELAY_HTTP_METRICS {}",
        serde_json::json!({
            "workers":state.created.load(Ordering::SeqCst),
            "callsWhileUploadBlocked":state.calls.load(Ordering::SeqCst),
            "retainedBytesAfterLocalCancel":MAX_BATCH+65536+2*MAX_ENVELOPE+SEND_SCRATCH,
            "replacementWorkers":0,
        })
    );
    state.release();
    until(|| limits.reserve(APP_QUEUE_BYTES - SMALL_RESERVE).is_ok()).await;
    pool.pool.stop();
    until(|| state.dropped.load(Ordering::SeqCst) == 2).await;
}

#[tokio::test]
async fn queued_cancellation_deadline_and_abandoned_future_make_no_request_or_replacement() {
    let limits = Limits::default();
    let (pool, state, _release) = fake_pool(limits.clone()).await;
    let first = spawn(
        &pool,
        request(false),
        None,
        Cancellation::default(),
        authorized(),
        Instant::now() + PROGRESS_TIMEOUT,
    );
    until(|| state.calls.load(Ordering::SeqCst) == 1).await;
    let cancel = Cancellation::default();
    let second = spawn(
        &pool,
        request(false),
        None,
        cancel.clone(),
        authorized(),
        Instant::now() + PROGRESS_TIMEOUT,
    );
    until(|| queued(&pool) == 1).await;
    cancel.cancel();
    assert!(matches!(second.await.unwrap(), Err(Error::Cancelled)));
    assert_eq!(queued(&pool), 0);
    let abandoned = spawn(
        &pool,
        request(false),
        None,
        Cancellation::default(),
        authorized(),
        Instant::now() + PROGRESS_TIMEOUT,
    );
    until(|| queued(&pool) == 1).await;
    abandoned.abort();
    assert!(abandoned.await.is_err());
    until(|| queued(&pool) == 0).await;
    let expired = spawn(
        &pool,
        request(false),
        None,
        Cancellation::default(),
        authorized(),
        Instant::now() + Duration::from_millis(80),
    );
    assert!(matches!(expired.await.unwrap(), Err(Error::Timeout)));
    assert_eq!(queued(&pool), 0);
    assert!(matches!(
        pool.execute(
            request(false),
            None,
            Cancellation::default(),
            authorized(),
            Instant::now()
        )
        .await,
        Err(Error::Timeout)
    ));
    let mut waiting = Vec::new();
    for n in 1..=QUEUED_PER_LANE {
        waiting.push(spawn(
            &pool,
            request(false),
            None,
            Cancellation::default(),
            authorized(),
            Instant::now() + PROGRESS_TIMEOUT,
        ));
        until(|| queued(&pool) == n).await;
    }
    assert!(matches!(
        pool.execute(
            request(false),
            None,
            Cancellation::default(),
            authorized(),
            Instant::now() + PROGRESS_TIMEOUT
        )
        .await,
        Err(Error::Busy)
    ));
    pool.pool.stop();
    for job in waiting {
        assert!(matches!(job.await.unwrap(), Err(Error::Closed)));
    }
    assert!(matches!(first.await.unwrap(), Err(Error::Closed)));
    assert_eq!(state.calls.load(Ordering::SeqCst), 1);
    assert_eq!(state.created.load(Ordering::SeqCst), 2);
    state.release();
    until(|| state.dropped.load(Ordering::SeqCst) == 2).await;
    drop(pool);
    assert!(limits.reserve(APP_QUEUE_BYTES).is_ok());
}

#[tokio::test]
async fn late_response_has_no_authority_after_membership_or_work_epoch_changes() {
    let (pool, state, _release) = fake_pool(Limits::default()).await;
    let current = Arc::new(AtomicBool::new(true));
    let fence = current.clone();
    let request = spawn(
        &pool,
        request(false),
        None,
        Cancellation::default(),
        Arc::new(move || fence.load(Ordering::SeqCst)),
        Instant::now() + PROGRESS_TIMEOUT,
    );
    until(|| state.calls.load(Ordering::SeqCst) == 1).await;
    current.store(false, Ordering::SeqCst);
    state.release();
    assert!(matches!(request.await.unwrap(), Err(Error::Unauthorized)));
    pool.pool.stop();
    until(|| state.dropped.load(Ordering::SeqCst) == 2).await;
}

#[tokio::test]
async fn body_mismatch_and_foreign_origin_fail_before_any_network_call() {
    let limits = Limits::default();
    let (pool, state, _release) = fake_pool(limits.clone()).await;
    assert!(matches!(
        pool.execute(
            upload(b"signed bytes"),
            Some(payload(&limits, b"other bytes").into()),
            Cancellation::default(),
            authorized(),
            Instant::now() + PROGRESS_TIMEOUT
        )
        .await,
        Err(Error::Protocol)
    ));
    assert_eq!(state.calls.load(Ordering::SeqCst), 0);
    pool.pool.stop();
    until(|| state.dropped.load(Ordering::SeqCst) == 2).await;
    let mut https = Https::default();
    let endpoint = Endpoint {
        generation: 0,
        destination: Destination::new("https://other.example").unwrap(),
        retired: Cancellation::default(),
    };
    let mut buffer = Buffer::new(&limits, 65536).unwrap();
    assert!(matches!(
        https.run(&endpoint, &request(false), None, &mut buffer, &guard()),
        Err(Error::Unauthorized)
    ));
    assert!(matches!(
        pool.pool.bind("https://worker.local"),
        Err(Error::InvalidSignal)
    ));
}

#[test]
fn response_limits_apply_before_json_or_binary_parsing_and_do_not_guess_quota() {
    for limit in [MAX_ENVELOPE, 65536, MAX_BATCH + MAX_ENVELOPE] {
        let limits = Limits::default();
        let text = "x".repeat(limit);
        let mut buffer = Buffer::new(&limits, limit).unwrap();
        let result = read_response(
            ureq::Response::new(200, "OK", &text).unwrap(),
            limit,
            &mut buffer,
            &guard(),
        )
        .unwrap();
        assert_eq!(result.status, 200);
        assert_eq!(buffer.len(), limit);
        drop(buffer);
        let mut buffer = Buffer::new(&limits, limit).unwrap();
        assert!(matches!(
            read_response(
                ureq::Response::new(200, "OK", &(text + "!")).unwrap(),
                limit,
                &mut buffer,
                &guard()
            ),
            Err(Error::Protocol)
        ));
        assert_eq!(buffer.len(), limit);
    }
    for status in [429, 503] {
        let mut b = Buffer::new(&Limits::default(), 65536).unwrap();
        let text = "<html>arbitrary 1027 or 1102 text is not quota evidence</html>";
        let parts = read_response(
            ureq::Response::new(status, "Error", text).unwrap(),
            65536,
            &mut b,
            &guard(),
        )
        .unwrap();
        assert_eq!(parts.status, status);
        assert_eq!(b.freeze().unwrap().bytes(), text.as_bytes());
    }
    let mut b = Buffer::new(&Limits::default(), 65536).unwrap();
    assert!(matches!(
        read_response(
            ureq::Response::new(302, "Found", "").unwrap(),
            65536,
            &mut b,
            &guard()
        ),
        Err(Error::Protocol)
    ));
}

#[test]
fn parsed_header_acceptance_includes_status_all_duplicate_values_and_final_crlf() {
    let header = "x-test";
    let base = "HTTP/1.1 200 OK\r\n".len() + 2 + header.len() + 4;
    let exact = format!(
        "HTTP/1.1 200 OK\r\n{header}: {}\r\n\r\n",
        "a".repeat(MAX_ENVELOPE - base)
    );
    assert_eq!(exact.len(), MAX_ENVELOPE);
    let mut b = Buffer::new(&Limits::default(), 65536).unwrap();
    assert!(read_response(exact.parse().unwrap(), 65536, &mut b, &guard()).is_ok());
    let oversized = exact.replacen("x-test: ", "x-test: a", 1);
    assert!(matches!(
        read_response(oversized.parse().unwrap(), 65536, &mut b, &guard()),
        Err(Error::Protocol)
    ));
    let duplicate = "HTTP/1.1 200 OK\r\nX-A: one\r\nX-A: two\r\n\r\n";
    let parts = read_response(duplicate.parse().unwrap(), 65536, &mut b, &guard()).unwrap();
    assert_eq!(parts.headers["x-a"], vec!["one", "two"]);
    // Exercise actual pinned-library parser errors as well as post-Response
    // acceptance; native line/count limits must not leak raw exception text.
    for raw in [
        format!(
            "HTTP/1.1 200 OK\r\nX-Big: {}\r\n\r\n",
            "a".repeat(100 * 1024)
        ),
        format!("HTTP/1.1 200 OK\r\n{}\r\n", "X-A: b\r\n".repeat(101)),
    ] {
        let error = raw.parse::<ureq::Response>().unwrap_err();
        assert_eq!(request_error(&error), Error::Protocol);
    }
}

#[test]
fn body_reader_observes_only_limit_plus_one_and_cancellation_checks_before_read() {
    struct Counted {
        count: Arc<AtomicUsize>,
    }
    impl Read for Counted {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            bytes.fill(1);
            self.count.fetch_add(bytes.len(), Ordering::SeqCst);
            Ok(bytes.len())
        }
    }
    let count = Arc::new(AtomicUsize::new(0));
    let mut b = Buffer::new(&Limits::default(), 65536).unwrap();
    assert!(matches!(
        read_body(
            Counted {
                count: count.clone()
            },
            65536,
            &mut b,
            &guard(),
            true
        ),
        Err(Error::Protocol)
    ));
    assert_eq!(count.load(Ordering::SeqCst), 65537);
    let g = guard();
    g.cancel.cancel();
    let mut b = Buffer::new(&Limits::default(), 65536).unwrap();
    assert!(matches!(
        read_body(
            Counted {
                count: count.clone()
            },
            65536,
            &mut b,
            &g,
            true
        ),
        Err(Error::Cancelled)
    ));
    assert_eq!(count.load(Ordering::SeqCst), 65537);
    let mut error_body = Buffer::new(&Limits::default(), MAX_ENVELOPE).unwrap();
    assert!(matches!(
        read_body(
            Counted {
                count: count.clone()
            },
            MAX_ENVELOPE,
            &mut error_body,
            &guard(),
            false
        ),
        Err(Error::CloudflareResourceLimit)
    ));
    assert_eq!(
        count.load(Ordering::SeqCst),
        65537 + MAX_ENVELOPE,
        "non-2xx reads stop at the bound without an extra byte or drain"
    );
}

#[test]
fn platform_boundary_table_distinguishes_header_limits_truncation_and_exact_errors() {
    use crate::bbs_sync::relay::response;
    let cases = [
        (
            503,
            "text/html",
            "h".repeat(12 * 1024),
            MAX_ENVELOPE,
            Error::CloudflareResourceLimit,
        ),
        (
            429,
            "text/html",
            "<html>1027</html>".into(),
            MAX_ENVELOPE,
            Error::CloudflareResourceLimit,
        ),
        (
            429,
            "application/json",
            r#"{"ok":false,"error":"rate_limited"}"#.into(),
            MAX_ENVELOPE,
            Error::Busy,
        ),
        (
            401,
            "application/json",
            r#"{"ok":false,"error":"stale_signature"}"#.into(),
            MAX_ENVELOPE,
            Error::StaleSignature,
        ),
        (
            200,
            "application/json",
            "x".repeat(9 * 1024),
            MAX_ENVELOPE,
            Error::Protocol,
        ),
        (
            200,
            "application/json",
            r#"{"accepted":true}"#.into(),
            MAX_ENVELOPE,
            Error::Protocol,
        ), // missing Send.consumed
        (
            200,
            "text/html",
            "<html>1102</html>".into(),
            MAX_ENVELOPE,
            Error::Protocol,
        ),
        (
            503,
            "text/html",
            "h".repeat(70 * 1024),
            65536,
            Error::CloudflareResourceLimit,
        ),
    ];
    for (status, mime, body, limit, expected) in cases {
        let raw=format!("HTTP/1.1 {status} Response\r\nContent-Type: {mime}\r\nContent-Length: {}\r\n\r\n{body}",body.len());
        let mut buffer = Buffer::new(&Limits::default(), limit).unwrap();
        let result = read_response(raw.parse().unwrap(), limit, &mut buffer, &guard());
        let error = match result {
            Err(error) => error,
            Ok(parts) => {
                let payload = buffer.freeze().unwrap();
                if status == 200 {
                    response::json(parts.status, &parts.headers, payload.bytes())
                        .and_then(|bytes| response::mutation(bytes, true))
                        .unwrap_err()
                } else {
                    response::rejected(parts.status, &parts.headers, payload.bytes())
                }
            }
        };
        assert_eq!(error, expected, "{status} {mime} {} B", body.len());
    }
    let mut raw = String::from("HTTP/1.1 503 Error\r\nContent-Length: 0\r\n");
    for i in 0..5 {
        raw.push_str(&format!("X-{i}: {}\r\n", "h".repeat(33 * 1024)));
    }
    raw.push_str("\r\n");
    assert!(raw.len() > 128 * 1024);
    let mut buffer = Buffer::new(&Limits::default(), MAX_ENVELOPE).unwrap();
    assert!(matches!(
        read_response(raw.parse().unwrap(), MAX_ENVELOPE, &mut buffer, &guard()),
        Err(Error::Protocol)
    ));
    assert_eq!(
        buffer.len(),
        0,
        "oversize headers are rejected before any body parsing"
    );
}

#[test]
fn rejected_empty_https_response_closes_connection_even_if_ureq_already_pooled_it() {
    https_refusal_closes_connection(0);
}

#[test]
fn ack_error_page_truncates_at_eight_kib_as_resource_and_closes_the_https_connection() {
    https_refusal_closes_connection(1);
}

#[test]
fn oversized_successful_ack_body_is_protocol_and_closes_the_https_connection() {
    https_refusal_closes_connection(2);
}

#[test]
fn complete_application_backpressure_is_busy_and_closes_the_https_connection() {
    https_refusal_closes_connection(3);
}

fn https_refusal_closes_connection(case: u8) {
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
    let server = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![der.clone()], key)
        .unwrap();
    let mut roots = rustls::RootCertStore::empty();
    roots.add(der).unwrap();
    let client = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (closed, receive) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut tls = rustls::StreamOwned::new(
            rustls::ServerConnection::new(Arc::new(server)).unwrap(),
            socket,
        );
        let mut reader = BufReader::new(&mut tls);
        let mut headers = Vec::new();
        loop {
            let mut line = String::new();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            if line == "\r\n" {
                break;
            }
            headers.push(line);
        }
        assert!(headers
            .iter()
            .any(|h| h.starts_with("x-kota-relay-signature:")));
        let relay_names: Vec<_> = headers
            .iter()
            .filter_map(|h| h.split_once(':').map(|(n, _)| n))
            .filter(|n| n.starts_with("x-kota-relay-"))
            .collect();
        if case == 0 {
            assert_eq!(relay_names.len(), 5);
        } // read proof only
        if case != 0 {
            let count: usize = headers
                .iter()
                .find_map(|h| h.strip_prefix("content-length:"))
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            let mut request_body = vec![0; count];
            reader.read_exact(&mut request_body).unwrap();
            assert_eq!(request_body, br#"{"through":"0","final":false}"#);
        }
        drop(reader);
        if case == 0 {
            write!(
                tls,
                "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nX-Over: {}\r\n\r\n",
                "a".repeat(MAX_ENVELOPE)
            )
            .unwrap();
        } else if case == 3 {
            let body = r#"{"ok":false,"error":"rate_limited"}"#;
            write!(tls, "HTTP/1.1 429 Response\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{body}", body.len()).unwrap();
        } else {
            let (status, size) = if case == 1 {
                (503, 12 * 1024)
            } else {
                (200, 9 * 1024)
            };
            write!(tls, "HTTP/1.1 {status} Response\r\nContent-Type: text/html\r\nContent-Length: {size}\r\nConnection: keep-alive\r\n\r\n{}", "h".repeat(size)).unwrap();
        }
        tls.flush().unwrap();
        let result = tls.read(&mut [0u8; 1]);
        // A TCP close without TLS close_notify is UnexpectedEof; a pooled open
        // socket would block until the read timeout, which must not pass.
        let disconnected = matches!(result, Ok(0))
            || result.as_ref().is_err_and(|e| {
                matches!(
                    e.kind(),
                    std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset
                )
            });
        closed.send(disconnected).unwrap();
    });
    let origin = format!("https://worker.example:{}", address.port());
    let destination = Destination::new(&origin).unwrap();
    // Test-only root and resolver reach this isolated loopback server. Neither
    // override is available to HttpPool::start or any production constructor.
    let agent = ureq::AgentBuilder::new()
        .https_only(true)
        .try_proxy_from_env(false)
        .redirects(0)
        .timeout(Duration::from_secs(5))
        .tls_config(Arc::new(client))
        .resolver(move |_: &str| Ok(vec![address]))
        .build();
    let endpoint = Endpoint {
        generation: 0,
        destination: destination.clone(),
        retired: Cancellation::default(),
    };
    let mut https = Https {
        selected: Some((0, destination, agent)),
        ..Https::default()
    };
    let f = fixture();
    let c = &f.contexts[0];
    let request = if case == 0 {
        SignedRequest::sign(
            &f.identities[0],
            &c.peer.local_membership_id,
            Target::poll(&origin, &c.peer.group_id, None).unwrap(),
            Fields::Read,
            &[],
            f.now,
        )
        .unwrap()
    } else {
        SignedRequest::sign(
            &f.identities[0],
            &c.peer.local_membership_id,
            Target::post(&origin, &c.peer.group_id, Route::Ack).unwrap(),
            Fields::Frame {
                boot: &c.boot,
                session: &"e".repeat(64),
                direction: Direction::ServerToClient,
                sequence: 0,
                final_flag: false,
            },
            br#"{"through":"0","final":false}"#,
            f.now,
        )
        .unwrap()
    };
    let limits = Limits::default();
    let body: Option<Body> =
        (case != 0).then(|| payload(&limits, br#"{"through":"0","final":false}"#).into());
    let mut response = Buffer::new(&limits, request.response_limit()).unwrap();
    let result = https.run(&endpoint, &request, body.as_ref(), &mut response, &guard());
    if case == 3 {
        let parts = result.unwrap();
        let bytes = response.freeze().unwrap();
        assert_eq!(
            super::super::response::rejected(parts.status, &parts.headers, bytes.bytes()),
            Error::Busy
        );
    } else {
        assert_eq!(
            result.err(),
            Some(if case == 1 {
                Error::CloudflareResourceLimit
            } else {
                Error::Protocol
            })
        );
        if case == 1 {
            assert_eq!(response.len(), MAX_ENVELOPE);
        }
    }
    assert!(
        https.selected.is_none(),
        "rejected connection must not return to the Agent pool"
    );
    assert!(receive.recv_timeout(Duration::from_secs(6)).unwrap());
    server.join().unwrap();
}

#[tokio::test]
async fn invalid_envelope_never_escapes_http_worker_and_discards_backend_without_new_workers() {
    type Answer = (u16, BTreeMap<String, Vec<String>>, Vec<u8>);
    struct Controlled {
        answer: Arc<Mutex<Answer>>,
        discarded: Arc<AtomicUsize>,
    }
    impl Backend for Controlled {
        fn discard(&mut self) {
            self.discarded.fetch_add(1, Ordering::SeqCst);
        }
        fn run(
            &mut self,
            _: &Endpoint,
            _: &SignedRequest,
            _: Option<&Body>,
            response: &mut Buffer,
            guard: &Guard,
        ) -> Result<Parts> {
            guard.check()?;
            let answer = self.answer.lock().unwrap();
            response.spare_mut()[..answer.2.len()].copy_from_slice(&answer.2);
            response.advance(answer.2.len())?;
            Ok(Parts {
                status: answer.0,
                headers: answer.1.clone(),
            })
        }
    }
    let expected = request(true);
    let bytes = envelope::tests::empty(expected.receive_request().unwrap());
    let headers = BTreeMap::from([("content-type".into(), vec![envelope::CONTENT_TYPE.into()])]);
    let answer = Arc::new(Mutex::new((200, headers.clone(), bytes.clone())));
    let discarded = Arc::new(AtomicUsize::new(0));
    let created = Arc::new(AtomicUsize::new(0));
    let pool = HttpPool::start_with(Limits::default(), {
        let answer = answer.clone();
        let discarded = discarded.clone();
        let created = created.clone();
        move |_| {
            created.fetch_add(1, Ordering::SeqCst);
            Box::new(Controlled {
                answer: answer.clone(),
                discarded: discarded.clone(),
            })
        }
    })
    .await
    .unwrap();
    let pool = pool.bind(&fixture().contexts[0].origin).unwrap();
    let run = || {
        pool.execute(
            request(true),
            None,
            Cancellation::default(),
            authorized(),
            Instant::now() + PROGRESS_TIMEOUT,
        )
    };
    assert_eq!(
        run().await.unwrap().into_receive().unwrap().items().len(),
        1
    );

    let mut wrong_magic = bytes.clone();
    wrong_magic[3] = b'2';
    let mut wrong_boot = bytes.clone();
    let boot = expected.receive_request().unwrap().boot.as_bytes();
    let at = wrong_boot
        .windows(boot.len())
        .position(|w| w == boot)
        .unwrap();
    wrong_boot[at] = b'c';
    let mut trailing = bytes.clone();
    trailing.push(0);
    let cases = [
        ("missing MIME", BTreeMap::new(), bytes.clone()),
        (
            "wrong MIME",
            BTreeMap::from([("content-type".into(), vec!["application/json".into()])]),
            bytes.clone(),
        ),
        (
            "duplicate MIME",
            BTreeMap::from([(
                "content-type".into(),
                vec![envelope::CONTENT_TYPE.into(); 2],
            )]),
            bytes.clone(),
        ),
        (
            "compression",
            BTreeMap::from([
                ("content-type".into(), vec![envelope::CONTENT_TYPE.into()]),
                ("content-encoding".into(), vec!["gzip".into()]),
            ]),
            bytes.clone(),
        ),
        ("bad magic", headers.clone(), wrong_magic),
        ("wrong boot", headers.clone(), wrong_boot),
        ("trailing bytes", headers.clone(), trailing),
        ("empty", headers.clone(), Vec::new()),
        ("JSON fallback", headers.clone(), b"{}".to_vec()),
    ];
    for (index, (label, invalid_headers, invalid_body)) in cases.into_iter().enumerate() {
        *answer.lock().unwrap() = (200, invalid_headers, invalid_body);
        assert!(matches!(run().await, Err(Error::Protocol)), "{label}");
        assert_eq!(discarded.load(Ordering::SeqCst), index + 1, "{label}");
        *answer.lock().unwrap() = (200, headers.clone(), bytes.clone());
        assert_eq!(
            run().await.unwrap().into_receive().unwrap().items().len(),
            1
        );
    }
    // Platform errors are outside this codec: bounded unknown HTML/status must
    // not be classified as quota or treated as a successful receive packet.
    *answer.lock().unwrap() = (429, BTreeMap::new(), b"<html>1027 1102</html>".to_vec());
    let error = run().await.unwrap();
    assert_eq!(error.status, 429);
    assert_eq!(
        error.body.as_ref().unwrap().bytes(),
        b"<html>1027 1102</html>"
    );
    assert!(matches!(error.into_receive(), Err(Error::Protocol)));
    assert_eq!(discarded.load(Ordering::SeqCst), 9);
    assert_eq!(created.load(Ordering::SeqCst), 2);
    pool.pool.stop();
}

#[tokio::test]
async fn real_https_valid_envelope_then_bad_magic_closes_the_pooled_socket() {
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
    let server = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![der.clone()], key)
        .unwrap();
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
    let origin = format!("https://worker.example:{}", address.port());
    let signed = || {
        let f = fixture();
        let c = &f.contexts[0];
        SignedRequest::sign(
            &f.identities[0],
            &c.peer.local_membership_id,
            Target::receive(
                &origin,
                &c.peer.group_id,
                &c.boot,
                ReceiveMode::Probe,
                &[("e".repeat(64), 0)],
            )
            .unwrap(),
            Fields::Read,
            &[],
            f.now,
        )
        .unwrap()
    };
    let valid = envelope::tests::empty(signed().receive_request().unwrap());
    let (closed, receive) = tokio::sync::oneshot::channel();
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let socket = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(e) => panic!("isolated HTTPS accept: {e}"),
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
            rustls::ServerConnection::new(Arc::new(server)).unwrap(),
            socket,
        );
        for turn in 0..2 {
            let mut reader = BufReader::new(&mut tls);
            let mut line = String::new();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            assert!(line.starts_with("GET /bbs/relay/receive?"));
            assert!(line.contains("&mode=probe&"));
            loop {
                line.clear();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
            }
            drop(reader);
            let mut body = valid.clone();
            if turn == 1 {
                body[3] = b'2';
            }
            write!(tls, "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n", body.len()).unwrap();
            tls.write_all(&body).unwrap();
            tls.flush().unwrap();
        }
        // Both responses deliberately allow pooling. Parsing rejection must
        // close the socket immediately, not merely drop the response handle.
        let result = tls.read(&mut [0u8; 1]);
        let disconnected = matches!(result, Ok(0))
            || result.as_ref().is_err_and(|e| {
                matches!(
                    e.kind(),
                    std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset
                )
            });
        let _ = closed.send(disconnected);
    });
    let destination = Destination::new(&origin).unwrap();
    // Test-only roots/resolver, unavailable to the production pool constructor.
    let pool = HttpPool::start_with(Limits::default(), move |_| {
        Box::new(Https {
            selected: Some((
                1,
                destination.clone(),
                ureq::AgentBuilder::new()
                    .https_only(true)
                    .try_proxy_from_env(false)
                    .redirects(0)
                    .timeout(Duration::from_secs(5))
                    .tls_config(client.clone())
                    .resolver(move |_: &str| Ok(vec![address]))
                    .build(),
            )),
            ..Https::default()
        })
    })
    .await
    .unwrap();
    let pool = pool.bind(&origin).unwrap();
    let packet = pool
        .execute(
            signed(),
            None,
            Cancellation::default(),
            authorized(),
            Instant::now() + PROGRESS_TIMEOUT,
        )
        .await
        .unwrap()
        .into_receive()
        .unwrap();
    assert_eq!(packet.items()[0].session(), "e".repeat(64));
    assert_eq!(packet.items()[0].window(), (0, 0, false));
    assert!(matches!(
        pool.execute(
            signed(),
            None,
            Cancellation::default(),
            authorized(),
            Instant::now() + PROGRESS_TIMEOUT
        )
        .await,
        Err(Error::Protocol)
    ));
    assert!(tokio::time::timeout(Duration::from_secs(6), receive)
        .await
        .unwrap()
        .unwrap());
    server.join().unwrap();
    pool.pool.stop();
}

#[tokio::test]
async fn ack_and_probe_use_reserved_credit_when_all_general_bytes_are_held() {
    let limits = Limits::default();
    let (pool, state, _release) = fake_pool(limits.clone()).await;
    let bulk = limits.reserve(APP_QUEUE_BYTES - SMALL_RESERVE).unwrap();
    assert!(matches!(limits.reserve(1), Err(Error::Busy)));
    let probe = pool
        .execute(
            request(true),
            None,
            Cancellation::default(),
            authorized(),
            Instant::now() + PROGRESS_TIMEOUT,
        )
        .await
        .unwrap();
    assert_eq!(probe.status, 200);
    drop(probe);
    let f = fixture();
    let c = &f.contexts[1];
    let body = br#"{"through":"0","final":false}"#;
    let ack = SignedRequest::sign(
        &f.identities[1],
        &c.peer.local_membership_id,
        Target::post(&c.origin, &c.peer.group_id, Route::Ack).unwrap(),
        Fields::Frame {
            boot: &c.boot,
            session: &"e".repeat(64),
            direction: Direction::ClientToServer,
            sequence: 0,
            final_flag: false,
        },
        body,
        f.now,
    )
    .unwrap();
    let reply = pool
        .execute(
            ack,
            Some(pool.small_body(body).unwrap().into()),
            Cancellation::default(),
            authorized(),
            Instant::now() + PROGRESS_TIMEOUT,
        )
        .await
        .unwrap();
    assert_eq!(reply.status, 200);
    assert_eq!(state.calls.load(Ordering::SeqCst), 2);
    assert!(matches!(limits.reserve(1), Err(Error::Busy)));
    // Even after dropping the owner, a returned buffer/header still owns the
    // reservation. It may not coexist with a second full account allocation.
    pool.pool.stop();
    drop(pool);
    until(|| state.dropped.load(Ordering::SeqCst) == 2).await;
    drop(bulk);
    assert!(matches!(limits.reserve(APP_QUEUE_BYTES), Err(Error::Busy)));
    drop(reply);
    assert!(limits.reserve(APP_QUEUE_BYTES).is_ok());
}

#[tokio::test]
async fn receipts_small_lane_fetches_authenticated_credit_when_full_receive_cannot_be_admitted() {
    use crate::bbs_sync::relay::{proof::SignedAck, window::SendWindow};
    // Isolated HTTP backend returns the real binary envelope. Timed
    // probe/receipts selection remains separate from this byte-budget test.
    struct ReceiptBackend {
        lane: Lane,
        bytes: Arc<Vec<u8>>,
    }
    impl Backend for ReceiptBackend {
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
            assert!(matches!(self.lane, Lane::Small));
            assert!(request.url().contains("&mode=receipts&"));
            assert!(body.is_none());
            response.spare_mut()[..self.bytes.len()].copy_from_slice(&self.bytes);
            response.advance(self.bytes.len())?;
            Ok(Parts {
                status: 200,
                headers: BTreeMap::from([(
                    "content-type".into(),
                    vec![envelope::CONTENT_TYPE.into()],
                )]),
            })
        }
    }
    let f = fixture();
    let c = &f.contexts[1];
    let session = "e".repeat(64);
    let body = r#"{"through":"1","final":false}"#;
    let signed = SignedRequest::sign(
        &f.identities[1],
        &c.peer.local_membership_id,
        Target::post(&c.origin, &c.peer.group_id, Route::Ack).unwrap(),
        Fields::Frame {
            boot: &c.boot,
            session: &session,
            direction: Direction::ClientToServer,
            sequence: 0,
            final_flag: false,
        },
        body.as_bytes(),
        f.now,
    )
    .unwrap();
    let receipt = serde_json::json!({
        "proof": {
            "device": c.peer.local_device_id, "membership": c.peer.local_membership_id,
            "time": f.now, "nonce": "", "boot": c.boot, "session": session,
            "direction": "c2s", "sequence": 0, "final": false,
            "signature": signed.headers().unwrap()["x-kota-relay-signature"],
        },
        "body": body,
    });
    let encoded = Arc::new(crate::bbs_sync::relay::envelope::tests::encode_json(
        &serde_json::json!({"boot": c.boot, "items": [{
            "session": session, "next": "0", "consumed": "0", "closed": false,
            "batches": [], "ack": receipt,
        }]}),
        &[],
    ));
    let limits = Limits::default();
    let pool = HttpPool::start_with(limits.clone(), move |lane| {
        Box::new(ReceiptBackend {
            lane,
            bytes: encoded.clone(),
        })
    })
    .await
    .unwrap();
    let pool = pool.bind(&c.origin).unwrap();
    let mut window = SendWindow::new(
        f.contexts[0].clone(),
        session.clone(),
        authorized(),
        Cancellation::default(),
    )
    .unwrap();
    window
        .enqueue(
            &f.identities[0],
            super::super::upload::Upload::single(payload(&limits, &vec![7; MAX_BATCH])).unwrap(),
            f.now,
        )
        .unwrap();
    window.submitted(0).unwrap();
    let filler = limits
        .reserve(APP_QUEUE_BYTES - SMALL_RESERVE - MAX_BATCH)
        .unwrap();
    assert!(matches!(limits.reserve(1), Err(Error::Busy)));
    let request = |mode| {
        SignedRequest::sign(
            &f.identities[0],
            &f.contexts[0].peer.local_membership_id,
            Target::receive(
                &c.origin,
                &c.peer.group_id,
                &c.boot,
                mode,
                &[(session.clone(), 0)],
            )
            .unwrap(),
            Fields::Read,
            b"",
            f.now,
        )
        .unwrap()
    };
    assert!(matches!(
        pool.execute(
            request(ReceiveMode::Data),
            None,
            Cancellation::default(),
            authorized(),
            Instant::now() + PROGRESS_TIMEOUT
        )
        .await,
        Err(Error::Busy)
    ));
    let reply = pool
        .execute(
            request(ReceiveMode::Receipts),
            None,
            Cancellation::default(),
            authorized(),
            Instant::now() + PROGRESS_TIMEOUT,
        )
        .await
        .unwrap();
    assert_eq!(reply.status, 200);
    // Receiving the response is not credit. A forged high-water mark cannot
    // free the account's last retained byte; only peer verification can.
    assert!(matches!(limits.reserve(1), Err(Error::Busy)));
    let mut forged = receipt;
    forged["body"] = r#"{"through":"2","final":false}"#.into();
    assert!(window
        .acknowledge(
            &SignedAck::decode(&serde_json::to_vec(&forged).unwrap()).unwrap(),
            f.now
        )
        .is_err());
    assert!(matches!(limits.reserve(1), Err(Error::Busy)));
    let packet = reply.into_receive().unwrap();
    let ack = packet.items()[0].unverified_ack().unwrap();
    assert_eq!(window.acknowledge(ack, f.now).unwrap().consumed_batches, 1);
    assert!(limits.reserve(MAX_BATCH).is_ok());
    assert_eq!(window.acknowledge(ack, f.now).unwrap().consumed_batches, 0);
    drop(filler);
    pool.pool.stop();
}

mod bindings;

mod uploads;
