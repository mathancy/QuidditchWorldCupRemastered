//! Resource packs (`docs/LAUNCHER.md`). Not the original's: the remaster's way of laying new files over the game.
//!
//! A pack is a folder with a manifest (`pack.ron`, [`Manifest`]) and any of the parts the engine reads from it
//! ([`PARTS`]): remastered textures, movies, screen layouts, fonts, the stadiums' looks, whole stadiums, and a raw
//! overlay in the disc's layout. Players get one as a `.qwcpack` file (a zip of the folder, [`build`]); installing
//! it ([`install`]) unpacks it into the library's `packs` folder, so the engine only ever reads folders (the movie
//! decoders want files).
//!
//! The player's [`Profile`] (`profile.ron` in the [library](library_dir)) lists the packs in order, the highest
//! first, each on or off. The packs on make a [`Stack`]: for each thing a pack can fill (a [slot](Stack::slots)) the
//! highest pack that has it wins. The process's stack is [`current`] (from `$QWC_PROFILE`, else the library's
//! profile, else none), so every part of the game asks the same one.

mod archive;
pub mod authoring;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

pub use archive::{build, install};

/// The remaster's version, which a pack's `engine` requirement is checked against.
pub const ENGINE: &str = env!("CARGO_PKG_VERSION");

/// The manifest's file name in a pack.
pub const MANIFEST: &str = "pack.ron";

/// What a pack's top level may hold, and what each is for. Anything else is reported by [`Pack::check`].
pub const PARTS: &[(&str, &str)] = &[
    ("pack.ron", "the manifest"),
    ("icon.png", "the launcher's picture of the pack"),
    ("textures", "textures.txt and its PNGs: textures standing in for the disc's"),
    ("movies", "MP4s standing in for the disc's movies of the same name"),
    ("screens", "screen layouts (<layer>.ron), their crops (crops.ron) and their images/"),
    ("hud", "<scene>.ron: a HUD scene (match, challenge) standing in for the stock one whole"),
    ("fonts.txt", "the vector fonts' list, laid over lower packs' (each line the highest's)"),
    ("fonts", "the font files fonts.txt names"),
    ("looks", "<stadium>.ron: a stadium's remastered lighting and colour grade"),
    ("stadiums", "<stadium>/: a stadium's scenery as GLB files"),
    ("players", "<team folder>/<model>.glb: a team's players (keeper, chaser1, ... seeker) on the original skeleton"),
    ("props", "<model>.glb: the balls (quaffle, bludger, snitch, bat, snidget)"),
    ("models", "any other model by its game path, as models/stadium/hoopnorm/gfx/singlehoop.glb"),
    ("shaders", "shaders.ron and its WGSL: material shaders and post passes (docs/SHADERS.md)"),
    ("tuning", "Gameplay packs only: *.cli console scripts run after the game's own, teams/<folder>/*.cli into a team"),
    ("scripts", "reserved for scripted mechanics"),
    ("disc", "files in the disc's layout, archive members included, laid over the game data"),
];

/// A pack stadium's `stadium.ron`: what its scenery stands in for and how far the camera sees round it. Without
/// the file (or for what it leaves out), the scenery has its own hoops and flags, the disc's sky is grown 6 times
/// and the camera sees 8000 units.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename = "Stadium")]
pub struct StadiumConfig {
    /// The disc's hoops drawn at the match's hoops (false: the scenery has its own).
    pub hoops: bool,
    /// The disc's wavy flags drawn (false: the scenery has its own).
    pub flags: bool,
    /// How much the disc's sky is grown round the scenery (1: as it is).
    pub sky_scale: f32,
    /// How far the camera sees (at least the original's).
    pub far: f32,
}

impl Default for StadiumConfig {
    fn default() -> StadiumConfig {
        StadiumConfig { hoops: false, flags: false, sky_scale: 6.0, far: 8000.0 }
    }
}

impl StadiumConfig {
    /// The `stadium.ron` of the stadium folder `dir`: the defaults without one.
    pub fn read(dir: &Path) -> Result<StadiumConfig, String> {
        let p = dir.join("stadium.ron");
        match std::fs::read_to_string(&p) {
            Ok(t) => ron::from_str(&t).map_err(|e| format!("{}: {e}", p.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(StadiumConfig::default()),
            Err(e) => Err(format!("{}: {e}", p.display())),
        }
    }

    /// Write it as the `stadium.ron` of the stadium folder `dir`.
    pub fn write(&self, dir: &Path) -> Result<(), String> {
        let p = dir.join("stadium.ron");
        let pretty = ron::ser::PrettyConfig::new().struct_names(true);
        let text = ron::ser::to_string_pretty(self, pretty).map_err(|e| e.to_string())? + "\n";
        std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))
    }
}

/// A pack's `shaders/shaders.ron`: its material shaders and post passes, each a WGSL file under `shaders/`
/// (`docs/SHADERS.md`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename = "Shaders")]
pub struct ShaderList {
    pub materials: Vec<MaterialShaderEntry>,
    pub post: Vec<PostPassEntry>,
}

/// A material shader: its file, and the materials it draws by their texture's path or their name (`*` and `?`,
/// case aside).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename = "Material")]
pub struct MaterialShaderEntry {
    pub file: String,
    pub textures: Vec<String>,
    pub materials: Vec<String>,
}

/// A post pass: its file and up to eight numbers of its own.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename = "Post")]
pub struct PostPassEntry {
    pub file: String,
    pub params: Vec<f32>,
}

impl ShaderList {
    /// The list in the shaders folder `dir` (`shaders.ron`): empty without one.
    pub fn read(dir: &Path) -> Result<ShaderList, String> {
        let p = dir.join("shaders.ron");
        match std::fs::read_to_string(&p) {
            Ok(t) => ron::from_str(&t).map_err(|e| format!("{}: {e}", p.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ShaderList::default()),
            Err(e) => Err(format!("{}: {e}", p.display())),
        }
    }

    /// Write it as the `shaders.ron` of the shaders folder `dir`.
    pub fn write(&self, dir: &Path) -> Result<(), String> {
        let p = dir.join("shaders.ron");
        let pretty = ron::ser::PrettyConfig::new().struct_names(true);
        let text = ron::ser::to_string_pretty(self, pretty).map_err(|e| e.to_string())? + "\n";
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))
    }

    /// The files it names, materials first.
    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.materials.iter().map(|m| m.file.as_str()).chain(self.post.iter().map(|p| p.file.as_str()))
    }
}

/// Whether a pack changes only how the game looks or also how it plays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    #[default]
    Visual,
    /// May carry `tuning/`; recorded in results and saves.
    Gameplay,
}

/// A pack's `pack.ron`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Pack")]
pub struct Manifest {
    /// `author.name`: lower-case letters, digits, `-` and `_`, in two or more parts joined by dots.
    pub id: String,
    pub name: String,
    /// Semantic versioning (`1.0.0`).
    pub version: String,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub kind: Kind,
    /// The remaster versions the pack works with, as a requirement (`>=0.1`); any when empty.
    #[serde(default)]
    pub engine: String,
    /// Where the pack and its updates come from.
    #[serde(default)]
    pub source: Option<String>,
    /// Other packs' ids this one needs on (below it or above it).
    #[serde(default)]
    pub requires: Vec<String>,
    /// Packs' ids that must not be on with this one.
    #[serde(default)]
    pub conflicts: Vec<String>,
}

impl Manifest {
    /// A new pack's manifest: version 0.1.0, Visual, for this remaster on.
    pub fn new(id: &str, name: &str) -> Manifest {
        Manifest {
            id: id.to_string(),
            name: name.to_string(),
            version: "0.1.0".into(),
            authors: vec![],
            description: String::new(),
            kind: Kind::Visual,
            engine: format!(">={ENGINE}"),
            source: None,
            requires: vec![],
            conflicts: vec![],
        }
    }

    pub fn parse(text: &str) -> Result<Manifest, String> {
        let m: Manifest = ron::from_str(text).map_err(|e| e.to_string())?;
        m.validate()?;
        Ok(m)
    }

    /// The fields' own rules: the id's form, the version and the engine requirement.
    pub fn validate(&self) -> Result<(), String> {
        if !valid_id(&self.id) {
            return Err(format!("id {:?} is not author.name (a-z, 0-9, - and _, parts joined by dots)", self.id));
        }
        if self.name.trim().is_empty() {
            return Err("no name".into());
        }
        semver::Version::parse(&self.version).map_err(|e| format!("version {:?}: {e}", self.version))?;
        if !self.engine.trim().is_empty() {
            semver::VersionReq::parse(&self.engine).map_err(|e| format!("engine {:?}: {e}", self.engine))?;
        }
        Ok(())
    }

    /// Whether this remaster ([`ENGINE`]) meets the pack's `engine` requirement.
    pub fn fits_engine(&self) -> bool {
        let engine = semver::Version::parse(ENGINE).expect("the crate's own version");
        self.engine.trim().is_empty() || semver::VersionReq::parse(&self.engine).is_ok_and(|r| r.matches(&engine))
    }

    pub fn to_text(&self) -> String {
        let pretty = ron::ser::PrettyConfig::new().struct_names(true);
        ron::ser::to_string_pretty(self, pretty).expect("a manifest always serialises") + "\n"
    }
}

/// `author.name`: two or more parts of `a-z 0-9 - _`, joined by single dots.
pub fn valid_id(id: &str) -> bool {
    let parts: Vec<&str> = id.split('.').collect();
    parts.len() >= 2
        && parts.iter().all(|p| {
            !p.is_empty() && p.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        })
}

/// A pack folder and its manifest.
#[derive(Clone, Debug, PartialEq)]
pub struct Pack {
    pub dir: PathBuf,
    pub manifest: Manifest,
}

/// How bad a problem [`Pack::check`] found is: an error stops a pack being built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Warning,
    Error,
}

impl Pack {
    /// The pack in folder `dir` (its manifest read).
    pub fn open(dir: impl AsRef<Path>) -> Result<Pack, String> {
        let dir = dir.as_ref().to_path_buf();
        let path = dir.join(MANIFEST);
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let manifest = Manifest::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Pack { dir, manifest })
    }

    /// A new pack in `dir` (made when missing) with `manifest`; refused where a pack is already.
    pub fn create(dir: impl AsRef<Path>, manifest: Manifest) -> Result<Pack, String> {
        let dir = dir.as_ref().to_path_buf();
        if dir.join(MANIFEST).exists() {
            return Err(format!("{}: a pack is there already", dir.display()));
        }
        manifest.validate()?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        std::fs::write(dir.join(MANIFEST), manifest.to_text()).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(Pack { dir, manifest })
    }

    /// Write the manifest back.
    pub fn save(&self) -> Result<(), String> {
        self.manifest.validate()?;
        let p = self.dir.join(MANIFEST);
        std::fs::write(&p, self.manifest.to_text()).map_err(|e| format!("{}: {e}", p.display()))
    }

    /// A part of the pack (`textures/textures.txt`, `screens`), when it is there.
    pub fn part(&self, rel: &str) -> Option<PathBuf> {
        let p = rel.split('/').fold(self.dir.clone(), |d, part| d.join(part));
        p.exists().then_some(p)
    }

    /// What is wrong with the pack's layout: unknown parts, `tuning/` in a Visual pack, parts this version does not
    /// read yet, an `engine` this remaster does not meet.
    pub fn check(&self) -> Vec<(Severity, String)> {
        let mut out = Vec::new();
        let known = |name: &str| {
            let lower = name.to_ascii_lowercase();
            PARTS.iter().any(|(p, _)| *p == lower)
                || ["readme", "license", "licence", "ofl", "changelog"].iter().any(|k| lower.starts_with(k))
        };
        let mut names: Vec<String> = std::fs::read_dir(&self.dir)
            .map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        names.sort();
        for n in &names {
            if !n.starts_with('.') && !known(n) {
                out.push((Severity::Warning, format!("{n}: not a part the remaster reads (it is left alone)")));
            }
        }
        if self.manifest.kind == Kind::Visual && self.part("tuning").is_some() {
            out.push((
                Severity::Error,
                "tuning/ changes how the game plays: the manifest's kind must be Gameplay".into(),
            ));
        }
        if self.part("scripts").is_some() {
            out.push((
                Severity::Error,
                "scripts/ is reserved for scripted mechanics, which this version cannot run".into(),
            ));
        }
        if let Some(dir) = self.part("shaders") {
            match ShaderList::read(&dir) {
                Ok(list) if list.materials.is_empty() && list.post.is_empty() => {
                    out.push((Severity::Warning, "shaders/: shaders.ron lists no shaders, so none are drawn".into()));
                }
                Ok(list) => {
                    for f in list.files().filter(|f| find_any_case(&dir, f).is_none()) {
                        out.push((Severity::Error, format!("shaders/shaders.ron names {f}, which is not there")));
                    }
                    for m in list.materials.iter().filter(|m| m.textures.is_empty() && m.materials.is_empty()) {
                        out.push((Severity::Warning, format!("shaders/{}: names no textures or materials", m.file)));
                    }
                    for p in list.post.iter().filter(|p| p.params.len() > 8) {
                        out.push((Severity::Warning, format!("shaders/{}: only 8 params are passed", p.file)));
                    }
                }
                Err(e) => out.push((Severity::Error, e)),
            }
        }
        if !self.manifest.fits_engine() {
            out.push((Severity::Warning, format!("made for remaster {}, this is {ENGINE}", self.manifest.engine)));
        }
        if let Some(list) = self.part("textures/textures.txt") {
            if let Err(e) = qwc_assets::remaster::Remastered::read(&list) {
                out.push((Severity::Error, e));
            }
        } else if self.part("textures").is_some() {
            out.push((Severity::Error, "textures/ without textures.txt (the list of what each PNG replaces)".into()));
        }
        out
    }

    /// The pack's glTF for the game's model at `path` ([`model_slot`]), if it has one.
    pub fn model(&self, path: &str) -> Option<PathBuf> {
        find_any_case(&self.dir, &model_slot(path))
    }

    /// The game paths of the models the pack stands in for (`teams/Gryff/seeker.fpm`), from its files.
    pub fn model_paths(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        out.extend(glbs_under(&self.dir.join("players")).into_iter().map(|p| format!("teams/{p}.fpm")));
        out.extend(glbs_under(&self.dir.join("props")).into_iter().map(|p| format!("root/balls/{p}.fpm")));
        out.extend(glbs_under(&self.dir.join("models")).into_iter().map(|p| format!("{p}.fpm")));
        out
    }

    /// The slots the pack fills ([`Stack::slots`]).
    pub fn slots(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(list) = self.part("textures/textures.txt") {
            if let Ok(r) = qwc_assets::remaster::Remastered::read(&list) {
                out.extend(r.textures().map(|t| format!("texture/{t}")));
            }
        }
        let stems = |dir: &str, ext: &str| -> Vec<String> {
            files_in(&self.dir.join(dir))
                .into_iter()
                .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)))
                .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()))
                .collect()
        };
        out.extend(stems("movies", "mp4").into_iter().map(|s| format!("movie/{s}")));
        // the crops are laid over the lower packs' by name, not a screen
        let screens = stems("screens", "ron").into_iter().filter(|s| s != "crops");
        out.extend(screens.map(|s| format!("screen/{s}")));
        out.extend(stems("hud", "ron").into_iter().map(|s| format!("hud/{s}")));
        out.extend(stems("looks", "ron").into_iter().map(|s| format!("look/{s}")));
        if let Ok(d) = std::fs::read_dir(self.dir.join("stadiums")) {
            for e in d.flatten().filter(|e| e.path().is_dir()) {
                out.push(format!("stadium/{}", e.file_name().to_string_lossy().to_ascii_lowercase()));
            }
        }
        if self.part("fonts.txt").is_some() {
            out.push("fonts".into());
        }
        out.extend(glbs_under(&self.dir.join("players")).into_iter().map(|p| format!("player/{p}")));
        out.extend(glbs_under(&self.dir.join("props")).into_iter().map(|p| format!("prop/{p}")));
        out.extend(glbs_under(&self.dir.join("models")).into_iter().map(|p| format!("model/{p}")));
        out.extend(stems("tuning", "cli").into_iter().map(|s| format!("tuning/{s}")));
        if let Some(list) = self.part("shaders").and_then(|d| ShaderList::read(&d).ok()) {
            // every post pass runs, so each is its own slot; material shaders overlap by file name
            let stem = |f: &str| f.rsplit('/').next().unwrap_or(f).trim_end_matches(".wgsl").to_ascii_lowercase();
            out.extend(list.materials.iter().map(|m| format!("shader/material/{}", stem(&m.file))));
            let id = &self.manifest.id;
            out.extend(list.post.iter().map(|p| format!("shader/post/{id}/{}", stem(&p.file))));
        }
        for team in std::fs::read_dir(self.dir.join("tuning").join("teams")).into_iter().flatten().flatten() {
            let folder = team.file_name().to_string_lossy().to_ascii_lowercase();
            for f in files_in(&team.path())
                .into_iter()
                .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("cli")))
            {
                let stem = f.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
                out.push(format!("tuning/teams/{folder}/{stem}"));
            }
        }
        let disc = self.dir.join("disc");
        let mut stack = vec![disc.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if let Ok(rel) = p.strip_prefix(&disc) {
                    let rel = rel.to_string_lossy().replace('\\', "/").to_ascii_lowercase();
                    out.push(format!("disc/{rel}"));
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }
}

/// The `.glb` files under `dir`, as lower-case paths relative to it without the extension (`gryff/seeker`).
fn glbs_under(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                todo.push(p);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("glb")) {
                if let Ok(rel) = p.with_extension("").strip_prefix(dir) {
                    out.push(rel.to_string_lossy().replace('\\', "/").to_ascii_lowercase());
                }
            }
        }
    }
    out.sort();
    out
}

/// The pack file standing in for the game's model at `path` (`teams/Gryff/seeker.fpm`): `players/<team
/// folder>/<name>.glb` for a team's model, `props/<name>.glb` for a ball's (`root/Balls/`), else
/// `models/<path>.glb`; any case.
pub fn model_slot(path: &str) -> String {
    let p = path.replace('\\', "/");
    let stem = p.rsplit_once('.').map_or(p.as_str(), |(s, _)| s);
    let parts: Vec<&str> = stem.split('/').collect();
    match parts.as_slice() {
        [t, team, name] if t.eq_ignore_ascii_case("teams") => format!("players/{team}/{name}.glb"),
        [r, b, name] if r.eq_ignore_ascii_case("root") && b.eq_ignore_ascii_case("balls") => {
            format!("props/{name}.glb")
        }
        _ => format!("models/{stem}.glb"),
    }
}

/// `rel` (with `/`) under `dir`, matched in any case, when it is there.
fn find_any_case(dir: &Path, rel: &str) -> Option<PathBuf> {
    let mut at = dir.to_path_buf();
    for part in rel.split('/') {
        let exact = at.join(part);
        at = if exact.exists() {
            exact
        } else {
            std::fs::read_dir(&at)
                .ok()?
                .flatten()
                .map(|e| e.path())
                .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(part)))?
        };
    }
    at.is_file().then_some(at)
}

/// The files (not folders) directly in `dir`, sorted; none when it is not there.
fn files_in(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// The `.cli` files directly in `dir`, by name.
fn cli_files(dir: &Path) -> Vec<PathBuf> {
    files_in(dir).into_iter().filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("cli"))).collect()
}

/// The remaster's folder in the user's profile: `$QWC_LIBRARY`, else `%APPDATA%\QWCRemastered` on Windows, else
/// `$XDG_DATA_HOME/qwc-remastered` (`~/.local/share/qwc-remastered`).
pub fn library_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("QWC_LIBRARY") {
        return Some(d.into());
    }
    if cfg!(windows) {
        return std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("QWCRemastered"));
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(data.join("qwc-remastered"))
}

/// One pack in the profile's order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

/// The player's choices: the game files, the exe, and the packs in order (the highest first).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    /// The imported game files (the disc's layout).
    pub data: Option<PathBuf>,
    /// The unwrapped `QWC.exe` the tables are read from.
    pub exe: Option<PathBuf>,
    /// Where the packs are: the library's `packs` by default.
    pub packs_dir: Option<PathBuf>,
    /// The packs, the highest first. Packs in the folder that are not listed are off.
    pub packs: Vec<Entry>,
}

impl Profile {
    /// The default profile's file: `profile.ron` in the [library](library_dir).
    pub fn default_path() -> Option<PathBuf> {
        library_dir().map(|d| d.join("profile.ron"))
    }

    pub fn read(path: &Path) -> Result<Profile, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut p: Profile = ron::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        // relative paths are the profile's folder's
        let base = path.parent().unwrap_or(Path::new("."));
        for f in [&mut p.data, &mut p.exe, &mut p.packs_dir].into_iter().flatten() {
            if f.is_relative() {
                *f = base.join(&*f);
            }
        }
        Ok(p)
    }

    pub fn write(&self, path: &Path) -> Result<(), String> {
        let pretty = ron::ser::PrettyConfig::new().struct_names(true);
        let text = ron::ser::to_string_pretty(self, pretty).map_err(|e| e.to_string())? + "\n";
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The packs' folder: the profile's, else the library's `packs`.
    pub fn packs_dir(&self) -> Option<PathBuf> {
        self.packs_dir.clone().or_else(|| library_dir().map(|d| d.join("packs")))
    }

    /// Every pack in the packs' folder, by id (a folder whose manifest does not read is reported in the errors).
    pub fn installed(&self) -> (BTreeMap<String, Pack>, Vec<String>) {
        let (mut packs, mut errors) = (BTreeMap::new(), Vec::new());
        let Some(dir) = self.packs_dir() else { return (packs, errors) };
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.join(MANIFEST).is_file()).collect())
            .unwrap_or_default();
        dirs.sort();
        for d in dirs {
            match Pack::open(&d) {
                Ok(p) => {
                    if let Some(old) = packs.get(&p.manifest.id).map(|o: &Pack| o.dir.clone()) {
                        errors.push(format!("{}: id {} is {}'s already", d.display(), p.manifest.id, old.display()));
                    } else {
                        packs.insert(p.manifest.id.clone(), p);
                    }
                }
                Err(e) => errors.push(e),
            }
        }
        (packs, errors)
    }

    /// Put `id` at the top of the order, on (moved there if it was listed).
    pub fn put_on_top(&mut self, id: &str) {
        self.packs.retain(|e| e.id != id);
        self.packs.insert(0, Entry { id: id.to_string(), enabled: true });
    }

    /// Turn a pack on or off (listed at the top when it was not).
    pub fn set_enabled(&mut self, id: &str, enabled: bool) {
        match self.packs.iter_mut().find(|e| e.id == id) {
            Some(e) => e.enabled = enabled,
            None => self.packs.insert(0, Entry { id: id.to_string(), enabled }),
        }
    }
}

/// The packs on, the highest first, and what the remaster reads from them.
#[derive(Clone, Debug, Default)]
pub struct Stack {
    /// Where the profile was read from (none: no profile, so no packs).
    pub profile_path: Option<PathBuf>,
    pub profile: Profile,
    /// The packs on, the highest first.
    pub packs: Vec<Pack>,
    /// What went wrong finding them: a listed pack not installed, a requirement not on, a conflict.
    pub problems: Vec<String>,
}

/// A slot more than one pack on fills: the winner (the highest) and the packs under it.
#[derive(Clone, Debug, PartialEq)]
pub struct Overlap {
    pub slot: String,
    /// The packs' ids, the winner first.
    pub packs: Vec<String>,
}

impl Stack {
    /// The packs `profile` turns on, in its order.
    pub fn from_profile(profile: Profile, profile_path: Option<PathBuf>) -> Stack {
        let (mut installed, mut problems) = profile.installed();
        let mut packs = Vec::new();
        for e in profile.packs.iter().filter(|e| e.enabled) {
            match installed.remove(&e.id) {
                Some(p) => packs.push(p),
                None => problems.push(format!("{}: listed in the profile but not installed", e.id)),
            }
        }
        let on: Vec<&str> = packs.iter().map(|p| p.manifest.id.as_str()).collect();
        for p in &packs {
            let id = &p.manifest.id;
            for r in p.manifest.requires.iter().filter(|r| !on.contains(&r.as_str())) {
                problems.push(format!("{id} needs {r}, which is not on"));
            }
            for c in p.manifest.conflicts.iter().filter(|c| on.contains(&c.as_str())) {
                problems.push(format!("{id} conflicts with {c}, which is on"));
            }
            if !p.manifest.fits_engine() {
                problems.push(format!("{id} is made for remaster {}, this is {ENGINE}", p.manifest.engine));
            }
        }
        Stack { profile_path, profile, packs, problems }
    }

    /// The stack of the profile at `path`.
    pub fn read(path: &Path) -> Result<Stack, String> {
        Ok(Stack::from_profile(Profile::read(path)?, Some(path.to_path_buf())))
    }

    /// The packs on, the lowest first.
    fn lowest_first(&self) -> impl Iterator<Item = &Pack> {
        self.packs.iter().rev()
    }

    /// The texture lists (`textures/textures.txt`), the lowest first: each read over the ones before it.
    pub fn texture_lists(&self) -> Vec<PathBuf> {
        self.lowest_first().filter_map(|p| p.part("textures/textures.txt")).collect()
    }

    /// The highest pack's font list, if any has one.
    pub fn font_list(&self) -> Option<PathBuf> {
        self.packs.iter().find_map(|p| p.part("fonts.txt"))
    }

    /// Every pack's font list, the lowest first: each laid over the ones before it (`qwc_text::FontList`).
    pub fn font_lists(&self) -> Vec<PathBuf> {
        self.lowest_first().filter_map(|p| p.part("fonts.txt")).collect()
    }

    /// Every pack's screen layouts' folder, the lowest first: each pack's layer files laid over the lower ones'.
    pub fn screens_dirs(&self) -> Vec<PathBuf> {
        self.lowest_first().filter_map(|p| p.part("screens")).filter(|d| d.is_dir()).collect()
    }

    /// The movie folders, the highest first.
    pub fn movie_dirs(&self) -> Vec<PathBuf> {
        self.packs.iter().filter_map(|p| p.part("movies")).collect()
    }

    /// The screen layouts' folder: the highest pack's that has one.
    pub fn screens(&self) -> Option<PathBuf> {
        self.packs.iter().find_map(|p| p.part("screens"))
    }

    /// The look file of the stadium in folder `stadium`, from the highest pack that has one.
    pub fn look(&self, stadium: &str) -> Option<PathBuf> {
        let file = format!("looks/{}.ron", stadium.to_ascii_lowercase());
        self.packs.iter().find_map(|p| p.part(&file))
    }

    /// The HUD scene file `file` (`match.ron`), from the highest pack that has one.
    pub fn hud(&self, file: &str) -> Option<PathBuf> {
        let file = format!("hud/{file}");
        self.packs.iter().find_map(|p| p.part(&file))
    }

    /// The scenery folder of the stadium `name` (`sum_fe`), from the highest pack that has one.
    pub fn stadium(&self, name: &str) -> Option<PathBuf> {
        let dir = format!("stadiums/{}", name.to_ascii_lowercase());
        self.packs.iter().find_map(|p| p.part(&dir)).filter(|d| d.is_dir())
    }

    /// The glTF standing in for the game's model at `path` ([`model_slot`]), from the highest pack that has one.
    pub fn model(&self, path: &str) -> Option<PathBuf> {
        self.packs.iter().find_map(|p| p.model(path))
    }

    /// The Gameplay packs on, the lowest first.
    pub fn gameplay(&self) -> impl Iterator<Item = &Pack> {
        self.lowest_first().filter(|p| p.manifest.kind == Kind::Gameplay)
    }

    /// The match's tuning scripts: the Gameplay packs' `tuning/*.cli`, the lowest pack first, each pack's in name
    /// order (so the highest pack's settings are the last word). A Visual pack's are never run.
    pub fn tuning_scripts(&self) -> Vec<PathBuf> {
        self.gameplay().flat_map(|p| cli_files(&p.dir.join("tuning"))).collect()
    }

    /// A team's tuning scripts (its folder under `teams/`, any case): the Gameplay packs'
    /// `tuning/teams/<folder>/*.cli`, in the same order.
    pub fn team_tuning(&self, folder: &str) -> Vec<PathBuf> {
        self.gameplay()
            .filter_map(|p| {
                std::fs::read_dir(p.dir.join("tuning").join("teams"))
                    .ok()?
                    .flatten()
                    .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(folder))
                    .map(|e| cli_files(&e.path()))
            })
            .flatten()
            .collect()
    }

    /// The material shaders, the highest pack's first (the first naming a material draws it): each pack's id, its
    /// shaders folder and the entry.
    pub fn material_shaders(&self) -> Vec<(String, PathBuf, MaterialShaderEntry)> {
        self.packs
            .iter()
            .filter_map(|p| Some((p, p.part("shaders")?)))
            .flat_map(|(p, dir)| {
                let list = ShaderList::read(&dir).unwrap_or_default();
                list.materials.into_iter().map(move |m| (p.manifest.id.clone(), dir.clone(), m))
            })
            .collect()
    }

    /// The post passes in the order they run: the lowest pack's first, each pack's in its list's order.
    pub fn post_passes(&self) -> Vec<(String, PathBuf, PostPassEntry)> {
        self.lowest_first()
            .filter_map(|p| Some((p, p.part("shaders")?)))
            .flat_map(|(p, dir)| {
                let list = ShaderList::read(&dir).unwrap_or_default();
                list.post.into_iter().map(move |e| (p.manifest.id.clone(), dir.clone(), e))
            })
            .collect()
    }

    /// A shader file a pack names (`file` under its shaders folder `dir`), any case.
    pub fn shader_file(dir: &Path, file: &str) -> Option<PathBuf> {
        find_any_case(dir, &file.replace('\\', "/"))
    }

    /// The overlays in the disc's layout (`disc/`), the lowest first: each laid over the ones before it.
    pub fn disc_overlays(&self) -> Vec<PathBuf> {
        self.lowest_first().filter_map(|p| p.part("disc")).filter(|d| d.is_dir()).collect()
    }

    /// Every slot a pack on fills, with the packs filling it (the winner first).
    pub fn slots(&self) -> BTreeMap<String, Vec<String>> {
        let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for p in &self.packs {
            for s in p.slots() {
                out.entry(s).or_default().push(p.manifest.id.clone());
            }
        }
        out
    }

    /// The slots more than one pack on fills.
    pub fn overlaps(&self) -> Vec<Overlap> {
        self.slots().into_iter().filter(|(_, p)| p.len() > 1).map(|(slot, packs)| Overlap { slot, packs }).collect()
    }
}

static CURRENT: OnceLock<Stack> = OnceLock::new();

/// Set the process's stack before anything asks for it ([`current`]): the profile at `profile`, or the default one,
/// or none at all with `no_packs`. Its problems are printed. Later calls change nothing and answer the first.
pub fn init(profile: Option<&Path>, no_packs: bool) -> &'static Stack {
    CURRENT.get_or_init(|| {
        if no_packs {
            return Stack::default();
        }
        let path = profile
            .map(Path::to_path_buf)
            .or_else(|| std::env::var_os("QWC_PROFILE").map(PathBuf::from))
            .or_else(|| Profile::default_path().filter(|p| p.is_file()));
        let Some(path) = path else { return Stack::default() };
        let stack = Stack::read(&path).unwrap_or_else(|e| {
            eprintln!("{e}; no packs");
            Stack::default()
        });
        for p in &stack.problems {
            eprintln!("packs: {p}");
        }
        stack
    })
}

/// The process's stack: [`init`]'s, else the default profile's.
pub fn current() -> &'static Stack {
    init(None, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder under the target directory's temp space.
    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("qwc-packs-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn manifest(id: &str) -> Manifest {
        Manifest {
            id: id.into(),
            name: id.into(),
            version: "1.0.0".into(),
            authors: vec!["someone".into()],
            description: String::new(),
            kind: Kind::Visual,
            engine: String::new(),
            source: None,
            requires: vec![],
            conflicts: vec![],
        }
    }

    /// A pack folder `id` in `packs` with `files` (path, text).
    fn pack(packs: &Path, id: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = packs.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MANIFEST), manifest(id).to_text()).unwrap();
        for (p, text) in files {
            let f = dir.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, text).unwrap();
        }
        dir
    }

    #[test]
    fn stadium_configs() {
        let root = scratch("stadium");
        assert_eq!(StadiumConfig::read(&root).unwrap(), StadiumConfig::default());
        std::fs::write(root.join("stadium.ron"), "Stadium(hoops: true, far: 5000)").unwrap();
        let c = StadiumConfig::read(&root).unwrap();
        assert_eq!((c.hoops, c.flags, c.sky_scale, c.far), (true, false, 6.0, 5000.0));
        std::fs::write(root.join("stadium.ron"), "Stadium(hoops: 3)").unwrap();
        assert!(StadiumConfig::read(&root).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ids() {
        assert!(valid_id("mathancy.hogwarts-summer") && valid_id("a.b.c_d") && valid_id("local.remastered"));
        for bad in ["nodot", "Upper.case", "a..b", ".a", "a.", "a.b c", "a/b.c"] {
            assert!(!valid_id(bad), "{bad}");
        }
    }

    #[test]
    fn manifest_reads_back_and_checks_its_fields() {
        let m = Manifest { kind: Kind::Gameplay, engine: ">=0.1".into(), ..manifest("me.pack") };
        assert_eq!(Manifest::parse(&m.to_text()).unwrap(), m);
        assert!(m.to_text().starts_with("Pack("));
        // the fields left out take their defaults
        let short = Manifest::parse("Pack(id: \"me.p\", name: \"P\", version: \"0.2.0\")").unwrap();
        assert_eq!((short.kind, short.engine.as_str(), short.authors.len()), (Kind::Visual, "", 0));
        assert!(Manifest::parse("Pack(id: \"me\", name: \"P\", version: \"1.0.0\")")
            .unwrap_err()
            .contains("author.name"));
        assert!(Manifest::parse("Pack(id: \"me.p\", name: \"P\", version: \"1.0\")").unwrap_err().contains("version"));
        assert!(Manifest::parse("Pack(id: \"me.p\", name: \"P\", version: \"1.0.0\", engine: \"what\")").is_err());
        assert!(Manifest { engine: ">=0.1, <1".into(), ..manifest("a.b") }.fits_engine());
        assert!(!Manifest { engine: ">=9".into(), ..manifest("a.b") }.fits_engine());
    }

    #[test]
    fn the_highest_pack_wins_each_slot() {
        let root = scratch("stack");
        let packs = root.join("packs");
        pack(&packs, "a.low", &[("movies/wb.mp4", ""), ("looks/england.ron", "()"), ("disc/x/y.fsh", "")]);
        pack(&packs, "b.high", &[("movies/WB.mp4", ""), ("movies/eag.mp4", ""), ("screens/Title.ron", "()")]);
        pack(&packs, "c.off", &[("looks/england.ron", "()")]);
        let profile = Profile {
            packs_dir: Some(packs.clone()),
            packs: vec![
                Entry { id: "b.high".into(), enabled: true },
                Entry { id: "c.off".into(), enabled: false },
                Entry { id: "a.low".into(), enabled: true },
                Entry { id: "gone.missing".into(), enabled: true },
            ],
            ..Profile::default()
        };
        let s = Stack::from_profile(profile, None);
        let ids: Vec<&str> = s.packs.iter().map(|p| p.manifest.id.as_str()).collect();
        assert_eq!(ids, ["b.high", "a.low"]);
        assert_eq!(s.problems, ["gone.missing: listed in the profile but not installed"]);
        assert_eq!(s.movie_dirs(), [packs.join("b.high/movies"), packs.join("a.low/movies")]);
        assert_eq!(s.look("England"), Some(packs.join("a.low/looks/england.ron")));
        assert_eq!(s.screens(), Some(packs.join("b.high/screens")));
        assert_eq!(s.disc_overlays(), [packs.join("a.low/disc")]);
        assert_eq!(s.overlaps(), [Overlap { slot: "movie/wb".into(), packs: vec!["b.high".into(), "a.low".into()] }]);
        assert_eq!(s.slots()["disc/x/y.fsh"], ["a.low"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn models_by_their_slots() {
        assert_eq!(model_slot("teams/Gryff/seeker.fpm"), "players/Gryff/seeker.glb");
        assert_eq!(model_slot("root/Balls/quaffle.fpm"), "props/quaffle.glb");
        assert_eq!(model_slot("stadium\\hoopnorm\\gfx\\singlehoop.fpm"), "models/stadium/hoopnorm/gfx/singlehoop.glb");
        let root = scratch("models");
        let packs = root.join("packs");
        pack(&packs, "a.low", &[("players/gryff/SEEKER.glb", ""), ("props/quaffle.glb", "")]);
        pack(&packs, "b.high", &[("props/Quaffle.GLB", "")]);
        let mut profile = Profile { packs_dir: Some(packs.clone()), ..Profile::default() };
        profile.put_on_top("a.low");
        profile.put_on_top("b.high");
        let s = Stack::from_profile(profile, None);
        // the file found whatever the case on either side (a case-blind file system answers in the asked case)
        let under = |p: Option<PathBuf>| {
            p.filter(|p| p.is_file())
                .map(|p| p.strip_prefix(&packs).unwrap().to_string_lossy().replace('\\', "/").to_ascii_lowercase())
        };
        assert_eq!(under(s.model("teams/Gryff/seeker.fpm")).as_deref(), Some("a.low/players/gryff/seeker.glb"));
        assert_eq!(under(s.model("root/Balls/quaffle.fpm")).as_deref(), Some("b.high/props/quaffle.glb"));
        assert_eq!(s.model("teams/Slyth/seeker.fpm"), None);
        assert_eq!(s.overlaps()[0].slot, "prop/quaffle");
        assert!(s.slots().contains_key("player/gryff/seeker"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn tuning_comes_from_gameplay_packs_only_lowest_first() {
        let root = scratch("tuning");
        let packs = root.join("packs");
        let a =
            pack(&packs, "a.rules", &[("tuning/b.cli", ""), ("tuning/a.cli", ""), ("tuning/teams/Gryff/x.cli", "")]);
        let b = pack(&packs, "b.rules", &[("tuning/z.cli", ""), ("tuning/notes.txt", "")]);
        let v = pack(&packs, "c.visual", &[("tuning/v.cli", "")]);
        for d in [&a, &b] {
            let m = Manifest { kind: Kind::Gameplay, ..Pack::open(d).unwrap().manifest };
            std::fs::write(d.join(MANIFEST), m.to_text()).unwrap();
        }
        let mut profile = Profile { packs_dir: Some(packs.clone()), ..Profile::default() };
        for id in ["a.rules", "c.visual", "b.rules"] {
            profile.put_on_top(id);
        }
        let s = Stack::from_profile(profile, None);
        let rel = |v: Vec<PathBuf>| {
            v.iter().map(|p| p.strip_prefix(&packs).unwrap().to_string_lossy().replace('\\', "/")).collect::<Vec<_>>()
        };
        assert_eq!(rel(s.tuning_scripts()), ["a.rules/tuning/a.cli", "a.rules/tuning/b.cli", "b.rules/tuning/z.cli"]);
        assert_eq!(rel(s.team_tuning("GRYFF")), ["a.rules/tuning/teams/Gryff/x.cli"]);
        assert!(s.team_tuning("Slyth").is_empty());
        assert!(Pack::open(&a).unwrap().slots().contains(&"tuning/teams/gryff/x".to_string()));
        assert!(Pack::open(&v)
            .unwrap()
            .check()
            .iter()
            .any(|(sev, t)| *sev == Severity::Error && t.contains("Gameplay")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn shaders_materials_highest_first_post_passes_lowest_first() {
        let root = scratch("shaders");
        let packs = root.join("packs");
        let list = "Shaders(materials: [(file: \"materials/water.wgsl\", textures: [\"*water*\"])], post: [(file: \"post/a.wgsl\", params: [0.5]), (file: \"post/b.wgsl\")])";
        pack(
            &packs,
            "a.low",
            &[
                ("shaders/shaders.ron", list),
                ("shaders/materials/water.wgsl", ""),
                ("shaders/post/a.wgsl", ""),
                ("shaders/post/B.wgsl", ""),
            ],
        );
        let high = "Shaders(materials: [(file: \"materials/water.wgsl\", materials: [\"lake\"])], post: [(file: \"post/gone.wgsl\")])";
        let h = pack(&packs, "b.high", &[("shaders/shaders.ron", high), ("shaders/materials/water.wgsl", "")]);
        let mut profile = Profile { packs_dir: Some(packs.clone()), ..Profile::default() };
        profile.put_on_top("a.low");
        profile.put_on_top("b.high");
        let s = Stack::from_profile(profile, None);
        let ids =
            |v: Vec<(String, PathBuf, String)>| v.into_iter().map(|(id, _, f)| format!("{id}:{f}")).collect::<Vec<_>>();
        let materials = s.material_shaders().into_iter().map(|(id, d, m)| (id, d, m.file)).collect();
        assert_eq!(ids(materials), ["b.high:materials/water.wgsl", "a.low:materials/water.wgsl"]);
        let post: Vec<_> = s.post_passes().into_iter().map(|(id, d, p)| (id, d, p.file)).collect();
        assert_eq!(ids(post.clone()), ["a.low:post/a.wgsl", "a.low:post/b.wgsl", "b.high:post/gone.wgsl"]);
        assert!(Stack::shader_file(&post[1].1, &post[1].2).is_some(), "found in any case");
        assert_eq!(s.post_passes()[0].2.params, [0.5]);
        assert_eq!(s.overlaps()[0].slot, "shader/material/water");
        let found = Pack::open(&h).unwrap().check();
        assert!(found.iter().any(|(sev, t)| *sev == Severity::Error && t.contains("post/gone.wgsl")), "{found:?}");
        let back = ShaderList::read(&packs.join("a.low/shaders")).unwrap();
        back.write(&root).unwrap();
        assert_eq!(ShaderList::read(&root).unwrap(), back);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn requirements_and_conflicts() {
        let root = scratch("needs");
        let packs = root.join("packs");
        let d = pack(&packs, "a.needs", &[]);
        let m = Manifest { requires: vec!["b.base".into()], conflicts: vec!["c.rival".into()], ..manifest("a.needs") };
        std::fs::write(d.join(MANIFEST), m.to_text()).unwrap();
        pack(&packs, "c.rival", &[]);
        let mut profile = Profile { packs_dir: Some(packs), ..Profile::default() };
        profile.put_on_top("c.rival");
        profile.put_on_top("a.needs");
        let s = Stack::from_profile(profile, None);
        assert_eq!(
            s.problems,
            ["a.needs needs b.base, which is not on", "a.needs conflicts with c.rival, which is on"]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn checks_the_layout() {
        let root = scratch("check");
        let d =
            pack(&root, "a.p", &[("tuning/x.ron", ""), ("notes.txt", ""), ("README.md", ""), ("textures/a.png", "")]);
        let p = Pack::open(&d).unwrap();
        let found = p.check();
        let has = |sev: Severity, text: &str| found.iter().any(|(s, t)| *s == sev && t.contains(text));
        assert!(has(Severity::Error, "kind must be Gameplay"), "{found:?}");
        assert!(has(Severity::Warning, "notes.txt"), "{found:?}");
        assert!(has(Severity::Error, "without textures.txt"), "{found:?}");
        assert!(!found.iter().any(|(_, t)| t.contains("README")), "{found:?}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn profile_paths_are_its_folders() {
        let root = scratch("profile");
        let path = root.join("profile.ron");
        let p = Profile {
            data: Some("game/usa".into()),
            exe: Some(root.join("QWC.exe")),
            packs: vec![Entry { id: "a.b".into(), enabled: false }],
            ..Profile::default()
        };
        p.write(&path).unwrap();
        let back = Profile::read(&path).unwrap();
        assert_eq!(back.data, Some(root.join("game/usa")));
        assert_eq!(back.exe, Some(root.join("QWC.exe")));
        assert_eq!(back.packs, p.packs);
        let _ = std::fs::remove_dir_all(root);
    }
}
