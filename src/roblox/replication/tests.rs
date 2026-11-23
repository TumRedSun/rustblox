//! Unit tests for the replication parser: legacy clusters, zstd fork
//! bodies and the str16/u8/u16 name-table conventions.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::body::extract_names;
use super::*;
use crate::world::{DataModel, Prop};

fn u32le(v: u32) -> [u8; 4] {
	v.to_le_bytes()
}

#[test]
fn create_and_property_events() {
	let mut p = ReplicationParser::new();
	let mut dm = DataModel::new();

	// cluster 1: create Part "Brick" ref 42 parent 41
	let mut create_body = Vec::new();
	create_body.extend_from_slice(&42i64.to_le_bytes());
	create_body.extend_from_slice(&4u16.to_le_bytes());
	create_body.extend_from_slice(b"Part");
	create_body.extend_from_slice(&5u16.to_le_bytes());
	create_body.extend_from_slice(b"Brick");
	create_body.extend_from_slice(&41i64.to_le_bytes());
	let mut cluster = Vec::new();
	cluster.extend_from_slice(&u32le(create_body.len() as u32));
	cluster.extend_from_slice(&u32le(1));
	cluster.extend_from_slice(&create_body);

	// cluster 2: property size vec3 on ref 42
	let mut prop_body = Vec::new();
	prop_body.extend_from_slice(&42i64.to_le_bytes());
	prop_body.extend_from_slice(&4u16.to_le_bytes());
	prop_body.extend_from_slice(b"Size");
	prop_body.push(5);
	prop_body.extend_from_slice(
		&[4.0f32, 1.2, 2.0].iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>(),
	);
	cluster.extend_from_slice(&u32le(prop_body.len() as u32));
	cluster.extend_from_slice(&u32le(2));
	cluster.extend_from_slice(&prop_body);

	let mut packet = u32le(0).to_vec();
	packet.extend_from_slice(&cluster);
	let events = p.feed(&packet, &mut dm);
	assert_eq!(events.len(), 2);
	let brick = dm.get_by_ref(42).unwrap();
	assert_eq!(brick.class, "Part");
	assert_eq!(brick.name, "Brick");
	assert_eq!(brick.prop("Size"), Some(&Prop::Vec3([4.0, 1.2, 2.0])));
	assert_eq!(p.decoded, 2);
}

#[test]
fn compressed_packet_inflates() {
	use flate2::write::DeflateEncoder;
	use flate2::Compression;
	use std::io::Write;
	let mut p = ReplicationParser::new();
	let mut dm = DataModel::new();

	let mut create_body = Vec::new();
	create_body.extend_from_slice(&7i64.to_le_bytes());
	create_body.extend_from_slice(&4u16.to_le_bytes());
	create_body.extend_from_slice(b"Part");
	create_body.extend_from_slice(&1u16.to_le_bytes());
	create_body.extend_from_slice(b"P");
	create_body.extend_from_slice(&(-1i64).to_le_bytes());
	let mut data_packet = Vec::new();
	data_packet.extend_from_slice(&u32le(create_body.len() as u32));
	data_packet.extend_from_slice(&u32le(1));
	data_packet.extend_from_slice(&create_body);

	let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
	enc.write_all(&data_packet).unwrap();
	let compressed = enc.finish().unwrap();

	let mut packet = u32le(0x8000_0000).to_vec(); // type 0 + compressed bit
	packet.extend_from_slice(&compressed);
	let events = p.feed(&packet, &mut dm);
	assert_eq!(events.len(), 1);
	assert!(dm.get_by_ref(7).is_some());
}

#[test]
fn unknown_types_are_tolerated() {
	let mut p = ReplicationParser::new();
	let mut dm = DataModel::new();
	let packet = u32le(9).to_vec();
	let events = p.feed(&packet, &mut dm);
	assert!(matches!(events[0], RepEvent::Unknown { packet_type: 9, .. }));
	// Stream must stay alive for more packets.
	let events2 = p.feed(&u32le(9).to_vec(), &mut dm);
	assert_eq!(events2.len(), 1);
}

#[test]
fn names_honour_both_length_conventions() {
	// `u8 len` (schema/cluster) and `u16 LE len` (policy message).
	let mut buf = Vec::new();
	buf.push(4);
	buf.extend_from_slice(b"Part");
	buf.extend_from_slice(&32u16.to_le_bytes());
	buf.extend_from_slice(b"UserPlayerScriptsSAuthDirectAPIs");
	buf.extend_from_slice(&5u16.to_le_bytes());
	buf.extend_from_slice(b"false");
	let names = extract_names(&buf);
	assert!(names.contains(&"Part".to_string()), "{:?}", names);
	assert!(names.contains(&"UserPlayerScriptsSAuthDirectAPIs".to_string()), "{:?}", names);
	assert!(names.contains(&"false".to_string()), "{:?}", names);
}

#[test]
fn delete_event() {
	let mut p = ReplicationParser::new();
	let mut dm = DataModel::new();
	dm.create(55, "Part", "x", -1);
	let mut cluster = Vec::new();
	let body = 55i64.to_le_bytes();
	cluster.extend_from_slice(&u32le(8));
	cluster.extend_from_slice(&u32le(3));
	cluster.extend_from_slice(&body);
	let mut packet = u32le(0).to_vec();
	packet.extend_from_slice(&cluster);
	let events = p.feed(&packet, &mut dm);
	assert!(matches!(events[0], RepEvent::InstanceRemoved { r: 55 }));
	assert!(dm.get_by_ref(55).is_none());
}

/// Hex decode for the captured vectors below.
fn unhex(s: &str) -> Vec<u8> {
	let clean: Vec<u8> = s.bytes().filter(|b| b.is_ascii_hexdigit()).collect();
	clean
		.chunks(2)
		.filter_map(|c| {
			let hi = (c[0] as char).to_digit(16)?;
			let lo = (c[1] as char).to_digit(16)?;
			Some((hi * 16 + lo) as u8)
		})
		.collect()
}

// Live v29 capture, `8d 01` #11922: the fork's biggest stream type is a
// zstd frame; 105 wire bytes inflate to 215 bytes of bit-packed cluster
// data. Proves the payload layer is zstd, not a second cipher.
#[test]
fn fork_zstd_body_inflates() {
	let mut p = ReplicationParser::new();
	let mut dm = DataModel::new();
	let msg = unhex(
		"8d01b17e0100045a000000d700000028b52ffd20d78d02004404040100e4008bffcb870e4b7ecb87b14b7fcb76014b7ecb870a4b7b4b604b564b5a4b6d4b80cb87094b674b514b474b4a4b5b4b7a4f4b444b484b594b7ccb87170880ff0003003965d0af5222c12614",
	);
	let events = p.feed(&msg, &mut dm);
	assert_eq!(p.replication, 1);
	assert_eq!(p.inflated, 1, "the body must come out of a zstd frame");
	assert_eq!(p.skipped, 0);
	assert!(events
		.iter()
		.any(|e| matches!(e, RepEvent::Decoded { head: 0x8d01, body: 215, inflated: true })));
	assert_eq!(p.bytes_out, 215);
}

// Live v29 capture, `83 0d` #12867: a world cluster body that inflates to
// plaintext asset ids and part names. This is the world data the client
// needs — it was never encrypted, only zstd-compressed.
#[test]
fn fork_cluster_names_decode() {
	let mut p = ReplicationParser::new();
	let mut dm = DataModel::new();
	let msg = unhex(
		"830d01000000b6000000ad00000028b52ffd20ad69050001746a00005001037a130041f84eb140361b7441050117726278617373657469643a2f2f3638343330353132313207010d010e0a92f990232494612626a4d19a156a4e0fbed693bd5d89433ddb864fbf106cee484537c050457b5fbf4411d2b44841069581428f188a4312c9a5964313011dcf6179447a35f341b6e8eac400647fc93e5140210023bb7657260031a00a340247a7f1233fb6f6e23ef93b9c3f4c06486f6c646572ff017ba400000c08000ffedf09090908000ff8e00908000ffee0090909090908000ff8e10908000ffee1090909090908000ff8e20908000ffee209090908000ffee309090100",
	);
	let events = p.feed(&msg, &mut dm);
	assert_eq!(p.inflated, 1);
	let names: Vec<String> = events
		.iter()
		.flat_map(|e| match e {
			RepEvent::Names(v) => v.clone(),
			_ => Vec::new(),
		})
		.collect();
	assert!(dm.strings.contains("rbxassetid://6843051212"), "asset id not harvested: {:?}", names);
	assert!(dm.strings.contains("Holder"), "part name not harvested");
	assert_eq!(p.bytes_out, 173, "inflated body length");
}
