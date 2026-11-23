//! Join-pipeline game events from the worker thread.

use crate::app::state::{GameView, Screen};
use crate::app::App;
use crate::game::GameEvent;

impl App {
	pub(in crate::app) fn on_game_event(&mut self, ev: GameEvent) {
		match ev {
			GameEvent::Progress(msg) => {
				if let Screen::Game(GameView::Joining { status, .. }) = &mut self.screen {
					*status = msg;
					self.request_redraw();
				}
			}
			GameEvent::Joined { resp, place_name } => {
				let user_id = match &self.screen {
					Screen::Game(GameView::Joining { user_id, .. }) => *user_id,
					_ => return, // join was cancelled from the UI
				};
				self.attach_session(resp, place_name, user_id);
			}
			GameEvent::JoinFailed(msg) => {
				crate::logs::push("app", &format!("join failed: {}", msg));
				self.leave_game(Some(format!("Could not join: {}", msg)));
			}
		}
	}
}
