//! Primary actions, searches and keyboard/list scrolling.

use crate::app::state::{Screen, StatusKind};
use crate::app::App;
use crate::menu_ui::{FriendPane, MenuTab};
use crate::theme;
use crate::ui::Hit;
use crate::worker::Command;

impl App {
	pub(in crate::app) fn primary_action(&mut self) {
		let tx = self.tx.clone();
		match &mut self.screen {
			Screen::Login(v) => {
				if v.busy {
					return;
				}
				if let Some(ts) = &v.two_step {
					let code = v.code.value.trim().to_string();
					if code.is_empty() {
						v.status = Some((StatusKind::Error, "Enter the verification code.".into()));
						return;
					}
					let (login, ticket) = (ts.login.clone(), ts.ticket.clone());
					v.busy = true;
					v.status = Some((StatusKind::Info, "Verifying\u{2026}".into()));
					let _ = tx.send(Command::Verify2FA { login, ticket, code, remember: false });
				} else {
					match v.tab {
						crate::login_ui::LoginTab::Password => {
							let login = v.username.value.trim().to_string();
							let password = v.password.value.clone();
							if login.is_empty() || password.is_empty() {
								v.status = Some((
									StatusKind::Error,
									"Enter your username and password.".into(),
								));
								return;
							}
							v.pending_login = login.clone();
							v.busy = true;
							v.status = Some((StatusKind::Info, "Signing in\u{2026}".into()));
							let _ = tx.send(Command::LoginPassword { login, password });
						}
						crate::login_ui::LoginTab::Cookie => {
							let token =
								crate::roblox::client::normalize_cookie_token(&v.cookie.value);
							if token.is_empty() {
								v.status = Some((
									StatusKind::Error,
									"Paste your .ROBLOSECURITY token first.".into(),
								));
								return;
							}
							v.busy = true;
							v.status =
								Some((StatusKind::Info, "Signing in with cookie\u{2026}".into()));
							let _ = tx.send(Command::LoginCookie { token });
						}
					}
				}
				self.needs_redraw = true;
			}
			Screen::Menu(v) => match (v.tab, v.friend_pane) {
				(MenuTab::Home, _) => self.run_place_search(),
				(MenuTab::Friends, FriendPane::Search) => self.run_user_search(),
				_ => {}
			},
			Screen::Game(_) => {}
		}
	}

	/// Kick off a user search from the Add Friends pane.
	pub(super) fn run_user_search(&mut self) {
		let query = match &self.screen {
			Screen::Menu(v) => v.add.query.value.trim().to_string(),
			_ => return,
		};
		if let Screen::Menu(v) = &mut self.screen {
			if query.chars().count() < 2 {
				v.add.error = Some("Enter at least 2 characters.".into());
				self.needs_redraw = true;
				return;
			}
			v.add.searching = true;
			v.add.error = None;
		}
		self.send_command(Command::SearchUsers { query });
		self.needs_redraw = true;
	}

	/// Kick off a place search from the Home pane (empty query = the default
	/// recommendation feed).
	pub(super) fn run_place_search(&mut self) {
		let query = match &self.screen {
			Screen::Menu(v) => v.search.value.trim().to_string(),
			_ => return,
		};
		if let Screen::Menu(v) = &mut self.screen {
			v.places_searching = true;
			v.places_error = None;
		}
		self.send_command(Command::SearchPlaces { query });
		self.needs_redraw = true;
	}

	/// Scroll the active list/strip by a step in the direction of the hit.
	pub(in crate::app) fn scroll_step(&mut self, hit: &Hit) {
		if let Screen::Menu(v) = &mut self.screen {
			let d = 1.5 * theme::ROW_H * self.scale;
			match (v.tab, v.friend_pane, v.settings.tab, hit) {
				(MenuTab::Home, _, _, Hit::ScrollUp) => {
					v.places_scroll = (v.places_scroll - d).max(0.0)
				}
				(MenuTab::Home, _, _, Hit::ScrollDown) => {
					let max = v
						.places_metrics
						.as_ref()
						.map(|m| (m.content_h - m.h).max(0.0))
						.unwrap_or(0.0);
					v.places_scroll = (v.places_scroll + d).min(max);
				}
				(MenuTab::Friends, FriendPane::Requests, _, Hit::ScrollUp) => {
					v.requests_scroll = (v.requests_scroll - d).max(0.0)
				}
				(MenuTab::Friends, FriendPane::Requests, _, Hit::ScrollDown) => {
					let max = v
						.requests_metrics
						.as_ref()
						.map(|m| (m.content_h - m.h).max(0.0))
						.unwrap_or(0.0);
					v.requests_scroll = (v.requests_scroll + d).min(max);
				}
				(MenuTab::Friends, FriendPane::Search, _, Hit::ScrollUp) => {
					v.add_scroll = (v.add_scroll - d).max(0.0)
				}
				(MenuTab::Friends, FriendPane::Search, _, Hit::ScrollDown) => {
					let max =
						v.add_metrics.as_ref().map(|m| (m.content_h - m.h).max(0.0)).unwrap_or(0.0);
					v.add_scroll = (v.add_scroll + d).min(max);
				}
				(MenuTab::Settings, _, _, Hit::ScrollUp) => {
					v.settings_scroll = (v.settings_scroll - d).max(0.0);
					v.settings_sync_logs_follow();
				}
				(MenuTab::Settings, _, _, Hit::ScrollDown) => {
					let max = v
						.settings_metrics
						.as_ref()
						.map(|m| (m.content_h - m.h).max(0.0))
						.unwrap_or(0.0);
					v.settings_scroll = (v.settings_scroll + d).min(max);
					v.settings_sync_logs_follow();
				}
				(_, _, _, Hit::StripLeft) => {
					let d = 3.0 * (theme::STRIP_CARD_W + 8.0) * self.scale;
					v.strip_scroll = (v.strip_scroll - d).max(0.0);
				}
				(_, _, _, Hit::StripRight) => {
					let d = 3.0 * (theme::STRIP_CARD_W + 8.0) * self.scale;
					let max = v
						.strip_metrics
						.as_ref()
						.map(|m| (m.content_h - m.h).max(0.0))
						.unwrap_or(0.0);
					v.strip_scroll = (v.strip_scroll + d).min(max);
				}
				_ => {}
			}
		}
		self.needs_redraw = true;
	}
}
