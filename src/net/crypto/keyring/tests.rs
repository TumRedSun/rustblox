//! Tests transcribed from a live join-script `ClientPublicKeyData` value.

#![allow(clippy::expect_used)]

use super::*;

#[test]
fn keyring_parses_the_live_join_json() {
	let (versions, send) = parse_keyring(
        r#"{"creationTime":"11:55 12/6/2022","applications":{"RakNetEarlyPublicKey":{"versions":[{"id":5,"value":"Jy0uXN86HFuODRudIzEzZeLfCd8/z+ER6CMO0wAa5XY=","allowed":true}],"send":5,"revert":5}}}"#,
    )
    .expect("the live keyring must parse");
	assert_eq!(send, Some(5));
	assert_eq!(versions.len(), 1);
	assert_eq!(versions[0].id, 5);
	assert!(versions[0].allowed);
	assert_eq!(
		versions[0].key,
		[
			0x27, 0x2d, 0x2e, 0x5c, 0xdf, 0x3a, 0x1c, 0x5b, 0x8e, 0x0d, 0x1b, 0x9d, 0x23, 0x31,
			0x33, 0x65, 0xe2, 0xdf, 0x09, 0xdf, 0x3f, 0xcf, 0xe1, 0x11, 0xe8, 0x23, 0x0e, 0xd3,
			0x00, 0x1a, 0xe5, 0x76
		]
	);
}
