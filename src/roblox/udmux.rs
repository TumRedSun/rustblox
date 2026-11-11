//! Roblox's game servers (RCC) sit behind NAT: the join script hands out a
//! private `10.x.x.x:port` address which is unroutable from the open
//! internet. Real clients dial a public "UDMUX" proxy in 128.116.0.0/16 on
//! the SAME port and the proxy relays the datagrams to the RCC server that
//! owns that port. Which proxy fronts a game server is decided by the
//! datacenter encoded in the private address (the `10.a.#.#` prefix, e.g.
//! `10.9.#.#` = Frankfurt → `128.116.44.33`).
//!
//! Evidence: client log lines of the form
//! `UDMUX Address = 128.116.x.x, Port = P | RCC Server Address = 10.x.x.x,
//! Port = P` (equal ports on both sides) and the devforum thread
//! "List of roblox servers with DDoS protection (udmux enabled)" for the
//! prefix → proxy table. Datacenter proxies move over time, so
//! `RUSTBLOX_UDMUX` (e.g. `10.9=128.116.44.33,10.20=128.116.50.33`)
//! overrides or extends the built-in table without a rebuild.

use std::net::Ipv4Addr;

/// Known RCC private prefixes (second octet of `10.a.b.c`) → the public
/// UDMUX proxy fronting that datacenter. Entries without a devforum comment
/// were observed in real client logs (Sober issue tracker).
const PROXIES: &[(u8, &str)] = &[
    (8, "128.116.95.33"),      // Dallas
    (9, "128.116.44.33"),      // Frankfurt
    (12, "128.116.45.33"),     // Miami
    (18, "128.116.32.33"),     // observed: RCC 10.18.8.89 → 128.116.32.33
    (20, "128.116.50.33"),     // Singapore
    (60, "128.116.51.33"),     // Sydney
    (127, "128.116.48.33"),    // observed: RCC 10.127.8.79 → 128.116.48.33
    (176, "128.116.13.1"),     // Amsterdam
    (182, "128.116.97.33"),    // Singapore 2
    (186, "128.116.48.33"),    // observed: RCC 10.186.6.40 → 128.116.48.33
    (194, "128.116.115.33"),   // Seattle / Portland
    (196, "128.116.4.33"),     // Paris
    (208, "128.116.49.254"),   // Los Angeles
];

/// The RCC address handed out by the join script is always RFC1918-private;
/// game servers are only known to live in `10.a.b.c`.
pub fn is_private(ip: Ipv4Addr) -> bool {
    ip.is_private()
}

/// Public dial endpoint for a private RCC address: `Some(proxy_ip)` when the
/// datacenter prefix is known, `None` otherwise (the caller then dials the
/// address as-is — which cannot leave the user's LAN — and logs why).
///
/// The dial PORT is not remapped: real client logs show the UDMUX proxy
/// listening on the same port as the RCC server it fronts.
pub fn proxy_for(rcc: Ipv4Addr) -> Option<Ipv4Addr> {
    let octets = rcc.octets();
    if octets[0] != 10 {
        return None;
    }
    // Environment override first, so a stale built-in table can be fixed
    // without shipping a new build.
    if let Ok(map) = std::env::var("RUSTBLOX_UDMUX") {
        let key = octets[1].to_string();
        for pair in map.split(',') {
            let mut it = pair.splitn(2, '=');
            if let (Some(prefix), Some(ip)) = (it.next(), it.next()) {
                if prefix.trim() == key {
                    if let Ok(proxy) = ip.trim().parse() {
                        return Some(proxy);
                    }
                }
            }
        }
    }
    PROXIES
        .iter()
        .find(|(prefix, _)| *prefix == octets[1])
        .and_then(|(_, proxy)| proxy.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_prefixes_map_to_their_proxy() {
        // Frankfurt — the 10.9.#.# datacenter from the devforum table.
        assert_eq!(
            proxy_for("10.9.5.206".parse().unwrap()),
            Some("128.116.44.33".parse().unwrap())
        );
        assert_eq!(
            proxy_for("10.208.1.2".parse().unwrap()),
            Some("128.116.49.254".parse().unwrap())
        );
    }

    #[test]
    fn public_addresses_have_no_proxy() {
        assert!(!is_private("128.116.44.33".parse().unwrap()));
        assert_eq!(proxy_for("128.116.44.33".parse().unwrap()), None);
        assert_eq!(proxy_for("192.168.1.1".parse().unwrap()), None);
        assert!(is_private("192.168.1.1".parse().unwrap()));
    }
}
