//! Connected-phase reliability: reliable/ordered frame queueing, resend
//! bookkeeping, ACK/NAK handling, split-frame reassembly and ordered
//! delivery. Everything here operates on an already-connected
//! [`RakClient`]; the offline + connected handshake lives in
//! [`super::super::handshake`].
//!
//! Split by responsibility: `send` owns the outgoing frame/datagram path,
//! `recv` the receive path (ACK parsing, frame dispatch, ordered delivery)
//! and `ack` the timers, control messages, ACK emission and split
//! reassembly.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod ack;
mod recv;
mod send;

use super::Frame;
use crate::net::raknet::RakClient;

const DATAGRAM_VALID: u8 = 0x80;
const DATAGRAM_IS_ACK: u8 = 0x40;
const DATAGRAM_IS_NAK: u8 = 0x20;

const RESEND_MS: u64 = 350;
const MAX_RESENDS: u32 = 12;

/// `true` for the reliability kinds that carry a message number and need
/// ACK-driven resends.
fn is_reliable(reliability: u8) -> bool {
	matches!(reliability, 2 | 3 | 4 | 6 | 7)
}

/// `true` for the reliability kinds that carry an ordering index.
fn is_ordered(reliability: u8) -> bool {
	matches!(reliability, 1 | 3 | 4 | 7)
}

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

	/// Number of payloads buffered behind the ordered delivery head.
	pub fn pending_ordered(&self) -> usize {
		self.ordered_hold.len()
	}
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
