//! Small vector icons drawn with the 2D framebuffer primitives.

use crate::draw::FB;
#[allow(unused_imports)]
use crate::menu_ui::*;
use crate::theme;

pub(crate) fn icon_person_add(fb: &mut FB, x: i32, y: i32, sz: i32, color: theme::Rgba) {
	let hr = (sz * 3) / 11;
	let cx = x + sz / 2 - 1;
	fb.circle(cx, y + hr + 2, hr, color);
	let sy = y + sz / 2 + 3;
	let rows = (y + sz - 1 - sy).max(1);
	for i in 0..rows {
		let half = (((i + 1) * (sz / 2 - 2)) / rows + 2).min(sz / 2);
		fb.rect(cx - half, sy + i, half * 2, 1, color);
	}
	// Plus badge, top-right.
	let px = x + sz - 2;
	let py = y + 2;
	fb.rect(px - 4, py, 9, 2, color);
	fb.rect(px - 1, py - 3, 2, 9, color);
}
pub(crate) fn icon_gear(fb: &mut FB, x: i32, y: i32, sz: i32, color: theme::Rgba) {
	// Proper gear: ring body + 8 teeth on the rim + open hub. Built from a
	// ring (no background-colored punching), so it renders correctly on any
	// surface including hover highlights.
	let cx = x + sz / 2;
	let cy = y + sz / 2;
	let ro = (sz / 2 - 1).max(3);
	let ri = (ro * 45 / 100).max(1);
	let tooth = ((sz / 8) as i32).max(2);
	for k in 0..8 {
		let a = k as f32 * std::f32::consts::FRAC_PI_4;
		let (sn, cs) = a.sin_cos();
		let tx = cx + (cs * ro as f32).round() as i32;
		let ty = cy + (sn * ro as f32).round() as i32;
		fb.rect(tx - tooth / 2, ty - tooth / 2, tooth, tooth, color);
	}
	fb.ring(cx, cy, ro, ri, color);
}
pub(crate) fn icon_person(fb: &mut FB, x: i32, y: i32, sz: i32, color: theme::Rgba) {
	let hr = sz / 4;
	fb.circle(x + sz / 2, y + hr + 1, hr, color);
	let sy = y + sz / 2 + 2;
	let rows = (sz - 1 - sy).max(1);
	for i in 0..rows {
		let half = (((i + 1) * (sz / 2 - 2)) / rows + 2).min(sz / 2);
		fb.rect(x + sz / 2 - half, sy + i, half * 2, 1, color);
	}
}
pub(crate) fn icon_charts(fb: &mut FB, x: i32, y: i32, sz: i32, color: theme::Rgba) {
	let bw = (sz / 4).max(2);
	for (i, hgt) in [(0usize, 5i32), (1, 9), (2, 13)] {
		let bx = x + (i as i32) * (bw + 2);
		fb.rect(bx, y + sz - hgt, bw, hgt, color);
	}
}
pub(crate) fn icon_chat(fb: &mut FB, x: i32, y: i32, sz: i32, color: theme::Rgba) {
	fb.rect_rounded(x, y + 1, sz, sz - 5, (sz / 4) as i32, color);
	// Speech tail.
	for i in 0..3 {
		fb.rect(x + 3 + i, y + sz - 5 + i, 3, 1, color);
	}
	// Three dots punched out with the sidebar background.
	let d = 1.max(sz / 9);
	for i in 0..3 {
		let dx = x + sz / 2 - sz / 4 + i * (sz / 4);
		fb.circle(dx, y + (sz - 5) / 2 + 1, d, crate::theme::cur().surface);
	}
}
/// Small collapse chevron: `right` = point right (expand), else left.
pub(crate) fn chevron(fb: &mut FB, cx: i32, cy: i32, color: theme::Rgba, right: bool) {
	for i in 0..4 {
		let px = if right { cx - 3 + i } else { cx + 3 - i };
		fb.rect(px, cy - 3 + i, 1, 1, color);
		fb.rect(px, cy + 3 - i, 1, 1, color);
	}
}
pub(crate) fn tri_left(fb: &mut FB, cx: i32, cy: i32, color: theme::Rgba) {
	// Apex on the LEFT, tall edge on the right (points outward).
	for i in 0..5 {
		fb.rect(cx + 4 - i, cy - (4 - i), 1, (4 - i) * 2 + 1, color);
	}
}
pub(crate) fn tri_right(fb: &mut FB, cx: i32, cy: i32, color: theme::Rgba) {
	// Apex on the RIGHT, tall edge on the left (points outward).
	for i in 0..5 {
		fb.rect(cx - 4 + i, cy - (4 - i), 1, (4 - i) * 2 + 1, color);
	}
}
pub(crate) fn tri_up(fb: &mut FB, cx: i32, cy: i32, color: theme::Rgba) {
	for i in 0..5 {
		fb.rect(cx - i, cy - 4 + i, i * 2 + 1, 1, color);
	}
}
pub(crate) fn tri_down(fb: &mut FB, cx: i32, cy: i32, color: theme::Rgba) {
	for i in 0..5 {
		fb.rect(cx - (4 - i), cy - 4 + i, (4 - i) * 2 + 1, 1, color);
	}
}
/// Small green "likes" triangle drawn before the rating percentage.
pub(crate) fn tri_up_small(fb: &mut FB, cx: i32, cy: i32, color: theme::Rgba) {
	for i in 0..4 {
		fb.rect(cx - i, cy - 3 + i, i * 2 + 1, 1, color);
	}
}

// ---------------------------------------------------------------------------
// Home pane: search, friends strip, sectioned places content
// ---------------------------------------------------------------------------
