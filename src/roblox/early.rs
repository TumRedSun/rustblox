//! Early encryption (join hypothesis (c)) — the fork's crypto handshake.
//!
//! Byte-exact ground truth: the Nov-12 live capture (`handshake_full.txt`,
//! 8 datagrams of a WORKING session whose join script we also have). The
//! shapes below are transcribed straight from that dump:
//!
//! ```text
//! OCR2 (client -> server, body padded to 1129):
//!   78 | MAGIC16 | 03 5a 00 5d 00 05 | client_pub(32) | 11 | 21 | preauth(33)
//!   | nonce(12) | box(81 = ct(65)+tag(16)) | "UniqueNumber" | unique(16)
//!
//! OpenReply2 (server -> client, body 135):
//!   7d | MAGIC16 | 01 2c 00 3a 03 | server_pub(32) | box(53)
//!   | "UniqueNumber" | unique(16)
//!
//! post-reply2 datagram (client -> server, 52 total):
//!   envelope(31) | nonce(12) | ct(24) | tag(16)   <- plaintext = the
//!   vanilla 24-byte CONNECTION_REQUEST, so the AEAD overhead is exactly
//!   nonce(12)+tag(16) = 28 (the libroblox preauth-window code computes the
//!   same `payload + 0x1c` expected size).
//!
//! connected phase (from sober_ro.pcap): envelope | nonce(12) | ct(...) —
//! the 56-byte client keepalives are 31+12+13 with NO tag, plaintext = the
//! 13-byte RakNet ping datagram (4-byte datagram header + 9-byte ping).
//! ```
//!
//! Key schedule (probe4.py, libsodium `crypto_kx` semantics):
//! `H = BLAKE2b-512(x25519(client_sk, server_pk) || client_pk || server_pk)`,
//! client sends with `H[32:]` (tx), receives with `H[:32]` (rx). The OCR2
//! box uses the ephemeral join key (`EphemeralEarlyPubKey`); OpenReply2
//! announces a second server key, and the post-reply2 boxes use those
//! session keys.
//!
//! Open items (cannot be settled offline, the private keys are lost):
//! * the 65-byte OCR2 box plaintext — defaults to zeros, override with
//!   `RUSTBLOX_OCR2_PAYLOAD` (hex) or the probe matrix;
//! * the 33-byte preauth blob — every HMAC/BLAKE2/keyring combination over
//!   the public join-script fields misses the captured sample, so it is
//!   carried verbatim (`RUSTBLOX_PREAUTH`, hex) until the derivation is
//!   recovered from libroblox;
//! * the AEAD AAD for post-reply2 boxes (empty vs the RUPP envelope
//!   header) — the opener tries both.

use blake2::digest::{Update, VariableOutput};
use blake2::Blake2bVar;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use std::net::Ipv4Addr;
use x25519_dalek::{PublicKey as XPublic, StaticSecret as XSecret};

/// RakNet ids of the crypto handshake (fork numbering, from the dump).
pub const OCR2_CRYPTO_ID: u8 = 0x78;
pub const OPEN_REPLY2_ID: u8 = 0x7d;

/// Fork OCR2 header constants, byte-verified against the dump:
/// `03 5a 00 5d 00 05` = ver 3, aad 90, box 93, supportedVersion 5.
pub const OCR2_VER: u8 = 0x03;
pub const OCR2_AAD: u8 = 0x5a; // 90 = id+magic+6+pub32+auth_ver+pa_len+pa33
pub const OCR2_BOX: u8 = 0x5d; // 93 = nonce12 + ct65 + tag16
pub const OCR2_SV: u16 = 5;
pub const OCR2_AUTH_VER: u8 = 0x11; // 17
/// OpenReply2 header `01 2c 00 3a 03` = ver 1, aad 44, box 58, ? 3.
pub const REPLY2_VER: u8 = 0x01;
pub const REPLY2_AAD: u8 = 0x2c; // 44
pub const REPLY2_BOX: u8 = 0x3a; // 58 (declared; the wire box is 53)
pub const REPLY2_TAIL: u8 = 0x03;

/// Sizes measured on the wire.
pub const OCR2_PUB: usize = 32;
pub const OCR2_PA_LEN: usize = 33; // the dump carries 0x21 = 33
pub const OCR2_BOX_LEN: usize = 93; // nonce12 + ct65+tag16
pub const OCR2_PT_LEN: usize = 65; // plaintext inside the OCR2 box
pub const UNIQUE_NAME: &[u8; 12] = b"UniqueNumber";
pub const UNIQUE_LEN: usize = 16;
pub const CONNECTION_REQUEST_PT: usize = 24; // id+guid8+time8+useSec1+password6
pub const BOX_OVERHEAD: usize = 12 + 16; // nonce + tag — the libroblox +0x1c

/// The captured preauth blob (Nov-12 session, `SOBER_PA33` in probe4.py).
/// Session-scoped or static is unknown; overridable via `RUSTBLOX_PREAUTH`.
pub const CAPTURED_PREAUTH: [u8; 33] = [
    0x38, 0xbc, 0xf5, 0x02, 0x73, 0xc8, 0x7e, 0x29, 0x40, 0x18, 0x4b, 0x68, 0xff, 0x8d, 0x87,
    0x45, 0x06, 0xb3, 0xf3, 0x10, 0x02, 0x71, 0x67, 0xd1, 0xef, 0x43, 0xef, 0x3b, 0xea, 0x78,
    0x60, 0x9f, 0xa4,
];

/// libsodium `crypto_kx` client key schedule.
/// `H = BLAKE2b-512(shared || client_pk || server_pk)`; tx = `H[32:]`, rx = `H[:32]`.
fn crypto_kx(shared: &[u8], client_pk: &[u8; 32], server_pk: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    let mut h = Blake2bVar::new(64).expect("blake2b-64");
    h.update(shared);
    h.update(client_pk);
    h.update(server_pk);
    let mut out = [0u8; 64];
    h.finalize_variable(&mut out).expect("blake2b finalize");
    let mut tx = [0u8; 32];
    let mut rx = [0u8; 32];
    tx.copy_from_slice(&out[32..]);
    rx.copy_from_slice(&out[..32]);
    (tx, rx)
}

/// AEAD box over one datagram: `nonce(12) || ct || tag(16)`, AAD optional.
/// Returns the whole box to append after the clear header.
pub fn seal(key32: &[u8; 32], nonce12: &[u8; 12], aad: &[u8], pt: &[u8]) -> Vec<u8> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key32));
    let ct = cipher
        .encrypt(Nonce::from_slice(nonce12), Payload { msg: pt, aad })
        .expect("chacha20poly1305 encrypt (12-byte nonce, valid key)");
    let mut out = Vec::with_capacity(12 + ct.len());
    out.extend_from_slice(nonce12);
    out.extend_from_slice(&ct);
    out
}

/// The nonce suffix of every post-OpenReply2 datagram (ref/raknet-decryption,
/// confirmed by the dump + pcap counter hints): `nonce = u64 LE counter ||
/// "mbeR"` — the ASCII tail comes from the literal `UniqueNumbeR` string.
const NONCE_SUFFIX: [u8; 4] = *b"mbeR";

fn nonce_bytes(counter: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[..8].copy_from_slice(&counter.to_le_bytes());
    n[8..].copy_from_slice(&NONCE_SUFFIX);
    n
}

/// Post-OpenReply2 framing (dump pkt4..7 + the whole pcap, proven by the
/// sequential counter hints): wire = `ct || counter_hint(u16 LE) || tag(16)`,
/// nonce = `u64 LE counter || "mbeR"`, AAD EMPTY. The hint is the low 16 bits
/// of the counter; the high bits come from the negotiated base (session
/// setup). The dump's first post-OR2 counter is 0x6e55 on BOTH sides — the
/// base is not zero and rides inside encrypted session material we cannot
/// read offline, so it is configurable (`RUSTBLOX_NONCE_BASE`) and the
/// opener scans a window of high bits.
pub fn seal_counter(key32: &[u8; 32], counter: u64, pt: &[u8]) -> Vec<u8> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key32));
    let box_out = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes(counter)), Payload { msg: pt, aad: b"" })
        .expect("chacha20poly1305 encrypt");
    let (ct, tag) = box_out.split_at(box_out.len() - 16);
    let mut out = Vec::with_capacity(ct.len() + 18);
    out.extend_from_slice(ct);
    out.extend_from_slice(&(counter as u16).to_le_bytes()); // hint
    out.extend_from_slice(tag);
    out
}

/// Open a counter-framed datagram. Scans the high bits of the counter
/// around `base` (the wire only carries the low 16). Returns
/// `(plaintext, counter_value)`.
pub fn open_counter(
    key32: &[u8; 32],
    wire: &[u8],
    base: u64,
    window: u64,
) -> Option<(Vec<u8>, u64)> {
    if wire.len() < 18 {
        return None;
    }
    let (ct, rest) = wire.split_at(wire.len() - 18);
    let hint = u16::from_le_bytes([rest[0], rest[1]]);
    let tag = &rest[2..];
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key32));
    let base_lo = (base & 0xFFFF) as u16;
    let diff = hint.wrapping_sub(base_lo) as u64;
    let mut data = Vec::with_capacity(ct.len() + 16);
    data.extend_from_slice(ct);
    data.extend_from_slice(tag);
    for k in 0..=window {
        let val = base.wrapping_add(diff).wrapping_add(k.wrapping_mul(0x10000));
        if let Ok(pt) = cipher.decrypt(
            Nonce::from_slice(&nonce_bytes(val)),
            Payload { msg: &data, aad: b"" },
        ) {
            return Some((pt, val));
        }
    }
    None
}

/// Open a box produced by [`seal`]. `aad_modes` are tried in order; returns
/// `(plaintext, aad_mode_index)` on the first success.
pub fn open(key32: &[u8; 32], box_wire: &[u8], aads: &[&[u8]]) -> Option<(Vec<u8>, usize)> {
    if box_wire.len() < 12 + 16 {
        return None;
    }
    let (nonce, ct) = box_wire.split_at(12);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key32));
    for (i, aad) in aads.iter().enumerate() {
        if let Ok(pt) = cipher.decrypt(
            Nonce::from_slice(nonce),
            Payload { msg: ct, aad },
        ) {
            return Some((pt, i));
        }
    }
    None
}

/// 32 random bytes for key generation (/dev/urandom — desktop and Android
/// both have it; a blocking read of 32 bytes never fails in practice).
fn random32() -> [u8; 32] {
    let mut b = [0u8; 32];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut b);
    } else {
        // Fallback: entropy from the clock, mixed like the pre-connect tag.
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        for (i, chunk) in b.chunks_mut(8).enumerate() {
            let z = t.wrapping_mul(0x9e37_79b9_7f4a_7c15u64.wrapping_add(i as u64));
            chunk.copy_from_slice(&z.to_le_bytes()[..chunk.len()]);
        }
    }
    b
}

/// A monotonic 12-byte nonce source (client counter — unique per direction,
/// so nonce reuse is impossible while the session lives).
pub struct NonceSource(u128);

impl NonceSource {
    pub fn new() -> Self {
        let seed = u128::from_le_bytes(random32()[..16].try_into().expect("16 bytes"));
        NonceSource(seed.wrapping_mul(2))
    }
    pub fn next(&mut self) -> [u8; 12] {
        self.0 = self.0.wrapping_add(1);
        let b = self.0.to_le_bytes();
        let mut n = [0u8; 12];
        n.copy_from_slice(&b[..12]);
        n
    }
}

/// One side of the handshake crypto: keys + the join-script material.
pub struct EarlyCrypto {
    /// `EphemeralEarlyPubKey` — the server's ephemeral X25519 key.
    pub server_ephemeral: [u8; 32],
    pub client_sk: XSecret,
    pub client_pk: [u8; 32],
    /// Early keys from the ephemeral agreement: (tx c2s, rx s2c).
    pub tx: [u8; 32],
    pub rx: [u8; 32],
    /// Session keys from the OpenReply2 key (set after the reply parses).
    pub session_tx: Option<[u8; 32]>,
    pub session_rx: Option<[u8; 32]>,
    /// The 33-byte preauth blob (captured constant or `RUSTBLOX_PREAUTH`).
    pub preauth: Vec<u8>,
    /// The client's 16-byte UniqueNumber value.
    pub unique: [u8; 16],
    /// The 65-byte OCR2 box plaintext (`RUSTBLOX_OCR2_PAYLOAD` or zeros).
    pub ocr2_payload: Vec<u8>,
    /// Post-OpenReply2 nonce counter base (`RUSTBLOX_NONCE_BASE`). The dump's
    /// session starts at 0x6e55 on both sides — the base rides inside
    /// encrypted setup material; default 0 until it is decoded.
    pub nonce_base: u64,
    /// Outgoing counter (base + increments, one per datagram we seal).
    pub send_counter: u64,
    /// Random nonces for the OCR2 box (nonce-on-the-wire phase).
    pub nonces: NonceSource,
}

impl EarlyCrypto {
    /// Build from the join script's percent-encoded `EphemeralEarlyPubKey`.
    /// `None` when the field is missing or does not decode to 32 bytes —
    /// the caller then runs the legacy handshake.
    pub fn from_join(eepk_encoded: &str) -> Option<EarlyCrypto> {
        if eepk_encoded.is_empty() {
            return None;
        }
        // The join script ships the key percent-encoded (`%2B`/`%2F`/`%3D`)
        // — percent-decode FIRST, then base64. Decoding the raw string
        // yields 0 bytes and silently disabled the whole crypto handshake.
        let decoded = crate::roblox::udmux::percent_decode(eepk_encoded);
        let key = crate::roblox::udmux::b64_decode(&decoded);
        if key.len() != 32 {
            return None;
        }
        let mut k = [0u8; 32];
        k.copy_from_slice(&key);
        Some(EarlyCrypto::new(k))
    }

    /// Build from the join script's percent-encoded `EphemeralEarlyPubKey`
    /// (32 raw bytes after decoding).
    pub fn new(server_ephemeral: [u8; 32]) -> Self {
        let client_sk = XSecret::from(random32());
        let client_pk_arr: [u8; 32] = XPublic::from(&client_sk).to_bytes();
        let shared = client_sk.diffie_hellman(&XPublic::from(server_ephemeral));
        let (tx, rx) = crypto_kx(shared.as_bytes(), &client_pk_arr, &server_ephemeral);
        let mut unique = [0u8; 16];
        // Mix the clock into the unique number so concurrent joins differ.
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for (i, chunk) in unique.chunks_mut(8).enumerate() {
            let z = t.wrapping_mul(u128::from(
                0x9e37_79b9_7f4a_7c15u64.wrapping_add(i as u64 * 7),
            ));
            chunk.copy_from_slice(&z.to_le_bytes()[..chunk.len()]);
        }
        let nonce_base = std::env::var("RUSTBLOX_NONCE_BASE")
            .ok()
            .and_then(|s| u64::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok())
            .unwrap_or(0);
        let preauth = std::env::var("RUSTBLOX_PREAUTH")
            .ok()
            .and_then(|s| hex_decode_33(&s))
            .unwrap_or_else(|| CAPTURED_PREAUTH.to_vec());
        let ocr2_payload = std::env::var("RUSTBLOX_OCR2_PAYLOAD")
            .ok()
            .and_then(|s| hex_decode(&s))
            .unwrap_or_else(|| vec![0u8; OCR2_PT_LEN]);
        EarlyCrypto {
            server_ephemeral,
            client_sk,
            client_pk: client_pk_arr,
            tx,
            rx,
            session_tx: None,
            session_rx: None,
            preauth,
            unique,
            ocr2_payload,
            nonce_base,
            send_counter: 0,
            nonces: NonceSource::new(),
        }
    }

    /// Derive the session keys from the OpenReply2 server key.
    pub fn adopt_reply2_key(&mut self, server_pk: [u8; 32]) {
        let shared = self.client_sk.diffie_hellman(&XPublic::from(server_pk));
        let (tx, rx) = crypto_kx(shared.as_bytes(), &self.client_pk, &server_pk);
        self.session_tx = Some(tx);
        self.session_rx = Some(rx);
    }

    /// Build the crypto OCR2 body (before envelope framing). Returns the
    /// `(aad, full_body)` pair — the AAD is the clear `78..preauth` prefix
    /// the box authenticates.
    pub fn build_ocr2(&mut self) -> (Vec<u8>, Vec<u8>) {
        let mut aad = Vec::with_capacity(OCR2_AAD as usize);
        aad.push(OCR2_CRYPTO_ID);
        aad.extend_from_slice(&crate::raknet::OFFLINE_MAGIC);
        aad.extend_from_slice(&[OCR2_VER, OCR2_AAD, 0x00, OCR2_BOX]);
        aad.extend_from_slice(&OCR2_SV.to_be_bytes());
        aad.extend_from_slice(&self.client_pk);
        aad.push(OCR2_AUTH_VER);
        aad.push(self.preauth.len() as u8);
        aad.extend_from_slice(&self.preauth);
        debug_assert_eq!(aad.len(), OCR2_AAD as usize, "OCR2 AAD must be 90");
        let mut pt = self.ocr2_payload.clone();
        pt.resize(OCR2_PT_LEN, 0);
        let nonce = self.nonces.next();
        let box_wire = seal(&self.tx, &nonce, &aad, &pt);
        debug_assert_eq!(box_wire.len(), OCR2_BOX_LEN, "OCR2 box must be 93");
        let mut body = aad.clone();
        body.extend_from_slice(&box_wire);
        body.extend_from_slice(UNIQUE_NAME);
        body.extend_from_slice(&self.unique);
        (aad, body)
    }

    /// Parse the crypto OpenReply2: extracts the server's session key and
    /// adopts the session keys. Returns `(server_pub, box_wire, unique)`.
    pub fn parse_reply2(body: &[u8]) -> Option<([u8; 32], &[u8], [u8; 16])> {
        if body.len() < 22 + OCR2_PUB + UNIQUE_NAME.len() + UNIQUE_LEN {
            return None;
        }
        if body[0] != OPEN_REPLY2_ID || body[1..17] != crate::raknet::OFFLINE_MAGIC[..] {
            return None;
        }
        // Header `01 2c 00 3a 03` — ver/aad/box/3 (the dump's exact bytes;
        // aad/box are recorded but not enforced, the reply2 box size on the
        // wire (53) does not match its own declared 58).
        let pub_off = 22; // id(1) + magic(16) + hdr(5)
        let mut sp = [0u8; 32];
        sp.copy_from_slice(&body[pub_off..pub_off + 32]);
        // UniqueNumber rides in the clear as the LAST 28 bytes.
        let tail = &body[body.len() - UNIQUE_NAME.len() - UNIQUE_LEN..];
        if tail[..12] != UNIQUE_NAME[..] {
            return None;
        }
        let mut un = [0u8; 16];
        un.copy_from_slice(&tail[12..]);
        let box_wire = &body[pub_off + 32..body.len() - UNIQUE_NAME.len() - UNIQUE_LEN];
        Some((sp, box_wire, un))
    }

    /// Open the post-reply2 handshake box (a CONNECTION_REQUEST-sized AEAD
    /// box). Tries the session keys first, then the early keys; AAD empty
    /// first, then the RUPP envelope header.
    pub fn open_handshake_box(&self, box_wire: &[u8], envelope_aad: Option<&[u8]>) -> Option<Vec<u8>> {
        let mut attempts: Vec<(&[u8; 32], &[u8])> = Vec::with_capacity(4);
        if let Some(sk) = &self.session_rx {
            attempts.push((sk, b""));
        }
        attempts.push((&self.rx, b""));
        if let Some(env) = envelope_aad {
            if let Some(sk) = &self.session_rx {
                attempts.push((sk, env));
            }
            attempts.push((&self.rx, env));
        }
        for (key, aad) in attempts {
            if let Some((pt, _)) = open(key, box_wire, &[aad]) {
                return Some(pt);
            }
        }
        None
    }

    /// Seal ANY post-OpenReply2 datagram (handshake boxes pkt4..7 AND the
    /// connected phase — one scheme, proven by the counter hints):
    /// `ct || hint(u16 LE) || tag(16)`, nonce = `counter u64 LE || "mbeR"`,
    /// AAD empty. The counter increments per datagram we send.
    pub fn seal_counter(&mut self, pt: &[u8]) -> Vec<u8> {
        let key = self.session_tx.unwrap_or(self.tx);
        let counter = self.nonce_base.wrapping_add(self.send_counter);
        self.send_counter += 1;
        seal_counter(&key, counter, pt)
    }

    /// Open any post-OpenReply2 datagram: counter-hint scheme, scanning the
    /// high counter bits around the base (window 8 by default — the hints
    /// stay near the base in a healthy session).
    pub fn open_counter(&self, wire: &[u8]) -> Option<(Vec<u8>, u64)> {
        let mut attempts: Vec<&[u8; 32]> = Vec::with_capacity(2);
        if let Some(sk) = &self.session_rx {
            attempts.push(sk);
        }
        attempts.push(&self.rx);
        for key in attempts {
            if let Some(hit) = open_counter(key, wire, self.nonce_base, 8) {
                return Some(hit);
            }
        }
        None
    }
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 || s.is_empty() || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

fn hex_decode_33(s: &str) -> Option<Vec<u8>> {
    let v = hex_decode(s)?;
    if v.len() == 33 {
        Some(v)
    } else {
        None
    }
}

/// One `RakNetEarlyPublicKey` version from the join script's
/// `ClientPublicKeyData` keyring JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyringVersion {
    pub id: u64,
    pub key: Vec<u8>,
    pub allowed: bool,
}

/// Parse the keyring JSON (`ClientPublicKeyData`): applications →
/// `RakNetEarlyPublicKey` → versions[]. Returns `(versions, send_id)`.
pub fn parse_keyring(cpkd: &str) -> Option<(Vec<KeyringVersion>, Option<u64>)> {
    let v: serde_json::Value = serde_json::from_str(cpkd).ok()?;
    let app = v
        .get("applications")?
        .get("RakNetEarlyPublicKey")?
        .clone();
    let mut versions = Vec::new();
    if let Some(arr) = app.get("versions").and_then(|a| a.as_array()) {
        for ver in arr {
            let id = ver.get("id").and_then(|x| x.as_u64()).unwrap_or(0);
            let b64 = ver.get("value").and_then(|x| x.as_str()).unwrap_or("");
            let key = crate::roblox::udmux::b64_decode(b64);
            if key.is_empty() {
                continue;
            }
            let allowed = ver
                .get("allowed")
                .and_then(|x| x.as_bool())
                .unwrap_or(true);
            versions.push(KeyringVersion { id, key, allowed });
        }
    }
    let send = app.get("send").and_then(|x| x.as_u64());
    if versions.is_empty() {
        None
    } else {
        Some((versions, send))
    }
}

/// The mux TLV (`02 06 <ip4> <port BE16>`) — shared by the envelope builder
/// and the probe tool documentation.
pub fn mux_tlv(rcc: (Ipv4Addr, u16)) -> [u8; 8] {
    let mut mux = [0u8; 8];
    mux[0] = 0x02;
    mux[1] = 0x06;
    mux[2..6].copy_from_slice(&rcc.0.octets());
    mux[6..8].copy_from_slice(&rcc.1.to_be_bytes());
    mux
}

// ---------------------------------------------------------------------------
// Tests — every constant below is transcribed from the Nov-12 live capture
// (handshake_full.txt + its join script), not invented.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const DUMP_CLIENT_PUB: [u8; 32] = [
        0xfb, 0xfa, 0x3b, 0x1b, 0x65, 0x05, 0x1e, 0x36, 0xfd, 0x87, 0x44, 0xa6, 0xc9, 0x58, 0x49,
        0xcc, 0xa3, 0x02, 0xb9, 0x28, 0xb0, 0x38, 0x1d, 0xe6, 0xb8, 0xcd, 0xf2, 0x76, 0xf3, 0x23,
        0x83, 0x5a,
    ];
    const DUMP_REPLY2_SERVER_PUB: [u8; 32] = [
        0xbf, 0x5a, 0xcc, 0x9a, 0x8f, 0xc0, 0xbe, 0xba, 0x8b, 0x40, 0x5a, 0xd8, 0x88, 0xc0, 0xe4,
        0xe5, 0x56, 0xe2, 0x20, 0xc7, 0x37, 0x64, 0x1c, 0x35, 0xc4, 0x6f, 0x77, 0x93, 0x6c, 0x25,
        0x76, 0x4e,
    ];
    const DUMP_REPLY2_UNIQUE: [u8; 16] = [
        0x44, 0xfa, 0xe9, 0x4b, 0x7f, 0x28, 0xdd, 0xd7, 0x2d, 0x0b, 0x21, 0x57, 0x87, 0x0f, 0x28,
        0xa8,
    ];

    /// The dump's pkt3 OpenReply2 body, byte-for-byte (minus the RUPP header).
    fn dump_reply2_body() -> Vec<u8> {
        let mut b = vec![OPEN_REPLY2_ID];
        b.extend_from_slice(&crate::raknet::OFFLINE_MAGIC);
        b.extend_from_slice(&[0x01, 0x2c, 0x00, 0x3a, 0x03]);
        b.extend_from_slice(&DUMP_REPLY2_SERVER_PUB);
        b.extend_from_slice(&[
            0x98, 0xab, 0x01, 0x10, 0x87, 0x05, 0x5a, 0x8a, 0xf3, 0x59, 0x2e, 0x8f, 0xbc, 0x5d,
            0x29, 0x24, 0xbc, 0x22, 0xf0, 0x38, 0xdd, 0xbf, 0x0e, 0x19, 0xf9, 0xb9, 0x57, 0x1c,
            0x3b, 0xc7, 0x18, 0xb0, 0xd3, 0x5a, 0xed, 0x18, 0xd5, 0x78, 0xdc, 0x3b, 0xd9, 0xff,
            0xcf, 0x51, 0x70, 0xa7, 0x95, 0x85, 0x18, 0x03, 0x65, 0x66, 0x36,
        ]);
        b.extend_from_slice(UNIQUE_NAME);
        b.extend_from_slice(&DUMP_REPLY2_UNIQUE);
        b
    }

    #[test]
    fn ocr2_aad_reconstructs_the_dump_bytes() {
        let ec = EarlyCrypto::from_join("M17S4M%2Bk%2FCjattHW2WMEufaCnwrgd6itqO%2FVngg5R0k%3D")
            .expect("the live percent-encoded key must decode");
        // The captured session key from the same join script (real decode).
        assert_eq!(
            ec.server_ephemeral,
            [
                0x33, 0x5e, 0xd2, 0xe0, 0xcf, 0xa4, 0xfc, 0x28, 0xda, 0xb6, 0xd1, 0xd6, 0xd9, 0x63,
                0x04, 0xb9, 0xf6, 0x82, 0x9f, 0x0a, 0xe0, 0x77, 0xa8, 0xad, 0xa8, 0xef, 0xd5, 0x9e,
                0x08, 0x39, 0x47, 0x49
            ]
        );
        // AAD = id + magic + 03 5a 00 5d 00 05 + pub32 + 11 + pa_len + pa33.
        let mut want = vec![OCR2_CRYPTO_ID];
        want.extend_from_slice(&crate::raknet::OFFLINE_MAGIC);
        want.extend_from_slice(&[0x03, 0x5a, 0x00, 0x5d, 0x00, 0x05]);
        want.extend_from_slice(&ec.client_pk);
        want.push(0x11);
        want.push(0x21);
        want.extend_from_slice(&CAPTURED_PREAUTH);
        assert_eq!(want.len(), 90);
        // Force the captured key so the structure matches the dump byte-wise.
        let mut ec2 = EarlyCrypto::new(ec.server_ephemeral);
        ec2.preauth = CAPTURED_PREAUTH.to_vec();
        ec2.ocr2_payload = vec![0u8; OCR2_PT_LEN];
        let (aad, body) = ec2.build_ocr2();
        // The 32 bytes at [23..55] are the FRESH client pub (regenerated per
        // run), so compare the fixed structure around it — the layout is the
        // dump's, byte-for-byte: id + magic + `03 5a 00 5d 00 05` + pub(32)
        // + `11 21` + preauth(33) = 90.
        assert_eq!(aad.len(), 90);
        assert_eq!(&aad[..23], &want[..23]);
        assert_eq!(&aad[55..], &want[55..]);
        // Body = aad + box(93) + "UniqueNumber" + 16.
        assert_eq!(&body[..23], &want[..23]);
        assert_eq!(&body[55..90], &want[55..]);
        assert_eq!(body.len(), 90 + 93 + 28);
        assert_eq!(&body[90 + 93..90 + 93 + 12], UNIQUE_NAME);
        assert_eq!(body.len(), 211);
    }

    #[test]
    fn reply2_parse_extracts_the_dump_fields() {
        let body = dump_reply2_body();
        assert_eq!(body.len(), 135, "the dump's OpenReply2 body is 135 bytes");
        let (sp, box_wire, un) = EarlyCrypto::parse_reply2(&body).expect("dump shape must parse");
        assert_eq!(sp, DUMP_REPLY2_SERVER_PUB);
        assert_eq!(un, DUMP_REPLY2_UNIQUE);
        assert_eq!(box_wire.len(), 53, "the server box on the wire is 53 bytes");
        // The client pub in the dump's OCR2 is 32 bytes as assumed.
        assert_eq!(DUMP_CLIENT_PUB.len(), 32);
    }

    #[test]
    fn crypto_kx_matches_the_libsodium_formula() {
        // RFC 7748 X25519 vector, BLAKE2b-512 over shared||client_pk||server_pk
        // (reference computed with Python's hashlib.blake2b).
        let alice_sk = XSecret::from([
            0x77, 0x07, 0x6d, 0x0a, 0x73, 0x18, 0xa5, 0x7d, 0x3c, 0x16, 0xc1, 0x72, 0x51, 0xb2,
            0x66, 0x45, 0xdf, 0x4c, 0x2f, 0x87, 0xeb, 0xc0, 0x99, 0x2a, 0xb1, 0x77, 0xfb, 0xa5,
            0x1d, 0xb9, 0x2c, 0x2a,
        ]);
        let alice_pk: [u8; 32] = XPublic::from(&alice_sk).to_bytes();
        assert_eq!(
            alice_pk,
            [
                0x85, 0x20, 0xf0, 0x09, 0x89, 0x30, 0xa7, 0x54, 0x74, 0x8b, 0x7d, 0xdc, 0xb4, 0x3e,
                0xf7, 0x5a, 0x0d, 0xbf, 0x3a, 0x0d, 0x26, 0x38, 0x1a, 0xf4, 0xeb, 0xa4, 0xa9, 0x8e,
                0xaa, 0x9b, 0x4e, 0x6a
            ]
        );
        let bob_pk: [u8; 32] = [
            0xde, 0x9e, 0xdb, 0x7d, 0x7b, 0x7d, 0xc1, 0xb4, 0xd3, 0x5b, 0x61, 0xc2, 0xec, 0xe4,
            0x35, 0x37, 0x3f, 0x83, 0x43, 0xc8, 0x5b, 0x78, 0x67, 0x4d, 0xad, 0xfc, 0x7e, 0x14,
            0x6f, 0x88, 0x2b, 0x4f,
        ];
        let shared = alice_sk.diffie_hellman(&XPublic::from(bob_pk));
        let (tx, rx) = crypto_kx(shared.as_bytes(), &alice_pk, &bob_pk);
        assert_eq!(
            rx,
            [
                0x32, 0x2b, 0x7b, 0xe3, 0xb9, 0xbc, 0xe4, 0xa8, 0x4f, 0xe6, 0xe2, 0xde, 0xa6, 0x1e,
                0x8e, 0x6d, 0x0a, 0x98, 0xf3, 0xe4, 0xc6, 0x0b, 0x58, 0xba, 0xd7, 0x22, 0xb1, 0xc8,
                0x55, 0xc9, 0xdb, 0x22
            ]
        );
        assert_eq!(
            tx,
            [
                0x28, 0x49, 0x01, 0xa6, 0x11, 0x70, 0x83, 0x79, 0xd0, 0xb5, 0xb0, 0xe4, 0x0d, 0x77,
                0xea, 0x20, 0x76, 0x24, 0xea, 0xab, 0x8d, 0xd0, 0xc9, 0x5e, 0x69, 0x3f, 0xc3, 0xee,
                0x76, 0xc7, 0x3c, 0xcb
            ]
        );
    }

    #[test]
    fn counter_framing_overhead_is_exactly_18_bytes() {
        // The dump's pkt4: envelope(31) + box(52) → the counter box adds
        // exactly ct..hint(2)+tag(16) = 18 bytes over the plaintext.
        let mut ec = EarlyCrypto::from_join("M17S4M%2Bk%2FCjattHW2WMEufaCnwrgd6itqO%2FVngg5R0k%3D")
            .unwrap();
        // A loopback peer shares the SAME key in both directions (the wire
        // semantics are tx on one side, rx on the other).
        ec.session_tx = Some(ec.tx);
        ec.session_rx = Some(ec.tx);
        let pt = vec![9u8; CONNECTION_REQUEST_PT];
        let boxed = ec.seal_counter(&pt);
        assert_eq!(boxed.len(), CONNECTION_REQUEST_PT + 18);
        // The hint = the low 16 bits of the counter (base 0 → 0).
        assert_eq!(&boxed[pt.len()..pt.len() + 2], &[0, 0]);
        // Round-trips through the scanning opener.
        let (back, counter) = ec.open_counter(&boxed).expect("loopback must open");
        assert_eq!(back, pt);
        assert_eq!(counter, 0);
        // The second datagram bumps the counter; the hint follows.
        let boxed2 = ec.seal_counter(&pt);
        assert_eq!(&boxed2[pt.len()..pt.len() + 2], &[1, 0]);
        assert!(ec.open_counter(&boxed2).is_some());
        // The two directions really use different keys (client tx != rx).
        assert_ne!(ec.tx, ec.rx);
    }

    #[test]
    fn counter_nonce_uses_the_mbeR_suffix() {
        // nonce = u64 LE counter || "mbeR" — the ref/raknet-decryption scheme
        // (confirmed by the pcap's sequential counter hints).
        let n = nonce_bytes(1);
        assert_eq!(&n[..8], &1u64.to_le_bytes());
        assert_eq!(&n[8..], b"mbeR");
    }

    #[test]
    fn keyring_parses_the_live_join_json() {
        let (versions, send) = parse_keyring(
            r#"{"creationTime":"11:55 12/6/2022","applications":{"RakNetEarlyPublicKey":{"versions":[{"id":5,"value":"Jy0uXN86HFuODRudIzEzZeLfCd8/z+ER6CMO0wAa5XY=","allowed":true}],"send":5,"revert":5}}}"#,
        )
        .expect("the live keyring must parse");
        assert_eq!(send, Some(5));
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].id, 5);
        assert!(versions[0].allowed);
        assert_eq!(
            versions[0].key,
            [
                0x27, 0x2d, 0x2e, 0x5c, 0xdf, 0x3a, 0x1c, 0x5b, 0x8e, 0x0d, 0x1b, 0x9d, 0x23, 0x31,
                0x33, 0x65, 0xe2, 0xdf, 0x09, 0xdf, 0x3f, 0xcf, 0xe1, 0x11, 0xe8, 0x23, 0x0e, 0xd3,
                0x00, 0x1a, 0xe5, 0x76
            ]
        );
    }

    #[test]
    fn counter_base_env_and_hint_scanning() {
        // The dump session's first post-OR2 counter is 0x6e55 — a nonzero
        // negotiated base. With a nonzero base the hint carries the low 16
        // bits and the opener reconstructs the value from base + hint.
        let mut ec = EarlyCrypto::from_join("M17S4M%2Bk%2FCjattHW2WMEufaCnwrgd6itqO%2FVngg5R0k%3D").unwrap();
        ec.nonce_base = 0x6e55;
        ec.session_tx = Some(ec.tx);
        ec.session_rx = Some(ec.tx);
        let pt = vec![7u8; 20];
        let boxed = ec.seal_counter(&pt);
        let hint = u16::from_le_bytes([boxed[20], boxed[21]]);
        assert_eq!(hint, 0x6e55);
        let (back, counter) = ec.open_counter(&boxed).expect("base 0x6e55 must open");
        assert_eq!(back, pt);
        assert_eq!(counter, 0x6e55);
    }

    #[test]
    fn from_join_rejects_undecodable_keys() {
        assert!(EarlyCrypto::from_join("").is_none());
        // Raw percent-encoding is not base64 — must go through unquote first.
        assert!(EarlyCrypto::from_join("M17S4M%2Bk%2FCjattHW2WMEufaCnwrgd6itqO%2FVngg5R0k%3D").is_some());
        // Too short after decode.
        assert!(EarlyCrypto::from_join("AAAA").is_none());
    }
}
