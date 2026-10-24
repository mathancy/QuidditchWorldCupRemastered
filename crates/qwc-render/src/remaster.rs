//! The remastered lighting ([`Look::Remastered`]): not the original's, the remaster's own. The original draws the
//! stadium unlit, in its baked vertex colours and lightmaps, the characters in fixed-function lights, and no fog
//! (`RE_NOTES.md`, "The lights", "Fog"); [`Look::Original`] keeps that.
//!
//! The remaster draws in linear light into an HDR target, then fogs and tone maps it into the caller's:
//! - **The sun:** the stadium's light set's `Sun` ([`crate::Lights`]), its colour times [`Settings::sun`], casting
//!   shadows from three cascaded shadow maps (`shadow.wgsl`) fitted around the camera's view, each snapped to its
//!   texels so they do not shimmer as the camera moves. Every model casts but the sky ([`crate::Renderer::set_sky`]);
//!   alpha-tested materials cut their holes.
//! - **The stadium:** lit by the sun on normals made from its geometry ([`crate::normals`]); its vertex colours,
//!   the original's baked light, stay as its indirect light, and its lightmaps are still drawn over it, both in
//!   linear light, so with the sun off and no fog it draws as the original does.
//! - **The characters (lit materials):** the sun, a sky and ground ambient, and the light set's `Fill`.
//! - **The sky:** as the original draws it, as are the materials [`crate::Renderer::keep_original`] names (a
//!   stadium's reflections).
//! - **Fog:** a height fog (thinner as it rises), brighter looking towards the sun (`composite.wgsl`).
//! - **Tone mapping:** Khronos' PBR Neutral's shoulder (its toe left out), which keeps the art's colours below its
//!   highlights: with the sun off (`sun=0`) and no fog the remaster draws as the original does.
//!
//! The settings can be changed with [`Settings::apply`] (the game's `QWC_REMASTER`, `sun=2,fog=0.002,...`).
//!
//! The packs' shaders ([`crate::custom`]) draw here: a material shader's pipelines stand in for the stock ones on
//! its materials, and the post passes run after the tone mapping, the composite then drawing into a float target
//! they take turns on, the last step putting their picture into the caller's.

use std::sync::Mutex;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3, Vec4};

use crate::custom::{self, PARAMS};
use crate::{
    draw_layers, model_pipelines, vertex_layout, Blend, Camera, Gpu, GpuModel, PipelineKey, Renderer, ALPHA_KEPT,
    DEPTH_FORMAT, KIND_AS_ORIGINAL, KIND_LIGHTMAP, KIND_SKY,
};

/// How the models are lit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Look {
    /// As the original draws them.
    #[default]
    Original,
    /// The remaster's lighting ([`crate::remaster`]).
    Remastered,
}

impl Look {
    pub fn parse(s: &str) -> Option<Look> {
        match s.to_ascii_lowercase().as_str() {
            "original" => Some(Look::Original),
            "remastered" | "remaster" => Some(Look::Remastered),
            _ => None,
        }
    }
}

/// The remastered lighting's settings (linear light; 1 is the original's full brightness), and the picture's colour
/// grade. A stadium's are kept in a look file (`qwc-game`'s `looks`); a field the file leaves out keeps its default.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The sun's colour is the light set's `Sun` times this, and times `sun_tint`.
    pub sun: f32,
    pub sun_tint: [f32; 3],
    /// The sun turned from the light set's, in degrees: round the sky (clockwise seen from above), and up.
    pub sun_turn: f32,
    pub sun_raise: f32,
    /// The ambient light from the sky (above) and the ground (below) on the lit models.
    pub sky: [f32; 3],
    pub ground: [f32; 3],
    /// The light set's `Fill` on the lit models, times this.
    pub fill: f32,
    /// The stadium's baked light (its vertex colours) times this, and the sun on it times `baked_sun`.
    pub baked: f32,
    pub baked_sun: f32,
    /// The sky's brightness.
    pub sky_gain: f32,
    /// How much of the stadium's lightmaps is drawn: 1 as the original, 0 none (their baked shadows left to the
    /// real sun's).
    pub lightmaps: f32,
    /// How far from the eye the shadows reach, and how dark they are (1 the sun's light all gone).
    pub shadow_distance: f32,
    pub shadow_strength: f32,
    /// How far from the stadium's middle the shadows' casters may be.
    pub scene_radius: f32,
    /// The fog's colour (mixed by `fog_sky` with the sky's mean, once the light probes have seen it); its density
    /// at `fog_base` (per unit), how fast it thins upwards (per unit), the most of it, and how far from the eye it
    /// starts.
    pub fog_colour: [f32; 3],
    pub fog_sky: f32,
    pub fog_density: f32,
    pub fog_base: f32,
    pub fog_falloff: f32,
    pub fog_max: f32,
    pub fog_start: f32,
    /// The picture's exposure before tone mapping, and its white balance (warmer above 0, cooler below).
    pub exposure: f32,
    pub temperature: f32,
    /// A colour filter over the whole picture (white: none).
    pub tint: [f32; 3],
    /// The colours' saturation (0 grey, 1 as they are) and the contrast about mid grey (1 as it is).
    pub saturation: f32,
    pub contrast: f32,
    /// After tone mapping, on the display's values, as a colourist's lift, gamma and gain: `lift` raises the
    /// shadows (0 none), `gamma` bends the middle (1 none, above brighter), `gain` scales the highlights (1 none).
    pub lift: [f32; 3],
    pub gamma: [f32; 3],
    pub gain: [f32; 3],
    /// How much the corners darken (0 none).
    pub vignette: f32,
    /// The ambient occlusion's reach in world units, its strength (0 off) and its samples a pixel.
    pub ao_radius: f32,
    pub ao_strength: f32,
    pub ao_samples: u32,
    /// The light probes' light on the lit models times this (0: the sky and ground ambient instead); they are
    /// made by [`crate::Renderer::bake_probes`].
    pub probes: f32,
    /// What is shown: 0 the picture, 1 the ambient occlusion, 2 the indirect light's share (`view=ao`, `share`).
    #[serde(skip)]
    pub view: u32,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            sun: 1.5,
            sun_tint: [1.0; 3],
            sun_turn: 0.0,
            sun_raise: 0.0,
            sky: [0.34, 0.40, 0.50],
            ground: [0.17, 0.16, 0.12],
            fill: 0.25,
            baked: 0.7,
            baked_sun: 0.85,
            sky_gain: 1.0,
            lightmaps: 1.0,
            shadow_distance: 320.0,
            shadow_strength: 0.8,
            scene_radius: 800.0,
            fog_colour: [0.60, 0.68, 0.80],
            fog_sky: 0.85,
            fog_density: 0.0008,
            fog_base: 0.0,
            fog_falloff: 0.012,
            fog_max: 0.7,
            fog_start: 30.0,
            exposure: 1.0,
            temperature: 0.0,
            tint: [1.0; 3],
            saturation: 1.0,
            contrast: 1.0,
            lift: [0.0; 3],
            gamma: [1.0; 3],
            gain: [1.0; 3],
            vignette: 0.0,
            ao_radius: 2.5,
            ao_strength: 0.8,
            ao_samples: 12,
            probes: 1.0,
            view: 0,
        }
    }
}

impl Settings {
    /// Change the settings named in `list`: `name=value` pairs split by commas, a colour's value `r/g/b`. The names
    /// are the fields'.
    pub fn apply(&mut self, list: &str) -> Result<(), String> {
        for pair in list.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (name, value) = pair.split_once('=').ok_or_else(|| format!("{pair}: no `=`"))?;
            let num = |v: &str| v.trim().parse::<f32>().map_err(|e| format!("{pair}: {e}"));
            let rgb = |v: &str| -> Result<[f32; 3], String> {
                let c: Vec<f32> = v.split('/').map(num).collect::<Result<_, _>>()?;
                match c[..] {
                    [g] => Ok([g; 3]),
                    [r, g, b] => Ok([r, g, b]),
                    _ => Err(format!("{pair}: a colour is r/g/b")),
                }
            };
            match name.trim() {
                "sun" => self.sun = num(value)?,
                "sun_tint" => self.sun_tint = rgb(value)?,
                "sun_turn" => self.sun_turn = num(value)?,
                "sun_raise" => self.sun_raise = num(value)?,
                "temperature" => self.temperature = num(value)?,
                "tint" => self.tint = rgb(value)?,
                "saturation" => self.saturation = num(value)?,
                "contrast" => self.contrast = num(value)?,
                "lift" => self.lift = rgb(value)?,
                "gamma" => self.gamma = rgb(value)?,
                "gain" => self.gain = rgb(value)?,
                "vignette" => self.vignette = num(value)?,
                "sky" => self.sky = rgb(value)?,
                "ground" => self.ground = rgb(value)?,
                "fill" => self.fill = num(value)?,
                "baked" => self.baked = num(value)?,
                "baked_sun" => self.baked_sun = num(value)?,
                "sky_gain" => self.sky_gain = num(value)?,
                "lightmaps" => self.lightmaps = num(value)?,
                "shadow_distance" => self.shadow_distance = num(value)?,
                "shadow_strength" => self.shadow_strength = num(value)?,
                "scene_radius" => self.scene_radius = num(value)?,
                "fog_colour" | "fog_color" => self.fog_colour = rgb(value)?,
                "fog" | "fog_density" => self.fog_density = num(value)?,
                "fog_sky" => self.fog_sky = num(value)?,
                "fog_base" => self.fog_base = num(value)?,
                "fog_falloff" => self.fog_falloff = num(value)?,
                "fog_max" => self.fog_max = num(value)?,
                "fog_start" => self.fog_start = num(value)?,
                "exposure" => self.exposure = num(value)?,
                "ao_radius" => self.ao_radius = num(value)?,
                "ao" | "ao_strength" => self.ao_strength = num(value)?,
                "ao_samples" => self.ao_samples = num(value)?.clamp(1.0, 64.0) as u32,
                "probes" => self.probes = num(value)?,
                "view" => {
                    self.view = match value.trim() {
                        "picture" => 0,
                        "ao" => 1,
                        "share" => 2,
                        v => return Err(format!("{v}: the views are picture, ao and share")),
                    }
                }
                other => return Err(format!("{other}: no such setting")),
            }
        }
        Ok(())
    }
}

/// The shadow maps' size, as `remaster.wgsl`'s `SHADOW_SIZE`.
const SHADOW_SIZE: u32 = 2048;
const CASCADES: usize = 3;
/// The cascades' far ends, as parts of the shadows' reach.
const SPLITS: [f32; CASCADES] = [0.06, 0.25, 1.0];
const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameUniform {
    view_proj: [f32; 16],
    eye: [f32; 4],
    sun_dir: [f32; 4],
    sun_colour: [f32; 4],
    sky: [f32; 4],
    ground: [f32; 4],
    fill_dir: [f32; 4],
    fill_colour: [f32; 4],
    baked: [f32; 4],
    cascades: [[f32; 16]; CASCADES],
    splits: [f32; 4],
    texels: [f32; 4],
    probe_lo: [f32; 4],
    probe_step: [f32; 4],
    probe_count: [f32; 4],
    time: [f32; 4],
}

/// The light probes' grid as `remaster.wgsl` takes it: none.
const NO_PROBES: [[f32; 4]; 3] = [[0.0; 4], [1.0; 4], [1.0; 4]];

/// A probe's cube faces' size in texels, as `probes.wgsl` takes it.
const PROBE_FACE: u32 = 16;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BakeUniform {
    inv_faces: [[f32; 16]; 6],
    size: [u32; 4],
}

/// The light probes made by [`crate::Renderer::bake_probes`]: their coefficients (4 texels a probe, a probe a row,
/// the grid's and then one seeing only the sky, whose mean is the fog's colour) and their grid.
struct Probes {
    view: wgpu::TextureView,
    grid: [[f32; 4]; 3],
    /// The sky's probe's row.
    sky: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PostUniform {
    inv_view_proj: [f32; 16],
    eye: [f32; 4],
    sun_dir: [f32; 4],
    sun_colour: [f32; 4],
    fog_colour: [f32; 4],
    fog: [f32; 4],
    tone: [f32; 4],
    rect: [f32; 4],
    ao: [f32; 4],
    /// x: the sky's probe's row; y: how much of the fog's colour is the sky's.
    sky: [f32; 4],
    /// The grade: x temperature, y saturation, z contrast, w vignette; the filter; lift, gamma and gain.
    grade: [f32; 4],
    tint: [f32; 4],
    lift: [f32; 4],
    gamma: [f32; 4],
    gain: [f32; 4],
}

/// A post pass's uniform (`post.wgsl`'s `Post`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PassUniform {
    inv_view_proj: [f32; 16],
    eye: [f32; 4],
    rect: [f32; 4],
    size: [f32; 4],
    time: [f32; 4],
    params: [f32; PARAMS],
    /// x: 1 = the last step encodes for a target holding stored values.
    engine: [f32; 4],
}

/// The packs' post passes made ([`custom::Shaders::post`]): each pass's pipeline, uniform and numbers; the
/// composite into a float target for them, and the last step into the caller's.
struct PostChain {
    passes: Vec<(wgpu::RenderPipeline, wgpu::Buffer, [f32; PARAMS])>,
    composite: wgpu::RenderPipeline,
    present: wgpu::RenderPipeline,
    present_buffer: wgpu::Buffer,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

/// Run `make` with the device's errors caught: a pack's shader the GPU refuses is reported (as from `name`) and
/// answers none, so the stock one draws.
fn guarded<T>(device: &wgpu::Device, name: &str, make: impl FnOnce() -> T) -> Option<T> {
    let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let made = make();
    let errors = [pollster::block_on(validation.pop()), pollster::block_on(internal.pop())];
    match errors.into_iter().flatten().next() {
        Some(e) => {
            eprintln!("{name}: {e}; the stock shader draws instead");
            None
        }
        None => Some(made),
    }
}

/// The share of each pixel's light that is indirect (`remaster.wgsl`), and the ambient occlusion (`ssao.wgsl`).
const SHARE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;

/// The targets the size of the caller's: the HDR picture, the indirect light's share, the ambient occlusion.
struct Targets {
    size: wgpu::Extent3d,
    hdr: wgpu::TextureView,
    share: wgpu::TextureView,
    ao: wgpu::TextureView,
    /// The post passes' two float targets, when there are passes.
    float: Option<[wgpu::TextureView; 2]>,
}

/// The pipelines, the shadow maps and the HDR target.
pub(crate) struct Lighting {
    pipelines: std::collections::HashMap<crate::PipelineKey, wgpu::RenderPipeline>,
    /// The packs' material shaders' pipelines, as [`custom::Shaders::materials`] (none: it was refused).
    custom: Vec<Option<std::collections::HashMap<PipelineKey, wgpu::RenderPipeline>>>,
    post_chain: Option<PostChain>,
    shadow_solid: wgpu::RenderPipeline,
    shadow_cut: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    ssao: wgpu::RenderPipeline,
    ssao_layout: wgpu::BindGroupLayout,
    frame_buffer: wgpu::Buffer,
    frame_group: wgpu::BindGroup,
    cascades: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
    shadow_layers: Vec<wgpu::TextureView>,
    shadow_layout: wgpu::BindGroupLayout,
    shadow_array: wgpu::TextureView,
    compare: wgpu::Sampler,
    /// The shadow maps and the probes' coefficients (a black probe until they are baked).
    shadow_group: Mutex<wgpu::BindGroup>,
    composite_layout: wgpu::BindGroupLayout,
    post_buffer: wgpu::Buffer,
    targets: Mutex<Option<Targets>>,
    probe_pipeline: wgpu::RenderPipeline,
    probe_layout: wgpu::BindGroupLayout,
    probes: Mutex<Option<Probes>>,
    /// A probe of black, bound before any are baked.
    black: wgpu::TextureView,
}

fn v4(v: impl Into<Vec3>, w: f32) -> [f32; 4] {
    v.into().extend(w).to_array()
}

impl Lighting {
    pub(crate) fn new(r: &Renderer, device: &wgpu::Device) -> Lighting {
        let module = |label: &str, src: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(src.into()),
            })
        };
        let lit = module("remaster", &format!("{}\n{}", custom::MATERIAL_INTERFACE, custom::MATERIAL_STOCK));
        let shadow = module("shadow", include_str!("shadow.wgsl"));
        let post = module("composite", include_str!("composite.wgsl"));
        let ssao = module("ssao", include_str!("ssao.wgsl"));
        let probe_module = module("probes", include_str!("probes.wgsl"));
        let entry = |binding, visibility, ty| wgpu::BindGroupLayoutEntry { binding, visibility, ty, count: None };
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let shadow_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadows"),
            entries: &[
                entry(
                    0,
                    fragment,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                ),
                entry(1, fragment, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison)),
                entry(
                    2,
                    fragment,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("remaster"),
            bind_group_layouts: &[
                Some(&r.frame_layout),
                Some(&r.bones_layout),
                Some(&r.material_layout),
                Some(&shadow_layout),
            ],
            immediate_size: 0,
        });
        let pipelines = model_pipelines(device, &layout, &lit, HDR_FORMAT, &[SHARE_FORMAT]);
        let custom = r
            .shaders
            .materials
            .iter()
            .map(|m| {
                if let Err(e) = custom::check_material(&m.source) {
                    eprintln!("{}: {e}; the stock shader draws its materials", m.name);
                    return None;
                }
                guarded(device, &m.name, || {
                    let own = module(&m.name, &format!("{}\n{}", custom::MATERIAL_INTERFACE, m.source));
                    model_pipelines(device, &layout, &own, HDR_FORMAT, &[SHARE_FORMAT])
                })
            })
            .collect();
        let shadow_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shadow"),
            bind_group_layouts: &[Some(&r.frame_layout), Some(&r.bones_layout), Some(&r.material_layout)],
            immediate_size: 0,
        });
        let shadow_pipeline = |cut: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(if cut { "shadow, cut" } else { "shadow" }),
                layout: Some(&shadow_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shadow,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(vertex_layout())],
                },
                // both sides cast: much of the stadium is single sheets
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: wgpu::DepthBiasState { constant: 2, slope_scale: 2.0, clamp: 0.0 },
                }),
                multisample: Default::default(),
                fragment: cut.then(|| wgpu::FragmentState {
                    module: &shadow,
                    entry_point: Some("fs_cut"),
                    compilation_options: Default::default(),
                    targets: &[],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let (shadow_solid, shadow_cut) = (shadow_pipeline(false), shadow_pipeline(true));
        let texture = |binding, sample_type| {
            entry(
                binding,
                fragment,
                wgpu::BindingType::Texture {
                    sample_type,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
            )
        };
        let post_uniform = |binding| {
            entry(
                binding,
                fragment,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            )
        };
        let float = wgpu::TextureSampleType::Float { filterable: false };
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("composite"),
            entries: &[
                texture(0, float),
                texture(1, wgpu::TextureSampleType::Depth),
                post_uniform(2),
                texture(3, float),
                texture(4, float),
                texture(5, float),
            ],
        });
        let ssao_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ssao"),
            entries: &[texture(0, wgpu::TextureSampleType::Depth), post_uniform(1)],
        });
        let composite = fullscreen(device, &post, &composite_layout, r.color_format, "composite");
        let ssao = fullscreen(device, &ssao, &ssao_layout, SHARE_FORMAT, "ssao");
        let probe_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("probes"),
            entries: &[texture(0, float), post_uniform(1)],
        });
        let probe_pipeline = fullscreen(device, &probe_module, &probe_layout, HDR_FORMAT, "probes");
        let uniform = |label, size: usize| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let group = |layout: &wgpu::BindGroupLayout, buffer: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
            })
        };
        let frame_buffer = uniform("remaster frame", std::mem::size_of::<FrameUniform>());
        let frame_group = group(&r.frame_layout, &frame_buffer);
        let cascades = (0..CASCADES)
            .map(|_| {
                let b = uniform("cascade", 64);
                let g = group(&r.frame_layout, &b);
                (b, g)
            })
            .collect();
        let shadow_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow maps"),
            size: wgpu::Extent3d { width: SHADOW_SIZE, height: SHADOW_SIZE, depth_or_array_layers: CASCADES as u32 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let shadow_layers = (0..CASCADES as u32)
            .map(|i| {
                shadow_tex.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: i,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let shadow_array = shadow_tex.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let compare = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let black = probe_texture(device, 1).create_view(&Default::default());
        let shadow_group = shadow_group(device, &shadow_layout, &shadow_array, &compare, &black);
        let post_buffer = uniform("post", std::mem::size_of::<PostUniform>());
        let post_chain = (!r.shaders.post.is_empty()).then(|| {
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("post pass"),
                entries: &[
                    texture(0, wgpu::TextureSampleType::Float { filterable: true }),
                    entry(1, fragment, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
                    texture(2, wgpu::TextureSampleType::Depth),
                    post_uniform(3),
                ],
            });
            let pass_uniform = || uniform("post pass", std::mem::size_of::<PassUniform>());
            let passes = r
                .shaders
                .post
                .iter()
                .filter_map(|p| {
                    if let Err(e) = custom::check_post(&p.source) {
                        eprintln!("{}: {e}; the pass is left out", p.name);
                        return None;
                    }
                    let source = format!("{}\n{}", custom::POST_INTERFACE, p.source);
                    let pipeline = guarded(device, &p.name, || {
                        fullscreen(device, &module(&p.name, &source), &layout, HDR_FORMAT, &p.name)
                    })?;
                    Some((pipeline, pass_uniform(), p.params))
                })
                .collect::<Vec<_>>();
            let present = module("present", &format!("{}\n{}", custom::POST_INTERFACE, custom::POST_PRESENT));
            PostChain {
                composite: fullscreen(device, &post, &composite_layout, HDR_FORMAT, "composite, for the post passes"),
                present: fullscreen(device, &present, &layout, r.color_format, "present"),
                present_buffer: pass_uniform(),
                sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                    label: Some("post pass"),
                    mag_filter: wgpu::FilterMode::Linear,
                    min_filter: wgpu::FilterMode::Linear,
                    ..Default::default()
                }),
                layout,
                passes,
            }
        });
        let post_chain = post_chain.filter(|c| !c.passes.is_empty());
        Lighting {
            pipelines,
            custom,
            post_chain,
            shadow_solid,
            shadow_cut,
            composite,
            ssao,
            ssao_layout,
            frame_buffer,
            frame_group,
            cascades,
            shadow_layers,
            shadow_layout,
            shadow_array,
            compare,
            shadow_group: Mutex::new(shadow_group),
            composite_layout,
            post_buffer,
            targets: Mutex::new(None),
            probe_pipeline,
            probe_layout,
            probes: Mutex::new(None),
            black,
        }
    }

    /// The frame's uniform for a view `view_proj` from `eye`, the shadows' cascades given.
    fn frame(
        &self,
        r: &Renderer,
        view_proj: Mat4,
        eye: Vec3,
        (cascades, splits, texels): ([Mat4; CASCADES], [f32; CASCADES], [f32; CASCADES]),
        probes: bool,
    ) -> FrameUniform {
        let s = &r.settings;
        let (sun_colour, sun_dir) = sun(r);
        let (fill_colour, fill_dir) = r.lights.lights[1];
        let grid = match &*self.probes.lock().unwrap_or_else(|e| e.into_inner()) {
            Some(p) if probes && s.probes > 0.0 => {
                let mut g = p.grid;
                g[0][3] = s.probes;
                g
            }
            _ => NO_PROBES,
        };
        FrameUniform {
            view_proj: view_proj.to_cols_array(),
            eye: v4(eye, 1.0),
            sun_dir: v4(sun_dir, 0.0),
            sun_colour: v4(sun_colour, 0.0),
            sky: v4(s.sky, 0.0),
            ground: v4(s.ground, 0.0),
            fill_dir: v4(Vec3::from(fill_dir).normalize_or_zero(), 0.0),
            fill_colour: v4(Vec3::from(fill_colour) * s.fill, 0.0),
            baked: [s.baked, s.baked_sun, f32::from(!r.texture_format.is_srgb()), s.sky_gain],
            cascades: cascades.map(|m| m.to_cols_array()),
            splits: [splits[0], splits[1], splits[2], s.shadow_strength],
            texels: [texels[0], texels[1], texels[2], s.lightmaps],
            probe_lo: grid[0],
            probe_step: grid[1],
            probe_count: grid[2],
            time: [r.time(), 0.0, 0.0, 0.0],
        }
    }

    /// Draw the shadow casters of `models` into shadow map `layer`, from the cascade whose bind group is `cascade`.
    fn draw_shadows(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        layer: usize,
        cascade: &wgpu::BindGroup,
        models: &[&GpuModel],
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("shadow"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.shadow_layers[layer],
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        pass.set_bind_group(0, cascade, &[]);
        let mut cut = None;
        for m in models.iter().filter(|m| m.casts_shadow) {
            pass.set_bind_group(1, &m.bones_group, &[]);
            for mesh in &m.meshes {
                let material = &m.materials[mesh.material];
                if material.flags[3] & (KIND_SKY | KIND_LIGHTMAP | KIND_AS_ORIGINAL) != 0 {
                    continue;
                }
                let cuts = material.flags[1] != ALPHA_KEPT || matches!(material.blend, Blend::Alpha | Blend::Additive);
                if cut != Some(cuts) {
                    pass.set_pipeline(if cuts { &self.shadow_cut } else { &self.shadow_solid });
                    cut = Some(cuts);
                }
                pass.set_bind_group(2, &material.bind_group, &[]);
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.count, 0, 0..1);
            }
        }
    }

    /// [`Renderer::bake_probes`].
    pub(crate) fn bake(&self, r: &Renderer, gpu: &Gpu, models: &[&GpuModel], lo: Vec3, hi: Vec3, count: [u32; 3]) {
        let device = &gpu.device;
        let count = count.map(|c| c.max(1));
        let step = (hi - lo) / (Vec3::new(count[0] as f32, count[1] as f32, count[2] as f32) - 1.0).max(Vec3::ONE);
        let places: Vec<Vec3> = (0..count[2])
            .flat_map(|z| (0..count[1]).flat_map(move |y| (0..count[0]).map(move |x| (x, y, z))))
            .map(|(x, y, z)| lo + step * Vec3::new(x as f32, y as f32, z as f32))
            .collect();
        // and one in the middle that sees only the sky
        let mut places = places;
        places.push((lo + hi) * 0.5);
        let sky_row = places.len() as u32 - 1;
        let sky_only: Vec<&GpuModel> = models.iter().filter(|m| !m.casts_shadow).copied().collect();
        let n = places.len() as u32;
        // the sun's shadows over the whole scene, in one map
        let (lo_all, hi_all) = scene_bounds(models).unwrap_or((lo, hi));
        let centre = (lo_all + hi_all) * 0.5;
        let radius = ((hi_all - lo_all).length() * 0.5).max(1.0);
        let (_, sun_dir) = sun(r);
        let view = Mat4::look_at_lh(centre + sun_dir * radius * 2.0, centre, up_for(sun_dir));
        let whole = Mat4::orthographic_lh(-radius, radius, -radius, radius, 0.0, radius * 4.0) * view;
        let texel = 2.0 * radius / SHADOW_SIZE as f32;
        let shadows = ([whole; CASCADES], [f32::MAX / 4.0; CASCADES], [texel; CASCADES]);
        gpu.queue.write_buffer(&self.cascades[0].0, 0, bytemuck::bytes_of(&whole.to_cols_array()));
        // each face's view: the six directions, and the up each is seen with
        let dirs = [
            (Vec3::X, Vec3::Y),
            (Vec3::NEG_X, Vec3::Y),
            (Vec3::Y, Vec3::NEG_Z),
            (Vec3::NEG_Y, Vec3::Z),
            (Vec3::Z, Vec3::Y),
            (Vec3::NEG_Z, Vec3::Y),
        ];
        let proj = Mat4::perspective_lh(std::f32::consts::FRAC_PI_2, 1.0, 0.25, 4000.0);
        let stride = (std::mem::size_of::<FrameUniform>() as u64).next_multiple_of(256);
        let faces = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("probe faces"),
            size: stride * u64::from(n) * 6,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut groups = Vec::with_capacity(places.len() * 6);
        for (p, place) in places.iter().enumerate() {
            for (f, (dir, up)) in dirs.iter().enumerate() {
                let vp = proj * Mat4::look_at_lh(*place, *place + *dir, *up);
                let frame = self.frame(r, vp, *place, shadows, false);
                let offset = stride * (p as u64 * 6 + f as u64);
                gpu.queue.write_buffer(&faces, offset, bytemuck::bytes_of(&frame));
                groups.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &r.frame_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &faces,
                            offset,
                            size: wgpu::BufferSize::new(std::mem::size_of::<FrameUniform>() as u64),
                        }),
                    }],
                }));
            }
        }
        let atlas = wgpu::Extent3d { width: PROBE_FACE * 6, height: PROBE_FACE * n, depth_or_array_layers: 1 };
        let target = |label, format| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: atlas,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let (colour, share, depth) = (
            target("probe faces", HDR_FORMAT),
            target("probe share", SHARE_FORMAT),
            target("probe depth", DEPTH_FORMAT),
        );
        let mut encoder = device.create_command_encoder(&Default::default());
        self.draw_shadows(&mut encoder, 0, &self.cascades[0].1, models);
        let lit_group = self.shadow_group.lock().unwrap_or_else(|e| e.into_inner()).clone();
        {
            let c = r.clear;
            let lin = |v: f64| v.max(0.0).powf(2.2);
            let attachment = |view, clear| {
                Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store },
                })
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("probe faces"),
                color_attachments: &[
                    attachment(&colour, wgpu::Color { r: lin(c.r), g: lin(c.g), b: lin(c.b), a: 1.0 }),
                    attachment(&share, wgpu::Color::TRANSPARENT),
                ],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_bind_group(3, &lit_group, &[]);
            for (k, group) in groups.iter().enumerate() {
                let (p, f) = (k as u32 / 6, k as u32 % 6);
                let (x, y) = (f * PROBE_FACE, p * PROBE_FACE);
                pass.set_viewport(x as f32, y as f32, PROBE_FACE as f32, PROBE_FACE as f32, 0.0, 1.0);
                pass.set_scissor_rect(x, y, PROBE_FACE, PROBE_FACE);
                pass.set_bind_group(0, group, &[]);
                draw_layers(&mut pass, &self.pipelines, &self.custom, if p == sky_row { &sky_only } else { models });
            }
        }
        // each probe's faces onto its coefficients
        let coefficients = probe_texture(device, n);
        let bake = BakeUniform {
            inv_faces: dirs.map(|(dir, up)| (proj * Mat4::look_at_lh(Vec3::ZERO, dir, up)).inverse().to_cols_array()),
            size: [PROBE_FACE, 0, 0, 0],
        };
        let bake_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("probe bake"),
            size: std::mem::size_of::<BakeUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.queue.write_buffer(&bake_buffer, 0, bytemuck::bytes_of(&bake));
        {
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("probes"),
                layout: &self.probe_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&colour) },
                    wgpu::BindGroupEntry { binding: 1, resource: bake_buffer.as_entire_binding() },
                ],
            });
            let view = coefficients.create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("probe coefficients"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.probe_pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        gpu.queue.submit([encoder.finish()]);
        let view = coefficients.create_view(&Default::default());
        *self.shadow_group.lock().unwrap_or_else(|e| e.into_inner()) =
            shadow_group(device, &self.shadow_layout, &self.shadow_array, &self.compare, &view);
        let grid = [v4(lo, 1.0), v4(step, 0.0), [count[0] as f32, count[1] as f32, count[2] as f32, 0.0]];
        *self.probes.lock().unwrap_or_else(|e| e.into_inner()) = Some(Probes { view, grid, sky: sky_row });
    }

    /// [`Renderer::draw_rect`] in the remastered lighting.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw(
        &self,
        r: &Renderer,
        gpu: &Gpu,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        rect: [u32; 4],
        clear: bool,
        camera: &Camera,
        models: &[&GpuModel],
    ) {
        let s = &r.settings;
        let aspect = rect[2] as f32 / rect[3].max(1) as f32;
        let view_proj = camera.view_proj(aspect);
        let (sun_colour, sun_dir) = sun(r);
        let cascades = fit_cascades(camera, aspect, sun_dir, s);
        let frame = self.frame(r, view_proj, camera.eye, cascades, true);
        gpu.queue.write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&frame));
        for (i, (b, _)) in self.cascades.iter().enumerate() {
            gpu.queue.write_buffer(b, 0, bytemuck::bytes_of(&cascades.0[i].to_cols_array()));
        }
        let post = PostUniform {
            inv_view_proj: view_proj.inverse().to_cols_array(),
            eye: v4(camera.eye, 1.0),
            sun_dir: v4(sun_dir, 0.0),
            sun_colour: v4(sun_colour, 0.0),
            fog_colour: v4(s.fog_colour, s.fog_density),
            fog: [s.fog_base, s.fog_falloff, s.fog_max, s.fog_start],
            // with post passes the composite writes linear light into their float target, the last step encoding
            tone: [s.exposure, f32::from(self.post_chain.is_none() && !r.color_format.is_srgb()), s.view as f32, 0.0],
            rect: rect.map(|v| v as f32),
            ao: [s.ao_radius, s.ao_strength, rect[3] as f32 / (2.0 * (camera.fov_y * 0.5).tan()), s.ao_samples as f32],
            sky: match &*self.probes.lock().unwrap_or_else(|e| e.into_inner()) {
                Some(p) => [p.sky as f32, s.fog_sky, 0.0, 0.0],
                None => [0.0; 4],
            },
            grade: [s.temperature, s.saturation, s.contrast, s.vignette],
            tint: v4(s.tint, 0.0),
            lift: v4(s.lift, 0.0),
            gamma: v4(s.gamma, 0.0),
            gain: v4(s.gain, 0.0),
        };
        gpu.queue.write_buffer(&self.post_buffer, 0, bytemuck::bytes_of(&post));

        let size = depth.texture().size();
        let mut targets = self.targets.lock().unwrap_or_else(|e| e.into_inner());
        if targets.as_ref().is_none_or(|t| t.size != size) {
            let target = |label, format| {
                gpu.device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some(label),
                        size,
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    })
                    .create_view(&Default::default())
            };
            *targets = Some(Targets {
                size,
                hdr: target("hdr", HDR_FORMAT),
                share: target("indirect share", SHARE_FORMAT),
                ao: target("ambient occlusion", SHARE_FORMAT),
                float: self.post_chain.as_ref().map(|_| [target("post a", HDR_FORMAT), target("post b", HDR_FORMAT)]),
            });
        }
        let t = targets.as_ref().expect("made above");
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        // the shadow maps
        for (i, (_, group)) in self.cascades.iter().enumerate() {
            self.draw_shadows(&mut encoder, i, group, models);
        }
        // the world, lit, into the HDR target
        {
            let c = r.clear;
            let lin = |v: f64| v.max(0.0).powf(2.2);
            let clear_colour = wgpu::Color { r: lin(c.r), g: lin(c.g), b: lin(c.b), a: c.a };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("remastered models"),
                color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment {
                        view: &t.hdr,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: if clear { wgpu::LoadOp::Clear(clear_colour) } else { wgpu::LoadOp::Load },
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &t.share,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: if clear {
                                wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                            } else {
                                wgpu::LoadOp::Load
                            },
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                ],
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
            pass.set_bind_group(3, &*self.shadow_group.lock().unwrap_or_else(|e| e.into_inner()), &[]);
            draw_layers(&mut pass, &self.pipelines, &self.custom, models);
        }
        let view = |binding, v| wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::TextureView(v) };
        let probe_view = self.probes.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|p| p.view.clone());
        let probe_view = probe_view.unwrap_or_else(|| self.black.clone());
        // the ambient occlusion
        if s.ao_strength > 0.0 {
            let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ssao"),
                layout: &self.ssao_layout,
                entries: &[
                    view(0, depth),
                    wgpu::BindGroupEntry { binding: 1, resource: self.post_buffer.as_entire_binding() },
                ],
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ssao"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.ao,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            pass.set_viewport(rect[0] as f32, rect[1] as f32, rect[2] as f32, rect[3] as f32, 0.0, 1.0);
            pass.set_scissor_rect(rect[0], rect[1], rect[2], rect[3]);
            pass.set_pipeline(&self.ssao);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        // fogged and tone mapped into the target, or into the post passes' first
        let chain = self.post_chain.as_ref().zip(t.float.as_ref());
        {
            let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("composite"),
                layout: &self.composite_layout,
                entries: &[
                    view(0, &t.hdr),
                    view(1, depth),
                    wgpu::BindGroupEntry { binding: 2, resource: self.post_buffer.as_entire_binding() },
                    view(3, &t.share),
                    view(4, &t.ao),
                    view(5, &probe_view),
                ],
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: chain.map_or(color, |(_, f)| &f[0]),
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            pass.set_viewport(rect[0] as f32, rect[1] as f32, rect[2] as f32, rect[3] as f32, 0.0, 1.0);
            pass.set_scissor_rect(rect[0], rect[1], rect[2], rect[3]);
            pass.set_pipeline(chain.map_or(&self.composite, |(c, _)| &c.composite));
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        // the packs' post passes, taking turns on the two float targets, then the last step into the target
        if let Some((c, float)) = chain {
            let size = [size.width as f32, size.height as f32, 1.0 / size.width as f32, 1.0 / size.height as f32];
            let uniform = |params: [f32; PARAMS]| PassUniform {
                inv_view_proj: post.inv_view_proj,
                eye: post.eye,
                rect: post.rect,
                size,
                time: [r.time(), 0.0, 0.0, 0.0],
                params,
                engine: [f32::from(!r.color_format.is_srgb()), 0.0, 0.0, 0.0],
            };
            let steps = c.passes.iter().map(|(p, b, params)| (p, b, *params));
            let last = (&c.present, &c.present_buffer, [0.0; PARAMS]);
            for (k, (pipeline, buffer, params)) in steps.chain([last]).enumerate() {
                gpu.queue.write_buffer(buffer, 0, bytemuck::bytes_of(&uniform(params)));
                let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("post pass"),
                    layout: &c.layout,
                    entries: &[
                        view(0, &float[k % 2]),
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&c.sampler) },
                        view(2, depth),
                        wgpu::BindGroupEntry { binding: 3, resource: buffer.as_entire_binding() },
                    ],
                });
                let into = if k == c.passes.len() { color } else { &float[(k + 1) % 2] };
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("post pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: into,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    })],
                    ..Default::default()
                });
                pass.set_viewport(rect[0] as f32, rect[1] as f32, rect[2] as f32, rect[3] as f32, 0.0, 1.0);
                pass.set_scissor_rect(rect[0], rect[1], rect[2], rect[3]);
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &group, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        gpu.queue.submit([encoder.finish()]);
    }
}

/// The sun's colour (the light set's `Sun` times [`Settings::sun`]) and the direction towards it.
fn sun(r: &Renderer) -> (Vec3, Vec3) {
    let s = &r.settings;
    let (colour, dir) = r.lights.lights[0];
    let dir = Vec3::from(dir).try_normalize().unwrap_or(Vec3::new(0.5, 0.7, 0.5).normalize());
    (Vec3::from(colour) * s.sun * Vec3::from(s.sun_tint), turned(dir, s.sun_turn, s.sun_raise))
}

/// `dir` turned `turn` degrees round the vertical (clockwise seen from above, in the game's left-handed space)
/// and `raise` degrees up, kept between the horizon and overhead.
fn turned(dir: Vec3, turn: f32, raise: f32) -> Vec3 {
    if turn == 0.0 && raise == 0.0 {
        return dir;
    }
    let flat = (dir.x * dir.x + dir.z * dir.z).sqrt();
    let azimuth = dir.z.atan2(dir.x) - turn.to_radians();
    let elevation = (dir.y.atan2(flat) + raise.to_radians()).clamp(1f32.to_radians(), 89.9f32.to_radians());
    Vec3::new(azimuth.cos() * elevation.cos(), elevation.sin(), azimuth.sin() * elevation.cos())
}

/// An up direction to look along `dir` with.
fn up_for(dir: Vec3) -> Vec3 {
    if dir.y.abs() > 0.99 {
        Vec3::Z
    } else {
        Vec3::Y
    }
}

/// The box around `models` (their own bounds placed by their root matrices), the sky left out.
fn scene_bounds(models: &[&GpuModel]) -> Option<(Vec3, Vec3)> {
    let mut out: Option<(Vec3, Vec3)> = None;
    for m in models.iter().filter(|m| m.casts_shadow) {
        let Some((lo, hi)) = m.bounds else { continue };
        let root = Mat4::from_cols_array(&m.matrix).transpose();
        for k in 0..8 {
            let c = Vec3::new(
                if k & 1 == 0 { lo[0] } else { hi[0] },
                if k & 2 == 0 { lo[1] } else { hi[1] },
                if k & 4 == 0 { lo[2] } else { hi[2] },
            );
            let p = root.transform_point3(c);
            out = Some(out.map_or((p, p), |(a, b)| (a.min(p), b.max(p))));
        }
    }
    out
}

/// A texture for the probes' coefficients: four texels (the bands' coefficients) a probe, a probe a row.
fn probe_texture(device: &wgpu::Device, probes: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("probe coefficients"),
        size: wgpu::Extent3d { width: 4, height: probes.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: HDR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

/// The lit draws' bind group 3: the shadow maps, their sampler, and the probes' coefficients.
fn shadow_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    shadows: &wgpu::TextureView,
    compare: &wgpu::Sampler,
    probes: &impl ProbeView,
) -> wgpu::BindGroup {
    let probes = probes.view();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("shadows"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(shadows) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(compare) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&probes) },
        ],
    })
}

/// A texture, or a view of one, to bind as the probes' coefficients.
trait ProbeView {
    fn view(&self) -> wgpu::TextureView;
}

impl ProbeView for wgpu::Texture {
    fn view(&self) -> wgpu::TextureView {
        self.create_view(&Default::default())
    }
}

impl ProbeView for wgpu::TextureView {
    fn view(&self) -> wgpu::TextureView {
        self.clone()
    }
}

/// A pipeline drawing one triangle over the view with `module`'s `vs_main` and `fs_main`, its bind group `layout`.
fn fullscreen(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
    label: &str,
) -> wgpu::RenderPipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// The sun's view of each cascade: an orthographic box around the part of `camera`'s view between the cascade's
/// ends, reaching back towards the sun across the scene, its middle snapped to the map's texels. Answers the
/// matrices, the cascades' far ends in view depth and their texels in world units.
fn fit_cascades(
    camera: &Camera,
    aspect: f32,
    sun: Vec3,
    s: &Settings,
) -> ([Mat4; CASCADES], [f32; CASCADES], [f32; CASCADES]) {
    let forward = (camera.target - camera.eye).normalize_or(Vec3::Z);
    let right = camera.up.cross(forward).normalize_or(Vec3::X);
    let up = forward.cross(right);
    let ty = (camera.fov_y * 0.5).tan();
    let tx = ty * aspect;
    let reach = s.shadow_distance.min(camera.far);
    let up_ref = up_for(sun);
    let mut mats = [Mat4::IDENTITY; CASCADES];
    let mut ends = [0.0; CASCADES];
    let mut texels = [0.0; CASCADES];
    let mut near = camera.near;
    for i in 0..CASCADES {
        let far = reach * SPLITS[i];
        let corners: Vec<Vec3> = [near, far]
            .iter()
            .flat_map(|&z| {
                [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)]
                    .map(|(x, y)| camera.eye + forward * z + right * (x * tx * z) + up * (y * ty * z))
            })
            .collect();
        let centre = corners.iter().sum::<Vec3>() / corners.len() as f32;
        // a sphere's radius, rounded up, so the box keeps its size as the view turns
        let radius = corners.iter().map(|c| c.distance(centre)).fold(0.0, f32::max).ceil().max(1.0);
        let back = s.scene_radius + radius;
        let view = Mat4::look_at_lh(centre + sun * back, centre, up_ref);
        let mut proj = Mat4::orthographic_lh(-radius, radius, -radius, radius, 0.0, back + radius);
        // snap the box to whole texels
        let origin = (proj * view) * Vec4::new(0.0, 0.0, 0.0, 1.0);
        let half = SHADOW_SIZE as f32 * 0.5;
        let shift = (origin.truncate().truncate() * half).round() / half - origin.truncate().truncate();
        proj.w_axis.x += shift.x;
        proj.w_axis.y += shift.y;
        mats[i] = proj * view;
        ends[i] = far;
        texels[i] = 2.0 * radius / SHADOW_SIZE as f32;
        near = far;
    }
    (mats, ends, texels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_by_name() {
        let mut s = Settings::default();
        s.apply("sun=2, fog=0.003,sky=0.1/0.2/0.3, ground=0.5").unwrap();
        assert_eq!((s.sun, s.fog_density, s.sky, s.ground), (2.0, 0.003, [0.1, 0.2, 0.3], [0.5; 3]));
        assert!(s.apply("nope=1").unwrap_err().contains("nope"));
        assert!(s.apply("sky=1/2").is_err());
        assert_eq!(Look::parse("Remastered"), Some(Look::Remastered));
    }

    #[test]
    fn the_sun_turns_and_rises() {
        let d = Vec3::new(0.5, 0.68, 0.5).normalize();
        assert_eq!(turned(d, 0.0, 0.0), d);
        // half round keeps its height and points the other way across the ground
        let back = turned(d, 180.0, 0.0);
        assert!((back.y - d.y).abs() < 1e-5 && (back.x + d.x).abs() < 1e-5 && (back.z + d.z).abs() < 1e-5);
        // raised past overhead it stops there
        assert!(turned(d, 0.0, 90.0).y > 0.999);
        assert!(turned(d, 0.0, -90.0).y > 0.0);
    }

    #[test]
    fn settings_files_keep_their_defaults() {
        let s: Settings = ron::from_str("(exposure: 1.2, tint: (1.0, 0.9, 0.8))").unwrap();
        assert_eq!((s.exposure, s.tint, s.sun), (1.2, [1.0, 0.9, 0.8], Settings::default().sun));
        let back: Settings = ron::from_str(&ron::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn cascades_hold_the_view_and_snap() {
        let camera = Camera {
            eye: Vec3::new(0.0, 20.0, -50.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov_y: 50f32.to_radians(),
            near: 0.1,
            far: 2000.0,
        };
        let s = Settings::default();
        let sun = Vec3::new(0.5, 0.68, 0.5).normalize();
        let (mats, ends, texels) = fit_cascades(&camera, 16.0 / 9.0, sun, &s);
        assert_eq!(ends[2], s.shadow_distance);
        assert!(texels[0] < texels[1] && texels[1] < texels[2]);
        // a point in view, inside each cascade's reach, lands inside its map
        let forward = (camera.target - camera.eye).normalize();
        for (i, m) in mats.iter().enumerate() {
            let p = *m * (camera.eye + forward * (ends[i] * 0.9)).extend(1.0);
            assert!(p.x.abs() <= 1.0 && p.y.abs() <= 1.0 && (0.0..=1.0).contains(&p.z), "cascade {i}: {p}");
        }
        // moving the eye a little keeps the world's origin on a whole texel
        let moved = Camera { eye: camera.eye + Vec3::new(0.013, 0.0, 0.007), ..camera };
        let (m2, _, _) = fit_cascades(&moved, 16.0 / 9.0, sun, &s);
        let o = m2[0] * Vec4::W;
        let t = o.truncate().truncate() * (SHADOW_SIZE as f32 * 0.5);
        assert!((t - t.round()).abs().max_element() < 1e-2, "{t}");
    }
}
