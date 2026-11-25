//! Theme slots: palette, names and the colour cycling used by the
//! Appearance settings pane.

#[allow(unused_imports)]
use crate::menu_ui::*;
use crate::theme;
use crate::theme::Theme;

pub(crate) fn pretty_theme_name(id: &str) -> &'static str {
	match id {
		"dark" => "Dark",
		"light" => "Light",
		"gruvbox" => "Gruvbox",
		"nord" => "Nord",
		"custom" => "Custom",
		_ => "Dark",
	}
}

// -- Appearance color slots --------------------------------------------------
pub(crate) const COLOR_LABELS: [&str; 17] = [
	"Background",
	"Surface",
	"Surface 2",
	"Input background",
	"Borders",
	"Text",
	"Text dim",
	"Text faint",
	"Accent",
	"Accent hover",
	"Disabled button",
	"Button text",
	"Green (online / Join)",
	"Blue (in game)",
	"Orange (Studio)",
	"Gray (offline)",
	"Red (errors / logo)",
];
pub(crate) fn color_slot_count() -> usize {
	COLOR_LABELS.len()
}
pub(crate) fn color_slot_label(i: usize) -> &'static str {
	COLOR_LABELS[i.min(COLOR_LABELS.len() - 1)]
}
/// Mutable access to color slot `i` of a theme.
pub(crate) fn theme_field_mut(t: &mut Theme, i: usize) -> &mut theme::Rgba {
	match i {
		0 => &mut t.bg,
		1 => &mut t.surface,
		2 => &mut t.surface_2,
		3 => &mut t.input_bg,
		4 => &mut t.border,
		5 => &mut t.text,
		6 => &mut t.text_dim,
		7 => &mut t.text_faint,
		8 => &mut t.accent,
		9 => &mut t.accent_hover,
		10 => &mut t.btn_disabled,
		11 => &mut t.btn_text,
		12 => &mut t.green,
		13 => &mut t.blue,
		14 => &mut t.orange,
		15 => &mut t.gray,
		_ => &mut t.red,
	}
}
/// Read a color slot (public: the app handler cycles slots).
pub fn theme_field(t: Theme, i: usize) -> theme::Rgba {
	match i {
		0 => t.bg,
		1 => t.surface,
		2 => t.surface_2,
		3 => t.input_bg,
		4 => t.border,
		5 => t.text,
		6 => t.text_dim,
		7 => t.text_faint,
		8 => t.accent,
		9 => t.accent_hover,
		10 => t.btn_disabled,
		11 => t.btn_text,
		12 => t.green,
		13 => t.blue,
		14 => t.orange,
		15 => t.gray,
		_ => t.red,
	}
}
/// Write a color slot (public: the app handler cycles slots).
pub fn set_theme_slot(mut t: Theme, i: usize, color: theme::Rgba) -> Theme {
	*theme_field_mut(&mut t, i) = color;
	t
}
/// Palette the color swatches cycle through (Settings → Appearance).
pub const COLOR_PALETTE: [theme::Rgba; 16] = [
	[0xFF, 0xFF, 0xFF, 0xFF],
	[0xC6, 0xCC, 0xD2, 0xFF],
	[0x8B, 0x91, 0x99, 0xFF],
	[0x5A, 0x60, 0x67, 0xFF],
	[0x2A, 0x2D, 0x2F, 0xFF],
	[0x17, 0x19, 0x1B, 0xFF],
	[0x0F, 0x11, 0x12, 0xFF],
	[0xE2, 0x23, 0x1A, 0xFF],
	[0xF2, 0x8A, 0x2E, 0xFF],
	[0xFA, 0xBD, 0x2F, 0xFF],
	[0x00, 0xB0, 0x6F, 0xFF],
	[0x00, 0xA2, 0xFF, 0xFF],
	[0x33, 0x5F, 0xFF, 0xFF],
	[0x8B, 0x5C, 0xF6, 0xFF],
	[0xE2, 0x56, 0x4E, 0xFF],
	[0x8C, 0x5A, 0x3C, 0xFF],
];
/// Next/prev palette entry for a color (unknown colors start at index 0).
pub fn cycle_color(cur: theme::Rgba, dir: i32) -> theme::Rgba {
	let idx = COLOR_PALETTE.iter().position(|c| *c == cur);
	let n = COLOR_PALETTE.len() as i32;
	match idx {
		Some(i) => {
			let next = (i as i32 + dir).rem_euclid(n);
			COLOR_PALETTE[next as usize]
		}
		None => {
			if dir < 0 {
				COLOR_PALETTE[n as usize - 1]
			} else {
				COLOR_PALETTE[0]
			}
		}
	}
}

// ---------------------------------------------------------------------------
// Scrollbar with arrows
// ---------------------------------------------------------------------------
