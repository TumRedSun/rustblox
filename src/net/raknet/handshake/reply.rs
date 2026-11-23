//! The request/reply plumbing `connect` drives: the generic offline
//! round-trip loop, the reachability probe, and the REPLY_1 / REPLY_2 /
//! ACCEPTED parsers.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::build::hex_head;
use super::{RBX_ID_OPEN_CONNECTION_REPLY_1, RBX_ID_OPEN_CONNECTION_REPLY_2};
use crate::net::raknet::wire::{
	read_address, read_address_rbx, write_address, write_address_rbx, Reader,
};
use crate::net::raknet::{
	ForkGen, RakClient, RakError, ID_NEW_INCOMING_CONNECTION, ID_OPEN_CONNECTION_REPLY_1,
	ID_OPEN_CONNECTION_REPLY_2, OFFLINE_MAGIC,
};
use crate::netlog;
use std::net::SocketAddr;
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

	/// Reachability probe: RakNet unconnected pings (0x01 / 0x02) with the
	/// standard magic, replayed through EVERY envelope candidate (bare +
	/// token-derived RUPP headers). Returns the envelope that drew a reply.
	/// This separates "the network path is dead" from "the handshake packet
	/// shape is rejected" AND pins down which clear-header shape the proxy
	/// expects.
	pub(super) fn probe_reachability(
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
	pub(super) fn wait_readable(&mut self, deadline: Instant) -> Option<Vec<u8>> {
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

	pub(super) fn parse_reply1(&mut self, payload: &[u8]) -> Option<(u64, u16, bool)> {
		// Both offline-id generations answer with the same field set; the
		// fork writes it BIG-endian (Sala Packet06: GUID u64BE, bool, MTU
		// u16BE) — the old parser read LE, so even a correct reply would
		// have parsed as garbage.
		match payload.first() {
			Some(&RBX_ID_OPEN_CONNECTION_REPLY_1) | Some(&ID_OPEN_CONNECTION_REPLY_1) => {}
			_ => return None,
		}
		let mut r = Reader::new(payload);
		let _id = r.u8()?;
		let _magic = r.bytes(16)?;
		let server_guid = r.u64be()?;
		let use_security = r.u8()? != 0;
		if use_security && payload[0] == ID_OPEN_CONNECTION_REPLY_1 {
			// Vanilla-only security cookie; the fork's REPLY_1 has none.
			let _cookie = r.u32be()?;
		}
		let mtu = r.u16be()?;
		Some((server_guid, mtu.max(400), use_security))
	}

	pub(super) fn parse_reply2(&mut self, payload: &[u8]) -> Option<(u64, u16)> {
		let mut r = Reader::new(payload);
		match r.u8()? {
			RBX_ID_OPEN_CONNECTION_REPLY_2 => {
				let _magic = r.bytes(16)?;
				let server_guid = r.u64be()?;
				let _addr = read_address_rbx(&mut r)?;
				let mtu = r.u16be()?;
				let _security = r.u8()?;
				if r.rest().len() >= 12 {
					let sv = r.u32be()?;
					let caps = r.u64be()?;
					netlog!(net, "REPLY_2 tail: supportedVersion={} capabilities=0x{:x}", sv, caps);
				}
				Some((server_guid, mtu.max(400)))
			}
			ID_OPEN_CONNECTION_REPLY_2 => {
				let _magic = r.bytes(16)?;
				let server_guid = r.u64be()?;
				let _addr = read_address(&mut r)?;
				let mtu = r.u16be()?;
				Some((server_guid, mtu.max(400)))
			}
			_ => None,
		}
	}

	pub(super) fn handle_accepted(&mut self, payload: &[u8]) -> Result<(), RakError> {
		let mut r = Reader::new(payload);
		let _id = r.u8();
		// ACCEPTED carries the client's external address (the server's view
		// of us), a system index, 10 address slots and two timestamps. The
		// fork writes addresses XOR-obfuscated (ip^0xFF, port BE) and all
		// multi-byte fields big-endian.
		let (client_addr, read_addr, sys_idx, rd_u64) = match self.fork {
			ForkGen::Roblox => (
				read_address_rbx(&mut r),
				read_address_rbx as fn(&mut Reader) -> Option<SocketAddr>,
				r.u16be().unwrap_or(0),
				(|r: &mut Reader| r.u64be().unwrap_or(0)) as fn(&mut Reader) -> u64,
			),
			ForkGen::Vanilla => (
				read_address(&mut r),
				read_address as fn(&mut Reader) -> Option<SocketAddr>,
				r.u16().unwrap_or(0),
				(|r: &mut Reader| r.u64().unwrap_or(0)) as fn(&mut Reader) -> u64,
			),
		};
		let _system_index = sys_idx;
		for _ in 0..10 {
			let _ = read_addr(&mut r);
		}
		let request_time = rd_u64(&mut r);
		let _server_time = rd_u64(&mut r);
		netlog!(
			net,
			"CONNECTION_REQUEST_ACCEPTED received (client external addr {:?})",
			client_addr
		);

		// Reply with the FULL NEW_INCOMING_CONNECTION the fork expects
		// (Sala Packet13): our external address + 10 address slots + the two
		// timestamps — the old two-address body was not parseable by the
		// RCC and left the connection dead right after the handshake.
		// With early crypto this rides a tagged handshake box (the dump's
		// pkt7: 31 + 12 + 16 + plaintext).
		//
		// Header switch (dump pkt7, byte-exact): the datagram right after
		// ACCEPTED ALREADY rides the SERVER's tag and class byte 0x02 —
		// pkt4 (CONNECTION_REQUEST) still had the client tag + 0x01, pkt7
		// (`01 00 00 1f 01 11 02 26 0e 2c 77 …`) does not. Adopt both here,
		// before the reply leaves, or the RCC drops it and the session
		// dies right after the handshake.
		if let Some(env) = self.envelope.as_mut() {
			if env.direction == 0x1f {
				env.cls = 0x02;
				if let Some(st) = self.server_tag {
					if env.epoch != st {
						netlog!(
							net,
							"envelope: adopting the server tag {} ahead of NEW_INCOMING_CONNECTION",
							hex_head(&st, 16)
						);
					}
					env.epoch = st;
				}
			}
		}
		let mut out = Vec::with_capacity(96);
		out.push(ID_NEW_INCOMING_CONNECTION);
		match self.fork {
			ForkGen::Roblox => {
				write_address_rbx(&mut out, client_addr.unwrap_or_else(|| self.local_addr_for()));
				// 10 slots: our local address first, zero placeholders after
				// (vanilla fills them with the host's LAN addresses).
				write_address_rbx(&mut out, self.local_addr_for());
				for _ in 0..9 {
					out.push(4);
					out.extend_from_slice(&[0xFF; 4]); // 0.0.0.0 XORed
					out.extend_from_slice(&0u16.to_be_bytes());
				}
				out.extend_from_slice(&request_time.to_be_bytes());
				let now = std::time::SystemTime::now()
					.duration_since(std::time::UNIX_EPOCH)
					.unwrap_or_default()
					.as_millis() as u64;
				out.extend_from_slice(&now.to_be_bytes());
			}
			ForkGen::Vanilla => {
				write_address(&mut out, self.local_addr_for());
				write_address(&mut out, self.server);
			}
		}
		let boxed = match self.early.as_mut() {
			Some(ec) => ec.seal_counter(&out),
			None => out,
		};
		let _ = self.send_raw(&boxed);
		Ok(())
	}
}
