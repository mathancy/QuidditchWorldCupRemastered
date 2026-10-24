//! The launcher's pages drawn headless (egui_kittest on wgpu), for looking at without a window:
//! `cargo run -p qwc-launcher --example shots -- OUT_DIR SETUP_LIBRARY HOME_LIBRARY [EXE GAME_DIR]`.
//! `setup.png` is the first run on an empty library (the usual install found on its own), `refused.png` the setup
//! given `EXE` and `GAME_DIR` (a retail exe, a folder without the game), `home.png` a library with the game imported
//! (`home_match*.png` with a last match shown, `home_hover.png` the pointer on a tab), `packs.png` its Packs page,
//! `online.png` its Online page.

use std::path::{Path, PathBuf};

use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use qwc_launcher::app::{for_shots, Launcher};
use qwc_launcher::saves::Played;

fn shoot(out: &Path, name: &str, launcher: Launcher) {
    shoot_sized(out, name, launcher, 860.0);
}

/// A page drawn `height` points tall (a long page whole).
fn shoot_sized(out: &Path, name: &str, launcher: Launcher, height: f32) {
    shoot_with(out, name, launcher, [1360.0, height], None);
}

/// A page drawn `size` points, the pointer over the widget labelled `hover` if given.
fn shoot_with(out: &Path, name: &str, launcher: Launcher, size: [f32; 2], hover: Option<&str>) {
    let mut harness =
        Harness::builder().with_size(size).wgpu().build_ui_state(|ui, l: &mut Launcher| l.ui(ui), launcher);
    let start = std::time::Instant::now();
    harness.step();
    while harness.state().busy() && start.elapsed().as_secs() < 60 {
        harness.step();
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if let Some(label) = hover {
        harness.get_by_label(label).hover();
    }
    for _ in 0..4 {
        harness.step();
    }
    let image = harness.render().expect("a wgpu adapter");
    let path = out.join(format!("{name}.png"));
    image.save(&path).expect("the picture saves");
    println!("{}", path.display());
}

fn main() {
    let args: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let [out, setup_lib, home_lib, rest @ ..] = args.as_slice() else {
        eprintln!("shots OUT_DIR SETUP_LIBRARY HOME_LIBRARY [EXE GAME_DIR]");
        std::process::exit(2)
    };
    std::fs::create_dir_all(out).unwrap();
    let root = qwc_launcher::launch::resources_root();
    shoot(out, "setup", Launcher::new(&root, Some(setup_lib.clone())));
    if let [exe, game] = rest {
        shoot(out, "refused", for_shots(&root, Some(setup_lib.clone()), Some(exe), Some(game)));
    }
    shoot(out, "home", Launcher::new(&root, Some(home_lib.clone())));
    shoot_with(out, "home_hover", Launcher::new(&root, Some(home_lib.clone())), [1360.0, 860.0], Some("Packs"));
    // the last match as a scoreboard: a house match, the World Cup final, and in the smallest, a large and a 4K window
    let matches = [
        ("home_match", ["Gryff", "Slyth"], [180, 30], "hog_sum", "2026-10-23 21-04-37", [1360.0, 860.0]),
        ("home_match_final", ["England", "Bulgaria"], [150, 160], "qditch", "2026-10-22 19-41-02", [1360.0, 860.0]),
        ("home_match_small", ["Huffle", "Nordic"], [90, 240], "nordic", "2026-10-21 08-15-55", [1120.0, 720.0]),
        ("home_match_large", ["England", "Nordic"], [170, 0], "england", "2026-10-23 22-17-03", [2560.0, 1440.0]),
        ("home_match_4k", ["Raven", "Slyth"], [60, 210], "hog_win", "2026-10-23 22-30-41", [3840.0, 2160.0]),
    ];
    for (name, teams, score, stadium, when, size) in matches {
        let mut l = Launcher::new(&root, Some(home_lib.clone()));
        l.show_match(Played {
            when: when.into(),
            stadium: stadium.into(),
            teams: teams.map(String::from),
            score,
            kind: "quidd".into(),
            rules: Vec::new(),
        });
        shoot_with(out, name, l, size, None);
    }
    let mut packs = Launcher::new(&root, Some(home_lib.clone()));
    packs.open(qwc_launcher::app::Page::Packs);
    shoot(out, "packs", packs);
    let mut settings = Launcher::new(&root, Some(home_lib.clone()));
    settings.open(qwc_launcher::app::Page::Settings);
    shoot(out, "settings", settings);
    let mut whole = Launcher::new(&root, Some(home_lib.clone()));
    whole.open(qwc_launcher::app::Page::Settings);
    shoot_sized(out, "settings_whole", whole, 1500.0);
    let mut online = Launcher::new(&root, Some(home_lib.clone()));
    online.open(qwc_launcher::app::Page::Online);
    shoot_sized(out, "online", online, 1100.0);
    // the Join panel's check: a near host, a far one, none
    let lobby = |ms| {
        Ok(qwc_net::LobbyInfo {
            host: "81.2.69.160:20030".parse().unwrap(),
            build: format!("{}/1", env!("CARGO_PKG_VERSION")),
            about: b"team Japan
stadium japan"
                .to_vec(),
            rtt: std::time::Duration::from_millis(ms),
        })
    };
    for (name, answer) in [
        ("online_check_near", lobby(18)),
        ("online_check_far", lobby(170)),
        ("online_check_none", Err("no answer".into())),
    ] {
        let mut l = Launcher::new(&root, Some(home_lib.clone()));
        l.open(qwc_launcher::app::Page::Online);
        l.show_check("81.2.69.160", answer);
        shoot_sized(out, name, l, 1100.0);
    }
    let mut saves = Launcher::new(&root, Some(home_lib.clone()));
    saves.open(qwc_launcher::app::Page::Saves);
    shoot(out, "saves", saves);
    let mut news = Launcher::new(&root, Some(home_lib.clone()));
    news.open(qwc_launcher::app::Page::News);
    news.show_releases(
        qwc_launcher::updates::parse(
            r#"[{"tag_name": "v0.2.0", "name": "Packs and settings", "published_at": "2026-11-20T10:00:00Z",
                 "body": "The launcher manages resource packs.\nSettings for the display, graphics and sound.\nSaves are backed up before each match.",
                 "assets": [{"name": "QWCRemastered-0.2.0-setup.exe", "browser_download_url": "https://example.invalid/a", "size": 182000000},
                            {"name": "QWCRemastered-0.2.0-x86_64.AppImage", "browser_download_url": "https://example.invalid/b", "size": 190000000},
                            {"name": "SHA256SUMS", "browser_download_url": "https://example.invalid/c", "size": 200}]},
                {"tag_name": "v0.1.0", "name": "The first release", "published_at": "2026-11-01T10:00:00Z",
                 "body": "Setup, Play, and the remaster's front end.", "assets": []}]"#,
        )
        .unwrap(),
    );
    shoot(out, "news", news);
}
