use super::*;
use crate::bbs_sync::relay::tests::fixture;
use rustls::{
    client::ResolvesClientCert,
    sign::{CertifiedKey, Signer, SigningKey, SingleCertAndKey},
    SignatureAlgorithm,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

fn pending() -> (PendingTls, PendingTls) {
    let f = fixture();
    let client = PendingTls::client(
        &f.identities[0],
        f.contexts[0].clone(),
        Arc::new(|| true),
        Cancellation::default(),
        f.now,
    )
    .unwrap();
    let server = PendingTls::server(
        &f.identities[1],
        f.contexts[1].clone(),
        Arc::new(|| true),
        Cancellation::default(),
        client.statement(),
        f.now,
    )
    .unwrap();
    (client, server)
}
fn pair() -> (TlsStream, TlsStream) {
    let (client, server) = pending();
    let a = client.statement().clone();
    let b = server.statement().clone();
    (
        client.accept(&b, fixture().now).unwrap(),
        server.accept(&a, fixture().now).unwrap(),
    )
}
fn transfer(sender: &mut TlsStream, receiver: &mut TlsStream, chunk: usize) -> Result<usize> {
    let mut buffer = [0; TLS_WRITE_LIMIT + 1024];
    let n = sender.drain_tls(&mut buffer)?;
    let mut offset = 0;
    while offset < n {
        let end = n.min(offset + chunk);
        offset += receiver.receive(&buffer[offset..end])?;
    }
    Ok(n)
}
fn handshake(client: &mut TlsStream, server: &mut TlsStream, chunk: usize) -> Result<()> {
    for _ in 0..32 {
        transfer(client, server, chunk)?;
        transfer(server, client, chunk)?;
        if client.handshake_complete()? && server.handshake_complete()? {
            return Ok(());
        }
    }
    panic!("bounded handshake did not complete");
}
fn replace_connections(
    client: &mut TlsStream,
    server: &mut TlsStream,
    c: ClientConfig,
    s: ServerConfig,
) {
    client.connection = Connection::Client(
        ClientConnection::new(
            Arc::new(c),
            ServerName::try_from("bbs.invalid").unwrap().to_owned(),
        )
        .unwrap(),
    );
    server.connection = Connection::Server(ServerConnection::new(Arc::new(s)).unwrap());
    for stream in [client, server] {
        stream.connection.set_buffer_limit(Some(TLS_WRITE_LIMIT));
    }
}

#[test]
fn real_tls13_mutual_auth_fragments_and_plaintext_follow_the_same_stream() {
    for chunk in [1, 7, 101, MAX_BATCH] {
        let (mut client, mut server) = pair();
        assert_eq!(client.session_id, server.session_id);
        assert!(matches!(
            client.write_plaintext(b"no early payload"),
            Err(Error::Busy)
        ));
        handshake(&mut client, &mut server, chunk).unwrap();
        assert_eq!(
            client.connection.protocol_version(),
            Some(rustls::ProtocolVersion::TLSv1_3)
        );
        assert_eq!(
            server.connection.protocol_version(),
            Some(rustls::ProtocolVersion::TLSv1_3)
        );
        assert_eq!(
            client.connection.handshake_kind(),
            Some(rustls::HandshakeKind::Full)
        );
        assert_eq!(
            server.connection.handshake_kind(),
            Some(rustls::HandshakeKind::Full)
        );
        if let Connection::Server(s) = &server.connection {
            assert_eq!(s.server_name(), None);
        }
        assert_eq!(client.connection.alpn_protocol(), None);
        for forward in [true, false] {
            let (from, to) = if forward {
                (&mut client, &mut server)
            } else {
                (&mut server, &mut client)
            };
            assert_eq!(
                from.write_plaintext(b"private roster + file bytes")
                    .unwrap(),
                27
            );
            transfer(from, to, chunk).unwrap();
            let mut text = [0; 64];
            let n = to.read_plaintext(&mut text).unwrap();
            assert_eq!(&text[..n], b"private roster + file bytes");
        }
    }
    let (a, _) = pending();
    let (b, _) = pending();
    assert_ne!(a.credentials.fingerprint(), b.credentials.fingerprint());
}

#[test]
fn both_pins_are_checked_even_when_device_signatures_are_valid() {
    let f = fixture();
    for wrong_sender in [0, 1] {
        let mut a = PendingTls::client(
            &f.identities[0],
            f.contexts[0].clone(),
            Arc::new(|| true),
            Cancellation::default(),
            f.now,
        )
        .unwrap();
        if wrong_sender == 0 {
            a.local.statement.certificate_sha256 = "f".repeat(64);
            a.local.signature = f.identities[0]
                .sign(&a.local.statement.signing_bytes().unwrap())
                .unwrap();
        }
        let mut b = PendingTls::server(
            &f.identities[1],
            f.contexts[1].clone(),
            Arc::new(|| true),
            Cancellation::default(),
            a.statement(),
            f.now,
        )
        .unwrap();
        if wrong_sender == 1 {
            b.local.statement.certificate_sha256 = "f".repeat(64);
            b.local.signature = f.identities[1]
                .sign(&b.local.statement.signing_bytes().unwrap())
                .unwrap();
        }
        let offer = a.statement().clone();
        // Signed declarations pass; the actual certificate on the wire does not.
        let mut client = a.accept(b.statement(), f.now).unwrap();
        let mut server = b.accept(&offer, f.now).unwrap();
        assert!(matches!(
            handshake(&mut client, &mut server, 4096),
            Err(Error::Integrity)
        ));
        let rejected = if wrong_sender == 0 {
            &mut server
        } else {
            &mut client
        };
        assert!(matches!(
            rejected.read_plaintext(&mut [0; 10]),
            Err(Error::Integrity)
        ));
    }
}

#[derive(Debug)]
struct NoCertificate;
impl ResolvesClientCert for NoCertificate {
    fn resolve(&self, _: &[&[u8]], _: &[SignatureScheme]) -> Option<Arc<CertifiedKey>> {
        None
    }
    fn has_certs(&self) -> bool {
        false
    }
}

#[test]
fn client_certificate_is_mandatory_and_resumption_early_data_sni_are_disabled() {
    let (a, b) = pending();
    let mut c = client_config(&a.credentials, b.credentials.fingerprint()).unwrap();
    let s = server_config(&b.credentials, a.credentials.fingerprint()).unwrap();
    assert!(!c.enable_early_data);
    assert!(!c.enable_sni);
    assert_eq!(s.max_early_data_size, 0);
    assert_eq!(s.send_tls13_tickets, 0);
    assert!(!s.session_storage.can_cache());
    assert!(!s.ticketer.enabled());
    let pin = PinnedCertificate(a.credentials.fingerprint());
    assert!(pin.offer_client_auth());
    assert!(pin.client_auth_mandatory());
    assert!(!ClientCertVerifier::requires_raw_public_keys(&pin));
    assert!(!ServerCertVerifier::requires_raw_public_keys(&pin));
    c.client_auth_cert_resolver = Arc::new(NoCertificate);
    let (mut client, mut server) = pair();
    replace_connections(&mut client, &mut server, c, s);
    if let Connection::Client(c) = &mut client.connection {
        assert!(c.early_data().is_none());
    }
    assert!(matches!(
        handshake(&mut client, &mut server, 4096),
        Err(Error::Integrity)
    ));
    assert_eq!(server.terminal, Some(Error::Integrity));
}

#[derive(Debug)]
struct WrongKey {
    key: Arc<dyn SigningKey>,
    calls: Arc<AtomicUsize>,
}
#[derive(Debug)]
struct WrongSigner {
    signer: Box<dyn Signer>,
    calls: Arc<AtomicUsize>,
}
impl SigningKey for WrongKey {
    fn choose_scheme(&self, offered: &[SignatureScheme]) -> Option<Box<dyn Signer>> {
        Some(Box::new(WrongSigner {
            signer: self.key.choose_scheme(offered)?,
            calls: self.calls.clone(),
        }))
    }
    fn algorithm(&self) -> SignatureAlgorithm {
        SignatureAlgorithm::ECDSA
    }
}
impl Signer for WrongSigner {
    fn sign(&self, message: &[u8]) -> std::result::Result<Vec<u8>, rustls::Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.signer.sign(message)
    }
    fn scheme(&self) -> SignatureScheme {
        self.signer.scheme()
    }
}

#[test]
fn replayed_public_certificate_without_private_key_fails_certificate_verify_both_ways() {
    for attacker_is_client in [true, false] {
        let (a, b) = pending();
        let mut c = client_config(&a.credentials, b.credentials.fingerprint()).unwrap();
        let mut s = server_config(&b.credentials, a.credentials.fingerprint()).unwrap();
        let stolen = if attacker_is_client {
            a.credentials.cert.clone()
        } else {
            b.credentials.cert.clone()
        };
        let attacker = Credentials::generate().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let bad_key = Arc::new(WrongKey {
            key: default_provider()
                .key_provider
                .load_private_key(attacker.key)
                .unwrap(),
            calls: calls.clone(),
        });
        // Deliberately bypass the honest builder's keys_match check to model a
        // malicious endpoint that knows the public DER but not its private key.
        let resolver = Arc::new(SingleCertAndKey::from(CertifiedKey::new(
            vec![stolen],
            bad_key,
        )));
        if attacker_is_client {
            c.client_auth_cert_resolver = resolver;
        } else {
            s.cert_resolver = resolver;
        }
        let (mut client, mut server) = pair();
        replace_connections(&mut client, &mut server, c, s);
        assert!(matches!(
            handshake(&mut client, &mut server, 4096),
            Err(Error::Integrity)
        ));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "rejection must reach real CertificateVerify"
        );
        let verifier = if attacker_is_client {
            &mut server
        } else {
            &mut client
        };
        assert_eq!(verifier.terminal, Some(Error::Integrity));
        assert!(matches!(
            verifier.write_plaintext(b"forbidden"),
            Err(Error::Integrity)
        ));
    }
}

#[test]
fn tls12_only_peer_is_rejected() {
    let (a, b) = pending();
    let c = ClientConfig::builder_with_provider(Arc::new(default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS12])
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedCertificate(b.credentials.fingerprint())))
        .with_client_auth_cert(
            vec![a.credentials.cert.clone()],
            a.credentials.key.clone_key(),
        )
        .unwrap();
    let s = server_config(&b.credentials, a.credentials.fingerprint()).unwrap();
    let (mut client, mut server) = pair();
    replace_connections(&mut client, &mut server, c, s);
    assert!(matches!(
        handshake(&mut client, &mut server, 4096),
        Err(Error::Integrity)
    ));
    assert_eq!(server.terminal, Some(Error::Integrity));
}

#[test]
fn reused_configuration_still_requires_a_full_handshake_and_has_no_zero_rtt() {
    let (a, b) = pending();
    let c = client_config(&a.credentials, b.credentials.fingerprint()).unwrap();
    let s = server_config(&b.credentials, a.credentials.fingerprint()).unwrap();
    for _ in 0..2 {
        let (mut client, mut server) = pair();
        replace_connections(&mut client, &mut server, c.clone(), s.clone());
        if let Connection::Client(c) = &mut client.connection {
            assert!(c.early_data().is_none());
        }
        handshake(&mut client, &mut server, 4096).unwrap();
        assert_eq!(
            client.connection.handshake_kind(),
            Some(rustls::HandshakeKind::Full)
        );
        assert_eq!(
            server.connection.handshake_kind(),
            Some(rustls::HandshakeKind::Full)
        );
        // No NewSessionTicket flight remains after the server accepts Finished.
        assert_eq!(server.drain_tls(&mut [0; 4096]).unwrap(), 0);
    }
}

#[test]
fn ciphertext_tamper_or_duplicate_is_fatal_and_cannot_be_retried_in_same_tls() {
    for corrupt in [true, false] {
        let (mut client, mut server) = pair();
        handshake(&mut client, &mut server, 4096).unwrap();
        client.write_plaintext(b"real payload").unwrap();
        let mut cipher = vec![0; 4096];
        let n = client.drain_tls(&mut cipher).unwrap();
        cipher.truncate(n);
        assert!(!cipher.windows(12).any(|w| w == b"real payload"));
        if corrupt {
            *cipher.last_mut().unwrap() ^= 1;
        } else {
            assert_eq!(server.receive(&cipher).unwrap(), n);
            assert_eq!(server.read_plaintext(&mut [0; 64]).unwrap(), 12);
        }
        assert!(matches!(server.receive(&cipher), Err(Error::Integrity)));
        assert!(matches!(
            server.read_plaintext(&mut [0; 64]),
            Err(Error::Integrity)
        ));
        assert!(matches!(server.receive(&[]), Err(Error::Integrity)));
    }
}

#[test]
fn tls_buffers_backpressure_without_unbounded_plaintext_or_ciphertext() {
    let (mut client, mut server) = pair();
    handshake(&mut client, &mut server, 4096).unwrap();
    let block = vec![42; MAX_FRAME];
    let mut written = 0;
    for _ in 0..32 {
        let n = client.write_plaintext(&block).unwrap();
        written += n;
        if n == 0 {
            break;
        }
    }
    assert!(written > 0 && written <= TLS_WRITE_LIMIT);
    assert_eq!(client.write_plaintext(&block).unwrap(), 0);
    let mut cipher = vec![0; TLS_WRITE_LIMIT + 1024];
    let n = client.drain_tls(&mut cipher).unwrap();
    assert!(n <= TLS_WRITE_LIMIT + 22);
    assert_eq!(client.drain_tls(&mut [0; 1024]).unwrap(), 0);
    let mut offset = 0;
    let mut received = 0;
    let mut plaintext_peak = 0;
    let mut plain = [0; MAX_FRAME * 2];
    while offset < n {
        offset += server.receive(&cipher[offset..n]).unwrap();
        if server.plaintext_pending > 0 {
            plaintext_peak = plaintext_peak.max(server.plaintext_pending);
            assert!(server.plaintext_pending <= MAX_FRAME * 2);
            assert!(matches!(server.receive(&[1]), Err(Error::Busy)));
            received += server.read_plaintext(&mut plain).unwrap();
        }
    }
    assert_eq!(received, written);
    println!(
        "BBS_RELAY_TLS_METRICS {}",
        serde_json::json!({
            "plaintextAcceptedBeforeBackpressure": written,
            "outgoingCiphertextBytes": n,
            "receivePlaintextPeak": plaintext_peak,
            "clientHandshakeInputBytes": client.handshake_input,
            "serverHandshakeInputBytes": server.handshake_input,
            "rssMeasured": false,
        })
    );
    assert!(client.write_plaintext(&block).unwrap() > 0);
    assert!(matches!(
        client.write_plaintext(&vec![0; MAX_FRAME + 1]),
        Err(Error::Protocol)
    ));
}

#[test]
fn membership_cancel_and_acceptance_expiry_fence_all_tls_payload_paths() {
    let f = fixture();
    let (a, b) = pending();
    assert!(matches!(
        a.accept(b.statement(), f.now + 120_000),
        Err(Error::StaleSignature)
    ));
    let valid = Arc::new(AtomicBool::new(true));
    let check = valid.clone();
    let cancel = Cancellation::default();
    let a = PendingTls::client(
        &f.identities[0],
        f.contexts[0].clone(),
        Arc::new(move || check.load(Ordering::SeqCst)),
        cancel.clone(),
        f.now,
    )
    .unwrap();
    let b = PendingTls::server(
        &f.identities[1],
        f.contexts[1].clone(),
        Arc::new(|| true),
        Cancellation::default(),
        a.statement(),
        f.now,
    )
    .unwrap();
    let offer = a.statement().clone();
    let mut client = a.accept(b.statement(), f.now).unwrap();
    let mut server = b.accept(&offer, f.now).unwrap();
    handshake(&mut client, &mut server, 4096).unwrap();
    valid.store(false, Ordering::SeqCst);
    assert!(matches!(
        client.drain_tls(&mut [0; 10]),
        Err(Error::Unauthorized)
    ));
    valid.store(true, Ordering::SeqCst);
    assert!(matches!(
        client.write_plaintext(b"cannot revive"),
        Err(Error::Unauthorized)
    ));
    let (mut client, _) = pair();
    client.cancel.cancel();
    assert!(matches!(client.receive(&[0]), Err(Error::Cancelled)));
    assert!(matches!(
        client.drain_tls(&mut [0; 10]),
        Err(Error::Cancelled)
    ));
    // No timer or wall-clock check is applied after the declaration is accepted:
    // a valid large transfer does not expire merely because 120 s have elapsed.
}
