//! The front end to a PNG.
//!
//! - `qwc-game fe SCRIPT.vso --out FILE.png [--time SECS] [--size WxH]`: one front-end layer, built as the screen
//!   manager builds it (`__Initialise`, every object's construct), run for `--time` seconds at 24 Hz (2 by default).
//! - `qwc-game screen NODE|boot --out FILE.png [--time SECS] [--size WxH]`: the screen manager over `game.flo`
//!   started at a node (by name), or booted with pad 0's choose pressed every 1.5 seconds, run for `--time`
//!   seconds; then the frame's draw list (`Manager::draw`: the current, previous and next screens). It prints the
//!   frame each screen is reached on; a front-end cutscene ends at once; `QWC_BOOT_UNTIL=FRAME` stops the boot's
//!   presses there, for `QWC_PADS` to go on (`QWC_SAVE_DIR` puts the card in a folder). `QWC_FE_BLEND=T` draws
//!   the frame `T` of the way from the last tick but one to the last, as the window draws between ticks.
//!   `QWC_AUDIO_LOG=1` prints the audio events the scripts send, by screen, and whether the front end's score's
//!   package is loaded to hear them (`crate::audio`).
//! - `qwc-game frontend [NODE|boot]` runs it live in a window (`crate::live`).
//!
//! Either way the draw list (`qwc_feck::draw`) goes through the overlay pass: sprites and text by material layer.
//! When the frame has a camera object (`StadiumOrbit.vso`'s), the stadium is drawn under it through that camera's
//! view (`FrontEnd::camera_view`, [`Painter`]), with its hoops and its wavy flags (`qwc_render::flags`, waving by
//! the time since the window opened, or for a PNG the flow's time). **[I]** the stadium is the front end's Hogwarts
//! (`stadium/sum_fe`), whatever `@StadiumLoadSet` holds. The screens' models and puppets are drawn in the same
//! world (`crate::fe3d`); regions with their own camera or texture, and the Quidditch cards, are not drawn yet.
//!
//! The load-set table (`qwc_formats::flow::loadset_table`) is read from a decrypted `QWC.exe` when one is found
//! (`--exe`, `$QWC_EXE`, the data folder's, `analysis/QWC.exe`; the disc's own is encrypted): it gives
//! `General_LoadSetIDFromCRC` its slots and the resources their packages. Without it the resources fall back to
//! folder names (`qwc_assets::frontend`).
//!
//! Remastered textures (`qwc_assets::remaster`; the list `--textures`, default `remastered/textures.txt`;
//! `--disc-textures` turns them off) are drawn in place of the disc's: the scripts are told the disc texture's
//! size, so placing is the game's, and each PNG keeps its own shape inside the original's quad (the 512 x 512
//! screen is stretched over the window: the disc's art stretches with it, the remastered art does not).

use std::collections::HashSet;

use qwc_assets::frontend::Resources;
use qwc_assets::remaster::Remastered;
use qwc_assets::Vfs;
use qwc_feck::draw::{pack_colour, sprite_quad, text_quads, DrawList, Vertex};
use qwc_feck::frontend::EngineEvent;
use qwc_feck::pad::PadWord;
use qwc_feck::FrontEnd;
use qwc_formats::flow::name_hash;
pub(crate) use qwc_frontend::disc::{font_files, load_set, packages, wipe_files, Disc};
use qwc_frontend::session::{card_numbers, load, loadset_table, Loaded};
pub(crate) use qwc_frontend::session::{log_lines, report, Flow};
use qwc_render::flags;
use qwc_render::glam::Vec3;
use qwc_render::overlay::{Overlay, OverlayQuad, OverlayTarget, OverlayVertex, OVERLAY_FORMAT};
use qwc_render::{wgpu, Camera, Gpu, GpuModel, Renderer};

use crate::{fail, stadium, Args};

/// The vector fonts' list unless `--disc-fonts`: `--fonts` alone, else the packs' lists laid over one another
/// (`qwc_packs`). None without one: the disc's fonts are drawn.
pub(crate) fn remastered_fonts(args: &Args) -> Option<qwc_text::FontList> {
    if args.disc_fonts {
        return None;
    }
    if args.fonts_given {
        return fonts_from(std::slice::from_ref(&args.fonts), true);
    }
    pack_fonts()
}

/// The vector fonts' list as the game reads it by default: each pack's laid over the lower ones', the lowest first.
pub fn pack_fonts() -> Option<qwc_text::FontList> {
    fonts_from(&qwc_packs::current().font_lists(), false)
}

/// The fonts' lists at `paths` laid over one another (a missing one reported when it was asked for by name).
fn fonts_from(paths: &[std::path::PathBuf], named: bool) -> Option<qwc_text::FontList> {
    if let Some(p) = paths.iter().find(|p| !p.is_file()) {
        if named {
            eprintln!("{}: no such list; the disc's fonts are drawn", p.display());
        }
        return None;
    }
    if paths.is_empty() {
        return None;
    }
    match qwc_text::FontList::read_layered(paths) {
        Ok(l) => Some(l),
        Err(e) => {
            eprintln!("{e}; the disc's fonts are drawn");
            None
        }
    }
}

/// The remastered textures unless `--disc-textures`: the list `--textures` names, else the packs' lists laid over
/// each other (`qwc_packs`), their problems printed. None without any.
pub(crate) fn remastered_textures(args: &Args, vfs: &Vfs) -> Option<Remastered> {
    if args.disc_textures {
        return None;
    }
    if args.textures_given && !args.textures.is_file() {
        eprintln!("{}: no such list; the disc's textures are drawn", args.textures.display());
        return None;
    }
    let lists = if args.textures_given { vec![args.textures.clone()] } else { qwc_packs::current().texture_lists() };
    textures_from(&lists, Some(vfs))
}

/// The remastered textures as the game reads them by default: the packs' lists laid over each other.
pub fn pack_textures() -> Option<Remastered> {
    textures_from(&qwc_packs::current().texture_lists(), None)
}

/// The lists at `lists` laid over each other (the lowest first), their problems against `vfs` printed.
fn textures_from(lists: &[std::path::PathBuf], vfs: Option<&Vfs>) -> Option<Remastered> {
    if lists.is_empty() {
        return None;
    }
    match Remastered::read_layers(lists) {
        Ok(r) => {
            for p in vfs.map(|v| r.problems(v)).unwrap_or_default() {
                eprintln!("remastered textures: {p}");
            }
            let names: Vec<String> = lists.iter().map(|l| l.display().to_string()).collect();
            eprintln!("{} textures remastered ({})", r.len(), names.join(", "));
            Some(r)
        }
        Err(e) => {
            eprintln!("{e}; the disc's textures are drawn");
            None
        }
    }
}

/// Where a decrypted exe may be ([`qwc_frontend::session::exe_candidates`]: `--exe` first).
pub(crate) fn exes(args: &Args) -> Vec<std::path::PathBuf> {
    qwc_frontend::session::exe_candidates(args.exe.as_deref(), &args.data)
}

pub fn fe(args: &Args, vfs: &'static Vfs) {
    let out = args.out.clone().unwrap_or_else(|| fail("fe needs --out FILE.png"));
    let want = args.models[0].to_lowercase().replace('\\', "/");
    let exes = exes(args);
    let sets = loadset_table(&exes);
    let Loaded { mut vm, mut fe, files, bytes } = load(vfs, &sets).unwrap_or_else(|e| fail(e));
    fe.card_numbers = card_numbers(&exes);
    // the wipes and the puppets' clips before the layer builds and constructs (it may start a wipe)
    let early = Resources::build_in(vfs, bytes.iter().map(Vec::as_slice), |_, _| None, &packages(&sets));
    fe.wipe_files = wipe_files(vfs, &early);
    fe.anim_sets = Some(qwc_frontend::session::puppet_sets(vfs, early));
    let target = files
        .iter()
        .position(|f| f.to_lowercase().replace('\\', "/").ends_with(&want))
        .unwrap_or_else(|| fail(format!("no layer script {want}")));
    let l = fe.add_layer(&mut vm, target, &bytes[target]).unwrap_or_else(|e| fail(e));
    let init = name_hash("__Initialise");
    if vm.scripts[target].export(init).is_some() {
        if let Err(e) = vm.execute(&mut fe, target, init, &[]) {
            eprintln!("__Initialise: {e}");
        }
    }
    if let Err(e) = fe.construct(&mut vm, l) {
        eprintln!("construct: {e}");
    }
    let _ = fe.pump(&mut vm);
    for _ in 0..frames(args.time) {
        if let Err(e) = fe.process(&mut vm, l) {
            eprintln!("process: {e}");
            break;
        }
    }
    fe.format_texts(&vm.mem, l);

    let run = |s, f| vm.execute(&mut fe, s, f, &[]).ok();
    let resources = Resources::build_in(vfs, bytes.iter().map(|b| b.as_slice()), run, &packages(&sets));
    let font_bytes = font_files(vfs, &resources);
    let disc = Disc::shared(vfs, resources, font_bytes, remastered_textures(args, vfs), remastered_fonts(args));
    fe.assets = Some(disc.clone());
    let fit = screen_fit(args.size);
    disc.set_screen(fit.area(args.size));
    fe.margins = fit.margins();
    disc.hold(packages(&sets).into_values().filter_map(|v| v.into_iter().next()));
    // twice: centred sprites bind their textures in the first
    fe.draw_screen(&[l], &mut vec![], &*disc);
    let list = fe.draw_screen(&[l], &mut vec![], &*disc);
    if std::env::var_os("QWC_FE_LIST").is_some() {
        print_list(&list, &fe, &disc);
        for (i, o) in fe.objects.iter().enumerate().filter(|(_, o)| o.layer == l) {
            let parent = o.parent.map(|p| (fe.objects[p].id, fe.objects[p].kind));
            eprintln!("object {i} {:?} {:#x} pos {:?} parent {parent:x?}", o.kind, o.id, &o.pos[..2]);
        }
    }
    let store = flag_store(&fe, &vm.mem);
    render(&list, &fe, &disc, args.size, &out, frames(args.time) as f64 / 24.0, &store, args.look);
}

/// The DataStore entries the stadium's flags pick their textures by (`qwc_render::flags::texture`): their first
/// bytes, 0 when missing.
pub fn flag_store(fe: &FrontEnd, mem: &qwc_feck::Memory) -> flags::Store {
    let byte = |k| fe.datastore_u32(mem, k).unwrap_or(0) & 0xff;
    flags::Store { match_type: byte(flags::KEY_MATCH_TYPE), in_world_cup_flow: byte(flags::KEY_IN_WORLD_CUP_FLOW) != 0 }
}

/// `--time` as 24 Hz frames, at least one (2 s when not given).
fn frames(time: f64) -> usize {
    let t = if time > 0.0 { time } else { 2.0 };
    (t * 24.0).max(1.0) as usize
}

/// The flow started at the node `args.models[0]` names (or the boot node for `boot` or none), the options' sound
/// sliders at the player's levels (`crate::options`).
pub(crate) fn start_flow(args: &Args, vfs: &'static Vfs) -> Flow {
    let start = args.models.first().map(String::as_str);
    let mut flow =
        qwc_frontend::start_flow_in(vfs, &exes(args), start, args.language.as_deref()).unwrap_or_else(|e| fail(e));
    crate::options::push(&args.sound, &flow.fe, &mut flow.vm.mem);
    flow
}

/// The rate `screen`'s speech file is mixed at.
const SPEECH_RATE: u32 = 44_160;

pub fn screen(args: &Args, vfs: &'static Vfs) {
    let out = args.out.clone().unwrap_or_else(|| fail("screen needs --out FILE.png"));
    let mut f = start_flow(args, vfs);
    // the Video options' modes as a window of the picture's size would have them
    f.fe.display_modes = crate::display::headless_modes(args.size);
    let at = f.fe.display_modes.iter().position(|&(w, h, d)| (w, h) == args.size && d == 32);
    f.fe.display_mode = at.map_or(u32::MAX, |i| i as u32);
    f.fe.note_display_modes(&mut f.vm.mem);
    // headless the card stays in memory, unless `QWC_SAVE_DIR` names a folder for it (`crate::record`)
    if std::env::var_os("QWC_SAVE_DIR").is_some() {
        if let Some(card) = crate::record::save_card() {
            f.fe.memcard.io = Box::new(card);
        }
        // and its key bindings beside the save, read as start-up reads them
        if let Some(file) = crate::record::keys_file() {
            f.fe.keys = qwc_feck::keys::Keys::with_file(file);
        }
    }
    let font_bytes = font_files(vfs, &f.resources);
    let disc = Disc::shared(
        vfs,
        std::mem::take(&mut f.resources),
        font_bytes,
        remastered_textures(args, vfs),
        remastered_fonts(args),
    );
    f.fe.assets = Some(disc.clone());
    let fit = screen_fit(args.size);
    disc.set_screen(fit.area(args.size));
    f.fe.margins = fit.margins();
    qwc_frontend::Layouts::default().apply(&mut f.fe, &disc);
    let (mut list, mut previous) = (DrawList::default(), DrawList::default());
    let mut logged = 0;
    let mut cutscenes = 0;
    let mut node = usize::MAX;
    let boot_until: Option<usize> = std::env::var("QWC_BOOT_UNTIL").ok().and_then(|v| v.parse().ok());
    // `QWC_FE_WAV=FILE.wav`: the menus' speech (`crate::fespeech`) said through the game's audio into that file
    let wav = std::env::var_os("QWC_FE_WAV").map(std::path::PathBuf::from);
    let mut speech = wav.as_ref().map(|_| (crate::audio::Audio::offline(vfs, SPEECH_RATE), Vec::new(), 0usize));
    for frame in 0..frames(args.time) {
        // booted: pad 0's choose every 1.5 s, through the controllers
        // (`QWC_BOOT_UNTIL=FRAME` stops the presses there, for `QWC_PADS` to go on)
        if f.boot && boot_until.is_none_or(|u| frame < u) {
            f.fe.pads.words[0][PadWord::Choose as usize] = (frame % 36 == 35) as u16;
        }
        // `QWC_PADS=FRAME:WORD,...`: pad 0's word (`Up`, `Down`, `Left`, `Right`, `Choose`, `Cancel`) for that frame
        if let Ok(list) = std::env::var("QWC_PADS") {
            for (at, word) in list.split(',').filter_map(|e| e.split_once(':')) {
                let w = match word {
                    "Up" => PadWord::Up,
                    "Down" => PadWord::Down,
                    "Left" => PadWord::Left,
                    "Right" => PadWord::Right,
                    "Choose" => PadWord::Choose,
                    "Cancel" => PadWord::Cancel,
                    _ => continue,
                };
                if at.trim().parse::<usize>() == Ok(frame) {
                    f.fe.pads.words[0][w as usize] = 1;
                } else if at.trim().parse::<usize>() == Ok(frame.wrapping_sub(1)) {
                    f.fe.pads.words[0][w as usize] = 0;
                }
            }
        }
        f.m.frame(&mut f.fe, &mut f.vm);
        // headless a front-end cutscene (`General_PlayCutScene`) is over at once: its end reaches the screen's
        // layers as the live front end's does (general event 2)
        if f.m.node != node {
            node = f.m.node;
            eprintln!("[{frame}] at {}", f.m.flow.nodes[node].name.as_deref().unwrap_or("?"));
        }
        let cuts = f.fe.engine.iter().filter(|e| matches!(e, EngineEvent::PlayCutScene(_))).count();
        if cuts > cutscenes {
            cutscenes = cuts;
            for l in f.m.current_screen().map(|s| s.layers.clone()).unwrap_or_default() {
                if let Err(e) = f.fe.general(&mut f.vm, l, 2) {
                    eprintln!("cutscene end: {e}");
                }
            }
        }
        disc.set_loaded(&f.m, &f.sets);
        previous = std::mem::replace(&mut list, f.draw(&*disc));
        log_lines(&f.fe, frame, &mut logged);
        if let Some((audio, samples, seen)) = &mut speech {
            // `QWC_FE_BRIEF=NUM,HOUSE`: challenge NUM's briefing for that house (a team code) at the start
            if let (0, Ok(v)) = (frame, std::env::var("QWC_FE_BRIEF")) {
                let n: Vec<u32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                if let [num, house] = n[..] {
                    audio.challenge_begins(num, house);
                }
            }
            for e in &f.fe.engine[(*seen).min(f.fe.engine.len())..] {
                match e {
                    EngineEvent::FrontendSpeech(a) => audio.frontend_speech(*a),
                    EngineEvent::ChallengeSpeech(a) => audio.challenge_speech(*a),
                    EngineEvent::ResultsSpeech(a) => audio.results_speech(*a),
                    _ => {}
                }
            }
            *seen = f.fe.engine.len();
            audio.speech_tick();
            let mut buf = vec![0.0f32; (SPEECH_RATE / 24) as usize * 2];
            audio.out.mixer().lock().unwrap().render(&mut buf);
            samples.extend(buf.iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16));
        }
        // the audio events, with whether the front end's score could hear them (`crate::audio`)
        let sent = std::mem::take(&mut f.fe.audio_events);
        if std::env::var_os("QWC_AUDIO_LOG").is_some() && !sent.is_empty() {
            // the screen coming, else the current one
            let screen = f.m.next.or(f.m.current).map_or("?", |d| f.m.defs[d].name.as_str());
            let fe = if disc.package_loaded(crate::audio::FE_PACKAGE) { "loaded" } else { "not loaded" };
            eprintln!("[{frame}] {screen}: audio events {sent:?} (the front end's score {fe})");
        }
    }
    if let (Some((_, samples, _)), Some(path)) = (speech, &wav) {
        let pcm = qwc_formats::ea_audio::Pcm { channels: 2, rate: SPEECH_RATE, samples };
        match std::fs::write(path, pcm.to_wav()) {
            Ok(()) => eprintln!("wrote {} ({:.1} s)", path.display(), pcm.frames() as f64 / SPEECH_RATE as f64),
            Err(e) => eprintln!("{}: {e}", path.display()),
        }
    }
    report(&f, 0);
    // `QWC_SPEC_LOG`: the match the DataStore describes now (`crate::fematch::spec`) and the teams' load sets
    if std::env::var_os("QWC_SPEC_LOG").is_some() {
        eprintln!("{:?}", crate::fematch::spec(&f));
        for k in ["Team1LoadSet", "Team2LoadSet"] {
            let id = f.fe.datastore_u32(&f.vm.mem, name_hash(k)).unwrap_or(0);
            eprintln!("{k} {id:#x}: {:?}", load_set(&f.sets, id).map(|s| &s.entries));
        }
    }
    let (m, fe) = (&f.m, &f.fe);
    let name = |n: usize| m.flow.nodes[n].name.clone().unwrap_or(format!("{n}"));
    let live: Vec<&str> =
        m.current_screen().map(|s| s.layers.iter().map(|&l| fe.layers[l].name.as_str()).collect()).unwrap_or_default();
    eprintln!("at {} (state {:?}), layers {live:?}", name(m.node), m.current_screen().map(|s| s.state));
    if let Some(s) = m.current_screen() {
        let slot = |k: u32| load_set(&f.sets, k).map_or(format!("{k:#x}"), |x| x.name.clone());
        eprintln!("load sets {:?}", s.load_sets.iter().map(|&k| slot(k)).collect::<Vec<_>>());
    }
    if std::env::var_os("QWC_FE_LIST").is_some() {
        print_list(&list, fe, &disc);
        eprintln!("clips not found {:?}; natives not ported {:?}", fe.missing_clips, fe.missing);
    }
    // `QWC_FE_BLEND=T`: the frame drawn `T` of the way from the last list but one to the last, as the window draws
    // between ticks (`crate::live`)
    let blend: Option<f32> = std::env::var("QWC_FE_BLEND").ok().and_then(|v| v.parse().ok());
    let list = match blend {
        Some(t) => list.blend(&previous, t),
        None => list,
    };
    let store = flag_store(fe, &f.vm.mem);
    render(&list, fe, &disc, args.size, &out, frames(args.time) as f64 / 24.0, &store, args.look);
}

fn print_list(list: &DrawList, fe: &FrontEnd, disc: &Disc) {
    for s in &list.sprites {
        // the file drawn, through the load sets as the painter resolves it
        let path = disc.texture(s.tex).map_or("?".into(), |(p, _)| p);
        eprintln!(
            "sprite {:#x} {path} at {:?} size {:?} colour {:?} rot {:?} flags {:#x} order {} material {:?}",
            s.id, s.pos, s.size, s.colour, s.rot, s.flags, s.order, s.material
        );
    }
    for t in &list.texts {
        let text = fe.text(t.object).map(|x| {
            String::from_utf16_lossy(&x[t.start as usize..(t.start as usize + t.count as usize).min(x.len())])
        });
        eprintln!("text {:#x} {text:?} at {:?} colour {:?} font {:#x}", t.id, t.pos, t.colour, t.font);
        if std::env::var_os("QWC_FE_CHAIN").is_some() {
            let mut at = Some(t.object);
            while let Some(i) = at {
                let o = &fe.objects[i];
                eprintln!("    {:?} {:#x} flags {:#x} colour {:?} pos {:?}", o.kind, o.id, o.flags, o.colour, o.pos);
                at = o.parent;
            }
        }
    }
    for md in &list.models {
        eprintln!("model {:#x} resource {:#x} at {:?} region {:?}", md.id, md.resource, md.pos, md.region);
    }
    for p in &list.puppets {
        let posed = fe.puppets.get(&p.object).is_some_and(|x| x.character.control.pose.is_some());
        let o = &fe.objects[p.object];
        eprintln!(
            "puppet {:#x} at {:?} rotation {:?} placed {} posed {posed}; object angles {:?} flags {:#x} region {:?}",
            p.id, p.pos, p.rotation, p.relative, o.rot, o.flags, p.region
        );
    }
    if let Some(v) = list.view {
        eprintln!("view eye {:?} dir {:?} up {:?} fov {} near {} far {}", v.eye, v.dir, v.up, v.fov_y, v.near, v.far);
    }
    for &e in &list.particles {
        let o = &fe.objects[e];
        let desc: Vec<f32> = (0..19).map(|k| f32::from_bits(o.slot(0xc0 + 4 * k))).collect();
        eprintln!(
            "emitter {:#x} flags {:#x} at {:?} on {} template {} to place {} desc {:#x} {:?}",
            o.id,
            o.flags,
            &o.pos[..2],
            o.slot(0xbc),
            o.slot(0x10c),
            o.slot(0x114) as i32,
            o.slot(0xc0),
            &desc[1..]
        );
    }
}

fn hash(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish() & !(1 << 63)
}

/// A vertex for the overlay: texture coordinates from 1/4096, the colour's bytes over 255.
fn corner(v: &Vertex) -> OverlayVertex {
    let c = |shift: u32| ((v.argb >> shift) & 0xff) as f32 / 255.0;
    OverlayVertex {
        pos: [v.pos[0], v.pos[1]],
        uv: [v.uv[0] as f32 / 4096.0, v.uv[1] as f32 / 4096.0],
        colour: [c(16), c(8), c(0), c(24)],
    }
}

/// The corners of a quad kept to a remastered texture's shape ([`qwc_frontend::disc::keep_shape`]).
fn keep_shape(c: [OverlayVertex; 4], shape: f32, screen: (u32, u32)) -> [OverlayVertex; 4] {
    let p = qwc_frontend::disc::keep_shape(c.map(|v| v.pos), shape, screen);
    let mut out = c;
    for (o, p) in out.iter_mut().zip(p) {
        o.pos = p;
    }
    out
}

/// Where the front end's hoops stand ([`FrontEnd::hoops`], `General_FrontendEntry(on)`, `0x4d3da0`): the match's six
/// (`qwc_sim::shooting::hoops`) at the stadium's `Pitch.GoalHeight` and `Pitch.ScaleGoals` (its `game/match.cli`: 20
/// and the default 1 for `sum_fe`), each at its centre unturned (the placement is an identity rotation). **[I]**
/// unscaled: the hoop renderer scales by its record's `+0x20` (`0x4a1c90`), which no placement writes.
fn hoop_places(vfs: &qwc_assets::Vfs) -> [[f32; 16]; 6] {
    let mut console = qwc_sim::Console::new();
    if let Ok(b) = vfs.read("stadium/sum_fe/game/match.cli") {
        // the lines for other systems (the match, the cut scenes) fail here and are left
        let _ = console.run_script(&b);
    }
    let num = |k: &str, d: f64| console.args(k, 0).iter().rev().find_map(|v| v.num()).unwrap_or(d);
    let hoops = qwc_sim::shooting::hoops(num("Pitch.ScaleGoals", 1.0), num("Pitch.GoalHeight", 0.0));
    hoops.map(|h| qwc_feck::place::translation(h.pos.map(|v| v as f32)))
}

/// The world camera a front-end view gives (`0x49f720`: the vertical field of view in degrees).
fn world_camera(v: &qwc_feck::draw::View) -> Camera {
    let eye = Vec3::new(v.eye[0], v.eye[1], v.eye[2]);
    Camera {
        eye,
        target: eye + Vec3::new(v.dir[0], v.dir[1], v.dir[2]),
        up: Vec3::new(v.up[0], v.up[1], v.up[2]),
        fov_y: v.fov_y.to_radians(),
        near: v.near,
        far: v.far,
    }
}

const IDENTITY: crate::cutscene::Mat4 =
    [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// A match drawn under the front end (`crate::fematch`): the match, what it is played with, how far into its tick
/// the frame is and the match's time.
pub struct MatchView<'a> {
    pub m: &'a qwc_sim::Match,
    pub spec: &'a crate::arena::MatchSpec,
    pub alpha: f64,
    pub time: f64,
    /// The challenge being played, with its passes' outcomes.
    pub challenge: Option<(&'a qwc_sim::challenge::Challenge, &'a [u32])>,
    /// How dark the screen is over the match, and how much of the HUD shows (`crate::restart`).
    pub dark: f32,
    pub hud: f32,
}

/// Drawing the front end on a GPU: the overlay with the textures it has uploaded, and the stadium behind (loaded
/// the first time a frame has a camera), or a match's world while one runs.
/// The front end's stadium (its 3D world behind the menus), whose look file is its look.
const FE_STADIUM: &str = "sum_fe";

/// Not the original's: the front end's stadium's remastered scenery (`crate::scenery`): the highest pack's
/// `stadiums/sum_fe/` (the local hog_sum test pack is made by `work/environments/export_fe_stadium.py`), or the folder
/// `QWC_FE_STADIUM` names (`disc`: none). The arena (`stadium.glb`) is where the light probes' box is taken round.
const FE_STADIUM_ENV: &str = "QWC_FE_STADIUM";

/// The front end's stadium's scenery folder, if any.
fn fe_scenery_dir() -> Option<std::path::PathBuf> {
    let dir = std::env::var_os(FE_STADIUM_ENV).map(std::path::PathBuf::from);
    dir.or_else(|| qwc_packs::current().stadium(FE_STADIUM)).filter(|d| d.as_os_str() != "disc")
}

pub struct Painter {
    overlay: Overlay,
    renderer: Renderer,
    stadium: Option<Vec<GpuModel>>,
    /// The front end's stadium's light set (`stadium/sum_fe/gfx/lights.cli`), loaded with it.
    stadium_lights: Option<qwc_render::Lights>,
    /// The six hoops, each its own copy of the hoop model at its place, loaded with the stadium.
    hoops: Vec<GpuModel>,
    /// The remastered stadium's surroundings ([`FE_STADIUM_ENV`]), drawn with it; none for the disc's.
    surroundings: Vec<GpuModel>,
    /// With remastered scenery: its config (whether the disc's flags are drawn, how far the camera sees).
    scenery: Option<qwc_packs::StadiumConfig>,
    /// The stadium's flags, loaded with it and again after each `General_RefreshWavyFlags`.
    flags: Option<crate::stadium::Flags>,
    /// The screens' models and puppets (`crate::fe3d`).
    objects: crate::fe3d::Objects,
    /// The match's world while one runs, with what it was loaded for.
    arena: Option<(crate::arena::MatchSpec, crate::arena::Arena)>,
    /// How the 3D world is lit ([`Painter::set_world`]): the front end's stadium and a match's.
    world_look: qwc_render::Look,
    /// The looks (`crate::looks`) of the front end's stadium and of the match's, each read again when saved.
    fe_look: Option<(qwc_render::remaster::Settings, crate::looks::Watch)>,
    match_look: Option<(qwc_render::remaster::Settings, crate::looks::Watch)>,
    /// The stadium the remastered lighting's light probes were last made in.
    probes_for: Option<String>,
    /// The match camera's heading across the ground (the sticks turn by it).
    pub heading: [f64; 2],
    /// The match's HUD (`crate::hud`).
    hud: crate::hud::Hud,
    /// The vector fonts' glyphs: drawn, uploaded (by texture key), and the window they are for.
    text: qwc_text::Renderer,
    glyphs: HashSet<u64>,
    text_screen: (u32, u32),
    /// A cutscene playing over the front end's stadium (`General_PlayCutScene`, `crate::cutscene`), when it started
    /// (painter seconds), how far it has played and up to when its events were handed out.
    cutscene: Option<(crate::cutscene::Cutscene, Option<f64>, f64, f64)>,
    /// Where it is placed (`crate::cutscene::Cutscene::anchor`).
    cutscene_anchor: crate::cutscene::Mat4,
    /// How many of the disc's dropped textures ([`Disc::dropped`]) the overlay has dropped too.
    dropped: usize,
}

impl Painter {
    pub fn new(gpu: &Gpu, format: wgpu::TextureFormat) -> Painter {
        let mut overlay = Overlay::new(&gpu.device, format);
        // a white texel for the alpha clear
        overlay.upload(gpu, 0, 1, 1, &[255; 4]);
        // the 3D view starts black where nothing is drawn, the screen's alpha (the masks' channel) clear
        let mut renderer = Renderer::new(&gpu.device, &gpu.queue, format);
        renderer.shaders = crate::packs::shaders();
        renderer.clear = wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };
        Painter {
            overlay,
            renderer,
            stadium: None,
            stadium_lights: None,
            hoops: Vec::new(),
            surroundings: Vec::new(),
            scenery: None,
            flags: None,
            objects: Default::default(),
            arena: None,
            world_look: qwc_render::Look::Original,
            fe_look: None,
            match_look: None,
            probes_for: None,
            heading: [-1.0, 0.0],
            hud: Default::default(),
            text: qwc_text::Renderer::new(),
            glyphs: HashSet::new(),
            text_screen: (0, 0),
            cutscene: None,
            cutscene_anchor: IDENTITY,
            dropped: 0,
        }
    }

    /// Start cutscene `name` (a folder of `root/cs`) over the front end's stadium, or over the match's while one
    /// runs, `teams` its `TEAM1` and `TEAM2` (folders of `teams/`) and `cast` the match's people; false when it
    /// cannot load.
    pub fn start_cutscene(
        &mut self,
        gpu: &Gpu,
        vfs: &qwc_assets::Vfs,
        name: &str,
        teams: [&str; 2],
        cast: &crate::cutscene::Cast,
        place: crate::cutscene::Place,
    ) -> bool {
        match crate::cutscene::Cutscene::load(gpu, &mut self.renderer, vfs, name, teams, cast) {
            Ok(cs) => {
                for n in &cs.notes {
                    eprintln!("  note: {n}");
                }
                eprintln!("cutscene {}: {:.1} s", cs.name, cs.duration());
                self.cutscene_anchor = cs.anchor(place);
                self.cutscene = Some((cs, None, 0.0, -1.0));
                true
            }
            Err(e) => {
                eprintln!("cutscene {name}: {e}");
                false
            }
        }
    }

    /// Whether the cutscene has played to its end.
    pub fn cutscene_over(&self) -> bool {
        self.cutscene.as_ref().is_some_and(|(cs, _, t, _)| *t >= cs.duration())
    }

    /// The cutscene's triggers reached since the last call (their names).
    pub fn take_cutscene_events(&mut self) -> Vec<String> {
        let Some((cs, _, played, reported)) = self.cutscene.as_mut() else { return Vec::new() };
        let out = cs.events_between(*reported, *played).map(|e| e.name.clone()).collect();
        *reported = *played;
        out
    }

    pub fn end_cutscene(&mut self) {
        self.cutscene = None;
    }

    /// Upload the texture at `path` (as [`Disc::texture`] loaded it) under `key` when it is not yet; answers its
    /// shape when it keeps its own ([`qwc_frontend::Shown::shape`]).
    fn upload(&mut self, gpu: &Gpu, disc: &Disc, path: &str, key: u64) -> Option<f32> {
        let t = disc.shown(path)?;
        if !self.overlay.has_texture(key) {
            let p = &t.pixels;
            if t.remastered {
                self.overlay.upload_levels(gpu, key, p.width, p.height, &p.levels);
            } else {
                self.overlay.upload(gpu, key, p.width, p.height, p.rgba());
            }
        }
        t.shape
    }

    /// The list's sprites and text runs as overlay quads on a `screen` of pixels, uploading textures as they first
    /// appear.
    fn quads(
        &mut self,
        gpu: &Gpu,
        list: &DrawList,
        fe: &FrontEnd,
        disc: &Disc,
        screen: (u32, u32),
    ) -> Vec<OverlayQuad> {
        // a texture the disc read again (a file written over, a crop changed) is uploaded again
        let (dropped, seen) = disc.dropped(self.dropped);
        self.dropped = seen;
        for path in dropped {
            self.overlay.forget(hash(&path));
        }
        let mut quads = Vec::new();
        if list.camera.is_some() {
            // **[I]** the screen's alpha (the masks' channel) is cleared before the front end draws: a quad writing
            // alpha 0 only (type 4) under everything
            let corners = whole(qwc_feck::draw::Fit { span: self.overlay.span() }, [0.0; 4]);
            quads.push(OverlayQuad { texture: 0, blend: 4, layer: 0, corners });
        }
        for s in &list.sprites {
            let Some((path, tsize)) = disc.texture(s.tex) else { continue };
            let key = hash(&path);
            let shape = self.upload(gpu, disc, &path, key);
            // faded by its layer's wipe, with flag 0x4000
            let mut corners = sprite_quad(s, tsize, list.wipe(s.layer).as_ref()).map(|v| corner(&v));
            if let Some(shape) = shape {
                corners = keep_shape(corners, shape, screen);
            }
            moved(&mut corners, list.xf_of(s.object));
            quads.push(OverlayQuad { texture: key, blend: s.material[0], layer: s.order, corners });
        }
        for p in &list.sparks {
            let Some((path, _)) = disc.texture(p.tex) else { continue };
            let key = hash(&path);
            let shape = self.upload(gpu, disc, &path, key);
            let mut corners = p.corners.map(|v| corner(&v));
            if let Some(shape) = shape {
                corners = keep_shape(corners, shape, screen);
            }
            moved(&mut corners, list.xf_of(p.object));
            quads.push(OverlayQuad { texture: key, blend: p.material[0], layer: p.order, corners });
        }
        if self.text_screen != screen {
            // new glyph sizes: the old ones go
            self.text.clear();
            for k in self.glyphs.drain() {
                self.overlay.forget(k);
            }
            self.text_screen = screen;
        }
        for run in &list.texts {
            if let (Some(v), Some(text)) = (disc.vector_font(run.font), fe.text(run.object)) {
                let from = quads.len();
                self.vector_run(gpu, run, text, v, screen, &mut quads);
                let xf = list.xf_of(run.object);
                quads[from..].iter_mut().for_each(|q| moved(&mut q.corners, xf));
                continue;
            }
            let (Some(font), Some(text)) = (disc.bitmap_font(run.font), fe.text(run.object)) else { continue };
            let key = 1 << 63 | run.font as u64;
            if !self.overlay.has_texture(key) {
                match font.atlas() {
                    Ok(rgba) => self.overlay.upload(gpu, key, font.tex_w as u32, font.tex_h as u32, &rgba),
                    Err(e) => {
                        eprintln!("font {:#x}: {e}", run.font);
                        continue;
                    }
                }
            }
            let xf = list.xf_of(run.object);
            for q in text_quads(run, text, font, [font.tex_w, font.tex_h]) {
                let mut corners = q.map(|v| corner(&v));
                moved(&mut corners, xf);
                // **[I]** a font's blend type is not traced: alpha blended, in the text's material layer
                quads.push(OverlayQuad { texture: key, blend: 2, layer: run.order, corners });
            }
        }
        quads
    }

    /// A text run in a vector font: its glyphs' shadows, then their fills, each glyph its own texture, in the run's
    /// colour as the original's glyphs are.
    fn vector_run(
        &mut self,
        gpu: &Gpu,
        run: &qwc_feck::draw::TextRun,
        text: &[u16],
        font: &qwc_text::TextFont,
        screen: (u32, u32),
        quads: &mut Vec<OverlayQuad>,
    ) {
        let start = (run.start as usize).min(text.len());
        let chars = &text[start..(start + run.count as usize).min(text.len())];
        let placed = self.text.run(font, chars, [run.pos[0], run.pos[1]], screen);
        let colour = corner(&Vertex { pos: [0.0; 3], uv: [0, 0], argb: pack_colour(run.colour) }).colour;
        let (kx, ky) = (screen.0.max(1) as f32 / 512.0, screen.1.max(1) as f32 / 512.0);
        for p in placed {
            let key = 1 << 61 | (p.key.hash64() >> 3);
            if !self.glyphs.contains(&key) {
                let Some(im) = self.text.image(&p.key) else { continue };
                self.overlay.upload(gpu, key, im.width, im.height, &im.rgba);
                self.glyphs.insert(key);
            }
            let (x0, y0) = (p.x / kx, p.y / ky);
            let (x1, y1) = ((p.x + p.width as f32) / kx, (p.y + p.height as f32) / ky);
            let v = |x, y, u, t| OverlayVertex { pos: [x, y], uv: [u, t], colour };
            let corners = [v(x0, y1, 0.0, 1.0), v(x1, y1, 1.0, 1.0), v(x0, y0, 0.0, 0.0), v(x1, y0, 1.0, 0.0)];
            // **[I]** as the disc's fonts: alpha blended, in the text's material layer
            quads.push(OverlayQuad { texture: key, blend: 2, layer: run.order, corners });
        }
    }

    /// Light the 3D world as `look`, the front end's stadium and the matches' (the screens' 2D over it is as it
    /// was), its models drawing the remastered textures of `textures`.
    /// The packs' big pictures compressed from now on (`qwc_render::Renderer::compress`).
    pub fn set_compression(&mut self, compress: Option<Option<std::path::PathBuf>>) {
        self.renderer.compress = compress;
    }

    pub fn set_world(&mut self, look: qwc_render::Look, textures: Option<qwc_assets::remaster::Remastered>) {
        self.world_look = look;
        self.renderer.remastered = textures;
    }

    /// Load the front end's stadium: the disc's sky and its light set, then the remastered scenery when
    /// [`remastered_fe_stadium`] finds any (the arena with the sky, the rest as surroundings), else the disc's
    /// `stadium.fpm` and the six hoops.
    fn load_stadium(&mut self, gpu: &Gpu, disc: &Disc) {
        let textures = qwc_assets::TextureIndex::build(disc.vfs);
        let mut models = stadium::models(gpu, &mut self.renderer, disc.vfs, &textures, &["stadium/sum_fe/gfx/sky.fpm"]);
        self.stadium_lights = stadium::lights(disc.vfs, "stadium/sum_fe/gfx");
        self.surroundings.clear();
        self.hoops.clear();
        let scenery = fe_scenery_dir().and_then(|d| crate::scenery::load(gpu, &mut self.renderer, disc.vfs, &d));
        self.scenery = scenery.as_ref().map(|s| s.config);
        if let Some(s) = scenery {
            // the disc's sky (about 830 across) would stand in front of the surroundings: around them instead
            crate::scenery::grow_sky(gpu, &self.renderer, &models, s.config.sky_scale);
            models.extend(s.arena);
            self.surroundings = s.surroundings;
            if s.config.hoops {
                let places = hoop_places(disc.vfs);
                self.hoops = stadium::hoops(gpu, &mut self.renderer, disc.vfs, &textures, &disc.hoop_model(), &places);
            }
        } else {
            models.extend(stadium::models(
                gpu,
                &mut self.renderer,
                disc.vfs,
                &textures,
                &["stadium/sum_fe/gfx/stadium.fpm"],
            ));
            let places = hoop_places(disc.vfs);
            self.hoops = stadium::hoops(gpu, &mut self.renderer, disc.vfs, &textures, &disc.hoop_model(), &places);
        }
        self.stadium = Some(models);
    }

    /// Before the front end's own 3D: its stadium's look (`sum_fe`'s file, read again when saved) and, once the
    /// stadium is loaded, the light probes in it (made again after a match's, or a saved look). Over the painter's
    /// fields, as the screen's models are borrowed meanwhile.
    fn front_end_look(&mut self, gpu: &Gpu) {
        front_end_look(
            gpu,
            &mut self.renderer,
            self.world_look,
            &mut self.fe_look,
            &mut self.probes_for,
            self.stadium.as_deref(),
            &self.hoops,
            &self.surroundings,
            self.stadium_lights,
        );
    }

    /// A tick of the match drawn (`crate::arena::Arena::ticked`).
    pub fn match_ticked(&mut self, m: &qwc_sim::Match, time: f64) {
        if let Some((_, a)) = &mut self.arena {
            a.ticked(m, time);
        }
    }

    /// Hold the match camera where it is (a challenge's outro), or let it follow again.
    pub fn hold_camera(&mut self, hold: bool) {
        if let Some((_, a)) = &mut self.arena {
            a.hold_camera = hold;
        }
    }

    /// The match is over: its world goes.
    pub fn end_match(&mut self) {
        self.arena = None;
    }

    /// One frame into `target` (with its `depth`, both `size`): a match's world when `game` gives one, else the
    /// stadium through the frame's camera when it has one, then the overlay. The stadium's flags wave by the clock, `time` seconds (the original's is the game's
    /// millisecond count, `0x494390`), and take their textures by the DataStore as `store` has it when they load.
    /// Answers how many quads it drew.
    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &mut self,
        gpu: &Gpu,
        target: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        size: (u32, u32),
        list: &DrawList,
        fe: &FrontEnd,
        disc: &Disc,
        time: f64,
        store: &flags::Store,
        game: Option<&MatchView>,
    ) -> usize {
        let fit = screen_fit(size);
        self.overlay.set_span(gpu, fit.span);
        let quads = self.quads(gpu, list, fe, disc, fit.area(size));
        let mut clear = Some(wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 });
        // a cutscene over the front end's stadium, through its live shot; the screen's layers over it
        if self.cutscene.is_some() && self.stadium.is_none() && game.is_none() {
            self.load_stadium(gpu, disc);
        }
        if game.is_none() {
            self.front_end_look(gpu);
        }
        // a match's world is loaded the first time a frame has the match, in its stadium's look; the light probes
        // made again in it when its look is saved (or after the front end's were made)
        if let Some(g) = game {
            self.renderer.look = self.world_look;
            let remastered = self.world_look == qwc_render::Look::Remastered;
            if self.arena.as_ref().is_none_or(|(spec, _)| spec != g.spec) {
                if remastered {
                    let stadium = &g.spec.stadium;
                    self.match_look = Some((crate::looks::load(stadium), crate::looks::Watch::new(stadium)));
                    self.renderer.settings = self.match_look.as_ref().expect("set above").0;
                    stadium::apply_env(&mut self.renderer);
                }
                let a = crate::arena::Arena::load(gpu, &mut self.renderer, disc.vfs, g.spec, g.m);
                self.arena = Some((g.spec.clone(), a));
                self.probes_for = Some(g.spec.stadium.clone());
            }
            if let (Some((settings, watch)), Some((_, a))) = (&mut self.match_look, &self.arena) {
                let saved = watch.poll();
                if let Some(s) = saved {
                    *settings = s;
                }
                self.renderer.settings = *settings;
                stadium::apply_env(&mut self.renderer);
                if remastered && (saved.is_some() || self.probes_for.as_deref() != Some(&g.spec.stadium)) {
                    a.bake_probes(gpu, &mut self.renderer, g.m);
                    self.probes_for = Some(g.spec.stadium.clone());
                }
            }
        }
        if let Some((cs, start, played, _)) = self.cutscene.as_mut() {
            let start = *start.get_or_insert(time);
            *played = time - start;
            if let Some(mut camera) = cs.frame(gpu, &self.renderer, *played, &self.cutscene_anchor) {
                // over the match's stadium while one runs, else the front end's
                let mut models: Vec<&GpuModel> = Vec::new();
                match (game, &mut self.arena) {
                    (Some(_), Some((_, arena))) => {
                        camera.far = camera.far.max(arena.far());
                        models.extend(arena.scenery(gpu, &self.renderer, time, cs.crowd_box()))
                    }
                    _ => {
                        let own = self.stadium.iter_mut().flatten().chain(&mut self.hoops);
                        stadium::animate(gpu, &self.renderer, own, time);
                        models.extend(self.stadium.iter().flatten());
                        models.extend(&self.surroundings);
                        models.extend(&self.hoops);
                        if let Some(f) = &self.flags {
                            f.wave(gpu, &self.renderer, time);
                            models.extend(f.drawn());
                        }
                    }
                }
                models.extend(cs.drawn());
                self.renderer.draw(gpu, target, depth, size, &camera, &models);
                clear = None;
            }
            // its fades: black over the world (the white texel, `128 = 1` colours), under the screen's layers
            let dark = cs.fade(*played) as f32;
            let corners = whole(fit, [0.0, 0.0, 0.0, dark * 0.5]);
            let mut all = vec![OverlayQuad { texture: 0, blend: 2, layer: 0, corners }];
            all.extend(quads);
            self.overlay.draw(gpu, target, clear, &all);
            return all.len();
        }
        if let (Some(g), Some((_, arena))) = (game, self.arena.as_mut()) {
            // the passing challenge's rings, drawn in the world as the original's hoop render module does
            arena.rings = match g.challenge.filter(|(c, _)| c.plays() && c.kind == 1) {
                Some((c, _)) => (0..2)
                    .map(|i| {
                        let p = c.rings.pos[i];
                        let v = qwc_render::glam::Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32);
                        (v, c.rings.size[i] as f32, c.rings.colour[i].map(|x| x as f32))
                    })
                    .collect(),
                None => Vec::new(),
            };
            let camera = arena.frame(gpu, &self.renderer, g.m, g.alpha, g.time);
            // the sticks turn by the follow camera's heading, whatever shot shows
            let ahead = arena.ahead().unwrap_or(camera.target - camera.eye);
            let flat = qwc_render::glam::Vec2::new(ahead.x, ahead.z).normalize_or_zero();
            if flat != qwc_render::glam::Vec2::ZERO {
                self.heading = [f64::from(flat.x), f64::from(flat.y)];
            }
            arena.draw_world(gpu, &mut self.renderer, target, depth, size, g.m, &camera, g.alpha);
            let vp = camera.view_proj(size.0 as f32 / size.1.max(1) as f32);
            // the passing challenge's rings on the 640 x 480 screen: centre, radius and colour
            let project = |p: [f64; 3]| {
                let c = vp * qwc_render::glam::Vec4::new(p[0] as f32, p[1] as f32, p[2] as f32, 1.0);
                (c.w > 0.1).then(|| [320.0 * (c.x / c.w + 1.0), 240.0 * (1.0 - c.y / c.w)])
            };
            let right = (camera.target - camera.eye).cross(camera.up).normalize_or_zero();
            // a ring at `p` of world radius `k` times `size`, in `colour`
            let ring = |p: [f64; 3], k: f64, size: f64, colour: [f64; 4]| {
                let edge = [p[0] + f64::from(right.x) * k, p[1], p[2] + f64::from(right.z) * k];
                let (a, b) = (project(p)?, project(edge)?);
                let radius = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt() * size as f32;
                Some(crate::hud::Ring { at: a, radius, colour: colour.map(|v| v as f32) })
            };
            use qwc_sim::challenge as ch;
            let rings: Vec<crate::hud::Ring> = match g.challenge.filter(|(c, _)| c.plays()) {
                // **[I]** the Beaters' targets as rings at their hit radius: green the one in range, red the
                // others, gone once hit (the original's are `0x10439d4`'s with `0x1018f24` and `0x1019010`)
                Some((c, _)) if c.kind == 4 => {
                    let t = &c.targets;
                    (0..5)
                        .filter(|&i| !t.hit[i])
                        .filter_map(|i| {
                            let colour = if t.in_range == Some(i) { ch::GREEN } else { ch::RED };
                            ring(t.pos(i), ch::TARGET_RADIUS, 1.0, colour)
                        })
                        .collect()
                }
                _ => Vec::new(),
            };
            // the HUD in the engine's material layers, with the front end's
            let hud =
                crate::hud::View { teams: [&g.spec.teams[0], &g.spec.teams[1]], rings: &rings, challenge: g.challenge };
            let mut all: Vec<OverlayQuad> = crate::restart::black(fit, g.dark).into_iter().collect();
            let mut hud = self.hud.quads(&mut self.overlay, gpu, disc.vfs, g.m, &hud, size, fit);
            crate::restart::fade(&mut hud, g.hud);
            all.extend(hud);
            all.extend(quads);
            self.overlay.draw(gpu, target, None, &all);
            return all.len();
        }
        // **[I]** the world is drawn while the stadium's render is on and the front end's scene is (the movie and
        // loading screens leave the scene, and the boot screens before the autosave never turn the stadium on)
        let view = list.view.or_else(|| list.camera.map(|c| fe.camera_view(c)));
        let Some(view) = view else {
            self.overlay.draw(gpu, target, clear, &quads);
            return quads.len();
        };
        let world = fe.stadium_render && fe.scene && disc.stadium_loaded();
        if world && self.stadium.is_none() {
            self.load_stadium(gpu, disc);
        }
        // the screen's models and puppets, drawn in the same world (`0x4e4ff0`, `0x4e5390`) whether the stadium is
        let loaded = |f: &str| disc.package_loaded(f);
        let objects = self.objects.prepare(gpu, &mut self.renderer, disc.vfs, disc.resources(), &loaded, list, fe);
        if world || !objects.is_empty() {
            front_end_look(
                gpu,
                &mut self.renderer,
                self.world_look,
                &mut self.fe_look,
                &mut self.probes_for,
                self.stadium.as_deref(),
                &self.hoops,
                &self.surroundings,
                self.stadium_lights,
            );
            // the scripts' light set (`General_Light_Set*`) once they have given one, else the front end's stadium's
            let l = &fe.lights;
            if l.ambient != [0.0; 3] || l.diffuse.iter().any(|d| *d != [0.0; 3]) {
                let lights = std::array::from_fn(|k| (l.diffuse[k], l.direction[k]));
                self.renderer.lights = qwc_render::Lights { ambient: l.ambient, lights };
            } else if let Some(sl) = self.stadium_lights {
                self.renderer.lights = sl;
            }
            if world {
                // the flags with the stadium, loaded again after each refresh (`0x49b6d0`); the remastered stadium
                // has its own
                let refreshes = fe.engine.iter().filter(|e| matches!(e, EngineEvent::RefreshWavyFlags)).count();
                let disc_flags = self.scenery.is_none_or(|c| c.flags);
                if disc_flags && self.flags.as_ref().is_none_or(|f| f.refreshes != refreshes) {
                    // **[I]** the teams' groups are empty in the front end (no match is loaded)
                    let group = disc.stadium_group();
                    let groups = |g| if g == flags::Group::Stadium { group.clone() } else { Vec::new() };
                    self.flags =
                        Some(stadium::Flags::load(gpu, &mut self.renderer, disc.vfs, &groups, store, refreshes));
                }
                if let Some(f) = &self.flags {
                    f.wave(gpu, &self.renderer, time);
                }
                let own = self.stadium.iter_mut().flatten().chain(&mut self.hoops);
                stadium::animate(gpu, &self.renderer, own, time);
            }
            let mut camera = world_camera(&view);
            if let Some(c) = self.scenery {
                camera.far = camera.far.max(c.far);
            }
            let mut models: Vec<&GpuModel> = if world { self.stadium.iter().flatten().collect() } else { Vec::new() };
            if world {
                models.extend(&self.surroundings);
            }
            // the hoops once the front end has placed them, with the stadium
            if world && fe.hoops {
                models.extend(&self.hoops);
            }
            if let Some(f) = self.flags.as_ref().filter(|_| world) {
                models.extend(f.drawn());
            }
            models.extend(objects);
            self.renderer.draw(gpu, target, depth, size, &camera, &models);
            clear = None;
        }
        self.overlay.draw(gpu, target, clear, &quads);
        quads.len()
    }
}

/// Not the original's: where the front end's 512 x 512 screen goes on a window of `size` pixels. At 4:3 in the
/// middle, the window reaching past it on two sides (`qwc_feck::draw::Fit`), its pinned objects at the edges;
/// `QWC_STRETCH_SCREEN=1` stretches it over the window as the original does.
pub fn screen_fit(size: (u32, u32)) -> qwc_feck::draw::Fit {
    use qwc_feck::draw::Fit;
    if std::env::var_os("QWC_STRETCH_SCREEN").is_some_and(|v| v == "1") {
        Fit::STRETCHED
    } else {
        Fit::four_three(size)
    }
}

/// A quad over the whole target (the screen and its margins) in one colour, on the white texel.
fn whole(fit: qwc_feck::draw::Fit, colour: [f32; 4]) -> [OverlayVertex; 4] {
    let [l, t, r, b] = fit.span;
    let v = |x, y| OverlayVertex { pos: [x, y], uv: [0.0, 0.0], colour };
    [v(l, b), v(r, b), v(l, t), v(r, t)]
}

/// A quad's corners where the remaster's adjustments draw them (`qwc_feck::draw::Xf`).
fn moved(corners: &mut [OverlayVertex; 4], xf: qwc_feck::draw::Xf) {
    if !xf.is_identity() {
        corners.iter_mut().for_each(|c| c.pos = xf.at(c.pos));
    }
}

/// The texture key a playing movie's picture is uploaded under.
const MOVIE_KEY: u64 = 1 << 62;

/// A movie picture to upload.
pub struct MoviePicture<'a> {
    pub pixels: &'a [u8],
    pub size: (u32, u32),
    /// Stored B G R X (the remastered movies) rather than R G B A.
    pub bgra: bool,
}

impl Painter {
    /// A movie's picture over the whole screen (`0x4d1e80`: the region `fullscreen`, the picture stretched over
    /// it, the vertex colour `0x808080`, so drawn as it is), uploading `picture` when given. A remastered movie
    /// keeps its shape instead: `keep` is its size, fitted into the `screen` (pixels) with black around it.
    pub fn movie(
        &mut self,
        gpu: &Gpu,
        target: &wgpu::TextureView,
        screen: (u32, u32),
        keep: Option<(u32, u32)>,
        picture: Option<MoviePicture>,
    ) {
        // fitted here to the whole target
        self.overlay.set_span(gpu, qwc_render::overlay::STRETCHED);
        if let Some(p) = picture {
            let (w, h) = p.size;
            if p.bgra {
                self.overlay.upload_bgra(gpu, MOVIE_KEY, w, h, p.pixels);
            } else {
                self.overlay.upload(gpu, MOVIE_KEY, w, h, p.pixels);
            }
        }
        // the part of the 512 x 512 screen covered
        let (x, y) = match keep {
            Some((w, h)) => {
                let (sw, sh) = (screen.0.max(1) as f32, screen.1.max(1) as f32);
                let k = (sw / w as f32).min(sh / h as f32);
                ((1.0 - w as f32 * k / sw) * 256.0, (1.0 - h as f32 * k / sh) * 256.0)
            }
            None => (0.0, 0.0),
        };
        let v = |x, y, u, t| OverlayVertex { pos: [x, y], uv: [u, t], colour: [0.5; 4] };
        let (x1, y1) = (512.0 - x, 512.0 - y);
        let corners = [v(x, y1, 0.0, 1.0), v(x1, y1, 1.0, 1.0), v(x, y, 0.0, 0.0), v(x1, y, 1.0, 0.0)];
        let quad = OverlayQuad { texture: MOVIE_KEY, blend: 1, layer: 0, corners };
        self.overlay.draw(gpu, target, Some(wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }), &[quad]);
    }
}

/// The list drawn into a PNG at `out`, `time` seconds into the flow.
#[allow(clippy::too_many_arguments)]
fn render(
    list: &DrawList,
    fe: &FrontEnd,
    disc: &Disc,
    size: (u32, u32),
    out: &std::path::Path,
    time: f64,
    store: &flags::Store,
    look: qwc_render::Look,
) {
    eprintln!(
        "{} sprites, {} text runs, {} models, {} puppets, {} particles",
        list.sprites.len(),
        list.texts.len(),
        list.models.len(),
        list.puppets.len(),
        list.sparks.len()
    );
    if let Some(c) = list.camera {
        eprintln!("camera {:#x}: {:?}", fe.objects[c].id, fe.camera_view(c));
    }
    let gpu = Gpu::headless().unwrap_or_else(|e| fail(e));
    let mut painter = Painter::new(&gpu, OVERLAY_FORMAT);
    painter.set_compression(crate::compression_when(false));
    painter.set_world(look, None);
    let target = OverlayTarget::new(&gpu.device, size);
    let depth = qwc_render::depth_view(&gpu.device, size);
    let quads = painter.paint(&gpu, &target.view, &depth, size, list, fe, disc, time, store, None);
    let rgba = target.read_rgba(&gpu).unwrap_or_else(|e| fail(e));
    write_png(out, size, rgba);
    eprintln!("wrote {} ({quads} quads)", out.display());
}

/// A frame's RGBA rows as a PNG. The alpha channel is the masks' working space, not transparency: it is written
/// opaque.
pub(crate) fn write_png(out: &std::path::Path, size: (u32, u32), mut rgba: Vec<u8>) {
    rgba.iter_mut().skip(3).step_by(4).for_each(|a| *a = 255);
    let file = std::fs::File::create(out).unwrap_or_else(|e| fail(format!("{}: {e}", out.display())));
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), size.0, size.1);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().unwrap_or_else(|e| fail(e));
    w.write_image_data(&rgba).unwrap_or_else(|e| fail(e));
}

/// [`Painter::front_end_look`].
#[allow(clippy::too_many_arguments)]
fn front_end_look(
    gpu: &Gpu,
    renderer: &mut Renderer,
    look: qwc_render::Look,
    fe_look: &mut Option<(qwc_render::remaster::Settings, crate::looks::Watch)>,
    probes_for: &mut Option<String>,
    stadium_models: Option<&[GpuModel]>,
    hoops: &[GpuModel],
    surroundings: &[GpuModel],
    lights: Option<qwc_render::Lights>,
) {
    renderer.look = look;
    if look != qwc_render::Look::Remastered {
        return;
    }
    let (settings, watch) =
        fe_look.get_or_insert_with(|| (crate::looks::load(FE_STADIUM), crate::looks::Watch::new(FE_STADIUM)));
    let saved = watch.poll();
    if let Some(s) = saved {
        *settings = s;
    }
    renderer.settings = *settings;
    stadium::apply_env(renderer);
    let Some(models) = stadium_models else { return };
    if saved.is_none() && probes_for.as_deref() == Some(FE_STADIUM) {
        return;
    }
    if let Some(l) = lights {
        renderer.lights = l;
    }
    let models: Vec<&GpuModel> = models.iter().chain(hoops).collect();
    // the box round the stadium; its surroundings (the remastered one's) seen from the probes, but not boxed
    if let Some((lo, hi)) = stadium::probe_box(&models) {
        let seen: Vec<&GpuModel> = models.iter().copied().chain(surroundings).collect();
        renderer.bake_probes(gpu, &seen, lo, hi, crate::arena::PROBES);
    }
    *probes_for = Some(FE_STADIUM.into());
}
