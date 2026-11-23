//! Events the worker sends back to the UI thread.

use crate::game::GameEvent;
use crate::model::{PlaceCard, Snapshot, UserEntry};
use crate::roblox::auth::AuthOutcome;
use crate::roblox::errors::ApiError;
use std::sync::Arc;

/// Result of one worker command.
#[derive(Debug)]
pub enum WorkerEvent {
	/// Login outcome (password, cookie, 2FA or session restore).
	Login(Result<AuthOutcome, ApiError>),
	/// A 2-step code was (re)sent; carries the refreshed ticket.
	TwoStepSent {
		/// Login ticket for the Verify2FA command.
		ticket: String,
		/// Delivery channel ("Email", "Authenticator"…).
		media_type: String,
	},
	/// Friends/presence snapshot refresh finished.
	RefreshDone(Result<Snapshot, ApiError>),
	/// User search finished.
	SearchDone {
		/// The query this result belongs to.
		query: String,
		/// Matching users.
		result: Result<Vec<UserEntry>, ApiError>,
	},
	/// Friend request send finished.
	FriendRequestDone {
		/// Target user id.
		user_id: u64,
		/// Success or the API error to show.
		result: Result<(), ApiError>,
	},
	/// Place search/recommendation finished.
	PlacesDone {
		/// The query this result belongs to (empty = discovery feed).
		query: String,
		/// Place cards.
		result: Result<Vec<PlaceCard>, ApiError>,
	},
	/// Incoming friend requests loaded.
	FriendRequestsDone {
		/// Requesting users.
		result: Result<Vec<UserEntry>, ApiError>,
	},
	/// A friend request was accepted or declined.
	RequestResponded {
		/// Requesting user id.
		user_id: u64,
		/// True when accepted.
		accept: bool,
		/// Success or the API error to show.
		result: Result<(), ApiError>,
	},
	/// Recently played places (discovery "continue" row + local tracking).
	RecentlyPlayedDone {
		/// Place cards for the row.
		result: Vec<PlaceCard>,
	},
	/// The session was dropped.
	LoggedOut,
	/// Live-game join pipeline events.
	Game(GameEvent),
}

/// Which slot a downloaded bitmap belongs to (users and places are kept in
/// separate maps, their id spaces can overlap).
#[derive(Debug, Clone, PartialEq)]
pub enum ImageKind {
	/// Avatar headshot of a user id.
	User(u64),
	/// Icon of a place (root place) id.
	Place(u64),
}

/// Everything the worker pushes into the winit event loop.
#[derive(Debug)]
pub enum UserEvent {
	/// A worker command finished.
	Worker(WorkerEvent),
	/// A decoded RGBA bitmap is ready for display.
	ImageReady {
		/// Target slot of the bitmap.
		kind: ImageKind,
		/// Square RGBA pixels.
		rgba: Arc<Vec<u8>>,
	},
}
