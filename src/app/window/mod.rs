//! Window-event dispatch; the individual input families live in siblings.

mod keys;
mod pointer;

use crate::app::state::Screen;
use crate::app::App;
use crate::menu_ui::{FriendPane, MenuTab};
use crate::theme;
use crate::worker::Command;
use winit::event::{Ime, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;

impl App {
	/// Dispatch one winit window event.
	pub(in crate::app) fn on_window_event(&mut self, elwt: &ActiveEventLoop, event: WindowEvent) {
		match event {
			WindowEvent::CloseRequested => {
				self.send_command(Command::Shutdown);
				elwt.exit();
			}
			WindowEvent::Resized(size) => {
				self.fbw = size.width.max(1) as i32;
				self.fbh = size.height.max(1) as i32;
				if let (Some(gfx), Some(w)) = (self.gfx.as_mut(), self.window.as_ref()) {
					gfx.resize(size.width.max(1), size.height.max(1));
					self.scale = w.scale_factor() as f32;
				}
				self.request_redraw();
			}
			WindowEvent::ModifiersChanged(m) => {
				self.mods = m.state();
			}
			WindowEvent::CursorMoved { position, .. } => self.on_cursor_moved(position),
			WindowEvent::CursorLeft { .. } => {
				self.cursor = (-1.0, -1.0);
				self.request_redraw();
			}
			WindowEvent::Touch(touch) => {
				self.handle_touch(touch);
			}
			WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
				self.on_mouse_input(state)
			}
			WindowEvent::MouseWheel { delta, .. } => self.on_mouse_wheel(delta),
			WindowEvent::KeyboardInput { event, .. } => self.on_key_event(event),
			WindowEvent::Ime(Ime::Commit(text)) => {
				if let Some(f) = self.focused_field_mut() {
					f.insert(&text);
					self.needs_redraw = true;
				}
			}
			WindowEvent::Ime(Ime::Enabled | Ime::Disabled) => {}
			WindowEvent::RedrawRequested => {
				self.draw_frame();
			}
			_ => {}
		}
	}

	fn on_mouse_wheel(&mut self, delta: MouseScrollDelta) {
		if let Screen::Menu(v) = &mut self.screen {
			let amount = match delta {
				MouseScrollDelta::LineDelta(_, y) => y * 3.0 * theme::ROW_H * self.scale,
				MouseScrollDelta::PixelDelta(p) => p.y as f32,
			};
			// Over the friends strip: scroll it horizontally.
			let over_strip = matches!(v.tab, MenuTab::Home)
				&& v.strip_rect.map(|r| r.contains(self.cursor.0, self.cursor.1)).unwrap_or(false);
			if over_strip {
				v.strip_scroll = (v.strip_scroll - amount).max(0.0);
			} else {
				match (v.tab, v.friend_pane, v.settings.tab) {
					(MenuTab::Home, _, _) => v.places_scroll = (v.places_scroll - amount).max(0.0),
					(MenuTab::Friends, FriendPane::Requests, _) => {
						v.requests_scroll = (v.requests_scroll - amount).max(0.0)
					}
					(MenuTab::Friends, FriendPane::Search, _) => {
						v.add_scroll = (v.add_scroll - amount).max(0.0)
					}
					(MenuTab::Settings, _, _) => {
						v.settings_scroll = (v.settings_scroll - amount).max(0.0);
						v.settings_sync_logs_follow();
					}
				}
			}
			self.request_redraw();
		}
	}
}
