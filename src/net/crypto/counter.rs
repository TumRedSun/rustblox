//! Counter-framed AEAD for everything sent after `OpenReply2` — the
//! handshake completion boxes (`CONNECTION_REQUEST`, ...) and the whole
//! connected phase share one scheme, proven by the sequential counter
//! hints in the live capture.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::EarlyCrypto;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};

/// The nonce suffix of every post-OpenReply2 datagram: `nonce = u64 LE
/// counter || "mber"`. Verified byte-exact against the live engine's own
/// code (Ghidra decompile of the real `FUN_041c2570`/nonce-builder function,
/// Nov 2026): it writes the 32-bit immediate `0x7265626d`, which as little-
/// endian bytes is `6d 62 65 72` = ASCII `"mber"` — lowercase `r`, not the
/// capitalized `"mbeR"` an earlier session assumed from a capture-only
/// read. See docs/ARCHITECTURE.md history / todo.md for the full trace.
const NONCE_SUFFIX: [u8; 4] = *b"mber";

fn nonce_bytes(counter: u64) -> [u8; 12] {
	let mut n = [0u8; 12];
	n[..8].copy_from_slice(&counter.to_le_bytes());
	n[8..].copy_from_slice(&NONCE_SUFFIX);
	n
}

/// Post-OpenReply2 framing (dump pkt4..7 + the whole pcap, proven by the
/// sequential counter hints): wire = `ct || counter_hint(u16 LE) || tag(16)`,
/// nonce = `u64 LE counter || "mber"`, AAD EMPTY. The hint is the low 16 bits
/// of the counter; the high bits come from the negotiated base (session
/// setup). The dump's first post-OR2 counter is 0x6e55 on BOTH sides — the
/// base is not zero and rides inside encrypted session material we cannot
/// read offline, so it is configurable (`RUSTBLOX_NONCE_BASE`) and the
/// opener scans a window of high bits.
// A few hundred bytes of plaintext, well under ChaCha20Poly1305's limit —
// the encrypt call cannot fail.
#[allow(clippy::expect_used)]
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
		if let Ok(pt) =
			cipher.decrypt(Nonce::from_slice(&nonce_bytes(val)), Payload { msg: &data, aad: b"" })
		{
			return Some((pt, val));
		}
	}
	None
}

impl EarlyCrypto {
	/// Seal ANY post-OpenReply2 datagram (handshake boxes pkt4..7 AND the
	/// connected phase — one scheme, proven by the counter hints):
	/// `ct || hint(u16 LE) || tag(16)`, nonce = `counter u64 LE || "mber"`,
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

#[cfg(test)]
#[path = "counter/tests.rs"]
mod tests;
