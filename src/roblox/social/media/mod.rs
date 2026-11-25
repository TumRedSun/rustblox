//! Thumbnail URLs (avatars, place icons) and CDN image decoding.

use super::{chunks, get_json_option, require_ok, unique_ids};
use crate::roblox::client::{RobloxClient, THUMBS_BASE};
use crate::roblox::errors::ApiError;
use crate::roblox::types::{ThumbEntry, ThumbsResponse};

/// Completed `(target id, image url)` pairs of a thumbnails reply.
fn completed_urls(thumbs: ThumbsResponse) -> Vec<(u64, String)> {
	thumbs.data.into_iter().filter_map(completed_url).collect()
}

fn completed_url(t: ThumbEntry) -> Option<(u64, String)> {
	if t.state != "Completed" {
		return None;
	}
	t.image_url.map(|url| (t.target_id, url))
}

/// GET /v1/users/avatar-headshot?userIds=... — [(user_id, image_url)] for
/// completed thumbnails only.
///
/// # Errors
/// Transport failures and the HTTP/parse error of the thumbnails endpoint.
pub fn fetch_avatar_index(
	c: &mut RobloxClient,
	ids: &[u64],
) -> Result<Vec<(u64, String)>, ApiError> {
	let mut out = Vec::with_capacity(ids.len());
	for chunk in chunks(ids) {
		let list: Vec<String> = chunk.iter().map(|i| i.to_string()).collect();
		let url = format!(
			"{}/v1/users/avatar-headshot?userIds={}&size=150x150&format=Png&isCircular=false",
			THUMBS_BASE,
			list.join(",")
		);
		let raw = c.send(&url, None)?;
		require_ok(&raw)?;
		out.extend(completed_urls(raw.json()?));
	}
	Ok(out)
}

/// GET /v1/places/gameicons?placeIds=... — [(place_id, image_url)] for
/// completed square game icons (place cards). Best effort: unreachable
/// thumbnails simply keep the placeholder tile.
pub fn fetch_game_icons(c: &mut RobloxClient, place_ids: &[u64]) -> Vec<(u64, String)> {
	let mut out = Vec::with_capacity(place_ids.len());
	for chunk in chunks(&unique_ids(place_ids)) {
		let list: Vec<String> = chunk.iter().map(|i| i.to_string()).collect();
		let url = format!(
			"{}/v1/places/gameicons?placeIds={}&size=512x512&format=Png&isCircular=false",
			THUMBS_BASE,
			list.join(",")
		);
		if let Some(tr) = get_json_option::<ThumbsResponse>(c, &url) {
			out.extend(completed_urls(tr));
		}
	}
	out
}

/// Download an image from the CDN and decode it to a square RGBA buffer of
/// `size` x `size` (avatars, place icons).
///
/// # Errors
/// Transport failures, the HTTP status of a non-2xx reply, or a decode error.
pub fn download_image(agent: &ureq::Agent, url: &str, size: u32) -> Result<Vec<u8>, ApiError> {
	use std::io::Read;
	let sent = agent.get(url).set("Accept", "image/png,image/*").call();
	let resp = match sent {
		Ok(r) => r,
		Err(ureq::Error::Status(code, _)) => {
			return Err(ApiError::Status { code, message: String::new() })
		}
		Err(ureq::Error::Transport(t)) => return Err(ApiError::Network(t.to_string())),
	};
	let mut bytes = Vec::new();
	resp.into_reader()
		.take(4 * 1024 * 1024)
		.read_to_end(&mut bytes)
		.map_err(|e| ApiError::Network(e.to_string()))?;
	let img =
		image::load_from_memory(&bytes).map_err(|e| ApiError::Parse(format!("image: {}", e)))?;
	let scaled = img.resize_exact(size, size, image::imageops::FilterType::Triangle);
	Ok(scaled.to_rgba8().into_raw())
}
