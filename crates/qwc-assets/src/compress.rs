//! Large textures compressed to BC7 for the GPU (a quarter of RGBA8's memory), each once: the result is kept in a
//! cache folder by the image's content, so later loads read it. Not the original's: the remaster's own, for the
//! resource packs' big pictures (a stadium's atlases, painted textures), which the original's small textures never
//! needed. Intel's ISPC encoder (`intel_tex_2`), its "fast" settings, on a band of block rows per thread.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use crate::texture::Texture;

// intel_tex_2's prebuilt kernels carry a C++ object (`ispc_texcomp_astc.cpp`), which needs the C++ runtime: MSVC's
// links on its own, on Linux it is named here
#[cfg(target_os = "linux")]
#[link(name = "stdc++")]
extern "C" {}

/// Textures with fewer pixels than this stay RGBA8 (small pictures gain little and keep their exact colours).
pub const MIN_PIXELS: u64 = 256 * 256;

/// The cache file's tag and version: a different one is not read.
const TAG: &[u8; 8] = b"QWCBC7\0\x01";

/// A texture's mip levels as BC7 blocks: level `l` is `ceil(w_l / 4) * ceil(h_l / 4)` blocks of 16 bytes, rows of
/// blocks top down, the level's size padded to whole blocks (edge pixels repeated).
#[derive(Debug, Clone, PartialEq)]
pub struct Bc7 {
    pub width: u32,
    pub height: u32,
    pub levels: Vec<Vec<u8>>,
}

/// Whether `t` is worth compressing (big enough, its sides whole blocks as wgpu asks of a compressed texture).
pub fn worth_it(t: &Texture) -> bool {
    t.width as u64 * t.height as u64 >= MIN_PIXELS && t.width.is_multiple_of(4) && t.height.is_multiple_of(4)
}

/// One level's RGBA8 (`w` x `h`) padded to whole blocks, its edge pixels repeated.
fn padded(rgba: &[u8], w: u32, h: u32) -> (Vec<u8>, u32, u32) {
    let (pw, ph) = (w.div_ceil(4) * 4, h.div_ceil(4) * 4);
    if (pw, ph) == (w, h) {
        return (rgba.to_vec(), w, h);
    }
    let mut out = vec![0u8; (pw * ph * 4) as usize];
    for y in 0..ph {
        let sy = y.min(h - 1);
        for x in 0..pw {
            let sx = x.min(w - 1);
            let s = ((sy * w + sx) * 4) as usize;
            let d = ((y * pw + x) * 4) as usize;
            out[d..d + 4].copy_from_slice(&rgba[s..s + 4]);
        }
    }
    (out, pw, ph)
}

/// One level's blocks: the padded image in bands of block rows, a band a thread.
fn encode_level(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let (data, pw, ph) = padded(rgba, w, h);
    let settings = intel_tex_2::bc7::alpha_fast_settings();
    let block_rows = ph / 4;
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(block_rows as usize).max(1);
    let per = block_rows.div_ceil(threads as u32);
    let bands: Vec<(u32, u32)> =
        (0..block_rows).step_by(per as usize).map(|start| (start, per.min(block_rows - start))).collect();
    let row_bytes = (pw * 4 * 4) as usize;
    let out: Vec<Vec<u8>> = std::thread::scope(|s| {
        let handles: Vec<_> = bands
            .iter()
            .map(|&(start, rows)| {
                let band = &data[start as usize * row_bytes..(start + rows) as usize * row_bytes];
                let settings = &settings;
                s.spawn(move || {
                    let surface = intel_tex_2::RgbaSurface { data: band, width: pw, height: rows * 4, stride: pw * 4 };
                    intel_tex_2::bc7::compress_blocks(settings, &surface)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("an encoder thread")).collect()
    });
    out.concat()
}

/// `t` as BC7, every level.
pub fn encode(t: &Texture) -> Bc7 {
    let levels = t
        .levels
        .iter()
        .enumerate()
        .map(|(l, rgba)| encode_level(rgba, (t.width >> l).max(1), (t.height >> l).max(1)))
        .collect();
    Bc7 { width: t.width, height: t.height, levels }
}

/// The cache key of `t`: its size and its pixels' hash.
fn key(t: &Texture) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (t.width, t.height, t.levels.len()).hash(&mut h);
    t.levels[0].hash(&mut h);
    format!("{:016x}-{}x{}", h.finish(), t.width, t.height)
}

fn cache_file(dir: &Path, t: &Texture) -> PathBuf {
    dir.join(format!("{}.bc7", key(t)))
}

fn read_cache(file: &Path, t: &Texture) -> Option<Bc7> {
    let b = std::fs::read(file).ok()?;
    if b.get(..8)? != TAG {
        return None;
    }
    let mut at = 8;
    let mut u32_at = |b: &[u8]| -> Option<u32> {
        let v = u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?);
        at += 4;
        Some(v)
    };
    let (w, h, n) = (u32_at(&b)?, u32_at(&b)?, u32_at(&b)?);
    if (w, h, n as usize) != (t.width, t.height, t.levels.len()) {
        return None;
    }
    let mut levels = Vec::with_capacity(n as usize);
    for l in 0..n {
        let len = ((w >> l).max(1).div_ceil(4) * (h >> l).max(1).div_ceil(4) * 16) as usize;
        levels.push(b.get(at..at + len)?.to_vec());
        at += len;
    }
    Some(Bc7 { width: w, height: h, levels })
}

fn write_cache(file: &Path, c: &Bc7) -> std::io::Result<()> {
    if let Some(d) = file.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut b = TAG.to_vec();
    for v in [c.width, c.height, c.levels.len() as u32] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    for l in &c.levels {
        b.extend_from_slice(l);
    }
    // written beside, then moved over, so a half-written file is never read
    let part = file.with_extension("part");
    std::fs::write(&part, b)?;
    std::fs::rename(part, file)
}

/// `t` as BC7, from the cache in `dir` when it holds it, else encoded now and kept there (a cache that cannot be
/// written is only reported).
pub fn cached(t: &Texture, dir: Option<&Path>) -> Bc7 {
    let file = dir.map(|d| cache_file(d, t));
    if let Some(c) = file.as_deref().and_then(|f| read_cache(f, t)) {
        return c;
    }
    let c = encode(t);
    if let Some(f) = &file {
        if let Err(e) = write_cache(f, &c) {
            eprintln!("{}: {e}", f.display());
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texture(w: u32, h: u32) -> Texture {
        let base: Vec<u8> = (0..w * h).flat_map(|i| [(i % 251) as u8, (i % 7 * 30) as u8, 200, 255]).collect();
        let mut levels = vec![base];
        let (mut lw, mut lh) = (w, h);
        while lw > 1 || lh > 1 {
            (lw, lh) = ((lw / 2).max(1), (lh / 2).max(1));
            levels.push(vec![128; (lw * lh * 4) as usize]);
        }
        Texture { path: "t".into(), width: w, height: h, levels }
    }

    #[test]
    fn levels_are_whole_blocks_and_the_cache_reads_back() {
        let t = texture(256, 256);
        assert!(worth_it(&t) && !worth_it(&texture(64, 64)));
        let c = encode(&t);
        assert_eq!(c.levels.len(), t.levels.len());
        assert_eq!(c.levels[0].len(), 64 * 64 * 16);
        assert_eq!(c.levels.last().unwrap().len(), 16, "1 x 1 is one block");
        let dir = std::env::temp_dir().join(format!("qwc-assets-bc7-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = cached(&t, Some(&dir));
        assert_eq!(first, c);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        assert_eq!(read_cache(&cache_file(&dir, &t), &t), Some(c));
        // another picture of the same size is another file
        let mut other = t.clone();
        other.levels[0][0] ^= 1;
        assert_eq!(read_cache(&cache_file(&dir, &other), &other), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn padding_repeats_the_edge() {
        let (p, w, h) = padded(&[1, 2, 3, 4, 5, 6, 7, 8], 2, 1);
        assert_eq!((w, h), (4, 4));
        assert_eq!(&p[12..16], &[5, 6, 7, 8]);
        assert_eq!(&p[(4 * 3) * 4..(4 * 3) * 4 + 4], &[1, 2, 3, 4]);
    }
}
