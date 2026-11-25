//! Counter-framed AEAD for everything sent after `OpenReply2` — the
//! handshake completion box (`CONNECTION_REQUEST`) and the whole connected
//! phase share one scheme. LIVE-VERIFIED (24 Nov 2026): the server accepts
//! our box and its own steady datagrams open with the same keys.
//!
//! Scheme: **AES-256-GCM**, not ChaCha20. The 12-byte nonce is
//! `LE64(base + counter) || "mbeR"`, i.e. it spells `UniqueNumbeR` for the
//! base `0x754E657571696E55` and the counter rides in the low byte. The wire
//! is `ct || counter_hint(u16 LE) || tag(16)` and the AAD is empty.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::EarlyCrypto;
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};

/// First counter value (`LE64` = ASCII `UniqueNu`); with the suffix below
/// the nonce reads `UniqueNumbeR`.
pub const STEADY_BASE: u64 = 0x754E_6575_7169_6E55;

/// Last 4 nonce bytes: `mbeR` (capital R — the capture's byte order).
const NONCE_SUFFIX: [u8; 4] = *b"mbeR";

fn nonce_bytes(counter: u64) -> [u8; 12] {
	let mut n = [0u8; 12];
	n[..8].copy_from_slice(&counter.to_le_bytes());
	n[8..].copy_from_slice(&NONCE_SUFFIX);
	n
}

/// Seal one steady datagram body: `ct || counter_hint(u16 LE) || tag(16)`.
// A few hundred bytes of plaintext, well under AES-GCM's limit.
#[allow(clippy::expect_used)]
pub fn seal_counter(key32: &[u8; 32], counter: u64, pt: &[u8]) -> Vec<u8> {
	let cipher = Aes256Gcm::new_from_slice(key32).expect("aes-256-gcm key");
	let blob = cipher
		.encrypt(Nonce::from_slice(&nonce_bytes(counter)), Payload { msg: pt, aad: b"" })
		.expect("aes-256-gcm encrypt");
	let (ct, tag) = blob.split_at(blob.len() - 16);
	let mut out = Vec::with_capacity(ct.len() + 18);
	out.extend_from_slice(ct);
	out.extend_from_slice(&(counter as u16).to_le_bytes()); // hint
	out.extend_from_slice(tag);
	out
}

/// Open a counter-framed datagram. The wire carries only the low 16 bits of
/// the counter, so scan a window of high bits around `base`.
#[allow(clippy::expect_used)]
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
	let cipher = Aes256Gcm::new_from_slice(key32).expect("aes-256-gcm key");
	let base_lo = (base & 0xFFFF) as u16;
	let delta = hint.wrapping_sub(base_lo) as u64;
	let mut data = Vec::with_capacity(ct.len() + 16);
	data.extend_from_slice(ct);
	data.extend_from_slice(tag);
	for k in 0..window.max(1) {
		let val = base.wrapping_add(delta).wrapping_add(k.wrapping_mul(0x1_0000));
		if let Ok(pt) =
			cipher.decrypt(Nonce::from_slice(&nonce_bytes(val)), Payload { msg: &data, aad: b"" })
		{
			return Some((pt, val));
		}
	}
	None
}

impl EarlyCrypto {
	/// Seal ANY post-OpenReply2 datagram (the handshake completion box AND
	/// the connected phase — one scheme): `ct || hint(u16 LE) || tag(16)`,
	/// nonce = `LE64(base + counter) || "mbeR"`, AAD empty. The counter
	/// increments per datagram we send.
	pub fn seal_counter(&mut self, pt: &[u8]) -> Vec<u8> {
		let key = self.session_tx.unwrap_or(self.tx);
		let counter = self.nonce_base.wrapping_add(self.send_counter);
		self.send_counter += 1;
		seal_counter(&key, counter, pt)
	}

	/// Open any post-OpenReply2 datagram: counter-hint scheme, scanning the
	/// high counter bits around the base (window 16 — the hints stay near
	/// the base in a healthy session).
	pub fn open_counter(&self, wire: &[u8]) -> Option<(Vec<u8>, u64)> {
		let mut attempts: Vec<&[u8; 32]> = Vec::with_capacity(2);
		if let Some(sk) = &self.session_rx {
			attempts.push(sk);
		}
		attempts.push(&self.rx);
		for key in attempts {
			if let Some(hit) = open_counter(key, wire, self.nonce_base, 16) {
				return Some(hit);
			}
		}
		None
	}
}

#[cfg(test)]
#[path = "counter/tests.rs"]
mod tests;
