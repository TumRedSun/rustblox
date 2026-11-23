//! Friend list, batch user details, presence and last-online times.

use super::{chunks, post_json_option, require_ok};
use crate::model::{Friend, Presence, PresenceKind};
use crate::roblox::client::{RobloxClient, FRIENDS_BASE, PRESENCE_BASE, USERS_BASE};
use crate::roblox::errors::ApiError;
use crate::roblox::types::{
	FriendsResponse, LastOnlineResponse, PresenceResponse, UsersBatchResponse,
};
use serde_json::json;
use std::collections::HashMap;

/// GET /v1/users/{userId}/friends
///
/// # Errors
/// Transport failures and the HTTP error of the friends endpoint; the
/// follow-up name resolution is best effort.
pub fn fetch_friends(c: &mut RobloxClient, user_id: u64) -> Result<Vec<Friend>, ApiError> {
	let url = format!("{}/v1/users/{}/friends", FRIENDS_BASE, user_id);
	let raw = c.send(&url, None)?;
	require_ok(&raw)?;
	// The endpoint returns {"data":[...],"nextPageCursor":...}; an erroring
	// context can also return an {"errors":[...]} envelope — parse defensibly.
	let fr: FriendsResponse = serde_json::from_slice(&raw.body)
		.map_err(|e| ApiError::Parse(format!("friends: {}", e)))?;
	let mut friends: Vec<Friend> = fr
		.data
		.into_iter()
		.map(|f| {
			let display_name =
				f.display_name.filter(|d| !d.is_empty()).unwrap_or_else(|| f.name.clone());
			Friend { id: f.id, name: f.name, display_name, is_banned: f.is_banned.unwrap_or(false) }
		})
		.collect();

	// Since Oct 2024 the friends endpoint no longer includes user names
	// (the fields come back empty). Resolve them with the batch users
	// endpoint; best effort — on failure the UI falls back to ids.
	if friends.iter().any(|f| f.name.is_empty() || f.display_name.is_empty()) {
		resolve_friend_names(c, &mut friends);
	}
	Ok(friends)
}

/// Fill the empty name fields from the batch users endpoint, with a
/// last-resort label so a row is never just "@".
fn resolve_friend_names(c: &mut RobloxClient, friends: &mut [Friend]) {
	let ids: Vec<u64> = friends.iter().map(|f| f.id).collect();
	let details = fetch_user_details(c, &ids);
	for f in friends.iter_mut() {
		if let Some((name, display, banned)) = details.get(&f.id) {
			if f.name.is_empty() {
				f.name = name.clone();
			}
			if f.display_name.is_empty() {
				f.display_name = if display.is_empty() { name.clone() } else { display.clone() };
			}
			if *banned {
				f.is_banned = true;
			}
		}
		if f.display_name.is_empty() {
			f.display_name = format!("User {}", f.id);
		}
		if f.name.is_empty() {
			f.name = f.id.to_string();
		}
	}
}

/// POST /v1/users — batch user details (<=100 ids per call). Returns
/// (username, display name, is_banned) keyed by user id.
pub fn fetch_user_details(
	c: &mut RobloxClient,
	ids: &[u64],
) -> HashMap<u64, (String, String, bool)> {
	let mut out = HashMap::with_capacity(ids.len());
	for chunk in chunks(ids) {
		let Some(ur) = post_json_option::<UsersBatchResponse>(
			c,
			&format!("{}/v1/users", USERS_BASE),
			&json!({ "userIds": chunk }),
		) else {
			continue;
		};
		for u in ur.data {
			let display = u.display_name.filter(|d| !d.is_empty()).unwrap_or_default();
			out.insert(u.id, (u.name, display, u.is_banned.unwrap_or(false)));
		}
	}
	out
}

/// POST /v1/presence/users — batched presence lookup.
///
/// # Errors
/// Transport failures and the HTTP error of the presence endpoint.
pub fn fetch_presence(
	c: &mut RobloxClient,
	ids: &[u64],
) -> Result<HashMap<u64, Presence>, ApiError> {
	let mut out = HashMap::with_capacity(ids.len());
	for chunk in chunks(ids) {
		let raw = c.post_raw(
			&format!("{}/v1/presence/users", PRESENCE_BASE),
			&json!({ "userIds": chunk }),
		)?;
		require_ok(&raw)?;
		let pr: PresenceResponse = raw.json()?;
		for p in pr.user_presences {
			out.insert(
				p.user_id,
				Presence {
					kind: PresenceKind::from_api(p.presence_type),
					place_id: p.place_id,
					root_place_id: p.root_place_id,
					universe_id: p.universe_id,
					game_id: p.game_id,
					last_location: p.last_location.unwrap_or_default(),
				},
			);
		}
	}
	Ok(out)
}

/// POST /v1/presence/last-online — best effort (empty map on any error).
pub fn fetch_last_online(c: &mut RobloxClient, ids: &[u64]) -> HashMap<u64, i64> {
	let mut out = HashMap::new();
	for chunk in chunks(ids) {
		let Some(lr) = post_json_option::<LastOnlineResponse>(
			c,
			&format!("{}/v1/presence/last-online", PRESENCE_BASE),
			&json!({ "userIds": chunk }),
		) else {
			continue;
		};
		for e in lr.entries {
			if let Some(ts) = e.last_online.and_then(|s| crate::model::parse_iso8601_utc(&s)) {
				out.insert(e.user_id, ts);
			}
		}
	}
	out
}
