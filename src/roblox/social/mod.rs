//! Social data: friends, presence, game names, avatar thumbnails, place
//! search/recommendations. All requests are batched (<=100 ids per call) to
//! minimize round trips.
//!
//! Layout: `users.rs` (user search + friend requests), `friends.rs`
//! (friends, details, presence, last-online), `places.rs` (place discovery +
//! game names), `media/` (thumbnail URLs + image downloads).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod friends;
mod media;
mod places;
mod users;

pub use friends::{fetch_friends, fetch_last_online, fetch_presence, fetch_user_details};
pub use media::{download_image, fetch_avatar_index, fetch_game_icons};
pub use places::{
	fetch_game_names, recently_played, recommended_games, search_games, OmniGroupKind,
};
pub use users::{
	accept_friend_request, decline_friend_request, fetch_friend_requests, parse_friend_request_ids,
	search_users, send_friend_request,
};

use crate::roblox::client::{RawResponse, RobloxClient};
use crate::roblox::errors::ApiError;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Max ids per batched API call.
const CHUNK: usize = 100;

/// Percent-encode a query-string component (RFC 3986 unreserved characters
/// stay readable, everything else is %XX-escaped byte-wise).
pub fn percent_encode(s: &str) -> String {
	let mut out = String::with_capacity(s.len());
	for b in s.bytes() {
		match b {
			b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
				out.push(b as char)
			}
			other => out.push_str(&format!("%{:02X}", other)),
		}
	}
	out
}

/// Split an id list into API page-sized chunks.
fn chunks(ids: &[u64]) -> impl Iterator<Item = &[u64]> {
	ids.chunks(CHUNK)
}

/// Non-zero, sorted, de-duplicated ids (batched endpoints reject 0 and
/// repeated ids only waste payload space).
fn unique_ids(ids: &[u64]) -> Vec<u64> {
	let mut v: Vec<u64> = ids.to_vec();
	v.sort_unstable();
	v.dedup();
	v.retain(|id| *id != 0);
	v
}

/// `Ok(())` for a 2xx response, otherwise the mapped API error.
fn require_ok(raw: &RawResponse) -> Result<(), ApiError> {
	if (200..300).contains(&raw.status) {
		Ok(())
	} else {
		Err(raw.error())
	}
}

/// Best-effort JSON GET: transport, status or parse failures all collapse to
/// `None` (the UI falls back to ids/last-location for these rows).
fn get_json_option<T: DeserializeOwned>(c: &mut RobloxClient, url: &str) -> Option<T> {
	let raw = c.send(url, None).ok()?;
	if !(200..300).contains(&raw.status) {
		return None;
	}
	raw.json().ok()
}

/// Best-effort JSON POST, same contract as [`get_json_option`].
fn post_json_option<T: DeserializeOwned>(
	c: &mut RobloxClient,
	url: &str,
	body: &Value,
) -> Option<T> {
	let raw = c.post_raw(url, body).ok()?;
	if !(200..300).contains(&raw.status) {
		return None;
	}
	raw.json().ok()
}
