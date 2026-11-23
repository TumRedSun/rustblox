//! Connected-phase reliability: reliable/ordered frame queueing, resend
//! bookkeeping, ACK/NAK handling, split-frame reassembly and ordered
//! delivery. Everything here operates on an already-connected
//! [`RakClient`]; the offline + connected handshake lives in
//! [`super::super::handshake`].

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::{frame_encoded_len, put_u24, Frame, OutFrame, Reader};
use crate::net::raknet::{
	RakClient, RakState, SplitState, ID_CONNECTED_PING, ID_CONNECTED_PONG, ID_CONNECTION_LOST,
	ID_DISCONNECTION_NOTIFICATION,
};
use crate::netlog;
use std::collections::HashMap;
use std::time::{Duration, Instant};

const DATAGRAM_VALID: u8 = 0x80;
const DATAGRAM_IS_ACK: u8 = 0x40;
const DATAGRAM_IS_NAK: u8 = 0x20;

const RESEND_MS: u64 = 350;
const MAX_RESENDS: u32 = 12;

impl RakClient {
	/// Queue a reliable-ordered datagram frame.
	pub fn send_reliable(&mut self, payload: &[u8]) {
		self.send_frame(Frame::reliable_ordered(payload.to_vec()), true);
	}

	/// Queue a reliable (but NOT ordered) datagram frame — the shape the
	/// fork uses for its padded ping/pong probes.
	pub fn send_reliable_unordered(&mut self, payload: &[u8]) {
		let mut f = Frame::unreliable(payload.to_vec());
		f.reliability = 2;
		self.send_frame(f, true);
	}

	/// Queue an unreliable datagram frame.
	pub fn send_unreliable(&mut self, payload: &[u8]) {
		self.send_frame(Frame::unreliable(payload.to_vec()), false);
	}

	fn send_frame(&mut self, mut frame: Frame, resend: bool) {
		if self.state != RakState::Connected {
			return;
		}
		let order = if matches!(frame.reliability, 1 | 3 | 4 | 7) {
			let o = self.order_index;
			self.order_index = self.order_index.wrapping_add(1);
			o
		} else {
			0
		};
		frame.order_index = order;

		// Split if it does not fit a datagram (leave room for datagram hdr).
		let max_payload = (self.mtu as usize).saturating_sub(4 + 16 + 64);
		if frame.payload.len() > max_payload {
			// Every split part is its own reliable message: the capture
			// numbers the two halves of the 1160-byte padded ping 2 and 3
			// (not 2 and 2), and the server ACKs them individually.
			let chunks: Vec<Vec<u8>> =
				frame.payload.chunks(max_payload).map(|c| c.to_vec()).collect();
			let count = chunks.len() as u32;
			let split_id = self.split_id;
			self.split_id = self.split_id.wrapping_add(1);
			for (i, chunk) in chunks.into_iter().enumerate() {
				let seq = self.reliability_index;
				self.reliability_index = self.reliability_index.wrapping_add(1);
				let f = Frame {
					reliability: frame.reliability,
					has_split: true,
					seq,
					order_index: order,
					order_channel: 0,
					split_id,
					split_count: count,
					split_index: i as u32,
					payload: chunk,
				};
				let mut bytes = Vec::new();
				f.encode(&mut bytes);
				if resend && matches!(frame.reliability, 2 | 3 | 4 | 6 | 7) {
					self.out_queue.insert(
						seq,
						OutFrame {
							reliability_index: seq,
							bytes: bytes.clone(),
							sent: Instant::now(),
							tries: 0,
						},
					);
				}
				self.frame_buf.extend_from_slice(&bytes);
			}
		} else {
			let seq = self.reliability_index;
			self.reliability_index = self.reliability_index.wrapping_add(1);
			frame.seq = seq;
			let mut bytes = Vec::with_capacity(frame.payload.len() + 8);
			frame.encode(&mut bytes);
			if resend && matches!(frame.reliability, 2 | 3 | 4 | 6 | 7) {
				self.out_queue.insert(
					seq,
					OutFrame {
						reliability_index: seq,
						bytes: bytes.clone(),
						sent: Instant::now(),
						tries: 0,
					},
				);
			}
			self.frame_buf.extend_from_slice(&bytes);
		}
	}

	/// Flush queued frames into datagrams (call every tick). Frames are
	/// packed whole — a frame never spans datagrams.
	pub fn flush(&mut self) {
		if self.frame_buf.is_empty() {
			return;
		}
		if self.state != RakState::Connected {
			self.frame_buf.clear();
			return;
		}
		let buf = std::mem::take(&mut self.frame_buf);
		let max = (self.mtu as usize).saturating_sub(4);
		let mut dg: Vec<u8> = Vec::with_capacity(max);
		let mut dg_msgs: Vec<u32> = Vec::new();
		let mut pos = 0usize;
		let mut seq = self.datagram_seq;
		while pos < buf.len() {
			let decoded = Frame::decode(&buf[pos..]);
			let flen = decoded.as_ref().map(frame_encoded_len).unwrap_or(buf.len() - pos);
			if !dg.is_empty() && dg.len() + 4 + flen > max {
				self.send_datagram(&dg, &mut seq, &mut dg_msgs);
				dg.clear();
			}
			if let Some(f) = &decoded {
				if matches!(f.reliability, 2 | 3 | 4 | 6 | 7) {
					dg_msgs.push(f.seq);
				}
			}
			let end = (pos + flen).min(buf.len());
			dg.extend_from_slice(&buf[pos..end]);
			pos = end;
		}
		if !dg.is_empty() {
			self.send_datagram(&dg, &mut seq, &mut dg_msgs);
		}
		self.datagram_seq = seq;
	}

	fn send_datagram(&mut self, dg: &[u8], seq: &mut u32, msgs: &mut Vec<u32>) {
		let mut pkt = Vec::with_capacity(dg.len() + 4);
		pkt.push(DATAGRAM_VALID | 0x04);
		put_u24(&mut pkt, *seq);
		pkt.extend_from_slice(dg);
		// ACKs name DATAGRAM sequence numbers, and one ACK covers every
		// reliable message the datagram carried — remember the mapping or
		// the resend queue never drains (the old code compared ack ranges
		// against message numbers, which only coincided at session start).
		if msgs.is_empty() {
			self.sent_msgs.remove(seq);
		} else {
			self.sent_msgs.insert(*seq, std::mem::take(msgs));
		}
		self.send_sealed(&pkt);
		*seq = seq.wrapping_add(1);
	}

	/// Send one connected-phase datagram payload, applying the steady
	/// counter box when the session is encrypted. ACKs and control replies
	/// MUST go through here: the server drops clear ACK datagrams (it
	/// kept retransmitting ACCEPTED forever while ours left unsealed).
	fn send_sealed(&mut self, pkt: &[u8]) {
		if self.crypto_connected {
			let sealed = self.early.as_mut().map(|ec| ec.seal_counter(pkt));
			if let Some(s) = sealed {
				let _ = self.send_raw(&s);
				return;
			}
		}
		let _ = self.send_raw(pkt);
	}

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
		for (_, f) in self.out_queue.iter_mut() {
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

	// -----------------------------------------------------------------
	// Receiving
	// -----------------------------------------------------------------

	/// Pump the socket: returns the next application payload if available.
	/// Handles ACKs, pings, splits, ordered delivery and the fork's
	/// connected-phase system messages internally.
	pub fn poll(&mut self) -> Option<Vec<u8>> {
		if self.state != RakState::Connected {
			return None;
		}
		if self.pending.is_empty() {
			let recv = self.socket.recv_from(&mut self.buf);
			let n = match recv {
				Ok((n, from)) if from == self.server => {
					self.last_recv = Instant::now();
					n
				}
				Ok(_) => return None,
				Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => return None,
				Err(_) => return None,
			};
			let pkt: Vec<u8> = self.strip_incoming(self.buf[..n].to_vec(), "poll");
			self.ingest(&pkt);
		}
		self.pending.pop_front()
	}

	/// Feed one RUPP-stripped datagram: open the steady box when the
	/// session is encrypted, then handle ACKs, control messages, frames and
	/// system messages. Also used by `connect` for the datagram that
	/// carries the framed ACCEPTED.
	pub(crate) fn ingest(&mut self, pkt: &[u8]) {
		let opened: Vec<u8>;
		let pkt: &[u8] = if self.crypto_connected {
			match self.early.as_ref().map(|ec| ec.open_counter(pkt)) {
				Some(Some((pt, counter))) => {
					netlog!(
						net,
						"poll: box opened (counter {:#x}, pt {} bytes)",
						counter,
						pt.len()
					);
					opened = pt;
					&opened
				}
				Some(None) => {
					netlog!(net, "poll: datagram ({}) failed counter-box decrypt", pkt.len());
					pkt
				}
				None => pkt,
			}
		} else {
			pkt
		};
		self.handle_packet(pkt);
	}

	/// Plaintext half of [`RakClient::ingest`] (the connect-phase loop has
	/// already opened the box of the datagram it is looking at).
	pub(crate) fn ingest_plain(&mut self, pkt: &[u8]) {
		self.handle_packet(pkt);
	}

	fn handle_packet(&mut self, pkt: &[u8]) {
		let first = match pkt.first() {
			Some(&f) => f,
			None => return,
		};
		if first & DATAGRAM_VALID == 0 {
			// Connected-level message ids (ping/pong/disconnect).
			self.handle_control(pkt);
			return;
		}
		if first & DATAGRAM_IS_ACK != 0 {
			self.handle_ack(pkt);
			return;
		}
		if first & DATAGRAM_IS_NAK != 0 {
			return;
		}
		let mut r = Reader::new(pkt);
		let _flags = match r.u8() {
			Some(v) => v,
			None => return,
		};
		let dg_seq = match r.u24() {
			Some(v) => v,
			None => return,
		};
		if !self.received_datagrams.insert(dg_seq) {
			// Duplicate datagram; re-ack but do not process.
			self.ack_buffer.push(dg_seq);
			self.send_acks();
			return;
		}
		self.ack_buffer.push(dg_seq);
		self.send_acks();

		let rest = r.rest();
		let mut pos = 0usize;
		while pos < rest.len() {
			match Frame::decode(&rest[pos..]) {
				Some(f) => {
					let flen = frame_encoded_len(&f);
					pos += flen;
					let ordered = matches!(f.reliability, 1 | 3 | 4 | 7);
					let order = f.order_index;
					if f.has_split {
						if let Some(assembled) = self.reassemble(f) {
							self.deliver(assembled, ordered, order);
						}
					} else {
						self.deliver(f.payload, ordered, order);
					}
				}
				None => {
					netlog!(net, "garbled frame at {} ({} bytes left)", pos, rest.len() - pos);
					break;
				}
			}
		}
	}

	/// Route one complete message: ordered messages are held until their
	/// turn on the ordering channel, connected-phase system messages are
	/// answered here, everything else goes up to the application layer
	/// (the replication parser).
	fn deliver(&mut self, body: Vec<u8>, ordered: bool, order: u32) {
		if !ordered {
			self.route(body);
			return;
		}
		if (order as i64) < (self.delivered_ordered as i64) {
			return; // already delivered (duplicate)
		}
		self.ordered_hold.insert(order, body);
		while let Some(next) = self.ordered_hold.remove(&self.delivered_ordered) {
			self.delivered_ordered = self.delivered_ordered.wrapping_add(1);
			self.route(next);
		}
	}

	fn route(&mut self, payload: Vec<u8>) {
		if self.crypto_connected && self.system_message(&payload) {
			return;
		}
		self.pending.push_back(payload);
	}

	fn handle_control(&mut self, pkt: &[u8]) -> Option<Vec<u8>> {
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
					let sent_ms = self.ping_time.as_ref().map(|(t, _)| *t).unwrap_or(0);
					let _ = pkt[1..9];
					let elapsed = self
						.ping_time
						.take()
						.map(|(_, at)| at.elapsed().as_secs_f32() * 1000.0)
						.unwrap_or(0.0);
					let _ = sent_ms;
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

	fn handle_ack(&mut self, pkt: &[u8]) {
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

	fn send_acks(&mut self) {
		if self.ack_buffer.is_empty() {
			return;
		}
		// Compress into [lo..hi] ranges.
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

	fn reassemble(&mut self, f: Frame) -> Option<Vec<u8>> {
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

	/// Number of payloads buffered behind the ordered delivery head.
	pub fn pending_ordered(&self) -> usize {
		self.ordered_hold.len()
	}
}

#[cfg(test)]
#[path = "reliability/tests.rs"]
mod tests;
