//! Headless reproduction of the game-scene frame path (diagnostics).
//!
//! Rebuilds the exact wgpu pipeline/pass structure of `Gfx` (scene.wgsl +
//! ui.wgsl, Depth32Float shadow map, the same vertex layouts and blend
//! states) against an OFFSCREEN rgba8unorm target — no window needed — so
//! the "join crashes with a wgpu Validation Error" report can be pinned
//! down on any machine (CI, sandbox) with a software Vulkan ICD:
//!
//!   VK_ICD_FILENAMES=/path/to/vk_swiftshader_icd.json \
//!     cargo run --example scene_repro
//!
//! Exits 0 and prints SCENE FRAME OK when the frame encodes/submits
//! cleanly; a validation error either panics (default handler) or is
//! printed in full.

use bytemuck::Pod;
use bytemuck::Zeroable;
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SceneInstance {
    rot: [f32; 9],    // 0..36
    _pad0: [f32; 3],  // 36..48
    pos: [f32; 4],    // 48..64
    size: [f32; 4],   // 64..80
    color: [f32; 4],  // 80..96
    params: [f32; 4], // 96..112
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct UiInstance {
    pos_size: [f32; 4],
    color: [f32; 4],
    params: [f32; 4],
    uv: [f32; 4],
    misc: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameUniforms {
    view_proj: [f32; 16],
    inv_view_proj: [f32; 16],
    light_view_proj: [f32; 16],
    cam_pos: [f32; 4],
    sun_dir: [f32; 4],
    sun_color: [f32; 4],
    ambient_sky: [f32; 4],
    ambient_ground: [f32; 4],
    fog_color: [f32; 4],
    sky_top: [f32; 4],
    sky_horizon: [f32; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct UiUniforms {
    resolution: [f32; 2],
    srgb: f32,
    _pad: f32,
}

fn main() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .expect("no adapter");
    let info = adapter.get_info();
    println!("adapter: {} / {:?} / {:?}", info.name, info.vendor, info.backend);

    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("repro-device"),
            ..Default::default()
        }))
        .expect("device");
    device.on_uncaptured_error(std::sync::Arc::new(|err| {
        println!("UNCAPTURED wgpu error: {err}");
        std::process::exit(2);
    }));

    let surface_format = wgpu::TextureFormat::Rgba8Unorm;
    let w = 1080u32;
    let h = 2400u32;

    // --- Targets (mirror Gfx::new / make_depth / make_shadow) ----------
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("repro-color"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: surface_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let color_view = color.create_view(&Default::default());
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("repro-depth"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth_view = depth.create_view(&Default::default());
    let shadow = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("repro-shadow-map"),
        size: wgpu::Extent3d { width: 2048, height: 2048, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let shadow_view = shadow.create_view(&Default::default());

    let scene_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("scene.wgsl"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../src/gfx/scene.wgsl").into()),
    });
    let ui_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ui.wgsl"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../src/gfx/ui.wgsl").into()),
    });

    // --- Scene bind groups (mirror scene_bgl0/1) ------------------------
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
    let frame_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("frame-uniform"),
        contents: bytemuck::bytes_of(&FrameUniforms {
            view_proj: ident(),
            inv_view_proj: ident(),
            light_view_proj: ident(),
            cam_pos: [0.0; 4],
            sun_dir: [0.3, -1.0, 0.2, 0.0],
            sun_color: [1.0; 4],
            ambient_sky: [0.4; 4],
            ambient_ground: [0.2; 4],
            fog_color: [0.7, 0.8, 0.9, 0.01],
            sky_top: [0.3, 0.5, 0.9, 1.0],
            sky_horizon: [0.8, 0.9, 1.0, 1.0],
            params: [0.0, 0.5, 0.0, 0.0],
        }),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let scene_group0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
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
    let scene_group1 = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("scene-bg1"),
        layout: &scene_bgl1,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&shadow_view) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&cmp_sampler) },
        ],
    });

    // --- Pipelines (mirror scene/shadow/sky/ui) --------------------------
    let scene_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("scene-pl"),
        bind_group_layouts: &[Some(&scene_bgl0), Some(&scene_bgl1)],
        immediate_size: 0,
    });
    let scene_light_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("scene-light-pl"),
        bind_group_layouts: &[Some(&scene_bgl0)],
        immediate_size: 0,
    });
    let scene_vertex_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<SceneInstance>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &[
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 12, shader_location: 1 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 24, shader_location: 2 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 48, shader_location: 4 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 64, shader_location: 5 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 80, shader_location: 6 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 96, shader_location: 7 },
        ],
    };
    let depth_stencil = wgpu::DepthStencilState {
        format: wgpu::TextureFormat::Depth32Float,
        depth_write_enabled: Some(true),
        depth_compare: Some(wgpu::CompareFunction::LessEqual),
        stencil: Default::default(),
        bias: Default::default(),
    };
    let overlay_depth = wgpu::DepthStencilState {
        format: wgpu::TextureFormat::Depth32Float,
        depth_write_enabled: Some(false),
        depth_compare: Some(wgpu::CompareFunction::Always),
        stencil: Default::default(),
        bias: Default::default(),
    };
    let target = |blend: Option<wgpu::BlendState>| wgpu::ColorTargetState {
        format: surface_format,
        blend,
        write_mask: wgpu::ColorWrites::ALL,
    };
    let scene_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("scene-pipeline"),
        layout: Some(&scene_pipeline_layout),
        vertex: wgpu::VertexState {
            module: &scene_module,
            entry_point: Some("vs_scene"),
            compilation_options: Default::default(),
            buffers: &[scene_vertex_layout.clone()],
        },
        fragment: Some(wgpu::FragmentState {
            module: &scene_module,
            entry_point: Some("fs_scene"),
            compilation_options: Default::default(),
            targets: &[Some(target(Some(wgpu::BlendState::ALPHA_BLENDING)))],
        }),
        primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
        depth_stencil: Some(depth_stencil.clone()),
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    });
    let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("shadow-pipeline"),
        layout: Some(&scene_light_layout),
        vertex: wgpu::VertexState {
            module: &scene_module,
            entry_point: Some("vs_shadow"),
            compilation_options: Default::default(),
            buffers: &[scene_vertex_layout.clone()],
        },
        fragment: Some(wgpu::FragmentState {
            module: &scene_module,
            entry_point: Some("fs_shadow"),
            compilation_options: Default::default(),
            targets: &[],
        }),
        primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
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
        layout: Some(&scene_light_layout),
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
            targets: &[Some(target(None))],
        }),
        primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
        depth_stencil: Some(overlay_depth.clone()),
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    });

    // --- UI path (atlas-less: ui_bgl with a 4×4 dummy texture) ----------
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
    let ui_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("ui-uniform"),
        contents: bytemuck::bytes_of(&UiUniforms { resolution: [w as f32, h as f32], srgb: 0.0, _pad: 0.0 }),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let dummy = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("repro-atlas"),
        size: wgpu::Extent3d { width: 4, height: 4, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let dummy_view = dummy.create_view(&Default::default());
    let lin_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("ui-sampler"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let ui_bind_atlas = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("ui-bg-atlas"),
        layout: &ui_bgl,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: ui_uniform.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&dummy_view) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&lin_sampler) },
        ],
    });
    let ui_vertex_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<UiInstance>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &[
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 0, shader_location: 0 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 16, shader_location: 1 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 32, shader_location: 2 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 48, shader_location: 3 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Uint32x4, offset: 64, shader_location: 4 },
        ],
    };
    let ui_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("ui-pipeline"),
        layout: Some(&device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ui-pl"),
            bind_group_layouts: &[Some(&ui_bgl)],
            immediate_size: 0,
        })),
        vertex: wgpu::VertexState {
            module: &ui_module,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[ui_vertex_layout],
        },
        fragment: Some(wgpu::FragmentState {
            module: &ui_module,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(target(Some(wgpu::BlendState {
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
            })))],
        }),
        primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
        depth_stencil: Some(overlay_depth.clone()),
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    });

    // --- Buffers ---------------------------------------------------------
    let scene_instances = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("scene-instances"),
        contents: bytemuck::cast_slice(&[SceneInstance {
            rot: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            _pad0: [0.0; 3],
            pos: [0.0, 0.0, 0.0, 0.0],
            size: [4.0, 1.0, 4.0, 0.0],
            color: [0.7, 0.7, 0.7, 1.0],
            params: [0.9, 0.0, 0.0, 0.0],
        }]),
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
    });
    let ui_instances = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("ui-instances"),
        contents: bytemuck::cast_slice(&[UiInstance {
            pos_size: [100.0, 100.0, 200.0, 40.0],
            color: [1.0, 1.0, 1.0, 1.0],
            params: [0.0; 4],
            uv: [0.0, 0.0, 1.0, 1.0],
            misc: [1, 0, 0, 0],
        }]),
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
    });

    // --- Frame (mirror render(): shadow → main(sky, scene, ui) ----------
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("repro-frame") });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("shadow"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &shadow_view,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&shadow_pipeline);
        pass.set_bind_group(0, &scene_group0, &[]);
        pass.set_vertex_buffer(0, scene_instances.slice(..));
        pass.draw(0..36, 0..1);
    }
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("main"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &color_view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.35, g: 0.55, b: 0.85, a: 1.0 }),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: &depth_view,
            depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(&sky_pipeline);
    pass.set_bind_group(0, &scene_group0, &[]);
    pass.draw(0..3, 0..1);
    pass.set_pipeline(&scene_pipeline);
    pass.set_bind_group(0, &scene_group0, &[]);
    pass.set_bind_group(1, &scene_group1, &[]);
    pass.set_vertex_buffer(0, scene_instances.slice(..));
    pass.draw(0..36, 0..1);
    pass.set_pipeline(&ui_pipeline);
    pass.set_vertex_buffer(0, ui_instances.slice(..));
    pass.set_bind_group(0, &ui_bind_atlas, &[]);
    pass.draw(0..6, 0..1);
    drop(pass);
    queue.submit([encoder.finish()]);
    println!("SCENE FRAME OK");
}

fn ident() -> [f32; 16] {
    let mut m = [0.0f32; 16];
    m[0] = 1.0;
    m[5] = 1.0;
    m[10] = 1.0;
    m[15] = 1.0;
    m
}
