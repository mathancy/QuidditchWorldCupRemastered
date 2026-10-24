//! A stadium's remastered scenery from a resource pack (`stadiums/<folder>/`, `qwc_packs`): GLB files standing in for
//! the disc's `stadium.fpm` in the front end (`sum_fe`) and in a match (the match's stadium folder, `hog_sum`).
//! Not the original's.
//!
//! `stadium.glb` is the arena, the other files its surroundings; each is drawn unlit, as the original draws its
//! stadium. The folder's `stadium.ron` (`qwc_packs::StadiumConfig`) says whether the disc's hoops and flags are still
//! drawn, how much the disc's sky grows round the scenery (it would stand in front of far surroundings) and how far
//! the camera sees.

use std::path::{Path, PathBuf};

use qwc_assets::Vfs;
use qwc_packs::StadiumConfig;
use qwc_render::{Gpu, GpuModel, Renderer};

/// The scenery loaded: the arena, its surroundings, and its folder's config.
pub(crate) struct Scenery {
    pub arena: Vec<GpuModel>,
    pub surroundings: Vec<GpuModel>,
    pub config: StadiumConfig,
}

/// The GLB files of `dir`, the arena (`stadium.glb`) first.
pub(crate) fn files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> =
        std::fs::read_dir(dir).map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect()).unwrap_or_default();
    files.retain(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("glb")));
    files.sort_by_key(|p| (p.file_stem().is_none_or(|s| !s.eq_ignore_ascii_case("stadium")), p.clone()));
    files
}

/// The scenery of the stadium folder `stadium` the packs on have (the highest's), if any.
pub(crate) fn for_stadium(gpu: &Gpu, renderer: &mut Renderer, vfs: &Vfs, stadium: &str) -> Option<Scenery> {
    load(gpu, renderer, vfs, &qwc_packs::current().stadium(stadium)?)
}

/// The scenery in `dir`: none when no file of it loads.
pub(crate) fn load(gpu: &Gpu, renderer: &mut Renderer, vfs: &Vfs, dir: &Path) -> Option<Scenery> {
    let config = StadiumConfig::read(dir).unwrap_or_else(|e| {
        eprintln!("{e}; the defaults are used");
        StadiumConfig::default()
    });
    let mut s = Scenery { arena: Vec::new(), surroundings: Vec::new(), config };
    for (i, file) in files(dir).iter().enumerate() {
        let loaded = qwc_assets::glb::load_scenery(file).map_err(|e| e.to_string()).and_then(|(m, report)| {
            for name in &report.skipped_materials {
                eprintln!("{}: material {name} has no embedded image: not drawn", file.display());
            }
            renderer.upload_model(gpu, vfs, &m)
        });
        match loaded {
            Ok(mut g) => {
                renderer.set_lit(gpu, &mut g, false);
                eprintln!("{}: remastered scenery", file.display());
                if i == 0 {
                    s.arena.push(g);
                } else {
                    s.surroundings.push(g);
                }
            }
            Err(e) => eprintln!("{}: {e}", file.display()),
        }
    }
    (!s.arena.is_empty() || !s.surroundings.is_empty()).then_some(s)
}

/// Grow the disc's sky models (those that cast no shadow) round the scenery, `scale` times about their own centre.
pub(crate) fn grow_sky(gpu: &Gpu, renderer: &Renderer, sky: &[GpuModel], scale: f32) {
    if (scale - 1.0).abs() < f32::EPSILON {
        return;
    }
    for g in sky.iter().filter(|g| !g.casts_shadow) {
        let mut m = g.matrix();
        m.iter_mut().enumerate().filter(|(i, _)| i % 4 < 3).for_each(|(_, v)| *v *= scale);
        renderer.set_transform(&gpu.queue, g, Some(m));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arena_comes_first() {
        let dir = std::env::temp_dir().join(format!("qwc-game-scenery-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["a_trees.glb", "Stadium.GLB", "notes.txt", "castle.glb"] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        let names: Vec<String> =
            files(&dir).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["Stadium.GLB", "a_trees.glb", "castle.glb"]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
