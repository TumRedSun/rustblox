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
    let w = bounds.width().ceil().max(1.0) as u16;
    let h = bounds.height().ceil().max(1.0) as u16;
    let mut data = vec![0u8; w as usize * h as usize];
    // ab_glyph's OutlinedGlyph::draw yields (u32, u32) coordinates that are
    // ALREADY relative to the outline bounds origin (0..w, 0..h) — they are
    // NOT pen-relative, so bounds.min must NOT be subtracted here (doing so
    // pushes every row out of the mask and renders empty glyphs).
    outlined.draw(|gx, gy, alpha| {
        let (ix, iy) = (gx as usize, gy as usize);
        if ix < w as usize && iy < h as usize {
            let idx = iy * w as usize + ix;
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
