//! Settings / Appearance: theme presets and colour slots.

use crate::app::UiCtx;
use crate::draw::FB;
use crate::font;
#[allow(unused_imports)]
use crate::menu_ui::*;
use crate::theme;
use crate::ui::{draw_button, BtnStyle, Hit, Rect};

/// Rough content height of the Appearance tab.
pub(crate) fn measure_appearance(s: f32) -> f32 {
	let slots = color_slot_count();
	let mut y = 6.0 * s;
	y += 24.0 * s; // CORNER ROUNDING
	y += 40.0 * s; // radius row
	y += 14.0 * s + 24.0 * s; // COLORS
	y += slots as f32 * (36.0 * s);
	y += 16.0 * s + 40.0 * s; // reset button
	y + 16.0 * s
}
pub(crate) fn draw_appearance(
	ctx: &mut UiCtx,
	fb: &mut FB,
	x: i32,
	y: &mut f32,
	w: f32,
	s: f32,
	app_settings: &mut crate::config::Settings,
) {
	let t = theme::cur();

	section_label(ctx, fb, x, y, "CORNER ROUNDING", s);
	let scale_pct = (t.radius_scale * 100.0).round();
	let btn = (30.0 * s) as i32;
	let b1 = Rect { x: x as i32, y: *y as i32, w: btn, h: btn };
	let b2 = Rect { x: x as i32 + btn + (10.0 * s) as i32, y: *y as i32, w: btn, h: btn };
	draw_button(ctx, fb, b1, "\u{2212}", BtnStyle::Secondary, false);
	draw_button(ctx, fb, b2, "+", BtnStyle::Secondary, false);
	ctx.hits.add(b1, Hit::RadiusDown);
	ctx.hits.add(b2, Hit::RadiusUp);
	let val = format!("{}%", scale_pct);
	font::draw(
		ctx.fonts,
		fb,
		x as f32 + (2.0 * btn as f32 + 22.0 * s),
		*y + btn as f32 / 2.0 + 12.0 * s * 0.35,
		&val,
		13.5 * s,
		t.text,
		true,
		0.0,
	);
	*y += 40.0 * s + 14.0 * s;

	section_label(ctx, fb, x, y, "COLORS", s);
	for i in 0..color_slot_count() {
		let cur = theme_field(app_settings.custom.unwrap_or(t), i);
		let label = color_slot_label(i);
		let sw = (26.0 * s) as i32;
		let sh = (22.0 * s) as i32;
		let row_cy = *y + 17.0 * s;
		font::draw(
			ctx.fonts,
			fb,
			x as f32,
			row_cy + 4.0 * s,
			label,
			12.5 * s,
			t.text_dim,
			false,
			0.0,
		);
		let chip_x = (x as f32 + w - (58.0 * s)) as i32;
		// Prev / next arrows around the swatch.
		let aw = (18.0 * s) as i32;
		let prev = Rect {
			x: chip_x - aw - (2.0 * s) as i32,
			y: (*y + (sh as f32 - aw as f32) / 2.0) as i32,
			w: aw,
			h: aw,
		};
		let next = Rect { x: chip_x + sw + (2.0 * s) as i32, y: prev.y, w: aw, h: aw };
		draw_small_arrow(ctx, fb, prev, false);
		draw_small_arrow(ctx, fb, next, true);
		ctx.hits.add(prev, Hit::ColorPrev(i));
		ctx.hits.add(next, Hit::ColorNext(i));
		fb.rect_rounded(chip_x, *y as i32, sw, sh, (5.0 * s) as i32, cur);
		*y += 36.0 * s;
	}
	*y += 6.0 * s;

	let bw = (170.0 * s) as i32;
	let bh = (32.0 * s) as i32;
	let r = draw_button(
		ctx,
		fb,
		Rect { x: x as i32, y: *y as i32, w: bw, h: bh },
		"Reset appearance",
		BtnStyle::Secondary,
		false,
	);
	ctx.hits.add(r, Hit::ResetAppearance);
	*y += 40.0 * s;
}
