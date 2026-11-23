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

use crate::netlog;
use crate::world::{DataModel, Prop, RefId};

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
	/// Raw packet we could not decode (id + first bytes), kept for diagnostics.
	Unknown {
		packet_type: u32,
		len: usize,
	},
}

/// zstd frame magic (`28 b5 2f fd`).
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

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

	/// Walk a data packet: a sequence of clusters. Each cluster:
	///   u32 length | u32 cluster kind | body
	/// Known cluster kinds (community knowledge, version-dependent):
	///   1 = instance create, 2 = property set, 3 = instance delete,
	///   5 = event, 7 = stats/heartbeat.
	fn parse_data(
		&mut self,
		body: &[u8],
		dm: &mut DataModel,
		events: &mut Vec<RepEvent>,
		depth: u32,
	) {
		let mut pos = 0usize;
		let mut progress = true;
		while pos + 8 <= body.len() && progress {
			progress = false;
			let len = u32::from_le_bytes([body[pos], body[pos + 1], body[pos + 2], body[pos + 3]])
				as usize;
			if len == 0 || pos + 8 + len > body.len() {
				break;
			}
			let kind =
				u32::from_le_bytes([body[pos + 4], body[pos + 5], body[pos + 6], body[pos + 7]]);
			let start = pos;
			pos += 8;
			let chunk = &body[pos..pos + len];
			match kind {
				1 => {
					if let Some(ev) = self.parse_create(chunk, dm) {
						events.push(ev);
						self.decoded += 1;
						progress = true;
					} else {
						self.skipped += 1;
						self.log_hex("cluster create undecodable", &body[start..pos + len]);
					}
				}
				2 => {
					if let Some(ev) = self.parse_property(chunk, dm) {
						events.push(ev);
						self.decoded += 1;
						progress = true;
					} else {
						self.skipped += 1;
						self.log_hex("cluster property undecodable", &body[start..pos + len]);
					}
				}
				3 => {
					if chunk.len() >= 8 {
						let r = i64::from_le_bytes(chunk[0..8].try_into().unwrap());
						dm.remove(r);
						events.push(RepEvent::InstanceRemoved { r });
						self.decoded += 1;
						progress = true;
					}
				}
				_ => {
					self.skipped += 1;
					if self.hexdumps < 24 {
						self.hexdumps += 1;
						netlog!(net, "cluster kind {} ({} bytes) at {}", kind, len, pos);
					}
					progress = true; // keep walking; the framing looks sane
				}
			}
			pos += len;
		}
		if depth < 1 && body.len() >= 4 && self.skipped == 0 && self.decoded == 0 {
			self.log_hex("data packet produced nothing", body);
		}
	}

	/// Instance create cluster (tolerant):
	///   i64 ref | u8 string-kind | u16 len | class | u16 len | name | i64 parent
	fn parse_create(&mut self, chunk: &[u8], dm: &mut DataModel) -> Option<RepEvent> {
		if chunk.len() < 8 + 1 + 2 {
			return None;
		}
		let mut pos = 0usize;
		let r = i64::from_le_bytes(match chunk[0..8].try_into() {
			Ok(v) => v,
			Err(_) => return None,
		});
		pos += 8;
		// Try: u16 len + class + u16 len + name + i64 parent.
		let (class, pos2) = read_str16(chunk, pos)?;
		let (name, pos3) = read_str16(chunk, pos2)?;
		if pos3 + 8 > chunk.len() {
			return None;
		}
		let parent = i64::from_le_bytes(match chunk[pos3..pos3 + 8].try_into() {
			Ok(v) => v,
			Err(_) => return None,
		});
		let _ = pos;
		dm.create(r, &class, &name, parent);
		Some(RepEvent::InstanceCreated { r, class, name, parent })
	}

	/// Property set cluster (tolerant):
	///   i64 ref | u16 len name | u8 value kind | value bytes
	/// Value kinds: 1 string(u32), 2 bool, 3 f32, 4 f64, 5 vec3, 6 cframe,
	/// 7 color3, 8 i32, 9 ref, 10 color3u8, 11 token(u32), 12 vec2.
	fn parse_property(&mut self, chunk: &[u8], dm: &mut DataModel) -> Option<RepEvent> {
		if chunk.len() < 8 + 2 {
			return None;
		}
		let r = i64::from_le_bytes(match chunk[0..8].try_into() {
			Ok(v) => v,
			Err(_) => return None,
		});
		let (name, mut pos) = read_str16(chunk, 8)?;
		if pos >= chunk.len() {
			return None;
		}
		let kind = chunk[pos];
		pos += 1;
		let rest = &chunk[pos..];
		let value = match kind {
			1 => {
				if rest.len() < 4 {
					return None;
				}
				let n = u32::from_le_bytes(match rest[0..4].try_into() {
					Ok(v) => v,
					Err(_) => return None,
				}) as usize;
				if rest.len() < 4 + n {
					return None;
				}
				Prop::Str(String::from_utf8_lossy(&rest[4..4 + n]).to_string())
			}
			2 => Prop::Bool(rest.first()? != &0),
			3 => Prop::F32(f32::from_le_bytes(match rest.get(0..4)?.try_into() {
				Ok(v) => v,
				Err(_) => return None,
			})),
			4 => Prop::F64(f64::from_le_bytes(match rest.get(0..8)?.try_into() {
				Ok(v) => v,
				Err(_) => return None,
			})),
			5 => Prop::Vec3([
				f32::from_le_bytes(match rest.get(0..4)?.try_into() {
					Ok(v) => v,
					Err(_) => return None,
				}),
				f32::from_le_bytes(match rest.get(4..8)?.try_into() {
					Ok(v) => v,
					Err(_) => return None,
				}),
				f32::from_le_bytes(match rest.get(8..12)?.try_into() {
					Ok(v) => v,
					Err(_) => return None,
				}),
			]),
			6 => {
				if rest.len() < 48 {
					return None;
				}
				let mut f = [0f32; 12];
				for (i, v) in f.iter_mut().enumerate() {
					*v = f32::from_le_bytes(match rest[i * 4..i * 4 + 4].try_into() {
						Ok(v) => v,
						Err(_) => return None,
					});
				}
				// Layout guess: pos (3) + rotation rows (9).
				let pos = [f[0], f[1], f[2]];
				let rot = [f[3], f[4], f[5], f[6], f[7], f[8], f[9], f[10], f[11]];
				Prop::CFrame { pos, rot }
			}
			7 => Prop::Color3([
				f32::from_le_bytes(match rest.get(0..4)?.try_into() {
					Ok(v) => v,
					Err(_) => return None,
				}),
				f32::from_le_bytes(match rest.get(4..8)?.try_into() {
					Ok(v) => v,
					Err(_) => return None,
				}),
				f32::from_le_bytes(match rest.get(8..12)?.try_into() {
					Ok(v) => v,
					Err(_) => return None,
				}),
			]),
			8 => Prop::I32(i32::from_le_bytes(match rest.get(0..4)?.try_into() {
				Ok(v) => v,
				Err(_) => return None,
			})),
			9 => Prop::Ref(i64::from_le_bytes(match rest.get(0..8)?.try_into() {
				Ok(v) => v,
				Err(_) => return None,
			})),
			10 => Prop::Color3u8([*rest.first()?, *rest.get(1)?, *rest.get(2)?]),
			11 => Prop::Token(u32::from_le_bytes(match rest.get(0..4)?.try_into() {
				Ok(v) => v,
				Err(_) => return None,
			})),
			12 => Prop::Vec2([
				f32::from_le_bytes(match rest.get(0..4)?.try_into() {
					Ok(v) => v,
					Err(_) => return None,
				}),
				f32::from_le_bytes(match rest.get(4..8)?.try_into() {
					Ok(v) => v,
					Err(_) => return None,
				}),
			]),
			_ => Prop::Raw(rest.to_vec()),
		};
		dm.set_prop(r, &name, value.clone());
		Some(RepEvent::PropertySet { r, name, value })
	}

	fn log_hex(&mut self, label: &str, data: &[u8]) {
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

fn read_str16(buf: &[u8], pos: usize) -> Option<(String, usize)> {
	if pos + 2 > buf.len() {
		return None;
	}
	let n = u16::from_le_bytes([buf[pos], buf[pos + 1]]) as usize;
	if pos + 2 + n > buf.len() {
		return None;
	}
	Some((String::from_utf8_lossy(&buf[pos + 2..pos + 2 + n]).to_string(), pos + 2 + n))
}

fn inflate(data: &[u8]) -> Option<Vec<u8>> {
	use std::io::Read;
	let mut out = Vec::new();
	match flate2::read::ZlibDecoder::new(data).take(64 * 1024 * 1024).read_to_end(&mut out) {
		Ok(_) => Some(out),
		Err(_) => {
			// Try raw deflate as a fallback.
			let mut out2 = Vec::new();
			match flate2::read::DeflateDecoder::new(data)
				.take(64 * 1024 * 1024)
				.read_to_end(&mut out2)
			{
				Ok(_) => Some(out2),
				Err(_) => None,
			}
		}
	}
}

/// Inflate one zstd frame (`28 b5 2f fd …`). ruzstd stops at the frame end;
/// the output is capped so a hostile length cannot exhaust memory.
fn inflate_zstd(data: &[u8]) -> Option<Vec<u8>> {
	use std::io::Read;
	let dec = ruzstd::streaming_decoder::StreamingDecoder::new(data).ok()?;
	let mut out = Vec::new();
	dec.take(32 * 1024 * 1024).read_to_end(&mut out).ok()?;
	if out.is_empty() {
		None
	} else {
		Some(out)
	}
}

/// Harvest the fork's length-prefixed ASCII strings from a replication body.
/// Two conventions appear: `u8 len` (the schema dictionary `97 0d` and the
/// cluster stream) and `u16 LE len` (the policy/flag message `93 00`). Used
/// for class and property names, part names and usernames.
fn extract_names(buf: &[u8]) -> Vec<String> {
	let mut out = Vec::new();
	let mut i = 0usize;
	while i < buf.len() {
		if let Some((s, used)) = string_at(buf, i) {
			out.push(s);
			i += used;
			continue;
		}
		i += 1;
	}
	out
}

/// Try to read one length-prefixed printable ASCII string at `i`.
fn string_at(buf: &[u8], i: usize) -> Option<(String, usize)> {
	let candidates: [(usize, usize); 2] =
		[(buf[i] as usize, 1), (u16::from_le_bytes([buf[i], *buf.get(i + 1)?]) as usize, 2)];
	for (n, hdr) in candidates {
		if !(2..=64).contains(&n) || i + hdr + n > buf.len() {
			continue;
		}
		let s = &buf[i + hdr..i + hdr + n];
		if s.iter().all(|c| (0x20..0x7f).contains(c)) {
			if let Ok(t) = std::str::from_utf8(s) {
				return Some((t.to_string(), hdr + n));
			}
		}
	}
	None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
	use super::*;

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
		assert!(
			dm.strings.contains("rbxassetid://6843051212"),
			"asset id not harvested: {:?}",
			names
		);
		assert!(dm.strings.contains("Holder"), "part name not harvested");
		assert_eq!(p.bytes_out, 173, "inflated body length");
	}
}
