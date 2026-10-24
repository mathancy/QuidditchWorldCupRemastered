//! The Screens mode: the front end's screens, run by their own scripts (`qwc_frontend`) and drawn on the canvas with
//! the game's textures and fonts, and the remaster's changes to where their objects are drawn
//! (`qwc_frontend::layouts`). Pick an object on the canvas or in the tree, drag it or hide it; Save writes its
//! layer's file in the highest pack (`screens/<layer>.ron`), which a running game reads again; without a pack on,
//! the screens are the original's and nothing is saved. Pictures cut out of the disc's textures are crops
//! (`crate::crop`), kept as recipes in the pack's `screens/crops.ron`.
//!
//! The 512 x 512 front-end screen lies on the previewed screen as the game puts it on its window
//! (`qwc_feck::draw::Fit`: at 4:3 in the middle, the pinned objects at the edges; or stretched over it as the
//! original does), and remastered textures keep their own shape as the game keeps them. With Exact (on wgpu) the
//! game's own painter draws it ([`crate::exact`]): the 3D world behind (the stadium, models, puppets) and every blend
//! type as the engine has it. Without, the canvas's own quads approximate it: no 3D world, the blends alpha
//! blended, the additive ones added, the mask-only ones left out. The picking works on the canvas's quads either way.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use qwc_assets::Vfs;
use qwc_feck::draw::{sprite_quad, text_quads, Adjust, DrawList, Fit, Tex, Vertex, Xf};
use qwc_feck::frontend::EngineEvent;
use qwc_feck::layer::Kind;
use qwc_feck::pad::PadWord;
use qwc_formats::flow::name_hash;
use qwc_frontend::disc::{font_files, keep_shape};
use qwc_frontend::layouts::{layer_stem, Edge, LayerLayout, Layouts};
use qwc_frontend::{Disc, Flow};

/// An object as the layouts know it: (its layer's hash, its id).
pub type Key = (u32, u32);

/// The front end's 24 Hz.
const TICK: f64 = 1.0 / 24.0;

const SELECTED: Color32 = qwc_style::GOLD;
const HOVER: Color32 = qwc_style::ACCENT;
const MOVED: Color32 = qwc_style::GOOD;

/// What saving or writing a picture answers without a pack on.
const NO_PACK: &str = "No pack is on to save into: turn one on, or make one, in the Packs mode";

pub struct Screens {
    vfs: &'static Vfs,
    exes: Vec<PathBuf>,
    /// The highest pack's layouts' folder, which Save writes to (its layouts, its crops, and its PNGs in
    /// `images/`); none without a pack on. The lower packs' are laid under it.
    pub pack: Option<PathBuf>,
    pub lower: Vec<PathBuf>,
    pub session: Option<Session>,
    /// The crop tool.
    pub crop: crate::crop::CropTool,
    /// The textures and fonts, kept from the first session for the next.
    disc: Option<Rc<Disc<'static>>>,
    /// The screen to start at: a flow node's name, or `boot`.
    pub start: String,
    /// The flow's nodes with a screen, in the flow's order.
    pub nodes: Vec<String>,
    /// The previewed screen, in pixels.
    pub screen: [u32; 2],
    /// The front end's screen stretched over the previewed one as the original stretches it, rather than at 4:3
    /// with the pinned objects at the edges as the game draws it.
    pub stretched: bool,
    /// The game's own painter, when the editor runs on wgpu ([`crate::exact`]), and whether the canvas shows it
    /// rather than its own approximation.
    pub exact: Option<crate::exact::Exact>,
    pub show_exact: bool,
    view: View,
    paint: Paint,
    pub status: String,
}

/// How the screen sits in the canvas, and the drag under way.
#[derive(Default)]
struct View {
    /// None fits the screen to the canvas.
    zoom: Option<f32>,
    pan: Vec2,
    drag: Option<Drag>,
    /// The pointer on the front end's 512 x 512 screen.
    pointer: Option<[f32; 2]>,
}

struct Drag {
    /// The object dragged (none: the view is panned).
    key: Option<Key>,
    /// The selection's anchor mark is dragged, not the object: the drag sets its anchor point.
    anchor: bool,
    from: Pos2,
    /// Its offset, or the view's pan, as the drag started.
    offset: [f32; 2],
    pan: Vec2,
}

/// The textures on the GPU (egui's), by file, with the shape a remastered one keeps; the fonts' atlases; the vector
/// fonts' glyphs.
#[derive(Default)]
struct Paint {
    textures: HashMap<String, Option<(egui::TextureHandle, Option<f32>)>>,
    atlases: HashMap<u32, Option<egui::TextureHandle>>,
    text: qwc_text::Renderer,
    glyphs: HashMap<qwc_text::GlyphKey, Option<egui::TextureHandle>>,
    text_screen: (u32, u32),
    /// How many of the disc's dropped textures (`Disc::dropped`) these have dropped too.
    dropped: usize,
}

/// A running screen and the layouts being edited over it.
pub struct Session {
    pub flow: Flow,
    disc: Rc<Disc<'static>>,
    list: DrawList,
    /// The adjustments the list was drawn with: an edit since moves what it drew by the difference.
    drawn: HashMap<Key, Adjust>,
    pub playing: bool,
    pub ticks: u64,
    owed: f64,
    last: Option<Instant>,
    engine_seen: usize,
    /// A pad button pressed for the next tick.
    press: Option<PadWord>,
    layouts: Layouts,
    /// The crops as edited (`crate::crop`); as saved, the layouts have them.
    pub crops: qwc_frontend::crops::Crops,
    /// The layers' layouts being edited, by layer hash, and as last saved (tidied).
    pub edits: HashMap<u32, LayerLayout>,
    saved: HashMap<u32, LayerLayout>,
    undo: Vec<HashMap<u32, LayerLayout>>,
    redo: Vec<HashMap<u32, LayerLayout>>,
    committed: HashMap<u32, LayerLayout>,
    pub selected: Option<Key>,
    /// Names for objects' ids (`qwc_frontend::names::object_names`), and the layers' names by hash.
    names: HashMap<u32, String>,
    layer_names: HashMap<u32, String>,
    /// The texture each sprite object showed in the last frame painted.
    shows: HashMap<usize, String>,
    /// What each object drew in the last frame painted, on the canvas, in drawing order.
    boxes: Vec<(usize, Rect)>,
    /// Where the front end's screen lies on the previewed one ([`Screens::fit`]).
    fit: Fit,
}

/// How a blend type is shown: blended by alpha, added, or opaque. None for those writing only the mask.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Alpha,
    Add,
    Opaque,
}

/// **[I]** a preview's approximation of the engine's blend types (`qwc_render::overlay::BLENDS`).
fn mode(blend: u8) -> Option<Mode> {
    match blend {
        0 | 1 | 11 | 14 => Some(Mode::Opaque),
        4 | 9 | 19 => None,
        7 | 8 | 12 | 13 => Some(Mode::Add),
        _ => Some(Mode::Alpha),
    }
}

/// A vertex colour (ARGB, as the renderer packs it) as egui draws it: the overlay doubles the colour.
fn colour(argb: u32, mode: Mode) -> Color32 {
    let byte = |shift: u32| (argb >> shift) & 0xff;
    let [r, g, b] = [byte(16), byte(8), byte(0)].map(|c| (c * 2).min(255) as u8);
    let a = byte(24) as u8;
    match mode {
        Mode::Alpha => Color32::from_rgba_unmultiplied(r, g, b, a),
        Mode::Opaque => Color32::from_rgb(r, g, b),
        // premultiplied with no alpha: added to what is under it
        Mode::Add => {
            let k = |c: u8| (c as u32 * a as u32 / 255) as u8;
            Color32::from_rgba_premultiplied(k(r), k(g), k(b), 0)
        }
    }
}

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Null => "Null",
        Kind::Menu => "Menu",
        Kind::Sprite => "Sprite",
        Kind::Text => "Text",
        Kind::Camera => "Camera",
        Kind::Model => "Model",
        Kind::Puppet => "Puppet",
        Kind::AnimatedSprite => "Animated",
        Kind::Particles => "Particles",
        Kind::Card => "Card",
        Kind::Slider => "Slider",
        Kind::PageMenu => "Page menu",
    }
}

/// A texture's file name without its folder or extension.
fn file_stem(path: &str) -> &str {
    layer_stem(path)
}

impl Screens {
    /// The Screens mode over the layouts in the highest pack's folder `pack` and the lower packs' under it
    /// (`qwc_frontend::layouts::lower_packs`), starting at node `start`.
    pub fn new(vfs: &'static Vfs, exes: Vec<PathBuf>, pack: Option<PathBuf>, start: Option<String>) -> Screens {
        let lower = qwc_frontend::layouts::lower_packs(pack.as_deref());
        Screens {
            vfs,
            exes,
            pack,
            lower,
            session: None,
            crop: Default::default(),
            disc: None,
            start: start.unwrap_or_else(|| "StartScreen".into()),
            nodes: Vec::new(),
            screen: [1280, 720],
            stretched: false,
            exact: None,
            show_exact: true,
            view: View::default(),
            paint: Paint::default(),
            status: String::new(),
        }
    }

    /// Where the front end's screen lies on the previewed one: as the game puts it on its window (at 4:3 in the
    /// middle), or stretched over it.
    pub fn fit(&self) -> Fit {
        if self.stretched {
            Fit::STRETCHED
        } else {
            Fit::four_three((self.screen[0], self.screen[1]))
        }
    }

    /// The preview's screen or its fit changed: the session lays out for it from the next tick.
    fn refit(&mut self) {
        let fit = self.fit();
        if let Some(s) = &mut self.session {
            s.fit = fit;
            s.flow.fe.margins = fit.margins();
        }
    }

    /// Whether a layout has changes not saved.
    pub fn dirty(&self) -> bool {
        self.session.as_ref().is_some_and(Session::dirty)
    }

    /// The game data.
    pub fn vfs(&self) -> &'static Vfs {
        self.vfs
    }

    /// The pack's PNGs' folder, when a pack is on.
    fn images(&self) -> Option<PathBuf> {
        self.pack.as_ref().map(|p| p.join("images"))
    }

    /// Start the flow at [`Screens::start`] (its first tick run, so the screen has drawn), keeping what was edited.
    pub fn restart(&mut self) {
        let keep = self.session.take().map(|s| (s.edits, s.saved, s.undo, s.redo, s.committed, s.crops));
        let mut flow = match qwc_frontend::start_flow(self.vfs, &self.exes, Some(&self.start)) {
            Ok(f) => f,
            Err(e) => {
                self.status = format!("Could not start {}: {e}", self.start);
                return;
            }
        };
        let vfs = self.vfs;
        let disc = self
            .disc
            .get_or_insert_with(|| {
                let fonts = font_files(vfs, &flow.resources);
                let textures = qwc_game::fe::pack_textures();
                let font_list = qwc_game::fe::pack_fonts();
                Disc::shared(vfs, std::mem::take(&mut flow.resources), fonts, textures, font_list)
            })
            .clone();
        flow.fe.assets = Some(disc.clone());
        let m = &flow.m;
        self.nodes = std::iter::once("boot".to_string())
            .chain(
                m.flow.nodes.iter().filter(|n| m.defs.iter().any(|d| d.hash == n.hash)).filter_map(|n| n.name.clone()),
            )
            .collect();
        let layer_names = m
            .defs
            .iter()
            .flat_map(|d| &d.layers)
            .map(|l| layer_stem(&l.file).to_string())
            .map(|stem| (name_hash(&stem), stem))
            .collect();
        let layouts = Layouts::new(self.pack.clone()).with_lower(self.lower.clone());
        let crops = layouts.crops().clone();
        let exe = self.exes.iter().find_map(|p| std::fs::read(p).ok());
        let names = qwc_frontend::names::object_names(&flow.bytes, exe.as_deref());
        let mut s = Session {
            flow,
            disc,
            list: DrawList::default(),
            drawn: HashMap::new(),
            playing: true,
            ticks: 0,
            owed: 0.0,
            last: None,
            engine_seen: 0,
            press: None,
            edits: HashMap::new(),
            saved: HashMap::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            committed: HashMap::new(),
            layouts,
            crops,
            selected: None,
            names,
            layer_names,
            shows: HashMap::new(),
            boxes: Vec::new(),
            fit: self.fit(),
        };
        match keep {
            Some((edits, saved, undo, redo, committed, crops)) => {
                (s.edits, s.saved, s.undo, s.redo, s.committed, s.crops) = (edits, saved, undo, redo, committed, crops);
            }
            None => {
                let files: HashMap<u32, LayerLayout> = s.layouts.layers().map(|l| (l.hash, l.clone())).collect();
                (s.edits, s.saved, s.committed) = (files.clone(), files.clone(), files);
            }
        }
        for a in s.edits.values().flat_map(|l| &l.adds) {
            s.names.insert(a.id(), a.name.clone());
        }
        s.apply();
        let screen = (self.screen[0], self.screen[1]);
        s.tick(screen);
        self.status = format!("Started at {}", self.start);
        self.session = Some(s);
    }

    /// The front end's ticks owed since the last frame, while it plays.
    fn advance(&mut self, ctx: &egui::Context) {
        let screen = (self.screen[0], self.screen[1]);
        let Some(s) = &mut self.session else { return };
        let now = Instant::now();
        let dt = s.last.map_or(0.0, |t| now.duration_since(t).as_secs_f64());
        s.last = Some(now);
        if !s.playing {
            s.owed = 0.0;
            return;
        }
        s.owed = (s.owed + dt).min(4.0 * TICK);
        while s.owed >= TICK {
            s.owed -= TICK;
            s.tick(screen);
        }
        ctx.request_repaint();
    }

    pub fn save(&mut self) {
        if let Some(s) = &mut self.session {
            self.status = s.save();
        }
    }

    pub fn undo(&mut self) {
        if let Some(s) = &mut self.session {
            s.undo_step();
        }
    }

    pub fn redo(&mut self) {
        if let Some(s) = &mut self.session {
            s.redo_step();
        }
    }

    /// The selection moved by `d` units of the front end's screen.
    pub fn nudge(&mut self, d: [f32; 2]) {
        if let Some(s) = &mut self.session {
            if let Some(key) = s.selected {
                let o = s.offset(key);
                s.set_offset(key, [o[0] + d[0], o[1] + d[1]]);
            }
        }
    }

    /// The selection's scale times `k`.
    pub fn rescale(&mut self, k: f32) {
        if let Some(s) = &mut self.session {
            if let Some(key) = s.selected {
                let scale = s.change(key).scale;
                s.set_scale(key, scale * k);
            }
        }
    }

    pub fn deselect(&mut self) {
        if let Some(s) = &mut self.session {
            s.selected = None;
        }
    }
}

impl Session {
    /// One tick of the flow: the pad (a button pressed for it), the screens' frame, the cutscenes asked for over at
    /// once (as the game's without a window), and the draw.
    fn tick(&mut self, screen: (u32, u32)) {
        let f = &mut self.flow;
        for w in f.fe.pads.words[0].iter_mut() {
            *w = 0;
        }
        if let Some(p) = self.press.take() {
            f.fe.pads.words[0][p as usize] = 1;
        }
        f.m.frame(&mut f.fe, &mut f.vm);
        self.disc.set_loaded(&f.m, &f.sets);
        // laid out for the screen's place in the preview, as the game lays it out for its window
        self.disc.set_screen(self.fit.area(screen));
        f.fe.margins = self.fit.margins();
        let events = &f.fe.engine[self.engine_seen.min(f.fe.engine.len())..];
        let cutscene = events.iter().any(|e| matches!(e, EngineEvent::PlayCutScene(_)));
        self.engine_seen = f.fe.engine.len();
        if cutscene {
            for l in f.m.current_screen().map(|s| s.layers.clone()).unwrap_or_default() {
                let _ = f.fe.general(&mut f.vm, l, 2);
            }
        }
        f.fe.audio_events.clear();
        self.drawn = f.fe.adjust.clone();
        self.list = f.draw(&*self.disc);
        self.ticks += 1;
    }

    /// The edits as the front end's adjustments.
    fn current(&self) -> HashMap<Key, Adjust> {
        let mut out = HashMap::new();
        for l in self.edits.values() {
            for o in l.objects.iter().filter(|o| !o.is_empty()) {
                out.insert((l.hash, o.id), o.adjust());
            }
        }
        out
    }

    pub fn apply(&mut self) {
        let images = self.layouts.images_dir();
        qwc_frontend::layouts::apply_layers(self.edits.values(), &images, &self.crops, &mut self.flow.fe, &self.disc);
    }

    /// The textures and fonts.
    pub fn disc(&self) -> &Disc<'static> {
        &self.disc
    }

    /// What sprite `i` showed in the last frame painted: a texture path of the data, a PNG's path or a crop.
    pub fn shown(&self, i: usize) -> Option<String> {
        self.shows.get(&i).cloned()
    }

    /// The size of the texture of the data at `path`, decoded on first use.
    pub fn texture_size(&self, path: &str) -> Option<[u16; 2]> {
        self.disc.texture_at(path)
    }

    /// How many objects and added sprites show crop `name`.
    pub fn crop_users(&self, name: &str) -> usize {
        let spec = qwc_frontend::crops::spec(name);
        let layers = self.edits.values();
        layers
            .map(|l| {
                l.objects.iter().filter(|o| o.image.as_deref() == Some(&spec)).count()
                    + l.adds.iter().filter(|a| a.image == spec).count()
            })
            .sum()
    }

    /// Crop `from` named `to`, the layouts showing it with it.
    pub fn rename_crop(&mut self, from: &str, to: &str) {
        let Some(c) = self.crops.remove(from) else { return };
        self.crops.insert(to.to_string(), c);
        let (old, new) = (qwc_frontend::crops::spec(from), qwc_frontend::crops::spec(to));
        for l in self.edits.values_mut() {
            for o in &mut l.objects {
                if o.image.as_deref() == Some(&old) {
                    o.image = Some(new.clone());
                }
            }
            for a in &mut l.adds {
                if a.image == old {
                    a.image = new.clone();
                }
            }
        }
        self.apply();
    }

    pub fn key(&self, i: usize) -> Key {
        let o = &self.flow.fe.objects[i];
        (self.flow.fe.layers[o.layer].hash, o.id)
    }

    /// The live object of `key`.
    pub fn object(&self, key: Key) -> Option<usize> {
        let fe = &self.flow.fe;
        fe.layers
            .iter()
            .filter(|l| l.alive && l.hash == key.0)
            .flat_map(|l| l.objects.iter().copied())
            .find(|&i| fe.objects[i].id == key.1)
    }

    fn layer_name(&self, hash: u32) -> String {
        self.layer_names.get(&hash).cloned().unwrap_or_else(|| format!("{hash:08x}"))
    }

    pub fn offset(&self, key: Key) -> [f32; 2] {
        self.edits.get(&key.0).and_then(|l| l.object(key.1)).map_or([0.0; 2], |o| o.offset)
    }

    /// `key`'s change in its layer's layout, made (noted with what the object is) when there is none.
    fn object_mut(&mut self, key: Key) -> &mut qwc_frontend::ObjectLayout {
        let note = self.object(key).map(|i| self.label(i)).unwrap_or_default();
        let name = self.layer_name(key.0);
        let l =
            self.edits.entry(key.0).or_insert_with(|| LayerLayout { layer: name, hash: key.0, ..Default::default() });
        let o = l.object_mut(key.1);
        if o.note.is_empty() {
            o.note = note;
        }
        o
    }

    pub fn set_offset(&mut self, key: Key, offset: [f32; 2]) {
        self.object_mut(key).offset = offset.map(|v| (v * 100.0).round() / 100.0);
        self.apply();
    }

    pub fn set_hidden(&mut self, key: Key, hidden: bool) {
        self.object_mut(key).hidden = hidden;
        self.apply();
    }

    /// `key`'s change as it stands (none: the scripts' own).
    pub fn change(&self, key: Key) -> qwc_frontend::ObjectLayout {
        self.edits.get(&key.0).and_then(|l| l.object(key.1)).cloned().unwrap_or_default()
    }

    pub fn set_scale(&mut self, key: Key, scale: f32) {
        self.object_mut(key).scale = (scale.clamp(0.05, 20.0) * 1000.0).round() / 1000.0;
        self.apply();
    }

    pub fn set_pins(&mut self, key: Key, pins: [Edge; 2]) {
        let o = self.object_mut(key);
        (o.pin_x, o.pin_y) = (pins[0], pins[1]);
        self.apply();
    }

    /// Show `text` in place of the scripts' (`None`: theirs again).
    pub fn set_text(&mut self, key: Key, text: Option<String>) {
        self.object_mut(key).text = text;
        self.apply();
    }

    /// Show this picture in place of the sprite's own (`None`: its own again).
    pub fn set_image(&mut self, key: Key, image: Option<String>) {
        match self.addition_mut(key) {
            // an added sprite's picture is its own field; "its own" means nothing for it
            Some(a) => {
                if let Some(image) = image {
                    a.image = image;
                }
            }
            None => self.object_mut(key).image = image,
        }
        self.apply();
    }

    /// Add `a` to layer `hash`'s layout (or replace the one of its name); answers its object id.
    pub fn add_sprite(&mut self, hash: u32, a: qwc_frontend::layouts::AddLayout) -> u32 {
        let id = a.id();
        self.names.insert(id, a.name.clone());
        let name = self.layer_name(hash);
        let l = self.edits.entry(hash).or_insert_with(|| LayerLayout { layer: name, hash, ..Default::default() });
        match l.adds.iter_mut().find(|x| x.name == a.name) {
            Some(x) => *x = a,
            None => l.adds.push(a),
        }
        self.apply();
        id
    }

    /// The added sprite of `key`, when the object is one.
    pub fn addition(&self, key: Key) -> Option<&qwc_frontend::layouts::AddLayout> {
        self.edits.get(&key.0)?.adds.iter().find(|a| a.id() == key.1)
    }

    fn addition_mut(&mut self, key: Key) -> Option<&mut qwc_frontend::layouts::AddLayout> {
        self.edits.get_mut(&key.0)?.adds.iter_mut().find(|a| a.id() == key.1)
    }

    /// Take an added sprite out again, its changes with it.
    pub fn remove_add(&mut self, key: Key) {
        if let Some(l) = self.edits.get_mut(&key.0) {
            l.adds.retain(|a| a.id() != key.1);
            l.objects.retain(|o| o.id != key.1);
        }
        if self.selected == Some(key) {
            self.selected = None;
        }
        self.apply();
    }

    /// What object `i` is, for a person: its name when the scripts give one, else its text or texture.
    pub fn label(&self, i: usize) -> String {
        let fe = &self.flow.fe;
        let o = &fe.objects[i];
        let kind = kind_name(o.kind);
        if let Some(n) = self.names.get(&o.id) {
            return format!("{kind} {n}");
        }
        let what = match o.kind {
            Kind::Text => fe.text(i).map(|t| {
                let s: String = String::from_utf16_lossy(t).chars().take_while(|&c| c != '\0').collect();
                let s = s.replace('\n', " ");
                let short: String = s.chars().take(28).collect();
                format!("\"{short}{}\"", if s.chars().count() > 28 { "..." } else { "" })
            }),
            Kind::Sprite | Kind::AnimatedSprite => self.shows.get(&i).map(|p| file_stem(p).to_string()),
            _ => None,
        };
        match what {
            Some(w) => format!("{kind} {w}"),
            None => format!("{kind} {:08x}", o.id),
        }
    }

    /// How much an edit since the list was drawn moves object `i` (with its parents' edits), on the front end's
    /// screen; none when one hides it now.
    fn shift(&self, now: &HashMap<Key, Adjust>, mut i: usize) -> Option<[f32; 2]> {
        let fe = &self.flow.fe;
        let mut s = [0.0f32; 2];
        loop {
            let o = &fe.objects[i];
            let key = (fe.layers[o.layer].hash, o.id);
            let (a, b) =
                (now.get(&key).copied().unwrap_or_default(), self.drawn.get(&key).copied().unwrap_or_default());
            if a.hidden && !b.hidden {
                return None;
            }
            s = [s[0] + a.offset[0] - b.offset[0], s[1] + a.offset[1] - b.offset[1]];
            match o.parent {
                Some(p) => i = p,
                None => return Some(s),
            }
        }
    }

    /// The list as the edits are now: each record moved by the edits since it was drawn (left out when one hides
    /// it now), and every object scaled and pinned as they say, on a window `margins` past the screen.
    fn adjusted(&self, now: &HashMap<Key, Adjust>, margins: [f32; 2]) -> DrawList {
        let mut list = self.list.clone();
        list.sprites.retain_mut(|sp| match self.shift(now, sp.object) {
            Some(d) => {
                sp.pos[0] += d[0];
                sp.pos[1] += d[1];
                true
            }
            None => false,
        });
        list.texts.retain_mut(|r| match self.shift(now, r.object) {
            Some(d) => {
                r.pos[0] += d[0];
                r.pos[1] += d[1];
                true
            }
            None => false,
        });
        list.sparks.retain_mut(|sp| match self.shift(now, sp.object) {
            Some(d) => {
                for v in &mut sp.corners {
                    v.pos[0] += d[0];
                    v.pos[1] += d[1];
                }
                true
            }
            None => false,
        });
        list.xf = (self.list.places.iter())
            .filter_map(|&(i, _)| Some((i, self.list.xf_under(&self.flow.fe, now, margins, i)?)))
            .filter(|(_, xf)| !xf.is_identity())
            .collect();
        list
    }

    /// Whether object `i` is `top` or below it.
    fn under(&self, mut i: usize, top: usize) -> bool {
        loop {
            if i == top {
                return true;
            }
            match self.flow.fe.objects[i].parent {
                Some(p) => i = p,
                None => return false,
            }
        }
    }

    fn tidied(&self) -> HashMap<u32, LayerLayout> {
        let mut out = HashMap::new();
        for (h, l) in &self.edits {
            let mut l = l.clone();
            l.tidy();
            if !l.objects.is_empty() || !l.adds.is_empty() {
                out.insert(*h, l);
            }
        }
        out
    }

    pub fn dirty(&self) -> bool {
        self.tidied() != self.saved || self.crops != *self.layouts.crops()
    }

    /// Where Save writes, for a person.
    fn place(&self) -> String {
        self.layouts.pack.as_ref().map_or_else(|| "Not saved".into(), |p| p.display().to_string())
    }

    /// Write each changed layer's files, and the crops' when they changed; answers what was done.
    fn save(&mut self) -> String {
        let now = self.tidied();
        let mut wrote = Vec::new();
        // a crop opened and left (no corners placed, no sprite showing it) is not kept
        let idle: Vec<String> =
            (self.crops.iter()).filter(|(n, c)| !c.cuts() && self.crop_users(n) == 0).map(|(n, _)| n.clone()).collect();
        for n in idle {
            self.crops.remove(&n);
        }
        if self.crops != *self.layouts.crops() {
            match self.layouts.save_crops(&self.crops) {
                Ok(p) => wrote.push(p.display().to_string()),
                Err(e) => return format!("{}: {e}", self.place()),
            }
        }
        let hashes: std::collections::BTreeSet<u32> = now.keys().chain(self.saved.keys()).copied().collect();
        for h in hashes {
            let (a, b) = (now.get(&h), self.saved.get(&h));
            if a == b {
                continue;
            }
            let layout = a.cloned().unwrap_or_else(|| LayerLayout {
                layer: b.map_or_else(|| self.layer_name(h), |l| l.layer.clone()),
                hash: h,
                ..Default::default()
            });
            match self.layouts.save(&layout) {
                Ok(p) => wrote.extend(p.iter().map(|p| p.display().to_string())),
                Err(e) => return format!("{}: {e}", self.place()),
            }
        }
        self.saved = now;
        if wrote.is_empty() {
            "Nothing to save".into()
        } else {
            format!("Saved {}", wrote.join(", "))
        }
    }

    /// An undo step when the edits changed and nothing is being dragged or typed.
    fn commit(&mut self, ctx: &egui::Context) {
        let busy = ctx.input(|i| i.pointer.any_down()) || ctx.memory(|m| m.focused().is_some());
        if !busy && self.edits != self.committed {
            self.undo.push(std::mem::replace(&mut self.committed, self.edits.clone()));
            self.redo.clear();
            if self.undo.len() > 500 {
                self.undo.remove(0);
            }
        }
    }

    fn undo_step(&mut self) {
        if self.edits != self.committed {
            self.undo.push(self.committed.clone());
        }
        if let Some(e) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.edits, e));
            self.committed = self.edits.clone();
            self.apply();
        }
    }

    fn redo_step(&mut self) {
        if let Some(e) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.edits, e));
            self.committed = self.edits.clone();
            self.apply();
        }
    }
}

impl Paint {
    /// The texture at `path` on the GPU (as the disc decoded it), and the shape a remastered one keeps.
    fn texture(&mut self, ctx: &egui::Context, disc: &Disc, path: &str) -> Option<(egui::TextureId, Option<f32>)> {
        let max = ctx.input(|i| i.max_texture_side).max(1) as u32;
        let t = self.textures.entry(path.to_string()).or_insert_with(|| {
            let shown = disc.shown(path)?;
            let p = &shown.pixels;
            // the largest level egui takes
            let level = (0..p.levels.len()).find(|&l| (p.width >> l).max(1) <= max && (p.height >> l).max(1) <= max)?;
            let (w, h) = ((p.width >> level).max(1) as usize, (p.height >> level).max(1) as usize);
            let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &p.levels[level]);
            Some((ctx.load_texture(path, image, egui::TextureOptions::LINEAR), shown.shape))
        });
        t.as_ref().map(|(h, shape)| (h.id(), *shape))
    }

    fn atlas(&mut self, ctx: &egui::Context, id: u32, font: &qwc_formats::ffn::Ffn) -> Option<egui::TextureId> {
        let t = self.atlases.entry(id).or_insert_with(|| {
            let rgba = font.atlas().ok()?;
            let image = egui::ColorImage::from_rgba_unmultiplied([font.tex_w as usize, font.tex_h as usize], &rgba);
            Some(ctx.load_texture(format!("font {id:x}"), image, egui::TextureOptions::LINEAR))
        });
        t.as_ref().map(|h| h.id())
    }

    fn glyph(&mut self, ctx: &egui::Context, key: &qwc_text::GlyphKey) -> Option<egui::TextureId> {
        if !self.glyphs.contains_key(key) {
            let t = self.text.image(key).filter(|im| im.width > 0 && im.height > 0).map(|im| {
                let image = egui::ColorImage::from_rgba_unmultiplied([im.width as usize, im.height as usize], &im.rgba);
                ctx.load_texture("glyph", image, egui::TextureOptions::LINEAR)
            });
            self.glyphs.insert(*key, t);
        }
        self.glyphs[key].as_ref().map(|h| h.id())
    }
}

/// A quad to draw: its texture, corners (bottom left, bottom right, top left, top right) on the canvas, texture
/// coordinates and colours, its material layer, and its object.
struct Quad {
    order: u32,
    texture: egui::TextureId,
    pos: [Pos2; 4],
    uv: [Pos2; 4],
    colour: [Color32; 4],
    object: Option<usize>,
}

fn uv(v: &Vertex) -> Pos2 {
    Pos2::new(v.uv[0] as f32 / 4096.0, v.uv[1] as f32 / 4096.0)
}

/// The box of what object `top` and everything below it drew, on the canvas.
fn bounds(s: &Session, boxes: &[(usize, Rect)], top: usize) -> Option<Rect> {
    boxes.iter().filter(|(o, _)| s.under(*o, top)).map(|(_, r)| *r).reduce(|a, b| a.union(b))
}

// ---------------------------------------------------------------------------------------------------------------
// the panels

/// The mode's panels: the screens and the tree (left), the selection (right), the transport (top), the canvas.
pub fn ui(sc: &mut Screens, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    if sc.session.is_none() && sc.status.is_empty() {
        sc.status = "Loading the front end's scripts...".into();
        ctx.request_repaint();
    } else if sc.session.is_none() && sc.status.starts_with("Loading") {
        sc.restart();
    }
    sc.advance(&ctx);
    // a PNG dropped on the window becomes the selected sprite's picture, copied into the layouts' images folder
    let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
    for path in dropped {
        sc.status = drop_png(sc, &path);
    }
    egui::Panel::top("screens transport").show(ui, |ui| transport(sc, ui));
    egui::Panel::left("screens tree").default_size(320.0).show(ui, |ui| tree(sc, ui));
    egui::Panel::right("screens details").default_size(340.0).show(ui, |ui| details(sc, ui));
    egui::CentralPanel::no_frame().show(ui, |ui| canvas(sc, ui));
    crate::crop::window(sc, &ctx);
    if let Some(s) = &mut sc.session {
        s.commit(&ctx);
    }
}

/// A PNG dropped on the window: copied into the texture pack's `images/` and shown by the selected sprite (its
/// changes then saved to the pack's layouts).
fn drop_png(sc: &mut Screens, path: &std::path::Path) -> String {
    if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")) {
        return format!("{}: only a PNG goes on a sprite", path.display());
    }
    let Some(images) = sc.images() else { return NO_PACK.into() };
    let Some(s) = &mut sc.session else { return "No screen is running".into() };
    let Some(key) = s.selected else { return "Select a sprite first, then drop its PNG".into() };
    if !s.object(key).is_some_and(|i| matches!(s.flow.fe.objects[i].kind, Kind::Sprite | Kind::AnimatedSprite)) {
        return "The selection is not a sprite; select one, then drop its PNG".into();
    }
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return format!("{}: no file name", path.display());
    };
    let dest = images.join(&name);
    if let Err(e) = std::fs::create_dir_all(&images) {
        return format!("{}: {e}", images.display());
    }
    // over itself (dropped from the images folder): nothing to copy
    let same = dest.exists() && path.canonicalize().ok() == dest.canonicalize().ok();
    if !same {
        if let Err(e) = std::fs::copy(path, &dest) {
            return format!("{} -> {}: {e}", path.display(), dest.display());
        }
    }
    // the file may stand in for one shown before: it is read again
    let spec = qwc_frontend::layouts::image_spec(&images, &name);
    s.disc.drop_texture(&spec);
    // a frame file (`base.k.png`) lands beside its base and is not made the picture itself
    let frame = name.rsplit('.').nth(1).is_some_and(|k| !k.is_empty() && k.bytes().all(|b| b.is_ascii_digit()));
    if !frame {
        s.set_image(key, Some(name.clone()));
    }
    sc.paint.textures.remove(&spec);
    if frame {
        format!("{name}: a frame beside its base picture")
    } else {
        format!("{name}: the sprite's picture")
    }
}

fn transport(sc: &mut Screens, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Screen");
        let mut start = None;
        egui::ComboBox::from_id_salt("start at").selected_text(&sc.start).width(220.0).height(600.0).show_ui(
            ui,
            |ui| {
                for n in &sc.nodes {
                    if ui.selectable_label(*n == sc.start, n).clicked() {
                        start = Some(n.clone());
                    }
                }
            },
        );
        if let Some(n) = start {
            sc.start = n;
            sc.restart();
        }
        if ui.button("Restart").on_hover_text("Start the flow at this screen again").clicked() {
            sc.restart();
        }
        ui.separator();
        ui.label("Preview");
        let label = crate::app::SCREENS
            .iter()
            .find(|(_, s)| *s == sc.screen)
            .map_or_else(|| format!("{} x {}", sc.screen[0], sc.screen[1]), |(l, _)| l.to_string());
        egui::ComboBox::from_id_salt("screens preview").selected_text(label).width(200.0).show_ui(ui, |ui| {
            for (l, s) in crate::app::SCREENS {
                if ui.selectable_label(sc.screen == s, l).clicked() {
                    sc.screen = s;
                    sc.view.zoom = None;
                    sc.view.pan = Vec2::ZERO;
                    sc.refit();
                }
            }
        });
        if ui
            .checkbox(&mut sc.stretched, "Stretched")
            .on_hover_text(
                "The front end's screen stretched over the preview as the original stretches it over its window. \
                 Off, it is drawn as the game draws it: at 4:3 in the middle, the pinned objects at the edges.",
            )
            .changed()
        {
            sc.refit();
        }
        if sc.exact.is_some() {
            ui.checkbox(&mut sc.show_exact, "Exact").on_hover_text(
                "Drawn by the game's own renderer: the stadium and the screen's models behind the menus, and every \
                 blend as the engine has it. Off, the editor's quicker approximation.",
            );
        }
        let Some(s) = &mut sc.session else { return };
        ui.separator();
        let play = if s.playing { "Pause" } else { "Play" };
        if ui.button(play).on_hover_text("The screen's scripts at 24 ticks a second").clicked() {
            s.playing = !s.playing;
        }
        if ui.add_enabled(!s.playing, egui::Button::new("Step")).on_hover_text("One tick").clicked() {
            let screen = (sc.screen[0], sc.screen[1]);
            s.tick(screen);
        }
        ui.monospace(format!("tick {}", s.ticks));
        ui.separator();
        ui.label("Press");
        for (label, word) in [
            ("\u{2190}", PadWord::Left),
            ("\u{2192}", PadWord::Right),
            ("\u{2191}", PadWord::Up),
            ("\u{2193}", PadWord::Down),
            ("Choose", PadWord::Choose),
            ("Back", PadWord::Cancel),
        ] {
            if ui.button(label).on_hover_text("A pad button pressed for a tick, to move the screen on").clicked() {
                s.press = Some(word);
                if !s.playing {
                    let screen = (sc.screen[0], sc.screen[1]);
                    s.tick(screen);
                }
            }
        }
        let at = s.flow.node_name(s.flow.m.node);
        ui.separator();
        ui.weak(format!("at {at}"));
    });
}

/// The current screen's layers and their objects.
fn tree(sc: &mut Screens, ui: &mut egui::Ui) {
    ui.label(qwc_style::heading("Screen", 20.0));
    let Some(s) = &mut sc.session else {
        ui.label(&sc.status);
        return;
    };
    let layers = s.flow.m.current_screen().map(|x| x.layers.clone()).unwrap_or_default();
    ui.weak("The layers of the screen shown and their objects. Click one to select it.");
    let mut clicked = None;
    egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        for l in layers {
            let hash = s.flow.fe.layers[l].hash;
            let changes =
                s.edits.get(&hash).map_or(0, |x| x.objects.iter().filter(|o| !o.is_empty()).count() + x.adds.len());
            let title = format!(
                "{}{}",
                s.layer_name(hash),
                if changes > 0 { format!("  ({changes} changed)") } else { String::new() }
            );
            egui::CollapsingHeader::new(title).id_salt(("layer", hash)).default_open(true).show(ui, |ui| {
                for r in s.flow.fe.layers[l].roots.clone() {
                    rows(ui, s, r, &mut clicked);
                }
            });
        }
    });
    if let Some(k) = clicked {
        s.selected = Some(k);
    }
}

fn rows(ui: &mut egui::Ui, s: &Session, i: usize, clicked: &mut Option<Key>) {
    let key = s.key(i);
    let children = s.flow.fe.objects[i].children.clone();
    let change = s.change(key);
    let mut text = egui::RichText::new(s.label(i));
    if change.hidden {
        text = text.weak().strikethrough();
    } else if !change.is_empty() {
        text = text.color(MOVED);
    }
    let row = |ui: &mut egui::Ui, clicked: &mut Option<Key>| {
        if ui.selectable_label(s.selected == Some(key), text.clone()).clicked() {
            *clicked = Some(key);
        }
    };
    if children.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(18.0);
            row(ui, clicked);
        });
        return;
    }
    let id = ui.make_persistent_id(("object", key, i));
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
        .show_header(ui, |ui| row(ui, clicked))
        .body(|ui| {
            for c in children {
                rows(ui, s, c, clicked);
            }
        });
}

/// The selected object's change: its offset, hidden, and the layer's.
fn details(sc: &mut Screens, ui: &mut egui::Ui) {
    let Some(s) = &mut sc.session else { return };
    ui.label(qwc_style::heading("Object", 20.0));
    let Some(key) = s.selected else {
        ui.weak(
            "Select an object on the canvas or in the tree. Drag it to move it (with its children); the arrows \
                 nudge it by a unit (Shift: 10).",
        );
        changes(sc, ui);
        return;
    };
    let Some(i) = s.object(key) else {
        ui.weak("The selected object is not on the screen now.");
        if ui.button("Deselect").clicked() {
            s.selected = None;
        }
        return;
    };
    ui.label(egui::RichText::new(s.label(i)).strong());
    let o = &s.flow.fe.objects[i];
    let (kind, pos, flags) = (o.kind, o.pos, o.flags);
    egui::Grid::new("object facts").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
        ui.label("Layer");
        ui.monospace(s.layer_name(key.0));
        ui.end_row();
        ui.label("Id");
        ui.monospace(format!("{:08x}", key.1));
        ui.end_row();
        ui.label("Kind");
        ui.label(kind_name(kind));
        ui.end_row();
        ui.label("Placed at").on_hover_text("Where the scripts put it, from its parent (the front end's units)");
        ui.monospace(format!("{:.1}, {:.1}", pos[0], pos[1]));
        ui.end_row();
        ui.label("Flags");
        ui.monospace(format!("{flags:#x}"));
        ui.end_row();
    });
    ui.separator();
    ui.label("The remaster's change");
    let change = s.change(key);
    let (mut offset, mut hidden, mut scale) = (change.offset, change.hidden, change.scale);
    let mut pins = [change.pin_x, change.pin_y];
    egui::Grid::new("object change").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
        ui.label("Moved by").on_hover_text("In the front end's 512 x 512 screen's units; its children move with it");
        ui.add(egui::DragValue::new(&mut offset[0]).speed(0.5).max_decimals(2));
        ui.add(egui::DragValue::new(&mut offset[1]).speed(0.5).max_decimals(2));
        ui.end_row();
        ui.label("Scale").on_hover_text("Drawn this many times its size about where it stands, its children with it");
        ui.add(egui::DragValue::new(&mut scale).speed(0.01).range(0.05..=20.0).max_decimals(3));
        ui.end_row();
        let names = |e: Edge, across: bool| match (e, across) {
            (Edge::None, _) => "In the middle",
            (Edge::Start, true) => "Left edge",
            (Edge::Start, false) => "Top edge",
            (Edge::End, true) => "Right edge",
            (Edge::End, false) => "Bottom edge",
            (Edge::At(_), _) => "Anchored at",
            (Edge::Stretch, _) => "Stretched",
        };
        for (k, label) in ["Pinned across", "Pinned down"].into_iter().enumerate() {
            ui.label(label).on_hover_text(
                "On a window of another shape than the original's 4:3: kept in the 4:3 middle, moved with an \
                 edge, anchored to the point this far along the window (0 the left or top edge, 0.5 the middle, \
                 1 the right or bottom; the selection's anchor mark on the canvas drags too, Alt off the snap), \
                 or stretched from edge to edge (a full-screen background). Its children go with it.",
            );
            let current = pins[k].anchor().unwrap_or(0.5);
            egui::ComboBox::from_id_salt(("pin", k)).selected_text(names(pins[k], k == 0)).show_ui(ui, |ui| {
                for e in [Edge::None, Edge::Start, Edge::End, Edge::At(current), Edge::Stretch] {
                    let on = std::mem::discriminant(&pins[k]) == std::mem::discriminant(&e);
                    if ui.selectable_label(on, names(e, k == 0)).clicked() {
                        pins[k] = e;
                    }
                }
            });
            if let Edge::At(mut t) = pins[k] {
                ui.add(egui::DragValue::new(&mut t).speed(0.005).range(0.0..=1.0).max_decimals(3))
                    .on_hover_text("0 the left or top edge, 0.5 the middle, 1 the right or bottom");
                pins[k] = Edge::At(t);
            }
            ui.end_row();
        }
    });
    if kind == Kind::Text {
        let script = s.flow.fe.script_text(i).map(|t| {
            let t = String::from_utf16_lossy(t);
            t.trim_end_matches('\0').to_string()
        });
        ui.horizontal(|ui| {
            ui.label("Text").on_hover_text(
                "Shown in place of the scripts' text, which they still set and read, so a counter or a name the \
                 scripts write stays replaced. The layout's control codes in the original are kept only until the \
                 first edit.",
            );
            if change.text.is_some() && ui.button("The scripts' own").clicked() {
                s.set_text(key, None);
            }
        });
        let mut t = change.text.clone().or(script).unwrap_or_default();
        if ui.add(egui::TextEdit::multiline(&mut t).desired_rows(1).desired_width(f32::INFINITY)).changed() {
            s.set_text(key, Some(t));
        }
    }
    let mut status = None;
    let mut crop = false;
    if kind == Kind::Sprite || kind == Kind::AnimatedSprite {
        let images = sc.pack.as_ref().map(|p| p.join("images"));
        let shown = s.shows.get(&i).cloned();
        let addition = s.addition(key).cloned();
        ui.horizontal(|ui| {
            ui.label("Picture").on_hover_text(
                "Shown in place of its own picture, at that picture's size: a crop (crop:NAME, cut out of one of \
                 the disc's textures, kept as a recipe in the pack's crops.ron), a texture path of the data, or a \
                 PNG in the pack's images/ (drop one on the canvas to set it). An animation's frame k is its \
                 base.k.png where there is one, else the base stands in for every frame. Export writes what it \
                 shows (an animated sprite: every frame, named that way) to that folder, to edit outside. \
                 Duplicate adds a sprite of the same picture and place to the layer — an animated one running the \
                 same animation on its own — to pin, move and retexture apart.",
            );
            if ui
                .add_enabled(shown.is_some(), egui::Button::new("Crop..."))
                .on_hover_text("Cut its picture out of the disc's texture it shows, or edit the crop it shows")
                .clicked()
            {
                crop = true;
            }
            if ui.add_enabled(shown.is_some(), egui::Button::new("Export PNG")).clicked() {
                status = Some(match (&images, kind) {
                    (None, _) => NO_PACK.into(),
                    (Some(images), Kind::AnimatedSprite) => export_frames(s, images, i),
                    (Some(images), _) => export_png(s, images, shown.as_deref().unwrap_or_default()),
                });
            }
            if ui.add_enabled(shown.is_some(), egui::Button::new("Duplicate")).clicked() {
                let parent = match &addition {
                    Some(a) => a.parent,
                    None => s.flow.fe.objects[i].parent.map_or(0, |p| s.flow.fe.objects[p].id),
                };
                let o = &s.flow.fe.objects[i];
                let (sized, osize, order, pos) = (o.flags & 0x10000 != 0, o.size, o.layer_order, [o.pos[0], o.pos[1]]);
                let (image, animate) = match &addition {
                    Some(a) => (a.image.clone(), a.animate),
                    // an animated sprite's copy runs its animation and shows its frames (or its replacement's)
                    None if kind == Kind::AnimatedSprite => {
                        (change.image.clone().unwrap_or_else(|| Disc::resource_spec(o.script, o.slot(0xc0))), key.1)
                    }
                    None => (change.image.clone().or_else(|| shown.clone()).unwrap_or_default(), 0),
                };
                let base = s.names.get(&key.1).cloned().unwrap_or_else(|| file_stem(&image).to_string());
                let taken = s.edits.get(&key.0).map(|l| l.adds.clone()).unwrap_or_default();
                let mut name = format!("{base} copy");
                let mut n = 2;
                while taken.iter().any(|a| a.name == name) {
                    name = format!("{base} copy {n}");
                    n += 1;
                }
                let a = qwc_frontend::layouts::AddLayout {
                    name: name.clone(),
                    image,
                    pos,
                    size: if sized { osize } else { [0; 2] },
                    parent,
                    order,
                    animate,
                };
                let id = s.add_sprite(key.0, a);
                s.selected = Some((key.0, id));
                status = Some(format!("Added {name}: drawn from the next tick; drag it, pin it, or drop its own PNG"));
            }
        });
        {
            let current = addition.as_ref().map(|a| a.image.clone()).or_else(|| change.image.clone());
            let mut img = current.clone().unwrap_or_default();
            ui.horizontal(|ui| {
                if ui
                    .add(egui::TextEdit::singleline(&mut img).hint_text("its own picture").desired_width(220.0))
                    .changed()
                {
                    let img = (!img.is_empty()).then_some(img);
                    if addition.is_none() || img.is_some() {
                        s.set_image(key, img);
                    }
                }
                if addition.is_none() && current.is_some() && ui.button("Its own").clicked() {
                    s.set_image(key, None);
                }
            });
        }
        if addition.is_some() && ui.button("Delete this added sprite").clicked() {
            s.remove_add(key);
            if let Some(st) = status.take() {
                sc.status = st;
            }
            changes(sc, ui);
            return;
        }
    }
    if crop {
        crate::crop::open_for(sc, key);
        return;
    }
    let Some(s) = &mut sc.session else { return };
    ui.checkbox(&mut hidden, "Hidden (and its children)");
    if offset != change.offset {
        s.set_offset(key, offset);
    }
    if hidden != change.hidden {
        s.set_hidden(key, hidden);
    }
    if scale != change.scale {
        s.set_scale(key, scale);
    }
    if pins != [change.pin_x, change.pin_y] {
        s.set_pins(key, pins);
    }
    ui.horizontal(|ui| {
        if ui.button("Reset").on_hover_text("As the scripts place and word it").clicked() {
            s.set_offset(key, [0.0; 2]);
            s.set_hidden(key, false);
            s.set_scale(key, 1.0);
            s.set_pins(key, [Edge::None; 2]);
            s.set_text(key, None);
        }
        if let Some(p) = s.flow.fe.objects[i].parent {
            if ui.button("Select its parent").clicked() {
                s.selected = Some(s.key(p));
            }
        }
    });
    if let Some(st) = status {
        sc.status = st;
    }
    ui.separator();
    changes(sc, ui);
}

/// Write what the sprite shows (the texture at `path`) to `<images>/<stem>.png`, to edit outside or stand in for
/// another sprite's picture; answers the status line.
fn export_png(s: &Session, images: &std::path::Path, path: &str) -> String {
    let Some(t) = s.disc.shown(path) else { return format!("{path}: not decoded yet") };
    let out = images.join(format!("{}.png", file_stem(path)));
    let p = &t.pixels;
    match qwc_assets::remaster::save_png(&out, p.width, p.height, &p.levels[0]) {
        Ok(()) => format!("Wrote {}", out.display()),
        Err(e) => e,
    }
}

/// Write every frame of animated sprite `i` to `<images>/`: frame 0 as `<stem>.png`, frame k as `<stem>.<k>.png`
/// (the names a replacement's frames are looked for by); answers the status line.
fn export_frames(s: &Session, images: &std::path::Path, i: usize) -> String {
    let fe = &s.flow.fe;
    let base = fe.shown_tex(i);
    // the frame count (`+0xc4`), as the frame script is clamped by
    let count = (fe.objects[i].slot(0xc4) as u8).max(1);
    let Some((first, _)) = s.disc.texture(base) else { return "Its texture is not loaded".into() };
    let stem = file_stem(&first).to_string();
    let mut wrote = 0;
    for k in 0..count {
        let Some((path, _)) = s.disc.texture(Tex { frame: k, ..base }) else { continue };
        let Some(t) = s.disc.shown(&path) else { continue };
        let name = if k == 0 { format!("{stem}.png") } else { format!("{stem}.{k}.png") };
        let p = &t.pixels;
        if let Err(e) = qwc_assets::remaster::save_png(&images.join(&name), p.width, p.height, &p.levels[0]) {
            return e;
        }
        wrote += 1;
    }
    format!("Wrote {wrote} frame{} of {stem} to {}", if wrote == 1 { "" } else { "s" }, images.display())
}

/// Every change made, by layer, and saving them.
fn changes(sc: &mut Screens, ui: &mut egui::Ui) {
    let Some(s) = &mut sc.session else { return };
    ui.label(qwc_style::heading("Changes", 20.0));
    let dirty = s.dirty();
    ui.horizontal(|ui| {
        if ui.add_enabled(dirty, egui::Button::new("Save    Ctrl+S")).clicked() {
            sc.status = s.save();
        }
        if ui.add_enabled(dirty, egui::Button::new("Revert")).on_hover_text("Back to the files as saved").clicked() {
            s.edits = s.saved.clone();
            s.crops = s.layouts.crops().clone();
            s.apply();
        }
    });
    ui.weak(match &sc.pack {
        Some(pack) => format!(
            "Saved to {} (one file per layer, the crops, the PNGs in images/): what the packs under it do not have \
             already. The game reads them again when they change.",
            pack.display()
        ),
        None => format!("{NO_PACK}; without one the screens are the original's."),
    });
    let mut select = None;
    let mut layers: Vec<&LayerLayout> =
        (s.edits.values()).filter(|l| !l.adds.is_empty() || l.objects.iter().any(|o| !o.is_empty())).collect();
    layers.sort_by(|a, b| a.layer.cmp(&b.layer));
    for l in layers {
        ui.label(egui::RichText::new(&l.layer).strong());
        for a in &l.adds {
            let label = format!("  + {} ({})", a.name, file_stem(&a.image));
            if ui.selectable_label(s.selected == Some((l.hash, a.id())), label).clicked() {
                select = Some((l.hash, a.id()));
            }
        }
        for o in l.objects.iter().filter(|o| !o.is_empty()) {
            let what = if o.note.is_empty() { format!("{:08x}", o.id) } else { o.note.clone() };
            let change = describe(o);
            if ui.selectable_label(s.selected == Some((l.hash, o.id)), format!("  {what}: {change}")).clicked() {
                select = Some((l.hash, o.id));
            }
        }
    }
    if let Some(k) = select {
        s.selected = Some(k);
    }
}

/// An object's change in a few words.
fn describe(o: &qwc_frontend::ObjectLayout) -> String {
    if o.hidden {
        return "hidden".into();
    }
    let mut parts = Vec::new();
    if o.offset != [0.0; 2] {
        parts.push(format!("moved {:+.1}, {:+.1}", o.offset[0], o.offset[1]));
    }
    if let Some(t) = &o.text {
        let t = t.replace('\n', " ");
        let short: String = t.chars().take(18).collect();
        parts.push(format!("\"{short}{}\"", if t.chars().count() > 18 { "..." } else { "" }));
    }
    if let Some(img) = &o.image {
        parts.push(format!("picture {}", file_stem(img)));
    }
    if o.scale != 1.0 {
        parts.push(format!("x{}", o.scale));
    }
    for (e, across) in [(o.pin_x, true), (o.pin_y, false)] {
        parts.push(match (e, across) {
            (Edge::None, _) => continue,
            (Edge::Start, true) => "left".into(),
            (Edge::Start, false) => "top".into(),
            (Edge::End, true) => "right".into(),
            (Edge::End, false) => "bottom".into(),
            (Edge::At(t), true) => format!("anchored {t} across"),
            (Edge::At(t), false) => format!("anchored {t} down"),
            (Edge::Stretch, true) => "stretched across".into(),
            (Edge::Stretch, false) => "stretched down".into(),
        });
    }
    parts.join(", ")
}

// ---------------------------------------------------------------------------------------------------------------
// the canvas

fn canvas(sc: &mut Screens, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
    let area = response.rect;
    let painter = painter.with_clip_rect(area);
    painter.rect_filled(area, 0.0, qwc_style::WELL);
    let [w, h] = sc.screen.map(|v| v as f32);
    let size = Vec2::new(w, h);
    let fit = ((area.width() - 40.0) / w).min((area.height() - 40.0) / h).max(0.01);
    let mut zoom = sc.view.zoom.unwrap_or(fit);
    let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
    if let (true, Some(p)) = (scroll != 0.0, response.hover_pos()) {
        let new = (zoom * (scroll * 0.002).exp()).clamp(0.02, 16.0);
        let origin = area.center() - size * zoom / 2.0 + sc.view.pan;
        let moved = p - (p - origin) * (new / zoom);
        sc.view.pan = moved - (area.center() - size * new / 2.0);
        (zoom, sc.view.zoom) = (new, Some(new));
    }
    let origin = area.center() - size * zoom / 2.0 + sc.view.pan;
    // the front end's 512 x 512 screen where the game puts it on the previewed one
    let fit = sc.fit();
    let span = fit.span;
    let (kx, ky) = (w * zoom / (span[2] - span[0]), h * zoom / (span[3] - span[1]));
    let at = |p: [f32; 2]| origin + Vec2::new((p[0] - span[0]) * kx, (p[1] - span[1]) * ky);
    let screen_rect = Rect::from_min_size(origin, size * zoom);
    painter.rect_filled(screen_rect, 0.0, Color32::from_rgb(0x2a, 0x2e, 0x3a));
    let four_three = Rect::from_two_pos(at([0.0, 0.0]), at([512.0, 512.0]));
    if four_three != screen_rect {
        // the original's 4:3 screen in the middle; the pinned objects go past it to the edges
        painter.rect_filled(four_three, 0.0, Color32::from_rgb(0x32, 0x37, 0x45));
    }
    sc.view.pointer = response.hover_pos().map(|p| [span[0] + (p.x - origin.x) / kx, span[1] + (p.y - origin.y) / ky]);

    let Some(s) = &mut sc.session else {
        painter.text(
            area.center(),
            egui::Align2::CENTER_CENTER,
            &sc.status,
            egui::FontId::proportional(16.0),
            Color32::GRAY,
        );
        return;
    };
    // the text and the remastered textures' shapes are measured for the screen's own place
    let screen = fit.area((sc.screen[0], sc.screen[1]));
    let paint = &mut sc.paint;
    let (dropped, seen) = s.disc.dropped(paint.dropped);
    paint.dropped = seen;
    for path in dropped {
        paint.textures.remove(&path);
    }
    if paint.text_screen != screen {
        paint.text.clear();
        paint.glyphs.clear();
        paint.text_screen = screen;
    }
    let now = s.current();
    let margins = fit.margins();
    // where an object's records go with the edits as they are now: moved by the edits since the list was drawn,
    // then scaled and pinned as they say (none when one hides it)
    let place = |i: usize| -> Option<([f32; 2], Xf)> {
        let shift = s.shift(&now, i)?;
        Some((shift, s.list.xf_under(&s.flow.fe, &now, margins, i)?))
    };
    let mut quads = Vec::new();
    let mut shows = HashMap::new();
    let disc = s.disc.clone();
    let list = &s.list;
    for sp in &list.sprites {
        let Some((shift, xf)) = place(sp.object) else { continue };
        let Some((path, tsize)) = disc.texture(sp.tex) else { continue };
        // what it shows, for Export and Duplicate, though a mask (which writes only the alpha) is not painted here
        shows.insert(sp.object, path.clone());
        let Some(m) = mode(sp.material[0]) else { continue };
        let Some((texture, shape)) = paint.texture(&ctx, &disc, &path) else { continue };
        let v = sprite_quad(sp, tsize, list.wipe(sp.layer).as_ref());
        let mut p = v.map(|v| [v.pos[0] + shift[0], v.pos[1] + shift[1]]);
        if let Some(shape) = shape {
            p = keep_shape(p, shape, screen);
        }
        let p = p.map(|p| xf.at(p));
        quads.push(Quad {
            order: sp.order,
            texture,
            pos: p.map(at),
            uv: v.map(|v| uv(&v)),
            colour: v.map(|v| colour(v.argb, m)),
            object: Some(sp.object),
        });
    }
    for spark in &list.sparks {
        let Some(m) = mode(spark.material[0]) else { continue };
        let Some((shift, xf)) = place(spark.object) else { continue };
        let Some((path, _)) = disc.texture(spark.tex) else { continue };
        let Some((texture, shape)) = paint.texture(&ctx, &disc, &path) else { continue };
        let mut p = spark.corners.map(|v| [v.pos[0] + shift[0], v.pos[1] + shift[1]]);
        if let Some(shape) = shape {
            p = keep_shape(p, shape, screen);
        }
        let p = p.map(|p| xf.at(p));
        let v = spark.corners;
        quads.push(Quad {
            order: spark.order,
            texture,
            pos: p.map(at),
            uv: v.map(|v| uv(&v)),
            colour: v.map(|v| colour(v.argb, m)),
            object: Some(spark.object),
        });
    }
    // a glyph's pixels (of the screen's own place) in the screen's units
    let (ux, uy) = (512.0 / screen.0 as f32, 512.0 / screen.1 as f32);
    for run in &list.texts {
        let Some((shift, xf)) = place(run.object) else { continue };
        let Some(text) = s.flow.fe.text(run.object) else { continue };
        let mut run = run.clone();
        run.pos[0] += shift[0];
        run.pos[1] += shift[1];
        let argb = qwc_feck::draw::pack_colour(run.colour);
        if let Some(font) = disc.vector_font(run.font) {
            let start = (run.start as usize).min(text.len());
            let chars = &text[start..(start + run.count as usize).min(text.len())];
            let c = colour(argb, Mode::Alpha);
            for g in paint.text.run(font, chars, [run.pos[0], run.pos[1]], screen) {
                let Some(texture) = paint.glyph(&ctx, &g.key) else { continue };
                let (x0, y0) = (g.x * ux, g.y * uy);
                let (x1, y1) = (x0 + g.width as f32 * ux, y0 + g.height as f32 * uy);
                let pos = [[x0, y1], [x1, y1], [x0, y0], [x1, y0]].map(|p| at(xf.at(p)));
                let uv = [Pos2::new(0.0, 1.0), Pos2::new(1.0, 1.0), Pos2::new(0.0, 0.0), Pos2::new(1.0, 0.0)];
                quads.push(Quad { order: run.order, texture, pos, uv, colour: [c; 4], object: Some(run.object) });
            }
        } else if let Some(font) = disc.bitmap_font(run.font) {
            let Some(texture) = paint.atlas(&ctx, run.font, font) else { continue };
            for q in text_quads(&run, text, font, [font.tex_w, font.tex_h]) {
                quads.push(Quad {
                    order: run.order,
                    texture,
                    pos: q.map(|v| at(xf.at([v.pos[0], v.pos[1]]))),
                    uv: q.map(|v| uv(&v)),
                    colour: q.map(|v| colour(v.argb, Mode::Alpha)),
                    object: Some(run.object),
                });
            }
        }
    }
    // by material layer, then in the list's order, as the overlay draws them
    quads.sort_by_key(|q| q.order);
    let clip = painter.clip_rect().intersect(screen_rect);
    let screen_painter = painter.with_clip_rect(clip);
    // the game's own painter's frame, with the edits not yet drawn laid over the list; else the quads here
    let exact = match sc.exact.as_mut().filter(|_| sc.show_exact) {
        Some(x) => {
            let list = s.adjusted(&now, margins);
            let size = (sc.screen[0], sc.screen[1]);
            Some(x.paint(&list, &s.flow, &s.disc, size, s.ticks as f64 * TICK))
        }
        None => None,
    };
    if let Some(id) = exact {
        // the game's alpha is the masks' channel: drawn over black, as the window shows it
        screen_painter.rect_filled(screen_rect, 0.0, Color32::BLACK);
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        screen_painter.image(id, screen_rect, uv, Color32::WHITE);
        if four_three != screen_rect {
            let edge = Stroke::new(1.0, Color32::from_white_alpha(70));
            screen_painter.extend(egui::Shape::dashed_line(
                &[four_three.left_top(), four_three.left_bottom()],
                edge,
                6.0,
                4.0,
            ));
            screen_painter.extend(egui::Shape::dashed_line(
                &[four_three.right_top(), four_three.right_bottom()],
                edge,
                6.0,
                4.0,
            ));
        }
    }
    let mut boxes = Vec::new();
    for q in &quads {
        if exact.is_none() {
            let mut mesh = egui::Mesh::with_texture(q.texture);
            for k in 0..4 {
                mesh.vertices.push(egui::epaint::Vertex { pos: q.pos[k], uv: q.uv[k], color: q.colour[k] });
            }
            mesh.indices.extend([0, 1, 2, 2, 1, 3]);
            screen_painter.add(egui::Shape::mesh(mesh));
        }
        if let Some(o) = q.object {
            boxes.push((o, Rect::from_points(&q.pos)));
        }
    }
    s.shows = shows;
    painter.rect_stroke(screen_rect, 0.0, Stroke::new(1.0, Color32::from_gray(110)), StrokeKind::Outside);

    // the hovered object's box, and the selection's (it and everything below it)
    let hover = response.hover_pos().and_then(|p| boxes.iter().rev().find(|(_, r)| r.contains(p)).map(|(o, _)| *o));
    if let Some(r) = hover.filter(|&o| Some(s.key(o)) != s.selected).and_then(|o| bounds(s, &boxes, o)) {
        painter.rect_stroke(r, 0.0, Stroke::new(1.0, HOVER), StrokeKind::Middle);
    }
    let selected = s.selected.and_then(|k| s.object(k));
    if let Some(r) = selected.and_then(|o| bounds(s, &boxes, o)) {
        painter.rect_stroke(r.expand(1.0), 0.0, Stroke::new(1.5, SELECTED), StrokeKind::Middle);
    }
    // the selection's anchor mark: the point of the window it is pinned to (the middle unpinned). Drag it to
    // anchor the object there; it snaps to the edges and the middle (Alt skips the snap).
    let anchor_mark = s.selected.filter(|_| selected.is_some()).map(|k| {
        let c = s.change(k);
        let a = [c.pin_x.anchor().unwrap_or(0.5), c.pin_y.anchor().unwrap_or(0.5)];
        at([span[0] + a[0] * (span[2] - span[0]), span[1] + a[1] * (span[3] - span[1])])
    });
    if let Some(p) = anchor_mark {
        let stroke = Stroke::new(1.5, SELECTED);
        painter.circle_stroke(p, 5.0, stroke);
        for d in [Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0)] {
            painter.line_segment([p - d * 9.0, p - d * 3.0], stroke);
            painter.line_segment([p + d * 3.0, p + d * 9.0], stroke);
        }
    }

    // picking and dragging
    let (mods, space, press) = ctx.input(|i| (i.modifiers, i.key_down(egui::Key::Space), i.pointer.press_origin()));
    let pick = |p: Pos2| boxes.iter().rev().find(|(_, r)| r.contains(p)).map(|(o, _)| *o);
    if response.clicked() {
        s.selected = match response.interact_pointer_pos().and_then(pick) {
            Some(o) if mods.alt => Some(s.flow.fe.objects[o].parent.map_or(s.key(o), |p| s.key(p))),
            Some(o) => Some(s.key(o)),
            None => None,
        };
    }
    if response.drag_started() {
        let from = press.unwrap_or_default();
        if response.dragged_by(egui::PointerButton::Middle) || space {
            sc.view.drag = Some(Drag { key: None, anchor: false, from, offset: [0.0; 2], pan: sc.view.pan });
        } else if let Some(k) = s.selected.filter(|_| anchor_mark.is_some_and(|m| m.distance(from) <= 9.0)) {
            sc.view.drag = Some(Drag { key: Some(k), anchor: true, from, offset: [0.0; 2], pan: sc.view.pan });
        } else {
            let on_selection = selected.and_then(|o| bounds(s, &boxes, o)).is_some_and(|r| r.contains(from));
            let key = if on_selection { s.selected } else { pick(from).map(|o| s.key(o)) };
            if let Some(k) = key {
                s.selected = Some(k);
                sc.view.drag = Some(Drag { key: Some(k), anchor: false, from, offset: s.offset(k), pan: sc.view.pan });
            }
        }
    }
    if let (Some(d), true, Some(p)) = (&sc.view.drag, response.dragged(), response.interact_pointer_pos()) {
        let delta = p - d.from;
        match d.key {
            None => sc.view.pan = d.pan + delta,
            // the anchor mark dragged: each axis anchored where the pointer is, as a share of the window,
            // snapped to the edges and the middle (Alt: anywhere); a stretched axis stays stretched
            Some(k) if d.anchor => {
                let u = [span[0] + (p.x - origin.x) / kx, span[1] + (p.y - origin.y) / ky];
                let c = s.change(k);
                let mut pins = [c.pin_x, c.pin_y];
                for (axis, (lo, hi)) in [(span[0], span[2]), (span[1], span[3])].into_iter().enumerate() {
                    if pins[axis] == Edge::Stretch {
                        continue;
                    }
                    let a = ((u[axis] - lo) / (hi - lo).max(1.0e-6)).clamp(0.0, 1.0);
                    pins[axis] = match a {
                        _ if mods.alt => Edge::At((a * 1000.0).round() / 1000.0),
                        _ if a < 0.02 => Edge::Start,
                        _ if (a - 0.5).abs() < 0.02 => Edge::None,
                        _ if a > 0.98 => Edge::End,
                        _ => Edge::At((a * 1000.0).round() / 1000.0),
                    };
                }
                s.set_pins(k, pins);
            }
            Some(k) => {
                let mut by = [delta.x / kx, delta.y / ky];
                if mods.shift {
                    let i = usize::from(by[0].abs() > by[1].abs());
                    by[i] = 0.0;
                }
                s.set_offset(k, [d.offset[0] + by[0], d.offset[1] + by[1]]);
            }
        }
    }
    if response.drag_stopped() || !ctx.input(|i| i.pointer.any_down()) {
        sc.view.drag = None;
    }
    s.boxes = boxes;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The game data from the crate's folder (`$QWC_DATA`, else the repo's `original/cd_files`).
    fn data() -> Option<(&'static Vfs, Vec<PathBuf>)> {
        let dir = std::env::var_os("QWC_DATA").map_or_else(|| PathBuf::from("../../original/cd_files"), PathBuf::from);
        let vfs = Vfs::open(&dir).ok()?;
        let mut exes = qwc_frontend::session::exe_candidates(None, &dir);
        exes.push("../../analysis/QWC.exe".into());
        Some((Box::leak(Box::new(vfs)), exes))
    }

    /// A frame of the canvas, with no window (the textures it uploads are dropped).
    fn paint(ctx: &egui::Context, sc: &mut Screens) {
        let screen_rect = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 800.0)));
        let mut out = ctx.run_ui(egui::RawInput { screen_rect, ..Default::default() }, |ui| canvas(sc, ui));
        out.textures_delta.clear();
    }

    #[test]
    fn the_title_runs_draws_and_its_logo_moves_with_the_layout_and_saves() {
        let Some((vfs, exes)) = data() else {
            eprintln!("no game data; skipping");
            return;
        };
        let dir = std::env::temp_dir().join(format!("qwc-editor-screens-{}", std::process::id()));
        let ctx = egui::Context::default();
        let pack = dir.join("pack");
        let mut sc = Screens::new(vfs, exes, Some(pack.clone()), Some("StartScreen".into()));
        // none of the profile's packs under it
        sc.lower.clear();
        sc.restart();
        let screen = (sc.screen[0], sc.screen[1]);
        let s = sc.session.as_mut().expect("the title starts");
        for _ in 0..120 {
            s.tick(screen);
        }
        paint(&ctx, &mut sc);
        let s = sc.session.as_mut().unwrap();
        assert!(!s.boxes.is_empty(), "the title draws");
        // the logo (HPLogo, an object of Opening_TitleScreen)
        let logo = (name_hash("Opening_TitleScreen"), name_hash("HPLogo"));
        let i = s.object(logo).expect("the logo is on the screen");
        assert_eq!(s.label(i), "Sprite HPLogo");
        let top = |s: &Session| bounds(s, &s.boxes, i).map(|r| r.top()).expect("the logo draws");
        let before = top(s);
        // moved while paused: drawn moved at once, and still there once the next tick has drawn it so
        s.set_offset(logo, [0.0, 40.0]);
        paint(&ctx, &mut sc);
        let s = sc.session.as_mut().unwrap();
        let moved = top(s);
        assert!(moved - before > 20.0, "{before} -> {moved}");
        s.tick(screen);
        paint(&ctx, &mut sc);
        let s = sc.session.as_mut().unwrap();
        assert!((top(s) - moved).abs() < 2.0, "{moved} -> {}", top(s));
        assert!(s.dirty());
        assert!(s.save().starts_with("Saved"));
        assert!(!s.dirty());
        let back = Layouts::new(Some(pack.clone()));
        let l = back.layer(logo.0).expect("the layer's file");
        assert_eq!((l.layer.as_str(), l.object(logo.1).map(|o| o.offset)), ("Opening_TitleScreen", Some([0.0, 40.0])));
        // on the 16:9 preview: pinned to the right edge it goes as far right as the window reaches past the 4:3
        // middle (a quarter of the 4:3 width across both sides at 16:9), at once while paused
        let left = |s: &Session| bounds(s, &s.boxes, i).map(|r| (r.left(), r.width())).expect("the logo draws");
        let (x0, w0) = left(s);
        s.set_pins(logo, [Edge::End, Edge::None]);
        paint(&ctx, &mut sc);
        let s = sc.session.as_mut().unwrap();
        let (x1, w1) = left(s);
        let canvas_w = (1400.0f32 - 40.0).min((800.0 - 40.0) * 16.0 / 9.0);
        let margin = canvas_w * (1.0 - 0.75) / 2.0;
        assert!((x1 - x0 - margin).abs() < 2.0 && (w1 - w0).abs() < 1.0, "{x0} -> {x1}, {margin}");
        // anchored three quarters of the way across: half as far as the right edge
        s.set_pins(logo, [Edge::At(0.75), Edge::None]);
        paint(&ctx, &mut sc);
        let s = sc.session.as_mut().unwrap();
        let (x2, w2) = left(s);
        assert!((x2 - x0 - margin * 0.5).abs() < 2.0 && (w2 - w0).abs() < 1.0, "{x0} -> {x2}, {margin}");
        s.set_pins(logo, [Edge::End, Edge::None]);
        paint(&ctx, &mut sc);
        let s = sc.session.as_mut().unwrap();
        // twice the size about where it stands
        s.set_scale(logo, 2.0);
        paint(&ctx, &mut sc);
        let s = sc.session.as_mut().unwrap();
        assert!((left(s).1 / w1 - 2.0).abs() < 0.05, "{w1} -> {}", left(s).1);
        // duplicated: a real object of the layer showing the same picture, with changes of its own
        let shown = s.shows.get(&i).cloned().expect("the logo's texture");
        let o = &s.flow.fe.objects[i];
        let (pos, order, parent) =
            ([o.pos[0], o.pos[1]], o.layer_order, o.parent.map_or(0, |p| s.flow.fe.objects[p].id));
        let add = qwc_frontend::layouts::AddLayout {
            name: "HPLogo copy".into(),
            image: shown.clone(),
            pos,
            size: [0; 2],
            parent,
            order,
            animate: 0,
        };
        let copy = (logo.0, s.add_sprite(logo.0, add));
        s.tick(screen);
        paint(&ctx, &mut sc);
        let s = sc.session.as_mut().unwrap();
        let k = s.object(copy).expect("the copy is an object of the layer");
        assert_eq!(s.label(k), "Sprite HPLogo copy");
        let copy_w = bounds(s, &s.boxes, k).expect("the copy draws").width();
        // unpinned and unscaled, it draws at the picture's own width, like the logo before its changes
        assert!((copy_w - w0).abs() < 1.5, "{copy_w} vs {w0}");
        // its picture through a PNG file: exported, then shown from the export
        let msg = export_png(s, &pack.join("images"), &shown);
        assert!(msg.starts_with("Wrote"), "{msg}");
        s.set_image(copy, Some(format!("{}.png", file_stem(&shown))));
        s.tick(screen);
        paint(&ctx, &mut sc);
        let s = sc.session.as_mut().unwrap();
        let k = s.object(copy).unwrap();
        assert!(bounds(s, &s.boxes, k).expect("drawn from the PNG").width() > 4.0);
        // saved with the layer's file, and taken out again
        assert!(s.save().starts_with("Saved"));
        assert_eq!(Layouts::new(Some(pack.clone())).layer(logo.0).map(|l| l.adds.len()), Some(1));
        let file = pack.join("Opening_TitleScreen.ron");
        let in_pack = LayerLayout::from_ron(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(in_pack.adds.len(), 1);
        // the copy showing the left half of the logo's texture, cut: drawn as wide as the logo, saved as a recipe
        let mut crops = s.crops.clone();
        let size = s.texture_size(&shown).expect("the logo's texture");
        let (w, h) = (size[0] as f32, size[1] as f32);
        let half = qwc_frontend::crops::Crop {
            texture: shown.clone(),
            points: vec![[0.0, 0.0], [w / 2.0, 0.0], [w / 2.0, h], [0.0, h]],
            outside: false,
        };
        crops.insert("logo_left".into(), half.clone());
        // one opened and left: not saved
        crops.insert("idle".into(), qwc_frontend::crops::Crop { texture: shown.clone(), ..Default::default() });
        s.crops = crops;
        s.set_image(copy, Some(qwc_frontend::crops::spec("logo_left")));
        assert_eq!(s.crop_users("logo_left"), 1);
        s.tick(screen);
        paint(&ctx, &mut sc);
        let s = sc.session.as_mut().unwrap();
        let k = s.object(copy).unwrap();
        let cut_w = bounds(s, &s.boxes, k).expect("drawn from the crop").width();
        assert!((cut_w - w0).abs() < 1.5, "{cut_w} vs {w0}");
        let (cut, _) = s.disc.texture(s.flow.fe.shown_tex(k)).expect("the cut");
        let right = s.disc.shown(&cut).expect("cut").pixels.levels[0][(size[0] as usize - 1) * 4 + 3];
        assert_eq!(right, 0, "the right half is cut away");
        assert!(s.save().starts_with("Saved"));
        assert_eq!(qwc_frontend::crops::read(&pack).get("logo_left"), Some(&half));
        assert!(!qwc_frontend::crops::read(&pack).contains_key("idle") && !s.crops.contains_key("idle"));
        // the copy shows the crop now, in the pack's file
        let in_pack = LayerLayout::from_ron(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(in_pack.adds.iter().map(|a| a.image.as_str()).collect::<Vec<_>>(), ["crop:logo_left"]);
        s.remove_add(copy);
        s.tick(screen);
        paint(&ctx, &mut sc);
        let s = sc.session.as_mut().unwrap();
        assert!(s.object(copy).is_none(), "removed from the layer");
        let _ = std::fs::remove_dir_all(dir);
    }
}
