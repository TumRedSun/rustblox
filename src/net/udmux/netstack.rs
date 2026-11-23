//! The join script's `NetStackConfig` blob parser.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

/// Best-effort parse of the join script's `NetStackConfig` blob. Roblox has
/// shipped it both as a JSON object and as a flat `k=v;k=v` (or `k:v,k:v`)
/// string; neither form is documented. Returns the key/value pairs we could
/// dig out, in order. Empty input yields an empty list; an unparsable blob
/// yields a single pair with an empty key so the raw value stays visible.
pub fn parse_netstack_config(cfg: &str) -> Vec<(String, String)> {
	let s = cfg.trim();
	if s.is_empty() {
		return Vec::new();
	}
	if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
		if let Some(obj) = v.as_object() {
			return obj
				.iter()
				.map(|(k, val)| {
					let val = match val {
						serde_json::Value::String(t) => t.clone(),
						other => other.to_string(),
					};
					(k.clone(), val)
				})
				.collect();
		}
	}
	// Flat form: pairs separated by `;` or `,`, key/value split on the
	// first `=` or `:`. A pair without a separator is kept verbatim under
	// an empty key.
	let mut out = Vec::new();
	for part in s.split([';', ',']) {
		let part = part.trim();
		if part.is_empty() {
			continue;
		}
		match part.find(['=', ':']) {
			Some(p) => out.push((part[..p].trim().to_string(), part[p + 1..].trim().to_string())),
			None => out.push((String::new(), part.to_string())),
		}
	}
	out
}

#[cfg(test)]
#[path = "netstack/tests.rs"]
mod tests;
