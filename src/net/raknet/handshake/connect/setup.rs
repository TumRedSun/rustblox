//! Client construction (resolve, bind, GUID/tag) and phase-0 envelope
//! pinning for the handshake.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::super::{hex_head, RBX_OFFLINE_PROBE};
use crate::net::raknet::{ForkGen, RakClient, RakError, RakState};
use crate::net::udmux::{self, Envelope};
use crate::netlog;
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

/// Resolve `host:port`, bind the UDP socket and build the unconnected client.
pub(super) fn new_client(
	host: &str,
	port: u16,
	early: Option<crate::net::crypto::EarlyCrypto>,
) -> Result<RakClient, RakError> {
	let addr: SocketAddr = (host, port)
		.to_socket_addrs()
		.map_err(|e| RakError::Network(e.to_string()))?
		.next()
		.ok_or(RakError::Network("host did not resolve".into()))?;
	let socket = UdpSocket::bind(if addr.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" })
		.map_err(|e| RakError::Network(e.to_string()))?;
	socket.set_nonblocking(true).map_err(|e| RakError::Network(e.to_string()))?;

	let guid = client_guid();
	Ok(RakClient {
		socket,
		server: addr,
		guid,
		server_guid: 0,
		mtu: RBX_OFFLINE_PROBE as u16,
		state: RakState::Connecting,
		ping_ms: None,
		envelope: None,
		client_tag: seed_tag(guid),
		// The server's 16-byte tag, learned from any incoming RUPP header.
		server_tag: None,
		early,
		crypto_connected: false,
		// Offline-id generation that answered phase 1.
		fork: ForkGen::Roblox,
		started: Instant::now(),
		request_ms: 0,
		server_ms: 0,
		accepted: None,
		announced: false,
		reliability_index: 0,
		order_index: 0,
		datagram_seq: 0,
		split_id: 0,
		out_queue: HashMap::new(),
		sent_msgs: HashMap::new(),
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
	})
}

/// GUID derived from the wall clock ("RUSTBLOX" XORed into the nanoseconds).
fn client_guid() -> u64 {
	let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
	(t.as_nanos() as u64) ^ 0x5255_5354_424c_4f58
}

/// SplitMix-style expansion of the GUID + wall clock into the 16-byte
/// pre-connect tag.
fn seed_tag(guid: u64) -> [u8; 16] {
	let mut t = (guid
		^ std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_nanos() as u64)
		| 1;
	let mut tag = [0u8; 16];
	for chunk in tag.chunks_mut(8) {
		t = t.wrapping_add(0x9e37_79b9_7f4a_7c15);
		let mut z = t;
		z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
		z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
		z ^= z >> 31;
		let n = chunk.len();
		chunk.copy_from_slice(&z.to_le_bytes()[..n]);
	}
	tag
}

/// Phase 0 (udmux sessions): pin the datagram ENVELOPE.
///
/// The 34k-datagram live capture (`sober_ro.pcap`, Nov 2026) shows the
/// client header `01 00 00 1f 01 11 02 <tag:16> 02 06 <rcc ip4> <port BE16>`
/// on 100% of the datagrams: a TokenTlv with token TYPE 0x02 (Rupp.h) and
/// the private RCC address in the mux field. The proxy routes on the mux AND
/// gates on the token — the type-0x01 + random-tag shape the older builds
/// pinned is exactly what produced total OCR1 silence.
///
/// Pre-connect the tag rides the session's static TokenValue; the connected
/// phase then mirrors the server's rotating epoch (`send_raw`). The random
/// client tag stays as a matrix fallback, and the OCR1 matrix walks
/// {token,random} x {02,01}.
///
/// Returns the envelope to pin (if any) and whether a reachability probe
/// drew a pong.
pub(super) fn pin_envelope(
	c: &mut RakClient,
	udmux_token: Option<(&str, u64)>,
	rcc: Option<(Ipv4Addr, u16)>,
) -> (Option<Envelope>, bool) {
	let token_tag = udmux_token.and_then(|(tok, _)| udmux::decode_token16(tok));
	let known_shape = match (rcc, token_tag) {
		(Some((ip, port)), Some(tag)) => {
			let env = udmux::client_envelope(&tag, (ip, port));
			netlog!(
				net,
				"envelope: pinning the capture shape with the session token as the tag \
                 (rcc {}:{}, tag {}) — the OCR1 matrix still walks the tag/type variants",
				ip,
				port,
				hex_head(&env.epoch, 16)
			);
			Some(env)
		}
		(Some((ip, port)), None) => {
			let env = udmux::client_envelope(&c.client_tag, (ip, port));
			netlog!(
				net,
				"envelope: no usable TokenValue — pinning the capture shape with a random \
                 tag (rcc {}:{}, tag {})",
				ip,
				port,
				hex_head(&env.epoch, 16)
			);
			Some(env)
		}
		_ => None,
	};
	if known_shape.is_some() {
		return (known_shape, false);
	}

	let envelopes: Vec<Envelope> = match udmux_token {
		None => vec![udmux::BARE],
		Some((tok, pepper)) => udmux::envelope_candidates(tok, pepper),
	};
	if envelopes.len() > 1 {
		let p = c.probe_reachability(&envelopes, Duration::from_millis(700));
		match &p {
			Some(env) => netlog!(
				net,
				"envelope probe: '{}' drew a pong — pinning it for the session",
				env.label
			),
			None => netlog!(
				net,
				"envelope probe: no pong in any RUPP shape — trying the OCR1 matrix anyway"
			),
		}
		return (p, p.is_some());
	}
	(None, false)
}
