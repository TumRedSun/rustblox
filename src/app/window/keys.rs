//! Keyboard input on the game screen and the UI screens.

use crate::app::state::{GameView, Screen};
use crate::app::App;
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};

impl App {
	pub(super) fn on_key_event(&mut self, event: KeyEvent) {
		// Game screen consumes keys first (movement + Esc).
		if let Screen::Game(GameView::Active { ctrl, .. }) = &mut self.screen {
			let mut action = 0u8; // 1 = release mouse, 2 = leave
			if event.state == ElementState::Pressed {
				ctrl.keys.insert(event.physical_key);
				if matches!(event.logical_key, Key::Named(NamedKey::Escape)) {
					// Roblox-like: the first Esc releases the captured
					// mouse, a second one leaves the experience.
					action = if ctrl.pointer_locked { 1 } else { 2 };
				}
				if let PhysicalKey::Code(code) = event.physical_key {
					match code {
						KeyCode::Space => ctrl.jump = true,
						KeyCode::KeyF => ctrl.toggle_fly(),
						KeyCode::KeyR => ctrl.respawn = true,
						_ => {}
					}
				}
			} else {
				ctrl.keys.remove(&event.physical_key);
			}
			if action == 1 {
				ctrl.pointer_locked = false;
			}
			match action {
				1 => self.release_pointer(),
				2 => self.leave_game(None),
				_ => {}
			}
			return;
		}
		if let Screen::Game(GameView::Joining { .. }) = &self.screen {
			if event.state == ElementState::Pressed
				&& matches!(event.logical_key, Key::Named(NamedKey::Escape))
			{
				self.leave_game(None);
			}
			return;
		}
		if event.state == ElementState::Pressed {
			self.on_key(event);
		}
	}
}
