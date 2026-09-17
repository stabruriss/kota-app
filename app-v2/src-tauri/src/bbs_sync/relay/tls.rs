//! Socket-free inner TLS. The relay owns HTTP/ciphertext credit; this module
//! only accepts authenticated declarations and processes bounded byte slices.
use super::wire::{Declarations, Role};
use super::{Error, Result, SessionContext, SignedStatement, MAX_BATCH};
use crate::bbs_sync::{
    raw_sha256,
    transport::{Cancellation, MembershipCheck, MAX_FRAME},
    DeviceIdentity,
};
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{ring::default_provider, verify_tls13_signature},
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime},
    server::danger::{ClientCertVerified, ClientCertVerifier},
    CertificateError, ClientConfig, ClientConnection, Connection, DigitallySignedStruct,
    DistinguishedName, ServerConfig, ServerConnection, SignatureScheme,
};
use std::{
    io::{self, Read, Write},
    sync::Arc,
};

pub(super) const TLS_WRITE_LIMIT: usize = 64 * 1024;
const TLS_READ_CHUNK: usize = 16 * 1024;
const MAX_HANDSHAKE_INPUT: usize = 64 * 1024;
const MAX_CERTIFICATE: usize = 2 * 1024;

struct Credentials {
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
}
impl Credentials {
    fn generate() -> Result<Self> {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
            .map_err(|_| Error::Runtime)?;
        let mut params =
            rcgen::CertificateParams::new(Vec::<String>::new()).map_err(|_| Error::Runtime)?;
        params.distinguished_name = rcgen::DistinguishedName::new();
        let cert = params.self_signed(&key).map_err(|_| Error::Runtime)?;
        if cert.der().len() > MAX_CERTIFICATE {
            return Err(Error::Runtime);
        }
        Ok(Self {
            cert: cert.der().clone(),
            key: PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
        })
    }
    fn fingerprint(&self) -> String {
        raw_sha256(self.cert.as_ref())
    }
}

/// Consumed on acceptance: the temporary private key cannot seed a second
/// stream. The coordinator separately fences wake/boot/nonce replay before this.
pub(crate) struct PendingTls {
    context: SessionContext,
    credentials: Credentials,
    local: SignedStatement,
    authorized: MembershipCheck,
    cancel: Cancellation,
}
impl PendingTls {
    pub(crate) fn client(
        identity: &DeviceIdentity,
        context: SessionContext,
        authorized: MembershipCheck,
        cancel: Cancellation,
        now: u64,
    ) -> Result<Self> {
        if context.role() != Role::Client {
            return Err(Error::Unauthorized);
        }
        Self::create(identity, context, authorized, cancel, None, now)
    }
    pub(crate) fn server(
        identity: &DeviceIdentity,
        context: SessionContext,
        authorized: MembershipCheck,
        cancel: Cancellation,
        offer: &SignedStatement,
        now: u64,
    ) -> Result<Self> {
        if context.role() != Role::Server || !authorized() {
            return Err(Error::Unauthorized);
        }
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        context.verify_remote(offer, None, now)?;
        Self::create(
            identity,
            context,
            authorized,
            cancel,
            Some(offer.digest()?),
            now,
        )
    }
    fn create(
        identity: &DeviceIdentity,
        context: SessionContext,
        authorized: MembershipCheck,
        cancel: Cancellation,
        reply_to: Option<String>,
        now: u64,
    ) -> Result<Self> {
        if !authorized() {
            return Err(Error::Unauthorized);
        }
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        context.validate()?;
        let credentials = Credentials::generate()?;
        let local =
            SignedStatement::create(identity, &context, credentials.fingerprint(), reply_to, now)?;
        Ok(Self {
            context,
            credentials,
            local,
            authorized,
            cancel,
        })
    }
    pub(crate) fn statement(&self) -> &SignedStatement {
        &self.local
    }
    pub(crate) fn accept(self, remote: &SignedStatement, now: u64) -> Result<TlsStream> {
        if !self.authorized.as_ref()() {
            return Err(Error::Unauthorized);
        }
        if self.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        self.local.statement.validate_time(now)?;
        let is_client = self.context.role() == Role::Client;
        let reply = if is_client {
            Some(self.local.digest()?)
        } else {
            None
        };
        self.context.verify_remote(remote, reply.as_deref(), now)?;
        if !is_client && self.local.statement.reply_to.as_deref() != Some(remote.digest()?.as_str())
        {
            return Err(Error::Unauthorized);
        }
        let pin = remote.statement.certificate_sha256.clone();
        let connection = if is_client {
            Connection::Client(
                ClientConnection::new(
                    Arc::new(client_config(&self.credentials, pin)?),
                    ServerName::try_from("bbs.invalid")
                        .map_err(|_| Error::Runtime)?
                        .to_owned(),
                )
                .map_err(|_| Error::Runtime)?,
            )
        } else {
            Connection::Server(
                ServerConnection::new(Arc::new(server_config(&self.credentials, pin)?))
                    .map_err(|_| Error::Runtime)?,
            )
        };
        let declarations = if is_client {
            Declarations {
                client: self.local,
                server: remote.clone(),
            }
        } else {
            Declarations {
                client: remote.clone(),
                server: self.local,
            }
        };
        TlsStream::new(
            connection,
            declarations,
            self.context,
            self.authorized,
            self.cancel,
        )
    }
}

fn client_config(credentials: &Credentials, fingerprint: String) -> Result<ClientConfig> {
    let mut config = ClientConfig::builder_with_provider(Arc::new(default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|_| Error::Runtime)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedCertificate(fingerprint)))
        .with_client_auth_cert(vec![credentials.cert.clone()], credentials.key.clone_key())
        .map_err(|_| Error::Runtime)?;
    config.resumption = rustls::client::Resumption::disabled();
    config.enable_early_data = false;
    config.enable_sni = false;
    config.alpn_protocols.clear();
    config.max_fragment_size = Some(MAX_FRAME);
    Ok(config)
}
fn server_config(credentials: &Credentials, fingerprint: String) -> Result<ServerConfig> {
    let mut config = ServerConfig::builder_with_provider(Arc::new(default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|_| Error::Runtime)?
        .with_client_cert_verifier(Arc::new(PinnedCertificate(fingerprint)))
        .with_single_cert(vec![credentials.cert.clone()], credentials.key.clone_key())
        .map_err(|_| Error::Runtime)?;
    config.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});
    config.send_tls13_tickets = 0;
    config.max_early_data_size = 0;
    config.alpn_protocols.clear();
    config.max_fragment_size = Some(MAX_FRAME);
    Ok(config)
}

#[derive(Debug)]
struct PinnedCertificate(String);
impl PinnedCertificate {
    fn check(
        &self,
        cert: &CertificateDer<'_>,
        chain: &[CertificateDer<'_>],
    ) -> std::result::Result<(), rustls::Error> {
        if cert.len() > MAX_CERTIFICATE || !chain.is_empty() || raw_sha256(cert.as_ref()) != self.0
        {
            return Err(rustls::Error::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            ));
        }
        rustls::server::ParsedCertificate::try_from(cert)?;
        Ok(())
    }
    fn signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        if dss.scheme != SignatureScheme::ECDSA_NISTP256_SHA256 {
            return Err(rustls::Error::InvalidCertificate(
                CertificateError::BadSignature,
            ));
        }
        // Pinning proves which certificate was authorized. This proves possession
        // of its private key and authenticates the actual handshake transcript.
        verify_tls13_signature(
            message,
            cert,
            dss,
            &default_provider().signature_verification_algorithms,
        )
    }
}
impl ServerCertVerifier for PinnedCertificate {
    fn verify_server_cert(
        &self,
        end: &CertificateDer<'_>,
        chain: &[CertificateDer<'_>],
        _name: &ServerName<'_>,
        ocsp: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        self.check(end, chain)?;
        if !ocsp.is_empty() {
            return Err(rustls::Error::General("unexpected OCSP".into()));
        }
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2 disabled".into()))
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        s: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.signature(m, c, s)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ECDSA_NISTP256_SHA256]
    }
    fn requires_raw_public_keys(&self) -> bool {
        false
    }
}
impl ClientCertVerifier for PinnedCertificate {
    fn offer_client_auth(&self) -> bool {
        true
    }
    fn client_auth_mandatory(&self) -> bool {
        true
    }
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn verify_client_cert(
        &self,
        end: &CertificateDer<'_>,
        chain: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> std::result::Result<ClientCertVerified, rustls::Error> {
        self.check(end, chain)?;
        Ok(ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2 disabled".into()))
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        s: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.signature(m, c, s)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ECDSA_NISTP256_SHA256]
    }
    fn requires_raw_public_keys(&self) -> bool {
        false
    }
}

pub(crate) struct TlsStream {
    connection: Connection,
    context: SessionContext,
    pub(crate) declarations: Declarations,
    pub(crate) session_id: String,
    authorized: MembershipCheck,
    cancel: Cancellation,
    terminal: Option<Error>,
    plaintext_pending: usize,
    handshake_input: usize,
}
impl TlsStream {
    fn new(
        mut connection: Connection,
        declarations: Declarations,
        context: SessionContext,
        authorized: MembershipCheck,
        cancel: Cancellation,
    ) -> Result<Self> {
        connection.set_buffer_limit(Some(TLS_WRITE_LIMIT));
        let session_id = declarations.session_id()?;
        Ok(Self {
            connection,
            context,
            declarations,
            session_id,
            authorized,
            cancel,
            terminal: None,
            plaintext_pending: 0,
            handshake_input: 0,
        })
    }
    pub(super) fn context(&self) -> &SessionContext {
        &self.context
    }
    pub(super) fn cancellation(&self) -> Cancellation {
        self.cancel.clone()
    }
    pub(super) fn membership(&self) -> MembershipCheck {
        self.authorized.clone()
    }
    fn check(&mut self) -> Result<()> {
        if let Some(error) = self.terminal {
            return Err(error);
        }
        let error = if self.cancel.is_cancelled() {
            Some(Error::Cancelled)
        } else if !(self.authorized)() {
            Some(Error::Unauthorized)
        } else {
            None
        };
        if let Some(error) = error {
            self.terminal = Some(error);
            return Err(error);
        }
        Ok(())
    }
    fn fail<T>(&mut self, error: Error) -> Result<T> {
        self.terminal = Some(error);
        Err(error)
    }
    pub(crate) fn handshake_complete(&mut self) -> Result<bool> {
        self.check()?;
        Ok(!self.connection.is_handshaking())
    }
    /// Never feeds more input while decrypted plaintext is waiting. A batch can
    /// span multiple calls; its owner retains the unconsumed suffix and credit.
    pub(crate) fn receive(&mut self, input: &[u8]) -> Result<usize> {
        self.check()?;
        if input.len() > MAX_BATCH {
            return self.fail(Error::Protocol);
        }
        if input.is_empty() {
            return Ok(0);
        } // An empty poll is not TLS EOF.
        if self.plaintext_pending != 0 {
            return Err(Error::Busy);
        }
        let mut chunk = &input[..input.len().min(TLS_READ_CHUNK)];
        let handshaking = self.connection.is_handshaking();
        if handshaking && self.handshake_input + chunk.len() > MAX_HANDSHAKE_INPUT {
            return self.fail(Error::Protocol);
        }
        let count = match self.connection.read_tls(&mut chunk) {
            Ok(n) if n > 0 => n,
            _ => return self.fail(Error::Closed),
        };
        if handshaking {
            self.handshake_input += count;
        }
        let state = match self.connection.process_new_packets() {
            Ok(state) => state,
            Err(_) => return self.fail(Error::Integrity),
        };
        if state.peer_has_closed() {
            return self.fail(Error::Closed);
        }
        self.plaintext_pending = state.plaintext_bytes_to_read();
        Ok(count)
    }
    pub(crate) fn read_plaintext(&mut self, output: &mut [u8]) -> Result<usize> {
        self.check()?;
        if self.connection.is_handshaking() {
            return Err(Error::Busy);
        }
        match self.connection.reader().read(output) {
            Ok(n) => {
                self.plaintext_pending = self.plaintext_pending.saturating_sub(n);
                Ok(n)
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(0),
            Err(_) => self.fail(Error::Closed),
        }
    }
    /// Returns partial writes when the TLS write budget is full. The caller
    /// keeps its frame permit until all plaintext has transferred ownership.
    pub(crate) fn write_plaintext(&mut self, input: &[u8]) -> Result<usize> {
        self.check()?;
        if self.connection.is_handshaking() {
            return Err(Error::Busy);
        }
        if input.len() > MAX_FRAME {
            return self.fail(Error::Protocol);
        }
        match self.connection.writer().write(input) {
            Ok(n) => Ok(n),
            Err(_) => self.fail(Error::Closed),
        }
    }
    /// A charged builder need not allocate a block for empty TLS output.
    pub(crate) fn wants_write(&mut self) -> Result<bool> {
        self.check()?;
        Ok(self.connection.wants_write())
    }
    /// Copies into an already-budgeted caller buffer; no batch allocation here.
    pub(crate) fn drain_tls(&mut self, mut output: &mut [u8]) -> Result<usize> {
        self.check()?;
        match self.connection.write_tls(&mut output) {
            Ok(n) => Ok(n),
            Err(_) => self.fail(Error::Closed),
        }
    }
}

#[cfg(test)]
mod tests;
