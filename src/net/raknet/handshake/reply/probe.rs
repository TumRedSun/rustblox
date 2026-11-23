//! Reachability probing and blocking socket reads used by the handshake
//! phases.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::super::hex_head;
use crate::net::raknet::{RakClient, OFFLINE_MAGIC};
use crate::netlog;
use std::time::{Duration, Instant};

impl RakClient {
	/// Reachability probe: RakNet unconnected pings (0x01 / 0x02) with the
	/// standard magic, replayed through EVERY envelope candidate (bare +
	/// token-derived RUPP headers). Returns the envelope that drew a reply.
	/// This separates "the network path is dead" from "the handshake packet
	/// shape is rejected" AND pins down which clear-header shape the proxy
	/// expects.
	pub(in crate::net::raknet::handshake) fn probe_reachability(
		&mut self,
		envelopes: &[crate::net::udmux::Envelope],
		per_env: Duration,
	) -> Option<crate::net::udmux::Envelope> {
		for env in envelopes {
			self.envelope = if env.direction == 0 { None } else { Some(*env) };
			for pid in [0x01u8, 0x02u8] {
				let mut pkt = Vec::with_capacity(1 + 16 + 8);
				pkt.push(pid);
				pkt.extend_from_slice(&OFFLINE_MAGIC);
				pkt.extend_from_slice(&self.guid.to_le_bytes());
				if let Err(e) = self.send_raw(&pkt) {
					netlog!(
						net,
						"envelope probe: ping 0x{:02x} via '{}' send failed: {}",
						pid,
						env.label,
						e
					);
					continue;
				}
				let deadline = Instant::now() + per_env;
				while Instant::now() < deadline {
					match self.socket.recv_from(&mut self.buf) {
						Ok((n, from)) => {
							if from != self.server {
								continue;
							}
							self.last_recv = Instant::now();
							let payload =
								self.strip_incoming(self.buf[..n].to_vec(), "envelope probe");
							netlog!(
								net,
								"envelope probe: reply to 0x{:02x} via '{}' ({} bytes: {})",
								pid,
								env.label,
								payload.len(),
								hex_head(&payload, 24)
							);
							return Some(*env);
						}
						Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
							std::thread::sleep(Duration::from_millis(8));
						}
						Err(_) => break,
					}
				}
			}
		}
		self.envelope = None;
		None
	}

	/// Block until a datagram arrives (nonblocking socket + sleep polling).
	/// Datagrams from other addresses are logged — a proxy answering from a
	/// different source port used to be silently discarded here.
	pub(in crate::net::raknet::handshake) fn wait_readable(
		&mut self,
		deadline: Instant,
	) -> Option<Vec<u8>> {
		while Instant::now() < deadline {
			match self.socket.recv_from(&mut self.buf) {
				Ok((n, from)) if from == self.server => {
					self.last_recv = Instant::now();
					return Some(self.strip_incoming(self.buf[..n].to_vec(), "recv"));
				}
				Ok((n, from)) => {
					netlog!(
						net,
						"datagram from unexpected addr {} while connected to {} ({} bytes, first byte 0x{:02x})",
						from,
						self.server,
						n,
						self.buf.first().copied().unwrap_or(0)
					);
				}
				Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
					std::thread::sleep(Duration::from_millis(8));
				}
				Err(_) => return None,
			}
		}
		None
	}
}
