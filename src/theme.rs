//! Runtime-switchable color palettes and layout metrics.
//!
//! Presets: dark (default), light, gruvbox, nord — plus a fully custom theme
//! edited in Settings → Appearance and persisted in settings.json. Draw code
//! reads colors through [`cur()`] (a cheap `Copy` snapshot of ~20 colors);
//! the global corner-radius multiplier lives on the theme too and is applied
//! by the rasterizer in `draw.rs`.

use serde::{Deserialize, Serialize};
use std::sync::RwLock;

pub type Rgba = [u8; 4];

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    pub bg: Rgba,
    pub surface: Rgba,
    pub surface_2: Rgba,
    pub input_bg: Rgba,
    /// Separators / scrollbar only.
    pub border: Rgba,
    pub selection: Rgba,
    pub text: Rgba,
    pub text_dim: Rgba,
    pub text_faint: Rgba,
    pub accent: Rgba,
    pub accent_hover: Rgba,
    pub btn_disabled: Rgba,
    pub btn_text: Rgba,
    /// Online presence / Join button.
    pub green: Rgba,
    pub green_hover: Rgba,
    /// In-game presence.
    pub blue: Rgba,
    /// Studio presence.
    pub orange: Rgba,
    /// Offline presence.
    pub gray: Rgba,
    /// Errors / logo tile.
    pub red: Rgba,
    pub white: Rgba,
    /// Global corner-radius multiplier (1.0 = default rounding).
    pub radius_scale: f32,
}

pub const DARK: Theme = Theme {
    bg: [0x17, 0x19, 0x1B, 0xFF],
    surface: [0x23, 0x25, 0x27, 0xFF],
    surface_2: [0x2A, 0x2D, 0x2F, 0xFF],
    input_bg: [0x1E, 0x21, 0x23, 0xFF],
    border: [0x3A, 0x3E, 0x42, 0xFF],
    selection: [0x33, 0x5F, 0xFF, 0x60],
    text: [0xF2, 0xF4, 0xF5, 0xFF],
    text_dim: [0x9A, 0x9F, 0xA6, 0xFF],
    text_faint: [0x69, 0x6E, 0x74, 0xFF],
    accent: [0x33, 0x5F, 0xFF, 0xFF],
    accent_hover: [0x2A, 0x52, 0xE0, 0xFF],
    btn_disabled: [0x30, 0x34, 0x38, 0xFF],
    btn_text: [0xFF, 0xFF, 0xFF, 0xFF],
    green: [0x00, 0xB0, 0x6F, 0xFF],
    green_hover: [0x00, 0x9A, 0x61, 0xFF],
    blue: [0x00, 0xA2, 0xFF, 0xFF],
    orange: [0xF2, 0x8A, 0x2E, 0xFF],
    gray: [0x6B, 0x70, 0x75, 0xFF],
    red: [0xE2, 0x23, 0x1A, 0xFF],
    white: [0xFF, 0xFF, 0xFF, 0xFF],
    radius_scale: 1.0,
};

pub const LIGHT: Theme = Theme {
    bg: [0xF2, 0xF4, 0xF5, 0xFF],
    surface: [0xFF, 0xFF, 0xFF, 0xFF],
    surface_2: [0xE4, 0xE7, 0xEA, 0xFF],
    input_bg: [0xEB, 0xEE, 0xF0, 0xFF],
    border: [0xC6, 0xCC, 0xD2, 0xFF],
    selection: [0x33, 0x5F, 0xFF, 0x3C],
    text: [0x1B, 0x1D, 0x1F, 0xFF],
    text_dim: [0x5A, 0x60, 0x67, 0xFF],
    text_faint: [0x8B, 0x91, 0x99, 0xFF],
    accent: [0x2B, 0x54, 0xD8, 0xFF],
    accent_hover: [0x23, 0x47, 0xC0, 0xFF],
    btn_disabled: [0xD6, 0xDA, 0xDD, 0xFF],
    btn_text: [0xFF, 0xFF, 0xFF, 0xFF],
    green: [0x00, 0x9E, 0x66, 0xFF],
    green_hover: [0x00, 0x88, 0x58, 0xFF],
    blue: [0x00, 0x92, 0xE8, 0xFF],
    orange: [0xD9, 0x76, 0x1C, 0xFF],
    gray: [0x9A, 0xA0, 0xA6, 0xFF],
    red: [0xD0, 0x1F, 0x16, 0xFF],
    white: [0xFF, 0xFF, 0xFF, 0xFF],
    radius_scale: 1.0,
};

pub const GRUVBOX: Theme = Theme {
    bg: [0x28, 0x28, 0x28, 0xFF],
    surface: [0x3C, 0x38, 0x36, 0xFF],
    surface_2: [0x50, 0x49, 0x45, 0xFF],
    input_bg: [0x32, 0x30, 0x2F, 0xFF],
    border: [0x66, 0x5C, 0x54, 0xFF],
    selection: [0xFA, 0xBD, 0x2F, 0x38],
    text: [0xEB, 0xDB, 0xB2, 0xFF],
    text_dim: [0xBD, 0xAE, 0x93, 0xFF],
    text_faint: [0x92, 0x83, 0x74, 0xFF],
    accent: [0xFA, 0xBD, 0x2F, 0xFF],
    accent_hover: [0xD7, 0x99, 0x21, 0xFF],
    btn_disabled: [0x50, 0x49, 0x45, 0xFF],
    btn_text: [0x28, 0x28, 0x28, 0xFF],
    green: [0xB8, 0xBB, 0x26, 0xFF],
    green_hover: [0x98, 0x97, 0x1A, 0xFF],
    blue: [0x83, 0xA5, 0x98, 0xFF],
    orange: [0xFE, 0x80, 0x19, 0xFF],
    gray: [0x92, 0x83, 0x74, 0xFF],
    red: [0xFB, 0x49, 0x34, 0xFF],
    white: [0xEB, 0xDB, 0xB2, 0xFF],
    radius_scale: 1.0,
};

pub const NORD: Theme = Theme {
    bg: [0x2E, 0x34, 0x40, 0xFF],
    surface: [0x3B, 0x42, 0x52, 0xFF],
    surface_2: [0x43, 0x4C, 0x5E, 0xFF],
    input_bg: [0x35, 0x3B, 0x48, 0xFF],
    border: [0x4C, 0x56, 0x6A, 0xFF],
    selection: [0x88, 0xC0, 0xD0, 0x38],
    text: [0xEC, 0xEF, 0xF4, 0xFF],
    text_dim: [0xD8, 0xDE, 0xE9, 0xFF],
    text_faint: [0x7B, 0x88, 0xA1, 0xFF],
    accent: [0x88, 0xC0, 0xD0, 0xFF],
    accent_hover: [0x81, 0xA1, 0xC1, 0xFF],
    btn_disabled: [0x4C, 0x56, 0x6A, 0xFF],
    btn_text: [0x2E, 0x34, 0x40, 0xFF],
    green: [0xA3, 0xBE, 0x8C, 0xFF],
    green_hover: [0x8F, 0xBC, 0xBB, 0xFF],
    blue: [0x81, 0xA1, 0xC1, 0xFF],
    orange: [0xD0, 0x87, 0x70, 0xFF],
    gray: [0x7B, 0x88, 0xA1, 0xFF],
    red: [0xBF, 0x61, 0x6A, 0xFF],
    white: [0xEC, 0xEF, 0xF4, 0xFF],
    radius_scale: 1.0,
};

/// (id, theme) of the built-in presets, in Settings-tab order.
pub const PRESETS: [(&str, Theme); 4] =
    [("dark", DARK), ("light", LIGHT), ("gruvbox", GRUVBOX), ("nord", NORD)];

static CURRENT: RwLock<Theme> = RwLock::new(DARK);

/// Snapshot of the theme currently applied. Cheap (`Copy`); call at the top
/// of every draw function that needs colors.
pub fn cur() -> Theme {
    *CURRENT.read().expect("theme lock")
}

/// Replace the active theme (Settings / startup).
pub fn apply(t: Theme) {
    *CURRENT.write().expect("theme lock") = t;
}

/// Resolve a preset id ("dark"|"light"|"gruvbox"|"nord"|"custom").
pub fn by_id(id: &str, custom: Option<Theme>) -> Theme {
    match id {
        "light" => LIGHT,
        "gruvbox" => GRUVBOX,
        "nord" => NORD,
        "custom" => custom.unwrap_or(DARK),
        _ => DARK,
    }
}

// Layout metrics (logical pixels, multiplied by scale factor at draw time).
pub const SIDEBAR_W_FULL: f32 = 178.0;
pub const SIDEBAR_W_MINI: f32 = 58.0;
pub const STATUSBAR_H: f32 = 26.0;
pub const ROW_H: f32 = 68.0;
/// Height of the horizontal friends strip (avatar + name + status).
pub const STRIP_H: f32 = 118.0;
/// Width of one friend card in the horizontal strip.
pub const STRIP_CARD_W: f32 = 92.0;
/// Width of one place card in the grid.
pub const PLACE_CARD_W: f32 = 158.0;
/// Side of the (square) place icon bitmap downloaded from the CDN.
pub const PLACE_ICON_PX: u32 = 176;
/// Width of the arrow scrollbar next to vertical lists.
pub const SCROLL_W: f32 = 16.0;
/// Side of the (square) avatar bitmap downloaded from the CDN.
pub const AVATAR_SIZE: u32 = 48;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_distinct_and_complete() {
        let ids: Vec<&str> = PRESETS.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, ["dark", "light", "gruvbox", "nord"]);
        assert_ne!(DARK.bg, LIGHT.bg);
        assert_ne!(DARK.bg, GRUVBOX.bg);
        assert_ne!(DARK.bg, NORD.bg);
        for (_, t) in PRESETS {
            assert_eq!(t.radius_scale, 1.0);
        }
    }

    #[test]
    fn theme_serializes_roundtrip() {
        let json = serde_json::to_string(&GRUVBOX).unwrap();
        let back: Theme = serde_json::from_str(&json).unwrap();
        assert_eq!(back, GRUVBOX);
    }
}
