//! The OCR2 crypto build and the OpenReply2 parse — the two wire shapes
//! that carry the handshake's own AEAD boxes, separate from the
//! counter-framed scheme used afterward (see [`super::counter`]).
//!
//! Both layouts are LIVE-VERIFIED (24 Nov 2026): the standalone client
//! completes the handshake against real RCCs, the OpenReply2 box opens with
//! the SESSDER#1 server->client key, and the recovered 32-byte peer yields
//! session keys that decrypt the server's real post-reply frames.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::{
	open, EarlyCrypto, OCR2_AAD, OCR2_BOX, OCR2_CRYPTO_ID, OCR2_CT_LEN, OCR2_PA_LEN, OCR2_PT_LEN,
	OCR2_SV, OCR2_VER, OPEN_REPLY2_ID, PT93_CONST, REPLY2_AAD, REPLY2_BOX, REPLY2_VER, UNIQUE_NAME,
};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};

/// Fields of a crypto OpenReply2 body (the bytes after the 23-byte RUPP
/// server prefix), split exactly as the wire carries them.
///
/// ```text
/// body[0]      7d
/// body[1:17]   offline magic
/// body[17:21]  01 2c 00 3a   (ver 1, aad 44, box 58 declared)
/// body[21:44]  first 23 bytes of the SESSDER#2 peer key (clear)
/// body[44:..]  ct | "UniqueNumber" (12) | tag (16)
/// ```
/// The remaining 9 bytes of the 32-byte peer key sit at the start of the
/// box plaintext — only the server can produce them, which is what binds
/// the box to this session.
pub struct Reply2Fields<'a> {
	/// `body[21..44]` — the first 23 bytes of the SESSDER#2 peer key.
	pub clear23: [u8; 23],
	/// The 44-byte AAD (`7d | magic | 01 2c 00 3a | clear23`).
	pub aad: &'a [u8],
	/// The sealed ciphertext (63 bytes on the wire).
	pub ct: &'a [u8],
	/// The nonce — always the ASCII `"UniqueNumber"`.
	pub nonce: &'a [u8],
	/// The AEAD tag (16 bytes).
	pub tag: &'a [u8],
}

impl EarlyCrypto {
	/// Build the crypto OCR2 body (before envelope framing). Returns the
	/// `(aad, full_body)` pair — the AAD is the clear `78..paBlob` prefix
	/// the box authenticates.
	///
	/// Wire = aad(90) || ct(93) || "UniqueNumber" || tag(16): the AEAD
	/// ciphertext rides BEFORE the 12-byte nonce and the tag is appended
	/// after it. The nonce is the fixed ASCII `"UniqueNumber"`, the
	/// plaintext is the 93-byte pt93, the key is the early `tx`, the AAD
	/// is the clear prefix.
	///
	/// Returns `None` when `pa_blob` is not exactly [`OCR2_PA_LEN`] bytes
	/// — the server verifies it against the join call.
	// The one `expect()` below encrypts a fixed 93-byte pt93 with a 32-byte
	// key and a 12-byte nonce — well inside ChaCha20Poly1305's limits, so it
	// cannot fail.
	#[allow(clippy::expect_used)]
	pub fn build_ocr2(&mut self, mtu: u16, rcc_port: u16) -> Option<(Vec<u8>, Vec<u8>)> {
		if self.pa_blob.len() != OCR2_PA_LEN {
			return None;
		}
		let mut aad = Vec::with_capacity(OCR2_AAD as usize);
		aad.push(OCR2_CRYPTO_ID);
		aad.extend_from_slice(&crate::net::raknet::OFFLINE_MAGIC);
		aad.extend_from_slice(&[OCR2_VER, OCR2_AAD, 0x00, OCR2_BOX]);
		aad.extend_from_slice(&OCR2_SV.to_be_bytes());
		aad.extend_from_slice(&self.client_pk);
		aad.push(self.auth_ver);
		aad.push(self.pa_blob.len() as u8);
		aad.extend_from_slice(&self.pa_blob);
		debug_assert_eq!(aad.len(), OCR2_AAD as usize, "OCR2 AAD must be 90");
		// pt93 layout, byte-verified against three engine sessions and
		// accepted live: const(8) | session id(8) | mtu BE16 | 02 04 |
		// 7f 8b | random(2) | RCC port BE16 | caps(67 = "42 06" + 65
		// random). The server does not verify the caps tail.
		let mut pt = Vec::with_capacity(OCR2_PT_LEN);
		pt.extend_from_slice(&PT93_CONST);
		pt.extend_from_slice(&self.session_id);
		pt.extend_from_slice(&mtu.to_be_bytes());
		pt.extend_from_slice(&[0x02, 0x04, 0x7f, 0x8b]);
		pt.extend_from_slice(&super::random_bytes(2));
		pt.extend_from_slice(&rcc_port.to_be_bytes());
		pt.extend_from_slice(&[0x42, 0x06]);
		pt.extend_from_slice(&super::random_bytes(65));
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

	/// Split a crypto OpenReply2 body (magic already stripped of the RUPP
	/// prefix) into its clear/signed parts.
	pub fn parse_reply2(body: &[u8]) -> Option<Reply2Fields<'_>> {
		// id(1) + magic(16) + hdr(4) + clear23 + ct(>=9) + nonce(12) + tag(16)
		const PUB_OFF: usize = 21;
		const AAD_LEN: usize = 44;
		if body.len() < AAD_LEN + 9 + UNIQUE_NAME.len() + 16 {
			return None;
		}
		if body[0] != OPEN_REPLY2_ID || body[1..17] != crate::net::raknet::OFFLINE_MAGIC[..] {
			return None;
		}
		if body[17..21] != [REPLY2_VER, REPLY2_AAD, 0x00, REPLY2_BOX] {
			return None;
		}
		let mut clear23 = [0u8; 23];
		clear23.copy_from_slice(&body[PUB_OFF..PUB_OFF + 23]);
		let aad = &body[..AAD_LEN];
		let rest = &body[AAD_LEN..];
		if rest.len() < UNIQUE_NAME.len() + 16 {
			return None;
		}
		let ct = &rest[..rest.len() - UNIQUE_NAME.len() - 16];
		let nonce = &rest[rest.len() - UNIQUE_NAME.len() - 16..rest.len() - 16];
		let tag = &rest[rest.len() - 16..];
		if nonce != UNIQUE_NAME {
			return None;
		}
		Some(Reply2Fields { clear23, aad, ct, nonce, tag })
	}

	/// Open the OpenReply2 box with the early server->client key and return
	/// the complete 32-byte SESSDER#2 peer: the 23 clear bytes plus the
	/// 9 leading plaintext bytes that ride inside the box.
	pub fn reply2_seed2(&self, body: &[u8]) -> Option<[u8; 32]> {
		let f = Self::parse_reply2(body)?;
		let mut boxed = Vec::with_capacity(f.nonce.len() + f.ct.len() + f.tag.len());
		boxed.extend_from_slice(f.nonce);
		boxed.extend_from_slice(f.ct);
		boxed.extend_from_slice(f.tag);
		let (pt, _) = open(&self.rx, &boxed, &[f.aad])?;
		if pt.len() < 9 {
			return None;
		}
		let mut seed = [0u8; 32];
		seed[..23].copy_from_slice(&f.clear23);
		seed[23..].copy_from_slice(&pt[..9]);
		Some(seed)
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
}

#[cfg(test)]
#[path = "handshake/tests.rs"]
mod tests;
