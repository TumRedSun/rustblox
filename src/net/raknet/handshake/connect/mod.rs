//! `RakClient::connect` — the full offline + connected handshake.
//!
//! One sequential state machine (envelope pinning -> OCR1/REPLY_1 ->
//! OCR2/REPLY_2 -> CONNECTION_REQUEST/ACCEPTED) where every phase's output
//! feeds the next. The phases are split across this folder so no file has to
//! carry the whole flow: [`setup`] resolves the server, binds the socket and
//! pins the datagram envelope, [`offline`] runs the OCR1/OCR2 matrices, and
//! [`session`] runs the connected CONNECTION_REQUEST/ACCEPTED exchange.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod offline;
mod session;
mod setup;

use crate::net::raknet::{RakClient, RakError};
use crate::netlog;
use std::time::Duration;

impl RakClient {
	/// `rcc` is the PRIVATE RCC `ip:port` from the join script's
	/// `ServerConnections` — on a live capture (Nov 2026) the client RUPP
	/// header embeds it verbatim (`02 06 <ip4> <port BE16>`) and the proxy
	/// routes on that. The proxy also gates on the TokenTlv, so when the
	/// token is known the session's static TokenValue rides as the tag
	/// (token type 0x02, per the 34k-datagram capture); the OCR1 matrix
	/// still walks the tag/type variants (token vs random tag, type 02 vs
	/// 01) so a wrong guess costs one 1160-byte probe, not the session.
	///
	/// `udmux_token` carries the join script's per-session routing material
	/// (`(TokenValue, PepperId)`) when the dial goes through a UDMUX proxy.
	pub fn connect(
		host: &str,
		port: u16,
		protocol_versions: &[u8],
		handshake_timeout: Duration,
		udmux_token: Option<(&str, u64)>,
		rcc: Option<(std::net::Ipv4Addr, u16)>,
		early: Option<crate::net::crypto::EarlyCrypto>,
	) -> Result<RakClient, RakError> {
		let mut c = setup::new_client(host, port, early)?;

		// Phase 0 (udmux sessions): pin the datagram envelope.
		let (pinned, probe_pong) = setup::pin_envelope(&mut c, udmux_token, rcc);

		// Phases 1-2: OPEN_CONNECTION_REQUEST/REPLY 1-2.
		let (server_guid, mtu, use_security) = offline::phase1(
			&mut c,
			&pinned,
			probe_pong,
			protocol_versions,
			udmux_token,
			handshake_timeout,
		)?;
		netlog!(net, "REPLY_1: guid={} mtu={} security={}", server_guid, mtu, use_security);
		c.server_guid = server_guid;
		c.mtu = mtu;
		offline::phase2(&mut c, rcc, port, handshake_timeout)?;

		// Phase 3: CONNECTION_REQUEST -> ACCEPTED -> NEW_INCOMING_CONNECTION.
		session::phase3(&mut c, handshake_timeout)?;
		session::finish(&mut c);
		Ok(c)
	}
}
