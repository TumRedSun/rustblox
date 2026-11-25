//! The join HTTP POST itself: request, refusal reporting, and recovery of the
//! endpoint + udmux/netstack material hidden in the join script.

mod diag;
mod fields;
mod script;

pub(crate) use diag::snippet;
#[cfg(test)]
pub(crate) use fields::extract_udmux;
pub(crate) use script::extract_endpoint;

use super::{join_refusal_detail, JoinResponse, GAMEJOIN_BASE};
use crate::roblox::client::{RobloxClient, CLIENT_UA};
use crate::roblox::errors::ApiError;
use serde_json::Value;

/// POST one join endpoint and turn its reply into a validated
/// [`JoinResponse`], recovering the endpoint from the join script when the
/// top-level `ip`/`port` are missing.
pub(super) fn join_request(
	client: &mut RobloxClient,
	path: &str,
	body: &serde_json::Value,
	referer: &str,
) -> Result<JoinResponse, ApiError> {
	// The join service only accepts client-shaped traffic: under a browser
	// User-Agent it refuses with status 12 even when the session is valid, so
	// the POST carries the client UA/Referer/Origin explicitly through the
	// shared agent/cookie machinery.
	let url = format!("{}{}", GAMEJOIN_BASE, path);
	let raw = client.post_with_headers(
		&url,
		body,
		&[("User-Agent", CLIENT_UA), ("Referer", referer), ("Origin", "https://www.roblox.com")],
	)?;
	let text = String::from_utf8_lossy(&raw.body).to_string();
	crate::netlog!(info, "{} -> HTTP {}: {}", url, raw.status, snippet(&text, 400));
	if !(200..300).contains(&raw.status) {
		return Err(raw.error());
	}
	// Parse through Value first: a refusal carries "jobId": null (which the
	// typed JoinResponse rejected with a useless serde message) plus the
	// joinError details we want to surface.
	let v: Value = serde_json::from_str(text.trim()).map_err(|e| parse_error(&e, &text))?;
	// Shape dump BEFORE the typed parse: names only (the values carry auth
	// tickets), so the next failing join shows exactly which fields Roblox
	// sent — new endpoint/udmux fields are visible without another round trip.
	diag::dump_join_shape(&v);
	let resp: JoinResponse =
		serde_json::from_value(v.clone()).map_err(|e| parse_error(&e, &text))?;
	if resp.job_id.is_empty() {
		let detail = join_refusal_detail(&v);
		crate::netlog!(info, "join: server refused the join: {}", detail);
		return Err(ApiError::JoinRefused(detail));
	}
	recover_join_material(resp, &v)
}

fn parse_error(e: &serde_json::Error, text: &str) -> ApiError {
	ApiError::Parse(format!("{} (body: {})", e, snippet(text, 200)))
}

/// Attach the join-script endpoint and udmux/netstack fields, then reject an
/// accepted-but-unaddressable reply.
fn recover_join_material(mut resp: JoinResponse, v: &Value) -> Result<JoinResponse, ApiError> {
	// The classic endpoint often hides the server address inside the
	// join-script blob instead of top-level ip/port — dig it out.
	if resp.server_addr().is_none() {
		if let Some((ip, port)) = extract_endpoint(v) {
			crate::netlog!(info, "join: endpoint recovered from join script: {}:{}", ip, port);
			resp.ip = ip;
			resp.port = port;
		}
	}
	// Udmux routing material straight from the join script — the real client
	// wraps every RakNet packet with a token-derived header, so without these
	// fields the proxy silently drops our handshake (observed 2026-11).
	if let Some(js) = v.get("joinScript").and_then(|j| j.as_object()) {
		fields::apply_join_script(&mut resp, js);
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
