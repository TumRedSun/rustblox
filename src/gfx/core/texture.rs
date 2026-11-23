//! Texture uploads (row-padded copies) and the UI image cache.

use crate::gfx::Gfx;

/// Upload a tightly-packed R8/RGBA8 image to a texture, padding every row
/// to wgpu's 256-byte copy-row alignment. `data` holds `width * height *
/// bytes_per_px` bytes, row-major, with no padding of its own.
pub(crate) fn write_texture_padded(
	queue: &wgpu::Queue,
	texture: &wgpu::Texture,
	origin: wgpu::Origin3d,
	data: &[u8],
	width: u32,
	height: u32,
	bytes_per_px: u32,
) {
	let src_bpr = (width * bytes_per_px) as usize;
	let dst_bpr = src_bpr.div_ceil(256) * 256; // COPY_BYTES_PER_ROW_ALIGNMENT
	let info = wgpu::TexelCopyTextureInfo {
		texture,
		mip_level: 0,
		origin,
		aspect: wgpu::TextureAspect::All,
	};
	let extent = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
	if data.len() < src_bpr * height as usize {
		return; // not enough pixels; refuse to upload garbage
	}
	if src_bpr == dst_bpr {
		queue.write_texture(
			info,
			data,
			wgpu::TexelCopyBufferLayout {
				offset: 0,
				bytes_per_row: Some(src_bpr as u32),
				rows_per_image: Some(height),
			},
			extent,
		);
		return;
	}
	let mut padded = vec![0u8; dst_bpr * height as usize];
	for row in 0..height as usize {
		let s = row * src_bpr;
		padded[row * dst_bpr..row * dst_bpr + src_bpr].copy_from_slice(&data[s..s + src_bpr]);
	}
	queue.write_texture(
		info,
		&padded,
		wgpu::TexelCopyBufferLayout {
			offset: 0,
			bytes_per_row: Some(dst_bpr as u32),
			rows_per_image: Some(height),
		},
		extent,
	);
}

/// One cached UI image: its texture, view and bind group, plus the frame
/// number it was last referenced in.
pub(crate) struct ImageEntry {
	#[allow(dead_code)]
	texture: wgpu::Texture,
	#[allow(dead_code)]
	view: wgpu::TextureView,
	pub(crate) group: wgpu::BindGroup,
	#[allow(dead_code)]
	last_used: u64,
}

impl Gfx {
	/// Return the bind group for `hash`, uploading and caching a new texture
	/// on first use. `None` when the pixel buffer is too small for `sw * sh`.
	pub(crate) fn image_group(
		&mut self,
		hash: u64,
		data: &[u8],
		sw: i32,
		sh: i32,
	) -> Option<&wgpu::BindGroup> {
		let sw = sw.max(1) as u32;
		let sh = sh.max(1) as u32;
		if (data.len() as u64) < (sw as u64) * (sh as u64) * 4 {
			return None;
		}
		if self.images.contains_key(&hash) {
			if let Some(e) = self.images.get_mut(&hash) {
				e.last_used = self.frame_no;
			}
			return self.images.get(&hash).map(|e| &e.group);
		}
		let texture = self.device.create_texture(&wgpu::TextureDescriptor {
			label: Some("ui-image"),
			size: wgpu::Extent3d { width: sw, height: sh, depth_or_array_layers: 1 },
			mip_level_count: 1,
			sample_count: 1,
			dimension: wgpu::TextureDimension::D2,
			format: wgpu::TextureFormat::Rgba8Unorm,
			usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
			view_formats: &[],
		});
		let view = texture.create_view(&Default::default());
		write_texture_padded(&self.queue, &texture, wgpu::Origin3d::ZERO, data, sw, sh, 4);
		let layout = self.ui.pipeline.get_bind_group_layout(0);
		let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("ui-image-bg"),
			layout: &layout,
			entries: &[
				wgpu::BindGroupEntry { binding: 0, resource: self.ui.uniform.as_entire_binding() },
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
		self.images.insert(hash, ImageEntry { texture, view, group, last_used: self.frame_no });
		self.image_fifo.push(hash);
		// Evict oldest when the cache grows too big.
		while self.image_fifo.len() > 512 {
			let victim = self.image_fifo.remove(0);
			self.images.remove(&victim);
		}
		self.images.get(&hash).map(|e| &e.group)
	}
}
