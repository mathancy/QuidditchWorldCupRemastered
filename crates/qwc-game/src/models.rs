//! The game's models, with the resource packs' glTF in their place (`qwc_packs::Stack::model`: a team's players under
//! `players/`, the balls under `props/`, any other model under `models/` by its path). Not the original's.
//!
//! A replacement for a skinned model (a player, a cutscene's character) is read on the original's skeleton, so the
//! original's animations play it: its joints are matched to the skeleton's bones by name, each vertex rides its
//! heaviest joint as the original's do (`qwc_assets::glb::load`), and its materials draw the images embedded in the
//! file, else the original's textures of the same name. A model without a skeleton (a ball, a hoop) is read as
//! static scenery (`qwc_assets::glb::load_scenery`). A replacement that does not read is reported and the original
//! drawn.

use qwc_assets::model::{self, Model};
use qwc_assets::{glb, TextureIndex, Vfs};

/// The model at the game path `path`: the packs' glTF for it when one is on, else the disc's.
pub(crate) fn load(vfs: &Vfs, textures: &TextureIndex, path: &str) -> Result<Model, String> {
    let original = model::load(vfs, textures, path).map_err(|e| e.to_string());
    let Some(file) = qwc_packs::current().model(path) else { return original };
    let skeleton = original.as_ref().ok().and_then(|m| m.skeleton.as_ref().map(|(p, _)| p.clone()));
    let replaced = match &skeleton {
        Some(skl) => glb::load(vfs, textures, &file, skl, path, true),
        None => glb::load_scenery(&file),
    };
    match replaced {
        Ok((m, report)) => {
            let what = if skeleton.is_some() { "on the original's skeleton" } else { "static" };
            eprintln!("{path}: {} ({what})", file.display());
            if !report.unmatched_joints.is_empty() {
                eprintln!(
                    "{}: joints with no bone of that name (their vertices ride the root): {}",
                    file.display(),
                    report.unmatched_joints.join(", ")
                );
            }
            for name in &report.skipped_materials {
                eprintln!("{}: material {name} has no texture: not drawn", file.display());
            }
            if !report.unnamed_morphs.is_empty() {
                eprintln!(
                    "{}: morph targets not named morphNN (the weight channel) are left out: {}",
                    file.display(),
                    report.unnamed_morphs.join(", ")
                );
            }
            Ok(m)
        }
        Err(e) => {
            eprintln!("{}: {e}; the game's model is drawn", file.display());
            original
        }
    }
}
