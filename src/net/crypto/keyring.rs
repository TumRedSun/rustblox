//! The join script's `ClientPublicKeyData` keyring JSON
//! (`applications.RakNetEarlyPublicKey.versions[]`).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

/// One `RakNetEarlyPublicKey` version from the join script's
/// `ClientPublicKeyData` keyring JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyringVersion {
	pub id: u64,
	pub key: Vec<u8>,
	pub allowed: bool,
}

/// Parse the keyring JSON (`ClientPublicKeyData`): applications →
/// `RakNetEarlyPublicKey` → versions[]. Returns `(versions, send_id)`.
pub fn parse_keyring(cpkd: &str) -> Option<(Vec<KeyringVersion>, Option<u64>)> {
	let v: serde_json::Value = serde_json::from_str(cpkd).ok()?;
	let app = v.get("applications")?.get("RakNetEarlyPublicKey")?.clone();
	let mut versions = Vec::new();
	if let Some(arr) = app.get("versions").and_then(|a| a.as_array()) {
		for ver in arr {
			let id = ver.get("id").and_then(|x| x.as_u64()).unwrap_or(0);
			let b64 = ver.get("value").and_then(|x| x.as_str()).unwrap_or("");
			let key = crate::roblox::udmux::b64_decode(b64);
			if key.is_empty() {
				continue;
			}
			let allowed = ver.get("allowed").and_then(|x| x.as_bool()).unwrap_or(true);
			versions.push(KeyringVersion { id, key, allowed });
		}
	}
	let send = app.get("send").and_then(|x| x.as_u64());
	if versions.is_empty() {
		None
	} else {
		Some((versions, send))
	}
}

#[cfg(test)]
#[path = "keyring/tests.rs"]
mod tests;
