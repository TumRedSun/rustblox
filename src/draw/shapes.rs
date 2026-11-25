//! Shape rasterization (rounded boxes, outlines, circles, rings) for the
//! software backend, plus the same shapes recorded as GPU primitives.

use super::FB;
use crate::theme::Rgba;

fn vec_len(x: f32, y: f32) -> f32 {
	(x * x + y * y).sqrt()
}

impl FB<'_> {
	/// Filled rounded rectangle (radius in px). The radius is scaled by the
	/// theme's global rounding multiplier (Settings → Appearance).
	pub fn rect_rounded(&mut self, x: i32, y: i32, w: i32, h: i32, radius: i32, color: Rgba) {
		if radius <= 0 {
			self.rect(x, y, w, h, color);
			return;
		}
		if color[3] == 0 {
			return;
		}
		let radius = (radius as f32 * crate::theme::cur().radius_scale).round() as i32;
		if self.is_gpu() {
			self.push(super::Prim::RoundRect {
				x,
				y,
				w,
				h,
				radius: radius.min(w / 2).min(h / 2) as f32,
				color,
			});
			return;
		}
		let r = radius.min(w / 2).min(h / 2);
		// Center band + middle columns.
		self.rect(x + r, y, w - 2 * r, h, color);
		self.rect(x, y + r, r, h - 2 * r, color);
		self.rect(x + w - r, y + r, r, h - 2 * r, color);
		// Corner tiles, anti-aliased with a 1px feather.
		for corner_x in [x, x + w - r] {
			for corner_y in [y, y + h - r] {
				let cx = if corner_x == x { x + r - 1 } else { x + w - r };
				let cy = if corner_y == y { y + r - 1 } else { y + h - r };
				for yy in corner_y..corner_y + r {
					for xx in corner_x..corner_x + r {
						let dx = (xx - cx) as f32;
						let dy = (yy - cy) as f32;
						let d = (dx * dx + dy * dy).sqrt();
						let aa = (r as f32 - 0.5 - d).clamp(0.0, 1.0);
						if aa > 0.0 {
							let mut px = color;
							px[3] = (color[3] as f32 * aa) as u8;
							self.set(xx, yy, px);
						}
					}
				}
			}
		}
	}

	/// Filled rounded rectangle rotated around its center (SDF-based).
	/// Used for the rotated logo tile.
	pub fn rect_rounded_rot(
		&mut self,
		cx: i32,
		cy: i32,
		w: i32,
		h: i32,
		radius: i32,
		angle: f32,
		color: Rgba,
	) {
		if angle.abs() < 0.001 {
			self.rect_rounded(cx - w / 2, cy - h / 2, w, h, radius, color);
			return;
		}
		if color[3] == 0 {
			return;
		}
		let radius = (radius as f32 * crate::theme::cur().radius_scale).max(0.0);
		if self.is_gpu() {
			self.push(super::Prim::RoundRectRot { cx, cy, w, h, radius, angle, color });
			return;
		}
		let (hw, hh) = (w as f32 / 2.0, h as f32 / 2.0);
		let (sin, cos) = angle.sin_cos();
		// Bounding box big enough for the rotation (hypotenuse of half-size).
		let bound = (hw * hw + hh * hh).sqrt().ceil() as i32 + 2;
		let alpha_scale = color[3] as f32;
		for dy in -bound..=bound {
			for dx in -bound..=bound {
				// Inverse-rotate the pixel offset into the tile's local space.
				let (lx, ly) =
					(dx as f32 * cos + dy as f32 * sin, -dx as f32 * sin + dy as f32 * cos);
				// Rounded-box signed distance.
				let qx = lx.abs() - (hw - radius);
				let qy = ly.abs() - (hh - radius);
				let dist = vec_len(qx.max(0.0), qy.max(0.0)) + qx.max(qy).min(0.0) - radius;
				let aa = (0.5 - dist).clamp(0.0, 1.0);
				if aa > 0.0 {
					let mut c = color;
					c[3] = (alpha_scale * aa) as u8;
					self.set(cx + dx, cy + dy, c);
				}
			}
		}
	}

	/// 1px outline (sharp corners).
	pub fn outline(&mut self, x: i32, y: i32, w: i32, h: i32, color: Rgba) {
		self.rect(x, y, w, 1, color);
		self.rect(x, y + h - 1, w, 1, color);
		self.rect(x, y + 1, 1, h - 2, color);
		self.rect(x + w - 1, y + 1, 1, h - 2, color);
	}

	/// 1px rounded outline.
	pub fn outline_rounded(&mut self, x: i32, y: i32, w: i32, h: i32, radius: i32, color: Rgba) {
		if radius <= 0 {
			self.outline(x, y, w, h, color);
			return;
		}
		let radius = (radius as f32 * crate::theme::cur().radius_scale).round() as i32;
		if self.is_gpu() {
			let r = radius.min(w / 2).min(h / 2) as f32;
			self.push(super::Prim::RoundRectBorder {
				cx: x + w / 2,
				cy: y + h / 2,
				w,
				h,
				radius: r,
				angle: 0.0,
				border: 1.0,
				color,
			});
			return;
		}
		let r = radius.min(w / 2).min(h / 2);
		self.rect(x + r, y, w - 2 * r, 1, color);
		self.rect(x + r, y + h - 1, w - 2 * r, 1, color);
		self.rect(x, y + r, 1, h - 2 * r, color);
		self.rect(x + w - 1, y + r, 1, h - 2 * r, color);
		let cx1 = (x + r - 1) as f32;
		let cy1 = (y + r - 1) as f32;
		let cx2 = (x + w - r) as f32;
		let cy2 = (y + h - r) as f32;
		for corner in [(cx1, cy1), (cx2, cy1), (cx1, cy2), (cx2, cy2)] {
			for a in 0..(r * 2) {
				let ang = (a as f32 / (r as f32 * 2.0)) * std::f32::consts::FRAC_PI_2;
				// Sample the ring at subpixel offsets for a smooth arc.
				for (sx, sy) in [(0.0, 0.0), (0.5, 0.0), (0.0, 0.5), (0.5, 0.5)] {
					let dx = (r as f32 - 0.5) * ang.cos();
					let dy = (r as f32 - 0.5) * ang.sin();
					let px = if corner.0 == cx1 {
						corner.0 + r as f32 - 1.0 - dx
					} else {
						corner.0 + dx
					};
					let py = if corner.1 == cy1 {
						corner.1 + r as f32 - 1.0 - dy
					} else {
						corner.1 + dy
					};
					self.set_f(px + sx - 0.5, py + sy - 0.5, color);
				}
			}
		}
	}

	/// Filled circle (fast: one sqrt per scanline).
	pub fn circle(&mut self, cx: i32, cy: i32, r: i32, color: Rgba) {
		if r <= 0 || color[3] == 0 {
			return;
		}
		if self.is_gpu() {
			self.push(super::Prim::Circle { cx, cy, r, color });
			return;
		}
		for dy in -r..=r {
			let dx = ((r * r - dy * dy) as f32).sqrt().round() as i32;
			self.rect(cx - dx, cy + dy, dx * 2 + 1, 1, color);
		}
	}

	/// Filled ring (annulus) around (`cx`, `cy`): outer radius `ro`, inner
	/// radius `ri` (`ri >= ro` or negative `ri` degrades to a full disc).
	/// Unlike a filled circle punched with the background color this works
	/// on any surface, so icons stay correct on hover highlights.
	pub fn ring(&mut self, cx: i32, cy: i32, ro: i32, ri: i32, color: Rgba) {
		if ro <= 0 || color[3] == 0 {
			return;
		}
		if self.is_gpu() {
			self.push(super::Prim::Ring { cx, cy, ro, ri: ri.max(0), color });
			return;
		}
		let ri = ri.max(0).min(ro - 1);
		for dy in -ro..=ro {
			let o2 = ro * ro - dy * dy;
			let ox = (o2 as f32).sqrt().round() as i32;
			let i2 = ri * ri - dy * dy;
			if i2 <= 0 {
				// Row does not reach the inner circle: solid span.
				self.rect(cx - ox, cy + dy, ox * 2 + 1, 1, color);
			} else {
				let ix = (i2 as f32).sqrt().round() as i32;
				let w = ox - ix;
				if w > 0 {
					self.rect(cx - ox, cy + dy, w, 1, color);
					self.rect(cx + ix + 1, cy + dy, w, 1, color);
				}
			}
		}
	}

	/// Horizontal line.
	pub fn hline(&mut self, x: i32, y: i32, w: i32, color: Rgba) {
		self.rect(x, y, w, 1, color);
	}
}
