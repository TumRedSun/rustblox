use super::*;

#[test]
fn b64_decode_handles_standard_and_urlsafe() {
	assert_eq!(b64_decode("SGVsbG8="), b"Hello");
	assert_eq!(b64_decode("SGVsbG8"), b"Hello");
	// 24 chars -> exactly 18 raw bytes, the join-script token shape.
	assert_eq!(b64_decode("AAAAAAAAAAAAAAAAAAAAAAAA"), vec![0u8; 18]);
	// url-safe alphabet decodes the same as the standard one.
	assert_eq!(b64_decode("-_-_"), b64_decode("+/+/"));
	// Non-base64 input yields an empty vec (caller falls back to ascii).
	assert!(b64_decode("token with spaces!#?").is_empty());
}

#[test]
fn percent_decode_handles_the_join_script_encoding() {
	// EphemeralEarlyPubKey verbatim from the live join_full.json
	// (Nov 2026): %2B = '+', %2F = '/', %3D = '=' — plain base64 below.
	assert_eq!(
		percent_decode("M17S4M%2Bk%2FCjattHW2WMEufaCnwrgd6itqO%2FVngg5R0k%3D"),
		"M17S4M+k/CjattHW2WMEufaCnwrgd6itqO/Vngg5R0k="
	);
	// Already-decoded input passes through; '+' is not a space here.
	assert_eq!(percent_decode("a+b=="), "a+b==");
	// A trailing lone % stays literal.
	assert_eq!(percent_decode("100%"), "100%");
}
