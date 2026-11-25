//! Per-session routing-token encoding: the join script's base64 and
//! percent-encoded blobs, decoded without pulling in a whole crate.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

/// Decode standard and url-safe base64, ignoring padding and whitespace.
/// Returns an empty vec when the input contains characters outside both
/// alphabets — the caller then keeps the raw ASCII as the candidate token.
pub fn b64_decode(s: &str) -> Vec<u8> {
	let mut acc: u32 = 0;
	let mut bits: u32 = 0;
	let mut out = Vec::with_capacity(s.len() / 4 * 3);
	for ch in s.chars() {
		let v = match ch {
			'A'..='Z' => ch as u32 - 'A' as u32,
			'a'..='z' => ch as u32 - 'a' as u32 + 26,
			'0'..='9' => ch as u32 - '0' as u32 + 52,
			'+' | '-' => 62,
			'/' | '_' => 63,
			'=' | '\r' | '\n' | ' ' | '\t' => continue,
			_ => return Vec::new(),
		};
		acc = (acc << 6) | v;
		bits += 6;
		if bits >= 8 {
			bits -= 8;
			out.push((acc >> bits) as u8);
		}
	}
	out
}

/// Percent-decode a join-script string (`%XX` sequences). `+` is kept
/// verbatim: these blobs are base64 first and query strings second — the
/// live join script encodes a literal `+` as `%2B`, so a bare `+` must not
/// become a space. Undecoded input passes through unchanged, which makes
/// this safe to apply unconditionally (EphemeralEarlyPubKey arrives
/// percent-encoded and used to fail base64 decoding with `%` in alphabet).
pub fn percent_decode(s: &str) -> String {
	let b = s.as_bytes();
	let mut out = Vec::with_capacity(b.len());
	let mut i = 0;
	while i < b.len() {
		if b[i] == b'%' && i + 2 < b.len() {
			let hi = (b[i + 1] as char).to_digit(16);
			let lo = (b[i + 2] as char).to_digit(16);
			if let (Some(hi), Some(lo)) = (hi, lo) {
				out.push((hi * 16 + lo) as u8);
				i += 3;
				continue;
			}
		}
		out.push(b[i]);
		i += 1;
	}
	String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
#[path = "token/tests.rs"]
mod tests;
