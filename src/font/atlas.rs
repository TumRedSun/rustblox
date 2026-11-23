//! Shelf-packing glyph atlas: one alpha-coverage mask per (face, size, char),
//! placed into a square page that doubles until [`AtlasState::MAX_PAGE`].

use crate::draw::GlyphSlot;
use std::collections::HashMap;

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
	pub(super) slots: HashMap<GlyphKey, GlyphSlot>,
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
			slots: HashMap::new(),
			pending: Vec::new(),
			all: Vec::new(),
			cursor_x: 1,
			cursor_y: 1,
			row_h: 0,
			page_size: 1024,
		}
	}

	/// Get (or create) the atlas slot for a glyph key. Returns `None` when
	/// the glyph does not fit anymore: the atlas is at [`Self::MAX_PAGE`] and
	/// packed. Dropping the glyph is strictly better than the previous
	/// behavior — re-growing a full max-size page re-placed the entire atlas
	/// on EVERY subsequent miss, which looped forever (client freeze) and
	/// piled up copies of every mask.
	pub(super) fn place(&mut self, key: GlyphKey, mut mask: GlyphMask) -> Option<GlyphSlot> {
		if let Some(slot) = self.slots.get(&key) {
			return Some(*slot);
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
			if self.page_size >= Self::MAX_PAGE {
				// Cap reached and still no room: skip this glyph. Everything
				// already placed stays valid.
				return None;
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
		Some(slot)
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
