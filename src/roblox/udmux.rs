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

// ---------------------------------------------------------------------------
// Per-session routing-token wrappers
// ---------------------------------------------------------------------------

/// Decode standard and url-safe base64, ignoring padding and whitespace.
/// Returns an empty vec when the input contains characters outside both
/// alphabets — the caller then keeps the raw ASCII as the candidate token.
pub fn b64_decode(s: &str) -> Vec<u8> {
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    for ch in s.chars() {
        let v = match ch {
            'A'..='Z' => ch as u32 - 'A' as u32,
            'a'..='z' => ch as u32 - 'a' as u32 + 26,
            '0'..='9' => ch as u32 - '0' as u32 + 52,
            '+' | '-' => 62,
            '/' | '_' => 63,
            '=' | '\r' | '\n' | ' ' | '\t' => continue,
            _ => return Vec::new(),
        };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// Candidate per-datagram prefixes the UDMUX proxy may require before it
/// forwards a packet to the RCC backend. The join script hands out a session
/// token (`TokenValue`, 24 chars) and a pepper (`PepperId`); the official
/// client applies both somewhere in its send path, but the exact wire shape
/// is not public (see the headless-join write-ups: the token generator is
/// still marked TODO even in the deepest public reversals). Until one shape
/// is confirmed empirically, the handshake tries them in order and keeps
/// the first one that gets a reply:
///
/// 1. `bare`        — no prefix (the proxy may route by port alone); if the
///                    proxy is content with a bare packet, a token prefix
///                    would only make things worse, so it goes first;
/// 2. `b64`         — TokenValue base64-decoded (24 chars -> 18 bytes);
/// 3. `pe-le+b64`   — PepperId as u32 LE + the decoded token;
/// 4. `pe-be+b64`   — PepperId as u32 BE + the decoded token;
/// 5. `ascii`       — the raw 24-char token string;
/// 6. `pe-le+ascii` — PepperId u32 LE + the raw token string.
pub fn wrap_candidates(token: &str, pepper: u64) -> Vec<(&'static str, Vec<u8>)> {
    let mut out: Vec<(&'static str, Vec<u8>)> = vec![("bare", Vec::new())];
    if token.is_empty() {
        return out;
    }
    let pe = pepper as u32;
    let raw = b64_decode(token);
    if raw.len() >= 8 {
        out.push(("b64", raw.clone()));
        out.push((
            "pe-le+b64",
            pe.to_le_bytes().into_iter().chain(raw.iter().copied()).collect(),
        ));
        out.push((
            "pe-be+b64",
            pe.to_be_bytes().into_iter().chain(raw.iter().copied()).collect(),
        ));
    }
    let ascii = token.as_bytes().to_vec();
    out.push(("ascii", ascii.clone()));
    out.push((
        "pe-le+ascii",
        pe.to_le_bytes().into_iter().chain(ascii.into_iter()).collect(),
    ));
    out
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
    fn b64_decode_handles_standard_and_urlsafe() {
        assert_eq!(b64_decode("SGVsbG8="), b"Hello");
        assert_eq!(b64_decode("SGVsbG8"), b"Hello");
        // 24 chars -> exactly 18 raw bytes, the join-script token shape.
        assert_eq!(b64_decode("AAAAAAAAAAAAAAAAAAAAAAAA"), vec![0u8; 18]);
        // url-safe alphabet decodes the same as the standard one.
        assert_eq!(b64_decode("-_-_"), b64_decode("+/+/"));
        // Non-base64 input yields an empty vec (caller falls back to ascii).
        assert!(b64_decode("token with spaces!#?").is_empty());
    }

    #[test]
    fn wrap_candidates_cover_the_observed_shapes() {
        // No token -> bare only (direct dial keeps working unchanged).
        assert_eq!(wrap_candidates("", 0), vec![("bare", Vec::<u8>::new())]);

        let cands = wrap_candidates("AAAAAAAAAAAAAAAAAAAAAAAA", 1794414724);
        let labels: Vec<&str> = cands.iter().map(|(l, _)| *l).collect();
        assert_eq!(
            labels,
            vec!["bare", "b64", "pe-le+b64", "pe-be+b64", "ascii", "pe-le+ascii"]
        );
        assert_eq!(cands[1].1, vec![0u8; 18]);
        // pepper u32 LE (1794414724 = 0x6AFA0004... verify little-endian bytes)
        assert_eq!(&cands[2].1[..4], &1794414724u32.to_le_bytes());
        assert_eq!(cands[2].1.len(), 4 + 18);
        assert_eq!(cands[4].1, b"AAAAAAAAAAAAAAAAAAAAAAAA".to_vec());
    }

    #[test]
    fn public_addresses_have_no_proxy() {
        assert!(!is_private("128.116.44.33".parse().unwrap()));
        assert_eq!(proxy_for("128.116.44.33".parse().unwrap()), None);
        assert_eq!(proxy_for("192.168.1.1".parse().unwrap()), None);
        assert!(is_private("192.168.1.1".parse().unwrap()));
    }
}
