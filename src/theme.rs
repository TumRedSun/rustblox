//! Color palette and layout metrics (Roblox-app-inspired dark theme).

pub type Rgba = [u8; 4];

pub const BG: Rgba = [0x17, 0x19, 0x1B, 0xFF];
pub const SURFACE: Rgba = [0x23, 0x25, 0x27, 0xFF];
pub const SURFACE_2: Rgba = [0x2A, 0x2D, 0x2F, 0xFF];
pub const INPUT_BG: Rgba = [0x1E, 0x21, 0x23, 0xFF];
pub const BORDER: Rgba = [0x3A, 0x3E, 0x42, 0xFF]; // separators / scrollbar only
pub const SELECTION: Rgba = [0x33, 0x5F, 0xFF, 0x60];

pub const TEXT: Rgba = [0xF2, 0xF4, 0xF5, 0xFF];
pub const TEXT_DIM: Rgba = [0x9A, 0x9F, 0xA6, 0xFF];
pub const TEXT_FAINT: Rgba = [0x69, 0x6E, 0x74, 0xFF];

pub const ACCENT: Rgba = [0x33, 0x5F, 0xFF, 0xFF];
pub const ACCENT_HOVER: Rgba = [0x2A, 0x52, 0xE0, 0xFF];
pub const BTN_DISABLED: Rgba = [0x30, 0x34, 0x38, 0xFF];
pub const BTN_TEXT: Rgba = [0xFF, 0xFF, 0xFF, 0xFF];

pub const GREEN: Rgba = [0x00, 0xB0, 0x6F, 0xFF]; // online / Join button
pub const GREEN_HOVER: Rgba = [0x00, 0x9A, 0x61, 0xFF];
pub const BLUE: Rgba = [0x00, 0xA2, 0xFF, 0xFF]; // in game
pub const ORANGE: Rgba = [0xF2, 0x8A, 0x2E, 0xFF]; // studio
pub const GRAY: Rgba = [0x6B, 0x70, 0x75, 0xFF]; // offline
pub const RED: Rgba = [0xE2, 0x23, 0x1A, 0xFF]; // errors / logo
pub const WHITE: Rgba = [0xFF, 0xFF, 0xFF, 0xFF];

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
