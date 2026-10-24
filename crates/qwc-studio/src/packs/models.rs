//! The Packs mode's Models tab: the game's models (every `.fpm`), each exported into the pack as a `.glb` to edit
//! (`qwc_assets::gltf_out`: the finest level, the skeleton, the morphs as `morphNN`, the textures), replaced by a
//! picked `.glb` (read as the game reads it first), shown in its folder or taken out. A model's file goes at its slot
//! (`qwc_packs::model_slot`: `players/<team>/<name>.glb`, `props/<name>.glb`, `models/<path>.glb`).
//!
//! The picked model is drawn by the engine's renderer (`qwc_render`, the remastered lighting, a player in the rest
//! pose of its skeleton): the pack's file when it has one, drawn again when the file changes, else the game's.

use std::path::Path;
use std::sync::OnceLock;
use std::time::SystemTime;

use eframe::egui::{self, RichText};
use qwc_packs::model_slot;
use qwc_render::{Camera, Gpu, Offscreen, Renderer};

use super::{open_with_system, PackEditor};

/// The preview's size in pixels.
const PREVIEW: u32 = 420;

/// A drawn preview and what it was drawn from: the model, its file's time (none: the game's own) and the turn.
pub(super) struct Preview {
    key: (String, Option<SystemTime>, i32),
    pub(crate) image: Result<egui::TextureHandle, String>,
}

/// A GPU of the Studio's own for the previews (none where the machine has no adapter).
fn gpu() -> Option<&'static Gpu> {
    static GPU: OnceLock<Option<Gpu>> = OnceLock::new();
    GPU.get_or_init(|| Gpu::headless().map_err(|e| eprintln!("previews: {e}")).ok()).as_ref()
}

/// The model at game path `path` (the pack's `file` when given) drawn turned `yaw` degrees: RGBA rows.
fn draw(vfs: &qwc_assets::Vfs, path: &str, file: Option<&Path>, yaw: f32) -> Result<Vec<u8>, String> {
    let gpu = gpu().ok_or("no graphics adapter for the preview")?;
    let textures = qwc_assets::TextureIndex::build(vfs);
    let original = qwc_assets::model::load(vfs, &textures, path).map_err(|e| e.to_string());
    let model = match file {
        Some(f) => match original.as_ref().ok().and_then(|o| o.skeleton.as_ref()) {
            Some((skl, _)) => qwc_assets::glb::load(vfs, &textures, f, skl, path, true),
            None => qwc_assets::glb::load_scenery(f),
        }
        .map(|(m, _)| m)
        .map_err(|e| e.to_string())?,
        None => original?,
    };
    let mut renderer = Renderer::new(&gpu.device, &gpu.queue, qwc_render::OFFSCREEN_FORMAT);
    renderer.look = qwc_render::Look::Remastered;
    renderer.clear = qwc_render::wgpu::Color { r: 0.12, g: 0.13, b: 0.17, a: 1.0 };
    let g = renderer.upload_model(gpu, vfs, &model)?;
    if let Some((_, skeleton)) = &model.skeleton {
        let rig = qwc_anim::Rig::new(skeleton.clone());
        renderer.set_pose(&gpu.queue, &g, &rig.skin(None, 0.0));
    }
    let (lo, hi) = model.bounds().unwrap_or(([-1.0; 3], [1.0; 3]));
    let camera = Camera::framing(lo, hi, yaw.to_radians(), 12f32.to_radians(), 0.7);
    let target = Offscreen::new(&gpu.device, (PREVIEW, PREVIEW));
    target.render(gpu, &renderer, &camera, &[&g]);
    target.read_rgba(gpu)
}

/// The preview of `path` (the pack's `file`, if any), drawn again when the model, the file or the turn changes.
fn preview(pe: &mut PackEditor, ui: &mut egui::Ui, path: &str, file: Option<&Path>) {
    let r = ui.add(egui::Slider::new(&mut pe.preview_yaw, -180.0..=180.0).suffix(" deg").text("Turn"));
    let settled = !r.dragged();
    let time = file.and_then(|f| f.metadata().ok()?.modified().ok());
    let key = (path.to_string(), time, pe.preview_yaw.round() as i32);
    if settled && pe.model_preview.as_ref().is_none_or(|p| p.key != key) {
        let image = draw(pe.vfs, path, file, pe.preview_yaw).map(|rgba| {
            let size = [PREVIEW as usize, PREVIEW as usize];
            let picture = egui::ColorImage::from_rgba_unmultiplied(size, &rgba);
            ui.ctx().load_texture(format!("preview {path}"), picture, egui::TextureOptions::LINEAR)
        });
        pe.model_preview = Some(Preview { key, image });
    }
    match pe.model_preview.as_ref().map(|p| &p.image) {
        Some(Ok(tex)) => {
            ui.image((tex.id(), egui::vec2(PREVIEW as f32, PREVIEW as f32) * 0.8));
        }
        Some(Err(e)) => {
            ui.colored_label(qwc_style::BAD, format!("No preview: {e}"));
        }
        None => {}
    }
}

/// What the game makes of a `.glb` for the model at `path`: its counts, or why it does not read.
fn read_as_game(pe: &PackEditor, path: &str, file: &std::path::Path) -> Result<String, String> {
    let textures = qwc_assets::TextureIndex::build(pe.vfs);
    let original = qwc_assets::model::load(pe.vfs, &textures, path).map_err(|e| e.to_string())?;
    let (m, r) = match &original.skeleton {
        Some((skl, _)) => qwc_assets::glb::load(pe.vfs, &textures, file, skl, path, true),
        None => qwc_assets::glb::load_scenery(file),
    }
    .map_err(|e| e.to_string())?;
    let verts: usize = m.meshes.iter().map(|x| x.positions.len()).sum();
    let mut said = format!("{} meshes, {verts} vertices, {} morph channels", m.meshes.len(), m.morph_channels().len());
    if !r.unmatched_joints.is_empty() {
        said += &format!("; joints with no bone: {}", r.unmatched_joints.join(", "));
    }
    if !r.unnamed_morphs.is_empty() {
        said += &format!("; morphs not named morphNN: {}", r.unnamed_morphs.join(", "));
    }
    Ok(said)
}

pub(super) fn models(pe: &mut PackEditor, ui: &mut egui::Ui) {
    let Some(dir) = pe.pack.as_ref().map(|p| p.dir.clone()) else { return };
    ui.label(
        "A model's .glb stands in for the game's: a player on the original skeleton (its animations play it, its \
         shape keys named morphNN move its face), a ball or any other model as it is. Export one to start from.",
    );
    ui.horizontal(|ui| {
        let l = ui.label("Find").id;
        ui.text_edit_singleline(&mut pe.model_filter).labelled_by(l);
        ui.checkbox(&mut pe.only_pack_models, "Only the pack's");
    });
    let filter = pe.model_filter.to_ascii_lowercase();
    let pack = pe.pack.clone();
    let in_pack = |m: &str| pack.as_ref().and_then(|p| p.model(m)).is_some();
    let shown: Vec<usize> = (0..pe.models.len())
        .filter(|&i| {
            pe.models[i].to_ascii_lowercase().contains(&filter) && (!pe.only_pack_models || in_pack(&pe.models[i]))
        })
        .collect();
    ui.columns(2, |cols| {
        let ui = &mut cols[0];
        ui.label(format!("{} models", shown.len()));
        let row = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
        egui::ScrollArea::vertical().id_salt("model list").auto_shrink([false; 2]).show_rows(
            ui,
            row,
            shown.len(),
            |ui, range| {
                for &i in &shown[range] {
                    let m = &pe.models[i];
                    let mark = if in_pack(m) { "* " } else { "  " };
                    if ui.selectable_label(pe.picked_model.as_deref() == Some(m), format!("{mark}{m}")).clicked() {
                        pe.picked_model = Some(m.clone());
                        pe.model_info = None;
                    }
                }
            },
        );
        let ui = &mut cols[1];
        let Some(path) = pe.picked_model.clone() else {
            ui.label("Pick a model. A * marks those the pack replaces.");
            return;
        };
        ui.label(RichText::new(&path).strong());
        let slot = model_slot(&path);
        let file = slot.split('/').fold(dir.clone(), |d, p| d.join(p));
        ui.label(RichText::new(format!("in the pack: {slot}")).small());
        let current = pack.as_ref().and_then(|p| p.model(&path));
        if pe.model_info.as_ref().is_none_or(|(p, _)| p != &path) {
            let info = match &current {
                Some(f) => read_as_game(pe, &path, f).map(|s| format!("The pack's: {s}")),
                None => Ok("The game's own.".into()),
            };
            pe.model_info = Some((path.clone(), info));
        }
        if let Some((_, info)) = &pe.model_info {
            match info {
                Ok(s) => ui.label(s),
                Err(e) => ui.colored_label(qwc_style::BAD, e),
            };
        }
        ui.add_space(6.0);
        let mut changed = false;
        ui.horizontal(|ui| {
            let label = if current.is_some() {
                "Export the game's again (over the pack's)"
            } else {
                "Export into the pack to edit"
            };
            if ui.button(label).on_hover_text("The game's model as a .glb at its slot: open it in Blender").clicked() {
                let textures = qwc_assets::TextureIndex::build(pe.vfs);
                let r = qwc_assets::model::load(pe.vfs, &textures, &path).map_err(|e| e.to_string()).and_then(|m| {
                    let (glb, report) = qwc_assets::gltf_out::export(pe.vfs, &m);
                    std::fs::create_dir_all(file.parent().expect("a folder")).map_err(|e| e.to_string())?;
                    std::fs::write(&file, glb).map_err(|e| e.to_string())?;
                    Ok(report)
                });
                pe.status = match r {
                    Ok(rep) => format!(
                        "Exported {} ({} vertices, {} joints, {} morph channels, {} textures).",
                        file.display(),
                        rep.vertices,
                        rep.joints,
                        rep.morph_channels,
                        rep.textures
                    ),
                    Err(e) => e,
                };
                changed = true;
            }
            if ui.button("Replace with a GLB...").clicked() {
                if let Some(f) = rfd::FileDialog::new().add_filter("glTF binary", &["glb"]).pick_file() {
                    pe.status = match read_as_game(pe, &path, &f) {
                        Ok(s) => qwc_packs::authoring::add_file(&dir, &slot, &f)
                            .map_or_else(|e| e, |to| format!("{} stands in for {path}: {s}.", to.display())),
                        Err(e) => format!("Not added: {e}"),
                    };
                    changed = true;
                }
            }
        });
        ui.add_space(6.0);
        preview(pe, ui, &path, current.as_deref());
        if let Some(f) = &current {
            ui.horizontal(|ui| {
                if ui.button("Show it in its folder").clicked() {
                    if let Some(d) = f.parent() {
                        open_with_system(d);
                    }
                }
                if ui.button("Take it out of the pack").clicked() {
                    pe.status = std::fs::remove_file(f)
                        .map_or_else(|e| e.to_string(), |_| format!("{path} is the game's again."));
                    changed = true;
                }
            });
        }
        if changed {
            pe.model_info = None;
            pe.refresh();
        }
    });
}

/// The game's models: every `.fpm` in the data, sorted.
pub(super) fn game_models(vfs: &qwc_assets::Vfs) -> Vec<String> {
    let mut v: Vec<String> =
        vfs.paths().filter(|p| p.to_ascii_lowercase().ends_with(".fpm")).map(str::to_string).collect();
    v.sort_by_key(|p| p.to_ascii_lowercase());
    v.dedup_by_key(|p| p.to_ascii_lowercase());
    v
}
