//! Worker-thread event dispatch (auth events, menu data events).

mod auth;
mod game;
mod menu;

use crate::app::App;
use crate::worker::WorkerEvent;

impl App {
	/// Route a background-worker event to the auth or menu handler.
	pub(in crate::app) fn on_worker_event(&mut self, ev: WorkerEvent) {
		// Join-pipeline events are dispatched by `on_game_event` already.
		if matches!(ev, WorkerEvent::Game(_)) {
			return;
		}
		let auth = matches!(
			ev,
			WorkerEvent::Login(_) | WorkerEvent::TwoStepSent { .. } | WorkerEvent::LoggedOut
		);
		if auth {
			self.on_auth_event(ev);
		} else {
			self.on_menu_event(ev);
		}
	}
}
