//! Live game session: joins a real Roblox server, drives the replication
//! stream into the DataModel and keeps a renderable SceneView up to date.

mod controller;
mod session;
mod state;
mod thread;

pub use controller::{ControlMode, GameController};
pub use session::GameSession;
pub use state::{Aabb, GameEvent, GamePhase, GameState};

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
