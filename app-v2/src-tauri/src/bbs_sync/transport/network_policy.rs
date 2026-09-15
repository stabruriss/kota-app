//! Closed network configuration. Only signed peer candidates and the two
//! declared STUN services can receive datagrams. No TCP, mDNS or relay fallback.
use super::{Error, Result};
use ring::hmac;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    future::Future,
    io::{self, IoSliceMut},
    net::SocketAddr,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll},
    time::Duration,
};
use webrtc::runtime::{
    AsyncInterval, AsyncTcpListener, AsyncTcpStream, AsyncUdpSocket, JoinHandle, RecvMeta, Runtime,
    TokioRuntime, Transmit,
};

pub(crate) const STUN_HOSTS: [&str; 2] = ["stun.cloudflare.com:3478", "stun.l.google.com:19302"];
pub(crate) const STUN_URLS: [&str; 2] = [
    "stun:stun.cloudflare.com:3478",
    "stun:stun.l.google.com:19302",
];

#[derive(Default)]
struct Addresses {
    peers: BTreeSet<SocketAddr>,
    peer_types: BTreeMap<SocketAddr, Option<String>>,
    stun: BTreeSet<SocketAddr>,
    reflexive: BTreeSet<SocketAddr>,
    local_password: Option<String>,
    last_direct_peer: Option<SocketAddr>,
}
pub(crate) struct NetworkPolicy {
    addresses: Mutex<Addresses>,
    loopback: bool,
    pub(crate) sent_bytes: AtomicU64,
    pub(crate) sent_packets: AtomicU64,
    pub(crate) denied: AtomicU64,
    #[cfg(test)]
    marker: Mutex<Vec<u8>>,
}
impl fmt::Debug for NetworkPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BbsNetworkPolicy")
    }
}
impl NetworkPolicy {
    pub(crate) fn new(loopback: bool) -> Arc<Self> {
        Arc::new(Self {
            addresses: Mutex::new(Addresses::default()),
            loopback,
            sent_bytes: AtomicU64::new(0),
            sent_packets: AtomicU64::new(0),
            denied: AtomicU64::new(0),
            #[cfg(test)]
            marker: Mutex::new(Vec::new()),
        })
    }
    pub(crate) fn peers(
        &self,
        peers: BTreeSet<SocketAddr>,
        kinds: BTreeMap<SocketAddr, Option<String>>,
    ) -> Result<()> {
        if peers.is_empty() || peers.len() > 32 || peers.iter().any(|a| !self.valid_peer(*a)) {
            return Err(Error::InvalidSignal);
        }
        if kinds.keys().copied().collect::<BTreeSet<_>>() != peers {
            return Err(Error::InvalidSignal);
        }
        let mut state = self.addresses.lock().map_err(|_| Error::Closed)?;
        state.peers = peers;
        state.peer_types = kinds;
        Ok(())
    }
    pub(crate) fn local_description(&self, sdp: &str) -> Result<()> {
        let passwords: BTreeSet<_> = sdp
            .lines()
            .filter_map(|l| l.trim_end_matches('\r').strip_prefix("a=ice-pwd:"))
            .collect();
        if passwords.len() != 1 {
            return Err(Error::InvalidSignal);
        }
        let password = passwords.first().ok_or(Error::InvalidSignal)?;
        if !(22..=256).contains(&password.len()) {
            return Err(Error::InvalidSignal);
        }
        self.addresses
            .lock()
            .map_err(|_| Error::Closed)?
            .local_password = Some((*password).into());
        Ok(())
    }
    fn valid_peer(&self, a: SocketAddr) -> bool {
        a.port() != 0
            && !a.ip().is_unspecified()
            && !a.ip().is_multicast()
            && (self.loopback || !a.ip().is_loopback())
    }
    pub(super) fn observed_peer(&self) -> Option<(SocketAddr, String, &'static str)> {
        let a = self.addresses.lock().ok()?;
        let peer = a.last_direct_peer?;
        if a.reflexive.contains(&peer) {
            return Some((peer, "prflx".into(), "authenticated-ice"));
        }
        let kind = a.peer_types.get(&peer)?.clone()?;
        Some((peer, kind, "socket+signed-sdp"))
    }
    fn allowed(&self, destination: SocketAddr, bytes: &[u8]) -> bool {
        #[cfg(test)]
        {
            let marker = self.marker.lock().unwrap();
            if !marker.is_empty() && bytes.windows(marker.len()).any(|w| w == marker.as_slice()) {
                return false;
            }
        }
        let Ok(mut a) = self.addresses.lock() else {
            return false;
        };
        if a.stun.contains(&destination) {
            return is_stun(bytes);
        }
        if a.peers.contains(&destination) || a.reflexive.contains(&destination) {
            return true;
        }
        // rtc-ice 0.20.5 Agent::handle_inbound checks username + MESSAGE-INTEGRITY
        // before creating a prflx candidate, then send_binding_success signs its
        // response with local_pwd. Only that native authenticated-success output
        // can admit an unsignaled address. Receiving a packet alone never does.
        if self.valid_peer(destination)
            && a.reflexive.len() < 32
            && a.local_password
                .as_ref()
                .is_some_and(|pwd| binding_success(bytes, pwd))
        {
            a.reflexive.insert(destination);
            return true;
        }
        false
    }
    #[cfg(test)]
    pub(crate) fn forbid_plaintext(&self, marker: &[u8]) {
        *self.marker.lock().unwrap() = marker.to_vec();
    }
}
fn is_stun(b: &[u8]) -> bool {
    b.len() >= 20
        && b[0] & 0xc0 == 0
        && b[4..8] == [0x21, 0x12, 0xa4, 0x42]
        && usize::from(u16::from_be_bytes([b[2], b[3]])) + 20 == b.len()
}
fn binding_success(b: &[u8], password: &str) -> bool {
    if !is_stun(b) || b[..2] != [1, 1] {
        return false;
    }
    let mut offset = 20;
    while offset + 4 <= b.len() {
        let kind = u16::from_be_bytes([b[offset], b[offset + 1]]);
        let len = usize::from(u16::from_be_bytes([b[offset + 2], b[offset + 3]]));
        if offset + 4 + len > b.len() {
            return false;
        }
        if kind == 8 {
            if len != 20 {
                return false;
            }
            let mut prefix = b[..offset].to_vec();
            let length = ((offset + 24) - 20) as u16;
            prefix[2..4].copy_from_slice(&length.to_be_bytes());
            return hmac::verify(
                &hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, password.as_bytes()),
                &prefix,
                &b[offset + 4..offset + 24],
            )
            .is_ok();
        }
        offset += 4 + (len + 3) / 4 * 4;
    }
    false
}
fn denied() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "BBS network policy")
}
#[derive(Debug)]
struct GuardedSocket {
    socket: Arc<dyn AsyncUdpSocket>,
    policy: Arc<NetworkPolicy>,
}
impl AsyncUdpSocket for GuardedSocket {
    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }
    fn poll_send(&self, cx: &mut Context<'_>, tx: &Transmit<'_>) -> Poll<io::Result<usize>> {
        if !self.policy.allowed(tx.destination, tx.contents) {
            self.policy.denied.fetch_add(1, Ordering::Relaxed);
            return Poll::Ready(Err(denied()));
        }
        let result = self.socket.poll_send(cx, tx);
        if let Poll::Ready(Ok(n)) = &result {
            if *n > 0 && !is_stun(tx.contents) {
                if let Ok(mut state) = self.policy.addresses.lock() {
                    state.last_direct_peer = Some(tx.destination);
                }
            }
            self.policy
                .sent_bytes
                .fetch_add(*n as u64, Ordering::Relaxed);
            if *n > 0 {
                self.policy.sent_packets.fetch_add(
                    n.div_ceil(tx.segment_size.unwrap_or(*n).max(1)) as u64,
                    Ordering::Relaxed,
                );
            }
        }
        result
    }
    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        self.socket.poll_recv(cx, bufs, meta)
    }
    fn max_gso_segments(&self) -> usize {
        self.socket.max_gso_segments()
    }
    fn max_gro_segments(&self) -> usize {
        self.socket.max_gro_segments()
    }
}
// Arc keeps the guard shared with the socket; no process-wide runtime change.
#[derive(Debug)]
pub(crate) struct PolicyRuntime(pub(crate) Arc<NetworkPolicy>);
impl Runtime for PolicyRuntime {
    fn spawn(&self, f: Pin<Box<dyn Future<Output = ()> + Send>>) -> Box<dyn JoinHandle> {
        TokioRuntime.spawn(f)
    }
    fn wrap_udp_socket(&self, socket: std::net::UdpSocket) -> io::Result<Arc<dyn AsyncUdpSocket>> {
        Ok(Arc::new(GuardedSocket {
            socket: TokioRuntime.wrap_udp_socket(socket)?,
            policy: self.0.clone(),
        }))
    }
    fn wrap_tcp_listener(&self, _: std::net::TcpListener) -> io::Result<Arc<dyn AsyncTcpListener>> {
        Err(denied())
    }
    fn connect_tcp<'a>(
        &'a self,
        _: SocketAddr,
    ) -> Pin<Box<dyn Future<Output = io::Result<Arc<dyn AsyncTcpStream>>> + Send + 'a>> {
        Box::pin(async { Err(denied()) })
    }
    fn resolve_host<'a>(
        &'a self,
        host: &'a str,
    ) -> Pin<Box<dyn Future<Output = io::Result<Vec<SocketAddr>>> + Send + 'a>> {
        Box::pin(async move {
            if self.0.loopback || !STUN_HOSTS.contains(&host) {
                return Err(denied());
            }
            let addresses = TokioRuntime.resolve_host(host).await?;
            if addresses.is_empty()
                || addresses.len() > 16
                || addresses.iter().any(|a| !self.0.valid_peer(*a))
            {
                return Err(denied());
            }
            let mut state = self.0.addresses.lock().map_err(|_| denied())?;
            let union: BTreeSet<_> = state
                .stun
                .iter()
                .copied()
                .chain(addresses.iter().copied())
                .collect();
            if union.len() > 32 {
                return Err(denied());
            }
            state.stun = union;
            Ok(addresses)
        })
    }
    fn sleep(&self, d: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        TokioRuntime.sleep(d)
    }
    fn interval(&self, d: Duration) -> Box<dyn AsyncInterval> {
        TokioRuntime.interval(d)
    }
    fn block_on(&self, f: Pin<Box<dyn Future<Output = ()> + '_>>) {
        TokioRuntime.block_on(f)
    }
    fn yield_now(&self) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        TokioRuntime.yield_now()
    }
    fn name(&self) -> &'static str {
        "bbs-direct-udp"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_signed_candidates_fixed_stun_and_authenticated_native_prflx_are_admitted() {
        let p = NetworkPolicy::new(true);
        let peer = "127.0.0.1:12345".parse().unwrap();
        let unknown = "127.0.0.1:23456".parse().unwrap();
        p.peers(
            BTreeSet::from([peer]),
            BTreeMap::from([(peer, Some("host".into()))]),
        )
        .unwrap();
        assert!(p.allowed(peer, b"encrypted dtls"));
        assert!(!p.allowed(unknown, b"encrypted dtls"));
        let pwd = "abcdefghijklmnopqrstuv";
        p.local_description(&format!("a=ice-pwd:{pwd}\r\n"))
            .unwrap();
        let mut packet = vec![1, 1, 0, 24, 0x21, 0x12, 0xa4, 0x42];
        packet.extend_from_slice(&[0; 12]);
        let signature = hmac::sign(
            &hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, pwd.as_bytes()),
            &packet,
        );
        packet.extend_from_slice(&[0, 8, 0, 20]);
        packet.extend_from_slice(signature.as_ref());
        let mut wrong = packet.clone();
        wrong[30] ^= 1;
        assert!(!p.allowed(unknown, &wrong));
        assert!(p.allowed(unknown, &packet));
        assert!(p.allowed(unknown, b"encrypted dtls"));
        p.forbid_plaintext(b"SECRET-PAYLOAD");
        assert!(!p.allowed(peer, b"prefix SECRET-PAYLOAD suffix"));
    }
}
