//! Menu state: tabs, per-pane state machines and scroll metrics.

#[allow(unused_imports)]
use crate::menu_ui::*;
use crate::model::UserEntry;
use crate::ui::TextField;
use std::collections::HashMap;

/// Which main-area pane is open (driven by the sidebar).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuTab {
	Home,
	Friends,
	Settings,
}
/// Sub-pane of the Friends tab: incoming requests first, search second.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FriendPane {
	Requests,
	Search,
}
/// Sub-tab of the Settings screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsTab {
	General,
	Appearance,
	Logs,
}
pub struct ListMetrics {
	/// Vertical lists: view top edge. Horizontal strip: view left edge.
	pub top: f32,
	/// Vertical lists: view height. Horizontal strip: view width.
	pub h: f32,
	pub content_h: f32,
}
impl Clone for ListMetrics {
	fn clone(&self) -> Self {
		ListMetrics { top: self.top, h: self.h, content_h: self.content_h }
	}
}
/// Clamp a scroll offset into `[0, content - view]`.
pub fn clamp_scroll(scroll: f32, content_h: f32, view_h: f32) -> f32 {
	scroll.max(0.0).min((content_h - view_h).max(0.0))
}
/// Delivery state of a friend request for a search-result row.
#[derive(Clone, Debug, PartialEq)]
pub enum RequestState {
	Idle,
	Pending,
	Sent,
	Failed(String),
}
/// State of the "Find friends" pane (search + request sending).
pub struct AddFriendsState {
	pub query: TextField,
	pub results: Vec<UserEntry>,
	pub searching: bool,
	pub searched_once: bool,
	pub error: Option<String>,
	pub pending: Vec<u64>,
	pub sent: Vec<u64>,
	pub failed: HashMap<u64, String>,
}
impl AddFriendsState {
	pub(crate) fn new() -> Self {
		AddFriendsState {
			query: TextField::new("Search users by username"),
			results: Vec::new(),
			searching: false,
			searched_once: false,
			error: None,
			pending: Vec::new(),
			sent: Vec::new(),
			failed: HashMap::new(),
		}
	}

	pub fn mark_pending(&mut self, id: u64) {
		if !self.pending.contains(&id) {
			self.pending.push(id);
		}
	}

	pub fn state_of(&self, id: u64) -> RequestState {
		if self.pending.contains(&id) {
			return RequestState::Pending;
		}
		if self.sent.contains(&id) {
			return RequestState::Sent;
		}
		match self.failed.get(&id) {
			Some(msg) => RequestState::Failed(msg.clone()),
			None => RequestState::Idle,
		}
	}
}
/// State of the incoming friend requests pane.
pub struct RequestsState {
	pub items: Vec<UserEntry>,
	pub loading: bool,
	pub loaded_once: bool,
	pub error: Option<String>,
	/// Requests with an accept/decline call in flight.
	pub acting: Vec<u64>,
	pub failed: HashMap<u64, String>,
}
impl RequestsState {
	pub(crate) fn new() -> Self {
		RequestsState {
			items: Vec::new(),
			loading: false,
			loaded_once: false,
			error: None,
			acting: Vec::new(),
			failed: HashMap::new(),
		}
	}
}
/// Settings screen state (sub-tab selection; the values themselves live in
/// `app_settings` and are persisted immediately on every change).
#[derive(Clone, Copy, Debug)]
pub struct SettingsState {
	pub tab: SettingsTab,
	/// Logs pane: keep pinned to the newest line while the user is at the
	/// bottom (set false as soon as they scroll up).
	pub logs_follow: bool,
}
