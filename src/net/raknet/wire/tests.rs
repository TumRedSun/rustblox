#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;

#[test]
fn frame_roundtrip_reliable() {
	let f = Frame {
		reliability: 2,
		has_split: false,
		seq: 0x123456,
		order_index: 0,
		order_channel: 0,
		split_id: 0,
		split_count: 0,
		split_index: 0,
		payload: vec![1, 2, 3, 4, 5],
	};
	let mut b = Vec::new();
	f.encode(&mut b);
	let g = Frame::decode(&b).unwrap();
	assert_eq!(g.reliability, 2);
	assert!(!g.has_split);
	assert_eq!(g.seq, 0x123456);
	assert_eq!(g.payload, vec![1, 2, 3, 4, 5]);
	assert_eq!(b.len(), frame_encoded_len(&g));
}

/// The exact byte layout a real server parses: reliability in the TOP
/// flag bits, bit_length BE, msg number + order index LE, split fields
/// BE. The old encoder put reliability in bits 0..2 and omitted
/// bit_length entirely — the server read "unreliable" frames with
/// garbage boundaries.
#[test]
fn frame_byte_layout_matches_the_fork() {
	let f = Frame {
		reliability: 3,
		has_split: false,
		seq: 0x123456,
		order_index: 7,
		order_channel: 0,
		split_id: 0,
		split_count: 0,
		split_index: 0,
		payload: vec![1, 2, 3, 4, 5],
	};
	let mut b = Vec::new();
	f.encode(&mut b);
	assert_eq!(
		b,
		vec![
			0x60, 0x00, 0x28, // flags (rel 3 << 5), bit_length = 40 BE
			0x56, 0x34, 0x12, // msg number LE
			0x07, 0x00, 0x00, 0x00, // order index LE + channel
			1, 2, 3, 4, 5,
		]
	);
	let g = Frame::decode(&b).unwrap();
	assert_eq!(g.reliability, 3);
	assert_eq!(g.seq, 0x123456);
	assert_eq!(g.order_index, 7);
	assert_eq!(g.payload, vec![1, 2, 3, 4, 5]);
	assert_eq!(b.len(), frame_encoded_len(&g));
}

#[test]
fn frame_split_fields_are_big_endian() {
	let f = Frame {
		reliability: 2,
		has_split: true,
		seq: 1,
		order_index: 0,
		order_channel: 0,
		split_id: 9,
		split_count: 5,
		split_index: 3,
		payload: vec![0xaa; 11],
	};
	let mut b = Vec::new();
	f.encode(&mut b);
	// flags 0x50 (rel 2 << 5 | split 0x10), bit_length 88 BE, msg# 1 LE,
	// count 5 BE, id 9 BE, index 3 BE, payload.
	assert_eq!(&b[0..3], &[0x50, 0x00, 88]);
	assert_eq!(&b[3..6], &[0x01, 0x00, 0x00]);
	assert_eq!(&b[6..16], &[0, 0, 0, 5, 0, 9, 0, 0, 0, 3]);
	let g = Frame::decode(&b).unwrap();
	assert_eq!((g.split_count, g.split_id, g.split_index), (5, 9, 3));
	assert_eq!(b.len(), frame_encoded_len(&g));
}

#[test]
fn frame_unreliable_roundtrip() {
	let f = Frame::unreliable(vec![9, 8, 7]);
	let mut b = Vec::new();
	f.encode(&mut b);
	let g = Frame::decode(&b).unwrap();
	assert_eq!(g.reliability, 0);
	assert_eq!(g.seq, 0);
	assert_eq!(g.payload, vec![9, 8, 7]);
}

#[test]
fn rbx_address_xor_roundtrip() {
	let addr: std::net::SocketAddr = "128.116.13.34:63646".parse().unwrap();
	let mut buf = Vec::new();
	write_address_rbx(&mut buf, addr);
	assert_eq!(buf[0], 4);
	assert_eq!(&buf[1..5], &[128 ^ 0xFF, 116 ^ 0xFF, 13 ^ 0xFF, 34 ^ 0xFF]);
	assert_eq!(&buf[5..7], &63646u16.to_be_bytes());
	let mut r = Reader::new(&buf);
	assert_eq!(read_address_rbx(&mut r), Some(addr));
}
