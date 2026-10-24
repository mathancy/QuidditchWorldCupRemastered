//! The Looks mode: each stadium's remastered look (`qwc_game::looks`: its lighting and colour grade,
//! `qwc_render::remaster::Settings`) on sliders over a live match in that stadium (`qwc_game::preview`), drawn by
//! the game's own renderer on the editor's wgpu device. Save writes the stadium's file in the highest pack
//! (`looks/<stadium>.ron`; nothing without a pack on), which a running match reads again straight away.

use std::time::Instant;

use eframe::egui::{self, Color32, Sense};
use eframe::egui_wgpu::RenderState;
use qwc_assets::Vfs;
use qwc_game::preview::{MatchPreview, Orbit};
use qwc_render::overlay::OVERLAY_FORMAT;
use qwc_render::remaster::Settings;
use qwc_render::{wgpu, Gpu, Look};

pub struct Looks {
    vfs: &'static Vfs,
    gpu: Option<(RenderState, Gpu)>,
    pub stadiums: Vec<String>,
    /// The stadium shown, and the one to load next frame (after the status says so).
    pub stadium: String,
    loading: Option<String>,
    preview: Option<MatchPreview>,
    target: Option<Target>,
    /// The look being edited, the one in its file, and the edits to undo and redo.
    pub settings: Settings,
    saved: Settings,
    undo: Vec<Settings>,
    redo: Vec<Settings>,
    /// The look before the edit under way (a drag), kept for undo when it ends.
    editing: Option<Settings>,
    /// The settings the light probes were made with.
    lit_with: Option<Settings>,
    pub free_camera: bool,
    orbit: Orbit,
    /// The original's look chosen, and whether Compare is held (it shows the other look while held).
    original: bool,
    comparing: bool,
    last: Instant,
    pub status: String,
}

struct Target {
    size: (u32, u32),
    view: wgpu::TextureView,
    depth: wgpu::TextureView,
    id: egui::TextureId,
}

impl Looks {
    pub fn new(vfs: &'static Vfs, state: Option<RenderState>, stadium: Option<String>) -> Looks {
        let gpu = state.map(|s| {
            let gpu = Gpu {
                instance: s.instance.clone(),
                adapter: s.adapter.clone(),
                device: s.device.clone(),
                queue: s.queue.clone(),
            };
            (s, gpu)
        });
        let stadiums = MatchPreview::stadiums(vfs);
        let stadium = stadium.unwrap_or_else(|| "england".into());
        let settings = qwc_game::looks::load(&stadium);
        Looks {
            vfs,
            gpu,
            stadiums,
            stadium: stadium.clone(),
            loading: Some(stadium),
            preview: None,
            target: None,
            settings,
            saved: settings,
            undo: Vec::new(),
            redo: Vec::new(),
            editing: None,
            lit_with: None,
            free_camera: false,
            orbit: Orbit::default(),
            original: false,
            comparing: false,
            last: Instant::now(),
            status: String::new(),
        }
    }

    pub fn dirty(&self) -> bool {
        self.settings != self.saved
    }

    pub fn save(&mut self) {
        match qwc_game::looks::save(&self.stadium, &self.settings) {
            Ok(p) => {
                self.saved = self.settings;
                self.status = format!("Saved {}", p.display());
            }
            Err(e) => self.status = e,
        }
    }

    pub fn undo(&mut self) {
        if let Some(s) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.settings, s));
        }
    }

    pub fn redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.settings, s));
        }
    }

    /// Show the stadium `name`, its look read from its file (the edits to the last one are dropped).
    fn open(&mut self, name: &str) {
        self.stadium = name.to_string();
        self.settings = qwc_game::looks::load(name);
        self.saved = self.settings;
        self.undo.clear();
        self.redo.clear();
        self.loading = Some(name.to_string());
        self.status = format!("Loading {name}...");
    }

    /// Load a stadium asked for on the last frame, so its status showed first.
    fn load_pending(&mut self, ctx: &egui::Context) {
        let Some(name) = self.loading.clone() else { return };
        if !self.status.starts_with("Loading") {
            self.status = format!("Loading {name}...");
            ctx.request_repaint();
            return;
        }
        self.loading = None;
        let Some((_, gpu)) = &self.gpu else {
            self.status = "The preview needs the editor's wgpu renderer".into();
            return;
        };
        let start = Instant::now();
        match MatchPreview::new(gpu, self.vfs, &name, OVERLAY_FORMAT) {
            Ok(mut p) => {
                p.renderer.settings = self.settings;
                p.relight(gpu);
                self.lit_with = Some(self.settings);
                self.preview = Some(p);
                self.status = format!("{name} loaded in {:.1}s", start.elapsed().as_secs_f32());
            }
            Err(e) => {
                self.preview = None;
                self.status = e;
            }
        }
        self.last = Instant::now();
    }

    /// Draw the match at `size` pixels; answers the texture.
    fn paint(&mut self, size: (u32, u32)) -> Option<egui::TextureId> {
        let (state, gpu) = self.gpu.as_ref()?;
        let preview = self.preview.as_mut()?;
        if self.target.as_ref().is_none_or(|t| t.size != size) {
            let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("look preview"),
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
            let mut renderer = state.renderer.write();
            let id = match self.target.take() {
                Some(t) => {
                    renderer.update_egui_texture_from_wgpu_texture(&gpu.device, &view, wgpu::FilterMode::Linear, t.id);
                    t.id
                }
                None => renderer.register_native_texture(&gpu.device, &view, wgpu::FilterMode::Linear),
            };
            self.target = Some(Target { size, view, depth, id });
        }
        let now = Instant::now();
        preview.advance((now - self.last).as_secs_f64());
        self.last = now;
        preview.renderer.settings = self.settings;
        let original = self.original != self.comparing;
        preview.renderer.look = if original { Look::Original } else { Look::Remastered };
        // the probes again once a change to what lights them has settled (they take a moment)
        if !original && self.lit_with.is_none_or(|l| lighting(&l) != lighting(&self.settings)) && self.editing.is_none()
        {
            preview.relight(gpu);
            self.lit_with = Some(self.settings);
        }
        let t = self.target.as_ref().expect("made above");
        let orbit = self.free_camera.then_some(&self.orbit);
        preview.draw(gpu, &t.view, &t.depth, size, orbit);
        Some(t.id)
    }
}

/// The settings the light probes see (`qwc_render::remaster`'s bake): a change to them makes the probes again.
fn lighting(s: &Settings) -> [f32; 11] {
    [
        s.sun,
        s.sun_tint[0],
        s.sun_tint[1],
        s.sun_tint[2],
        s.sun_turn,
        s.sun_raise,
        s.baked,
        s.baked_sun,
        s.lightmaps,
        s.sky_gain,
        s.shadow_strength,
    ]
}

pub fn ui(lk: &mut Looks, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    lk.load_pending(&ctx);
    let before = lk.settings;
    egui::Panel::top("looks transport").show(ui, |ui| transport(lk, ui));
    egui::Panel::left("looks stadiums").default_size(180.0).show(ui, |ui| stadiums(lk, ui));
    egui::Panel::right("looks settings").default_size(360.0).show(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| settings(lk, ui));
    });
    egui::CentralPanel::no_frame().show(ui, |ui| view(lk, ui));
    // an edit is one step to undo, however long its drag
    if lk.settings != before && lk.editing.is_none() {
        lk.editing = Some(before);
    }
    if !ctx.input(|i| i.pointer.any_down()) {
        if let Some(b) = lk.editing.take() {
            if b != lk.settings {
                lk.undo.push(b);
                lk.redo.clear();
            }
        }
    }
    ctx.request_repaint();
}

fn transport(lk: &mut Looks, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let playing = lk.preview.as_ref().is_some_and(|p| p.playing);
        if ui.button(if playing { "Pause" } else { "Play" }).clicked() {
            if let Some(p) = &mut lk.preview {
                p.playing = !p.playing;
            }
        }
        ui.separator();
        ui.label("Camera");
        ui.selectable_value(&mut lk.free_camera, false, "Match").on_hover_text("The match's own camera");
        ui.selectable_value(&mut lk.free_camera, true, "Free")
            .on_hover_text("Drag to turn round, right-drag to move, scroll to come closer");
        ui.separator();
        ui.selectable_value(&mut lk.original, false, "Remastered");
        ui.selectable_value(&mut lk.original, true, "Original").on_hover_text("The original's look, for comparison");
        lk.comparing =
            ui.button("Hold to compare").on_hover_text("The other look while held").is_pointer_button_down_on();
        ui.separator();
        if ui.add_enabled(lk.dirty(), egui::Button::new("Save")).on_hover_text("Ctrl+S").clicked() {
            lk.save();
        }
        if ui.add_enabled(lk.dirty(), egui::Button::new("Revert")).on_hover_text("Back to the file's").clicked() {
            lk.undo.push(lk.settings);
            lk.settings = lk.saved;
        }
        if ui.button("Defaults").on_hover_text("Every setting at its default").clicked() {
            lk.undo.push(lk.settings);
            lk.settings = Settings { view: lk.settings.view, ..Settings::default() };
        }
        ui.separator();
        ui.label("Show");
        ui.selectable_value(&mut lk.settings.view, 0, "Picture");
        ui.selectable_value(&mut lk.settings.view, 1, "AO");
        ui.selectable_value(&mut lk.settings.view, 2, "Indirect");
    });
}

fn stadiums(lk: &mut Looks, ui: &mut egui::Ui) {
    ui.label(qwc_style::heading("Stadiums", 20.0));
    ui.label("Bold: has a look file");
    egui::ScrollArea::vertical().show(ui, |ui| {
        for name in lk.stadiums.clone() {
            let has = qwc_game::looks::path(&name).is_some_and(|p| p.is_file());
            let mut text = egui::RichText::new(&name);
            if has {
                text = text.strong();
            }
            if ui.selectable_label(lk.stadium == name, text).clicked() && lk.stadium != name {
                if lk.dirty() {
                    lk.save();
                }
                lk.open(&name);
            }
        }
    });
}

fn view(lk: &mut Looks, ui: &mut egui::Ui) {
    let rect = ui.available_rect_before_wrap();
    let ppp = ui.ctx().pixels_per_point();
    let size = ((rect.width() * ppp).round().max(16.0) as u32, (rect.height() * ppp).round().max(16.0) as u32);
    let response = ui.allocate_rect(rect, Sense::click_and_drag());
    match lk.paint(size) {
        Some(id) => {
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            ui.painter().image(id, rect, uv, Color32::WHITE);
        }
        None => {
            ui.painter().rect_filled(rect, 0.0, qwc_style::WELL);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                &lk.status,
                egui::FontId::proportional(18.0),
                Color32::GRAY,
            );
        }
    }
    if !lk.free_camera {
        return;
    }
    let o = &mut lk.orbit;
    if response.dragged_by(egui::PointerButton::Primary) {
        let d = response.drag_delta();
        o.yaw += d.x * 0.3;
        o.pitch = (o.pitch + d.y * 0.3).clamp(-10.0, 89.0);
    }
    if response.dragged_by(egui::PointerButton::Secondary) || response.dragged_by(egui::PointerButton::Middle) {
        let d = response.drag_delta() * o.distance * 0.002;
        let yaw = o.yaw.to_radians();
        // across the view and along the ground
        o.target[0] -= yaw.cos() * d.x + yaw.sin() * d.y;
        o.target[2] -= yaw.sin() * d.x - yaw.cos() * d.y;
    }
    if response.hovered() {
        let scroll = ui.ctx().input(|i| i.smooth_scroll_delta.y);
        o.distance = (o.distance * (-scroll * 0.002).exp()).clamp(2.0, 1500.0);
    }
}

/// A number with its slider, a reset to its default and a note.
fn num(ui: &mut egui::Ui, label: &str, v: &mut f32, default: f32, range: std::ops::RangeInclusive<f32>, note: &str) {
    ui.horizontal(|ui| {
        ui.add_sized([110.0, 18.0], egui::Label::new(label)).on_hover_text(note);
        ui.add(egui::Slider::new(v, range).clamping(egui::SliderClamping::Never).max_decimals(4)).on_hover_text(note);
        reset(ui, *v != default, || *v = default);
    });
}

/// A colour of 0 to 1 a channel, with a picker.
fn colour(ui: &mut egui::Ui, label: &str, v: &mut [f32; 3], default: [f32; 3], note: &str) {
    ui.horizontal(|ui| {
        ui.add_sized([110.0, 18.0], egui::Label::new(label)).on_hover_text(note);
        ui.color_edit_button_rgb(v).on_hover_text(note);
        for c in v.iter_mut() {
            ui.add(egui::DragValue::new(c).speed(0.005).range(0.0..=4.0).max_decimals(3));
        }
        reset(ui, *v != default, || *v = default);
    });
}

/// Three channels about a middle (lift, gamma, gain): one slider for all, then each.
fn wheel(
    ui: &mut egui::Ui,
    label: &str,
    v: &mut [f32; 3],
    default: f32,
    range: std::ops::RangeInclusive<f32>,
    note: &str,
) {
    ui.horizontal(|ui| {
        ui.add_sized([110.0, 18.0], egui::Label::new(label)).on_hover_text(note);
        let mean = (v[0] + v[1] + v[2]) / 3.0;
        let mut all = mean;
        ui.add(egui::Slider::new(&mut all, range.clone()).max_decimals(3)).on_hover_text("All three channels");
        if all != mean {
            v.iter_mut().for_each(|c| *c += all - mean);
        }
        reset(ui, *v != [default; 3], || *v = [default; 3]);
    });
    ui.horizontal(|ui| {
        ui.add_space(114.0);
        for (c, name) in v.iter_mut().zip(["R", "G", "B"]) {
            ui.label(name);
            ui.add(egui::DragValue::new(c).speed(0.002).range(range.clone()).max_decimals(3));
        }
    });
}

fn reset(ui: &mut egui::Ui, changed: bool, to_default: impl FnOnce()) {
    if ui.add_enabled(changed, egui::Button::new("Reset").small()).on_hover_text("Back to the default").clicked() {
        to_default();
    }
}

fn settings(lk: &mut Looks, ui: &mut egui::Ui) {
    let d = Settings::default();
    ui.label(qwc_style::heading(format!("{}{}", lk.stadium, if lk.dirty() { " *" } else { "" }), 20.0));
    ui.label(qwc_game::looks::path(&lk.stadium).map_or_else(
        || "No pack is on to save into: turn one on, or make one, in the Packs mode".into(),
        |p| p.display().to_string(),
    ));
    let s = &mut lk.settings;
    egui::CollapsingHeader::new("Grade").default_open(true).show(ui, |ui| {
        num(ui, "Exposure", &mut s.exposure, d.exposure, 0.0..=4.0, "The picture's brightness before tone mapping");
        num(ui, "Temperature", &mut s.temperature, d.temperature, -1.0..=1.0, "Warmer above 0, cooler below");
        colour(ui, "Tint", &mut s.tint, d.tint, "A colour filter over the whole picture (white: none)");
        num(ui, "Saturation", &mut s.saturation, d.saturation, 0.0..=2.0, "0 grey, 1 as it is");
        num(ui, "Contrast", &mut s.contrast, d.contrast, 0.5..=2.0, "About mid grey; 1 as it is");
        wheel(ui, "Lift", &mut s.lift, 0.0, -0.2..=0.2, "Raises (or lowers) the shadows");
        wheel(ui, "Gamma", &mut s.gamma, 1.0, 0.5..=2.0, "Bends the middle tones: above 1 brighter");
        wheel(ui, "Gain", &mut s.gain, 1.0, 0.5..=1.5, "Scales the highlights");
        num(ui, "Vignette", &mut s.vignette, d.vignette, 0.0..=1.0, "How much the corners darken");
    });
    egui::CollapsingHeader::new("Sun").default_open(true).show(ui, |ui| {
        num(ui, "Strength", &mut s.sun, d.sun, 0.0..=4.0, "Times the stadium's own sun (lights.cli)");
        colour(ui, "Tint", &mut s.sun_tint, d.sun_tint, "Times the sun's colour");
        num(ui, "Turn", &mut s.sun_turn, d.sun_turn, -180.0..=180.0, "Degrees round the sky from the stadium's sun");
        num(ui, "Raise", &mut s.sun_raise, d.sun_raise, -60.0..=60.0, "Degrees up from the stadium's sun");
        num(ui, "Shadow strength", &mut s.shadow_strength, d.shadow_strength, 0.0..=1.0, "1: no sunlight in the shade");
        num(
            ui,
            "Shadow reach",
            &mut s.shadow_distance,
            d.shadow_distance,
            50.0..=1000.0,
            "How far from the eye shadows are drawn",
        );
    });
    egui::CollapsingHeader::new("Baked light").default_open(true).show(ui, |ui| {
        num(ui, "Baked", &mut s.baked, d.baked, 0.0..=2.0, "The stadium's vertex colours (the original's baked light)");
        num(ui, "Sun on stadium", &mut s.baked_sun, d.baked_sun, 0.0..=2.0, "The sun's light on the stadium");
        num(ui, "Lightmaps", &mut s.lightmaps, d.lightmaps, 0.0..=1.0, "1 the original's lightmaps, 0 none");
        num(ui, "Sky", &mut s.sky_gain, d.sky_gain, 0.0..=3.0, "The sky's brightness");
    });
    egui::CollapsingHeader::new("Characters").default_open(false).show(ui, |ui| {
        num(ui, "Probes", &mut s.probes, d.probes, 0.0..=3.0, "The light probes' light (0: the ambient below instead)");
        colour(ui, "Ambient above", &mut s.sky, d.sky, "Without probes: the light from above");
        colour(ui, "Ambient below", &mut s.ground, d.ground, "Without probes: the light from below");
        num(ui, "Fill", &mut s.fill, d.fill, 0.0..=2.0, "The stadium's fill light (lights.cli)");
    });
    egui::CollapsingHeader::new("Fog").default_open(true).show(ui, |ui| {
        num(ui, "Density", &mut s.fog_density, d.fog_density, 0.0..=0.01, "At the base height, per unit");
        num(ui, "Falloff", &mut s.fog_falloff, d.fog_falloff, 0.0..=0.1, "How fast it thins upwards");
        num(ui, "Base height", &mut s.fog_base, d.fog_base, -50.0..=100.0, "Where it is thickest");
        num(ui, "Most", &mut s.fog_max, d.fog_max, 0.0..=1.0, "The most fog in the way");
        num(ui, "Start", &mut s.fog_start, d.fog_start, 0.0..=300.0, "How far from the eye it starts");
        colour(ui, "Colour", &mut s.fog_colour, d.fog_colour, "Its own colour");
        num(ui, "From the sky", &mut s.fog_sky, d.fog_sky, 0.0..=1.0, "How much of its colour is the sky's");
    });
    egui::CollapsingHeader::new("Ambient occlusion").default_open(false).show(ui, |ui| {
        num(ui, "Strength", &mut s.ao_strength, d.ao_strength, 0.0..=2.0, "0 off");
        num(ui, "Radius", &mut s.ao_radius, d.ao_radius, 0.1..=10.0, "Its reach in world units");
    });
}
