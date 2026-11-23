//! Pointer events: cursor motion, scrollbar drags and mouse buttons.

use crate::app::state::{GameView, Screen, ARROW_REPEAT_DELAY_MS};
use crate::app::App;
use crate::menu_ui::{FriendPane, MenuTab, SettingsTab};
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
					*scroll = (start_scroll + (position.y as f32 - start_y) * k).max(0.0);
					v.settings_sync_logs_follow();
				}
			}
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
	}
}
