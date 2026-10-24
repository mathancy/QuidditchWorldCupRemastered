//! Texture lookup and decoding.
//!
//! Models name their textures; the name is the stem of a single-image `.fsh` file, or a Groper material (`.gro`)
//! whose first `map`/`clampmap`/`animmap` names the texture (`card_glow` -> `glow`). When several files share a
//! name, the one whose path shares the most leading directories with the model wins. This is the rule of
//! `tools/fpm_to_gltf.py` (`TextureIndex`); the game's own lookup has not been traced.

use std::collections::HashMap;

use qwc_formats::fsh::Fsh;
use qwc_formats::text_defs::{self, Ext, Parsed};

use crate::groper::{Render, Stage};
use crate::vfs::{normalize, Error, Result, Vfs};

/// A decoded texture: RGBA8 pixels of level 0 and of every stored mip level after it.
#[derive(Debug, Clone)]
pub struct Texture {
    pub path: String,
    pub width: u32,
    pub height: u32,
    /// `levels[0]` is the full image; level `l` is `max(1, width >> l) x max(1, height >> l)`.
    pub levels: Vec<Vec<u8>>,
}

impl Texture {
    pub fn rgba(&self) -> &[u8] {
        &self.levels[0]
    }

    /// True when any pixel is not fully opaque (the exporter's rule for alpha blending).
    pub fn has_alpha(&self) -> bool {
        self.levels[0].as_chunks::<4>().0.iter().any(|p| p[3] < 255)
    }
}

/// Decode image 0 of the `.fsh` at `path`, with its mip levels (at most down to 1x1).
pub fn load(vfs: &Vfs, path: &str) -> Result<Texture> {
    let bytes = vfs.read(path)?;
    let fsh = Fsh::parse(&bytes).map_err(|e| Error::Format(path.into(), e))?;
    let Some(img) = fsh.images.first() else {
        return Err(Error::Format(path.into(), qwc_formats::Error::Format("no images".into())));
    };
    let max_levels = (usize::BITS - img.width().max(img.height()).max(1).leading_zeros()) as usize;
    let mut levels = Vec::new();
    let (mut width, mut height) = (0, 0);
    for level in 0..(img.mip_count() + 1).min(max_levels) {
        let (w, h, rgba) = fsh.decode_rgba(0, level).map_err(|e| Error::Format(path.into(), e))?;
        if level == 0 {
            (width, height) = (w as u32, h as u32);
        }
        levels.push(rgba);
    }
    Ok(Texture { path: path.to_string(), width, height, levels })
}

/// Name -> texture path index over the whole file system.
#[derive(Debug, Default)]
pub struct TextureIndex {
    /// Lower-case stem -> paths of single-image `.fsh` files.
    by_name: HashMap<String, Vec<String>>,
    /// Lower-case material name -> (path of the `.gro`, first texture name, its stage when it animates).
    materials: HashMap<String, Vec<(String, String, Option<Stage>)>>,
    /// How each of those materials draws ([`Render`]), in the same order.
    renders: HashMap<String, Vec<Option<Render>>>,
}

/// Number of leading path components two paths share (case-insensitive).
fn shared_prefix(a: &str, b: &str) -> usize {
    let (a, b) = (normalize(a), normalize(b));
    a.split('/').zip(b.split('/')).take_while(|(x, y)| x == y).count()
}

fn stem(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.').map_or(name, |(s, _)| s)
}

/// The first texture of a Groper material: `map TEX`, `clampmap TEX` or `animmap FPS TEX...`.
fn first_map(stages: &[Vec<String>]) -> Option<String> {
    for directive in stages.iter().flatten() {
        let w: Vec<&str> = directive.split_whitespace().collect();
        let Some(op) = w.first().map(|s| s.to_ascii_lowercase()) else { continue };
        match op.as_str() {
            "map" | "clampmap" if w.len() > 1 => return Some(w[1].to_string()),
            "animmap" if w.len() > 2 => return Some(w[2].to_string()),
            _ => {}
        }
    }
    None
}

impl TextureIndex {
    pub fn build(vfs: &Vfs) -> TextureIndex {
        let mut ix = TextureIndex::default();
        for path in vfs.with_extension("fsh") {
            let single = vfs.read(path).ok().and_then(|b| Fsh::parse(&b).ok().map(|f| f.images.len() == 1));
            if single == Some(true) {
                ix.by_name.entry(stem(path).to_ascii_lowercase()).or_default().push(path.to_string());
            }
        }
        for path in vfs.with_extension("gro") {
            let Ok(bytes) = vfs.read(path) else { continue };
            let Ok(Parsed::Gro { materials }) = text_defs::parse(Ext::Gro, &bytes) else { continue };
            for (name, stages) in materials {
                if let Some(tex) = first_map(&stages) {
                    let stage = Stage::of(&stages);
                    let key = name.to_ascii_lowercase();
                    ix.renders.entry(key.clone()).or_default().push(Render::of(&stages));
                    ix.materials.entry(key).or_default().push((path.to_string(), tex, stage));
                }
            }
        }
        ix
    }

    /// The texture path for `name` as used by the model at `model_path`, if any.
    pub fn find(&self, name: &str, model_path: &str) -> Option<&str> {
        let best = |paths: &mut dyn Iterator<Item = (usize, usize)>| {
            // max_by_key keeps the last maximum; Python's max keeps the first, so compare on (score, -index).
            paths.max_by_key(|&(i, s)| (s, std::cmp::Reverse(i))).map(|(i, _)| i)
        };
        let key = name.to_ascii_lowercase();
        if let Some(cands) = self.by_name.get(&key) {
            let i = best(&mut cands.iter().enumerate().map(|(i, p)| (i, shared_prefix(p, model_path))))?;
            return Some(&cands[i]);
        }
        let (_, tex, _) = self.material(&key, model_path)?;
        if tex.eq_ignore_ascii_case(name) {
            return None;
        }
        self.find(tex, model_path)
    }

    /// The Groper material `name` nearest the model at `model_path`: (its `.gro`, first texture, animated stage).
    fn material(&self, name: &str, model_path: &str) -> Option<&(String, String, Option<Stage>)> {
        let i = self.nearest(name, model_path)?;
        self.materials.get(&name.to_ascii_lowercase())?.get(i)
    }

    /// Which of the Groper materials called `name` is nearest the model at `model_path` (the most leading folders
    /// shared, the first of those).
    fn nearest(&self, name: &str, model_path: &str) -> Option<usize> {
        let mats = self.materials.get(&name.to_ascii_lowercase())?;
        let score = |(i, (p, _, _)): (usize, &(String, String, Option<Stage>))| {
            (shared_prefix(p, model_path), std::cmp::Reverse(i))
        };
        mats.iter().enumerate().max_by_key(|&e| score(e)).map(|(i, _)| i)
    }

    /// How the Groper material `name` nearest the model at `model_path` draws its faces, in place of the name's
    /// flags ([`Render`]); `None` without one, or with one the port does not draw by its stages. Only a `.gro` in
    /// the model's folder or one above it counts. **[I]** The engine knows the `.gro` files loaded with the model's
    /// (its load set's); another stadium's or team's is never among them.
    pub fn render(&self, name: &str, model_path: &str) -> Option<Render> {
        let i = self.nearest(name, model_path)?;
        let (gro, _, _) = self.materials.get(&name.to_ascii_lowercase())?.get(i)?;
        let folder = |p: &str| normalize(p).rsplit_once('/').map_or(String::new(), |(d, _)| d.to_string());
        let (theirs, ours) = (folder(gro), folder(model_path));
        if !(ours == theirs || ours.starts_with(&format!("{theirs}/")) || theirs.is_empty()) {
            return None;
        }
        *self.renders.get(&name.to_ascii_lowercase())?.get(i)?
    }

    /// The animated stage of the Groper material `name` as the model at `model_path` uses it (an `animmap` of
    /// several frames, or `tcMod`s), with each frame's texture path. A name that is also a plain texture still
    /// animates when a material of that name does (the crowd's `crowdtest0000`-`0007`).
    pub fn stage(&self, name: &str, model_path: &str) -> Option<(&Stage, Vec<Option<&str>>)> {
        let stage = self.material(name, model_path)?.2.as_ref()?;
        let frames = stage
            .frames
            .iter()
            .map(|f| {
                let key = f.to_ascii_lowercase();
                let cands = self.by_name.get(&key)?;
                cands
                    .iter()
                    .enumerate()
                    .max_by_key(|&(i, p)| (shared_prefix(p, model_path), std::cmp::Reverse(i)))
                    .map(|(_, p)| p.as_str())
            })
            .collect();
        Some((stage, frames))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(shared_prefix("a/B/c.fsh", "A/b/d/e.fpm"), 2);
        assert_eq!(stem("x/y/Glow.FSH"), "Glow");
        let st = vec![vec!["blendfunc add".to_string()], vec!["animmap 10 f1 f2".to_string()]];
        assert_eq!(first_map(&st).as_deref(), Some("f1"));
        assert_eq!(first_map(&[vec!["map glow".to_string()]]).as_deref(), Some("glow"));
    }
}
