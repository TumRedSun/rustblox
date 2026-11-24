//! Verified decoder for the application-level **join item lists**.
//!
//! Live capture (24 Nov 2026, Android client 2.748.1527) shows that the
//! client's game-join upload contains three messages whose bodies are plain
//! counted arrays of 16-byte opaque items — not compressed and not encrypted:
//!
//! ```text
//! 0xa7 | u16 LE count | count × 16 bytes
//! 0x90 |               count × 16 bytes   (count = (len - 1) / 16)
//! 0x8a |               count × 16 bytes   (count = (len - 1) / 16)
//! ```
//!
//! Evidence (all four sessions byte-checked):
//! * `a7` 7363 B → count 460, `(len-3)/16 == 460`, rem 0; a second session
//!   7443 B → count 465, rem 0.
//! * `90` 13025 B → 814 items; 13153 B → 822 items; rem 0 in both.
//! * `8a` 1745 B → 109 items; 1793 B → 112 items; rem 0 in both.
//! * no zstd/zlib/lz4/brotli frame anywhere in the bodies; entropy ~7.9 is
//!   the natural entropy of random 16-byte hashes/GUIDs.
//!
//! The 16-byte items are opaque to this decoder: the evidence proves the
//! container, not their semantics (`docs/GAME-JOIN-RE.md` records the open
//! question). `0xa7` is largely persistent across sessions (431/460 items
//! identical in the compared pair, divergence starts at item 429), while
//! `0x90`/`0x8a` are per-session.

/// Length of one opaque join item.
pub const JOIN_ITEM_LEN: usize = 16;

/// A decoded join item list (message id + opaque 16-byte items).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinItemList {
	pub message_id: u8,
	pub items: Vec<[u8; JOIN_ITEM_LEN]>,
}

/// Decode one join item list, or `None` when the head/length is not the
/// verified shape. Never panics, never guesses a partial list.
pub fn parse(payload: &[u8]) -> Option<JoinItemList> {
	let (&message_id, rest) = payload.split_first()?;
	match message_id {
		0xa7 => {
			if rest.len() < 2 {
				return None;
			}
			let count = usize::from(u16::from_le_bytes([rest[0], rest[1]]));
			let body = &rest[2..];
			if body.len() != count * JOIN_ITEM_LEN {
				return None;
			}
			Some(JoinItemList { message_id, items: collect_items(body) })
		}
		0x90 | 0x8a => {
			if rest.len() % JOIN_ITEM_LEN != 0 {
				return None;
			}
			Some(JoinItemList { message_id, items: collect_items(rest) })
		}
		_ => None,
	}
}

fn collect_items(body: &[u8]) -> Vec<[u8; JOIN_ITEM_LEN]> {
	body.chunks_exact(JOIN_ITEM_LEN)
		.filter_map(|chunk| <[u8; JOIN_ITEM_LEN]>::try_from(chunk).ok())
		.collect()
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
	use super::*;

	#[test]
	fn a7_counted_list() {
		// First two items of the live `join_plain/a7_321.bin` capture.
		let mut p = vec![0xa7, 0x02, 0x00];
		p.extend_from_slice(&hex("7eaf79ea0fad0f8be223e179aaae641b"));
		p.extend_from_slice(&hex("36f4e73e06ed717bacd2594f527a3351"));
		let l = parse(&p).expect("valid a7 list");
		assert_eq!(l.message_id, 0xa7);
		assert_eq!(l.items.len(), 2);
		assert_eq!(l.items[0], hex16("7eaf79ea0fad0f8be223e179aaae641b"));
		// Count disagreeing with the body length is rejected, not partially read.
		let mut bad = p.clone();
		bad[1] = 0x03;
		assert!(parse(&bad).is_none());
	}

	#[test]
	fn head_only_lists() {
		for id in [0x90u8, 0x8a] {
			let mut p = vec![id];
			p.extend_from_slice(&hex("c23d3c2d341d3a5415c5489f397c692d"));
			p.extend_from_slice(&hex("bc33024cad1c975ceaf42b8ead128722"));
			p.extend_from_slice(&hex("22d1288b6a2fc0090fcd84be50857c61"));
			let l = parse(&p).expect("valid head-only list");
			assert_eq!(l.items.len(), 3);
			assert_eq!(l.items[2], hex16("22d1288b6a2fc0090fcd84be50857c61"));
			// A tail that is not a whole number of items is rejected.
			let mut bad = p.clone();
			bad.push(0);
			assert!(parse(&bad).is_none());
		}
	}

	fn hex(s: &str) -> Vec<u8> {
		let mut out = Vec::new();
		let b = s.as_bytes();
		let mut i = 0;
		while i + 1 < b.len() {
			let hi = (b[i] as char).to_digit(16).expect("hex");
			let lo = (b[i + 1] as char).to_digit(16).expect("hex");
			out.push((hi * 16 + lo) as u8);
			i += 2;
		}
		out
	}

	fn hex16(s: &str) -> [u8; JOIN_ITEM_LEN] {
		let v = hex(s);
		<[u8; JOIN_ITEM_LEN]>::try_from(v.as_slice()).expect("16 bytes")
	}
}
