use super::*;

#[test]
fn netstack_config_parses_json_and_flat_forms() {
	// JSON form (one of the two shapes Roblox ships).
	let pairs = parse_netstack_config(
		r#"{"port":64469,"mtuSize":1200,"canUseAesGcm":true,"protocolVersion":"1"}"#,
	);
	assert!(pairs.contains(&("port".to_string(), "64469".to_string())));
	assert!(pairs.contains(&("mtuSize".to_string(), "1200".to_string())));
	assert!(pairs.contains(&("canUseAesGcm".to_string(), "true".to_string())));
	assert!(pairs.contains(&("protocolVersion".to_string(), "1".to_string())));
	// Flat `;`-separated form with `=` and `:` separators mixed.
	let flat = "mtuSize=1200; canUseAesGcm:true ,protocolVersion=1";
	let pairs = parse_netstack_config(flat);
	assert!(pairs.contains(&("mtuSize".to_string(), "1200".to_string())));
	assert!(pairs.contains(&("canUseAesGcm".to_string(), "true".to_string())));
	assert!(pairs.contains(&("protocolVersion".to_string(), "1".to_string())));
	assert!(parse_netstack_config("").is_empty());
	assert!(parse_netstack_config("   ").is_empty());
}
