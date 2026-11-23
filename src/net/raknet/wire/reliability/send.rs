//! Outgoing path: frame queueing, datagram packing and the sealed send.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::super::{frame_encoded_len, put_u24, Frame, OutFrame};
use super::{is_ordered, is_reliable, DATAGRAM_VALID};
use crate::net::raknet::{RakClient, RakState};
use std::time::Instant;

impl RakClient {
	/// Stamp the frame with the next ordering index (0 for kinds that do not
	/// order), split it when it exceeds a datagram, and queue it.
	pub(super) fn send_frame(&mut self, mut frame: Frame, resend: bool) {
		if self.state != RakState::Connected {
			return;
		}
		frame.order_index = self.next_order(frame.reliability);

		// Split if it does not fit a datagram (leave room for datagram hdr).
		let max_payload = (self.mtu as usize).saturating_sub(4 + 16 + 64);
		if frame.payload.len() > max_payload {
			self.queue_split(frame, resend, max_payload);
		} else {
			self.queue_single(frame, resend);
		}
	}

	/// Allocate the next ordering index when the kind is ordered.
	fn next_order(&mut self, reliability: u8) -> u32 {
		if !is_ordered(reliability) {
			return 0;
		}
		let o = self.order_index;
		self.order_index = self.order_index.wrapping_add(1);
		o
	}

	/// Queue one whole frame.
	fn queue_single(&mut self, mut frame: Frame, resend: bool) {
		let seq = self.reliability_index;
		self.reliability_index = self.reliability_index.wrapping_add(1);
		frame.seq = seq;
		let mut bytes = Vec::with_capacity(frame.payload.len() + 8);
		frame.encode(&mut bytes);
		if resend && is_reliable(frame.reliability) {
			self.remember(seq, &bytes);
		}
		self.frame_buf.extend_from_slice(&bytes);
	}

	/// Queue a frame as N split parts. Every part is its own reliable
	/// message: the capture numbers the two halves of the 1160-byte padded
	/// ping 2 and 3 (not 2 and 2), and the server ACKs them individually.
	fn queue_split(&mut self, frame: Frame, resend: bool, max_payload: usize) {
		let chunks: Vec<Vec<u8>> = frame.payload.chunks(max_payload).map(|c| c.to_vec()).collect();
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
				order_index: frame.order_index,
				order_channel: 0,
				split_id,
				split_count: count,
				split_index: i as u32,
				payload: chunk,
			};
			let mut bytes = Vec::new();
			f.encode(&mut bytes);
			if resend && is_reliable(frame.reliability) {
				self.remember(seq, &bytes);
			}
			self.frame_buf.extend_from_slice(&bytes);
		}
	}

	/// Record a reliable frame for ACK-driven resends.
	fn remember(&mut self, seq: u32, bytes: &[u8]) {
		self.out_queue.insert(
			seq,
			OutFrame {
				reliability_index: seq,
				bytes: bytes.to_vec(),
				sent: Instant::now(),
				tries: 0,
			},
		);
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
				if is_reliable(f.reliability) {
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

	/// Wrap one packed datagram body in the 0x84 header and send it.
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
	pub(super) fn send_sealed(&mut self, pkt: &[u8]) {
		if self.crypto_connected {
			if let Some(s) = self.early.as_mut().map(|ec| ec.seal_counter(pkt)) {
				let _ = self.send_raw(&s);
				return;
			}
		}
		let _ = self.send_raw(pkt);
	}
}
