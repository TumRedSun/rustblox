//! Log ring-buffer tests (moved out of the module body, docs/ARCHITECTURE.md).

use super::*;
use std::collections::VecDeque;

#[test]
fn push_into_split_clamp_and_cap() {
	let mut buf = VecDeque::new();
	push_into(&mut buf, "[00:00:00]", "test", "hello\nworld\n\n");
	assert_eq!(buf.len(), 2);
	assert!(buf[0].contains("[test] hello"), "{buf:?}");
	assert!(buf[1].contains("world"), "{buf:?}");

	// Overlong lines are clamped with an ellipsis (the "[stamp] [kind] "
	// prefix adds up to ~20 chars on top of the clamped body).
	push_into(&mut buf, "[00:00:01]", "test", &"x".repeat(2000));
	let last = buf.back().unwrap();
	assert!(last.chars().count() <= MAX_LINE_CHARS + 20, "{}", last.chars().count());
	assert!(last.ends_with('…'), "{last:?}");

	// Ring buffer capacity: oldest entries are evicted.
	for i in 0..(CAP + 50) {
		push_into(&mut buf, "[00:00:02]", "test", &format!("line {i}"));
	}
	assert_eq!(buf.len(), CAP);
	assert!(buf.back().unwrap().contains(&format!("line {}", CAP + 49)));
	// 453 entries in total: the first 3 (hello/world/xxx) and lines
	// 0..49 were evicted, so "line 50" now fronts the buffer.
	assert!(buf.front().unwrap().contains("line 50"), "{:?}", buf.front());
}

#[test]
fn stamp_format() {
	let s = stamp();
	assert!(s.starts_with('[') && s.ends_with(']'), "{s}");
	assert_eq!(s.len(), 10);
	for (i, c) in s.chars().enumerate() {
		match i {
			3 | 6 => assert_eq!(c, ':', "{s}"),
			1 | 2 | 4 | 5 | 7 | 8 => assert!(c.is_ascii_digit(), "{s}"),
			_ => {}
		}
	}
}
