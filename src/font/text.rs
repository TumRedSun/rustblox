//! Text measurement, wrapping and drawing (both backends).

use super::{Fonts, GlyphKey, GlyphMask};
use crate::draw::FB;
use crate::theme::Rgba;
use ab_glyph::{Font, FontArc, Glyph, PxScale, ScaleFont};

/// Width of `text` when rendered at `px` pixel size.
pub fn measure(fonts: &Fonts, text: &str, px: f32, bold: bool) -> f32 {
	let font = fonts.face(bold);
	let scaled = font.as_scaled(PxScale::from(px));
	let mut w = 0.0f32;
	for ch in text.chars() {
		let id = font.glyph_id(ch);
		w += scaled.h_advance(id);
	}
	w
}

/// Rasterize one glyph into an alpha mask (or None for whitespace).
fn rasterize(font: &FontArc, px: f32, ch: char) -> Option<GlyphMask> {
	let id = font.glyph_id(ch);
	if id.0 == 0 {
		return None;
	}
	let scale = PxScale::from(px);
	let glyph = id.with_scale(scale);
	let outlined = font.outline_glyph(glyph)?;
	let bounds = outlined.px_bounds();
	let w = bounds.width().ceil().max(1.0) as u16;
	let h = bounds.height().ceil().max(1.0) as u16;
	let mut data = vec![0u8; w as usize * h as usize];
	// ab_glyph's OutlinedGlyph::draw yields (u32, u32) coordinates that are
	// ALREADY relative to the outline bounds origin (0..w, 0..h) — they are
	// NOT pen-relative, so bounds.min must NOT be subtracted here (doing so
	// pushes every row out of the mask and renders empty glyphs).
	outlined.draw(|gx, gy, alpha| {
		let (ix, iy) = (gx as usize, gy as usize);
		if ix < w as usize && iy < h as usize {
			let idx = iy * w as usize + ix;
			data[idx] = (alpha * 255.0).round().min(255.0) as u8;
		}
	});
	Some(GlyphMask {
		key: GlyphKey { bold: false, px_x4: 0, ch: '\0' }, // filled by caller
		x: 0,
		y: 0,
		w,
		h,
		data,
		left: bounds.min.x,
		top: bounds.min.y,
	})
}

/// Draw text with baseline at `y`. Returns the advance width (including
/// letter_spacing).
pub fn draw(
	fonts: &Fonts,
	fb: &mut FB,
	mut x: f32,
	y_baseline: f32,
	text: &str,
	px: f32,
	color: Rgba,
	bold: bool,
	letter_spacing: f32,
) -> f32 {
	let font = fonts.face(bold);
	let scale = PxScale::from(px);
	let scaled = font.as_scaled(scale);
	for ch in text.chars() {
		let id = font.glyph_id(ch);
		if fb.is_gpu() {
			// GPU path: rasterize once, place in the atlas, emit a quad.
			let key = GlyphKey { bold, px_x4: (px * 4.0).round() as u16, ch };
			if id.0 != 0 {
				let placed = match fonts.atlas.lock() {
					Ok(mut atlas) => {
						if let Some(slot) = atlas.slots.get(&key) {
							Some(*slot)
						} else if let Some(mut mask) = rasterize(font, px, ch) {
							mask.key = key;
							atlas.place(key, mask)
						} else {
							None
						}
					}
					// A poisoned atlas skips one glyph; text must never take
					// the UI down through a mutex unwrap.
					Err(_) => None,
				};
				if let Some(slot) = placed {
					// The slot's left/top are pen-relative from the glyph
					// bounds (baseline-based).
					fb.push_glyph(x + slot.left, y_baseline + slot.top, slot, color);
				}
			}
		} else {
			// Software path: straight into the buffer (as before).
			let glyph: Glyph = id.with_scale_and_position(scale, ab_glyph::point(x, y_baseline));
			if let Some(outlined) = font.outline_glyph(glyph) {
				let bmin = outlined.px_bounds().min;
				let bx = bmin.x as i32;
				let by = bmin.y as i32;
				outlined.draw(|gx, gy, alpha| {
					if alpha > 0.004 {
						let mut c = color;
						c[3] = (color[3] as f32 * alpha).round().min(255.0) as u8;
						fb.set(bx + gx as i32, by + gy as i32, c);
					}
				});
			}
		}
		x += scaled.h_advance(id) + letter_spacing;
	}
	x
}

/// Draw text truncated with "…" so it never exceeds `max_w`. Returns the
/// advance width actually used.
pub fn draw_ellipsis(
	fonts: &Fonts,
	fb: &mut FB,
	x: f32,
	y_baseline: f32,
	text: &str,
	px: f32,
	color: Rgba,
	bold: bool,
	max_w: f32,
) -> f32 {
	if measure(fonts, text, px, bold) <= max_w {
		return draw(fonts, fb, x, y_baseline, text, px, color, bold, 0.0);
	}
	let ellipsis = "\u{2026}";
	let mut end = text.len();
	while end > 0 {
		// Step back over whole chars.
		end -= 1;
		while end > 0 && !text.is_char_boundary(end) {
			end -= 1;
		}
		let candidate = format!("{}{}", &text[..end], ellipsis);
		if measure(fonts, &candidate, px, bold) <= max_w {
			return draw(fonts, fb, x, y_baseline, &candidate, px, color, bold, 0.0);
		}
	}
	0.0
}

/// Width of the first `n` chars (byte length must be a char boundary).
pub fn measure_prefix(fonts: &Fonts, text: &str, n_bytes: usize, px: f32, bold: bool) -> f32 {
	let n = n_bytes.min(text.len());
	let mut n = n;
	while n > 0 && !text.is_char_boundary(n) {
		n -= 1;
	}
	measure(fonts, &text[..n], px, bold)
}

/// Greedy word-wrap `text` to `max_w` pixels. A word wider than the whole
/// line is hard-broken per character (so a long unbreakable token can never
/// overflow). Returns the visual lines; a blank input yields one empty line.
pub fn wrap_text(fonts: &Fonts, text: &str, max_w: f32, px: f32, bold: bool) -> Vec<String> {
	if !max_w.is_finite() || max_w <= 0.0 {
		return vec![text.to_string()];
	}
	let space_w = measure(fonts, " ", px, bold);
	let mut lines: Vec<String> = Vec::new();
	let mut cur = String::new();
	let mut cur_w = 0.0f32;
	for word in text.split(' ') {
		if word.is_empty() {
			continue;
		}
		let ww = measure(fonts, word, px, bold);
		if !cur.is_empty() && cur_w + space_w + ww > max_w {
			lines.push(std::mem::take(&mut cur));
			cur_w = 0.0;
		}
		if !cur.is_empty() {
			cur.push(' ');
			cur_w += space_w;
		}
		if ww > max_w {
			// Hard-break a word wider than the whole line.
			for ch in word.chars() {
				let c = ch.to_string();
				let cw = measure(fonts, &c, px, bold);
				if cur_w + cw > max_w && !cur.is_empty() {
					lines.push(std::mem::take(&mut cur));
					cur_w = 0.0;
				}
				cur.push_str(&c);
				cur_w += cw;
			}
		} else {
			cur.push_str(word);
			cur_w += ww;
		}
	}
	if !cur.is_empty() || lines.is_empty() {
		lines.push(cur);
	}
	lines
}

/// Draw `text` word-wrapped to `max_w` (see [`wrap_text`]). `y` is the
/// baseline of the first line; every next line is advanced by `line_h`.
/// Returns the total height actually used (`lines * line_h`).
pub fn draw_wrapped(
	fonts: &Fonts,
	fb: &mut FB,
	x: f32,
	y: f32,
	text: &str,
	px: f32,
	color: Rgba,
	bold: bool,
	max_w: f32,
	line_h: f32,
) -> f32 {
	let lines = wrap_text(fonts, text, max_w, px, bold);
	for (i, line) in lines.iter().enumerate() {
		if line.is_empty() {
			continue;
		}
		draw(fonts, fb, x, y + i as f32 * line_h, line, px, color, bold, 0.0);
	}
	lines.len() as f32 * line_h
}

/// Draw a password-style field content: one bullet per char.
pub fn masked_str(fonts: &Fonts, text: &str) -> String {
	let ch = if fonts.bullet { '\u{2022}' } else { '*' };
	std::iter::repeat(ch).take(text.chars().count()).collect()
}

/// Split `text` into lines that each fit `max_w` px at the given size.
/// Breaks at spaces; a single word wider than the whole line is hard-broken
/// at the character that fits. Existing '\n' breaks are kept. (Mobile-era
/// API — a '\n'-aware wrapper over [`wrap_text`].)
pub fn wrap(fonts: &Fonts, text: &str, max_w: f32, px: f32, bold: bool) -> Vec<String> {
	let mut lines: Vec<String> = Vec::new();
	for para in text.split('\n') {
		lines.extend(wrap_text(fonts, para, max_w, px, bold));
	}
	if lines.is_empty() {
		lines.push(String::new());
	}
	lines
}

/// Draw `text` wrapped to `max_w` as a horizontally centered block whose
/// vertical center sits at `cy`. Returns the total block height.
pub fn draw_wrapped_centered(
	fonts: &Fonts,
	fb: &mut FB,
	cx: f32,
	cy: f32,
	max_w: f32,
	text: &str,
	px: f32,
	color: Rgba,
	bold: bool,
) -> f32 {
	let lines = wrap(fonts, text, max_w, px, bold);
	let line_h = px * 1.4;
	let total_h = lines.len() as f32 * line_h;
	let mut y_baseline = cy - total_h / 2.0 + px * 0.85;
	for line in &lines {
		let w = measure(fonts, line, px, bold);
		draw(fonts, fb, cx - w / 2.0, y_baseline, line, px, color, bold, 0.0);
		y_baseline += line_h;
	}
	total_h
}
