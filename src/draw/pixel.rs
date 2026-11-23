//! Per-pixel writes of the software rasterizer: axis-aligned rectangles and
//! single (optionally anti-aliased) pixels.

use super::{Backing, Prim, FB};
use crate::theme::Rgba;

/// Alpha-blend `c` over one RGBA8 destination pixel.
#[inline(always)]
pub(super) fn blend_px(dst: &mut [u8], c: Rgba) {
	let a = c[3] as u32;
	if a == 255 {
		dst[0] = c[0];
		dst[1] = c[1];
		dst[2] = c[2];
		dst[3] = 255;
	} else if a != 0 {
		let ia = 255 - a;
		dst[0] = ((c[0] as u32 * a + dst[0] as u32 * ia + 127) / 255) as u8;
		dst[1] = ((c[1] as u32 * a + dst[1] as u32 * ia + 127) / 255) as u8;
		dst[2] = ((c[2] as u32 * a + dst[2] as u32 * ia + 127) / 255) as u8;
		dst[3] = dst[3].max(c[3]);
	}
}

impl FB<'_> {
	/// Fast filled rectangle: builds one row then memcpy's it per scanline.
	pub fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: Rgba) {
		if color[3] == 0 {
			return;
		}
		if self.is_gpu() {
			self.push(Prim::Rect { x, y, w, h, color });
			return;
		}
		let c = self.clip;
		let x0 = x.max(c.x0);
		let y0 = y.max(c.y0);
		let x1 = (x + w).min(c.x1);
		let y1 = (y + h).min(c.y1);
		if x0 >= x1 || y0 >= y1 {
			return;
		}
		let Backing::Soft(buf) = &mut self.backing else { return };
		let rw = (x1 - x0) as usize;
		if color[3] == 255 {
			// Opaque fast path: scratch row + row-wise memcpy.
			if self.scratch.len() < rw * 4 {
				self.scratch.resize(rw * 4, 0);
			}
			let mut i = 0;
			while i < rw {
				self.scratch[i * 4..i * 4 + 4].copy_from_slice(&color);
				i += 1;
			}
			for yy in y0..y1 {
				let off = (yy as usize * self.w as usize + x0 as usize) * 4;
				buf[off..off + rw * 4].copy_from_slice(&self.scratch[..rw * 4]);
			}
		} else {
			for yy in y0..y1 {
				let off = (yy as usize * self.w as usize + x0 as usize) * 4;
				for xx in 0..rw {
					let px = &mut buf[off + xx * 4..off + xx * 4 + 4];
					blend_px(px, color);
				}
			}
		}
	}

	/// Set a single pixel with alpha blending (clipped).
	pub fn set(&mut self, x: i32, y: i32, color: Rgba) {
		if color[3] == 0 {
			return;
		}
		if self.is_gpu() {
			self.push(Prim::Pix { x: x as f32, y: y as f32, color });
			return;
		}
		if !self.clip.contains(x, y) || x < 0 || y < 0 || x >= self.w || y >= self.h {
			return;
		}
		let Backing::Soft(buf) = &mut self.backing else { return };
		let off = (y as usize * self.w as usize + x as usize) * 4;
		blend_px(&mut buf[off..off + 4], color);
	}

	/// Anti-aliased pixel (splits coverage over 4 neighbors).
	pub fn set_f(&mut self, x: f32, y: f32, color: Rgba) {
		if color[3] == 0 {
			return;
		}
		if self.is_gpu() {
			self.push(Prim::Pix { x, y, color });
			return;
		}
		let fx = x - x.floor();
		let fy = y - y.floor();
		let x = x.floor() as i32;
		let y = y.floor() as i32;
		let a = color[3] as f32;
		let put = |fb: &mut Self, px: i32, py: i32, cov: f32| {
			if cov > 0.0 {
				let mut c = color;
				c[3] = (a * cov) as u8;
				fb.set(px, py, c);
			}
		};
		put(self, x, y, (1.0 - fx) * (1.0 - fy));
		put(self, x + 1, y, fx * (1.0 - fy));
		put(self, x, y + 1, (1.0 - fx) * fy);
		put(self, x + 1, y + 1, fx * fy);
	}
}
