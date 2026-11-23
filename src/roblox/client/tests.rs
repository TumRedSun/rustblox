//! Tests for the raw response wrapper.

use super::*;

#[test]
fn raw_header_lookup_is_case_insensitive() {
	let raw = RawResponse {
		status: 200,
		body: b"{}".to_vec(),
		csrf: None,
		set_cookies: Vec::new(),
		headers: vec![
			("content-type".to_string(), "application/json".to_string()),
			("RBX-Authentication-Ticket".to_string(), "TICKET".to_string()),
		],
	};
	assert_eq!(raw.header("rbx-authentication-ticket"), Some("TICKET"));
	assert_eq!(raw.header("RBX-AUTHENTICATION-TICKET"), Some("TICKET"));
	assert_eq!(raw.header("content-TYPE"), Some("application/json"));
	assert_eq!(raw.header("missing"), None);
}

#[test]
fn cookie_lookup_skips_empty_values_and_trims_the_name() {
	// An empty value on the first matching line must not win: the lookup
	// walks the remaining `Set-Cookie` lines. The cookie name is compared
	// after `trim()`, so surrounding whitespace (e.g. after the comma a
	// folded header leaves `" .ROBLOSECURITY"`) still matches.
	let raw = RawResponse {
		status: 200,
		body: Vec::new(),
		csrf: None,
		set_cookies: vec![
			".ROBLOSECURITY=; Path=/; Domain=.roblox.com".to_string(),
			" .ROBLOSECURITY=REAL; Path=/; Domain=.roblox.com".to_string(),
		],
		headers: Vec::new(),
	};
	assert_eq!(raw.header_cookies_find(".ROBLOSECURITY").as_deref(), Some("REAL"));
	// A single empty value with no fallback still yields `None`.
	let only_empty = RawResponse {
		status: 200,
		body: Vec::new(),
		csrf: None,
		set_cookies: vec![".ROBLOSECURITY=; Path=/".to_string()],
		headers: Vec::new(),
	};
	assert_eq!(only_empty.header_cookies_find(".ROBLOSECURITY"), None);
}

#[test]
fn cookie_lookup_matches_name_case_insensitively() {
	let raw = RawResponse {
		status: 200,
		body: Vec::new(),
		csrf: None,
		set_cookies: vec![
			".ROBLOSECURITY=TOKEN; Path=/; Domain=.roblox.com".to_string(),
			"RBXSession=OTHER; Path=/".to_string(),
		],
		headers: Vec::new(),
	};
	assert_eq!(raw.header_cookies_find(".ROBLOSECURITY").as_deref(), Some("TOKEN"));
	assert_eq!(raw.header_cookies_find(".roblosecurity").as_deref(), Some("TOKEN"));
	assert_eq!(raw.header_cookies_find("RBXSession").as_deref(), Some("OTHER"));
	assert_eq!(raw.header_cookies_find("missing"), None);
}
