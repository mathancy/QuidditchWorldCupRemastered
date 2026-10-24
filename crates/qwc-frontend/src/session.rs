//! The front end's scripts over the game data: every layer script loaded into the VM and linked, the strings and
//! the DataStore built as the boot builds them, and the screen manager over `game.flo` and `Screens.lst` started at
//! a node ([`Flow`]). The load-set table and the cards' numbers are read from a decrypted `QWC.exe` when one is
//! found ([`exe_candidates`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use qwc_anim::Rig;
use qwc_assets::frontend::Resources;
use qwc_assets::Vfs;
use qwc_feck::draw::{Assets, DrawList};
use qwc_feck::puppet::{PuppetSet, SetLoader};
use qwc_feck::text::Strings;
use qwc_feck::{FrontEnd, Vm};
use qwc_formats::fas::Fas;
use qwc_formats::flow::{name_hash, LoadSet};
use qwc_formats::sts::{Sdx, Sts};

use crate::disc::{packages, wipe_files};

/// Every layer script in the VM, linked, with the US strings and the DataStore built (`CreateDataStore`,
/// `InitialiseDataStore`), as the boot does.
pub struct Loaded {
    pub vm: Vm,
    pub fe: FrontEnd,
    /// The scripts' paths and files, by their index in the VM.
    pub files: Vec<String>,
    pub bytes: Vec<Vec<u8>>,
}

/// Load and link every `.vso` of the data, name the load-set slots (`sets`), and build the strings and the
/// DataStore.
pub fn load(vfs: &Vfs, sets: &[LoadSet]) -> Result<Loaded, String> {
    let files: Vec<String> = vfs.with_extension("vso").map(str::to_string).collect();
    let mut vm = Vm::new();
    let mut fe = FrontEnd::new();
    let mut bytes = Vec::new();
    for f in &files {
        let b = vfs.read(f).map_err(|e| e.to_string())?.to_vec();
        vm.load(f, &b).map_err(|e| format!("{f}: {e}"))?;
        bytes.push(b);
    }
    vm.link(&fe);
    fe.learn_names(&vm);
    // the slots' name hashes (`0x68d530`), before the DataStore's initialisers ask for any
    if let Some(n) = sets.iter().map(|s| s.slot + 1).max() {
        fe.load_sets = vec![0; n];
        for s in sets {
            fe.load_sets[s.slot] = s.hash;
        }
    }
    let read = |p: &str| vfs.read(p).map_err(|e| e.to_string());
    let sdx = Sdx::parse(&read("language/INDEX.SDX")?).map_err(|e| e.to_string())?;
    let sts = Sts::parse(&read("language/US/COMMON.STS")?).map_err(|e| e.to_string())?;
    fe.strings = Some(Strings::new(&sdx, sts));
    fe.languages = language_tables(vfs);
    if let Some(init) = vm.scripts.iter().position(|s| s.name.to_lowercase().contains("initialisedatastore")) {
        for (f, a) in [("CreateDataStore", vec![u32::MAX]), ("InitialiseDataStore", vec![])] {
            if let Err(e) = vm.execute(&mut fe, init, name_hash(f), &a) {
                eprintln!("{f}: {e}");
            }
        }
    }
    Ok(Loaded { vm, fe, files, bytes })
}

/// Where a decrypted exe may be, in order: `exe` (`--exe`), `$QWC_EXE`, the data folder's `QWC.exe`, then
/// `analysis/QWC.exe`.
pub fn exe_candidates(exe: Option<&Path>, data: &Path) -> Vec<PathBuf> {
    exe.map(Path::to_path_buf)
        .into_iter()
        .chain(std::env::var_os("QWC_EXE").map(PathBuf::from))
        .chain([data.join("QWC.exe"), "analysis/QWC.exe".into()])
        .collect()
}

/// A table read from the first of `exes` it reads from. None, with a note saying `without` happens instead, when none
/// does.
fn from_exe<T>(
    exes: &[PathBuf],
    what: &str,
    without: &str,
    read: impl Fn(&[u8]) -> qwc_formats::Result<T>,
) -> Option<T> {
    let mut tried = Vec::new();
    for p in exes {
        let Ok(d) = std::fs::read(p) else { continue };
        match read(&d) {
            Ok(t) => return Some(t),
            Err(e) => tried.push(format!("{}: {e}", p.display())),
        }
    }
    eprintln!("no {what} (give a decrypted QWC.exe with --exe); {without}");
    for t in tried {
        eprintln!("  {t}");
    }
    None
}

/// The load-set table (`qwc_formats::flow::loadset_table`), empty without one.
pub fn loadset_table(exes: &[PathBuf]) -> Vec<LoadSet> {
    let without = "resources go by folder names";
    from_exe(exes, "load-set table", without, qwc_formats::flow::loadset_table).unwrap_or_default()
}

/// The cards' numbers (`qwc_formats::exe::card_numbers`), for the Game Boy Advance packets.
pub fn card_numbers(exes: &[PathBuf]) -> Option<qwc_formats::exe::CardNumbers> {
    from_exe(exes, "card table", "the Game Boy Advance packets hold no cards", qwc_formats::exe::card_numbers)
}

/// The puppets' clip sets (`0x4dc200`): the animation resource's `name.fas`, decoded with `name.btf`'s triggers,
/// and the skeleton beside it. Each set is decoded once.
pub fn puppet_sets(vfs: &'static Vfs, resources: Resources) -> SetLoader {
    let mut sets: HashMap<String, Option<PuppetSet>> = HashMap::new();
    Box::new(move |script, r| {
        let Some(path) = resources.path_of(script, r.id, 0).map(str::to_string) else {
            eprintln!("puppet clips {}::{}: no file", r.group, r.name);
            return None;
        };
        sets.entry(path.clone())
            .or_insert_with(|| match puppet_set(vfs, &path) {
                Ok(s) => Some(s),
                Err(e) => {
                    eprintln!("puppet clips {path}: {e}");
                    None
                }
            })
            .clone()
    })
}

fn puppet_set(vfs: &Vfs, path: &str) -> Result<PuppetSet, String> {
    if !path.to_ascii_lowercase().ends_with(".fas") {
        return Err("not a clip set".into());
    }
    let bytes = vfs.read(path).map_err(|e| e.to_string())?;
    let fas = Fas::parse(&bytes).map_err(|e| e.to_string())?;
    let btf_path = format!("{}.btf", &path[..path.len() - 4]);
    let btf = match vfs.exists(&btf_path) {
        true => Some(
            qwc_formats::btf::Btf::parse(&vfs.read(&btf_path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?,
        ),
        false => None,
    };
    let clips = qwc_anim::control::decode_set(&fas, btf.as_ref(), None).map_err(|e| e.to_string())?;
    let pose = qwc_assets::anim::set_skeleton(vfs, path, &fas).ok().map(|(_, s)| Rig::new(s).bind_pose());
    Ok(PuppetSet { clips, pose })
}

/// The screen manager over `game.flo` and `Screens.lst`, started at a node (or the boot node), with the resources
/// indexed.
pub struct Flow {
    pub vm: Vm,
    pub fe: FrontEnd,
    pub m: qwc_feck::screens::Manager,
    pub resources: Resources,
    /// Started at the boot node.
    pub boot: bool,
    /// The load-set table (empty without a decrypted exe).
    pub sets: Vec<LoadSet>,
    /// The scripts' paths and files, by their index in the VM.
    pub files: Vec<String>,
    pub bytes: Vec<Vec<u8>>,
}

impl Flow {
    /// Not the original's: the boot's card check (`PreFlow_TrcStartGame`) is drawn only while it shows a card
    /// message. On PC with a working save folder it shows none and leaves after a few frames (the check's three
    /// steps and the screen's transitions), which drew its red backdrop between the language select and the movies.
    pub fn hides_screen(&self) -> bool {
        let m = &self.m;
        let name = m.flow.nodes.get(m.node).and_then(|n| n.name.as_deref());
        name == Some("PreFlow_TrcStartGame") && !self.fe.memcard.message_up
    }

    /// The frame's draw list ([`Flow::hides_screen`]: an empty one).
    pub fn draw(&mut self, assets: &dyn Assets) -> DrawList {
        let list = self.m.draw(&mut self.fe, &self.vm, assets);
        if self.hides_screen() {
            DrawList::default()
        } else {
            list
        }
    }

    /// The flow's node names, by index (`None` for an unnamed node).
    pub fn node_name(&self, n: usize) -> String {
        self.m.flow.nodes.get(n).and_then(|x| x.name.clone()).unwrap_or(format!("{n}"))
    }
}

/// The flow over `vfs`, its tables from the first of `exes` that has them, started at the node named `start`
/// (any case), or the boot node for `None` or `boot`.
pub fn start_flow(vfs: &'static Vfs, exes: &[PathBuf], start: Option<&str>) -> Result<Flow, String> {
    start_flow_in(vfs, exes, start, None)
}

/// Every language's string table in the data (`language/<LANGUAGE>/COMMON.STS`), in [`LANGUAGES`]' order; a table
/// that does not read is left out.
///
/// [`LANGUAGES`]: qwc_formats::sts::LANGUAGES
pub fn language_tables(vfs: &Vfs) -> Vec<Sts> {
    qwc_formats::sts::LANGUAGES
        .iter()
        .filter_map(|(folder, _, _)| Sts::parse(&vfs.read(&format!("language/{folder}/COMMON.STS")).ok()?).ok())
        .collect()
}

/// [`start_flow`] in `language` (its hash name, `French`; `qwc_formats::sts::LANGUAGES`): set before the screens
/// choose their fonts and logo, as the original's start-up language is. A language the data lacks is reported and
/// the game starts in US English.
pub fn start_flow_in(
    vfs: &'static Vfs,
    exes: &[PathBuf],
    start: Option<&str>,
    language: Option<&str>,
) -> Result<Flow, String> {
    use qwc_feck::screens::Manager;
    let sets = loadset_table(exes);
    let Loaded { mut vm, mut fe, files, bytes } = load(vfs, &sets)?;
    if let Some(name) = language {
        if !fe.set_language(&mut vm.mem, name_hash(name)) {
            eprintln!("{name}: no such language in the game's data; US English instead");
        }
    }
    fe.card_numbers = card_numbers(exes);
    // **[I]** the original's path for a PC without render-to-texture (`General_UseRenderToTexture` answers
    // `[0x5d01a5]`): the `_LoPC` screens show the Quidditch cards by swapping the card window's texture, which the port
    // draws, where the others render them into regions it does not have yet. `QWC_RENDER_TO_TEXTURE=1` keeps 1.
    fe.render_to_texture = std::env::var_os("QWC_RENDER_TO_TEXTURE").is_some();
    let read = |p: &str| vfs.read(p).map_err(|e| e.to_string());
    let flow = qwc_formats::flow::Flow::parse(&read("frontend/ScrnLst/game.flo")?).map_err(|e| e.to_string())?;
    let defs = qwc_formats::flow::parse_screens(&read("frontend/ScrnLst/Screens.lst")?).map_err(|e| e.to_string())?;
    let run = |s, f| vm.execute(&mut fe, s, f, &[]).ok();
    let resources = Resources::build_in(vfs, bytes.iter().map(|b| b.as_slice()), run, &packages(&sets));
    fe.wipe_files = wipe_files(vfs, &resources);
    // the puppets get their clips as their layers build
    fe.anim_sets = Some(puppet_sets(vfs, resources.clone()));
    let mut m = Manager::new(flow, defs, &vm, bytes.iter().cloned().enumerate().collect());
    let boot = start.is_none_or(|n| n.eq_ignore_ascii_case("boot"));
    let at = match start.filter(|_| !boot) {
        None => m.boot_node(),
        Some(want) => m.flow.nodes.iter().position(|n| n.name.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(want))),
    };
    let at = at.ok_or_else(|| format!("no flow node {}", start.unwrap_or("boot")))?;
    m.start(at);
    Ok(Flow { vm, fe, m, resources, boot, sets, files, bytes })
}

/// The flow's steps from `from` on, printed: the screens entered, the errors and refusals (with `QWC_FLOW_LOG`,
/// every state and change too).
pub fn report(f: &Flow, from: usize) {
    use qwc_feck::screens::Step;
    let verbose = std::env::var_os("QWC_FLOW_LOG").is_some();
    for s in &f.m.steps[from.min(f.m.steps.len())..] {
        match s {
            Step::Entered(n) => eprintln!("entered {}", f.node_name(*n)),
            Step::State { node, state } if verbose => eprintln!("screen {node:#010x}: state {state}"),
            Step::Change { from, to } if verbose => eprintln!("flow {} -> {}", f.node_name(*from), f.node_name(*to)),
            Step::Error(e) | Step::Refused(e) => eprintln!("{e}"),
            _ => {}
        }
    }
}

/// With `QWC_FE_LOG` set: the scripts' log lines (`FECK_Printf`, failed callbacks) since the last call, with the
/// frame and the memory card's state.
pub fn log_lines(fe: &FrontEnd, frame: usize, logged: &mut usize) {
    if std::env::var_os("QWC_FE_LOG").is_none() {
        return;
    }
    for l in &fe.log[(*logged).min(fe.log.len())..] {
        eprintln!("[{frame} card {:#x}.{}] {}", fe.memcard.state, fe.memcard.sub, l.trim_end());
    }
    *logged = fe.log.len();
}
