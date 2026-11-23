//! Cluster body decoding: the data-packet walk, instance create, property
//! set, and the length-prefixed string harvesting they share.

use super::{RepEvent, ReplicationParser};
use crate::netlog;
use crate::world::{DataModel, Prop};

impl ReplicationParser {
	/// Walk a data packet: a sequence of clusters. Each cluster:
	///   u32 length | u32 cluster kind | body
	/// Known cluster kinds (community knowledge, version-dependent):
	///   1 = instance create, 2 = property set, 3 = instance delete,
	///   5 = event, 7 = stats/heartbeat.
	pub(super) fn parse_data(
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

/// Harvest the fork's length-prefixed ASCII strings from a replication body.
/// Two conventions appear: `u8 len` (the schema dictionary `97 0d` and the
/// cluster stream) and `u16 LE len` (the policy/flag message `93 00`). Used
/// for class and property names, part names and usernames.
pub(super) fn extract_names(buf: &[u8]) -> Vec<String> {
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
