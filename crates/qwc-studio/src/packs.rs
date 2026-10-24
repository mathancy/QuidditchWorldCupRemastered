//! The Studio's Packs mode (`docs/LAUNCHER.md`, "QWC Studio"): a resource pack made and changed. A pack is opened
//! (a folder, or one of the library's installed packs) or made new; its manifest edited; the game's textures browsed
//! and one exported into the pack as a PNG to paint (then seen beside the original, read again when it changes on
//! disk), or replaced by a picked PNG; movies and stadium scenery added; the pack checked as the game reads it,
//! built into a `.qwcpack`, installed into the library and played.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use eframe::egui::{self, RichText};
use qwc_assets::Vfs;
use qwc_packs::{authoring, Kind, Manifest, Pack, Severity};

mod models;
mod shaders;
mod tuning;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Contents,
    Textures,
    Movies,
    Stadiums,
    Models,
    Shaders,
    Tuning,
    Build,
}

/// An image shown in the mode: its texture and size, and for a file what it was read from and when.
struct Shown {
    handle: egui::TextureHandle,
    size: [u32; 2],
    file: Option<(PathBuf, Option<SystemTime>)>,
}

/// A build's answer: the file and its warnings.
type Built = Result<(PathBuf, Vec<String>), String>;

/// The new-pack form.
#[derive(Default)]
struct NewPack {
    id: String,
    name: String,
    /// Made in the library's packs folder (installed and read by the game as it is), else in a folder picked.
    in_library: bool,
}

pub struct PackEditor {
    vfs: &'static Vfs,
    pub pack: Option<Pack>,
    /// The manifest being edited, and its lists as typed.
    manifest: Manifest,
    authors: String,
    requires: String,
    conflicts: String,
    found: Vec<(Severity, String)>,
    slots: Vec<String>,
    tab: Tab,
    /// The game's textures (every `.fsh`), the filter, the one picked and its two pictures.
    textures: Vec<String>,
    filter: String,
    only_pack: bool,
    picked: Option<String>,
    original: Option<Shown>,
    painted: Option<Shown>,
    /// The texture the pack replaces, normalised, and its PNG.
    replaced: BTreeMap<String, PathBuf>,
    /// The disc's movies and stadium folders.
    movies: Vec<String>,
    stadiums: Vec<String>,
    /// Building: the folder the `.qwcpack` goes to, the build running and its answer.
    out_dir: String,
    building: Option<Arc<Mutex<Option<Built>>>>,
    built: Option<Built>,
    new_pack: Option<NewPack>,
    /// The game's models, the filter, the one picked and what the game makes of the pack's file for it.
    models: Vec<String>,
    model_filter: String,
    only_pack_models: bool,
    picked_model: Option<String>,
    model_info: Option<(String, Result<String, String>)>,
    /// The picked model drawn by the engine, and its turn in degrees.
    model_preview: Option<models::Preview>,
    preview_yaw: f32,
    /// The tuning scripts' editor.
    tuning: tuning::TuningView,
    /// The shaders' editor.
    shaders: shaders::ShadersView,
    /// The library's installed packs (name, id, folder), read again when packs come or go.
    installed: Vec<(String, String, PathBuf)>,
    pub status: String,
}

/// The pack's slots counted by kind.
fn counts(slots: &[String]) -> String {
    let mut by: BTreeMap<&str, usize> = BTreeMap::new();
    for s in slots {
        *by.entry(s.split('/').next().unwrap_or(s)).or_default() += 1;
    }
    let parts: Vec<String> = by.iter().map(|(k, n)| format!("{n} {k}")).collect();
    if parts.is_empty() {
        "nothing yet".into()
    } else {
        parts.join(", ")
    }
}

/// `a, b , c` as a list.
fn list(text: &str) -> Vec<String> {
    text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

/// Open `path` with the system's own program for it (a folder in the file manager).
pub fn open_with_system(path: &Path) {
    let _ = if cfg!(windows) {
        if path.is_dir() {
            std::process::Command::new("explorer").arg(path).spawn()
        } else {
            std::process::Command::new("cmd").args(["/C", "start", ""]).arg(path).spawn()
        }
    } else {
        std::process::Command::new("xdg-open").arg(path).spawn()
    };
}

/// The next version: `which` 0 the patch, 1 the minor, 2 the major.
fn bump(version: &str, which: usize) -> String {
    let Ok(mut v) = semver::Version::parse(version) else { return version.to_string() };
    match which {
        0 => v.patch += 1,
        1 => (v.minor, v.patch) = (v.minor + 1, 0),
        _ => (v.major, v.minor, v.patch) = (v.major + 1, 0, 0),
    }
    v.pre = semver::Prerelease::EMPTY;
    v.to_string()
}

impl PackEditor {
    pub fn new(vfs: &'static Vfs, open: Option<&Path>) -> PackEditor {
        let mut textures: Vec<String> =
            vfs.paths().filter(|p| p.to_ascii_lowercase().ends_with(".fsh")).map(str::to_string).collect();
        textures.sort_by_key(|p| p.to_ascii_lowercase());
        textures.dedup_by_key(|p| p.to_ascii_lowercase());
        let stem = |p: &str| p.rsplit('/').next().and_then(|f| f.rsplit_once('.')).map(|(s, _)| s.to_string());
        let mut movies: Vec<String> =
            vfs.paths().filter(|p| p.to_ascii_lowercase().ends_with(".tgq")).filter_map(stem).collect();
        movies.sort_by_key(|m| m.to_ascii_lowercase());
        movies.dedup_by_key(|m| m.to_ascii_lowercase());
        let mut stadiums: Vec<String> = vfs
            .paths()
            .filter_map(|p| match p.split('/').collect::<Vec<_>>()[..] {
                [s, name, g, f] if s.eq_ignore_ascii_case("stadium") && g.eq_ignore_ascii_case("gfx") => {
                    f.eq_ignore_ascii_case("stadium.fpm").then(|| name.to_ascii_lowercase())
                }
                _ => None,
            })
            .collect();
        stadiums.sort();
        stadiums.dedup();
        let mut pe = PackEditor {
            vfs,
            pack: None,
            manifest: Manifest::new("you.pack", "A pack"),
            authors: String::new(),
            requires: String::new(),
            conflicts: String::new(),
            found: Vec::new(),
            slots: Vec::new(),
            tab: Tab::Contents,
            textures,
            filter: String::new(),
            only_pack: false,
            picked: None,
            original: None,
            painted: None,
            replaced: BTreeMap::new(),
            movies,
            stadiums,
            out_dir: String::new(),
            building: None,
            built: None,
            new_pack: None,
            models: models::game_models(vfs),
            model_filter: String::new(),
            only_pack_models: false,
            picked_model: None,
            model_info: None,
            model_preview: None,
            preview_yaw: 30.0,
            tuning: tuning::TuningView::default(),
            shaders: shaders::ShadersView::default(),
            installed: Vec::new(),
            status: String::new(),
        };
        pe.read_installed();
        if let Some(dir) = open {
            pe.open(dir);
        }
        pe
    }

    /// The library's packs, read again.
    fn read_installed(&mut self) {
        let mut p = qwc_packs::current().profile.clone();
        if let Some((packs, _)) = Self::library() {
            p.packs_dir = Some(packs);
        }
        self.installed = p.installed().0.into_iter().map(|(id, pack)| (pack.manifest.name, id, pack.dir)).collect();
    }

    /// The manifest as edited differs from the pack's, or a tuning script or a shader has changes.
    pub fn dirty(&self) -> bool {
        self.pack.as_ref().is_some_and(|p| p.manifest != self.edited()) || self.tuning.dirty() || self.shaders.dirty()
    }

    /// Only the manifest differs from the pack's.
    fn manifest_dirty(&self) -> bool {
        self.pack.as_ref().is_some_and(|p| p.manifest != self.edited())
    }

    /// The manifest with the typed lists in.
    fn edited(&self) -> Manifest {
        Manifest {
            authors: list(&self.authors),
            requires: list(&self.requires),
            conflicts: list(&self.conflicts),
            ..self.manifest.clone()
        }
    }

    pub fn open(&mut self, dir: &Path) {
        match Pack::open(dir) {
            Ok(p) => {
                self.manifest = p.manifest.clone();
                self.authors = p.manifest.authors.join(", ");
                self.requires = p.manifest.requires.join(", ");
                self.conflicts = p.manifest.conflicts.join(", ");
                self.out_dir = p.dir.parent().map_or(String::new(), |d| d.display().to_string());
                self.status = format!("Opened {}.", p.dir.display());
                self.pack = Some(p);
                self.painted = None;
                self.built = None;
                self.refresh();
            }
            Err(e) => self.status = e,
        }
    }

    /// Read the pack's parts again: its findings, its slots, the textures it replaces.
    pub fn refresh(&mut self) {
        let Some(p) = &self.pack else { return };
        self.found = qwc_game::packs::findings(p, Some(self.vfs));
        self.slots = p.slots();
        self.replaced = authoring::texture_list(&p.dir)
            .is_file()
            .then(|| qwc_assets::remaster::Remastered::read(&authoring::texture_list(&p.dir)).ok())
            .flatten()
            .map(|r| r.textures().map(|t| (t.to_string(), r.png(t).expect("listed").to_path_buf())).collect())
            .unwrap_or_default();
    }

    pub fn save(&mut self) {
        if self.tuning.dirty() {
            self.status = self.tuning.save().unwrap_or_else(|e| e);
        }
        if self.shaders.dirty() {
            self.status = self.shaders.save().unwrap_or_else(|e| e);
        }
        if !self.manifest_dirty() {
            return;
        }
        let edited = self.edited();
        let Some(p) = &mut self.pack else { return };
        if let Err(e) = edited.validate() {
            self.status = format!("Not saved: {e}");
            return;
        }
        let old = std::mem::replace(&mut p.manifest, edited);
        match p.save() {
            Ok(()) => self.status = format!("Saved {}.", p.dir.join(qwc_packs::MANIFEST).display()),
            Err(e) => {
                p.manifest = old;
                self.status = e;
            }
        }
        self.refresh();
    }

    /// The library's packs folder and the profile's file (none without a library).
    fn library() -> Option<(PathBuf, PathBuf)> {
        let stack = qwc_packs::current();
        let profile = stack.profile_path.clone().or_else(qwc_packs::Profile::default_path)?;
        Some((stack.profile.packs_dir()?, profile))
    }

    /// Whether the open pack is one of the library's (the game reads it as it is, when it is on).
    fn installed(&self) -> bool {
        let (Some(p), Some((packs, _))) = (&self.pack, Self::library()) else { return false };
        p.dir.parent().is_some_and(|d| d == packs)
    }

    /// The profile with the open pack on, at the top; written.
    fn turn_on(&mut self) -> Result<(), String> {
        let (Some(p), Some((_, path))) = (&self.pack, Self::library()) else { return Err("no library".into()) };
        let mut profile = if path.is_file() { qwc_packs::Profile::read(&path)? } else { qwc_packs::Profile::default() };
        profile.put_on_top(&p.manifest.id);
        profile.write(&path)
    }
}

pub fn ui(pe: &mut PackEditor, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    ui_inner(pe, ui);
    new_pack_window(pe, &ctx);
}

fn ui_inner(pe: &mut PackEditor, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    poll_build(pe);
    egui::Panel::left("pack").default_size(360.0).show(ui, |ui| {
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| side(pe, ui));
    });
    // the page in the night blue beside the panel, as the launcher's
    let page = egui::Frame::central_panel(ui.style()).fill(qwc_style::BACKGROUND);
    egui::CentralPanel::default().frame(page).show(ui, |ui| {
        if pe.pack.is_none() {
            ui.label(qwc_style::heading("No pack open", 20.0));
            ui.label("Make a new pack or open one on the left. A pack lays its files over the game's: textures, movies, screens, fonts, looks and stadiums.");
            return;
        }
        ui.horizontal(|ui| {
            for (tab, name) in [
                (Tab::Contents, "Contents"),
                (Tab::Textures, "Textures"),
                (Tab::Movies, "Movies"),
                (Tab::Stadiums, "Stadiums"),
                (Tab::Models, "Models"),
                (Tab::Shaders, "Shaders"),
                (Tab::Tuning, "Tuning"),
                (Tab::Build, "Build and play"),
            ] {
                if qwc_style::tab(ui, pe.tab == tab, name, 16.0).clicked() {
                    pe.tab = tab;
                }
            }
        });
        ui.separator();
        match pe.tab {
            Tab::Contents => contents(pe, ui),
            Tab::Textures => textures(pe, ui),
            Tab::Movies => movies(pe, ui),
            Tab::Stadiums => stadiums(pe, ui),
            Tab::Models => models::models(pe, ui),
            Tab::Shaders => shaders::shaders(pe, ui),
            Tab::Tuning => tuning::tuning(pe, ui),
            Tab::Build => build(pe, ui),
        }
    });
    if pe.building.is_some() {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

fn side(pe: &mut PackEditor, ui: &mut egui::Ui) {
    ui.label(qwc_style::heading("Pack", 20.0));
    ui.horizontal(|ui| {
        if ui.button("New pack...").clicked() {
            pe.new_pack = Some(NewPack { in_library: true, ..NewPack::default() });
        }
        if ui.button("Open a folder...").clicked() {
            if let Some(d) = rfd::FileDialog::new().pick_folder() {
                pe.open(&d);
            }
        }
    });
    if !pe.installed.is_empty() {
        let mut open = None;
        ui.collapsing("Your installed packs", |ui| {
            for (name, id, dir) in &pe.installed {
                if ui.button(format!("{name}  ({id})")).clicked() {
                    open = Some(dir.clone());
                }
            }
        });
        if let Some(d) = open {
            pe.open(&d);
        }
    }
    let Some(dir) = pe.pack.as_ref().map(|p| p.dir.clone()) else { return };
    ui.separator();
    ui.label(RichText::new(dir.display().to_string()).small());
    ui.horizontal(|ui| {
        if ui.button("Show the folder").clicked() {
            open_with_system(&dir);
        }
        if ui.button("Read again").clicked() {
            pe.refresh();
        }
    });
    ui.add_space(6.0);
    egui::Grid::new("manifest").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        let l = ui.label("Id").id;
        ui.text_edit_singleline(&mut pe.manifest.id)
            .labelled_by(l)
            .on_hover_text("author.name: lower case, digits, - and _");
        ui.end_row();
        let l = ui.label("Name").id;
        ui.text_edit_singleline(&mut pe.manifest.name).labelled_by(l);
        ui.end_row();
        let l = ui.label("Version").id;
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut pe.manifest.version).desired_width(80.0)).labelled_by(l);
            for (i, what) in ["patch", "minor", "major"].iter().enumerate() {
                if ui.small_button(format!("+{what}")).clicked() {
                    pe.manifest.version = bump(&pe.manifest.version, i);
                }
            }
        });
        ui.end_row();
        let l = ui.label("Authors").id;
        ui.text_edit_singleline(&mut pe.authors).labelled_by(l).on_hover_text("Separated by commas");
        ui.end_row();
        ui.label("Kind");
        ui.horizontal(|ui| {
            ui.radio_value(&mut pe.manifest.kind, Kind::Visual, "Visual");
            ui.radio_value(&mut pe.manifest.kind, Kind::Gameplay, "Gameplay")
                .on_hover_text("Changes how the game plays (tuning/): marked in the launcher, recorded in results");
        });
        ui.end_row();
        let l = ui.label("Remaster").id;
        ui.text_edit_singleline(&mut pe.manifest.engine)
            .labelled_by(l)
            .on_hover_text("The versions it works with, as >=0.1");
        ui.end_row();
        let l = ui.label("Source").id;
        let mut source = pe.manifest.source.clone().unwrap_or_default();
        if ui
            .text_edit_singleline(&mut source)
            .labelled_by(l)
            .on_hover_text("Where the pack and its updates come from")
            .changed()
        {
            pe.manifest.source = (!source.trim().is_empty()).then_some(source);
        }
        ui.end_row();
        let l = ui.label("Needs").id;
        ui.text_edit_singleline(&mut pe.requires).labelled_by(l).on_hover_text("Other packs' ids, separated by commas");
        ui.end_row();
        let l = ui.label("Not with").id;
        ui.text_edit_singleline(&mut pe.conflicts).labelled_by(l).on_hover_text("Packs' ids it must not be on with");
        ui.end_row();
    });
    let l = ui.label("Description").id;
    ui.add(egui::TextEdit::multiline(&mut pe.manifest.description).desired_rows(3).desired_width(f32::INFINITY))
        .labelled_by(l);
    ui.horizontal(|ui| {
        if ui.add_enabled(pe.dirty(), egui::Button::new("Save    Ctrl+S")).clicked() {
            pe.save();
        }
        if ui.add_enabled(pe.dirty(), egui::Button::new("Revert")).clicked() {
            pe.open(&dir);
        }
    });
    ui.separator();
    ui.label(RichText::new(format!("Changes: {}", counts(&pe.slots))).strong());
    if pe.found.is_empty() {
        ui.colored_label(qwc_style::GOOD, "No problems found.");
    }
    for (sev, text) in &pe.found {
        let colour = if *sev == Severity::Error { qwc_style::BAD } else { qwc_style::WARN };
        ui.colored_label(colour, text);
    }
}

fn contents(pe: &mut PackEditor, ui: &mut egui::Ui) {
    ui.label("What the pack fills, by kind. The game reads these when the pack is on.");
    let mut by: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for s in &pe.slots {
        let (kind, rest) = s.split_once('/').unwrap_or((s, ""));
        by.entry(kind.to_string()).or_default().push(rest.to_string());
    }
    egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
        if by.is_empty() {
            ui.label("Nothing yet: add textures, movies or stadiums in the other tabs, or files in the pack's folder.");
        }
        for (kind, items) in &by {
            ui.collapsing(format!("{kind} ({})", items.len()), |ui| {
                for i in items {
                    ui.label(i);
                }
            });
        }
    });
}

/// An image file or a game texture uploaded for showing.
fn show(ctx: &egui::Context, name: &str, width: u32, height: u32, rgba: &[u8]) -> Option<Shown> {
    (width > 0 && height > 0).then(|| {
        let image = egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], rgba);
        Shown { handle: ctx.load_texture(name, image, egui::TextureOptions::LINEAR), size: [width, height], file: None }
    })
}

fn modified(p: &Path) -> Option<SystemTime> {
    p.metadata().ok()?.modified().ok()
}

fn textures(pe: &mut PackEditor, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let Some(dir) = pe.pack.as_ref().map(|p| p.dir.clone()) else { return };
    ui.horizontal(|ui| {
        let l = ui.label("Find").id;
        ui.text_edit_singleline(&mut pe.filter).labelled_by(l);
        ui.checkbox(&mut pe.only_pack, "Only the pack's");
        ui.label(format!("{} in the pack", pe.replaced.len()));
    });
    let filter = pe.filter.to_ascii_lowercase();
    let shown: Vec<usize> = (0..pe.textures.len())
        .filter(|&i| {
            let t = pe.textures[i].to_ascii_lowercase();
            t.contains(&filter) && (!pe.only_pack || pe.replaced.contains_key(&qwc_assets::vfs::normalize(&t)))
        })
        .collect();
    ui.columns(2, |cols| {
        let ui = &mut cols[0];
        ui.label(format!("{} textures", shown.len()));
        let row = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
        egui::ScrollArea::vertical().id_salt("texture list").auto_shrink([false; 2]).show_rows(
            ui,
            row,
            shown.len(),
            |ui, range| {
                for &i in &shown[range] {
                    let t = &pe.textures[i];
                    let mark = if pe.replaced.contains_key(&qwc_assets::vfs::normalize(t)) { "* " } else { "  " };
                    if ui.selectable_label(pe.picked.as_deref() == Some(t), format!("{mark}{t}")).clicked() {
                        pe.picked = Some(t.clone());
                        pe.original = None;
                        pe.painted = None;
                    }
                }
            },
        );
        let ui = &mut cols[1];
        let Some(t) = pe.picked.clone() else {
            ui.label("Pick a texture. A * marks those the pack replaces.");
            return;
        };
        ui.label(RichText::new(&t).strong());
        if pe.original.is_none() {
            pe.original = qwc_assets::texture::load(pe.vfs, &t)
                .ok()
                .and_then(|x| show(&ctx, &format!("original {t}"), x.width, x.height, &x.levels[0]));
        }
        let key = qwc_assets::vfs::normalize(&t);
        let png = pe.replaced.get(&key).cloned();
        // the pack's PNG, read again when it changes on disk
        if let Some(p) = &png {
            let stale =
                pe.painted.as_ref().and_then(|s| s.file.clone()).is_none_or(|(f, when)| &f != p || when != modified(p));
            if stale {
                pe.painted = qwc_assets::remaster::load_png(p).ok().and_then(|x| {
                    let mut s = show(&ctx, &format!("pack {}", p.display()), x.width, x.height, &x.levels[0])?;
                    s.file = Some((p.clone(), modified(p)));
                    Some(s)
                });
            }
        } else {
            pe.painted = None;
        }
        ui.horizontal(|ui| {
            for (label, s) in [("The game's", &pe.original), ("The pack's", &pe.painted)] {
                ui.vertical(|ui| {
                    match s {
                        Some(s) => {
                            ui.label(format!("{label}: {} x {}", s.size[0], s.size[1]));
                            let scale = (256.0 / s.size[0].max(s.size[1]) as f32).min(4.0);
                            ui.image((s.handle.id(), egui::vec2(s.size[0] as f32 * scale, s.size[1] as f32 * scale)));
                        }
                        None => {
                            ui.label(format!("{label}: none"));
                        }
                    };
                });
            }
        });
        if let (Some(o), Some(p)) = (&pe.original, &pe.painted) {
            let (a, b) = (o.size[0] as f32 / o.size[1] as f32, p.size[0] as f32 / p.size[1] as f32);
            if (a - b).abs() > 0.01 {
                ui.colored_label(
                    qwc_style::WARN,
                    "Its proportions differ from the game's: it is drawn fitted inside the original's area.",
                );
            }
        }
        ui.add_space(6.0);
        match png {
            None => {
                ui.horizontal(|ui| {
                    if ui
                        .button("Export into the pack to paint")
                        .on_hover_text("The game's picture as a PNG in the pack, listed in textures.txt")
                        .clicked()
                    {
                        let to = authoring::texture_png(&dir, &t);
                        let r = qwc_assets::texture::load(pe.vfs, &t)
                            .map_err(|e| e.to_string())
                            .and_then(|x| qwc_assets::remaster::save_png(&to, x.width, x.height, &x.levels[0]))
                            .and_then(|_| authoring::add_texture(&dir, &t, &to));
                        pe.status = r.map_or_else(
                            |e| e,
                            |_| format!("Exported {}: paint it, it is shown here as it changes.", to.display()),
                        );
                        pe.refresh();
                    }
                    if ui.button("Replace with a PNG...").clicked() {
                        if let Some(f) = rfd::FileDialog::new().add_filter("PNG", &["png"]).pick_file() {
                            let to = authoring::texture_png(&dir, &t);
                            let r = std::fs::create_dir_all(to.parent().expect("a folder"))
                                .and_then(|_| std::fs::copy(&f, &to))
                                .map_err(|e| e.to_string())
                                .and_then(|_| authoring::add_texture(&dir, &t, &to));
                            pe.status = r.map_or_else(|e| e, |_| format!("{} replaces {t}.", to.display()));
                            pe.refresh();
                        }
                    }
                });
            }
            Some(p) => {
                ui.label(RichText::new(p.display().to_string()).small());
                ui.horizontal(|ui| {
                    if ui.button("Open the PNG").clicked() {
                        open_with_system(&p);
                    }
                    if ui.button("Show it in its folder").clicked() {
                        if let Some(d) = p.parent() {
                            open_with_system(d);
                        }
                    }
                    if ui
                        .button("Take it out of the pack")
                        .on_hover_text(
                            "Its line goes from textures.txt; the PNG is deleted when no other texture uses it",
                        )
                        .clicked()
                    {
                        match authoring::remove_texture(&dir, &t) {
                            Ok(freed) => {
                                if let Some(f) = freed {
                                    let _ = std::fs::remove_file(f);
                                }
                                pe.status = format!("{t} is the game's again.");
                            }
                            Err(e) => pe.status = e,
                        }
                        pe.painted = None;
                        pe.refresh();
                    }
                });
            }
        }
    });
    // the painted file is looked at again now and then
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
}

fn movies(pe: &mut PackEditor, ui: &mut egui::Ui) {
    let Some(dir) = pe.pack.as_ref().map(|p| p.dir.clone()) else { return };
    ui.label("An MP4 named as the game's movie plays in its place (the remastered movies' setting on).");
    egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
        egui::Grid::new("movies").num_columns(3).striped(true).show(ui, |ui| {
            for m in pe.movies.clone() {
                let file = dir.join("movies").join(format!("{m}.mp4"));
                ui.label(&m);
                if file.is_file() {
                    let mb = file.metadata().map_or(0, |x| x.len()) as f64 / 1e6;
                    ui.label(format!("{m}.mp4 ({mb:.0} MB)"));
                    if ui.button("Take it out").clicked() {
                        pe.status = std::fs::remove_file(&file)
                            .map_or_else(|e| e.to_string(), |_| format!("{m}: the game's again."));
                        pe.refresh();
                    }
                } else {
                    ui.label(RichText::new("the game's").weak());
                    if ui.button("Add an MP4...").clicked() {
                        if let Some(f) = rfd::FileDialog::new().add_filter("MP4", &["mp4"]).pick_file() {
                            pe.status = authoring::add_file(&dir, &format!("movies/{m}.mp4"), &f)
                                .map_or_else(|e| e, |to| format!("{} plays in place of {m}.", to.display()));
                            pe.refresh();
                        }
                    }
                }
                ui.end_row();
            }
        });
    });
}

fn stadiums(pe: &mut PackEditor, ui: &mut egui::Ui) {
    let Some(dir) = pe.pack.as_ref().map(|p| p.dir.clone()) else { return };
    ui.label(
        "A stadium's scenery as GLB files: stadium.glb the arena, any others its surroundings. sum_fe is the front \
         end's stadium; the others stand in for the stadium in matches and their cutscenes. Its settings say whether \
         the game's hoops and flags are still drawn, how much the sky grows round it and how far the camera sees.",
    );
    egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
        egui::Grid::new("stadiums").num_columns(3).striped(true).show(ui, |ui| {
            for s in pe.stadiums.clone() {
                let folder = dir.join("stadiums").join(&s);
                let glbs: Vec<String> = std::fs::read_dir(&folder)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.to_ascii_lowercase().ends_with(".glb"))
                    .collect();
                ui.label(&s);
                if glbs.is_empty() {
                    ui.label(RichText::new("the game's").weak());
                } else {
                    ui.label(glbs.join(", "));
                }
                ui.horizontal(|ui| {
                    if ui.button("Add GLB files...").clicked() {
                        if let Some(files) = rfd::FileDialog::new().add_filter("glTF binary", &["glb"]).pick_files() {
                            let mut said = Vec::new();
                            for f in files {
                                let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                                // read as the game reads it before it goes in
                                match qwc_assets::glb::load_scenery(&f) {
                                    Ok(_) => match authoring::add_file(&dir, &format!("stadiums/{s}/{name}"), &f) {
                                        Ok(_) => said.push(format!("{name} added")),
                                        Err(e) => said.push(e),
                                    },
                                    Err(e) => said.push(format!("{name}: {e}")),
                                }
                            }
                            pe.status = said.join("; ");
                            pe.refresh();
                        }
                    }
                    if !glbs.is_empty() && ui.button("Take them out").clicked() {
                        pe.status = std::fs::remove_dir_all(&folder)
                            .map_or_else(|e| e.to_string(), |_| format!("{s}: the game's again."));
                        pe.refresh();
                    }
                });
                ui.end_row();
                if !glbs.is_empty() {
                    // the folder's stadium.ron, written as it changes
                    let mut c = qwc_packs::StadiumConfig::read(&folder).unwrap_or_default();
                    let before = c;
                    ui.label("");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut c.hoops, "the game's hoops too");
                        ui.checkbox(&mut c.flags, "the game's flags too");
                    });
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut c.sky_scale).range(1.0..=20.0).speed(0.1).prefix("sky x"));
                        ui.add(egui::DragValue::new(&mut c.far).range(1000.0..=50000.0).speed(50.0).prefix("sees "));
                    });
                    ui.end_row();
                    if c != before {
                        if let Err(e) = c.write(&folder) {
                            pe.status = e;
                        }
                    }
                }
            }
        });
    });
}

fn poll_build(pe: &mut PackEditor) {
    let done = pe.building.as_ref().and_then(|b| b.lock().ok()?.take());
    if let Some(r) = done {
        pe.building = None;
        pe.status = match &r {
            Ok((f, _)) => format!("Built {}.", f.display()),
            Err(e) => format!("Not built: {e}"),
        };
        pe.built = Some(r);
    }
}

fn build(pe: &mut PackEditor, ui: &mut egui::Ui) {
    let Some(pack) = pe.pack.clone() else { return };
    let errors = pe.found.iter().filter(|(s, _)| *s == Severity::Error).count();
    ui.label(qwc_style::heading("Play it", 20.0));
    if pe.installed() {
        ui.label("This pack is in your library: the game reads it as it is, whenever it is on.");
    } else {
        ui.label(
            "This pack is outside your library: build and install it to play it, or make new packs in the library.",
        );
    }
    ui.horizontal(|ui| {
        if pe.installed() && ui.button("Turn it on, at the top, and play").clicked() {
            pe.status = pe.turn_on().and_then(|_| play()).map_or_else(|e| e, |_| "The game is starting.".into());
        }
        if ui.add_enabled(errors == 0, egui::Button::new("Install into my library and play")).clicked() {
            let r = (|| {
                let (packs, _) = PackEditor::library().ok_or("no library")?;
                let tmp = std::env::temp_dir().join(format!("qwc-studio-{}", std::process::id()));
                let _ = std::fs::create_dir_all(&tmp);
                let (file, _) = qwc_packs::build(&pack.dir, &tmp)?;
                let installed = qwc_packs::install(&file, &packs)?;
                let _ = std::fs::remove_file(&file);
                pe.turn_on()?;
                play()?;
                Ok::<_, String>(installed.dir)
            })();
            pe.status = r.map_or_else(|e| e, |d| format!("Installed in {} and on; the game is starting.", d.display()));
            pe.read_installed();
        }
    });
    ui.separator();
    ui.label(qwc_style::heading("Build the .qwcpack", 20.0));
    ui.label("Checked first; an error stops it. The file is what players install.");
    ui.horizontal(|ui| {
        let l = ui.label("Into").id;
        ui.add(egui::TextEdit::singleline(&mut pe.out_dir).desired_width(400.0)).labelled_by(l);
        if ui.button("Browse...").clicked() {
            if let Some(d) = rfd::FileDialog::new().pick_folder() {
                pe.out_dir = d.display().to_string();
            }
        }
    });
    let busy = pe.building.is_some();
    ui.horizontal(|ui| {
        if ui.add_enabled(!busy && errors == 0 && !pe.dirty(), egui::Button::new("Build")).clicked() {
            let slot = Arc::new(Mutex::new(None));
            let (s, dir, out) = (slot.clone(), pack.dir.clone(), PathBuf::from(pe.out_dir.trim()));
            std::thread::spawn(move || {
                let r = qwc_packs::build(&dir, &out);
                if let Ok(mut g) = s.lock() {
                    *g = Some(r);
                }
            });
            pe.building = Some(slot);
        }
        if busy {
            ui.spinner();
        }
        if errors > 0 {
            ui.colored_label(qwc_style::BAD, format!("{errors} problem(s) to fix first (on the left)"));
        } else if pe.dirty() {
            ui.label("Save first (Ctrl+S).");
        }
    });
    match &pe.built {
        Some(Ok((f, warnings))) => {
            let mb = f.metadata().map_or(0, |m| m.len()) as f64 / 1e6;
            ui.colored_label(qwc_style::GOOD, format!("{} ({mb:.1} MB)", f.display()));
            for w in warnings {
                ui.colored_label(qwc_style::WARN, w);
            }
            if ui.button("Show it in its folder").clicked() {
                if let Some(d) = f.parent() {
                    open_with_system(d);
                }
            }
        }
        Some(Err(e)) => {
            ui.colored_label(qwc_style::BAD, e);
        }
        None => {}
    }
}

/// Start the game's front end (`qwc-game` beside the Studio) on the player's profile.
fn play() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let game = exe.with_file_name(if cfg!(windows) { "qwc-game.exe" } else { "qwc-game" });
    if !game.is_file() {
        return Err(format!("{} is not built (cargo build --release -p qwc-game)", game.display()));
    }
    std::process::Command::new(game).args(["frontend", "boot"]).spawn().map(|_| ()).map_err(|e| e.to_string())
}

fn new_pack_window(pe: &mut PackEditor, ctx: &egui::Context) {
    let Some(np) = &mut pe.new_pack else { return };
    let mut close = false;
    let mut made = None;
    egui::Modal::new(egui::Id::unique("new pack")).show(ctx, |ui| {
        ui.set_width(460.0);
        ui.label(qwc_style::heading("A new pack", 20.0));
        egui::Grid::new("new pack form").num_columns(2).show(ui, |ui| {
            let l = ui.label("Id").id;
            ui.text_edit_singleline(&mut np.id).labelled_by(l).on_hover_text("author.name, as you.my-pack");
            ui.end_row();
            let l = ui.label("Name").id;
            ui.text_edit_singleline(&mut np.name).labelled_by(l);
            ui.end_row();
        });
        ui.checkbox(&mut np.in_library, "In my library (the game reads it as it is made)");
        let valid = qwc_packs::valid_id(&np.id) && !np.name.trim().is_empty();
        if !np.id.is_empty() && !qwc_packs::valid_id(&np.id) {
            ui.colored_label(qwc_style::WARN, "The id is author.name: lower case letters, digits, - and _.");
        }
        ui.horizontal(|ui| {
            if ui.add_enabled(valid, egui::Button::new("Make it")).clicked() {
                let dir = if np.in_library {
                    PackEditor::library().map(|(packs, _)| packs.join(&np.id))
                } else {
                    rfd::FileDialog::new().pick_folder().map(|d| d.join(&np.id))
                };
                made = dir.map(|d| (d, Manifest::new(&np.id, np.name.trim()), np.in_library));
                close = true;
            }
            if ui.button("Cancel").clicked() {
                close = true;
            }
        });
    });
    if close {
        pe.new_pack = None;
    }
    if let Some((dir, m, in_library)) = made {
        match Pack::create(&dir, m) {
            Ok(_) => {
                pe.open(&dir);
                pe.read_installed();
                if in_library {
                    if let Err(e) = pe.turn_on() {
                        pe.status = e;
                    }
                }
            }
            Err(e) => pe.status = e,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With the game data (`$QWC_DATA`, else the repo's `original/cd_files`): a new pack, a game texture exported
    /// into it through the Textures tab's button, listed and read as the game reads it, then taken out again. With
    /// `$QWC_SHOTS`, the tab is drawn there (`packs.png`).
    #[test]
    fn exports_a_texture_into_a_pack() {
        use egui_kittest::kittest::Queryable;
        let dir = std::env::var_os("QWC_DATA").map_or_else(|| PathBuf::from("../../original/cd_files"), PathBuf::from);
        let Ok(vfs) = Vfs::open(&dir) else { return };
        let vfs: &'static Vfs = Box::leak(Box::new(vfs));
        let root = std::env::temp_dir().join(format!("qwc-studio-pack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pack_dir = root.join("me.test");
        let mut m = Manifest::new("me.test", "Test pack");
        m.description = "Brighter menus.".into();
        Pack::create(&pack_dir, m).unwrap();
        let mut pe = PackEditor::new(vfs, Some(&pack_dir));
        let texture = "frontend/Common/Controls/arrow_up.fsh";
        assert!(pe.textures.iter().any(|t| t.eq_ignore_ascii_case(texture)));
        pe.tab = Tab::Textures;
        pe.filter = "controls/arrow".into();
        pe.picked = pe.textures.iter().find(|t| t.eq_ignore_ascii_case(texture)).cloned();
        let shots = std::env::var_os("QWC_SHOTS").map(PathBuf::from);
        let mut builder = egui_kittest::Harness::builder().with_size([1500.0, 860.0]);
        if shots.is_some() {
            builder = builder.wgpu();
        }
        let mut h = builder.build_ui_state(|ui, pe: &mut PackEditor| ui_for_tests(pe, ui), pe);
        h.run();
        h.get_by_label("Export into the pack to paint").click();
        h.run();
        let png = authoring::texture_png(&pack_dir, h.state().picked.as_ref().unwrap());
        assert!(png.is_file(), "{}", png.display());
        let list = qwc_assets::remaster::Remastered::read(&authoring::texture_list(&pack_dir)).unwrap();
        assert_eq!(list.png(texture), Some(png.as_path()));
        assert!(h.state().slots.iter().any(|s| s.eq_ignore_ascii_case(&format!("texture/{texture}"))));
        h.run();
        if let Some(out) = shots {
            std::fs::create_dir_all(&out).unwrap();
            h.render().unwrap().save(out.join("packs.png")).unwrap();
        }
        h.get_by_label("Take it out of the pack").click();
        h.run();
        assert!(!png.exists());
        assert!(h.state().slots.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    /// With the game data: a player exported into a pack through the Models tab's button reads on the original's
    /// skeleton with its morphs, and goes again; a Gameplay pack's tuning script made, its misspelled line shown as
    /// an error, mended and saved. With `$QWC_SHOTS`, both tabs are drawn there.
    #[test]
    fn models_and_tuning_tabs() {
        use egui_kittest::kittest::Queryable;
        let dir = std::env::var_os("QWC_DATA").map_or_else(|| PathBuf::from("../../original/cd_files"), PathBuf::from);
        let Ok(vfs) = Vfs::open(&dir) else { return };
        let vfs: &'static Vfs = Box::leak(Box::new(vfs));
        let root = std::env::temp_dir().join(format!("qwc-studio-models-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pack_dir = root.join("me.models");
        let mut m = Manifest::new("me.models", "Models pack");
        m.kind = Kind::Gameplay;
        Pack::create(&pack_dir, m).unwrap();
        let mut pe = PackEditor::new(vfs, Some(&pack_dir));
        pe.tab = Tab::Models;
        pe.model_filter = "gryff/seeker".into();
        pe.picked_model = pe.models.iter().find(|x| x.eq_ignore_ascii_case("teams/Gryff/seeker.fpm")).cloned();
        assert!(pe.picked_model.is_some());
        let shots = std::env::var_os("QWC_SHOTS").map(PathBuf::from);
        let mut builder = egui_kittest::Harness::builder().with_size([1500.0, 860.0]);
        if shots.is_some() {
            builder = builder.wgpu();
        }
        let mut h = builder.build_ui_state(|ui, pe: &mut PackEditor| ui_for_tests(pe, ui), pe);
        h.run();
        h.get_by_label("Export into the pack to edit").click();
        h.run();
        let file = pack_dir.join("players").join("Gryff").join("seeker.glb");
        assert!(file.is_file());
        assert!(h.state().slots.iter().any(|s| s.eq_ignore_ascii_case("player/gryff/seeker")));
        assert!(!h.state().found.iter().any(|(s, _)| *s == Severity::Error), "{:?}", h.state().found);
        let info = h.state().model_info.clone().unwrap().1.unwrap();
        assert!(info.contains("35 morph channels"), "{info}");
        // the engine draws the pack's file
        let drawn = h.state().model_preview.as_ref().map(|p| p.image.as_ref().map(|_| ()).map_err(Clone::clone));
        assert!(matches!(drawn, Some(Ok(()))), "{drawn:?}");
        h.run();
        if let Some(out) = &shots {
            std::fs::create_dir_all(out).unwrap();
            h.render().unwrap().save(out.join("models.png")).unwrap();
        }
        h.get_by_label("Take it out of the pack").click();
        h.run();
        assert!(!file.exists());

        h.state_mut().tab = Tab::Tuning;
        h.state_mut().tuning.new_name = "match.cli".into();
        h.run();
        h.get_by_label("Make it").click();
        h.run();
        let script = pack_dir.join("tuning").join("match.cli");
        assert_eq!(h.state().tuning.file.as_deref(), Some(script.as_path()));
        h.state_mut().tuning.text = "Pitch.GoalHieght 40\n".into();
        h.run();
        assert!(h.query_by_label_contains("unknown console entry").is_some());
        h.state_mut().tuning.text = "Pitch.GoalHeight 40\n".into();
        h.run();
        assert!(h.query_by_label("Every line reads.").is_some());
        if let Some(out) = &shots {
            h.render().unwrap().save(out.join("tuning.png")).unwrap();
        }
        h.get_by_label("Save the script").click();
        h.run();
        assert_eq!(std::fs::read_to_string(&script).unwrap(), "Pitch.GoalHeight 40\n");

        h.state_mut().tab = Tab::Shaders;
        h.state_mut().shaders.new_name = "warm".into();
        h.run();
        h.get_by_label("Post pass").click();
        h.run();
        let pass = pack_dir.join("shaders").join("post").join("warm.wgsl");
        assert_eq!(std::fs::read_to_string(&pass).unwrap(), qwc_render::custom::POST_TEMPLATE);
        assert!(h.query_by_label("It compiles.").is_some());
        h.state_mut().shaders.text =
            "@fragment\nfn fs_main(in: PostIn) -> @location(0) vec4<f32> {\n    return pictur_at(in.uv);\n}\n".into();
        h.run();
        assert!(h.query_by_label_contains("line 3").is_some());
        h.state_mut().shaders.text = "@fragment\nfn fs_main(in: PostIn) -> @location(0) vec4<f32> {\n    return picture_at(in.uv) * vec4<f32>(1.0 + param(0u), 1.0, 1.0, 1.0);\n}\n".into();
        h.run();
        h.get_by_label("Numbers").click();
        h.get_by_label("Numbers").type_text("0.1");
        h.run();
        if let Some(out) = &shots {
            h.render().unwrap().save(out.join("shaders.png")).unwrap();
        }
        h.get_by_label("Save the shader").click();
        h.run();
        let list = qwc_packs::ShaderList::read(&pack_dir.join("shaders")).unwrap();
        assert_eq!((list.post[0].file.as_str(), list.post[0].params.as_slice()), ("post/warm.wgsl", &[0.1][..]));
        assert!(std::fs::read_to_string(&pass).unwrap().contains("param(0u)"));
        assert!(!h.state().found.iter().any(|(s, _)| *s == Severity::Error), "{:?}", h.state().found);
        let _ = std::fs::remove_dir_all(root);
    }

    /// The mode's frame, its new-pack window aside (it needs no window here).
    fn ui_for_tests(pe: &mut PackEditor, ui: &mut egui::Ui) {
        if !crate::theme_once(ui) {
            ui_inner(pe, ui);
        }
    }

    #[test]
    fn versions_and_lists() {
        assert_eq!(bump("1.2.3", 0), "1.2.4");
        assert_eq!(bump("1.2.3", 1), "1.3.0");
        assert_eq!(bump("1.2.3-beta", 2), "2.0.0");
        assert_eq!(bump("odd", 0), "odd");
        assert_eq!(list(" a, b ,, c"), ["a", "b", "c"]);
        assert_eq!(counts(&["texture/a".into(), "texture/b".into(), "movie/c".into()]), "1 movie, 2 texture");
    }
}
