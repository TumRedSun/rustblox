#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crate::net::udmux::token::b64_decode;

#[test]
fn envelope_candidates_cover_the_observed_shapes() {
	// No token -> bare only (direct dial keeps working unchanged).
	assert_eq!(envelope_candidates("", 0), vec![BARE]);

	// A padded 24-char token decodes to exactly 16 bytes -> the epoch tag.
	let padded = "AAAAAAAAAAAAAAAAAAAAAA==";
	assert_eq!(b64_decode(padded).len(), 16);
	let cands = envelope_candidates(padded, 1794414724);
	let labels: Vec<&str> = cands.iter().map(|c| c.label).collect();
	assert_eq!(
		labels,
		vec![
			"bare",
			"env17-token",
			"env1f-token",
			"env1f-token-muxtok",
			"env1f-token-muxpe",
			"env17-pe+token",
			"env17-token+pe",
			"env17-ascii",
			"env17-zeros",
		]
	);
	// env17-token: the decoded token IS the tag, no 18-byte skip variant.
	assert_eq!(cands[1].direction, 0x17);
	assert_eq!(cands[1].epoch, [0u8; 16]);
	// env1f-token-muxpe: the 8-byte mux field is the pepper repeated.
	let pe = 1794414724u32.to_le_bytes();
	assert_eq!(&cands[4].mux[..4], &pe);
	assert_eq!(&cands[4].mux[4..], &pe);
	// env17-pe+token mixes the pepper into the first 4 tag bytes.
	assert_eq!(&cands[5].epoch[..4], &pe);
}

#[test]
fn unpadded_tokens_add_the_18_byte_variant() {
	// 24 chars without padding -> 18 bytes -> a tok18 (skip-2) candidate.
	let cands = envelope_candidates("AAAAAAAAAAAAAAAAAAAAAAAA", 0);
	let labels: Vec<&str> = cands.iter().map(|c| c.label).collect();
	assert!(labels.contains(&"env17-token18"), "labels: {:?}", labels);
	let tok18 = cands.iter().find(|c| c.label == "env17-token18").unwrap();
	assert_eq!(tok18.epoch, [0u8; 16]);
}

#[test]
fn rupp_frame_round_trips_through_parse() {
	let env = Envelope {
		label: "test17",
		direction: 0x17,
		cls: 0x02,
		epoch: *b"0123456789abcdef",
		mux: [0u8; 8],
	};
	let framed = rupp_frame(&env, b"PINGBODY");
	let (dir, epoch, mux, hlen) = rupp_parse(&framed).expect("17 must parse");
	assert_eq!((dir, hlen), (0x17, 23usize));
	assert_eq!(&epoch, b"0123456789abcdef");
	assert_eq!(mux, [0u8; 8]);
	assert_eq!(&framed[hlen..], b"PINGBODY");

	let env31 = Envelope {
		label: "test1f",
		direction: 0x1f,
		cls: 0x01,
		epoch: [9u8; 16],
		mux: *b"MUXMUXMX",
	};
	let framed31 = rupp_frame(&env31, b"X");
	let (dir, epoch, mux, hlen) = rupp_parse(&framed31).expect("1f must parse");
	assert_eq!((dir, hlen), (0x1f, 31usize));
	assert_eq!(epoch, [9u8; 16]);
	assert_eq!(&mux, b"MUXMUXMX");
	assert_eq!(&framed31[hlen..], b"X");
}

#[test]
fn client_envelope_matches_the_live_capture_bytes() {
	// Live capture (raknet37, Android client): header bytes 0..30 with token
	// type 0x01, rcc 10.9.0.140:50902 in the mux field and the 16-byte
	// TokenValue as the tag.
	let env = client_envelope(
		&[
			0x9f, 0xf7, 0x3e, 0x01, 0x0c, 0xdb, 0x08, 0xa5, 0xa9, 0x91, 0x61, 0x12, 0x44, 0xa4,
			0xd0, 0x55,
		],
		(std::net::Ipv4Addr::new(10, 9, 0, 140), 50902),
	);
	let framed = rupp_frame(&env, &[0x7b]);
	assert_eq!(
		&framed[..32],
		&[
			0x01, 0x00, 0x00, 0x1f, 0x01, 0x11, 0x01, 0x9f, 0xf7, 0x3e, 0x01, 0x0c, 0xdb, 0x08,
			0xa5, 0xa9, 0x91, 0x61, 0x12, 0x44, 0xa4, 0xd0, 0x55, 0x02, 0x06, 0x0a, 0x09, 0x00,
			0x8c, 0xc6, 0xd6, 0x7b,
		][..]
	);

	// REPLY_1 arrives with a bare 01 00 00 04 prefix (capture pkt 1) and
	// must parse so the 0x7E body is not misread as a raw datagram.
	let reply1 = [
		0x01, 0x00, 0x00, 0x04, 0x7e, 0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd,
		0xfd, 0xfd, 0x12, 0x34, 0x56, 0x78, 0x41, 0xef, 0x3f, 0xd3, 0x2e, 0xfc, 0x18, 0xa9, 0x00,
		0x04, 0xb0,
	];
	let (dir, _tag, _mux, hlen) = rupp_parse(&reply1).expect("bare prefix must parse");
	assert_eq!((dir, hlen), (0x04, 4usize));
	assert_eq!(reply1[hlen], 0x7e);
}

#[test]
fn rupp_parse_rejects_non_envelopes() {
	// Raw RakNet offline packets never start with the RUPP signature.
	assert!(rupp_parse(&[]).is_none());
	assert!(rupp_parse(&[0x06, 0x00, 0x00, 0x17, 0x01, 0x11, 0x02]).is_none());
	assert!(rupp_parse(&[0x01, 0x00, 0x00, 0x99, 0x01, 0x11, 0x02]).is_none());
	assert!(rupp_parse(&[0x01, 0x00, 0x00, 0x17, 0x09, 0x09, 0x09]).is_none());
}
