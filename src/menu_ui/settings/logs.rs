//! Settings / Logs: the in-app log viewer.

use crate::app::UiCtx;
use crate::draw::FB;
use crate::font;
use crate::font::Fonts;
#[allow(unused_imports)]
use crate::menu_ui::*;
use crate::theme;

/// Diagnostic log pane: every buffered line, greedy-wrapped to the pane
/// width. Newest lines sit at the bottom; the follow flag pins the view to
/// the tail until the user scrolls up.
pub(crate) const LOG_LINE_H: f32 = 17.0;
pub(crate) fn measure_logs(fonts: &Fonts, inner_w: f32, s: f32) -> f32 {
	let px = 11.5 * s;
	let line_h = LOG_LINE_H * s;
	let mut h = 8.0 * s;
	let lines = crate::logs::lines();
	if lines.is_empty() {
		// Keep in sync with draw_logs' empty state (the hint wraps too).
		let segs = font::wrap_text(
			fonts,
			"Try joining a place — every request, response and error lands here.",
			inner_w.max(32.0),
			px,
			false,
		);
		h += (segs.len() + 1) as f32 * line_h;
	} else {
		for l in &lines {
			h += log_layout(fonts, l, inner_w, px).2.len() as f32 * line_h;
		}
	}
	h + 12.0 * s
}
pub(crate) fn draw_logs(ctx: &mut UiCtx, fb: &mut FB, x: i32, y: &mut f32, w: f32, s: f32) {
	let t = theme::cur();
	let px = 11.5 * s;
	let line_h = LOG_LINE_H * s;
	section_label(ctx, fb, x, y, "DIAGNOSTIC LOG", s);
	let lines = crate::logs::lines();
	if lines.is_empty() {
		font::draw(
			ctx.fonts,
			fb,
			x as f32,
			*y + 14.0 * s,
			"No events yet.",
			px,
			t.text_faint,
			false,
			0.0,
		);
		let hint_h = font::draw_wrapped(
			ctx.fonts,
			fb,
			x as f32,
			*y + 14.0 * s + line_h,
			"Try joining a place — every request, response and error lands here.",
			px,
			t.text_faint,
			false,
			w.max(32.0),
			line_h,
		);
		*y += line_h + hint_h;
		return;
	}
	for l in &lines {
		let (head, head_w, segs) = log_layout(ctx.fonts, l, w, px);
		for (k, seg) in segs.iter().enumerate() {
			let (sx, base) =
				if k == 0 { (x as f32 + head_w, *y) } else { (x as f32, *y + k as f32 * line_h) };
			if seg.is_empty() {
				continue;
			}
			let color = if k == 0 { t.text } else { t.text_dim };
			if k == 0 && !head.is_empty() {
				font::draw(
					ctx.fonts,
					fb,
					x as f32,
					*y + px * 0.85,
					&head,
					px,
					t.text_dim,
					false,
					0.0,
				);
			}
			font::draw(ctx.fonts, fb, sx, base + px * 0.85, seg, px, color, false, 0.0);
		}
		*y += segs.len() as f32 * line_h;
	}
}
/// Split a stored log line "[HH:MM:SS] [kind] message" into the dim header
/// (timestamp + kind tag + the space that follows) and the message, which
/// starts right after that space. Pure string logic so it is unit-testable
/// without a font.
pub(crate) fn split_log_line(line: &str) -> (String, &str) {
	let Some(fb1) = line.find(']') else { return (String::new(), line) };
	let rest = &line[fb1 + 1..];
	match rest.find("] ") {
		// `second` is the byte index of the space between the kind tag and
		// the message; the header keeps that space, so the message starts
		// at `second + 1`. An earlier `+ 2` here ate the first character
		// of every message ("log file" rendered as "og file").
		Some(off) => {
			let second = fb1 + 1 + off + 1;
			(line[..second + 1].to_string(), &line[second + 1..])
		}
		None => (String::new(), line),
	}
}
/// Split a stored log line "[HH:MM:SS] [kind] message" into the dim header
/// (timestamp + kind tag) and the message, then greedy-wrap the message to
/// the pane width (long words are broken per character). The first visual
/// line starts after the header, the rest span the full width. Used for
/// BOTH measuring and drawing so the scroll height can never drift.
pub(crate) fn log_layout(
	fonts: &Fonts,
	line: &str,
	max_w: f32,
	px: f32,
) -> (String, f32, Vec<String>) {
	let space_w = font::measure(fonts, " ", px, false);
	let (head, msg) = split_log_line(line);
	let head_w =
		if head.is_empty() { 0.0 } else { font::measure(fonts, &head, px, false) + space_w };
	let first_w = (max_w - head_w).max(32.0);
	let mut segs: Vec<String> = Vec::new();
	let mut cur = String::new();
	let mut cur_w = 0.0f32;
	let mut cap = first_w;
	let flush = |segs: &mut Vec<String>, cur: &mut String, cur_w: &mut f32| {
		segs.push(std::mem::take(cur));
		*cur_w = 0.0;
	};
	for word in msg.split(' ') {
		if word.is_empty() {
			continue;
		}
		let ww = font::measure(fonts, word, px, false);
		if !cur.is_empty() && cur_w + space_w + ww > cap {
			flush(&mut segs, &mut cur, &mut cur_w);
			cap = max_w;
		}
		if !cur.is_empty() {
			cur.push(' ');
			cur_w += space_w;
		}
		if ww > cap {
			// Hard-break a word wider than the whole line.
			for ch in word.chars() {
				let c = ch.to_string();
				let cw = font::measure(fonts, &c, px, false);
				if cur_w + cw > cap && !cur.is_empty() {
					flush(&mut segs, &mut cur, &mut cur_w);
					cap = max_w;
				}
				cur.push_str(&c);
				cur_w += cw;
			}
		} else {
			cur.push_str(word);
			cur_w += ww;
		}
	}
	let _ = cur_w;
	if !cur.is_empty() {
		segs.push(cur);
	}
	if segs.is_empty() {
		segs.push(String::new());
	}
	(head, head_w, segs)
}
