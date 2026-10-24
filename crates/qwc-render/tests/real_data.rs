//! Run every stadium's `wavyflags.cli` through the flag handlers (`qwc_render::flags`) and check what they set
//! against the totals in `docs/RE_NOTES.md` "Wavy flags". Runs only when `QWC_DATA_DIR` points at a repo root holding
//! the bootstrap's `extracted/` (e.g. `QWC_DATA_DIR=$PWD cargo test -p qwc-render`); otherwise it is a no-op.

use std::path::PathBuf;

use qwc_render::flags::{Kind, Store, WavyFlags};

#[test]
fn every_stadium_flag_script_runs() {
    let Some(root) = std::env::var_os("QWC_DATA_DIR").map(PathBuf::from) else {
        eprintln!("QWC_DATA_DIR not set; skipping");
        return;
    };
    let mut scripts = Vec::new();
    for dir in std::fs::read_dir(root.join("extracted/stadium")).unwrap().flatten() {
        for sub in ["gfx", "fx"] {
            let p = dir.path().join(sub).join("wavyflags.cli");
            if p.is_file() {
                scripts.push(p);
            }
        }
    }
    scripts.sort();
    let (mut flags, mut pennants, mut banners, mut gry) = (0, 0, 0, 0);
    for p in &scripts {
        let mut w = WavyFlags::new();
        let ignored = w.load(&std::fs::read(p).unwrap(), &Store::default());
        // every line does something, and every flag in use has a size and a texture
        assert!(ignored.is_empty(), "{}: {ignored:?}", p.display());
        for (i, f) in w.active() {
            assert!(f.length > 0.0 && f.height > 0.0 && f.texture.is_some(), "{} flag {i}: {f:?}", p.display());
        }
        flags += w.count;
        pennants += w.active().filter(|(_, f)| f.kind == Kind::Pennant).count();
        banners += w.active().filter(|(_, f)| f.kind == Kind::Banner).count();
        // outside a match every banner is the Gryffindor one
        gry += w.active().filter(|(_, f)| f.texture.as_ref().is_some_and(|t| t.name == "Gry_Banner")).count();
    }
    assert_eq!((scripts.len(), flags, pennants, banners, gry), (17, 373, 153, 220, 220));
}

/// The normals made for meshes without them ([`qwc_render::normals::smooth`]) face as the disc's own do, on models
/// that carry normals (the characters, a broom, the balls).
#[test]
fn made_normals_face_as_the_discs() {
    let Some(root) = std::env::var_os("QWC_DATA_DIR").map(PathBuf::from) else {
        eprintln!("QWC_DATA_DIR not set; skipping");
        return;
    };
    let vfs = qwc_assets::Vfs::open(root.join("original/cd_files")).unwrap();
    let textures = qwc_assets::TextureIndex::build(&vfs);
    let (mut agree, mut total) = (0usize, 0usize);
    for path in ["teams/England/chaser1.fpm", "teams/France/keeper.fpm", "root/Balls/quaffle.fpm"] {
        let m = qwc_assets::model::load(&vfs, &textures, path).unwrap();
        for mesh in m.meshes.iter().filter(|m| m.level == 0) {
            let Some(theirs) = &mesh.normals else { continue };
            let ours = qwc_render::normals::smooth(&mesh.positions, &mesh.indices);
            for (a, b) in ours.iter().zip(theirs) {
                let d = glam::Vec3::from(*a).dot(glam::Vec3::from(*b));
                total += 1;
                agree += usize::from(d > 0.5);
            }
        }
    }
    assert!(total > 1000 && agree * 100 >= total * 90, "{agree} of {total} within 60 degrees");
}
