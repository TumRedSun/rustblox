//! Live game join flow — the real sequence the official client performs to
//! enter a running place instance on Roblox's game servers.
//!
//! Steps:
//!  1. POST `auth.roblox.com/v1/authentication-ticket` — exchange the
//!     `.ROBLOSECURITY` session for a short-lived join ticket. The request
//!     needs the session cookie + CSRF dance + a roblox.com `Referer`; the
//!     answer is HTTP 200 with an EMPTY body and the ticket inside the
//!     `rbx-authentication-ticket` RESPONSE HEADER.
//!  2. POST `gamejoin.roblox.com/v1/join-game` — request membership in a
//!     running server, authenticating with the session cookie + CSRF under a
//!     client User-Agent ("Roblox/WinInet"). Success answers
//!     `{"jobId": "…", "status": 2, "joinScript": {…}}` with the game-server
//!     endpoint inside the joinScript object.
//!  3. (fallbacks) the same join without the ticket, then
//!     `gamejoin.roblox.com/v1/join-game-instance`.
//!
//! The UDP handshake with the returned server then happens in
//! `crate::raknet` + `crate::game`.

use crate::roblox::client::{CLIENT_UA, RobloxClient};
use crate::roblox::errors::ApiError;
use serde::{Deserialize, Deserializer};
use std::borrow::Cow;

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
    /// Where to exchange the join ticket for a client ticket (modern flow).
    #[serde(default, deserialize_with = "null_to_default", alias = "authentication_url")]
    pub authentication_url: String,
    /// One-time ticket handed back for the client-auth exchange.
    #[serde(default, deserialize_with = "null_to_default", alias = "authentication_ticket")]
    pub authentication_ticket: String,
    #[serde(default, deserialize_with = "null_to_default", alias = "data_center_id")]
    pub data_center_id: u64,
    #[serde(default, deserialize_with = "null_to_default", alias = "membership_type")]
    pub membership_type: i32,
    #[serde(default, deserialize_with = "null_to_default", alias = "is_playtesting_game")]
    pub is_playtesting_game: bool,
    /// v2 responses name the transport (e.g. "RakNet").
    #[serde(default, deserialize_with = "null_to_default", alias = "connection_type")]
    pub connection_type: String,
    /// UDMUX proxy endpoint from the join script (`UdmuxEndpoints[0]`) when
    /// Roblox supplies one — the real client dials THIS for private RCC
    /// addresses, not a guessed proxy IP.
    #[serde(default, deserialize_with = "null_to_default")]
    pub udmux_host: String,
    /// Per-session udmux routing token (`TokenValue` / `UdmuxToken`) and its
    /// pepper (`PepperId`). Carried for the handshake wrapper and diagnostics.
    #[serde(default, deserialize_with = "null_to_default")]
    pub udmux_token: String,
    #[serde(default, deserialize_with = "null_to_default")]
    pub udmux_pepper: u64,
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

/// Locate the authentication ticket in a successful /v1/authentication-ticket
/// response. Priority: the `rbx-authentication-ticket` header (the only place
/// the live endpoint puts it today), then a JSON body ticket field, then a
/// legacy plain-text body. A JSON body WITHOUT a ticket field (the documented
/// `{}` 200 body) is None — returning the body itself as the ticket is exactly
/// the bug that shipped "{}" to gamejoin and drew refusal status 12.
fn ticket_from_parts<'a>(header: Option<&'a str>, body: &'a str) -> Option<Cow<'a, str>> {
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

/// Fetch the one-time authentication ticket used by game join.
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
        &[
            ("Referer", referer.as_str()),
            ("Origin", "https://www.roblox.com"),
        ],
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

/// A fresh `gameJoinAttemptId` (UUID-v4-shaped). The official client tags
/// every join attempt with one; the value is informational but matching the
/// client shape costs nothing. std-only xorshift — no uuid dependency.
fn join_attempt_id() -> String {
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

/// Join a running place server: authentication ticket, then the game-join
/// service. The classic `/v1/join-game` is the primary; `/v1/join-game-instance`
/// (newer unified API) is the final fallback. NOTE: `/v2/join-game` does NOT
/// exist on gamejoin.roblox.com (probed 2026-11: 404) — never use it.
pub fn join_game(client: &mut RobloxClient, place_id: u64) -> Result<JoinResponse, ApiError> {
    crate::netlog!(info, "join: requesting server for place {}", place_id);
    let ticket = match fetch_auth_ticket(client, place_id) {
        Ok(t) => Some(t),
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
            crate::netlog!(info, "join: auth ticket unavailable ({}); trying cookie-only join", e);
            None
        }
    };
    let referer = format!("https://www.roblox.com/games/{}/", place_id);
    // Modern client shape (RoZod gamejoinv1 schema + live client traffic):
    // the join is authenticated by the session cookie + CSRF under a client
    // User-Agent. The authenticationTicket rides along when we have one —
    // what the official client passes in via the roblox-player URI.
    let base = serde_json::json!({
        "placeId": place_id,
        "isTeleport": false,
        "isImmersiveAdsTeleport": false,
        "gameJoinAttemptId": join_attempt_id(),
    });
    let with_ticket = if let Some(t) = &ticket {
        let mut b = base.clone();
        b["authenticationTicket"] = serde_json::Value::String(t.clone());
        b
    } else {
        base.clone()
    };
    let first_err = match join_request(client, "/v1/join-game", &with_ticket, &referer) {
        Ok(resp) => return Ok(resp),
        Err(e) => e,
    };
    crate::netlog!(info, "join-game v1 failed ({}); trying fallbacks", first_err);
    // Fallback 1: drop the ticket — public client implementations join with
    // cookie-only auth, so retry that way in case the server disliked ours.
    if ticket.is_some() {
        let mut cookie_only = base.clone();
        cookie_only["authenticationTicket"] = serde_json::Value::Null;
        match join_request(client, "/v1/join-game", &cookie_only, &referer) {
            Ok(resp) => return Ok(resp),
            Err(e) => crate::netlog!(info, "join-game without ticket failed ({}); trying join-game-instance", e),
        }
    }
    // Fallback 2: the newer unified endpoint.
    let mut body2 = base;
    body2["authenticationTicket"] = ticket
        .clone()
        .map(serde_json::Value::String)
        .unwrap_or(serde_json::Value::Null);
    join_request(client, "/v1/join-game-instance", &body2, &referer).map_err(|e| {
        crate::netlog!(info, "join-game-instance failed too: {}", e);
        if matches!(e, ApiError::JoinRefused(_)) { e } else { first_err }
    })
}

/// Best-effort hint for the numeric v1/v2 join status. Only codes that are
/// actually attested (devforum reports + client-emulation sources we could
/// inspect, which mirror the legacy PlaceLauncher codes) get a name; unknown
/// ones stay numeric.
fn join_status_hint(status: i64) -> Option<&'static str> {
    match status {
        // Repro: POST /v1/join-game without an authentication ticket answers
        // {"status":12,"message":"Unable to join Game 211"}.
        12 => Some("join refused — the request carried no valid authentication ticket"),
        1 => Some("queued — no server available yet"),
        6 => Some("the server is full"),
        14 => Some("invalid, private or unavailable place"),
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
    // manually through the same agent/cookie machinery. The join service
    // only accepts client-shaped traffic: under a browser User-Agent it
    // refuses with status 12 even when the session is valid.
    let url = format!("{}{}", GAMEJOIN_BASE, path);
    let raw = client.post_with_headers(
        &url,
        body,
        &[
            ("User-Agent", CLIENT_UA),
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
    // Shape dump BEFORE the typed parse: names only (the values carry auth
    // tickets), so the next failing join shows exactly which fields Roblox
    // sent — new endpoint/udmux fields are visible without another round trip.
    dump_join_shape(&v);
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
    // Udmux routing material straight from the join script — the real client
    // wraps every RakNet packet with a token-derived header, so without these
    // fields the proxy silently drops our handshake (observed 2026-11).
    if let Some(js) = v.get("joinScript").and_then(|j| j.as_object()) {
        let (host, token, pepper) = extract_udmux(js);
        if let Some(h) = &host {
            crate::netlog!(info, "join: udmux endpoint from join script: {}", h);
            if resp.udmux_host.is_empty() {
                resp.udmux_host = h.clone();
                if resp.port == 0 {
                    resp.port = h.rsplit(':').next().and_then(|p| p.parse().ok()).unwrap_or(0);
                }
            }
        }
        if let Some(t) = &token {
            // Shape diagnostics for the routing token: 24 base64 chars are
            // 18 raw bytes unpadded but 16 with "==" — and 16 is exactly the
            // RUPP epoch-tag width the proxy routes on.
            let dec = crate::roblox::udmux::b64_decode(t);
            crate::netlog!(
                info,
                "join: udmux token present ({} chars, b64 -> {} bytes, padded={})",
                t.len(),
                dec.len(),
                t.ends_with('=')
            );
            if resp.udmux_token.is_empty() {
                resp.udmux_token = t.clone();
            }
        }
        if let Some(a) = js.get("TokenGenAlgorithm").and_then(|v| v.as_u64()) {
            crate::netlog!(info, "join: udmux token-gen algorithm {}", a);
        }
        if let Some(p) = pepper {
            crate::netlog!(info, "join: udmux pepper {}", p);
            resp.udmux_pepper = p;
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

/// Pull the udmux routing material out of a join-script JSON object:
/// `UdmuxEndpoints[0].Address:Port`, the routing token (`TokenValue` or
/// legacy `UdmuxToken`) and `PepperId`. All optional.
fn extract_udmux(
    js: &serde_json::Map<String, serde_json::Value>,
) -> (Option<String>, Option<String>, Option<u64>) {
    let mut host = None;
    if let Some(ep) = first_endpoint_list(js.get("UdmuxEndpoints")) {
        host = Some(if ep.1 > 0 { format!("{}:{}", ep.0, ep.1) } else { ep.0 });
    }
    let token = ["TokenValue", "UdmuxToken"]
        .iter()
        .find_map(|k| js.get(*k).and_then(|t| t.as_str()).map(str::to_string))
        .filter(|t| !t.is_empty());
    let pepper = js.get("PepperId").and_then(|p| p.as_u64());
    (host, token, pepper)
}

/// Log name lists in chunks so the 240-char log-line clamp never eats the
/// tail — the truncated key list is exactly what hid the udmux fields from us.
fn log_name_chunks(prefix: &str, names: &[String]) {
    if names.is_empty() {
        return;
    }
    const PER_LINE: usize = 12;
    let mut sorted = names.to_vec();
    sorted.sort();
    let lines: Vec<&[String]> = sorted.chunks(PER_LINE).collect();
    for (i, chunk) in lines.iter().enumerate() {
        crate::netlog!(info, "{}/{}: {}", prefix, lines.len(), chunk.join(", "));
    }
}

/// Token-ish join fields are logged as lengths only — the values are bearer
/// credentials, everything else (addresses, ports, booleans) is printable.
fn is_secret_join_field(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    ["ticket", "token", "session", "cookie", "pubkey", "pepper", "security", "seed"]
        .iter()
        .any(|s| k.contains(s))
}

/// Presence + safe values for every field that matters to the connection
/// layer, so one log shows exactly which endpoint dialect a join used.
fn log_join_object_shape(obj: &serde_json::Map<String, serde_json::Value>) {
    const INTERESTING: [&str; 22] = [
        "MachineAddress", "ClientPort", "ServerPort", "ServerConnections",
        "UdmuxEndpoints", "UdmuxToken", "TokenValue", "PepperId",
        "DirectServerReturn", "DataCenterId", "NetStackConfig", "NetStackPort",
        "NetStackTokenValue", "GameFqdn", "gameFqdn", "EphemeralEarlyPubKey",
        "ClientPublicKeyData", "JobId", "GameId", "SeleniumTestMode",
        "IsLocalDebugRcc", "JoinRequestType",
    ];
    for key in INTERESTING {
        let Some(val) = obj.get(key) else { continue };
        match val {
            serde_json::Value::String(s) => {
                if is_secret_join_field(key) || s.chars().count() > 60 {
                    crate::netlog!(info, "join shape: {} = <str len {}>", key, s.len());
                } else {
                    crate::netlog!(info, "join shape: {} = {:?}", key, s);
                }
            }
            serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
                crate::netlog!(info, "join shape: {} = {}", key, val);
            }
            other => crate::netlog!(info, "join shape: {} = {}", key, other),
        }
    }
}

/// Log the SHAPE of a join response — top-level keys plus the join script's
/// field names (XML tags or JSON keys). Names only, never values: the script
/// carries authentication tickets. This makes a failed join diagnosable from
/// the log alone — if Roblox starts handing out a udmux proxy address (or any
/// other endpoint dialect) the next log shows the exact field name to parse.
fn dump_join_shape(v: &serde_json::Value) {
    let top: Vec<String> = v
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    log_name_chunks("join response fields", &top);

    let Some(js) = v.get("joinScript") else { return };
    if let Some(s) = js.as_str() {
        let trimmed = s.trim_start();
        if trimmed.starts_with('{') {
            if let Ok(jv) = serde_json::from_str::<serde_json::Value>(trimmed) {
                let keys: Vec<String> = jv
                    .as_object()
                    .map(|o| o.keys().cloned().collect())
                    .unwrap_or_default();
                log_name_chunks(&format!("join script (json, {} bytes) keys", s.len()), &keys);
                if let Some(obj) = jv.as_object() {
                    log_join_object_shape(obj);
                }
                return;
            }
        }
        let tags = xml_tag_names(s);
        if !tags.is_empty() {
            log_name_chunks(&format!("join script (xml, {} bytes) tags", s.len()), &tags);
        } else {
            crate::netlog!(info, "join script: {} bytes, no recognizable tags/keys", s.len());
        }
    } else if let Some(obj) = js.as_object() {
        let keys: Vec<String> = obj.keys().cloned().collect();
        log_name_chunks("join script (object) keys", &keys);
        log_join_object_shape(obj);
    }
}

/// Distinct tag names of an XML-ish blob, in order of first appearance
/// (capped so a pathological script can't flood the log).
fn xml_tag_names(blob: &str) -> Vec<String> {
    let bytes = blob.as_bytes();
    let mut tags: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() && tags.len() < 60 {
        if bytes[i] == b'<' {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            if j > start && j < bytes.len() && (bytes[j] == b'>' || bytes[j] == b' ' || bytes[j] == b'/') {
                let name = blob[start..j].to_string();
                if !tags.iter().any(|t| t == &name) {
                    tags.push(name);
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    tags
}

/// Pull the game-server endpoint out of a join response that doesn't carry
/// top-level `ip`/`port`. Two dialects exist:
///  * modern: `joinScript` is a JSON OBJECT with `ServerConnections` /
///    `UdmuxEndpoints` arrays and flat `MachineAddress` + `ServerPort`;
///  * classic: `joinScript` (or `joinTicket`) is a STRING holding an XML-ish
///    blob (`<MachineAddress>…</MachineAddress>`) or flat JSON keys.
fn extract_endpoint(v: &serde_json::Value) -> Option<(String, u16)> {
    if let Some(js) = v.get("joinScript") {
        if js.is_object() {
            for key in ["ServerConnections", "UdmuxEndpoints"] {
                if let Some(ep) = first_endpoint_list(js.get(key)) {
                    return Some(ep);
                }
            }
            if let Some(ip) = js.get("MachineAddress").and_then(|s| s.as_str()) {
                for pk in ["ServerPort", "MachinePort", "ClientPort"] {
                    if let Some(port) = js.get(pk).and_then(|p| p.as_u64()) {
                        if !ip.is_empty() && port > 0 && port <= u64::from(u16::MAX) {
                            return Some((ip.to_string(), port as u16));
                        }
                    }
                }
            }
        }
    }
    // Classic dialect: collect candidate strings — the inline joinScript plus
    // the URL one.
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

/// First usable (Address, Port) pair of a joinScript endpoint array.
fn first_endpoint_list(v: Option<&serde_json::Value>) -> Option<(String, u16)> {
    let arr = v?.as_array()?;
    for item in arr {
        let ip = item.get("Address").and_then(|s| s.as_str()).unwrap_or("");
        let port = item.get("Port").and_then(|p| p.as_u64()).unwrap_or(0);
        if !ip.is_empty() && port > 0 && port <= u64::from(u16::MAX) {
            return Some((ip.to_string(), port as u16));
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

    #[test]
    fn ticket_from_header_wins() {
        // The live endpoint: empty {} body, ticket in the response header.
        assert_eq!(ticket_from_parts(Some(" T-HEADER "), "{}"), Some("T-HEADER".into()));
    }

    #[test]
    fn ticket_from_body_field_fallback() {
        assert_eq!(ticket_from_parts(None, r#"{"ticket":"T-BODY"}"#), Some("T-BODY".into()));
        assert_eq!(
            ticket_from_parts(None, r#"{"authenticationTicket":"T-ALT"}"#),
            Some("T-ALT".into())
        );
    }

    #[test]
    fn ticket_empty_json_body_is_not_a_ticket() {
        // REGRESSION: the 200 body IS an empty object — returning it as the
        // ticket produced authenticationTicket: "{}" and join status 12.
        assert_eq!(ticket_from_parts(None, "{}"), None);
        assert_eq!(ticket_from_parts(Some("   "), "{}"), None);
        assert_eq!(ticket_from_parts(None, r#"{"ticket": null}"#), None);
        assert_eq!(ticket_from_parts(None, r#"{"ticket": ""}"#), None);
        assert_eq!(ticket_from_parts(None, ""), None);
        assert_eq!(ticket_from_parts(None, "<html>error</html>"), None);
        // Legacy dialect: bare plain-text ticket.
        assert_eq!(ticket_from_parts(None, "PLAINTICKET123"), Some("PLAINTICKET123".into()));
    }

    #[test]
    fn extract_endpoint_from_join_script_object() {
        // Modern dialect: joinScript is a JSON object (RoZod gamejoinv1
        // schema). ServerConnections first.
        let v: serde_json::Value = serde_json::from_str(
            r#"{"jobId":"j1","status":2,"joinScript":{"MachineAddress":"203.0.113.9","ServerPort":20005,
                "ServerConnections":[{"Address":"203.0.113.9","Port":20005}]}}"#,
        )
        .unwrap();
        assert_eq!(extract_endpoint(&v), Some(("203.0.113.9".into(), 20005)));
        // Udmux layer only.
        let v2: serde_json::Value = serde_json::from_str(
            r#"{"jobId":"j2","status":2,"joinScript":{"UdmuxEndpoints":[{"Address":"198.51.100.4","Port":20019}]}}"#,
        )
        .unwrap();
        assert_eq!(extract_endpoint(&v2), Some(("198.51.100.4".into(), 20019)));
        // Flat legacy keys inside the object.
        let v3: serde_json::Value = serde_json::from_str(
            r#"{"jobId":"j3","status":2,"joinScript":{"MachineAddress":"10.1.2.3","ServerPort":20005}}"#,
        )
        .unwrap();
        assert_eq!(extract_endpoint(&v3), Some(("10.1.2.3".into(), 20005)));
        // Object without any endpoint information.
        let v4: serde_json::Value = serde_json::from_str(
            r#"{"jobId":"j4","status":2,"joinScript":{"UserId":1,"BaseUrl":"https://www.roblox.com"}}"#,
        )
        .unwrap();
        assert_eq!(extract_endpoint(&v4), None);
    }

    #[test]
    fn extract_udmux_from_join_script_object() {
        // Field set mirrored from a real TeamTest join script (devforum
        // 4580722): UdmuxEndpoints + TokenValue + PepperId + DSR.
        let v: serde_json::Value = serde_json::from_str(
            r#"{"MachineAddress":"10.186.12.41","ServerPort":53928,"ClientPort":0,
                "ServerConnections":[{"Address":"10.186.12.41","Port":53928}],
                "UdmuxEndpoints":[{"Address":"128.116.48.33","Port":53928}],
                "DirectServerReturn":true,"PepperId":1776353748,
                "TokenValue":"SECRET-TOKEN","NetStackPort":54115,"NetStackTokenValue":""}"#,
        )
        .unwrap();
        let js = v.as_object().unwrap();
        let (host, token, pepper) = extract_udmux(js);
        assert_eq!(host.as_deref(), Some("128.116.48.33:53928"));
        assert_eq!(token.as_deref(), Some("SECRET-TOKEN"));
        assert_eq!(pepper, Some(1776353748));
        // Legacy token field name, no endpoints.
        let v2: serde_json::Value = serde_json::from_str(
            r#"{"UdmuxToken":"LEGACY","PepperId":7}"#,
        )
        .unwrap();
        let (host2, token2, pepper2) = extract_udmux(v2.as_object().unwrap());
        assert_eq!(host2, None);
        assert_eq!(token2.as_deref(), Some("LEGACY"));
        assert_eq!(pepper2, Some(7));
        // Nothing at all.
        let v3: serde_json::Value = serde_json::from_str(r#"{"UserId":1}"#).unwrap();
        let (host3, token3, pepper3) = extract_udmux(v3.as_object().unwrap());
        assert_eq!(host3, None);
        assert_eq!(token3, None);
        assert_eq!(pepper3, None);
    }

    #[test]
    fn refusal_status_hints() {
        let mk = |status: i64| {
            let v = serde_json::json!({"jobId": null, "status": status});
            join_refusal_detail(&v)
        };
        assert!(mk(12).starts_with("status 12 (join refused"));
        assert!(mk(1).starts_with("status 1 (queued"));
        assert!(mk(6).starts_with("status 6 (the server is full"));
        assert!(mk(14).starts_with("status 14 (invalid, private"));
        assert_eq!(mk(99), "status 99");
    }

    #[test]
    fn join_attempt_id_is_uuid_shaped() {
        let id = join_attempt_id();
        assert_eq!(id.len(), 36);
        assert_eq!(id.matches('-').count(), 4);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
        // Version 4 nibble and variant bits.
        assert_eq!(id.as_bytes()[14], b'4');
        assert!(matches!(id.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
        // Two calls in a row must not collide.
        assert_ne!(join_attempt_id(), join_attempt_id());
    }
}
