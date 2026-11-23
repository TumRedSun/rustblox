//! Shared pointer routing: single-finger emulation, drag scrolling, clicks.
//!
//! The desktop mouse and the touch pointer both go through these methods, so
//! hit-testing and drag behaviour cannot drift between the two paths.

mod fields;
mod keys;
mod touch;

pub use keys::char_input_allowed;

use crate::app::state::{GameView, Screen, ARROW_REPEAT_DELAY_MS};
use crate::app::App;
use crate::menu_ui::{FriendPane, MenuTab, SettingsTab};
use crate::ui::Hit;
use std::time::{Duration, Instant};
use winit::event::{Touch, TouchPhase};

impl App {
	pub(in crate::app) fn handle_touch(&mut self, touch: Touch) {
		// The game screen uses its own multi-touch pad (joystick / look /
		// buttons); the single-finger pointer emulation below is UI-only.
		if matches!(&self.screen, Screen::Game(GameView::Active { .. })) {
			self.game_touch(touch);
			return;
		}
		// Single-finger pointer emulation: the first finger down drives the
		// synthetic pointer; other fingers are ignored while it is active.
		match touch.phase {
			TouchPhase::Started => {
				if self.pointer_touch.is_some() {
					return;
				}
				self.pointer_touch = Some(touch.id);
				self.pointer_moved(touch.location.x as f32, touch.location.y as f32);
				self.pointer_pressed();
			}
			TouchPhase::Moved => {
				if self.pointer_touch != Some(touch.id) {
					return;
				}
				self.pointer_moved(touch.location.x as f32, touch.location.y as f32);
			}
			TouchPhase::Ended | TouchPhase::Cancelled => {
				if self.pointer_touch != Some(touch.id) {
					return;
				}
				self.pointer_touch = None;
				self.pointer_moved(touch.location.x as f32, touch.location.y as f32);
				self.pointer_released();
			}
		}
	}

	pub(in crate::app) fn pointer_moved(&mut self, x: f32, y: f32) {
		// Hover-change detection: `hits` still reflects the previous frame,
		// which is exactly the "what was under the pointer before" side of
		// the comparison below.
		let prev_hit = self.hits.at(self.cursor.0, self.cursor.1);
		self.cursor = (x, y);
		// Scrollbar drag (whichever list is active).
		if let (Some((start_y, start_scroll)), true) = (
			self.drag_scroll,
			self.mouse_down.as_ref().map(|h| matches!(h, Hit::Scrollbar)).unwrap_or(false),
		) {
			if let Screen::Menu(v) = &mut self.screen {
				let (scroll, k) = match (v.tab, v.friend_pane, v.settings.tab) {
					(MenuTab::Home, _, _) => (
						&mut v.places_scroll,
						v.places_metrics
							.as_ref()
							.map(|m| (m.content_h / m.h.max(1.0)).max(1.0))
							.unwrap_or(1.0),
					),
					(MenuTab::Friends, FriendPane::Requests, _) => (
						&mut v.requests_scroll,
						v.requests_metrics
							.as_ref()
							.map(|m| (m.content_h / m.h.max(1.0)).max(1.0))
							.unwrap_or(1.0),
					),
					(MenuTab::Friends, FriendPane::Search, _) => (
						&mut v.add_scroll,
						v.add_metrics
							.as_ref()
							.map(|m| (m.content_h / m.h.max(1.0)).max(1.0))
							.unwrap_or(1.0),
					),
					(MenuTab::Settings, _, SettingsTab::General)
					| (MenuTab::Settings, _, SettingsTab::Appearance)
					| (MenuTab::Settings, _, SettingsTab::Logs) => (
						&mut v.settings_scroll,
						v.settings_metrics
							.as_ref()
							.map(|m| (m.content_h / m.h.max(1.0)).max(1.0))
							.unwrap_or(1.0),
					),
				};
				*scroll = (start_scroll + (y - start_y) * k).max(0.0);
				v.settings_sync_logs_follow();
			}
		}
		// Touch: content drag scrolls the active pane 1:1 with the finger
		// (natural scrolling: finger down moves the content down).
		if let Some((start_y, start_scroll)) = self.content_drag {
			if let Screen::Menu(v) = &mut self.screen {
				let scroll = match (v.tab, v.friend_pane, v.settings.tab) {
					(MenuTab::Home, _, _) => &mut v.places_scroll,
					(MenuTab::Friends, FriendPane::Requests, _) => &mut v.requests_scroll,
					(MenuTab::Friends, FriendPane::Search, _) => &mut v.add_scroll,
					(MenuTab::Settings, _, _) => &mut v.settings_scroll,
				};
				*scroll = (start_scroll - (y - start_y)).max(0.0);
				v.settings_sync_logs_follow();
			}
		}
		// Touch drag on the friends strip: resolve the gesture axis once,
		// then either swipe the strip horizontally or hand the gesture over
		// to the vertical page scroll above.
		if let Some((sx, sy, s0, axis)) = self.strip_drag {
			if let Screen::Menu(v) = &mut self.screen {
				if matches!(v.tab, MenuTab::Home) {
					let dx = x - sx;
					let dy = y - sy;
					let resolved = match axis {
						Some(a) => Some(a),
						None => {
							let th = 8.0 * self.scale;
							if dx.abs() > th || dy.abs() > th {
								Some(dx.abs() >= dy.abs())
							} else {
								None
							}
						}
					};
					self.strip_drag = Some((sx, sy, s0, resolved));
					match resolved {
						Some(true) => {
							v.strip_scroll = (s0 - dx).max(0.0);
							// Horizontal swipe wins: stop the page scroll.
							self.content_drag = None;
						}
						Some(false) => {
							// Vertical swipe: the page scroll takes over.
							self.strip_drag = None;
						}
						None => {}
					}
				} else {
					self.strip_drag = None;
				}
			}
		}
		// Raw pointer motion fires hundreds of events per second; a full
		// redraw per event burned ~20% CPU while the cursor was moving and
		// 0% the moment it stopped. Redraw only when the hover target actually
		// changed (highlight in/out of a widget) or when a drag/gesture is in
		// flight — those genuinely move content every event.
		let dragging =
			self.drag_scroll.is_some() || self.content_drag.is_some() || self.strip_drag.is_some();
		if dragging || self.hits.at(x, y) != prev_hit {
			self.request_redraw();
		}
	}

	pub(in crate::app) fn pointer_pressed(&mut self) {
		let hit = self.hits.at(self.cursor.0, self.cursor.1);
		if let Hit::Field(id) = hit {
			self.focus = Some(id);
			// Reflect the new field's text into the platform IME and show the
			// soft keyboard on touch devices.
			let text = self.focused_field_mut().map(|f| f.value.clone()).unwrap_or_default();
			crate::platform::ime_push(&text);
			if cfg!(any(target_os = "android", target_os = "ios")) {
				if let Some(w) = &self.window {
					w.set_ime_allowed(true);
				}
			}
		} else if cfg!(any(target_os = "android", target_os = "ios")) {
			// Tapping outside a field dismisses the soft keyboard.
			if self.focus.take().is_some() {
				if let Some(w) = &self.window {
					w.set_ime_allowed(false);
				}
			}
		}
		if let Hit::Scrollbar = hit {
			if let Screen::Menu(v) = &mut self.screen {
				let start = match (v.tab, v.friend_pane, v.settings.tab) {
					(MenuTab::Home, _, _) => v.places_scroll,
					(MenuTab::Friends, FriendPane::Requests, _) => v.requests_scroll,
					(MenuTab::Friends, FriendPane::Search, _) => v.add_scroll,
					(MenuTab::Settings, _, _) => v.settings_scroll,
				};
				self.drag_scroll = Some((self.cursor.1, start));
			}
		}
		if matches!(hit, Hit::ScrollUp | Hit::ScrollDown) {
			// First step immediately, then auto-repeat while held.
			self.scroll_step(&hit);
			self.arrow_hold =
				Some((hit.clone(), Instant::now() + Duration::from_millis(ARROW_REPEAT_DELAY_MS)));
		}
		// Touch: dragging empty content scrolls the active pane.
		if self.pointer_touch.is_some() && matches!(hit, Hit::None) {
			if let Screen::Menu(v) = &mut self.screen {
				let start = match (v.tab, v.friend_pane, v.settings.tab) {
					(MenuTab::Home, _, _) => v.places_scroll,
					(MenuTab::Friends, FriendPane::Requests, _) => v.requests_scroll,
					(MenuTab::Friends, FriendPane::Search, _) => v.add_scroll,
					(MenuTab::Settings, _, _) => v.settings_scroll,
				};
				self.content_drag = Some((self.cursor.1, start));
			}
		}
		// Touch: a drag starting on the friends strip may become either a
		// horizontal strip swipe or a vertical page scroll (axis resolved on
		// first movement in `pointer_moved`). Taps on the strip arrows stay
		// plain button clicks.
		if self.pointer_touch.is_some() && self.strip_drag.is_none() {
			if let Screen::Menu(v) = &mut self.screen {
				if matches!(v.tab, MenuTab::Home) {
					if let Some(r) = v.strip_rect {
						if r.contains(self.cursor.0, self.cursor.1)
							&& !matches!(hit, Hit::StripLeft | Hit::StripRight)
						{
							self.strip_drag =
								Some((self.cursor.0, self.cursor.1, v.strip_scroll, None));
						}
					}
				}
			}
		}
		self.mouse_down = Some(hit);
		self.needs_redraw = true;
	}

	pub(in crate::app) fn pointer_released(&mut self) {
		self.content_drag = None;
		self.strip_drag = None;
		let pressed = self.mouse_down.take();
		self.drag_scroll = None;
		self.arrow_hold = None;
		if let Some(pressed_hit) = pressed {
			let released_hit = self.hits.at(self.cursor.0, self.cursor.1);
			if pressed_hit == released_hit {
				match released_hit {
					Hit::TabPassword
					| Hit::TabCookie
					| Hit::SignIn
					| Hit::Verify
					| Hit::Resend
					| Hit::Back
					| Hit::Logout
					| Hit::NavFriends
					| Hit::NavSettings
					| Hit::AddSearchBtn
					| Hit::AddUser(_)
					| Hit::SearchPlacesBtn
					| Hit::SidebarToggle
					| Hit::Logo
					| Hit::TabRequests
					| Hit::TabSearch
					| Hit::RequestAccept(_)
					| Hit::RequestDecline(_)
					| Hit::SwitchAccount(_)
					| Hit::AddAccount
					| Hit::ThemePreset(_)
					| Hit::ToggleRecommendations
					| Hit::SettingsTab(_)
					| Hit::RadiusUp
					| Hit::RadiusDown
					| Hit::ColorPrev(_)
					| Hit::ColorNext(_)
					| Hit::ResetAppearance
					| Hit::CancelAdd
					| Hit::ClearField(_)
					| Hit::CopyLogs
					| Hit::SaveLogs
					| Hit::ClearLogs
					| Hit::StripLeft
					| Hit::StripRight
					| Hit::JoinPlace(_) => {
						self.handle_button(released_hit);
					}
					Hit::Field(id) => {
						self.focus = Some(id);
					}
					_ => {}
				}
			}
		}
		self.request_redraw();
	}
}
