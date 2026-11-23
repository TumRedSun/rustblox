//! Screen model and the draw/input context shared by the screen modules.

use crate::font::Fonts;
use crate::game::{GameController, GameSession};
use crate::login_ui::LoginView;
use crate::menu_ui::MenuView;
use crate::ui::{FieldId, HitMap};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

pub const WINDOW_W: f32 = 1100.0;
pub const WINDOW_H: f32 = 740.0;
/// Presence auto-refresh interval (no manual refresh button — automatic only).
pub const REFRESH_SECS: u64 = 10;
pub const BACKOFF_SECS: u64 = 30;
/// Delay before the first auto-repeat tick of a held scroll arrow.
pub const ARROW_REPEAT_DELAY_MS: u64 = 250;
/// Auto-repeat interval of a held scroll arrow.
pub const ARROW_REPEAT_MS: u64 = 110;

pub enum Screen {
	Login(LoginView),
	Menu(MenuView),
	Game(GameView),
}

/// The live game screen: joining (HTTP pipeline) or connected (UDP session).
pub enum GameView {
	Joining {
		place_id: u64,
		user_id: u64,
		status: String,
	},
	Active {
		session: GameSession,
		ctrl: GameController,
		place_name: String,
		last_frame: Instant,
		fps: f32,
	},
}

#[derive(Clone, Copy, PartialEq)]
pub enum StatusKind {
	Info,
	Error,
	Success,
}

/// Draw/input context handed to screen modules each frame.
pub struct UiCtx<'a> {
	pub fonts: &'a Fonts,
	pub cursor: (f32, f32),
	pub hits: &'a mut HitMap,
	pub focus: Option<FieldId>,
	pub scale: f32,
	pub now_ms: u64,
	pub avatars: &'a mut HashMap<u64, Arc<Vec<u8>>>,
	/// Decoded square game icons keyed by root place id.
	pub icons: &'a mut HashMap<u64, Arc<Vec<u8>>>,
}

/// On-screen touch controls for the game screen (mobile). The UI pointer
/// path is single-finger; flying needs MULTIPLE fingers at once (steer with
/// the left thumb while looking with the right), so game touches are routed
/// by touch id into per-role slots instead of the synthetic pointer.
#[derive(Default)]
pub struct GamePad {
	/// Joystick finger: (touch id, origin x, origin y).
	pub stick: Option<(u64, f32, f32)>,
	/// Current joystick deflection: (strafe, fwd), each -1..=1 (screen y is
	/// inverted: dragging up = fly forward).
	pub stick_vec: (f32, f32),
	/// Camera-look finger: (touch id, last x, last y).
	pub look: Option<(u64, f32, f32)>,
	/// Finger holding the + (up) / - (down) button.
	pub up: Option<u64>,
	pub down: Option<u64>,
}

/// Geometry of the on-screen game pad for a (w, h) window at `s` scale:
/// (stick center, stick radius, up rect, down rect, leave rect).
/// The same layout is used for hit-testing (`game_touch`) and drawing
/// (`draw_game_hud`) so the two can never drift apart.
pub fn game_pad_layout(
	w: f32,
	h: f32,
	s: f32,
) -> ((f32, f32), f32, (i32, i32, i32, i32), (i32, i32, i32, i32), (i32, i32, i32, i32)) {
	let r = 96.0 * s;
	let m = 40.0 * s;
	let stick_center = (r + m, h - r - m);
	let bw = 96.0 * s;
	let bh = 76.0 * s;
	let gap = 18.0 * s;
	let up = ((w - bw - m) as i32, (h - bh * 2.0 - gap - m) as i32, bw as i32, bh as i32);
	let down = ((w - bw - m) as i32, (h - bh - m) as i32, bw as i32, bh as i32);
	let lw = 64.0 * s;
	let lh = 52.0 * s;
	let leave = ((w - lw - 14.0 * s) as i32, (14.0 * s) as i32, lw as i32, lh as i32);
	(stick_center, r, up, down, leave)
}
