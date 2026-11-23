//! Application core: window/event-loop glue (winit 0.30 `ApplicationHandler`),
//! screen state machine, input routing and worker event handling.
//!
//! Split by responsibility (docs/ARCHITECTURE.md): `state` holds the screen
//! model, `handler`/`window` the event-loop glue, `input` the pointer and
//! keyboard routing, `flow` the screen transitions and button actions,
//! `redraw` the framebuffer pass, `worker` the background-event handlers.

mod flow;
mod handler;
mod input;
mod redraw;
mod state;
mod window;
mod worker;

pub use input::char_input_allowed;
pub use state::{
	game_pad_layout, GamePad, GameView, Screen, StatusKind, UiCtx, ARROW_REPEAT_DELAY_MS,
	ARROW_REPEAT_MS, BACKOFF_SECS, REFRESH_SECS, WINDOW_H, WINDOW_W,
};

use crate::config;
use crate::font::Fonts;
use crate::login_ui::LoginView;
use crate::menu_ui::MenuView;
use crate::theme;
use crate::ui::{FieldId, Hit, HitMap};
use crate::worker::Command;
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Instant;
use winit::keyboard::ModifiersState;
use winit::window::Window;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

/// Shared application state: the window, the screen being driven and the
/// input/worker plumbing around them.
pub struct App {
	pub window: Option<Arc<Window>>,
	pub gfx: Option<crate::gfx::Gfx>,
	pub fonts: Fonts,
	pub hits: HitMap,
	pub cursor: (f32, f32),
	pub mouse_down: Option<Hit>,
	pub drag_scroll: Option<(f32, f32)>, // (start cursor y, start scroll)
	/// Touch id of the finger driving the synthetic pointer (mobile).
	pub pointer_touch: Option<u64>,
	/// Per-finger role slots for the on-screen game pad (mobile).
	pub game_pad: GamePad,
	/// Touch drag over empty content: scroll the active pane 1:1 with the finger.
	pub content_drag: Option<(f32, f32)>, // (start cursor y, start scroll)
	/// Touch drag that started on the horizontal friends strip:
	/// `(start_x, start_y, start_strip_scroll, axis)` — the axis is decided
	/// once the finger moves: horizontal swipes scroll the strip, vertical
	/// swipes fall through to the page (`content_drag`).
	pub strip_drag: Option<(f32, f32, f32, Option<bool>)>,
	/// Held scroll arrow (Hit::ScrollUp/ScrollDown) with the next repeat tick.
	pub arrow_hold: Option<(Hit, Instant)>,
	pub mods: ModifiersState,
	pub focus: Option<FieldId>,
	pub screen: Screen,
	pub tx: Sender<Command>,
	pub fbw: i32,
	pub fbh: i32,
	pub scale: f32,
	pub needs_redraw: bool,
	pub next_anim: Option<Instant>,
	pub avatars: HashMap<u64, Arc<Vec<u8>>>,
	pub place_icons: HashMap<u64, Arc<Vec<u8>>>,
	#[cfg(not(any(target_os = "android", target_os = "ios")))]
	pub clipboard: Option<arboard::Clipboard>,
	/// Mobile targets have no arboard backend; paste is a no-op until the
	/// platform IME integration lands (see docs/MOBILE.md).
	#[cfg(any(target_os = "android", target_os = "ios"))]
	pub clipboard: Option<()>,
	pub start_instant: Instant,
	/// True while validating a saved session at startup.
	pub restoring: bool,
	/// Pending headless auto-join (`RUSTBLOX_AUTOJOIN`): the session restore
	/// arrives asynchronously and would otherwise replace the game screen
	/// with the menu right after startup.
	pending_autojoin: Option<(u64, u64)>,
	/// True while the login screen is open for "add another account" (the
	/// running menu is parked in `suspended_menu`).
	pub adding_account: bool,
	pub suspended_menu: Option<MenuView>,
}

impl App {
	/// Build the app on the login screen, applying the persisted theme.
	pub fn new(tx: Sender<Command>, has_saved_session: bool) -> App {
		// Apply the persisted theme before the first frame.
		let settings = config::load_settings();
		theme::apply(theme::by_id(&settings.theme, settings.custom));
		App {
			window: None,
			gfx: None,
			fonts: Fonts::load(),
			hits: HitMap::default(),
			cursor: (-1.0, -1.0),
			mouse_down: None,
			drag_scroll: None,
			pointer_touch: None,
			game_pad: GamePad::default(),
			content_drag: None,
			strip_drag: None,
			arrow_hold: None,
			mods: ModifiersState::empty(),
			focus: None,
			screen: Screen::Login(LoginView::new()),
			tx,
			fbw: 1,
			fbh: 1,
			scale: 1.0,
			needs_redraw: true,
			next_anim: None,
			avatars: HashMap::new(),
			place_icons: HashMap::new(),
			#[cfg(not(any(target_os = "android", target_os = "ios")))]
			clipboard: arboard::Clipboard::new().ok(),
			#[cfg(any(target_os = "android", target_os = "ios"))]
			clipboard: None,
			start_instant: Instant::now(),
			restoring: has_saved_session,
			pending_autojoin: None,
			adding_account: false,
			suspended_menu: None,
		}
	}

	/// Forward a command to the worker thread (dropped if it is gone).
	pub fn send_command(&mut self, cmd: Command) {
		let _ = self.tx.send(cmd);
	}

	/// Mark the window for a redraw on the next frame.
	pub fn request_redraw(&mut self) {
		self.needs_redraw = true;
	}
}
