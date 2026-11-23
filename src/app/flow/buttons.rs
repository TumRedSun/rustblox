//! Button hit dispatch for the login and menu screens.
//!
//! Settings/appearance/logs arms are delegated to `settings.rs`.

use crate::app::state::{Screen, StatusKind, REFRESH_SECS};
use crate::app::App;
use crate::config;
use crate::login_ui::LoginView;
use crate::menu_ui::{FriendPane, MenuTab, MenuView};
use crate::ui::{FieldId, Hit};
use crate::worker::Command;
use std::time::{Duration, Instant};

impl App {
	pub(in crate::app) fn handle_button(&mut self, hit: Hit) {
		match hit {
			Hit::TabPassword | Hit::TabCookie => {
				if let Screen::Login(v) = &mut self.screen {
					v.tab = if hit == Hit::TabPassword {
						crate::login_ui::LoginTab::Password
					} else {
						crate::login_ui::LoginTab::Cookie
					};
					v.status = None;
					self.focus = None;
				}
				self.request_redraw();
			}
			Hit::SignIn | Hit::Verify => {
				self.primary_action();
			}
			Hit::Resend => {
				let tx = self.tx.clone();
				if let Screen::Login(v) = &mut self.screen {
					if let Some(ts) = &v.two_step {
						let (login, ticket) = (ts.login.clone(), ts.ticket.clone());
						let _ = tx.send(Command::Resend2FA { login, ticket });
						v.status = Some((StatusKind::Info, "Resending…".into()));
					}
				}
				self.needs_redraw = true;
			}
			Hit::Back => {
				if let Screen::Login(v) = &mut self.screen {
					v.two_step = None;
					v.status = None;
					self.focus = Some(FieldId::Password);
				}
				self.needs_redraw = true;
			}
			Hit::NavFriends => {
				if let Screen::Menu(v) = &mut self.screen {
					v.tab = MenuTab::Friends;
					self.focus = None;
					// Fresh list of incoming requests on every visit.
					v.requests.loading = true;
					v.requests.error = None;
				}
				self.send_command(Command::FetchFriendRequests);
				self.needs_redraw = true;
			}
			Hit::NavSettings => {
				if let Screen::Menu(v) = &mut self.screen {
					v.tab = MenuTab::Settings;
					self.focus = None;
				}
				self.needs_redraw = true;
			}
			Hit::Logo => {
				// The wordmark is Home: refresh + jump to the top.
				if let Screen::Menu(v) = &mut self.screen {
					v.tab = MenuTab::Home;
					v.places_scroll = 0.0;
					v.strip_scroll = 0.0;
					if !v.refreshing {
						v.refreshing = true;
						v.next_refresh = Some(Instant::now() + Duration::from_secs(REFRESH_SECS));
						self.send_command(Command::StartRefresh);
					}
				}
				self.focus = None;
				self.needs_redraw = true;
			}
			Hit::CancelAdd => {
				if let Some(m) = self.suspended_menu.take() {
					self.screen = Screen::Menu(m);
				}
				self.adding_account = false;
				self.focus = None;
				self.needs_redraw = true;
			}
			Hit::TabRequests => {
				if let Screen::Menu(v) = &mut self.screen {
					v.friend_pane = FriendPane::Requests;
					self.focus = None;
				}
				self.needs_redraw = true;
			}
			Hit::TabSearch => {
				if let Screen::Menu(v) = &mut self.screen {
					v.friend_pane = FriendPane::Search;
					self.focus = Some(FieldId::AddSearch);
				}
				self.needs_redraw = true;
			}
			Hit::RequestAccept(user_id) | Hit::RequestDecline(user_id) => {
				let accept = matches!(hit, Hit::RequestAccept(_));
				if let Screen::Menu(v) = &mut self.screen {
					if !v.requests.acting.contains(&user_id) {
						v.requests.acting.push(user_id);
						v.requests.failed.remove(&user_id);
						self.send_command(Command::RespondFriendRequest { user_id, accept });
					}
				}
				self.needs_redraw = true;
			}
			Hit::SwitchAccount(user_id) => {
				let account = config::load_accounts().into_iter().find(|a| a.user_id == user_id);
				if let Some(acc) = account {
					if let Screen::Menu(v) = &mut self.screen {
						v.notice = Some((
							"Switching account…".into(),
							Instant::now() + Duration::from_secs(4),
						));
					}
					self.send_command(Command::LoginCookie { token: acc.security_token });
				}
				self.needs_redraw = true;
			}
			Hit::AddAccount => {
				if let Screen::Menu(v) = &mut self.screen {
					let menu = std::mem::replace(v, MenuView::new(v.profile.clone()));
					self.suspended_menu = Some(menu);
				}
				self.adding_account = true;
				self.screen = Screen::Login(LoginView::new());
				self.focus = None;
				self.needs_redraw = true;
			}
			Hit::ThemePreset(_)
			| Hit::ToggleRecommendations
			| Hit::SettingsTab(_)
			| Hit::RadiusUp
			| Hit::RadiusDown
			| Hit::ColorPrev(_)
			| Hit::ColorNext(_)
			| Hit::ResetAppearance => self.settings_hit(&hit),
			Hit::CopyLogs | Hit::SaveLogs | Hit::ClearLogs => self.logs_hit(&hit),
			Hit::ClearField(id) => {
				if let Screen::Menu(v) = &mut self.screen {
					match id {
						FieldId::Search => {
							v.search.clear();
							// Restore the default feed (hides search output).
							self.run_place_search();
						}
						FieldId::AddSearch => {
							v.add.query.clear();
							v.add.results.clear();
							v.add.searched_once = false;
							v.add.error = None;
							v.add_scroll = 0.0;
						}
						_ => {}
					}
				}
				if let Screen::Login(v) = &mut self.screen {
					match id {
						FieldId::Username => v.username.clear(),
						FieldId::Password => v.password.clear(),
						FieldId::Cookie => v.cookie.clear(),
						FieldId::Code => v.code.clear(),
						_ => {}
					}
				}
				self.needs_redraw = true;
			}
			Hit::AddSearchBtn => {
				self.run_user_search();
			}
			Hit::SearchPlacesBtn => {
				self.run_place_search();
			}
			Hit::SidebarToggle => {
				if let Screen::Menu(v) = &mut self.screen {
					v.collapsed = !v.collapsed;
				}
				self.needs_redraw = true;
			}
			Hit::StripLeft | Hit::StripRight => {
				self.scroll_step(&hit);
			}
			Hit::JoinPlace(place_id) => {
				let user_id = match &self.screen {
					Screen::Menu(v) => v.profile.id,
					_ => 0,
				};
				self.enter_game_joining(place_id, user_id);
			}
			Hit::AddUser(user_id) => {
				if let Screen::Menu(v) = &mut self.screen {
					let already = v.add.pending.contains(&user_id) || v.add.sent.contains(&user_id);
					if !already {
						v.add.mark_pending(user_id);
						self.send_command(Command::AddFriend { user_id });
					}
				}
				self.needs_redraw = true;
			}
			Hit::Logout => {
				self.send_command(Command::Logout);
			}
			_ => {}
		}
	}
}
