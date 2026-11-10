//! DNS resolution with pinned Roblox infrastructure IPs.
//!
//! Some ISPs (RKN-style censorship in particular) block or poison DNS
//! answers for `*.roblox.com` / `*.rbxcdn.com` zones, so the client can fail
//! to reach the very servers that are actually reachable. To be independent
//! of the system resolver, the hosts below resolve to hand-pinned IP
//! addresses (verified by a full TLS + SNI handshake and a real request at
//! release time).
//!
//! How it stays correct:
//!  * the URL keeps the real hostname, so rustls still sends the right SNI
//!    and validates the certificate against it, and the `Host` header is
//!    unchanged — only the connect step is redirected;
//!  * API hosts (`*.roblox.com`) use the pinned IPs exclusively: no DNS
//!    round-trip happens for them at all, so a dead/poisoned resolver can
//!    never stall login, presence or thumbnails;
//!  * CDN hosts (`*.rbxcdn.com`) are behind CDN providers that rotate edge
//!    IPs more often — the pinned IPs are tried first and the system DNS
//!    result is appended as fallback candidates (ureq connects to the first
//!    address that accepts a connection);
//!  * unknown hosts fall through to the system resolver as before.
//!
//! If Roblox ever moves the API edge IPs, refresh the table below (re-run
//! the verification `curl --resolve <host>:443:<ip> https://<host>/...`).

use std::io;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};

/// `host -> pinned IPs` (tried in the listed order, before anything else).
/// Every entry was verified to serve the vhost with a valid certificate.
const PINNED: &[(&str, &[&str])] = &[
    // Roblox's own API edge (128.116.0.0/16, stable anycast).
    ("auth.roblox.com", &["128.116.50.3"]),
    ("users.roblox.com", &["128.116.50.3"]),
    ("friends.roblox.com", &["128.116.50.3"]),
    ("presence.roblox.com", &["128.116.50.3"]),
    ("apis.roblox.com", &["128.116.50.3"]),
    ("thumbnails.roblox.com", &["128.116.97.3"]),
    ("games.roblox.com", &["128.116.97.3"]),
    // Thumbnail/image CDN (the `imageUrl` hosts returned by the API).
    ("tr.rbxcdn.com", &["23.32.91.208", "23.32.91.206"]),
    ("t0.rbxcdn.com", &["13.226.69.4", "13.226.69.58"]),
    ("t1.rbxcdn.com", &["54.230.70.103", "54.230.70.37"]),
    ("t2.rbxcdn.com", &["13.33.183.59", "13.33.183.12"]),
    ("t3.rbxcdn.com", &["18.65.14.67", "18.65.14.108"]),
    ("t4.rbxcdn.com", &["18.65.14.100", "18.65.14.102"]),
    ("t5.rbxcdn.com", &["13.33.183.80", "13.33.183.16"]),
    ("t6.rbxcdn.com", &["18.65.14.125", "18.65.14.24"]),
    ("t7.rbxcdn.com", &["54.230.70.5", "54.230.70.114"]),
];

/// Resolver for the ureq agents: pinned IPs first (see [`PINNED`]), system
/// DNS as a fallback for CDN hosts and the only source for unknown hosts.
#[derive(Debug, Clone, Copy, Default)]
pub struct PinnedResolver;

impl ureq::Resolver for PinnedResolver {
    fn resolve(&self, netloc: &str) -> io::Result<Vec<SocketAddr>> {
        let (host, port) = split_netloc(netloc);
        if let Some((_, ips)) = PINNED.iter().find(|(h, _)| host.eq_ignore_ascii_case(h)) {
            let mut out: Vec<SocketAddr> = ips
                .iter()
                .filter_map(|s| s.parse::<IpAddr>().ok())
                .map(|ip| SocketAddr::new(ip, port))
                .collect();
            // CDN edge IPs rotate more often than Roblox's own edge, so let
            // the system resolver append fallback candidates for CDN hosts
            // (API hosts never touch DNS at all).
            if !host.ends_with(".roblox.com") {
                if let Ok(extra) = netloc.to_socket_addrs() {
                    for a in extra {
                        if !out.contains(&a) {
                            out.push(a);
                        }
                    }
                }
            }
            if !out.is_empty() {
                return Ok(out);
            }
            // Malformed table entry: fall through to the system resolver.
        }
        netloc.to_socket_addrs().map(|iter| iter.collect())
    }
}

/// Splits `"host:port"` (IPv4/host names only — the client only talks to
/// explicit HTTPS URL hosts). Defaults the port to 443.
fn split_netloc(netloc: &str) -> (&str, u16) {
    match netloc.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().unwrap_or(443)),
        None => (netloc, 443),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Needed to call `resolve` directly in the tests below.
    use ureq::Resolver;

    #[test]
    fn api_hosts_resolve_to_pinned_ips_without_dns() {
        for (host, ips) in PINNED {
            if !host.ends_with(".roblox.com") {
                continue;
            }
            let addrs = PinnedResolver
                .resolve(&format!("{}:443", host))
                .expect("pinned API host must resolve");
            assert_eq!(addrs.len(), ips.len(), "{}: pinned only", host);
            for (a, ip) in addrs.iter().zip(ips.iter()) {
                assert_eq!(a.ip().to_string(), *ip);
                assert_eq!(a.port(), 443);
            }
        }
    }

    #[test]
    fn cdn_host_tries_pinned_ip_first() {
        let addrs = PinnedResolver
            .resolve("tr.rbxcdn.com:443")
            .expect("pinned CDN host must resolve");
        assert!(!addrs.is_empty());
        // Pinned candidate comes first even when DNS adds fallbacks.
        assert_eq!(addrs[0].ip().to_string(), "23.32.91.208");
    }

    #[test]
    fn netloc_splitting() {
        assert_eq!(split_netloc("users.roblox.com:443"), ("users.roblox.com", 443));
        assert_eq!(split_netloc("users.roblox.com"), ("users.roblox.com", 443));
        assert_eq!(split_netloc("tr.rbxcdn.com:80"), ("tr.rbxcdn.com", 80));
    }
}
