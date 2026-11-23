//! Friends/presence refresh and recently-played pipelines (free functions,
//! no UI state needed beyond the shared dedup maps).

use super::super::event::ImageKind;
use super::super::{lock, ImageJob};
use crate::model::{PlaceCard, Presence, PresenceKind, Snapshot};
use crate::roblox::client::RobloxClient;
use crate::roblox::errors::ApiError;
use crate::roblox::social;
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::Mutex;

/// Enqueue thumbnails whose URL changed since the last push (dedup map).
pub(super) fn enqueue_thumbs(
	map: &Mutex<HashMap<u64, String>>,
	jobs: &Sender<ImageJob>,
	kind: fn(u64) -> ImageKind,
	index: Vec<(u64, String)>,
) {
	let mut pushed = lock(map);
	for (id, url) in index {
		if pushed.get(&id).map(|old| old != &url).unwrap_or(true) {
			let job = ImageJob { kind: kind(id), url: url.clone() };
			if jobs.send(job).is_ok() {
				pushed.insert(id, url);
			}
		}
	}
}

/// Resolve + enqueue place icons for a card list (dedup across pushes).
pub(super) fn push_place_icons(
	c: &mut RobloxClient,
	cards: &[PlaceCard],
	jobs: &Sender<ImageJob>,
	icons: &Mutex<HashMap<u64, String>>,
) {
	let place_ids: Vec<u64> = cards.iter().map(|p| p.root_place_id).collect();
	let icons_index = social::fetch_game_icons(c, &place_ids);
	enqueue_thumbs(icons, jobs, ImageKind::Place, icons_index);
}

/// Recently played = the official discovery "continue" row, extended with
/// locally tracked places (recorded while the client is open). Best effort:
/// never fails, returns whatever source worked.
pub(super) fn collect_recently_played(
	c: &mut RobloxClient,
	jobs: &Sender<ImageJob>,
	icons: &Mutex<HashMap<u64, String>>,
) -> Vec<PlaceCard> {
	const LIMIT: usize = 12;
	let mut out = social::recently_played(c, LIMIT).unwrap_or_default();
	append_local_recent(c, &mut out, LIMIT);
	if !out.is_empty() {
		push_place_icons(c, &out, jobs, icons);
	}
	out
}

/// Local fallback / supplement from settings.json.
fn append_local_recent(c: &mut RobloxClient, out: &mut Vec<PlaceCard>, limit: usize) {
	let local = crate::config::load_settings().recent_places;
	if local.is_empty() {
		return;
	}
	let universes: Vec<u64> = local.iter().map(|r| r.universe_id).filter(|u| *u != 0).collect();
	let names = social::fetch_game_names(c, &universes);
	for r in local {
		if r.root_place_id == 0 || out.iter().any(|c| c.root_place_id == r.root_place_id) {
			continue;
		}
		if out.len() >= limit {
			break;
		}
		let name = if r.name.is_empty() {
			names.get(&r.universe_id).cloned().unwrap_or_default()
		} else {
			r.name
		};
		if name.is_empty() {
			continue;
		}
		out.push(PlaceCard {
			universe_id: r.universe_id,
			root_place_id: r.root_place_id,
			name,
			creator: String::new(),
			player_count: 0,
			likes_pct: None,
			sponsored: false,
		});
	}
}

/// Best-effort place display name for the HUD.
pub(super) fn fetch_place_name(c: &mut RobloxClient, place_id: u64) -> Option<String> {
	let url =
		format!("https://games.roblox.com/v1/games/multiget-place-details?placeIds={}", place_id);
	let v: serde_json::Value = c.get_json(&url).ok()?;
	v.as_array()?.first()?.get("name")?.as_str().map(|s| s.to_string())
}

/// Full refresh pipeline: friends -> presence -> last-online -> game names,
/// then enqueue avatar downloads for URLs we have not pushed yet.
pub(super) fn run_refresh(
	c: &mut RobloxClient,
	me_id: u64,
	jobs: &Sender<ImageJob>,
	pushed: &Mutex<HashMap<u64, String>>,
) -> Result<Snapshot, ApiError> {
	let friends = social::fetch_friends(c, me_id)?;
	let ids: Vec<u64> = friends.iter().map(|f| f.id).collect();

	// Presence for friends AND ourselves: our own entry feeds the local
	// "recently played" tracker (settings.json).
	let mut lookup: Vec<u64> = ids.clone();
	lookup.push(me_id);
	let presences: HashMap<u64, Presence> = social::fetch_presence(c, &lookup)?;
	let last_online = social::fetch_last_online(c, &ids);

	let universes: Vec<u64> = presences
		.values()
		.filter(|p| p.kind == PresenceKind::InGame)
		.filter_map(|p| p.universe_id)
		.collect();
	let games = social::fetch_game_names(c, &universes);
	note_current_place(presences.get(&me_id), &games);

	// Avatars: resolve thumbnail URLs and enqueue downloads for anything not
	// pushed before (dedup across refreshes; disk cache absorbs the rest).
	// Includes our own id so the sidebar profile chip shows the real avatar.
	let mut want = ids;
	want.push(me_id);
	if let Ok(index) = social::fetch_avatar_index(c, &want) {
		enqueue_thumbs(pushed, jobs, ImageKind::User, index);
	}
	Ok(Snapshot { friends, presences, games, last_online })
}

/// Track the place we are playing right now for the recently played row.
fn note_current_place(me: Option<&Presence>, games: &HashMap<u64, String>) {
	let Some(me) = me else {
		return;
	};
	if !matches!(me.kind, PresenceKind::InGame | PresenceKind::Studio) {
		return;
	}
	let Some(root) = me.root_place_id else {
		return;
	};
	let name = me
		.universe_id
		.and_then(|u| games.get(&u).cloned())
		.or_else(|| crate::model::clean_last_location(&me.last_location))
		.unwrap_or_default();
	crate::config::note_recent_place(me.universe_id.unwrap_or(0), root, &name);
}
