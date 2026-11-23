//! Live game join flow — the real sequence the official client performs to
//! enter a running place instance on Roblox's game servers.
//!
//! Steps:
//!  1. POST `auth.roblox.com/v1/authentication-ticket` — exchange the
//!     `.ROBLOSECURITY` session for a short-lived join ticket. The request
//!     needs the session cookie + CSRF dance + a roblox.com `Referer`; the
//!     answer is HTTP 200 with an EMPTY body and the ticket inside the
//!     `rbx-authentication-ticket` RESPONSE HEADER.
//!  2. POST `gamejoin.roblox.com/v1/join-game` — request membership in a
//!     running server, authenticating with the session cookie + CSRF under a
//!     client User-Agent ("Roblox/WinInet"). Success answers
//!     `{"jobId": "…", "status": 2, "joinScript": {…}}` with the game-server
//!     endpoint inside the joinScript object.
//!  3. (fallbacks) the same join without the ticket, then
//!     `gamejoin.roblox.com/v1/join-game-instance`.
//!
//! The UDP handshake with the returned server then happens in
//! `crate::net::raknet` + `crate::game`.
//!
//! Layout: `request/` performs the join POST and recovers the endpoint +
//! udmux/netstack material, `response.rs` is the typed reply, `ticket.rs`
//! the authentication ticket exchange, `avatar.rs` the avatar appearance.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod avatar;
mod request;
mod response;
mod ticket;

#[cfg(test)]
#[path = "tests.rs"]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests;

pub use avatar::{fetch_avatar_appearance, AvatarAppearance, AvatarAsset, BodyColors};
pub use response::JoinResponse;
pub use ticket::{fetch_auth_ticket, AUTH_TICKET_URL};

use crate::roblox::client::RobloxClient;
use crate::roblox::errors::ApiError;
use request::join_request;

pub(crate) use response::join_refusal_detail;

/// Host of the game-join service (join-game / join-game-instance).
pub const GAMEJOIN_BASE: &str = "https://gamejoin.roblox.com";

/// Join a running place server: authentication ticket, then the game-join
/// service. The classic `/v1/join-game` is the primary; `/v1/join-game-instance`
/// (newer unified API) is the final fallback. NOTE: `/v2/join-game` does NOT
/// exist on gamejoin.roblox.com (probed 2026-11: 404) — never use it.
///
/// # Errors
/// [`ApiError::JoinRefused`] when not signed in or the server refuses the
/// join, [`ApiError::Parse`] when an accepted join carries no usable server
/// address, plus any transport error of the two HTTP calls.
pub fn join_game(client: &mut RobloxClient, place_id: u64) -> Result<JoinResponse, ApiError> {
	crate::netlog!(info, "join: requesting server for place {}", place_id);
	let ticket = request_ticket(client, place_id)?;
	let referer = referer_for(place_id);
	let base = base_body(place_id);
	let primary = match ticket.as_deref() {
		Some(t) => with_ticket(base.clone(), t),
		None => base.clone(),
	};
	let first_err = match join_request(client, "/v1/join-game", &primary, &referer) {
		Ok(resp) => return Ok(resp),
		Err(e) => e,
	};
	crate::netlog!(info, "join-game v1 failed ({}); trying fallbacks", first_err);
	// Fallback 1: drop the ticket — public client implementations join with
	// cookie-only auth, so retry that way in case the server disliked ours.
	if ticket.is_some() {
		match join_request(
			client,
			"/v1/join-game",
			&with_ticket_or_null(base.clone(), None),
			&referer,
		) {
			Ok(resp) => return Ok(resp),
			Err(e) => crate::netlog!(
				info,
				"join-game without ticket failed ({}); trying join-game-instance",
				e
			),
		}
	}
	// Fallback 2: the newer unified endpoint.
	let unified = with_ticket_or_null(base, ticket.as_deref());
	join_request(client, "/v1/join-game-instance", &unified, &referer).map_err(|e| {
		crate::netlog!(info, "join-game-instance failed too: {}", e);
		if matches!(e, ApiError::JoinRefused(_)) {
			e
		} else {
			first_err
		}
	})
}

/// Fetch the one-time join ticket, or `None` when the endpoint failed but a
/// session cookie is present (the cookie-only fallback that follows may still
/// work). Without a session the ticket call can never succeed and an
/// anonymous join is always refused (status 12), so that case fails fast with
/// an actionable message.
fn request_ticket(client: &mut RobloxClient, place_id: u64) -> Result<Option<String>, ApiError> {
	match fetch_auth_ticket(client, place_id) {
		Ok(t) => Ok(Some(t)),
		Err(e) => {
			if !client.has_session() {
				crate::netlog!(
					info,
					"join: no session cookie — cannot request an authentication ticket"
				);
				return Err(ApiError::JoinRefused(
                    "not signed in: joining a server needs a session — sign in or add a .ROBLOSECURITY cookie first".into(),
                ));
			}
			crate::netlog!(info, "join: auth ticket unavailable ({}); trying cookie-only join", e);
			Ok(None)
		}
	}
}

fn referer_for(place_id: u64) -> String {
	format!("https://www.roblox.com/games/{}/", place_id)
}

/// Modern client shape (RoZod gamejoinv1 schema + live client traffic): the
/// join is authenticated by the session cookie + CSRF under a client
/// User-Agent. The authenticationTicket rides along when we have one — what
/// the official client passes in via the roblox-player URI.
fn base_body(place_id: u64) -> serde_json::Value {
	serde_json::json!({
		"placeId": place_id,
		"isTeleport": false,
		"isImmersiveAdsTeleport": false,
		"gameJoinAttemptId": ticket::join_attempt_id(),
	})
}

fn with_ticket(mut body: serde_json::Value, ticket: &str) -> serde_json::Value {
	body["authenticationTicket"] = serde_json::Value::String(ticket.to_string());
	body
}

/// Explicit JSON null (not a missing key) — the shape the cookie-only retry
/// and the unified endpoint expect.
fn with_ticket_or_null(mut body: serde_json::Value, ticket: Option<&str>) -> serde_json::Value {
	body["authenticationTicket"] =
		ticket.map(|t| serde_json::Value::String(t.to_string())).unwrap_or(serde_json::Value::Null);
	body
}
