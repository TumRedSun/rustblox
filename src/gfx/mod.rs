//! wgpu graphics context: surface, pipelines, font atlas, image cache and
//! the per-frame render loop (optional 3D scene + 2D UI on top).

pub mod math;
pub mod scene;

use crate::draw::{Clip, GlyphSlot, Prim};
use crate::gfx::math::Mat4;
use crate::gfx::scene::{Camera, SceneView};
use crate::theme::Rgba;
use bytemuck::{Pod, Zeroable};
use std::collections::HashMap;
use std::sync::Arc;
use winit::window::Window;

// ---------------------------------------------------------------------------
// GPU-side vertex/instance structs (must match the WGSL layouts)
// ---------------------------------------------------------------------------

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
struct SceneInstance {
    rot: [f32; 9],      // column-major 3x3 at 0..36
    _pad0: [f32; 3],    // 36..48
    pos: [f32; 4],      // 48..64
    size: [f32; 4],     // 64..80
    color: [f32; 4],    // 80..96
    params: [f32; 4],   // 96..112
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

// Kinds mirrored from the WGSL ui.wgsl switch.
const K_RECT: u32 = 0;
const K_ROUND: u32 = 1;
const K_ROUND_ROT: u32 = 2;
const K_ROUND_BORDER: u32 = 3;
const K_CIRCLE: u32 = 4;
const K_RING: u32 = 5;
const K_GLYPH: u32 = 6;
const K_IMAGE: u32 = 7;

const SHADOW_SIZE: u32 = 2048;
const MAX_UI_INSTANCES: usize = 1 << 17;
const MAX_SCENE_INSTANCES: usize = 1 << 18;

fn rgba_to_f32(c: Rgba) -> [f32; 4] {
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0]
}

/// Upload a tightly-packed R8/RGBA8 image to a texture, padding every row
/// to wgpu's 256-byte copy-row alignment. `data` holds `width * height *
/// bytes_per_px` bytes, row-major, with no padding of its own.
fn write_texture_padded(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    origin: wgpu::Origin3d,
    data: &[u8],
    width: u32,
    height: u32,
    bytes_per_px: u32,
) {
    let src_bpr = (width * bytes_per_px) as usize;
    let dst_bpr = ((src_bpr + 255) / 256) * 256; // COPY_BYTES_PER_ROW_ALIGNMENT
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

fn clip_to_scissor(c: Clip, w: u32, h: u32) -> (u32, u32, u32, u32) {
    let x0 = c.x0.clamp(0, w as i32) as u32;
    let y0 = c.y0.clamp(0, h as i32) as u32;
    let x1 = c.x1.clamp(0, w as i32) as u32;
    let y1 = c.y1.clamp(0, h as i32) as u32;
    (x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
}

// ---------------------------------------------------------------------------
// Image cache
// ---------------------------------------------------------------------------

struct ImageEntry {
    #[allow(dead_code)]
    texture: wgpu::Texture,
    #[allow(dead_code)]
    view: wgpu::TextureView,
    group: wgpu::BindGroup,
    #[allow(dead_code)]
    last_used: u64,
}

// ---------------------------------------------------------------------------
// Gfx context
// ---------------------------------------------------------------------------

pub struct Gfx {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    #[allow(dead_code)]
    surface_format: wgpu::TextureFormat,
    config: wgpu::SurfaceConfiguration,

    depth: wgpu::TextureView,
    sampler: wgpu::Sampler,

    // UI pipeline
    ui_pipeline: wgpu::RenderPipeline,
    ui_uniform: wgpu::Buffer,
    ui_instances: wgpu::Buffer,
    ui_capacity: usize,
    ui_bind_atlas: wgpu::BindGroup,
    atlas_texture: wgpu::Texture,
    atlas_page_size: u32,

    // Image cache (avatars, icons, place thumbnails)
    images: HashMap<u64, ImageEntry>,
    image_fifo: Vec<u64>,
    frame_no: u64,

    // Scene pipeline
    scene_pipeline: wgpu::RenderPipeline,
    shadow_pipeline: wgpu::RenderPipeline,
    sky_pipeline: wgpu::RenderPipeline,
    frame_uniform: wgpu::Buffer,
    scene_instances: wgpu::Buffer,
    scene_capacity: usize,
    scene_group0: wgpu::BindGroup,
    shadow_tex: wgpu::TextureView,
    scene_group1: wgpu::BindGroup,

    scratch_ui: Vec<UiInstance>,
    scratch_scene: Vec<SceneInstance>,
}

impl Gfx {
    pub fn new(window: &Arc<Window>) -> Gfx {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(Arc::clone(window))
            .expect("create wgpu surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .expect("no suitable GPU adapter");

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("rustblox-device"),
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
        let surface_format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);
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

        // --- UI pipeline ----------------------------------------------------
        let ui_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ui.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("ui.wgsl").into()),
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
        let ui_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ui-uniform"),
            size: std::mem::size_of::<UiUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let ui_bind_atlas = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ui-bg-atlas"),
            layout: &ui_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: ui_uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&atlas_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        let ui_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ui-pl"),
            bind_group_layouts: &[Some(&ui_bgl)],
            immediate_size: 0,
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
        // The main pass always owns a Depth32Float attachment, and wgpu 29
        // requires every pipeline used in that pass to declare the same
        // depth-stencil format. UI and sky ignore depth: test always, write
        // never.
        let overlay_depth = wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: Default::default(),
            bias: Default::default(),
        };
        let ui_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ui-pipeline"),
            layout: Some(&ui_pipeline_layout),
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
            depth_stencil: Some(overlay_depth.clone()),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        // --- Scene pipeline -------------------------------------------------
        let scene_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("scene.wgsl").into()),
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
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&shadow_tex) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&cmp_sampler) },
            ],
        });
        let scene_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
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
            depth_stencil: Some(overlay_depth.clone()),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        let ui_instances = Self::make_buf(&device, MAX_UI_INSTANCES, std::mem::size_of::<UiInstance>(), "ui-instances");
        let scene_instances = Self::make_buf(&device, MAX_SCENE_INSTANCES, std::mem::size_of::<SceneInstance>(), "scene-instances");

        Gfx {
            device,
            queue,
            surface,
            surface_format,
            config,
            depth,
            sampler,
            ui_pipeline,
            ui_uniform,
            ui_instances,
            ui_capacity: MAX_UI_INSTANCES,
            ui_bind_atlas,
            atlas_texture,
            atlas_page_size: 1024,
            images: HashMap::new(),
            image_fifo: Vec::new(),
            frame_no: 0,
            scene_pipeline,
            shadow_pipeline,
            sky_pipeline,
            frame_uniform,
            scene_instances,
            scene_capacity: MAX_SCENE_INSTANCES,
            scene_group0,
            shadow_tex,
            scene_group1,
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
            size: wgpu::Extent3d { width: SHADOW_SIZE, height: SHADOW_SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        tex.create_view(&Default::default())
    }

    fn make_buf(device: &wgpu::Device, n: usize, stride: usize, label: &str) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (n * stride) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
        self.depth = Self::make_depth(&self.device, self.config.width, self.config.height);
    }

    // -----------------------------------------------------------------
    // Image cache
    // -----------------------------------------------------------------

    fn image_group(&mut self, hash: u64, data: &[u8], sw: i32, sh: i32) -> Option<&wgpu::BindGroup> {
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
        write_texture_padded(
            &self.queue,
            &texture,
            wgpu::Origin3d::ZERO,
            data,
            sw,
            sh,
            4,
        );
        let layout = self.ui_pipeline.get_bind_group_layout(0);
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ui-image-bg"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.ui_uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
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

    // -----------------------------------------------------------------
    // Font atlas
    // -----------------------------------------------------------------

    fn sync_atlas(&mut self, atlas: &crate::font::AtlasState) {
        // Regrow the texture if the atlas grew past our page size.
        if atlas.page_size != self.atlas_page_size {
            self.atlas_page_size = atlas.page_size;
            self.atlas_texture = self.device.create_texture(&wgpu::TextureDescriptor {
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
            let view = self.atlas_texture.create_view(&Default::default());
            let layout = self.ui_pipeline.get_bind_group_layout(0);
            self.ui_bind_atlas = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ui-bg-atlas"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.ui_uniform.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
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
        let page = self.atlas_page_size;
        if m.x + m.w > page as u16 || m.y + m.h > page as u16 {
            return;
        }
        write_texture_padded(
            &self.queue,
            &self.atlas_texture,
            wgpu::Origin3d { x: m.x as u32, y: m.y as u32, z: 0 },
            &m.data,
            w,
            h,
            1,
        );
    }

    // -----------------------------------------------------------------
    // Instance packing
    // -----------------------------------------------------------------

    fn pack_ui(&mut self, prims: &[(Prim, Clip)]) {
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
                Prim::RoundRectBorder { cx, cy, w, h, radius, angle, border, color } => UiInstance {
                    pos_size: [*cx as f32, *cy as f32, *w as f32, *h as f32],
                    color: rgba_to_f32(*color),
                    params: [*radius, *angle, *border, 0.0],
                    uv: [0.0; 4],
                    misc: [K_ROUND_BORDER, 0, 0, 0],
                },
                Prim::Circle { cx, cy, r, color } => UiInstance {
                    pos_size: [(*cx - *r) as f32, (*cy - *r) as f32, (*r * 2 + 1) as f32, (*r * 2 + 1) as f32],
                    color: rgba_to_f32(*color),
                    params: [*r as f32, 0.0, 0.0, 0.0],
                    uv: [0.0; 4],
                    misc: [K_CIRCLE, 0, 0, 0],
                },
                Prim::Ring { cx, cy, ro, ri, color } => UiInstance {
                    pos_size: [(*cx - *ro) as f32, (*cy - *ro) as f32, (*ro * 2 + 1) as f32, (*ro * 2 + 1) as f32],
                    color: rgba_to_f32(*color),
                    params: [*ro as f32, 0.0, *ri as f32, 0.0],
                    uv: [0.0; 4],
                    misc: [K_RING, 0, 0, 0],
                },
                Prim::Glyph { x, y, slot, color } => {
                    let ps = glyph_uv(slot, self.atlas_page_size);
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
        if self.scratch_ui.len() > self.ui_capacity {
            let mut n = self.ui_capacity;
            while n < self.scratch_ui.len() {
                n *= 2;
            }
            self.ui_instances = Self::make_buf(&self.device, n, std::mem::size_of::<UiInstance>(), "ui-instances");
            self.ui_capacity = n;
        }
    }

    fn ensure_scene_capacity(&mut self) {
        if self.scratch_scene.len() > self.scene_capacity {
            let mut n = self.scene_capacity;
            while n < self.scratch_scene.len() {
                n *= 2;
            }
            self.scene_instances = Self::make_buf(&self.device, n, std::mem::size_of::<SceneInstance>(), "scene-instances");
            self.scene_capacity = n;
        }
    }

    fn build_frame_uniforms(&self, cam: &Camera, env: &crate::gfx::scene::Environment) -> FrameUniforms {
        let aspect = self.config.width as f32 / self.config.height as f32;
        let eye = cam.pos;
        let fwd = cam.forward();
        let target = [
            eye[0] + fwd[0],
            eye[1] + fwd[1],
            eye[2] + fwd[2],
        ];
        let view_proj = Mat4::mul(
            &Mat4::perspective(cam.fov, aspect, 0.05, 1200.0),
            &Mat4::look_at(eye, target, [0.0, 1.0, 0.0]),
        );
        let sun = math::norm3(env.sun_dir);
        // Shadow camera: ortho box around the viewer.
        let radius = 180.0_f32;
        let light_eye = [
            eye[0] + sun[0] * 300.0,
            eye[1] + sun[1] * 300.0,
            eye[2] + sun[2] * 300.0,
        ];
        let light_view = Mat4::look_at(light_eye, eye, [0.0, 1.0, 0.0]);
        let light_view_proj = Mat4::mul(&Mat4::ortho(-radius, radius, -radius, radius, 20.0, 700.0), &light_view);
        FrameUniforms {
            view_proj: view_proj.0,
            inv_view_proj: view_proj.inverse().0,
            light_view_proj: light_view_proj.0,
            cam_pos: [eye[0], eye[1], eye[2], 1.0],
            sun_dir: [sun[0], sun[1], sun[2], 0.0],
            // Gamma-space lighting throughout (consistent with the UI path
            // on a non-sRGB surface): colors pass through unchanged.
            sun_color: [env.sun_color[0], env.sun_color[1], env.sun_color[2], 1.0],
            ambient_sky: [env.ambient_sky[0], env.ambient_sky[1], env.ambient_sky[2], 1.0],
            ambient_ground: [env.ambient_ground[0], env.ambient_ground[1], env.ambient_ground[2], 1.0],
            fog_color: [env.fog_color[0], env.fog_color[1], env.fog_color[2], env.fog_density],
            sky_top: [env.sky_top[0], env.sky_top[1], env.sky_top[2], 1.0],
            sky_horizon: [env.sky_horizon[0], env.sky_horizon[1], env.sky_horizon[2], 1.0],
            params: [0.0, env.shadow_strength, 0.0, 0.0],
        }
    }

    fn pack_scene(&mut self, scene: &SceneView) {
        self.scratch_scene.clear();
        for p in &scene.parts {
            self.scratch_scene.push(SceneInstance {
                rot: p.rot,
                _pad0: [0.0; 3],
                pos: [p.pos[0], p.pos[1], p.pos[2], 0.0],
                size: [p.size[0], p.size[1], p.size[2], 0.0],
                color: p.color,
                params: [p.roughness, p.emissive, 0.0, 0.0],
            });
        }
        self.ensure_scene_capacity();
    }

    // -----------------------------------------------------------------
    // Frame render
    // -----------------------------------------------------------------

    pub fn render(
        &mut self,
        prims: &[(Prim, Clip)],
        atlas: &mut crate::font::AtlasState,
        scene: Option<&SceneView>,
    ) -> Result<bool, ()> {
        self.frame_no += 1;
        self.sync_atlas(atlas);
        // The pending masks were just uploaded; drop them so the next frame
        // only uploads glyphs rasterized since. Leaving the list intact made
        // every frame re-upload the ENTIRE accumulated glyph set — per-frame
        // upload cost (and driver staging pressure) grew with the session,
        // which is the progressive-freeze / memory-bloat bug.
        atlas.pending.clear();

        // UI uniforms.
        self.queue.write_buffer(
            &self.ui_uniform,
            0,
            bytemuck::bytes_of(&UiUniforms {
                resolution: [self.config.width as f32, self.config.height as f32],
                srgb: 0.0,
                _pad: 0.0,
            }),
        );
        self.pack_ui(prims);
        if !self.scratch_ui.is_empty() {
            self.queue.write_buffer(
                &self.ui_instances,
                0,
                bytemuck::cast_slice(&self.scratch_ui),
            );
        }

        let has_scene = scene.is_some();
        if let Some(sc) = scene {
            let fu = self.build_frame_uniforms(&sc.camera, &sc.env);
            self.queue.write_buffer(&self.frame_uniform, 0, bytemuck::bytes_of(&fu));
            self.pack_scene(sc);
            if !self.scratch_scene.is_empty() {
                self.queue.write_buffer(
                    &self.scene_instances,
                    0,
                    bytemuck::cast_slice(&self.scratch_scene),
                );
            }
        }

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            // Skip the frame; the surface will be reconfigured on resize.
            _ => return Ok(false),
        };
        let view = frame.texture.create_view(&Default::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });

        // --- Shadow pass ---------------------------------------------------
        if has_scene && !self.scratch_scene.is_empty() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_tex,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.shadow_pipeline);
            pass.set_bind_group(0, &self.scene_group0, &[]);
            pass.set_vertex_buffer(0, self.scene_instances.slice(..));
            let n = self.scratch_scene.len() as u32;
            pass.draw(0..36, 0..n);
        }

        // --- Main pass -----------------------------------------------------
        let clear: [f64; 4] = match scene {
            Some(_) => [0.35, 0.55, 0.85, 1.0],
            None => [0.06, 0.07, 0.09, 1.0],
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("main"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: clear[0],
                        g: clear[1],
                        b: clear[2],
                        a: clear[3],
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        if has_scene {
            // Sky first (no depth interaction).
            pass.set_pipeline(&self.sky_pipeline);
            pass.set_bind_group(0, &self.scene_group0, &[]);
            pass.draw(0..3, 0..1);

            // Lit geometry.
            if !self.scratch_scene.is_empty() {
                pass.set_pipeline(&self.scene_pipeline);
                pass.set_bind_group(0, &self.scene_group0, &[]);
                pass.set_bind_group(1, &self.scene_group1, &[]);
                pass.set_vertex_buffer(0, self.scene_instances.slice(..));
                let n = self.scratch_scene.len() as u32;
                pass.draw(0..36, 0..n);
            }
        }

        // --- UI on top -----------------------------------------------------
        pass.set_pipeline(&self.ui_pipeline);
        pass.set_vertex_buffer(0, self.ui_instances.slice(..));

        let screen_w = self.config.width;
        let screen_h = self.config.height;
        let mut batch_start: usize = 0;
        let mut cur_clip: Option<Clip> = None;
        let mut cur_img: Option<u64> = None;
        let n = prims.len();
        for i in 0..=n {
            let next = if i < n { Some(&prims[i]) } else { None };
            let (img, clip) = match next {
                Some((Prim::Image { hash, .. }, c)) => (Some(*hash), *c),
                Some((_, c)) => (None, *c),
                None => (None, Clip::full(0, 0)), // sentinel: force a flush
            };
            let same_batch = match (cur_clip, cur_img, next.is_some()) {
                (None, _, _) => false,
                (_, _, false) => false,
                (Some(c), cur, true) => {
                    c == clip
                        && match (cur, img) {
                            (None, None) => true,
                            (Some(a), Some(b)) => a == b,
                            _ => false,
                        }
                }
            };
            if same_batch {
                continue;
            }
            // Flush the finished batch.
            if let (Some(c), start_ok) = (cur_clip, batch_start < i) {
                if start_ok {
                    ui_flush(&mut pass, screen_w, screen_h, batch_start as u32, i as u32, c);
                }
            }
            // Start the next batch (bind the right texture group).
            match img {
                Some(h) => {
                    match self.images.get(&h) {
                        Some(group) => pass.set_bind_group(0, &group.group, &[]),
                        // The image failed to upload (bad data, size guard)
                        // — draw the quad against the atlas instead of
                        // leaving slot 0 unbound: a draw with a missing
                        // required bind group is recorded as a lazy
                        // validation error that surfaces at finish().
                        None => pass.set_bind_group(0, &self.ui_bind_atlas, &[]),
                    }
                }
                None => {
                    if next.is_some() {
                        pass.set_bind_group(0, &self.ui_bind_atlas, &[]);
                    }
                }
            }
            cur_clip = Some(clip);
            cur_img = if next.is_some() { img } else { None };
            batch_start = i;
        }
        drop(pass);

        self.queue.submit([encoder.finish()]);
        frame.present();
        Ok(true)
    }
}

/// Cubic-ish conversion of a gamma-space color to linear (unused today;
/// kept for the future linear-lighting path).
#[allow(dead_code)]
fn lin3(c: [f32; 3]) -> [f32; 4] {
    let f = |x: f32| -> f32 {
        if x <= 0.04045 {
            x / 12.92
        } else {
            ((x + 0.055) / 1.055).powf(2.4)
        }
    };
    [f(c[0]), f(c[1]), f(c[2]), 1.0]
}

/// Flush one UI batch: set the scissor to the batch clip and draw the
/// instance range.
fn ui_flush<'p>(
    pass: &mut wgpu::RenderPass<'p>,
    screen_w: u32,
    screen_h: u32,
    start: u32,
    end: u32,
    clip: Clip,
) {
    let (x, y, w, h) = clip_to_scissor(clip, screen_w, screen_h);
    // An empty or fully off-screen clip has nothing to draw; a clamped
    // 1x1 scissor could stick out of the target and fail validation.
    if w == 0 || h == 0 {
        return;
    }
    pass.set_scissor_rect(x, y, w, h);
    pass.draw(0..6, start..end);
}

fn glyph_uv(slot: &GlyphSlot, page: u32) -> [f32; 4] {
    let p = page as f32;
    [
        slot.x as f32 / p,
        slot.y as f32 / p,
        slot.w as f32 / p,
        slot.h as f32 / p,
    ]
}

// ---------------------------------------------------------------------------
// Shader validation: wgpu only parses/validates WGSL at runtime on the GPU
// path, which no CI machine can exercise. Parse + validate every embedded
// shader with naga (the same front-end wgpu uses) so shader mistakes break
// `cargo test` instead of the client on launch.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod shader_tests {
    #[test]
    fn gfx_shaders_parse_and_validate() {
        let sources: [(&str, &str); 2] = [
            ("ui.wgsl", include_str!("ui.wgsl")),
            ("scene.wgsl", include_str!("scene.wgsl")),
        ];
        for (name, src) in sources {
            let module = naga::front::wgsl::parse_str(src)
                .unwrap_or_else(|e| panic!("{name}: WGSL parse error: {e}"));
            let mut validator = naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            );
            validator
                .validate(&module)
                .unwrap_or_else(|e| panic!("{name}: WGSL validation error: {e}"));
            // Entry points must exist.
            assert!(!module.entry_points.is_empty(), "{name}: no entry points");
        }
    }
}
