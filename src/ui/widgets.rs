//! Framebuffer-facing widgets: text field drawing, buttons, spinner and the
//! status dot.

use super::field::TextField;
use super::hit::{FieldId, Hit, Rect};
use crate::app::UiCtx;
use crate::draw::FB;
use crate::font;
use crate::theme::{self, Rgba};

impl TextField {
	/// Draw the field (label, box, value/placeholder, clear button) and
	/// register its hit targets. Returns the box rect.
	pub fn draw(
		&mut self,
		ctx: &mut UiCtx,
		fb: &mut FB,
		x: i32,
		y: i32,
		w: i32,
		h: i32,
		id: FieldId,
		label: Option<&str>,
	) -> Rect {
		let s = ctx.scale;
		let t = crate::theme::cur();
		// Label above the box.
		if let Some(lb) = label {
			let ly = y - (16.0 * s) as i32;
			font::draw(
				ctx.fonts,
				fb,
				x as f32,
				(ly + (12.0 * s) as i32) as f32,
				lb,
				12.0 * s,
				t.text_dim,
				false,
				0.0,
			);
		}
		let focused = ctx.focus == Some(id);
		// Frameless input: filled rounded rect only. Focus is shown with a
		// brighter fill plus an accent underline drawn inside the rect (no
		// separate outline => it can never misalign).
		fb.rect_rounded(
			x,
			y,
			w,
			h,
			(8.0 * s) as i32,
			if focused { t.surface_2 } else { t.input_bg },
		);
		if focused {
			let bar_h = (3.0 * s).max(1.0) as i32;
			let inset = (10.0 * s) as i32;
			fb.rect(x + inset, y + h - bar_h - 1, w - 2 * inset, bar_h, t.accent);
		}

		let pad_x = (12.0 * s) as i32;
		// The clear (×) zone on the right, present while there is text.
		let clear_w = if self.value.is_empty() { 0 } else { (26.0 * s) as i32 };
		let inner_x = x + pad_x;
		let inner_w = w - 2 * pad_x - clear_w;
		let font_px = 14.0 * s;
		let baseline = y as f32 + h as f32 * 0.5 + font_px * 0.35;

		let shown =
			if self.masked { font::masked_str(ctx.fonts, &self.value) } else { self.value.clone() };

		// Horizontal scrolling: keep the cursor position in view.
		let cursor_x_rel = font::measure_prefix(ctx.fonts, &shown, self.cursor, font_px, false);
		let text_w = font::measure(ctx.fonts, &shown, font_px, false);
		if cursor_x_rel - self.scroll > inner_w as f32 - 14.0 * s {
			self.scroll = cursor_x_rel - inner_w as f32 + 14.0 * s;
		}
		if cursor_x_rel - self.scroll < 0.0 {
			self.scroll = (cursor_x_rel - 14.0 * s).max(0.0);
		}
		if text_w < self.scroll {
			self.scroll = (text_w - inner_w as f32).max(0.0);
		}

		let text_clip =
			crate::draw::Clip { x0: inner_x, y0: y + 2, x1: inner_x + inner_w, y1: y + h - 2 };
		let old_clip = fb.clip();
		fb.set_clip(text_clip);

		if self.value.is_empty() && !focused {
			// Placeholder: truncate with an ellipsis so it never clips at
			// the field edge on narrow screens.
			font::draw_ellipsis(
				ctx.fonts,
				fb,
				inner_x as f32,
				baseline,
				self.placeholder,
				font_px,
				t.text_faint,
				false,
				inner_w as f32,
			);
		} else {
			// Selection highlight.
			if let Some((a, b)) = self.sel {
				let (a, b) = (a.min(b), a.max(b));
				let sx = font::measure_prefix(ctx.fonts, &shown, a, font_px, false);
				let ex = font::measure_prefix(ctx.fonts, &shown, b, font_px, false);
				fb.rect(
					inner_x + ((sx - self.scroll) as i32),
					y + (4.0 * s) as i32,
					((ex - sx) as i32).max(2),
					h - (8.0 * s) as i32,
					t.selection,
				);
			}
			font::draw(
				ctx.fonts,
				fb,
				inner_x as f32 - self.scroll,
				baseline,
				&shown,
				font_px,
				t.text,
				false,
				0.0,
			);
			// No blinking caret: focus is already shown by the accent
			// underline above, and the software caret rendered at the wrong
			// spot on touch devices. Cursor positioning (scroll) still uses
			// `cursor_x_rel` so long values stay in view while typing.
		}
		fb.set_clip(old_clip);

		let r = Rect { x, y, w, h };
		ctx.hits.add(r, Hit::Field(id));

		// Clear (×) button on the right: erases everything typed. Registered
		// after the field hit so it wins the hit test on top of it.
		if clear_w > 0 {
			let bw = (20.0 * s) as i32;
			let bx = x + w - bw - (4.0 * s) as i32;
			let by = y + (h - bw) / 2;
			let crect = Rect { x: bx, y: by, w: bw, h: bw };
			let (mx, my) = ctx.cursor;
			let hov = crect.contains(mx, my);
			let cc = if hov { t.text } else { t.text_faint };
			let c = bw as f32;
			let (x0, x1) = (bx as f32 + c * 0.30, bx as f32 + c * 0.70);
			let (y0, y1) = (by as f32 + c * 0.30, by as f32 + c * 0.70);
			let n = ((x1 - x0).ceil() as i32).max(1);
			for i in 0..=n {
				let fr = i as f32 / n as f32;
				fb.set_f(x0 + fr * (x1 - x0), y0 + fr * (y1 - y0), cc);
				fb.set_f(x0 + fr * (x1 - x0), y1 - fr * (y1 - y0), cc);
			}
			ctx.hits.add(crect, Hit::ClearField(id));
		}
		r
	}
}

#[derive(Clone, Copy)]
pub enum BtnStyle {
	Primary,
	Secondary,
	Ghost,
	/// Green action (Roblox-style "Join").
	Success,
}

/// Draw a button; `hover`/`disabled` affect colors. Adds to the hit map.
pub fn draw_button(
	ctx: &mut UiCtx,
	fb: &mut FB,
	r: Rect,
	label: &str,
	style: BtnStyle,
	disabled: bool,
) -> Rect {
	let s = ctx.scale;
	let t = theme::cur();
	let hovered = !disabled
		&& ctx.cursor.0 >= r.x as f32
		&& ctx.cursor.0 < (r.x + r.w) as f32
		&& ctx.cursor.1 >= r.y as f32
		&& ctx.cursor.1 < (r.y + r.h) as f32;
	let (bg, fg) = match style {
		BtnStyle::Primary => {
			if disabled {
				(t.btn_disabled, t.text_faint)
			} else if hovered {
				(t.accent_hover, t.btn_text)
			} else {
				(t.accent, t.btn_text)
			}
		}
		BtnStyle::Secondary => {
			if disabled {
				(t.btn_disabled, t.text_faint)
			} else if hovered {
				(t.surface_2, t.text)
			} else {
				(t.surface, t.text)
			}
		}
		BtnStyle::Ghost => (t.bg, if hovered { t.text } else { t.text_dim }),
		BtnStyle::Success => {
			if disabled {
				(t.btn_disabled, t.text_faint)
			} else if hovered {
				(t.green_hover, t.btn_text)
			} else {
				(t.green, t.btn_text)
			}
		}
	};
	fb.rect_rounded(r.x, r.y, r.w, r.h, (8.0 * s) as i32, bg);
	let tw = font::measure(ctx.fonts, label, 14.0 * s, true);
	let _ = font::draw(
		ctx.fonts,
		fb,
		r.x as f32 + (r.w as f32 - tw) * 0.5,
		r.y as f32 + r.h as f32 * 0.5 + 14.0 * s * 0.35,
		label,
		14.0 * s,
		fg,
		true,
		0.0,
	);
	r
}

/// Small spinner (dots rotating around a circle).
pub fn draw_spinner(fb: &mut FB, cx: f32, cy: f32, radius: f32, now_ms: u64, color: Rgba, s: f32) {
	let n = 8;
	for i in 0..n {
		let phase = (i as f64 / n as f64) * std::f64::consts::TAU + (now_ms as f64 / 130.0);
		let a = i as f64 / n as f64;
		let alpha = ((1.0 - a) * 0.9 + 0.1) * 255.0;
		let x = cx as f64 + radius as f64 * phase.cos();
		let y = cy as f64 + radius as f64 * phase.sin();
		let mut c = color;
		c[3] = alpha as u8;
		fb.circle(x as i32, y as i32, (2.2 * s).round() as i32, c);
	}
}

/// Colored status dot.
pub fn draw_dot(fb: &mut FB, cx: i32, cy: i32, d: i32, color: Rgba) {
	fb.circle(cx + d / 2, cy, d / 2, color);
}
