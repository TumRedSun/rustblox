//! Live game session: joins a real Roblox server, drives the replication
//! stream into the DataModel and keeps a renderable SceneView up to date.

use crate::gfx::scene::{Camera, SceneView};
use crate::netlog;
use crate::raknet::{RakClient, RakState};
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
    pub instances: usize,
    pub players: usize,
    pub world: DataModel,
    pub scene: SceneView,
    pub fps: f32,
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
            instances: 0,
            players: 0,
            world: DataModel::new(),
            scene: SceneView::default(),
            fps: 0.0,
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
            Ok(ip) if crate::roblox::udmux::is_private(ip) => {
                match crate::roblox::udmux::proxy_for(ip) {
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
                }
            }
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
    // When the join script carries a udmux routing token, phase 0 first
    // probes token-derived RUPP envelope headers (see
    // udmux::envelope_candidates), so a udmux session needs a longer budget.
    let protocols: [u8; 4] = [5, 10, 11, 1];
    let udmux_token = if resp.udmux_token.is_empty() {
        None
    } else {
        Some((resp.udmux_token.as_str(), resp.udmux_pepper))
    };
    let handshake_timeout = if udmux_token.is_some() {
        Duration::from_secs(30)
    } else {
        Duration::from_secs(6)
    };
    let mut client = match RakClient::connect(
        &dial_host,
        dial_port,
        &protocols,
        handshake_timeout,
        udmux_token,
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
                match crate::raknet::netstack_probe(
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
    let appearance = crate::roblox::join::fetch_avatar_appearance(&mut anon, &appearance_url, user_id)
        .unwrap_or_default();

    let mut last_scene = Instant::now();
    let mut spawn: Option<crate::gfx::math::Vec3> = None;
    let spawn_yaw = 0.0f32;
    let mut last_pkt_time = Instant::now();

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
                    .filter(|e| {
                        !matches!(e, crate::roblox::replication::RepEvent::Unknown { .. })
                    })
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
            st.instances = st.world.len();
            let world = std::mem::take(&mut st.world);
            let count = world.build_scene(&mut st.scene);
            let _ = count;
            st.world = world;

            // Determine a spawn point: first SpawnLocation, else origin.
            if spawn.is_none() {
                if let Some(sp) = find_spawn(&st.world) {
                    spawn = Some(sp);
                }
            }
            // Render our own rig at the spawn until the replication stream
            // provides the real character.
            if st.scene.parts.len() < 4000 {
                let origin = spawn.unwrap_or([0.0, 8.0, 0.0]);
                let _ = build_r6_rig(origin, spawn_yaw, &appearance, &mut st.scene.parts);
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

static PARSER: std::sync::OnceLock<std::sync::Mutex<crate::roblox::replication::ReplicationParser>> =
    std::sync::OnceLock::new();

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

/// Camera used by the local view (free-fly).
#[derive(Clone)]
pub struct GameController {
    pub camera: Camera,
    pub pointer_locked: bool,
    pub last_mouse: (f32, f32),
    pub keys: std::collections::HashSet<winit::keyboard::PhysicalKey>,
}

impl Default for GameController {
    fn default() -> Self {
        GameController {
            camera: Camera::default(),
            pointer_locked: false,
            last_mouse: (-1.0, -1.0),
            keys: std::collections::HashSet::new(),
        }
    }
}

impl GameController {
    /// Advance the camera by dt using the currently held keys.
    pub fn step(&mut self, dt: f32) {
        use winit::keyboard::PhysicalKey;
        let speed = 28.0_f32;
        let fwd = self.camera.forward();
        let right = self.camera.right();
        let mut move_vec = [0.0f32; 3];
        let held = |k: PhysicalKey, c: &std::collections::HashSet<PhysicalKey>| c.contains(&k);
        if held(PhysicalKey::Code(winit::keyboard::KeyCode::KeyW), &self.keys) {
            move_vec = crate::gfx::math::add3(move_vec, fwd);
        }
        if held(PhysicalKey::Code(winit::keyboard::KeyCode::KeyS), &self.keys) {
            move_vec = crate::gfx::math::sub3(move_vec, fwd);
        }
        if held(PhysicalKey::Code(winit::keyboard::KeyCode::KeyD), &self.keys) {
            move_vec = crate::gfx::math::add3(move_vec, right);
        }
        if held(PhysicalKey::Code(winit::keyboard::KeyCode::KeyA), &self.keys) {
            move_vec = crate::gfx::math::sub3(move_vec, right);
        }
        if held(PhysicalKey::Code(winit::keyboard::KeyCode::Space), &self.keys) {
            move_vec[1] += 1.0;
        }
        if held(PhysicalKey::Code(winit::keyboard::KeyCode::ShiftLeft), &self.keys) {
            move_vec[1] -= 1.0;
        }
        let len = crate::gfx::math::len3(move_vec);
        if len > 1e-4 {
            let dir = crate::gfx::math::scale3(move_vec, 1.0 / len);
            let d = crate::gfx::math::scale3(dir, speed * dt);
            self.camera.pos = crate::gfx::math::add3(self.camera.pos, d);
        }
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
        self.camera.pitch = (self.camera.pitch - dy * 0.0028)
            .clamp(-1.55, 1.55);
    }
}
