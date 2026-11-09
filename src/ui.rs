//! Retained widget state (text fields) + immediate-mode hit testing and
//! button drawing helpers shared by all screens.

use crate::draw::FB;
use crate::font;
use crate::theme::{self, Rgba};

// ---------------------------------------------------------------------------
// Hit testing
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FieldId {
    Username,
    Password,
    Cookie,
    Code,
    Search,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    None,
    Field(FieldId),
    TabPassword,
    TabCookie,
    SignIn,
    Verify,
    Resend,
    Back,
    Refresh,
    Logout,
    Scrollbar,
    Row(u64),
}

#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x as f32 && x < (self.x + self.w) as f32 && y >= self.y as f32 && y < (self.y + self.h) as f32
    }
}

#[derive(Default)]
pub struct HitMap {
    rects: Vec<(Rect, Hit)>,
}

impl HitMap {
    pub fn clear(&mut self) {
        self.rects.clear();
    }
    pub fn add(&mut self, r: Rect, h: Hit) {
        self.rects.push((r, h));
    }
    pub fn at(&self, x: f32, y: f32) -> Hit {
        for (r, h) in self.rects.iter().rev() {
            if r.contains(x, y) {
                return h.clone();
            }
        }
        Hit::None
    }
    pub fn rect_of(&self, target: &Hit) -> Option<Rect> {
        self.rects.iter().find(|(_, h)| h == target).map(|(r, _)| *r)
    }
}

// ---------------------------------------------------------------------------
// Text field
// ---------------------------------------------------------------------------

pub struct TextField {
    pub value: String,
    /// Byte offset of the caret inside `value`.
    pub cursor: usize,
    /// Selected byte range (start <= end).
    pub sel: Option<(usize, usize)>,
    pub masked: bool,
    pub numeric: bool,
    pub max_bytes: usize,
    pub placeholder: &'static str,
    /// Horizontal scroll offset in px (for long values like cookies).
    pub scroll: f32,
}

impl TextField {
    pub fn new(placeholder: &'static str) -> Self {
        TextField {
            value: String::new(),
            cursor: 0,
            sel: None,
            masked: false,
            numeric: false,
            max_bytes: 4096,
            placeholder,
            scroll: 0.0,
        }
    }

    pub fn masked(mut self) -> Self {
        self.masked = true;
        self
    }

    pub fn numeric(mut self) -> Self {
        self.numeric = true;
        self
    }

    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
        self.sel = None;
        self.scroll = 0.0;
    }

    fn clamp_cursor(&mut self) {
        let mut c = self.cursor.min(self.value.len());
        while c > 0 && !self.value.is_char_boundary(c) {
            c -= 1;
        }
        self.cursor = c;
    }

    pub fn insert(&mut self, s: &str) {
        self.delete_selection();
        let filtered: String = if self.numeric {
            s.chars().filter(|c| c.is_ascii_digit()).collect()
        } else {
            s.chars().filter(|c| *c != '\n' && *c != '\r' && *c != '\t').collect()
        };
        if self.value.len() + filtered.len() > self.max_bytes {
            return;
        }
        if filtered.is_empty() {
            return;
        }
        self.clamp_cursor();
        self.value.insert_str(self.cursor, &filtered);
        self.cursor += filtered.len();
        self.clamp_cursor();
    }

    pub fn delete_selection(&mut self) -> bool {
        if let Some((a, b)) = self.sel.take() {
            let (a, b) = (a.min(b), a.max(b));
            if a <= self.value.len() && b <= self.value.len() {
                self.value.replace_range(a..b, "");
                self.cursor = a;
                self.clamp_cursor();
                return true;
            }
        }
        false
    }

    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        self.clamp_cursor();
        if self.cursor > 0 {
            let mut start = self.cursor - 1;
            while start > 0 && !self.value.is_char_boundary(start) {
                start -= 1;
            }
            self.value.replace_range(start..self.cursor, "");
            self.cursor = start;
        }
    }

    pub fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        self.clamp_cursor();
        if self.cursor < self.value.len() {
            let mut end = self.cursor + 1;
            while end < self.value.len() && !self.value.is_char_boundary(end) {
                end += 1;
            }
            self.value.replace_range(self.cursor..end, "");
        }
    }

    pub fn left(&mut self) {
        if self.sel.is_some() {
            self.sel = None;
            self.clamp_cursor();
            return;
        }
        self.clamp_cursor();
        if self.cursor > 0 {
            self.cursor -= 1;
            self.clamp_cursor();
        }
    }

    pub fn right(&mut self) {
        if self.sel.is_some() {
            self.sel = None;
            self.clamp_cursor();
            return;
        }
        self.clamp_cursor();
        if self.cursor < self.value.len() {
            self.cursor += 1;
            self.clamp_cursor();
        }
    }

    pub fn home(&mut self) {
        self.sel = None;
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.sel = None;
        self.cursor = self.value.len();
    }

    pub fn select_all(&mut self) {
        if !self.value.is_empty() {
            self.sel = Some((0, self.value.len()));
            self.cursor = self.value.len();
        }
    }

    /// Click inside the field: place caret near byte offset `click_bytes`.
    pub fn click_at(&mut self, click_bytes: usize) {
        // Convert byte estimate to a char boundary (approximation is fine).
        let mut n = click_bytes.min(self.value.len());
        while n > 0 && !self.value.is_char_boundary(n) {
            n -= 1;
        }
        self.cursor = n;
        self.sel = None;
    }

    pub fn draw(
        &mut self,
        ctx: &mut crate::app::UiCtx,
        fb: &mut FB,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        id: FieldId,
        label: Option<&str>,
    ) -> Rect {
        let s = ctx.scale;
        // Label above the box.
        if let Some(lb) = label {
            let ly = y - (16.0 * s) as i32;
            font::draw(ctx.fonts, fb, x as f32, (ly + (12.0 * s) as i32) as f32, lb, 12.0 * s, theme::TEXT_DIM, false, 0.0);
        }
        let focused = ctx.focus == Some(id);
        // Frameless input: filled rounded rect only. Focus is shown with a
        // brighter fill plus an accent underline drawn inside the rect (no
        // separate outline => it can never misalign).
        fb.rect_rounded(x, y, w, h, (8.0 * s) as i32, if focused { theme::SURFACE_2 } else { theme::INPUT_BG });
        if focused {
            let bar_h = (3.0 * s).max(1.0) as i32;
            let inset = (10.0 * s) as i32;
            fb.rect(x + inset, y + h - bar_h - 1, w - 2 * inset, bar_h, theme::ACCENT);
        }

        let pad_x = (12.0 * s) as i32;
        let inner_x = x + pad_x;
        let inner_w = w - 2 * pad_x;
        let font_px = 14.0 * s;
        let baseline = y as f32 + h as f32 * 0.5 + font_px * 0.35;

        let shown = if self.masked { font::masked_str(ctx.fonts, &self.value) } else { self.value.clone() };

        // Horizontal scrolling: keep the caret visible.
        let caret_x_rel = font::measure_prefix(ctx.fonts, &shown, self.cursor, font_px, false);
        let text_w = font::measure(ctx.fonts, &shown, font_px, false);
        if caret_x_rel - self.scroll > inner_w as f32 - 14.0 * s {
            self.scroll = caret_x_rel - inner_w as f32 + 14.0 * s;
        }
        if caret_x_rel - self.scroll < 0.0 {
            self.scroll = (caret_x_rel - 14.0 * s).max(0.0);
        }
        if text_w < self.scroll {
            self.scroll = (text_w - inner_w as f32).max(0.0);
        }

        let text_clip = crate::draw::Clip {
            x0: inner_x,
            y0: y + 2,
            x1: inner_x + inner_w,
            y1: y + h - 2,
        };
        let old_clip = fb.clip();
        fb.set_clip(text_clip);

        if self.value.is_empty() && !focused {
            font::draw(ctx.fonts, fb, inner_x as f32, baseline, self.placeholder, font_px, theme::TEXT_FAINT, false, 0.0);
        } else {
            // Selection highlight.
            if let Some((a, b)) = self.sel {
                let (a, b) = (a.min(b), a.max(b));
                let sx = font::measure_prefix(ctx.fonts, &shown, a, font_px, false);
                let ex = font::measure_prefix(ctx.fonts, &shown, b, font_px, false);
                fb.rect(
                    inner_x + ((sx - self.scroll) as i32),
                    y + (4.0 * s) as i32,
                    ((ex - sx) as i32).max(2),
                    h - (8.0 * s) as i32,
                    theme::SELECTION,
                );
            }
            font::draw(
                ctx.fonts,
                fb,
                inner_x as f32 - self.scroll,
                baseline,
                &shown,
                font_px,
                theme::TEXT,
                false,
                0.0,
            );
            // Caret.
            if focused {
                let cx = inner_x as f32 + caret_x_rel - self.scroll;
                let blink_on = (ctx.now_ms / 530) % 2 == 0;
                if blink_on {
                    fb.rect(
                        cx as i32,
                        y + (7.0 * s) as i32,
                        (2.0 * s).max(1.0) as i32,
                        h - (14.0 * s) as i32,
                        theme::ACCENT,
                    );
                }
                ctx.caret_anim = true;
            }
        }
        fb.set_clip(old_clip);

        let r = Rect { x, y, w, h };
        ctx.hits.add(r, Hit::Field(id));
        r
    }
}

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub enum BtnStyle {
    Primary,
    Secondary,
    Ghost,
}

/// Draw a button; `hover`/`disabled` affect colors. Adds to the hit map.
pub fn draw_button(
    ctx: &mut crate::app::UiCtx,
    fb: &mut FB,
    r: Rect,
    label: &str,
    style: BtnStyle,
    disabled: bool,
) -> Rect {
    let s = ctx.scale;
    let hovered = !disabled
        && ctx.cursor.0 >= r.x as f32
        && ctx.cursor.0 < (r.x + r.w) as f32
        && ctx.cursor.1 >= r.y as f32
        && ctx.cursor.1 < (r.y + r.h) as f32;
    let (bg, fg) = match style {
        BtnStyle::Primary => {
            if disabled {
                (theme::BTN_DISABLED, theme::TEXT_FAINT)
            } else if hovered {
                (theme::ACCENT_HOVER, theme::BTN_TEXT)
            } else {
                (theme::ACCENT, theme::BTN_TEXT)
            }
        }
        BtnStyle::Secondary => {
            if disabled {
                (theme::BTN_DISABLED, theme::TEXT_FAINT)
            } else if hovered {
                (theme::SURFACE_2, theme::TEXT)
            } else {
                (theme::SURFACE, theme::TEXT)
            }
        }
        BtnStyle::Ghost => (theme::BG, if hovered { theme::TEXT } else { theme::TEXT_DIM }),
    };
    fb.rect_rounded(r.x, r.y, r.w, r.h, (8.0 * s) as i32, bg);
    let tw = font::measure(ctx.fonts, label, 14.0 * s, true);
    let _ = font::draw(
        ctx.fonts,
        fb,
        r.x as f32 + (r.w as f32 - tw) * 0.5,
        r.y as f32 + r.h as f32 * 0.5 + 14.0 * s * 0.35,
        label,
        14.0 * s,
        fg,
        true,
        0.0,
    );
    r
}

/// Small spinner (dots rotating around a circle).
pub fn draw_spinner(fb: &mut FB, cx: f32, cy: f32, radius: f32, now_ms: u64, color: Rgba, s: f32) {
    let n = 8;
    for i in 0..n {
        let phase = (i as f64 / n as f64) * std::f64::consts::TAU + (now_ms as f64 / 130.0);
        let a = i as f64 / n as f64;
        let alpha = ((1.0 - a) * 0.9 + 0.1) * 255.0;
        let x = cx as f64 + radius as f64 * phase.cos();
        let y = cy as f64 + radius as f64 * phase.sin();
        let mut c = color;
        c[3] = alpha as u8;
        fb.circle(x as i32, y as i32, (2.2 * s).round() as i32, c);
    }
}

/// Colored status dot.
pub fn draw_dot(fb: &mut FB, cx: i32, cy: i32, d: i32, color: Rgba) {
    fb.circle(cx + d / 2, cy, d / 2, color);
}
