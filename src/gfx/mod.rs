//! wgpu graphics context: surface, pipelines, font atlas, image cache and
//! the per-frame render loop (optional 3D scene + 2D UI on top).

pub mod math;
pub mod scene;

mod core;
mod render;
mod scene3d;
mod ui;

use crate::gfx::core::texture::ImageEntry;
use std::collections::HashMap;

/// GPU context owned by the app: one surface, one UI pipeline and the
/// optional 3D scene pipeline, sharing a single device/queue.
pub struct Gfx {
	pub device: wgpu::Device,
	pub queue: wgpu::Queue,
	surface: wgpu::Surface<'static>,
	#[allow(dead_code)]
	surface_format: wgpu::TextureFormat,
	config: wgpu::SurfaceConfiguration,

	depth: wgpu::TextureView,
	sampler: wgpu::Sampler,

	// UI pipeline + font atlas.
	ui: ui::UiResources,

	// Image cache (avatars, icons, place thumbnails).
	images: HashMap<u64, ImageEntry>,
	image_fifo: Vec<u64>,
	frame_no: u64,

	// 3D scene pipeline.
	scene: scene3d::SceneResources,

	scratch_ui: Vec<ui::UiInstance>,
	scratch_scene: Vec<scene3d::SceneInstance>,
}

/// Allocate a vertex buffer for `n` instances of `stride` bytes.
pub(crate) fn make_buf(
	device: &wgpu::Device,
	n: usize,
	stride: usize,
	label: &str,
) -> wgpu::Buffer {
	device.create_buffer(&wgpu::BufferDescriptor {
		label: Some(label),
		size: (n * stride) as u64,
		usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
		mapped_at_creation: false,
	})
}

/// Depth state for overlay pipelines in the main pass (UI, sky): the main
/// pass always owns a depth attachment, but these ignore depth — test
/// always, write never.
pub(crate) fn overlay_depth() -> wgpu::DepthStencilState {
	wgpu::DepthStencilState {
		format: wgpu::TextureFormat::Depth32Float,
		depth_write_enabled: Some(false),
		depth_compare: Some(wgpu::CompareFunction::Always),
		stencil: Default::default(),
		bias: Default::default(),
	}
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
