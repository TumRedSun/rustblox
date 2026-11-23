//! Card renderers: place cards, friend cards and the strip arrows.

use crate::app::UiCtx;
use crate::draw::FB;
use crate::font;
#[allow(unused_imports)]
use crate::menu_ui::*;
use crate::model::{format_count, FriendRow, PlaceCard, PresenceKind};
use crate::theme;
use crate::ui::{self, draw_button, BtnStyle, Hit, Rect};

pub(crate) fn draw_friends_strip(
	v: &mut MenuView,
	ctx: &mut UiCtx,
	fb: &mut FB,
	x: i32,
	y: i32,
	w: i32,
	h: i32,
) {
	let s = ctx.scale;
	let t = theme::cur();
	fb.rect_rounded(x, y, w, h, (12.0 * s) as i32, t.surface);
	v.strip_rect = Some(Rect { x, y, w, h });

	if v.rows.is_empty() {
		let (msg, spin) = if v.refreshing && v.last_update.is_none() {
			("Loading friends\u{2026}", true)
		} else {
			("No friends yet \u{2014} open Friends to find people", false)
		};
		let mw = font::measure(ctx.fonts, msg, 13.0 * s, false);
		font::draw(
			ctx.fonts,
			fb,
			x as f32 + w as f32 / 2.0 - mw / 2.0,
			y as f32 + h as f32 / 2.0,
			msg,
			13.0 * s,
			t.text_faint,
			false,
			0.0,
		);
		if spin {
			ui::draw_spinner(
				fb,
				x as f32 + w as f32 / 2.0 - mw / 2.0 - 16.0 * s,
				y as f32 + h as f32 / 2.0 - 4.0 * s,
				6.0 * s,
				ctx.now_ms,
				t.accent,
				s,
			);
		}
		v.strip_metrics = None;
		return;
	}

	let arrow_w = (26.0 * s) as i32;
	let inset = (6.0 * s) as i32;
	let view_w = (w - 2 * arrow_w - 2 * inset) as f32;
	// Card width scales with the theme but never overflows the strip view.
	let card_w = ((theme::STRIP_CARD_W * s) as i32).min(view_w as i32).max(1);
	let gap = (8.0 * s) as i32;
	let content_w = v.rows.len() as f32 * (card_w + gap) as f32;
	v.strip_scroll = clamp_scroll(v.strip_scroll, content_w, view_w);
	v.strip_metrics = Some(ListMetrics { top: x as f32, h: view_w, content_h: content_w });
	let scrollable = content_w > view_w + 0.5;

	// Cards (clipped to the strip viewport).
	let clip = crate::draw::Clip { x0: x + arrow_w, y0: y + 1, x1: x + w - arrow_w, y1: y + h - 1 };
	let old_clip = fb.clip();
	fb.set_clip(clip);
	let mut cx = x + arrow_w + inset - v.strip_scroll as i32;
	for row in &v.rows {
		if cx + card_w >= clip.x0 && cx <= clip.x1 {
			draw_friend_card(ctx, fb, row, cx, y, card_w, h, s);
		}
		cx += card_w + gap;
	}
	fb.set_clip(old_clip);

	// Left/right arrow buttons (dimmed when the strip end is reached).
	if scrollable {
		let at_start = v.strip_scroll <= 0.5;
		let at_end = v.strip_scroll >= content_w - view_w - 0.5;
		draw_strip_arrow(ctx, fb, Rect { x, y, w: arrow_w, h }, true, at_start, Hit::StripLeft);
		draw_strip_arrow(
			ctx,
			fb,
			Rect { x: x + w - arrow_w, y, w: arrow_w, h },
			false,
			at_end,
			Hit::StripRight,
		);
	}
}
pub(crate) fn draw_strip_arrow(
	ctx: &mut UiCtx,
	fb: &mut FB,
	r: Rect,
	left: bool,
	dim: bool,
	hit: Hit,
) {
	let s = ctx.scale;
	let t = theme::cur();
	let hovered = !dim && r.contains(ctx.cursor.0, ctx.cursor.1);
	if hovered {
		fb.rect_rounded(r.x + 1, r.y + 1, r.w - 2, r.h - 2, (8.0 * s) as i32, t.surface_2);
	}
	let color = if dim {
		t.text_faint
	} else if hovered {
		t.text
	} else {
		t.text_dim
	};
	if left {
		tri_left(fb, r.x + r.w / 2, r.y + r.h / 2, color);
	} else {
		tri_right(fb, r.x + r.w / 2, r.y + r.h / 2, color);
	}
	ctx.hits.add(r, hit);
}
pub(crate) fn draw_friend_card(
	ctx: &mut UiCtx,
	fb: &mut FB,
	row: &FriendRow,
	x: i32,
	y: i32,
	w: i32,
	h: i32,
	s: f32,
) {
	let s2 = s;
	let t = theme::cur();
	let hovered = Rect { x, y, w, h }.contains(ctx.cursor.0, ctx.cursor.1);
	if hovered {
		fb.rect_rounded(x + 2, y + 2, w - 4, h - 4, (10.0 * s2) as i32, t.surface_2);
	}

	let a = (56.0 * s2) as i32;
	let ax = x + (w - a) / 2;
	let ay = y + (8.0 * s2) as i32;
	draw_avatar(ctx, fb, row.friend.id, &row.friend.display_name, ax, ay, a);

	// Presence dot pinned to the avatar corner.
	let d = (12.0 * s2) as i32;
	let dx = ax + a - d + 1;
	let dy = ay + a - d + 1;
	fb.circle(dx + d / 2, dy + d / 2, d / 2 + (1.5 * s2) as i32, t.surface);
	fb.circle(dx + d / 2, dy + d / 2, d / 2, row.presence.kind.dot_color());

	let center = x as f32 + w as f32 / 2.0;
	let name_baseline = (ay + a) as f32 + (18.0 * s2);
	draw_centered(
		ctx,
		fb,
		center,
		name_baseline,
		&row.friend.display_name,
		12.0 * s2,
		t.text,
		true,
		w as f32 - 10.0 * s2,
	);
	let label = row.status_label();
	let label_color = match row.presence.kind {
		PresenceKind::Online => t.green,
		PresenceKind::InGame => t.blue,
		PresenceKind::Studio => t.orange,
		PresenceKind::Offline | PresenceKind::Invisible => t.text_dim,
	};
	draw_centered(
		ctx,
		fb,
		center,
		name_baseline + (15.0 * s2),
		&label,
		10.5 * s2,
		label_color,
		false,
		w as f32 - 8.0 * s2,
	);
}
/// Draw a one-line text horizontally centered at `cx`, truncated if needed.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_centered(
	ctx: &UiCtx,
	fb: &mut FB,
	cx: f32,
	baseline: f32,
	text: &str,
	px: f32,
	color: theme::Rgba,
	bold: bool,
	max_w: f32,
) {
	let full = font::measure(ctx.fonts, text, px, bold);
	if full <= max_w {
		font::draw(ctx.fonts, fb, cx - full / 2.0, baseline, text, px, color, bold, 0.0);
	} else {
		font::draw_ellipsis(
			ctx.fonts,
			fb,
			cx - max_w / 2.0,
			baseline,
			text,
			px,
			color,
			bold,
			max_w,
		);
	}
}
pub(crate) fn draw_place_card(
	ctx: &mut UiCtx,
	fb: &mut FB,
	card: &PlaceCard,
	x: i32,
	y: i32,
	w: i32,
	h: i32,
	s: f32,
) {
	let t = theme::cur();
	let hovered = Rect { x, y, w, h }.contains(ctx.cursor.0, ctx.cursor.1);
	fb.rect_rounded(x, y, w, h, (10.0 * s) as i32, if hovered { t.surface_2 } else { t.surface });

	let pad = (8.0 * s) as i32;
	let icon = w - 2 * pad;
	match ctx.icons.get(&card.root_place_id) {
		Some(rgba) => {
			fb.image(
				x + pad,
				y + pad,
				rgba,
				theme::PLACE_ICON_PX as i32,
				theme::PLACE_ICON_PX as i32,
				icon,
				icon,
				(8.0 * s) as i32,
			);
		}
		None => {
			// Placeholder: dark tile with a tiny "photo" glyph (sun + hills).
			fb.rect_rounded(x + pad, y + pad, icon, icon, (8.0 * s) as i32, t.input_bg);
			let px0 = x + pad;
			let py0 = y + pad;
			let sun = (icon * 3 / 10).max(3);
			fb.circle(px0 + icon / 3, py0 + icon / 3, sun / 2, t.text_faint);
			let base = py0 + icon - icon / 4;
			for i in 0..(icon / 3).max(2) {
				let half = i * 2 / 2 + 1;
				fb.rect(px0 + icon / 2 - half, base - i, half * 2, 1, t.text_faint);
			}
		}
	}

	let text_x = (x + pad) as f32;
	let max_text = (w - 2 * pad) as f32;
	let mut ty = (y + pad + icon) as f32 + (16.0 * s);
	font::draw_ellipsis(ctx.fonts, fb, text_x, ty, &card.name, 12.5 * s, t.text, true, max_text);

	if !card.creator.is_empty() {
		ty += 14.0 * s;
		font::draw_ellipsis(
			ctx.fonts,
			fb,
			text_x,
			ty,
			&format!("By {}", card.creator),
			11.0 * s,
			t.text_faint,
			false,
			max_text,
		);
	}

	// Stats: likes triangle + percent, then player count (locally tracked
	// cards have no live stats — skip the row entirely then).
	if card.likes_pct.is_some() || card.player_count > 0 {
		ty += 15.0 * s;
		let mut sx = text_x;
		if let Some(p) = card.likes_pct {
			tri_up_small(fb, (sx + (4.0 * s)) as i32, (ty - (4.0 * s)) as i32, t.green);
			sx += 11.0 * s;
			let pct = format!("{}%", p);
			font::draw(ctx.fonts, fb, sx, ty, &pct, 11.0 * s, t.text_dim, false, 0.0);
			sx += font::measure(ctx.fonts, &pct, 11.0 * s, false) + (6.0 * s);
			font::draw(ctx.fonts, fb, sx, ty, "\u{b7}", 11.0 * s, t.text_faint, false, 0.0);
			sx += font::measure(ctx.fonts, "\u{b7}", 11.0 * s, false) + (6.0 * s);
		}
		if card.player_count > 0 {
			let players = format!("{} playing", format_count(card.player_count));
			font::draw_ellipsis(
				ctx.fonts,
				fb,
				sx,
				ty,
				&players,
				11.0 * s,
				t.text_faint,
				false,
				(x + w) as f32 - sx - pad as f32,
			);
		}
	}

	// Join button (joining is not implemented yet — visual only).
	let btn_h = (28.0 * s) as i32;
	let btn = Rect { x: x + pad, y: y + h - btn_h - pad, w: w - 2 * pad, h: btn_h };
	let r = draw_button(ctx, fb, btn, "Join", BtnStyle::Success, false);
	ctx.hits.add(r, Hit::JoinPlace(card.root_place_id));
}

// ---------------------------------------------------------------------------
// Friends pane: incoming requests + find-friends search
// ---------------------------------------------------------------------------
