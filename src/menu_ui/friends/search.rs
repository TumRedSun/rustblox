//! Friends pane: user search and the request-sending rows.

use crate::app::UiCtx;
use crate::draw::FB;
use crate::font;
#[allow(unused_imports)]
use crate::menu_ui::*;
use crate::model::UserEntry;
use crate::theme;
use crate::ui::{self, draw_button, BtnStyle, FieldId, Hit, Rect};

pub(crate) fn draw_search_pane(
	v: &mut MenuView,
	ctx: &mut UiCtx,
	fb: &mut FB,
	x: i32,
	w: i32,
	h: i32,
	top: f32,
	_list_h: f32,
) {
	let s = ctx.scale;
	let t = theme::cur();
	let pad = (18.0 * s) as i32;

	// Search row: field + button.
	let field_h = (36.0 * s) as i32;
	let field_y = top as i32 + ((14.0 * s) as i32);
	let btn_w = (96.0 * s) as i32;
	let field_w = (w - 2 * pad - btn_w - (10.0 * s) as i32).min(460 * s as i32);
	v.add.query.draw(ctx, fb, x + pad, field_y, field_w, field_h, FieldId::AddSearch, None);
	let btn_rect = draw_button(
		ctx,
		fb,
		Rect { x: x + pad + field_w + (10.0 * s) as i32, y: field_y, w: btn_w, h: field_h },
		if v.add.searching { "Searching\u{2026}" } else { "Search" },
		BtnStyle::Primary,
		v.add.searching,
	);
	ctx.hits.add(btn_rect, Hit::AddSearchBtn);
	if v.add.searching {
		ui::draw_spinner(
			fb,
			btn_rect.x as f32 + 14.0 * s,
			btn_rect.y as f32 + btn_rect.h as f32 / 2.0,
			5.5 * s,
			ctx.now_ms,
			t.btn_text,
			s,
		);
	}

	// Results list.
	let list_top = field_y as f32 + field_h as f32 + (16.0 * s);
	let list_h = (h as f32 - (theme::STATUSBAR_H * s)) - list_top;
	let scroll_w = (theme::SCROLL_W * s) as i32;
	let rows_x = (x + pad) as f32;
	let rows_w = (w - 2 * pad - scroll_w) as f32;
	let row_h = (58.0 * s) as f32;
	let content_h = v.add.results.len() as f32 * row_h;
	v.add_scroll = clamp_scroll(v.add_scroll, content_h, list_h);
	v.add_metrics = Some(ListMetrics { top: list_top, h: list_h, content_h });

	let list_clip =
		crate::draw::Clip { x0: x, y0: list_top as i32, x1: x + w, y1: (list_top + list_h) as i32 };
	let old_clip = fb.clip();
	fb.set_clip(list_clip);

	if v.add.results.is_empty() {
		fb.set_clip(old_clip);
		let (msg, spin): (String, bool) = if v.add.searching {
			("Searching\u{2026}".into(), true)
		} else if let Some(err) = &v.add.error {
			let mut m = format!("Error: {}", err);
			m.truncate(120);
			(m, false)
		} else if v.add.searched_once {
			("No users found".into(), false)
		} else {
			("Type at least 2 characters of a username and press Enter".into(), false)
		};
		let mw = font::measure(ctx.fonts, &msg, 14.0 * s, false).min(rows_w);
		font::draw_wrapped_centered(
			ctx.fonts,
			fb,
			x as f32 + w as f32 / 2.0,
			list_top + list_h / 2.0,
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
				list_top + list_h / 2.0 - 5.0 * s,
				7.0 * s,
				ctx.now_ms,
				t.accent,
				s,
			);
		}
	} else {
		let first = (v.add_scroll / row_h) as usize;
		for (vi, user) in v.add.results.iter().enumerate().skip(first) {
			let y = list_top - v.add_scroll + vi as f32 * row_h;
			if y > list_top + list_h {
				break;
			}
			if y + row_h < list_top {
				continue;
			}
			draw_result_row(&v.add, ctx, fb, user, rows_x, y, rows_w, row_h, s);
		}
		fb.set_clip(old_clip);
		draw_scrollbar(
			ctx,
			fb,
			x + w - pad - scroll_w + scroll_w / 2,
			list_top as i32,
			scroll_w,
			list_h as i32,
			v.add_scroll,
			content_h,
			list_h,
			s,
		);
	}
}
pub(crate) fn draw_result_row(
	add: &AddFriendsState,
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
	let btn_w = (92.0 * s) as i32;
	let max_text_w = w - (text_x - x) - btn_w as f32 - (24.0 * s);

	let state = add.state_of(user.id);
	let name_c = t.text;
	font::draw_ellipsis(
		ctx.fonts,
		fb,
		text_x,
		y + 23.0 * s,
		user.display_or_name(),
		14.0 * s,
		name_c,
		true,
		max_text_w,
	);
	let sub = format!("@{}", user.name);
	let sub_c = match &state {
		RequestState::Failed(_) => t.red,
		_ => t.text_dim,
	};
	let sub_text = match &state {
		RequestState::Failed(msg) => {
			let mut t2 = msg.clone();
			if t2.len() > 80 {
				t2.truncate(80);
			}
			t2
		}
		_ => sub,
	};
	font::draw_ellipsis(
		ctx.fonts,
		fb,
		text_x,
		y + 42.0 * s,
		&sub_text,
		12.0 * s,
		sub_c,
		false,
		max_text_w,
	);

	let btn_rect = Rect {
		x: (x + w - pad as f32 - btn_w as f32) as i32,
		y: (y + (h - 30.0 * s) / 2.0) as i32,
		w: btn_w,
		h: (30.0 * s) as i32,
	};
	match state {
		RequestState::Idle => {
			let r = draw_button(ctx, fb, btn_rect, "Add", BtnStyle::Primary, false);
			ctx.hits.add(r, Hit::AddUser(user.id));
		}
		RequestState::Pending => {
			draw_button(ctx, fb, btn_rect, "Sending\u{2026}", BtnStyle::Secondary, true);
		}
		RequestState::Sent => {
			let tw = font::measure(ctx.fonts, "Request sent", 12.5 * s, false);
			font::draw(
				ctx.fonts,
				fb,
				btn_rect.x as f32 + (btn_rect.w as f32 - tw) / 2.0,
				btn_rect.y as f32 + btn_rect.h as f32 / 2.0 + 12.5 * s * 0.35,
				"Request sent",
				12.5 * s,
				t.green,
				false,
				0.0,
			);
		}
		RequestState::Failed(_) => {
			let r = draw_button(ctx, fb, btn_rect, "Retry", BtnStyle::Secondary, false);
			ctx.hits.add(r, Hit::AddUser(user.id));
		}
	}

	fb.hline(x as i32, (y + h) as i32, w as i32, [0x1E, 0x20, 0x22, 0xFF]);
}

// ---------------------------------------------------------------------------
// Settings pane: account, theme, appearance
// ---------------------------------------------------------------------------
