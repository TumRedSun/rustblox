//! Typed join reply plus the human-readable refusal report.

use serde::{Deserialize, Deserializer};

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
	/// Job id of the assigned server (empty when the join was refused).
	#[serde(default, deserialize_with = "null_to_default")]
	pub job_id: String,
	/// Session id of the assigned server, when the reply carries one.
	#[serde(default, deserialize_with = "null_to_default", alias = "session_id")]
	pub session_id: String,
	/// Server hostname or IP of the game server.
	#[serde(default, deserialize_with = "null_to_default")]
	pub ip: String,
	/// Server UDP port.
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
	/// Data-center id of the assigned server.
	#[serde(default, deserialize_with = "null_to_default", alias = "data_center_id")]
	pub data_center_id: u64,
	/// Membership type of the joining account.
	#[serde(default, deserialize_with = "null_to_default", alias = "membership_type")]
	pub membership_type: i32,
	/// True when the place is opened in playtest mode.
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
	/// Per-session udmux routing token (`TokenValue` / `UdmuxToken`).
	#[serde(default, deserialize_with = "null_to_default")]
	pub udmux_token: String,
	/// Pepper id of the udmux routing token.
	#[serde(default, deserialize_with = "null_to_default")]
	pub udmux_pepper: u64,
	/// NetStack side-channel (join hypothesis (b)): if game traffic moved
	/// off the legacy udmux port, the transport lives on `NetStackPort` with
	/// `NetStackTokenValue` as its routing token. Both default to
	/// zero/empty when the join script omits them.
	#[serde(default, deserialize_with = "null_to_default")]
	pub netstack_port: u16,
	/// Decoded NetStack config blob (`NetStackConfig`).
	#[serde(default, deserialize_with = "null_to_default")]
	pub netstack_config: String,
	/// NetStack routing token (`NetStackTokenValue`).
	#[serde(default, deserialize_with = "null_to_default")]
	pub netstack_token: String,
	/// `TokenGenAlgorithm` from the join script (1 = current generation).
	/// The Nov-2026 capture settled the tag question: the pre-connect RUPP
	/// tag is 16 client-chosen bytes and matches no join-script field, so
	/// this field no longer feeds the routing tag — it stays recorded for
	/// the early-encryption work (the OCR1 epoch byte may be it in disguise).
	#[serde(default, deserialize_with = "null_to_default")]
	pub token_gen_algorithm: u64,
	/// Early-encryption material (join hypothesis (c)): the server's
	/// ephemeral X25519 public key (`EphemeralEarlyPubKey`), percent-decoded.
	#[serde(default, deserialize_with = "null_to_default")]
	pub ephemeral_pubkey: String,
	/// The client's key-rotation config (`ClientPublicKeyData`),
	/// percent-decoded.
	#[serde(default, deserialize_with = "null_to_default")]
	pub client_pubkey_data: String,
	/// The join call's own ClientTicket paBlob (part2, b64-decoded — 33
	/// bytes on the live wire). The crypto OCR2 carries it verbatim and
	/// the server verifies it against THIS join call, so it must come
	/// from the same response as the RCC address and EphemeralEarlyPubKey
	/// (live-verified 2026-11-17: it differs between two join calls).
	#[serde(default, deserialize_with = "null_to_default")]
	pub client_ticket_pa_blob: Vec<u8>,
	/// ClientTicket part4 as a byte (the ASCII "18" -> 0x12) — the OCR2
	/// authVer the server verifies.
	#[serde(default, deserialize_with = "null_to_default")]
	pub client_ticket_auth_ver: u8,
}

impl JoinResponse {
	/// `(host, port)` of the game server, `None` when the reply named none.
	pub fn server_addr(&self) -> Option<(&str, u16)> {
		if self.ip.is_empty() || self.port == 0 {
			None
		} else {
			Some((self.ip.as_str(), self.port))
		}
	}
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
pub(crate) fn join_refusal_detail(v: &serde_json::Value) -> String {
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
