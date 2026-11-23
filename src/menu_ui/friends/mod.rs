//! Friends pane: requests (accept/decline) and the pane switch.

use crate::app::UiCtx;
use crate::draw::FB;
use crate::font;
#[allow(unused_imports)]
use crate::menu_ui::*;
use crate::model::UserEntry;
use crate::theme;
use crate::ui::{self, draw_button, BtnStyle, Hit, Rect};

mod search;
pub(crate) use search::*;

pub(crate) fn draw_friends(v: &mut MenuView, ctx: &mut UiCtx, fb: &mut FB, x: i32, w: i32, h: i32) {
	let s = ctx.scale;
	let t = theme::cur();
	let pad = (18.0 * s) as i32;
	let header_h = (54.0 * s) as i32;
	let tabs_h = (46.0 * s) as i32;
	let status_h = (theme::STATUSBAR_H * s) as i32;

	// Header.
	font::draw(
		ctx.fonts,
		fb,
		x as f32 + pad as f32,
		header_h as f32 / 2.0 + 9.5 * s,
		"Friends",
		19.0 * s,
		t.text,
		true,
		0.0,
	);
	let hint = match v.friend_pane {
		FriendPane::Requests => "People who want to add you",
		FriendPane::Search => "Find people by username and send a friend request",
	};
	let hw = font::measure(ctx.fonts, hint, 12.5 * s, false);
	font::draw(
		ctx.fonts,
		fb,
		(x + w - pad) as f32 - hw,
		header_h as f32 / 2.0 + 12.5 * s * 0.35,
		hint,
		12.5 * s,
		t.text_dim,
		false,
		0.0,
	);
	fb.hline(x, header_h, w, [0x1E, 0x20, 0x22, 0xFF]);

	// Pane tabs (share the row; never overflow narrow screens).
	let tab_h = (32.0 * s) as i32;
	let tw = (((w - 2 * pad) as f32 - (8.0 * s)) / 2.0).min(200.0 * s) as i32;
	draw_seg_tab(
		ctx,
		fb,
		Rect { x: x + pad, y: header_h + (tabs_h - tab_h) / 2, w: tw, h: tab_h },
		"Requests",
		v.friend_pane == FriendPane::Requests,
		Hit::TabRequests,
	);
	draw_seg_tab(
		ctx,
		fb,
		Rect {
			x: x + pad + tw + (8.0 * s) as i32,
			y: header_h + (tabs_h - tab_h) / 2,
			w: tw,
			h: tab_h,
		},
		"Find friends",
		v.friend_pane == FriendPane::Search,
		Hit::TabSearch,
	);

	let list_top = (header_h + tabs_h) as f32;
	let list_h = (h - status_h) as f32 - list_top;

	match v.friend_pane {
		FriendPane::Requests => draw_requests_pane(v, ctx, fb, x, w, list_top, list_h),
		FriendPane::Search => draw_search_pane(v, ctx, fb, x, w, h, list_top, list_h),
	}
}
pub(crate) fn draw_seg_tab(
	ctx: &mut UiCtx,
	fb: &mut FB,
	r: Rect,
	label: &str,
	active: bool,
	hit: Hit,
) {
	let s = ctx.scale;
	let t = theme::cur();
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
	let tw = font::measure(ctx.fonts, label, 13.0 * s, true);
	font::draw(
		ctx.fonts,
		fb,
		r.x as f32 + (r.w as f32 - tw) * 0.5,
		r.y as f32 + r.h as f32 * 0.5 + 13.0 * s * 0.35,
		label,
		13.0 * s,
		if active { t.text } else { t.text_dim },
		true,
		0.0,
	);
	ctx.hits.add(r, hit);
}
pub(crate) fn draw_requests_pane(
	v: &mut MenuView,
	ctx: &mut UiCtx,
	fb: &mut FB,
	x: i32,
	w: i32,
	top: f32,
	list_h: f32,
) {
	let s = ctx.scale;
	let t = theme::cur();
	let pad = (18.0 * s) as i32;
	let scroll_w = (theme::SCROLL_W * s) as i32;
	let rows_x = (x + pad) as f32;
	let rows_w = (w - 2 * pad - scroll_w) as f32;
	let row_h = (64.0 * s) as f32;
	let content_h = v.requests.items.len() as f32 * row_h;
	v.requests_scroll = clamp_scroll(v.requests_scroll, content_h, list_h);
	v.requests_metrics = Some(ListMetrics { top, h: list_h, content_h });

	let list_clip =
		crate::draw::Clip { x0: x, y0: top as i32, x1: x + w, y1: (top + list_h) as i32 };
	let old_clip = fb.clip();
	fb.set_clip(list_clip);

	if v.requests.items.is_empty() {
		fb.set_clip(old_clip);
		let (msg, spin): (String, bool) = if v.requests.loading {
			("Loading requests\u{2026}".into(), true)
		} else if let Some(err) = &v.requests.error {
			let mut m = format!("Error: {}", err);
			m.truncate(120);
			(m, false)
		} else {
			("No incoming friend requests".into(), false)
		};
		let mw = font::measure(ctx.fonts, &msg, 14.0 * s, false).min(rows_w);
		font::draw_wrapped_centered(
			ctx.fonts,
			fb,
			x as f32 + w as f32 / 2.0,
			top + list_h / 2.0,
			rows_w,
			&msg,
			14.0 * s,
			t.text_faint,
			false,
		);
		if spin {
			ui::draw_spinner(
				fb,
				x as f32 + w as f32 / 2.0 - mw / 2.0 - 18.0 * s,
				top + list_h / 2.0 - 5.0 * s,
				7.0 * s,
				ctx.now_ms,
				t.accent,
				s,
			);
		}
		return;
	}

	let first = (v.requests_scroll / row_h) as usize;
	for (vi, user) in v.requests.items.iter().enumerate().skip(first) {
		let y = top - v.requests_scroll + vi as f32 * row_h;
		if y > top + list_h {
			break;
		}
		if y + row_h < top {
			continue;
		}
		draw_request_row(&v.requests, ctx, fb, user, rows_x, y, rows_w, row_h, s);
	}
	fb.set_clip(old_clip);
	draw_scrollbar(
		ctx,
		fb,
		x + w - pad - scroll_w + scroll_w / 2,
		top as i32,
		scroll_w,
		list_h as i32,
		v.requests_scroll,
		content_h,
		list_h,
		s,
	);
}
pub(crate) fn draw_request_row(
	reqs: &RequestsState,
	ctx: &mut UiCtx,
	fb: &mut FB,
	user: &UserEntry,
	x: f32,
	y: f32,
	w: f32,
	h: f32,
	s: f32,
) {
	let t = theme::cur();
	let hovered = Rect { x: x as i32, y: y as i32, w: w as i32, h: h as i32 }
		.contains(ctx.cursor.0, ctx.cursor.1);
	if hovered {
		fb.rect(x as i32, y as i32, w as i32, h as i32, t.surface_2);
	}

	let pad = (12.0 * s) as i32;
	let avatar_sz = (42.0 * s) as i32;
	let avatar_y = y as i32 + ((h as i32) - avatar_sz) / 2;
	draw_avatar(ctx, fb, user.id, user.display_or_name(), x as i32 + pad, avatar_y, avatar_sz);

	let text_x = x + (pad + avatar_sz + (12.0 * s) as i32) as f32;
	let btn_w = (86.0 * s) as i32;
	let btn_h = (30.0 * s) as i32;
	let max_text_w = w - (text_x - x) - (btn_w * 2 + (8.0 * s) as i32) as f32 - (24.0 * s);

	font::draw_ellipsis(
		ctx.fonts,
		fb,
		text_x,
		y + 24.0 * s,
		user.display_or_name(),
		14.0 * s,
		t.text,
		true,
		max_text_w,
	);
	let failed = reqs.failed.get(&user.id);
	let sub_color = if failed.is_some() { t.red } else { t.text_dim };
	let sub = match failed {
		Some(msg) => {
			let mut m = msg.clone();
			if m.len() > 80 {
				m.truncate(80);
			}
			m
		}
		None => format!("@{}", user.name),
	};
	font::draw_ellipsis(
		ctx.fonts,
		fb,
		text_x,
		y + 43.0 * s,
		&sub,
		12.0 * s,
		sub_color,
		false,
		max_text_w,
	);

	let busy = reqs.acting.contains(&user.id);
	let by = (y + (h - btn_h as f32) / 2.0) as i32;
	let accept_r = Rect {
		x: (x + w - pad as f32 - (btn_w * 2 + (8.0 * s) as i32) as f32) as i32,
		y: by,
		w: btn_w,
		h: btn_h,
	};
	let decline_r = Rect { x: accept_r.x + btn_w + (8.0 * s) as i32, y: by, w: btn_w, h: btn_h };
	let ra = draw_button(ctx, fb, accept_r, "Accept", BtnStyle::Success, busy);
	let rd = draw_button(ctx, fb, decline_r, "Decline", BtnStyle::Secondary, busy);
	if !busy {
		ctx.hits.add(ra, Hit::RequestAccept(user.id));
		ctx.hits.add(rd, Hit::RequestDecline(user.id));
	}

	fb.hline(x as i32, (y + h) as i32, w as i32, [0x1E, 0x20, 0x22, 0xFF]);
}
