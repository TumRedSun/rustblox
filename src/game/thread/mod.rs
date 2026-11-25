//! Background game thread: dial the RCC/UDMUX endpoint, run the RakNet +
//! early-crypto handshake, pump replication into the DataModel and rebuild
//! the renderable scene a few times per second.

use super::state::{lock_state, GamePhase, GameState};
use crate::net::raknet::{RakClient, RakState};
use crate::netlog;
use crate::roblox::join::JoinResponse;
use crate::world::build_r6_rig;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod world;
use world::{collect_colliders, find_spawn};

pub(super) fn game_thread(
	state: Arc<Mutex<GameState>>,
	stop: Arc<AtomicBool>,
	resp: JoinResponse,
	appearance_url: String,
	user_id: u64,
) {
	// Roblox hands out private RCC addresses (10.x.x.x) that are unroutable
	// from the open internet: real clients dial the datacenter's public
	// UDMUX proxy instead. Resolve that proxy up front and log every
	// decision so a failed join is diagnosable from the log alone.
	//
	// NOTE: host and port are carried SEPARATELY from here on — the join
	// script's udmux endpoint already includes its own port (equal to the
	// RCC port today, but not guaranteed), and appending resp.port to a
	// host:port string used to produce "ip:port:port", which to_socket_addrs
	// tried to resolve as a DNS name ("Name or service not known").
	let mut dial_host = resp.ip.clone();
	let mut dial_port = resp.port;
	if !resp.udmux_host.is_empty() {
		netlog!(
			net,
			"join script supplies udmux endpoint {} — dialing it (rcc {}:{})",
			resp.udmux_host,
			resp.ip,
			resp.port
		);
		match resp.udmux_host.rsplit_once(':') {
			Some((h, p)) => {
				dial_host = h.to_string();
				if let Ok(p) = p.parse::<u16>() {
					if p != 0 {
						dial_port = p;
					}
				}
			}
			None => dial_host = resp.udmux_host.clone(),
		}
	} else {
		match resp.ip.parse::<std::net::Ipv4Addr>() {
			Ok(ip) if crate::net::udmux::is_private(ip) => match crate::net::udmux::proxy_for(ip) {
				Some(proxy) => {
					netlog!(
						net,
						"rcc {} is private — dialing udmux proxy {}:{} (same port)",
						ip,
						proxy,
						resp.port
					);
					dial_host = proxy.to_string();
				}
				None => {
					netlog!(
                            net,
                            "rcc {} is private and no udmux proxy is known for the 10.{}.x.x prefix \
                             — direct dial will not leave the LAN (set RUSTBLOX_UDMUX=10.{}=<proxy-ip>)",
                            ip,
                            ip.octets()[1],
                            ip.octets()[1]
                        );
				}
			},
			Ok(_) => {}
			Err(_) => netlog!(net, "rcc address {:?} is not an IPv4 literal", resp.ip),
		}
	}
	netlog!(game, "connecting to {}:{} (rcc {}:{})", dial_host, dial_port, resp.ip, resp.port);
	// RakNet handshake with several protocol version guesses (the Roblox
	// fork's exact protocol byte is not public knowledge; vanilla RakNet 4
	// defaults to 5, Minecraft Bedrock bumped to 10/11). Every guess now
	// really gets sent — each variant gets its own retransmit window — and
	// each attempt is logged, so the log shows which shapes were tried.
	// When the join script hands out both the udmux routing token and the
	// private RCC address, phase 0 pins the live-capture client envelope
	// (`01 00 00 1f 01 11 01 <tag> 02 06 <rcc ip:port>`) without probing;
	// otherwise the legacy token-derived probe matrix runs (see
	// udmux::envelope_candidates), so a udmux session needs a longer budget.
	let protocols: [u8; 4] = [5, 10, 11, 1];
	let udmux_token = if resp.udmux_token.is_empty() {
		None
	} else {
		Some((resp.udmux_token.as_str(), resp.udmux_pepper))
	};
	// The private RCC address rides verbatim inside the client RUPP header
	// (`02 06 <ip4> <port BE16>`) — the udmux proxy routes on it (live
	// capture, Nov 2026). Only meaningful for a proxy session.
	let rcc = if udmux_token.is_some() {
		resp.ip.parse::<std::net::Ipv4Addr>().ok().map(|ip| (ip, resp.port))
	} else {
		None
	};
	let handshake_timeout =
		if udmux_token.is_some() { Duration::from_secs(30) } else { Duration::from_secs(6) };
	// Early-encryption material (live-verified 2026-11-17): the crypto
	// OCR2/OpenReply2 handshake keyed by the join script's percent-encoded
	// `EphemeralEarlyPubKey`, with the join call's own ClientTicket paBlob
	// (part2 — the server verifies it against THIS join call) and part4
	// authVer ("18" -> 0x12). Built here so a missing or undecodable key
	// falls back to the legacy handshake with a log line.
	let early = crate::net::crypto::EarlyCrypto::from_join(
		&resp.ephemeral_pubkey,
		resp.client_ticket_pa_blob.clone(),
		if resp.client_ticket_auth_ver == 0 {
			crate::net::crypto::OCR2_AUTH_VER
		} else {
			resp.client_ticket_auth_ver
		},
	);
	if early.is_none() && !resp.ephemeral_pubkey.is_empty() {
		netlog!(
            game,
            "early crypto: join script carried EphemeralEarlyPubKey ({} chars) but it did not decode to a 32-byte X25519 key — using the legacy handshake",
            resp.ephemeral_pubkey.len()
        );
	}
	let mut client = match RakClient::connect(
		&dial_host,
		dial_port,
		&protocols,
		handshake_timeout,
		udmux_token,
		rcc,
		early,
	) {
		Ok(c) => c,
		Err(e) => {
			netlog!(
				game,
				"raknet handshake to {}:{} failed after {} protocol attempt(s): {}",
				dial_host,
				dial_port,
				protocols.len(),
				e
			);
			// Hypothesis (b) diagnostic: if the join script handed us a
			// NetStack side-channel, probe it before giving up — game
			// traffic may have moved off the legacy udmux port entirely.
			// The probe is read-only and its verdict goes to the log.
			if resp.netstack_port != 0 {
				netlog!(
                    game,
                    "netstack probe: join script advertises NetStackPort {} — testing hypothesis (b) (transport on the NetStack channel)",
                    resp.netstack_port
                );
				match crate::net::raknet::netstack_probe(
                    &dial_host,
                    resp.netstack_port,
                    &resp.netstack_token,
                    Duration::from_millis(400),
                ) {
                    Some(shape) => netlog!(
                        game,
                        "netstack probe: channel ALIVE via '{}' — the transport moved to NetStack; the session must be ported onto it next",
                        shape
                    ),
                    None => netlog!(
                        game,
                        "netstack probe: silent — the NetStack channel is not the answer for this session; next suspects are the token-tag format (hypothesis (a), TokenGenAlgorithm={}) or early encryption (hypothesis (c))",
                        resp.token_gen_algorithm
                    ),
                }
			}
			let mut st = lock_state(&state);
			st.phase = GamePhase::Failed(format!("Не удалось подключиться: {}", e));
			return;
		}
	};
	netlog!(game, "raknet connected: {}:{} (rcc {}:{})", dial_host, dial_port, resp.ip, resp.port);
	lock_state(&state).phase = GamePhase::InGame;

	// Fetch our avatar appearance (best effort, no cookie needed for the
	// standard endpoint).
	let mut anon = crate::roblox::client::RobloxClient::new();
	let appearance =
		crate::roblox::join::fetch_avatar_appearance(&mut anon, &appearance_url, user_id)
			.unwrap_or_default();

	let mut last_scene = Instant::now();
	let mut last_pkt_time = Instant::now();
	let mut logged_scene = false;

	while !stop.load(Ordering::Relaxed) {
		// Pump RakNet: all available payloads.
		let mut got_any = false;
		while let Some(payload) = client.poll() {
			got_any = true;
			let events = {
				let mut st = lock_state(&state);
				let events = parser().feed(&payload, &mut st.world);
				st.packets += 1;
				st.decoded += events
					.iter()
					.filter(|e| !matches!(e, crate::roblox::replication::RepEvent::Unknown { .. }))
					.count() as u64;
				events
			};
			let _ = events;
			let _ = last_pkt_time;
			last_pkt_time = Instant::now();
		}
		let _ = got_any;
		client.tick();

		let ping = client.ping_ms;
		{
			let mut st = lock_state(&state);
			st.ping_ms = ping;
			st.skipped = parser().skipped;
			st.replication = parser().replication;
			st.inflated = parser().inflated;
			st.names = st.world.strings.len();
			if st.replication > 0 && st.repl_heads.is_empty() {
				let mut heads: Vec<(u16, u64)> =
					parser().heads.iter().map(|(k, v)| (*k, *v)).collect();
				heads.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
				st.repl_heads = heads
					.iter()
					.take(4)
					.map(|(h, n)| format!("{:02x}/{:02x}x{}", h >> 8, h & 0xff, n))
					.collect::<Vec<_>>()
					.join(" ");
			}
		}
		if client.state == RakState::Disconnected {
			let mut st = lock_state(&state);
			if st.phase == GamePhase::InGame {
				st.phase = GamePhase::Failed("Соединение с сервером потеряно".into());
			}
			break;
		}

		// Rebuild the renderable scene a few times per second.
		if last_scene.elapsed() >= Duration::from_millis(180) {
			last_scene = Instant::now();
			let mut st = lock_state(&state);
			st.scene.clear();
			st.scene.env = crate::gfx::scene::Environment::daylight();
			st.instances = st.world.len();
			{
				let world = std::mem::take(&mut st.world);
				let _ = world.build_scene(&mut st.scene);
				// Colliders + spawn point come from the decoded world; the
				// UI thread's character controller resolves against them.
				st.colliders = collect_colliders(&world);
				if !st.pos_init {
					if let Some(sp) = find_spawn(&world) {
						st.local_pos = sp;
						st.pos_init = true;
					}
				}
				st.world = world;
			}
			// Always render a recognizable place: sky (from the environment),
			// a baseplate with a grid and the local character. Real replicated
			// parts simply sit on top of it.
			crate::world::build_ground(&mut st.scene);
			if st.scene.parts.len() < 6000 {
				// The rig's origin sits half a stud above the feet (legs are
				// centred at +0.5), so lift it off the ground plane.
				let origin = [st.local_pos[0], st.local_pos[1] + 0.5, st.local_pos[2]];
				let _ = build_r6_rig(origin, st.local_yaw, &appearance, &mut st.scene.parts);
			}
			if !logged_scene {
				logged_scene = true;
				netlog!(
					game,
					"scene: {} parts ({} world colliders), spawn {:?}, appearance parts {}",
					st.scene.parts.len(),
					st.colliders.len(),
					st.local_pos,
					appearance.body_colors.torso_color_id
				);
			}
		}

		std::thread::sleep(Duration::from_millis(2));
	}
	client.close();
	let mut st = lock_state(&state);
	if st.phase == GamePhase::InGame {
		st.phase = GamePhase::Left;
	}
	netlog!(game, "game thread finished");
}

static PARSER: std::sync::OnceLock<
	std::sync::Mutex<crate::roblox::replication::ReplicationParser>,
> = std::sync::OnceLock::new();

fn parser() -> std::sync::MutexGuard<'static, crate::roblox::replication::ReplicationParser> {
	PARSER
		.get_or_init(|| std::sync::Mutex::new(crate::roblox::replication::ReplicationParser::new()))
		.lock()
		// Poison-tolerant: a panicked feed() must not brick every later
		// packet with a lock unwrap.
		.unwrap_or_else(|p| p.into_inner())
}
