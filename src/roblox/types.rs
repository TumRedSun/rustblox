//! serde models for the official Roblox web APIs.
//!
//! Shapes verified against the live endpoints and public community
//! documentation of Auth v2 (e.g. the `rbxauth` wrapper).

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ApiErrorBody {
    #[serde(default)]
    pub code: i64,
    #[serde(default)]
    pub message: String,
}

impl ApiErrorBody {
    pub fn is_captcha(&self) -> bool {
        self.message.to_lowercase().contains("captcha")
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct ErrorsEnvelope {
    #[serde(default)]
    pub errors: Vec<ApiErrorBody>,
}

#[derive(Debug, Deserialize, Default)]
pub struct LoginUser {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub name: String,
}

/// Response of POST https://auth.roblox.com/v2/login
#[derive(Debug, Deserialize, Default)]
pub struct LoginResponse {
    #[serde(default)]
    pub user: Option<LoginUser>,
    #[serde(default, rename = "twoStepVerificationData")]
    pub two_step: Option<TwoStepData>,
    #[serde(default)]
    pub errors: Vec<ApiErrorBody>,
}

#[derive(Debug, Deserialize, Default)]
pub struct TwoStepData {
    #[serde(default, rename = "mediaType")]
    pub media_type: String,
    #[serde(default)]
    pub ticket: String,
}

/// Response of POST .../twostepverification/resend
#[derive(Debug, Deserialize, Default)]
pub struct TwoStepResendResponse {
    #[serde(default, rename = "mediaType")]
    pub media_type: String,
    #[serde(default)]
    pub ticket: String,
    #[serde(default)]
    pub errors: Vec<ApiErrorBody>,
}

/// GET https://users.roblox.com/v1/users/authenticated
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthenticatedUser {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

/// GET https://users.roblox.com/v1/users/{id}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FullUser {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub is_banned: Option<bool>,
}

/// GET https://friends.roblox.com/v1/users/{userId}/friends
///
/// NOTE (Oct 2024 API change): Roblox removed `name`, `displayName`,
/// `isBanned` etc. from this endpoint. The fields still appear but come back
/// EMPTY; real names must be resolved with POST users.roblox.com/v1/users
/// (see `social::fetch_user_details`).
#[derive(Debug, Deserialize, Default)]
pub struct FriendsResponse {
    #[serde(default)]
    pub data: Vec<FriendEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FriendEntry {
    pub id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub is_banned: Option<bool>,
}

/// POST https://users.roblox.com/v1/users — batch user details (names).
#[derive(Debug, Deserialize, Default)]
pub struct UsersBatchResponse {
    #[serde(default)]
    pub data: Vec<UserBatchEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserBatchEntry {
    pub id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub is_banned: Option<bool>,
    #[serde(default)]
    pub has_verified_badge: Option<bool>,
}

/// POST https://presence.roblox.com/v1/presence/users
#[derive(Debug, Deserialize, Default)]
pub struct PresenceResponse {
    #[serde(default, rename = "userPresences")]
    pub user_presences: Vec<PresenceEntry>,
}

#[derive(Debug, Deserialize)]
pub struct PresenceEntry {
    #[serde(rename = "userId")]
    pub user_id: u64,
    #[serde(rename = "userPresenceType", default)]
    pub presence_type: u8,
    #[serde(rename = "lastLocation", default)]
    pub last_location: Option<String>,
    #[serde(rename = "placeId", default)]
    pub place_id: Option<u64>,
    #[serde(rename = "rootPlaceId", default)]
    pub root_place_id: Option<u64>,
    #[serde(rename = "universeId", default)]
    pub universe_id: Option<u64>,
    #[serde(rename = "gameId", default)]
    pub game_id: Option<String>,
}

/// POST https://presence.roblox.com/v1/presence/last-online
#[derive(Debug, Deserialize, Default)]
pub struct LastOnlineResponse {
    #[serde(default, rename = "userLastOnlineTimestamps")]
    pub entries: Vec<LastOnlineEntry>,
}

#[derive(Debug, Deserialize)]
pub struct LastOnlineEntry {
    #[serde(rename = "userId")]
    pub user_id: u64,
    #[serde(rename = "lastOnline")]
    pub last_online: Option<String>,
}

/// GET https://thumbnails.roblox.com/v1/users/avatar-headshot?userIds=...
#[derive(Debug, Deserialize, Default)]
pub struct ThumbsResponse {
    #[serde(default)]
    pub data: Vec<ThumbEntry>,
}

#[derive(Debug, Deserialize)]
pub struct ThumbEntry {
    #[serde(rename = "targetId")]
    pub target_id: u64,
    #[serde(default)]
    pub state: String,
    #[serde(rename = "imageUrl", default)]
    pub image_url: Option<String>,
}

/// GET https://games.roblox.com/v1/games?universeIds=...
#[derive(Debug, Deserialize, Default)]
pub struct GamesResponse {
    #[serde(default)]
    pub data: Vec<GameEntry>,
}

#[derive(Debug, Deserialize)]
pub struct GameEntry {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub name: Option<String>,
}

/// GET https://users.roblox.com/v1/users/search?keyword=...&limit=25
#[derive(Debug, Deserialize, Default)]
pub struct UserSearchResponse {
    #[serde(default)]
    pub data: Vec<UserSearchEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserSearchEntry {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub display_name: Option<String>,
}
