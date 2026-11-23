//! Commands the UI thread sends to the background worker.

/// Background work requested by the UI thread.
#[derive(Debug)]
pub enum Command {
	/// Password sign-in.
	LoginPassword {
		/// Username or e-mail.
		login: String,
		/// Account password.
		password: String,
	},
	/// Submit a 2-step verification code.
	Verify2FA {
		/// Username the login ticket belongs to.
		login: String,
		/// Login ticket from the rejected password attempt.
		ticket: String,
		/// Code the user typed.
		code: String,
		/// Remember this device for later sessions.
		remember: bool,
	},
	/// Ask for a new 2-step code.
	Resend2FA {
		/// Username the login ticket belongs to.
		login: String,
		/// Login ticket from the rejected password attempt.
		ticket: String,
	},
	/// Cookie (`.ROBLOSECURITY`) sign-in.
	LoginCookie {
		/// Session token value.
		token: String,
	},
	/// Reload friends/presence/avatars.
	StartRefresh,
	/// User search.
	SearchUsers {
		/// Search keyword.
		query: String,
	},
	/// Send a friend request.
	AddFriend {
		/// Target user id.
		user_id: u64,
	},
	/// Place search: an empty query requests the default recommendation feed.
	SearchPlaces {
		/// Search keyword (empty = discovery feed).
		query: String,
	},
	/// List incoming friend requests.
	FetchFriendRequests,
	/// Accept (true) or decline (false) an incoming friend request.
	RespondFriendRequest {
		/// Requesting user id.
		user_id: u64,
		/// Accept when true, decline when false.
		accept: bool,
	},
	/// Enter a live place server (desktop): runs the real join flow.
	JoinPlace {
		/// Place id to join.
		place_id: u64,
	},
	/// Drop the session and clear cached social state.
	Logout,
	/// Stop the worker thread.
	Shutdown,
}
