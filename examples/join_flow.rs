//! Headless live-join smoke test (no GUI).
//!
//! Usage:
//!   RUSTBLOX_COOKIE="<.ROBLOSECURITY>" cargo run --example join_flow -- <placeId>
//!
//! Performs the full real pipeline: auth ticket -> gamejoin v2 -> avatar
//! appearance -> RakNet handshake against the game server -> replication
//! diagnostics for a few seconds. RUSTBLOX_DEBUG=net|game enables logs.

use rustblox::net::raknet::{RakClient, RakState};
use rustblox::roblox::client::RobloxClient;
use rustblox::roblox::join;
use std::time::{Duration, Instant};

fn main() {
	let args: Vec<String> = std::env::args().collect();
	if args.len() < 2 {
		eprintln!("usage: join_flow <placeId>");
		std::process::exit(2);
	}
	let place_id: u64 = args[1].parse().expect("placeId must be a number");
	let cookie = std::env::var("RUSTBLOX_COOKIE").unwrap_or_default();

	let mut client = RobloxClient::new();
	if !cookie.is_empty() {
		client.set_security(&rustblox::roblox::client::normalize_cookie_token(&cookie));
		println!("[1/4] session cookie set");
	} else {
		println!("[1/4] no RUSTBLOX_COOKIE — join flow will likely fail with 401");
	}

	println!("[2/4] join_game(place {}) — auth ticket + gamejoin v2/v1...", place_id);
	let resp = match join::join_game(&mut client, place_id) {
		Ok(r) => {
			println!(
				"      jobId={} ip={} port={} dc={}",
				r.job_id, r.ip, r.port, r.data_center_id
			);
			println!("      appearance url: {}", r.character_appearance_url);
			r
		}
		Err(e) => {
			eprintln!("      JOIN FAILED: {}", e);
			eprintln!("      (check the cookie; captchas/guest joins are rejected server-side)");
			std::process::exit(1);
		}
	};

	println!("[3/4] fetching avatar appearance...");
	let appearance = join::fetch_avatar_appearance(&mut client, &resp.character_appearance_url, 0)
		.unwrap_or_default();
	println!("      avatar type: {}", appearance.player_avatar_type);

	println!("[4/4] RakNet handshake against {}:{}...", resp.ip, resp.port);
	// Roblox hands out private RCC addresses (10.x.x.x): dial the join
	// script's public UDMUX proxy instead (same logic as the game flow).
	let mut dial_host = resp.ip.clone();
	let mut dial_port = resp.port;
	if !resp.udmux_host.is_empty() {
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
		println!("      private RCC — dialing the udmux proxy {}", resp.udmux_host);
	}
	// The join script may hand out a udmux routing token — the example dials
	// the RCC address directly, so the token is only forwarded when present.
	// The private RCC address also rides inside the client RUPP header
	// (`02 06 <ip4> <port BE16>`) — pass it so the proxy can route.
	let udmux_token = if resp.udmux_token.is_empty() {
		None
	} else {
		Some((resp.udmux_token.as_str(), resp.udmux_pepper))
	};
	let rcc = if udmux_token.is_some() {
		resp.ip.parse::<std::net::Ipv4Addr>().ok().map(|ip| (ip, resp.port))
	} else {
		None
	};
	// Early-encryption material from the join script (live-verified
	// 2026-11-17): the crypto OCR2/OpenReply2 handshake keyed by
	// EphemeralEarlyPubKey + the join call's own ClientTicket paBlob.
	let early = rustblox::net::crypto::EarlyCrypto::from_join(
		&resp.ephemeral_pubkey,
		resp.client_ticket_pa_blob.clone(),
		if resp.client_ticket_auth_ver == 0 {
			rustblox::net::crypto::OCR2_AUTH_VER
		} else {
			resp.client_ticket_auth_ver
		},
	);
	let mut rak = match RakClient::connect(
		&dial_host,
		dial_port,
		&[10, 11, 5, 1],
		Duration::from_secs(8),
		udmux_token,
		rcc,
		early,
	) {
		Ok(c) => {
			println!("      CONNECTED (server guid {})", c.server_guid);
			c
		}
		Err(e) => {
			eprintln!("      RAKNET FAILED: {}", e);
			eprintln!("      The join flow is real; the server may reject unknown clients");
			eprintln!("      or the protocol version guess may need an update.");
			std::process::exit(1);
		}
	};

	// Replication diagnostics.
	let mut parser = rustblox::roblox::replication::ReplicationParser::new();
	let mut dm = rustblox::world::DataModel::new();
	// Experimental: replay the captured client-side game-join payloads
	// (`a7 90 92 8a 8f`, see analysis/gamejoin_frames.py) once the server
	// has sent its policy message. Used to find out what the server needs
	// before it starts replicating; the blobs are opaque (app-layer
	// encrypted) so they cannot be synthesized yet.
	let replay_dir = std::env::var("RUSTBLOX_REPLAY_JOIN").ok();
	let mut replay: Vec<(String, Vec<u8>)> = Vec::new();
	if let Some(dir) = &replay_dir {
		for name in ["a7_321.bin", "90_359.bin", "92_359.bin", "8a_363.bin", "8f_363.bin"] {
			let path = format!("{}/{}", dir, name);
			if let Ok(b) = std::fs::read(&path) {
				println!("      replay blob {} ({} bytes)", name, b.len());
				replay.push((name.to_string(), b));
			}
		}
	}
	let mut replayed = false;
	let start = Instant::now();
	let mut events_total = 0usize;
	let mut app_log = 0usize;
	while start.elapsed() < Duration::from_secs(12) {
		while let Some(payload) = rak.poll() {
			if payload.first().map(|b| b & 0x80 != 0).unwrap_or(false) && app_log < 40 {
				app_log += 1;
				let head: String =
					payload[..payload.len().min(20)].iter().map(|b| format!("{:02x}", b)).collect();
				println!("      app msg len={} head={}", payload.len(), head);
				if payload[0] == 0x9b && payload.len() > 9 {
					// Experiment: send the 9-byte reply captured from the real
					// client (head 0x9b + 8 opaque bytes) and see whether the
					// server accepts it (account-constant) or not (per-challenge).
					if let Ok(path) = std::env::var("RUSTBLOX_9B_REPLY") {
						if let Ok(b) = std::fs::read(&path) {
							rak.send_reliable(&b);
							rak.flush();
							println!(
								"      -> sent captured 9b reply ({} bytes): {}",
								b.len(),
								b.iter().map(|x| format!("{:02x}", x)).collect::<String>()
							);
							continue;
						}
					}
				}
				if payload[0] == 0x9b
					&& payload.len() > 9
					&& std::env::var("RUSTBLOX_ANSWER_9B").is_ok()
				{
					// The client's answer echoes server bytes [5..9] and adds
					// four opaque bytes (capture #377). Try a zero tail.
					let mut ans = vec![0x9bu8];
					ans.extend_from_slice(&payload[5..9]);
					ans.extend_from_slice(&[0u8; 4]);
					rak.send_reliable(&ans);
					println!(
						"      -> answered 9b: {}",
						ans.iter().map(|b| format!("{:02x}", b)).collect::<String>()
					);
				}
			}
			if !replayed && !replay.is_empty() && payload.first() == Some(&0x93) {
				replayed = true;
				for (name, blob) in &replay {
					rak.send_reliable(blob);
					println!("      -> sent replay {}", name);
				}
				rak.flush();
			}
			events_total += parser.feed(&payload, &mut dm).len();
		}
		rak.tick();
		std::thread::sleep(Duration::from_millis(5));
	}
	println!(
        "      12s diagnostics: raknet state={:?} ping={:?} packets={} decoded={} skipped={} replication={} zstd={} inflated-bytes={} events={} instances={} names={}",
        rak.state,
        rak.ping_ms.map(|p| format!("{:.0}ms", p)),
        parser.packets,
        parser.decoded,
        parser.skipped,
        parser.replication,
        parser.inflated,
        parser.bytes_out,
        events_total,
        dm.len(),
        dm.strings.len()
    );
	if !dm.strings.is_empty() {
		let sample: Vec<&str> = dm.strings.iter().take(8).map(|s| s.as_str()).collect();
		println!("      decoded names: {:?}", sample);
	}
	// Which replication heads the server actually sends (top 6).
	let mut heads: Vec<(u16, u64)> = parser.heads.iter().map(|(k, v)| (*k, *v)).collect();
	heads.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
	if !heads.is_empty() {
		println!(
			"      replication heads: {}",
			heads
				.iter()
				.take(6)
				.map(|(h, n)| format!("{:02x}{:02x}×{}", h >> 8, h & 0xff, n))
				.collect::<Vec<_>>()
				.join(" ")
		);
	}
	if rak.state == RakState::Connected {
		rak.close();
	}
	println!("DONE");
}
