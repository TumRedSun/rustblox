//! Login/logout handlers (password, 2FA, cookie).

use super::super::event::WorkerEvent;
use super::super::lock;
use super::Ctx;
use crate::roblox::auth;

impl Ctx {
	pub(super) fn login_password(&mut self, login: &str, password: &str) {
		let outcome = auth::login_password(&mut self.client, login, password);
		self.login(outcome);
	}

	pub(super) fn verify_2fa(&mut self, login: &str, ticket: &str, code: &str, remember: bool) {
		let outcome = auth::verify_2fa(&mut self.client, login, ticket, code, remember);
		self.login(outcome);
	}

	pub(super) fn resend_2fa(&mut self, login: &str, ticket: &str) {
		let ev = match auth::resend_2fa(&mut self.client, login, ticket) {
			Ok((ticket, media_type)) => WorkerEvent::TwoStepSent { ticket, media_type },
			Err(_) => {
				WorkerEvent::TwoStepSent { ticket: ticket.to_string(), media_type: String::new() }
			}
		};
		self.send(ev);
	}

	pub(super) fn login_cookie(&mut self, token: &str) {
		let outcome = auth::login_cookie(&mut self.client, token);
		self.login(outcome);
	}

	pub(super) fn logout(&mut self) {
		auth::logout(&mut self.client);
		self.me_id = None;
		lock(&self.avatars).clear();
		lock(&self.icons).clear();
		self.send(WorkerEvent::LoggedOut);
	}
}
