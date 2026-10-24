//! Mount the real disc and check every archive member against `extracted/` (written by `tools/ccd_extract.py`).
//! Runs only when `QWC_DATA_DIR` points at a repo root holding `original/cd_files/` and the bootstrap's
//! `extracted/` (e.g. `QWC_DATA_DIR=$PWD cargo test --release`); otherwise it is a no-op.

use std::path::PathBuf;

use qwc_assets::Vfs;

#[test]
fn disc_mounts_like_extracted() {
    let Some(root) = std::env::var_os("QWC_DATA_DIR").map(PathBuf::from) else {
        eprintln!("QWC_DATA_DIR not set; skipping");
        return;
    };
    let vfs = Vfs::open(root.join("original/cd_files")).unwrap();
    assert_eq!(vfs.archive_count(), 518);
    let (mut checked, mut loose) = (0, 0);
    for path in vfs.paths() {
        let on_disc = root.join("original/cd_files").join(path);
        if on_disc.is_file() {
            loose += 1;
            continue;
        }
        let want = std::fs::read(root.join("extracted").join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert!(*vfs.read(path).unwrap() == *want, "{path}: bytes differ from extracted/");
        checked += 1;
    }
    assert_eq!(checked, 5945, "every archive member is reachable under its own path");
    assert!(vfs.exists("ROOT\\BALLS\\SNITCH.FPM"));
    eprintln!("{checked} members match extracted/, {loose} loose files");
}

/// The stadiums' animated Groper materials: every stadium model's material that animates finds its frames.
#[test]
fn stadium_materials_animate() {
    let Some(root) = std::env::var_os("QWC_DATA_DIR").map(PathBuf::from) else {
        eprintln!("QWC_DATA_DIR not set; skipping");
        return;
    };
    let vfs = Vfs::open(root.join("original/cd_files")).unwrap();
    let textures = qwc_assets::TextureIndex::build(&vfs);
    let (mut animated, mut frames) = (0, 0);
    let models: Vec<String> = vfs
        .with_extension("fpm")
        .filter(|p| p.to_ascii_lowercase().starts_with("stadium/") && p.to_ascii_lowercase().contains("/gfx/"))
        .map(str::to_string)
        .collect();
    for path in &models {
        let Ok(m) = qwc_assets::model::load(&vfs, &textures, path) else { continue };
        for mat in &m.materials {
            if let Some((stage, found)) = textures.stage(&mat.name, path) {
                eprintln!(
                    "{path}: {} {} frames at {} fps, {:?}",
                    mat.name,
                    stage.frames.len(),
                    stage.fps,
                    stage.tcmods
                );
                assert!(found.iter().all(Option::is_some), "{path}: {}: a frame is missing: {found:?}", mat.name);
                animated += 1;
                frames += found.len();
            }
        }
    }
    eprintln!("{animated} animated materials, {frames} frames, over {} models", models.len());
    assert!(animated > 0);
}

/// A player exported to glTF by `tools/fpm_to_gltf.py` (`converted/models/`) reads back with the original's morph
/// targets: the same weight channels, and deltas of the same size, so a pack's replacement keeps its face moving.
#[test]
fn gltf_players_keep_their_morphs() {
    let Some(root) = std::env::var_os("QWC_DATA_DIR").map(PathBuf::from) else {
        eprintln!("QWC_DATA_DIR not set; skipping");
        return;
    };
    let file = root.join("converted/models/teams/Gryff/seeker.glb");
    if !file.is_file() {
        eprintln!("{}: not converted; skipping", file.display());
        return;
    }
    let vfs = Vfs::open(root.join("original/cd_files")).unwrap();
    let textures = qwc_assets::TextureIndex::build(&vfs);
    let original = qwc_assets::model::load(&vfs, &textures, "teams/Gryff/seeker.fpm").unwrap();
    let skl = original.skeleton.as_ref().unwrap().0.clone();
    let (glb, report) = qwc_assets::glb::load(&vfs, &textures, &file, &skl, "teams/Gryff/seeker.fpm", true).unwrap();
    assert!(report.unnamed_morphs.is_empty(), "{:?}", report.unnamed_morphs);
    assert!(!original.morph_channels().is_empty());
    assert_eq!(glb.morph_channels(), original.morph_channels());
    // the deltas' total length, every level's (the exporter writes them all), as a check that the same targets came
    // back
    let size = |m: &qwc_assets::model::Model| -> f64 {
        m.meshes
            .iter()
            .flat_map(|x| x.morphs.values().flatten())
            .map(|(_, d)| (d[0] as f64).hypot(d[1] as f64).hypot(d[2] as f64))
            .sum()
    };
    let (a, b) = (size(&original), size(&glb));
    assert!((a - b).abs() < a * 0.01, "{a} {b}");
    eprintln!("{} morph channels, {} targets, delta length {a:.3}", glb.morph_channels().len(), report.morph_targets);
}

/// The glTF exporter (`qwc_assets::gltf_out`) round trip: a player and a ball written and read back by the loaders
/// have the finest level's vertices, bones, morph channels and morphs, and their textures embedded.
#[test]
fn exported_models_read_back() {
    let Some(root) = std::env::var_os("QWC_DATA_DIR").map(PathBuf::from) else {
        eprintln!("QWC_DATA_DIR not set; skipping");
        return;
    };
    let vfs = Vfs::open(root.join("original/cd_files")).unwrap();
    let textures = qwc_assets::TextureIndex::build(&vfs);
    let dir = std::env::temp_dir().join(format!("qwc-assets-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let finest = |m: &qwc_assets::model::Model| -> (usize, Vec<u16>, f64) {
        let fine: Vec<_> = m.meshes.iter().filter(|x| x.level == 0).collect();
        let verts = fine.iter().map(|x| x.positions.len()).sum();
        let bones = fine.iter().flat_map(|x| x.bones.clone().unwrap_or_default()).collect();
        let size = fine
            .iter()
            .flat_map(|x| x.morphs.values().flatten())
            .map(|(_, d)| (d[0] as f64).hypot(d[1] as f64).hypot(d[2] as f64))
            .sum();
        (verts, bones, size)
    };

    let path = "teams/Gryff/seeker.fpm";
    let original = qwc_assets::model::load(&vfs, &textures, path).unwrap();
    let (glb, report) = qwc_assets::gltf_out::export(&vfs, &original);
    assert!(report.missing_textures.is_empty(), "{:?}", report.missing_textures);
    assert_eq!(report.joints, original.skeleton.as_ref().unwrap().1.bones.len());
    let file = dir.join("seeker.glb");
    std::fs::write(&file, &glb).unwrap();
    let skl = original.skeleton.as_ref().unwrap().0.clone();
    let (back, r) = qwc_assets::glb::load(&vfs, &textures, &file, &skl, path, true).unwrap();
    assert!(r.unmatched_joints.is_empty() && r.unnamed_morphs.is_empty() && r.skipped_materials.is_empty());
    assert_eq!(r.embedded_textures.len(), report.textures);
    let (a, b) = (finest(&original), finest(&back));
    assert_eq!((a.0, &a.1), (b.0, &b.1), "vertices and bones");
    assert!((a.2 - b.2).abs() < 1e-3 * a.2.max(1.0), "morphs {} {}", a.2, b.2);
    let fine_channels: Vec<u16> = {
        let mut c: Vec<u16> =
            original.meshes.iter().filter(|x| x.level == 0).flat_map(|x| x.morphs.keys().copied()).collect();
        c.sort_unstable();
        c.dedup();
        c
    };
    assert_eq!(back.morph_channels(), fine_channels);

    let ball = qwc_assets::model::load(&vfs, &textures, "root/Balls/quaffle.fpm").unwrap();
    let (glb, report) = qwc_assets::gltf_out::export(&vfs, &ball);
    let file = dir.join("quaffle.glb");
    std::fs::write(&file, &glb).unwrap();
    let (back, r) = qwc_assets::glb::load_scenery(&file).unwrap();
    assert!(r.skipped_materials.is_empty(), "{:?}", r.skipped_materials);
    assert_eq!(finest(&ball).0, finest(&back).0);
    eprintln!(
        "seeker: {} vertices, {} joints, {} morph channels; quaffle: {} vertices",
        a.0,
        report.joints,
        fine_channels.len(),
        finest(&ball).0
    );
    let _ = std::fs::remove_dir_all(dir);
}
