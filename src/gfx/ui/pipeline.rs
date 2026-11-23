//! UI render pipeline, bind groups and the font-atlas texture.

use super::{UiInstance, MAX_UI_INSTANCES};

/// GPU resources owned by the 2D UI pass.
pub(crate) struct UiResources {
	pub(crate) pipeline: wgpu::RenderPipeline,
	pub(crate) uniform: wgpu::Buffer,
	pub(crate) instances: wgpu::Buffer,
	pub(crate) capacity: usize,
	pub(crate) bind_atlas: wgpu::BindGroup,
	pub(crate) atlas_texture: wgpu::Texture,
	pub(crate) atlas_page_size: u32,
}

impl UiResources {
	/// Build the UI pipeline, its atlas bind group and the initial instance
	/// buffer.
	pub(crate) fn create(
		device: &wgpu::Device,
		sampler: &wgpu::Sampler,
		surface_format: wgpu::TextureFormat,
	) -> UiResources {
		let ui_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
			label: Some("ui.wgsl"),
			source: wgpu::ShaderSource::Wgsl(include_str!("../ui.wgsl").into()),
		});
		let atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
			label: Some("font-atlas"),
			size: wgpu::Extent3d { width: 1024, height: 1024, depth_or_array_layers: 1 },
			mip_level_count: 1,
			sample_count: 1,
			dimension: wgpu::TextureDimension::D2,
			format: wgpu::TextureFormat::R8Unorm,
			usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
			view_formats: &[],
		});
		let atlas_view = atlas_texture.create_view(&Default::default());
		let ui_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("ui-bgl"),
			entries: &[
				wgpu::BindGroupLayoutEntry {
					binding: 0,
					visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
					ty: wgpu::BindingType::Buffer {
						ty: wgpu::BufferBindingType::Uniform,
						has_dynamic_offset: false,
						min_binding_size: None,
					},
					count: None,
				},
				wgpu::BindGroupLayoutEntry {
					binding: 1,
					visibility: wgpu::ShaderStages::FRAGMENT,
					ty: wgpu::BindingType::Texture {
						sample_type: wgpu::TextureSampleType::Float { filterable: true },
						view_dimension: wgpu::TextureViewDimension::D2,
						multisampled: false,
					},
					count: None,
				},
				wgpu::BindGroupLayoutEntry {
					binding: 2,
					visibility: wgpu::ShaderStages::FRAGMENT,
					ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
					count: None,
				},
			],
		});
		let uniform = device.create_buffer(&wgpu::BufferDescriptor {
			label: Some("ui-uniform"),
			size: std::mem::size_of::<super::UiUniforms>() as u64,
			usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
			mapped_at_creation: false,
		});
		let bind_atlas = device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("ui-bg-atlas"),
			layout: &ui_bgl,
			entries: &[
				wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
				wgpu::BindGroupEntry {
					binding: 1,
					resource: wgpu::BindingResource::TextureView(&atlas_view),
				},
				wgpu::BindGroupEntry {
					binding: 2,
					resource: wgpu::BindingResource::Sampler(sampler),
				},
			],
		});
		let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
			label: Some("ui-pl"),
			bind_group_layouts: &[Some(&ui_bgl)],
			immediate_size: 0,
		});
		let vertex_layout = wgpu::VertexBufferLayout {
			array_stride: std::mem::size_of::<UiInstance>() as u64,
			step_mode: wgpu::VertexStepMode::Instance,
			attributes: &[
				wgpu::VertexAttribute {
					format: wgpu::VertexFormat::Float32x4,
					offset: 0,
					shader_location: 0,
				},
				wgpu::VertexAttribute {
					format: wgpu::VertexFormat::Float32x4,
					offset: 16,
					shader_location: 1,
				},
				wgpu::VertexAttribute {
					format: wgpu::VertexFormat::Float32x4,
					offset: 32,
					shader_location: 2,
				},
				wgpu::VertexAttribute {
					format: wgpu::VertexFormat::Float32x4,
					offset: 48,
					shader_location: 3,
				},
				wgpu::VertexAttribute {
					format: wgpu::VertexFormat::Uint32x4,
					offset: 64,
					shader_location: 4,
				},
			],
		};
		let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
			label: Some("ui-pipeline"),
			layout: Some(&pipeline_layout),
			vertex: wgpu::VertexState {
				module: &ui_module,
				entry_point: Some("vs_main"),
				compilation_options: Default::default(),
				buffers: &[vertex_layout],
			},
			fragment: Some(wgpu::FragmentState {
				module: &ui_module,
				entry_point: Some("fs_main"),
				compilation_options: Default::default(),
				targets: &[Some(wgpu::ColorTargetState {
					format: surface_format,
					blend: Some(wgpu::BlendState {
						color: wgpu::BlendComponent {
							src_factor: wgpu::BlendFactor::One,
							dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
							operation: wgpu::BlendOperation::Add,
						},
						alpha: wgpu::BlendComponent {
							src_factor: wgpu::BlendFactor::One,
							dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
							operation: wgpu::BlendOperation::Add,
						},
					}),
					write_mask: wgpu::ColorWrites::ALL,
				})],
			}),
			primitive: wgpu::PrimitiveState {
				topology: wgpu::PrimitiveTopology::TriangleList,
				cull_mode: None,
				..Default::default()
			},
			depth_stencil: Some(crate::gfx::overlay_depth()),
			multisample: Default::default(),
			multiview_mask: None,
			cache: None,
		});
		let instances = crate::gfx::make_buf(
			device,
			MAX_UI_INSTANCES,
			std::mem::size_of::<UiInstance>(),
			"ui-instances",
		);
		UiResources {
			pipeline,
			uniform,
			instances,
			capacity: MAX_UI_INSTANCES,
			bind_atlas,
			atlas_texture,
			atlas_page_size: 1024,
		}
	}
}
