//! 2D UI pipeline state: instance packing, the font-atlas upload and the
//! per-frame glyph/prim batching primitives.

use super::Gfx;
use crate::draw::{Clip, GlyphSlot, Prim};
use crate::theme::Rgba;
use bytemuck::{Pod, Zeroable};

mod pipeline;
pub(crate) use pipeline::UiResources;

/// One UI instance (must match the WGSL vertex layout).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct UiInstance {
	pos_size: [f32; 4],
	color: [f32; 4],
	params: [f32; 4],
	uv: [f32; 4],
	misc: [u32; 4],
}

/// Per-frame UI uniform (resolution + color-space flag).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct UiUniforms {
	pub(crate) resolution: [f32; 2],
	/// Non-zero when the target is an sRGB surface.
	pub(crate) srgb: f32,
	pub(crate) _pad: f32,
}

/// Instance-buffer growth starts here and doubles as needed.
const MAX_UI_INSTANCES: usize = 1 << 17;

// Kinds mirrored from the WGSL ui.wgsl switch.
const K_RECT: u32 = 0;
const K_ROUND: u32 = 1;
const K_ROUND_ROT: u32 = 2;
const K_ROUND_BORDER: u32 = 3;
const K_CIRCLE: u32 = 4;
const K_RING: u32 = 5;
const K_GLYPH: u32 = 6;
const K_IMAGE: u32 = 7;

fn rgba_to_f32(c: Rgba) -> [f32; 4] {
	[c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0]
}

impl Gfx {
	/// Re-upload the font atlas when it grows and every glyph mask rasterized
	/// since the last frame.
	pub(super) fn sync_atlas(&mut self, atlas: &crate::font::AtlasState) {
		// Regrow the texture if the atlas grew past our page size.
		if atlas.page_size != self.ui.atlas_page_size {
			self.ui.atlas_page_size = atlas.page_size;
			self.ui.atlas_texture = self.device.create_texture(&wgpu::TextureDescriptor {
				label: Some("font-atlas"),
				size: wgpu::Extent3d {
					width: atlas.page_size,
					height: atlas.page_size,
					depth_or_array_layers: 1,
				},
				mip_level_count: 1,
				sample_count: 1,
				dimension: wgpu::TextureDimension::D2,
				format: wgpu::TextureFormat::R8Unorm,
				usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
				view_formats: &[],
			});
			let view = self.ui.atlas_texture.create_view(&Default::default());
			let layout = self.ui.pipeline.get_bind_group_layout(0);
			self.ui.bind_atlas = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
				label: Some("ui-bg-atlas"),
				layout: &layout,
				entries: &[
					wgpu::BindGroupEntry {
						binding: 0,
						resource: self.ui.uniform.as_entire_binding(),
					},
					wgpu::BindGroupEntry {
						binding: 1,
						resource: wgpu::BindingResource::TextureView(&view),
					},
					wgpu::BindGroupEntry {
						binding: 2,
						resource: wgpu::BindingResource::Sampler(&self.sampler),
					},
				],
			});
			// Re-upload every mask ever placed.
			for m in &atlas.all {
				self.upload_mask(m);
			}
		}
		for m in &atlas.pending {
			self.upload_mask(m);
		}
	}

	fn upload_mask(&mut self, m: &crate::font::GlyphMask) {
		let w = m.w.max(1) as u32;
		let h = m.h.max(1) as u32;
		let page = self.ui.atlas_page_size;
		if m.x + m.w > page as u16 || m.y + m.h > page as u16 {
			return;
		}
		crate::gfx::core::texture::write_texture_padded(
			&self.queue,
			&self.ui.atlas_texture,
			wgpu::Origin3d { x: m.x as u32, y: m.y as u32, z: 0 },
			&m.data,
			w,
			h,
			1,
		);
	}

	/// Convert the draw list into GPU instances and grow the vertex buffer
	/// when the frame needs more room.
	pub(super) fn pack_ui(&mut self, prims: &[(Prim, Clip)]) {
		self.scratch_ui.clear();
		for (prim, _clip) in prims.iter() {
			let inst = match prim {
				Prim::Rect { x, y, w, h, color } => UiInstance {
					pos_size: [*x as f32, *y as f32, *w as f32, *h as f32],
					color: rgba_to_f32(*color),
					params: [0.0; 4],
					uv: [0.0; 4],
					misc: [K_RECT, 0, 0, 0],
				},
				Prim::RoundRect { x, y, w, h, radius, color } => UiInstance {
					pos_size: [*x as f32, *y as f32, *w as f32, *h as f32],
					color: rgba_to_f32(*color),
					params: [*radius, 0.0, 0.0, 0.0],
					uv: [0.0; 4],
					misc: [K_ROUND, 0, 0, 0],
				},
				Prim::RoundRectRot { cx, cy, w, h, radius, angle, color } => UiInstance {
					pos_size: [*cx as f32, *cy as f32, *w as f32, *h as f32],
					color: rgba_to_f32(*color),
					params: [*radius, *angle, 0.0, 0.0],
					uv: [0.0; 4],
					misc: [K_ROUND_ROT, 0, 0, 0],
				},
				Prim::RoundRectBorder { cx, cy, w, h, radius, angle, border, color } => {
					UiInstance {
						pos_size: [*cx as f32, *cy as f32, *w as f32, *h as f32],
						color: rgba_to_f32(*color),
						params: [*radius, *angle, *border, 0.0],
						uv: [0.0; 4],
						misc: [K_ROUND_BORDER, 0, 0, 0],
					}
				}
				Prim::Circle { cx, cy, r, color } => UiInstance {
					pos_size: [
						(*cx - *r) as f32,
						(*cy - *r) as f32,
						(*r * 2 + 1) as f32,
						(*r * 2 + 1) as f32,
					],
					color: rgba_to_f32(*color),
					params: [*r as f32, 0.0, 0.0, 0.0],
					uv: [0.0; 4],
					misc: [K_CIRCLE, 0, 0, 0],
				},
				Prim::Ring { cx, cy, ro, ri, color } => UiInstance {
					pos_size: [
						(*cx - *ro) as f32,
						(*cy - *ro) as f32,
						(*ro * 2 + 1) as f32,
						(*ro * 2 + 1) as f32,
					],
					color: rgba_to_f32(*color),
					params: [*ro as f32, 0.0, *ri as f32, 0.0],
					uv: [0.0; 4],
					misc: [K_RING, 0, 0, 0],
				},
				Prim::Glyph { x, y, slot, color } => {
					let ps = glyph_uv(slot, self.ui.atlas_page_size);
					UiInstance {
						pos_size: [*x, *y, slot.w as f32, slot.h as f32],
						color: rgba_to_f32(*color),
						params: [0.0; 4],
						uv: ps,
						misc: [K_GLYPH, 0, 0, 0],
					}
				}
				Prim::Image { x, y, sw, sh, dw, dh, corner, hash, data } => {
					// Upload (or refresh) the texture, then reference it.
					self.image_group(*hash, data, *sw, *sh);
					UiInstance {
						pos_size: [*x as f32, *y as f32, *dw as f32, *dh as f32],
						color: [1.0; 4],
						params: [*corner as f32, 0.0, 0.0, 0.0],
						uv: [0.0, 0.0, 1.0, 1.0],
						misc: [K_IMAGE, (*hash & 0xffff_ffff) as u32, (hash >> 32) as u32, 0],
					}
				}
				Prim::Pix { x, y, color } => UiInstance {
					pos_size: [*x, *y, 1.0, 1.0],
					color: rgba_to_f32(*color),
					params: [0.0; 4],
					uv: [0.0; 4],
					misc: [K_RECT, 0, 0, 0],
				},
			};
			self.scratch_ui.push(inst);
		}
		self.ensure_ui_capacity();
	}

	fn ensure_ui_capacity(&mut self) {
		if self.scratch_ui.len() > self.ui.capacity {
			let mut n = self.ui.capacity;
			while n < self.scratch_ui.len() {
				n *= 2;
			}
			self.ui.instances =
				super::make_buf(&self.device, n, std::mem::size_of::<UiInstance>(), "ui-instances");
			self.ui.capacity = n;
		}
	}
}

fn glyph_uv(slot: &GlyphSlot, page: u32) -> [f32; 4] {
	let p = page as f32;
	[slot.x as f32 / p, slot.y as f32 / p, slot.w as f32 / p, slot.h as f32 / p]
}
