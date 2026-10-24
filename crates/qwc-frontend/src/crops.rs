//! Not the original's: pictures the remaster's layouts cut out of the disc's textures. A crop names a texture of the
//! data and the polygon of it kept; a layout's picture `crop:NAME` shows it. Only the recipe is kept, never pixels:
//! the picture is cut at load from the player's own disc (or from a texture pack's PNG standing in for that texture,
//! the polygon scaled to it), so the layouts that use crops can be shared without any of the game's art.
//!
//! The crops are one RON file per pack, `crops.ron` in its layouts' folder (`crate::layouts`), a map from name to
//! [`Crop`], a higher pack's crop of a name standing in for a lower one's; the Studio's crop tool makes them.

use std::collections::BTreeMap;
use std::path::Path;

use qwc_assets::texture::Texture;
use serde::{Deserialize, Serialize};

/// The crops' file in the layouts' folder.
pub const FILE: &str = "crops.ron";

/// What a layout's picture starts with to show a crop.
pub const PREFIX: &str = "crop:";

/// The crops, by name.
pub type Crops = BTreeMap<String, Crop>;

/// A picture cut from a texture of the data.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Crop {
    /// The texture it is cut from, a path of the data (`frontend/common/bordertb/tb_template_swirlmask.fsh`).
    pub texture: String,
    /// The polygon kept, its corners in the texture's pixels: (0, 0) the texture's top left corner, (width, height)
    /// its bottom right. A pixel is kept when its centre is inside. Fewer than three corners keep the whole texture.
    pub points: Vec<[f32; 2]>,
    /// Keep what lies outside the polygon instead: the other part of a texture cut in two.
    #[serde(skip_serializing_if = "is_false")]
    pub outside: bool,
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// The layout picture that shows crop `name`.
pub fn spec(name: &str) -> String {
    format!("{PREFIX}{name}")
}

/// The crop a layout picture shows, when it shows one.
pub fn name(spec: &str) -> Option<&str> {
    spec.strip_prefix(PREFIX)
}

/// The crops in `dir`'s file: none without one (a file that does not read is reported).
pub fn read(dir: &Path) -> Crops {
    let path = dir.join(FILE);
    let Ok(text) = std::fs::read_to_string(&path) else { return Crops::new() };
    from_ron(&text).unwrap_or_else(|e| {
        eprintln!("{}: {e}; no crops", path.display());
        Crops::new()
    })
}

pub fn from_ron(text: &str) -> Result<Crops, String> {
    ron::from_str(text).map_err(|e| e.to_string())
}

pub fn to_ron(crops: &Crops) -> String {
    let config = ron::ser::PrettyConfig::new().indentor("    ").new_line("\n");
    let mut text = ron::ser::to_string_pretty(crops, config).unwrap_or_default();
    text.push('\n');
    text
}

/// Write the crops to `dir`'s file (removed when there are none), making the folder.
pub fn write(dir: &Path, crops: &Crops) -> std::io::Result<std::path::PathBuf> {
    let path = dir.join(FILE);
    if crops.is_empty() {
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        return Ok(path);
    }
    std::fs::create_dir_all(dir)?;
    std::fs::write(&path, to_ron(crops))?;
    Ok(path)
}

/// Whether `(x, y)` is inside the polygon `points` (even-odd, by the crossings of a ray to the right; a point on a
/// side the ray runs along or ends on counts once, so two crops of complementary halves share no pixel).
pub fn inside(points: &[[f32; 2]], x: f32, y: f32) -> bool {
    let mut odd = false;
    let mut j = points.len().wrapping_sub(1);
    for (i, a) in points.iter().enumerate() {
        let b = points[j];
        if (a[1] > y) != (b[1] > y) && x < (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]) + a[0] {
            odd = !odd;
        }
        j = i;
    }
    odd
}

impl Crop {
    /// Whether it cuts anything.
    pub fn cuts(&self) -> bool {
        self.points.len() >= 3
    }

    /// Whether the pixel at column `x`, row `y` of the texture, `scale` times the size the points are in, is kept.
    pub fn keeps(&self, x: u32, y: u32, scale: [f32; 2]) -> bool {
        if !self.cuts() {
            return true;
        }
        let (px, py) = ((x as f32 + 0.5) / scale[0], (y as f32 + 0.5) / scale[1]);
        inside(&self.points, px, py) != self.outside
    }

    /// `rgba` (`width` x `height`, 8 bits a channel) cut: the pixels not kept made transparent, their colour left
    /// (so filtering at the cut does not darken). `scale` is the picture's size over the size the points are in:
    /// 1 for the disc's texture, more for a texture pack's larger PNG standing in for it.
    pub fn cut(&self, rgba: &mut [u8], width: u32, height: u32, scale: [f32; 2]) {
        for y in 0..height {
            for x in 0..width {
                if !self.keeps(x, y, scale) {
                    rgba[((y * width + x) * 4 + 3) as usize] = 0;
                }
            }
        }
    }

    /// `tex` cut, its mip chain made again from the cut picture; `size` is the size the points are in (the disc
    /// texture's).
    pub fn apply(&self, tex: &Texture, size: [u16; 2]) -> Texture {
        let mut base = tex.levels[0].clone();
        let scale = [tex.width as f32 / size[0].max(1) as f32, tex.height as f32 / size[1].max(1) as f32];
        self.cut(&mut base, tex.width, tex.height, scale);
        let levels = qwc_assets::remaster::mip_chain(tex.width, tex.height, base);
        Texture { path: tex.path.clone(), width: tex.width, height: tex.height, levels }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The texture's top left half, below its diagonal from the bottom left corner to the top right.
    fn half(w: f32, h: f32, outside: bool) -> Crop {
        Crop { texture: "a.fsh".into(), points: vec![[0.0, 0.0], [w, 0.0], [0.0, h]], outside }
    }

    #[test]
    fn two_halves_share_no_pixel_and_miss_none() {
        let (a, b) = (half(8.0, 8.0, false), half(8.0, 8.0, true));
        for y in 0..8 {
            for x in 0..8 {
                assert_ne!(a.keeps(x, y, [1.0; 2]), b.keeps(x, y, [1.0; 2]), "{x}, {y}");
                // above the diagonal through the pixels' centres is kept, below it cut
                if x + y < 7 {
                    assert!(a.keeps(x, y, [1.0; 2]));
                }
                if x + y > 7 {
                    assert!(!a.keeps(x, y, [1.0; 2]));
                }
            }
        }
        // two polygons sharing a side (the square's halves the other way) share no pixel either
        let p = Crop { points: vec![[0.0, 0.0], [8.0, 0.0], [8.0, 8.0]], ..half(8.0, 8.0, false) };
        let q = Crop { points: vec![[0.0, 0.0], [8.0, 8.0], [0.0, 8.0]], ..half(8.0, 8.0, false) };
        for y in 0..8 {
            for x in 0..8 {
                assert_ne!(p.keeps(x, y, [1.0; 2]), q.keeps(x, y, [1.0; 2]), "{x}, {y}");
            }
        }
    }

    #[test]
    fn a_larger_picture_is_cut_in_the_same_place() {
        // a texture pack's PNG four times the disc texture's size: its own diagonal, not the small one's steps
        let c = half(4.0, 4.0, false);
        for y in 0..16 {
            for x in 0..16 {
                assert_eq!(c.keeps(x, y, [4.0; 2]), x + y < 15, "{x}, {y}");
            }
        }
    }

    #[test]
    fn a_cut_clears_alpha_only_and_remakes_the_mips() {
        let tex = Texture { path: "a.fsh".into(), width: 2, height: 2, levels: vec![vec![200; 16], vec![200; 4]] };
        // keeps the top left pixel only
        let c = Crop { points: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]], ..Crop::default() };
        let out = c.apply(&tex, [2, 2]);
        assert_eq!(&out.levels[0][..4], &[200; 4]);
        assert_eq!(&out.levels[0][4..8], &[200, 200, 200, 0]);
        assert_eq!(out.levels[1], [200, 200, 200, 50]);
        // fewer than three corners cut nothing
        let none = Crop { points: vec![[0.0, 0.0], [1.0, 1.0]], ..Crop::default() };
        assert_eq!(none.apply(&tex, [2, 2]).levels[0], tex.levels[0]);
    }

    #[test]
    fn the_file_reads_back() {
        let mut crops = Crops::new();
        crops.insert("left".into(), half(256.0, 256.0, false));
        crops.insert("right".into(), half(256.0, 256.0, true));
        let text = to_ron(&crops);
        assert!(!text.contains("outside: false"), "{text}");
        assert_eq!(from_ron(&text).unwrap(), crops);
        assert_eq!(name(&spec("left")), Some("left"));
        assert_eq!(name("left.png"), None);
    }
}
