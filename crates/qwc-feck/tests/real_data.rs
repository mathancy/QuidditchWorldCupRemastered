//! Load every `.vso` on the disc into one VM. Runs only when `QWC_DATA_DIR` points at a repo root holding
//! `original/cd_files/`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use qwc_assets::Vfs;
use qwc_feck::frontend::FrontEnd;
use qwc_feck::layer::Kind;
use qwc_feck::layout::Metrics;
use qwc_feck::text::Strings;
use qwc_feck::{Error, Host, Result, Vm};
use qwc_formats::ffn::Ffn;
use qwc_formats::sts::{Sdx, Sts};

/// Every native exists and returns 0.
struct Stub;

impl Host for Stub {
    fn has_native(&self, _: u32) -> bool {
        true
    }
    fn call(&mut self, _: &mut Vm, _: u32, _: &[u32], _: u32) -> Result<u32> {
        Ok(0)
    }
}

/// Puppets' clip sets: `name.fas` (decoded, with `name.btf`'s triggers) and `name.skl` in the resource group's
/// package. The original takes the package from the exe's load-set table; here a group's folder is the one holding
/// the scripts `Screens.lst` lists under that group, and for a group only load sets register, a file name found
/// once on the disc (`AnimMed` of `FE_QuidCard_QuidCard`, in four folders, stays unresolved).
fn anim_sets(vfs: &Vfs) -> qwc_feck::puppet::SetLoader {
    use std::collections::HashMap;
    let split = |p: &str| {
        let p = p.to_lowercase().replace('\\', "/");
        let (dir, file) =
            p.rsplit_once('/').map_or((String::new(), p.clone()), |(d, f)| (d.to_string(), f.to_string()));
        (dir, file)
    };
    let mut sets: HashMap<(String, String), qwc_feck::puppet::PuppetSet> = HashMap::new();
    let mut dirs: HashMap<String, String> = HashMap::new();
    for f in vfs.with_extension("fas") {
        let b = vfs.read(f).unwrap();
        let fas = qwc_formats::fas::Fas::parse(&b).unwrap();
        let btf_path = format!("{}.btf", &f[..f.len() - 4]);
        let btf = vfs.exists(&btf_path).then(|| qwc_formats::btf::Btf::parse(&vfs.read(&btf_path).unwrap()).unwrap());
        // cutscene sets stream from `.sfa` files the front end never plays
        let Ok(clips) = qwc_anim::control::decode_set(&fas, btf.as_ref(), None) else { continue };
        let pose = qwc_assets::anim::set_skeleton(vfs, f, &fas).ok().map(|(_, s)| qwc_anim::Rig::new(s).bind_pose());
        sets.insert(split(f), qwc_feck::puppet::PuppetSet { clips, pose });
    }
    let vsos: HashMap<String, String> = vfs.with_extension("vso").map(|f| (split(f).1, split(f).0)).collect();
    let defs = qwc_formats::flow::parse_screens(&vfs.read("frontend/ScrnLst/Screens.lst").unwrap()).unwrap();
    for l in defs.iter().flat_map(|s| &s.layers) {
        if let Some(d) = vsos.get(&l.file.to_lowercase()) {
            dirs.entry(l.group.to_lowercase()).or_insert(d.clone());
        }
    }
    // groups only load sets register (no layer script in them): a file name found once on the disc
    let mut by_name: HashMap<String, Vec<String>> = HashMap::new();
    for (dir, file) in sets.keys() {
        by_name.entry(file.clone()).or_default().push(dir.clone());
    }
    Box::new(move |_: u32, r: &qwc_formats::vso::Resource| {
        let (group, name) = (r.group.as_str(), r.name.as_str());
        let file = format!("{}.fas", name.to_lowercase());
        let dir = match dirs.get(&group.to_lowercase()) {
            Some(d) => d.clone(),
            None => match by_name.get(&file).map(Vec::as_slice) {
                Some([d]) => d.clone(),
                _ => return None,
            },
        };
        sets.get(&(dir, file)).cloned()
    })
}

/// What drawing needs, from the disc: textures through `qwc_assets::frontend::Resources` (their sizes from the
/// `.fsh` headers, read as first asked for); fonts as the screens test loads them.
struct Assets<'a> {
    vfs: &'a Vfs,
    sizes: std::cell::RefCell<HashMap<String, Option<[u16; 2]>>>,
    resources: qwc_assets::frontend::Resources,
    fonts: &'a HashMap<u32, Ffn<'a>>,
    missing: std::cell::RefCell<std::collections::BTreeSet<u32>>,
}

use std::collections::HashMap;

impl<'a> Assets<'a> {
    fn new(vfs: &'a Vfs, scripts: &[Vec<u8>], fonts: &'a HashMap<u32, Ffn<'a>>) -> Assets<'a> {
        let resources = qwc_assets::frontend::Resources::build(vfs, scripts.iter().map(Vec::as_slice));
        Assets { vfs, sizes: Default::default(), resources, fonts, missing: Default::default() }
    }
}

/// The wipes the screens name (`_ResourceData` type 5), by resource id, for `LayerObj_ActivateWipe`.
fn wipe_files(vfs: &Vfs, r: &qwc_assets::frontend::Resources) -> HashMap<u32, std::rc::Rc<qwc_feck::wipe::WipeData>> {
    let wipe = |p: &str| qwc_feck::wipe::WipeData::new(&vfs.read(p).ok()?).map(std::rc::Rc::new);
    r.paths.iter().filter(|(id, _)| r.kinds.get(id) == Some(&5)).filter_map(|(&id, p)| Some((id, wipe(p)?))).collect()
}

impl qwc_feck::draw::Assets for Assets<'_> {
    fn texture(&self, tex: qwc_feck::draw::Tex) -> Option<[u16; 2]> {
        let size = self.resources.path_of(tex.script, tex.resource, tex.frame).and_then(|path| {
            *self.sizes.borrow_mut().entry(path.to_string()).or_insert_with(|| {
                let d = self.vfs.read(path).ok()?;
                let f = qwc_formats::fsh::Fsh::parse(&d).ok()?;
                f.images.first().map(|img| [img.width() as u16, img.height() as u16])
            })
        });
        if size.is_none() {
            self.missing.borrow_mut().insert(tex.resource);
        }
        size
    }
    fn font(&self, font: u32) -> Option<&dyn qwc_feck::layout::Metrics> {
        self.fonts.get(&font).map(|f| f as &dyn qwc_feck::layout::Metrics)
    }
}

/// Totals over the screens' draw lists.
#[derive(Debug, Default)]
struct Draws {
    sprites: usize,
    on_screen: usize,
    runs: usize,
    models: usize,
    puppets: usize,
    glyphs: usize,
    /// Positions or colours not finite, or quads with a texture coordinate past one texture.
    bad: usize,
}

impl Draws {
    fn add(&mut self, list: &qwc_feck::draw::DrawList, fe: &FrontEnd, assets: &Assets) {
        use qwc_feck::draw::{sprite_quad, text_quads, Assets as _};
        self.sprites += list.sprites.len();
        self.runs += list.texts.len();
        self.models += list.models.len();
        self.puppets += list.puppets.len();
        for s in &list.sprites {
            let q = sprite_quad(s, assets.texture(s.tex).unwrap(), None);
            let finite = q.iter().all(|v| v.pos.iter().all(|x| x.is_finite()));
            let uv_ok = q.iter().all(|v| (0..=0x1fff).contains(&v.uv[0]) && (0..=0x1fff).contains(&v.uv[1]));
            self.bad += (!finite || !uv_ok || s.colour.iter().any(|c| !c.is_finite())) as usize;
            let inside = |v: f32| (-64.0..=576.0).contains(&v);
            self.on_screen += q.iter().any(|v| inside(v.pos[0]) && inside(v.pos[1])) as usize;
        }
        for t in &list.texts {
            self.bad += t.pos.iter().chain(&t.colour).any(|x| !x.is_finite()) as usize;
            if let (Some(font), Some(text)) = (assets.fonts.get(&t.font), fe.text(t.object)) {
                let quads = text_quads(t, text, font, [font.tex_w, font.tex_h]);
                self.bad += quads.iter().flatten().any(|v| v.uv.iter().any(|&u| !(-1..=0x1001).contains(&u))) as usize;
                self.glyphs += quads.len();
            }
        }
    }
}

fn data() -> Option<Vfs> {
    let root = std::env::var_os("QWC_DATA_DIR").map(PathBuf::from)?;
    Some(Vfs::open(root.join("original/cd_files")).unwrap())
}

#[test]
fn every_script_loads_and_runs_against_stubs() {
    let Some(vfs) = data() else {
        eprintln!("QWC_DATA_DIR not set; skipping");
        return;
    };
    let files: Vec<String> = vfs.with_extension("vso").map(str::to_string).collect();
    assert_eq!(files.len(), 110);
    let mut vm = Vm::new();
    for f in &files {
        vm.load(f, &vfs.read(f).unwrap()).unwrap();
    }
    vm.link(&Stub);
    assert!(vm.unresolved().is_empty());
    let (mut ok, mut failed) = (0, 0);
    for s in 0..vm.scripts.len() {
        let fns: Vec<u32> = vm.scripts[s].exports.iter().filter(|e| e.function).map(|e| e.hash).collect();
        for hash in fns {
            match vm.execute(&mut Stub, s, hash, &[0; 8]) {
                Ok(_) => ok += 1,
                Err(_) => failed += 1,
            }
        }
    }
    eprintln!("stub host: {ok} functions ran, {failed} stopped");
    assert!(ok > 2800);
}

/// The DataStore is built by `InitialiseDataStore`, then every screen's layers are built and their objects'
/// construct handlers run against the front end's natives.
#[test]
fn screens_construct_with_the_front_end() {
    let Some(vfs) = data() else {
        eprintln!("QWC_DATA_DIR not set; skipping");
        return;
    };
    let files: Vec<String> = vfs.with_extension("vso").map(str::to_string).collect();
    let mut vm = Vm::new();
    let mut fe = FrontEnd::new();
    fe.anim_sets = Some(anim_sets(&vfs));
    let mut bytes = Vec::new();
    for f in &files {
        let b = vfs.read(f).unwrap().to_vec();
        vm.load(f, &b).unwrap();
        bytes.push(b);
    }
    vm.link(&fe);
    fe.learn_names(&vm);
    let sdx = Sdx::parse(&vfs.read("language/INDEX.SDX").unwrap()).unwrap();
    fe.strings = Some(Strings::new(&sdx, Sts::parse(&vfs.read("language/US/COMMON.STS").unwrap()).unwrap()));
    let init = vm.scripts.iter().position(|s| s.name.to_lowercase().contains("initialisedatastore")).unwrap();
    // `CreateDataStore(mask)` creates the entries of `g_DataStoreEntries` whose flags match; then the defaults
    for (f, args) in [("CreateDataStore", vec![u32::MAX]), ("InitialiseDataStore", vec![])] {
        vm.execute(&mut fe, init, qwc_formats::flow::name_hash(f), &args).unwrap();
    }
    eprintln!("DataStore: {} entries", fe.datastore.len());
    assert!(fe.datastore.len() > 20);
    let (mut built, mut ran, mut errors) = (0, 0, BTreeMap::<String, usize>::new());
    let (mut texts, mut shown, mut samples, mut unshown) = (0, 0, Vec::new(), BTreeMap::<String, usize>::new());
    // the fonts the screens name (`_ResourceData` type 1), from the Latin set
    let mut font_bytes = std::collections::HashMap::new();
    for b in &bytes {
        for r in qwc_formats::vso::Vso::parse(b).unwrap().resources().unwrap() {
            if r.kind == 1 {
                if let Ok(d) = vfs.read(&format!("frontend/Common/Fonts/Latin/{}.ffn", r.name)) {
                    font_bytes.insert(r.id, d.to_vec());
                }
            }
        }
    }
    let fonts: std::collections::HashMap<u32, Ffn> =
        font_bytes.iter().map(|(id, d)| (*id, Ffn::parse(d).unwrap())).collect();
    let assets = Assets::new(&vfs, &bytes, &fonts);
    // a script's ids name its own records' files (71 ids name other files in other scripts)
    let (mut placed, mut others) = (0, Vec::new());
    for b in &bytes {
        let v = qwc_formats::vso::Vso::parse(b).unwrap();
        for r in v.resources().unwrap().iter().filter(|r| r.choices.is_none()) {
            let Some(p) = assets.resources.path_of(v.hash, r.id, 0) else { continue };
            placed += 1;
            let file = p.rsplit('/').next().unwrap_or(p).to_lowercase();
            if !file.starts_with(&r.name.to_lowercase()) {
                others.push(format!("{}: {p}", r.name));
            }
        }
    }
    eprintln!("records placed per script: {placed}, as another file: {others:?}");
    assert!(placed > 1800 && others.is_empty());
    fe.wipe_files = wipe_files(&vfs, &assets.resources);
    eprintln!("{} wipes", fe.wipe_files.len());
    // `black2white` and `clock_wipe` (`WHITE2BLACK` is on the disc, named by no script)
    assert_eq!(fe.wipe_files.len(), 2);
    let mut draws = Draws::default();
    let mut wiped = 0;
    let (mut laid, mut lines, mut outside, mut wrapped) = (0, 0, 0, 0);
    for (s, b) in bytes.iter().enumerate() {
        let l = fe.add_layer(&mut vm, s, b).unwrap();
        built += fe.layers[l].objects.len();
        let init = qwc_formats::flow::name_hash("__Initialise");
        if vm.scripts[s].export(init).is_some() {
            if let Err(e) = vm.execute(&mut fe, s, init, &[]) {
                *errors.entry(format!("__Initialise: {e}")).or_default() += 1;
            }
        }
        let constructed = fe.construct(&mut vm, l);
        if let Err(e) = fe.pump(&mut vm) {
            fe.queue.clear();
            *errors.entry(format!("highlight handlers: {e}")).or_default() += 1;
        }
        match constructed {
            Ok(n) => ran += n,
            Err(e) => {
                let kind = match e {
                    Error::Fault(f) if f.addr < 0x1000 => "fault near 0".to_string(),
                    other => other.to_string(),
                };
                *errors.entry(kind).or_default() += 1;
            }
        }
        for _ in 0..48 {
            if fe.process(&mut vm, l).is_err() {
                break;
            }
        }
        // what each text object would show once drawn
        fe.format_texts(&vm.mem, l);
        // the layer drawn twice (the second frame has the centred sprites' textures bound)
        wiped += fe.draw_screen(&[l], &mut vec![], &assets).wipes.len();
        let list = fe.draw_screen(&[l], &mut vec![], &assets);
        wiped += list.wipes.len();
        draws.add(&list, &fe, &assets);
        for &i in &fe.layers[l].objects {
            if fe.objects[i].kind != Kind::Text || fe.objects[i].slot(0xbc) == 0 {
                continue;
            }
            texts += 1;
            if fe.text(i).is_none() {
                let (id, index) = (fe.objects[i].slot(0xbc), fe.objects[i].slot(0xe8));
                let mut strings = fe.strings.clone().unwrap();
                let why = match strings.acquire(id, index, &fe.values(&vm.mem)) {
                    Ok(_) => "a DataStore entry is missing".to_string(),
                    Err(e) => format!("{e:?}"),
                };
                *unshown.entry(why).or_default() += 1;
            }
            if let (Some(t), Some(font)) = (fe.text(i), fonts.get(&fe.objects[i].slot(0xc0))) {
                let lay = fe.layout_text(i, font, &|id| fe.inline_extent(l, id, &assets)).unwrap();
                laid += 1;
                lines += lay.lines.len();
                wrapped +=
                    (lay.height > font.line_height() * (1 + t.iter().filter(|&&u| u == 10).count() as u16)) as usize;
                outside += lay.lines.iter().filter(|r| r.start as usize + r.count as usize > t.len()).count();
            }
            if let Some(t) = fe.text(i) {
                shown += 1;
                if samples.len() < 12 && t.len() > 3 {
                    samples.push(String::from_utf16_lossy(t));
                }
            }
        }
    }
    eprintln!("text objects with a string id: {texts}, showing text: {shown}; for example {samples:?}");
    eprintln!("not shown: {unshown:?}");
    eprintln!(
        "{} fonts; laid out {laid} texts in {lines} lines ({wrapped} wrapped); {outside} lines past the text",
        fonts.len()
    );
    assert!(laid * 10 > shown * 9);
    assert!(shown * 10 > texts * 9);
    let sprites = fe.objects.iter().filter(|o| matches!(o.kind, Kind::Sprite | Kind::AnimatedSprite));
    let (all, hidden) = sprites.fold((0, 0), |(a, h), o| (a + 1, h + (o.flags & 0xc != 0) as usize));
    let missing = assets.missing.borrow();
    eprintln!("drawn: {draws:?}; of {all} sprite objects ({hidden} hidden); {} textures not found", missing.len());
    assert!(draws.sprites > 1000 && draws.runs > 500 && draws.glyphs > 5000, "{draws:?}");
    assert_eq!(draws.bad, 0, "{draws:?}");
    assert!(draws.on_screen * 10 > draws.sprites * 8, "{draws:?}");
    let mut missing: Vec<_> = fe.missing.iter().collect();
    missing.sort_by(|a, b| b.1.cmp(a.1));
    eprintln!("{built} objects; {ran} construct handlers ran; layer errors {errors:?}");
    eprintln!(
        "{} log lines, {} audio events, {} flow requests",
        fe.log.len(),
        fe.audio_events.len(),
        fe.requests.len()
    );
    eprintln!("natives not ported, by calls: {:?}", &missing[..missing.len().min(40)]);
    // the screens that start a wipe as they construct (the main menu, the team selects) draw it
    eprintln!("wipe records drawn: {wiped}");
    assert!(wiped >= 4);
    assert!(!fe.missing.contains_key("LayerObj_ActivateWipe"));
    // puppets: every PUPT whose animation resource names a .fas gets its clips, and the construct handlers'
    // plays leave clips queued
    let playing = fe
        .puppets
        .values()
        .filter(|p| p.character.control.state.layers.iter().any(|l| !l.channels[0].queue.is_empty()))
        .count();
    eprintln!("puppets: {} made, {playing} playing; clips not found: {:?}", fe.puppets.len(), fe.missing_clips);
    assert!(fe.puppets.len() >= 40, "{} puppets", fe.puppets.len());
    // ten seconds of the animated objects: clip ends call the scripts back
    let mut errors = 0;
    for _ in 0..240 {
        errors += fe.anim_frame(&mut vm).is_err() as u32;
    }
    eprintln!("after 10 s: {} puppet callbacks ran, {errors} failed", fe.puppet_callbacks);
    assert!(fe.puppet_callbacks > 0);
    // and their poses: every puppet with a skeleton has its nodes set, finite, by what it plays
    let posed: Vec<_> = fe.puppets.values().filter_map(|p| p.character.control.pose.as_ref()).collect();
    let rotated = posed.iter().filter(|p| p.rotation.iter().any(Option::is_some)).count();
    let faces = posed.iter().filter(|p| p.morphs.iter().flatten().any(|&w| w != 0.0)).count();
    eprintln!("poses: {} puppets with skeletons, {rotated} posed, {faces} with morph weights", posed.len());
    assert!(rotated >= 30, "{rotated} posed");
    for p in &posed {
        let finite = |v: &[f32; 3]| v.iter().all(|x| x.is_finite());
        assert!(p.rotation.iter().flatten().all(finite) && p.translation.iter().all(finite));
        assert!(p.morphs.iter().flatten().all(|w| w.is_finite()));
    }
    assert!(built > 3000 && ran > 500);
}

/// The flow from boot: every script loaded, the DataStore built, then the screen manager runs frames from
/// `PreFlow_XboxSkip`. Every 1.5 seconds pad 0's choose button is pressed (`qwc_feck::pad`).
#[test]
fn the_flow_runs_from_boot() {
    use qwc_feck::screens::{Manager, Step};
    let Some(vfs) = data() else {
        eprintln!("QWC_DATA_DIR not set; skipping");
        return;
    };
    let files: Vec<String> = vfs.with_extension("vso").map(str::to_string).collect();
    let mut vm = Vm::new();
    let mut fe = FrontEnd::new();
    fe.anim_sets = Some(anim_sets(&vfs));
    let mut scripts = Vec::new();
    for (i, f) in files.iter().enumerate() {
        let b = vfs.read(f).unwrap().to_vec();
        vm.load(f, &b).unwrap();
        scripts.push((i, b));
    }
    vm.link(&fe);
    fe.learn_names(&vm);
    let sdx = Sdx::parse(&vfs.read("language/INDEX.SDX").unwrap()).unwrap();
    fe.strings = Some(Strings::new(&sdx, Sts::parse(&vfs.read("language/US/COMMON.STS").unwrap()).unwrap()));
    let init = vm.scripts.iter().position(|s| s.name.to_lowercase().contains("initialisedatastore")).unwrap();
    for (f, args) in [("CreateDataStore", vec![u32::MAX]), ("InitialiseDataStore", vec![])] {
        vm.execute(&mut fe, init, qwc_formats::flow::name_hash(f), &args).unwrap();
    }
    let flow = qwc_formats::flow::Flow::parse(&vfs.read("frontend/ScrnLst/game.flo").unwrap()).unwrap();
    let defs = qwc_formats::flow::parse_screens(&vfs.read("frontend/ScrnLst/Screens.lst").unwrap()).unwrap();
    let bytes: Vec<Vec<u8>> = scripts.iter().map(|(_, b)| b.clone()).collect();
    let mut m = Manager::new(flow, defs, &vm, scripts);
    m.start(m.boot_node().unwrap());
    for frame in 0..24 * 120 {
        // choose on pad 0 every 1.5 s, through the controllers: its press, then next tick its release (input
        // button 1 with flags 1, which the title screen's `Root_OnInput` waits for, and choose)
        fe.pads.words[0][qwc_feck::pad::PadWord::Choose as usize] = (frame % 36 == 35) as u16;
        m.frame(&mut fe, &mut vm);
    }
    let name = |n: usize| m.flow.nodes[n].name.clone().unwrap_or(format!("{n}"));
    let visited: Vec<String> =
        m.steps.iter().filter_map(|s| if let Step::Entered(n) = s { Some(name(*n)) } else { None }).collect();
    let errors: Vec<&Step> = m.steps.iter().filter(|s| matches!(s, Step::Error(_) | Step::Refused(_))).collect();
    eprintln!("screens entered: {visited:?}");
    eprintln!("now at {}, screen state {:?}", name(m.node), m.current_screen().map(|s| s.state));
    eprintln!("errors and refusals: {errors:?}");
    eprintln!("live layers: {:?}", fe.layers.iter().filter(|l| l.alive).map(|l| &l.name).collect::<Vec<_>>());
    eprintln!("last log lines: {:?}", &fe.log[fe.log.len().saturating_sub(8)..]);
    eprintln!("natives not ported, by calls: {:?}", fe.missing);
    eprintln!("puppets made: {}", fe.puppets.len());
    // the frame's draw: the current screen (and any leaving or coming) through the walk
    let mut font_bytes = std::collections::HashMap::new();
    for b in &bytes {
        for r in qwc_formats::vso::Vso::parse(b).unwrap().resources().unwrap() {
            if let (1, Ok(d)) = (r.kind, vfs.read(&format!("frontend/Common/Fonts/Latin/{}.ffn", r.name))) {
                font_bytes.insert(r.id, d.to_vec());
            }
        }
    }
    let fonts: std::collections::HashMap<u32, Ffn> =
        font_bytes.iter().map(|(id, d)| (*id, Ffn::parse(d).unwrap())).collect();
    let assets = Assets::new(&vfs, &bytes, &fonts);
    m.draw(&mut fe, &vm, &assets);
    let list = m.draw(&mut fe, &vm, &assets);
    eprintln!("the frame: {} sprites, {} text runs, camera {:?}", list.sprites.len(), list.texts.len(), list.camera);
    assert!(!list.sprites.is_empty() && !list.texts.is_empty());
    if let Some(c) = list.camera {
        let v = fe.camera_view(c);
        assert!(v.eye.iter().chain(&v.dir).chain(&v.up).all(|x| x.is_finite()) && v.near < v.far, "{v:?}");
    }
    assert!(visited.len() >= 2);
    // the memory card (an empty one in memory): the start-up checks find no save and the flow goes on to the
    // start screen; every saved key is in the DataStore, so the slots have a size
    assert!(visited.iter().any(|n| n == "StartScreen"), "{visited:?}");
    let slot = qwc_feck::memcard::slot_size(&fe.datastore, &qwc_feck::memcard::SAVE_KEYS);
    eprintln!("save slot size {slot}, buffer {} bytes", fe.memcard.buffer.len());
    assert!(slot > 0);
    assert_eq!(fe.memcard.buffer.len() as u32, (slot + 8) * qwc_feck::memcard::SLOTS + 0x14);
    // a save the original game wrote on this machine, when there is one, has our size, CRC and schema
    use qwc_feck::memcard::{is_bad, CardIo, DirCard, GAME_NAME};
    if let Some(mut card) = DirCard::documents(GAME_NAME).filter(|c| c.file.exists()) {
        let saved = card.read(usize::MAX).unwrap();
        eprintln!("checking the original save {}", card.file.display());
        assert_eq!(saved.len(), fe.memcard.buffer.len());
        assert!(!is_bad(&saved, &fe.memcard.buffer));
    }
}
