//! Menu tests (moved out of the module body, docs/ARCHITECTURE.md).

use super::*;
use crate::model::format_count;

#[test]
fn log_line_split_keeps_the_first_character() {
	// Regression: the message used to start one character too far in
	// ("log file" rendered as "og file", "Join" as "oin").
	let (head, msg) = super::split_log_line("[12:06:06] [app] log file: /tmp/client.log");
	assert_eq!(head, "[12:06:06] [app] ");
	assert_eq!(msg, "log file: /tmp/client.log");

	let (head, msg) = super::split_log_line("[12:07:14] [net] Auth ticket received (576 chars)");
	assert_eq!(head, "[12:07:14] [net] ");
	assert_eq!(msg, "Auth ticket received (576 chars)");

	// A "] " inside the message must not confuse the split.
	let (head, msg) = super::split_log_line("[00:00:00] [gfx] wgpu error [bad] stuff");
	assert_eq!(head, "[00:00:00] [gfx] ");
	assert_eq!(msg, "wgpu error [bad] stuff");

	// No stamp/kind header → the whole line is the message.
	let (head, msg) = super::split_log_line("just a line");
	assert!(head.is_empty());
	assert_eq!(msg, "just a line");
}

#[test]
fn clamp_scroll_basics() {
	// Nothing to scroll.
	assert_eq!(clamp_scroll(10.0, 100.0, 200.0), 0.0);
	// Within range stays.
	assert_eq!(clamp_scroll(30.0, 300.0, 200.0), 30.0);
	// Negative and overflow clamp.
	assert_eq!(clamp_scroll(-5.0, 300.0, 200.0), 0.0);
	assert_eq!(clamp_scroll(500.0, 300.0, 200.0), 100.0);
}

#[test]
fn count_formatting() {
	assert_eq!(format_count(98), "98");
	assert_eq!(format_count(1_626), "1.6K");
	assert_eq!(format_count(12_345), "12K");
	assert_eq!(format_count(1_234_567), "1.2M");
}

#[test]
fn color_cycling() {
	let a = COLOR_PALETTE[0];
	let b = COLOR_PALETTE[1];
	assert_eq!(cycle_color(a, 1), b);
	assert_eq!(cycle_color(b, -1), a);
	// Wraps both directions.
	let last = COLOR_PALETTE[COLOR_PALETTE.len() - 1];
	assert_eq!(cycle_color(last, 1), COLOR_PALETTE[0]);
	assert_eq!(cycle_color(COLOR_PALETTE[0], -1), last);
	// Unknown color enters the palette at the front (next) / back (prev).
	let unknown = [1u8, 2, 3, 4];
	assert_eq!(cycle_color(unknown, 1), COLOR_PALETTE[0]);
	assert_eq!(cycle_color(unknown, -1), last);
}
