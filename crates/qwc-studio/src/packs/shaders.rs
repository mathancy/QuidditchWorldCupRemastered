//! The Packs mode's Shaders tab: the pack's `shaders/shaders.ron` (its material shaders, with the textures and
//! materials each draws, and its post passes in order, with their numbers) edited in place, and each shader's WGSL
//! edited as text, checked as the renderer checks it (`qwc_render::custom`) while it is typed, the engine's
//! interface it is added after shown beside it.

use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText};
use qwc_packs::{MaterialShaderEntry, PostPassEntry, ShaderList};
use qwc_render::custom;

use super::PackEditor;

/// Which entry of the list is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Picked {
    Material(usize),
    Post(usize),
}

/// The tab's state: the pack's list as read, the entry open, its file's text as edited and as saved, the entry's
/// typed fields, and a new shader's name.
#[derive(Default)]
pub(super) struct ShadersView {
    /// The shaders folder the list is from (read again when another pack is open).
    dir: Option<PathBuf>,
    pub(crate) list: ShaderList,
    pub(crate) picked: Option<Picked>,
    pub(crate) text: String,
    saved: String,
    /// The open entry's patterns or numbers, as typed (comma separated).
    textures: String,
    materials: String,
    params: String,
    pub(crate) new_name: String,
    show_interface: bool,
}

impl ShadersView {
    pub(super) fn dirty(&self) -> bool {
        self.picked.is_some() && self.text != self.saved
    }

    /// Write the open shader's file; what was done.
    pub(super) fn save(&mut self) -> Result<String, String> {
        let Some(file) = self.file() else { return Ok(String::new()) };
        std::fs::write(&file, &self.text).map_err(|e| format!("{}: {e}", file.display()))?;
        self.saved = self.text.clone();
        Ok(format!("Saved {}.", file.display()))
    }

    /// The open entry's file.
    fn file(&self) -> Option<PathBuf> {
        let dir = self.dir.as_ref()?;
        let name = match self.picked? {
            Picked::Material(i) => &self.list.materials.get(i)?.file,
            Picked::Post(i) => &self.list.post.get(i)?.file,
        };
        Some(qwc_packs::Stack::shader_file(dir, name).unwrap_or_else(|| dir.join(name)))
    }

    /// Open entry `p`: its file's text and its fields.
    fn pick(&mut self, p: Picked) {
        self.picked = Some(p);
        self.text = self.file().and_then(|f| std::fs::read_to_string(f).ok()).unwrap_or_default();
        self.saved = self.text.clone();
        match p {
            Picked::Material(i) => {
                let m = &self.list.materials[i];
                self.textures = m.textures.join(", ");
                self.materials = m.materials.join(", ");
            }
            Picked::Post(i) => {
                self.params = self.list.post[i].params.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", ");
            }
        }
    }

    /// Write the list; what went wrong.
    fn write_list(&self) -> Option<String> {
        self.dir.as_ref().and_then(|d| self.list.write(d).err())
    }
}

fn split(text: &str) -> Vec<String> {
    text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

pub(super) fn shaders(pe: &mut PackEditor, ui: &mut egui::Ui) {
    let Some(pack_dir) = pe.pack.as_ref().map(|p| p.dir.clone()) else { return };
    let dir = pack_dir.join("shaders");
    let sv = &mut pe.shaders;
    if sv.dir.as_deref() != Some(dir.as_path()) && !sv.dirty() {
        *sv = ShadersView {
            list: ShaderList::read(&dir).unwrap_or_default(),
            dir: Some(dir.clone()),
            new_name: std::mem::take(&mut sv.new_name),
            ..Default::default()
        };
    }
    ui.label(
        "WGSL for the remastered look. A material shader draws the materials it names in place of the stock shading; \
         the post passes run in order over the picture after its tone mapping (the menus and HUD are drawn after them). \
         A shader that does not compile is reported and the stock one draws. See docs/SHADERS.md.",
    );
    let mut status = None;
    ui.columns(2, |cols| {
        let ui = &mut cols[0];
        list_column(sv, &dir, ui, &mut status);
        let ui = &mut cols[1];
        source_column(sv, &pack_dir, ui, &mut status);
    });
    if let Some(s) = status {
        if !s.is_empty() {
            pe.status = s;
        }
        pe.refresh();
    }
}

/// The list: the entries to open, a new shader, the open entry's patterns or numbers, and its place.
fn list_column(sv: &mut ShadersView, dir: &Path, ui: &mut egui::Ui, status: &mut Option<String>) {
    ui.label(RichText::new("Material shaders (the first naming a material draws it)").strong());
    let mut pick = None;
    for (i, m) in sv.list.materials.iter().enumerate() {
        let names: Vec<&str> = m.textures.iter().chain(&m.materials).map(String::as_str).collect();
        let text = format!("{}  ({})", m.file, names.join(", "));
        if ui.selectable_label(sv.picked == Some(Picked::Material(i)), text).clicked() && !sv.dirty() {
            pick = Some(Picked::Material(i));
        }
    }
    ui.label(RichText::new("Post passes, in the order they run").strong());
    for (i, p) in sv.list.post.iter().enumerate() {
        let params: Vec<String> = p.params.iter().map(|v| v.to_string()).collect();
        let text = format!("{}. {}  [{}]", i + 1, p.file, params.join(", "));
        if ui.selectable_label(sv.picked == Some(Picked::Post(i)), text).clicked() && !sv.dirty() {
            pick = Some(Picked::Post(i));
        }
    }
    if let Some(p) = pick {
        sv.pick(p);
    }
    ui.horizontal(|ui| {
        let l = ui.label("New shader").id;
        ui.add(egui::TextEdit::singleline(&mut sv.new_name).hint_text("water").desired_width(140.0)).labelled_by(l);
        let name = sv.new_name.trim().trim_end_matches(".wgsl").to_string();
        let ok =
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') && !sv.dirty();
        for (label, material) in [("Material shader", true), ("Post pass", false)] {
            if ui.add_enabled(ok, egui::Button::new(label)).clicked() {
                let (folder, template) =
                    if material { ("materials", custom::MATERIAL_TEMPLATE) } else { ("post", custom::POST_TEMPLATE) };
                let file = format!("{folder}/{name}.wgsl");
                let path = dir.join(folder).join(format!("{name}.wgsl"));
                let _ = std::fs::create_dir_all(dir.join(folder));
                if !path.exists() {
                    let _ = std::fs::write(&path, template);
                }
                let picked = if material {
                    sv.list.materials.push(MaterialShaderEntry { file, ..Default::default() });
                    Picked::Material(sv.list.materials.len() - 1)
                } else {
                    sv.list.post.push(PostPassEntry { file, ..Default::default() });
                    Picked::Post(sv.list.post.len() - 1)
                };
                *status = Some(sv.write_list().unwrap_or_default());
                sv.pick(picked);
                sv.new_name.clear();
            }
        }
    });
    let Some(picked) = sv.picked else {
        ui.label("Open a shader above, or make one.");
        return;
    };
    let mut changed = false;
    match picked {
        Picked::Material(i) => {
            ui.horizontal(|ui| {
                let l = ui.label("Textures").id;
                let r = ui.add(egui::TextEdit::singleline(&mut sv.textures).hint_text("*water*, stadium/*/gfx/sky*"));
                if r.labelled_by(l).changed() {
                    sv.list.materials[i].textures = split(&sv.textures);
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                let l = ui.label("Materials").id;
                let r = ui.add(egui::TextEdit::singleline(&mut sv.materials).hint_text("lake, glass*"));
                if r.labelled_by(l).changed() {
                    sv.list.materials[i].materials = split(&sv.materials);
                    changed = true;
                }
            });
            ui.label(
                RichText::new(
                    "By the texture's game path or the material's name: * any run of characters, ? any one, case aside.",
                )
                .small(),
            );
        }
        Picked::Post(i) => {
            ui.horizontal(|ui| {
                let l = ui.label("Numbers").id;
                let r = ui.add(egui::TextEdit::singleline(&mut sv.params).hint_text("0.5, 1"));
                if r.labelled_by(l).changed() {
                    let numbers: Vec<f32> = split(&sv.params).iter().filter_map(|v| v.parse().ok()).collect();
                    sv.list.post[i].params = numbers.into_iter().take(custom::PARAMS).collect();
                    changed = true;
                }
            });
            ui.label(RichText::new("Up to eight, read in the shader as param(0u) to param(7u).").small());
        }
    }
    ui.horizontal(|ui| {
        let (index, len) = match picked {
            Picked::Material(i) => (i, sv.list.materials.len()),
            Picked::Post(i) => (i, sv.list.post.len()),
        };
        let mut moved = None;
        if ui.add_enabled(index > 0, egui::Button::new("Earlier")).clicked() {
            moved = Some(index - 1);
        }
        if ui.add_enabled(index + 1 < len, egui::Button::new("Later")).clicked() {
            moved = Some(index + 1);
        }
        if let Some(to) = moved {
            match picked {
                Picked::Material(_) => sv.list.materials.swap(index, to),
                Picked::Post(_) => sv.list.post.swap(index, to),
            }
            sv.picked = Some(match picked {
                Picked::Material(_) => Picked::Material(to),
                Picked::Post(_) => Picked::Post(to),
            });
            changed = true;
        }
        if ui.button("Take it out of the list").on_hover_text("The file stays in the pack").clicked() {
            match picked {
                Picked::Material(i) => drop(sv.list.materials.remove(i)),
                Picked::Post(i) => drop(sv.list.post.remove(i)),
            }
            sv.picked = None;
            sv.text.clear();
            sv.saved.clear();
            changed = true;
        }
    });
    if changed {
        *status = Some(sv.write_list().unwrap_or_default());
    }
}

/// The open shader's WGSL, its check, and the interface it is added after.
fn source_column(sv: &mut ShadersView, pack_dir: &Path, ui: &mut egui::Ui, status: &mut Option<String>) {
    let Some(picked) = sv.picked else { return };
    let name = sv.file().map(|f| f.strip_prefix(pack_dir).unwrap_or(&f).to_string_lossy().replace('\\', "/"));
    let l = ui.label(RichText::new(name.unwrap_or_default()).strong()).id;
    ui.add(egui::TextEdit::multiline(&mut sv.text).code_editor().desired_rows(20).desired_width(f32::INFINITY))
        .labelled_by(l);
    let (checked, interface) = match picked {
        Picked::Material(_) => (custom::check_material(&sv.text), custom::MATERIAL_INTERFACE),
        Picked::Post(_) => (custom::check_post(&sv.text), custom::POST_INTERFACE),
    };
    match checked {
        Ok(()) => ui.colored_label(qwc_style::GOOD, "It compiles."),
        Err(e) => ui.colored_label(qwc_style::BAD, e),
    };
    ui.horizontal(|ui| {
        if ui.add_enabled(sv.dirty(), egui::Button::new("Save the shader")).clicked() {
            *status = Some(sv.save().unwrap_or_else(|e| e));
        }
        if ui.add_enabled(sv.dirty(), egui::Button::new("Revert")).clicked() {
            sv.text = sv.saved.clone();
        }
        ui.checkbox(&mut sv.show_interface, "Show the engine's interface");
    });
    if sv.show_interface {
        egui::ScrollArea::vertical().id_salt("interface").max_height(320.0).show(ui, |ui| {
            ui.label(RichText::new(interface).monospace().small());
        });
    }
}
