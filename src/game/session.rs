//! Game session handle: owns the shared state and the background thread that
//! dials the server and pumps replication into it.

use super::state::{lock_state, GamePhase, GameState};
use super::thread::game_thread;
use crate::netlog;
use crate::roblox::join::JoinResponse;
use crate::world::DataModel;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

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
			scene: crate::gfx::scene::SceneView::default(),
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
					let mut g = lock_state(&st_after);
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
