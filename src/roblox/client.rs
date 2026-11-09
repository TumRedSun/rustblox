//! HTTP client for Roblox web APIs.
//!
//! Mirrors what the official site does:
//!  * every mutating request carries `X-CSRF-Token`;
//!  * a 403 "Token Validation Failed" response includes a fresh
//!    `x-csrf-token` header — the token is rotated and the request retried;
//!  * the authenticated session is the `.ROBLOSECURITY` cookie sent as
//!    `Cookie: .ROBLOSECURITY=<token>` on every request.

use crate::roblox::errors::ApiError;
use std::collections::BTreeMap;
use std::io::Read;
use std::time::Duration;

pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36 RustBlox/0.1.0";

pub const AUTH_BASE: &str = "https://auth.roblox.com";
pub const USERS_BASE: &str = "https://users.roblox.com";
pub const FRIENDS_BASE: &str = "https://friends.roblox.com";
pub const PRESENCE_BASE: &str = "https://presence.roblox.com";
pub const THUMBS_BASE: &str = "https://thumbnails.roblox.com";
pub const GAMES_BASE: &str = "https://games.roblox.com";

/// Raw response we keep alive long enough to extract headers + body.
pub struct RawResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub csrf: Option<String>,
    pub set_cookies: Vec<String>,
}

impl RawResponse {
    pub fn header_cookies_find(&self, name: &str) -> Option<String> {
        for c in &self.set_cookies {
            let prefix = format!("{}=", name);
            if c.starts_with(&prefix) || c.to_lowercase().starts_with(&prefix.to_lowercase()) {
                let val = c[prefix.len()..].split(';').next().unwrap_or("");
                if !val.is_empty() {
                    return Some(val.trim().to_string());
                }
            }
        }
        None
    }

    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, ApiError> {
        serde_json::from_slice(&self.body).map_err(|e| ApiError::Parse(e.to_string()))
    }

    pub fn error(&self) -> ApiError {
        ApiError::from_body(self.status, &self.body)
    }
}

pub struct RobloxClient {
    agent: ureq::Agent,
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
        .build()
}

impl Default for RobloxClient {
    fn default() -> Self {
        Self::new()
    }
}

impl RobloxClient {
    pub fn new() -> Self {
        RobloxClient { agent: build_agent(), csrf: None, security: None, jar: BTreeMap::new() }
    }

    pub fn set_security(&mut self, token: &str) {
        self.security = Some(token.to_string());
        self.jar.insert(".ROBLOSECURITY".to_string(), token.to_string());
    }

    pub fn clear_security(&mut self) {
        self.security = None;
        self.jar.remove(".ROBLOSECURITY");
    }

    /// Number of cookies currently in the jar.
    pub fn jar_len(&self) -> usize {
        self.jar.len()
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
        let joined = self
            .jar
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("; ");
        Some(joined)
    }

    /// Perform a request with the CSRF rotation dance, exactly like the
    /// official site: on 403 "Token Validation Failed" the response carries a
    /// fresh `x-csrf-token` plus fresh anonymous cookies; store both, retry.
    pub fn send(
        &mut self,
        url: &str,
        json_body: Option<&serde_json::Value>,
    ) -> Result<RawResponse, ApiError> {
        let mut attempts = 0usize;
        loop {
            attempts += 1;
            let raw = self.send_once(url, json_body)?;
            self.apply_set_cookies(&raw);
            if let Some(tok) = &raw.csrf {
                if self.csrf.as_deref() != Some(tok.as_str()) {
                    self.csrf = Some(tok.clone());
                }
            }
            let token_invalid = raw.status == 403
                && String::from_utf8_lossy(&raw.body).contains("Token Validation Failed");
            if token_invalid && attempts < 3 && raw.csrf.is_some() {
                continue;
            }
            return Ok(raw);
        }
    }

    fn send_once(&mut self, url: &str, json_body: Option<&serde_json::Value>) -> Result<RawResponse, ApiError> {
        let mut req = if json_body.is_some() {
            self.agent.post(url)
        } else {
            self.agent.get(url)
        };
        req = req.set("Accept", "application/json");
        if let Some(tok) = &self.csrf {
            req = req.set("X-CSRF-Token", tok);
        }
        if let Some(cookie) = self.cookie_header() {
            req = req.set("Cookie", &cookie);
        }

        let sent = if let Some(body) = json_body {
            req.set("Content-Type", "application/json")
                .send_bytes(&serde_json::to_vec(body).map_err(|e| ApiError::Network(e.to_string()))?)
        } else {
            req.call()
        };

        match sent {
            Ok(resp) => Ok(into_raw(resp)),
            Err(ureq::Error::Status(_code, resp)) => Ok(into_raw(resp)),
            Err(ureq::Error::Transport(t)) => Err(ApiError::Network(t.to_string())),
        }
    }

    /// GET expecting JSON.
    pub fn get_json<T: serde::de::DeserializeOwned>(&mut self, url: &str) -> Result<T, ApiError> {
        let raw = self.send(url, None)?;
        if (200..300).contains(&raw.status) {
            raw.json()
        } else {
            Err(raw.error())
        }
    }

    /// POST JSON expecting JSON.
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

    /// POST JSON returning the raw response (for cookie extraction).
    pub fn post_raw(&mut self, url: &str, body: &serde_json::Value) -> Result<RawResponse, ApiError> {
        self.send(url, Some(body))
    }

    /// Plain GET without auth headers (CDN downloads).
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
            Err(ureq::Error::Status(code, _)) => Err(ApiError::Status { code, message: String::new() }),
            Err(ureq::Error::Transport(t)) => Err(ApiError::Network(t.to_string())),
        }
    }
}

fn into_raw(resp: ureq::Response) -> RawResponse {
    let status = resp.status();
    let csrf = resp.header("x-csrf-token").map(|s| s.to_string());
    let set_cookies: Vec<String> = resp
        .all("set-cookie")
        .into_iter()
        .map(|s| s.to_string())
        .collect();
    let mut body = Vec::new();
    let _ = resp.into_reader().take(4 * 1024 * 1024).read_to_end(&mut body);
    RawResponse { status, body, csrf, set_cookies }
}

/// Normalize a pasted `.ROBLOSECURITY` value: accepts the bare token, a full
/// `Set-Cookie` line, a `Cookie:` header, or a token with the anti-sharing
/// warning prefix (both `|_token` and `|token` endings are handled).
pub fn normalize_cookie_token(input: &str) -> String {
    let mut t = input.trim().trim_matches('"').trim().to_string();
    if let Some(pos) = t.find(".ROBLOSECURITY=") {
        let rest = t[pos + ".ROBLOSECURITY=".len()..].to_string();
        let cut = rest.find(';').unwrap_or(rest.len());
        t = rest[..cut].trim().to_string();
    }
    if t.starts_with('_') && t.contains("WARNING") {
        if let Some(pos) = t.rfind('|') {
            let rest = t[pos + 1..].trim_start_matches('_');
            if !rest.is_empty() {
                t = rest.to_string();
            }
        }
    }
    t.trim().to_string()
}
