//! Settings / General: account, recommendations, actions.

use crate::app::UiCtx;
use crate::config::Session;
use crate::draw::FB;
use crate::font;
#[allow(unused_imports)]
use crate::menu_ui::*;
use crate::theme;
use crate::ui::{draw_button, BtnStyle, Hit, Rect};

/// Rough content height of the General tab (kept in sync with draw_general).
pub(crate) fn measure_general(v: &MenuView, s: f32) -> f32 {
	let others = v.accounts.iter().filter(|a| a.user_id != v.profile.id).count();
	let mut y = 6.0 * s;
	y += 24.0 * s; // ACCOUNT label
	y += 52.0 * s; // account row
	y += 42.0 * s; // add/sign-out buttons
	if others > 0 {
		y += 24.0 * s; // SWITCH ACCOUNT label
		y += others as f32 * (56.0 * s);
	}
	y += 14.0 * s + 24.0 * s; // THEME label
	y += 38.0 * s; // preset chips
	y += 14.0 * s + 24.0 * s; // HOME label
	y += 36.0 * s; // toggle row
	y += 22.0 * s; // version
	y + 16.0 * s
}
pub(crate) fn draw_general(
	v: &mut MenuView,
	ctx: &mut UiCtx,
	fb: &mut FB,
	x: i32,
	y: &mut f32,
	w: f32,
	s: f32,
) {
	let t = theme::cur();
	let pad = (12.0 * s) as i32;

	section_label(ctx, fb, x, y, "ACCOUNT", s);
	// Current account row.
	let avatar_sz = (40.0 * s) as i32;
	draw_avatar(
		ctx,
		fb,
		v.profile.id,
		&v.profile.display_name,
		x as i32,
		(*y + 4.0 * s) as i32,
		avatar_sz,
	);
	let nx = x as f32 + (pad + avatar_sz + (12.0 * s) as i32) as f32;
	let name_w = w - (nx - x as f32) - 8.0 * s;
	font::draw_ellipsis(
		ctx.fonts,
		fb,
		nx,
		*y + 20.0 * s,
		&v.profile.display_name,
		14.0 * s,
		t.text,
		true,
		name_w,
	);
	font::draw_ellipsis(
		ctx.fonts,
		fb,
		nx,
		*y + 38.0 * s,
		&format!("@{}", v.profile.name),
		12.0 * s,
		t.text_dim,
		false,
		name_w,
	);
	*y += 52.0 * s;

	// Add account / Sign out.
	let btn_w = (150.0 * s) as i32;
	let btn_h = (32.0 * s) as i32;
	let b1 = draw_button(
		ctx,
		fb,
		Rect { x: x as i32, y: *y as i32, w: btn_w, h: btn_h },
		"Add account",
		BtnStyle::Primary,
		false,
	);
	ctx.hits.add(b1, Hit::AddAccount);
	let b2 = draw_button(
		ctx,
		fb,
		Rect { x: x as i32 + btn_w + (10.0 * s) as i32, y: *y as i32, w: btn_w, h: btn_h },
		"Sign out",
		BtnStyle::Secondary,
		false,
	);
	ctx.hits.add(b2, Hit::Logout);
	*y += 42.0 * s;

	// Other saved accounts (quick switch, Discord-style).
	let others: Vec<&Session> = v.accounts.iter().filter(|a| a.user_id != v.profile.id).collect();
	if !others.is_empty() {
		section_label(ctx, fb, x, y, "SWITCH ACCOUNT", s);
		for acc in others {
			let row_y = *y;
			let hovered = Rect { x: x as i32, y: row_y as i32, w: w as i32, h: (48.0 * s) as i32 }
				.contains(ctx.cursor.0, ctx.cursor.1);
			if hovered {
				fb.rect_rounded(
					x as i32,
					row_y as i32,
					w as i32,
					(48.0 * s) as i32,
					(8.0 * s) as i32,
					t.surface_2,
				);
			}
			let avatar_sz = (34.0 * s) as i32;
			draw_avatar(
				ctx,
				fb,
				acc.user_id,
				&acc.display_name,
				x as i32 + pad,
				(row_y + 7.0 * s) as i32,
				avatar_sz,
			);
			let nx = x as f32 + (pad + avatar_sz + (12.0 * s) as i32) as f32;
			let name_w = w - (nx - x as f32) - (110.0 * s);
			font::draw_ellipsis(
				ctx.fonts,
				fb,
				nx,
				row_y + 20.0 * s,
				&acc.display_name,
				13.0 * s,
				t.text,
				true,
				name_w,
			);
			font::draw_ellipsis(
				ctx.fonts,
				fb,
				nx,
				row_y + 37.0 * s,
				&format!("@{}", acc.username),
				11.5 * s,
				t.text_dim,
				false,
				name_w,
			);
			let sw = (86.0 * s) as i32;
			let sh = (28.0 * s) as i32;
			let r = Rect {
				x: (x as f32 + w - sw as f32 - 4.0 * s) as i32,
				y: (row_y + (48.0 * s - sh as f32) / 2.0) as i32,
				w: sw,
				h: sh,
			};
			let r = draw_button(ctx, fb, r, "Switch", BtnStyle::Secondary, false);
			ctx.hits.add(r, Hit::SwitchAccount(acc.user_id));
			*y += 56.0 * s;
		}
	}

	// Theme presets.
	section_label(ctx, fb, x, y, "THEME", s);
	let mut chips: Vec<(&str, &str)> =
		theme::PRESETS.iter().map(|(id, _)| (*id, pretty_theme_name(id))).collect();
	if v.app_settings.custom.is_some() {
		chips.push(("custom", "Custom"));
	}
	let chip_h = (30.0 * s) as i32;
	let mut cx = x as i32;
	for (i, (id, label)) in chips.iter().enumerate() {
		let cw = (font::measure(ctx.fonts, label, 12.5 * s, true) + 24.0 * s) as i32;
		let active = v.app_settings.theme == *id;
		let rect = Rect { x: cx, y: *y as i32, w: cw, h: chip_h };
		if active {
			fb.rect_rounded(rect.x, rect.y, rect.w, rect.h, (8.0 * s) as i32, t.surface_2);
			fb.rect(
				rect.x + (8.0 * s) as i32,
				rect.y + rect.h - (3.0 * s).max(1.0) as i32,
				rect.w - (16.0 * s) as i32,
				(3.0 * s).max(1.0) as i32,
				t.accent,
			);
		} else {
			fb.rect_rounded(rect.x, rect.y, rect.w, rect.h, (8.0 * s) as i32, t.input_bg);
		}
		let lw = font::measure(ctx.fonts, label, 12.5 * s, true);
		font::draw(
			ctx.fonts,
			fb,
			cx as f32 + (cw as f32 - lw) * 0.5,
			*y + chip_h as f32 / 2.0 + 12.5 * s * 0.35,
			label,
			12.5 * s,
			if active { t.text } else { t.text_dim },
			true,
			0.0,
		);
		ctx.hits.add(rect, Hit::ThemePreset(i as u8));
		cx += cw + (8.0 * s) as i32;
	}
	*y += 38.0 * s + 14.0 * s;

	// Home: recommendations toggle.
	section_label(ctx, fb, x, y, "HOME", s);
	let label = "Show recommended places";
	font::draw(ctx.fonts, fb, x as f32, *y + 17.0 * s, label, 13.5 * s, t.text, false, 0.0);
	let tr = draw_toggle(
		ctx,
		fb,
		(x as f32 + w - (52.0 * s)) as i32,
		*y as i32,
		v.app_settings.show_recommendations,
		Hit::ToggleRecommendations,
	);
	let _ = tr;
	*y += 36.0 * s + 10.0 * s;

	// Version.
	let ver = format!("RustBlox v{} \u{b7} unofficial Roblox client", env!("CARGO_PKG_VERSION"));
	font::draw(ctx.fonts, fb, x as f32, *y + 12.0 * s, &ver, 11.0 * s, t.text_faint, false, 0.0);
	*y += 22.0 * s;
}
