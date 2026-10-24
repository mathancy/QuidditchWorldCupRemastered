//! `qwc-launcher`: the remaster's launcher window, or with a command its setup's steps without one:
//! - `check-exe FILE`: what an exe is (verified, readable, copy-protected, not the game's);
//! - `check-game DIR`: a game folder against the known editions;
//!   both exit with 3 when the setup cannot use it (the installer's setup pages ask them);
//! - `import EXE DIR`: both checked, then imported into the library (`$QWC_LIBRARY` elsewhere) and the profile
//!   pointed at them;
//! - `hashes DIR ID NAME`: an edition list of a game folder (`editions/*.txt`), printed;
//! - `check-updates`: the releases (`$QWC_RELEASES_URL` elsewhere) and whether one is newer.

// in a release build on Windows the launcher has no console window (its commands print to the one it was started
// from, if any)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// the game's crates are not linked here, but eframe's wgpu may warn the same way the HUD editor's link does
#![allow(linker_messages)]

use std::path::Path;

use qwc_launcher::setup::{self, Progress};

const USAGE: &str =
    "qwc-launcher [check-exe FILE | check-game DIR | import EXE DIR | hashes DIR ID NAME | check-updates]";

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

fn main() {
    attach_parent_console();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] => window(),
        ["check-exe", file] => {
            let c = setup::check_exe(Path::new(file));
            println!("{}  sha256 {}  {} load sets", c.verdict.message(), c.sha256, c.loadsets);
            if !c.verdict.usable() {
                std::process::exit(3)
            }
        }
        ["check-game", dir] => {
            let c = setup::check_game(Path::new(dir), &Progress::default()).unwrap_or_else(|e| fail(e));
            println!("{}", c.summary());
            for (what, list) in [("changed", &c.changed), ("missing", &c.missing), ("not in the edition", &c.extra)] {
                for f in list {
                    println!("  {what}: {f}");
                }
            }
            if !c.importable() {
                std::process::exit(3)
            }
        }
        ["import", exe, dir] => {
            let exe = setup::check_exe(Path::new(exe));
            println!("exe: {}", exe.verdict.message());
            let game = setup::check_game(Path::new(dir), &Progress::default()).unwrap_or_else(|e| fail(e));
            println!("game: {}", game.summary());
            let library = qwc_packs::library_dir().unwrap_or_else(|| fail("no library folder"));
            let profile = setup::import(&game, &exe, &library, &Progress::default()).unwrap_or_else(|e| fail(e));
            println!("imported into {}; profile {}", setup::game_dir(&library, &game).display(), profile.display());
        }
        ["hashes", dir, id, name @ ..] if !name.is_empty() => {
            let files = setup::hash_tree(Path::new(dir), &Progress::default()).unwrap_or_else(|e| fail(e));
            println!(
                "# {}: SHA-256 of the game files the remaster reads, made by `qwc-launcher hashes` (hashes",
                name.join(" ")
            );
            println!("# and file names only, no game data).");
            println!("edition {id} {}", name.join(" "));
            for (path, sha) in files {
                println!("{sha} {path}");
            }
        }
        ["check-updates"] => {
            let url = qwc_launcher::updates::releases_url();
            match qwc_launcher::updates::fetch(&url) {
                Ok(list) => {
                    for r in &list {
                        println!("{} {} {}", r.tag_name, r.date(), r.title());
                    }
                    let newer = qwc_launcher::updates::newer(&list, &qwc_launcher::updates::current());
                    println!(
                        "newer than {}: {}",
                        qwc_launcher::updates::current(),
                        newer.map_or("none".into(), |r| r.tag_name.clone())
                    );
                }
                Err(e) => println!("{url}: {e}"),
            }
        }
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2)
        }
    }
}

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("qwc-launcher: {msg}");
    std::process::exit(1)
}

fn window() {
    let root = qwc_launcher::launch::resources_root();
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title("QWC Remastered")
        .with_inner_size([1360.0, 860.0])
        .with_min_inner_size([1120.0, 720.0])
        // the launcher draws its own frame and top bar (`app::chrome`)
        .with_decorations(false);
    if let Some(icon) = qwc_launcher::style::icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    let result = eframe::run_native(
        "QWC Remastered",
        options,
        Box::new(move |_| {
            let mut launcher = qwc_launcher::app::Launcher::new(&root, qwc_packs::library_dir());
            launcher.check_updates_on_start();
            Ok(Box::new(launcher))
        }),
    );
    if let Err(e) = result {
        fail(e);
    }
}
