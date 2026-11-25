//! Join diagnostics: shape dumps and log-line helpers. These must never
//! print credential values — tokens/tickets are reported as lengths only.

/// Truncate `text` for log lines (keep the head — status/code fields live
/// there).
pub(crate) fn snippet(text: &str, max: usize) -> String {
	let t: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
	if t.chars().count() <= max {
		t
	} else {
		let mut s: String = t.chars().take(max).collect();
		s.push_str("…");
		s
	}
}

/// Log name lists in chunks so the 240-char log-line clamp never eats the
/// tail — the truncated key list is exactly what hid the udmux fields from us.
fn log_name_chunks(prefix: &str, names: &[String]) {
	if names.is_empty() {
		return;
	}
	const PER_LINE: usize = 12;
	let mut sorted = names.to_vec();
	sorted.sort();
	let lines: Vec<&[String]> = sorted.chunks(PER_LINE).collect();
	for chunk in &lines {
		crate::netlog!(info, "{}/{}: {}", prefix, lines.len(), chunk.join(", "));
	}
}

/// Token-ish join fields are logged as lengths only — the values are bearer
/// credentials, everything else (addresses, ports, booleans) is printable.
pub(super) fn is_secret_join_field(key: &str) -> bool {
	let k = key.to_ascii_lowercase();
	["ticket", "token", "session", "cookie", "pubkey", "pepper", "security", "seed"]
		.iter()
		.any(|s| k.contains(s))
}

/// Presence + safe values for every field that matters to the connection
/// layer, so one log shows exactly which endpoint dialect a join used.
fn log_join_object_shape(obj: &serde_json::Map<String, serde_json::Value>) {
	const INTERESTING: [&str; 22] = [
		"MachineAddress",
		"ClientPort",
		"ServerPort",
		"ServerConnections",
		"UdmuxEndpoints",
		"UdmuxToken",
		"TokenValue",
		"PepperId",
		"DirectServerReturn",
		"DataCenterId",
		"NetStackConfig",
		"NetStackPort",
		"NetStackTokenValue",
		"GameFqdn",
		"gameFqdn",
		"EphemeralEarlyPubKey",
		"ClientPublicKeyData",
		"JobId",
		"GameId",
		"SeleniumTestMode",
		"IsLocalDebugRcc",
		"JoinRequestType",
	];
	for key in INTERESTING {
		let Some(val) = obj.get(key) else { continue };
		match val {
			serde_json::Value::String(s) => {
				if is_secret_join_field(key) || s.chars().count() > 60 {
					crate::netlog!(info, "join shape: {} = <str len {}>", key, s.len());
				} else {
					crate::netlog!(info, "join shape: {} = {:?}", key, s);
				}
			}
			other => crate::netlog!(info, "join shape: {} = {}", key, other),
		}
	}
}

/// Log the SHAPE of a join response — top-level keys plus the join script's
/// field names (XML tags or JSON keys). Names only, never values: the script
/// carries authentication tickets. This makes a failed join diagnosable from
/// the log alone — if Roblox starts handing out a udmux proxy address (or any
/// other endpoint dialect) the next log shows the exact field name to parse.
pub(super) fn dump_join_shape(v: &serde_json::Value) {
	let top: Vec<String> = v.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
	log_name_chunks("join response fields", &top);

	let Some(js) = v.get("joinScript") else {
		return;
	};
	if let Some(s) = js.as_str() {
		let trimmed = s.trim_start();
		if trimmed.starts_with('{') {
			if let Ok(jv) = serde_json::from_str::<serde_json::Value>(trimmed) {
				let keys: Vec<String> =
					jv.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
				log_name_chunks(&format!("join script (json, {} bytes) keys", s.len()), &keys);
				if let Some(obj) = jv.as_object() {
					log_join_object_shape(obj);
				}
				return;
			}
		}
		let tags = xml_tag_names(s);
		if !tags.is_empty() {
			log_name_chunks(&format!("join script (xml, {} bytes) tags", s.len()), &tags);
		} else {
			crate::netlog!(info, "join script: {} bytes, no recognizable tags/keys", s.len());
		}
	} else if let Some(obj) = js.as_object() {
		let keys: Vec<String> = obj.keys().cloned().collect();
		log_name_chunks("join script (object) keys", &keys);
		log_join_object_shape(obj);
	}
}

/// Distinct tag names of an XML-ish blob, in order of first appearance
/// (capped so a pathological script can't flood the log).
fn xml_tag_names(blob: &str) -> Vec<String> {
	let bytes = blob.as_bytes();
	let mut tags: Vec<String> = Vec::new();
	let mut i = 0usize;
	while i < bytes.len() && tags.len() < 60 {
		if bytes[i] == b'<' {
			let start = i + 1;
			let mut j = start;
			while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
				j += 1;
			}
			if j > start
				&& j < bytes.len()
				&& (bytes[j] == b'>' || bytes[j] == b' ' || bytes[j] == b'/')
			{
				let name = blob[start..j].to_string();
				if !tags.iter().any(|t| t == &name) {
					tags.push(name);
				}
			}
			i = j;
		} else {
			i += 1;
		}
	}
	tags
}
