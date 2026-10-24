//! QWC Studio (`qwc-studio`), the remaster's editor. Its Packs mode makes and changes resource packs
//! (`crate::packs`, `docs/LAUNCHER.md`); it began as an editor for the remaster's HUD layouts (`qwc_hud` scenes), drawn with the game's own
//! textures, the way Unreal's UMG designer edits a canvas panel: anchors, offsets, alignment, pivots and scales on
//! a canvas previewed at any screen size, and the values the game provides (a team's power, its score, its colour)
//! on sliders, so whatever they move (the Snitch halves, the bars) can be watched moving. Its Screens mode runs the
//! front end's screens by their own scripts and moves or hides their objects for the remaster (`crate::screens`).
//!
//! `qwc-studio [SCENE] [--screen NODE] [--look STADIUM] [--packs [--open DIR]] [--data DIR] [--mods DIR]...
//! [--scenes DIR] [--pack DIR] [--exe FILE] [--profile FILE] [--no-packs]`, from the repo's root:
//! - `--packs` opens the Packs mode, on the pack folder `--open` names (`studio.cmd`);
//! - `SCENE` is a HUD scene without its `.ron` (default `match`; `challenge`), as the game draws it: the highest
//!   pack's (its `hud/`), else the stock one. Save writes it to the folder `--scenes`, else `$QWC_HUD_SCENES`, else
//!   the highest pack's `hud/`;
//! - `--screen NODE` opens the Screens mode at that flow node (`StartScreen`, `MainMenuScreen`, `boot`); its
//!   layouts are the packs' (`qwc_frontend::layouts`), saved into `--pack`, else `$QWC_SCREEN_PACK`, else the
//!   highest pack's `screens/`;
//! - `--look STADIUM` opens the Looks mode at that stadium (`england`, `hog_win`): its remastered lighting and
//!   colour grade over a live match (`crate::looks`), kept in the highest pack's `looks/` (`$QWC_LOOKS`);
//! - the game data is found as the game finds it (`--data`, else `$QWC_DATA`, else the profile's, else
//!   `original/cd_files`), with the resource packs' `disc/` and `--mods` laid over it, and a decrypted exe for the
//!   load-set table as the game finds one (`--exe`, `$QWC_EXE`, the profile's, the data's `QWC.exe`,
//!   `analysis/QWC.exe`);
//! - the resource packs are the profile's (`qwc_packs`: `--profile FILE`, else `$QWC_PROFILE`, else the library's;
//!   `--no-packs` for none): their textures, fonts, looks, HUD scenes and screen layouts are the editor's.
//!
//! Without a pack on, nothing is saved: the game then draws the original's screens, HUD and fonts.

// the game's library links OpenH264, whose assembly and C both have a `dct.o`: MSVC's link warns (LNK4255) and
// drops that object's debug information, which is harmless
// in a release build on Windows the Studio is a window without a console (the Start menu's shortcut opens it)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(linker_messages)]

mod app;
mod canvas;
mod crop;
mod exact;
mod looks;
mod packs;
mod panels;
mod screens;
mod textures;

use std::path::PathBuf;

use eframe::egui;
use qwc_assets::Vfs;

const USAGE: &str =
    "qwc-studio [SCENE] [--screen NODE] [--look STADIUM] [--packs [--open DIR]] [--data DIR] [--mods DIR]... \
                     [--scenes DIR] [--pack DIR] [--exe FILE] [--profile FILE] [--no-packs]";

/// A release build on Windows has no console of its own: started from one (a terminal, `studio.cmd`), it writes its
/// messages there.
fn attach_parent_console() {
    #[cfg(all(windows, not(debug_assertions)))]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn AttachConsole(process: u32) -> i32;
        }
        const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
        // SAFETY: a plain Win32 call with no pointers; failing (no parent console) changes nothing
        unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    }
}

/// The Studio's look: the launcher's (`qwc_style`: night blue, gold, Germania One and Cardo), compact.
pub fn theme(ctx: &egui::Context) {
    qwc_style::install(ctx, &qwc_style::find_root(), qwc_style::Density::Compact);
}

/// For a test's harness, which draws its first frame as it is built: the theme put on, once (true then: that frame
/// draws nothing, as the window's first frame comes after its theme).
#[cfg(test)]
pub fn theme_once(ui: &egui::Ui) -> bool {
    let id = egui::Id::unique("qwc-studio themed");
    if ui.ctx().data(|d| d.get_temp::<bool>(id)).is_some() {
        return false;
    }
    theme(ui.ctx());
    ui.ctx().data_mut(|d| d.insert_temp(id, true));
    true
}

fn main() -> eframe::Result {
    attach_parent_console();
    // the profile is read first: the default folders below come from its packs
    let all: Vec<String> = std::env::args().skip(1).collect();
    let profile = all.windows(2).find(|w| w[0] == "--profile").map(|w| PathBuf::from(&w[1]));
    let packs = qwc_packs::init(profile.as_deref(), all.iter().any(|a| a == "--no-packs"));
    let mut data = std::env::var_os("QWC_DATA")
        .map(PathBuf::from)
        .or_else(|| packs.profile.data.clone())
        .unwrap_or_else(|| "original/cd_files".into());
    // the HUD scenes are saved where the game reads them first
    let mut scenes = std::env::var_os("QWC_HUD_SCENES")
        .map(PathBuf::from)
        .or_else(|| packs.packs.first().map(|p| p.dir.join("hud")));
    let mut pack = qwc_frontend::layouts::default_pack();
    let (mut mods, mut exe, mut screen, mut look) = (Vec::new(), packs.profile.exe.clone(), None, None);
    let (mut packs_mode, mut open) = (false, None::<PathBuf>);
    let mut scene = "match".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = || {
            args.next().unwrap_or_else(|| {
                eprintln!("{a} needs a value\n{USAGE}");
                std::process::exit(2)
            })
        };
        match a.as_str() {
            "--data" => data = value().into(),
            "--mods" => mods.push(PathBuf::from(value())),
            "--scenes" => scenes = Some(value().into()),
            "--pack" => pack = Some(value().into()),
            "--screen" => screen = Some(value()),
            "--look" => look = Some(value()),
            "--packs" => packs_mode = true,
            "--open" => {
                open = Some(value().into());
                packs_mode = true;
            }
            "--exe" => exe = Some(PathBuf::from(value())),
            // read above
            "--profile" => {
                value();
            }
            "--no-packs" => {}
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            _ if a.starts_with('-') => {
                eprintln!("unknown option {a}\n{USAGE}");
                std::process::exit(2)
            }
            _ => scene = a.trim_end_matches(".ron").to_string(),
        }
    }
    let mut vfs = Vfs::open(&data).unwrap_or_else(|e| {
        eprintln!("game data at {}: {e}", data.display());
        std::process::exit(1)
    });
    for dir in packs.disc_overlays().iter().chain(&mods) {
        if let Err(e) = vfs.add_overlay(dir) {
            eprintln!("mods at {}: {e}", dir.display());
            std::process::exit(1)
        }
    }
    // the front end's session keeps the data for the program's life
    let vfs: &'static Vfs = Box::leak(Box::new(vfs));
    let exes = qwc_frontend::session::exe_candidates(exe.as_deref(), &data);
    let mode = match (&screen, &look) {
        _ if packs_mode => app::Mode::Packs,
        (_, Some(_)) => app::Mode::Looks,
        (Some(_), None) => app::Mode::Screens,
        (None, None) => app::Mode::Scenes,
    };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("QWC Studio")
        .with_inner_size([1600.0, 960.0])
        .with_drag_and_drop(true);
    if let Some(icon) = qwc_style::icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(
        "QWC Studio",
        options,
        Box::new(move |cc| {
            theme(&cc.egui_ctx);
            let mut screens = screens::Screens::new(vfs, exes, pack, screen);
            screens.exact = exact::Exact::new(cc.wgpu_render_state.clone());
            let looks = looks::Looks::new(vfs, cc.wgpu_render_state.clone(), look);
            let mut editor = app::Editor::new(vfs, scenes, &scene, screens, looks, mode);
            if let Some(dir) = &open {
                editor.packs.open(dir);
            }
            Ok(Box::new(editor))
        }),
    )
}
