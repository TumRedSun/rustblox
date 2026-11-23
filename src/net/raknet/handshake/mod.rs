//! The offline + connected handshake: OPEN_CONNECTION_REQUEST/REPLY 1-2,
//! CONNECTION_REQUEST / ACCEPTED / NEW_INCOMING_CONNECTION, and the crypto
//! variant of OCR2/OpenReply2 (`crate::net::crypto`).
//!
//! [`connect`] is the entry point and the whole state machine; [`reply`]
//! holds the request/reply plumbing it calls into (`offline_roundtrip`,
//! `probe_reachability`, the REPLY_1/REPLY_2/ACCEPTED parsers); [`build`]
//! holds the OCR2 packet builder, the NetStack diagnostic probe and the
//! shared hex-dump helper.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod build;
pub mod connect;
pub mod reply;

pub use build::netstack_probe;
use build::{build_ocr2, hex_head};

use crate::net::raknet::RakState;
use crate::netlog;
use std::net::SocketAddr;

// ---------------------------------------------------------------------------
// Roblox fork offline ids — the proxy answers THESE, not vanilla 0x05/0x06.
// ---------------------------------------------------------------------------
//
// The root cause of the total udmux silence: builds ce2ad87..3f5bb64 ran the
// VANILLA offline handshake (OCR1 = 0x05, OCR2 = 0x07), while Roblox's RakNet
// fork remapped the offline ids. Three independent dissectors of live client
// traffic agree on the mapping — Sala's roblox-dissector PacketReader map
// (0x7B -> OCR1, 0x7E -> REPLY_1, 0x78 -> OCR2, 0x7D -> REPLY_2), raknet-dec's
// PREAUTH_HEADERS = {0x78, 0x7B, 0x7D, 0x7E} and the Anaminus dissector's
// id table. The body keeps the vanilla layout (16-byte offline magic right
// after the id byte); servers/proxies silently drop datagrams whose first
// byte is not one of the fork ids — exactly the observed
// "1400/1377/1369 bytes on the wire, zero replies".

pub const RBX_ID_OPEN_CONNECTION_REQUEST_1: u8 = 0x7B;
pub const RBX_ID_OPEN_CONNECTION_REPLY_1: u8 = 0x7E;
pub const RBX_ID_OPEN_CONNECTION_REQUEST_2: u8 = 0x78;
pub const RBX_ID_OPEN_CONNECTION_REPLY_2: u8 = 0x7D;

/// The fork keeps RakNet protocol version 5 in OCR1 (Sala: "always 5").
pub const RBX_PROTOCOL_VERSION: u8 = 5;

/// The byte the live capture shows immediately after the protocol byte in
/// the fork OCR1: `7b | magic | 05 | 01 | zero pad`. Meaning unknown (an
/// auth/encryption epoch flag is the best guess), but the wire does not
/// lie: the Nov-12 run sent `05 00` and the RCC never answered, while the
/// capture client's `05 01` drew REPLY_1 in 65 ms.
pub const RBX_OCR1_EPOCH: u8 = 0x01;

/// Exact C->S datagram size (envelope header included) of OCR1/OCR2 in the
/// live capture: MTU 1200 minus a 40-byte probe allowance. The old probe
/// sized OCR1 to 1400 bytes, above the proxy's 1200 path MTU, and the
/// datagram died in transit before the RCC ever saw it.
pub const RBX_OFFLINE_PROBE: usize = 1160;

/// Connection password in CONNECTION_REQUEST (0x09) — Sala's
/// `DefaultPasswordBytes`. (Studio sends `{0x5E, 0x11}`; our old packets had
/// no password at all.)
pub const RBX_PASSWORD: [u8; 6] = [0x37, 0x4F, 0x5E, 0x11, 0x6C, 0x45];

/// OCR2 Capabilities (Sala JoinPacketLayers.go): the base Roblox client set
/// (Basic | ServerCopiesPlayerGui3 | IHasMinDistToUnstreamed |
/// ReplicateLuau | VersionedIDSync) and the modern superset that adds
/// PositionBasedStreaming | SystemAddressIsPeerId | PubKeyExchange |
/// StreamingPrefetch. The exact value the server requires is not published —
/// the phase-2 matrix tries both.
pub const RBX_CAPABILITY_ROBLOX: u64 = 0x2CFE;
pub const RBX_CAPABILITY_MODERN: u64 = 0x01A0_3CFE;

impl crate::net::raknet::RakClient {
	/// Send one datagram to the server, applying the confirmed RUPP
	/// envelope. ALL session traffic must ride the exact clear-header shape
	/// the proxy accepted during the handshake — bare packets would no
	/// longer map to our client->RCC binding (if an envelope was what got
	/// us in). During the handshake every outgoing datagram is logged as
	/// hex (first 32 bytes, exactly as framed for the wire) — one look at
	/// the log shows which shape left the machine.
	///
	/// Tag mirroring (sober_ro.pcap, 33k-datagram session): the RUPP tag is
	/// a TIME-ROTATING epoch — it changes every wall-clock second. The
	/// proxy switches to the new tag first and the client mirrors it within
	/// ~10-70 ms (165 tags / 169 s, client and server ALWAYS carried the
	/// same tag within the same second). A tag pinned once at connect time
	/// goes stale after the first second, so every outgoing datagram in the
	/// connected phase carries the newest server tag we have seen.
	pub(crate) fn send_raw(&mut self, buf: &[u8]) -> std::io::Result<()> {
		if self.state == RakState::Connected {
			if let Some(env) = self.envelope.as_mut() {
				// Connected phase (pcap: cls byte 0x02 on 100% of the client
				// datagrams) + the newest server tag (rotates every second —
				// the proxy leads, the client mirrors).
				env.cls = 0x02;
				if let Some(st) = self.server_tag {
					if env.epoch != st {
						netlog!(
							net,
							"envelope: mirroring the server's rotated tag {}",
							hex_head(&st, 16)
						);
						env.epoch = st;
					}
				}
			}
		}
		let owned;
		let out: &[u8] = match &self.envelope {
			None => buf,
			Some(env) if env.direction == 0 => buf,
			Some(env) => {
				owned = crate::net::udmux::rupp_frame(env, buf);
				&owned
			}
		};
		let res = self.socket.send_to(out, self.server).map(|_| ());
		if self.state == RakState::Connecting {
			netlog!(net, "out[{}] {}", out.len(), hex_head(out, 32));
		}
		res
	}

	/// Open a connect-phase AEAD box (early crypto). Live capture: pkt4 is
	/// envelope(31) + box(52) = ct(34) + hint + tag(16).
	pub fn open_handshake_box(&self, body: &[u8]) -> Option<Vec<u8>> {
		self.early.as_ref()?.open_counter(body).map(|(pt, _)| pt)
	}

	/// Parse the crypto OpenReply2 (`7d | magic | 01 2c 00 3a 03 |
	/// server_pub(32) | box | "UniqueNumber" | unique(16)`), adopt the
	/// session keys, and report the wire facts in the log. Returns the
	/// current (guid, mtu) so the crypto path plugs into the legacy
	/// phase-2 result tuple.
	fn parse_reply2_crypto(&mut self, payload: &[u8]) -> Option<(u64, u16)> {
		let fields = crate::net::crypto::EarlyCrypto::parse_reply2(payload)?;
		let (server_pub, box_wire, unique) = (fields.server_pub, fields.box_wire, fields.unique);
		let ec = self.early.as_mut()?;
		ec.adopt_reply2_key(server_pub);
		netlog!(
			net,
			"crypto OpenReply2: server_pub {} box {} bytes unique {} (hdr 01 2c 00 3a)",
			hex_head(&server_pub, 32),
			box_wire.len(),
			hex_head(&unique, 16)
		);
		// Diagnostic: the reply2 box is nonce-on-the-wire (the dump's first
		// 12 bytes look random, unlike the sequential counter hints after it).
		// Try the nonce-on-wire open with an empty AAD and the reply's own
		// RakNet prefix as the AAD.
		let mut aads: Vec<Vec<u8>> = vec![Vec::new()];
		if payload.len() > 54 {
			aads.push(payload[..54].to_vec());
		}
		let mut opened = false;
		if let Some(sk) = ec.session_rx {
			if crate::net::crypto::open(&sk, box_wire, &[&aads[0], &aads[1]]).is_some() {
				opened = true;
			}
		}
		if !opened && crate::net::crypto::open(&ec.rx, box_wire, &[&aads[0], &aads[1]]).is_some() {
			opened = true;
		}
		netlog!(
            net,
            "crypto OpenReply2 box {} (no key matched — content stays opaque until a live session validates the schedule)",
            if opened { "OPENED" } else { "not opened" }
        );
		Some((self.server_guid, self.mtu))
	}

	/// Detect a RUPP-framed incoming datagram, log its clear header and
	/// return the inner bytes. Raw datagrams pass through unchanged — the
	/// server answers inside the envelope of its direction (0x1f), so this
	/// runs on every recv path.
	pub(crate) fn strip_incoming(&mut self, payload: Vec<u8>, ctx: &str) -> Vec<u8> {
		if let Some((dir, epoch, mux, hlen)) = crate::net::udmux::rupp_parse(&payload) {
			netlog!(
				net,
				"{}: RUPP reply dir 0x{:02x} epoch {} mux {} body {} bytes",
				ctx,
				dir,
				hex_head(&epoch, 16),
				hex_head(&mux, 8),
				payload.len() - hlen
			);
			// Live capture: the server announces its 16-byte session tag in
			// every 0x17/0x1f-framed reply; once connected both sides send
			// that SAME tag (bare 0x04 frames carry none). The tag ROTATES
			// every wall-clock second (165 distinct tags over 169 s in the
			// capture) and the proxy leads the rotation — tracking the
			// newest one here is what keeps the mirrored outgoing tag fresh
			// (send_raw re-frames with it on every datagram).
			if dir == 0x17 || dir == 0x1f {
				self.server_tag = Some(epoch);
			}
			payload[hlen..].to_vec()
		} else {
			payload
		}
	}

	fn local_addr_for(&self) -> SocketAddr {
		self.socket.local_addr().unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)))
	}
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
