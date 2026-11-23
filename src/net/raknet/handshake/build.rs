//! The OCR2 packet builder (both offline-id generations), the NetStack
//! side-channel diagnostic probe, and the hex-dump helper shared across the
//! handshake log lines.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::{
	RBX_ID_OPEN_CONNECTION_REQUEST_1, RBX_ID_OPEN_CONNECTION_REQUEST_2, RBX_PROTOCOL_VERSION,
};
use crate::net::raknet::wire::{write_address, write_address_rbx};
use crate::net::raknet::{ID_OPEN_CONNECTION_REQUEST_1, OFFLINE_MAGIC};
use crate::netlog;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

/// Build an OCR2 datagram body for either offline-id generation (the
/// phase-2 matrix step). The fork shape (Sala Packet07Layer):
/// `[0x78][magic][addr: family, ip^0xFF, port BE][mtu BE][guid BE]
/// [supportedVersion u32 BE][capabilities u64 BE]`; vanilla keeps the
/// legacy two-field tail.
pub(super) fn build_ocr2(
	id: u8,
	supported_version: u32,
	caps: u64,
	server: SocketAddr,
	mtu: u16,
	client_guid: u64,
) -> Vec<u8> {
	let mut out = Vec::with_capacity(64);
	out.push(id);
	out.extend_from_slice(&OFFLINE_MAGIC);
	match id {
		RBX_ID_OPEN_CONNECTION_REQUEST_2 => {
			write_address_rbx(&mut out, server);
			out.extend_from_slice(&mtu.to_be_bytes());
			out.extend_from_slice(&client_guid.to_be_bytes());
			out.extend_from_slice(&supported_version.to_be_bytes());
			out.extend_from_slice(&caps.to_be_bytes());
		}
		_ => {
			write_address(&mut out, server);
			out.extend_from_slice(&mtu.to_be_bytes());
			out.extend_from_slice(&client_guid.to_be_bytes());
		}
	}
	out
}

/// Diagnostic probe of the NetStack side-channel (join hypothesis (b)): the
/// join script carries `NetStackPort` + `NetStackTokenValue` — if game
/// traffic moved off the legacy udmux port, an OCR1 there should draw a
/// reply while the main channel stays silent. Sends the fork OCR1 through
/// every RUPP envelope candidate for the netstack token (plus the bare
/// vanilla fallback) and reports which shape answered. Read-only: never
/// touches session state; the verdict goes to the log.
pub fn netstack_probe(host: &str, port: u16, token: &str, per_shape: Duration) -> Option<String> {
	let addr: SocketAddr = match (host, port).to_socket_addrs() {
		Ok(mut it) => match it.next() {
			Some(a) => a,
			None => return None,
		},
		Err(e) => {
			netlog!(net, "netstack probe: resolve {}:{} failed: {}", host, port, e);
			return None;
		}
	};
	let socket = match UdpSocket::bind(if addr.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" }) {
		Ok(s) => s,
		Err(e) => {
			netlog!(net, "netstack probe: bind failed: {}", e);
			return None;
		}
	};
	let _ = socket.set_nonblocking(true);

	let mut shapes: Vec<(String, Vec<u8>)> = Vec::new();
	for env in crate::net::udmux::envelope_candidates(token, 0) {
		let mut body = Vec::with_capacity(1400);
		body.push(RBX_ID_OPEN_CONNECTION_REQUEST_1);
		body.extend_from_slice(&OFFLINE_MAGIC);
		body.push(RBX_PROTOCOL_VERSION);
		body.resize(1400usize.saturating_sub(env.direction as usize), 0x00);
		let wire =
			if env.direction == 0 { body } else { crate::net::udmux::rupp_frame(&env, &body) };
		shapes.push((format!("{}+0x7b", env.label), wire));
	}
	let mut vanilla = Vec::with_capacity(1400);
	vanilla.push(ID_OPEN_CONNECTION_REQUEST_1);
	vanilla.extend_from_slice(&OFFLINE_MAGIC);
	vanilla.push(RBX_PROTOCOL_VERSION);
	vanilla.resize(1400, 0x00);
	shapes.push(("bare+0x05".into(), vanilla));

	for (label, wire) in &shapes {
		if socket.send_to(wire, addr).is_err() {
			continue;
		}
		netlog!(
			net,
			"netstack probe: sent '{}' ({} bytes: {})",
			label,
			wire.len(),
			hex_head(wire, 32)
		);
		let deadline = Instant::now() + per_shape;
		while Instant::now() < deadline {
			let mut buf = vec![0u8; 2048];
			match socket.recv_from(&mut buf) {
				Ok((n, from)) if from == addr => {
					netlog!(
						net,
						"netstack probe: '{}' drew a reply ({} bytes: {})",
						label,
						n,
						hex_head(&buf[..n], 32)
					);
					return Some(label.clone());
				}
				Ok(_) => {}
				Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
					std::thread::sleep(Duration::from_millis(8));
				}
				Err(_) => break,
			}
		}
	}
	netlog!(
        net,
        "netstack probe: total silence across {} shapes — the NetStack channel is not answering for this session",
        shapes.len()
    );
	None
}

/// Compact hex dump for log lines.
pub(super) fn hex_head(buf: &[u8], n: usize) -> String {
	buf.iter().take(n).map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ")
}
