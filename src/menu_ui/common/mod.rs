//! Drawing helpers shared by two or more menu panes.

use crate::app::UiCtx;
use crate::draw::FB;
use crate::font;
#[allow(unused_imports)]
use crate::menu_ui::*;
use crate::model::avatar_placeholder_color;
use crate::theme;
use crate::ui::{Hit, Rect};

mod icons;
pub(crate) use icons::*;
mod theme_edit;
pub use theme_edit::*;

/// Roblox-style vertical scrollbar: up/down arrow buttons plus a draggable
/// thumb. `x` is the track's left edge; hits are registered for both arrows
/// and the whole track (thumb drag).
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_scrollbar(
	ctx: &mut UiCtx,
	fb: &mut FB,
	x: i32,
	y: i32,
	w: i32,
	h: i32,
	scroll: f32,
	content_h: f32,
	view_h: f32,
	s: f32,
) {
	if content_h <= view_h + 0.5 {
		return;
	}
	let t = theme::cur();
	let aw = w.max((13.0 * s) as i32); // arrow zone height (square-ish)
	let track_y = y + aw;
	let track_h = (h - 2 * aw).max(1);

	// Whole-track drag hit first, so the arrows (added later) win on top.
	ctx.hits.add(Rect { x: x - (6.0 * s) as i32, y, w: w + (12.0 * s) as i32, h }, Hit::Scrollbar);
	ctx.hits.add(Rect { x, y, w, h: aw }, Hit::ScrollUp);
	ctx.hits.add(Rect { x, y: y + h - aw, w, h: aw }, Hit::ScrollDown);

	// Thumb.
	let thumb_h = ((view_h / content_h) * track_h as f32).max(22.0 * s) as i32;
	let scrollable = (content_h - view_h).max(0.001);
	let thumb_y = track_y as f32 + (scroll / scrollable) * (track_h as f32 - thumb_h as f32);
	fb.rect_rounded(x + w / 2 - 3, thumb_y as i32, 6, thumb_h, (3.0 * s) as i32, t.border);

	// Arrows.
	let cx = x + w / 2;
	tri_up(fb, cx, y + aw / 2, t.text_dim);
	tri_down(fb, cx, y + h - aw / 2, t.text_dim);
}

// ---------------------------------------------------------------------------
// Shared: status bar, avatar
// ---------------------------------------------------------------------------
pub(crate) fn draw_avatar(
	ctx: &mut UiCtx,
	fb: &mut FB,
	id: u64,
	display_name: &str,
	x: i32,
	y: i32,
	size: i32,
) {
	let t = theme::cur();
	let radius = (size as f32 * 0.18) as i32;
	match ctx.avatars.get(&id) {
		Some(rgba) => {
			fb.image(
				x,
				y,
				rgba,
				theme::AVATAR_SIZE as i32,
				theme::AVATAR_SIZE as i32,
				size,
				size,
				radius,
			);
		}
		None => {
			let color = avatar_placeholder_color(if id == u64::MAX { 0 } else { id });
			fb.rect_rounded(x, y, size, size, radius, color);
			let initial = display_name.chars().next().unwrap_or('?').to_uppercase().to_string();
			let iw = font::measure(ctx.fonts, &initial, size as f32 * 0.42, true);
			font::draw(
				ctx.fonts,
				fb,
				x as f32 + (size as f32 - iw) / 2.0,
				y as f32 + size as f32 / 2.0 + size as f32 * 0.15,
				&initial,
				size as f32 * 0.42,
				t.white,
				true,
				0.0,
			);
		}
	}
}
