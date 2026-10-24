//! The Screens mode's exact preview: the front end's screen drawn by the game's own painter (`qwc_game::fe::Painter`)
//! on the editor's wgpu device, into a texture the canvas shows. It is what the game's window shows: the stadium,
//! the screen's models and puppets in the 3D world behind the menus, every blend type as the engine has it, and the
//! remaster's text. The edits not yet drawn by a tick are laid over the list first, as the canvas lays them.

use eframe::egui;
use eframe::egui_wgpu::RenderState;
use qwc_feck::draw::DrawList;
use qwc_game::fe::{flag_store, Painter};
use qwc_render::overlay::OVERLAY_FORMAT;
use qwc_render::{wgpu, Gpu};

pub struct Exact {
    state: RenderState,
    gpu: Gpu,
    painter: Option<Painter>,
    target: Option<Target>,
}

/// The texture drawn into, its depth, and its id for egui.
struct Target {
    size: (u32, u32),
    view: wgpu::TextureView,
    depth: wgpu::TextureView,
    id: egui::TextureId,
}

impl Exact {
    /// Over eframe's device (`None` without its wgpu renderer).
    pub fn new(state: Option<RenderState>) -> Option<Exact> {
        let state = state?;
        let gpu = Gpu {
            instance: state.instance.clone(),
            adapter: state.adapter.clone(),
            device: state.device.clone(),
            queue: state.queue.clone(),
        };
        Some(Exact { state, gpu, painter: None, target: None })
    }

    /// Draw `list` (the screen of `flow`, its textures and fonts in `disc`) at `size` pixels, `time` seconds into
    /// the flow; answers the texture to show.
    pub fn paint(
        &mut self,
        list: &DrawList,
        flow: &qwc_frontend::Flow,
        disc: &qwc_frontend::Disc,
        size: (u32, u32),
        time: f64,
    ) -> egui::TextureId {
        let gpu = &self.gpu;
        if self.target.as_ref().is_none_or(|t| t.size != size) {
            let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("exact preview"),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: OVERLAY_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let depth = qwc_render::depth_view(&gpu.device, size);
            let mut renderer = self.state.renderer.write();
            let id = match self.target.take() {
                Some(t) => {
                    renderer.update_egui_texture_from_wgpu_texture(&gpu.device, &view, wgpu::FilterMode::Linear, t.id);
                    t.id
                }
                None => renderer.register_native_texture(&gpu.device, &view, wgpu::FilterMode::Linear),
            };
            self.target = Some(Target { size, view, depth, id });
        }
        let t = self.target.as_ref().expect("made above");
        let painter = self.painter.get_or_insert_with(|| {
            // the world lit as the game lights it
            let mut p = Painter::new(gpu, OVERLAY_FORMAT);
            p.set_world(qwc_render::Look::Remastered, None);
            p
        });
        let store = flag_store(&flow.fe, &flow.vm.mem);
        painter.paint(gpu, &t.view, &t.depth, size, list, &flow.fe, disc, time, &store, None);
        t.id
    }
}
