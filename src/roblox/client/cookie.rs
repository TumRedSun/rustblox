//! Normalization of user-pasted `.ROBLOSECURITY` cookies.

/// Normalize a pasted `.ROBLOSECURITY` value: accepts the bare token, a full
/// `Set-Cookie` line, a `Cookie:` header, or a token with the anti-sharing
/// warning prefix (both `|_token` and `|token` endings are handled).
pub fn normalize_cookie_token(input: &str) -> String {
	let mut t = input.trim().trim_matches('"').trim().to_string();
	if let Some(pos) = t.find(".ROBLOSECURITY=") {
		let rest = t[pos + ".ROBLOSECURITY=".len()..].to_string();
		let cut = rest.find(';').unwrap_or(rest.len());
		t = rest[..cut].trim().to_string();
	}
	if t.starts_with('_') && t.contains("WARNING") {
		if let Some(pos) = t.rfind('|') {
			let rest = t[pos + 1..].trim_start_matches('_');
			if !rest.is_empty() {
				t = rest.to_string();
			}
		}
	}
	t.trim().to_string()
}
