//! Only the account-configured public Worker origin is resolved. Peer metadata
//! never supplies a host, and DNS answers are checked before ureq can connect.
use super::{wire::canonical_origin, Error, Result};
use std::{
    collections::BTreeSet,
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs},
};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Destination {
    origin: String,
    netloc: String,
    port: u16,
}
impl Destination {
    pub(super) fn new(origin: &str) -> Result<Self> {
        canonical_origin(origin)?;
        let url = url::Url::parse(origin).map_err(|_| Error::InvalidSignal)?;
        match url.host().ok_or(Error::InvalidSignal)? {
            url::Host::Domain(host) => {
                // A trailing dot is another identity; do not hand local names
                // or single-label search domains to the system resolver.
                if !host.contains('.')
                    || host.ends_with('.')
                    || host.ends_with(".local")
                    || host.ends_with(".localhost")
                    || host.ends_with(".internal")
                {
                    return Err(Error::InvalidSignal);
                }
            }
            url::Host::Ipv4(ip) if public_ip(ip.into()) => {}
            url::Host::Ipv6(ip) if public_ip(ip.into()) => {}
            _ => return Err(Error::InvalidSignal),
        }
        let port = url.port_or_known_default().ok_or(Error::InvalidSignal)?;
        if port == 0 {
            return Err(Error::InvalidSignal);
        }
        Ok(Self {
            origin: origin.into(),
            netloc: format!("{}:{port}", url.host_str().ok_or(Error::InvalidSignal)?),
            port,
        })
    }
    pub(super) fn validate_url(&self, url: &str) -> Result<()> {
        let parsed = url::Url::parse(url).map_err(|_| Error::InvalidSignal)?;
        if parsed.origin().ascii_serialization() != self.origin
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.fragment().is_some()
        {
            return Err(Error::Unauthorized);
        }
        Ok(())
    }
    fn resolved(
        &self,
        netloc: &str,
        addresses: Vec<SocketAddr>,
        local: &[Subnet],
    ) -> io::Result<Vec<SocketAddr>> {
        if netloc != self.netloc
            || addresses.is_empty()
            || addresses.len() > 16
            || addresses.iter().any(|a| {
                a.port() != self.port
                    || !public_ip(a.ip())
                    || local.iter().any(|s| s.contains(a.ip()))
            })
        {
            return Err(denied());
        }
        let mut seen = BTreeSet::new();
        Ok(addresses.into_iter().filter(|a| seen.insert(*a)).collect())
    }
}
impl ureq::Resolver for Destination {
    fn resolve(&self, netloc: &str) -> io::Result<Vec<SocketAddr>> {
        if netloc != self.netloc {
            return Err(denied());
        }
        // getaddrinfo may remain blocked after local cancellation. The fixed
        // worker retains ownership; callers never spawn a replacement thread.
        let addresses = netloc.to_socket_addrs()?.take(17).collect();
        self.resolved(netloc, addresses, &local_networks()?)
    }
}

fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "BBS relay requires a public Worker endpoint",
    )
}
fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            a != 0
                && a < 224
                && !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_documentation()
                && !(a == 100 && (64..=127).contains(&b))
                && !(a == 198 && (b == 18 || b == 19))
                && !(a == 192 && b == 0 && c == 0)
                && !(a == 192 && b == 88 && c == 99)
        }
        IpAddr::V6(ip) => {
            let s = ip.segments();
            // Global unicast only; IPv4-mapped/translated, local, multicast and
            // legacy tunnel addresses cannot hide a private/on-link IPv4 peer.
            (s[0] & 0xe000) == 0x2000
                && s[0] != 0x2002
                && !(s[0] == 0x2001 && s[1] < 0x0200)
                && !(s[0] == 0x2001 && s[1] == 0x0db8)
                && !(s[0] == 0x3fff && s[1] < 0x1000)
        }
    }
}

struct Subnet {
    address: IpAddr,
    mask: IpAddr,
}
impl Subnet {
    fn contains(&self, ip: IpAddr) -> bool {
        match (self.address, self.mask, ip) {
            (IpAddr::V4(a), IpAddr::V4(m), IpAddr::V4(ip)) => {
                u32::from(a) & u32::from(m) == u32::from(ip) & u32::from(m)
            }
            (IpAddr::V6(a), IpAddr::V6(m), IpAddr::V6(ip)) => {
                u128::from(a) & u128::from(m) == u128::from(ip) & u128::from(m)
            }
            _ => false,
        }
    }
}

#[cfg(unix)]
fn local_networks() -> io::Result<Vec<Subnet>> {
    let mut head = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Err(denied());
    }
    struct List(*mut libc::ifaddrs);
    impl Drop for List {
        fn drop(&mut self) {
            unsafe { libc::freeifaddrs(self.0) };
        }
    }
    let _list = List(head);
    let mut rows = Vec::new();
    let mut next = head;
    let mut seen = 0;
    while !next.is_null() {
        // getifaddrs supplies native sockaddr_in/in6 for the tagged family;
        // the RAII list outlives every read. No address is logged or connected.
        let row = unsafe { &*next };
        next = row.ifa_next;
        seen += 1;
        if seen > 1024 {
            return Err(denied());
        }
        if row.ifa_addr.is_null() || row.ifa_flags & libc::IFF_UP as u32 == 0 {
            continue;
        }
        let family = unsafe { (*row.ifa_addr).sa_family as i32 };
        if family != libc::AF_INET && family != libc::AF_INET6 {
            continue;
        }
        if row.ifa_netmask.is_null() {
            return Err(denied());
        }
        let subnet = unsafe {
            if family == libc::AF_INET {
                let addr = (*(row.ifa_addr as *const libc::sockaddr_in))
                    .sin_addr
                    .s_addr;
                let mask = (*(row.ifa_netmask as *const libc::sockaddr_in))
                    .sin_addr
                    .s_addr;
                Subnet {
                    address: Ipv4Addr::from(addr.to_ne_bytes()).into(),
                    mask: Ipv4Addr::from(mask.to_ne_bytes()).into(),
                }
            } else {
                let addr = (*(row.ifa_addr as *const libc::sockaddr_in6))
                    .sin6_addr
                    .s6_addr;
                let mask = (*(row.ifa_netmask as *const libc::sockaddr_in6))
                    .sin6_addr
                    .s6_addr;
                Subnet {
                    address: Ipv6Addr::from(addr).into(),
                    mask: Ipv6Addr::from(mask).into(),
                }
            }
        };
        rows.push(subnet);
    }
    if rows.is_empty() {
        return Err(denied());
    }
    Ok(rows)
}
#[cfg(not(unix))]
fn local_networks() -> io::Result<Vec<Subnet>> {
    Err(denied())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_origin_rejects_local_aliases_and_unsafe_dns_answers_before_connect() {
        for origin in [
            "http://worker.example",
            "https://worker.local",
            "https://worker.LOCAL",
            "https://worker.local.",
            "https://localhost",
            "https://worker",
            "https://127.0.0.1",
            "https://[::ffff:127.0.0.1]",
            "https://10.0.0.1",
            "https://worker.example:0",
            "https://worker.example/path",
        ] {
            assert!(Destination::new(origin).is_err(), "{origin}");
        }
        let d = Destination::new("https://worker.example").unwrap();
        let nets = vec![Subnet {
            address: "104.16.8.2".parse().unwrap(),
            mask: "255.255.255.0".parse().unwrap(),
        }];
        for ip in [
            "0.1.2.3",
            "127.0.0.1",
            "192.168.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "198.19.0.1",
            "192.0.2.1",
            "224.0.0.1",
            "255.255.255.255",
            "104.16.8.3",
            "::",
            "::1",
            "::ffff:10.1.2.3",
            "64:ff9b::a00:1",
            "fe80::1",
            "fc00::1",
            "ff02::1",
            "2001:db8::1",
            "3fff:123::1",
            "2002:a01:203::",
        ] {
            let addr = SocketAddr::new(ip.parse().unwrap(), 443);
            assert!(
                d.resolved("worker.example:443", vec![addr], &nets).is_err(),
                "{ip}"
            );
        }
        let good = "104.16.9.3:443".parse().unwrap();
        assert_eq!(
            d.resolved("worker.example:443", vec![good, good], &nets)
                .unwrap(),
            vec![good]
        );
        assert!(d.resolved("peer.example:443", vec![good], &nets).is_err());
        assert!(d
            .resolved(
                "worker.example:443",
                vec![good, "10.0.0.1:443".parse().unwrap()],
                &nets
            )
            .is_err());
        assert!(d
            .resolved("worker.example:443", vec![good; 17], &nets)
            .is_err());
        assert!(d
            .resolved(
                "worker.example:443",
                vec!["104.16.9.3:444".parse().unwrap()],
                &nets
            )
            .is_err());
        assert!(d
            .validate_url("https://worker.example/bbs/relay/poll?group=safe")
            .is_ok());
        assert!(d
            .validate_url("https://other.example/bbs/relay/poll")
            .is_err());
        assert!(d
            .validate_url("https://secret@worker.example/bbs/relay/poll")
            .is_err());
    }
    #[test]
    fn public_v6_on_link_prefixes_and_actual_interface_enumeration_are_checked() {
        let d = Destination::new("https://worker.example").unwrap();
        let subnet = Subnet {
            address: "2606:4700:1234:5678::1".parse().unwrap(),
            mask: "ffff:ffff:ffff:ffff::".parse().unwrap(),
        };
        assert!(d
            .resolved(
                "worker.example:443",
                vec!["[2606:4700:1234:5678::2]:443".parse().unwrap()],
                &[subnet]
            )
            .is_err());
        assert!(d
            .resolved(
                "worker.example:443",
                vec!["[2606:4700::1111]:443".parse().unwrap()],
                &[]
            )
            .is_ok());
        assert!(!local_networks().unwrap().is_empty());
        assert_eq!(
            Destination::new("https://[2606:4700::1111]")
                .unwrap()
                .netloc,
            "[2606:4700::1111]:443"
        );
    }
}
