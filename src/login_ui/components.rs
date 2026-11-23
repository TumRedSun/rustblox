//! Reusable pieces of the login screen: 2FA pane, logo, tabs and footer.

use super::LoginView;
use crate::app::UiCtx;
use crate::draw::FB;
use crate::font;
use crate::theme;
use crate::ui::{draw_button, BtnStyle, FieldId, Hit, Rect};

pub(super) fn draw_two_step(
	v: &mut LoginView,
	ctx: &mut UiCtx,
	fb: &mut FB,
	x: i32,
	y: &mut i32,
	inner_w: i32,
) {
	let s = ctx.scale;
	let t = theme::cur();
	font::draw(
		ctx.fonts,
		fb,
		x as f32,
		*y as f32 + 14.0 * s,
		"2-Step Verification",
		17.0 * s,
		t.text,
		true,
		0.0,
	);
	*y += (26.0 * s) as i32;
	let media = v.two_step.as_ref().map(|t| t.media_type.clone()).unwrap_or_default();
	let via =
		if media.eq_ignore_ascii_case("Email") { "your email" } else { "your authenticator app" };
	font::draw_ellipsis(
		ctx.fonts,
		fb,
		x as f32,
		*y as f32 + 12.0 * s,
		&format!("Enter the code from {}.", via),
		13.0 * s,
		t.text_dim,
		false,
		inner_w as f32,
	);
	// Extra room so the field label below never collides with the subtitle.
	*y += (34.0 * s) as i32;

	let field_h = (42.0 * s) as i32;
	v.code.draw(ctx, fb, x, *y, inner_w, field_h, FieldId::Code, Some("Verification code"));
	*y += field_h + (18.0 * s) as i32;

	let gap = (10.0 * s) as i32;
	let bw = (inner_w - gap) / 2;
	let verify = draw_button(
		ctx,
		fb,
		Rect { x, y: *y, w: bw, h: field_h },
		if v.busy { "Verifying\u{2026}" } else { "Verify" },
		BtnStyle::Primary,
		v.busy,
	);
	ctx.hits.add(verify, Hit::Verify);
	let resend = draw_button(
		ctx,
		fb,
		Rect { x: x + bw + gap, y: *y, w: bw, h: field_h },
		"Resend code",
		BtnStyle::Secondary,
		v.busy,
	);
	ctx.hits.add(resend, Hit::Resend);
	*y += field_h + gap;
	let back = draw_button(
		ctx,
		fb,
		Rect { x, y: *y, w: inner_w, h: field_h },
		"Back to login",
		BtnStyle::Ghost,
		false,
	);
	ctx.hits.add(back, Hit::Back);
}

pub(super) fn draw_logo(ctx: &mut UiCtx, fb: &mut FB, w: i32, top: i32) {
	let s = ctx.scale;
	let t = theme::cur();
	let sq = (26.0 * s) as i32;
	let cx = w / 2 - (110.0 * s) as i32 / 2;
	fb.rect_rounded(cx, top, sq, sq, (6.0 * s) as i32, t.red);
	let r_w = font::measure(ctx.fonts, "R", 16.0 * s, true);
	font::draw(
		ctx.fonts,
		fb,
		cx as f32 + (sq as f32 - r_w) * 0.5,
		top as f32 + sq as f32 * 0.5 + 16.0 * s * 0.35,
		"R",
		16.0 * s,
		t.white,
		true,
		0.0,
	);
	font::draw(
		ctx.fonts,
		fb,
		(cx + sq + (10.0 * s) as i32) as f32,
		top as f32 + sq as f32 * 0.5 + 16.0 * s * 0.35,
		"RUSTBLOX",
		16.0 * s,
		t.text,
		true,
		1.2 * s,
	);
	let sub = "Unofficial open-source Roblox client";
	let sub_w = font::measure(ctx.fonts, sub, 12.5 * s, false);
	font::draw(
		ctx.fonts,
		fb,
		w as f32 / 2.0 - sub_w / 2.0,
		top as f32 + sq as f32 + 20.0 * s,
		sub,
		12.5 * s,
		t.text_faint,
		false,
		0.0,
	);
}

pub(super) fn draw_tab(ctx: &mut UiCtx, fb: &mut FB, r: Rect, label: &str, active: bool, hit: Hit) {
	let s = ctx.scale;
	let t = theme::cur();
	// Frameless tabs: active = lighter fill + accent underline, inactive =
	// plain dim text on the card background.
	if active {
		fb.rect_rounded(r.x, r.y, r.w, r.h, (8.0 * s) as i32, t.surface_2);
		fb.rect(
			r.x + (10.0 * s) as i32,
			r.y + r.h - (3.0 * s).max(1.0) as i32,
			r.w - (20.0 * s) as i32,
			(3.0 * s).max(1.0) as i32,
			t.accent,
		);
	}
	let tw = font::measure(ctx.fonts, label, 13.5 * s, true);
	font::draw(
		ctx.fonts,
		fb,
		r.x as f32 + (r.w as f32 - tw) * 0.5,
		r.y as f32 + r.h as f32 * 0.5 + 13.5 * s * 0.35,
		label,
		13.5 * s,
		if active { t.text } else { t.text_dim },
		true,
		0.0,
	);
	ctx.hits.add(r, hit);
}

pub(super) fn draw_footer(ctx: &mut UiCtx, fb: &mut FB, w: i32, h: i32) {
	let s = ctx.scale;
	let t = theme::cur();
	let text = "GPL-3.0 open-source \u{b7} not affiliated with Roblox Corporation";
	let tw = font::measure(ctx.fonts, text, 11.5 * s, false);
	font::draw(
		ctx.fonts,
		fb,
		w as f32 / 2.0 - tw / 2.0,
		h as f32 - 14.0 * s,
		text,
		11.5 * s,
		t.text_faint,
		false,
		0.0,
	);
}
