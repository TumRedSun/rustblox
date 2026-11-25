//! RakNet client transport — the reliable-UDP protocol Roblox game servers
//! speak underneath their replication stream.
//!
//! Implements the client side of the standard RakNet wire protocol:
//! offline handshake (OPEN_CONNECTION_REQUEST/REPLY 1-2), connected
//! handshake (CONNECTION_REQUEST / ACCEPTED / NEW_INCOMING_CONNECTION),
//! reliability (reliable + reliable-ordered frames), split-frame
//! reassembly, ACK bookkeeping, resend queue and connected ping/pong.
//!
//! The application payload stream that rides on top of RakNet is Roblox's
//! replication protocol, parsed in `crate::roblox::replication`.
//!
//! Split by responsibility: [`wire`] holds frame/address encode-decode and
//! connected-phase reliability, [`handshake`] holds the offline + connected
//! handshake flow (the only place `RakClient` touches the socket directly
//! during setup — a dedicated `socket.rs` was judged unnecessary, since the
//! `UdpSocket` is a single struct field used only through `send_raw`/`poll`,
//! not a reusable abstraction).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod handshake;
pub mod system;
pub mod wire;

pub use handshake::netstack_probe;

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{SocketAddr, UdpSocket};
use std::time::Instant;
use wire::OutFrame;

// ---------------------------------------------------------------------------
// Message IDs (RakNet MessageIdentifiers.h)
// ---------------------------------------------------------------------------

pub const ID_CONNECTED_PING: u8 = 0x00;
pub const ID_CONNECTED_PONG: u8 = 0x03;
pub const ID_OPEN_CONNECTION_REQUEST_1: u8 = 0x05;
pub const ID_OPEN_CONNECTION_REPLY_1: u8 = 0x06;
pub const ID_OPEN_CONNECTION_REQUEST_2: u8 = 0x07;
pub const ID_OPEN_CONNECTION_REPLY_2: u8 = 0x08;
pub const ID_CONNECTION_REQUEST: u8 = 0x09;
pub const ID_CONNECTION_REQUEST_ACCEPTED: u8 = 0x10;
pub const ID_CONNECTION_ATTEMPT_FAILED: u8 = 0x11;
pub const ID_ALREADY_CONNECTED: u8 = 0x12;
pub const ID_NEW_INCOMING_CONNECTION: u8 = 0x13;
pub const ID_NO_FREE_INCOMING_CONNECTIONS: u8 = 0x14;
pub const ID_DISCONNECTION_NOTIFICATION: u8 = 0x15;
pub const ID_CONNECTION_LOST: u8 = 0x16;
pub const ID_CONNECTION_BANNED: u8 = 0x17;

/// RakNet offline message data id (16 bytes) shared by all offline packets.
///
/// This is the canonical OFFLINE_MESSAGE_DATA_ID from RakNet's
/// MessageIdentifiers.h, and Roblox's fork keeps it byte-for-byte (confirmed
/// against the roblox-dissector `OfflineMessageID`). An earlier build shipped
/// a guessed tail (`fe` × 12) — servers silently drop handshake packets whose
/// magic doesn't match, which was exactly the "timed out during offline
/// handshake" failure.
pub const OFFLINE_MAGIC: [u8; 16] = [
	0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56, 0x78,
];

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum RakError {
	Network(String),
	TimedOut(&'static str),
	Rejected(&'static str),
	AlreadyConnected,
	Banned,
	Closed,
}

impl std::fmt::Display for RakError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			RakError::Network(m) => write!(f, "network: {}", m),
			RakError::TimedOut(s) => write!(f, "timed out during {}", s),
			RakError::Rejected(s) => write!(f, "connection rejected: {}", s),
			RakError::AlreadyConnected => write!(f, "already connected"),
			RakError::Banned => write!(f, "banned"),
			RakError::Closed => write!(f, "connection closed"),
		}
	}
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RakState {
	Connecting,
	Connected,
	Disconnected,
}

/// Which offline-id generation won the handshake: the Roblox fork ids
/// (0x7B/0x7E/0x78/0x7D, big-endian, XOR addresses) or vanilla RakNet
/// (0x05/0x06/0x07/0x08 — kept as fallback for direct dials like TeamTest
/// and local servers). Phase 3 shares its message ids across generations,
/// but the address field order and endianness differ — replies are parsed
/// with the layout of whoever won phase 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkGen {
	Roblox,
	Vanilla,
}

pub struct RakClient {
	pub(super) socket: UdpSocket,
	pub server: SocketAddr,
	pub guid: u64,
	pub server_guid: u64,
	pub mtu: u16,
	pub state: RakState,
	pub ping_ms: Option<f32>,
	/// Datagram envelope confirmed by the handshake (the live-capture client
	/// shape `01 00 00 1f 01 11 <cls> <tag> 02 06 <rcc ip:port>`, a legacy
	/// probe candidate, or bare/raw when no envelope is needed). Applied to
	/// every datagram sent after it was confirmed — the proxy binds the
	/// client->RCC mapping to the exact clear-header shape.
	pub(super) envelope: Option<crate::net::udmux::Envelope>,
	/// The 16-byte tag we put in our pre-connect RUPP headers (client's own
	/// choice — the live capture shows it matches no join-script field).
	pub(super) client_tag: [u8; 16],
	/// The server's 16-byte tag, learned from any incoming RUPP header.
	/// Once the connection is up, both sides send the SAME tag.
	pub(super) server_tag: Option<[u8; 16]>,
	/// Early-encryption state (join hypothesis (c)): key schedule + the
	/// crypto OCR2/OpenReply2 handshake. `None` = legacy unencrypted fork
	/// handshake (direct dials without join-script crypto material).
	pub(super) early: Option<crate::net::crypto::EarlyCrypto>,
	/// Connected-phase framing: after ACCEPTED the session switches from
	/// the tagged handshake boxes (`nonce12+ct+tag16`) to the stream shape
	/// the pcap proves out (`nonce12+ct`, no tag — the 56-byte keepalives
	/// are 31+12+13 = envelope+nonce+13-byte ping datagram).
	pub(super) crypto_connected: bool,
	/// Offline-id generation that answered phase 1.
	pub(super) fork: ForkGen,

	// Connected-phase system bookkeeping (see [`system`]): the fork's
	// steady stream interleaves RakNet-level messages with application data
	// and the server wants them answered before it starts replicating.
	/// When this client started — the base of the 8-byte microsecond clock
	/// every system message carries.
	pub(super) started: Instant,
	/// Milliseconds announced in the steady CONNECTION_REQUEST (the ACK
	/// echoes it back).
	pub(super) request_ms: u64,
	/// Newest server millisecond timestamp seen on the wire.
	pub(super) server_ms: u64,
	/// Parsed `ID_CONNECTION_REQUEST_ACCEPTED` of the steady stream.
	pub(super) accepted: Option<system::Accepted>,
	/// Whether `ID_NEW_INCOMING_CONNECTION` was already answered.
	pub(super) announced: bool,

	// Send side
	pub(super) reliability_index: u32,
	pub(super) order_index: u32,
	pub(super) datagram_seq: u32,
	pub(super) split_id: u16,
	pub(in crate::net::raknet) out_queue: HashMap<u32, OutFrame>,
	/// Datagram seq -> reliable message numbers it carried (ACK routing).
	pub(in crate::net::raknet) sent_msgs: HashMap<u32, Vec<u32>>,
	pub(super) frame_buf: Vec<u8>, // frames awaiting a datagram flush

	// Receive side
	pub(super) received_datagrams: HashSet<u32>,
	pub(super) ack_buffer: Vec<u32>,
	pub(super) delivered_ordered: u32,
	pub(super) ordered_hold: HashMap<u32, Vec<u8>>,
	pub(super) splits: HashMap<u16, SplitState>,

	// Assembled application payloads waiting to be returned by poll().
	pub(super) pending: VecDeque<Vec<u8>>,

	// Ping
	pub(super) last_ping: Instant,
	pub(super) ping_time: Option<(u64, Instant)>,

	pub(super) last_recv: Instant,
	pub(super) buf: Vec<u8>,
}

pub(super) struct SplitState {
	pub(super) count: u32,
	pub(super) parts: HashMap<u32, Vec<u8>>,
	pub(super) first_seen: Instant,
}

impl RakClient {
	/// Connection idle time (used by the session to detect death).
	pub fn idle(&self) -> std::time::Duration {
		self.last_recv.elapsed()
	}

	pub fn close(&mut self) {
		if self.state == RakState::Connected {
			let out = [ID_DISCONNECTION_NOTIFICATION];
			let _ = self.send_raw(&out);
			self.flush();
		}
		self.state = RakState::Disconnected;
	}
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
