//! Social data: friends, presence, game names, avatar thumbnails.
//! All requests are batched (<=100 ids per call) to minimize round trips.

use crate::model::{Friend, Presence, PresenceKind, UserEntry};
use crate::roblox::client::{RobloxClient, FRIENDS_BASE, GAMES_BASE, PRESENCE_BASE, THUMBS_BASE, USERS_BASE};
use crate::roblox::errors::ApiError;
use crate::roblox::types::{
    FriendsResponse, GamesResponse, LastOnlineResponse, PresenceResponse, ThumbsResponse, UserSearchResponse,
    UsersBatchResponse,
};
use serde_json::json;
use std::collections::HashMap;

const CHUNK: usize = 100;

/// Percent-encode a query-string component (RFC 3986 unreserved characters
/// stay readable, everything else is %XX-escaped byte-wise).
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            other => out.push_str(&format!("%{:02X}", other)),
        }
    }
    out
}

/// GET /v1/users/search?keyword=...&limit=25 — find users by name.
/// The endpoint requires at least 2 characters in the keyword.
pub fn search_users(c: &mut RobloxClient, keyword: &str, limit: u32) -> Result<Vec<UserEntry>, ApiError> {
    let url = format!(
        "{}/v1/users/search?keyword={}&limit={}",
        USERS_BASE,
        percent_encode(keyword.trim()),
        limit.clamp(1, 25)
    );
    let raw = c.send(&url, None)?;
    if !(200..300).contains(&raw.status) {
        return Err(raw.error());
    }
    let sr: UserSearchResponse = serde_json::from_slice(&raw.body)
        .map_err(|e| ApiError::Parse(format!("user search: {}", e)))?;
    let mut out: Vec<UserEntry> = sr
        .data
        .into_iter()
        .filter(|u| u.id != 0)
        .map(|u| UserEntry {
            id: u.id,
            name: u.name,
            display_name: u.display_name.filter(|d| !d.is_empty()).unwrap_or_default(),
        })
        .collect();
    out.dedup_by(|a, b| a.id == b.id);
    Ok(out)
}

/// POST /v1/friends/{userId}/request-friendship — send a friend request.
/// 200 = request sent (or already pending); 4xx carries a human message
/// ("Already friends", "Friend request already pending", …) which is shown
/// to the user as-is.
pub fn send_friend_request(c: &mut RobloxClient, user_id: u64) -> Result<(), ApiError> {
    let url = format!("{}/v1/friends/{}/request-friendship", FRIENDS_BASE, user_id);
    let raw = c.post_raw(&url, &json!({}))?;
    if (200..300).contains(&raw.status) {
        return Ok(());
    }
    Err(raw.error())
}

fn chunks(ids: &[u64]) -> impl Iterator<Item = &[u64]> {
    ids.chunks(CHUNK)
}

/// GET /v1/users/{userId}/friends
pub fn fetch_friends(c: &mut RobloxClient, user_id: u64) -> Result<Vec<Friend>, ApiError> {
    let url = format!("{}/v1/users/{}/friends", FRIENDS_BASE, user_id);
    let raw = c.send(&url, None)?;
    if !(200..300).contains(&raw.status) {
        return Err(raw.error());
    }
    // The endpoint returns {"data":[...],"nextPageCursor":...}; an erroring
    // context can also return an {"errors":[...]} envelope — parse defensibly.
    let fr: FriendsResponse = serde_json::from_slice(&raw.body)
        .map_err(|e| ApiError::Parse(format!("friends: {}", e)))?;
    let mut friends: Vec<Friend> = fr
        .data
        .into_iter()
        .map(|f| {
            let display_name = f
                .display_name
                .filter(|d| !d.is_empty())
                .unwrap_or_else(|| f.name.clone());
            Friend {
                id: f.id,
                name: f.name,
                display_name,
                is_banned: f.is_banned.unwrap_or(false),
            }
        })
        .collect();

    // Since Oct 2024 the friends endpoint no longer includes user names
    // (the fields come back empty). Resolve them with the batch users
    // endpoint; best effort — on failure the UI falls back to ids.
    if friends.iter().any(|f| f.name.is_empty() || f.display_name.is_empty()) {
        let ids: Vec<u64> = friends.iter().map(|f| f.id).collect();
        let details = fetch_user_details(c, &ids);
        for f in &mut friends {
            if let Some((name, display, banned)) = details.get(&f.id) {
                if f.name.is_empty() {
                    f.name = name.clone();
                }
                if f.display_name.is_empty() {
                    f.display_name = if display.is_empty() { name.clone() } else { display.clone() };
                }
                if *banned {
                    f.is_banned = true;
                }
            }
        }
        // Last-resort label so a row is never just "@".
        for f in &mut friends {
            if f.display_name.is_empty() {
                f.display_name = format!("User {}", f.id);
            }
            if f.name.is_empty() {
                f.name = f.id.to_string();
            }
        }
    }
    Ok(friends)
}

/// POST /v1/users — batch user details (<=100 ids per call). Returns
/// (username, display name, is_banned) keyed by user id.
pub fn fetch_user_details(c: &mut RobloxClient, ids: &[u64]) -> HashMap<u64, (String, String, bool)> {
    let mut out = HashMap::with_capacity(ids.len());
    for chunk in chunks(ids) {
        let raw = c.post_raw(
            &format!("{}/v1/users", USERS_BASE),
            &json!({ "userIds": chunk }),
        );
        let raw = match raw {
            Ok(r) => r,
            Err(_) => continue,
        };
        if !(200..300).contains(&raw.status) {
            continue;
        }
        if let Ok(ur) = raw.json::<UsersBatchResponse>() {
            for u in ur.data {
                let display = u.display_name.filter(|d| !d.is_empty()).unwrap_or_default();
                out.insert(u.id, (u.name, display, u.is_banned.unwrap_or(false)));
            }
        }
    }
    out
}

/// POST /v1/presence/users — batched presence lookup.
pub fn fetch_presence(c: &mut RobloxClient, ids: &[u64]) -> Result<HashMap<u64, Presence>, ApiError> {
    let mut out = HashMap::with_capacity(ids.len());
    for chunk in chunks(ids) {
        let raw = c.post_raw(
            &format!("{}/v1/presence/users", PRESENCE_BASE),
            &json!({ "userIds": chunk }),
        )?;
        if !(200..300).contains(&raw.status) {
            return Err(raw.error());
        }
        let pr: PresenceResponse = raw.json()?;
        for p in pr.user_presences {
            out.insert(
                p.user_id,
                Presence {
                    kind: PresenceKind::from_api(p.presence_type),
                    place_id: p.place_id,
                    root_place_id: p.root_place_id,
                    universe_id: p.universe_id,
                    game_id: p.game_id,
                    last_location: p.last_location.unwrap_or_default(),
                },
            );
        }
    }
    Ok(out)
}

/// POST /v1/presence/last-online — best effort (empty map on any error).
pub fn fetch_last_online(c: &mut RobloxClient, ids: &[u64]) -> HashMap<u64, i64> {
    let mut out = HashMap::new();
    for chunk in chunks(ids) {
        let raw = c.post_raw(
            &format!("{}/v1/presence/last-online", PRESENCE_BASE),
            &json!({ "userIds": chunk }),
        );
        if let Ok(raw) = raw {
            if (200..300).contains(&raw.status) {
                if let Ok(lr) = raw.json::<LastOnlineResponse>() {
                    for e in lr.entries {
                        if let Some(ts) = e.last_online.and_then(|s| crate::model::parse_iso8601_utc(&s)) {
                            out.insert(e.user_id, ts);
                        }
                    }
                }
            }
        }
    }
    out
}

/// Names that are placeholders, not real titles.
fn is_placeholder_game_name(name: &str) -> bool {
    let n = name.trim();
    n.is_empty()
        || n.eq_ignore_ascii_case("[TITLE UNAVAILABLE]")
        || n.eq_ignore_ascii_case("[Content Deleted]")
        || n.eq_ignore_ascii_case("[content deleted]")
}

/// GET /v1/games?universeIds=... — game display names (skip placeholders).
/// Universes the requester cannot view (private / age-restricted games) are
/// silently missing from the response; the UI falls back to presence
/// `lastLocation` for those.
pub fn fetch_game_names(c: &mut RobloxClient, universe_ids: &[u64]) -> HashMap<u64, String> {
    let mut out = HashMap::new();
    let unique: Vec<u64> = {
        let mut v: Vec<u64> = universe_ids.to_vec();
        v.sort_unstable();
        v.dedup();
        v.retain(|id| *id != 0);
        v
    };
    for chunk in chunks(&unique) {
        let url = format!("{}/v1/games?universeIds={}", GAMES_BASE, {
            let list: Vec<String> = chunk.iter().map(|i| i.to_string()).collect();
            list.join(",")
        });
        if let Ok(raw) = c.send(&url, None) {
            if (200..300).contains(&raw.status) {
                if let Ok(gr) = raw.json::<GamesResponse>() {
                    for g in gr.data {
                        if let Some(name) = g.name {
                            if !is_placeholder_game_name(&name) {
                                out.insert(g.id, name);
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

/// GET /v1/users/avatar-headshot?userIds=... — [(user_id, image_url)] for
/// completed thumbnails only.
pub fn fetch_avatar_index(c: &mut RobloxClient, ids: &[u64]) -> Result<Vec<(u64, String)>, ApiError> {
    let mut out = Vec::with_capacity(ids.len());
    for chunk in chunks(ids) {
        let list: Vec<String> = chunk.iter().map(|i| i.to_string()).collect();
        let url = format!(
            "{}/v1/users/avatar-headshot?userIds={}&size=150x150&format=Png&isCircular=false",
            THUMBS_BASE,
            list.join(",")
        );
        let raw = c.send(&url, None)?;
        if !(200..300).contains(&raw.status) {
            return Err(raw.error());
        }
        let tr: ThumbsResponse = raw.json()?;
        for t in tr.data {
            if t.state == "Completed" {
                if let Some(u) = t.image_url {
                    out.push((t.target_id, u));
                }
            }
        }
    }
    Ok(out)
}

/// Download an avatar image from the CDN and decode it to a square RGBA
/// buffer of `size` x `size`.
pub fn download_avatar(agent: &ureq::Agent, url: &str, size: u32) -> Result<Vec<u8>, ApiError> {
    let sent = agent.get(url).set("Accept", "image/png,image/*").call();
    let resp = match sent {
        Ok(r) => r,
        Err(ureq::Error::Status(code, _)) => return Err(ApiError::Status { code, message: String::new() }),
        Err(ureq::Error::Transport(t)) => return Err(ApiError::Network(t.to_string())),
    };
    use std::io::Read;
    let mut bytes = Vec::new();
    resp.into_reader()
        .take(4 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|e| ApiError::Network(e.to_string()))?;
    let img = image::load_from_memory(&bytes).map_err(|e| ApiError::Parse(format!("avatar: {}", e)))?;
    let scaled = img.resize_exact(size, size, image::imageops::FilterType::Triangle);
    Ok(scaled.to_rgba8().into_raw())
}
