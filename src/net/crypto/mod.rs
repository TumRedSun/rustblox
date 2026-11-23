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
//!
//! The counter-framed post-handshake AEAD (`seal_counter`/`open_counter`)
//! lives in [`counter`]; the join-script keyring parser lives in
//! [`keyring`].

// New module under the ARCHITECTURE.md migration policy: unwrap/expect/panic
// are denied by default; the few infallible-by-construction `expect()` calls
// below are each locally re-allowed with a comment explaining why they can't
// actually fail (`forbid` is not used because it can't be locally overridden
// even for a documented, reviewed exception).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod counter;
pub mod handshake;
pub mod keyring;

pub use handshake::Reply2Fields;

pub use keyring::{parse_keyring, KeyringVersion};

use blake2::digest::{Update, VariableOutput};
use blake2::Blake2bVar;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
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

/// libsodium `crypto_kx` client key schedule.
/// `H = BLAKE2b-512(shared || client_pk || server_pk)`; tx = `H[32:]`, rx = `H[:32]`.
// `Blake2bVar::new` only errors for an out-of-range output size, and
// `finalize_variable` only errors for a buffer shorter than that size — both
// are fixed to 64 here, so neither can actually fail.
#[allow(clippy::expect_used)]
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
// ChaCha20Poly1305 only rejects a plaintext over ~256 GiB; every caller here
// passes a few hundred bytes at most, so the encrypt call cannot fail.
#[allow(clippy::expect_used)]
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

/// Open a box produced by [`seal`]. `aads` are tried in order; returns
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

/// `n` random bytes (`/dev/urandom` — desktop and Android both have it; a
/// blocking read never fails in practice for small sizes). Falls back to a
/// clock-mixed value if the device is unreadable.
pub(super) fn random_bytes(n: usize) -> Vec<u8> {
	let mut out = vec![0u8; n];
	if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
		use std::io::Read;
		let _ = f.read_exact(&mut out);
	} else {
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
		let decoded = crate::net::udmux::percent_decode(eepk_encoded);
		let key = crate::net::udmux::b64_decode(&decoded);
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
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
