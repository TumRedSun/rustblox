//! Place discovery (omni search/recommendations) plus game display names.

use super::{chunks, get_json_option, percent_encode, require_ok, unique_ids};
use crate::model::PlaceCard;
use crate::roblox::client::{RobloxClient, APIS_BASE, GAMES_BASE};
use crate::roblox::errors::ApiError;
use crate::roblox::types::{GamesResponse, OmniResponse};
use serde_json::json;
use std::collections::HashMap;

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
	for chunk in chunks(&unique_ids(universe_ids)) {
		let list: Vec<String> = chunk.iter().map(|i| i.to_string()).collect();
		let url = format!("{}/v1/games?universeIds={}", GAMES_BASE, list.join(","));
		let Some(gr) = get_json_option::<GamesResponse>(c, &url) else {
			continue;
		};
		for g in gr.data {
			if let Some(name) = g.name {
				if !is_placeholder_game_name(&name) {
					out.insert(g.id, name);
				}
			}
		}
	}
	out
}

/// Which content group a omni payload group belongs to.
pub struct OmniGroupKind<'a> {
	/// `contentGroupType` of the group (e.g. "Game", "ContinuePlaying").
	pub group_type: &'a str,
}

/// Any group (the default feed flattening).
fn any_group(_: &OmniGroupKind) -> bool {
	true
}

/// The "continue playing" group of the home discovery feed (recently played).
fn continue_group(g: &OmniGroupKind) -> bool {
	g.group_type.to_lowercase().contains("continue")
}

/// Parse an omni discovery payload (search or recommendation feed) into
/// place cards, keeping only groups matching `want`. Sponsored entries and
/// entries without a name/place id are skipped; duplicates by universe id
/// are removed.
fn parse_omni_where(
	body: &[u8],
	want: &dyn Fn(&OmniGroupKind) -> bool,
	limit: usize,
) -> Result<Vec<PlaceCard>, ApiError> {
	let omni: OmniResponse =
		serde_json::from_slice(body).map_err(|e| ApiError::Parse(format!("places: {}", e)))?;
	let mut out: Vec<PlaceCard> = Vec::new();
	let mut seen = std::collections::HashSet::new();
	'groups: for group in omni.search_results {
		let kind = OmniGroupKind { group_type: group.content_group_type.as_deref().unwrap_or("") };
		if !want(&kind) {
			continue;
		}
		for item in group.contents {
			if out.len() >= limit {
				break 'groups;
			}
			let name = item.name.unwrap_or_default();
			if name.trim().is_empty() || item.is_sponsored.unwrap_or(false) {
				continue;
			}
			let universe_id = item.universe_id.unwrap_or(0);
			let root_place_id = item.root_place_id.unwrap_or(0);
			if universe_id == 0 || root_place_id == 0 || !seen.insert(universe_id) {
				continue;
			}
			out.push(PlaceCard {
				universe_id,
				root_place_id,
				name,
				creator: item.creator_name.unwrap_or_default(),
				player_count: item.player_count.unwrap_or(0),
				likes_pct: likes_percent(item.total_up_votes, item.total_down_votes),
				sponsored: false,
			});
		}
	}
	Ok(out)
}

/// Rounded up-vote share, `None` when nobody voted yet.
fn likes_percent(up: Option<u64>, down: Option<u64>) -> Option<u8> {
	let (up, down) = (up.unwrap_or(0), down.unwrap_or(0));
	if up + down == 0 {
		return None;
	}
	Some(((up as f64 / (up + down) as f64) * 100.0).round() as u8)
}

fn parse_omni(body: &[u8], limit: usize) -> Result<Vec<PlaceCard>, ApiError> {
	parse_omni_where(body, &any_group, limit)
}

/// GET apis.roblox.com/search-api/omni-search?searchQuery=... — place search
/// by name. Returns up to `limit` cards.
///
/// # Errors
/// Transport failures, the HTTP error of the search endpoint, or a parse error.
pub fn search_games(
	c: &mut RobloxClient,
	keyword: &str,
	limit: usize,
) -> Result<Vec<PlaceCard>, ApiError> {
	let url = format!(
		"{}/search-api/omni-search?searchQuery={}&pageType=all&sessionId={}&pageToken=",
		APIS_BASE,
		percent_encode(keyword.trim()),
		pseudo_uuid()
	);
	let raw = c.send(&url, None)?;
	require_ok(&raw)?;
	parse_omni(&raw.body, limit)
}

/// POST apis.roblox.com/discovery-api/omni-recommendation — the default
/// "discover" feed shown while the place search box is empty (requires the
/// authenticated session cookie, exactly like the website).
///
/// # Errors
/// Transport failures, the HTTP error of the discovery endpoint, or a parse
/// error.
pub fn recommended_games(c: &mut RobloxClient, limit: usize) -> Result<Vec<PlaceCard>, ApiError> {
	let url = format!("{}/discovery-api/omni-recommendation", APIS_BASE);
	let body = json!({ "sessionId": pseudo_uuid(), "pageType": "all" });
	let raw = c.send(&url, Some(&body))?;
	require_ok(&raw)?;
	parse_omni(&raw.body, limit)
}

/// POST apis.roblox.com/discovery-api/omni-recommendation with pageType
/// "Home" — the website home feed; its "ContinuePlaying" group is the
/// recently played row. Returns an empty Vec (not an error) when the payload
/// has no such group.
///
/// # Errors
/// Transport failures, the HTTP error of the discovery endpoint, or a parse
/// error.
pub fn recently_played(c: &mut RobloxClient, limit: usize) -> Result<Vec<PlaceCard>, ApiError> {
	let url = format!("{}/discovery-api/omni-recommendation", APIS_BASE);
	let body = json!({ "sessionId": pseudo_uuid(), "pageType": "Home" });
	let raw = c.send(&url, Some(&body))?;
	require_ok(&raw)?;
	parse_omni_where(&raw.body, &continue_group, limit)
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
