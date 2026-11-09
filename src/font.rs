//! Glyph rasterization with `ab_glyph`, drawn straight into the pixel buffer.

use crate::draw::FB;
use crate::theme::Rgba;
use ab_glyph::{Font, FontArc, Glyph, PxScale, ScaleFont};

pub struct Fonts {
    pub regular: FontArc,
    pub bold: FontArc,
    pub bullet: bool, // whether U+2022 exists (password masking)
}

impl Fonts {
    pub fn load() -> Fonts {
        let regular = FontArc::try_from_slice(include_bytes!("../assets/fonts/DejaVuSans.ttf"))
            .expect("embedded regular font");
        let bold = FontArc::try_from_slice(include_bytes!("../assets/fonts/DejaVuSans-Bold.ttf"))
            .expect("embedded bold font");
        let bullet = regular.glyph_id('\u{2022}').0 != 0 || regular.glyph_id('\u{25CF}').0 != 0;
        Fonts { regular, bold, bullet }
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

/// Draw text with baseline at `y`. Returns advance width (including
/// letter_spacing). Clips to `fb.clip`.
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
        let glyph: Glyph = id.with_scale_and_position(scale, ab_glyph::point(x, y_baseline));
        if let Some(outlined) = font.outline_glyph(glyph) {
            outlined.draw(|gx, gy, alpha| {
                if alpha > 0.004 {
                    let mut c = color;
                    c[3] = (color[3] as f32 * alpha).round().min(255.0) as u8;
                    fb.set(gx as i32, gy as i32, c);
                }
            });
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
