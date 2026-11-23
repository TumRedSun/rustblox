//! Avatar appearance: body colors + asset ids of the joining character.

use crate::roblox::client::RobloxClient;
use crate::roblox::errors::ApiError;
use serde::Deserialize;

/// Avatar description for our own character (body colors + asset ids).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AvatarAppearance {
	/// Rig type ("R6" or "R15").
	#[serde(alias = "player_avatar_type")]
	pub player_avatar_type: String,
	/// Limb/torso colors.
	#[serde(alias = "body_colors")]
	pub body_colors: BodyColors,
	/// Worn assets (hats, shirts, packages…).
	#[serde(alias = "assets")]
	pub assets: Vec<AvatarAsset>,
}

/// The six body-color slots of a character.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BodyColors {
	/// Head color id.
	pub head_color_id: u64,
	/// Torso color id.
	pub torso_color_id: u64,
	/// Left arm color id.
	pub left_arm_color_id: u64,
	/// Right arm color id.
	pub right_arm_color_id: u64,
	/// Left leg color id.
	pub left_leg_color_id: u64,
	/// Right leg color id.
	pub right_leg_color_id: u64,
}

/// One asset worn by the character.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AvatarAsset {
	/// Asset id.
	pub id: u64,
	/// Asset name, when the endpoint reports one.
	pub name: Option<String>,
	/// Asset type (number, or a nested object in some dialects).
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

/// Fetch the avatar description for our own character (body colors + asset
/// ids) from the join response URL (or the standard endpoint as fallback).
///
/// # Errors
/// Transport failures and the HTTP/parse error of the avatar endpoint.
pub fn fetch_avatar_appearance(
	client: &mut RobloxClient,
	url: &str,
	user_id: u64,
) -> Result<AvatarAppearance, ApiError> {
	let url = if url.is_empty() {
		format!("https://avatar.roblox.com/v1/users/{}/avatar", user_id)
	} else {
		url.to_string()
	};
	client.get_json(&url)
}
