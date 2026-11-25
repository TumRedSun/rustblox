//! Worker-thread state and command dispatch.
//!
//! `login.rs` holds the login/logout handlers, `social.rs` the user/friend/
//! place commands, `refresh.rs` the friends + recently-played pipelines.

mod login;
mod refresh;
mod social;

use super::event::{UserEvent, WorkerEvent};
use super::{Command, ImageJob, PushedMap};
use crate::config::Session;
use crate::game::GameEvent;
use crate::roblox::auth::{self, AuthOutcome};
use crate::roblox::client::RobloxClient;
use crate::roblox::errors::ApiError;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use winit::event_loop::EventLoopProxy;

/// Worker-thread state: one HTTP client plus everything the commands share.
pub(super) struct Ctx {
	client: RobloxClient,
	me_id: Option<u64>,
	proxy: Arc<EventLoopProxy<UserEvent>>,
	jobs: Sender<ImageJob>,
	avatars: PushedMap,
	icons: PushedMap,
}

impl Ctx {
	pub(super) fn new(
		proxy: Arc<EventLoopProxy<UserEvent>>,
		jobs: Sender<ImageJob>,
		avatars: PushedMap,
		icons: PushedMap,
	) -> Self {
		Ctx { client: RobloxClient::new(), me_id: None, proxy, jobs, avatars, icons }
	}

	/// Validate a restored session once at startup.
	pub(super) fn restore_session(&mut self, session: Option<Session>) {
		let Some(sess) = session else {
			return;
		};
		let outcome = auth::login_cookie(&mut self.client, &sess.security_token);
		self.login(outcome);
	}

	/// Serve commands until the UI drops the sender or sends `Shutdown`.
	pub(super) fn run(&mut self, rx: &Receiver<Command>) {
		while let Ok(cmd) = rx.recv() {
			if !self.handle(cmd) {
				break;
			}
		}
	}

	fn send(&self, ev: WorkerEvent) {
		let _ = self.proxy.send_event(UserEvent::Worker(ev));
	}

	/// Record the signed-in user (if any) and report the login outcome.
	fn login(&mut self, outcome: Result<AuthOutcome, ApiError>) {
		if let Ok(AuthOutcome::Session { profile, .. }) = &outcome {
			self.me_id = Some(profile.id);
		}
		self.send(WorkerEvent::Login(outcome));
	}

	fn handle(&mut self, cmd: Command) -> bool {
		match cmd {
			Command::Shutdown => return false,
			Command::LoginPassword { login, password } => self.login_password(&login, &password),
			Command::Verify2FA { login, ticket, code, remember } => {
				self.verify_2fa(&login, &ticket, &code, remember)
			}
			Command::Resend2FA { login, ticket } => self.resend_2fa(&login, &ticket),
			Command::LoginCookie { token } => self.login_cookie(&token),
			Command::Logout => self.logout(),
			Command::StartRefresh => self.refresh_all(),
			Command::SearchUsers { query } => self.search_users(query),
			Command::AddFriend { user_id } => self.add_friend(user_id),
			Command::SearchPlaces { query } => self.search_places(query),
			Command::FetchFriendRequests => self.fetch_friend_requests(),
			Command::RespondFriendRequest { user_id, accept } => {
				self.respond_friend_request(user_id, accept)
			}
			Command::JoinPlace { place_id } => self.join_place(place_id),
		}
		true
	}

	/// Run the real join flow and report progress/failure to the UI.
	fn join_place(&mut self, place_id: u64) {
		crate::logs::push("app", &format!("join requested: place {}", place_id));
		self.send(WorkerEvent::Game(GameEvent::Progress("Запрашиваем сервер…".into())));
		match crate::roblox::join::join_game(&mut self.client, place_id) {
			Ok(resp) => {
				let place_name = refresh::fetch_place_name(&mut self.client, place_id)
					.unwrap_or_else(|| format!("Place {}", place_id));
				self.send(WorkerEvent::Game(GameEvent::Joined { resp, place_name }));
			}
			Err(e) => self.send(WorkerEvent::Game(GameEvent::JoinFailed(format!("{}", e)))),
		}
	}

	/// Friends/presence refresh, no-op while signed out.
	fn refresh_friends(&mut self) {
		let Some(me_id) = self.me_id else {
			return;
		};
		let snap = refresh::run_refresh(&mut self.client, me_id, &self.jobs, &self.avatars);
		self.send(WorkerEvent::RefreshDone(snap));
	}

	/// Full refresh: friends snapshot plus the recently played row.
	fn refresh_all(&mut self) {
		if self.me_id.is_none() {
			return;
		}
		self.refresh_friends();
		// Piggyback the recently played feed on every refresh (official
		// "continue" row + local).
		let recent = refresh::collect_recently_played(&mut self.client, &self.jobs, &self.icons);
		self.send(WorkerEvent::RecentlyPlayedDone { result: recent });
	}
}
