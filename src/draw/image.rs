//! Image blitting for the software backend, plus the cheap content hash
//! that keys the GPU-side image cache.

use super::{Prim, FB};
use std::sync::Arc;

impl FB<'_> {
	/// Blit an RGBA8 source image (opaque or alpha-blended), optionally with
	/// rounded corners. The source is scaled (bilinear on GPU, nearest on
	/// CPU) from `sw` x `sh` into the `dw` x `dh` destination box.
	pub fn image(
		&mut self,
		x: i32,
		y: i32,
		src: &Arc<Vec<u8>>,
		sw: i32,
		sh: i32,
		dw: i32,
		dh: i32,
		corner_radius: i32,
	) {
		if sw <= 0 || sh <= 0 || dw <= 0 || dh <= 0 {
			return;
		}
		if self.is_gpu() {
			self.push(Prim::Image {
				x,
				y,
				sw,
				sh,
				dw,
				dh,
				corner: corner_radius,
				hash: image_hash(src, sw, sh),
				data: Arc::clone(src),
			});
			return;
		}
		let c = self.clip;
		// Rounded-corner silhouette is evaluated in destination space.
		let r = corner_radius.min(dw / 2).min(dh / 2);
		for dy_i in 0..dh {
			let dy = y + dy_i;
			if dy < c.y0 || dy >= c.y1 {
				continue;
			}
			let sy = (dy_i * sh) / dh;
			for dx_i in 0..dw {
				let dx = x + dx_i;
				if dx < c.x0 || dx >= c.x1 {
					continue;
				}
				if r > 0 {
					// Skip pixels outside the rounded silhouette (corners only).
					let (ncx, ncy) = (
						if dx_i < r {
							r - 1
						} else if dx_i >= dw - r {
							dw - r
						} else {
							-999
						},
						if dy_i < r {
							r - 1
						} else if dy_i >= dh - r {
							dh - r
						} else {
							-999
						},
					);
					if ncx != -999 && ncy != -999 {
						let ddx = (dx_i - ncx) as f32;
						let ddy = (dy_i - ncy) as f32;
						if (ddx * ddx + ddy * ddy).sqrt() > r as f32 - 0.5 {
							continue;
						}
					}
				}
				let sx = (dx_i * sw) / dw;
				let off = ((sy * sw + sx) * 4) as usize;
				if off + 4 <= src.len() {
					let col = [src[off], src[off + 1], src[off + 2], src[off + 3]];
					self.set(dx, dy, col);
				}
			}
		}
	}
}

/// Cheap content hash for the GPU image cache. Samples the buffer so the
/// cost stays low even for large icons.
pub fn image_hash(src: &[u8], sw: i32, sh: i32) -> u64 {
	let mut h: u64 = 0xcbf29ce484222325;
	let step = |b: u8, h: &mut u64| {
		*h ^= b as u64;
		*h = h.wrapping_mul(0x100000001b3);
	};
	step(sw as u8, &mut h);
	step((sw >> 8) as u8, &mut h);
	step(sh as u8, &mut h);
	step((sh >> 8) as u8, &mut h);
	let len = src.len();
	step(len as u8, &mut h);
	step((len >> 8) as u8, &mut h);
	step((len >> 16) as u8, &mut h);
	step((len >> 24) as u8, &mut h);
	// Sample evenly: first/last 16 bytes + every (len/32)th byte.
	let n = src.len();
	for i in 0..16.min(n) {
		step(src[i], &mut h);
	}
	if n > 16 {
		for i in 0..16.min(n) {
			step(src[n - 1 - i], &mut h);
		}
	}
	let stride = (n / 37).max(1);
	let mut i = stride;
	while i < n {
		step(src[i], &mut h);
		i += stride;
	}
	h
}
