//! The front end's 2D pass: textured quads in the front end's 512 × 512 screen, as `qwc_feck::draw` makes them,
//! each blended by its material's blend type ([`BLENDS`]) with no depth. Textures are uploaded once by a key the
//! caller picks. Colours stay in gamma space (an `Rgba8Unorm` target), as the original blends, and the target's
//! alpha is a mask channel: some types write only it, others blend through it.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::Gpu;

/// The target format the overlay draws into.
pub const OVERLAY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// A corner: front-end pixels, a texture coordinate (0 to 1), a colour (0 to 1; 0.5 is full, doubled by the
/// shader).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct OverlayVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub colour: [f32; 4],
}

/// A quad: a texture, its blend type, its material layer, and its corners in the order bottom left, bottom
/// right, top left, top right.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayQuad {
    pub texture: u64,
    pub blend: u8,
    pub layer: u32,
    pub corners: [OverlayVertex; 4],
}

/// One of the engine's blend types (Direct3D 8 factors: 1 zero, 2 one, 3 source colour, 5 source alpha,
/// 6 its inverse, 7 destination alpha, 8 its inverse, 9 destination colour).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Blend {
    pub src: u8,
    pub dst: u8,
    pub enabled: bool,
    /// The destination minus the source (`D3DBLENDOP_REVSUBTRACT`).
    pub reverse: bool,
    /// Which channels are written: bits red, green, blue, alpha (`D3DRS_COLORWRITEENABLE`).
    pub writes: u8,
}

const fn b(src: u8, dst: u8, enabled: bool, writes: u8) -> Blend {
    Blend { src, dst, enabled, reverse: false, writes }
}

/// The blend types (`0x508340` fills the state blocks `[0xf9d188]`, 0x3c bytes each, applied by `0x507460`; the
/// writes follow the type's value in `[0x5a5578]`, `0x507852`: none set writes all, alpha set colour only, colour
/// set alpha only, both set nothing). This is the path for cards that can mask colour writes; **[I]** type 0's
/// block is never filled, taken as opaque.
pub const BLENDS: [Blend; 22] = [
    b(2, 1, false, 0xf),
    b(2, 1, false, 0xf),
    b(5, 6, true, 0xf),
    b(5, 6, true, 0x7),
    b(2, 1, true, 0x8),
    b(7, 8, true, 0xf),
    b(7, 8, true, 0x7),
    b(5, 2, true, 0xf),
    b(7, 2, true, 0xf),
    Blend { src: 5, dst: 2, enabled: true, reverse: true, writes: 0x7 },
    b(7, 1, true, 0x7),
    b(2, 1, true, 0xf),
    b(5, 2, true, 0x7),
    b(7, 2, true, 0x7),
    b(2, 1, false, 0x7),
    b(6, 5, true, 0xf),
    b(6, 5, true, 0x7),
    b(8, 7, true, 0xf),
    b(8, 7, true, 0x7),
    b(1, 2, true, 0x0),
    b(9, 3, false, 0xf),
    b(9, 3, false, 0x7),
];

fn factor(f: u8) -> wgpu::BlendFactor {
    use wgpu::BlendFactor as F;
    match f {
        1 => F::Zero,
        3 => F::Src,
        4 => F::OneMinusSrc,
        5 => F::SrcAlpha,
        6 => F::OneMinusSrcAlpha,
        7 => F::DstAlpha,
        8 => F::OneMinusDstAlpha,
        9 => F::Dst,
        10 => F::OneMinusDst,
        _ => F::One,
    }
}

impl Blend {
    /// Its blend state (none when it does not blend).
    pub fn blend_state(&self) -> Option<wgpu::BlendState> {
        self.state()
    }

    /// Its colour writes.
    pub fn write_mask(&self) -> wgpu::ColorWrites {
        self.mask()
    }

    fn state(&self) -> Option<wgpu::BlendState> {
        if !self.enabled {
            return None;
        }
        // Direct3D 8 blends alpha with the same factors as colour
        let op = if self.reverse { wgpu::BlendOperation::ReverseSubtract } else { wgpu::BlendOperation::Add };
        let c = wgpu::BlendComponent { src_factor: factor(self.src), dst_factor: factor(self.dst), operation: op };
        Some(wgpu::BlendState { color: c, alpha: c })
    }

    fn mask(&self) -> wgpu::ColorWrites {
        wgpu::ColorWrites::from_bits_truncate(self.writes as u32)
    }
}

/// The whole target in the screen's units when it is stretched over it, as the original draws it.
pub const STRETCHED: [f32; 4] = [0.0, 0.0, 512.0, 512.0];

pub struct Overlay {
    pipelines: Vec<wgpu::RenderPipeline>,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    textures: HashMap<u64, wgpu::BindGroup>,
    /// The target's left, top, right and bottom in the screen's units (`qwc_feck::draw::Fit`).
    span: [f32; 4],
    fit_buffer: wgpu::Buffer,
    fit_group: wgpu::BindGroup,
}

impl Overlay {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Overlay {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay"),
            source: wgpu::ShaderSource::Wgsl(include_str!("overlay.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let fit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay fit"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let span = STRETCHED;
        let fit_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("overlay fit"),
            contents: bytemuck::cast_slice(&span),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let fit_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay fit"),
            layout: &fit_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: fit_buffer.as_entire_binding() }],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay"),
            bind_group_layouts: &[Some(&layout), Some(&fit_layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4];
        let pipeline = |blend: &Blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("overlay"),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<OverlayVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attributes,
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend: blend.state(), write_mask: blend.mask() })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipelines = BLENDS.iter().map(pipeline).collect();
        // **[I]** the front end's sampler state is not traced: bilinear, clamped (the half-texel inset suggests it)
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("overlay"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            // the remastered textures' mip levels (the disc's are uploaded with one)
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        Overlay { pipelines, layout, sampler, textures: HashMap::new(), span, fit_buffer, fit_group }
    }

    /// Where the next draws put the screen on their target: the target's left, top, right and bottom in the
    /// screen's units ([`STRETCHED`] until set; not the original's otherwise, `qwc_feck::draw::Fit`).
    pub fn set_span(&mut self, gpu: &Gpu, span: [f32; 4]) {
        if self.span != span {
            self.span = span;
            gpu.queue.write_buffer(&self.fit_buffer, 0, bytemuck::cast_slice(&span));
        }
    }

    pub fn span(&self) -> [f32; 4] {
        self.span
    }

    /// Drop the texture under `key` (the text's glyphs when the window's size changes).
    pub fn forget(&mut self, key: u64) {
        self.textures.remove(&key);
    }

    pub fn has_texture(&self, key: u64) -> bool {
        self.textures.contains_key(&key)
    }

    /// Upload an RGBA8 texture under `key` (kept as is: no sRGB conversion).
    pub fn upload(&mut self, gpu: &Gpu, key: u64, width: u32, height: u32, rgba: &[u8]) {
        self.upload_as(gpu, key, width, height, wgpu::TextureFormat::Rgba8Unorm, 1, rgba);
    }

    /// [`Overlay::upload`] for pixels stored B G R A.
    pub fn upload_bgra(&mut self, gpu: &Gpu, key: u64, width: u32, height: u32, bgra: &[u8]) {
        self.upload_as(gpu, key, width, height, wgpu::TextureFormat::Bgra8Unorm, 1, bgra);
    }

    /// [`Overlay::upload`] with mip levels (`levels[l]` is `max(1, width >> l) x max(1, height >> l)`), for
    /// textures drawn much smaller than they are.
    pub fn upload_levels(&mut self, gpu: &Gpu, key: u64, width: u32, height: u32, levels: &[Vec<u8>]) {
        let format = wgpu::TextureFormat::Rgba8Unorm;
        self.upload_as(gpu, key, width, height, format, levels.len().max(1) as u32, &levels.concat());
    }

    #[allow(clippy::too_many_arguments)]
    fn upload_as(
        &mut self,
        gpu: &Gpu,
        key: u64,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        mips: u32,
        data: &[u8],
    ) {
        let tex = gpu.device.create_texture_with_data(
            &gpu.queue,
            &wgpu::TextureDescriptor {
                label: Some("overlay texture"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: mips,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            data,
        );
        let view = tex.create_view(&Default::default());
        let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay texture"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        self.textures.insert(key, group);
    }

    /// Draw `quads` into `target` (cleared to `clear` first when given), by material layer and in order within
    /// one (**[I]** the engine draws its material layers in turn). Quads whose texture is not uploaded are
    /// skipped.
    pub fn draw(&self, gpu: &Gpu, target: &wgpu::TextureView, clear: Option<wgpu::Color>, quads: &[OverlayQuad]) {
        let mut quads: Vec<&OverlayQuad> = quads.iter().filter(|q| self.textures.contains_key(&q.texture)).collect();
        quads.sort_by_key(|q| q.layer);
        let vertices: Vec<OverlayVertex> = quads
            .iter()
            .flat_map(|q| {
                let [bl, br, tl, tr] = q.corners;
                [bl, br, tl, tl, br, tr]
            })
            .collect();
        let buffer = (!vertices.is_empty()).then(|| {
            gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("overlay"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("overlay"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: clear.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            if let Some(buffer) = &buffer {
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.set_bind_group(1, &self.fit_group, &[]);
                // runs of quads on one texture and blend type are one draw
                let mut start = 0;
                while start < quads.len() {
                    let (key, blend) = (quads[start].texture, quads[start].blend);
                    let end = (start..quads.len())
                        .find(|&k| quads[k].texture != key || quads[k].blend != blend)
                        .unwrap_or(quads.len());
                    let p = self.pipelines.get(blend as usize).unwrap_or(&self.pipelines[2]);
                    pass.set_pipeline(p);
                    pass.set_bind_group(0, &self.textures[&key], &[]);
                    pass.draw(start as u32 * 6..end as u32 * 6, 0..1);
                    start = end;
                }
            }
        }
        gpu.queue.submit([encoder.finish()]);
    }
}

/// An RGBA8 target for the overlay that can be read back.
pub struct OverlayTarget {
    texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub size: (u32, u32),
}

impl OverlayTarget {
    pub fn new(device: &wgpu::Device, size: (u32, u32)) -> OverlayTarget {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("overlay target"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OVERLAY_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        OverlayTarget { texture, view, size }
    }

    /// The pixels as tightly packed RGBA8 rows.
    pub fn read_rgba(&self, gpu: &Gpu) -> Result<Vec<u8>, String> {
        crate::read_texture(gpu, &self.texture, self.size)
    }
}
