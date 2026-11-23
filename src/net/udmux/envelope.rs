//! The outer datagram framing every real client uses on the wire, now
//! measured byte-for-byte on a live capture (`sober_ro.pcap`, Nov 2026,
//! ~34k-datagram session + an 8-datagram handshake dump):
//!
//! ```text
//! byte[0]      0x01        UDMUX/RUPP flag — NOT a RakNet message id
//! byte[1:3]    00 00       header length high bytes (always 0)
//! byte[3]      04|17|1f    TOTAL header length (4 = bare, prefix only)
//! byte[4:7]    01 11 <tt>  TLV-1 header: type 01 = TokenTlv, length 0x11
//!                          (17 = 1 + 16). <tt> is the TOKEN TYPE byte —
//!                          0x02 on 100% of the 34k captured datagrams
//!                          (Rupp.h: `struct TokenTlv { type[1]; value[16]; }`,
//!                          ErrorCode::INVALID_TOKEN). The old reading
//!                          ("cls 0x01 while the offline handshake is on the
//!                          wire") came from an 8-datagram dump that likely
//!                          mixed in our own probe datagrams; the OCR1 matrix
//!                          still tries 0x01 as a variant.
//! byte[7:23]   tag         the 16-byte TOKEN VALUE. In the connected phase
//!                          it is a TIME-ROTATING epoch: it changes every
//!                          wall-clock second, the proxy switches first and
//!                          the client mirrors the newest tag the server
//!                          sent (main flow: 165 distinct tags over 169 s,
//!                          transitions exactly on 1 s boundaries; the
//!                          second flow shows the client/server overlap
//!                          dance at each boundary). The rotation key
//!                          schedule is not published — until it is,
//!                          pre-connect datagrams ride the session's static
//!                          TokenValue (b64-decoded — the same material the
//!                          known UDMUX-bypass abuse is built on), with the
//!                          random client tag kept as a matrix fallback.
//!                          The server announces its own tag in every reply
//!                          header; the client adopts it by NEW_INCOMING_
//!                          CONNECTION and mirrors the rotations once
//!                          connected (see RakClient::send_raw).
//! byte[23:31]  mux         header length 0x1f only: `02 06` + the PRIVATE
//!                          RCC IPv4 (4B) + port (BE u16) straight from the
//!                          join script's ServerConnections — e.g.
//!                          02 06 0a ce 12 43 c0 63 = 10.206.18.67:49251.
//!                          This is what the proxy routes on, NOT the tag;
//!                          server replies (header len 0x17) omit it.
//! body                     RakNet datagram (plaintext magic handshake first,
//!                          then early-key/session-encrypted datagrams)
//! ```
//!
//! The server's REPLY_1 arrives with a bare `01 00 00 04` prefix (header
//! length 4, no tag/mux) — `rupp_parse` must recognize that shape or the
//! reply is misread as a raw datagram. The legacy token-derived candidates
//! below are kept as probing fallbacks; `client_envelope` is the shape the
//! live capture proves out.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::token::b64_decode;
use std::net::Ipv4Addr;

pub const RUPP_FLAG: u8 = 0x01;
pub const RUPP_SUBFLAGS: [u8; 3] = [0x01, 0x11, 0x02];

/// A datagram wrapper candidate: either the raw legacy shape
/// (`direction == 0`, nothing prepended) or a RUPP envelope header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Envelope {
	pub label: &'static str,
	/// 0 = send raw (no envelope); otherwise the header length 0x17 or 0x1f.
	pub direction: u8,
	/// TokenTlv token-type byte (byte[6]): 0x02 on every datagram the live
	/// client sent (34k-datagram pcap, both game flows, both directions).
	/// 0x01 is kept as an OCR1-matrix variant.
	pub cls: u8,
	pub epoch: [u8; 16],
	pub mux: [u8; 8],
}

/// No-envelope variant (direct dial, or a proxy that routes by 5-tuple).
pub const BARE: Envelope =
	Envelope { label: "bare", direction: 0, cls: 0x02, epoch: [0u8; 16], mux: [0u8; 8] };

/// The client envelope: header length 0x1f, token type 0x02, and the private
/// RCC `ip:port` from the join script riding in the mux field as
/// `02 06 <ip4> <port BE16>`. The proxy routes on that embedded address and
/// gates on the TokenTlv — the type-0x01 + random-tag shape the old builds
/// pinned is what the RCC silently dropped (total OCR1 silence, not even an
/// incompatible-protocol error, while the leaked server handler proves it
/// would have answered a datagram that reached it).
pub fn client_envelope(label_tag: &[u8; 16], rcc: (Ipv4Addr, u16)) -> Envelope {
	let mut mux = [0u8; 8];
	mux[0] = 0x02;
	mux[1] = 0x06;
	mux[2..6].copy_from_slice(&rcc.0.octets());
	mux[6..8].copy_from_slice(&rcc.1.to_be_bytes());
	Envelope { label: "client-2026", direction: 0x1f, cls: 0x02, epoch: *label_tag, mux }
}

/// The session token as a 16-byte tag: `TokenValue` is base64 and its first
/// 16 decoded bytes are the token the UDMUX proxy expects in the TokenTlv.
pub fn decode_token16(token: &str) -> Option<[u8; 16]> {
	let dec = b64_decode(token);
	if dec.len() < 16 {
		return None;
	}
	// `dec[..16]` is checked above; the conversion cannot fail.
	Some(dec[..16].try_into().unwrap_or([0u8; 16]))
}

/// Wrap `body` in the RUPP envelope described by `env`.
pub fn rupp_frame(env: &Envelope, body: &[u8]) -> Vec<u8> {
	let mut out = Vec::with_capacity(env.direction as usize + body.len());
	out.push(RUPP_FLAG);
	out.push(0);
	out.push(0);
	out.push(env.direction);
	out.push(RUPP_SUBFLAGS[0]);
	out.push(RUPP_SUBFLAGS[1]);
	out.push(env.cls);
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
	if p.len() < 4 || p[0] != RUPP_FLAG || p[1] != 0 || p[2] != 0 {
		return None;
	}
	let dir = p[3];
	// Header length 4 = a bare RUPP prefix with no tag/mux fields — the
	// shape REPLY_1 arrives in on the live capture. Strip just the prefix.
	if dir == 0x04 {
		return Some((4, [0u8; 16], [0u8; 8], 4));
	}
	if dir != 0x17 && dir != 0x1f {
		return None;
	}
	if p.len() < 7 || p[4..6] != [RUPP_SUBFLAGS[0], RUPP_SUBFLAGS[1]] {
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
		out.push(Envelope { label, direction, cls: 0x02, epoch, mux });
	};

	if dec.len() >= 16 {
		push_token_variants(&dec, pe, &mut push);
	}
	// Raw ASCII token (in case TokenValue is not base64 at all).
	let mut ascii = [0u8; 16];
	let n = token.len().min(16);
	ascii[..n].copy_from_slice(&token.as_bytes()[..n]);
	push("env17-ascii", 0x17, ascii, [0u8; 8]);
	// All-zero tag: proves whether the proxy routes by source 5-tuple.
	push("env17-zeros", 0x17, [0u8; 16], [0u8; 8]);
	out
}

/// The token-derived candidate shapes, split out of [`envelope_candidates`]
/// to keep that function under the line-length lint.
fn push_token_variants(
	dec: &[u8],
	pe: [u8; 4],
	push: &mut impl FnMut(&'static str, u8, [u8; 16], [u8; 8]),
) {
	// `dec.len() >= 16` is the caller's precondition.
	let tok16: [u8; 16] = dec[..16].try_into().unwrap_or([0u8; 16]);
	// The most likely shape: the decoded token IS the first epoch tag.
	push("env17-token", 0x17, tok16, [0u8; 8]);
	if dec.len() >= 18 {
		// Unpadded tokens decode to 18 bytes; maybe the first two bytes
		// are a sub-header the tag follows.
		let tok18: [u8; 16] = dec[2..18].try_into().unwrap_or([0u8; 16]);
		push("env17-token18", 0x17, tok18, [0u8; 8]);
	}
	// Direction 0x1f variants (8-byte mux field after the tag).
	push("env1f-token", 0x1f, tok16, [0u8; 8]);
	let muxtok: [u8; 8] = dec[..8].try_into().unwrap_or([0u8; 8]);
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

#[cfg(test)]
#[path = "envelope/tests.rs"]
mod tests;
