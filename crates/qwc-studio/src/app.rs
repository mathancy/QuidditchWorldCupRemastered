//! The editor: its scene and preview values, its files, its undo, its menus and its panels.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use eframe::egui;
use qwc_assets::Vfs;
use qwc_hud::{Element, Frame, Hint, Kind, Scene, Value, Values};

use crate::canvas;
use crate::panels;
use crate::textures::Textures;

/// The screens the canvas previews.
pub const SCREENS: [(&str, [u32; 2]); 11] = [
    ("640 x 480 (4:3, the original's)", [640, 480]),
    ("1024 x 768 (4:3)", [1024, 768]),
    ("1280 x 720 (16:9)", [1280, 720]),
    ("1600 x 900 (16:9)", [1600, 900]),
    ("1920 x 1080 (16:9)", [1920, 1080]),
    ("2560 x 1440 (16:9)", [2560, 1440]),
    ("3840 x 2160 (16:9)", [3840, 2160]),
    ("1280 x 800 (16:10)", [1280, 800]),
    ("1920 x 1200 (16:10)", [1920, 1200]),
    ("2560 x 1080 (21:9)", [2560, 1080]),
    ("3440 x 1440 (21:9)", [3440, 1440]),
];

/// A picture under the canvas (a screenshot of the original, say) to lay the HUD out against.
pub struct Background {
    pub texture: egui::TextureHandle,
    pub name: String,
    pub opacity: f32,
}

/// What the editor edits: the HUD's scenes, or the front end's screens (`crate::screens`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Scenes,
    Screens,
    /// The stadiums' remastered looks (`crate::looks`).
    Looks,
    /// A resource pack made and changed (`crate::packs`).
    Packs,
}

/// What a texture picked in the browser goes to.
#[derive(Clone, Debug, PartialEq)]
pub enum Pick {
    /// The image at this path.
    Image(Vec<usize>),
    /// A text value.
    Value(String),
    /// A new image.
    Add,
}

pub struct Editor {
    pub mode: Mode,
    /// The front end's screens and their layouts.
    pub screens: crate::screens::Screens,
    /// The stadiums' looks.
    pub looks: crate::looks::Looks,
    /// The resource pack being made.
    pub packs: crate::packs::PackEditor,
    pub textures: Textures,
    /// The scenes' folder Save writes to (the highest pack's `hud/`); none without a pack on.
    pub dir: Option<PathBuf>,
    /// The scene's file in it, without `.ron`.
    pub name: String,
    pub scene: Scene,
    /// The scene as last opened or saved.
    saved: Scene,
    /// The preview's values.
    pub values: Values,
    pub selected: Option<Vec<usize>>,
    undo: Vec<Scene>,
    redo: Vec<Scene>,
    /// The scene as of the last undo step.
    committed: Scene,
    /// The previewed screen, in pixels.
    pub screen: [u32; 2],
    pub view: canvas::View,
    /// The scene as last drawn on the canvas.
    pub frame: Frame,
    pub show_boxes: bool,
    pub show_anchors: bool,
    pub show_safe: bool,
    /// Moves and sizes snap to this many units (none at 0).
    pub snap: f32,
    /// Dragging a driven element sets its drivers' keys at the values' current values instead of moving it.
    pub record: bool,
    /// Values swept to and fro, from when.
    pub sweeping: HashMap<String, f64>,
    pub background: Option<Background>,
    pub background_path: String,
    /// The hierarchy's folded rows.
    pub folded: HashSet<Vec<usize>>,
    /// The texture browser, open for this.
    pub pick: Option<Pick>,
    pub filter: String,
    /// The values panel shows their definitions.
    pub edit_values: bool,
    pub status: String,
    /// The name dialog: its text, and whether it starts a new scene (else saves this one as it).
    new_name: Option<(String, bool)>,
    confirm_close: bool,
    may_close: bool,
    help: bool,
    title: String,
    /// The project's logo for the header, once loaded (none inside: it would not load).
    logo: Option<Option<egui::TextureHandle>>,
}

impl Editor {
    pub fn new(
        vfs: &'static Vfs,
        dir: Option<PathBuf>,
        name: &str,
        screens: crate::screens::Screens,
        looks: crate::looks::Looks,
        mode: Mode,
    ) -> Editor {
        let mut ed = Editor {
            mode,
            screens,
            looks,
            packs: crate::packs::PackEditor::new(vfs, None),
            textures: Textures::new(vfs),
            dir,
            name: name.to_string(),
            scene: Scene::default(),
            saved: Scene::default(),
            values: Values::default(),
            selected: None,
            undo: Vec::new(),
            redo: Vec::new(),
            committed: Scene::default(),
            screen: [1920, 1080],
            view: canvas::View::default(),
            frame: Frame::default(),
            show_boxes: true,
            show_anchors: true,
            show_safe: false,
            snap: 0.0,
            record: false,
            sweeping: HashMap::new(),
            background: None,
            background_path: String::new(),
            folded: HashSet::new(),
            pick: None,
            filter: String::new(),
            edit_values: false,
            status: String::new(),
            new_name: None,
            confirm_close: false,
            may_close: false,
            help: false,
            title: String::new(),
            logo: None,
        };
        ed.open(name);
        ed
    }

    /// The file Save writes scene `name` to: in the scenes' folder, none without one.
    fn file(&self, name: &str) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(format!("{name}.ron")))
    }

    /// Where Save writes, for a person.
    pub fn dir_label(&self) -> String {
        self.dir
            .as_ref()
            .map_or_else(|| "no pack: turn one on, or make one, in the Packs mode".into(), |d| d.display().to_string())
    }

    /// The scenes: the stock ones and those in the folder.
    fn scenes(&self) -> Vec<String> {
        let files = self.dir.iter().filter_map(|d| std::fs::read_dir(d).ok()).flatten().flatten();
        let mut names: Vec<String> = files
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                name.strip_suffix(".ron").map(str::to_string)
            })
            .chain(qwc_hud::BUILT_IN.iter().map(|(n, _)| n.to_string()))
            .collect();
        names.sort();
        names.dedup();
        names
    }

    pub fn dirty(&self) -> bool {
        self.scene != self.saved
    }

    /// Open scene `name` as the game draws it: the folder's file, else the highest pack's (`qwc_packs`, its
    /// `hud/`), else the stock one; a new one when there is none.
    fn open(&mut self, name: &str) {
        let file = format!("{name}.ron");
        let path = self.file(name).filter(|p| p.is_file()).or_else(|| qwc_packs::current().hud(&file));
        let (text, what) = match (&path, qwc_hud::built_in(name)) {
            (Some(p), _) => (std::fs::read_to_string(p).map_err(|e| e.to_string()), p.display().to_string()),
            (None, Some(text)) => (Ok(text.to_string()), format!("the stock {name}")),
            (None, None) => {
                self.load(name, Scene::default());
                self.status = format!("New scene (Save writes it to {})", self.dir_label());
                return;
            }
        };
        match text.and_then(|t| Scene::from_ron(&t)) {
            Ok(s) => {
                self.load(name, s);
                self.status = format!("Opened {what} (Save writes it to {})", self.dir_label());
            }
            Err(e) => self.status = format!("{what}: {e}"),
        }
    }

    fn load(&mut self, name: &str, scene: Scene) {
        self.name = name.to_string();
        self.values = Values::defaults(&scene);
        self.saved = scene.clone();
        self.committed = scene.clone();
        self.scene = scene;
        self.undo.clear();
        self.redo.clear();
        self.selected = None;
        self.sweeping.clear();
        self.folded.clear();
    }

    fn save(&mut self) {
        let (Some(dir), Some(path)) = (self.dir.clone(), self.file(&self.name)) else {
            self.status = format!("Not saved: {}", self.dir_label());
            return;
        };
        let result = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, self.scene.to_ron()));
        match result {
            Ok(()) => {
                self.saved = self.scene.clone();
                self.status = format!("Saved {}", path.display());
            }
            Err(e) => self.status = format!("{}: {e}", path.display()),
        }
    }

    /// An undo step when the scene changed and nothing is being dragged or typed.
    fn commit(&mut self, ctx: &egui::Context) {
        let busy = ctx.input(|i| i.pointer.any_down()) || ctx.memory(|m| m.focused().is_some());
        if !busy && self.scene != self.committed {
            self.undo.push(std::mem::replace(&mut self.committed, self.scene.clone()));
            self.redo.clear();
            if self.undo.len() > 500 {
                self.undo.remove(0);
            }
        }
    }

    fn undo(&mut self) {
        if self.scene != self.committed {
            self.undo.push(self.committed.clone());
        }
        if let Some(s) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.scene, s));
            self.committed = self.scene.clone();
            self.keep_selection();
        }
    }

    fn redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.scene, s));
            self.committed = self.scene.clone();
            self.keep_selection();
        }
    }

    fn keep_selection(&mut self) {
        if self.selected.as_ref().is_some_and(|p| self.scene.get(p).is_none()) {
            self.selected = None;
        }
    }

    /// The selected element's parent's size in units, as last drawn.
    pub fn parent_size(&self, path: &[usize]) -> [f32; 2] {
        self.frame.placed.iter().find(|p| p.path == path).map_or(self.scene.design, |p| p.parent_size)
    }

    /// Add `e` into the selected group, else after the selected element, else at the top; select it.
    pub fn add(&mut self, e: Element) {
        let path = match self.selected.clone() {
            Some(p) if matches!(self.scene.get(&p).map(|e| &e.kind), Some(Kind::Group)) => {
                let group = self.scene.get_mut(&p).unwrap();
                group.children.push(e);
                [p, vec![group.children.len() - 1]].concat()
            }
            Some(mut p) if self.scene.get(&p).is_some() => {
                let list = self.scene.siblings_mut(&p).unwrap();
                let at = p.last().unwrap() + 1;
                list.insert(at, e);
                *p.last_mut().unwrap() = at;
                p
            }
            _ => {
                self.scene.elements.push(e);
                vec![self.scene.elements.len() - 1]
            }
        };
        self.selected = Some(path);
    }

    pub fn duplicate(&mut self, mirrored: bool) {
        let Some(mut p) = self.selected.clone() else { return };
        let Some(mut e) = self.scene.get(&p).cloned() else { return };
        if mirrored {
            mirror(&mut e);
            e.name = format!("{} (mirrored)", e.name);
        } else {
            e.name = format!("{} copy", e.name);
        }
        let list = self.scene.siblings_mut(&p).unwrap();
        let at = p.last().unwrap() + 1;
        list.insert(at, e);
        *p.last_mut().unwrap() = at;
        self.selected = Some(p);
    }

    pub fn delete(&mut self) {
        let Some(p) = self.selected.take() else { return };
        if let Some(list) = self.scene.siblings_mut(&p) {
            list.remove(*p.last().unwrap());
        }
    }

    /// Move the selection `by` places among its siblings.
    pub fn reorder(&mut self, by: isize) {
        let Some(mut p) = self.selected.clone() else { return };
        let i = *p.last().unwrap();
        let Some(list) = self.scene.siblings_mut(&p) else { return };
        let j = i as isize + by;
        if j < 0 || j as usize >= list.len() {
            return;
        }
        list.swap(i, j as usize);
        *p.last_mut().unwrap() = j as usize;
        self.selected = Some(p);
    }

    /// Move the selection into the group before it.
    pub fn indent(&mut self) {
        let Some(p) = self.selected.clone() else { return };
        let i = *p.last().unwrap();
        if i == 0 || !matches!(self.scene.siblings_mut(&p).map(|l| &l[i - 1].kind), Some(Kind::Group)) {
            self.status = "Only a group just above takes an element in".into();
            return;
        }
        let list = self.scene.siblings_mut(&p).unwrap();
        let e = list.remove(i);
        list[i - 1].children.push(e);
        let n = list[i - 1].children.len() - 1;
        self.selected = Some([&p[..p.len() - 1], &[i - 1, n]].concat());
    }

    /// Move the selection out of its parent, to just after it.
    pub fn outdent(&mut self) {
        let Some(p) = self.selected.clone() else { return };
        if p.len() < 2 {
            return;
        }
        let e = self.scene.siblings_mut(&p).unwrap().remove(*p.last().unwrap());
        let parent = &p[..p.len() - 1];
        let list = self.scene.siblings_mut(parent).unwrap();
        let at = parent.last().unwrap() + 1;
        list.insert(at, e);
        self.selected = Some([&parent[..parent.len() - 1], &[at]].concat());
    }

    /// Nudge the selection by `d` units.
    fn nudge(&mut self, d: [f32; 2]) {
        let Some(p) = self.selected.clone() else { return };
        let parent = self.parent_size(&p);
        if let Some(e) = self.scene.get_mut(&p).filter(|e| !e.locked) {
            let b = e.local_box(parent);
            e.set_local_box([b[0] + d[0], b[1] + d[1], b[2] + d[0], b[3] + d[1]], parent);
        }
    }

    /// The values the scene names but the preview hasn't, and the team colours from their teams.
    fn derive_values(&mut self) {
        for p in &self.scene.params {
            if self.values.get(&p.name).is_none() {
                self.values.set(&p.name, p.value.clone());
            }
        }
        let teams: Vec<(String, String)> = self
            .scene
            .params
            .iter()
            .filter_map(|p| match &p.hint {
                Hint::TeamColour(team) => Some((p.name.clone(), self.values.text(team))),
                _ => None,
            })
            .collect();
        for (name, folder) in teams {
            if let Some([r, g, b]) = self.textures.team_colour(&folder) {
                self.values.set(&name, Value::Colour([r, g, b, 1.0]));
            }
        }
    }

    /// The swept values' values now: to and fro across their ranges every 4 seconds.
    fn sweep(&mut self, ctx: &egui::Context) {
        if self.sweeping.is_empty() {
            return;
        }
        let now = ctx.input(|i| i.time);
        for (name, start) in &self.sweeping {
            let Some(p) = self.scene.param(name) else { continue };
            let t = ((now - start) / 2.0) % 2.0;
            let f = 1.0 - (t - 1.0).abs();
            let v = p.range[0] + (p.range[1] - p.range[0]) * f as f32;
            let value = match p.value {
                Value::Flag(_) => Value::Flag(f > 0.5),
                _ => Value::Scalar(v),
            };
            self.values.set(name, value);
        }
        ctx.request_repaint();
    }

    fn load_background(&mut self, ctx: &egui::Context, path: PathBuf) {
        let image = std::fs::read(&path).ok().and_then(|b| qwc_assets::glb::decode_image(&b));
        match image {
            Some((w, h, rgba)) => {
                let image = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
                let texture = ctx.load_texture("background", image, egui::TextureOptions::LINEAR);
                let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                self.status = format!("Background {name} ({w} x {h}); the screen is now {w} x {h}");
                self.screen = [w, h];
                self.background = Some(Background { texture, name, opacity: 0.5 });
                self.background_path = path.display().to_string();
            }
            None => self.status = format!("{}: not a PNG or JPEG", path.display()),
        }
    }

    /// The Screens mode's keys: save, undo and redo, nudge, scale, deselect.
    fn screen_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (cmd, shift, [z, y, s, esc, left, right, up, down, bigger, smaller]) = ctx.input(|i| {
            use egui::Key::*;
            let keys = [Z, Y, S, Escape, ArrowLeft, ArrowRight, ArrowUp, ArrowDown, Plus, Minus];
            (i.modifiers.command, i.modifiers.shift, keys.map(|k| i.key_pressed(k)))
        });
        if bigger || smaller {
            let k = if shift { 1.1 } else { 1.01 };
            self.screens.rescale(if bigger { k } else { 1.0 / k });
        }
        let sc = &mut self.screens;
        if cmd && z && !shift {
            sc.undo();
        }
        if cmd && (y || (z && shift)) {
            sc.redo();
        }
        if cmd && s {
            sc.save();
        }
        if esc {
            sc.deselect();
        }
        let step = if shift { 10.0 } else { 1.0 };
        for (pressed, d) in [(left, [-step, 0.0]), (right, [step, 0.0]), (up, [0.0, -step]), (down, [0.0, step])] {
            if pressed {
                sc.nudge(d);
            }
        }
    }

    /// The Looks mode's keys: Ctrl+S, Ctrl+Z, Ctrl+Y.
    fn look_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (cmd, shift, z, y, s) = ctx.input(|i| {
            let k = |key| i.key_pressed(key);
            (i.modifiers.command, i.modifiers.shift, k(egui::Key::Z), k(egui::Key::Y), k(egui::Key::S))
        });
        if cmd && z && !shift {
            self.looks.undo();
        }
        if cmd && (y || (z && shift)) {
            self.looks.redo();
        }
        if cmd && s {
            self.looks.save();
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.mode == Mode::Screens {
            self.screen_shortcuts(ctx);
            return;
        }
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        for path in dropped {
            let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if ext == "ron" {
                match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| Scene::from_ron(&t)) {
                    Ok(s) => {
                        let name = path.file_stem().map_or("dropped".into(), |n| n.to_string_lossy().into_owned());
                        self.load(&name, s);
                        self.saved = Scene::default();
                        self.status = format!("Opened {} (Save writes it to {})", path.display(), self.dir_label());
                    }
                    Err(e) => self.status = format!("{}: {e}", path.display()),
                }
            } else {
                self.load_background(ctx, path);
            }
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (cmd, shift, pressed) = ctx.input(|i| {
            let keys = [
                egui::Key::Z,
                egui::Key::Y,
                egui::Key::S,
                egui::Key::D,
                egui::Key::M,
                egui::Key::F,
                egui::Key::Delete,
                egui::Key::Escape,
                egui::Key::ArrowLeft,
                egui::Key::ArrowRight,
                egui::Key::ArrowUp,
                egui::Key::ArrowDown,
            ];
            (i.modifiers.command, i.modifiers.shift, keys.map(|k| i.key_pressed(k)))
        });
        let [z, y, s, d, m, f, del, esc, left, right, up, down] = pressed;
        if cmd && z && !shift {
            self.undo();
        }
        if cmd && (y || (z && shift)) {
            self.redo();
        }
        if cmd && s {
            self.save();
        }
        if cmd && d {
            self.duplicate(false);
        }
        if cmd && m {
            self.duplicate(true);
        }
        if f && !cmd {
            self.view.fit();
        }
        if del {
            self.delete();
        }
        if esc {
            self.selected = None;
            self.pick = None;
        }
        let step = if shift { 10.0 } else { 1.0 };
        for (pressed, d) in [(left, [-step, 0.0]), (right, [step, 0.0]), (up, [0.0, -step]), (down, [0.0, step])] {
            if pressed {
                self.nudge(d);
            }
        }
    }

    /// The header across the top of every mode: the logo and the Studio's name, the modes as tabs, and Help.
    fn header(&mut self, ui: &mut egui::Ui) {
        if self.logo.is_none() {
            self.logo = Some(qwc_style::logo(ui.ctx()));
        }
        ui.horizontal(|ui| {
            if let Some(Some(logo)) = &self.logo {
                ui.image((logo.id(), egui::vec2(30.0, 30.0)));
            }
            ui.label(qwc_style::heading("QWC Studio", 24.0));
            ui.add_space(18.0);
            let modes = [
                (Mode::Scenes, "HUD scenes", "The match's and the challenges' HUD (qwc-hud scenes)"),
                (Mode::Screens, "Front-end screens", "The menus, run by their own scripts: move or hide their objects"),
                (
                    Mode::Looks,
                    "Stadium looks",
                    "Each stadium's remastered lighting and colour grade, over a live match",
                ),
                (
                    Mode::Packs,
                    "Resource packs",
                    "Make or change a pack: its contents, models, shaders; build and play it",
                ),
            ];
            for (mode, name, what) in modes {
                if qwc_style::tab(ui, self.mode == mode, name, 17.0).on_hover_text(what).clicked() {
                    self.mode = mode;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Help").clicked() {
                    self.help = true;
                }
            });
        });
    }

    fn menus(&mut self, ui: &mut egui::Ui) {
        if self.mode == Mode::Screens {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("Screens", |ui| {
                    if ui.add_enabled(self.screens.dirty(), egui::Button::new("Save    Ctrl+S")).clicked() {
                        self.screens.save();
                        ui.close();
                    }
                    if ui.button("Undo    Ctrl+Z").clicked() {
                        self.screens.undo();
                        ui.close();
                    }
                    if ui.button("Redo    Ctrl+Y").clicked() {
                        self.screens.redo();
                        ui.close();
                    }
                });
            });
            return;
        }
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("Scene", |ui| {
                if ui.button("New...").clicked() {
                    self.new_name = Some((String::new(), true));
                    ui.close();
                }
                ui.menu_button("Open", |ui| {
                    for name in self.scenes() {
                        if ui.button(&name).clicked() {
                            self.open(&name);
                            ui.close();
                        }
                    }
                });
                if ui.button("Save    Ctrl+S").clicked() {
                    self.save();
                    ui.close();
                }
                if ui.button("Save as...").clicked() {
                    self.new_name = Some((format!("{} copy", self.name), false));
                    ui.close();
                }
                if ui.add_enabled(self.dirty(), egui::Button::new("Revert")).clicked() {
                    let name = self.name.clone();
                    self.open(&name);
                    ui.close();
                }
                ui.separator();
                if ui.button("Copy as RON").on_hover_text("The scene's text, to paste into a message").clicked() {
                    ui.ctx().copy_text(self.scene.to_ron());
                    self.status = "Copied the scene as RON".into();
                    ui.close();
                }
            });
            ui.menu_button("Edit", |ui| {
                if ui.add_enabled(!self.undo.is_empty(), egui::Button::new("Undo    Ctrl+Z")).clicked() {
                    self.undo();
                    ui.close();
                }
                if ui.add_enabled(!self.redo.is_empty(), egui::Button::new("Redo    Ctrl+Y")).clicked() {
                    self.redo();
                    ui.close();
                }
                ui.separator();
                panels::add_menu(self, ui);
                ui.separator();
                let some = self.selected.is_some();
                if ui.add_enabled(some, egui::Button::new("Duplicate    Ctrl+D")).clicked() {
                    self.duplicate(false);
                    ui.close();
                }
                if ui.add_enabled(some, egui::Button::new("Mirrored copy    Ctrl+M")).clicked() {
                    self.duplicate(true);
                    ui.close();
                }
                if ui.add_enabled(some, egui::Button::new("Delete    Del")).clicked() {
                    self.delete();
                    ui.close();
                }
            });
            ui.menu_button("View", |ui| {
                if ui.button("Fit    F").clicked() {
                    self.view.fit();
                    ui.close();
                }
                if ui.button("Actual pixels").clicked() {
                    self.view.actual();
                    ui.close();
                }
                ui.separator();
                ui.checkbox(&mut self.show_boxes, "Every element's box");
                ui.checkbox(&mut self.show_anchors, "The selection's anchors");
                ui.checkbox(&mut self.show_safe, "Safe areas (90% and 80%)");
                ui.separator();
                ui.label("Background (or drop a PNG or JPEG on the window):");
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut self.background_path);
                    if ui.button("Load").clicked() {
                        let path = PathBuf::from(self.background_path.trim().trim_matches('"'));
                        self.load_background(ui.ctx(), path);
                    }
                });
                if let Some(bg) = &mut self.background {
                    ui.weak(&bg.name);
                    ui.add(egui::Slider::new(&mut bg.opacity, 0.0..=1.0).text("opacity"));
                    if ui.button("No background").clicked() {
                        self.background = None;
                    }
                }
            });
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Screen");
            let label = SCREENS
                .iter()
                .find(|(_, s)| *s == self.screen)
                .map_or_else(|| format!("{} x {}", self.screen[0], self.screen[1]), |(l, _)| l.to_string());
            egui::ComboBox::from_id_salt("screen").selected_text(label).width(210.0).show_ui(ui, |ui| {
                for (l, s) in SCREENS {
                    if ui.selectable_label(self.screen == s, l).clicked() {
                        self.screen = s;
                        self.view.fit();
                    }
                }
            });
            ui.add(egui::DragValue::new(&mut self.screen[0]).range(64..=8192).suffix(" w"));
            ui.add(egui::DragValue::new(&mut self.screen[1]).range(64..=8192).suffix(" h"));
            if ui.button("Swap").on_hover_text("Portrait or landscape").clicked() {
                self.screen.swap(0, 1);
                self.view.fit();
            }
            ui.separator();
            ui.label(format!("Zoom {:.0}%", self.view.zoom_shown * 100.0));
            if ui.button("Fit").clicked() {
                self.view.fit();
            }
            if ui.button("1:1").clicked() {
                self.view.actual();
            }
            ui.separator();
            ui.label("Snap");
            ui.add(egui::DragValue::new(&mut self.snap).range(0.0..=64.0).speed(0.25).suffix(" u"))
                .on_hover_text("Moves and sizes snap to this many units (0 for none; hold Alt to skip)");
            ui.separator();
            // the record dot is painted (the fonts have no circle)
            let colour = if self.record { qwc_style::BAD } else { ui.visuals().text_color() };
            let rec = egui::Button::new(egui::RichText::new("     Record keys").color(colour)).selected(self.record);
            let r = ui.add(rec).on_hover_text(RECORD_HELP);
            let dot = egui::pos2(r.rect.left() + ui.spacing().button_padding.x + 5.0, r.rect.center().y);
            ui.painter().circle_filled(dot, 5.0, colour);
            if r.clicked() {
                self.record = !self.record;
            }
        });
    }

    /// The name asked for a new scene, or for saving this one as another.
    fn name_dialog(&mut self, ctx: &egui::Context) {
        let Some((mut name, empty)) = self.new_name.take() else { return };
        let (mut ok, mut cancel) = (false, false);
        let r = egui::Modal::new(egui::Id::unique("scene name")).show(ctx, |ui| {
            ui.label(qwc_style::heading(if empty { "New scene" } else { "Save as" }, 20.0));
            ui.label(format!("<name>.ron in {}", self.dir_label()));
            ui.text_edit_singleline(&mut name).request_focus();
            let valid = !name.trim().is_empty() && !name.contains(['/', '\\', '.']);
            ui.horizontal(|ui| {
                ok = ui.add_enabled(valid, egui::Button::new("OK")).clicked()
                    || (valid && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                cancel = ui.button("Cancel").clicked();
            });
        });
        let name = name.trim().to_string();
        if ok && empty {
            self.load(&name, Scene::default());
            self.status = format!("New scene {name}: Save writes it");
        } else if ok {
            self.name = name;
            self.save();
        } else if !cancel && !r.should_close() {
            self.new_name = Some((name, empty));
        }
    }

    fn close_dialog(&mut self, ctx: &egui::Context) {
        let dirty = self.dirty() || self.screens.dirty() || self.looks.dirty() || self.packs.dirty();
        if ctx.input(|i| i.viewport().close_requested()) && dirty && !self.may_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_close = true;
        }
        if !self.confirm_close {
            return;
        }
        egui::Modal::new(egui::Id::unique("close")).show(ctx, |ui| {
            let mut parts = Vec::new();
            if self.dirty() {
                parts.push(format!("{}.ron", self.name));
            }
            if self.screens.dirty() {
                parts.push("the screens' layouts".to_string());
            }
            if self.looks.dirty() {
                parts.push(format!("{}'s look", self.looks.stadium));
            }
            if self.packs.dirty() {
                parts.push("the pack's manifest".to_string());
            }
            let what = parts.join(" and ");
            ui.label(qwc_style::heading(format!("Save {what} before closing?"), 20.0));
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    if self.dirty() {
                        self.save();
                    }
                    if self.screens.dirty() {
                        self.screens.save();
                    }
                    if self.looks.dirty() {
                        self.looks.save();
                    }
                    if self.packs.dirty() {
                        self.packs.save();
                    }
                    self.close(ctx);
                }
                if ui.button("Don't save").clicked() {
                    self.close(ctx);
                }
                if ui.button("Cancel").clicked() {
                    self.confirm_close = false;
                }
            });
        });
    }

    fn close(&mut self, ctx: &egui::Context) {
        self.may_close = true;
        self.confirm_close = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn help_window(&mut self, ctx: &egui::Context) {
        egui::Window::new("Help").open(&mut self.help).default_width(520.0).show(ctx, |ui| {
            ui.label(HELP);
        });
    }
}

const RECORD_HELP: &str = "While on, dragging an element that a value moves (a driver on its position) sets the \
driver's key at that value's current value, rather than moving the element itself: set the value on its slider, \
drag the element to where it should be then, and the slider shows the motion between the keys.";

const HELP: &str = "\
The canvas: click to select, drag to move, drag the handles to size, drag the anchor flower to move a point \
anchor (the element stays put). Shift keeps a move to one axis; Alt skips the snap. Scroll to zoom, drag with the \
middle button (or Space held) to pan, F fits the screen.

Anchors work as in UMG's canvas panel: where an axis's two anchors meet, the element sits at its position from the \
anchor, by its alignment, at its own size; where they are apart it stretches with its parent and position and size \
are its insets. The presets keep the element where it is; Shift-click a preset to also set the alignment and put \
the element at the anchor.

The values are what the game provides. Slide them, or sweep them with the play buttons, to see what they move. \
A driver adds its keys (value, amount) to a property, linear between the keys. Record keys sets a driver's keys \
by dragging the element at a value. Position moves by units, whatever the anchors; Anchor moves by a share of the \
parent's size, which is what keeps up with a parent that stretches with the screen: to ride a bar's tip, put the \
rider in a group with the bar (filling it) and drive the rider's Anchor X from 0 to 1 (the match's Track).

A scale of -1 across mirrors an element and its children (numbers still read left to right): the match's away \
side is the home side mirrored. Edit > Mirrored copy makes one, swapping left and right in the values it names.

Keys: Ctrl+S save, Ctrl+Z / Ctrl+Y undo and redo, Ctrl+D duplicate, Ctrl+M mirrored copy, Del delete, arrows \
nudge (Shift: 10 units), Esc deselect.

Scenes are RON files in the highest pack's hud/ folder, which the game reads (and reloads when they change) in \
place of the stock ones; without a pack it draws the stock HUD. Scene > Copy as \
RON puts the scene's text on the clipboard. Drop a .ron on the window to open it, a PNG or JPEG to lay it under \
the canvas.

Front-end screens (the switch in the menu bar): the menus run by their own scripts. Pick a screen to start at, \
press the pad's buttons to move it on, pause or step it. Click an object on the canvas or in the tree (Alt-click: \
its parent) and drag it, or nudge it with the arrows; + and - scale it (Shift: faster); tick Hidden to leave it \
out. Its children move with it, and the scripts still animate it as they did, moved. The game draws the screens \
at 4:3 in the middle of a wider window (between the dashed lines): pin an object to an edge to keep it there, \
anchor it to any point of the window (drag the selection's anchor mark, which snaps to the edges and the middle, \
Alt off the snap; or set the fraction in the details), or stretch a full-screen background from edge to edge \
(Stretched shows the original's stretch instead). Exact draws \
the screen with the game's own renderer, the stadium and models behind the menus included; off, a quicker \
approximation without them. Crop... beside a sprite's picture cuts it out of the disc's texture (only the recipe \
is kept, so a pack can share it without the game's art). Ctrl+S writes each changed layer's file into the highest \
pack (its screens/, with the crops and the PNGs in images/), which the running game reads again at once; without \
a pack on, the screens are the original's and nothing is saved.";

/// Mirror `e` across its parent: its anchors, alignment, position and scale across, and left and right swapped in
/// the values it and its children name.
pub fn mirror(e: &mut Element) {
    let a = e.anchors;
    e.anchors.min[0] = 1.0 - a.max[0];
    e.anchors.max[0] = 1.0 - a.min[0];
    if a.stretches(0) {
        (e.position[0], e.size[0]) = (e.size[0], e.position[0]);
    } else {
        e.alignment[0] = 1.0 - e.alignment[0];
        e.position[0] = -e.position[0];
    }
    e.scale[0] = -e.scale[0];
    swap_sides(e);
}

fn swap_sides(e: &mut Element) {
    let swap = |s: &mut String| {
        *s = s.replace("left", "\u{1}").replace("right", "left").replace('\u{1}', "right");
    };
    swap(&mut e.tint);
    swap(&mut e.show_if);
    for d in &mut e.drivers {
        swap(&mut d.value);
    }
    match &mut e.kind {
        Kind::Number(n) => swap(&mut n.value),
        Kind::Image(i) => {
            // only the values named in braces
            let mut out = String::new();
            let mut rest = i.texture.as_str();
            while let (Some(open), Some(close)) = (rest.find('{'), rest.find('}')) {
                if close < open {
                    break;
                }
                let mut name = rest[open..=close].to_string();
                swap(&mut name);
                out.push_str(&rest[..open]);
                out.push_str(&name);
                rest = &rest[close + 1..];
            }
            out.push_str(rest);
            i.texture = out;
        }
        Kind::Group => {}
    }
    for c in &mut e.children {
        swap_sides(c);
    }
}

/// A mode's status line: the last thing done, in the dim text.
fn status(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(qwc_style::DIM));
}

impl eframe::App for Editor {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

impl Editor {
    /// The window's frame: the mode's panels, dialogs and title.
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if self.mode == Mode::Packs {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::S)) && self.packs.dirty() {
                self.packs.save();
            }
            egui::Panel::top("header").frame(qwc_style::bar()).show(ui, |ui| self.header(ui));
            egui::Panel::bottom("packs status").frame(qwc_style::bar()).show(ui, |ui| status(ui, &self.packs.status));
            crate::packs::ui(&mut self.packs, ui);
            self.close_dialog(&ctx);
            self.help_window(&ctx);
            let name = self.packs.pack.as_ref().map_or("no pack".into(), |p| p.manifest.name.clone());
            let title = format!("QWC Studio: {name}{}", if self.packs.dirty() { " *" } else { "" });
            if title != self.title {
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
                self.title = title;
            }
            return;
        }
        if self.mode == Mode::Looks {
            self.look_shortcuts(&ctx);
            egui::Panel::top("header").frame(qwc_style::bar()).show(ui, |ui| self.header(ui));
            egui::Panel::bottom("looks status").frame(qwc_style::bar()).show(ui, |ui| status(ui, &self.looks.status));
            crate::looks::ui(&mut self.looks, ui);
            self.close_dialog(&ctx);
            self.help_window(&ctx);
            let star = if self.looks.dirty() { " *" } else { "" };
            let title = format!("QWC HUD editor: {}'s look{star}", self.looks.stadium);
            if title != self.title {
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
                self.title = title;
            }
            return;
        }
        self.shortcuts(&ctx);
        if self.mode == Mode::Screens {
            egui::Panel::top("header").frame(qwc_style::bar()).show(ui, |ui| self.header(ui));
            egui::Panel::top("menus").show(ui, |ui| self.menus(ui));
            egui::Panel::bottom("screens status")
                .frame(qwc_style::bar())
                .show(ui, |ui| status(ui, &self.screens.status));
            crate::screens::ui(&mut self.screens, ui);
            self.close_dialog(&ctx);
            self.help_window(&ctx);
            let title =
                format!("QWC HUD editor: the front end's screens{}", if self.screens.dirty() { " *" } else { "" });
            if title != self.title {
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
                self.title = title;
            }
            return;
        }
        self.derive_values();
        self.sweep(&ctx);
        egui::Panel::top("header").frame(qwc_style::bar()).show(ui, |ui| self.header(ui));
        egui::Panel::top("menus").show(ui, |ui| {
            self.menus(ui);
            self.toolbar(ui);
        });
        egui::Panel::bottom("status").frame(qwc_style::bar()).show(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some([x, y]) = self.view.pointer {
                    let [kx, ky] = self.frame.scale;
                    ui.monospace(format!("{:7.1} {:7.1} px  {:7.1} {:7.1} units", x, y, x / kx, y / ky));
                    ui.separator();
                }
                ui.monospace(format!(
                    "canvas {:.0} x {:.0} units, {:.3} px a unit",
                    self.frame.canvas[0], self.frame.canvas[1], self.frame.scale[0]
                ));
                ui.separator();
                ui.label(&self.status);
            });
        });
        egui::Panel::left("hierarchy").default_size(280.0).show(ui, |ui| panels::hierarchy(self, ui));
        egui::Panel::right("details").default_size(360.0).show(ui, |ui| panels::details(self, ui));
        egui::Panel::bottom("values").resizable(true).default_size(230.0).show(ui, |ui| panels::values(self, ui));
        egui::CentralPanel::no_frame().show(ui, |ui| canvas::show(self, ui));
        panels::browser(self, &ctx);
        self.name_dialog(&ctx);
        self.close_dialog(&ctx);
        self.help_window(&ctx);
        self.commit(&ctx);
        let title = format!("QWC HUD editor: {}.ron{}", self.name, if self.dirty() { " *" } else { "" });
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With the game data (`$QWC_DATA`, else the repo's `original/cd_files`): every mode draws, headless, with a pack
    /// open in the Packs mode. With `$QWC_SHOTS`, each is drawn there (`mode_<name>.png`).
    #[test]
    fn every_mode_draws() {
        let data = std::env::var_os("QWC_DATA").map_or_else(|| PathBuf::from("../../original/cd_files"), PathBuf::from);
        let Ok(vfs) = Vfs::open(&data) else { return };
        let vfs: &'static Vfs = Box::leak(Box::new(vfs));
        let pack_dir = std::env::temp_dir().join(format!("qwc-studio-modes-{}", std::process::id())).join("me.look");
        let _ = std::fs::remove_dir_all(&pack_dir);
        let mut m = qwc_packs::Manifest::new("me.look", "Look pack");
        m.description = "A pack to look at.".into();
        qwc_packs::Pack::create(&pack_dir, m).unwrap();
        let shots = std::env::var_os("QWC_SHOTS").map(PathBuf::from);
        for (mode, name) in
            [(Mode::Scenes, "scenes"), (Mode::Screens, "screens"), (Mode::Looks, "looks"), (Mode::Packs, "packs")]
        {
            let exes = qwc_frontend::session::exe_candidates(None, &data);
            let mut screens =
                crate::screens::Screens::new(vfs, exes, Some(pack_dir.join("screens")), Some("MainMenuScreen".into()));
            screens.lower.clear();
            let looks = crate::looks::Looks::new(vfs, None, Some("england".into()));
            let mut ed = Editor::new(vfs, Some(pack_dir.join("hud")), "match", screens, looks, mode);
            ed.packs.open(&pack_dir);
            // **[I]** the HUD and screen editors have unlabelled fields (the Packs mode's own tests check its)
            let mut builder =
                egui_kittest::Harness::builder().with_size([1600.0, 960.0]).with_accessibility_check(false);
            if shots.is_some() {
                builder = builder.wgpu();
            }
            // the first frame only puts the theme on, as the window does before its first frame
            let mut h = builder.build_ui_state(
                |ui, ed: &mut Editor| {
                    if !crate::theme_once(ui) {
                        ed.draw(ui)
                    }
                },
                ed,
            );
            for _ in 0..8 {
                h.step();
            }
            if let Some(out) = &shots {
                std::fs::create_dir_all(out).unwrap();
                h.render().unwrap().save(out.join(format!("mode_{name}.png"))).unwrap();
            }
        }
        let _ = std::fs::remove_dir_all(pack_dir.parent().unwrap());
    }

    #[test]
    fn a_mirrored_copy_of_the_home_side_is_the_away_side() {
        let scene = Scene::from_ron(qwc_hud::MATCH).unwrap();
        let bar = &scene.elements[0];
        let (home, away) = (&bar.children[0], &bar.children[1]);
        let mut copy = home.clone();
        mirror(&mut copy);
        assert_eq!(
            (copy.anchors, copy.position, copy.size, copy.scale),
            (away.anchors, away.position, away.size, away.scale)
        );
        let names = |e: &Element| {
            e.children
                .iter()
                .map(|c| (c.tint.clone(), c.drivers.iter().map(|d| d.value.clone()).collect::<Vec<_>>()))
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&copy), names(away));
        let child = |e: &Element, name: &str| e.children.iter().find(|c| c.name == name).cloned().unwrap();
        let track = child(&copy, "Track");
        let Kind::Image(i) = &child(&track, "Bar").kind else { panic!() };
        assert_eq!(i.texture, "teams/{team.right}/QWC_GUI_SnitchBar");
        let snitch = child(&track, "Snitch half");
        assert_eq!(snitch.drivers[0].value, "power.right");
        let Kind::Image(i) = &snitch.kind else { panic!() };
        assert_eq!(i.texture, "QWC_GUI_SnitchLeft");
    }
}
