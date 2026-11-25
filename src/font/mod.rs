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
//!
//! Layout: the shelf-packing atlas in `atlas.rs`, measurement/wrapping/draw
//! in `text.rs`.

mod atlas;
mod text;

pub use atlas::{AtlasState, GlyphKey, GlyphMask};
pub use text::{
	draw, draw_ellipsis, draw_wrapped, draw_wrapped_centered, masked_str, measure, measure_prefix,
	wrap, wrap_text,
};

use ab_glyph::{Font, FontArc};
use std::sync::Mutex;

/// The two embedded faces plus the shared glyph atlas they rasterize into.
pub struct Fonts {
	pub regular: FontArc,
	pub bold: FontArc,
	pub bullet: bool, // whether U+2022 exists (password masking)
	pub atlas: Mutex<AtlasState>,
}

impl Fonts {
	/// Load the DejaVu faces embedded in the binary.
	///
	/// # Panics
	/// Only for a corrupt build: the bytes come from `include_bytes!` of the
	/// crate's own assets, never from user input.
	pub fn load() -> Fonts {
		#[allow(clippy::expect_used)] // deliberate exception, see `# Panics`
		let regular = FontArc::try_from_slice(include_bytes!("../../assets/fonts/DejaVuSans.ttf"))
			.expect("embedded regular font");
		#[allow(clippy::expect_used)] // deliberate exception, see `# Panics`
		let bold =
			FontArc::try_from_slice(include_bytes!("../../assets/fonts/DejaVuSans-Bold.ttf"))
				.expect("embedded bold font");
		let bullet = regular.glyph_id('\u{2022}').0 != 0 || regular.glyph_id('\u{25CF}').0 != 0;
		Fonts { regular, bold, bullet, atlas: Mutex::new(AtlasState::new()) }
	}

	/// Face used for a draw call.
	pub(super) fn face(&self, bold: bool) -> &FontArc {
		if bold {
			&self.bold
		} else {
			&self.regular
		}
	}
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
