//! GPU bootstrap: adapter/device request, surface configuration, depth and
//! shadow targets, plus resize handling.

pub(crate) mod texture;

use super::{scene3d, ui, Gfx};
use std::sync::Arc;
use winit::window::Window;

/// Shadow map side length (square, `Depth32Float`).
const SHADOW_SIZE: u32 = 2048;

impl Gfx {
	/// Request a GPU adapter, configure the window surface and build every
	/// pipeline. Panics only when the platform provides no usable adapter.
	pub fn new(window: &Arc<Window>) -> Gfx {
		let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
		let surface = instance.create_surface(Arc::clone(window)).expect("create wgpu surface");
		let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
			power_preference: wgpu::PowerPreference::HighPerformance,
			compatible_surface: Some(&surface),
			force_fallback_adapter: false,
		}))
		.expect("no suitable GPU adapter");

		// Request what the adapter advertises, minus the EXPERIMENTAL_* bits:
		// newer wgpu refuses request_device outright if any experimental
		// feature is requested without a separate opt-in, so passing the raw
		// adapter.features() here makes the device request fail on drivers
		// that advertise ray query / mesh shader support (observed:
		// RequestDeviceError::ExperimentalFeaturesNotEnabled). The GL
		// fallback's TEXTURE_FORMAT_16BIT_NORM need (see below) is not an
		// experimental bit, so it survives the mask.
		let experimental = wgpu::Features::EXPERIMENTAL_RAY_QUERY
			| wgpu::Features::EXPERIMENTAL_MESH_SHADER
			| wgpu::Features::EXPERIMENTAL_RAY_HIT_VERTEX_RETURN
			| wgpu::Features::EXPERIMENTAL_MESH_SHADER_MULTIVIEW
			| wgpu::Features::EXPERIMENTAL_MESH_SHADER_POINTS
			| wgpu::Features::EXPERIMENTAL_COOPERATIVE_MATRIX;
		let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
			label: Some("rustblox-device"),
			required_features: adapter.features() - experimental,
			..Default::default()
		}))
		.expect("request wgpu device");
		// Log the GPU that won the adapter request: driver/renderer quirks
		// are the usual root cause of platform-specific render bugs, and
		// this line pins them into the log the user already knows how to
		// export.
		let info = adapter.get_info();
		crate::logs::push(
            "debug",
            &format!(
                "gpu: {:?} — {} ({:?}, vendor {:#06x}, device {:#06x}, driver {}/{}), surface {}x{}",
                info.backend,
                info.name,
                info.device_type,
                info.vendor,
                info.device,
                info.driver,
                info.driver_info,
                window.inner_size().width,
                window.inner_size().height,
            ),
        );
		// Replace wgpu's default uncaptured-error handler (which PANICS —
		// the reported "client just crashes when joining") with a logging
		// one: the full validation message lands in the persistent log
		// file unclamped, the offending frame is skipped and the client
		// keeps running. The full text is what pins down the actual
		// invalid resource/pipeline on device-specific drivers. Repeats
		// are collapsed (a bad draw fires once per frame — 60 identical
		// KB-sized lines per second would drown the log and rotate it
		// out within seconds).
		device.on_uncaptured_error(Arc::new(|err| {
			use std::sync::atomic::{AtomicU64, Ordering};
			static LAST_KEY: AtomicU64 = AtomicU64::new(0);
			static ERR_NO: AtomicU64 = AtomicU64::new(0);
			// FNV-1a over the first 512 chars identifies the error kind.
			let text = format!("{err}");
			let mut hash: u64 = 0xcbf29ce484222325;
			for b in text.as_bytes().iter().take(512) {
				hash ^= *b as u64;
				hash = hash.wrapping_mul(0x100000001b3);
			}
			let n = ERR_NO.fetch_add(1, Ordering::Relaxed) + 1;
			if LAST_KEY.swap(hash, Ordering::Relaxed) == hash && n > 1 {
				return; // same error as before — already logged
			}
			let tag = if n > 1 { format!(" (wgpu error #{n})") } else { String::new() };
			crate::logs::push_unclamped("gfx", &format!("wgpu error: {text}{tag}"));
		}));

		let size = window.inner_size();
		let caps = surface.get_capabilities(&adapter);
		// Prefer a non-sRGB format: UI colors are authored in gamma space
		// (matching the software rasterizer), and the shaders do their own
		// gamma handling for the lit scene.
		let surface_format =
			caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
		let config = wgpu::SurfaceConfiguration {
			usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
			format: surface_format,
			width: size.width.max(1),
			height: size.height.max(1),
			present_mode: wgpu::PresentMode::Fifo,
			alpha_mode: caps.alpha_modes[0],
			view_formats: vec![],
			desired_maximum_frame_latency: 2,
		};
		surface.configure(&device, &config);

		let depth = Self::make_depth(&device, config.width, config.height);
		let shadow_tex = Self::make_shadow(&device);
		let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
			label: Some("ui-sampler"),
			mag_filter: wgpu::FilterMode::Linear,
			min_filter: wgpu::FilterMode::Linear,
			..Default::default()
		});

		let ui = ui::UiResources::create(&device, &sampler, surface_format);
		let scene = scene3d::SceneResources::create(&device, shadow_tex, surface_format);

		Gfx {
			device,
			queue,
			surface,
			surface_format,
			config,
			depth,
			sampler,
			ui,
			images: std::collections::HashMap::new(),
			image_fifo: Vec::new(),
			frame_no: 0,
			scene,
			scratch_ui: Vec::with_capacity(8192),
			scratch_scene: Vec::with_capacity(4096),
		}
	}

	fn make_depth(device: &wgpu::Device, w: u32, h: u32) -> wgpu::TextureView {
		let tex = device.create_texture(&wgpu::TextureDescriptor {
			label: Some("depth"),
			size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
			mip_level_count: 1,
			sample_count: 1,
			dimension: wgpu::TextureDimension::D2,
			format: wgpu::TextureFormat::Depth32Float,
			usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
			view_formats: &[],
		});
		tex.create_view(&Default::default())
	}

	fn make_shadow(device: &wgpu::Device) -> wgpu::TextureView {
		let tex = device.create_texture(&wgpu::TextureDescriptor {
			label: Some("shadow-map"),
			size: wgpu::Extent3d {
				width: SHADOW_SIZE,
				height: SHADOW_SIZE,
				depth_or_array_layers: 1,
			},
			mip_level_count: 1,
			sample_count: 1,
			dimension: wgpu::TextureDimension::D2,
			format: wgpu::TextureFormat::Depth32Float,
			usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
			view_formats: &[],
		});
		tex.create_view(&Default::default())
	}

	/// Reconfigure the surface for a new window size and recreate depth.
	pub fn resize(&mut self, width: u32, height: u32) {
		self.config.width = width.max(1);
		self.config.height = height.max(1);
		self.surface.configure(&self.device, &self.config);
		self.depth = Self::make_depth(&self.device, self.config.width, self.config.height);
	}
}
