//! The one-time authentication ticket: request, extraction, and the join
//! attempt id that tags every join request.

use super::request::snippet;
use crate::roblox::client::RobloxClient;
use crate::roblox::errors::ApiError;
use std::borrow::Cow;

/// The one-time join ticket lives on the auth service (NOT apis.roblox.com —
/// that host answers 404 for this path, which silently broke the whole join
/// chain: ticket 404 → anonymous v1 fallback → refusal status 12).
pub const AUTH_TICKET_URL: &str = "https://auth.roblox.com/v1/authentication-ticket";

/// Fetch the one-time authentication ticket used by game join.
///
/// # Errors
/// The HTTP error of the ticket endpoint, or [`ApiError::Parse`] when a 2xx
/// response carries no ticket at all (the failure mode that used to ship
/// `{}` to gamejoin).
pub fn fetch_auth_ticket(client: &mut RobloxClient, place_id: u64) -> Result<String, ApiError> {
	// The endpoint answers 200 with an EMPTY JSON body ({}) and delivers the
	// ticket in the `rbx-authentication-ticket` response header (verified
	// live 2026-11; matches every public client implementation). Mandatory
	// request pieces on top of the .ROBLOSECURITY cookie: the CSRF dance
	// (post_with_headers runs it) and a roblox.com `Referer` — without that
	// the server answers 403 "The UrlReferrer was invalid or came from
	// outside Roblox.".
	let body = serde_json::json!({});
	let referer = format!("https://www.roblox.com/games/{}/", place_id);
	let raw = client.post_with_headers(
		AUTH_TICKET_URL,
		&body,
		&[("Referer", referer.as_str()), ("Origin", "https://www.roblox.com")],
	)?;
	if !(200..300).contains(&raw.status) {
		let err = raw.error();
		crate::netlog!(info, "auth ticket HTTP {}: {}", raw.status, err);
		return Err(err);
	}
	crate::netlog!(
		info,
		"auth ticket {} -> HTTP {} ({} bytes body)",
		AUTH_TICKET_URL,
		raw.status,
		raw.body.len()
	);
	let text = String::from_utf8_lossy(&raw.body).to_string();
	let header_ticket = raw
		.header("rbx-authentication-ticket")
		.map(|t| t.trim().to_string())
		.filter(|t| !t.is_empty());
	match ticket_from_parts(header_ticket.as_deref(), &text) {
		Some(t) => {
			// Log the length only — the ticket is a bearer credential.
			crate::netlog!(info, "auth ticket received ({} chars)", t.len());
			Ok(t.into_owned())
		}
		None => {
			let names: Vec<&str> = raw.headers.iter().map(|(k, _)| k.as_str()).collect();
			crate::netlog!(
				info,
				"auth ticket: no ticket in the response (headers present: {})",
				names.join(", ")
			);
			Err(ApiError::Parse(format!(
                "authentication-ticket response carried no ticket (rbx-authentication-ticket header missing; body: {})",
                snippet(&text, 80)
            )))
		}
	}
}

/// Locate the authentication ticket in a successful /v1/authentication-ticket
/// response. Priority: the `rbx-authentication-ticket` header (the only place
/// the live endpoint puts it today), then a JSON body ticket field, then a
/// legacy plain-text body. A JSON body WITHOUT a ticket field (the documented
/// `{}` 200 body) is None — returning the body itself as the ticket is exactly
/// the bug that shipped "{}" to gamejoin and drew refusal status 12.
pub(super) fn ticket_from_parts<'a>(
	header: Option<&'a str>,
	body: &'a str,
) -> Option<Cow<'a, str>> {
	if let Some(t) = header.map(str::trim).filter(|t| !t.is_empty()) {
		return Some(Cow::Borrowed(t));
	}
	if let Ok(v) = serde_json::from_str::<serde_json::Value>(body.trim()) {
		for key in ["ticket", "authenticationTicket", "token"] {
			if let Some(t) = v.get(key).and_then(|t| t.as_str()) {
				let t = t.trim();
				if !t.is_empty() {
					return Some(Cow::Owned(t.to_string()));
				}
			}
		}
		return None;
	}
	// Plain-text body (older dialects returned the bare ticket).
	let t = body.trim();
	if !t.is_empty() && !t.starts_with('<') {
		Some(Cow::Borrowed(t))
	} else {
		None
	}
}

/// A fresh `gameJoinAttemptId` (UUID-v4-shaped). The official client tags
/// every join attempt with one; the value is informational but matching the
/// client shape costs nothing. std-only xorshift — no uuid dependency.
pub(super) fn join_attempt_id() -> String {
	use std::time::{SystemTime, UNIX_EPOCH};
	fn next(x: &mut u64) -> u64 {
		*x ^= *x << 13;
		*x ^= *x >> 7;
		*x ^= *x << 17;
		*x
	}
	let seed = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.map(|d| d.as_nanos() as u64)
		.unwrap_or(0x9E37_79B9_7F4A_7C15)
		^ ((std::process::id() as u64) << 32);
	let mut x = seed | 1;
	let a = next(&mut x);
	let b = next(&mut x);
	let mut bytes = [0u8; 16];
	bytes[..8].copy_from_slice(&a.to_be_bytes());
	bytes[8..].copy_from_slice(&b.to_be_bytes());
	// RFC 4122 version 4 + variant bits.
	bytes[6] = (bytes[6] & 0x0f) | 0x40;
	bytes[8] = (bytes[8] & 0x3f) | 0x80;
	let h: Vec<String> = bytes.iter().map(|b| format!("{:02x}", b)).collect();
	format!(
		"{}-{}-{}-{}-{}",
		h[0..4].join(""),
		h[4..6].join(""),
		h[6..8].join(""),
		h[8..10].join(""),
		h[10..16].join("")
	)
}
