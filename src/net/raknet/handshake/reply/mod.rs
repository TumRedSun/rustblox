//! The request/reply plumbing `connect` drives: the generic offline
//! round-trip loop (this file), the reachability probe (`probe`) and the
//! REPLY_1 / REPLY_2 / ACCEPTED parsers (`accepted`).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod accepted;
mod probe;

use super::hex_head;
use crate::net::raknet::{RakClient, RakError};
use crate::netlog;
use std::time::{Duration, Instant};

impl RakClient {
	/// Generic offline request/reply round.
	///
	/// Every variant gets an equal slice of `timeout` (with mid-window
	/// retransmits for packet loss) — an earlier build handed the WHOLE
	/// budget to the first variant, so "4 protocol attempts" in the failure
	/// message really sent only the first packet shape. Any datagram that
	/// arrives is logged (first bytes) whether or not it parses, so a wrong
	/// reply shape is visible in the client log alone. `env_of` selects the
	/// datagram envelope for a variant (phase 1 probes several).
	pub(super) fn offline_roundtrip<P, R, F>(
		&mut self,
		variants: &[P],
		timeout: Duration,
		label: &str,
		mut build: impl FnMut(&P, &mut Vec<u8>, u16),
		env_of: impl Fn(&P) -> crate::net::udmux::Envelope,
		mut parse: F,
	) -> Result<(R, usize), RakError>
	where
		F: FnMut(&mut RakClient, &[u8]) -> Option<R>,
	{
		let deadline = Instant::now() + timeout;
		let per_variant = {
			let t = timeout / (variants.len().max(1) as u32);
			t.clamp(Duration::from_millis(600), Duration::from_millis(2500))
		};
		for (i, v) in variants.iter().enumerate() {
			let window_end = (Instant::now() + per_variant).min(deadline);
			let mut next_send = Instant::now();
			let mut sends = 0u32;
			let mut out = Vec::with_capacity(self.mtu as usize);
			while Instant::now() < window_end {
				if Instant::now() >= next_send {
					out.clear();
					build(v, &mut out, self.mtu);
					// send_raw frames the body with this variant's envelope;
					// phases after phase 1 always see their pinned envelope.
					let env = env_of(v);
					self.envelope = if env.direction == 0 { None } else { Some(env) };
					sends += 1;
					if let Err(e) = self.send_raw(&out) {
						netlog!(
							net,
							"handshake {}: attempt {} send #{} failed: {}",
							label,
							i + 1,
							sends,
							e
						);
					} else if sends == 1 {
						netlog!(net, "handshake {}: attempt {} — {} bytes on the wire (retransmitting every 500 ms)", label, i + 1, out.len());
					}
					next_send += Duration::from_millis(500);
				}
				match self.socket.recv_from(&mut self.buf) {
					Ok((n, from)) => {
						let payload = self.buf[..n].to_vec();
						if from != self.server {
							netlog!(
								net,
								"handshake {}: datagram from unexpected addr {} ({} bytes, first byte 0x{:02x})",
								label,
								from,
								n,
								payload.first().copied().unwrap_or(0)
							);
							continue;
						}
						self.last_recv = Instant::now();
						let payload = self.strip_incoming(payload, label);
						if let Some(r) = parse(self, &payload) {
							netlog!(
								net,
								"handshake {}: attempt {} got a valid reply after {} send(s)",
								label,
								i + 1,
								sends
							);
							return Ok((r, i));
						}
						netlog!(
							net,
							"handshake {}: attempt {} — unparsed datagram ({} bytes: {})",
							label,
							i + 1,
							n,
							hex_head(&payload, 24)
						);
					}
					Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
						std::thread::sleep(Duration::from_millis(8));
					}
					Err(e) => {
						netlog!(net, "handshake {}: recv failed: {}", label, e);
						break;
					}
				}
			}
		}
		Err(RakError::TimedOut("offline handshake"))
	}
}
