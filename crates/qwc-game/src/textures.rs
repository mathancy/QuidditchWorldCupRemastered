//! `qwc-game textures`: a stadium's textures as PNGs to paint over, and the remastered list that draws them
//! (`qwc_assets::remaster`). Not the original's: the remaster's own tool.
//!
//! The stadium's models (`sky.fpm`, `stadium.fpm`, `crowdbox.fpm`) and its hoop's are read, and each texture their
//! materials draw (an animated material's frames too) is written as `OUT/NAME.png`, the lightmaps (`lm_*`, drawn
//! over their base as twice the base times the lightmap, so mid grey changes nothing) under `OUT/lightmaps/`.
//! `OUT/textures.txt` lists them all, so `--textures OUT/textures.txt`, or `include` in another list, draws them in
//! the disc's place. A PNG already there is kept, so painting is not lost by exporting again.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use qwc_assets::{model, texture, TextureIndex, Vfs};

use crate::{fail, Args};

pub fn textures(args: &Args, vfs: &Vfs) {
    let stadium = args.stadium.clone().unwrap_or_else(|| "england".into());
    let out = args.out.clone().unwrap_or_else(|| Path::new("work/textures").join(&stadium));
    let index = TextureIndex::build(vfs);
    let gfx = format!("stadium/{stadium}/gfx");
    let models = ["sky.fpm", "stadium.fpm", "crowdbox.fpm"].map(|m| format!("{gfx}/{m}"));
    let hoop = "stadium/hoopnorm/gfx/singlehoop.fpm".to_string();
    let mut paths = BTreeSet::new();
    for path in models.iter().chain([&hoop]).filter(|p| vfs.exists(p)) {
        let m = model::load(vfs, &index, path).unwrap_or_else(|e| fail(e));
        for mat in &m.materials {
            paths.extend(mat.texture.clone());
            if let Some((_, frames)) = index.stage(&mat.name, &m.path) {
                paths.extend(frames.into_iter().flatten().map(str::to_string));
            }
        }
    }
    if paths.is_empty() {
        fail(format!("{gfx}: no stadium models"));
    }
    let mut list = format!(
        "# The textures of stadium/{stadium} (`qwc-game textures`): each PNG here is drawn in its texture's place.\n\
         # Lightmaps (lightmaps/) are drawn over their base as twice the base times the lightmap: mid grey (128)\n\
         # leaves the base as it is, darker shades it, lighter brightens it.\n"
    );
    let (mut written, mut kept) = (0, 0);
    for path in &paths {
        let name = path.rsplit('/').next().unwrap_or(path).trim_end_matches(".fsh");
        let rel = if is_lightmap(name) { format!("lightmaps/{name}.png") } else { format!("{name}.png") };
        let png: PathBuf = out.join(&rel);
        if png.is_file() {
            kept += 1;
        } else {
            let t = match texture::load(vfs, path) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("{path}: {e}");
                    continue;
                }
            };
            save_png(&png, t.width, t.height, &t.levels[0]).unwrap_or_else(|e| fail(format!("{}: {e}", png.display())));
            written += 1;
        }
        list += &format!("{rel} = {path}\n");
    }
    let list_path = out.join("textures.txt");
    std::fs::write(&list_path, list).unwrap_or_else(|e| fail(format!("{}: {e}", list_path.display())));
    eprintln!(
        "{} textures of {gfx}: {written} written, {kept} kept as painted; the list is {}",
        paths.len(),
        list_path.display()
    );
}

/// Write RGBA8 `rgba` (its alpha kept) as a PNG at `path`, making its folders.
fn save_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    w.write_image_data(rgba).map_err(|e| e.to_string())
}

/// Whether a texture (its file name without `.fsh`) is a lightmap: the disc names them `lm_...`.
fn is_lightmap(name: &str) -> bool {
    name.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("lm_"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn lightmaps_by_name() {
        assert!(super::is_lightmap("lm_pitch10000"));
        assert!(super::is_lightmap("LM_Keep"));
        assert!(!super::is_lightmap("aus_st_wood"));
        assert!(!super::is_lightmap("lm"));
    }
}
