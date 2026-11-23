//! Endpoint extraction from a join script: the modern JSON-object dialects
//! plus the classic XML / JSON-string blobs.

/// Pull the game-server endpoint out of a join response that doesn't carry
/// top-level `ip`/`port`. Two dialects exist:
///  * modern: `joinScript` is a JSON OBJECT with `ServerConnections` /
///    `UdmuxEndpoints` arrays and flat `MachineAddress` + `ServerPort`;
///  * classic: `joinScript` (or `joinTicket`) is a STRING holding an XML-ish
///    blob (`<MachineAddress>…</MachineAddress>`) or flat JSON keys.
pub(crate) fn extract_endpoint(v: &serde_json::Value) -> Option<(String, u16)> {
	if let Some(js) = v.get("joinScript").filter(|js| js.is_object()) {
		for key in ["ServerConnections", "UdmuxEndpoints"] {
			if let Some(ep) = first_endpoint_list(js.get(key)) {
				return Some(ep);
			}
		}
		if let Some(ip) = js.get("MachineAddress").and_then(|s| s.as_str()) {
			for pk in ["ServerPort", "MachinePort", "ClientPort"] {
				if let Some(port) = js.get(pk).and_then(|p| p.as_u64()) {
					if !ip.is_empty() && port > 0 && port <= u64::from(u16::MAX) {
						return Some((ip.to_string(), port as u16));
					}
				}
			}
		}
	}
	// Classic dialect: collect candidate strings — the inline joinScript plus
	// the URL one.
	let blobs: Vec<&str> = ["joinScript", "joinScriptUrl", "joinTicket"]
		.iter()
		.filter_map(|key| v.get(*key).and_then(|s| s.as_str()))
		.collect();
	for blob in blobs {
		let Some(ip) = find_tag(blob, "MachineAddress")
			.or_else(|| find_json_str(blob, "ip"))
			.or_else(|| find_json_str(blob, "address"))
		else {
			continue;
		};
		let Some(port) = find_tag(blob, "MachinePort")
			.and_then(|p| p.parse::<u16>().ok())
			.or_else(|| find_json_num(blob, "port"))
		else {
			continue;
		};
		if !ip.is_empty() && port > 0 {
			return Some((ip, port));
		}
	}
	None
}

/// First usable (Address, Port) pair of a joinScript endpoint array.
pub(crate) fn first_endpoint_list(v: Option<&serde_json::Value>) -> Option<(String, u16)> {
	let arr = v?.as_array()?;
	for item in arr {
		let ip = item.get("Address").and_then(|s| s.as_str()).unwrap_or("");
		let port = item.get("Port").and_then(|p| p.as_u64()).unwrap_or(0);
		if !ip.is_empty() && port > 0 && port <= u64::from(u16::MAX) {
			return Some((ip.to_string(), port as u16));
		}
	}
	None
}

/// Value of `<Tag>value</Tag>` (lenient about the exact join-script dialect).
fn find_tag(hay: &str, tag: &str) -> Option<String> {
	let close = format!("</{}>", tag);
	let after_open = |after: &str| -> Option<String> {
		let value = match after.find(&close) {
			Some(b) => &after[..b],
			None => after.split(['<', '"']).next().unwrap_or(""),
		};
		let value = value.trim();
		if value.is_empty() {
			None
		} else {
			Some(value.to_string())
		}
	};
	// Plain form: <Tag>value</Tag>
	let open = format!("<{}>", tag);
	if let Some(a) = hay.find(&open) {
		if let Some(value) = after_open(&hay[a + open.len()..]) {
			return Some(value);
		}
	}
	// Attribute form: <Tag attr="...">value</Tag>
	let open2 = format!("<{} ", tag);
	if let Some(a) = hay.find(&open2) {
		let rest = &hay[a + open2.len()..];
		let end = rest.find('>')?;
		if !rest[..end].ends_with('/') {
			if let Some(value) = after_open(&rest[end + 1..]) {
				return Some(value);
			}
		}
	}
	None
}

/// Value of `"key":"value"` in a JSON-ish string.
fn find_json_str(hay: &str, key: &str) -> Option<String> {
	let pat = format!("\"{}\"", key);
	let a = hay.find(&pat)?;
	let rest = &hay[a + pat.len()..];
	let colon = rest.find(':')?;
	let rest = rest[colon + 1..].trim_start().strip_prefix('"')?;
	let end = rest.find('"')?;
	let value = rest[..end].trim();
	if value.is_empty() {
		None
	} else {
		Some(value.to_string())
	}
}

/// Numeric value of `"key":123` in a JSON-ish string.
fn find_json_num(hay: &str, key: &str) -> Option<u16> {
	let pat = format!("\"{}\"", key);
	let a = hay.find(&pat)?;
	let rest = &hay[a + pat.len()..];
	let colon = rest.find(':')?;
	let rest = rest[colon + 1..].trim_start();
	let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
	digits.parse().ok()
}
