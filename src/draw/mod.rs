//! UI drawing primitives with two interchangeable backends.
//!
//! * [`FB::new`] — the original software rasterizer writing into an RGBA8
//!   buffer (used by the headless `render_preview` example and unit tests).
//! * [`FB::gpu`] — records the same primitives into a command list that
//!   [`crate::gfx`] renders on the GPU (the live path).
//!
//! Keeping one API for both means every widget draws through the same code
//! no matter which backend is active.
//!
//! Layout by responsibility: [`Clip`]/[`Prim`]/[`FB`] state here, per-pixel
//! writes in `pixel.rs`, shape rasterization in `shapes.rs`, and image blits
//! plus the GPU image-cache hash in `image.rs`.

mod image;
mod pixel;
mod shapes;

pub use image::image_hash;

use crate::theme::Rgba;
use std::sync::Arc;

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
		Clip { x0: a.x0.max(b.x0), y0: a.y0.max(b.y0), x1: a.x1.min(b.x1), y1: a.y1.min(b.y1) }
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
	Rect {
		x: i32,
		y: i32,
		w: i32,
		h: i32,
		color: Rgba,
	},
	/// SDF rounded rectangle; `radius` is already scaled by the theme.
	RoundRect {
		x: i32,
		y: i32,
		w: i32,
		h: i32,
		radius: f32,
		color: Rgba,
	},
	/// SDF rounded rectangle rotated around its center (angle in radians).
	RoundRectRot {
		cx: i32,
		cy: i32,
		w: i32,
		h: i32,
		radius: f32,
		angle: f32,
		color: Rgba,
	},
	/// Stroked rounded rectangle (GPU-only, from outline_rounded).
	RoundRectBorder {
		cx: i32,
		cy: i32,
		w: i32,
		h: i32,
		radius: f32,
		angle: f32,
		border: f32,
		color: Rgba,
	},
	Circle {
		cx: i32,
		cy: i32,
		r: i32,
		color: Rgba,
	},
	Ring {
		cx: i32,
		cy: i32,
		ro: i32,
		ri: i32,
		color: Rgba,
	},
	/// Texture from the image cache (`hash` keys the GPU-side cache).
	Image {
		x: i32,
		y: i32,
		sw: i32,
		sh: i32,
		dw: i32,
		dh: i32,
		corner: i32,
		hash: u64,
		data: Arc<Vec<u8>>,
	},
	/// Glyph quad sampled from the font atlas (alpha in the R channel).
	Glyph {
		x: f32,
		y: f32,
		slot: GlyphSlot,
		color: Rgba,
	},
	/// Single pixel (from set/set_f; only a handful of call sites exist).
	Pix {
		x: f32,
		y: f32,
		color: Rgba,
	},
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
	pub(super) backing: Backing<'a>,
	pub(super) scratch: Vec<u8>,
}

impl<'a> FB<'a> {
	/// Software backend: draw straight into `buf` (w*h*4 bytes, RGBA8).
	pub fn new(buf: &'a mut [u8], w: i32, h: i32) -> Self {
		FB { w, h, clip: Clip::full(w, h), backing: Backing::Soft(buf), scratch: Vec::new() }
	}

	/// GPU backend: record primitives for the wgpu renderer.
	pub fn gpu(w: i32, h: i32) -> Self {
		FB {
			w,
			h,
			clip: Clip::full(w, h),
			backing: Backing::Gpu(Vec::with_capacity(4096)),
			scratch: Vec::new(),
		}
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
	pub(super) fn push(&mut self, p: Prim) {
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
}
