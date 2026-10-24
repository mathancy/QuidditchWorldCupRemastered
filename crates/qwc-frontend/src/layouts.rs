//! Not the original's: the remaster's layout changes to the front end's screens. Per layer script, objects drawn
//! moved or hidden ([`qwc_feck::draw::Adjust`]: only the drawing changes, the scripts still place and animate the
//! objects as they did). Kept as RON, one file per layer (`<layer>.ron`, the layer script's name as `Screens.lst`
//! gives it, without `.vso`); made in the HUD editor's Screens mode. The game reads them all, and again when one
//! changes ([`Layouts::refresh`]).
//!
//! They live in the resource packs only (`qwc_packs`, each pack's `screens/`): without a pack on, every screen is
//! drawn as the original draws it. A pack's folder holds its layer files, its crops' file (`crate::crops`: pictures
//! cut from the disc's textures, kept as recipes) and its PNGs in `images/`.
//!
//! A layer's changes are the packs' merged ([`merge`]), the lowest pack's first ([`Layouts::with_lower`]): each
//! object takes the highest pack's change of it, and a lower pack's pictures are found in its own `images/`. The
//! editor saves into the highest pack only ([`default_pack`], `$QWC_SCREEN_PACK` elsewhere), and only what the lower
//! packs do not already have.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use qwc_feck::draw::{Adjust, Pin};
use qwc_feck::FrontEnd;
use serde::{Deserialize, Serialize};

use crate::crops::Crops;

/// One layer's changes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayerLayout {
    /// The layer script's name as `Screens.lst` gives it, without `.vso` (`Opening_TitleScreen`).
    pub layer: String,
    /// Its name hash (the script's, `qwc_formats::flow::name_hash` of `layer` as `Screens.lst` spells it), by which
    /// its objects are found; 0 to take it from `layer`.
    pub hash: u32,
    pub objects: Vec<ObjectLayout>,
    /// The sprites the remaster adds to the layer.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub adds: Vec<AddLayout>,
}

/// A sprite added to a layer: a real object of it, moved, pinned and hidden by an [`ObjectLayout`] of its id
/// (the hash of `name`), which the scripts never touch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AddLayout {
    /// Its name, for a person; its id is the name's hash.
    pub name: String,
    /// Its picture: a PNG in the texture pack's `images/`, a crop (`crop:NAME`), or a texture path of the data.
    pub image: String,
    /// Where it stands, as a script would place it (from its parent).
    #[serde(skip_serializing_if = "is_zero2")]
    pub pos: [f32; 2],
    /// Drawn this size; `(0, 0)` takes the image's own.
    #[serde(skip_serializing_if = "is_zero_size")]
    pub size: [u16; 2],
    /// The id of the layer object it hangs under (moved and coloured with it); 0 for a layer root.
    #[serde(skip_serializing_if = "is_zero")]
    pub parent: u32,
    /// The overlay's material layer it is drawn in.
    #[serde(skip_serializing_if = "is_zero")]
    pub order: u32,
    /// The id of an animated sprite of the layer whose animation it copies, run on its own from the start (its
    /// `image` is usually that sprite's frames, `res:<script>:<id>`); 0 for a still sprite.
    #[serde(skip_serializing_if = "is_zero")]
    pub animate: u32,
}

impl Default for AddLayout {
    fn default() -> AddLayout {
        AddLayout {
            name: String::new(),
            image: String::new(),
            pos: [0.0; 2],
            size: [0; 2],
            parent: 0,
            order: 0,
            animate: 0,
        }
    }
}

impl AddLayout {
    /// Its object id, which its changes go by.
    pub fn id(&self) -> u32 {
        qwc_formats::flow::name_hash(&self.name)
    }
}

/// One object's change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ObjectLayout {
    /// Its id in the layer (`_LayerData`; the scripts find it by this).
    pub id: u32,
    /// What it is, for whoever reads the file (its kind, its text or texture).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// Drawn moved by this, its children with it, in the front end's 512 x 512 screen's units.
    #[serde(skip_serializing_if = "is_zero2")]
    pub offset: [f32; 2],
    /// Not drawn, nor its children.
    #[serde(skip_serializing_if = "is_false")]
    pub hidden: bool,
    /// Drawn this many times its size about where it stands, its children with it.
    #[serde(skip_serializing_if = "is_one")]
    pub scale: f32,
    /// A text object only: drawn showing this in place of the scripts' text (which they still set and read).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// A sprite only: drawn showing this picture in place of its own, at the picture's size (a PNG in the texture
    /// pack's `images/`, a crop, `crop:NAME`, or a texture path of the data).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Across and down: kept at an edge of a window wider (or taller) than 4:3, anchored to a point of it, or
    /// stretched to both edges.
    #[serde(skip_serializing_if = "Edge::is_none")]
    pub pin_x: Edge,
    #[serde(skip_serializing_if = "Edge::is_none")]
    pub pin_y: Edge,
}

impl Default for ObjectLayout {
    fn default() -> ObjectLayout {
        ObjectLayout {
            id: 0,
            note: String::new(),
            offset: [0.0; 2],
            hidden: false,
            scale: 1.0,
            text: None,
            image: None,
            pin_x: Edge::None,
            pin_y: Edge::None,
        }
    }
}

/// [`qwc_feck::draw::Pin`] as the files spell it: `Start` the left (or top) edge, `End` the right (or bottom),
/// `At(t)` the point `t` of the way along the window (0 `Start`, 0.5 the middle, 1 `End`), `Stretch` both edges.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Edge {
    #[default]
    None,
    Start,
    End,
    At(f32),
    Stretch,
}

impl Edge {
    fn is_none(&self) -> bool {
        *self == Edge::None
    }

    /// The anchor point the edge stands for, for an editor: `None` ([`Edge::Stretch`] has none).
    pub fn anchor(self) -> Option<f32> {
        match self {
            Edge::None => Some(0.5),
            Edge::Start => Some(0.0),
            Edge::End => Some(1.0),
            Edge::At(t) => Some(t),
            Edge::Stretch => None,
        }
    }

    pub fn pin(self) -> Pin {
        match self {
            Edge::None => Pin::None,
            Edge::Start => Pin::Start,
            Edge::End => Pin::End,
            Edge::At(t) => Pin::At(t),
            Edge::Stretch => Pin::Stretch,
        }
    }
}

impl ObjectLayout {
    /// Whether it changes nothing.
    pub fn is_empty(&self) -> bool {
        self.adjust() == Adjust::default() && self.text.is_none() && self.image.is_none()
    }

    pub fn adjust(&self) -> Adjust {
        Adjust {
            offset: self.offset,
            hidden: self.hidden,
            scale: self.scale,
            pin: [self.pin_x.pin(), self.pin_y.pin()],
        }
    }
}

impl LayerLayout {
    pub fn from_ron(text: &str) -> Result<LayerLayout, String> {
        let mut l: LayerLayout = ron::from_str(text).map_err(|e| e.to_string())?;
        if l.hash == 0 {
            l.hash = qwc_formats::flow::name_hash(&l.layer);
        }
        Ok(l)
    }

    pub fn to_ron(&self) -> String {
        let config = ron::ser::PrettyConfig::new().indentor("    ").escape_strings(false).new_line("\n");
        let mut text = ron::ser::to_string_pretty(self, config).unwrap_or_default();
        text.push('\n');
        text
    }

    /// The object `id`'s change, made when there is none.
    pub fn object_mut(&mut self, id: u32) -> &mut ObjectLayout {
        let i = match self.objects.iter().position(|o| o.id == id) {
            Some(i) => i,
            None => {
                self.objects.push(ObjectLayout { id, ..Default::default() });
                self.objects.len() - 1
            }
        };
        &mut self.objects[i]
    }

    pub fn object(&self, id: u32) -> Option<&ObjectLayout> {
        self.objects.iter().find(|o| o.id == id)
    }

    /// Without the objects that change nothing, in id order.
    pub fn tidy(&mut self) {
        self.objects.retain(|o| !o.is_empty());
        self.objects.sort_by_key(|o| o.id);
    }
}

/// The packs' layouts merged ([`merge`], the lowest pack's first) and their crops; read again when they change.
pub struct Layouts {
    /// The highest pack's folder, which the editor saves into: its layouts, its crops' file, and its PNGs in
    /// `images/`. None without a pack on (and then there are no layouts).
    pub pack: Option<PathBuf>,
    pack_files: Option<Folder>,
    merged: Vec<LayerLayout>,
    /// The lower packs' layouts, the lowest first.
    lower: Vec<Folder>,
    /// The crops, each pack's laid over the lower ones', and when each folder's file was written.
    crops: (Vec<Option<SystemTime>>, Crops),
    checked: Option<Instant>,
}

/// The highest pack's layouts' folder: `$QWC_SCREEN_PACK`, else the highest resource pack's `screens` that has one
/// (`qwc_packs`), else the highest pack's (made when the editor saves into it). None without a pack on: the screens
/// are drawn as the original draws them.
pub fn default_pack() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("QWC_SCREEN_PACK") {
        return Some(d.into());
    }
    let packs = qwc_packs::current();
    packs.screens().or_else(|| packs.packs.first().map(|p| p.dir.join("screens")))
}

/// The lower packs' layouts' folders under the highest pack's `pack` ([`default_pack`]), the lowest first; none when
/// `$QWC_SCREEN_PACK` names the pack.
pub fn lower_packs(pack: Option<&Path>) -> Vec<PathBuf> {
    if std::env::var_os("QWC_SCREEN_PACK").is_some() {
        return Vec::new();
    }
    qwc_packs::current().screens_dirs().into_iter().filter(|d| Some(d.as_path()) != pack).collect()
}

/// The error of saving without a pack on.
fn no_pack() -> std::io::Error {
    std::io::Error::other("no pack is on to save into: turn one on, or make one, in the Packs mode")
}

/// Whether a layout's picture is a texture pack's: a PNG of its `images/`.
pub fn is_pack_image(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".png")
}

/// A layer's changes, a lower pack's with a higher one's over them: an object the higher pack changes takes its
/// change whole, and its added sprites come after the lower one's (one of the same name in its place).
pub fn merge(lower: Option<&LayerLayout>, pack: Option<&LayerLayout>) -> LayerLayout {
    let (Some(s), Some(p)) = (lower, pack) else {
        return lower.or(pack).cloned().unwrap_or_default();
    };
    let mut l = s.clone();
    for o in &p.objects {
        *l.object_mut(o.id) = o.clone();
    }
    for a in &p.adds {
        match l.adds.iter_mut().find(|x| x.name == a.name) {
            Some(x) => *x = a.clone(),
            None => l.adds.push(a.clone()),
        }
    }
    l.tidy();
    l
}

/// `l` with its pack pictures named by their full paths in `images` (a lower pack's, so that they are found there
/// whichever pack's folder the pictures are looked for in).
fn with_images_in(l: &LayerLayout, images: &Path) -> LayerLayout {
    let full = |name: &str| images.join(name).to_string_lossy().into_owned();
    let mut l = l.clone();
    for o in &mut l.objects {
        if let Some(i) = o.image.as_mut().filter(|i| is_pack_image(i)) {
            *i = full(i);
        }
    }
    for a in &mut l.adds {
        if is_pack_image(&a.image) {
            a.image = full(&a.image);
        }
    }
    l
}

/// One folder's layer files, by file name: when each was written, and what it holds.
struct Folder {
    dir: PathBuf,
    files: HashMap<String, (Option<SystemTime>, LayerLayout)>,
}

impl Folder {
    fn read(&mut self) -> bool {
        let mut seen = Vec::new();
        let mut changed = false;
        for entry in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| !e.eq_ignore_ascii_case("ron")) {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_string) else { continue };
            if name.eq_ignore_ascii_case(crate::crops::FILE) {
                continue;
            }
            let time = entry.metadata().and_then(|m| m.modified()).ok();
            seen.push(name.clone());
            if self.files.get(&name).is_some_and(|(t, _)| *t == time) {
                continue;
            }
            let first = !self.files.contains_key(&name);
            match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| LayerLayout::from_ron(&t)) {
                Ok(l) => {
                    if !first {
                        eprintln!("screens: read {} again", path.display());
                    }
                    self.files.insert(name, (time, l));
                    changed = true;
                }
                Err(e) => eprintln!("{}: {e}", path.display()),
            }
        }
        let before = self.files.len();
        self.files.retain(|n, _| seen.contains(n));
        changed || self.files.len() != before
    }

    fn layer(&self, hash: u32) -> Option<&LayerLayout> {
        self.files.values().map(|(_, l)| l).find(|l| l.hash == hash)
    }

    /// Write a layer's changes to its file (`<layer>.ron`), or remove the file when it changes nothing; answers the
    /// file when it was written or removed.
    fn save(&mut self, l: &LayerLayout) -> std::io::Result<Option<PathBuf>> {
        let name = format!("{}.ron", file_stem(&l.layer));
        let path = self.dir.join(&name);
        if l.objects.is_empty() && l.adds.is_empty() {
            self.files.remove(&name);
            if !path.exists() {
                return Ok(None);
            }
            std::fs::remove_file(&path)?;
        } else {
            std::fs::create_dir_all(&self.dir)?;
            std::fs::write(&path, l.to_ron())?;
            let time = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            self.files.insert(name, (time, l.clone()));
        }
        Ok(Some(path))
    }
}

impl Default for Layouts {
    /// The packs' layouts from their default folders ([`default_pack`], [`lower_packs`]).
    fn default() -> Layouts {
        let pack = default_pack();
        let lower = lower_packs(pack.as_deref());
        Layouts::new(pack).with_lower(lower)
    }
}

impl Layouts {
    /// The layouts in the highest pack's folder `pack`, read now (none from a folder that is not there, nor without
    /// a pack).
    pub fn new(pack: Option<PathBuf>) -> Layouts {
        let pack_files = pack.clone().map(|dir| Folder { dir, files: HashMap::new() });
        let mut l = Layouts {
            pack,
            pack_files,
            lower: Vec::new(),
            merged: Vec::new(),
            crops: (Vec::new(), Crops::new()),
            checked: None,
        };
        l.read();
        l
    }

    /// These layouts with the lower packs' screens folders `dirs` (the lowest first) laid under the highest pack's.
    pub fn with_lower(mut self, dirs: Vec<PathBuf>) -> Layouts {
        self.lower = dirs.into_iter().map(|dir| Folder { dir, files: HashMap::new() }).collect();
        self.read();
        self
    }

    /// Read the folders again when it is time (twice a second) and a file came, went or changed; answers whether
    /// one did.
    pub fn refresh(&mut self) -> bool {
        if self.checked.is_some_and(|t| t.elapsed() < Duration::from_millis(500)) {
            return false;
        }
        self.read()
    }

    /// The folders, the lowest pack's first.
    fn folders(&self) -> impl Iterator<Item = &Folder> {
        self.lower.iter().chain(&self.pack_files)
    }

    fn read(&mut self) -> bool {
        self.checked = Some(Instant::now());
        let mut changed = false;
        for f in self.lower.iter_mut().chain(&mut self.pack_files) {
            changed |= f.read();
        }
        let crops_changed = self.read_crops();
        if changed {
            self.merge_all();
        }
        changed || crops_changed
    }

    /// Read the crops' files again when one came, went or changed; answers whether one did.
    fn read_crops(&mut self) -> bool {
        let time = |f: &Folder| std::fs::metadata(f.dir.join(crate::crops::FILE)).and_then(|m| m.modified()).ok();
        let times: Vec<_> = self.folders().map(time).collect();
        if times == self.crops.0 {
            return false;
        }
        // a higher pack's crop of a name stands in for a lower one's
        let crops = self.folders().flat_map(|f| crate::crops::read(&f.dir)).collect();
        self.crops = (times, crops);
        true
    }

    fn merge_all(&mut self) {
        let mut hashes: Vec<u32> = self.folders().flat_map(|f| f.files.values()).map(|(_, l)| l.hash).collect();
        hashes.sort();
        hashes.dedup();
        let top = |h| self.pack_files.as_ref().and_then(|f| f.layer(h));
        self.merged = hashes.into_iter().map(|h| merge(self.lower_layer(h).as_ref(), top(h))).collect();
    }

    /// The layer of hash `h`'s changes in the lower packs: each one's over the ones below it, their pictures by
    /// their full paths.
    fn lower_layer(&self, h: u32) -> Option<LayerLayout> {
        let mut l = None;
        for f in &self.lower {
            if let Some(own) = f.layer(h) {
                let own = with_images_in(own, &f.dir.join("images"));
                l = Some(merge(l.as_ref(), Some(&own)));
            }
        }
        l
    }

    /// Every layer's changes, the packs' merged.
    pub fn layers(&self) -> impl Iterator<Item = &LayerLayout> {
        self.merged.iter()
    }

    /// The crops the layouts' `crop:` pictures are cut by.
    pub fn crops(&self) -> &Crops {
        &self.crops.1
    }

    /// Write the crops to the highest pack's file: those the lower packs do not have as they are (the file removed
    /// when that leaves none); answers it.
    pub fn save_crops(&mut self, crops: &Crops) -> std::io::Result<PathBuf> {
        let pack = self.pack.clone().ok_or_else(no_pack)?;
        let lower: Crops = self.lower.iter().flat_map(|f| crate::crops::read(&f.dir)).collect();
        let own: Crops =
            crops.iter().filter(|(n, c)| lower.get(*n) != Some(c)).map(|(n, c)| (n.clone(), c.clone())).collect();
        let path = crate::crops::write(&pack, &own)?;
        self.crops.0.clear();
        self.read_crops();
        Ok(path)
    }

    /// The layer of hash `hash`'s changes, when it has any.
    pub fn layer(&self, hash: u32) -> Option<&LayerLayout> {
        self.layers().find(|l| l.hash == hash)
    }

    /// The front end's adjustments, by (layer hash, object id).
    pub fn adjustments(&self) -> HashMap<(u32, u32), Adjust> {
        let mut out = HashMap::new();
        for l in self.layers() {
            for o in l.objects.iter().filter(|o| !o.is_empty()) {
                out.insert((l.hash, o.id), o.adjust());
            }
        }
        out
    }

    /// The replaced texts, by (layer hash, object id), in UTF-16 as the front end holds them.
    pub fn texts(&self) -> HashMap<(u32, u32), Vec<u16>> {
        let mut out = HashMap::new();
        for l in self.layers() {
            for o in &l.objects {
                if let Some(t) = &o.text {
                    out.insert((l.hash, o.id), t.encode_utf16().collect());
                }
            }
        }
        out
    }

    /// The folder the highest pack's PNGs are in: `images/` in its folder (an empty path without a pack, when there
    /// are no layouts to show any).
    pub fn images_dir(&self) -> PathBuf {
        self.pack.as_ref().map(|p| p.join("images")).unwrap_or_default()
    }

    /// Give `fe` everything these files change — the adjustments, the replaced texts and pictures, and the added
    /// sprites, their images minted in `disc` — and bring the live layers' added sprites in line.
    pub fn apply(&self, fe: &mut FrontEnd, disc: &crate::Disc) {
        apply_layers(self.layers(), &self.images_dir(), self.crops(), fe, disc);
    }

    /// Write a layer's changes to the highest pack's file: what the lower packs do not already have as it is, and
    /// none of their pictures (a file that would change nothing is removed); answers the file written or removed.
    pub fn save(&mut self, layout: &LayerLayout) -> std::io::Result<Option<PathBuf>> {
        let below = self.lower_layer(layout.hash).unwrap_or_default();
        let top = self.pack_files.as_mut().ok_or_else(no_pack)?;
        let mut l = layout.clone();
        l.tidy();
        l.objects.retain(|o| below.object(o.id) != Some(o));
        l.adds.retain(|a| !below.adds.contains(a));
        // a lower pack's pictures (by their full paths) stay that pack's: the highest pack keeps only its own
        let lower = |name: &str| Path::new(name).is_absolute();
        let lower_adds: Vec<u32> = l.adds.iter().filter(|a| lower(&a.image)).map(AddLayout::id).collect();
        l.objects.retain(|o| !o.image.as_deref().is_some_and(lower) && !lower_adds.contains(&o.id));
        l.adds.retain(|a| !lower(&a.image));
        let file = top.save(&l)?;
        self.merge_all();
        Ok(file)
    }
}

/// [`Layouts::apply`] for any set of layer layouts (the editor applies its unsaved edits with it): `fe` takes
/// their adjustments, replaced texts and pictures, and added sprites, the images minted in `disc`
/// ([`crate::Disc::add_image`], the crops cut by `crops`), and the live layers' added sprites are brought in line.
/// A picture that is not there (a texture pack's PNG without the pack, a crop not in `crops`) is no change: the
/// object shows its own, and a sprite added to show it is not added.
pub fn apply_layers<'a>(
    layers: impl Iterator<Item = &'a LayerLayout>,
    images: &std::path::Path,
    crops: &Crops,
    fe: &mut FrontEnd,
    disc: &crate::Disc,
) {
    disc.set_crops(crops);
    let have = |name: &str| match crate::crops::name(name) {
        Some(c) => crops.contains_key(c),
        None => !is_pack_image(name) || images.join(name).is_file(),
    };
    fe.adjust.clear();
    fe.text_over.clear();
    fe.image_over.clear();
    fe.additions.clear();
    for l in layers {
        for o in l.objects.iter().filter(|o| !o.is_empty()) {
            let key = (l.hash, o.id);
            let a = o.adjust();
            if a != Adjust::default() {
                fe.adjust.insert(key, a);
            }
            if let Some(t) = &o.text {
                fe.text_over.insert(key, t.encode_utf16().collect());
            }
            if let Some(img) = o.image.as_deref().filter(|i| have(i)) {
                fe.image_over.insert(key, disc.add_image(&image_spec(images, img)));
            }
        }
        if !l.adds.is_empty() {
            let adds = (l.adds.iter().filter(|a| have(&a.image)))
                .map(|a| qwc_feck::frontend::SpriteAdd {
                    id: a.id(),
                    image: disc.add_image(&image_spec(images, &a.image)),
                    pos: a.pos,
                    size: a.size,
                    parent: a.parent,
                    order: a.order,
                    animate_like: a.animate,
                })
                .collect();
            fe.additions.insert(l.hash, adds);
        }
    }
    fe.sync_additions();
}

/// An image name of a layout as [`crate::Disc::add_image`] takes it: a `.png` under `images`, a `res:` alias or a
/// crop as it is, else a path of the data as it is spelt there.
pub fn image_spec(images: &std::path::Path, name: &str) -> String {
    if name.starts_with("res:") || crate::crops::name(name).is_some() {
        name.to_string()
    } else if is_pack_image(name) {
        images.join(name).display().to_string()
    } else {
        qwc_assets::vfs::normalize(name)
    }
}

/// A layer's name as a file name: its letters, digits, `_` and `-`.
fn file_stem(layer: &str) -> String {
    let s: String = layer.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    if s.is_empty() {
        "layer".into()
    } else {
        s
    }
}

/// The stem `Screens.lst` gives a layer file (`Opening_TitleScreen.vso` -> `Opening_TitleScreen`).
pub fn layer_stem(file: &str) -> &str {
    let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
    name.rsplit_once('.').map_or(name, |(s, _)| s)
}

fn is_zero2(v: &[f32; 2]) -> bool {
    *v == [0.0, 0.0]
}

fn is_false(v: &bool) -> bool {
    !*v
}

fn is_one(v: &f32) -> bool {
    *v == 1.0
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

fn is_zero_size(v: &[u16; 2]) -> bool {
    *v == [0, 0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layout_reads_back_and_saves_only_what_changes() {
        let mut l = LayerLayout { layer: "Opening_TitleScreen".into(), ..Default::default() };
        l.object_mut(7).offset = [3.0, -2.5];
        l.object_mut(9).hidden = true;
        l.object_mut(4);
        l.object_mut(12).scale = 1.5;
        l.object_mut(12).pin_x = Edge::End;
        l.object_mut(15).pin_x = Edge::At(0.25);
        l.object_mut(15).pin_y = Edge::At(0.75);
        l.object_mut(20).text = Some("REMASTERED \"QUIDDITCH\"\nline two".into());
        l.object_mut(21).image = Some("base_right.png".into());
        let left = AddLayout {
            name: "Split left".into(),
            image: "base_left.png".into(),
            pos: [10.0, 0.0],
            ..Default::default()
        };
        l.adds.push(left.clone());
        let back = LayerLayout::from_ron(&l.to_ron()).unwrap();
        // the hash comes from the name when the file has none
        assert_eq!(back.hash, qwc_formats::flow::name_hash("Opening_TitleScreen"));
        assert_eq!(back.objects.len(), 7);
        // a replaced picture and an added sprite read back
        assert_eq!(back.object(21).and_then(|o| o.image.clone()).as_deref(), Some("base_right.png"));
        assert_eq!(back.adds, [left]);
        // an anchor point reads back and reaches the adjustment
        assert_eq!(back.object(15).map(|o| (o.pin_x, o.pin_y)), Some((Edge::At(0.25), Edge::At(0.75))));
        // a replaced text reads back, quotes, line break and all, and reaches the front end in UTF-16
        assert_eq!(back.object(20).and_then(|o| o.text.clone()).as_deref(), Some("REMASTERED \"QUIDDITCH\"\nline two"));
        // an object as it was (scale 1, no pins) is written without them
        assert!(!l.to_ron().contains("scale: 1.0"), "{}", l.to_ron());
        assert_eq!(back.object(12).map(|o| (o.scale, o.pin_x, o.pin_y)), Some((1.5, Edge::End, Edge::None)));
        let mut t = back.clone();
        t.tidy();
        assert_eq!(t.objects.iter().map(|o| o.id).collect::<Vec<_>>(), [7, 9, 12, 15, 20, 21]);
        let dir = std::env::temp_dir().join(format!("qwc-screens-test-{}", std::process::id()));
        let pack = dir.join("pack");
        let mut ls = Layouts::new(Some(pack.clone()));
        let path = ls.save(&back).unwrap().unwrap();
        // every change goes to the pack's file, the PNGs' objects and the rest
        assert_eq!(path, pack.join("Opening_TitleScreen.ron"));
        let saved = LayerLayout::from_ron(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(saved.object(21).is_some() && saved.adds.len() == 1 && saved.object(7).is_some());
        let fe_adjust = ls.adjustments();
        assert_eq!(fe_adjust[&(back.hash, 7)], Adjust { offset: [3.0, -2.5], ..Default::default() });
        assert_eq!(fe_adjust[&(back.hash, 12)].pin, [Pin::End, Pin::None]);
        assert_eq!(fe_adjust[&(back.hash, 15)].pin, [Pin::At(0.25), Pin::At(0.75)]);
        let texts = ls.texts();
        assert_eq!(texts[&(back.hash, 20)], "REMASTERED \"QUIDDITCH\"\nline two".encode_utf16().collect::<Vec<u16>>());
        assert!(!texts.contains_key(&(back.hash, 7)));
        assert!(fe_adjust[&(back.hash, 9)].hidden);
        assert!(!fe_adjust.contains_key(&(back.hash, 4)));
        // read by another reader of the folder, merged again; an empty layout removes the file
        let again = Layouts::new(Some(pack.clone()));
        assert_eq!(again.layer(back.hash).map(|l| (l.objects.len(), l.adds.len())), Some((6, 1)));
        ls.save(&LayerLayout { layer: back.layer.clone(), hash: back.hash, ..Default::default() }).unwrap();
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn without_a_pack_there_are_no_layouts_and_nothing_saves() {
        let mut ls = Layouts::new(None);
        assert_eq!((ls.layers().count(), ls.crops().len()), (0, 0));
        let mut l = LayerLayout { layer: "L".into(), hash: 1, ..Default::default() };
        l.object_mut(1).hidden = true;
        assert!(ls.save(&l).is_err() && ls.save_crops(&Crops::new()).is_err());
    }

    #[test]
    fn a_higher_pack_goes_over_a_lower_one() {
        let mut low = LayerLayout { layer: "L".into(), hash: 1, ..Default::default() };
        low.object_mut(5).pin_x = Edge::Start;
        low.object_mut(6).offset = [1.0, 2.0];
        low.adds.push(AddLayout { name: "cut".into(), image: crate::crops::spec("half"), ..Default::default() });
        let mut high = LayerLayout { layer: "L".into(), hash: 1, ..Default::default() };
        high.object_mut(6).image = Some("oval.png".into());
        high.object_mut(6).scale = 0.5;
        let card = AddLayout { name: "card".into(), image: "card.png".into(), ..Default::default() };
        high.object_mut(card.id()).offset = [3.0, 0.0];
        high.adds.push(card.clone());
        let m = merge(Some(&low), Some(&high));
        // the higher pack's change to an object replaces the lower one's whole
        assert_eq!(m.object(6).map(|o| (o.offset, o.scale)), Some(([0.0; 2], 0.5)));
        assert_eq!(m.object(5).map(|o| o.pin_x), Some(Edge::Start));
        assert_eq!(m.adds.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["cut", "card"]);
        assert_eq!(merge(Some(&low), None), low);
    }

    #[test]
    fn lower_packs_lie_under_the_highest_and_it_saves_only_its_own() {
        let root = std::env::temp_dir().join(format!("qwc-screens-layers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (lowest, low, high) = (root.join("lowest"), root.join("low"), root.join("high"));
        let write = |dir: &Path, l: &LayerLayout| {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("L.ron"), l.to_ron()).unwrap();
        };
        let mut l = LayerLayout { layer: "L".into(), hash: 1, ..Default::default() };
        l.object_mut(1).offset = [1.0, 0.0];
        l.object_mut(2).offset = [2.0, 0.0];
        write(&lowest, &l);
        let mut l = LayerLayout { layer: "L".into(), hash: 1, ..Default::default() };
        l.object_mut(2).image = Some("low.png".into());
        l.object_mut(3).hidden = true;
        write(&low, &l);
        let mut l = LayerLayout { layer: "L".into(), hash: 1, ..Default::default() };
        l.object_mut(3).image = Some("high.png".into());
        write(&high, &l);
        let mut layouts = Layouts::new(Some(high.clone())).with_lower(vec![lowest.clone(), low.clone()]);
        let mut m = layouts.layer(1).unwrap().clone();
        // the lowest pack's object 1, the low pack's object 2 with its picture found in its own folder, the high
        // pack's 3
        assert_eq!(m.object(1).unwrap().offset, [1.0, 0.0]);
        let low_png = low.join("images").join("low.png").to_string_lossy().into_owned();
        assert_eq!(m.object(2).unwrap().image.as_deref(), Some(low_png.as_str()));
        assert_eq!(m.object(3).unwrap().image.as_deref(), Some("high.png"));
        assert!(!m.object(3).unwrap().hidden, "the high pack's change of it whole");
        // saving the merged layer with a change: the high pack's file takes the change and its own object, not
        // what the lower packs already have nor the low pack's picture
        m.object_mut(4).offset = [0.0, 5.0];
        layouts.save(&m).unwrap();
        let saved = LayerLayout::from_ron(&std::fs::read_to_string(high.join("L.ron")).unwrap()).unwrap();
        assert_eq!(saved.objects.iter().map(|o| o.id).collect::<Vec<_>>(), [3, 4], "{saved:?}");
        assert_eq!(layouts.layer(1).map(|l| l.objects.len()), Some(4));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_crops_file_is_not_a_layer_and_a_higher_packs_crops_go_over_a_lower_ones() {
        let root = std::env::temp_dir().join(format!("qwc-screens-crops-{}", std::process::id()));
        let (low, high) = (root.join("low"), root.join("high"));
        let crop = |texture: &str| crate::crops::Crop { texture: texture.into(), ..Default::default() };
        let mut crops = Crops::new();
        crops.insert("half".into(), crop("a.fsh"));
        crops.insert("other".into(), crop("b.fsh"));
        crate::crops::write(&low, &crops).unwrap();
        crate::crops::write(&high, &Crops::from([("half".to_string(), crop("c.fsh"))])).unwrap();
        let mut l = Layouts::new(Some(high.clone())).with_lower(vec![low.clone()]);
        assert_eq!(l.layers().count(), 0);
        assert_eq!(l.crops().get("half"), Some(&crop("c.fsh")));
        assert_eq!(l.crops().get("other"), Some(&crop("b.fsh")));
        // saving keeps in the high pack only the crops the low one lacks as they are
        let mut all = l.crops().clone();
        all.insert("new".into(), crop("d.fsh"));
        all.insert("half".into(), crop("a.fsh"));
        l.save_crops(&all).unwrap();
        assert_eq!(crate::crops::read(&high).into_keys().collect::<Vec<_>>(), ["new"]);
        assert_eq!(l.crops(), &all);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_layer_file_is_named_by_its_stem() {
        assert_eq!(layer_stem("Opening_TitleScreen.vso"), "Opening_TitleScreen");
        assert_eq!(layer_stem("frontend/x/StadiumOrbit.VSO"), "StadiumOrbit");
        assert_eq!(file_stem("a b/c"), "a_b_c");
    }
}
