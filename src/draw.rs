//! UI drawing primitives with two interchangeable backends.
//!
//! * [`FB::new`] — the original software rasterizer writing into an RGBA8
//!   buffer (used by the headless `render_preview` example and unit tests).
//! * [`FB::gpu`] — records the same primitives into a command list that
//!   [`crate::gfx`] renders on the GPU (the live path).
//!
//! Keeping one API for both means every widget draws through the same code
//! no matter which backend is active.

use crate::theme::Rgba;
use std::sync::Arc;

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

/// A glyph placed from the shared font atlas (GPU path). The soft backend
/// rasterizes text directly, so it never builds these.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphSlot {
    /// Atlas page (0 = the single page today) and pixel rect inside it.
    pub page: u32,
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    /// Offset of the glyph's top-left corner from the pen position.
    pub left: f32,
    pub top: f32,
}

/// One recorded primitive (GPU backend).
#[derive(Clone, Debug)]
pub enum Prim {
    Rect { x: i32, y: i32, w: i32, h: i32, color: Rgba },
    /// SDF rounded rectangle; `radius` is already scaled by the theme.
    RoundRect { x: i32, y: i32, w: i32, h: i32, radius: f32, color: Rgba },
    /// SDF rounded rectangle rotated around its center (angle in radians).
    RoundRectRot { cx: i32, cy: i32, w: i32, h: i32, radius: f32, angle: f32, color: Rgba },
    /// Stroked rounded rectangle (GPU-only, from outline_rounded).
    RoundRectBorder { cx: i32, cy: i32, w: i32, h: i32, radius: f32, angle: f32, border: f32, color: Rgba },
    Circle { cx: i32, cy: i32, r: i32, color: Rgba },
    Ring { cx: i32, cy: i32, ro: i32, ri: i32, color: Rgba },
    /// Texture from the image cache (`hash` keys the GPU-side cache).
    Image { x: i32, y: i32, sw: i32, sh: i32, dw: i32, dh: i32, corner: i32, hash: u64, data: Arc<Vec<u8>> },
    /// Glyph quad sampled from the font atlas (alpha in the R channel).
    Glyph { x: f32, y: f32, slot: GlyphSlot, color: Rgba },
    /// Single pixel (from set/set_f; only a handful of call sites exist).
    Pix { x: f32, y: f32, color: Rgba },
}

#[derive(Debug)]
pub enum Backing<'a> {
    Soft(&'a mut [u8]),
    /// (primitive, clip at record time) — the renderer batches by clip for
    /// scissor changes.
    Gpu(Vec<(Prim, Clip)>),
}

/// A drawable surface: either a writable RGBA8 buffer with clipping, or a
/// GPU primitive recorder. Both carry the same size + scissor state.
pub struct FB<'a> {
    pub w: i32,
    pub h: i32,
    pub clip: Clip,
    backing: Backing<'a>,
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
    /// Software backend: draw straight into `buf` (w*h*4 bytes, RGBA8).
    pub fn new(buf: &'a mut [u8], w: i32, h: i32) -> Self {
        FB { w, h, clip: Clip::full(w, h), backing: Backing::Soft(buf), scratch: Vec::new() }
    }

    /// GPU backend: record primitives for the wgpu renderer.
    pub fn gpu(w: i32, h: i32) -> Self {
        FB { w, h, clip: Clip::full(w, h), backing: Backing::Gpu(Vec::with_capacity(4096)), scratch: Vec::new() }
    }

    pub fn is_gpu(&self) -> bool {
        matches!(self.backing, Backing::Gpu(_))
    }

    /// Take the recorded (primitive, clip) list (GPU backend only).
    pub fn take_prims(&mut self) -> Vec<(Prim, Clip)> {
        match &mut self.backing {
            Backing::Gpu(prims) => std::mem::take(prims),
            Backing::Soft(_) => Vec::new(),
        }
    }

    /// Push a primitive in GPU mode (no-op in soft mode; soft methods paint).
    fn push(&mut self, p: Prim) {
        if let Backing::Gpu(prims) = &mut self.backing {
            prims.push((p, self.clip));
        }
    }

    /// Glyph quad from the shared font atlas (GPU mode; soft mode ignores it
    /// because `font::draw` rasterizes text directly in soft mode).
    pub fn push_glyph(&mut self, x: f32, y: f32, slot: GlyphSlot, color: Rgba) {
        if let Backing::Gpu(prims) = &mut self.backing {
            prims.push((Prim::Glyph { x, y, slot, color }, self.clip));
        }
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
            self.push(Prim::RoundRect { x, y, w, h, radius: radius.min(w / 2).min(h / 2) as f32, color });
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
    pub fn rect_rounded_rot(&mut self, cx: i32, cy: i32, w: i32, h: i32, radius: i32, angle: f32, color: Rgba) {
        if angle.abs() < 0.001 {
            self.rect_rounded(cx - w / 2, cy - h / 2, w, h, radius, color);
            return;
        }
        if color[3] == 0 {
            return;
        }
        let radius = (radius as f32 * crate::theme::cur().radius_scale).max(0.0);
        if self.is_gpu() {
            self.push(Prim::RoundRectRot { cx, cy, w, h, radius, angle, color });
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
                let (lx, ly) = (dx as f32 * cos + dy as f32 * sin, -dx as f32 * sin + dy as f32 * cos);
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
            self.push(Prim::RoundRectBorder {
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
        let cy2 = (y + r) as f32;
        let _ = cy2;
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
        if r <= 0 || color[3] == 0 {
            return;
        }
        if self.is_gpu() {
            self.push(Prim::Circle { cx, cy, r, color });
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
            self.push(Prim::Ring { cx, cy, ro, ri: ri.max(0), color });
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

    /// Horizontal line.
    pub fn hline(&mut self, x: i32, y: i32, w: i32, color: Rgba) {
        self.rect(x, y, w, 1, color);
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
