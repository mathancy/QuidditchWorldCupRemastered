//! Remastered textures: PNGs standing in for the disc's `.fsh` textures, by a list (`remastered/textures.txt`).
//! Not the original's: the remaster's own addition.
//!
//! Each line of the list is `PNG = TEXTURE [TEXTURE ...]`: the PNG's path relative to the list, then the game paths
//! of the textures it replaces (any case), or `include LIST`: another list, relative to this one (`qwc-game
//! textures` writes one per stadium). `#` starts a comment. The PNGs are decoded to RGBA8 with a mip chain
//! (2x2 boxes, colours weighted by alpha so transparent pixels do not darken the edges), since they are usually
//! drawn far smaller than they are.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::texture::Texture;
use crate::vfs::{normalize, Vfs};

/// The list: which PNG replaces each texture.
#[derive(Debug, Default, Clone)]
pub struct Remastered {
    /// Normalised texture path -> PNG.
    by_texture: HashMap<String, PathBuf>,
    /// Each PNG, in the list's order.
    pngs: Vec<PathBuf>,
}

impl Remastered {
    /// Read the list at `path`, and the lists it includes.
    pub fn read(path: &Path) -> Result<Remastered, String> {
        let mut r = Remastered::default();
        r.read_into(path, 0)?;
        Ok(r)
    }

    /// The lists at `lists`, each laid over the ones before it: where two replace the same texture, the later's
    /// PNG is drawn (the resource packs' order, the lowest first).
    pub fn read_layers(lists: &[PathBuf]) -> Result<Remastered, String> {
        let mut r = Remastered::default();
        for l in lists {
            r.layer(Remastered::read(l)?);
        }
        Ok(r)
    }

    /// Lay `over` on this list: its PNGs replace this one's for the textures both replace.
    pub fn layer(&mut self, over: Remastered) {
        self.by_texture.extend(over.by_texture);
        for p in over.pngs {
            if !self.pngs.contains(&p) {
                self.pngs.push(p);
            }
        }
    }

    fn read_into(&mut self, path: &Path, depth: usize) -> Result<(), String> {
        let fail = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
        if depth > 8 {
            return Err(fail(&"lists included too deep"));
        }
        let text = std::fs::read_to_string(path).map_err(|e| fail(&e))?;
        let base = path.parent().unwrap_or(Path::new("."));
        for list in self.parse_into(&text, base).map_err(|e| fail(&e))? {
            self.read_into(&list, depth + 1)?;
        }
        Ok(())
    }

    /// The list's text, its PNGs relative to `base` (an `include` line is refused: [`Remastered::read`] reads them).
    pub fn parse(text: &str, base: &Path) -> Result<Remastered, String> {
        let mut r = Remastered::default();
        if !r.parse_into(text, base)?.is_empty() {
            return Err("`include` needs the list read from a file".into());
        }
        Ok(r)
    }

    /// Add the list's text to this one, answering the lists it includes (`include LIST`, relative to `base`).
    fn parse_into(&mut self, text: &str, base: &Path) -> Result<Vec<PathBuf>, String> {
        let r = self;
        let mut includes = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if let Some(list) = line.strip_prefix("include ").filter(|_| !line.contains('=')) {
                includes.push(base.join(list.trim()));
                continue;
            }
            let (png, textures) = line.split_once('=').ok_or_else(|| format!("line {}: no `=`", n + 1))?;
            let png = base.join(png.trim());
            let mut any = false;
            for t in textures.split_whitespace() {
                if let Some(old) = r.by_texture.insert(normalize(t), png.clone()) {
                    return Err(format!("line {}: {t} is replaced by {} already", n + 1, old.display()));
                }
                any = true;
            }
            if !any {
                return Err(format!("line {}: no textures after `=`", n + 1));
            }
            r.pngs.push(png);
        }
        Ok(includes)
    }

    /// How many textures are replaced.
    pub fn len(&self) -> usize {
        self.by_texture.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_texture.is_empty()
    }

    /// The textures replaced (normalised paths).
    pub fn textures(&self) -> impl Iterator<Item = &str> {
        self.by_texture.keys().map(String::as_str)
    }

    /// The PNG replacing the texture at `path`.
    pub fn png(&self, path: &str) -> Option<&Path> {
        self.by_texture.get(&normalize(path)).map(PathBuf::as_path)
    }

    /// What is wrong with the list against the disc: PNGs that are missing, textures the disc does not have.
    pub fn problems(&self, vfs: &Vfs) -> Vec<String> {
        let mut out: Vec<String> =
            self.pngs.iter().filter(|p| !p.is_file()).map(|p| format!("{}: no such PNG", p.display())).collect();
        let mut missing: Vec<&String> = self.by_texture.keys().filter(|t| !vfs.exists(t)).collect();
        missing.sort();
        out.extend(missing.into_iter().map(|t| format!("{t}: no such texture on the disc")));
        out
    }
}

/// A PNG as a [`Texture`]: RGBA8 with its mip chain down to 1x1.
pub fn load_png(path: &Path) -> Result<Texture, String> {
    let fail = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let file = std::fs::File::open(path).map_err(|e| fail(&e))?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(|e| fail(&e))?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| fail(&e))?;
    let px = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px.as_chunks::<3>().0.iter().flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => px.as_chunks::<2>().0.iter().flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Indexed => return Err(fail(&"indexed colour left unexpanded")),
    };
    let levels = mip_chain(info.width, info.height, rgba);
    Ok(Texture { path: path.display().to_string(), width: info.width, height: info.height, levels })
}

/// Write `rgba` (8 bits a channel) as a PNG at `path`, making the folders on the way.
pub fn save_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let fail = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| fail(&e))?;
    }
    let file = std::fs::File::create(path).map_err(|e| fail(&e))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| fail(&e))?;
    writer.write_image_data(rgba).map_err(|e| fail(&e))?;
    writer.finish().map_err(|e| fail(&e))
}

/// Mip levels by 2x2 boxes (`levels[l]` is `max(1, w >> l) x max(1, h >> l)`, an odd side's last row or column
/// taken twice), the colour of each box the alpha-weighted mean of its pixels' (their plain mean when all four are
/// transparent).
pub fn mip_chain(w: u32, h: u32, base: Vec<u8>) -> Vec<Vec<u8>> {
    let mut levels = vec![base];
    let (mut cw, mut ch) = (w as usize, h as usize);
    while cw > 1 || ch > 1 {
        let (nw, nh) = ((cw / 2).max(1), (ch / 2).max(1));
        let src = levels.last().unwrap();
        let mut out = vec![0u8; nw * nh * 4];
        for y in 0..nh {
            for x in 0..nw {
                let (mut sum, mut plain, mut alpha) = ([0u32; 3], [0u32; 3], 0u32);
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let p = &src[((y * 2 + dy).min(ch - 1) * cw + (x * 2 + dx).min(cw - 1)) * 4..][..4];
                    let a = p[3] as u32;
                    for c in 0..3 {
                        sum[c] += p[c] as u32 * a;
                        plain[c] += p[c] as u32;
                    }
                    alpha += a;
                }
                let o = &mut out[(y * nw + x) * 4..][..4];
                for c in 0..3 {
                    o[c] = (sum[c] + alpha / 2).checked_div(alpha).unwrap_or((plain[c] + 2) / 4) as u8;
                }
                o[3] = ((alpha + 2) / 4) as u8;
            }
        }
        levels.push(out);
        (cw, ch) = (nw, nh);
    }
    levels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_list() {
        let text = "# a comment\n\npngs/bg.png = frontend/PreFlow/Language/background.fsh  # trailing\n\
                    pngs/arrow.png = Frontend\\Common\\Controls\\arrow_up.fsh frontend/common/acontrol/arrow_up.fsh\n";
        let r = Remastered::parse(text, Path::new("remastered")).unwrap();
        assert_eq!(r.len(), 3);
        let bg = Path::new("remastered").join("pngs/bg.png");
        assert_eq!(r.png("FRONTEND/preflow/language/BACKGROUND.fsh"), Some(bg.as_path()));
        assert_eq!(r.png("frontend/common/controls/arrow_up.fsh"), r.png("frontend/Common/AControl/arrow_up.fsh"));
        assert_eq!(r.png("frontend/common/controls/arrow_down.fsh"), None);
        assert!(Remastered::parse("a.png\n", Path::new(".")).unwrap_err().contains("line 1"));
        assert!(Remastered::parse("a.png = \n", Path::new(".")).unwrap_err().contains("no textures"));
        assert!(Remastered::parse("a.png = x.fsh\nb.png = X.FSH\n", Path::new(".")).unwrap_err().contains("line 2"));
        assert!(Remastered::parse("include other.txt\n", Path::new(".")).unwrap_err().contains("include"));
    }

    #[test]
    fn a_later_list_wins() {
        let mut low = Remastered::parse("a.png = x.fsh y.fsh\n", Path::new("low")).unwrap();
        let high = Remastered::parse("b.png = Y.FSH z.fsh\n", Path::new("high")).unwrap();
        low.layer(high);
        assert_eq!(low.png("x.fsh"), Some(Path::new("low/a.png")));
        assert_eq!(low.png("y.fsh"), Some(Path::new("high/b.png")));
        assert_eq!(low.png("z.fsh"), Some(Path::new("high/b.png")));
        assert_eq!(low.len(), 3);
    }

    #[test]
    fn reads_included_lists() {
        let dir = std::env::temp_dir().join(format!("qwc-remaster-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("top.txt"), "a.png = a.fsh\ninclude sub/more.txt\n").unwrap();
        std::fs::write(dir.join("sub/more.txt"), "b.png = b.fsh\n").unwrap();
        let r = Remastered::read(&dir.join("top.txt")).unwrap();
        assert_eq!(r.png("b.fsh"), Some(dir.join("sub").join("b.png").as_path()));
        assert_eq!(r.len(), 2);
        // a list including itself fails, on its textures listed again
        std::fs::write(dir.join("sub/more.txt"), "include ../top.txt\n").unwrap();
        assert!(Remastered::read(&dir.join("top.txt")).unwrap_err().contains("already"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn mips_weight_colour_by_alpha() {
        // an opaque red pixel beside three transparent black ones: the box stays red, a quarter opaque
        let base = [[255, 0, 0, 255], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]].concat();
        let levels = mip_chain(2, 2, base);
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[1], [255, 0, 0, 64]);
        // odd sides: 3x1 halves to 1x1, then stops
        let levels = mip_chain(3, 1, vec![10; 12]);
        assert_eq!(levels.iter().map(Vec::len).collect::<Vec<_>>(), [12, 4]);
        // all transparent: the plain mean
        assert_eq!(mip_chain(2, 1, [[100, 0, 0, 0], [200, 0, 0, 0]].concat())[1], [150, 0, 0, 0]);
    }
}
