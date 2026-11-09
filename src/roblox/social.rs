//! Social data: friends, presence, game names, avatar thumbnails.
//! All requests are batched (<=100 ids per call) to minimize round trips.

use crate::model::{Friend, Presence, PresenceKind};
use crate::roblox::client::{RobloxClient, FRIENDS_BASE, GAMES_BASE, PRESENCE_BASE, THUMBS_BASE};
use crate::roblox::errors::ApiError;
use crate::roblox::types::{FriendsResponse, GamesResponse, LastOnlineResponse, PresenceResponse, ThumbsResponse};
use serde_json::json;
use std::collections::HashMap;

const CHUNK: usize = 100;

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
    Ok(fr
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
        .collect())
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

/// GET /v1/games?universeIds=... — game display names (skip placeholders).
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
                            if !name.is_empty() && !name.starts_with('[') {
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
