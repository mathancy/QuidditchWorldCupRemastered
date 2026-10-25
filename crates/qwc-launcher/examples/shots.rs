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

    // the first release's own notes (hard-wrapped Markdown, an alert, nested lists): the News page and the Home
    // card's scrolling notes
    let notes =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/releases/v0.1.0.md"))
            .expect("the release notes");
    let first = serde_json::json!([{"tag_name": "v0.1.0", "name": "QWC Remastered 0.1.0",
        "published_at": "2026-10-24T18:19:00Z", "body": notes, "assets": []}]);
    let releases = || qwc_launcher::updates::parse(&first.to_string()).unwrap();
    let mut news = Launcher::new(&root, Some(home_lib.clone()));
    news.open(qwc_launcher::app::Page::News);
    news.show_releases(releases());
    shoot_sized(out, "news_notes", news, 1400.0);
    let mut home = Launcher::new(&root, Some(home_lib.clone()));
    home.show_releases(releases());
    shoot(out, "home_notes", home);

    // the Packs page's Store view, from a store running on this computer (`store/seed/dev.sql`) when QWC_STORE_URL
    // names one
    if let Ok(base) = std::env::var("QWC_STORE_URL") {
        let page = qwc_launcher::store::browse(&base, &Default::default()).expect("the store");
        let mut store = Launcher::new(&root, Some(home_lib.clone()));
        store.show_store(page.packs.clone());
        shoot(out, "packs_store", store);
        accounts(out, &root, home_lib, page.packs);
    }
}

/// A made-up round picture: a gold disc on navy, `side` pixels.
fn made_up_picture(side: u32) -> Vec<u8> {
    (0..side * side)
        .flat_map(|i| {
            let (x, y) = ((i % side) as f32 - side as f32 / 2.0, (i / side) as f32 - side as f32 / 2.0);
            if x * x + y * y < (side as f32 / 3.5).powi(2) {
                [0xd4, 0xa6, 0x4a, 255]
            } else {
                [0x1b, 0x27, 0x40, 255]
            }
        })
        .collect()
}

/// The pack store's account: the bottom strip signed in and out, and the Profile page, yours and someone else's.
fn accounts(out: &Path, root: &Path, home_lib: &Path, packs: Vec<qwc_launcher::store::Listed>) {
    let me: qwc_launcher::account::Me = serde_json::from_value(serde_json::json!({
        "id": "acct-sam-0001", "name": "Seeker Sam", "discord_username": "seeker_sam", "show_discord": true,
        "uploaded_avatar": false, "role": "reviewer", "joined": "2026-10-24T20:00:00Z", "name_change_after": null
    }))
    .unwrap();
    let picture = || vec![("acct-sam-0001".to_string(), made_up_picture(128), 128, 128)];
    let mut home = Launcher::new(root, Some(home_lib.to_path_buf()));
    home.show_signed_in(Some(("token".into(), me.clone())), picture());
    shoot(out, "home_signed_in", home);
    let mut home = Launcher::new(root, Some(home_lib.to_path_buf()));
    home.show_signed_in(None, vec![]);
    shoot(out, "home_signed_out", home);

    let mut own: qwc_launcher::account::Profile = serde_json::from_value(serde_json::json!({
        "id": "acct-sam-0001", "name": "Seeker Sam", "discord_username": "seeker_sam",
        "joined": "2026-10-24T20:00:00Z", "role": "reviewer", "packs": []
    }))
    .unwrap();
    own.packs = packs.into_iter().take(2).collect();
    let mut mine = Launcher::new(root, Some(home_lib.to_path_buf()));
    mine.show_profile(own.clone(), Some(("token".into(), me)), picture());
    shoot_sized(out, "profile_own", mine, 1100.0);
    let mut theirs = Launcher::new(root, Some(home_lib.to_path_buf()));
    theirs.show_profile(own.clone(), None, picture());
    shoot(out, "profile_other", theirs);

    // collections: the Store's list with one opened, a profile's, and the Your packs view's saved orders
    let summary = |id: &str, name: &str, packs: u32, complete: bool| {
        serde_json::json!({ "id": id, "name": name, "description": "Every stadium at night, a quieter HUD, and the old rules.",
            "author": { "id": "acct-sam-0001", "name": "Seeker Sam" }, "revision": 2, "packs": packs, "complete": complete,
            "updated": "2026-10-24T21:00:00Z" })
    };
    let item = |id: &str, name: &str, version: &str, enabled: bool, listed: bool| {
        serde_json::json!({ "enabled": enabled, "listing": 1, "id": id, "name": name, "version": version, "kind": "Visual",
            "engine": ">=0.1", "url": "https://x/p", "sha256": "a", "size": 1, "listed": listed })
    };
    let list: Vec<qwc_launcher::store::CollectionSummary> = serde_json::from_value(serde_json::json!([
        summary("coll-night-01", "Night matches", 3, true),
        summary("coll-classic-1", "Classic feel", 2, false),
    ]))
    .unwrap();
    let mut opened = serde_json::json!({ "items": [
        item("ana.floodlights", "Floodlit night matches", "1.1.0", true, true),
        item("qwcr.clean-hud", "Clean HUD", "1.0.0", true, true),
        item("bo.classic-rules", "Classic rules", "0.3.0", false, true),
    ] });
    opened
        .as_object_mut()
        .unwrap()
        .extend(summary("coll-night-01", "Night matches", 3, true).as_object().unwrap().clone());
    let opened: qwc_launcher::store::Collection = serde_json::from_value(opened).unwrap();
    let mut store = Launcher::new(root, Some(home_lib.to_path_buf()));
    store.show_collections(list.clone(), vec![opened]);
    shoot(out, "store_collections", store);
    let mut with = own;
    with.collections = list;
    let mut theirs = Launcher::new(root, Some(home_lib.to_path_buf()));
    theirs.show_profile(with, None, picture());
    shoot_sized(out, "profile_collections", theirs, 1100.0);
    let mut orders = Launcher::new(root, Some(home_lib.to_path_buf()));
    orders.show_saved_orders(&["My usual", "Before Night matches"]);
    shoot_sized(out, "packs_saved_orders", orders, 1100.0);
    review(out, root, home_lib, me_for_review());
}

fn me_for_review() -> (String, qwc_launcher::account::Me) {
    let me = serde_json::from_value(serde_json::json!({
        "id": "acct-sam-0001", "name": "Seeker Sam", "discord_username": "seeker_sam", "show_discord": true,
        "uploaded_avatar": false, "role": "admin", "joined": "2026-10-24T20:00:00Z", "name_change_after": null
    }))
    .unwrap();
    ("token".to_string(), me)
}

/// Reviewing, rating and submitting: the Review page, the store's stars and Report, your submissions, the dialog.
fn review(out: &Path, root: &Path, home_lib: &Path, me: (String, qwc_launcher::account::Me)) {
    let pending: Vec<qwc_launcher::account::Pending> = serde_json::from_value(serde_json::json!([
        { "listing": 6, "id": "bo.starry-sky", "name": "Starry sky", "version": "0.2.0", "kind": "Visual", "engine": ">=0.1",
          "requires": [], "conflicts": [], "description": "A night sky with stars over every stadium.", "licence": "CC BY 4.0",
          "url": "https://github.com/bo/starry-sky/releases/download/v0.2.0/bo.starry-sky-0.2.0.qwcpack",
          "sha256": "f", "size": 2400000, "screenshots": ["https://example.invalid/sky.png"],
          "provenance": { "own_work": true, "recipes": false, "credits": "Bo; the sky photographs are mine" },
          "submitted": "2026-10-24T09:00:00Z", "author": { "id": "dev-bo", "name": "Bo" }, "listed_before": 0 },
        { "listing": 7, "id": "ana.floodlights", "name": "Floodlit night matches", "version": "1.2.0", "kind": "Visual",
          "engine": ">=0.1", "description": "Now with lens flares.", "licence": "CC BY", "url": "https://github.com/ana/f/releases/download/v1.2.0/f.qwcpack",
          "sha256": "1", "size": 2600000, "provenance": { "own_work": true, "recipes": true, "credits": "Ana" },
          "submitted": "2026-10-24T11:00:00Z", "author": { "id": "dev-ana", "name": "Ana" }, "listed_before": 2 }
    ]))
    .unwrap();
    let reports: Vec<qwc_launcher::account::OpenReport> = serde_json::from_value(serde_json::json!([
        { "id": 1, "target_kind": "listing", "target_id": "3", "reason": "broken", "note": "The keepers never save anything with this on.",
          "at": "2026-10-24T12:00:00Z", "reporter": "Ana", "about": "Classic rules 0.3.0", "state": "approved" }
    ]))
    .unwrap();
    let checked = qwc_launcher::store::Checked {
        file: std::path::PathBuf::new(),
        manifest: qwc_packs::Manifest::new("bo.starry-sky", "Starry sky"),
        fills: "12 texture, 1 look".into(),
        problems: vec![(
            qwc_packs::Severity::Warning,
            "textures/sky_dome.png: 4096 x 4096, larger than the remaster draws it".into(),
        )],
    };
    let mut page = Launcher::new(root, Some(home_lib.to_path_buf()));
    page.show_review(me.clone(), pending, reports, Some(checked));
    shoot_sized(out, "review", page, 1500.0);

    if let Ok(base) = std::env::var("QWC_STORE_URL") {
        // the store's packs with your rating and Report
        let packs = qwc_launcher::store::browse(&base, &Default::default()).expect("the store").packs;
        let mut store = Launcher::new(root, Some(home_lib.to_path_buf()));
        store.show_mine(me.clone(), vec![("ana.floodlights".into(), 4)], vec![]);
        store.show_store(packs);
        shoot(out, "store_rating", store);
        // the dialog on a sample pack (without its statement: not ready)
        let sample =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../store/.dev-files/files/bo.starry-sky-0.2.0.qwcpack");
        if sample.is_file() {
            let mut submit = Launcher::new(root, Some(home_lib.to_path_buf()));
            submit.show_mine(me.clone(), vec![], vec![]);
            submit.show_submit(
                sample,
                "https://github.com/bo/starry-sky/releases/download/v0.2.0/bo.starry-sky-0.2.0.qwcpack",
            );
            shoot(out, "submit", submit);
        }
    }
}
