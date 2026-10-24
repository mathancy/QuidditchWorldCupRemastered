//! The effects' pass: textured quads in world space (the particles, trails and dynamic geometry of the
//! original's `GameRender\Common` systems), each blended by its material's blend type ([`overlay::BLENDS`], the
//! engine's) and depth-tested against the scene drawn before them, writing depth or not as its material says.
//! Textures are uploaded once by a key the caller picks.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::overlay::{self, BLENDS};
use crate::{Camera, Gpu, DEPTH_FORMAT};

/// A corner: a world position, a texture coordinate, a colour (1 leaves the texture as it is).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct EffectVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub colour: [f32; 4],
}

/// A quad: a texture, its blend type, whether it writes depth, and its corners in the order bottom left, bottom
/// right, top left, top right.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EffectQuad {
    pub texture: u64,
    pub blend: u8,
    pub depth_write: bool,
    /// The texture repeats past its edges (a material's `map`); else it stops at them.
    pub wrap: bool,
    pub corners: [EffectVertex; 4],
}

pub struct Effects {
    /// By blend type, without and with depth writes.
    pipelines: Vec<[wgpu::RenderPipeline; 2]>,
    frame_layout: wgpu::BindGroupLayout,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Each texture's bind groups, stopping at its edges and repeating.
    textures: HashMap<u64, [wgpu::BindGroup; 2]>,
    repeat: wgpu::Sampler,
}

impl Effects {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Effects {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("effects"),
            source: wgpu::ShaderSource::Wgsl(include_str!("effects.wgsl").into()),
        });
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("effects frame"),
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
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("effects texture"),
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
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("effects"),
            bind_group_layouts: &[Some(&frame_layout), Some(&layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x4];
        let pipeline = |blend: &overlay::Blend, depth_write: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("effects"),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<EffectVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attributes,
                    })],
                },
                // both sides: the effects' quads face any way
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
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
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: blend.blend_state(),
                        write_mask: blend.write_mask(),
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipelines = BLENDS.iter().map(|b| [pipeline(b, false), pipeline(b, true)]).collect();
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("effects"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let repeat = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("effects, repeating"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        Effects { pipelines, frame_layout, layout, sampler, textures: HashMap::new(), repeat }
    }

    pub fn has_texture(&self, key: u64) -> bool {
        self.textures.contains_key(&key)
    }

    /// Upload an RGBA8 texture under `key` (kept as is), with its mip levels (`levels[l]` is `max(1, width >> l) x
    /// max(1, height >> l)`).
    pub fn upload(&mut self, gpu: &Gpu, key: u64, width: u32, height: u32, levels: &[Vec<u8>]) {
        let tex = gpu.device.create_texture_with_data(
            &gpu.queue,
            &wgpu::TextureDescriptor {
                label: Some("effects texture"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: levels.len().max(1) as u32,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &levels.concat(),
        );
        let view = tex.create_view(&Default::default());
        let group = |sampler: &wgpu::Sampler| {
            gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("effects texture"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
                ],
            })
        };
        let groups = [group(&self.sampler), group(&self.repeat)];
        self.textures.insert(key, groups);
    }

    /// Draw `quads` over `color` and `depth` (the scene's, kept), seen by `camera` on a `size` target: the ones that
    /// write depth first, then the rest, each set in order. Quads whose texture is not uploaded are skipped.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        gpu: &Gpu,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        size: (u32, u32),
        camera: &Camera,
        quads: &[EffectQuad],
    ) {
        self.draw_rect(gpu, color, depth, [0, 0, size.0, size.1], camera, quads);
    }

    /// [`Effects::draw`] into the rectangle `rect` (`x, y, width, height`) of the targets, a split screen's view.
    pub fn draw_rect(
        &self,
        gpu: &Gpu,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        rect: [u32; 4],
        camera: &Camera,
        quads: &[EffectQuad],
    ) {
        let size = (rect[2], rect[3]);
        let mut quads: Vec<&EffectQuad> = quads.iter().filter(|q| self.textures.contains_key(&q.texture)).collect();
        if quads.is_empty() {
            return;
        }
        quads.sort_by_key(|q| !q.depth_write);
        let vertices: Vec<EffectVertex> = quads
            .iter()
            .flat_map(|q| {
                let [bl, br, tl, tr] = q.corners;
                [bl, br, tl, tl, br, tr]
            })
            .collect();
        let buffer = gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("effects"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let aspect = size.0 as f32 / size.1.max(1) as f32;
        let view_proj = camera.view_proj(aspect).to_cols_array();
        let uniform = gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("effects frame"),
            contents: bytemuck::cast_slice(&view_proj),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let frame = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("effects frame"),
            layout: &self.frame_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
        });
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("effects"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_viewport(rect[0] as f32, rect[1] as f32, rect[2] as f32, rect[3] as f32, 0.0, 1.0);
            pass.set_scissor_rect(rect[0], rect[1], rect[2], rect[3]);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.set_bind_group(0, &frame, &[]);
            // runs of quads on one texture, blend type and depth write are one draw
            let mut start = 0;
            while start < quads.len() {
                let q = quads[start];
                let end = (start..quads.len())
                    .find(|&k| {
                        let o = quads[k];
                        o.texture != q.texture
                            || o.blend != q.blend
                            || o.depth_write != q.depth_write
                            || o.wrap != q.wrap
                    })
                    .unwrap_or(quads.len());
                let set = self.pipelines.get(q.blend as usize).unwrap_or(&self.pipelines[2]);
                pass.set_pipeline(&set[usize::from(q.depth_write)]);
                pass.set_bind_group(1, &self.textures[&q.texture][usize::from(q.wrap)], &[]);
                pass.draw(start as u32 * 6..end as u32 * 6, 0..1);
                start = end;
            }
        }
        gpu.queue.submit([encoder.finish()]);
    }
}
