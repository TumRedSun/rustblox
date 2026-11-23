#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::build::build_ocr2;
use super::{RBX_CAPABILITY_MODERN, RBX_ID_OPEN_CONNECTION_REQUEST_2};
use crate::net::raknet::{ForkGen, RakClient, RakState, OFFLINE_MAGIC};
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{SocketAddr, UdpSocket};
use std::time::Instant;

/// The phase-2 packet, byte for byte: id 0x78, magic, XOR address,
/// MTU BE, GUID BE, then the two fork-only tail fields BE.
#[test]
fn rbx_ocr2_layout_is_the_fork_shape() {
	let srv: SocketAddr = "10.30.2.71:63646".parse().unwrap();
	let pkt = build_ocr2(
		RBX_ID_OPEN_CONNECTION_REQUEST_2,
		5,
		RBX_CAPABILITY_MODERN,
		srv,
		1400,
		0xAABB_CCDD_EEFF_0011,
	);
	assert_eq!(pkt[0], 0x78);
	assert_eq!(&pkt[1..17], &OFFLINE_MAGIC);
	assert_eq!(pkt[17], 4);
	assert_eq!(&pkt[18..22], &[10 ^ 0xFF, 30 ^ 0xFF, 2 ^ 0xFF, 71 ^ 0xFF]);
	assert_eq!(&pkt[22..24], &63646u16.to_be_bytes());
	assert_eq!(&pkt[24..26], &1400u16.to_be_bytes());
	assert_eq!(&pkt[26..34], &0xAABB_CCDD_EEFF_0011u64.to_be_bytes());
	assert_eq!(&pkt[34..38], &5u32.to_be_bytes());
	assert_eq!(&pkt[38..46], &RBX_CAPABILITY_MODERN.to_be_bytes());
	assert_eq!(pkt.len(), 46);
}

/// A minimal connected client with a pinned 0x1f envelope, for the
/// tag-mirroring tests below.
fn test_client(state: RakState, server_tag: Option<[u8; 16]>) -> RakClient {
	let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
	socket.set_nonblocking(true).unwrap();
	RakClient {
		socket,
		server: "127.0.0.1:1".parse().unwrap(),
		guid: 1,
		server_guid: 2,
		mtu: 1400,
		state,
		ping_ms: None,
		envelope: Some(crate::net::udmux::client_envelope(
			&[7u8; 16],
			("10.206.18.67".parse().unwrap(), 49251),
		)),
		client_tag: [7u8; 16],
		server_tag,
		early: None,
		crypto_connected: false,
		fork: ForkGen::Roblox,
		reliability_index: 0,
		order_index: 0,
		datagram_seq: 0,
		split_id: 0,
		out_queue: HashMap::new(),
		frame_buf: Vec::new(),
		received_datagrams: HashSet::new(),
		ack_buffer: Vec::new(),
		delivered_ordered: 0,
		ordered_hold: HashMap::new(),
		splits: HashMap::new(),
		pending: VecDeque::new(),
		last_ping: Instant::now(),
		ping_time: None,
		last_recv: Instant::now(),
		buf: vec![0u8; 65536],
	}
}

#[test]
fn connected_send_mirrors_the_rotating_server_tag() {
	// sober_ro.pcap: the tag rotates every wall-clock second, the proxy
	// leads and the client mirrors. A datagram sent while connected must
	// ride the NEWEST server tag (not the tag pinned at connect time)
	// and class byte 0x02.
	let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
	peer.set_nonblocking(true).unwrap();
	let mut c = test_client(RakState::Connected, Some([9u8; 16]));
	c.server = peer.local_addr().unwrap();
	c.send_raw(b"X").unwrap();
	let mut b = vec![0u8; 2048];
	let (n, _) = peer.recv_from(&mut b).unwrap();
	assert_eq!(
		&b[..31],
		&[
			0x01, 0x00, 0x00, 0x1f, 0x01, 0x11, 0x02, // cls 0x02
			9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, // rotated tag
			0x02, 0x06, 10, 206, 18, 67, 0xc0, 0x63, // mux = 10.206.18.67:49251
		][..]
	);
	assert_eq!(&b[31..n], b"X");
}

#[test]
fn connecting_send_keeps_the_client_tag() {
	// Until ACCEPTED the client still frames its datagrams with ITS OWN
	// tag — mirroring starts only in the connected phase. The token-type
	// byte is 0x02 (34k pcap: 100% of the captured client datagrams).
	let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
	peer.set_nonblocking(true).unwrap();
	let mut c = test_client(RakState::Connecting, Some([9u8; 16]));
	c.server = peer.local_addr().unwrap();
	c.send_raw(b"X").unwrap();
	let mut b = vec![0u8; 2048];
	let (n, _) = peer.recv_from(&mut b).unwrap();
	assert_eq!(
		&b[..31],
		&[
			0x01, 0x00, 0x00, 0x1f, 0x01, 0x11, 0x02, // token type 0x02
			7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, // client tag
			0x02, 0x06, 10, 206, 18, 67, 0xc0, 0x63,
		][..]
	);
	assert_eq!(&b[31..n], b"X");
}
