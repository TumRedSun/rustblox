//! Scene render pipelines (lit, shadow-only and sky) and their bind groups.

use super::{FrameUniforms, SceneInstance, MAX_SCENE_INSTANCES};

/// GPU resources owned by the 3D scene pass.
pub(crate) struct SceneResources {
	pub(crate) pipeline: wgpu::RenderPipeline,
	pub(crate) shadow_pipeline: wgpu::RenderPipeline,
	pub(crate) sky_pipeline: wgpu::RenderPipeline,
	pub(crate) frame_uniform: wgpu::Buffer,
	pub(crate) instances: wgpu::Buffer,
	pub(crate) capacity: usize,
	pub(crate) group0: wgpu::BindGroup,
	pub(crate) shadow_tex: wgpu::TextureView,
	pub(crate) group1: wgpu::BindGroup,
}

impl SceneResources {
	/// Build the scene/shadow/sky pipelines and the bind groups that share
	/// the frame uniform and the shadow map.
	pub(crate) fn create(
		device: &wgpu::Device,
		shadow_tex: wgpu::TextureView,
		surface_format: wgpu::TextureFormat,
	) -> SceneResources {
		let scene_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
			label: Some("scene.wgsl"),
			source: wgpu::ShaderSource::Wgsl(include_str!("../scene.wgsl").into()),
		});
		let frame_uniform = device.create_buffer(&wgpu::BufferDescriptor {
			label: Some("frame-uniform"),
			size: std::mem::size_of::<FrameUniforms>() as u64,
			usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
			mapped_at_creation: false,
		});
		let scene_bgl0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("scene-bgl0"),
			entries: &[wgpu::BindGroupLayoutEntry {
				binding: 0,
				visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
				ty: wgpu::BindingType::Buffer {
					ty: wgpu::BufferBindingType::Uniform,
					has_dynamic_offset: false,
					min_binding_size: None,
				},
				count: None,
			}],
		});
		let scene_bgl1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("scene-bgl1"),
			entries: &[
				wgpu::BindGroupLayoutEntry {
					binding: 0,
					visibility: wgpu::ShaderStages::FRAGMENT,
					ty: wgpu::BindingType::Texture {
						sample_type: wgpu::TextureSampleType::Depth,
						view_dimension: wgpu::TextureViewDimension::D2,
						multisampled: false,
					},
					count: None,
				},
				wgpu::BindGroupLayoutEntry {
					binding: 1,
					visibility: wgpu::ShaderStages::FRAGMENT,
					ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
					count: None,
				},
			],
		});
		let group0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("scene-bg0"),
			layout: &scene_bgl0,
			entries: &[wgpu::BindGroupEntry {
				binding: 0,
				resource: frame_uniform.as_entire_binding(),
			}],
		});
		let cmp_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
			label: Some("shadow-sampler"),
			compare: Some(wgpu::CompareFunction::LessEqual),
			..Default::default()
		});
		let group1 = device.create_bind_group(&wgpu::BindGroupDescriptor {
			label: Some("scene-bg1"),
			layout: &scene_bgl1,
			entries: &[
				wgpu::BindGroupEntry {
					binding: 0,
					resource: wgpu::BindingResource::TextureView(&shadow_tex),
				},
				wgpu::BindGroupEntry {
					binding: 1,
					resource: wgpu::BindingResource::Sampler(&cmp_sampler),
				},
			],
		});
		let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
			label: Some("scene-pl"),
			bind_group_layouts: &[Some(&scene_bgl0), Some(&scene_bgl1)],
			immediate_size: 0,
		});
		// The shadow and sky entry points never touch the shadow map
		// (group 1) — the shadow pass WRITES the map it would read, which
		// would even be a usage conflict. Their pipelines therefore carry a
		// group-0-only layout: with the shared two-group layout every
		// shadow/sky draw was missing bind group 1 — a lazy validation
		// error surfacing only at encoder finish(), killing the client on
		// the first scene frame (the reported join crash).
		let light_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
			label: Some("scene-light-pl"),
			bind_group_layouts: &[Some(&scene_bgl0)],
			immediate_size: 0,
		});
		let vertex_layout = wgpu::VertexBufferLayout {
			array_stride: std::mem::size_of::<SceneInstance>() as u64,
			step_mode: wgpu::VertexStepMode::Instance,
			attributes: &[
				wgpu::VertexAttribute {
					format: wgpu::VertexFormat::Float32x3,
					offset: 0,
					shader_location: 0,
				},
				wgpu::VertexAttribute {
					format: wgpu::VertexFormat::Float32x3,
					offset: 12,
					shader_location: 1,
				},
				wgpu::VertexAttribute {
					format: wgpu::VertexFormat::Float32x3,
					offset: 24,
					shader_location: 2,
				},
				wgpu::VertexAttribute {
					format: wgpu::VertexFormat::Float32x4,
					offset: 48,
					shader_location: 4,
				},
				wgpu::VertexAttribute {
					format: wgpu::VertexFormat::Float32x4,
					offset: 64,
					shader_location: 5,
				},
				wgpu::VertexAttribute {
					format: wgpu::VertexFormat::Float32x4,
					offset: 80,
					shader_location: 6,
				},
				wgpu::VertexAttribute {
					format: wgpu::VertexFormat::Float32x4,
					offset: 96,
					shader_location: 7,
				},
			],
		};
		let depth_stencil = wgpu::DepthStencilState {
			format: wgpu::TextureFormat::Depth32Float,
			depth_write_enabled: Some(true),
			depth_compare: Some(wgpu::CompareFunction::LessEqual),
			stencil: Default::default(),
			bias: Default::default(),
		};
		let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
			label: Some("scene-pipeline"),
			layout: Some(&pipeline_layout),
			vertex: wgpu::VertexState {
				module: &scene_module,
				entry_point: Some("vs_scene"),
				compilation_options: Default::default(),
				buffers: &[vertex_layout.clone()],
			},
			fragment: Some(wgpu::FragmentState {
				module: &scene_module,
				entry_point: Some("fs_scene"),
				compilation_options: Default::default(),
				targets: &[Some(wgpu::ColorTargetState {
					format: surface_format,
					blend: Some(wgpu::BlendState::ALPHA_BLENDING),
					write_mask: wgpu::ColorWrites::ALL,
				})],
			}),
			primitive: wgpu::PrimitiveState {
				topology: wgpu::PrimitiveTopology::TriangleList,
				cull_mode: None,
				..Default::default()
			},
			depth_stencil: Some(depth_stencil.clone()),
			multisample: Default::default(),
			multiview_mask: None,
			cache: None,
		});
		let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
			label: Some("shadow-pipeline"),
			layout: Some(&light_layout),
			vertex: wgpu::VertexState {
				module: &scene_module,
				entry_point: Some("vs_shadow"),
				compilation_options: Default::default(),
				buffers: &[vertex_layout.clone()],
			},
			fragment: Some(wgpu::FragmentState {
				module: &scene_module,
				entry_point: Some("fs_shadow"),
				compilation_options: Default::default(),
				targets: &[],
			}),
			primitive: wgpu::PrimitiveState {
				topology: wgpu::PrimitiveTopology::TriangleList,
				cull_mode: None,
				..Default::default()
			},
			depth_stencil: Some(wgpu::DepthStencilState {
				format: wgpu::TextureFormat::Depth32Float,
				depth_write_enabled: Some(true),
				depth_compare: Some(wgpu::CompareFunction::LessEqual),
				stencil: Default::default(),
				bias: wgpu::DepthBiasState { constant: 2, slope_scale: 2.0, clamp: 0.0 },
			}),
			multisample: Default::default(),
			multiview_mask: None,
			cache: None,
		});
		let sky_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
			label: Some("sky-pipeline"),
			layout: Some(&light_layout),
			vertex: wgpu::VertexState {
				module: &scene_module,
				entry_point: Some("vs_sky"),
				compilation_options: Default::default(),
				buffers: &[],
			},
			fragment: Some(wgpu::FragmentState {
				module: &scene_module,
				entry_point: Some("fs_sky"),
				compilation_options: Default::default(),
				targets: &[Some(wgpu::ColorTargetState {
					format: surface_format,
					blend: None,
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
			MAX_SCENE_INSTANCES,
			std::mem::size_of::<SceneInstance>(),
			"scene-instances",
		);
		SceneResources {
			pipeline,
			shadow_pipeline,
			sky_pipeline,
			frame_uniform,
			instances,
			capacity: MAX_SCENE_INSTANCES,
			group0,
			shadow_tex,
			group1,
		}
	}
}
