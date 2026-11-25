//! Raw HTTP response kept alive long enough to extract headers + body.

use crate::roblox::errors::ApiError;
use std::io::Read;

/// Raw response we keep alive long enough to extract headers + body.
pub struct RawResponse {
	/// HTTP status code.
	pub status: u16,
	/// Response body bytes (capped at 4 MiB).
	pub body: Vec<u8>,
	/// `x-csrf-token` response header, when the server sent one.
	pub csrf: Option<String>,
	/// Every `Set-Cookie` line, verbatim.
	pub set_cookies: Vec<String>,
	/// Every response header as (name, value) pairs — needed for endpoints
	/// that deliver results in headers (the authentication ticket arrives in
	/// `rbx-authentication-ticket`, not in the body).
	pub headers: Vec<(String, String)>,
}

impl RawResponse {
	/// Case-insensitive single-value header lookup (HTTP header names are
	/// case-insensitive; servers emit both `RBX-Authentication-Ticket` and
	/// `rbx-authentication-ticket`).
	pub fn header(&self, name: &str) -> Option<&str> {
		self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
	}

	/// Value of the cookie named `name` inside the received `Set-Cookie`
	/// lines (case-insensitive cookie-name match).
	pub fn header_cookies_find(&self, name: &str) -> Option<String> {
		for c in &self.set_cookies {
			let Some((key, rest)) = c.split_once('=') else {
				continue;
			};
			if !key.trim().eq_ignore_ascii_case(name) {
				continue;
			}
			let value = rest.split(';').next().unwrap_or("").trim();
			if !value.is_empty() {
				return Some(value.to_string());
			}
		}
		None
	}

	/// Parse the body as JSON.
	///
	/// # Errors
	/// Returns [`ApiError::Parse`] when the body is not valid JSON for `T`.
	pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, ApiError> {
		serde_json::from_slice(&self.body).map_err(|e| ApiError::Parse(e.to_string()))
	}

	/// Map a non-2xx response to the shared API error type.
	pub fn error(&self) -> ApiError {
		ApiError::from_body(self.status, &self.body)
	}
}

/// Capture status, every header and the body of a ureq response.
pub(super) fn into_raw(resp: ureq::Response) -> RawResponse {
	let status = resp.status();
	let csrf = resp.header("x-csrf-token").map(|s| s.to_string());
	let set_cookies: Vec<String> =
		resp.all("set-cookie").into_iter().map(|s| s.to_string()).collect();
	// Capture the full header set BEFORE the response is moved into the
	// body reader.
	let headers: Vec<(String, String)> = resp
		.headers_names()
		.into_iter()
		.filter_map(|name| resp.header(&name).map(|v| (name, v.to_string())))
		.collect();
	let mut body = Vec::new();
	let _ = resp.into_reader().take(4 * 1024 * 1024).read_to_end(&mut body);
	RawResponse { status, body, csrf, set_cookies, headers }
}
