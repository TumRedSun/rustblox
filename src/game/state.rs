//! Shared game state: phases, events and the snapshot the UI reads each
//! frame while the network thread fills it.

use crate::gfx::math::Vec3;
use crate::gfx::scene::SceneView;
use crate::roblox::join::JoinResponse;
use crate::world::DataModel;

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

/// Poison-tolerant state lock: after any (caught) panic the mutex stays
/// poisoned; the UI and the game thread must keep working off the surviving
/// data instead of dying on the next `.unwrap()`.
pub(super) fn lock_state(
	state: &std::sync::Mutex<GameState>,
) -> std::sync::MutexGuard<'_, GameState> {
	state.lock().unwrap_or_else(|p| p.into_inner())
}
