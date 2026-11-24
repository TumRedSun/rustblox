//! Application-message envelope: fork (`0x8x`) vs legacy framing and
//! per-head dispatch into the shared counters and the body walkers.

use super::body::extract_names;
use super::inflate::{inflate, inflate_zstd, ZSTD_MAGIC};
use super::{RepEvent, ReplicationParser};
use crate::netlog;
use crate::world::DataModel;

impl ReplicationParser {
	/// Feed one application payload (after RakNet deframing). Returns events.
	pub fn feed(&mut self, payload: &[u8], dm: &mut DataModel) -> Vec<RepEvent> {
		let mut events = Vec::new();
		self.feed_inner(payload, dm, &mut events, 0);
		events
	}

	fn feed_inner(
		&mut self,
		payload: &[u8],
		dm: &mut DataModel,
		events: &mut Vec<RepEvent>,
		depth: u32,
	) {
		if payload.len() < 4 {
			return;
		}
		self.packets += 1;
		self.bytes_in += payload.len() as u64;
		// The fork's application layer always sets bit 7 in its first byte
		// (`0x81`, `0x83`, `0x8d`, `0x97`, `0xa1`…), while the legacy stream
		// used a little-endian u32 header whose low nibble is the type.
		if payload[0] & 0x80 != 0 {
			self.fork_message(payload, dm, events, depth);
			return;
		}
		self.legacy_message(payload, dm, events, depth);
	}

	/// One fork application message: `[head u8][sub u8]…`, with big bodies
	/// carried as zstd frames (the magic sits at different header offsets
	/// depending on the subtype, so it is located, not assumed).
	fn fork_message(
		&mut self,
		payload: &[u8],
		dm: &mut DataModel,
		events: &mut Vec<RepEvent>,
		_depth: u32,
	) {
		self.replication += 1;
		let head = u16::from_be_bytes([payload[0], payload[1.min(payload.len() - 1)]]);
		*self.heads.entry(head).or_insert(0) += 1;

		// Verified join item lists (`0xa7`/`0x90`/`0x8a`) are counted arrays
		// of 16-byte items, not zstd bodies — decoding them here also stops
		// the name harvester from reporting random items as garbage strings.
		if let Some(list) = super::join_items::parse(payload) {
			self.decoded += 1;
			events.push(RepEvent::JoinItemList { message_id: list.message_id, items: list.items });
			return;
		}

		let z = payload.windows(4).take(24).position(|w| w == ZSTD_MAGIC);
		let body = match z {
			Some(z) => match inflate_zstd(&payload[z..]) {
				Some(b) => {
					self.inflated += 1;
					self.bytes_out += b.len() as u64;
					b
				}
				None => {
					self.skipped += 1;
					if self.hexdumps < 24 {
						self.log_hex(
							&format!(
								"zstd inflate failed for {:02x}{:02x}",
								payload[0],
								payload[1.min(payload.len() - 1)]
							),
							payload,
						);
					}
					events.push(RepEvent::Unknown {
						packet_type: u32::from(head),
						len: payload.len(),
					});
					return;
				}
			},
			None => payload[2..].to_vec(),
		};

		// String table: both the schema dictionary (`97 0d`) and the cluster
		// bodies use `u8 length + ASCII`. Harvest what is readable; classes,
		// property names, part names and usernames all show up this way.
		let names = extract_names(&body);
		if !names.is_empty() {
			for n in &names {
				if dm.strings.len() < 8192 {
					dm.strings.insert(n.clone());
				}
			}
			events.push(RepEvent::Names(names));
		}
		if self.hexdumps < 24 && (payload[0] == 0x97 || payload[1.min(payload.len() - 1)] == 0x0d) {
			self.log_hex(
				&format!(
					"fork {:02x}/{:02x} inflated {} bytes ({} in)",
					payload[0],
					payload[1.min(payload.len() - 1)],
					body.len(),
					payload.len()
				),
				&body,
			);
		}
		events.push(RepEvent::Decoded { head, body: body.len(), inflated: z.is_some() });
	}

	/// Legacy u32-header stream (Studio/TeamTest servers and the older
	/// public fork shape).
	fn legacy_message(
		&mut self,
		payload: &[u8],
		dm: &mut DataModel,
		events: &mut Vec<RepEvent>,
		depth: u32,
	) {
		let header = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
		let ptype = header & 0x0f;
		let compressed = header & 0x8000_0000 != 0;
		let body = &payload[4..];

		let body = if compressed {
			match inflate(body) {
				Some(b) => b,
				None => {
					self.skipped += 1;
					self.log_hex("failed to inflate compressed packet", payload);
					return;
				}
			}
		} else {
			body.to_vec()
		};

		match ptype {
			0 => self.parse_data(&body, dm, events, depth),
			4 | 5 => {
				// Ping/pong style service packets — informational only.
				netlog!(net, "service packet type {} ({} bytes)", ptype, body.len());
			}
			other => {
				events.push(RepEvent::Unknown { packet_type: other, len: body.len() });
				if self.hexdumps < 24 {
					self.hexdumps += 1;
					self.log_hex(&format!("packet type {}", other), &body);
				}
			}
		}
	}

	pub(super) fn log_hex(&mut self, label: &str, data: &[u8]) {
		if self.hexdumps >= 24 {
			return;
		}
		self.hexdumps += 1;
		let take = data.len().min(64);
		let hex: String =
			data[..take].iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ");
		netlog!(net, "HEX {} ({} bytes): {}", label, data.len(), hex);
	}
}
