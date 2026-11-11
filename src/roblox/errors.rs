//! Errors shared across the networking layer, with human-readable messages.

use std::fmt;

#[derive(Debug, Clone)]
pub enum ApiError {
    /// Transport-level failure (DNS, TLS, timeout...).
    Network(String),
    /// Non-2xx response that is not a recognized auth error.
    Status { code: u16, message: String },
    /// HTTP 401 "Incorrect username or password".
    InvalidCredentials,
    /// Server asked for a captcha challenge (happens with password login).
    CaptchaRequired,
    /// HTTP 429.
    RateLimited { retry_after: Option<u64> },
    /// 2-step verification is required; carries the login ticket.
    TwoStepRequired { ticket: String, media_type: String },
    /// 2FA code was wrong / expired.
    BadTwoStepCode(String),
    /// Cookie login rejected (invalid or expired .ROBLOSECURITY).
    InvalidCookie(String),
    /// The game-join service refused the join (HTTP 200 with jobId: null);
    /// carries the human-readable reason.
    JoinRefused(String),
    /// Response body failed to parse.
    Parse(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Network(m) => write!(f, "Network error: {}", m),
            ApiError::Status { code, message } => {
                if message.is_empty() {
                    write!(f, "HTTP error {}", code)
                } else {
                    write!(f, "HTTP {}: {}", code, message)
                }
            }
            ApiError::InvalidCredentials => write!(f, "Incorrect username or password"),
            ApiError::CaptchaRequired => {
                write!(f, "Roblox requires a captcha for this login. Use the Cookie sign-in method instead.")
            }
            ApiError::RateLimited { retry_after } => match retry_after {
                Some(secs) => write!(f, "Rate limited. Retry in {}s", secs),
                None => write!(f, "Rate limited. Try again later"),
            },
            ApiError::TwoStepRequired { media_type, .. } => {
                write!(f, "2-step verification required ({})", media_type)
            }
            ApiError::BadTwoStepCode(m) => write!(f, "Verification failed: {}", m),
            ApiError::InvalidCookie(m) => write!(f, "Invalid session cookie: {}", m),
            ApiError::JoinRefused(m) => write!(f, "Server refused the join: {}", m),
            ApiError::Parse(m) => write!(f, "Failed to parse API response: {}", m),
        }
    }
}

impl std::error::Error for ApiError {}

impl ApiError {
    /// Extracts the first error message out of a Roblox JSON error envelope:
    /// {"errors":[{"code":1,"message":"..."}]}
    pub fn from_body(status: u16, body: &[u8]) -> ApiError {
        let text = String::from_utf8_lossy(body);
        let envelope: crate::roblox::types::ErrorsEnvelope = serde_json::from_slice(body).unwrap_or_default();
        let first = envelope.errors.first();
        if let Some(e) = first {
            if e.is_captcha() {
                return ApiError::CaptchaRequired;
            }
            // Roblox returns "Incorrect username or password" (code 1) with
            // either 401 or 403 depending on the endpoint/flow.
            if e.code == 1 {
                return ApiError::InvalidCredentials;
            }
            if status == 429 {
                return ApiError::RateLimited { retry_after: None };
            }
            return ApiError::Status { code: status, message: e.message.clone() };
        }
        // Some error bodies are flat: {"code":0,"message":"Token Validation Failed"}
        let flat: Result<crate::roblox::types::ApiErrorBody, _> = serde_json::from_slice(body);
        if let Ok(e) = flat {
            if e.is_captcha() {
                return ApiError::CaptchaRequired;
            }
            if status == 429 {
                return ApiError::RateLimited { retry_after: None };
            }
            return ApiError::Status { code: status, message: e.message };
        }
        if text.trim().is_empty() {
            ApiError::Status { code: status, message: String::new() }
        } else {
            ApiError::Status { code: status, message: text.chars().take(160).collect() }
        }
    }
}
