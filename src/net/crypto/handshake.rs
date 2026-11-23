//! The OCR2 crypto build and the OpenReply2 parse — the two wire shapes
//! that carry the handshake's own AEAD boxes, separate from the
//! counter-framed scheme used afterward (see [`super::counter`]).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::{
	open, EarlyCrypto, OCR2_AAD, OCR2_BOX, OCR2_CRYPTO_ID, OCR2_CT_LEN, OCR2_PA_LEN, OCR2_PT_LEN,
	OCR2_PUB, OCR2_SV, OCR2_VER, OPEN_REPLY2_ID, PT93_CONST, REPLY2_AAD, REPLY2_BOX, REPLY2_VER,
	UNIQUE_LEN, UNIQUE_NAME,
};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};

/// Fields extracted from a parsed OpenReply2 body by
/// [`EarlyCrypto::parse_reply2`].
pub struct Reply2Fields<'a> {
	/// The server's ephemeral X25519 public key for this reply.
	pub server_pub: [u8; 32],
	/// The still-sealed AEAD box (opened separately, key schedule pending).
	pub box_wire: &'a [u8],
	/// The server's 16-byte `UniqueNumber` tail.
	pub unique: [u8; 16],
}

impl EarlyCrypto {
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
	///
	/// Returns `None` when `pa_blob` is not exactly [`OCR2_PA_LEN`] bytes
	/// — the server verifies it against the join call.
	// The one `expect()` below encrypts a fixed 93-byte pt93 with a 32-byte
	// key and a 12-byte nonce — well inside ChaCha20Poly1305's limits, so it
	// cannot fail.
	#[allow(clippy::expect_used)]
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
		pt.extend_from_slice(&super::random_bytes(8));
		pt.extend_from_slice(&mtu.to_be_bytes());
		pt.push(0x02);
		pt.extend_from_slice(&super::random_bytes(74));
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

	/// Parse the crypto OpenReply2: extracts the fields needed to adopt the
	/// session keys (the caller still calls [`EarlyCrypto::adopt_reply2_key`]).
	pub fn parse_reply2(body: &[u8]) -> Option<Reply2Fields<'_>> {
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
		let mut unique = [0u8; 16];
		unique.copy_from_slice(&tail[12..]);
		let box_wire = &body[pub_off + 32..body.len() - UNIQUE_NAME.len() - UNIQUE_LEN];
		Some(Reply2Fields { server_pub: sp, box_wire, unique })
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
