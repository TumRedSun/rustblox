//! The collapsible left sidebar (full labels <-> icon-only mini).

use crate::app::UiCtx;
use crate::draw::FB;
use crate::font;
#[allow(unused_imports)]
use crate::menu_ui::*;
use crate::theme;
use crate::ui::{Hit, Rect};

pub(crate) fn draw_sidebar(v: &mut MenuView, ctx: &mut UiCtx, fb: &mut FB, w: i32, h: i32) {
	let s = ctx.scale;
	let t = theme::cur();
	let mini = v.collapsed;
	fb.rect(0, 0, w, h, t.surface);
	fb.rect(w - 1, 0, 1, h, [0x1E, 0x20, 0x22, 0xFF]);

	let pad = (12.0 * s) as i32;

	// Collapse/expand toggle (chevrons).
	let tg = (22.0 * s) as i32;
	let tgx = if mini { (w - tg) / 2 } else { w - pad - tg };
	let tgr = Rect { x: tgx, y: pad, w: tg, h: tg };
	let tghov = tgr.contains(ctx.cursor.0, ctx.cursor.1);
	if tghov {
		fb.rect_rounded(tgr.x, tgr.y, tgr.w, tgr.h, (6.0 * s) as i32, t.surface_2);
	}
	chevron(
		fb,
		tgr.x + tgr.w / 2,
		tgr.y + tgr.h / 2,
		if tghov { t.text } else { t.text_dim },
		mini,
	);
	ctx.hits.add(tgr, Hit::SidebarToggle);
	let mut y = tgr.y + tgr.h + (10.0 * s) as i32;

	// Logo: the slightly rotated red tile IS the "R" of the wordmark.
	// Clicking it refreshes and jumps to the top of the Home screen.
	let sq = (26.0 * s) as i32;
	let tile_cx = if mini { w / 2 } else { pad + sq / 2 + 1 };
	let tile_cy = y + sq / 2;
	let logo_rect = if mini {
		Rect { x: tile_cx - sq / 2 - 4, y: y - 4, w: sq + 8, h: sq + 8 }
	} else {
		Rect { x: pad, y: y - 4, w: w - 2 * pad, h: sq + 8 }
	};
	let logo_hov = logo_rect.contains(ctx.cursor.0, ctx.cursor.1);
	if logo_hov {
		fb.rect_rounded(
			logo_rect.x,
			logo_rect.y,
			logo_rect.w,
			logo_rect.h,
			(8.0 * s) as i32,
			t.surface_2,
		);
	}
	fb.rect_rounded_rot(tile_cx, tile_cy, sq, sq, (6.0 * s) as i32, -0.16, t.red);
	let ir = 14.0 * s;
	let ir_w = font::measure(ctx.fonts, "R", ir, true);
	font::draw(
		ctx.fonts,
		fb,
		tile_cx as f32 - ir_w / 2.0,
		tile_cy as f32 + ir * 0.35,
		"R",
		ir,
		t.white,
		true,
		0.0,
	);
	if !mini {
		let wx = (tile_cx + sq / 2 + (6.0 * s) as i32) as f32;
		font::draw(
			ctx.fonts,
			fb,
			wx,
			tile_cy as f32 + 14.0 * s * 0.35,
			"ustBlox",
			14.0 * s,
			t.text,
			true,
			0.0,
		);
	}
	ctx.hits.add(logo_rect, Hit::Logo);
	y = tile_cy + sq / 2 + (14.0 * s) as i32;

	// Profile chip.
	let avatar_sz = ((if mini { 32.0 } else { 34.0 }) * s) as i32;
	let ax = if mini { (w - avatar_sz) / 2 } else { pad };
	draw_avatar(ctx, fb, v.profile.id, &v.profile.display_name, ax, y, avatar_sz);
	if !mini {
		let nx = (ax + avatar_sz + (9.0 * s) as i32) as f32;
		let name_w = (w - pad) as f32 - nx - (6.0 * s);
		font::draw_ellipsis(
			ctx.fonts,
			fb,
			nx,
			y as f32 + 13.0 * s,
			&v.profile.display_name,
			12.5 * s,
			t.text,
			true,
			name_w,
		);
		font::draw_ellipsis(
			ctx.fonts,
			fb,
			nx,
			y as f32 + 27.0 * s,
			&format!("@{}", v.profile.name),
			11.0 * s,
			t.text_dim,
			false,
			name_w,
		);
	}
	y += avatar_sz + (12.0 * s) as i32;

	// Navigation (no Home button: the logo does that; no Settings entry
	// either — the only one is the gear pinned to the bottom of the sidebar
	// below, in both mini and full layout, so there is exactly one).
	let item_h = (34.0 * s) as i32;
	let item_pad = if mini { (6.0 * s) as i32 } else { pad };
	let item_w = w - 2 * item_pad;
	nav_item(
		ctx,
		fb,
		item_pad,
		y,
		item_w,
		item_h,
		"Friends",
		icon_person_add,
		v.tab == MenuTab::Friends,
		true,
		Hit::NavFriends,
		None,
		mini,
	);
	y += item_h + (9.0 * s) as i32;

	if !mini {
		fb.hline(pad, y, w - 2 * pad, t.border);
		y += (9.0 * s) as i32;
	}

	// Future sections (disabled placeholders).
	for (label, icon) in [
		("Charts", icon_charts as fn(&mut FB, i32, i32, i32, theme::Rgba)),
		("Avatar", icon_person),
		("Chat", icon_chat),
	] {
		nav_item(
			ctx,
			fb,
			item_pad,
			y,
			item_w,
			item_h,
			label,
			icon,
			false,
			false,
			Hit::None,
			None,
			mini,
		);
		y += item_h + (5.0 * s) as i32;
	}

	// Bottom: THE single Settings entry point — the gear, pinned here in
	// both sidebar layouts (account actions live in Settings). In full mode
	// the version label sits next to it, right-aligned.
	let bw = (34.0 * s) as i32;
	let bx = if mini { (w - bw) / 2 } else { pad };
	let by = h - bw - (12.0 * s) as i32;
	let hovered = Rect { x: bx, y: by, w: bw, h: bw }.contains(ctx.cursor.0, ctx.cursor.1);
	if hovered {
		fb.rect_rounded(bx, by, bw, bw, (8.0 * s) as i32, t.surface_2);
	}
	icon_gear(
		fb,
		bx + (bw - (16.0 * s) as i32) / 2,
		by + (bw - (16.0 * s) as i32) / 2,
		(16.0 * s) as i32,
		if hovered { t.text } else { t.text_dim },
	);
	ctx.hits.add(Rect { x: bx, y: by, w: bw, h: bw }, Hit::NavSettings);
	if !mini {
		let ver = format!("RustBlox v{}", env!("CARGO_PKG_VERSION"));
		let vw = font::measure(ctx.fonts, &ver, 10.5 * s, false);
		font::draw(
			ctx.fonts,
			fb,
			(w - pad) as f32 - vw,
			by as f32 + bw as f32 * 0.5 + 10.5 * s * 0.35,
			&ver,
			10.5 * s,
			t.text_faint,
			false,
			0.0,
		);
	}
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn nav_item(
	ctx: &mut UiCtx,
	fb: &mut FB,
	x: i32,
	y: i32,
	w: i32,
	h: i32,
	label: &str,
	icon: fn(&mut FB, i32, i32, i32, theme::Rgba),
	active: bool,
	enabled: bool,
	hit: Hit,
	badge: Option<&str>,
	mini: bool,
) {
	let s = ctx.scale;
	let t = theme::cur();
	let hovered = enabled
		&& ctx.cursor.0 >= x as f32
		&& ctx.cursor.0 < (x + w) as f32
		&& ctx.cursor.1 >= y as f32
		&& ctx.cursor.1 < (y + h) as f32;
	if active || hovered {
		fb.rect_rounded(x, y, w, h, (8.0 * s) as i32, t.surface_2);
	}
	if active {
		let bar_h = (16.0 * s) as i32;
		fb.rect(
			x + (3.0 * s) as i32,
			y + (h - bar_h) / 2,
			(3.0 * s).max(1.0) as i32,
			bar_h,
			t.accent,
		);
	}

	let ic = (16.0 * s) as i32;
	let (icon_c, text_c) = if active || hovered {
		(t.text, t.text)
	} else if enabled {
		(t.text_dim, t.text_dim)
	} else {
		(t.text_faint, t.text_faint)
	};
	let ix = if mini { x + (w - ic) / 2 } else { x + (12.0 * s) as i32 };
	icon(fb, ix, y + (h - ic) / 2, ic, icon_c);

	if !mini {
		let tx = (ix + ic + (10.0 * s) as i32) as f32;
		let ty = y as f32 + h as f32 / 2.0 + 12.5 * s * 0.35;
		font::draw_ellipsis(ctx.fonts, fb, tx, ty, label, 12.5 * s, text_c, active, {
			let mut avail = (x + w) as f32 - tx - (10.0 * s);
			if badge.is_some() {
				avail -= 32.0 * s;
			}
			avail.max(20.0)
		});
		if let Some(b) = badge {
			let bw = font::measure(ctx.fonts, b, 10.0 * s, false);
			let bx = (x + w - (10.0 * s) as i32) as f32 - bw;
			font::draw(ctx.fonts, fb, bx, ty, b, 10.0 * s, t.text_faint, false, 0.0);
		}
	}
	if enabled {
		ctx.hits.add(Rect { x, y, w, h }, hit);
	}
}

// ---------------------------------------------------------------------------
// Icons (tiny vector glyphs built from primitives)
// ---------------------------------------------------------------------------
