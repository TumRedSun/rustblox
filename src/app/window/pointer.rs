//! Pointer events: cursor motion, scrollbar drags and mouse buttons.

use crate::app::state::{GameView, Screen, ARROW_REPEAT_DELAY_MS};
use crate::app::App;
use crate::ui::Hit;
use std::time::{Duration, Instant};
use winit::dpi::PhysicalPosition;
use winit::event::ElementState;

impl App {
	/// Desktop cursor motion (mobile routes through `pointer_moved`).
	pub(super) fn on_cursor_moved(&mut self, position: PhysicalPosition<f64>) {
		if cfg!(any(target_os = "android", target_os = "ios")) {
			// Mobile: unified touch/mouse pointer path.
			self.pointer_moved(position.x as f32, position.y as f32);
		} else {
			let new_pos = (position.x as f32, position.y as f32);
			if let Screen::Game(GameView::Active { ctrl, .. }) = &mut self.screen {
				ctrl.mouse_delta(new_pos.0, new_pos.1);
				self.request_redraw();
				return;
			}
			// Same hover-change guard as `pointer_moved`: the hit map
			// still holds the previous frame's geometry.
			let prev_hit = self.hits.at(self.cursor.0, self.cursor.1);
			self.cursor = new_pos;
			// Scrollbar drag (whichever list is active).
			self.drag_scrollbar(position.y as f32);
			let dragging = self.drag_scroll.is_some()
				|| self.content_drag.is_some()
				|| self.strip_drag.is_some();
			if dragging || self.hits.at(new_pos.0, new_pos.1) != prev_hit {
				self.request_redraw();
			}
		}
	}

	/// Left-button press/release on the desktop pointer path.
	pub(super) fn on_mouse_input(&mut self, state: ElementState) {
		match state {
			ElementState::Pressed => {
				if cfg!(any(target_os = "android", target_os = "ios")) {
					// Mobile: unified touch/mouse pointer path.
					self.pointer_pressed();
					return;
				}
				if let Screen::Game(GameView::Active { ctrl, .. }) = &mut self.screen {
					if !ctrl.pointer_locked {
						self.capture_pointer();
					}
					self.needs_redraw = true;
					return;
				}
				if let Screen::Game(_) = &self.screen {
					return;
				}
				let hit = self.hits.at(self.cursor.0, self.cursor.1);
				if let Hit::Field(id) = hit {
					self.focus = Some(id);
				}
				self.begin_scrollbar_drag(&hit);
				if matches!(hit, Hit::ScrollUp | Hit::ScrollDown) {
					// First step immediately, then auto-repeat while held.
					self.scroll_step(&hit);
					self.arrow_hold = Some((
						hit.clone(),
						Instant::now() + Duration::from_millis(ARROW_REPEAT_DELAY_MS),
					));
				}
				self.mouse_down = Some(hit);
				self.needs_redraw = true;
			}
			ElementState::Released => {
				if cfg!(any(target_os = "android", target_os = "ios")) {
					// Mobile: unified touch/mouse pointer path.
					self.pointer_released();
					return;
				}
				let pressed = self.mouse_down.take();
				self.drag_scroll = None;
				self.arrow_hold = None;
				if let Some(pressed_hit) = pressed {
					let released_hit = self.hits.at(self.cursor.0, self.cursor.1);
					if pressed_hit == released_hit {
						self.activate_hit(released_hit);
					}
				}
				self.request_redraw();
			}
		}
	}
}
