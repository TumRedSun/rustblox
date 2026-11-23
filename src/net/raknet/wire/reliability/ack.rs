//! Timers (resends, pings, split expiry), connected control messages, ACK
//! emission and split reassembly.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::super::{put_u24, Frame, Reader};
use super::{DATAGRAM_IS_ACK, DATAGRAM_VALID, MAX_RESENDS, RESEND_MS};
use crate::net::raknet::{
	RakClient, RakState, SplitState, ID_CONNECTED_PING, ID_CONNECTED_PONG, ID_CONNECTION_LOST,
	ID_DISCONNECTION_NOTIFICATION,
};
use crate::netlog;
use std::collections::HashMap;
use std::time::{Duration, Instant};

impl RakClient {
	/// Resend overdue reliable frames; drop dead ones.
	pub fn tick(&mut self) {
		if self.state != RakState::Connected {
			return;
		}
		// Ping every second (the fork's millisecond stamp is big-endian).
		if self.last_ping.elapsed() > Duration::from_secs(1) {
			self.last_ping = Instant::now();
			let mut out = vec![ID_CONNECTED_PING];
			out.extend_from_slice(&self.now_ms().to_be_bytes());
			self.send_frame(Frame::unreliable(out), false);
			self.ping_time = Some((0, Instant::now()));
			netlog!(net, "ping sent");
		}

		// Resends.
		let now = Instant::now();
		let dead: Vec<u32> = self
			.out_queue
			.iter()
			.filter(|(_, f)| f.tries >= MAX_RESENDS)
			.map(|(k, _)| *k)
			.collect();
		for k in dead {
			self.out_queue.remove(&k);
			netlog!(net, "dropping dead reliable frame {}", k);
			self.state = RakState::Disconnected;
		}
		for f in self.out_queue.values_mut() {
			if f.sent.elapsed() > Duration::from_millis(RESEND_MS) {
				f.sent = now;
				f.tries += 1;
				let bytes = f.bytes.clone();
				self.frame_buf.extend_from_slice(&bytes);
			}
		}
		self.flush();

		// Expire stale split assemblies.
		let stale: Vec<u16> = self
			.splits
			.iter()
			.filter(|(_, s)| s.first_seen.elapsed() > Duration::from_secs(30))
			.map(|(k, _)| *k)
			.collect();
		for k in stale {
			self.splits.remove(&k);
		}
	}

	/// Connected-level message ids (ping/pong/disconnect) — never ACKed.
	pub(super) fn handle_control(&mut self, pkt: &[u8]) -> Option<Vec<u8>> {
		match pkt[0] {
			ID_CONNECTED_PING => {
				// Echo pong.
				if pkt.len() >= 9 {
					let mut out = vec![ID_CONNECTED_PONG];
					out.extend_from_slice(&pkt[1..9]);
					self.send_sealed(&out);
				}
				None
			}
			ID_CONNECTED_PONG => {
				if pkt.len() >= 9 && self.ping_time.is_some() {
					let elapsed = self
						.ping_time
						.take()
						.map(|(_, at)| at.elapsed().as_secs_f32() * 1000.0)
						.unwrap_or(0.0);
					self.ping_ms = Some(elapsed);
					netlog!(net, "pong: {:.1} ms", elapsed);
				}
				None
			}
			ID_DISCONNECTION_NOTIFICATION | ID_CONNECTION_LOST => {
				netlog!(net, "server disconnected us (0x{:02x})", pkt[0]);
				self.state = RakState::Disconnected;
				None
			}
			other => {
				// Some servers push small in-band control messages; surface
				// them to the caller for logging/forwarding.
				netlog!(net, "control id 0x{:02x} ({} bytes)", other, pkt.len());
				self.pending.push_back(pkt.to_vec());
				None
			}
		}
	}

	/// Handle an ACK datagram: release every reliable message the named
	/// datagram sequence carried.
	pub(super) fn handle_ack(&mut self, pkt: &[u8]) {
		let mut r = Reader::new(pkt);
		let _flags = r.u8();
		// The record count is BIG-endian (raknet-dec: `count = u16be()`;
		// Anaminus notes it as "probably an oversight" in the original
		// RakNet code). The record VALUES are u24 LE.
		let count = r.u16be().unwrap_or(0);
		for _ in 0..count {
			let kind = match r.u8() {
				Some(k) => k,
				None => break,
			};
			let (lo, hi) = match kind {
				0 => match (r.u24(), r.u24()) {
					(Some(a), Some(b)) => (a, b),
					_ => break,
				},
				_ => match r.u24() {
					Some(a) => (a, a),
					None => break,
				},
			};
			for seq in lo..=hi.min(lo + 8192) {
				if let Some(msgs) = self.sent_msgs.remove(&seq) {
					for m in msgs {
						self.out_queue.remove(&m);
					}
				}
			}
		}
	}

	/// Send the buffered ACKs, compressed into `[lo..hi]` ranges.
	pub(super) fn send_acks(&mut self) {
		if self.ack_buffer.is_empty() {
			return;
		}
		self.ack_buffer.sort_unstable();
		self.ack_buffer.dedup();
		let mut ranges: Vec<(u32, u32)> = Vec::new();
		for &v in &self.ack_buffer {
			match ranges.last_mut() {
				Some((_, hi)) if *hi + 1 == v => *hi = v,
				_ => ranges.push((v, v)),
			}
		}
		let mut out = vec![DATAGRAM_VALID | DATAGRAM_IS_ACK];
		out.extend_from_slice(&(ranges.len() as u16).to_be_bytes());
		for (lo, hi) in ranges {
			if lo != hi {
				out.push(0);
				put_u24(&mut out, lo);
				put_u24(&mut out, hi);
			} else {
				out.push(1);
				put_u24(&mut out, lo);
			}
		}
		self.send_sealed(&out);
		self.ack_buffer.clear();
	}

	/// Insert one split part; return the whole payload once every part is
	/// present.
	pub(super) fn reassemble(&mut self, f: Frame) -> Option<Vec<u8>> {
		let entry = self.splits.entry(f.split_id).or_insert_with(|| SplitState {
			count: f.split_count,
			parts: HashMap::new(),
			first_seen: Instant::now(),
		});
		entry.parts.insert(f.split_index, f.payload);
		if entry.parts.len() == entry.count as usize {
			let st = self.splits.remove(&f.split_id)?;
			let mut data = Vec::new();
			for i in 0..st.count {
				data.extend_from_slice(st.parts.get(&i)?);
			}
			Some(data)
		} else {
			None
		}
	}
}
