//! Headless live-join smoke test (no GUI).
//!
//! Usage:
//!   RUSTBLOX_COOKIE="<.ROBLOSECURITY>" cargo run --example join_flow -- <placeId>
//!
//! Performs the full real pipeline: auth ticket -> gamejoin v2 -> avatar
//! appearance -> RakNet handshake against the game server -> replication
//! diagnostics for a few seconds. RUSTBLOX_DEBUG=net|game enables logs.

use rustblox::raknet::{RakClient, RakState};
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
            println!("      jobId={} ip={} port={} dc={}", r.job_id, r.ip, r.port, r.data_center_id);
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
    let mut rak = match RakClient::connect(&resp.ip, resp.port, &[10, 11, 5, 1], Duration::from_secs(8), udmux_token, rcc) {
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
    let start = Instant::now();
    let mut events_total = 0usize;
    while start.elapsed() < Duration::from_secs(12) {
        while let Some(payload) = rak.poll() {
            events_total += parser.feed(&payload, &mut dm).len();
        }
        rak.tick();
        std::thread::sleep(Duration::from_millis(5));
    }
    println!(
        "      12s diagnostics: raknet state={:?} ping={:?} packets={} decoded={} skipped={} events={} instances={}",
        rak.state,
        rak.ping_ms.map(|p| format!("{:.0}ms", p)),
        parser.packets,
        parser.decoded,
        parser.skipped,
        events_total,
        dm.len()
    );
    if rak.state == RakState::Connected {
        rak.close();
    }
    println!("DONE");
}
