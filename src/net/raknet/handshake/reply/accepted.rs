//! REPLY_1 / REPLY_2 parsing and the connected-phase ACCEPTED handling
//! (NEW_INCOMING_CONNECTION reply).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::super::{hex_head, RBX_ID_OPEN_CONNECTION_REPLY_1, RBX_ID_OPEN_CONNECTION_REPLY_2};
use crate::net::raknet::wire::{
	read_address, read_address_rbx, write_address, write_address_rbx, Reader,
};
use crate::net::raknet::{
	ForkGen, RakClient, RakError, ID_NEW_INCOMING_CONNECTION, ID_OPEN_CONNECTION_REPLY_1,
	ID_OPEN_CONNECTION_REPLY_2,
};
use crate::netlog;
use std::net::SocketAddr;

impl RakClient {
	/// Parse REPLY_1. Both offline-id generations answer with the same field
	/// set; the fork writes it BIG-endian (Sala Packet06: GUID u64BE, bool,
	/// MTU u16BE) — the old parser read LE, so even a correct reply would
	/// have parsed as garbage.
	pub(in crate::net::raknet::handshake) fn parse_reply1(
		&mut self,
		payload: &[u8],
	) -> Option<(u64, u16, bool)> {
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

	/// Parse REPLY_2 in either address layout (fork XOR vs vanilla).
	pub(in crate::net::raknet::handshake) fn parse_reply2(
		&mut self,
		payload: &[u8],
	) -> Option<(u64, u16)> {
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

	/// Handle CONNECTION_REQUEST_ACCEPTED: adopt the server tag, then reply
	/// with the FULL NEW_INCOMING_CONNECTION the fork expects (Sala Packet13)
	/// — our external address + 10 address slots + the two timestamps. With
	/// early crypto this rides a tagged handshake box (dump pkt7).
	pub(in crate::net::raknet::handshake) fn handle_accepted(
		&mut self,
		payload: &[u8],
	) -> Result<(), RakError> {
		let mut r = Reader::new(payload);
		let _id = r.u8();
		// ACCEPTED carries the client's external address (the server's view
		// of us), a system index, 10 address slots and two timestamps. The
		// fork writes addresses XOR-obfuscated (ip^0xFF, port BE).
		let client_addr = read_addr(self.fork, &mut r);
		let _system_index = read_u16(self.fork, &mut r);
		for _ in 0..10 {
			let _ = read_addr(self.fork, &mut r);
		}
		let request_time = read_u64(self.fork, &mut r);
		let _server_time = read_u64(self.fork, &mut r);
		netlog!(
			net,
			"CONNECTION_REQUEST_ACCEPTED received (client external addr {:?})",
			client_addr
		);

		// Header switch (dump pkt7, byte-exact): the datagram right after
		// ACCEPTED ALREADY rides the SERVER's tag and class byte 0x02 —
		// pkt4 (CONNECTION_REQUEST) still had the client tag + 0x01. Adopt
		// both before the reply leaves, or the RCC drops it.
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
		let out = build_incoming(self, client_addr, request_time);
		let boxed = match self.early.as_mut() {
			Some(ec) => ec.seal_counter(&out),
			None => out,
		};
		let _ = self.send_raw(&boxed);
		Ok(())
	}
}

/// Read one address block with the layout of the winning offline generation.
fn read_addr(fork: ForkGen, r: &mut Reader) -> Option<SocketAddr> {
	match fork {
		ForkGen::Roblox => read_address_rbx(r),
		ForkGen::Vanilla => read_address(r),
	}
}

/// Read the ACCEPTED system index (fork BE, vanilla LE).
fn read_u16(fork: ForkGen, r: &mut Reader) -> u16 {
	match fork {
		ForkGen::Roblox => r.u16be().unwrap_or(0),
		ForkGen::Vanilla => r.u16().unwrap_or(0),
	}
}

/// Read one of the ACCEPTED timestamps (fork BE, vanilla LE).
fn read_u64(fork: ForkGen, r: &mut Reader) -> u64 {
	match fork {
		ForkGen::Roblox => r.u64be().unwrap_or(0),
		ForkGen::Vanilla => r.u64().unwrap_or(0),
	}
}

/// Build the NEW_INCOMING_CONNECTION body for the winning generation.
fn build_incoming(c: &RakClient, client_addr: Option<SocketAddr>, request_time: u64) -> Vec<u8> {
	let mut out = Vec::with_capacity(96);
	out.push(ID_NEW_INCOMING_CONNECTION);
	match c.fork {
		ForkGen::Roblox => {
			write_address_rbx(&mut out, client_addr.unwrap_or_else(|| c.local_addr_for()));
			// 10 slots: our local address first, zero placeholders after
			// (vanilla fills them with the host's LAN addresses).
			write_address_rbx(&mut out, c.local_addr_for());
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
			write_address(&mut out, c.local_addr_for());
			write_address(&mut out, c.server);
		}
	}
	out
}
