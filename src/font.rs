//! Glyph rasterization with `ab_glyph`.
//!
//! Two paths:
//! * software (`FB` soft backend): glyphs are rasterized straight into the
//!   pixel buffer (headless previews / tests);
//! * GPU (`FB` gpu backend): each glyph is rasterized once into a shared
//!   atlas texture, and text becomes instanced quads (one per glyph).
//!
//! The atlas state lives inside [`Fonts`] behind a mutex because the draw
//! path only has `&Fonts`. The renderer drains the pending masks each frame
//! and uploads them to the atlas texture.

use crate::draw::{FB, GlyphSlot};
use crate::theme::Rgba;
use ab_glyph::{Font, FontArc, Glyph, PxScale, ScaleFont};
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// Atlas
// ---------------------------------------------------------------------------

/// Pixel dimensions + placement of a rasterized glyph (alpha coverage mask).
pub struct GlyphMask {
    pub key: GlyphKey,
    /// Placement inside the atlas page (set when the slot is created).
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    /// Coverage bytes (0..=255), row-major, `w` per row.
    pub data: Vec<u8>,
    /// Pen-relative offset of the glyph's top-left corner.
    pub left: f32,
    pub top: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct GlyphKey {
    pub bold: bool,
    /// Font size in px, quantized to 0.25 steps so nearby scales share slots.
    pub px_x4: u16,
    pub ch: char,
}

#[derive(Default)]
pub struct AtlasState {
    /// Placed slots (key -> slot in the atlas page).
    slots: std::collections::HashMap<GlyphKey, GlyphSlot>,
    /// Masks waiting to be uploaded by the renderer, in placement order.
    pub pending: Vec<GlyphMask>,
    /// All masks ever placed (kept for GPU texture regrowth).
    pub(crate) all: Vec<GlyphMask>,
    /// Shelf packer cursor.
    cursor_x: u16,
    cursor_y: u16,
    row_h: u16,
    /// Current atlas page size (square).
    pub page_size: u32,
}

impl AtlasState {
    const MAX_PAGE: u32 = 4096;

    pub fn new() -> AtlasState {
        AtlasState {
            slots: std::collections::HashMap::new(),
            pending: Vec::new(),
            all: Vec::new(),
            cursor_x: 1,
            cursor_y: 1,
            row_h: 0,
            page_size: 1024,
        }
    }

    /// Get (or create) the atlas slot for a glyph key.
    fn place(&mut self, key: GlyphKey, mut mask: GlyphMask) -> GlyphSlot {
        if let Some(slot) = self.slots.get(&key) {
            return *slot;
        }
        let (w, h) = (mask.w.max(1), mask.h.max(1));
        loop {
            // Shelf packer with 1px gutters.
            if self.cursor_x + w + 1 > self.page_size as u16 {
                // Move to the next shelf.
                self.cursor_y += self.row_h + 1;
                self.cursor_x = 1;
                self.row_h = 0;
            }
            if self.cursor_y + h + 1 <= self.page_size as u16 {
                break;
            }
            // Page is full: regrow (doubling) and re-place everything.
            self.page_size = (self.page_size * 2).min(Self::MAX_PAGE);
            self.slots.clear();
            self.pending.clear();
            self.cursor_x = 1;
            self.cursor_y = 1;
            self.row_h = 0;
            let all = std::mem::take(&mut self.all);
            for m in all {
                self.place_inner(m);
            }
        }
        let slot = GlyphSlot {
            page: 0,
            x: self.cursor_x,
            y: self.cursor_y,
            w,
            h,
            left: mask.left,
            top: mask.top,
        };
        self.slots.insert(key, slot);
        self.cursor_x += w + 1;
        self.row_h = self.row_h.max(h);
        mask.x = slot.x;
        mask.y = slot.y;
        self.pending.push(GlyphMask {
            key,
            x: mask.x,
            y: mask.y,
            w: mask.w,
            h: mask.h,
            data: mask.data.clone(),
            left: mask.left,
            top: mask.top,
        });
        self.all.push(mask);
        slot
    }

    fn place_inner(&mut self, m: GlyphMask) {
        let (w, h) = (m.w.max(1), m.h.max(1));
        if self.cursor_x + w + 1 > self.page_size as u16 {
            self.cursor_y += self.row_h + 1;
            self.cursor_x = 1;
            self.row_h = 0;
        }
        if self.cursor_y + h + 1 > self.page_size as u16 {
            // Cannot happen after regrowth places everything on the bigger
            // page; clamp defensively.
            return;
        }
        let slot = GlyphSlot {
            page: 0,
            x: self.cursor_x,
            y: self.cursor_y,
            w,
            h,
            left: m.left,
            top: m.top,
        };
        self.slots.insert(m.key, slot);
        self.cursor_x += w + 1;
        self.row_h = self.row_h.max(h);
        let mut m = m;
        m.x = slot.x;
        m.y = slot.y;
        self.all.push(m);
    }
}

// ---------------------------------------------------------------------------
// Fonts
// ---------------------------------------------------------------------------

pub struct Fonts {
    pub regular: FontArc,
    pub bold: FontArc,
    pub bullet: bool, // whether U+2022 exists (password masking)
    pub atlas: Mutex<AtlasState>,
}

impl Fonts {
    pub fn load() -> Fonts {
        let regular = FontArc::try_from_slice(include_bytes!("../assets/fonts/DejaVuSans.ttf"))
            .expect("embedded regular font");
        let bold = FontArc::try_from_slice(include_bytes!("../assets/fonts/DejaVuSans-Bold.ttf"))
            .expect("embedded bold font");
        let bullet = regular.glyph_id('\u{2022}').0 != 0 || regular.glyph_id('\u{25CF}').0 != 0;
        Fonts { regular, bold, bullet, atlas: Mutex::new(AtlasState::new()) }
    }

    fn face(&self, bold: bool) -> &FontArc {
        if bold { &self.bold } else { &self.regular }
    }
}

/// Width of `text` when rendered at `px` pixel size.
pub fn measure(fonts: &Fonts, text: &str, px: f32, bold: bool) -> f32 {
    let font = fonts.face(bold);
    let scaled = font.as_scaled(PxScale::from(px));
    let mut w = 0.0f32;
    for ch in text.chars() {
        let id = font.glyph_id(ch);
        w += scaled.h_advance(id);
    }
    w
}

/// Rasterize one glyph into an alpha mask (or None for whitespace).
fn rasterize(font: &FontArc, px: f32, ch: char) -> Option<GlyphMask> {
    let id = font.glyph_id(ch);
    if id.0 == 0 {
        return None;
    }
    let scale = PxScale::from(px);
    let scaled = font.as_scaled(scale);
    let glyph = id.with_scale(scale);
    let outlined = font.outline_glyph(glyph)?;
    let bounds = outlined.px_bounds();
    let (bx, by) = (bounds.min.x as i32, bounds.min.y as i32);
    let w = bounds.width().ceil().max(1.0) as u16;
    let h = bounds.height().ceil().max(1.0) as u16;
    let mut data = vec![0u8; w as usize * h as usize];
    outlined.draw(|gx, gy, alpha| {
        let (ix, iy) = (gx as i32 - bx, gy as i32 - by);
        if ix >= 0 && iy >= 0 && (ix as u16) < w && (iy as u16) < h {
            let idx = iy as usize * w as usize + ix as usize;
            data[idx] = (alpha * 255.0).round().min(255.0) as u8;
        }
    });
    let _ = scaled; // bearing comes from the outline bounds directly
    Some(GlyphMask {
        key: GlyphKey { bold: false, px_x4: 0, ch: '\0' }, // filled by caller
        x: 0,
        y: 0,
        w,
        h,
        data,
        left: bounds.min.x,
        top: bounds.min.y,
    })
}

/// Draw text with baseline at `y`. Returns the advance width (including
/// letter_spacing).
pub fn draw(
    fonts: &Fonts,
    fb: &mut FB,
    mut x: f32,
    y_baseline: f32,
    text: &str,
    px: f32,
    color: Rgba,
    bold: bool,
    letter_spacing: f32,
) -> f32 {
    let font = fonts.face(bold);
    let scale = PxScale::from(px);
    let scaled = font.as_scaled(scale);
    for ch in text.chars() {
        let id = font.glyph_id(ch);
        if fb.is_gpu() {
            // GPU path: rasterize once, place in the atlas, emit a quad.
            let key = GlyphKey { bold, px_x4: (px * 4.0).round() as u16, ch };
            if id.0 != 0 {
                let placed = {
                    let mut atlas = fonts.atlas.lock().unwrap();
                    if let Some(slot) = atlas.slots.get(&key) {
                        Some(*slot)
                    } else if let Some(mut mask) = rasterize(font, px, ch) {
                        mask.key = key;
                        let slot = atlas.place(key, mask);
                        Some(slot)
                    } else {
                        None
                    }
                };
                if let Some(slot) = placed {
                    // The slot's left/top are pen-relative from the glyph
                    // bounds (baseline-based).
                    fb.push_glyph(x + slot.left, y_baseline + slot.top, slot, color);
                }
            }
        } else {
            // Software path: straight into the buffer (as before).
            let glyph: Glyph = id.with_scale_and_position(scale, ab_glyph::point(x, y_baseline));
            if let Some(outlined) = font.outline_glyph(glyph) {
                let bmin = outlined.px_bounds().min;
                let bx = bmin.x as i32;
                let by = bmin.y as i32;
                outlined.draw(|gx, gy, alpha| {
                    if alpha > 0.004 {
                        let mut c = color;
                        c[3] = (color[3] as f32 * alpha).round().min(255.0) as u8;
                        fb.set(bx + gx as i32, by + gy as i32, c);
                    }
                });
            }
        }
        x += scaled.h_advance(id) + letter_spacing;
    }
    x
}

/// Draw text truncated with "…" so it never exceeds `max_w`. Returns the
/// advance width actually used.
pub fn draw_ellipsis(
    fonts: &Fonts,
    fb: &mut FB,
    x: f32,
    y_baseline: f32,
    text: &str,
    px: f32,
    color: Rgba,
    bold: bool,
    max_w: f32,
) -> f32 {
    if measure(fonts, text, px, bold) <= max_w {
        return draw(fonts, fb, x, y_baseline, text, px, color, bold, 0.0);
    }
    let ellipsis = "\u{2026}";
    let mut end = text.len();
    while end > 0 {
        // Step back over whole chars.
        end -= 1;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        let candidate = format!("{}{}", &text[..end], ellipsis);
        if measure(fonts, &candidate, px, bold) <= max_w {
            return draw(fonts, fb, x, y_baseline, &candidate, px, color, bold, 0.0);
        }
    }
    0.0
}

/// Width of the first `n` chars (byte length must be a char boundary).
pub fn measure_prefix(fonts: &Fonts, text: &str, n_bytes: usize, px: f32, bold: bool) -> f32 {
    let n = n_bytes.min(text.len());
    let mut n = n;
    while n > 0 && !text.is_char_boundary(n) {
        n -= 1;
    }
    measure(fonts, &text[..n], px, bold)
}

/// Draw a password-style field content: one bullet per char.
pub fn masked_str(fonts: &Fonts, text: &str) -> String {
    let ch = if fonts.bullet { '\u{2022}' } else { '*' };
    std::iter::repeat(ch).take(text.chars().count()).collect()
}

/// Split `text` into lines that each fit `max_w` px at the given size.
/// Breaks at spaces; a single word wider than the whole line is
/// hard-broken at the character that fits. Existing '\n' breaks are kept.
pub fn wrap(fonts: &Fonts, text: &str, max_w: f32, px: f32, bold: bool) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for para in text.split('\n') {
        let mut cur = String::new();
        for word in para.split(' ') {
            if word.is_empty() {
                continue;
            }
            let candidate = if cur.is_empty() {
                word.to_string()
            } else {
                format!("{} {}", cur, word)
            };
            if measure(fonts, &candidate, px, bold) <= max_w {
                cur = candidate;
                continue;
            }
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            if measure(fonts, word, px, bold) <= max_w {
                cur = word.to_string();
            } else {
                // Hard-break a word wider than the whole line.
                for ch in word.chars() {
                    let cand = format!("{}{}", cur, ch);
                    if !cur.is_empty() && measure(fonts, &cand, px, bold) > max_w {
                        lines.push(std::mem::take(&mut cur));
                    }
                    cur.push(ch);
                }
            }
        }
        lines.push(cur);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Draw `text` wrapped to `max_w` as a horizontally centered block whose
/// vertical center sits at `cy`. Returns the total block height.
pub fn draw_wrapped_centered(
    fonts: &Fonts,
    fb: &mut FB,
    cx: f32,
    cy: f32,
    max_w: f32,
    text: &str,
    px: f32,
    color: Rgba,
    bold: bool,
) -> f32 {
    let lines = wrap(fonts, text, max_w, px, bold);
    let line_h = px * 1.4;
    let total_h = lines.len() as f32 * line_h;
    let mut y_baseline = cy - total_h / 2.0 + px * 0.85;
    for line in &lines {
        let w = measure(fonts, line, px, bold);
        draw(fonts, fb, cx - w / 2.0, y_baseline, line, px, color, bold, 0.0);
        y_baseline += line_h;
    }
    total_h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_respects_max_width() {
        let fonts = Fonts::load();
        let long = "Type at least 2 characters of a username and press Enter";
        // Narrow phone pane: the sentence must split into several lines.
        let lines = wrap(&fonts, long, 160.0, 14.0, false);
        assert!(lines.len() > 1, "expected wrapping, got {:?}", lines);
        for l in &lines {
            assert!(measure(&fonts, l, 14.0, false) <= 160.0, "line too wide: {:?}", l);
        }
        // Wide pane: stays a single line, unchanged.
        let one = wrap(&fonts, long, 600.0, 14.0, false);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0], long);
        // Existing newlines are preserved as hard breaks.
        let two = wrap(&fonts, "alpha\nbeta", 600.0, 14.0, false);
        assert_eq!(two, vec!["alpha".to_string(), "beta".to_string()]);
        // A word wider than the whole line is hard-broken to fit.
        let hard = wrap(&fonts, "AAAAAAAAAAAAAAAAAAAA", 50.0, 14.0, false);
        assert!(hard.len() > 1);
        for l in &hard {
            assert!(measure(&fonts, l, 14.0, false) <= 51.0, "hard line too wide: {:?}", l);
        }
    }
}
