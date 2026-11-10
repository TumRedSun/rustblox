//! Live game join flow — the real sequence the official client performs to
//! enter a running place instance on Roblox's game servers.
//!
//! Steps:
//!  1. POST `apis.roblox.com/authentication-ticket/v1` — exchange the
//!     `.ROBLOSECURITY` session for a short-lived join ticket.
//!  2. POST `gamejoin.roblox.com/v2/join-game` — request membership in a
//!     running server; the response names the datacenter host (ip/port),
//!     the job id and the avatar endpoint for our character.
//!  3. (fallback) POST `gamejoin.roblox.com/v1/join-game` — legacy endpoint
//!     that authenticates with the session cookie + referer directly.
//!
//! The UDP handshake with the returned server then happens in
//! `crate::raknet` + `crate::game`.

use crate::roblox::client::RobloxClient;
use crate::roblox::errors::ApiError;
use serde::{Deserialize, Deserializer};

pub const GAMEJOIN_BASE: &str = "https://gamejoin.roblox.com";
pub const AUTH_TICKET_URL: &str = "https://apis.roblox.com/authentication-ticket/v1";

/// serde: treat an explicit JSON null as the field default (Roblox returns
/// "jobId": null when it refuses a join; plain `#[serde(default)]` only
/// covers missing fields and rejects null).
fn null_to_default<'de, T, D>(d: D) -> Result<T, D::Error>
where
    T: Deserialize<'de> + Default,
    D: Deserializer<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// Result of a successful join request.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinResponse {
    #[serde(default, deserialize_with = "null_to_default")]
    pub job_id: String,
    #[serde(default, deserialize_with = "null_to_default", alias = "session_id")]
    pub session_id: String,
    /// Server hostname or IP of the game server.
    #[serde(default, deserialize_with = "null_to_default")]
    pub ip: String,
    #[serde(default, deserialize_with = "null_to_default")]
    pub port: u16,
    /// Where to fetch our avatar description (body colors, assets).
    #[serde(default, deserialize_with = "null_to_default", alias = "character_appearance_url")]
    pub character_appearance_url: String,
    #[serde(default, deserialize_with = "null_to_default", alias = "data_center_id")]
    pub data_center_id: u64,
    #[serde(default, deserialize_with = "null_to_default", alias = "membership_type")]
    pub membership_type: i32,
    #[serde(default, deserialize_with = "null_to_default", alias = "is_playtesting_game")]
    pub is_playtesting_game: bool,
    /// v2 responses name the transport (e.g. "RakNet").
    #[serde(default, deserialize_with = "null_to_default", alias = "connection_type")]
    pub connection_type: String,
}

impl JoinResponse {
    pub fn server_addr(&self) -> Option<(&str, u16)> {
        if self.ip.is_empty() || self.port == 0 {
            None
        } else {
            Some((self.ip.as_str(), self.port))
        }
    }
}

/// Truncate `text` for log lines (keep the head — status/code fields live there).
fn snippet(text: &str, max: usize) -> String {
    let t: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    if t.chars().count() <= max {
        t
    } else {
        let mut s: String = t.chars().take(max).collect();
        s.push_str("…");
        s
    }
}

/// Fetch the one-time authentication ticket used by v2 game join.
pub fn fetch_auth_ticket(client: &mut RobloxClient, place_id: u64) -> Result<String, ApiError> {
    // The endpoint returns {"ticket": "..."}.
    let body = serde_json::json!({
        "userUuid": uuid_v4_placeholder(),
        "placeId": place_id,
    });
    let raw = client.send(AUTH_TICKET_URL, Some(&body))?;
    if !(200..300).contains(&raw.status) {
        let err = raw.error();
        crate::netlog!(info, "auth ticket HTTP {}: {}", raw.status, err);
        return Err(err);
    }
    let text = String::from_utf8_lossy(&raw.body).to_string();
    crate::netlog!(info, "auth ticket {} -> {} bytes: {}", AUTH_TICKET_URL, raw.body.len(), snippet(&text, 200));
    let trimmed = text.trim().trim_matches('"').to_string();
    // Either plain text or JSON {"ticket": "..."}.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
        if let Some(t) = v.get("ticket").and_then(|t| t.as_str()) {
            return Ok(t.to_string());
        }
        if v.get("ticket").is_some() {
            crate::netlog!(info, "auth ticket body has null/empty ticket field");
        }
    }
    if trimmed.is_empty() || trimmed.starts_with('<') {
        return Err(ApiError::Parse(format!(
            "authentication-ticket returned unusable body ({} bytes)",
            text.len()
        )));
    }
    Ok(trimmed)
}

/// Join a running place server (v2 with ticket, falling back to v1).
pub fn join_game(client: &mut RobloxClient, place_id: u64) -> Result<JoinResponse, ApiError> {
    crate::netlog!(info, "join: requesting server for place {}", place_id);
    // The official client hits v2 with an authentication ticket.
    let ticket = match fetch_auth_ticket(client, place_id) {
        Ok(t) => t,
        Err(e) => {
            crate::netlog!(info, "join: auth ticket unavailable ({}); falling back to v1", e);
            String::new()
        }
    };
    if !ticket.is_empty() {
        let body = serde_json::json!({
            "placeId": place_id,
            "isTeleport": false,
            "teleportData": serde_json::Value::Null,
            "authenticationTicket": ticket,
            "gameInfo": "{}",
        });
        let referer = format!("https://www.roblox.com/games/{}/", place_id);
        match join_request(client, "/v2/join-game", &body, &referer) {
            Ok(resp) => return Ok(resp),
            Err(e) => {
                crate::netlog!(info, "join-game v2 failed ({}); trying v1", e);
            }
        }
    }
    // v1 legacy path: session cookie + referer.
    let body = serde_json::json!({
        "placeId": place_id,
        "isTeleport": false,
        "gameInfo": "{}",
    });
    let referer = format!("https://www.roblox.com/games/{}/", place_id);
    join_request(client, "/v1/join-game", &body, &referer)
}

/// Human-readable reason from a refused join response. Roblox answers
/// {"jobId":null,"status":N,"joinError":{"joinStatusCode":X,"errorCode":Y,
/// "description":"..."}} — with the description optionally nested.
fn join_refusal_detail(v: &serde_json::Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(s) = v.get("status").and_then(|s| s.as_i64()) {
        parts.push(format!("status {}", s));
    }
    let err = v.get("joinError");
    for key in ["joinStatusCode", "errorCode"] {
        if let Some(c) = err.and_then(|e| e.get(key)).and_then(|c| c.as_i64()) {
            parts.push(format!("{}={}", key, c));
        }
    }
    if let Some(c) = v.get("joinErrorCode").and_then(|c| c.as_i64()) {
        parts.push(format!("joinErrorCode={}", c));
    }
    let desc = err
        .and_then(|e| e.get("description"))
        .and_then(|d| d.as_str())
        .or_else(|| err.and_then(|e| e.get("message")).and_then(|d| d.as_str()))
        .unwrap_or("");
    if !desc.is_empty() {
        parts.push(format!("\"{}\"", desc));
    }
    if parts.is_empty() {
        "server refused the join (jobId is null)".into()
    } else {
        format!("server refused the join: {}", parts.join(", "))
    }
}

fn join_request(
    client: &mut RobloxClient,
    path: &str,
    body: &serde_json::Value,
    referer: &str,
) -> Result<JoinResponse, ApiError> {
    // RobloxClient.send() does not support custom headers, so do the POST
    // manually through the same agent/cookie machinery.
    let url = format!("{}{}", GAMEJOIN_BASE, path);
    let raw = client.post_with_headers(
        &url,
        body,
        &[
            ("Referer", referer),
            ("Origin", "https://www.roblox.com"),
        ],
    )?;
    let text = String::from_utf8_lossy(&raw.body).to_string();
    crate::netlog!(info, "{} -> HTTP {}: {}", url, raw.status, snippet(&text, 400));
    if !(200..300).contains(&raw.status) {
        return Err(raw.error());
    }
    // Parse through Value first: a refusal carries "jobId": null (which the
    // typed JoinResponse rejected with a useless serde message) plus the
    // joinError details we want to surface.
    let v: serde_json::Value = serde_json::from_str(text.trim())
        .map_err(|e| ApiError::Parse(format!("{} (body: {})", e, snippet(&text, 200))))?;
    let resp: JoinResponse = serde_json::from_value(v.clone())
        .map_err(|e| ApiError::Parse(format!("{} (body: {})", e, snippet(&text, 200))))?;
    if resp.job_id.is_empty() {
        let detail = join_refusal_detail(&v);
        crate::netlog!(info, "join: {}", detail);
        return Err(ApiError::Parse(detail));
    }
    crate::netlog!(
        info,
        "join: jobId={} ip={} port={} dc={}",
        resp.job_id,
        resp.ip,
        resp.port,
        resp.data_center_id
    );
    Ok(resp)
}

/// RFC-4122 v4-ish UUID from the OS RNG (used as a request correlation id).
fn uuid_v4_placeholder() -> String {
    let mut b = [0u8; 16];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut b))
        .is_err()
    {
        // Fallback: time-based entropy.
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        b[..16].copy_from_slice(&t.to_le_bytes()[..16]);
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = b.iter().map(|x| format!("{:02x}", x)).collect::<String>();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// Fetch the avatar description for our own character (body colors + asset
/// ids) from the join response URL (or the standard endpoint as fallback).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AvatarAppearance {
    #[serde(alias = "player_avatar_type")]
    pub player_avatar_type: String,
    #[serde(alias = "body_colors")]
    pub body_colors: BodyColors,
    #[serde(alias = "assets")]
    pub assets: Vec<AvatarAsset>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BodyColors {
    pub head_color_id: u64,
    pub torso_color_id: u64,
    pub left_arm_color_id: u64,
    pub right_arm_color_id: u64,
    pub left_leg_color_id: u64,
    pub right_leg_color_id: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AvatarAsset {
    pub id: u64,
    pub name: Option<String>,
    pub asset_type: Option<serde_json::Value>,
}

impl Default for AvatarAppearance {
    fn default() -> Self {
        AvatarAppearance {
            player_avatar_type: "R6".into(),
            body_colors: BodyColors::default(),
            assets: Vec::new(),
        }
    }
}

pub fn fetch_avatar_appearance(client: &mut RobloxClient, url: &str, user_id: u64) -> Result<AvatarAppearance, ApiError> {
    let url = if url.is_empty() {
        format!("https://avatar.roblox.com/v1/users/{}/avatar", user_id)
    } else {
        url.to_string()
    };
    client.get_json(&url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_response_parses() {
        let j: JoinResponse = serde_json::from_str(
            r#"{
                "jobId": "abc-123",
                "session_id": "s1",
                "ip": "10.0.0.1",
                "port": 20005,
                "characterAppearanceUrl": "https://avatar.roblox.com/x",
                "dataCenterId": 7,
                "connectionType": "RakNet"
            }"#,
        )
        .unwrap();
        assert_eq!(j.job_id, "abc-123");
        assert_eq!(j.server_addr(), Some(("10.0.0.1", 20005)));
    }

    #[test]
    fn join_response_snake_and_camel() {
        // v1 uses snake_case keys; v2 camelCase. Both must parse.
        let j1: JoinResponse = serde_json::from_str(
            r#"{"jobId":"j","session_id":"s","ip":"h","port":1,"characterAppearanceUrl":"u"}"#,
        )
        .unwrap();
        assert_eq!(j1.session_id, "s");
        let j2: JoinResponse = serde_json::from_str(
            r#"{"jobId":"j","sessionId":"s","ip":"h","port":1,"characterAppearanceUrl":"u"}"#,
        )
        .unwrap();
        assert_eq!(j2.session_id, "s");
    }

    #[test]
    fn appearance_parses() {
        let a: AvatarAppearance = serde_json::from_str(
            r#"{"playerAvatarType":"R15","bodyColors":{"headColorId":26,"torsoColorId":23},"assets":[{"id":1}]}"#,
        )
        .unwrap();
        assert_eq!(a.player_avatar_type, "R15");
        assert_eq!(a.body_colors.head_color_id, 26);
        assert_eq!(a.assets.len(), 1);
    }

    #[test]
    fn uuid_format() {
        let u = uuid_v4_placeholder();
        assert_eq!(u.len(), 36);
        assert_eq!(u.as_bytes()[14], b'4');
    }

    #[test]
    fn join_response_tolerates_nulls() {
        // Roblox answers {"jobId":null,...} when it refuses the join; the
        // typed struct must still parse (the refusal is reported separately).
        let v: serde_json::Value = serde_json::from_str(
            r#"{"jobId":null,"status":5,"joinError":{"joinStatusCode":6,"errorCode":6,"description":"You cannot join this experience"}}"#,
        )
        .unwrap();
        let resp: JoinResponse = serde_json::from_value(v.clone()).unwrap();
        assert!(resp.job_id.is_empty());
        assert_eq!(join_refusal_detail(&v), "server refused the join: status 5, joinStatusCode=6, errorCode=6, \"You cannot join this experience\"");
    }

    #[test]
    fn refusal_detail_v1_flat_code() {
        let v: serde_json::Value =
            serde_json::from_str(r#"{"jobId":null,"joinErrorCode":8}"#).unwrap();
        assert_eq!(join_refusal_detail(&v), "server refused the join: joinErrorCode=8");
        let empty: serde_json::Value = serde_json::json!({"jobId": null});
        assert_eq!(join_refusal_detail(&empty), "server refused the join (jobId is null)");
    }

    #[test]
    fn join_response_null_optional_fields() {
        let j: JoinResponse = serde_json::from_str(
            r#"{"jobId":"j","sessionId":null,"ip":null,"port":null,"characterAppearanceUrl":null,"connectionType":null}"#,
        )
        .unwrap();
        assert_eq!(j.job_id, "j");
        assert!(j.ip.is_empty());
        assert_eq!(j.port, 0);
    }
}
