//! Receive path: socket pump, box opening, datagram dispatch and ordered
//! delivery.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::super::{frame_encoded_len, Frame, Reader};
use super::{is_ordered, DATAGRAM_IS_ACK, DATAGRAM_IS_NAK, DATAGRAM_VALID};
use crate::net::raknet::{RakClient, RakState};
use crate::netlog;

impl RakClient {
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
					self.last_recv = std::time::Instant::now();
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

	/// Dispatch one datagram by its first byte and walk its frames.
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
		self.handle_frames(r.rest());
	}

	/// Decode every frame of a datagram body and hand it to delivery.
	fn handle_frames(&mut self, rest: &[u8]) {
		let mut pos = 0usize;
		while pos < rest.len() {
			let f = match Frame::decode(&rest[pos..]) {
				Some(f) => f,
				None => {
					netlog!(net, "garbled frame at {} ({} bytes left)", pos, rest.len() - pos);
					return;
				}
			};
			pos += frame_encoded_len(&f);
			let ordered = is_ordered(f.reliability);
			let order = f.order_index;
			if f.has_split {
				if let Some(assembled) = self.reassemble(f) {
					self.deliver(assembled, ordered, order);
				}
			} else {
				self.deliver(f.payload, ordered, order);
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

	/// Hand a payload to the system layer or the application queue.
	fn route(&mut self, payload: Vec<u8>) {
		if self.crypto_connected && self.system_message(&payload) {
			return;
		}
		self.pending.push_back(payload);
	}
}
