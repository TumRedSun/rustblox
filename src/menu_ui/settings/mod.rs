//! Settings pane: the tab strip and the shared small controls.

use crate::app::UiCtx;
use crate::draw::FB;
use crate::font;
#[allow(unused_imports)]
use crate::menu_ui::*;
use crate::theme;
use crate::ui::{draw_button, BtnStyle, Hit, Rect};

mod appearance;
pub(crate) use appearance::*;
mod general;
pub(crate) use general::*;
mod logs;
pub(crate) use logs::*;

pub(crate) fn draw_settings(
	v: &mut MenuView,
	ctx: &mut UiCtx,
	fb: &mut FB,
	x: i32,
	w: i32,
	h: i32,
) {
	let s = ctx.scale;
	let t = theme::cur();
	let pad = (18.0 * s) as i32;
	let header_h = (54.0 * s) as i32;
	let tabs_h = (46.0 * s) as i32;
	let status_h = (theme::STATUSBAR_H * s) as i32;

	font::draw(
		ctx.fonts,
		fb,
		x as f32 + pad as f32,
		header_h as f32 / 2.0 + 9.5 * s,
		"Settings",
		19.0 * s,
		t.text,
		true,
		0.0,
	);
	let hint = "Account, themes and diagnostics";
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

	let tab_h = (32.0 * s) as i32;
	let tw = (((w - 2 * pad) as f32 - 2.0 * (8.0 * s)) / 3.0).min(120.0 * s) as i32;
	let tabs_y = header_h + (tabs_h - tab_h) / 2;
	draw_seg_tab(
		ctx,
		fb,
		Rect { x: x + pad, y: tabs_y, w: tw, h: tab_h },
		"General",
		v.settings.tab == SettingsTab::General,
		Hit::SettingsTab(0),
	);
	draw_seg_tab(
		ctx,
		fb,
		Rect { x: x + pad + tw + (8.0 * s) as i32, y: tabs_y, w: tw, h: tab_h },
		"Appearance",
		v.settings.tab == SettingsTab::Appearance,
		Hit::SettingsTab(1),
	);
	draw_seg_tab(
		ctx,
		fb,
		Rect { x: x + pad + 2 * (tw + (8.0 * s) as i32), y: tabs_y, w: tw, h: tab_h },
		"Logs",
		v.settings.tab == SettingsTab::Logs,
		Hit::SettingsTab(2),
	);

	let top = (header_h + tabs_h) as f32;
	let view_h = (h - status_h) as f32 - top;
	// The Logs pane reserves a fixed footer row for the action buttons.
	let footer_h = if v.settings.tab == SettingsTab::Logs { 46.0 * s } else { 0.0 };
	let list_view = (view_h - footer_h).max(0.0);

	// Measure content first (any tab can exceed the viewport).
	let scroll_w = (theme::SCROLL_W * s) as i32;
	let inner_w = (w - 2 * pad - scroll_w) as f32;
	let content_h = match v.settings.tab {
		SettingsTab::General => measure_general(v, s),
		SettingsTab::Appearance => measure_appearance(s),
		SettingsTab::Logs => measure_logs(ctx.fonts, inner_w, s),
	};
	if v.settings.logs_follow && v.settings.tab == SettingsTab::Logs {
		v.settings_scroll = (content_h - list_view).max(0.0);
	}
	v.settings_scroll = clamp_scroll(v.settings_scroll, content_h, list_view);
	v.settings_metrics = Some(ListMetrics { top, h: list_view, content_h });

	let clip = crate::draw::Clip { x0: x, y0: top as i32, x1: x + w, y1: (top + list_view) as i32 };
	let old_clip = fb.clip();
	fb.set_clip(clip);
	let mut y = top - v.settings_scroll;
	match v.settings.tab {
		SettingsTab::General => draw_general(v, ctx, fb, x + pad, &mut y, inner_w, s),
		SettingsTab::Appearance => {
			draw_appearance(ctx, fb, x + pad, &mut y, inner_w, s, &mut v.app_settings)
		}
		SettingsTab::Logs => draw_logs(ctx, fb, x + pad, &mut y, inner_w, s),
	}
	fb.set_clip(old_clip);

	if content_h > list_view + 0.5 {
		draw_scrollbar(
			ctx,
			fb,
			x + w - pad - scroll_w + scroll_w / 2,
			top as i32,
			scroll_w,
			list_view as i32,
			v.settings_scroll,
			content_h,
			list_view,
			s,
		);
	}

	// Logs footer: action buttons pinned under the log view.
	if v.settings.tab == SettingsTab::Logs {
		let btn_h = (30.0 * s) as i32;
		let fy = (top + view_h - 46.0 * s) as i32;
		let copy_w = (120.0 * s) as i32;
		let r1 = Rect { x: x + pad, y: fy, w: copy_w, h: btn_h };
		let b1 = draw_button(ctx, fb, r1, "Copy logs", BtnStyle::Primary, false);
		ctx.hits.add(b1, Hit::CopyLogs);
		let save_w = (86.0 * s) as i32;
		let r3 = Rect { x: x + pad + copy_w + (10.0 * s) as i32, y: fy, w: save_w, h: btn_h };
		let b3 = draw_button(ctx, fb, r3, "Save", BtnStyle::Primary, false);
		ctx.hits.add(b3, Hit::SaveLogs);
		let clear_w = (90.0 * s) as i32;
		let r2 =
			Rect { x: x + pad + copy_w + save_w + (20.0 * s) as i32, y: fy, w: clear_w, h: btn_h };
		let b2 = draw_button(ctx, fb, r2, "Clear", BtnStyle::Secondary, false);
		ctx.hits.add(b2, Hit::ClearLogs);
		let count = crate::logs::len();
		let label = if count == 1 { "1 line".to_string() } else { format!("{} lines", count) };
		let lw = font::measure(ctx.fonts, &label, 11.5 * s, false);
		font::draw(
			ctx.fonts,
			fb,
			(x + w - pad) as f32 - lw,
			fy as f32 + btn_h as f32 / 2.0 + 11.5 * s * 0.35,
			&label,
			11.5 * s,
			t.text_dim,
			false,
			0.0,
		);
	}
}
pub(crate) fn draw_small_arrow(ctx: &mut UiCtx, fb: &mut FB, r: Rect, right: bool) {
	let t = theme::cur();
	let hov = r.contains(ctx.cursor.0, ctx.cursor.1);
	let color = if hov { t.text } else { t.text_faint };
	if right {
		tri_right(fb, r.x + r.w / 2, r.y + r.h / 2, color);
	} else {
		tri_left(fb, r.x + r.w / 2, r.y + r.h / 2, color);
	}
}
/// Pill-style on/off toggle (Settings → recommendations).
pub(crate) fn draw_toggle(
	ctx: &mut UiCtx,
	fb: &mut FB,
	x: i32,
	y: i32,
	on: bool,
	hit: Hit,
) -> Rect {
	let s = ctx.scale;
	let t = theme::cur();
	let w = (44.0 * s) as i32;
	let h = (24.0 * s) as i32;
	fb.rect_rounded(x, y, w, h, h / 2, if on { t.accent } else { t.border });
	let k = h - (6.0 * s) as i32;
	let kx = if on { x + w - k - (3.0 * s) as i32 } else { x + (3.0 * s) as i32 };
	fb.circle(kx + k / 2, y + h / 2, k / 2, t.white);
	let r = Rect { x, y, w, h };
	ctx.hits.add(r, hit);
	r
}
pub(crate) fn section_label(ctx: &mut UiCtx, fb: &mut FB, x: i32, y: &mut f32, text: &str, s: f32) {
	let t = theme::cur();
	font::draw(ctx.fonts, fb, x as f32, *y + 12.0 * s, text, 11.0 * s, t.text_faint, true, 1.0 * s);
	*y += 24.0 * s;
}
