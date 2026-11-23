//! User search, friend requests and place search command handlers.

use super::super::event::{ImageKind, WorkerEvent};
use super::refresh::push_place_icons;
use super::Ctx;
use crate::roblox::social;

impl Ctx {
	pub(super) fn search_users(&mut self, query: String) {
		let result = social::search_users(&mut self.client, &query, 25);
		if let Ok(users) = &result {
			// Resolve headshots for the results so the rows get avatars via
			// the download pool.
			let ids: Vec<u64> = users.iter().map(|u| u.id).collect();
			self.push_user_avatars(&ids);
		}
		self.send(WorkerEvent::SearchDone { query, result });
	}

	pub(super) fn add_friend(&mut self, user_id: u64) {
		let result = social::send_friend_request(&mut self.client, user_id);
		self.send(WorkerEvent::FriendRequestDone { user_id, result });
	}

	pub(super) fn search_places(&mut self, query: String) {
		// Empty query -> default recommendation feed.
		let result = if query.trim().is_empty() {
			social::recommended_games(&mut self.client, 18)
		} else {
			social::search_games(&mut self.client, &query, 18)
		};
		if let Ok(cards) = &result {
			push_place_icons(&mut self.client, cards, &self.jobs, &self.icons);
		}
		self.send(WorkerEvent::PlacesDone { query, result });
	}

	pub(super) fn fetch_friend_requests(&mut self) {
		// The endpoint targets the logged-in user ("my"), so no user id is
		// needed; unauthenticated calls fail with 401 and surface as an
		// error message.
		let result = social::fetch_friend_requests(&mut self.client);
		if let Ok(users) = &result {
			let ids: Vec<u64> = users.iter().map(|u| u.id).collect();
			self.push_user_avatars(&ids);
		}
		self.send(WorkerEvent::FriendRequestsDone { result });
	}

	pub(super) fn respond_friend_request(&mut self, user_id: u64, accept: bool) {
		let result = if accept {
			social::accept_friend_request(&mut self.client, user_id)
		} else {
			social::decline_friend_request(&mut self.client, user_id)
		};
		if result.is_ok() {
			// The friends list changed: force the next refresh to pick it up
			// immediately.
			self.refresh_friends();
		}
		self.send(WorkerEvent::RequestResponded { user_id, accept, result });
	}

	/// Resolve and enqueue headshots for user ids not pushed yet.
	pub(super) fn push_user_avatars(&mut self, ids: &[u64]) {
		if let Ok(index) = social::fetch_avatar_index(&mut self.client, ids) {
			super::refresh::enqueue_thumbs(&self.avatars, &self.jobs, ImageKind::User, index);
		}
	}
}
