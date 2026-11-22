//! Early encryption — the fork's crypto handshake, LIVE-VERIFIED
//! 2026-11-17 (fresh join, probe7.py: our own OCR2 was accepted and the
//! server answered with a 158-byte OpenReply2 in 74 ms; the engine's
//! captures match the same layout byte-for-byte).
//!
//! ```text
//! OCR2 (client -> server, msg 211 bytes, zero-padded to 1129 inside the
//!       1160-byte RUPP datagram):
//!   78 | MAGIC16 | 03 5a 00 5d 00 05 | client_pub(32) | auth_ver(1) |
//!   21 | paBlob(33) || ct(93) | "UniqueNumber" | tag(16)
//!
//!   AAD = msg[0..90) (everything through paBlob); the box is
//!   ChaCha20Poly1305(tx, nonce=b"UniqueNumber", aad) over the 93-byte
//!   plaintext pt93; on the wire the ct rides BEFORE the nonce and the
//!   tag again after it (ct || nonce || tag — NOT nonce || ct || tag).
//!   auth_ver comes from ClientTicket part4 ("18" -> 0x12); the declared
//!   `00 5d` (93) is the PT length.
//!
//!   pt93 = const(8) + urandom(8) + mtu BE16 (from REPLY_1) + 0x02 +
//!   urandom(74) — the tail is session entropy the server does not
//!   verify (a random 74-byte tail was accepted live).
//!
//! OpenReply2 (server -> client, body 135):
//!   7d | MAGIC16 | 01 2c 00 3a | server_pub(32) @21 | box(54) @53..107
//!   | "UniqueNumber" @107 | 16B tail. The header is 4 bytes — verified
//!   on 2 fresh live replies + the engine's (no trailing 0x03).
//!
//! post-reply2 datagram (client -> server, 52 total):
//!   envelope(31) | nonce(12) | ct(24) | tag(16)   <- plaintext = the
//!   vanilla 24-byte CONNECTION_REQUEST, so the AEAD overhead is exactly
//!   nonce(12)+tag(16) = 28 (the libroblox preauth-window code computes the
//!   same `payload + 0x1c` expected size).
//! ```
//!
//! Key schedule (libsodium `crypto_kx` semantics):
//! `H = BLAKE2b-512(x25519(client_sk, server_pk) || client_pk || server_pk)`,
//! client sends with `H[32:]` (tx), receives with `H[:32]` (rx). The OCR2
//! box uses the ephemeral join key (`EphemeralEarlyPubKey`); OpenReply2
//! announces a second server key, and the post-reply2 boxes use those
//! session keys.
//!
//! The 33-byte paBlob is the join call's own `ClientTicket` part2
//! (b64) — it differs per join call, so it is threaded in from the join
//! response, not carried as a constant.

use aes_gcm::Aes256Gcm;
use blake2::digest::{Update, VariableOutput};
use blake2::Blake2bVar;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use std::net::Ipv4Addr;
use x25519_dalek::{PublicKey as XPublic, StaticSecret as XSecret};

/// RakNet ids of the crypto handshake (fork numbering, from the dump).
pub const OCR2_CRYPTO_ID: u8 = 0x78;
pub const OPEN_REPLY2_ID: u8 = 0x7d;

/// Fork OCR2 header constants, byte-verified against the live captures:
/// `03 5a 00 5d 00 05` = ver 3, aad 90, pt 93, supportedVersion 5.
pub const OCR2_VER: u8 = 0x03;
pub const OCR2_AAD: u8 = 0x5a; // 90 = id+magic+6+pub32+auth_ver+pa_len+pa33
pub const OCR2_BOX: u8 = 0x5d; // 93 = the declared PT length (pt93)
pub const OCR2_SV: u16 = 5;
/// OCR2 authVer: the join call's ClientTicket part4 is the ASCII string
/// "18" — 0x12 on the wire (verified 2026-11-17; the older 0x11 came from
/// a stale Nov-12 transcription). The parser overrides per join call.
pub const OCR2_AUTH_VER: u8 = 0x12; // 18
/// OpenReply2 header `01 2c 00 3a` = ver 1, aad 44, box 58 (declared; the
/// wire box is 54 and there is NO trailing byte — the header is exactly
/// 4 bytes, verified on 3 live replies).
pub const REPLY2_VER: u8 = 0x01;
pub const REPLY2_AAD: u8 = 0x2c; // 44
pub const REPLY2_BOX: u8 = 0x3a; // 58 (declared)

/// Sizes measured on the wire.
pub const OCR2_PUB: usize = 32;
pub const OCR2_PA_LEN: usize = 33; // the wire carries 0x21 = 33
pub const OCR2_CT_LEN: usize = 93; // ct rides first: aad(90) + ct(93)
/// pt93 layout (decrypted from 2 engine sessions, 2026-11-16/17):
/// `[0:8]=PT93_CONST  [8:16]=urandom(8)  [16:18]=mtu BE16  [18]=0x02
/// [19:93]=urandom(74)`.
pub const OCR2_PT_LEN: usize = 93;
/// pt93 [0:8] — constant across engine sessions (2026-11-16).
pub const PT93_CONST: [u8; 8] = [0x00, 0x0d, 0xdb, 0xc7, 0x50, 0xb3, 0x08, 0x92];
pub const UNIQUE_NAME: &[u8; 12] = b"UniqueNumber";
pub const UNIQUE_LEN: usize = 16;
pub const CONNECTION_REQUEST_PT: usize = 24; // id+guid8+time8+useSec1+password6
pub const BOX_OVERHEAD: usize = 12 + 16; // nonce + tag — the libroblox +0x1c

/// `n` random bytes (/dev/urandom — desktop and Android both have it; a
/// blocking read never fails in practice for small sizes).

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
	let cipher = Aes256Gcm::new(aes_gcm::Key::<Aes256Gcm>::from_slice(key32));
	let box_out = cipher
		.encrypt(aes_gcm::Nonce::from_slice(&nonce_bytes(counter)), Payload { msg: pt, aad: b"" })
		.expect("aes-256-gcm encrypt");
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
	let cipher = Aes256Gcm::new(aes_gcm::Key::<Aes256Gcm>::from_slice(key32));
	let base_lo = (base & 0xFFFF) as u16;
	let diff = hint.wrapping_sub(base_lo) as u64;
	let mut data = Vec::with_capacity(ct.len() + 16);
	data.extend_from_slice(ct);
	data.extend_from_slice(tag);
	for k in 0..=window {
		let val = base.wrapping_add(diff).wrapping_add(k.wrapping_mul(0x10000));
		if let Ok(pt) = cipher.decrypt(
			aes_gcm::Nonce::from_slice(&nonce_bytes(val)),
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
		if let Ok(pt) = cipher.decrypt(Nonce::from_slice(nonce), Payload { msg: ct, aad }) {
			return Some((pt, i));
		}
	}
	None
}

/// 32 random bytes for key generation.
fn random32() -> [u8; 32] {
	let mut b = [0u8; 32];
	b.copy_from_slice(&random_bytes(32));
	b
}

fn random_bytes(n: usize) -> Vec<u8> {
	let mut out = vec![0u8; n];
	if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
		use std::io::Read;
		let _ = f.read_exact(&mut out);
	} else {
		// Fallback: entropy from the clock, mixed like the pre-connect tag.
		let t = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_nanos();
		for (i, chunk) in out.chunks_mut(8).enumerate() {
			let z = t.wrapping_mul(u128::from(0x9e37_79b9_7f4a_7c15u64.wrapping_add(i as u64 * 7)));
			chunk.copy_from_slice(&z.to_le_bytes()[..chunk.len()]);
		}
	}
	out
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
	/// The join call's own ClientTicket part2 (b64-decoded, 33 bytes) —
	/// the paBlob. DIFFERS per join call (proven live: two calls to the
	/// same job produce different blobs), so it must come from the same
	/// join response that handed out the RCC address and EphemeralEarlyPubKey.
	pub pa_blob: Vec<u8>,
	/// OCR2 authVer byte (ClientTicket part4, "18" -> 0x12).
	pub auth_ver: u8,
	/// The client's 16-byte UniqueNumber value.
	pub unique: [u8; 16],
	/// Post-OpenReply2 nonce counter base (`RUSTBLOX_NONCE_BASE`). The dump's
	/// session starts at 0x6e55 on both sides — the base rides inside
	/// encrypted setup material; default 0 until it is decoded.
	pub nonce_base: u64,
	/// Outgoing counter (base + increments, one per datagram we seal).
	pub send_counter: u64,
}

impl EarlyCrypto {
	/// Build from the join script's percent-encoded `EphemeralEarlyPubKey`,
	/// the join call's ClientTicket paBlob (part2, 33 bytes) and its part4
	/// authVer byte. `None` when the key is missing or does not decode to
	/// 32 bytes — the caller then runs the legacy handshake.
	pub fn from_join(eepk_encoded: &str, pa_blob: Vec<u8>, auth_ver: u8) -> Option<EarlyCrypto> {
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
		Some(EarlyCrypto::new(k, pa_blob, auth_ver))
	}

	/// Build from the join script's percent-encoded `EphemeralEarlyPubKey`
	/// (32 raw bytes after decoding).
	pub fn new(server_ephemeral: [u8; 32], pa_blob: Vec<u8>, auth_ver: u8) -> Self {
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
			let z = t.wrapping_mul(u128::from(0x9e37_79b9_7f4a_7c15u64.wrapping_add(i as u64 * 7)));
			chunk.copy_from_slice(&z.to_le_bytes()[..chunk.len()]);
		}
		let nonce_base = std::env::var("RUSTBLOX_NONCE_BASE")
			.ok()
			.and_then(|s| u64::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok())
			.unwrap_or(0);
		EarlyCrypto {
			server_ephemeral,
			client_sk,
			client_pk: client_pk_arr,
			tx,
			rx,
			session_tx: None,
			session_rx: None,
			pa_blob,
			auth_ver,
			unique,
			nonce_base,
			send_counter: 0,
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
	/// `(aad, full_body)` pair — the AAD is the clear `78..paBlob` prefix
	/// the box authenticates. LIVE-VERIFIED 2026-11-17: this exact wire
	/// was accepted and answered with a crypto OpenReply2 in 74 ms.
	///
	/// Wire = aad(90) || ct(93) || "UniqueNumber" || tag(16): the AEAD
	/// ciphertext rides BEFORE the 12-byte nonce and the tag is appended
	/// after it. The nonce is the fixed ASCII `"UniqueNumber"`, the
	/// plaintext is the 93-byte pt93, the key is the early `tx`, the AAD
	/// is the clear prefix.
	pub fn build_ocr2(&mut self, mtu: u16) -> Option<(Vec<u8>, Vec<u8>)> {
		if self.pa_blob.len() != OCR2_PA_LEN {
			return None;
		}
		let mut aad = Vec::with_capacity(OCR2_AAD as usize);
		aad.push(OCR2_CRYPTO_ID);
		aad.extend_from_slice(&crate::raknet::OFFLINE_MAGIC);
		aad.extend_from_slice(&[OCR2_VER, OCR2_AAD, 0x00, OCR2_BOX]);
		aad.extend_from_slice(&OCR2_SV.to_be_bytes());
		aad.extend_from_slice(&self.client_pk);
		aad.push(self.auth_ver);
		aad.push(self.pa_blob.len() as u8);
		aad.extend_from_slice(&self.pa_blob);
		debug_assert_eq!(aad.len(), OCR2_AAD as usize, "OCR2 AAD must be 90");
		// pt93 = const(8) + urandom(8) + mtu BE16 (from REPLY_1) + 0x02 +
		// urandom(74). The engine's decrypted layout; the 74-byte tail is
		// session entropy the server does not verify.
		let mut pt = Vec::with_capacity(OCR2_PT_LEN);
		pt.extend_from_slice(&PT93_CONST);
		pt.extend_from_slice(&random_bytes(8));
		pt.extend_from_slice(&mtu.to_be_bytes());
		pt.push(0x02);
		pt.extend_from_slice(&random_bytes(74));
		debug_assert_eq!(pt.len(), OCR2_PT_LEN, "pt93 must be 93");
		let cipher = ChaCha20Poly1305::new(Key::from_slice(&self.tx));
		// ct = 93B ciphertext || 16B tag = 109 bytes.
		let ct = cipher
			.encrypt(Nonce::from_slice(UNIQUE_NAME), Payload { msg: &pt, aad: &aad })
			.expect("chacha20poly1305 encrypt (valid key, 93-byte pt)");
		let mut body = aad.clone();
		body.extend_from_slice(&ct[..OCR2_CT_LEN]); // ct93  [90..183)
		body.extend_from_slice(UNIQUE_NAME); // nonce12 [183..195)
		body.extend_from_slice(&ct[OCR2_CT_LEN..]); // tag16 [195..211)
		Some((aad, body))
	}

	/// Parse the crypto OpenReply2: extracts the server's session key and
	/// adopts the session keys. Returns `(server_pub, box_wire, unique)`.
	pub fn parse_reply2(body: &[u8]) -> Option<([u8; 32], &[u8], [u8; 16])> {
		if body.len() < 21 + OCR2_PUB + UNIQUE_NAME.len() + UNIQUE_LEN {
			return None;
		}
		if body[0] != OPEN_REPLY2_ID || body[1..17] != crate::raknet::OFFLINE_MAGIC[..] {
			return None;
		}
		// Header `01 2c 00 3a` — ver 1, aad 44, box 58 (declared). The
		// header is EXACTLY 4 bytes (verified on 2 fresh live replies + the
		// engine's capture, 2026-11-17): the server pub starts at byte 21
		// and there is no trailing byte. The older `pub_off = 22` with a
		// `REPLY2_TAIL 0x03` came from an off-by-one Nov-12 transcription —
		// at a 5-byte header byte 21 would be constant 0x03, but the live
		// captures carry 0x89 (ours) and 0x45 (engine) there instead.
		if body[17..21] != [REPLY2_VER, REPLY2_AAD, 0x00, REPLY2_BOX] {
			return None;
		}
		let pub_off = 21; // id(1) + magic(16) + hdr(4)
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
	pub fn open_handshake_box(
		&self,
		box_wire: &[u8],
		envelope_aad: Option<&[u8]>,
	) -> Option<Vec<u8>> {
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
	let app = v.get("applications")?.get("RakNetEarlyPublicKey")?.clone();
	let mut versions = Vec::new();
	if let Some(arr) = app.get("versions").and_then(|a| a.as_array()) {
		for ver in arr {
			let id = ver.get("id").and_then(|x| x.as_u64()).unwrap_or(0);
			let b64 = ver.get("value").and_then(|x| x.as_str()).unwrap_or("");
			let key = crate::roblox::udmux::b64_decode(b64);
			if key.is_empty() {
				continue;
			}
			let allowed = ver.get("allowed").and_then(|x| x.as_bool()).unwrap_or(true);
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

	/// The live-verified OpenReply2 server pub (our 2026-11-17 fresh join,
	/// `probe_full_reply_158.bin` body[21..53)).
	const LIVE_REPLY2_SERVER_PUB: [u8; 32] = [
		0x89, 0x63, 0x31, 0x0f, 0x91, 0x95, 0xe0, 0xf0, 0xf0, 0x6c, 0xfc, 0xe9, 0xcb, 0xc0, 0x93,
		0xda, 0x4e, 0xc3, 0x75, 0xd2, 0xb9, 0xe5, 0xb4, 0x4a, 0x5c, 0xe8, 0xa1, 0x6e, 0xa6, 0x9f,
		0x9f, 0x19,
	];
	/// The same reply's 16-byte UniqueNumber tail (body[119..135)).
	const LIVE_REPLY2_UNIQUE: [u8; 16] = [
		0x1e, 0xc3, 0x0d, 0x06, 0x08, 0xfa, 0xeb, 0x46, 0xc0, 0x9a, 0x0e, 0xc2, 0xfa, 0x4a, 0x7e,
		0xd2,
	];

	/// The live-verified OpenReply2 body shape: 4-byte header, pub at 21,
	/// 54-byte box, "UniqueNumber" + 16B tail — 135 bytes total.
	fn live_reply2_body() -> Vec<u8> {
		let mut b = vec![OPEN_REPLY2_ID];
		b.extend_from_slice(&crate::raknet::OFFLINE_MAGIC);
		b.extend_from_slice(&[0x01, 0x2c, 0x00, 0x3a]);
		b.extend_from_slice(&LIVE_REPLY2_SERVER_PUB);
		b.extend_from_slice(&[0u8; 54]); // box — opaque to the parser
		b.extend_from_slice(UNIQUE_NAME);
		b.extend_from_slice(&LIVE_REPLY2_UNIQUE);
		b
	}

	#[test]
	fn ocr2_wire_matches_the_verified_layout() {
		let mut ec = EarlyCrypto::from_join(
			"M17S4M%2Bk%2FCjattHW2WMEufaCnwrgd6itqO%2FVngg5R0k%3D",
			vec![0x42u8; 33],
			0x12,
		)
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
		// AAD = id + magic + 03 5a 00 5d 00 05 + pub32 + auth_ver + pa_len + pa33.
		let mut want = vec![OCR2_CRYPTO_ID];
		want.extend_from_slice(&crate::raknet::OFFLINE_MAGIC);
		want.extend_from_slice(&[0x03, 0x5a, 0x00, 0x5d, 0x00, 0x05]);
		want.extend_from_slice(&ec.client_pk);
		want.push(0x12); // auth_ver — the engine's wire byte (ClientTicket part4 "18")
		want.push(0x21);
		want.extend_from_slice(&[0x42u8; 33]);
		assert_eq!(want.len(), 90);
		let (aad, body) = ec.build_ocr2(1200).expect("33-byte paBlob must build");
		// The 32 bytes at [23..55] are the FRESH client pub (regenerated per
		// run), so compare the fixed structure around it — the layout is the
		// live capture's, byte-for-byte: id + magic + `03 5a 00 5d 00 05` +
		// pub(32) + `12 21` + paBlob(33) = 90.
		assert_eq!(aad.len(), 90);
		assert_eq!(&aad[..23], &want[..23]);
		assert_eq!(&aad[55..], &want[55..]);
		// Body = aad(90) + ct(93) + "UniqueNumber"(12) + tag(16) = 211 —
		// the ct rides BEFORE the nonce, no unique tail (live-verified).
		assert_eq!(&body[..23], &want[..23]);
		assert_eq!(&body[55..90], &want[55..]);
		assert_eq!(body.len(), 211);
		assert_eq!(&body[183..195], UNIQUE_NAME);
		// A short paBlob must refuse to build (the server verifies it).
		let mut bad = EarlyCrypto::new(ec.server_ephemeral, Vec::new(), 0x12);
		assert!(bad.build_ocr2(1200).is_none());
	}

	#[test]
	fn reply2_parse_extracts_the_verified_fields() {
		let body = live_reply2_body();
		assert_eq!(body.len(), 135, "the live OpenReply2 body is 135 bytes");
		let (sp, box_wire, un) = EarlyCrypto::parse_reply2(&body).expect("live shape must parse");
		assert_eq!(sp, LIVE_REPLY2_SERVER_PUB);
		assert_eq!(un, LIVE_REPLY2_UNIQUE);
		assert_eq!(box_wire.len(), 54, "the server box on the wire is 54 bytes");
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
		let mut ec = EarlyCrypto::from_join(
			"M17S4M%2Bk%2FCjattHW2WMEufaCnwrgd6itqO%2FVngg5R0k%3D",
			Vec::new(),
			OCR2_AUTH_VER,
		)
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
	fn counter_nonce_uses_the_mber_suffix() {
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
		let mut ec = EarlyCrypto::from_join(
			"M17S4M%2Bk%2FCjattHW2WMEufaCnwrgd6itqO%2FVngg5R0k%3D",
			Vec::new(),
			OCR2_AUTH_VER,
		)
		.unwrap();
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
		assert!(EarlyCrypto::from_join("", Vec::new(), OCR2_AUTH_VER).is_none());
		// Raw percent-encoding is not base64 — must go through unquote first.
		assert!(EarlyCrypto::from_join(
			"M17S4M%2Bk%2FCjattHW2WMEufaCnwrgd6itqO%2FVngg5R0k%3D",
			Vec::new(),
			OCR2_AUTH_VER
		)
		.is_some());
		// Too short after decode.
		assert!(EarlyCrypto::from_join("AAAA", Vec::new(), OCR2_AUTH_VER).is_none());
	}
}
