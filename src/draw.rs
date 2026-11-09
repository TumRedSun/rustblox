//! Minimal software rasterizer on top of the `pixels` RGBA8 frame buffer.
//!
//! Everything draws through [`FB`], which carries the frame buffer, its size
//! and a scissor rectangle so widgets can clip themselves (scroll lists etc.).

use crate::theme::Rgba;

fn vec_len(x: f32, y: f32) -> f32 {
    (x * x + y * y).sqrt()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clip {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Clip {
    pub fn full(w: i32, h: i32) -> Self {
        Clip { x0: 0, y0: 0, x1: w, y1: h }
    }

    pub fn intersect(a: Clip, b: Clip) -> Clip {
        Clip {
            x0: a.x0.max(b.x0),
            y0: a.y0.max(b.y0),
            x1: a.x1.min(b.x1),
            y1: a.y1.min(b.y1),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x0 && x < self.x1 && y >= self.y0 && y < self.y1
    }
}

/// A writable RGBA8 frame buffer with clipping.
pub struct FB<'a> {
    pub buf: &'a mut [u8],
    pub w: i32,
    pub h: i32,
    pub clip: Clip,
    scratch: Vec<u8>,
}

#[inline(always)]
fn blend_px(dst: &mut [u8], c: Rgba) {
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

impl<'a> FB<'a> {
    pub fn new(buf: &'a mut [u8], w: i32, h: i32) -> Self {
        FB { buf, w, h, clip: Clip::full(w, h), scratch: Vec::new() }
    }

    pub fn set_clip(&mut self, clip: Clip) {
        self.clip = Clip::intersect(clip, Clip::full(self.w, self.h));
    }

    pub fn clip(&self) -> Clip {
        self.clip
    }

    /// Fill the whole clip area.
    pub fn fill(&mut self, color: Rgba) {
        let c = self.clip;
        self.rect(c.x0, c.y0, c.x1 - c.x0, c.y1 - c.y0, color);
    }

    /// Fast filled rectangle: builds one row then memcpy's it per scanline.
    pub fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: Rgba) {
        let c = self.clip;
        let x0 = x.max(c.x0);
        let y0 = y.max(c.y0);
        let x1 = (x + w).min(c.x1);
        let y1 = (y + h).min(c.y1);
        if x0 >= x1 || y0 >= y1 || color[3] == 0 {
            return;
        }
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
                self.buf[off..off + rw * 4].copy_from_slice(&self.scratch[..rw * 4]);
            }
        } else {
            for yy in y0..y1 {
                let off = (yy as usize * self.w as usize + x0 as usize) * 4;
                for xx in 0..rw {
                    let px = &mut self.buf[off + xx * 4..off + xx * 4 + 4];
                    blend_px(px, color);
                }
            }
        }
    }

    /// Filled rounded rectangle (radius in px). Center uses the fast path,
    /// only corner tiles do per-pixel work. The radius is scaled by the
    /// theme's global rounding multiplier (Settings → Appearance).
    pub fn rect_rounded(&mut self, x: i32, y: i32, w: i32, h: i32, radius: i32, color: Rgba) {
        if radius <= 0 {
            self.rect(x, y, w, h, color);
            return;
        }
        let radius = (radius as f32 * crate::theme::cur().radius_scale).round() as i32;
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
    /// Used for the rotated logo tile; the area is small so the per-pixel
    /// cost is negligible.
    pub fn rect_rounded_rot(&mut self, cx: i32, cy: i32, w: i32, h: i32, radius: i32, angle: f32, color: Rgba) {
        if angle.abs() < 0.001 {
            self.rect_rounded(cx - w / 2, cy - h / 2, w, h, radius, color);
            return;
        }
        let (hw, hh) = (w as f32 / 2.0, h as f32 / 2.0);
        let r = (radius as f32 * crate::theme::cur().radius_scale).max(0.0);
        let (sin, cos) = angle.sin_cos();
        // Bounding box big enough for the rotation (hypotenuse of half-size).
        let bound = (hw * hw + hh * hh).sqrt().ceil() as i32 + 2;
        let alpha_scale = color[3] as f32;
        for dy in -bound..=bound {
            for dx in -bound..=bound {
                // Inverse-rotate the pixel offset into the tile's local space.
                let (lx, ly) = (dx as f32 * cos + dy as f32 * sin, -dx as f32 * sin + dy as f32 * cos);
                // Rounded-box signed distance.
                let qx = lx.abs() - (hw - r);
                let qy = ly.abs() - (hh - r);
                let dist = vec_len(qx.max(0.0), qy.max(0.0)) + qx.max(qy).min(0.0) - r;
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
                    let px = if corner.0 == cx1 { corner.0 + r as f32 - 1.0 - dx } else { corner.0 + dx };
                    let py = if corner.1 == cy1 { corner.1 + r as f32 - 1.0 - dy } else { corner.1 + dy };
                    self.set_f(px + sx - 0.5, py + sy - 0.5, color);
                }
            }
        }
    }

    /// Filled circle (fast: one sqrt per scanline).
    pub fn circle(&mut self, cx: i32, cy: i32, r: i32, color: Rgba) {
        if r <= 0 {
            return;
        }
        for dy in -r..=r {
            let dx = ((r * r - dy * dy) as f32).sqrt().round() as i32;
            self.rect(cx - dx, cy + dy, dx * 2 + 1, 1, color);
        }
    }

    /// Blit an RGBA8 source image (opaque or alpha-blended), optionally with
    /// rounded corners. The source is scaled (nearest neighbor) from
    /// `sw` x `sh` into the `dw` x `dh` destination box, so the buffer and
    /// the on-screen size may differ (UI scale, chip vs row avatars).
    pub fn image(
        &mut self,
        x: i32,
        y: i32,
        src: &[u8],
        sw: i32,
        sh: i32,
        dw: i32,
        dh: i32,
        corner_radius: i32,
    ) {
        if sw <= 0 || sh <= 0 || dw <= 0 || dh <= 0 {
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
                        if dx_i < r { r - 1 } else if dx_i >= dw - r { dw - r } else { -999 },
                        if dy_i < r { r - 1 } else if dy_i >= dh - r { dh - r } else { -999 },
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

    /// Set a single pixel with alpha blending (clipped).
    pub fn set(&mut self, x: i32, y: i32, color: Rgba) {
        if !self.clip.contains(x, y) || x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        let off = (y as usize * self.w as usize + x as usize) * 4;
        blend_px(&mut self.buf[off..off + 4], color);
    }

    /// Anti-aliased pixel (splits coverage over 4 neighbors).
    pub fn set_f(&mut self, x: f32, y: f32, color: Rgba) {
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

    /// Horizontal line.
    pub fn hline(&mut self, x: i32, y: i32, w: i32, color: Rgba) {
        self.rect(x, y, w, 1, color);
    }
}
