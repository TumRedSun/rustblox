//! Phase 3 of the handshake: the CONNECTION_REQUEST / ACCEPTED /
//! NEW_INCOMING_CONNECTION exchange and the connected-phase hand-off.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::super::{hex_head, RBX_PASSWORD};
use crate::net::raknet::{
	RakClient, RakError, RakState, ID_ALREADY_CONNECTED, ID_CONNECTION_ATTEMPT_FAILED,
	ID_CONNECTION_BANNED, ID_CONNECTION_REQUEST, ID_CONNECTION_REQUEST_ACCEPTED,
	ID_NO_FREE_INCOMING_CONNECTIONS,
};
use crate::netlog;
use std::time::{Duration, Instant};

/// Phase 3: send CONNECTION_REQUEST until the session is accepted.
///
/// With early crypto the request rides inside an AEAD box
/// (`nonce12 + ct + tag16`) — the dump's pkt4 is exactly envelope(31) +
/// box(52-12-16 = 24-byte plaintext CONNECTION_REQUEST), and the libroblox
/// preauth-window code validates the same `payload + 0x1c` overhead rule.
pub(super) fn phase3(c: &mut RakClient, handshake_timeout: Duration) -> Result<(), RakError> {
	let deadline = Instant::now() + handshake_timeout;
	loop {
		if Instant::now() >= deadline {
			return Err(RakError::TimedOut("connection request"));
		}
		let wire = build_request(c);
		if let Err(e) = c.send_raw(&wire) {
			netlog!(net, "CONNECTION_REQUEST send failed: {}", e);
		} else {
			netlog!(net, "CONNECTION_REQUEST sent");
			mark_request_sent(c);
		}
		if let Some(payload) = c.wait_readable(deadline) {
			let payload = open_connect_box(c, payload);
			if classify(c, &payload)? {
				break;
			}
		}
		std::thread::sleep(Duration::from_millis(120));
	}
	Ok(())
}

/// Build the CONNECTION_REQUEST datagram (boxed when the session is
/// encrypted, plaintext otherwise).
fn build_request(c: &mut RakClient) -> Vec<u8> {
	match c.early.as_mut() {
		Some(ec) => {
			// The fork's first steady frame is the 34-byte CONNECTION_REQUEST
			// the capture carries (raknet27 rec 257, decrypted and confirmed
			// live):
			//   81 00 00 00 40 00 c0 00 00 00 09
			//   | session id(8) | 00*6 | fd fc | 00 | password(6)
			// The session id is the one announced in pt93[8:16].
			let mut out = Vec::with_capacity(34);
			out.extend_from_slice(&[
				0x81, 0x00, 0x00, 0x00, 0x40, 0x00, 0xc0, 0x00, 0x00, 0x00, 0x09,
			]);
			out.extend_from_slice(&ec.session_id);
			out.extend_from_slice(&[0u8; 6]);
			out.extend_from_slice(&[0xfd, 0xfc]);
			out.push(0);
			out.extend_from_slice(&RBX_PASSWORD);
			let boxed = ec.seal_counter(&out);
			netlog!(
				net,
				"CONNECTION_REQUEST (steady) boxed: pt {} + 18 = {} bytes, hint {}",
				out.len(),
				boxed.len(),
				hex_head(&boxed[boxed.len() - 18..boxed.len() - 16], 2)
			);
			boxed
		}
		None => {
			let mut out = Vec::with_capacity(64);
			out.push(ID_CONNECTION_REQUEST);
			out.extend_from_slice(&c.guid.to_be_bytes());
			let t = std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_millis() as u64;
			out.extend_from_slice(&t.to_be_bytes());
			out.push(0); // useSecurity = false
						 // The fork checks this 6-byte password (Sala Packet09:
						 // DefaultPasswordBytes) — without it the RCC refuses silently.
			out.extend_from_slice(&RBX_PASSWORD);
			out
		}
	}
}

/// The steady CONNECTION_REQUEST is RakNet datagram 0 / reliable message 0;
/// the next frame must continue from 1 or the server treats it as duplicate.
fn mark_request_sent(c: &mut RakClient) {
	if c.early.is_some() {
		c.datagram_seq = 1;
		c.reliability_index = 1;
		c.request_ms = u64::from(u16::from_be_bytes([0xfd, 0xfc]));
	}
}

/// Open the connect-phase counter box when the session is encrypted;
/// plaintext datagrams pass through unchanged.
fn open_connect_box(c: &mut RakClient, payload: Vec<u8>) -> Vec<u8> {
	// Early crypto: post-OpenReply2 datagrams ride the counter
	// framing (`ct || hint u16 LE || tag 16`, nonce = counter ||
	// "mbeR", AAD empty — dump pkt4..7). Plaintext sessions pass
	// through unchanged.
	let opened = match c.early.as_ref() {
		Some(ec) => ec.open_counter(&payload),
		None => return payload,
	};
	match opened {
		Some((pt, counter)) => {
			netlog!(
				net,
				"connect-phase box opened (counter {:#x}, pt {} bytes)",
				counter,
				pt.len()
			);
			pt
		}
		None => {
			netlog!(
				net,
				"connect-phase datagram did not open as a counter box ({} bytes) — \
                 treating as plaintext",
				payload.len()
			);
			payload
		}
	}
}

/// Classify one datagram received while connecting. `Ok(true)` means the
/// server accepted the session and the loop may stop.
fn classify(c: &mut RakClient, payload: &[u8]) -> Result<bool, RakError> {
	// A 0-byte UDP datagram is legal — split_first keeps the match panic-free.
	match payload.split_first() {
		Some((&ID_CONNECTION_REQUEST_ACCEPTED, _)) => {
			c.handle_accepted(payload)?;
			Ok(true)
		}
		Some((&ID_CONNECTION_ATTEMPT_FAILED, _)) => Err(RakError::Rejected("attempt failed")),
		Some((&ID_NO_FREE_INCOMING_CONNECTIONS, _)) => Err(RakError::Rejected("server full")),
		Some((&ID_CONNECTION_BANNED, _)) => Err(RakError::Banned),
		Some((&ID_ALREADY_CONNECTED, _)) => Err(RakError::AlreadyConnected),
		// The fork's steady plaintext is a RakNet *datagram* (`0x80 | count`,
		// then frames) — the ACCEPTED message rides inside one of them, not at
		// byte 0. Once the server sends any steady datagram beyond bare ACKs
		// (0xC0/0xA0/0xD0), it has accepted the boxed request; this is the
		// same evidence the standalone reference client uses (live-verified
		// 24 Nov).
		Some((&first, _)) if first & 0x80 != 0 && !matches!(first, 0xc0 | 0xa0 | 0xd0) => {
			netlog!(
				net,
				"steady datagram 0x{:02x} ({} bytes) received — session accepted",
				first,
				payload.len()
			);
			if c.early.is_some() {
				// The first steady datagram carries the framed ACCEPTED, so
				// switch the crypto framing on and let the system layer
				// answer it (NEW_INCOMING + the clock-sync burst) right now —
				// the server starts replicating only after that.
				c.crypto_connected = true;
				c.state = RakState::Connected;
				c.ingest_plain(payload);
				if !c.announced {
					netlog!(
						net,
						"steady datagram did not carry a framed ACCEPTED — \
                         the connected handshake was not answered"
					);
				}
			}
			Ok(true)
		}
		Some((&other, _)) => {
			netlog!(net, "got id 0x{:02x} while connecting", other);
			Ok(false)
		}
		None => {
			netlog!(net, "ignoring empty datagram during handshake");
			Ok(false)
		}
	}
}

/// Final hand-off to the connected phase (live capture): class byte 0x02,
/// the server's 16-byte session tag, and the untagged crypto stream framing.
pub(super) fn finish(c: &mut RakClient) {
	c.state = RakState::Connected;
	c.last_recv = Instant::now();
	if let Some(env) = c.envelope {
		if env.direction == 0x1f {
			let mut e2 = env;
			e2.cls = 0x02;
			if let Some(st) = c.server_tag {
				e2.epoch = st;
				netlog!(net, "envelope: adopting the server session tag {}", hex_head(&st, 16));
			}
			c.envelope = Some(e2);
		}
	}
	if c.early.is_some() {
		c.crypto_connected = true;
		netlog!(
			net,
			"crypto: switching to the connected-phase stream framing (nonce12+ct, no tag)"
		);
	}
}
