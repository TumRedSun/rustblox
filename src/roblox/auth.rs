//! Authentication flows against the official Auth API v2 — the same endpoints
//! the Roblox website and official clients use.

use crate::model::UserProfile;
use crate::roblox::client::{normalize_cookie_token, RawResponse, RobloxClient, USERS_BASE};
use crate::roblox::errors::ApiError;
use crate::roblox::types::{AuthenticatedUser, FullUser, LoginResponse, TwoStepResendResponse};
use serde_json::json;

pub const LOGIN_URL: &str = concat!("https://auth.roblox.com", "/v2/login");
pub const VERIFY_URL: &str = concat!("https://auth.roblox.com", "/v2/twostepverification/verify");
pub const RESEND_URL: &str = concat!("https://auth.roblox.com", "/v2/twostepverification/resend");
pub const LOGOUT_URL: &str = concat!("https://auth.roblox.com", "/v2/logout");

#[derive(Debug)]
pub enum AuthOutcome {
    /// Logged in; carries the session token + profile.
    Session { security: String, profile: UserProfile },
    /// 2-step verification challenge issued.
    TwoStep { ticket: String, media_type: String },
}

fn profile_from(id: u64, name: String, display: Option<String>) -> UserProfile {
    let display_name = display.filter(|d| !d.is_empty()).unwrap_or_else(|| name.clone());
    UserProfile { id, name, display_name }
}

/// Fetch the full public profile (fills displayName).
pub fn fetch_profile(c: &mut RobloxClient, id: u64) -> Result<UserProfile, ApiError> {
    let url = format!("{}/v1/users/{}", USERS_BASE, id);
    let u: FullUser = c.get_json(&url)?;
    Ok(profile_from(u.id, u.name, u.display_name))
}

/// POST /v2/login with username/email + password.
pub fn login_password(c: &mut RobloxClient, login: &str, password: &str) -> Result<AuthOutcome, ApiError> {
    let body = json!({
        "ctype": "Username",
        "cvalue": login,
        "password": password,
    });
    let raw = c.post_raw(LOGIN_URL, &body)?;
    parse_login_response(c, raw)
}

fn parse_login_response(c: &mut RobloxClient, raw: RawResponse) -> Result<AuthOutcome, ApiError> {
    if !(200..300).contains(&raw.status) {
        // 2FA may arrive with 200 or 401 depending on API version — check the
        // body first before treating it as an error.
        let parsed: Result<LoginResponse, _> = raw.json();
        if let Ok(lr) = parsed {
            if let Some(t) = lr.two_step {
                if !t.ticket.is_empty() {
                    return Ok(AuthOutcome::TwoStep { ticket: t.ticket, media_type: t.media_type });
                }
            }
        }
        return Err(raw.error());
    }
    let lr: LoginResponse = raw.json()?;
    if let Some(t) = lr.two_step {
        if !t.ticket.is_empty() {
            return Ok(AuthOutcome::TwoStep { ticket: t.ticket, media_type: t.media_type });
        }
    }
    if let Some(user) = lr.user {
        if let Some(security) = raw.header_cookies_find(".ROBLOSECURITY") {
            // Keep the client's session field in sync with the cookie jar
            // (join flow checks it to fail fast with a clear message).
            c.set_security(&security);
            let profile = fetch_profile(c, user.id).unwrap_or_else(|_| profile_from(user.id, user.name.clone(), None));
            return Ok(AuthOutcome::Session { security, profile });
        }
        return Err(ApiError::Parse("login response did not contain a session cookie".into()));
    }
    Err(ApiError::Status { code: raw.status, message: "unexpected login response".into() })
}

/// POST /v2/twostepverification/verify — exchange the code for a session.
pub fn verify_2fa(
    c: &mut RobloxClient,
    login: &str,
    ticket: &str,
    code: &str,
    remember_device: bool,
) -> Result<AuthOutcome, ApiError> {
    let body = json!({
        "username": login,
        "ticket": ticket,
        "actionType": "Login",
        "code": code,
        "rememberDevice": remember_device,
    });
    let raw = c.post_raw(VERIFY_URL, &body)?;
    if !(200..300).contains(&raw.status) {
        return Err(match raw.error() {
            ApiError::Status { code, message } => ApiError::BadTwoStepCode(if message.is_empty() {
                format!("HTTP {}", code)
            } else {
                message
            }),
            other => other,
        });
    }
    match raw.header_cookies_find(".ROBLOSECURITY") {
        Some(security) => {
            // The verify response does not carry the user id; resolve it with
            // the fresh session.
            c.set_security(&security);
            let profile = whoami(c)?;
            Ok(AuthOutcome::Session { security, profile })
        }
        None => Err(ApiError::Parse("verification succeeded but no session cookie was returned".into())),
    }
}

/// POST /v2/twostepverification/resend — ask for a new code.
pub fn resend_2fa(c: &mut RobloxClient, login: &str, ticket: &str) -> Result<(String, String), ApiError> {
    let body = json!({
        "username": login,
        "ticket": ticket,
        "actionType": "Login",
    });
    let raw = c.post_raw(RESEND_URL, &body)?;
    if !(200..300).contains(&raw.status) {
        return Err(raw.error());
    }
    let rr: TwoStepResendResponse = raw.json()?;
    if rr.ticket.is_empty() {
        return Err(ApiError::Parse("resend response did not include a ticket".into()));
    }
    Ok((rr.ticket, rr.media_type))
}

/// Validate a `.ROBLOSECURITY` token and return the owning profile.
pub fn login_cookie(c: &mut RobloxClient, token: &str) -> Result<AuthOutcome, ApiError> {
    let token = normalize_cookie_token(token);
    if token.is_empty() {
        return Err(ApiError::InvalidCookie("empty token".into()));
    }
    c.set_security(&token);
    match c.get_json::<AuthenticatedUser>(&format!("{}/v1/users/authenticated", USERS_BASE)) {
        Ok(u) => {
            let profile = profile_from(u.id, u.name, u.display_name);
            Ok(AuthOutcome::Session { security: token, profile })
        }
        Err(e @ ApiError::Parse(_)) => Err(e),
        Err(e) => {
            c.clear_security();
            Err(ApiError::InvalidCookie(e.to_string()))
        }
    }
}

/// Resolve the authenticated user for the current session.
pub fn whoami(c: &mut RobloxClient) -> Result<UserProfile, ApiError> {
    let u: AuthenticatedUser = c.get_json(&format!("{}/v1/users/authenticated", USERS_BASE))?;
    Ok(profile_from(u.id, u.name, u.display_name))
}

/// Invalidate the session server-side (best effort).
pub fn logout(c: &mut RobloxClient) {
    if c.security.is_some() {
        let raw = c.post_raw(LOGOUT_URL, &json!({}));
        let _ = raw.map(|r| r.status);
    }
    c.clear_security();
}
