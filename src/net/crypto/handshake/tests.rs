//! Tests transcribed from the Nov-12/17 live captures, not invented.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::super::{EarlyCrypto, OCR2_CRYPTO_ID, UNIQUE_NAME};
use super::Reply2Fields;

/// The live-verified OpenReply2 server pub (our 2026-11-17 fresh join,
/// `probe_full_reply_158.bin` body[21..53)).
const LIVE_REPLY2_SERVER_PUB: [u8; 32] = [
	0x89, 0x63, 0x31, 0x0f, 0x91, 0x95, 0xe0, 0xf0, 0xf0, 0x6c, 0xfc, 0xe9, 0xcb, 0xc0, 0x93, 0xda,
	0x4e, 0xc3, 0x75, 0xd2, 0xb9, 0xe5, 0xb4, 0x4a, 0x5c, 0xe8, 0xa1, 0x6e, 0xa6, 0x9f, 0x9f, 0x19,
];
/// The same reply's 16-byte UniqueNumber tail (body[119..135)).
const LIVE_REPLY2_UNIQUE: [u8; 16] = [
	0x1e, 0xc3, 0x0d, 0x06, 0x08, 0xfa, 0xeb, 0x46, 0xc0, 0x9a, 0x0e, 0xc2, 0xfa, 0x4a, 0x7e, 0xd2,
];

/// The live-verified OpenReply2 body shape: 4-byte header, pub at 21,
/// 54-byte box, "UniqueNumber" + 16B tail — 135 bytes total.
fn live_reply2_body() -> Vec<u8> {
	let mut b = vec![super::super::OPEN_REPLY2_ID];
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
	let fields: Reply2Fields = EarlyCrypto::parse_reply2(&body).expect("live shape must parse");
	assert_eq!(fields.server_pub, LIVE_REPLY2_SERVER_PUB);
	assert_eq!(fields.unique, LIVE_REPLY2_UNIQUE);
	assert_eq!(fields.box_wire.len(), 54, "the server box on the wire is 54 bytes");
}
