//! Print one JSON line per model with what `tests/model_parity.py` compares against the glTF exports:
//! `cargo run --release -p qwc-assets --example model_summary -- original/cd_files`

use qwc_assets::{model, TextureIndex, Vfs};
use serde_json::json;

fn main() {
    let root = std::env::args().nth(1).expect("usage: model_summary DATA_DIR");
    let vfs = Vfs::open(&root).unwrap();
    let textures = TextureIndex::build(&vfs);
    for path in vfs.with_extension("fpm") {
        let line = match model::load(&vfs, &textures, path) {
            Err(e) => json!({"path": path, "error": e.to_string()}),
            Ok(m) => {
                let meshes: Vec<_> = m
                    .meshes
                    .iter()
                    .map(|me| {
                        let sum: Vec<f64> =
                            (0..3).map(|k| me.positions.iter().map(|p| p[k] as f64).sum::<f64>()).collect();
                        json!({
                            "material": me.material,
                            "vertices": me.positions.len(),
                            "indices": me.indices.len(),
                            "pos_sum": sum,
                            "morphs": me.morphs.len(),
                        })
                    })
                    .collect();
                json!({
                    "path": path,
                    "skeleton": m.skeleton.as_ref().map(|(p, _)| p),
                    "joints": m.skeleton.as_ref().map(|(_, s)| s.bones.len()),
                    "textured": m.materials.iter().map(|x| x.texture.is_some()).collect::<Vec<_>>(),
                    "channels": m.morph_channels(),
                    "meshes": meshes,
                })
            }
        };
        println!("{line}");
    }
}
