//! `RakClient::connect` — the full offline + connected handshake.
//!
//! This single function is intentionally NOT split further: it is one
//! sequential state machine (envelope pinning -> OCR1/REPLY_1 ->
//! OCR2/REPLY_2 -> CONNECTION_REQUEST/ACCEPTED) where every phase's output
//! feeds the next, and splitting it into helper methods would scatter the
//! handshake's actual control flow across files without reducing real
//! complexity. It is a documented exception to the 350-line file /
//! 60-line function budget in `docs/ARCHITECTURE.md` — breaking it into
//! smaller pieces is future work, not part of this structural move.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::too_many_lines)]

use super::hex_head;
use super::{
	build_ocr2, RBX_CAPABILITY_MODERN, RBX_CAPABILITY_ROBLOX, RBX_ID_OPEN_CONNECTION_REQUEST_1,
	RBX_ID_OPEN_CONNECTION_REQUEST_2, RBX_OCR1_EPOCH, RBX_OFFLINE_PROBE, RBX_PASSWORD,
	RBX_PROTOCOL_VERSION,
};
use crate::net::raknet::{
	ForkGen, RakClient, RakError, RakState, ID_ALREADY_CONNECTED, ID_CONNECTION_ATTEMPT_FAILED,
	ID_CONNECTION_BANNED, ID_CONNECTION_REQUEST, ID_CONNECTION_REQUEST_ACCEPTED,
	ID_NO_FREE_INCOMING_CONNECTIONS, ID_OPEN_CONNECTION_REQUEST_1, ID_OPEN_CONNECTION_REQUEST_2,
	OFFLINE_MAGIC,
};
use crate::netlog;
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

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
		let addr: SocketAddr = (host, port)
			.to_socket_addrs()
			.map_err(|e| RakError::Network(e.to_string()))?
			.next()
			.ok_or(RakError::Network("host did not resolve".into()))?;
		let socket = UdpSocket::bind(if addr.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" })
			.map_err(|e| RakError::Network(e.to_string()))?;
		socket.set_nonblocking(true).map_err(|e| RakError::Network(e.to_string()))?;

		let guid: u64 = {
			let t = std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default();
			(t.as_nanos() as u64) ^ 0x52555354424c4f58 // "RUSTBLOX"
		};

		let mut c = RakClient {
			socket,
			server: addr,
			guid,
			server_guid: 0,
			mtu: RBX_OFFLINE_PROBE as u16,
			state: RakState::Connecting,
			ping_ms: None,
			envelope: None,
			client_tag: [0u8; 16],
			// The server's 16-byte tag, learned from any incoming RUPP header.
			server_tag: None,
			early,
			crypto_connected: false,
			// Offline-id generation that answered phase 1.
			fork: ForkGen::Roblox,
			reliability_index: 0,
			order_index: 0,
			datagram_seq: 0,
			split_id: 0,
			out_queue: HashMap::new(),
			frame_buf: Vec::new(),
			received_datagrams: HashSet::new(),
			ack_buffer: Vec::new(),
			delivered_ordered: 0,
			ordered_hold: HashMap::new(),
			splits: HashMap::new(),
			pending: VecDeque::new(),
			last_ping: Instant::now(),
			ping_time: None,
			last_recv: Instant::now(),
			buf: vec![0u8; 65536],
		};

		// Phase 0 (udmux sessions): pin the datagram ENVELOPE.
		//
		// The 34k-datagram live capture (`sober_ro.pcap`, Nov 2026) shows the
		// client header `01 00 00 1f 01 11 02 <tag:16> 02 06 <rcc ip4> <port
		// BE16>` on 100% of the datagrams: a TokenTlv with token TYPE 0x02
		// (Rupp.h) and the private RCC address in the mux field. The proxy
		// routes on the mux AND gates on the token — the type-0x01 +
		// random-tag shape the older builds pinned is exactly what produced
		// total OCR1 silence (no reply of any kind, while the leaked server
		// handler proves it would have answered a datagram that reached it).
		//
		// Pre-connect the tag rides the session's static TokenValue
		// (b64-decoded — the same material the known UDMUX-bypass abuse is
		// built on); the connected phase then mirrors the server's rotating
		// epoch (send_raw). The random client tag below stays as a matrix
		// fallback, and the OCR1 matrix walks {token,random} x {02,01} so a
		// wrong guess costs one 1160-byte probe, not the session.
		{
			let mut t = (guid
				^ std::time::SystemTime::now()
					.duration_since(std::time::UNIX_EPOCH)
					.unwrap_or_default()
					.as_nanos() as u64)
				| 1;
			for chunk in c.client_tag.chunks_mut(8) {
				t = t.wrapping_add(0x9e3779b97f4a7c15);
				let mut z = t;
				z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
				z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
				z ^= z >> 31;
				let n = chunk.len();
				chunk.copy_from_slice(&z.to_le_bytes()[..n]);
			}
		}
		let token_tag: Option<[u8; 16]> =
			udmux_token.and_then(|(tok, _)| crate::net::udmux::decode_token16(tok));
		let known_shape = match (rcc, token_tag) {
			(Some((ip, port)), Some(tag)) => {
				let env = crate::net::udmux::client_envelope(&tag, (ip, port));
				netlog!(
					net,
					"envelope: pinning the capture shape with the session token as the tag \
                     (rcc {}:{}, tag {}) — the OCR1 matrix still walks the tag/type variants",
					ip,
					port,
					hex_head(&env.epoch, 16)
				);
				Some(env)
			}
			(Some((ip, port)), None) => {
				let env = crate::net::udmux::client_envelope(&c.client_tag, (ip, port));
				netlog!(
					net,
					"envelope: no usable TokenValue — pinning the capture shape with a random \
                     tag (rcc {}:{}, tag {})",
					ip,
					port,
					hex_head(&env.epoch, 16)
				);
				Some(env)
			}
			_ => None,
		};
		let envelopes: Vec<crate::net::udmux::Envelope> = match udmux_token {
			None => vec![crate::net::udmux::BARE],
			Some((tok, pepper)) => crate::net::udmux::envelope_candidates(tok, pepper),
		};
		let (pinned, probe_pong) = if known_shape.is_some() {
			(known_shape, false)
		} else if envelopes.len() > 1 {
			let p = c.probe_reachability(&envelopes, Duration::from_millis(700));
			match &p {
				Some(env) => netlog!(
					net,
					"envelope probe: '{}' drew a pong — pinning it for the session",
					env.label
				),
				None => netlog!(
					net,
					"envelope probe: no pong in any RUPP shape — trying the OCR1 matrix anyway"
				),
			}
			(p, p.is_some())
		} else {
			(None, false)
		};

		// Phase 1: OPEN_CONNECTION_REQUEST_1 -> REPLY_1 (MTU probing).
		//
		// Matrix = envelope × offline id × protocol byte, FORK SHAPES FIRST
		// (0x7B + proto 5 — what the modern proxy actually answers), vanilla
		// ids as fallback for direct dials. With a pinned envelope all of
		// its shapes go first; without one bare shapes lead (a direct dial
		// must not inherit a token header), then each envelope candidate ×
		// the fork shape — enough to notice a shape that answers OCR1 but
		// not pings.
		let mut p1_variants: Vec<(crate::net::udmux::Envelope, u8, u8)> = Vec::new();
		match &pinned {
			Some(env) => {
				// Token type / tag-value variants FIRST — the proxy gates on
				// the TokenTlv and the winning shape is pinned for the whole
				// session. Order: the pinned tag (token when available) with
				// type 02, then type 01, then the random client tag (02/01)
				// when it differs.
				let with_cls = |cls: u8| {
					let mut e = *env;
					e.cls = cls;
					e
				};
				let lead = with_cls(0x02);
				p1_variants.push((lead, RBX_ID_OPEN_CONNECTION_REQUEST_1, RBX_PROTOCOL_VERSION));
				p1_variants.push((
					with_cls(0x01),
					RBX_ID_OPEN_CONNECTION_REQUEST_1,
					RBX_PROTOCOL_VERSION,
				));
				if env.epoch != c.client_tag {
					let mut rnd02 = *env;
					rnd02.cls = 0x02;
					rnd02.epoch = c.client_tag;
					p1_variants.push((
						rnd02,
						RBX_ID_OPEN_CONNECTION_REQUEST_1,
						RBX_PROTOCOL_VERSION,
					));
					let mut rnd01 = rnd02;
					rnd01.cls = 0x01;
					p1_variants.push((
						rnd01,
						RBX_ID_OPEN_CONNECTION_REQUEST_1,
						RBX_PROTOCOL_VERSION,
					));
				}
				// Protocol variants on the leading shape, then the vanilla
				// id fallback (direct dials without a proxy).
				for &pr in protocol_versions {
					if pr != RBX_PROTOCOL_VERSION {
						p1_variants.push((lead, RBX_ID_OPEN_CONNECTION_REQUEST_1, pr));
					}
				}
				p1_variants.push((lead, ID_OPEN_CONNECTION_REQUEST_1, RBX_PROTOCOL_VERSION));
			}
			None => {
				p1_variants.push((
					crate::net::udmux::BARE,
					RBX_ID_OPEN_CONNECTION_REQUEST_1,
					RBX_PROTOCOL_VERSION,
				));
				p1_variants.push((
					crate::net::udmux::BARE,
					ID_OPEN_CONNECTION_REQUEST_1,
					RBX_PROTOCOL_VERSION,
				));
				for &pr in protocol_versions {
					if pr != RBX_PROTOCOL_VERSION {
						p1_variants.push((
							crate::net::udmux::BARE,
							RBX_ID_OPEN_CONNECTION_REQUEST_1,
							pr,
						));
					}
				}
				if envelopes.len() > 1 {
					for env in envelopes.iter().skip(1) {
						p1_variants.push((
							*env,
							RBX_ID_OPEN_CONNECTION_REQUEST_1,
							RBX_PROTOCOL_VERSION,
						));
					}
				}
			}
		}
		let reply1 = match c.offline_roundtrip(
			&p1_variants,
			handshake_timeout,
			"request-1",
			|(env, id, proto), out, mtu| {
				out.push(*id);
				out.extend_from_slice(&OFFLINE_MAGIC);
				out.push(*proto);
				// Live capture, byte-exact: the fork OCR1 carries one more
				// byte after the protocol byte (`05 01` on the wire, then
				// the zero pad). Without it the RCC ignores the request —
				// the `05 00` shape never drew a REPLY_1.
				if *id == RBX_ID_OPEN_CONNECTION_REQUEST_1 {
					out.push(RBX_OCR1_EPOCH);
				}
				// MTU probe: pad to the datagram size (envelope header
				// included — the proxy judges the path MTU by the whole
				// datagram, and send_raw adds the header on top).
				out.resize((mtu as usize).saturating_sub(env.direction as usize), 0x00);
			},
			|(env, _, _)| *env,
			|c, payload| c.parse_reply1(payload),
		) {
			Ok((r, winner)) => {
				let (env, id, proto) = p1_variants[winner];
				c.envelope = if env.direction == 0 { None } else { Some(env) };
				c.fork = if id == RBX_ID_OPEN_CONNECTION_REQUEST_1 {
					ForkGen::Roblox
				} else {
					ForkGen::Vanilla
				};
				netlog!(
                    net,
                    "handshake envelope '{}' (id 0x{:02x}, protocol {}, token type 0x{:02x}, tag {}) confirmed — applying it to every datagram this session",
                    env.label,
                    id,
                    proto,
                    env.cls,
                    hex_head(&env.epoch, 16)
                );
				r
			}
			Err(e) => {
				// Classifiable without re-probing: when a ping probe ran and
				// ponged, the proxy routes us and the RCC rejected the
				// handshake shape; when the shape was pinned without a probe
				// (rcc known), total silence means the proxy dropped every
				// datagram it saw.
				match (pinned, probe_pong) {
					(Some(env), true) => netlog!(
						net,
						"path diagnosis: envelope '{}' got a probe pong but no OCR1 reply — \
                         the proxy routes this shape, the RCC rejects the handshake \
                         (protocol byte, MTU or required encryption)",
						env.label
					),
					(Some(_), false) => netlog!(
						net,
						"path diagnosis: no reply to any OCR1 shape and no ping probe ran — \
                         the proxy dropped every datagram (TokenTlv gate) or the path is dead"
					),
					(None, _) => netlog!(
						net,
						"path diagnosis: no envelope shape answered pings or OCR1 — the proxy \
                         drops unauthenticated datagrams (stale session port, firewall, or \
                         a routing shape we did not try)"
					),
				}
				return Err(e);
			}
		};
		let (server_guid, mtu, use_security) = reply1;
		netlog!(net, "REPLY_1: guid={} mtu={} security={}", server_guid, mtu, use_security);
		c.server_guid = server_guid;
		c.mtu = mtu;

		// Phase 2: OPEN_CONNECTION_REQUEST_2 -> REPLY_2.
		//
		// The envelope pinned in phase 1 (Copy) — every later datagram,
		// including OCR2, must ride the shape the proxy accepted.
		let pinned_env = c.envelope;
		let env_for_pad = pinned_env;

		// Fork crypto path (LIVE-VERIFIED 2026-11-17, probe7.py): when the
		// join script hands out the ephemeral early key AND the join call's
		// own ClientTicket paBlob, the OCR2 is the crypto shape —
		// `78 | magic | 03 5a 00 5d 00 05 | client_pub(32) | auth_ver |
		// 21 | paBlob(33) | ct(93) | "UniqueNumber" | tag(16)` — with the
		// 93-byte pt93 inside the box, and the server answers with the
		// crypto OpenReply2. The paBlob differs per join call; a missing
		// or wrong one draws total silence. The sala matrix stays as the
		// fallback for sessions without (or failing) early material.
		let mut crypto_reply2 = false;
		if c.early.is_some() {
			let early_body =
				c.early.as_mut().and_then(|ec| ec.build_ocr2(c.mtu).map(|(_, body)| body));
			if let Some(mut body) = early_body {
				let hlen = env_for_pad.map(|e| e.direction as usize).unwrap_or(0);
				body.resize(RBX_OFFLINE_PROBE.saturating_sub(hlen), 0x00);
				let res = c.offline_roundtrip(
					&[body],
					handshake_timeout,
					"request-2-crypto",
					|b, out, _mtu| {
						out.clear();
						out.extend_from_slice(b);
						// Keep the capture's total: content ends at 242
						// bytes, the rest is the zero pad to 1160.
						out.resize(RBX_OFFLINE_PROBE.saturating_sub(hlen), 0x00);
					},
					move |_| pinned_env.unwrap_or(crate::net::udmux::BARE),
					|c, payload| c.parse_reply2_crypto(payload),
				);
				match res {
					Ok((_, _)) => {
						netlog!(net, "crypto OpenReply2 accepted — session keys adopted, skipping the sala matrix");
						crypto_reply2 = true;
					}
					Err(e) => {
						netlog!(
							net,
							"crypto OCR2 got no reply ({}); falling back to the sala shapes",
							e
						);
					}
				}
			} else {
				netlog!(
					net,
					"crypto OCR2 unavailable: ClientTicket paBlob missing/short ({} bytes) — \
                     the server verifies it against the join call; using the sala shapes",
					c.early.as_ref().map(|ec| ec.pa_blob.len()).unwrap_or(0)
				);
			}
		}
		let (reply2, _) = if crypto_reply2 {
			((c.server_guid, c.mtu), 0usize)
		} else {
			// The fork's legacy OCR2 has no security cookie and appends two
			// fields the vanilla packet does not carry: SupportedVersion
			// (u32 BE — value not published, matrix 5/1/0) and Capabilities
			// (u64 BE — base client set vs the modern superset). The server
			// address block is XOR-obfuscated (Sala readAddress: family,
			// ip^0xFF, port BE).
			let mut p2_variants: Vec<(u8, u32, u64)> = Vec::new();
			for &sv in &[5u32, 1, 0] {
				p2_variants.push((RBX_ID_OPEN_CONNECTION_REQUEST_2, sv, RBX_CAPABILITY_MODERN));
				p2_variants.push((RBX_ID_OPEN_CONNECTION_REQUEST_2, sv, RBX_CAPABILITY_ROBLOX));
			}
			// Vanilla fallback (direct dials): the legacy OCR2 shape.
			p2_variants.push((ID_OPEN_CONNECTION_REQUEST_2, 0, 0));
			let guid = c.guid;
			c.offline_roundtrip(
				&p2_variants,
				handshake_timeout,
				"request-2",
				|(id, sv, caps), out, mtu| {
					*out = build_ocr2(*id, *sv, *caps, addr, mtu, guid);
					// The capture pads OCR2 to the same 1160-byte datagram as
					// OCR1 (real content ends ~243 bytes in, the rest is zero
					// pad) — the negotiated MTU only governs the connected
					// phase, not the offline probe size.
					let hlen = env_for_pad.map(|e| e.direction as usize).unwrap_or(0);
					out.resize(RBX_OFFLINE_PROBE.saturating_sub(hlen), 0x00);
				},
				move |_| pinned_env.unwrap_or(crate::net::udmux::BARE),
				|c, payload| c.parse_reply2(payload),
			)?
		};
		netlog!(net, "REPLY_2: guid={} mtu={}", reply2.0, reply2.1);
		c.server_guid = reply2.0;

		// Phase 3: CONNECTION_REQUEST -> ACCEPTED -> NEW_INCOMING_CONNECTION.
		//
		// With early crypto the request rides inside an AEAD box
		// (`nonce12 + ct + tag16`) — the dump's pkt4 is exactly
		// envelope(31) + box(52-12-16 = 24-byte plaintext CONNECTION_REQUEST),
		// and the libroblox preauth-window code validates the same
		// `payload + 0x1c` (nonce12+tag16) overhead rule.
		let deadline = Instant::now() + handshake_timeout;
		loop {
			if Instant::now() >= deadline {
				return Err(RakError::TimedOut("connection request"));
			}
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
			let wire: Vec<u8> = match c.early.as_mut() {
				Some(ec) => {
					let boxed = ec.seal_counter(&out);
					netlog!(net, "CONNECTION_REQUEST boxed (pt {} + 18 overhead = {} bytes, counter hint {})", out.len(), boxed.len(), boxed[boxed.len()-18..boxed.len()-16].iter().map(|b| format!("{:02x}", b)).collect::<String>());
					boxed
				}
				None => out,
			};
			if let Err(e) = c.send_raw(&wire) {
				netlog!(net, "CONNECTION_REQUEST send failed: {}", e);
			} else {
				netlog!(net, "CONNECTION_REQUEST sent");
			}

			if let Some(payload) = c.wait_readable(deadline) {
				// Early crypto: post-OpenReply2 datagrams ride the counter
				// framing (`ct || hint u16 LE || tag 16`, nonce = counter ||
				// "mbeR", AAD empty — dump pkt4..7). Plaintext sessions pass
				// through unchanged.
				let payload = match c.early.as_ref() {
					Some(ec) => match ec.open_counter(&payload) {
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
                                "connect-phase datagram did not open as a counter box ({} bytes) — treating as plaintext",
                                payload.len()
                            );
							payload
						}
					},
					None => payload,
				};
				// A 0-byte UDP datagram is legal — payload.first() keeps the
				// match panic-free (payload[0] here took the process down).
				match payload.split_first() {
					Some((&ID_CONNECTION_REQUEST_ACCEPTED, _)) => {
						c.handle_accepted(&payload)?;
						break;
					}
					Some((&ID_CONNECTION_ATTEMPT_FAILED, _)) => {
						return Err(RakError::Rejected("attempt failed"))
					}
					Some((&ID_NO_FREE_INCOMING_CONNECTIONS, _)) => {
						return Err(RakError::Rejected("server full"))
					}
					Some((&ID_CONNECTION_BANNED, _)) => return Err(RakError::Banned),
					Some((&ID_ALREADY_CONNECTED, _)) => return Err(RakError::AlreadyConnected),
					Some((&other, _)) => netlog!(net, "got id 0x{:02x} while connecting", other),
					None => netlog!(net, "ignoring empty datagram during handshake"),
				}
			}
			std::thread::sleep(Duration::from_millis(120));
		}

		c.state = RakState::Connected;
		c.last_recv = Instant::now();
		// Connected phase (live capture): the client switches its header to
		// class byte 0x02 and to the SERVER's 16-byte tag — both sides then
		// frame every datagram with the same session tag (which keeps
		// rotating every second; send_raw mirrors the newest server tag).
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
		// And the crypto framing switches from the tagged handshake boxes to
		// the untagged session stream (pcap: 56-byte keepalives are
		// envelope(31) + nonce(12) + ct(13), no MAC).
		if c.early.is_some() {
			c.crypto_connected = true;
			netlog!(
				net,
				"crypto: switching to the connected-phase stream framing (nonce12+ct, no tag)"
			);
		}
		Ok(c)
	}
}
