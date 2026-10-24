//! The remaster's renderer, on `wgpu`.
//!
//! First cut: models from `qwc_assets::model` drawn with their textures and vertex colours, GPU skinning with one
//! bone per vertex (the game's rule), one directional light. It runs in a window or offscreen ([`Offscreen`]), which
//! is how the tests and screenshots work without a display.
//!
//! A model's own materials are drawn as the original sets them up from its names' flags (`qwc_formats::fpm::Surface`,
//! `0x4a57a0`): layer by layer across every model (the unblended names, then the lightmaps, then the blended ones),
//! each blended as its mode's record (`0x508340`) with the texture wrapped or clamped and alpha tested as its flags
//! say. A lightmap is its own draw over its base's vertices (bit for bit), blended `DESTCOLOR, SRCCOLOR`: twice the
//! base times the lightmap. An edited model's materials have no flags: those with alpha in their texture blend, after
//! everything else, the others are cut at half alpha. A draw flagged two-sided is not culled (`D3DCULL_NONE`, the
//! others `D3DCULL_CCW`). Only the finest level of detail is drawn (**[I]** the original picks one by distance,
//! `0x492b00`).
//!
//! The textures are uploaded in the target's encoding: for an sRGB target as sRGB (blended in linear light), for one
//! without (the front end's, which works in the stored values as the original's screen did) as they are stored, so
//! the blends are the original's.
//!
//! Not done yet: Groper material stages (`.gro`: `tcmod`, `rgbgen`, animated maps), the original lighting, morph
//! targets on the GPU, particles, trails, lens flare and depth of field.
//!
//! [`Look::Remastered`] draws the same models in the remaster's own lighting instead ([`remaster`]: the sun with
//! shadows, fog, HDR and tone mapping); [`Look::Original`], the default, as above.
//!
//! [`overlay`] is the front end's 2D pass; [`flags`] the stadium's wavy flags (their cloth, which a mesh takes each
//! frame through [`Renderer::set_vertices`]).
//!
//! Conventions: the game is Y-up and left-handed with row-vector matrices; [`Camera`] builds a left-handed
//! view-projection with a 0..1 depth range, and front faces are clockwise as in Direct3D.

use std::collections::HashMap;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use qwc_assets::{groper, model::Model, texture, Vfs};
use qwc_formats::fpm;
use wgpu::util::DeviceExt;

pub use glam;
pub use wgpu;

pub mod custom;
pub mod effects;
pub mod flags;
pub mod normals;
pub mod overlay;
pub mod remaster;

pub use remaster::Look;

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// A GPU device and queue.
pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl Gpu {
    /// Open the default adapter, compatible with `surface` when one is given.
    pub fn new(instance: wgpu::Instance, surface: Option<&wgpu::Surface<'_>>) -> Result<Gpu, String> {
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: surface,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .map_err(|e| format!("no graphics adapter: {e}"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("qwc"),
            // BC textures where the adapter has them (`Renderer::compress`)
            required_features: adapter.features() & wgpu::Features::TEXTURE_COMPRESSION_BC,
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            ..Default::default()
        }))
        .map_err(|e| format!("no graphics device: {e}"))?;
        Ok(Gpu { instance, adapter, device, queue })
    }

    /// A device for offscreen rendering (no window).
    pub fn headless() -> Result<Gpu, String> {
        Gpu::new(wgpu::Instance::new(primary_backends(wgpu::InstanceDescriptor::new_without_display_handle())), None)
    }
}

/// Restrict an instance to Vulkan, Metal and DX12 (the shaders read bone matrices from a storage buffer in the
/// vertex stage, which many OpenGL drivers lack), unless `WGPU_BACKEND` and the other wgpu variables say otherwise.
pub fn primary_backends(mut desc: wgpu::InstanceDescriptor) -> wgpu::InstanceDescriptor {
    desc.backends = wgpu::Backends::PRIMARY;
    desc.with_env()
}

/// A perspective camera in game space (Y up, left-handed).
#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub eye: Vec3,
    pub target: Vec3,
    /// The view's up direction (it need not be square to the view).
    pub up: Vec3,
    /// Vertical field of view in radians.
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
}

impl Camera {
    /// A camera looking at the box `lo..hi` from `yaw`/`pitch` (radians), far enough back to frame it.
    pub fn framing(lo: [f32; 3], hi: [f32; 3], yaw: f32, pitch: f32, distance_scale: f32) -> Camera {
        let (lo, hi) = (Vec3::from(lo), Vec3::from(hi));
        let center = (lo + hi) * 0.5;
        let radius = ((hi - lo).length() * 0.5).max(1e-3);
        let fov_y = 40f32.to_radians();
        let dist = radius / (fov_y * 0.5).sin() * distance_scale;
        let dir = Vec3::new(yaw.sin() * pitch.cos(), pitch.sin(), -yaw.cos() * pitch.cos());
        let up = if dir.y.abs() > 0.999 { Vec3::Z } else { Vec3::Y };
        Camera { eye: center + dir * dist, target: center, up, fov_y, near: dist * 0.01, far: dist + radius * 4.0 }
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        Mat4::perspective_lh(self.fov_y, aspect, self.near, self.far) * Mat4::look_at_lh(self.eye, self.target, self.up)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
    color: [u8; 4],
    bone: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameUniform {
    view_proj: [f32; 16],
    ambient: [f32; 4],
    /// Towards each light (the scripts' directions), and its colour.
    dirs: [[f32; 4]; 3],
    colours: [[f32; 4]; 3],
}

/// A light set (`Lights.*`: a stadium's `gfx/lights.cli`; `RE_NOTES.md`, "The lights"): an ambient colour and the
/// three directional lights, sun, fill and kick, each a colour and the direction towards it (the draw state
/// `0x50f250` gives Direct3D its negation). A lit material is drawn with the ambient plus each light by the
/// normal's cosine to it, clamped to 1 as Direct3D's lighting is; an unlit one with its vertex colour alone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lights {
    pub ambient: [f32; 3],
    /// `(colour, direction)`; a zero direction is a light turned off.
    pub lights: [([f32; 3], [f32; 3]); 3],
}

impl Lights {
    /// The renderer's own light when no stadium gives one: `ambient` of white, the rest from one direction the
    /// light travels.
    pub fn simple(ambient: f32, travels: Vec3) -> Lights {
        let rest = 1.0 - ambient;
        let off = ([0.0; 3], [0.0; 3]);
        Lights { ambient: [ambient; 3], lights: [([rest; 3], (-travels).to_array()), off, off] }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MaterialUniform {
    flags: [u32; 4],
    /// The texture matrix's rows (`u' = uv[0].xyz . (u, v, 1)`): a Groper stage's `tcMod`s, the identity otherwise.
    uv: [[f32; 4]; 2],
}

const UV_IDENTITY: [[f32; 4]; 2] = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]];

/// A material's Groper stage (`qwc_assets::groper`) and a bind group per frame of its `animmap`.
struct Animation {
    stage: groper::Stage,
    frames: Vec<wgpu::BindGroup>,
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
    material: usize,
    two_sided: bool,
}

/// Which blend a material's draws take.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Blend {
    Opaque,
    /// `SRCALPHA, INVSRCALPHA`.
    Alpha,
    /// `DESTCOLOR, SRCCOLOR`, the destination's alpha kept.
    Lightmap,
    /// `SRCALPHA, ONE` (`SRCALPHA_ADD` and its kin), the destination's alpha kept.
    Additive,
    /// No colour, depth only: a Groper `zfill` stage (mode 19, `ZERO, ONE`).
    DepthOnly,
}

/// A pipeline: its blend, whether it writes depth and whether it draws both sides (no culling).
type PipelineKey = (Blend, bool, bool);

struct GpuMaterial {
    bind_group: wgpu::BindGroup,
    blend: Blend,
    depth_write: bool,
    /// Its faces' depth is drawn first, alone (a Groper `zfill` stage before its colour stage).
    zfill: bool,
    /// The order it is drawn in, across every model: lower first.
    layer: u16,
    /// Its name, its own texture, its uniform and sampler: what [`Renderer::set_material_texture`] rebuilds it from.
    name: String,
    texture: Arc<GpuTexture>,
    uniform: wgpu::Buffer,
    sampler: usize,
    /// The uniform's values (`w` lit).
    flags: [u32; 4],
    uv: [[f32; 4]; 2],
    animation: Option<Animation>,
    /// The pack's material shader drawing it in the remastered look ([`custom::Shaders::material_for`]; 0 stock).
    shader: u16,
}

/// The layer an edited model's blended materials are drawn in: after every layer of the original's.
const LAST_LAYER: u16 = 0x100;

/// How the shader treats a texel's alpha (`MaterialUniform::flags[1]`).
const ALPHA_KEPT: u32 = 0;
const ALPHA_HALF: u32 = 1;
const ALPHA_TESTED: u32 = 2;

/// The material's kind (`MaterialUniform::flags[3]`, bits): lit by the [`Lights`]; the sky (the remastered
/// lighting's: drawn as it is, casting no shadow); a lightmap.
const KIND_LIT: u32 = 1;
const KIND_SKY: u32 = 2;
const KIND_LIGHTMAP: u32 = 4;
/// Drawn by the remastered lighting as the original draws it (a painted reflection), casting no shadow.
const KIND_AS_ORIGINAL: u32 = 8;

struct GpuTexture {
    view: wgpu::TextureView,
    has_alpha: bool,
}

/// A model uploaded to the GPU. Its bone matrices start at the bind pose; see [`Renderer::set_pose`].
pub struct GpuModel {
    meshes: Vec<GpuMesh>,
    materials: Vec<GpuMaterial>,
    bones: wgpu::Buffer,
    bones_group: wgpu::BindGroup,
    bone_count: usize,
    skinned: bool,
    /// Root matrix applied to unskinned models (row-vector layout).
    matrix: [f32; 16],
    pub bounds: Option<([f32; 3], [f32; 3])>,
    pub path: String,
    /// Whether it casts the sun's shadow in the remastered lighting (all but the sky, [`Renderer::set_sky`]).
    pub casts_shadow: bool,
}

impl GpuModel {
    pub fn is_skinned(&self) -> bool {
        self.skinned
    }

    /// The model's root matrix as loaded (row-vector convention), the one [`Renderer::set_transform`] falls back to.
    pub fn matrix(&self) -> [f32; 16] {
        self.matrix
    }

    pub fn bone_count(&self) -> usize {
        self.bone_count
    }
}

/// Pipelines, layouts and the texture cache.
pub struct Renderer {
    pipelines: HashMap<PipelineKey, wgpu::RenderPipeline>,
    bones_layout: wgpu::BindGroupLayout,
    material_layout: wgpu::BindGroupLayout,
    frame_buffer: wgpu::Buffer,
    frame_group: wgpu::BindGroup,
    /// By wrapping in U and in V (clamped otherwise): `[clamp, wrap U, wrap V, both]`.
    samplers: [wgpu::Sampler; 4],
    white: Arc<GpuTexture>,
    textures: HashMap<String, Arc<GpuTexture>>,
    /// The textures' format: sRGB for an sRGB target, else as stored.
    texture_format: wgpu::TextureFormat,
    pub color_format: wgpu::TextureFormat,
    pub clear: wgpu::Color,
    /// The lights lit materials are drawn with ([`Lights::simple`] until a stadium's are set).
    pub lights: Lights,
    /// The remastered PNGs drawn in place of the disc's textures of the models uploaded after it is set.
    pub remastered: Option<qwc_assets::remaster::Remastered>,
    /// How the models are lit: the original's way, or the remaster's ([`remaster`]).
    pub look: Look,
    /// The remastered lighting's settings.
    pub settings: remaster::Settings,
    /// Not the original's: the big pictures uploaded after it is set (the packs' textures and models' own images,
    /// [`qwc_assets::compress::worth_it`]) go to the GPU as BC7, a quarter of the memory, where the device can sample
    /// BC; `Some(cache)` turns it on, the folder keeping each picture's blocks (none: encoded every time).
    pub compress: Option<Option<std::path::PathBuf>>,
    /// The packs' shaders the remastered look draws with ([`custom`]): set before the first remastered draw and
    /// the models' upload (which picks each material's).
    pub shaders: Arc<custom::Shaders>,
    /// The game's clock in seconds, as the shaders see it ([`Renderer::set_time`]), as an `f32`'s bits.
    time: std::sync::atomic::AtomicU32,
    frame_layout: wgpu::BindGroupLayout,
    /// The remastered lighting's pipelines and targets, made the first time it draws.
    lighting: std::sync::OnceLock<remaster::Lighting>,
}

fn row_major(m: &[f64; 16]) -> [f32; 16] {
    m.map(|v| v as f32)
}

/// The vertex layout the model shaders take.
const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Unorm8x4, 4 => Uint32
];

fn vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Vertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &VERTEX_ATTRIBUTES,
    }
}

/// A pipeline of `shader` (`vs_main`, `fs_main`) on `layout` for each [`PipelineKey`], drawing into `format`, and
/// into each of `extra` (written by the unblended draws only).
fn model_pipelines(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    extra: &[wgpu::TextureFormat],
) -> HashMap<PipelineKey, wgpu::RenderPipeline> {
    let pipeline = |(blend, depth_write, two_sided): PipelineKey| {
        let blend_state = match blend {
            Blend::Opaque => None,
            Blend::Alpha => Some(wgpu::BlendState::ALPHA_BLENDING),
            Blend::Lightmap => Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Dst,
                    dst_factor: wgpu::BlendFactor::Src,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Zero,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
            Blend::Additive => Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Zero,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
            Blend::DepthOnly => None,
        };
        let mask = if blend == Blend::DepthOnly { wgpu::ColorWrites::empty() } else { wgpu::ColorWrites::ALL };
        let mut targets = vec![Some(wgpu::ColorTargetState { format, blend: blend_state, write_mask: mask })];
        let writes = if blend == Blend::Opaque { wgpu::ColorWrites::ALL } else { wgpu::ColorWrites::empty() };
        targets.extend(
            extra.iter().map(|&format| Some(wgpu::ColorTargetState { format, blend: None, write_mask: writes })),
        );
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(&format!(
                "{blend:?}{}{}",
                if depth_write { "" } else { ", no depth write" },
                if two_sided { ", two-sided" } else { "" }
            )),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex_layout())],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Cw,
                cull_mode: (!two_sided).then_some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(depth_write),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &targets,
            }),
            multiview_mask: None,
            cache: None,
        })
    };
    [Blend::Opaque, Blend::Alpha, Blend::Lightmap, Blend::Additive, Blend::DepthOnly]
        .into_iter()
        .flat_map(|b| [(b, true, false), (b, false, false), (b, true, true), (b, false, true)])
        .map(|k| (k, pipeline(k)))
        .collect()
}

/// A mesh's vertices as the shader takes them: no colours white. A mesh without normals is given some
/// ([`normals::smooth`]) at half length, [`MADE_NORMAL`], which the original's lighting leaves unlit as it is.
fn vertices(mesh: &qwc_assets::model::Mesh) -> Vec<Vertex> {
    vertices_at(mesh, &mesh.positions)
}

/// The length of a normal made for a mesh without them: `shader.wgsl` lights only longer ones (the disc's).
const MADE_NORMAL: f32 = 0.5;

/// The mesh's vertices at `positions` (as many as it has).
fn vertices_at(mesh: &qwc_assets::model::Mesh, positions: &[[f32; 3]]) -> Vec<Vertex> {
    let n = positions.len().min(mesh.positions.len());
    let made = mesh.normals.is_none().then(|| normals::smooth(&positions[..n], &mesh.indices));
    (0..n)
        .map(|i| Vertex {
            position: positions[i],
            normal: match (&mesh.normals, &made) {
                (Some(v), _) => v[i],
                (None, Some(m)) => m[i].map(|c| c * MADE_NORMAL),
                (None, None) => [0.0; 3],
            },
            uv: mesh.uvs[i],
            color: mesh.colors.as_ref().map_or([255; 4], |v| v[i]),
            bone: mesh.bones.as_ref().map_or(0, |v| v[i] as u32),
        })
        .collect()
}

impl Renderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, color_format: wgpu::TextureFormat) -> Renderer {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("model"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let entry = |binding, visibility, ty| wgpu::BindGroupLayoutEntry { binding, visibility, ty, count: None };
        let uniform = wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let vf = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT;
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame"),
            entries: &[entry(0, vf, uniform)],
        });
        let bones_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bones"),
            entries: &[entry(
                0,
                wgpu::ShaderStages::VERTEX,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            )],
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material"),
            entries: &[
                entry(
                    0,
                    wgpu::ShaderStages::FRAGMENT,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(1, wgpu::ShaderStages::FRAGMENT, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
                entry(2, wgpu::ShaderStages::FRAGMENT, uniform),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("model"),
            bind_group_layouts: &[Some(&frame_layout), Some(&bones_layout), Some(&material_layout)],
            immediate_size: 0,
        });
        let pipelines = model_pipelines(device, &layout, &shader, color_format, &[]);
        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame"),
            size: std::mem::size_of::<FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let frame_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame"),
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: frame_buffer.as_entire_binding() }],
        });
        let samplers = [0, 1, 2, 3].map(|k| {
            let mode = |wrap| if wrap { wgpu::AddressMode::Repeat } else { wgpu::AddressMode::ClampToEdge };
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("model"),
                address_mode_u: mode(k & 1 != 0),
                address_mode_v: mode(k & 2 != 0),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                // not the original's: textures seen at a glancing angle (the ground, a stand's banners) stay sharp
                anisotropy_clamp: 16,
                ..Default::default()
            })
        });
        let texture_format =
            if color_format.is_srgb() { wgpu::TextureFormat::Rgba8UnormSrgb } else { wgpu::TextureFormat::Rgba8Unorm };
        let white = Arc::new(Self::upload_texture(device, queue, texture_format, 1, 1, &[vec![255; 4]], "white"));
        Renderer {
            pipelines,
            bones_layout,
            material_layout,
            frame_buffer,
            frame_group,
            samplers,
            white,
            textures: HashMap::new(),
            texture_format,
            color_format,
            clear: wgpu::Color { r: 0.42, g: 0.55, b: 0.72, a: 1.0 },
            lights: Lights::simple(0.45, Vec3::new(-0.4, -1.0, 0.6).normalize()),
            remastered: None,
            look: Look::Original,
            settings: remaster::Settings::default(),
            compress: None,
            shaders: Arc::default(),
            time: Default::default(),
            frame_layout,
            lighting: std::sync::OnceLock::new(),
        }
    }

    fn upload_texture(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        levels: &[Vec<u8>],
        label: &str,
    ) -> GpuTexture {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (level, rgba) in levels.iter().enumerate() {
            let (w, h) = ((width >> level).max(1), (height >> level).max(1));
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                rgba,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
        }
        let has_alpha = levels[0].as_chunks::<4>().0.iter().any(|p| p[3] < 255);
        GpuTexture { view: tex.create_view(&Default::default()), has_alpha }
    }

    /// Upload `t`, as BC7 when [`Renderer::compress`] is on, the device has BC and the picture is worth it.
    fn upload_image(&self, gpu: &Gpu, t: &qwc_assets::texture::Texture, label: &str) -> GpuTexture {
        let bc = gpu.device.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC);
        match &self.compress {
            Some(cache) if bc && qwc_assets::compress::worth_it(t) => {
                let blocks = qwc_assets::compress::cached(t, cache.as_deref());
                let has_alpha = t.levels[0].as_chunks::<4>().0.iter().any(|p| p[3] < 255);
                let format = if self.texture_format.is_srgb() {
                    wgpu::TextureFormat::Bc7RgbaUnormSrgb
                } else {
                    wgpu::TextureFormat::Bc7RgbaUnorm
                };
                Self::upload_bc7(&gpu.device, &gpu.queue, format, &blocks, has_alpha, label)
            }
            _ => {
                Self::upload_texture(&gpu.device, &gpu.queue, self.texture_format, t.width, t.height, &t.levels, label)
            }
        }
    }

    /// Upload BC7 blocks (each level's padded to whole blocks, as the GPU keeps them).
    fn upload_bc7(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        b: &qwc_assets::compress::Bc7,
        has_alpha: bool,
        label: &str,
    ) -> GpuTexture {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: b.width, height: b.height, depth_or_array_layers: 1 },
            mip_level_count: b.levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (level, blocks) in b.levels.iter().enumerate() {
            let (w, h) = ((b.width >> level).max(1), (b.height >> level).max(1));
            let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                blocks,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bw * 16), rows_per_image: Some(bh) },
                // the level's physical size: whole blocks
                wgpu::Extent3d { width: bw * 4, height: bh * 4, depth_or_array_layers: 1 },
            );
        }
        GpuTexture { view: tex.create_view(&Default::default()), has_alpha }
    }

    /// Set the game's clock (seconds) the shaders see: the material shaders' `frame.time`, the post passes'
    /// `post.time`.
    pub fn set_time(&self, seconds: f32) {
        self.time.store(seconds.to_bits(), std::sync::atomic::Ordering::Relaxed);
    }

    fn time(&self) -> f32 {
        f32::from_bits(self.time.load(std::sync::atomic::Ordering::Relaxed))
    }

    fn texture(&mut self, gpu: &Gpu, vfs: &Vfs, path: &str) -> Result<Arc<GpuTexture>, String> {
        if let Some(t) = self.textures.get(path) {
            return Ok(t.clone());
        }
        let png = self.remastered.as_ref().and_then(|r| r.png(path)).map(qwc_assets::remaster::load_png);
        let gt = Arc::new(match png {
            // a pack's picture: compressed when it is big (the disc's own are left as they are)
            Some(Ok(t)) => self.upload_image(gpu, &t, path),
            Some(Err(e)) => {
                eprintln!("{e}; the disc's texture is drawn");
                let t = texture::load(vfs, path).map_err(|e| e.to_string())?;
                Self::upload_texture(&gpu.device, &gpu.queue, self.texture_format, t.width, t.height, &t.levels, path)
            }
            None => {
                let t = texture::load(vfs, path).map_err(|e| e.to_string())?;
                Self::upload_texture(&gpu.device, &gpu.queue, self.texture_format, t.width, t.height, &t.levels, path)
            }
        });
        self.textures.insert(path.to_string(), gt.clone());
        Ok(gt)
    }

    /// Upload a model and its textures (its finest level of detail). Missing textures draw white.
    pub fn upload_model(&mut self, gpu: &Gpu, vfs: &Vfs, model: &Model) -> Result<GpuModel, String> {
        let device = &gpu.device;
        let mut materials = Vec::with_capacity(model.materials.len().max(1));
        for m in model.materials.iter().map(Some).chain([None]) {
            // One extra white material at the end for draws whose material index is out of range.
            let tex = match (m.and_then(|m| m.image.as_ref()), m.and_then(|m| m.texture.as_deref())) {
                (Some(img), _) => Arc::new(self.upload_image(gpu, img, &img.path)),
                (None, Some(p)) => self.texture(gpu, vfs, p)?,
                (None, None) => self.white.clone(),
            };
            let ((blend, depth_write), layer, alpha, wrap) = match m.and_then(|m| m.surface) {
                Some(s) => {
                    let blend = match s.blend {
                        _ if !s.blends() => Blend::Opaque,
                        fpm::blend::SRCALPHA_LIGHTMAP_KEEP => Blend::Lightmap,
                        _ => Blend::Alpha,
                    };
                    let alpha = if s.alpha_test { ALPHA_TESTED } else { ALPHA_KEPT };
                    ((blend, s.depth_write), u16::from(s.layer), alpha, s.wrap)
                }
                // an edited model's material, or the fallback: blended when its texture has alpha, after the rest
                None if m.is_some() && tex.has_alpha => ((Blend::Alpha, false), LAST_LAYER, ALPHA_KEPT, [true; 2]),
                None => ((Blend::Opaque, true), 0, ALPHA_HALF, [true; 2]),
            };
            // a name with a Groper material draws as its stage says, not as its flags (`groper::Render`); the
            // layer and the wrap stay the flags'
            let groper = m.and_then(|m| m.groper);
            let (blend, depth_write, alpha, alpha_min) = match groper {
                Some(g) => {
                    let blend = match g.blend {
                        fpm::blend::COPYRGB | fpm::blend::COPYRGB_KEEP => Blend::Opaque,
                        fpm::blend::SRCALPHA_LIGHTMAP_KEEP | 21 => Blend::Lightmap,
                        19 => Blend::DepthOnly,
                        _ if g.adds() => Blend::Additive,
                        _ => Blend::Alpha,
                    };
                    // not the original's: a blended cut-out keeps the flags' test where it is the stricter (the
                    // stage's `GT0` lets the mips' faded edges blend and write depth: the trees wash out)
                    let flags_test = m.and_then(|m| m.surface).is_some_and(|s| s.alpha_test);
                    let alpha_min = match g.alpha_min {
                        Some(a) if blend == Blend::Alpha && flags_test => Some(a.max(fpm::ALPHA_REF)),
                        a => a,
                    };
                    let alpha = if alpha_min.is_some() { ALPHA_TESTED } else { ALPHA_KEPT };
                    (blend, g.depth_write, alpha, alpha_min.unwrap_or(0))
                }
                None => (blend, depth_write, alpha, fpm::ALPHA_REF),
            };
            let zfill = groper.is_some_and(|g| g.zfill);
            let textured = m.is_some_and(|m| m.texture.is_some() || m.image.is_some());
            // `w`: lit (every model is until [`Renderer::set_lit`] says otherwise), and a lightmap
            let kind = KIND_LIT | if blend == Blend::Lightmap { KIND_LIGHTMAP } else { 0 };
            let flags = [u32::from(textured), alpha, u32::from(alpha_min), kind];
            let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("material"),
                contents: bytemuck::bytes_of(&MaterialUniform { flags, uv: UV_IDENTITY }),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            let sampler = usize::from(wrap[0]) | usize::from(wrap[1]) << 1;
            let name = m.map_or("fallback", |m| m.name.as_str());
            let texture_path = m.and_then(|m| m.image.as_ref().map(|i| i.path.as_str()).or(m.texture.as_deref()));
            let shader = self.shaders.material_for(name, texture_path);
            let bind_group = self.material_group(device, name, &tex, sampler, &uniform);
            materials.push(GpuMaterial {
                bind_group,
                blend,
                depth_write,
                zfill,
                layer,
                name: name.to_string(),
                texture: tex,
                uniform,
                sampler,
                flags,
                uv: UV_IDENTITY,
                animation: None,
                shader,
            });
        }
        let fallback = materials.len() - 1;
        let mut meshes = Vec::new();
        for mesh in model.meshes.iter().filter(|m| m.level == 0) {
            let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("vertices"),
                contents: bytemuck::cast_slice(&vertices(mesh)),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
            let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("indices"),
                contents: bytemuck::cast_slice(&mesh.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            let material = if mesh.material < fallback { mesh.material } else { fallback };
            let two_sided = mesh.two_sided;
            meshes.push(GpuMesh { vertices, indices, count: mesh.indices.len() as u32, material, two_sided });
        }
        let skinned = model.skeleton.is_some();
        let bone_count = model.skeleton.as_ref().map_or(1, |(_, s)| s.bones.len()).max(1);
        let identity = Mat4::IDENTITY.to_cols_array();
        let initial: Vec<[f32; 16]> = if skinned { vec![identity; bone_count] } else { vec![model.matrix] };
        let bones = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bones"),
            contents: bytemuck::cast_slice(&initial),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let bones_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bones"),
            layout: &self.bones_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: bones.as_entire_binding() }],
        });
        Ok(GpuModel {
            meshes,
            materials,
            bones,
            bones_group,
            bone_count,
            skinned,
            matrix: model.matrix,
            bounds: model.bounds(),
            path: model.path.clone(),
            casts_shadow: true,
        })
    }

    fn material_group(
        &self,
        device: &wgpu::Device,
        name: &str,
        tex: &GpuTexture,
        sampler: usize,
        uniform: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(name),
            layout: &self.material_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&tex.view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.samplers[sampler]) },
                wgpu::BindGroupEntry { binding: 2, resource: uniform.as_entire_binding() },
            ],
        })
    }

    /// Whether `model`'s materials take the [`Lights`] (the characters) or only their vertex colour (the stadium,
    /// whose materials the original draws unlit: a default material leaves the lit bit clear, `0x507670`).
    pub fn set_lit(&self, gpu: &Gpu, model: &mut GpuModel, lit: bool) {
        self.set_kind(gpu, model, KIND_LIT, lit);
    }

    /// Mark `model` as the sky: the remastered lighting ([`Look::Remastered`]) draws it as it is, unshadowed, and it
    /// casts no shadow. The original's draw is unchanged.
    pub fn set_sky(&self, gpu: &Gpu, model: &mut GpuModel) {
        self.set_kind(gpu, model, KIND_SKY, true);
        model.casts_shadow = false;
    }

    /// Have the remastered lighting draw `model`'s materials whose names `which` picks as the original does, in
    /// their vertex colours alone, casting no shadow: a stadium's reflections (painted, or its buildings mirrored
    /// under its water), which a sun would light as if they were there.
    pub fn keep_original(&self, gpu: &Gpu, model: &mut GpuModel, which: &dyn Fn(&str) -> bool) {
        for m in model.materials.iter_mut().filter(|m| which(&m.name)) {
            m.flags[3] |= KIND_AS_ORIGINAL;
            gpu.queue.write_buffer(&m.uniform, 0, bytemuck::bytes_of(&MaterialUniform { flags: m.flags, uv: m.uv }));
        }
    }

    fn set_kind(&self, gpu: &Gpu, model: &mut GpuModel, bit: u32, on: bool) {
        for m in &mut model.materials {
            m.flags[3] = if on { m.flags[3] | bit } else { m.flags[3] & !bit };
            gpu.queue.write_buffer(&m.uniform, 0, bytemuck::bytes_of(&MaterialUniform { flags: m.flags, uv: m.uv }));
        }
    }

    /// Animate `model`'s material number `material` by a Groper stage: `frames` are its `animmap` frames' texture
    /// paths in `vfs` (one, or none, for a stage that only has `tcMod`s; a missing one keeps the material's own).
    /// [`Renderer::animate`] then shows them.
    pub fn set_animation(
        &mut self,
        gpu: &Gpu,
        vfs: &Vfs,
        model: &mut GpuModel,
        material: usize,
        stage: groper::Stage,
        frames: &[Option<String>],
    ) {
        let Some(m) = model.materials.get(material) else { return };
        let mut groups = Vec::new();
        for f in frames {
            let tex = f.as_deref().and_then(|p| self.texture(gpu, vfs, p).ok()).unwrap_or_else(|| m.texture.clone());
            groups.push(self.material_group(&gpu.device, &m.name, &tex, m.sampler, &m.uniform));
        }
        model.materials[material].animation = Some(Animation { stage, frames: groups });
    }

    /// Show each animated material of `model` as it is `t` seconds into the game (the engine's clock,
    /// `[0x64cbf0]`): its `animmap` frame and its texture matrix.
    pub fn animate(&self, gpu: &Gpu, model: &mut GpuModel, t: f32) {
        for m in &mut model.materials {
            let Some(a) = &m.animation else { continue };
            if let Some(g) = a.frames.get(a.stage.frame(t)) {
                m.bind_group = g.clone();
            }
            if !a.stage.tcmods.is_empty() {
                let r = a.stage.uv(t);
                m.uv = [[r[0][0], r[0][1], r[0][2], 0.0], [r[1][0], r[1][1], r[1][2], 0.0]];
                gpu.queue.write_buffer(
                    &m.uniform,
                    0,
                    bytemuck::bytes_of(&MaterialUniform { flags: m.flags, uv: m.uv }),
                );
            }
        }
    }

    /// Whether any material of `model` animates.
    pub fn animates(model: &GpuModel) -> bool {
        model.materials.iter().any(|m| m.animation.is_some())
    }

    /// Draw `model`'s materials named `material` (case aside) with the texture at `path` in `vfs` instead of their
    /// own, or with their own again for `None` (the front end's `General_SetQuidditchCardTexture`). Answers whether
    /// the model has such a material.
    pub fn set_material_texture(
        &mut self,
        gpu: &Gpu,
        vfs: &Vfs,
        model: &mut GpuModel,
        material: &str,
        path: Option<&str>,
    ) -> Result<bool, String> {
        let tex = match path {
            Some(p) => Some(self.texture(gpu, vfs, p)?),
            None => None,
        };
        let mut found = false;
        for i in 0..model.materials.len() {
            if !model.materials[i].name.eq_ignore_ascii_case(material) {
                continue;
            }
            found = true;
            let m = &model.materials[i];
            let t = tex.clone().unwrap_or_else(|| m.texture.clone());
            let group = self.material_group(&gpu.device, &m.name, &t, m.sampler, &m.uniform);
            model.materials[i].bind_group = group;
        }
        Ok(found)
    }

    /// Set a skinned model's pose: one skinning matrix per bone (`qwc_anim::Rig::skin`), row-vector layout.
    pub fn set_pose(&self, queue: &wgpu::Queue, model: &GpuModel, skin: &[[f64; 16]]) {
        if !model.skinned {
            return;
        }
        let mats: Vec<[f32; 16]> = skin.iter().take(model.bone_count).map(row_major).collect();
        queue.write_buffer(&model.bones, 0, bytemuck::cast_slice(&mats));
    }

    /// Place an unskinned model with `matrix` (row-vector layout) instead of its root matrix.
    pub fn set_transform(&self, queue: &wgpu::Queue, model: &GpuModel, matrix: Option<[f32; 16]>) {
        if !model.skinned {
            queue.write_buffer(&model.bones, 0, bytemuck::bytes_of(&matrix.unwrap_or(model.matrix)));
        }
    }

    /// Give uploaded mesh `index` (of the finest level's, in order) new vertices, as many as it has (a cloth's,
    /// each frame); its indices and material stay. A mesh of another size is left.
    pub fn set_vertices(&self, queue: &wgpu::Queue, model: &GpuModel, index: usize, mesh: &qwc_assets::model::Mesh) {
        self.set_positions(queue, model, index, mesh, &mesh.positions);
    }

    /// [`Renderer::set_vertices`] with `mesh` at `positions` (a morphed face's, each frame).
    pub fn set_positions(
        &self,
        queue: &wgpu::Queue,
        model: &GpuModel,
        index: usize,
        mesh: &qwc_assets::model::Mesh,
        positions: &[[f32; 3]],
    ) {
        let verts = vertices_at(mesh, positions);
        if let Some(m) =
            model.meshes.get(index).filter(|m| m.vertices.size() == std::mem::size_of_val(&verts[..]) as u64)
        {
            queue.write_buffer(&m.vertices, 0, bytemuck::cast_slice(&verts));
        }
    }

    /// Make the remastered lighting's light probes ([`remaster`]): a grid of `count` probes over `lo..hi`, each
    /// seeing `models` (the stadium and its sky) in the sun and its shadows. The lit models then take their ambient
    /// light from the probes around them. Nothing is done in the original look.
    pub fn bake_probes(&self, gpu: &Gpu, models: &[&GpuModel], lo: [f32; 3], hi: [f32; 3], count: [u32; 3]) {
        if self.look != Look::Remastered {
            return;
        }
        let lighting = self.lighting.get_or_init(|| remaster::Lighting::new(self, &gpu.device));
        lighting.bake(self, gpu, models, lo.into(), hi.into(), count);
    }

    /// Draw `models` into `color`/`depth`, clearing both.
    pub fn draw(
        &self,
        gpu: &Gpu,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        size: (u32, u32),
        camera: &Camera,
        models: &[&GpuModel],
    ) {
        self.draw_rect(gpu, color, depth, [0, 0, size.0, size.1], true, camera, models);
    }

    /// Draw `models` into the rectangle `rect` (`x, y, width, height` in pixels) of `color`/`depth`, the camera's
    /// aspect the rectangle's: a split screen's view. `clear` clears the whole of both targets first (the first view
    /// drawn), else what is there is kept.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_rect(
        &self,
        gpu: &Gpu,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        rect: [u32; 4],
        clear: bool,
        camera: &Camera,
        models: &[&GpuModel],
    ) {
        if self.look == Look::Remastered {
            let lighting = self.lighting.get_or_init(|| remaster::Lighting::new(self, &gpu.device));
            lighting.draw(self, gpu, color, depth, rect, clear, camera, models);
            return;
        }
        let aspect = rect[2] as f32 / rect[3].max(1) as f32;
        let l = &self.lights;
        let v4 = |v: [f32; 3]| [v[0], v[1], v[2], 0.0];
        let uniform = FrameUniform {
            view_proj: camera.view_proj(aspect).to_cols_array(),
            ambient: v4(l.ambient),
            dirs: l.lights.map(|(_, d)| v4(d)),
            colours: l.lights.map(|(c, _)| v4(c)),
        };
        gpu.queue.write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&uniform));
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("models"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if clear { wgpu::LoadOp::Clear(self.clear) } else { wgpu::LoadOp::Load },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth,
                    depth_ops: Some(wgpu::Operations {
                        load: if clear { wgpu::LoadOp::Clear(1.0) } else { wgpu::LoadOp::Load },
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_viewport(rect[0] as f32, rect[1] as f32, rect[2] as f32, rect[3] as f32, 0.0, 1.0);
            pass.set_scissor_rect(rect[0], rect[1], rect[2], rect[3]);
            pass.set_bind_group(0, &self.frame_group, &[]);
            draw_layers(&mut pass, &self.pipelines, &[], models);
        }
        gpu.queue.submit([encoder.finish()]);
    }
}

/// Draw `models` layer by layer across every model, as the engine sorts its draws (`0x4a57b4`), each with its
/// key's pipeline of `pipelines`, or of `custom[shader - 1]` for a material a pack's shader draws (where that one
/// was made); the frame's bind group (0) and any after the material's are the caller's. A material with a `zfill`
/// stage draws its faces' depth first, then its colour (`0x4ac9fe` runs a draw's stages in turn).
fn draw_layers(
    pass: &mut wgpu::RenderPass<'_>,
    pipelines: &HashMap<PipelineKey, wgpu::RenderPipeline>,
    custom: &[Option<HashMap<PipelineKey, wgpu::RenderPipeline>>],
    models: &[&GpuModel],
) {
    let mut layers: Vec<u16> = models.iter().flat_map(|m| m.materials.iter().map(|x| x.layer)).collect();
    layers.sort_unstable();
    layers.dedup();
    let mut current = None;
    for layer in layers {
        for m in models {
            pass.set_bind_group(1, &m.bones_group, &[]);
            for mesh in m.meshes.iter().filter(|me| m.materials[me.material].layer == layer) {
                let material = &m.materials[mesh.material];
                let set = match custom.get(usize::from(material.shader).wrapping_sub(1)) {
                    Some(Some(own)) => (material.shader, own),
                    _ => (0, pipelines),
                };
                pass.set_bind_group(2, &material.bind_group, &[]);
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                let depth = material.zfill.then_some((Blend::DepthOnly, true, mesh.two_sided));
                for key in depth.into_iter().chain([(material.blend, material.depth_write, mesh.two_sided)]) {
                    if current != Some((set.0, key)) {
                        pass.set_pipeline(&set.1[&key]);
                        current = Some((set.0, key));
                    }
                    pass.draw_indexed(0..mesh.count, 0, 0..1);
                }
            }
        }
    }
}

/// Create a depth texture view for a target of `size` (which the remastered lighting's fog reads as well).
pub fn depth_view(device: &wgpu::Device, size: (u32, u32)) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d { width: size.0.max(1), height: size.1.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

/// An offscreen colour + depth target that can be read back as RGBA8.
pub struct Offscreen {
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    pub size: (u32, u32),
}

pub const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

impl Offscreen {
    pub fn new(device: &wgpu::Device, size: (u32, u32)) -> Offscreen {
        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let color_view = color.create_view(&Default::default());
        Offscreen { color, color_view, depth_view: depth_view(device, size), size }
    }

    pub fn render(&self, gpu: &Gpu, renderer: &Renderer, camera: &Camera, models: &[&GpuModel]) {
        renderer.draw(gpu, &self.color_view, &self.depth_view, self.size, camera, models);
    }

    /// Read the colour target back as tightly packed RGBA8 rows.
    pub fn read_rgba(&self, gpu: &Gpu) -> Result<Vec<u8>, String> {
        read_texture(gpu, &self.color, self.size)
    }
}

/// Read a 4-byte-per-texel texture of `size` back as tightly packed rows.
pub fn read_texture(gpu: &Gpu, texture: &wgpu::Texture, size: (u32, u32)) -> Result<Vec<u8>, String> {
    {
        let (w, h) = size;
        let row = (4 * w).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        gpu.queue.submit([encoder.finish()]);
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        gpu.device
            .poll(wgpu::PollType::Wait { submission_index: None, timeout: None })
            .map_err(|e| format!("GPU readback failed: {e}"))?;
        let data = slice.get_mapped_range().map_err(|e| format!("GPU readback failed: {e}"))?;
        let mut out = Vec::with_capacity((4 * w * h) as usize);
        for y in 0..h as usize {
            out.extend_from_slice(&data[y * row as usize..y * row as usize + 4 * w as usize]);
        }
        Ok(out)
    }
}
