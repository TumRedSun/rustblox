//! Live game session: joins a real Roblox server, drives the replication
//! stream into the DataModel and keeps a renderable SceneView up to date.

use crate::gfx::math::Vec3;
use crate::gfx::scene::{Camera, SceneView};
use crate::net::raknet::{RakClient, RakState};
use crate::netlog;
use crate::roblox::join::JoinResponse;
use crate::world::{build_r6_rig, DataModel};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Events from the join pipeline (HTTP side), delivered through the event
/// loop proxy.
#[derive(Debug)]
pub enum GameEvent {
	Progress(String),
	Joined { resp: JoinResponse, place_name: String },
	JoinFailed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum GamePhase {
	Joining,
	Connecting,
	InGame,
	Failed(String),
	Left,
}

/// Shared state between the network thread and the UI thread.
pub struct GameState {
	pub phase: GamePhase,
	pub place_name: String,
	pub server: String,
	pub job_id: String,
	pub ping_ms: Option<f32>,
	pub packets: u64,
	pub decoded: u64,
	pub skipped: u64,
	/// Real fork replication packets (`0x83..`/`0x8d..`) delivered by the
	/// server (the transport decodes the RakNet framing; the payload layer is
	/// still being reverse-engineered).
	pub replication: u64,
	/// "83/07×12 8d/01×3" — the head histogram, top entries first.
	pub repl_heads: String,
	/// Fork bodies that arrived as zstd frames.
	pub inflated: u64,
	/// Readable strings harvested from the replication stream (class and
	/// property names, part names, usernames).
	pub names: usize,
	pub instances: usize,
	pub players: usize,
	pub world: DataModel,
	pub scene: SceneView,
	pub fps: f32,
	/// Axis-aligned colliders of the decoded world, rebuilt with the scene.
	pub colliders: Vec<Aabb>,
	/// Local character (feet) position + facing, written by the UI thread's
	/// controller and read by the scene builder.
	pub local_pos: Vec3,
	pub local_yaw: f32,
	/// Set once the spawn point has been resolved from the world.
	pub pos_init: bool,
}

/// Axis-aligned box used for the simple character collision (centre ± half
/// extents, already in world space).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
	pub min: Vec3,
	pub max: Vec3,
}

impl Aabb {
	pub fn centre_half(pos: Vec3, half: Vec3) -> Aabb {
		Aabb {
			min: [pos[0] - half[0], pos[1] - half[1], pos[2] - half[2]],
			max: [pos[0] + half[0], pos[1] + half[1], pos[2] + half[2]],
		}
	}
}

pub struct GameSession {
	pub state: Arc<Mutex<GameState>>,
	pub stop: Arc<AtomicBool>,
}

impl GameSession {
	/// Spawn the network thread for a successful join.
	pub fn start(resp: JoinResponse, place_name: String, user_id: u64) -> GameSession {
		let state = Arc::new(Mutex::new(GameState {
			phase: GamePhase::Connecting,
			place_name,
			server: format!("{}:{}", resp.ip, resp.port),
			job_id: resp.job_id.clone(),
			ping_ms: None,
			packets: 0,
			decoded: 0,
			skipped: 0,
			replication: 0,
			repl_heads: String::new(),
			inflated: 0,
			names: 0,
			instances: 0,
			players: 0,
			world: DataModel::new(),
			scene: SceneView::default(),
			fps: 0.0,
			colliders: Vec::new(),
			local_pos: [0.0, 0.0, 0.0],
			local_yaw: 0.0,
			pos_init: false,
		}));
		let stop = Arc::new(AtomicBool::new(false));
		let st = Arc::clone(&state);
		let sp = Arc::clone(&stop);
		let appearance = resp.character_appearance_url.clone();
		let user_id = user_id;
		let sp_catch = Arc::clone(&stop);
		let st_after = Arc::clone(&state);

		std::thread::Builder::new()
			.name("rustblox-game".into())
			.spawn(move || {
				// A panic in this thread must never take the whole client
				// down: the UI locks `state` every frame, so a poisoned
				// mutex would cascade the panic there (the reported "client
				// just crashed"). Catch, report, surface as a Failed phase.
				let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
					game_thread(st, sp, resp, appearance, user_id);
				}));
				drop(sp_catch);
				if let Err(panic) = result {
					let msg = panic_message(&panic);
					netlog!(game, "game thread panicked: {}", msg);
					let mut g = st_after.lock().unwrap_or_else(|p| p.into_inner());
					g.phase = GamePhase::Failed(format!(
						"Внутренняя ошибка игрового потока: {} — пришлите лог из Настройки → Логи",
						msg
					));
				}
			})
			.expect("spawn game thread");

		GameSession { state, stop }
	}

	pub fn stop(&self) {
		self.stop.store(true, Ordering::Relaxed);
	}
}

fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
	if let Some(s) = panic.downcast_ref::<&str>() {
		(*s).to_string()
	} else if let Some(s) = panic.downcast_ref::<String>() {
		s.clone()
	} else {
		"unknown panic".to_string()
	}
}

/// Poison-tolerant state lock: after any (caught) panic the mutex stays
/// poisoned; the UI and the game thread must keep working off the surviving
/// data instead of dying on the next `.unwrap()`.
fn lock_state(state: &std::sync::Mutex<GameState>) -> std::sync::MutexGuard<'_, GameState> {
	state.lock().unwrap_or_else(|p| p.into_inner())
}

fn game_thread(
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

/// First SpawnLocation/SpawnPart position in the world.
fn find_spawn(dm: &DataModel) -> Option<crate::gfx::math::Vec3> {
	let root = dm.find_root("DataModel").or(dm.root)?;
	for id in dm.walk(root) {
		let Some(inst) = dm.get(id) else { continue };
		if matches!(inst.class.as_str(), "SpawnLocation" | "SpawnPart") {
			let pos = match inst.prop("CFrame") {
				Some(crate::world::Prop::CFrame { pos, .. }) => Some(*pos),
				_ => inst.prop("Position").and_then(|p| match p {
					crate::world::Prop::Vec3(v) => Some(*v),
					_ => None,
				}),
			};
			if let Some(mut p) = pos {
				p[1] += 4.0;
				return Some(p);
			}
		}
	}
	None
}

/// Colliders for the local character: every solid part of the decoded world
/// as a world-space AABB (rotation approximated by its extents, which is
/// exact for the axis-aligned parts that dominate Roblox builds).
fn collect_colliders(dm: &DataModel) -> Vec<Aabb> {
	let Some(root) = dm.find_root("DataModel").or(dm.root) else { return Vec::new() };
	let mut out = Vec::new();
	for id in dm.walk(root) {
		let Some(inst) = dm.get(id) else { continue };
		if !matches!(
			inst.class.as_str(),
			"Part"
				| "WedgePart"
				| "CornerWedgePart"
				| "MeshPart"
				| "TrussPart"
				| "SpawnLocation"
				| "UnionOperation"
		) {
			continue;
		}
		let (pos, size) = match (inst.prop("CFrame"), inst.prop("Size")) {
			(Some(crate::world::Prop::CFrame { pos, .. }), Some(crate::world::Prop::Vec3(s))) => {
				(pos, s)
			}
			_ => continue,
		};
		let rot = match inst.prop("CFrame") {
			Some(crate::world::Prop::CFrame { rot, .. }) => *rot,
			_ => [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
		};
		// Column-major basis: columns are rot[0..3], rot[3..6], rot[6..9].
		let half = [
			(rot[0].abs() * size[0] + rot[3].abs() * size[1] + rot[6].abs() * size[2]) * 0.5,
			(rot[1].abs() * size[0] + rot[4].abs() * size[1] + rot[7].abs() * size[2]) * 0.5,
			(rot[2].abs() * size[0] + rot[5].abs() * size[1] + rot[8].abs() * size[2]) * 0.5,
		];
		if !pos.iter().all(|v| v.is_finite()) || !half.iter().all(|v| v.is_finite()) {
			continue;
		}
		out.push(Aabb::centre_half(*pos, half));
		if out.len() >= 20_000 {
			break;
		}
	}
	out
}

/// Which movement model the local character uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlMode {
	/// Roblox-like: walk on the ground, jump, gravity, collisions (default).
	Walk,
	/// Debug free-fly (F toggles).
	Fly,
}

/// Local character + camera controller.
#[derive(Clone)]
pub struct GameController {
	pub camera: Camera,
	pub pointer_locked: bool,
	pub last_mouse: (f32, f32),
	pub keys: std::collections::HashSet<winit::keyboard::PhysicalKey>,
	/// Touch pad (mobile): joystick deflection (strafe, fwd), each -1..=1.
	pub touch_stick: (f32, f32),
	/// Touch pad (mobile): +1 while the up button is held, -1 down, 0 none.
	pub touch_lift: f32,
	/// Character feet position (walk mode).
	pub pos: crate::gfx::math::Vec3,
	pub vel: crate::gfx::math::Vec3,
	pub on_ground: bool,
	/// Facing yaw of the character.
	pub yaw: f32,
	pub mode: ControlMode,
	/// Pending jump request (consumed by `step`).
	pub jump: bool,
	/// Pending respawn request (R key).
	pub respawn: bool,
	/// True once the character was placed at the spawn point.
	pub spawned: bool,
}

impl Default for GameController {
	fn default() -> Self {
		GameController {
			camera: Camera::default(),
			pointer_locked: false,
			last_mouse: (-1.0, -1.0),
			keys: std::collections::HashSet::new(),
			touch_stick: (0.0, 0.0),
			touch_lift: 0.0,
			pos: [0.0, 0.0, 0.0],
			vel: [0.0, 0.0, 0.0],
			on_ground: false,
			yaw: 0.0,
			mode: ControlMode::Walk,
			jump: false,
			respawn: false,
			spawned: false,
		}
	}
}

/// Roblox R6-ish body dimensions (studs).
const BODY_RADIUS: f32 = 1.1;
const BODY_HEIGHT: f32 = 5.0;
const WALK_SPEED: f32 = 16.0;
const FLY_SPEED: f32 = 30.0;
const GRAVITY: f32 = 196.2;
const JUMP_SPEED: f32 = 50.0;
/// Third-person camera distance behind the character.
const CAM_BACK: f32 = 12.0;

impl GameController {
	/// Place the character (and the camera) at a spawn point.
	pub fn place(&mut self, feet: crate::gfx::math::Vec3) {
		self.pos = feet;
		self.vel = [0.0; 3];
		self.on_ground = false;
		self.spawned = true;
		self.update_camera();
	}

	pub fn toggle_fly(&mut self) {
		self.mode = match self.mode {
			ControlMode::Walk => ControlMode::Fly,
			ControlMode::Fly => ControlMode::Walk,
		};
		self.vel = [0.0; 3];
		if self.mode == ControlMode::Walk {
			// Drop out of the air onto whatever is below.
			self.on_ground = false;
		}
	}

	fn held(&self, code: winit::keyboard::KeyCode) -> bool {
		self.keys.contains(&winit::keyboard::PhysicalKey::Code(code))
	}

	/// Wish direction on the ground plane from WASD + the touch stick,
	/// relative to where the camera looks.
	fn wish_dir(&self, axes: (f32, f32)) -> (f32, f32) {
		let fwd = self.camera.forward();
		let flat = (fwd[0] * fwd[0] + fwd[2] * fwd[2]).sqrt();
		let (fx, fz) = if flat > 1e-4 { (fwd[0] / flat, fwd[2] / flat) } else { (0.0, -1.0) };
		let right = self.camera.right();
		let (ix, iz) = axes;
		let x = fx * iz + right[0] * ix;
		let z = fz * iz + right[2] * ix;
		let len = (x * x + z * z).sqrt();
		if len > 1e-4 {
			(x / len, z / len)
		} else {
			(0.0, 0.0)
		}
	}

	fn axes(&self) -> (f32, f32) {
		use winit::keyboard::KeyCode;
		let mut ix = 0.0;
		let mut iz = 0.0;
		if self.held(KeyCode::KeyW) || self.held(KeyCode::ArrowUp) {
			iz += 1.0;
		}
		if self.held(KeyCode::KeyS) || self.held(KeyCode::ArrowDown) {
			iz -= 1.0;
		}
		if self.held(KeyCode::KeyD) || self.held(KeyCode::ArrowRight) {
			ix += 1.0;
		}
		if self.held(KeyCode::KeyA) || self.held(KeyCode::ArrowLeft) {
			ix -= 1.0;
		}
		let (sx, sy) = self.touch_stick;
		ix += sx;
		iz += sy;
		let len = (ix * ix + iz * iz).sqrt();
		if len > 1.0 {
			(ix / len, iz / len)
		} else {
			(ix, iz)
		}
	}

	/// Advance the character / camera by `dt`. `colliders` are the world-space
	/// boxes of the decoded scene (empty before replication arrives).
	pub fn step(&mut self, dt: f32, colliders: &[Aabb]) {
		let axes = self.axes();
		match self.mode {
			ControlMode::Fly => self.step_fly(dt, axes),
			ControlMode::Walk => self.step_walk(dt, axes, colliders),
		}
		self.update_camera();
	}

	fn step_fly(&mut self, dt: f32, axes: (f32, f32)) {
		use winit::keyboard::KeyCode;
		let fwd = self.camera.forward();
		let right = self.camera.right();
		let mut delta = crate::gfx::math::add3(
			crate::gfx::math::scale3(fwd, axes.1),
			crate::gfx::math::scale3(right, axes.0),
		);
		if self.held(KeyCode::Space) {
			delta[1] += 1.0;
		}
		if self.held(KeyCode::ShiftLeft) || self.held(KeyCode::ShiftRight) {
			delta[1] -= 1.0;
		}
		if self.touch_lift.abs() > 1e-3 {
			delta[1] += self.touch_lift;
		}
		let len = crate::gfx::math::len3(delta);
		if len > 1e-4 {
			let d = crate::gfx::math::scale3(delta, FLY_SPEED * dt / len);
			self.camera.pos = crate::gfx::math::add3(self.camera.pos, d);
		}
	}

	fn step_walk(&mut self, dt: f32, axes: (f32, f32), colliders: &[Aabb]) {
		let (wx, wz) = self.wish_dir(axes);
		let want = [wx * WALK_SPEED, 0.0, wz * WALK_SPEED];
		// Snappy ground control: blend the horizontal velocity toward the wish.
		let blend = (dt * 14.0).min(1.0);
		self.vel[0] += (want[0] - self.vel[0]) * blend;
		self.vel[2] += (want[2] - self.vel[2]) * blend;
		if (wx * wx + wz * wz).sqrt() > 1e-4 {
			self.yaw = wx.atan2(wz);
		}

		if self.on_ground && self.jump {
			self.vel[1] = JUMP_SPEED;
			self.on_ground = false;
		}
		self.jump = false;
		self.vel[1] -= GRAVITY * dt;
		if self.vel[1] < -200.0 {
			self.vel[1] = -200.0;
		}

		let prev_y = self.pos[1];
		self.pos[0] += self.vel[0] * dt;
		self.resolve_horizontal(colliders, 0);
		self.pos[2] += self.vel[2] * dt;
		self.resolve_horizontal(colliders, 2);
		self.pos[1] += self.vel[1] * dt;
		self.resolve_vertical(colliders, prev_y);

		// Safety net: never fall through the world (the rendered baseplate
		// sits at y = 0 until the server's ground is decoded).
		if self.pos[1] < 0.0 {
			self.pos[1] = 0.0;
			if self.vel[1] < 0.0 {
				self.vel[1] = 0.0;
				self.on_ground = true;
			}
		}
		if self.respawn {
			self.respawn = false;
			if self.pos[1] < 0.0 {
				self.pos[1] = 8.0;
			}
		}
	}

	fn overlaps(&self, c: &Aabb) -> bool {
		let p = self.pos;
		p[0] + BODY_RADIUS > c.min[0]
			&& p[0] - BODY_RADIUS < c.max[0]
			&& p[1] + BODY_HEIGHT > c.min[1]
			&& p[1] < c.max[1]
			&& p[2] + BODY_RADIUS > c.min[2]
			&& p[2] - BODY_RADIUS < c.max[2]
	}

	fn resolve_horizontal(&mut self, colliders: &[Aabb], axis: usize) {
		for c in colliders {
			if !self.overlaps(c) {
				continue;
			}
			// Ignore boxes we are standing on top of.
			if self.pos[1] >= c.max[1] - 0.05 {
				continue;
			}
			if self.vel[axis] > 0.0 {
				self.pos[axis] = c.min[axis] - BODY_RADIUS - 1e-3;
			} else if self.vel[axis] < 0.0 {
				self.pos[axis] = c.max[axis] + BODY_RADIUS + 1e-3;
			}
			self.vel[axis] = 0.0;
		}
	}

	fn resolve_vertical(&mut self, colliders: &[Aabb], prev_y: f32) {
		let mut landed = false;
		for c in colliders {
			if self.pos[0] + BODY_RADIUS <= c.min[0]
				|| self.pos[0] - BODY_RADIUS >= c.max[0]
				|| self.pos[2] + BODY_RADIUS <= c.min[2]
				|| self.pos[2] - BODY_RADIUS >= c.max[2]
			{
				continue;
			}
			if self.vel[1] <= 0.0
				&& prev_y >= c.max[1] - 0.35
				&& self.pos[1] <= c.max[1]
				&& self.pos[1] + BODY_HEIGHT > c.min[1]
			{
				self.pos[1] = c.max[1];
				landed = true;
				continue;
			}
			if self.vel[1] > 0.0 && self.overlaps(c) {
				self.pos[1] = c.min[1] - BODY_HEIGHT - 1e-3;
				self.vel[1] = 0.0;
			}
		}
		if landed {
			self.vel[1] = 0.0;
			self.on_ground = true;
		} else if self.vel[1].abs() > 0.001 {
			self.on_ground = false;
		}
	}

	/// Chase camera: sit behind the character along the look direction.
	fn update_camera(&mut self) {
		if self.mode == ControlMode::Fly {
			return;
		}
		let fwd = self.camera.forward();
		let eye = [self.pos[0], self.pos[1] + 4.6, self.pos[2]];
		let mut cam =
			[eye[0] - fwd[0] * CAM_BACK, eye[1] - fwd[1] * CAM_BACK, eye[2] - fwd[2] * CAM_BACK];
		// Never dip below the character's feet.
		if cam[1] < self.pos[1] + 0.8 {
			cam[1] = self.pos[1] + 0.8;
		}
		self.camera.pos = cam;
	}

	pub fn mouse_delta(&mut self, x: f32, y: f32) {
		if !self.pointer_locked {
			self.last_mouse = (x, y);
			return;
		}
		if self.last_mouse.0 < 0.0 {
			self.last_mouse = (x, y);
			return;
		}
		let dx = x - self.last_mouse.0;
		let dy = y - self.last_mouse.1;
		self.last_mouse = (x, y);
		self.camera.yaw -= dx * 0.0028;
		self.camera.pitch = (self.camera.pitch - dy * 0.0028).clamp(-1.35, 1.1);
	}

	/// Camera look from a touch drag (mobile game pad): the finger delta
	/// rotates the camera with the same sensitivity as the locked mouse.
	pub fn touch_look(&mut self, dx: f32, dy: f32) {
		self.camera.yaw -= dx * 0.0028;
		self.camera.pitch = (self.camera.pitch - dy * 0.0028).clamp(-1.35, 1.1);
	}
}

#[cfg(test)]
mod controller_tests {
	use super::*;
	use winit::keyboard::{KeyCode, PhysicalKey};

	fn walker() -> GameController {
		let mut c = GameController::default();
		c.place([0.0, 0.0, 0.0]);
		c
	}

	#[test]
	fn character_falls_and_lands() {
		let mut c = walker();
		c.pos = [0.0, 10.0, 0.0];
		for _ in 0..240 {
			c.step(1.0 / 60.0, &[]);
		}
		assert!(c.on_ground, "character should be standing");
		assert!(c.pos[1].abs() < 0.01, "feet at y=0, got {}", c.pos[1]);
	}

	#[test]
	fn wasd_moves_and_faces_the_move_direction() {
		let mut c = walker();
		c.keys.insert(PhysicalKey::Code(KeyCode::KeyW));
		for _ in 0..60 {
			c.step(1.0 / 60.0, &[]);
		}
		// yaw 0 looks along -Z, so W walks toward -Z.
		assert!(c.pos[2] < -5.0, "walked along -Z, z={}", c.pos[2]);
		assert!(c.pos[0].abs() < 0.5);
	}

	#[test]
	fn space_jumps() {
		let mut c = walker();
		for _ in 0..30 {
			c.step(1.0 / 60.0, &[]);
		}
		assert!(c.on_ground);
		c.jump = true;
		c.step(1.0 / 60.0, &[]);
		assert!(c.vel[1] > 10.0, "jump impulse applied, vy={}", c.vel[1]);
		assert!(!c.on_ground);
	}

	#[test]
	fn collider_blocks_walking() {
		let mut c = walker();
		// Wall spanning x at z = -10.
		let wall = Aabb::centre_half([0.0, 5.0, -10.0], [20.0, 5.0, 1.0]);
		c.keys.insert(PhysicalKey::Code(KeyCode::KeyW));
		for _ in 0..180 {
			c.step(1.0 / 60.0, &[wall]);
		}
		// Stopped in front of the wall (its face is at z = -9).
		assert!(c.pos[2] > -9.5, "blocked by the wall, z={}", c.pos[2]);
	}

	#[test]
	fn stands_on_a_collider_top() {
		let mut c = walker();
		let platform = Aabb::centre_half([0.0, 5.0, 0.0], [10.0, 5.0, 10.0]);
		c.pos = [0.0, 15.0, 0.0];
		for _ in 0..240 {
			c.step(1.0 / 60.0, &[platform]);
		}
		assert!(c.on_ground);
		assert!((c.pos[1] - 10.0).abs() < 0.05, "standing on the box top, y={}", c.pos[1]);
	}

	#[test]
	fn fly_mode_ignores_gravity() {
		let mut c = walker();
		c.toggle_fly();
		c.keys.insert(PhysicalKey::Code(KeyCode::Space));
		let y0 = c.camera.pos[1];
		for _ in 0..30 {
			c.step(1.0 / 60.0, &[]);
		}
		assert!(c.camera.pos[1] > y0 + 5.0, "flew up: {} -> {}", y0, c.camera.pos[1]);
	}
}
