//! `qwc-game pack`: the resource packs from the command line (`qwc_packs`, `docs/LAUNCHER.md`), until the launcher
//! and QWC Studio do it. Not the original's.
//!
//! - `list`: the profile, the packs installed in its order (on or off), what each on fills, where they overlap;
//! - `check DIR`: a pack folder's layout and its files (the texture list, the fonts' list, the looks, the HUD
//!   scenes, the stadiums);
//! - `build DIR [--out FILE|DIR]`: the folder zipped into a `.qwcpack` (beside the folder by default);
//! - `install FILE`: a `.qwcpack` unpacked into the packs' folder, on and at the top of the profile;
//! - `new DIR ID NAME`: a pack folder with its manifest;
//! - `enable ID`, `disable ID`, `top ID`: the profile's order;
//! - `migrate [FROM]`: a development `remastered/` folder made into packs: `local.remastered` (textures, movies,
//!   screen layouts, HUD scenes, looks, fonts) on, and `local.hogsum-test` (the stadiums) off. The folder is
//!   copied, not moved.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use qwc_packs::{Kind, Manifest, Pack, Profile, Severity};

use crate::{fail, usage, Args};

pub fn cli(args: &Args) {
    let (sub, rest) = args.models.split_first().unwrap_or_else(|| usage());
    match (sub.as_str(), rest) {
        ("list", []) => list(),
        ("check", [dir]) => {
            if !check(Path::new(dir), data(args).as_ref()) {
                std::process::exit(1)
            }
        }
        ("build", [dir]) => build(Path::new(dir), args.out.as_deref(), data(args).as_ref()),
        ("install", [file]) => install(Path::new(file)),
        ("new", [dir, id, name @ ..]) if !name.is_empty() => new(Path::new(dir), id, &name.join(" ")),
        ("enable" | "disable" | "top", [id]) => order(sub, id),
        ("migrate", from) if from.len() <= 1 => migrate(Path::new(from.first().map_or("remastered", |f| f.as_str()))),
        _ => usage(),
    }
}

/// The profile to change and where it is written: the one read at start, else a new one at the default place.
fn profile() -> (PathBuf, Profile) {
    let stack = qwc_packs::current();
    let path = stack
        .profile_path
        .clone()
        .or_else(Profile::default_path)
        .unwrap_or_else(|| fail("no profile: give --profile FILE (no library folder in this environment)"));
    (path, stack.profile.clone())
}

fn save(path: &Path, profile: &Profile) {
    profile.write(path).unwrap_or_else(|e| fail(e));
    println!("profile: {}", path.display());
}

fn packs_dir(profile: &Profile) -> PathBuf {
    profile.packs_dir().unwrap_or_else(|| fail("no packs folder: set packs_dir in the profile"))
}

/// What each kind of slot is called in the list (`texture/...` -> textures).
fn slot_counts(slots: &[String]) -> String {
    let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
    for s in slots {
        *counts.entry(s.split('/').next().unwrap_or(s)).or_default() += 1;
    }
    let parts: Vec<String> = counts.iter().map(|(k, n)| format!("{n} {k}")).collect();
    if parts.is_empty() {
        "nothing the remaster reads".into()
    } else {
        parts.join(", ")
    }
}

fn list() {
    let stack = qwc_packs::current();
    match &stack.profile_path {
        Some(p) => println!("profile: {}", p.display()),
        None => {
            let at = Profile::default_path().map_or("(no library folder)".into(), |p| p.display().to_string());
            println!("no profile, so no packs (the default is {at})");
        }
    }
    let Some(dir) = stack.profile.packs_dir() else { return };
    println!("packs: {}", dir.display());
    let (installed, errors) = stack.profile.installed();
    let mut shown = Vec::new();
    for e in &stack.profile.packs {
        let state = if e.enabled { "on " } else { "off" };
        match installed.get(&e.id) {
            Some(p) => {
                let m = &p.manifest;
                let kind = if m.kind == Kind::Gameplay { " [Gameplay]" } else { "" };
                println!("  {state} {} {} \"{}\"{kind}: {}", m.id, m.version, m.name, slot_counts(&p.slots()));
            }
            None => println!("  {state} {} (not installed)", e.id),
        }
        shown.push(e.id.as_str());
    }
    for (id, p) in installed.iter().filter(|(id, _)| !shown.contains(&id.as_str())) {
        println!("  off {id} {} \"{}\" (not in the profile)", p.manifest.version, p.manifest.name);
    }
    let overlaps = stack.overlaps();
    if !overlaps.is_empty() {
        println!("overlaps (the first pack wins):");
        for o in overlaps {
            println!("  {}: {}", o.slot, o.packs.join(" over "));
        }
    }
    for p in errors.iter().chain(&stack.problems) {
        println!("problem: {p}");
    }
}

/// The game data, for the checks that read the game's models (none when it does not open: those are left out).
fn data(args: &Args) -> Option<qwc_assets::Vfs> {
    qwc_assets::Vfs::open(&args.data).ok()
}

/// The pack's models read as the game reads them (`crate::models`): each must stand for a model the game has, read,
/// and for a skinned one have joints the skeleton has bones of.
pub fn model_findings(pack: &Pack, vfs: &qwc_assets::Vfs) -> Vec<(Severity, String)> {
    let mut found = Vec::new();
    let textures = qwc_assets::TextureIndex::build(vfs);
    for path in pack.model_paths() {
        let Some(file) = pack.model(&path) else { continue };
        let original = match qwc_assets::model::load(vfs, &textures, &path) {
            Ok(m) => m,
            Err(_) => {
                found.push((
                    Severity::Warning,
                    format!("{}: the game has no model {path} to stand in for", file.display()),
                ));
                continue;
            }
        };
        let read = match &original.skeleton {
            Some((skl, _)) => qwc_assets::glb::load(vfs, &textures, &file, skl, &path, true),
            None => qwc_assets::glb::load_scenery(&file),
        };
        match read {
            Ok((_, report)) => {
                if !report.unmatched_joints.is_empty() {
                    found.push((
                        Severity::Warning,
                        format!(
                            "{}: joints the skeleton has no bone of (their vertices ride the root): {}",
                            file.display(),
                            report.unmatched_joints.join(", ")
                        ),
                    ));
                }
                if !report.unnamed_morphs.is_empty() {
                    found.push((
                        Severity::Warning,
                        format!(
                            "{}: morph targets not named morphNN (the weight channel), left out: {}",
                            file.display(),
                            report.unnamed_morphs.join(", ")
                        ),
                    ));
                }
                for m in &report.skipped_materials {
                    found.push((
                        Severity::Warning,
                        format!("{}: material {m} has no texture: not drawn", file.display()),
                    ));
                }
            }
            Err(e) => found.push((Severity::Error, format!("{}: {e}", file.display()))),
        }
    }
    found
}

/// What is wrong with `pack`: its layout (`Pack::check`) and its files read as the game reads them (the font list
/// and its fonts, the texture list's PNGs, the looks, the stadiums' scenery, and with the game data `vfs` the
/// models). Used by `check` and QWC Studio.
pub fn findings(pack: &Pack, vfs: Option<&qwc_assets::Vfs>) -> Vec<(Severity, String)> {
    let dir = &pack.dir;
    let mut found = pack.check();
    match vfs {
        Some(v) if !pack.model_paths().is_empty() => found.extend(model_findings(pack, v)),
        Some(_) => {}
        None if !pack.model_paths().is_empty() => {
            found.push((Severity::Warning, "the models are not checked: no game data (--data)".into()))
        }
        None => {}
    }
    if let Some(list) = pack.part("fonts.txt") {
        match qwc_text::FontList::read(&list) {
            Ok(l) => {
                for f in l.families.values().flat_map(|(u, i)| std::iter::once(u).chain(i)).chain(&l.fallback) {
                    if !f.is_file() {
                        found.push((Severity::Error, format!("fonts.txt: {}: no such font file", f.display())));
                    }
                }
            }
            Err(e) => found.push((Severity::Error, e)),
        }
    }
    if let Some(list) = pack.part("textures/textures.txt") {
        if let Ok(r) = qwc_assets::remaster::Remastered::read(&list) {
            for t in r.textures() {
                if let Some(png) = r.png(t).filter(|p| !p.is_file()) {
                    found.push((Severity::Error, format!("{}: no such PNG (for {t})", png.display())));
                }
            }
        }
    }
    // the tuning scripts as the match's console runs them: a name it does not know is a mistake
    let tuning = dir.join("tuning");
    let teams = std::fs::read_dir(tuning.join("teams")).into_iter().flatten().flatten().map(|e| e.path());
    for folder in std::iter::once(tuning.clone()).chain(teams) {
        for f in std::fs::read_dir(&folder).into_iter().flatten().flatten().map(|e| e.path()) {
            if !f.extension().is_some_and(|e| e.eq_ignore_ascii_case("cli")) {
                continue;
            }
            match std::fs::read(&f) {
                Ok(b) => {
                    for e in qwc_sim::Console::new().run_script(&b) {
                        found.push((Severity::Error, format!("{}: {e}", f.display())));
                    }
                }
                Err(e) => found.push((Severity::Error, format!("{}: {e}", f.display()))),
            }
        }
    }
    // the shaders as the renderer checks them: one that does not compile would be drawn by the stock shader
    if let Some(list) = pack.part("shaders").and_then(|d| Some((qwc_packs::ShaderList::read(&d).ok()?, d))) {
        let (list, sdir) = list;
        let materials = list.materials.iter().map(|m| (m.file.as_str(), true));
        for (file, material) in materials.chain(list.post.iter().map(|p| (p.file.as_str(), false))) {
            let Some(path) = qwc_packs::Stack::shader_file(&sdir, file) else { continue };
            let checked = std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|s| {
                if material {
                    qwc_render::custom::check_material(&s)
                } else {
                    qwc_render::custom::check_post(&s)
                }
            });
            if let Err(e) = checked {
                found.push((Severity::Error, format!("shaders/{file}: {e}")));
            }
        }
    }
    for look in std::fs::read_dir(dir.join("looks")).into_iter().flatten().flatten().map(|e| e.path()) {
        if let Err(e) = std::fs::read_to_string(&look).map_err(|e| e.to_string()).and_then(|t| crate::looks::parse(&t))
        {
            found.push((Severity::Error, format!("{}: {e}", look.display())));
        }
    }
    // the HUD scenes: each read as the game reads it, standing in for a stock scene of its name
    for scene in std::fs::read_dir(dir.join("hud")).into_iter().flatten().flatten().map(|e| e.path()) {
        let read = std::fs::read_to_string(&scene).map_err(|e| e.to_string());
        if let Err(e) = read.and_then(|t| qwc_hud::Scene::from_ron(&t)) {
            found.push((Severity::Error, format!("{}: {e}", scene.display())));
        }
        let stem = scene.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if qwc_hud::built_in(&stem).is_none() {
            let names: Vec<&str> = qwc_hud::BUILT_IN.iter().map(|(n, _)| *n).collect();
            found.push((
                Severity::Warning,
                format!("{}: not a HUD scene the game draws ({})", scene.display(), names.join(", ")),
            ));
        }
    }
    for stadium in std::fs::read_dir(dir.join("stadiums")).into_iter().flatten().flatten().map(|e| e.path()) {
        let glbs = std::fs::read_dir(&stadium)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("glb")))
            .count();
        if glbs == 0 {
            found.push((Severity::Error, format!("{}: no .glb scenery", stadium.display())));
        } else if !stadium.join("stadium.glb").is_file() {
            found.push((Severity::Warning, format!("{}: no stadium.glb (the arena)", stadium.display())));
        }
    }
    found
}

/// The packs' shaders as the renderer takes them (`qwc_render::custom`), read once from the packs on: a file that
/// cannot be read is reported and left out (one that does not compile is reported by the renderer, which draws the
/// stock shader instead).
pub fn shaders() -> Arc<qwc_render::custom::Shaders> {
    static SHADERS: OnceLock<Arc<qwc_render::custom::Shaders>> = OnceLock::new();
    SHADERS.get_or_init(|| Arc::new(read_shaders(qwc_packs::current()))).clone()
}

/// [`shaders`] of `stack`.
pub fn read_shaders(stack: &qwc_packs::Stack) -> qwc_render::custom::Shaders {
    use qwc_render::custom::{MaterialShader, PostPass, Shaders, PARAMS};
    let read = |id: &str, dir: &Path, file: &str| -> Option<(String, String)> {
        let name = format!("{id}:shaders/{file}");
        let path = qwc_packs::Stack::shader_file(dir, file);
        match path.map(std::fs::read_to_string) {
            Some(Ok(source)) => Some((name, source)),
            Some(Err(e)) => {
                eprintln!("{name}: {e}");
                None
            }
            None => {
                eprintln!("{name}: no such file");
                None
            }
        }
    };
    let materials = stack
        .material_shaders()
        .into_iter()
        .filter_map(|(id, dir, m)| {
            let (name, source) = read(&id, &dir, &m.file)?;
            Some(MaterialShader { name, source, textures: m.textures, materials: m.materials })
        })
        .collect();
    let post = stack
        .post_passes()
        .into_iter()
        .filter_map(|(id, dir, p)| {
            let (name, source) = read(&id, &dir, &p.file)?;
            let mut params = [0.0; PARAMS];
            for (to, from) in params.iter_mut().zip(&p.params) {
                *to = *from;
            }
            Some(PostPass { name, source, params })
        })
        .collect();
    Shaders { materials, post }
}

/// The pack's findings printed; false when there is an error.
fn check(dir: &Path, vfs: Option<&qwc_assets::Vfs>) -> bool {
    let pack = Pack::open(dir).unwrap_or_else(|e| fail(e));
    let found = findings(&pack, vfs);
    let m = &pack.manifest;
    println!("{} {} \"{}\": {}", m.id, m.version, m.name, slot_counts(&pack.slots()));
    for (sev, text) in &found {
        println!("  {}: {text}", if *sev == Severity::Error { "error" } else { "warning" });
    }
    let ok = !found.iter().any(|(s, _)| *s == Severity::Error);
    println!("{}", if ok { "ok" } else { "not ok" });
    ok
}

fn build(dir: &Path, out: Option<&Path>, vfs: Option<&qwc_assets::Vfs>) {
    if !check(dir, vfs) {
        std::process::exit(1)
    }
    let beside = dir.canonicalize().ok().and_then(|d| d.parent().map(Path::to_path_buf));
    let out = out.map(Path::to_path_buf).or(beside).unwrap_or_else(|| ".".into());
    let (file, _) = qwc_packs::build(dir, &out).unwrap_or_else(|e| fail(e));
    let size = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
    println!("built {} ({:.1} MB)", file.display(), size as f64 / 1e6);
}

fn install(file: &Path) {
    let (path, mut profile) = profile();
    let pack = qwc_packs::install(file, &packs_dir(&profile)).unwrap_or_else(|e| fail(e));
    profile.put_on_top(&pack.manifest.id);
    println!("installed {} {} into {}, on at the top", pack.manifest.id, pack.manifest.version, pack.dir.display());
    save(&path, &profile);
}

fn new(dir: &Path, id: &str, name: &str) {
    if dir.join(qwc_packs::MANIFEST).exists() {
        fail(format!("{}: a pack is there already", dir.display()));
    }
    let p = Pack::create(dir, Manifest::new(id, name)).unwrap_or_else(|e| fail(e));
    println!("made {} in {}", p.manifest.id, dir.display());
}

fn order(what: &str, id: &str) {
    let (path, mut profile) = profile();
    if !profile.installed().0.contains_key(id) {
        fail(format!("{id}: not installed in {}", packs_dir(&profile).display()));
    }
    match what {
        "enable" => profile.set_enabled(id, true),
        "disable" => profile.set_enabled(id, false),
        _ => profile.put_on_top(id),
    }
    save(&path, &profile);
}

/// Copy `from` (a file or a folder) to `to`, answering the bytes copied.
fn copy_tree(from: &Path, to: &Path) -> u64 {
    if from.is_file() {
        if let Some(d) = to.parent() {
            std::fs::create_dir_all(d).unwrap_or_else(|e| fail(format!("{}: {e}", d.display())));
        }
        return std::fs::copy(from, to).unwrap_or_else(|e| fail(format!("{}: {e}", from.display())));
    }
    let mut n = 0;
    std::fs::create_dir_all(to).unwrap_or_else(|e| fail(format!("{}: {e}", to.display())));
    for e in std::fs::read_dir(from).unwrap_or_else(|e| fail(format!("{}: {e}", from.display()))).flatten() {
        n += copy_tree(&e.path(), &to.join(e.file_name()));
    }
    n
}

/// A new pack `id` in `packs` made of `parts` of `from` (source, destination in the pack), with a manifest.
fn migrate_pack(packs: &Path, id: &str, name: &str, description: &str, from: &Path, parts: &[(&str, &str)]) -> bool {
    let dest = packs.join(id);
    if dest.exists() {
        println!("{id}: {} is there already (delete it to migrate again)", dest.display());
        return false;
    }
    let present: Vec<&(&str, &str)> = parts.iter().filter(|(src, _)| from.join(src).exists()).collect();
    if present.is_empty() {
        println!("{id}: nothing in {} for it", from.display());
        return false;
    }
    let mut bytes = 0;
    for (src, dst) in present {
        bytes += copy_tree(&from.join(src), &dest.join(dst));
    }
    let m = Manifest {
        id: id.to_string(),
        name: name.to_string(),
        version: "1.0.0".into(),
        authors: vec![],
        description: description.to_string(),
        kind: Kind::Visual,
        engine: String::new(),
        source: None,
        requires: vec![],
        conflicts: vec![],
    };
    std::fs::write(dest.join(qwc_packs::MANIFEST), m.to_text()).unwrap_or_else(|e| fail(e));
    println!("{id}: {} ({:.0} MB)", dest.display(), bytes as f64 / 1e6);
    check(&dest, None);
    true
}

fn migrate(from: &Path) {
    if !from.is_dir() {
        fail(format!("{}: no such folder", from.display()));
    }
    let (path, mut profile) = profile();
    let packs = packs_dir(&profile);
    let main = migrate_pack(
        &packs,
        "local.remastered",
        "Local remaster",
        "Made from a development remastered/ folder: its textures, movies, screen layouts, HUD scenes, looks and \
         fonts. Made from the game's own files: keep it to yourself.",
        from,
        &[
            ("textures.txt", "textures/textures.txt"),
            ("pngs", "textures/pngs"),
            ("movies", "movies"),
            ("screens", "screens"),
            ("hud", "hud"),
            ("looks", "looks"),
            ("fonts.txt", "fonts.txt"),
            ("fonts", "fonts"),
        ],
    );
    let stadium = migrate_pack(
        &packs,
        "local.hogsum-test",
        "Hogwarts summer stadium (test)",
        "The remastered hog_sum stadium as the front end's background, made in Blender over the game's geometry: \
         a test, kept to yourself.",
        from,
        &[("stadiums", "stadiums")],
    );
    if main {
        profile.put_on_top("local.remastered");
    }
    if stadium {
        profile.packs.retain(|e| e.id != "local.hogsum-test");
        profile.packs.push(qwc_packs::Entry { id: "local.hogsum-test".into(), enabled: false });
    }
    if main || stadium {
        save(&path, &profile);
        println!("{} is left as it was; the game reads the packs now", from.display());
    }
}
