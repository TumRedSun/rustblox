//! Join-script field extraction: udmux routing, ClientTicket parts, NetStack
//! side channel and early-encryption keys.

use super::diag::is_secret_join_field;
use super::JoinResponse;
use crate::net::udmux::{b64_decode, parse_netstack_config, percent_decode};

/// Copy every optional join-script field the connection layer consumes into
/// the typed response. Fields absent from the script keep their defaults.
pub(super) fn apply_join_script(
	resp: &mut JoinResponse,
	js: &serde_json::Map<String, serde_json::Value>,
) {
	let (host, token, pepper) = extract_udmux(js);
	apply_udmux(resp, host.as_deref(), token.as_deref(), pepper);
	apply_client_ticket(resp, js);
	apply_netstack(resp, js);
	apply_early_keys(resp, js);
	if let Some(a) = js.get("TokenGenAlgorithm").and_then(|v| v.as_u64()) {
		crate::netlog!(info, "join: token gen algorithm {} (1 = current token generation)", a);
		resp.token_gen_algorithm = a;
	}
}

/// Pull the udmux routing material out of a join-script JSON object:
/// `UdmuxEndpoints[0].Address:Port`, the routing token (`TokenValue` or
/// legacy `UdmuxToken`) and `PepperId`. All optional.
pub(crate) fn extract_udmux(
	js: &serde_json::Map<String, serde_json::Value>,
) -> (Option<String>, Option<String>, Option<u64>) {
	let mut host = None;
	if let Some(ep) = super::script::first_endpoint_list(js.get("UdmuxEndpoints")) {
		host = Some(if ep.1 > 0 { format!("{}:{}", ep.0, ep.1) } else { ep.0 });
	}
	let token = ["TokenValue", "UdmuxToken"]
		.iter()
		.find_map(|k| js.get(*k).and_then(|t| t.as_str()).map(str::to_string))
		.filter(|t| !t.is_empty());
	let pepper = js.get("PepperId").and_then(|p| p.as_u64());
	(host, token, pepper)
}

fn apply_udmux(
	resp: &mut JoinResponse,
	host: Option<&str>,
	token: Option<&str>,
	pepper: Option<u64>,
) {
	if let Some(h) = host {
		crate::netlog!(info, "join: udmux endpoint from join script: {}", h);
		if resp.udmux_host.is_empty() {
			resp.udmux_host = h.to_string();
			if resp.port == 0 {
				resp.port = h.rsplit(':').next().and_then(|p| p.parse().ok()).unwrap_or(0);
			}
		}
	}
	if let Some(t) = token {
		// Shape diagnostics for the routing token: 24 base64 chars are
		// 18 raw bytes unpadded but 16 with "==" — and 16 is exactly the
		// RUPP epoch-tag width the proxy routes on.
		let dec = b64_decode(t);
		crate::netlog!(
			info,
			"join: udmux token present ({} chars, b64 -> {} bytes, padded={})",
			t.len(),
			dec.len(),
			t.ends_with('=')
		);
		if resp.udmux_token.is_empty() {
			resp.udmux_token = t.to_string();
		}
	}
	if let Some(p) = pepper {
		crate::netlog!(info, "join: udmux pepper {}", p);
		resp.udmux_pepper = p;
	}
}

/// The join call's own ClientTicket: part2 (b64) is the 33-byte paBlob the
/// crypto OCR2 carries verbatim (live-verified 2026-11-17 — the server checks
/// it against this join call), and part4 is the authVer byte as a decimal
/// string ("18" -> 0x12).
fn apply_client_ticket(resp: &mut JoinResponse, js: &serde_json::Map<String, serde_json::Value>) {
	let Some(ct) =
		js.get("ClientTicket").and_then(|v| v.as_str()).map(str::trim).filter(|t| !t.is_empty())
	else {
		return;
	};
	let parts: Vec<&str> = ct.split(';').collect();
	if let Some(part) = parts.get(2) {
		let blob = b64_decode(part.trim());
		crate::netlog!(info, "join: ClientTicket part2 = {} bytes (the OCR2 paBlob)", blob.len());
		resp.client_ticket_pa_blob = blob;
	}
	if let Some(part) = parts.get(4) {
		if let Ok(v) = part.trim().parse::<u8>() {
			crate::netlog!(
				info,
				"join: ClientTicket part4 = {} (auth_ver 0x{:02x})",
				part.trim(),
				v
			);
			resp.client_ticket_auth_ver = v;
		}
	}
}

/// NetStack side-channel fields (hypothesis (b)) — parse, dump and carry them
/// so a failed handshake can be diagnosed from the log.
fn apply_netstack(resp: &mut JoinResponse, js: &serde_json::Map<String, serde_json::Value>) {
	if let Some(p) = js.get("NetStackPort").and_then(|v| v.as_u64()) {
		if p > 0 && p <= u64::from(u16::MAX) {
			crate::netlog!(
				info,
				"join: netstack port {} (hypothesis (b): side-channel transport)",
				p
			);
			resp.netstack_port = p as u16;
		}
	}
	if let Some(c) =
		js.get("NetStackConfig").and_then(|v| v.as_str()).map(str::trim).filter(|c| !c.is_empty())
	{
		// NetStackConfig arrives PERCENT-ENCODED (`%7B%22pubKey%22…` — live
		// join_full.json, Nov 2026): decoding must happen before the JSON
		// parse, or the parser falls back to the flat split and the keys
		// (pubKey/port/caps/certHashes) never come out. The config itself is
		// not a credential; token-ish VALUES inside the parsed pairs are
		// logged as lengths only.
		let dec = percent_decode(c);
		crate::netlog!(
			info,
			"join: NetStackConfig ({} chars raw, {} decoded): {}",
			c.len(),
			dec.len(),
			dec
		);
		resp.netstack_config = dec.clone();
		for (k, val) in parse_netstack_config(&dec) {
			if is_secret_join_field(&k) {
				crate::netlog!(info, "join: netstack config: {} = <len {}>", k, val.len());
			} else {
				crate::netlog!(info, "join: netstack config: {} = {}", k, val);
			}
		}
	}
	if let Some(t) = js
		.get("NetStackTokenValue")
		.and_then(|v| v.as_str())
		.map(str::trim)
		.filter(|t| !t.is_empty())
	{
		crate::netlog!(
			info,
			"join: netstack token present ({} chars, b64 -> {} bytes)",
			t.len(),
			b64_decode(t).len()
		);
		resp.netstack_token = t.to_string();
	}
}

/// Early-encryption material (hypothesis (c)): `EphemeralEarlyPubKey` rides
/// percent-encoded (`%2B`/`%2F`/`%3D`); without the decode the base64 reader
/// saw `%` and bailed with 0 bytes, hiding a perfectly good 32-byte X25519
/// public key. `ClientPublicKeyData` is not base64 at all but a JSON
/// key-rotation config — logged verbatim.
fn apply_early_keys(resp: &mut JoinResponse, js: &serde_json::Map<String, serde_json::Value>) {
	for (key, slot) in [
		("EphemeralEarlyPubKey", &mut resp.ephemeral_pubkey),
		("ClientPublicKeyData", &mut resp.client_pubkey_data),
	] {
		let Some(p) = js.get(key).and_then(|v| v.as_str()).map(str::trim).filter(|p| !p.is_empty())
		else {
			continue;
		};
		let dec = percent_decode(p);
		let n = b64_decode(&dec).len();
		if n > 0 {
			crate::netlog!(
                info,
                "join: {} present ({} chars, b64 -> {} bytes) — early-encryption hypothesis (c) is live",
                key,
                dec.len(),
                n
            );
		} else {
			crate::netlog!(
                info,
                "join: {} present ({} chars, not b64): {} — early-encryption hypothesis (c) is live",
                key,
                dec.len(),
                dec
            );
		}
		*slot = dec;
	}
}
