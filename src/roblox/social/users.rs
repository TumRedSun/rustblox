//! User search and friend requests (send/accept/decline + incoming list).

use super::{percent_encode, require_ok};
use crate::model::UserEntry;
use crate::roblox::client::{RobloxClient, FRIENDS_BASE, USERS_BASE};
use crate::roblox::errors::ApiError;
use crate::roblox::types::UserSearchResponse;
use serde_json::json;

/// GET /v1/users/search?keyword=...&limit=25 — find users by name.
/// The endpoint requires at least 2 characters in the keyword.
///
/// # Errors
/// Transport failures, the HTTP error of a non-2xx reply, or a parse error.
pub fn search_users(
	c: &mut RobloxClient,
	keyword: &str,
	limit: u32,
) -> Result<Vec<UserEntry>, ApiError> {
	let url = format!(
		"{}/v1/users/search?keyword={}&limit={}",
		USERS_BASE,
		percent_encode(keyword.trim()),
		limit.clamp(1, 25)
	);
	let raw = c.send(&url, None)?;
	require_ok(&raw)?;
	let sr: UserSearchResponse = serde_json::from_slice(&raw.body)
		.map_err(|e| ApiError::Parse(format!("user search: {}", e)))?;
	let mut out: Vec<UserEntry> = sr
		.data
		.into_iter()
		.filter(|u| u.id != 0)
		.map(|u| UserEntry {
			id: u.id,
			name: u.name,
			display_name: u.display_name.filter(|d| !d.is_empty()).unwrap_or_default(),
		})
		.collect();
	out.dedup_by(|a, b| a.id == b.id);
	Ok(out)
}

/// POST an empty body to a friendship action endpoint.
fn post_empty(c: &mut RobloxClient, url: &str) -> Result<(), ApiError> {
	require_ok(&c.post_raw(url, &json!({}))?)
}

/// POST /v1/friends/{userId}/request-friendship — send a friend request.
/// 200 = request sent (or already pending); 4xx carries a human message
/// ("Already friends", "Friend request already pending", …) which is shown
/// to the user as-is.
///
/// # Errors
/// The HTTP error of the friendship endpoint.
pub fn send_friend_request(c: &mut RobloxClient, user_id: u64) -> Result<(), ApiError> {
	post_empty(c, &format!("{}/v1/friends/{}/request-friendship", FRIENDS_BASE, user_id))
}

/// POST /v1/users/{userId}/accept-friend-request — accept an incoming request.
///
/// # Errors
/// The HTTP error of the friendship endpoint.
pub fn accept_friend_request(c: &mut RobloxClient, user_id: u64) -> Result<(), ApiError> {
	post_empty(c, &format!("{}/v1/users/{}/accept-friend-request", FRIENDS_BASE, user_id))
}

/// POST /v1/users/{userId}/decline-friend-request — decline an incoming request.
///
/// # Errors
/// The HTTP error of the friendship endpoint.
pub fn decline_friend_request(c: &mut RobloxClient, user_id: u64) -> Result<(), ApiError> {
	post_empty(c, &format!("{}/v1/users/{}/decline-friend-request", FRIENDS_BASE, user_id))
}

/// GET /v1/my/friends/requests — incoming friend requests of the logged-in
/// user. (The old /v1/users/{userId}/friends/requests route was removed by
/// Roblox and answers 404, so the "my" path is the only one left.) Names are
/// resolved with the batch users endpoint afterwards.
///
/// # Errors
/// Transport failures and the HTTP error of the friend-requests endpoint.
pub fn fetch_friend_requests(c: &mut RobloxClient) -> Result<Vec<UserEntry>, ApiError> {
	let url = format!("{}/v1/my/friends/requests?limit=50", FRIENDS_BASE);
	let raw = c.send(&url, None)?;
	require_ok(&raw)?;
	let ids = parse_friend_request_ids(&raw.body);
	if ids.is_empty() {
		return Ok(Vec::new());
	}
	let details = super::friends::fetch_user_details(c, &ids);
	Ok(ids
		.iter()
		.map(|id| {
			let (name, display, _) = details.get(id).cloned().unwrap_or_default();
			UserEntry {
				id: *id,
				name: if name.is_empty() { id.to_string() } else { name },
				display_name: display,
			}
		})
		.collect())
}

/// Extract requester ids from a friend-requests payload. Tolerates: a bare
/// JSON array, an object with the list under "data", and entries keyed by
/// either `userId` or `id`. Public for tests.
pub fn parse_friend_request_ids(body: &[u8]) -> Vec<u64> {
	let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else {
		return Vec::new();
	};
	let items = match &v {
		serde_json::Value::Array(a) => a.as_slice(),
		serde_json::Value::Object(o) => {
			o.get("data").and_then(|d| d.as_array()).map(|a| a.as_slice()).unwrap_or(&[])
		}
		_ => &[],
	};
	let mut ids: Vec<u64> = Vec::new();
	for it in items {
		let id = it
			.get("userId")
			.or_else(|| it.get("id"))
			.and_then(|x| x.as_u64())
			.filter(|id| *id != 0);
		if let Some(id) = id {
			ids.push(id);
		}
	}
	ids.dedup();
	ids
}
