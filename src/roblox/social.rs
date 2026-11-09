//! Social data: friends, presence, game names, avatar thumbnails, place
//! search/recommendations. All requests are batched (<=100 ids per call) to
//! minimize round trips.

use crate::model::{Friend, PlaceCard, Presence, PresenceKind, UserEntry};
use crate::roblox::client::{
    RobloxClient, APIS_BASE, FRIENDS_BASE, GAMES_BASE, PRESENCE_BASE, THUMBS_BASE, USERS_BASE,
};
use crate::roblox::errors::ApiError;
use crate::roblox::types::{
    FriendsResponse, GamesResponse, LastOnlineResponse, OmniResponse, PresenceResponse,
    ThumbsResponse, UserSearchResponse, UsersBatchResponse,
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

/// Download an image from the CDN and decode it to a square RGBA buffer of
/// `size` x `size` (avatars, place icons).
pub fn download_image(agent: &ureq::Agent, url: &str, size: u32) -> Result<Vec<u8>, ApiError> {
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
    let img = image::load_from_memory(&bytes).map_err(|e| ApiError::Parse(format!("image: {}", e)))?;
    let scaled = img.resize_exact(size, size, image::imageops::FilterType::Triangle);
    Ok(scaled.to_rgba8().into_raw())
}

/// GET /v1/places/gameicons?placeIds=... — [(place_id, image_url)] for
/// completed square game icons (place cards).
pub fn fetch_game_icons(c: &mut RobloxClient, place_ids: &[u64]) -> Vec<(u64, String)> {
    let mut out = Vec::with_capacity(place_ids.len());
    let unique: Vec<u64> = {
        let mut v: Vec<u64> = place_ids.to_vec();
        v.sort_unstable();
        v.dedup();
        v.retain(|id| *id != 0);
        v
    };
    for chunk in chunks(&unique) {
        let list: Vec<String> = chunk.iter().map(|i| i.to_string()).collect();
        let url = format!(
            "{}/v1/places/gameicons?placeIds={}&size=512x512&format=Png&isCircular=false",
            THUMBS_BASE,
            list.join(",")
        );
        if let Ok(raw) = c.send(&url, None) {
            if (200..300).contains(&raw.status) {
                if let Ok(tr) = raw.json::<ThumbsResponse>() {
                    for t in tr.data {
                        if t.state == "Completed" {
                            if let Some(u) = t.image_url {
                                out.push((t.target_id, u));
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

/// A plausible RFC-4122-shaped session id for the discovery APIs (no external
/// rand crate: mixes the clock, a process-lifetime counter and the thread
/// name hash through FNV-1a; the APIs treat it as an opaque token).
pub fn pseudo_uuid() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64 ^ d.as_secs())
        .unwrap_or(0);
    let tid = format!("{:?}", std::thread::current().id());
    let a = fnv1a64(&nanos.to_le_bytes());
    let b = fnv1a64(&n.to_le_bytes()) ^ fnv1a64(tid.as_bytes());
    let c = fnv1a64(&b.to_le_bytes()) ^ nanos.rotate_left(17);
    let d = a ^ b.rotate_left(32) ^ c;
    // Set the version (4) and variant bits so the shape looks canonical.
    let b2 = (b & 0xFFFF_FFFF_FFFF_0FFF) | 0x0000_0000_0000_4000;
    let c2 = (c & 0x3FFF_FFFF_FFFF_FFFF) | 0x8000_0000_0000_0000;
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (a >> 32) as u32,
        (a & 0xFFFF) as u16,
        (b2 >> 48) as u16,
        (c2 >> 48) as u16,
        d & 0xFFFF_FFFF_FFFF
    )
}

fn fnv1a64(data: &[u8]) -> u64 {
    crate::avatar_cache::fnv1a64(data)
}

/// Parse an omni discovery payload (search or recommendation feed) into
/// place cards. Sponsored entries and entries without a name/place id are
/// skipped; duplicates by universe id are removed.
fn parse_omni(body: &[u8], limit: usize) -> Result<Vec<PlaceCard>, ApiError> {
    let omni: OmniResponse =
        serde_json::from_slice(body).map_err(|e| ApiError::Parse(format!("places: {}", e)))?;
    let mut out: Vec<PlaceCard> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    'groups: for group in omni.search_results {
        for item in group.contents {
            if out.len() >= limit {
                break 'groups;
            }
            let name = item.name.unwrap_or_default();
            if name.trim().is_empty() {
                continue;
            }
            if item.is_sponsored.unwrap_or(false) {
                continue;
            }
            let universe_id = item.universe_id.unwrap_or(0);
            let root_place_id = item.root_place_id.unwrap_or(0);
            if universe_id == 0 || root_place_id == 0 || !seen.insert(universe_id) {
                continue;
            }
            let (up, down) = (
                item.total_up_votes.unwrap_or(0),
                item.total_down_votes.unwrap_or(0),
            );
            let likes_pct = if up + down > 0 {
                Some(((up as f64 / (up + down) as f64) * 100.0).round() as u8)
            } else {
                None
            };
            out.push(PlaceCard {
                universe_id,
                root_place_id,
                name,
                creator: item.creator_name.unwrap_or_default(),
                player_count: item.player_count.unwrap_or(0),
                likes_pct,
                sponsored: false,
            });
        }
    }
    Ok(out)
}

/// GET apis.roblox.com/search-api/omni-search?searchQuery=... — place search
/// by name. Returns up to `limit` cards.
pub fn search_games(c: &mut RobloxClient, keyword: &str, limit: usize) -> Result<Vec<PlaceCard>, ApiError> {
    let url = format!(
        "{}/search-api/omni-search?searchQuery={}&pageType=all&sessionId={}&pageToken=",
        APIS_BASE,
        percent_encode(keyword.trim()),
        pseudo_uuid()
    );
    let raw = c.send(&url, None)?;
    if !(200..300).contains(&raw.status) {
        return Err(raw.error());
    }
    parse_omni(&raw.body, limit)
}

/// POST apis.roblox.com/discovery-api/omni-recommendation — the default
/// "discover" feed shown while the place search box is empty (requires the
/// authenticated session cookie, exactly like the website).
pub fn recommended_games(c: &mut RobloxClient, limit: usize) -> Result<Vec<PlaceCard>, ApiError> {
    let url = format!("{}/discovery-api/omni-recommendation", APIS_BASE);
    let body = json!({ "sessionId": pseudo_uuid(), "pageType": "all" });
    let raw = c.send(&url, Some(&body))?;
    if !(200..300).contains(&raw.status) {
        return Err(raw.error());
    }
    parse_omni(&raw.body, limit)
}
