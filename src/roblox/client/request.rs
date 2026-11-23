//! `RobloxClient`: the request engine (cookies, CSRF rotation, JSON shims).

use super::raw::{into_raw, RawResponse};
use super::UA;
use crate::roblox::dns::PinnedResolver;
use crate::roblox::errors::ApiError;
use std::collections::BTreeMap;
use std::io::Read;
use std::time::Duration;

/// CSRF rotation attempts per request (the first one primes the token).
const MAX_ATTEMPTS: usize = 3;

/// HTTP verb for [`RobloxClient::dispatch`].
#[derive(Clone, Copy)]
enum Method {
	Get,
	Post,
}

/// Session-bearing client for the Roblox web APIs.
pub struct RobloxClient {
	agent: ureq::Agent,
	/// Last CSRF token seen (`X-CSRF-Token` request header).
	pub csrf: Option<String>,
	/// `.ROBLOSECURITY` cookie value (the session token).
	pub security: Option<String>,
	/// Browser-style cookie jar for all *.roblox.com hosts. The server
	/// binds CSRF tokens to an anonymous cookie session, so replaying the
	/// cookies we received is required for the auth dance to work.
	jar: BTreeMap<String, String>,
}

fn build_agent() -> ureq::Agent {
	ureq::AgentBuilder::new()
		.timeout_connect(Duration::from_secs(10))
		.timeout(Duration::from_secs(30))
		.redirects(0)
		.user_agent(UA)
		// Pinned Roblox IPs (see dns): DNS is bypassed entirely for the API
		// hosts so resolver blocking/poisoning cannot take the client down.
		.resolver(PinnedResolver)
		.build()
}

impl Default for RobloxClient {
	fn default() -> Self {
		Self::new()
	}
}

impl RobloxClient {
	/// Create a client with a fresh agent and an empty cookie jar.
	pub fn new() -> Self {
		RobloxClient { agent: build_agent(), csrf: None, security: None, jar: BTreeMap::new() }
	}

	/// Install a session token (explicit field + cookie jar).
	pub fn set_security(&mut self, token: &str) {
		self.security = Some(token.to_string());
		self.jar.insert(".ROBLOSECURITY".to_string(), token.to_string());
	}

	/// Drop the session token from both the field and the jar.
	pub fn clear_security(&mut self) {
		self.security = None;
		self.jar.remove(".ROBLOSECURITY");
	}

	/// Number of cookies currently in the jar.
	pub fn jar_len(&self) -> usize {
		self.jar.len()
	}

	/// True when the client carries a Roblox session (explicit field or the
	/// browser-style jar holding `.ROBLOSECURITY`).
	pub fn has_session(&self) -> bool {
		self.security.is_some() || self.jar.contains_key(".ROBLOSECURITY")
	}

	/// Perform a request with the CSRF rotation dance, exactly like the
	/// official site: on 403 "Token Validation Failed" the response carries a
	/// fresh `x-csrf-token` plus fresh anonymous cookies; store both, retry.
	///
	/// # Errors
	/// Returns [`ApiError::Network`] on transport failures and
	/// [`ApiError::Parse`] when the JSON body cannot be serialized. HTTP
	/// statuses are returned as-is in the [`RawResponse`].
	pub fn send(
		&mut self,
		url: &str,
		json_body: Option<&serde_json::Value>,
	) -> Result<RawResponse, ApiError> {
		let method = if json_body.is_some() { Method::Post } else { Method::Get };
		self.dispatch(method, url, json_body, &[])
	}

	/// POST JSON returning the raw response (for cookie extraction).
	///
	/// # Errors
	/// Same contract as [`RobloxClient::send`].
	pub fn post_raw(
		&mut self,
		url: &str,
		body: &serde_json::Value,
	) -> Result<RawResponse, ApiError> {
		self.dispatch(Method::Post, url, Some(body), &[])
	}

	/// POST JSON with extra headers (game join needs Referer/Origin).
	/// Runs the same CSRF-rotation dance as `send`.
	///
	/// # Errors
	/// Same contract as [`RobloxClient::send`].
	pub fn post_with_headers(
		&mut self,
		url: &str,
		body: &serde_json::Value,
		headers: &[(&str, &str)],
	) -> Result<RawResponse, ApiError> {
		self.dispatch(Method::Post, url, Some(body), headers)
	}

	/// GET expecting JSON.
	///
	/// # Errors
	/// Transport/parse failures plus the HTTP error of a non-2xx response.
	pub fn get_json<T: serde::de::DeserializeOwned>(&mut self, url: &str) -> Result<T, ApiError> {
		let raw = self.send(url, None)?;
		if (200..300).contains(&raw.status) {
			raw.json()
		} else {
			Err(raw.error())
		}
	}

	/// POST JSON expecting JSON.
	///
	/// # Errors
	/// Transport/parse failures plus the HTTP error of a non-2xx response.
	pub fn post_json<T: serde::de::DeserializeOwned>(
		&mut self,
		url: &str,
		body: &serde_json::Value,
	) -> Result<T, ApiError> {
		let raw = self.send(url, Some(body))?;
		if (200..300).contains(&raw.status) {
			raw.json()
		} else {
			Err(raw.error())
		}
	}

	/// Plain GET without auth headers (CDN downloads).
	///
	/// # Errors
	/// Transport failures and the HTTP status of a non-2xx response.
	pub fn get_cdn_bytes(&self, url: &str) -> Result<Vec<u8>, ApiError> {
		let sent = self.agent.get(url).set("Accept", "*/*").call();
		match sent {
			Ok(resp) => {
				let mut buf = Vec::new();
				resp.into_reader()
					.take(8 * 1024 * 1024)
					.read_to_end(&mut buf)
					.map_err(|e| ApiError::Network(e.to_string()))?;
				Ok(buf)
			}
			Err(ureq::Error::Status(code, _)) => {
				Err(ApiError::Status { code, message: String::new() })
			}
			Err(ureq::Error::Transport(t)) => Err(ApiError::Network(t.to_string())),
		}
	}

	/// One request plus the CSRF retry loop shared by every public entry
	/// point. Behavioral contract: at most `MAX_ATTEMPTS` round trips, and
	/// only a 403 carrying a fresh token is retried.
	fn dispatch(
		&mut self,
		method: Method,
		url: &str,
		json_body: Option<&serde_json::Value>,
		headers: &[(&str, &str)],
	) -> Result<RawResponse, ApiError> {
		let mut attempt = 0usize;
		loop {
			attempt += 1;
			let raw = self.send_once(method, url, json_body, headers)?;
			self.apply_set_cookies(&raw);
			if let Some(tok) = &raw.csrf {
				self.csrf = Some(tok.clone());
			}
			if !is_csrf_rejection(&raw) || attempt >= MAX_ATTEMPTS {
				return Ok(raw);
			}
		}
	}

	fn send_once(
		&self,
		method: Method,
		url: &str,
		json_body: Option<&serde_json::Value>,
		headers: &[(&str, &str)],
	) -> Result<RawResponse, ApiError> {
		let mut req = match method {
			Method::Get => self.agent.get(url),
			Method::Post => self.agent.post(url),
		};
		req = req.set("Accept", "application/json");
		if let Some(tok) = &self.csrf {
			req = req.set("X-CSRF-Token", tok);
		}
		if let Some(cookie) = self.cookie_header() {
			req = req.set("Cookie", &cookie);
		}
		for (k, v) in headers {
			req = req.set(k, v);
		}

		let sent = match json_body {
			Some(body) => {
				let bytes =
					serde_json::to_vec(body).map_err(|e| ApiError::Network(e.to_string()))?;
				req.set("Content-Type", "application/json").send_bytes(&bytes)
			}
			None => req.call(),
		};
		match sent {
			Ok(resp) => Ok(into_raw(resp)),
			Err(ureq::Error::Status(_code, resp)) => Ok(into_raw(resp)),
			Err(ureq::Error::Transport(t)) => Err(ApiError::Network(t.to_string())),
		}
	}

	/// Store every cookie the server sent us (browser-style jar). Empty
	/// values (cookie deletion) are ignored.
	fn apply_set_cookies(&mut self, raw: &RawResponse) {
		for line in &raw.set_cookies {
			let pair = line.split(';').next().unwrap_or("");
			if let Some((name, value)) = pair.split_once('=') {
				let name = name.trim();
				let value = value.trim();
				if !name.is_empty() && !value.is_empty() {
					self.jar.insert(name.to_string(), value.to_string());
				}
			}
		}
	}

	fn cookie_header(&self) -> Option<String> {
		if self.jar.is_empty() {
			return None;
		}
		let joined =
			self.jar.iter().map(|(k, v)| format!("{}={}", k, v)).collect::<Vec<_>>().join("; ");
		Some(joined)
	}
}

/// A 403 "Token Validation Failed" carrying a fresh token is the only
/// retryable response — anything else is returned to the caller.
fn is_csrf_rejection(raw: &RawResponse) -> bool {
	raw.status == 403
		&& raw.csrf.is_some()
		&& String::from_utf8_lossy(&raw.body).contains("Token Validation Failed")
}
