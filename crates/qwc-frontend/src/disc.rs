//! The textures and fonts the front end's scripts see ([`Disc`]): the disc's, decoded on demand through the resource
//! index and the load sets held, or the remastered PNGs standing in for them, and the bitmap fonts with the vector
//! fonts drawing them.

use std::cell::{Cell, Ref, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use qwc_assets::frontend::Resources;
use qwc_assets::remaster::Remastered;
use qwc_assets::texture::Texture;
use qwc_assets::Vfs;
use qwc_feck::draw::{Assets, Tex};
use qwc_feck::layout::Metrics;
use qwc_feck::wipe::WipeData;
use qwc_formats::ffn::Ffn;
use qwc_formats::flow::{name_hash, LoadSet};

/// A texture as the front end has it.
pub struct Shown {
    /// What is drawn: the disc's texture, or the remastered PNG with its mip levels.
    pub pixels: Texture,
    pub remastered: bool,
    /// The size the scripts see: always the disc texture's.
    pub size: [u16; 2],
    /// A remastered PNG's width over its height: it keeps that shape on screen.
    pub shape: Option<f32>,
}

/// An original font drawn with a vector font (`qwc_text`): the layout's metrics are the vector font's, in units for
/// the window ([`Disc::set_screen`]).
pub struct VectorFont(pub qwc_text::TextFont);

impl Metrics for VectorFont {
    fn advance(&self, c: u16) -> i32 {
        self.0.advance(c)
    }
    fn kern(&self, c: u16, prev: u16) -> i32 {
        self.0.kern(c, prev)
    }
    fn line_height(&self) -> u16 {
        self.0.line_height
    }
}

/// Textures decoded on demand, and the fonts.
pub struct Disc<'a> {
    pub vfs: &'a Vfs,
    resources: Resources,
    /// The package folders the live screens' load sets hold ([`Disc::set_loaded`]), and whether there is a
    /// load-set table to hold them.
    loaded: RefCell<HashSet<String>>,
    has_sets: Cell<bool>,
    /// Those of them in the `stadium` render group, sorted.
    stadium_group: RefCell<Vec<String>>,
    textures: RefCell<HashMap<String, Option<Shown>>>,
    /// Not the original's: the minted image resources ([`qwc_feck::draw::IMAGE_SCRIPT`]) by id, each a PNG file's
    /// path, a crop (`crop:NAME`) or a texture path of the data ([`Disc::add_image`]).
    images: RefCell<HashMap<u32, String>>,
    /// The crops the `crop:` images are cut by ([`Disc::set_crops`]).
    crops: RefCell<crate::crops::Crops>,
    /// Every texture dropped ([`Disc::drop_texture`]), in order, for those holding copies of them ([`Disc::dropped`]).
    dropped: RefCell<Vec<String>>,
    fonts: HashMap<u32, Ffn<'a>>,
    /// The fonts drawn with vector fonts, by the same ids.
    vector: HashMap<u32, VectorFont>,
    remastered: Option<Remastered>,
}

impl Disc<'static> {
    /// A disc the front end's natives share ([`qwc_feck::FrontEnd::assets`]: `GetCalculatedExtents` and the text
    /// layout need its textures and fonts). It lives as long as the program: the fonts' bytes are kept for good.
    pub fn shared(
        vfs: &'static Vfs,
        resources: Resources,
        font_bytes: HashMap<u32, Vec<u8>>,
        remastered: Option<Remastered>,
        font_list: Option<qwc_text::FontList>,
    ) -> Rc<Disc<'static>> {
        Rc::new(Disc::new(vfs, resources, Box::leak(Box::new(font_bytes)), remastered, font_list))
    }
}

impl<'a> Disc<'a> {
    pub fn new(
        vfs: &'a Vfs,
        resources: Resources,
        font_bytes: &'a HashMap<u32, Vec<u8>>,
        remastered: Option<Remastered>,
        font_list: Option<qwc_text::FontList>,
    ) -> Disc<'a> {
        let fonts: HashMap<u32, Ffn> =
            font_bytes.iter().filter_map(|(&id, d)| Some((id, Ffn::parse(d).ok()?))).collect();
        let mut vector = HashMap::new();
        if let Some(list) = &font_list {
            let mut faces = qwc_text::Faces::default();
            let mut missing = std::collections::BTreeSet::new();
            for (&id, ffn) in &fonts {
                let Some(path) = resources.path(id, 0) else { continue };
                match qwc_text::TextFont::new(list, &mut faces, path, Metrics::line_height(ffn)) {
                    Some(Ok(f)) => {
                        vector.insert(id, VectorFont(f));
                    }
                    Some(Err(e)) => eprintln!("{e}: the disc's font is drawn"),
                    None => {
                        missing.insert(qwc_text::Style::of(path).map_or(path.to_string(), |s| s.family));
                    }
                }
            }
            eprintln!("{} of {} fonts drawn with vector fonts", vector.len(), fonts.len());
            if !missing.is_empty() {
                eprintln!("no vector font for {missing:?}: the disc's are drawn");
            }
        }
        Disc {
            vfs,
            resources,
            loaded: Default::default(),
            has_sets: Default::default(),
            stadium_group: Default::default(),
            textures: Default::default(),
            images: Default::default(),
            crops: Default::default(),
            dropped: Default::default(),
            fonts,
            vector,
            remastered,
        }
    }

    /// Whether the front end's stadium (the sky and the stadium the painter draws, `stadium/sum_fe/gfx`) is loaded:
    /// its package is in a load set held now (`LOADSET_STADIUM_GFX_HOGWARTS_SUMMER_FRONTEND`). Before the title
    /// none is (the intro's screen turns the stadium's render on before its movie, which drew the sky). Always,
    /// without the load-set table.
    pub fn stadium_loaded(&self) -> bool {
        self.package_loaded("stadium/sum_fe/gfx")
    }

    /// Whether the package `folder` (`audio/music/fe`) is in a load set held now. Always, without the load-set
    /// table.
    pub fn package_loaded(&self, folder: &str) -> bool {
        !self.has_sets.get() || self.loaded.borrow().contains(folder)
    }

    /// Hold these package folders as loaded (one layer drawn on its own holds every load set's first package).
    pub fn hold(&self, folders: impl IntoIterator<Item = String>) {
        self.loaded.borrow_mut().extend(folders);
    }

    /// The hoops' model: the hoop renderer's `singlehoop` (`0x4a197e`) in the hoop package loaded with the stadium
    /// (the front end's set holds `Stadium\HoopNorm\gfx`); that one without the load-set table.
    pub fn hoop_model(&self) -> String {
        let loaded = self.loaded.borrow();
        let package = loaded.iter().filter(|p| p.starts_with("stadium/hoop") && p.ends_with("/gfx")).min();
        format!("{}/singlehoop.fpm", package.map_or("stadium/hoopnorm/gfx", |p| p.as_str()))
    }

    /// The `stadium` render group's packages held now: the stadium's flags find their script and textures there
    /// (`0x4c7760`, `qwc_render::flags`). Without the load-set table, the front end's (its stadium set's and
    /// `STADIUM_COMMON`'s).
    pub fn stadium_group(&self) -> Vec<String> {
        if !self.has_sets.get() {
            let fe = ["stadium/common/gfx", "stadium/hoopnorm/gfx", "stadium/sum_fe/game", "stadium/sum_fe/gfx"];
            return fe.map(String::from).to_vec();
        }
        self.stadium_group.borrow().clone()
    }

    /// The resource index (`_ResourceData` ids to files).
    pub fn resources(&self) -> &Resources {
        &self.resources
    }

    /// The window the text is laid out for: the vector fonts' metrics are in units for it.
    pub fn set_screen(&self, screen: (u32, u32)) {
        for f in self.vector.values() {
            f.0.set_screen(screen);
        }
    }

    /// Font `id` drawn with a vector font, when one draws it.
    pub fn vector_font(&self, id: u32) -> Option<&qwc_text::TextFont> {
        self.vector.get(&id).map(|v| &v.0)
    }

    /// Font `id`'s bitmap font (the disc's).
    pub fn bitmap_font(&self, id: u32) -> Option<&Ffn<'a>> {
        self.fonts.get(&id)
    }

    /// The texture at `path` as [`Disc::texture`] loaded it.
    pub fn shown(&self, path: &str) -> Option<Ref<'_, Shown>> {
        Ref::filter_map(self.textures.borrow(), |m| m.get(path).and_then(Option::as_ref)).ok()
    }

    /// Decode the remastered textures of the packages loaded that are not yet, together (the game loads a screen's
    /// packages when it comes; a PNG many times the disc texture's size takes a while).
    fn preload(&self) {
        let Some(r) = &self.remastered else { return };
        let want: Vec<&str> = {
            let (loaded, cache) = (self.loaded.borrow(), self.textures.borrow());
            r.textures()
                .filter(|t| !cache.contains_key(*t) && t.rsplit_once('/').is_some_and(|(dir, _)| loaded.contains(dir)))
                .collect()
        };
        if want.is_empty() {
            return;
        }
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(want.len());
        let vfs = self.vfs;
        let done: Vec<(String, Option<Shown>)> = std::thread::scope(|s| {
            let jobs: Vec<_> = want
                .chunks(want.len().div_ceil(threads))
                .map(|c| {
                    s.spawn(move || {
                        c.iter().map(|&p| (p.to_string(), load_texture(vfs, Some(r), p))).collect::<Vec<_>>()
                    })
                })
                .collect();
            jobs.into_iter().flat_map(|j| j.join().unwrap_or_default()).collect::<Vec<_>>()
        });
        self.textures.borrow_mut().extend(done);
    }

    /// The packages loaded now: those of every screen the manager holds (current, previous and next), by their
    /// load sets ([`load_set`]).
    pub fn set_loaded(&self, m: &qwc_feck::screens::Manager, sets: &[LoadSet]) {
        self.has_sets.set(!sets.is_empty());
        let (mut loaded, mut stadium) = (self.loaded.borrow_mut(), self.stadium_group.borrow_mut());
        loaded.clear();
        stadium.clear();
        for s in m.screens.iter().flatten() {
            for set in s.load_sets.iter().filter_map(|&v| load_set(sets, v)) {
                loaded.extend(set.entries.iter().map(|(_, p)| package_folder(p)));
                let group = set.entries.iter().filter(|(g, _)| g.eq_ignore_ascii_case("stadium"));
                stadium.extend(group.map(|(_, p)| package_folder(p)));
            }
        }
        stadium.sort();
        stadium.dedup();
        drop((loaded, stadium));
        self.preload();
    }

    /// Mint an image resource for `spec` — a PNG file's path (ending `.png`, read from the file system; frame `k`
    /// of an animation is its `<base>.<k>.png` where that file exists, else the base), a crop (`crop:NAME`, cut by
    /// [`Disc::set_crops`]), a texture path of the data, or `res:<script>:<id>` (two hex words), another record's
    /// frames as [`Resources::path_in_script`] finds them — answering its id, stable for a spec. Sprites of [`qwc_feck::draw::IMAGE_SCRIPT`] show them
    /// ([`qwc_feck::FrontEnd::image_over`] and the added sprites).
    pub fn add_image(&self, spec: &str) -> u32 {
        let id = name_hash(spec);
        self.images.borrow_mut().insert(id, spec.to_string());
        id
    }

    /// The `res:` spec for record `id` of layer script `script`: an alias of its frames, for [`Disc::add_image`]
    /// (what a duplicate of an animated sprite shows).
    pub fn resource_spec(script: u32, id: u32) -> String {
        format!("res:{script:08x}:{id:08x}")
    }

    /// Forget the decoded texture at `path` (a [`Disc::texture`] path or an image spec), so the next draw reads the
    /// file again: for an image file written over, or a crop changed.
    pub fn drop_texture(&self, path: &str) {
        self.textures.borrow_mut().remove(path);
        self.dropped.borrow_mut().push(path.to_string());
    }

    /// The textures dropped since `seen` of them were ([`Disc::drop_texture`]), and how many have been: whoever keeps
    /// copies (the GPU's, the editor's) drops those too.
    pub fn dropped(&self, seen: usize) -> (Vec<String>, usize) {
        let d = self.dropped.borrow();
        (d.get(seen..).unwrap_or_default().to_vec(), d.len())
    }

    /// The crops the `crop:` images are cut by: those whose recipe changed are cut again.
    pub fn set_crops(&self, crops: &crate::crops::Crops) {
        let changed: Vec<String> = {
            let old = self.crops.borrow();
            let names = old.keys().chain(crops.keys());
            names.filter(|n| old.get(*n) != crops.get(*n)).map(|n| crate::crops::spec(n)).collect()
        };
        if changed.is_empty() {
            return;
        }
        *self.crops.borrow_mut() = crops.clone();
        for spec in changed {
            self.drop_texture(&spec);
        }
    }

    /// The texture of the data at `path` (or the remastered PNG standing in for it), decoded on first use, and its
    /// size: for a tool showing a texture no sprite does ([`Disc::shown`] then has it).
    pub fn texture_at(&self, path: &str) -> Option<[u16; 2]> {
        let path = qwc_assets::vfs::normalize(path);
        let mut cache = self.textures.borrow_mut();
        let t = cache.entry(path.clone()).or_insert_with(|| load_texture(self.vfs, self.remastered.as_ref(), &path));
        t.as_ref().map(|t| t.size)
    }

    /// The file sprite texture `tex` shows (through the load sets held) and the size the scripts see, decoding it on
    /// first use ([`Disc::shown`] then has it).
    pub fn texture(&self, tex: Tex) -> Option<(String, [u16; 2])> {
        if tex.script == qwc_feck::draw::IMAGE_SCRIPT {
            let spec = self.images.borrow().get(&tex.resource).cloned()?;
            let spec = match parse_resource_spec(&spec) {
                // an alias of another record: its frames are the record's, through the load sets held
                Some((script, id)) => {
                    let loaded = self.loaded.borrow();
                    let path = self.resources.path_in_script(script, id, tex.frame, &|f| loaded.contains(f))?;
                    qwc_assets::vfs::normalize(path)
                }
                None => frame_file(&spec, tex.frame, &|p| std::path::Path::new(p).is_file()),
            };
            if let Some(name) = crate::crops::name(&spec) {
                let crop = self.crops.borrow().get(name).cloned();
                let mut cache = self.textures.borrow_mut();
                let t = cache.entry(spec.clone()).or_insert_with(|| self.cut(name, crop.as_ref()?));
                return t.as_ref().map(|t| (spec, t.size));
            }
            let mut cache = self.textures.borrow_mut();
            let t = cache.entry(spec.clone()).or_insert_with(|| load_image(self.vfs, self.remastered.as_ref(), &spec));
            return t.as_ref().map(|t| (spec, t.size));
        }
        let loaded = self.loaded.borrow();
        let path = self.resources.path_in_script(tex.script, tex.resource, tex.frame, &|f| loaded.contains(f))?;
        // one spelling per file: the disc's paths come in mixed case
        let path = qwc_assets::vfs::normalize(path);
        let mut cache = self.textures.borrow_mut();
        let t = cache.entry(path.clone()).or_insert_with(|| load_texture(self.vfs, self.remastered.as_ref(), &path));
        t.as_ref().map(|t| (path, t.size))
    }
}

impl Disc<'_> {
    /// Crop `name`'s picture: its texture (the disc's, or the texture pack's PNG standing in for it, the polygon
    /// scaled to that) cut, at the disc texture's size as the scripts see it.
    fn cut(&self, name: &str, crop: &crate::crops::Crop) -> Option<Shown> {
        let path = qwc_assets::vfs::normalize(&crop.texture);
        let Some(src) = load_texture(self.vfs, self.remastered.as_ref(), &path) else {
            eprintln!("crop {name}: no texture {path} on the disc; nothing is drawn for it");
            return None;
        };
        let pixels = crop.apply(&src.pixels, src.size);
        Some(Shown { pixels, remastered: true, ..src })
    }
}

/// A `res:<script>:<id>` image spec's two words, when the spec is one.
fn parse_resource_spec(spec: &str) -> Option<(u32, u32)> {
    let rest = spec.strip_prefix("res:")?;
    let (s, id) = rest.split_once(':')?;
    Some((u32::from_str_radix(s, 16).ok()?, u32::from_str_radix(id, 16).ok()?))
}

/// Frame `frame` of the PNG at `spec`: its `<base>.<frame>.png` where `exists` says there is one, else the base
/// file (frame 0 always; a single PNG stands in for every frame). Not a PNG, the spec as it is.
fn frame_file(spec: &str, frame: u8, exists: &dyn Fn(&str) -> bool) -> String {
    if frame == 0 {
        return spec.to_string();
    }
    let Some(base) = spec.to_ascii_lowercase().ends_with(".png").then(|| &spec[..spec.len() - 4]) else {
        return spec.to_string();
    };
    let framed = format!("{base}.{frame}.png");
    if exists(&framed) {
        framed
    } else {
        spec.to_string()
    }
}

/// A minted image: a PNG file at its own size and proportions (a sprite showing it draws it as it is), or a
/// texture of the data as [`load_texture`] has it.
fn load_image(vfs: &Vfs, remastered: Option<&Remastered>, spec: &str) -> Option<Shown> {
    if !spec.to_ascii_lowercase().ends_with(".png") {
        return load_texture(vfs, remastered, spec);
    }
    match qwc_assets::remaster::load_png(std::path::Path::new(spec)) {
        Ok(p) => {
            let size = [p.width as u16, p.height as u16];
            Some(Shown { pixels: p, remastered: true, size, shape: None })
        }
        Err(e) => {
            eprintln!("{e}: nothing is drawn for it");
            None
        }
    }
}

/// The texture at `path`: the disc's, or the remastered PNG standing in for it (sized as the disc's).
fn load_texture(vfs: &Vfs, remastered: Option<&Remastered>, path: &str) -> Option<Shown> {
    let disc = qwc_assets::texture::load(vfs, path).ok()?;
    let size = [disc.width as u16, disc.height as u16];
    if let Some(png) = remastered.and_then(|r| r.png(path)) {
        match qwc_assets::remaster::load_png(png) {
            Ok(p) => {
                let (was, is) = (disc.width as f32 / disc.height as f32, p.width as f32 / p.height as f32);
                // drawn in its own proportions, whether they are the disc texture's or not
                let shape = Some(is);
                eprintln!(
                    "{path}: remastered {} ({}x{} for {}x{}{})",
                    png.display(),
                    p.width,
                    p.height,
                    disc.width,
                    disc.height,
                    if (is / was - 1.0).abs() > 0.01 { ", other proportions" } else { "" }
                );
                return Some(Shown { pixels: p, remastered: true, size, shape });
            }
            Err(e) => eprintln!("{e}: the disc's texture is drawn"),
        }
    }
    Some(Shown { pixels: disc, remastered: false, size, shape: None })
}

impl Assets for Disc<'_> {
    fn texture(&self, tex: Tex) -> Option<[u16; 2]> {
        Disc::texture(self, tex).map(|t| t.1)
    }
    fn font(&self, font: u32) -> Option<&dyn Metrics> {
        match self.vector.get(&font) {
            Some(v) => Some(v as &dyn Metrics),
            None => self.fonts.get(&font).map(|f| f as &dyn Metrics),
        }
    }
    fn refresh_choices(&self, run: &mut dyn FnMut(usize, u32) -> Option<u32>) {
        self.resources.refresh_choices(run);
    }
}

/// A quad's corners `p` (bottom left, bottom right, top left, top right, in the 512 x 512 screen) narrowed about its
/// centre along one of its sides, so that a picture `shape` wide per unit of height keeps that shape on a `screen`
/// of pixels (the 512 x 512 screen is stretched over it): a remastered texture's own proportions ([`Shown::shape`]).
pub fn keep_shape(p: [[f32; 2]; 4], shape: f32, screen: (u32, u32)) -> [[f32; 2]; 4] {
    let (sx, sy) = (screen.0 as f32 / 512.0, screen.1 as f32 / 512.0);
    let [bl, br, tl, _] = p;
    let u = [br[0] - bl[0], br[1] - bl[1]];
    let v = [tl[0] - bl[0], tl[1] - bl[1]];
    let on_screen = |d: [f32; 2]| ((d[0] * sx).powi(2) + (d[1] * sy).powi(2)).sqrt();
    let (lu, lv) = (on_screen(u), on_screen(v));
    if lu <= 0.0 || lv <= 0.0 {
        return p;
    }
    let quad = lu / lv;
    let (ku, kv) = if shape < quad { (shape / quad, 1.0) } else { (1.0, quad / shape) };
    let centre = [0, 1].map(|k| p.iter().map(|c| c[k]).sum::<f32>() / 4.0);
    [(-0.5, -0.5), (0.5, -0.5), (-0.5, 0.5), (0.5, 0.5)]
        .map(|(su, sv)| [0, 1].map(|k| centre[k] + su * u[k] * ku + sv * v[k] * kv))
}

/// A screen's load set: a slot (an `@Key`'s DataStore value) or, **[I]** for the fixed sets the port keeps as
/// `Screens.lst` names, a name hash.
pub fn load_set(sets: &[LoadSet], v: u32) -> Option<&LoadSet> {
    sets.iter().find(|x| x.slot == v as usize).or_else(|| sets.iter().find(|x| x.hash == v))
}

/// A package path of the load-set table as a folder of the disc (`/` separated, lower case).
pub fn package_folder(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}

/// Each group's package folders.
pub fn packages(sets: &[LoadSet]) -> HashMap<String, Vec<String>> {
    let mut g: HashMap<String, Vec<String>> = HashMap::new();
    for (group, path) in sets.iter().flat_map(|s| &s.entries) {
        let folders = g.entry(group.clone()).or_default();
        let f = package_folder(path);
        if !folders.contains(&f) {
            folders.push(f);
        }
    }
    g
}

/// The wipes (resource type 5), by resource id, for `LayerObj_ActivateWipe`.
pub fn wipe_files(vfs: &Vfs, resources: &Resources) -> HashMap<u32, Rc<WipeData>> {
    let wipe = |p: &str| WipeData::new(&vfs.read(p).ok()?).map(Rc::new);
    let five = resources.kinds.iter().filter(|(_, &k)| k == 5);
    five.filter_map(|(&id, _)| Some((id, wipe(resources.path(id, 0)?)?))).collect()
}

/// The fonts' files (resource type 1), by resource id.
pub fn font_files(vfs: &Vfs, resources: &Resources) -> HashMap<u32, Vec<u8>> {
    resources
        .kinds
        .iter()
        .filter(|(_, &k)| k == 1)
        .filter_map(|(&id, _)| Some((id, vfs.read(resources.path(id, 0)?).ok()?.to_vec())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_specs_name_frames_and_aliases() {
        // frame 0 is the base file; a frame with its own file takes it, one without falls back to the base
        let have = |p: &str| p == "a/b.2.png";
        assert_eq!(frame_file("a/b.png", 0, &have), "a/b.png");
        assert_eq!(frame_file("a/b.png", 2, &have), "a/b.2.png");
        assert_eq!(frame_file("a/b.png", 3, &have), "a/b.png");
        assert_eq!(frame_file("frontend/x/glow.fsh", 2, &have), "frontend/x/glow.fsh");
        // a resource alias round-trips through its spec
        let spec = Disc::resource_spec(0xaabbccdd, 0x1234);
        assert_eq!(parse_resource_spec(&spec), Some((0xaabbccdd, 0x1234)));
        assert_eq!(parse_resource_spec("a/b.png"), None);
    }
}
