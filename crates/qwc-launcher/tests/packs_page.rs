//! The Packs page driven like a player would (egui_kittest, no GPU): a pack moved down, one turned on, one removed,
//! each written to the profile at once.

use std::path::{Path, PathBuf};

use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use qwc_launcher::app::{Launcher, Page};
use qwc_packs::Profile;

/// A library with game files (empty stand-ins: the page does not read them) and three packs.
fn library() -> PathBuf {
    let lib = std::env::temp_dir().join(format!("qwc-launcher-packs-page-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&lib);
    std::fs::create_dir_all(lib.join("game/usa")).unwrap();
    std::fs::write(lib.join("game/usa/QWC.exe"), b"").unwrap();
    for (id, name) in [("a.first", "First Pack"), ("b.second", "Second Pack"), ("c.third", "Third Pack")] {
        let dir = lib.join("packs").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("pack.ron"), format!("Pack(id: \"{id}\", name: \"{name}\", version: \"1.0.0\")"))
            .unwrap();
    }
    let profile = Profile {
        data: Some(lib.join("game/usa")),
        exe: Some(lib.join("game/usa/QWC.exe")),
        packs: vec![
            qwc_packs::Entry { id: "a.first".into(), enabled: true },
            qwc_packs::Entry { id: "b.second".into(), enabled: false },
        ],
        ..Profile::default()
    };
    profile.write(&lib.join("profile.ron")).unwrap();
    lib
}

fn order(lib: &Path) -> Vec<(String, bool)> {
    Profile::read(&lib.join("profile.ron")).unwrap().packs.into_iter().map(|e| (e.id, e.enabled)).collect()
}

#[test]
fn moves_turns_on_and_removes() {
    let lib = library();
    let mut launcher = Launcher::new(Path::new("."), Some(lib.clone()));
    launcher.open(Page::Packs);
    let mut h =
        Harness::builder().with_size([1060.0, 1400.0]).build_ui_state(|ui, l: &mut Launcher| l.ui(ui), launcher);
    h.run();

    // the first pack down below the second
    h.get_all_by_label("Down").next().unwrap().click();
    h.run();
    // the unlisted third pack is listed, off, at the bottom
    assert_eq!(order(&lib), [("b.second".into(), false), ("a.first".into(), true), ("c.third".into(), false)]);

    h.get_by_label("Second Pack").click();
    h.run();
    assert_eq!(order(&lib)[0], ("b.second".into(), true));

    // removing asks first, then deletes the folder
    h.get_all_by_label("Remove").nth(2).unwrap().click();
    h.run();
    assert!(lib.join("packs/c.third").is_dir());
    h.get_by_label("Delete it").click();
    h.run();
    assert!(!lib.join("packs/c.third").exists());
    assert_eq!(order(&lib).len(), 2);
    let _ = std::fs::remove_dir_all(lib);
}
