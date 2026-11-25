//! Roblox replication stream parser (best-effort, version-tolerant).
//!
//! The live-server replication protocol is proprietary and version-gated.
//! This parser implements the framing layers that are stable knowledge
//! (packet header, zlib-compressed payloads) and walks the data stream
//! tolerantly: anything it cannot confidently decode is skipped and
//! hex-logged (RUSTBLOX_DEBUG=net) instead of desynchronizing the stream.
//!
//! The decoded events feed the DataModel. As live captures converge the
//! unknown structures, the decode rules here get upgraded — the design
//! goal is "never crash, always log".

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::world::{Prop, RefId};

mod body;
mod envelope;
mod inflate;
pub mod join_items;

/// A decoded replication event.
#[derive(Debug, Clone)]
pub enum RepEvent {
	InstanceCreated {
		r: RefId,
		class: String,
		name: String,
		parent: RefId,
	},
	InstanceRemoved {
		r: RefId,
	},
	PropertySet {
		r: RefId,
		name: String,
		value: Prop,
	},
	/// A fork message whose payload was decoded: `inflated` = the body came
	/// out of a zstd frame, `body` = its plaintext length.
	Decoded {
		head: u16,
		body: usize,
		inflated: bool,
	},
	/// Readable strings pulled out of the replication stream (class and
	/// property names from the schema dictionary, part names, usernames).
	Names(Vec<String>),
	/// A verified client→server game-join item list (heads `0xa7`, `0x90`,
	/// `0x8a`): a counted array of 16-byte opaque items. The container is
	/// proven from four live captures; the item semantics are still open
	/// (`docs/GAME-JOIN-RE.md`).
	JoinItemList {
		message_id: u8,
		items: Vec<[u8; join_items::JOIN_ITEM_LEN]>,
	},
	/// Raw packet we could not decode (id + first bytes), kept for diagnostics.
	Unknown {
		packet_type: u32,
		len: usize,
	},
}

pub struct ReplicationParser {
	/// Packets processed (any kind).
	pub packets: u64,
	/// Data packets fully decoded.
	pub decoded: u64,
	/// Chunks we had to skip.
	pub skipped: u64,
	/// Real fork replication packets (`0x83..`/`0x8d..` heads) delivered by
	/// the transport.
	pub replication: u64,
	/// Fork bodies that arrived as zstd frames.
	pub inflated: u64,
	/// Compressed / plaintext byte totals of the fork bodies.
	pub bytes_in: u64,
	pub bytes_out: u64,
	/// Per-head counts of those packets, keyed by the two-byte head.
	pub heads: std::collections::BTreeMap<u16, u64>,
	/// How many hexdumps we already logged (limit the flood).
	pub hexdumps: u32,
}

impl ReplicationParser {
	/// A parser with zeroed counters and an empty head histogram.
	pub fn new() -> ReplicationParser {
		ReplicationParser {
			packets: 0,
			decoded: 0,
			skipped: 0,
			replication: 0,
			inflated: 0,
			bytes_in: 0,
			bytes_out: 0,
			heads: std::collections::BTreeMap::new(),
			hexdumps: 0,
		}
	}
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
