//! Screen transitions: login/menu/game switching and pointer capture.

mod actions;
mod buttons;
mod settings;

use crate::app::state::{GameView, Screen, StatusKind, REFRESH_SECS};
use crate::app::App;
use crate::game::{GameController, GameSession};
use crate::login_ui::LoginView;
use crate::menu_ui::MenuView;
use crate::model::UserProfile;
use crate::worker::Command;
use std::time::{Duration, Instant};

impl App {
	pub(in crate::app) fn enter_menu(&mut self, profile: UserProfile) {
		self.screen = Screen::Menu(MenuView::new(profile));
		self.focus = None;
		self.restoring = false;
		self.adding_account = false;
		self.suspended_menu = None;
		// Kick off the first refresh and the default places feed immediately.
		if let Screen::Menu(v) = &mut self.screen {
			v.refreshing = true;
			v.next_refresh = Some(Instant::now() + Duration::from_secs(REFRESH_SECS));
		}
		self.send_command(Command::StartRefresh);
		self.send_command(Command::SearchPlaces { query: String::new() });
		self.request_redraw();
		// Headless capture: start the join the caller asked for at startup,
		// now that the restored session owns the screen.
		if let Some((place, user_id)) = self.pending_autojoin.take() {
			self.enter_game_joining(place, user_id);
		}
	}

	pub(in crate::app) fn enter_login(&mut self, status: Option<(StatusKind, String)>) {
		self.screen = Screen::Login(LoginView::with_status(status));
		self.focus = None;
		self.restoring = false;
		self.request_redraw();
	}

	/// Start joining a place (click on a place card). The running menu is
	/// parked so Esc/leave restores it.
	pub(in crate::app) fn enter_game_joining(&mut self, place_id: u64, user_id: u64) {
		let prev = std::mem::replace(
			&mut self.screen,
			Screen::Game(GameView::Joining {
				place_id,
				user_id,
				status: "Requesting a server…".into(),
			}),
		);
		if let Screen::Menu(m) = prev {
			self.suspended_menu = Some(m);
		}
		self.focus = None;
		self.send_command(Command::JoinPlace { place_id });
		self.request_redraw();
	}

	/// Start a join without any UI input — used by the headless capture
	/// (`RUSTBLOX_AUTOJOIN=<placeId>`) so a live in-game frame can be
	/// screenshotted in CI or by a reviewer.
	pub fn auto_join(&mut self, place_id: u64, user_id: u64) {
		self.pending_autojoin = Some((place_id, user_id));
		self.enter_game_joining(place_id, user_id);
	}

	/// Attach a successfully joined session (from GameEvent::Joined).
	pub(in crate::app) fn attach_session(
		&mut self,
		resp: crate::roblox::join::JoinResponse,
		place_name: String,
		user_id: u64,
	) {
		let session = GameSession::start(resp, place_name.clone(), user_id);
		self.screen = Screen::Game(GameView::Active {
			session,
			ctrl: GameController::default(),
			place_name,
			last_frame: Instant::now(),
			fps: 60.0,
		});
		self.focus = None;
		self.request_redraw();
	}

	/// Leave the game and restore the parked menu (Esc / connection lost).
	pub(in crate::app) fn leave_game(&mut self, notice: Option<String>) {
		if let Screen::Game(GameView::Active { session, .. }) = &self.screen {
			session.stop();
		}
		self.release_pointer();
		self.screen = match self.suspended_menu.take() {
			Some(m) => Screen::Menu(m),
			None => {
				self.enter_login(None);
				return;
			}
		};
		if let Some(n) = notice {
			if let Screen::Menu(v) = &mut self.screen {
				v.notice = Some((n, Instant::now() + Duration::from_secs(5)));
			}
		}
		self.request_redraw();
	}

	pub(in crate::app) fn release_pointer(&self) {
		if let Some(w) = self.window.as_ref() {
			let _ = w.set_cursor_grab(winit::window::CursorGrabMode::None);
			w.set_cursor_visible(true);
		}
	}

	pub(in crate::app) fn capture_pointer(&mut self) {
		let Some(w) = self.window.as_ref() else { return };
		let grabbed = w
			.set_cursor_grab(winit::window::CursorGrabMode::Locked)
			.or_else(|_| w.set_cursor_grab(winit::window::CursorGrabMode::Confined));
		if grabbed.is_ok() {
			w.set_cursor_visible(false);
			if let Screen::Game(GameView::Active { ctrl, .. }) = &mut self.screen {
				ctrl.pointer_locked = true;
			}
		}
	}
}
