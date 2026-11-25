#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::handshake::{
	RBX_CAPABILITY_MODERN, RBX_ID_OPEN_CONNECTION_REPLY_1, RBX_ID_OPEN_CONNECTION_REPLY_2,
	RBX_ID_OPEN_CONNECTION_REQUEST_1, RBX_ID_OPEN_CONNECTION_REQUEST_2, RBX_PASSWORD,
	RBX_PROTOCOL_VERSION,
};
use super::wire::{put_u24, Frame};
use super::*;
use std::time::{Duration, Instant};

#[test]
fn offline_magic_is_the_canonical_raknet_bytes() {
	// Guards against re-guessing the magic: Roblox keeps the vanilla
	// OFFLINE_MESSAGE_DATA_ID (see the const docs). A wrong byte here
	// made every server silently drop our handshake.
	assert_eq!(
		OFFLINE_MAGIC,
		[
			0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34,
			0x56, 0x78,
		]
	);
}

#[test]
fn two_clients_loopback() {
	// Spin a fake RakNet server speaking the FORK wire format on
	// localhost and run the full handshake, then exchange a reliable
	// frame both ways. Exercises the whole pipeline end to end: 0x7B ->
	// 0x7E (BE), 0x78 -> 0x7D (BE, XOR addr), 0x09 with the password ->
	// 0x10 -> full 0x13, then fork-framed data datagrams.
	let server = UdpSocket::bind("127.0.0.1:0").unwrap();
	server.set_nonblocking(false).unwrap();
	let sport = server.local_addr().unwrap().port();
	let _ = sport;

	let server_thread = std::thread::spawn(move || {
		let mut buf = vec![0u8; 2048];
		// REQUEST_1 (fork id; the client matrix sends it first).
		let (n, peer) = server.recv_from(&mut buf).unwrap();
		assert_eq!(buf[0], RBX_ID_OPEN_CONNECTION_REQUEST_1);
		assert_eq!(&buf[1..17], &OFFLINE_MAGIC);
		assert_eq!(buf[17], RBX_PROTOCOL_VERSION);
		let _ = n;
		server
			.send_to(
				&[
					&[RBX_ID_OPEN_CONNECTION_REPLY_1],
					&OFFLINE_MAGIC[..],
					&1234567u64.to_be_bytes()[..],
					&[0],
					&1400u16.to_be_bytes()[..],
				]
				.concat(),
				peer,
			)
			.unwrap();
		// REQUEST_2 (fork: family | ip^FF | port BE | mtu BE | guid BE
		// | supportedVersion BE | capabilities BE).
		let (n, peer) = server.recv_from(&mut buf).unwrap();
		assert_eq!(buf[0], RBX_ID_OPEN_CONNECTION_REQUEST_2);
		let _ = n;
		assert_eq!(buf[17], 4); // family
		assert_eq!(buf[18], 127 ^ 0xFF); // ip[0] XOR-obfuscated
		server
			.send_to(
				&[
					&[RBX_ID_OPEN_CONNECTION_REPLY_2],
					&OFFLINE_MAGIC[..],
					&1234567u64.to_be_bytes()[..],
					&[4],
					&[127 ^ 0xFF, 0 ^ 0xFF, 0 ^ 0xFF, 1 ^ 0xFF],
					&peer.port().to_be_bytes()[..],
					&1400u16.to_be_bytes()[..],
					&[0],                    // useSecurity
					&5u32.to_be_bytes()[..], // supportedVersion
					&RBX_CAPABILITY_MODERN.to_be_bytes()[..],
				]
				.concat(),
				peer,
			)
			.unwrap();
		// CONNECTION_REQUEST: guid BE, time BE, security 0, password.
		let (n, peer) = server.recv_from(&mut buf).unwrap();
		assert_eq!(buf[0], ID_CONNECTION_REQUEST);
		assert_eq!(&buf[18..24], &RBX_PASSWORD[..]);
		let _ = n;
		// ACCEPTED: client addr (XOR) + systemIndex BE + 10 addrs + 2
		// times BE.
		let mut acc = vec![ID_CONNECTION_REQUEST_ACCEPTED];
		acc.push(4);
		acc.extend_from_slice(&[127 ^ 0xFF, 0 ^ 0xFF, 0 ^ 0xFF, 1 ^ 0xFF]);
		acc.extend_from_slice(&peer.port().to_be_bytes());
		acc.extend_from_slice(&0u16.to_be_bytes()); // systemIndex
		for _ in 0..10 {
			acc.push(4);
			acc.extend_from_slice(&[0xFF; 4]); // 0.0.0.0 XORed
			acc.extend_from_slice(&0u16.to_be_bytes());
		}
		acc.extend_from_slice(&111u64.to_be_bytes()); // request time
		acc.extend_from_slice(&222u64.to_be_bytes()); // server time
		server.send_to(&acc, peer).unwrap();
		// NEW_INCOMING_CONNECTION — the full fork layout: 11 address
		// blocks + 16 bytes of timestamps.
		let (n, _peer) = server.recv_from(&mut buf).unwrap();
		assert_eq!(buf[0], ID_NEW_INCOMING_CONNECTION);
		assert_eq!(n, 1 + 11 * 7 + 16);
		let _ = n;

		// Receive a reliable frame datagram and reply with one.
		loop {
			let (n, peer) = server.recv_from(&mut buf).unwrap();
			let pkt = &buf[..n];
			if pkt[0] == ID_DISCONNECTION_NOTIFICATION {
				break;
			}
			if pkt[0] & 0x80 != 0 && pkt[0] & 0xC0 != 0xC0 {
				// Data datagram: frame inside at offset 4.
				let f = Frame::decode(&pkt[4..]).unwrap();
				assert_eq!(f.payload, b"hello-rustblox");
				// Reply: our own datagram with a reliable-ordered frame
				// (encode now produces the fork layout).
				let mut dg = vec![0x84];
				put_u24(&mut dg, 0);
				let rf = Frame {
					reliability: 3,
					has_split: false,
					seq: 0,
					order_index: 0,
					order_channel: 0,
					split_id: 0,
					split_count: 0,
					split_index: 0,
					payload: b"reply-from-server".to_vec(),
				};
				rf.encode(&mut dg);
				server.send_to(&dg, peer).unwrap();
				// Then wait for the client ACK (loop again).
				let (n2, _) = server.recv_from(&mut buf).unwrap();
				let _ = n2;
				break;
			}
		}
	});

	let mut client =
		RakClient::connect("127.0.0.1", sport, &[10, 5], Duration::from_secs(5), None, None, None)
			.expect("handshake failed");
	assert_eq!(client.state, RakState::Connected);
	assert_eq!(client.server_guid, 1234567);
	assert_eq!(client.fork, ForkGen::Roblox);

	client.send_reliable(b"hello-rustblox");
	client.flush();

	// Pump until we get the reply.
	let deadline = Instant::now() + Duration::from_secs(5);
	let mut got = None;
	while Instant::now() < deadline {
		if let Some(p) = client.poll() {
			got = Some(p);
			break;
		}
		client.tick();
		std::thread::sleep(Duration::from_millis(10));
	}
	assert_eq!(got.as_deref(), Some(&b"reply-from-server"[..]));
	client.close();
	server_thread.join().unwrap();
}
