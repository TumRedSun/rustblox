#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;

#[test]
fn known_prefixes_map_to_their_proxy() {
	// Frankfurt — the 10.9.#.# datacenter from the devforum table.
	assert_eq!(proxy_for("10.9.5.206".parse().unwrap()), Some("128.116.44.33".parse().unwrap()));
	assert_eq!(proxy_for("10.208.1.2".parse().unwrap()), Some("128.116.49.254".parse().unwrap()));
	// Prefix 17 from the Nov-2026 Sober capture (RCC 10.17.6.165).
	assert_eq!(proxy_for("10.17.6.165".parse().unwrap()), Some("128.116.21.33".parse().unwrap()));
}

#[test]
fn public_addresses_have_no_proxy() {
	assert!(!is_private("128.116.44.33".parse().unwrap()));
	assert_eq!(proxy_for("128.116.44.33".parse().unwrap()), None);
	assert_eq!(proxy_for("192.168.1.1".parse().unwrap()), None);
	assert!(is_private("192.168.1.1".parse().unwrap()));
}
