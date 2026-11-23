//! HTTP client for Roblox web APIs.
//!
//! Mirrors what the official site does:
//!  * every mutating request carries `X-CSRF-Token`;
//!  * a 403 "Token Validation Failed" response includes a fresh
//!    `x-csrf-token` header — the token is rotated and the request retried;
//!  * the authenticated session is the `.ROBLOSECURITY` cookie sent as
//!    `Cookie: .ROBLOSECURITY=<token>` on every request.
//!
//! Layout: `request.rs` holds the `RobloxClient` request engine and its CSRF
//! retry dance, `raw.rs` the response wrapper (headers + body), `cookie.rs`
//! the normalizer for pasted session cookies.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod cookie;
mod raw;
mod request;

#[cfg(test)]
#[path = "tests.rs"]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests;

pub use cookie::normalize_cookie_token;
pub use raw::RawResponse;
pub use request::RobloxClient;

/// Browser User-Agent used for web API traffic (the site-facing identity).
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36 RustBlox/0.1.0";
/// The User-Agent the official game client sends to gamejoin.roblox.com
/// ("WinInet" = the Windows HTTP stack the client uses). The join service
/// refuses browser-shaped requests with status 12 even when the session is
/// valid, so game-join traffic must present this identity.
pub const CLIENT_UA: &str = "Roblox/WinInet";

/// Auth API host (login, 2FA, authentication tickets).
pub const AUTH_BASE: &str = "https://auth.roblox.com";
/// Users API host (profiles, batch details, search).
pub const USERS_BASE: &str = "https://users.roblox.com";
/// Friends API host (friend list, requests).
pub const FRIENDS_BASE: &str = "https://friends.roblox.com";
/// Presence API host (in-game status, last online).
pub const PRESENCE_BASE: &str = "https://presence.roblox.com";
/// Thumbnails API host (avatar headshots, place icons).
pub const THUMBS_BASE: &str = "https://thumbnails.roblox.com";
/// Games API host (universe names, place details).
pub const GAMES_BASE: &str = "https://games.roblox.com";
/// Modern discovery/search APIs (omni-search, omni-recommendation).
pub const APIS_BASE: &str = "https://apis.roblox.com";
