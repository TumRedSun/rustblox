//! Authentication and session worker events.

use crate::app::state::{Screen, StatusKind};
use crate::app::App;
use crate::config::{self, Session};
use crate::roblox::auth::AuthOutcome;
use crate::roblox::errors::ApiError;
use crate::ui::FieldId;
use crate::worker::WorkerEvent;
use std::time::{Duration, Instant};

impl App {
	pub(super) fn on_auth_event(&mut self, ev: WorkerEvent) {
		match ev {
			WorkerEvent::Login(Ok(outcome)) => match outcome {
				AuthOutcome::Session { security, profile } => {
					let sess = Session {
						security_token: security,
						user_id: profile.id,
						username: profile.name.clone(),
						display_name: profile.display_name.clone(),
					};
					if let Err(e) = config::save_session(&sess) {
						eprintln!("warn: could not persist session: {e}");
					}
					// Remember the account for the quick switcher.
					config::upsert_account(&sess);
					self.enter_menu(profile);
				}
				AuthOutcome::TwoStep { ticket, media_type } => {
					if let Screen::Login(v) = &mut self.screen {
						v.busy = false;
						v.two_step = Some(crate::login_ui::TwoStepPending {
							login: v.pending_login.clone(),
							ticket,
							media_type,
						});
						v.code.clear();
						v.status = Some((StatusKind::Info, "Enter the verification code.".into()));
						self.focus = Some(FieldId::Code);
					}
					self.request_redraw();
				}
			},
			WorkerEvent::Login(Err(e)) => {
				let msg = e.to_string();
				if self.restoring {
					config::clear_session();
					self.enter_login(Some((
						StatusKind::Error,
						format!("Saved session expired. {msg}"),
					)));
				} else if let Screen::Login(v) = &mut self.screen {
					v.busy = false;
					if matches!(e, ApiError::TwoStepRequired { .. }) {
						// Should not normally happen; handled via AuthOutcome.
					}
					v.status = Some((StatusKind::Error, msg));
				} else if let Screen::Menu(v) = &mut self.screen {
					// A quick account switch failed; stay on the current one.
					let mut m = format!("Could not switch account: {}", msg);
					m.truncate(120);
					v.notice = Some((m, Instant::now() + Duration::from_secs(5)));
				}
				self.request_redraw();
			}
			WorkerEvent::TwoStepSent { ticket, media_type } => {
				if let Screen::Login(v) = &mut self.screen {
					if ticket.is_empty() {
						v.status = Some((StatusKind::Error, "Could not resend the code.".into()));
					} else {
						if let Some(ts) = &mut v.two_step {
							ts.ticket = ticket;
							if !media_type.is_empty() {
								ts.media_type = media_type;
							}
						}
						v.status = Some((StatusKind::Info, "A new code was sent.".into()));
					}
				}
				self.request_redraw();
			}
			WorkerEvent::LoggedOut => {
				// Remove the signed-out account from the switcher.
				if let Screen::Menu(v) = &self.screen {
					config::remove_account(v.profile.id);
				} else if let Some(m) = &self.suspended_menu {
					config::remove_account(m.profile.id);
				}
				config::clear_session();
				self.adding_account = false;
				self.suspended_menu = None;
				self.enter_login(None);
			}
			_ => {}
		}
	}
}
