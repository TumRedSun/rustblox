//! The fork's connected-phase RakNet system messages.
//!
//! After the steady `CONNECTION_REQUEST` the server does not start
//! replicating immediately: it first walks the client through the connected
//! RakNet handshake, and every one of those messages rides as an ordinary
//! frame payload *inside* the steady datagram stream (byte-mapped from the
//! live v29 capture, `/storage/rblx_debug/session-crypto/analysis`).
//! Missing them is why the client sat in `InGame` with `pkts: 11` and
//! `replication: 0` while the server streamed nothing.
//!
//! Every system message starts with a message id below `0x80`; the
//! application layer always sets bit 7 (`0x81`, `0x83`, `0x8d`, …), so the
//! split between the two layers is unambiguous.
//!
//! ```text
//! id  layout                                                   wire size
//! 0x10 ACCEPTED      peer addr | sysIndex u16 | 10 addr slots |
//!                    request_ms u64 | server_ms u64 | server_us u64   104
//! 0x13 NEW_INCOMING  peer addr | 10 addr slots | server_ms u64 |
//!                    client_ms u64 | client_us u64                    102
//! 0x00 ping          client_ms u64                                 9 / 1160
//! 0x03 pong          peer_ms u64 | own_ms u64 | own_us u64         25 / ping
//! 0x51..0x53 sync    own_ms u64                                        9
//! 0x54..0x56 ack     peer_ms u64 | own_ms u64 | own_us u64            25
//! ```
//!
//! Addresses use the fork's XOR-obfuscated block (`family 4 | ip^0xFF |
//! port BE`). The 8-byte clock is a monotonic microsecond counter (the
//! captured Android client had ~21 days of uptime); the millisecond
//! timestamps are what the peer echoes, so we mirror them verbatim.
//!
//! The real client answers `0x10` inside its very first steady datagram
//! (message 1, order 0) together with an unpadded ping, then sends an
//! MTU-sized padded ping and the three clock-sync announcements. The server
//! starts streaming application data right after that burst.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::net::raknet::wire::{read_address_rbx, Reader};
use crate::net::raknet::{
	RakClient, ID_CONNECTED_PING, ID_CONNECTED_PONG, ID_NEW_INCOMING_CONNECTION,
};
use crate::netlog;
use std::net::SocketAddr;

/// Padded ping/pong size: the negotiated 1200-byte MTU minus the connected
/// framing allowance, exactly as measured on the wire.
pub const PADDED_PING: usize = 1160;

/// Zero address slot in the fork's 7-byte block form (`0.0.0.0:0`).
const ZERO_ADDR_RBX: [u8; 7] = [4, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0];

/// Clock-sync announce ids the server answers with `+3`; the whole set is
/// sent in both directions.
pub const CLOCK_SYNC_ANNOUNCE: [u8; 3] = [0x53, 0x52, 0x51];

/// Parsed `ID_CONNECTION_REQUEST_ACCEPTED` tail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Accepted {
	pub client_addr: Option<SocketAddr>,
	pub request_ms: u64,
	pub server_ms: u64,
}

/// Parse the steady ACCEPTED body (tolerant: any short/garbled field falls
/// back to zero instead of desynchronizing the session).
pub fn parse_accepted(payload: &[u8]) -> Accepted {
	let mut r = Reader::new(payload);
	let _ = r.u8();
	let client_addr = read_address_rbx(&mut r);
	let _sys_index = r.u16be().unwrap_or(0);
	for _ in 0..10 {
		let _ = read_address_rbx(&mut r);
	}
	Accepted { client_addr, request_ms: r.u64be().unwrap_or(0), server_ms: r.u64be().unwrap_or(0) }
}

/// A millisecond timestamp at offset 1 (big-endian), or 0.
pub fn ms_at(payload: &[u8], off: usize) -> u64 {
	match payload.get(off..off + 8) {
		Some(b) => {
			let mut a = [0u8; 8];
			a.copy_from_slice(b);
			u64::from_be_bytes(a)
		}
		None => 0,
	}
}

impl RakClient {
	/// Milliseconds since this client started (the value the fork's
	/// millisecond timestamps carry).
	pub fn now_ms(&self) -> u64 {
		self.started.elapsed().as_millis() as u64
	}

	/// Microseconds since this client started (the trailing 8-byte clock).
	pub fn now_us(&self) -> u64 {
		self.started.elapsed().as_micros() as u64
	}

	/// Is this frame payload a connected-phase system message? Application
	/// data always carries bit 7, so only the known ids below 0x80 match.
	pub fn is_system_message(payload: &[u8]) -> bool {
		match payload.first() {
			Some(0x00 | 0x03 | 0x51..=0x56) => payload.len() >= 9,
			Some(0x10 | 0x13) => payload.len() >= 30,
			Some(0x15 | 0x16) => true,
			_ => false,
		}
	}

	/// Handle (and answer) one system message. Returns true when the
	/// payload was consumed and must not reach the application layer.
	pub fn system_message(&mut self, payload: &[u8]) -> bool {
		match payload.first().copied() {
			Some(0x10) => {
				self.on_accepted(payload);
				true
			}
			Some(ID_CONNECTED_PING) => {
				self.on_ping(payload);
				true
			}
			Some(ID_CONNECTED_PONG) => {
				self.on_pong(payload);
				true
			}
			Some(id @ 0x51..=0x53) => {
				self.on_clock_sync(id, payload);
				true
			}
			// 0x13 (peer echo) and the clock-sync acks are informational.
			Some(0x54..=0x56 | 0x13) => true,
			// The fork wraps the disconnect notification in the steady
			// stream instead of sending a bare datagram (capture: a 5-byte
			// 0x15 frame right after the join challenge was rejected).
			Some(0x15 | 0x16) => {
				netlog!(net, "server disconnected us (steady id 0x{:02x})", payload[0]);
				self.state = crate::net::raknet::RakState::Disconnected;
				true
			}
			_ => false,
		}
	}

	/// `ID_CONNECTION_REQUEST_ACCEPTED`: reply with the full
	/// `ID_NEW_INCOMING_CONNECTION` and the opening clock-sync burst.
	fn on_accepted(&mut self, payload: &[u8]) {
		let info = parse_accepted(payload);
		netlog!(
			net,
			"steady ACCEPTED: external {:?} request_ms={} server_ms={}",
			info.client_addr,
			info.request_ms,
			info.server_ms
		);
		if info.request_ms != 0 {
			self.request_ms = info.request_ms;
		}
		if info.server_ms != 0 {
			self.server_ms = info.server_ms;
		}
		self.accepted = Some(info);
		if !self.announced {
			self.announce_connection();
		}
	}

	/// The client's half of the connected handshake, byte-shaped like the
	/// capture: NEW_INCOMING, a ping, an MTU-padded ping and the three
	/// clock-sync announcements.
	fn announce_connection(&mut self) {
		let Some(info) = self.accepted else { return };
		let now_ms = self.now_ms();
		let mut out = Vec::with_capacity(102);
		out.push(ID_NEW_INCOMING_CONNECTION);
		// The capture puts the SERVER address in the peer slot (the fork
		// answers with the remote side it believes it is connected to).
		crate::net::raknet::wire::write_address_rbx(&mut out, self.server);
		for _ in 0..10 {
			out.extend_from_slice(&ZERO_ADDR_RBX);
		}
		out.extend_from_slice(&info.server_ms.to_be_bytes());
		out.extend_from_slice(&now_ms.to_be_bytes());
		out.extend_from_slice(&self.now_us().to_be_bytes());
		self.send_reliable(&out);
		self.announced = true;
		netlog!(
			net,
			"NEW_INCOMING_CONNECTION queued ({} bytes, peer {:?}, server_ms={})",
			out.len(),
			self.server,
			info.server_ms
		);

		let mut ping = vec![ID_CONNECTED_PING];
		ping.extend_from_slice(&now_ms.to_be_bytes());
		self.send_unreliable(&ping);

		let mut padded = vec![0u8; PADDED_PING];
		padded[0] = ID_CONNECTED_PING;
		padded[1..9].copy_from_slice(&now_ms.to_be_bytes());
		self.send_reliable_unordered(&padded);

		for id in CLOCK_SYNC_ANNOUNCE {
			let mut m = vec![id];
			m.extend_from_slice(&now_ms.to_be_bytes());
			self.send_unreliable(&m);
		}
		self.flush();
		netlog!(
			net,
			"connected handshake burst sent (ping + {}B MTU probe + clock sync)",
			PADDED_PING
		);
	}

	/// Ping: echo the peer's millisecond timestamp, add ours and the µs
	/// clock. An MTU-padded ping gets an equally padded pong (reliable, as
	/// in the capture).
	fn on_ping(&mut self, payload: &[u8]) {
		let peer_ms = ms_at(payload, 1);
		if peer_ms != 0 {
			self.server_ms = peer_ms;
		}
		let mut pong = Vec::with_capacity(25);
		pong.push(ID_CONNECTED_PONG);
		pong.extend_from_slice(&peer_ms.to_be_bytes());
		pong.extend_from_slice(&self.now_ms().to_be_bytes());
		pong.extend_from_slice(&self.now_us().to_be_bytes());
		if payload.len() > 32 {
			pong.resize(payload.len(), 0);
			self.send_reliable_unordered(&pong);
		} else {
			self.send_unreliable(&pong);
		}
	}

	/// Pong: record the round-trip if we had a ping outstanding.
	fn on_pong(&mut self, payload: &[u8]) {
		let peer_ms = ms_at(payload, 1);
		if peer_ms != 0 {
			self.server_ms = peer_ms;
		}
		if let Some((_, at)) = self.ping_time.take() {
			self.ping_ms = Some(at.elapsed().as_secs_f32() * 1000.0);
			netlog!(net, "pong: {:.1} ms", self.ping_ms.unwrap_or(0.0));
		}
	}

	/// Clock-sync announce (0x51..0x53): answer with the matching ack
	/// (0x54..0x56) carrying the peer's time, ours and the µs clock.
	fn on_clock_sync(&mut self, id: u8, payload: &[u8]) {
		let peer_ms = ms_at(payload, 1);
		if peer_ms != 0 {
			self.server_ms = peer_ms;
		}
		let mut out = Vec::with_capacity(25);
		out.push(id + 3);
		out.extend_from_slice(&peer_ms.to_be_bytes());
		out.extend_from_slice(&self.now_ms().to_be_bytes());
		out.extend_from_slice(&self.now_us().to_be_bytes());
		self.send_unreliable(&out);
	}
}
