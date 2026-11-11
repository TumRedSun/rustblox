//! Live game join flow — the real sequence the official client performs to
//! enter a running place instance on Roblox's game servers.
//!
//! Steps:
//!  1. POST `auth.roblox.com/v1/authentication-ticket` — exchange the
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
/// The one-time join ticket lives on the auth service (NOT apis.roblox.com —
/// that host answers 404 for this path, which silently broke the whole join
/// chain: ticket 404 → anonymous v1 fallback → refusal status 12).
pub const AUTH_TICKET_URL: &str = "https://auth.roblox.com/v1/authentication-ticket";

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
    // The endpoint returns {"ticket": "..."} and expects an empty JSON
    // object (what the official client posts). The request must carry the
    // .ROBLOSECURITY session cookie + the CSRF dance — RobloxClient::send
    // does both.
    let _ = place_id;
    let body = serde_json::json!({});
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

/// Join a running place server: authentication ticket, then the game-join
/// service. The classic `/v1/join-game` is the primary (battle-tested with
/// tickets); `/v1/join-game-instance` (newer unified API, response carries
/// ip/port directly) is the fallback. NOTE: `/v2/join-game` does NOT exist
/// on gamejoin.roblox.com (probed 2026-11: 404) — never use it.
pub fn join_game(client: &mut RobloxClient, place_id: u64) -> Result<JoinResponse, ApiError> {
    crate::netlog!(info, "join: requesting server for place {}", place_id);
    let ticket = match fetch_auth_ticket(client, place_id) {
        Ok(t) => t,
        Err(e) => {
            // Without a session the ticket endpoint can never succeed and an
            // anonymous join is always refused (status 12, "Unable to join
            // Game ..."). Fail fast with an actionable message instead.
            if !client.has_session() {
                crate::netlog!(info, "join: no session cookie — cannot request an authentication ticket");
                return Err(ApiError::JoinRefused(
                    "not signed in: joining a server needs a session — sign in or add a .ROBLOSECURITY cookie first".into(),
                ));
            }
            crate::netlog!(info, "join: auth ticket unavailable ({}); trying v1 with the session cookie", e);
            String::new()
        }
    };
    let referer = format!("https://www.roblox.com/games/{}/", place_id);
    // Classic endpoint: session cookie + ticket in the body.
    let mut body = serde_json::json!({
        "placeId": place_id,
        "isTeleport": false,
        "gameInfo": "{}",
    });
    if !ticket.is_empty() {
        body["authenticationTicket"] = serde_json::Value::String(ticket.clone());
    }
    match join_request(client, "/v1/join-game", &body, &referer) {
        Ok(resp) => Ok(resp),
        Err(classic_err) => {
            crate::netlog!(info, "join-game v1 failed ({}); trying join-game-instance", classic_err);
            // Newer unified endpoint; its success body carries ip/port.
            let body2 = serde_json::json!({
                "placeId": place_id,
                "isTeleport": false,
                "authenticationTicket": if ticket.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(ticket) },
            });
            join_request(client, "/v1/join-game-instance", &body2, &referer)
                .map_err(|e| {
                    crate::netlog!(info, "join-game-instance failed too: {}", e);
                    if matches!(e, ApiError::JoinRefused(_)) { e } else { classic_err }
                })
        }
    }
}

/// Best-effort hint for the numeric v1/v2 join status. Only codes that are
/// actually attested (devforum reports + our own repro against the live
/// endpoint) get a name; unknown ones stay numeric.
fn join_status_hint(status: i64) -> Option<&'static str> {
    match status {
        // Repro: POST /v1/join-game without an authentication ticket answers
        // {"status":12,"message":"Unable to join Game 211"}.
        12 => Some("join refused — the request carried no valid authentication ticket"),
        _ => None,
    }
}

/// Human-readable reason from a refused join response. Roblox answers
/// {"jobId":null,"status":N,"message":"...","joinError":{"joinStatusCode":X,
/// "errorCode":Y,"description":"..."}} — with the description optionally
/// nested and the flat "message" only on v1.
fn join_refusal_detail(v: &serde_json::Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(s) = v.get("status").and_then(|s| s.as_i64()) {
        parts.push(match join_status_hint(s) {
            Some(hint) => format!("status {} ({})", s, hint),
            None => format!("status {}", s),
        });
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
    // v1 refusals carry a flat top-level "message" ("Unable to join Game 211").
    let top_msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("");
    if !top_msg.is_empty() && top_msg != desc {
        parts.push(format!("\"{}\"", top_msg));
    }
    if parts.is_empty() {
        "no reason given (jobId is null)".into()
    } else {
        parts.join(", ")
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
        crate::netlog!(info, "join: server refused the join: {}", detail);
        return Err(ApiError::JoinRefused(detail));
    }
    // Accepted. The classic endpoint often hides the server address inside
    // the join-script blob instead of top-level ip/port — dig it out.
    let mut resp = resp;
    if resp.server_addr().is_none() {
        if let Some((ip, port)) = extract_endpoint(&v) {
            crate::netlog!(info, "join: endpoint recovered from join script: {}:{}", ip, port);
            resp.ip = ip;
            resp.port = port;
        }
    }
    if resp.server_addr().is_none() {
        return Err(ApiError::Parse(format!(
            "server accepted the join but returned no server address (jobId {})",
            resp.job_id
        )));
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

/// Pull the game-server endpoint out of a join response that doesn't carry
/// top-level `ip`/`port`. The classic join flow embeds it in the join script
/// (an XML-ish blob) as `<MachineAddress>x.x.x.x</MachineAddress>` /
/// `<MachinePort>20005</MachinePort>`, or as flat JSON keys. Best-effort.
fn extract_endpoint(v: &serde_json::Value) -> Option<(String, u16)> {
    // Collect candidate strings: the inline joinScript plus the URL one.
    let mut blobs: Vec<String> = Vec::new();
    for key in ["joinScript", "joinScriptUrl", "joinTicket"] {
        if let Some(s) = v.get(key).and_then(|s| s.as_str()) {
            blobs.push(s.to_string());
        }
    }
    for blob in &blobs {
        let Some(ip) = find_tag(blob, "MachineAddress")
            .or_else(|| find_json_str(blob, "ip"))
            .or_else(|| find_json_str(blob, "address"))
        else {
            continue;
        };
        let Some(port) = find_tag(blob, "MachinePort")
            .and_then(|p| p.parse::<u16>().ok())
            .or_else(|| find_json_num(blob, "port"))
        else {
            continue;
        };
        if !ip.is_empty() && port > 0 {
            return Some((ip, port));
        }
    }
    None
}

/// Value of `<Tag>value</Tag>` (lenient about the exact join-script dialect).
fn find_tag(hay: &str, tag: &str) -> Option<String> {
    let close = format!("</{}>", tag);
    // Plain form: <Tag>value</Tag>
    let open = format!("<{}>", tag);
    if let Some(a) = hay.find(&open) {
        let after = &hay[a + open.len()..];
        let value = match after.find(&close) {
            Some(b) => &after[..b],
            None => after.split(['<', '"']).next().unwrap_or(""),
        };
        let value = value.trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    // Attribute form: <Tag attr="...">value</Tag>
    let open2 = format!("<{} ", tag);
    if let Some(a) = hay.find(&open2) {
        let rest = &hay[a + open2.len()..];
        let end = rest.find('>')?;
        if !rest[..end].ends_with('/') {
            let after = &rest[end + 1..];
            let value = match after.find(&close) {
                Some(b) => &after[..b],
                None => after.split(['<', '"']).next().unwrap_or(""),
            };
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Value of `"key":"value"` in a JSON-ish string.
fn find_json_str(hay: &str, key: &str) -> Option<String> {
    let pat = format!("\"{}\"", key);
    let a = hay.find(&pat)?;
    let rest = &hay[a + pat.len()..];
    let colon = rest.find(':')?;
    let rest = rest[colon + 1..].trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    let value = rest[..end].trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// Numeric value of `"key":123` in a JSON-ish string.
fn find_json_num(hay: &str, key: &str) -> Option<u16> {
    let pat = format!("\"{}\"", key);
    let a = hay.find(&pat)?;
    let rest = &hay[a + pat.len()..];
    let colon = rest.find(':')?;
    let rest = rest[colon + 1..].trim_start();
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
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
    fn join_response_tolerates_nulls() {
        // Roblox answers {"jobId":null,...} when it refuses the join; the
        // typed struct must still parse (the refusal is reported separately).
        let v: serde_json::Value = serde_json::from_str(
            r#"{"jobId":null,"status":5,"joinError":{"joinStatusCode":6,"errorCode":6,"description":"You cannot join this experience"}}"#,
        )
        .unwrap();
        let resp: JoinResponse = serde_json::from_value(v.clone()).unwrap();
        assert!(resp.job_id.is_empty());
        assert_eq!(join_refusal_detail(&v), "status 5, joinStatusCode=6, errorCode=6, \"You cannot join this experience\"");
    }

    #[test]
    fn refusal_detail_v1_flat_code() {
        let v: serde_json::Value =
            serde_json::from_str(r#"{"jobId":null,"joinErrorCode":8}"#).unwrap();
        assert_eq!(join_refusal_detail(&v), "joinErrorCode=8");
        let empty: serde_json::Value = serde_json::json!({"jobId": null});
        assert_eq!(join_refusal_detail(&empty), "no reason given (jobId is null)");
    }

    #[test]
    fn refusal_detail_unauthenticated_v1() {
        // The exact body an anonymous /v1/join-game request gets (repro'd
        // against the live endpoint): status 12 + flat message.
        let v: serde_json::Value = serde_json::from_str(
            r#"{"jobId":null,"status":12,"statusData":null,"joinScriptUrl":null,"authenticationUrl":null,"authenticationTicket":null,"message":"Unable to join Game 211","joinScript":null,"joinTicket":null,"queuePosition":0}"#,
        )
        .unwrap();
        assert_eq!(
            join_refusal_detail(&v),
            "status 12 (join refused — the request carried no valid authentication ticket), \"Unable to join Game 211\""
        );
    }

    #[test]
    fn endpoint_extracted_from_join_script_variants() {
        // Classic XML-ish join script.
        let xml = r#"{"jobId":"j1","status":0,"joinScript":"<JoinScript><MachineAddress>203.0.113.7</MachineAddress><MachinePort>20005</MachinePort></JoinScript>"}"#;
        let v: serde_json::Value = serde_json::from_str(xml).unwrap();
        assert_eq!(extract_endpoint(&v), Some(("203.0.113.7".into(), 20005)));
        // JSON keys inside the blob.
        let js = r#"{"jobId":"j2","status":0,"joinScript":"{\"ip\":\"198.51.100.3\",\"port\":20005}"}"#;
        let v2: serde_json::Value = serde_json::from_str(js).unwrap();
        assert_eq!(extract_endpoint(&v2), Some(("198.51.100.3".into(), 20005)));
        // Top-level ip/port make the extractor unnecessary, but it must not
        // misfire on blobs without any endpoint.
        let none: serde_json::Value =
            serde_json::from_str(r#"{"jobId":"j3","status":0,"joinScript":"<JoinScript><JobId>abc</JobId></JoinScript>"}"#).unwrap();
        assert_eq!(extract_endpoint(&none), None);
        let empty: serde_json::Value = serde_json::from_str(r#"{"jobId":"j4"}"#).unwrap();
        assert_eq!(extract_endpoint(&empty), None);
    }

    #[test]
    fn accepted_join_without_address_is_an_error() {
        // A success body without any recoverable endpoint must produce the
        // clear "no server address" error path (via server_addr being None).
        let j: JoinResponse = serde_json::from_str(r#"{"jobId":"abc"}"#).unwrap();
        assert_eq!(j.server_addr(), None);
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
