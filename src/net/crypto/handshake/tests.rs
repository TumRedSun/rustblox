//! Tests transcribed from live captures and re-verified against a live run on
//! 24 Nov 2026 (the OpenReply2 below is the exact datagram body an RCC sent
//! the standalone client; `out1` is the SESSDER#1 server->client key of that
//! session and the seed2 is what the box must yield).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::super::{EarlyCrypto, OCR2_CRYPTO_ID, UNIQUE_NAME};

fn unhex(s: &str) -> Vec<u8> {
	let b = s.as_bytes();
	(0..b.len() / 2)
		.map(|i| {
			u8::from_str_radix(std::str::from_utf8(&b[i * 2..i * 2 + 2]).expect("hex"), 16)
				.expect("hex")
		})
		.collect()
}

/// Live OpenReply2 body (135 bytes, RUPP prefix stripped) from a real RCC.
const LIVE_ORP2_BODY: &str = "7d00ffff00fefefefefdfdfdfd12345678012c003a511df6b01afbafe6f8c593e32a1e16b1686b5ec32cf4891528297e3d5aa84c588f925313dc319d41b780bfde73bb5a79c33163515ef2b9aa84be28ec953981fea584ddeada00029ed00c92850b80c7b26967faecdd74556e697175654e756d6265723309388f0f2103faee6d4539d0bd84b6";
/// SESSDER#1 out1 of that session (server -> client).
const LIVE_OUT1: &str = "81eec78394fcca95c86bbf1c998b790844047002fa4ed3b3927dfef549884eb8";
/// The complete SESSDER#2 peer recovered from the box.
const LIVE_SEED2: &str = "511df6b01afbafe6f8c593e32a1e16b1686b5ec32cf489af3bf6ba4dc086c15d";

#[test]
fn ocr2_wire_matches_the_verified_layout() {
	let mut ec = EarlyCrypto::from_join(
		"M17S4M%2Bk%2FCjattHW2WMEufaCnwrgd6itqO%2FVngg5R0k%3D",
		vec![0x42u8; 33],
		0x12,
	)
	.expect("the live percent-encoded key must decode");
	assert_eq!(
		ec.server_ephemeral,
		[
			0x33, 0x5e, 0xd2, 0xe0, 0xcf, 0xa4, 0xfc, 0x28, 0xda, 0xb6, 0xd1, 0xd6, 0xd9, 0x63,
			0x04, 0xb9, 0xf6, 0x82, 0x9f, 0x0a, 0xe0, 0x77, 0xa8, 0xad, 0xa8, 0xef, 0xd5, 0x9e,
			0x08, 0x39, 0x47, 0x49
		]
	);
	// AAD = id + magic + `03 5a 00 5d 00 05` + pub32 + auth_ver + pa_len + pa33.
	let mut want = vec![OCR2_CRYPTO_ID];
	want.extend_from_slice(&crate::net::raknet::OFFLINE_MAGIC);
	want.extend_from_slice(&[0x03, 0x5a, 0x00, 0x5d, 0x00, 0x05]);
	want.extend_from_slice(&ec.client_pk);
	want.push(0x12); // auth_ver — ClientTicket part4 "18"
	want.push(0x21);
	want.extend_from_slice(&[0x42u8; 33]);
	assert_eq!(want.len(), 90);
	let (aad, body) = ec.build_ocr2(1200, 59486).expect("33-byte paBlob must build");
	assert_eq!(aad.len(), 90);
	assert_eq!(&aad[..23], &want[..23]);
	assert_eq!(&aad[55..], &want[55..]);
	// Body = aad(90) + ct(93) + "UniqueNumber"(12) + tag(16) = 211.
	assert_eq!(&body[..23], &want[..23]);
	assert_eq!(&body[55..90], &want[55..]);
	assert_eq!(body.len(), 211);
	assert_eq!(&body[183..195], UNIQUE_NAME);
	// A short paBlob must refuse to build (the server verifies it).
	let mut bad = EarlyCrypto::new(ec.server_ephemeral, Vec::new(), 0x12);
	assert!(bad.build_ocr2(1200, 59486).is_none());
}

#[test]
fn reply2_parse_extracts_the_live_split() {
	let body = unhex(LIVE_ORP2_BODY);
	assert_eq!(body.len(), 135, "the live OpenReply2 body is 135 bytes");
	let f = EarlyCrypto::parse_reply2(&body).expect("live shape must parse");
	// 23 clear seed bytes at [21..44]; the rest of the seed rides in the box.
	assert_eq!(f.clear23, unhex(&LIVE_SEED2[..46])[..]);
	assert_eq!(f.aad.len(), 44);
	assert_eq!(f.ct.len(), 63);
	assert_eq!(f.nonce, UNIQUE_NAME);
	assert_eq!(f.tag.len(), 16);
}

#[test]
fn reply2_seed2_opens_with_the_early_server_key() {
	let body = unhex(LIVE_ORP2_BODY);
	let out1 = unhex(LIVE_OUT1);
	let mut ec = EarlyCrypto::new([0u8; 32], vec![0x42u8; 33], 0x12);
	ec.rx = out1.try_into().expect("32 bytes");
	let seed2 = ec.reply2_seed2(&body).expect("the live box must open");
	assert_eq!(seed2[..], unhex(LIVE_SEED2)[..]);
	// A wrong key must not open it (it is a real AEAD tag, not a checksum).
	ec.rx = [0x11u8; 32];
	assert!(ec.reply2_seed2(&body).is_none());
}
