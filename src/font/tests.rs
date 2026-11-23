//! Font tests (moved out of the module body, docs/ARCHITECTURE.md).

use super::*;

#[test]
fn wrap_respects_max_width() {
	let fonts = Fonts::load();
	let long = "Type at least 2 characters of a username and press Enter";
	// Narrow phone pane: the sentence must split into several lines.
	let lines = wrap(&fonts, long, 160.0, 14.0, false);
	assert!(lines.len() > 1, "expected wrapping, got {:?}", lines);
	for l in &lines {
		assert!(measure(&fonts, l, 14.0, false) <= 160.0, "line too wide: {:?}", l);
	}
	// Wide pane: stays a single line, unchanged.
	let one = wrap(&fonts, long, 600.0, 14.0, false);
	assert_eq!(one.len(), 1);
	assert_eq!(one[0], long);
	// Existing newlines are preserved as hard breaks.
	let two = wrap(&fonts, "alpha\nbeta", 600.0, 14.0, false);
	assert_eq!(two, vec!["alpha".to_string(), "beta".to_string()]);
	// A word wider than the whole line is hard-broken to fit.
	let hard = wrap(&fonts, "AAAAAAAAAAAAAAAAAAAA", 50.0, 14.0, false);
	assert!(hard.len() > 1);
	for l in &hard {
		assert!(measure(&fonts, l, 14.0, false) <= 51.0, "hard line too wide: {:?}", l);
	}
}

#[test]
fn wrap_respects_width() {
	let fonts = Fonts::load();
	let px = 14.0;
	let one = measure(&fonts, "one", px, false);
	let two = measure(&fonts, "one two", px, false);
	// Fits on a single line.
	assert_eq!(wrap_text(&fonts, "one two", two + 1.0, px, false), vec!["one two"]);
	// Two lines once the width forces a break.
	assert_eq!(wrap_text(&fonts, "one two", two - 1.0, px, false), vec!["one", "two"]);
	// Width that only fits one word per line.
	assert_eq!(wrap_text(&fonts, "one two", one + 1.0, px, false), vec!["one", "two"]);
}

#[test]
fn wrap_hard_breaks_overlong_words() {
	let fonts = Fonts::load();
	let px = 14.0;
	let c = measure(&fonts, "a", px, false);
	// 10 'a's into a 4-char-wide box: hard-broken, never overflowing.
	let lines = wrap_text(&fonts, "aaaaaaaaaa", c * 4.0 + 0.5, px, false);
	assert!(lines.len() >= 3);
	for l in &lines {
		assert!(measure(&fonts, l, px, false) <= c * 4.0 + 0.6);
	}
	assert_eq!(lines.concat(), "aaaaaaaaaa");
}

#[test]
fn wrap_handles_unicode_and_blanks() {
	let fonts = Fonts::load();
	let px = 12.0;
	// Multi-byte text must not be split mid-char; rejoining the lines
	// with spaces must restore the original text.
	let w_word = measure(&fonts, "привет", px, false);
	let lines = wrap_text(&fonts, "привет мир привет мир", w_word + 2.0, px, false);
	assert!(lines.len() >= 2);
	assert_eq!(lines.join(" "), "привет мир привет мир");
	// Blank input -> one empty line (so callers can advance y safely).
	assert_eq!(wrap_text(&fonts, "", 100.0, px, false), vec![""]);
	// Degenerate width: passthrough, no panic.
	assert_eq!(wrap_text(&fonts, "abc", 0.0, px, false), vec!["abc"]);
}
