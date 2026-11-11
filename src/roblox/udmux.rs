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

/// The outer datagram framing every real client uses on the wire (measured
/// by the `rbxraknet` decoder across a 16k-payload live capture — see the
/// research notes in `docs/` of the upstream project):
///
/// ```text
/// byte[0]      0x01        UDMUX/RUPP flag — NOT a RakNet message id
/// byte[1:3]    00 00       header length high bytes (always 0)
/// byte[3]      0x17|0x1f   header length, doubles as the direction id
/// byte[4:7]    01 11 02    constant sub-flags
/// byte[7:23]   epoch_tag   16 bytes, rotates on re-key epochs
/// byte[23:31]  mux_extra   only on direction 0x1f (header len 31)
/// body                     RakNet datagram (AEAD-encrypted once the
///                          session keys are active; plaintext handshake
///                          bytes while it is not)
/// ```
///
/// A proxy sees only the CLEAR part of that header, so the per-session
/// routing token (`TokenValue` from the join script) almost certainly rides
/// in the 16-byte `epoch_tag` — 24 base64 chars decode to exactly 16 bytes
/// when the token carries `==` padding. Every candidate below therefore
/// frames the handshake inside this envelope and lets the proxy pick the
/// shape it answers.

pub const RUPP_FLAG: u8 = 0x01;
pub const RUPP_SUBFLAGS: [u8; 3] = [0x01, 0x11, 0x02];

/// A datagram wrapper candidate: either the raw legacy shape
/// (`direction == 0`, nothing prepended) or a RUPP envelope header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Envelope {
    pub label: &'static str,
    /// 0 = send raw (no envelope); otherwise 0x17 or 0x1f.
    pub direction: u8,
    pub epoch: [u8; 16],
    pub mux: [u8; 8],
}

/// No-envelope variant (direct dial, or a proxy that routes by 5-tuple).
pub const BARE: Envelope = Envelope {
    label: "bare",
    direction: 0,
    epoch: [0u8; 16],
    mux: [0u8; 8],
};

/// Wrap `body` in the RUPP envelope described by `env`.
pub fn rupp_frame(env: &Envelope, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(env.direction as usize + body.len());
    out.push(RUPP_FLAG);
    out.push(0);
    out.push(0);
    out.push(env.direction);
    out.extend_from_slice(&RUPP_SUBFLAGS);
    out.extend_from_slice(&env.epoch);
    if env.direction == 0x1f {
        out.extend_from_slice(&env.mux);
    }
    out.extend_from_slice(body);
    out
}

/// Recognize a RUPP-framed datagram. Returns
/// `(direction, epoch_tag, mux_extra, header_len)`; `mux_extra` is all-zero
/// for direction 0x17 (it has no mux field). `None` when the payload is not
/// a RUPP datagram (e.g. a legacy raw RakNet reply).
pub fn rupp_parse(p: &[u8]) -> Option<(u8, [u8; 16], [u8; 8], usize)> {
    if p.len() < 7 || p[0] != RUPP_FLAG || p[1] != 0 || p[2] != 0 {
        return None;
    }
    let dir = p[3];
    if dir != 0x17 && dir != 0x1f {
        return None;
    }
    if p[4..7] != RUPP_SUBFLAGS {
        return None;
    }
    if p.len() < dir as usize {
        return None;
    }
    let mut epoch = [0u8; 16];
    epoch.copy_from_slice(&p[7..23]);
    let mut mux = [0u8; 8];
    if dir == 0x1f {
        mux.copy_from_slice(&p[23..31]);
    }
    Some((dir, epoch, mux, dir as usize))
}

/// Envelope candidates for a session that carries join-script routing
/// material (`TokenValue`, `PepperId`). Ordered by likelihood; "bare" first
/// so a proxy that routes by 5-tuple alone is not confused by a token. The
/// winner is pinned and applied to every datagram of the session.
pub fn envelope_candidates(token: &str, pepper: u64) -> Vec<Envelope> {
    let mut out = vec![BARE];
    if token.is_empty() {
        return out;
    }
    let dec = b64_decode(token);
    let pe = (pepper as u32).to_le_bytes();
    let mut push = |label: &'static str, direction: u8, epoch: [u8; 16], mux: [u8; 8]| {
        out.push(Envelope { label, direction, epoch, mux });
    };

    if dec.len() >= 16 {
        let tok16: [u8; 16] = dec[..16].try_into().unwrap();
        // The most likely shape: the decoded token IS the first epoch tag.
        push("env17-token", 0x17, tok16, [0u8; 8]);
        if dec.len() >= 18 {
            // Unpadded tokens decode to 18 bytes; maybe the first two bytes
            // are a sub-header the tag follows.
            let tok18: [u8; 16] = dec[2..18].try_into().unwrap();
            push("env17-token18", 0x17, tok18, [0u8; 8]);
        }
        // Direction 0x1f variants (8-byte mux field after the tag).
        push("env1f-token", 0x1f, tok16, [0u8; 8]);
        let muxtok: [u8; 8] = dec[..8].try_into().unwrap();
        push("env1f-token-muxtok", 0x1f, tok16, muxtok);
        let mut muxpe = [0u8; 8];
        muxpe[..4].copy_from_slice(&pe);
        muxpe[4..].copy_from_slice(&pe);
        push("env1f-token-muxpe", 0x1f, tok16, muxpe);
        // Pepper mixed into the tag.
        let mut pe_tok = [0u8; 16];
        pe_tok[..4].copy_from_slice(&pe);
        pe_tok[4..].copy_from_slice(&dec[..12]);
        push("env17-pe+token", 0x17, pe_tok, [0u8; 8]);
        let mut tok_pe = [0u8; 16];
        tok_pe[..12].copy_from_slice(&dec[..12]);
        tok_pe[12..].copy_from_slice(&pe);
        push("env17-token+pe", 0x17, tok_pe, [0u8; 8]);
    }
    // Raw ASCII token (in case TokenValue is not base64 at all).
    let mut ascii = [0u8; 16];
    let n = token.as_bytes().len().min(16);
    ascii[..n].copy_from_slice(&token.as_bytes()[..n]);
    push("env17-ascii", 0x17, ascii, [0u8; 8]);
    // All-zero tag: proves whether the proxy routes by source 5-tuple.
    push("env17-zeros", 0x17, [0u8; 16], [0u8; 8]);
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
    fn envelope_candidates_cover_the_observed_shapes() {
        // No token -> bare only (direct dial keeps working unchanged).
        assert_eq!(envelope_candidates("", 0), vec![BARE]);

        // A padded 24-char token decodes to exactly 16 bytes -> the epoch tag.
        let padded = "AAAAAAAAAAAAAAAAAAAAAA==";
        assert_eq!(b64_decode(padded).len(), 16);
        let cands = envelope_candidates(padded, 1794414724);
        let labels: Vec<&str> = cands.iter().map(|c| c.label).collect();
        assert_eq!(
            labels,
            vec![
                "bare",
                "env17-token",
                "env1f-token",
                "env1f-token-muxtok",
                "env1f-token-muxpe",
                "env17-pe+token",
                "env17-token+pe",
                "env17-ascii",
                "env17-zeros",
            ]
        );
        // env17-token: the decoded token IS the tag, no 18-byte skip variant.
        assert_eq!(cands[1].direction, 0x17);
        assert_eq!(cands[1].epoch, [0u8; 16]);
        // env1f-token-muxpe: the 8-byte mux field is the pepper repeated.
        let pe = 1794414724u32.to_le_bytes();
        assert_eq!(&cands[4].mux[..4], &pe);
        assert_eq!(&cands[4].mux[4..], &pe);
        // env17-pe+token mixes the pepper into the first 4 tag bytes.
        assert_eq!(&cands[5].epoch[..4], &pe);
    }

    #[test]
    fn unpadded_tokens_add_the_18_byte_variant() {
        // 24 chars without padding -> 18 bytes -> a tok18 (skip-2) candidate.
        let cands = envelope_candidates("AAAAAAAAAAAAAAAAAAAAAAAA", 0);
        let labels: Vec<&str> = cands.iter().map(|c| c.label).collect();
        assert!(labels.contains(&"env17-token18"), "labels: {:?}", labels);
        let tok18 = cands.iter().find(|c| c.label == "env17-token18").unwrap();
        assert_eq!(tok18.epoch, [0u8; 16]);
    }

    #[test]
    fn rupp_frame_round_trips_through_parse() {
        let env = Envelope {
            label: "test17",
            direction: 0x17,
            epoch: *b"0123456789abcdef",
            mux: [0u8; 8],
        };
        let framed = rupp_frame(&env, b"PINGBODY");
        let (dir, epoch, mux, hlen) = rupp_parse(&framed).expect("17 must parse");
        assert_eq!((dir, hlen), (0x17, 23usize));
        assert_eq!(&epoch, b"0123456789abcdef");
        assert_eq!(mux, [0u8; 8]);
        assert_eq!(&framed[hlen..], b"PINGBODY");

        let env31 = Envelope {
            label: "test1f",
            direction: 0x1f,
            epoch: [9u8; 16],
            mux: *b"MUXMUXMX",
        };
        let framed31 = rupp_frame(&env31, b"X");
        let (dir, epoch, mux, hlen) = rupp_parse(&framed31).expect("1f must parse");
        assert_eq!((dir, hlen), (0x1f, 31usize));
        assert_eq!(epoch, [9u8; 16]);
        assert_eq!(&mux, b"MUXMUXMX");
        assert_eq!(&framed31[hlen..], b"X");
    }

    #[test]
    fn rupp_parse_rejects_non_envelopes() {
        // Raw RakNet offline packets never start with the RUPP signature.
        assert!(rupp_parse(&[]).is_none());
        assert!(rupp_parse(&[0x06, 0x00, 0x00, 0x17, 0x01, 0x11, 0x02]).is_none());
        assert!(rupp_parse(&[0x01, 0x00, 0x00, 0x99, 0x01, 0x11, 0x02]).is_none());
        assert!(rupp_parse(&[0x01, 0x00, 0x00, 0x17, 0x09, 0x09, 0x09]).is_none());
    }

    #[test]
    fn public_addresses_have_no_proxy() {
        assert!(!is_private("128.116.44.33".parse().unwrap()));
        assert_eq!(proxy_for("128.116.44.33".parse().unwrap()), None);
        assert_eq!(proxy_for("192.168.1.1".parse().unwrap()), None);
        assert!(is_private("192.168.1.1".parse().unwrap()));
    }
}
