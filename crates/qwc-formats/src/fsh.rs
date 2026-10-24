//! EA "SHPI" textures (`.fsh`; port of `tools/fsh_to_png.py`, loader `cSHAPE::Load` at `0x503980`).
//!
//! ```text
//! file  : "SHPI" | u32 file_size | u32 count | char[4] dir_id | count x { char[4] tag, u32 offset }
//! block : u8 type | u24 next (relative, 0 = last) | u16 w, h, cx, cy, x, y   (16 bytes, data follows)
//!         mip count = y >> 12; level l is (w>>l)*(h>>l)*bpp/8 bytes, padded to 16
//! ```
//! Image types: `0x79` 4-bit indexed (low nibble = first pixel), `0x7B` 8-bit indexed, `0x7D` 32-bit B,G,R,A.
//! Palettes: `0x24` 24-bit R,G,B (opaque), `0x2A` 32-bit B,G,R,A; a palette header's `w` is its colour count.

use crate::util::{bad, latin1, slice, u16_at, u32_at, Result};

pub fn bits_per_pixel(t: u8) -> Option<usize> {
    match t {
        0x79 => Some(4),
        0x7B => Some(8),
        0x7D => Some(32),
        _ => None,
    }
}

pub(crate) fn is_palette(t: u8) -> bool {
    t == 0x24 || t == 0x2A
}

fn align16(n: usize) -> usize {
    (n + 15) & !15
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockHeader {
    pub offset: usize,
    pub kind: u8,
    pub w: u16,
    pub h: u16,
    pub cx: u16,
    pub cy: u16,
    pub x: u16,
    pub y: u16,
}

#[derive(Debug, Clone)]
pub struct Image {
    pub tag: String,
    /// The image block followed by its attached blocks (palette, name, ...).
    pub blocks: Vec<BlockHeader>,
    /// Padded byte size of each mip level.
    pub level_sizes: Vec<usize>,
    /// RGBA palette (the last palette block attached), if any.
    pub palette: Option<Vec<[u8; 4]>>,
    /// False when the mip chain does not exactly fill the space before the first attached block.
    pub levels_fill_block: bool,
}

impl Image {
    pub fn kind(&self) -> u8 {
        self.blocks[0].kind
    }
    pub fn width(&self) -> usize {
        self.blocks[0].w as usize
    }
    pub fn height(&self) -> usize {
        self.blocks[0].h as usize
    }
    pub fn mip_count(&self) -> usize {
        (self.blocks[0].y >> 12) as usize
    }
}

#[derive(Debug, Clone)]
pub struct Fsh<'a> {
    pub data: &'a [u8],
    pub magic: String,
    pub dir_id: String,
    pub images: Vec<Image>,
}

pub(crate) fn read_blocks(d: &[u8], mut off: usize) -> Result<Vec<BlockHeader>> {
    let mut out = Vec::new();
    loop {
        let word = u32_at(d, off)?;
        let f = |i: usize| u16_at(d, off + 4 + 2 * i);
        out.push(BlockHeader {
            offset: off,
            kind: (word & 0xFF) as u8,
            w: f(0)?,
            h: f(1)?,
            cx: f(2)?,
            cy: f(3)?,
            x: f(4)?,
            y: f(5)?,
        });
        let next = (word >> 8) as usize;
        if next == 0 || out.len() > 4096 {
            return Ok(out);
        }
        off += next;
    }
}

pub(crate) fn decode_palette(d: &[u8], b: &BlockHeader) -> Result<Vec<[u8; 4]>> {
    let n = b.w as usize;
    let base = b.offset + 16;
    if b.kind == 0x24 {
        let p = slice(d, base, n * 3)?;
        Ok(p.as_chunks::<3>().0.iter().map(|c| [c[0], c[1], c[2], 255]).collect())
    } else {
        let p = slice(d, base, n * 4)?;
        Ok(p.as_chunks::<4>().0.iter().map(|c| [c[2], c[1], c[0], c[3]]).collect())
    }
}

/// Decode `w x h` pixels of image type `kind` at `pos` to RGBA8 (indices past the palette become magenta).
pub(crate) fn decode_pixels(d: &[u8], pos: usize, kind: u8, w: usize, h: usize, pal: &[[u8; 4]]) -> Result<Vec<u8>> {
    let n = w * h;
    let mut out = Vec::with_capacity(n * 4);
    match kind {
        0x7D => {
            for c in slice(d, pos, n * 4)?.as_chunks::<4>().0 {
                out.extend_from_slice(&[c[2], c[1], c[0], c[3]]);
            }
        }
        kind => {
            let raw = if kind == 0x7B { slice(d, pos, n)? } else { slice(d, pos, n.div_ceil(2))? };
            for i in 0..n {
                let idx = if kind == 0x7B {
                    raw[i]
                } else if i & 1 == 0 {
                    raw[i >> 1] & 0x0F
                } else {
                    raw[i >> 1] >> 4
                } as usize;
                out.extend_from_slice(pal.get(idx).unwrap_or(&[255, 0, 255, 255]));
            }
        }
    }
    Ok(out)
}

impl<'a> Fsh<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Fsh<'a>> {
        let magic = slice(data, 0, 4)?;
        if !matches!(magic, b"SHPI" | b"SHPX" | b"SHPS") {
            return bad(format!("unsupported magic {magic:?}"));
        }
        let count = u32_at(data, 8)? as usize;
        let mut images = Vec::with_capacity(count);
        for i in 0..count {
            let tag = latin1(slice(data, 16 + i * 8, 4)?).trim_end_matches(['\0', ' ']).to_string();
            let blocks = read_blocks(data, u32_at(data, 20 + i * 8)? as usize)?;
            let img = &blocks[0];
            let Some(bpp) = bits_per_pixel(img.kind) else {
                return bad(format!("{tag}: unsupported image type 0x{:02x}", img.kind));
            };
            let (w, h) = (img.w as usize, img.h as usize);
            let level_sizes: Vec<usize> =
                (0..=(img.y >> 12) as usize).map(|l| align16((w >> l) * (h >> l) * bpp / 8)).collect();
            let levels_fill_block =
                blocks.get(1).is_none_or(|b| b.offset.checked_sub(img.offset + 16) == Some(level_sizes.iter().sum()));
            let mut palette = None;
            for b in &blocks[1..] {
                if is_palette(b.kind) {
                    palette = Some(decode_palette(data, b)?);
                }
            }
            if bpp <= 8 && palette.is_none() {
                return bad(format!("{tag}: indexed image without palette"));
            }
            images.push(Image { tag, blocks, level_sizes, palette, levels_fill_block });
        }
        Ok(Fsh { data, magic: latin1(magic), dir_id: latin1(slice(data, 12, 4)?), images })
    }

    /// Decode mip level `level` of image `index` to RGBA8 (indices past the palette become magenta).
    pub fn decode_rgba(&self, index: usize, level: usize) -> Result<(usize, usize, Vec<u8>)> {
        let img = &self.images[index];
        let pos = img.blocks[0].offset + 16 + img.level_sizes[..level].iter().sum::<usize>();
        let (w, h) = ((img.width() >> level).max(1), (img.height() >> level).max(1));
        Ok((w, h, decode_pixels(self.data, pos, img.kind(), w, h, img.palette.as_deref().unwrap_or(&[]))?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(kind: u8, next: u32, w: u16, h: u16, y: u16) -> Vec<u8> {
        let mut b = (kind as u32 | next << 8).to_le_bytes().to_vec();
        for v in [w, h, 0, 0, 0, y] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b
    }

    #[test]
    fn four_bit_image_with_rgb_palette() {
        // 4x2 4-bit image: 4 bytes of pixels padded to 16; then a 2-colour 0x24 palette
        let mut d = b"SHPI".to_vec();
        d.extend_from_slice(&[0; 4]);
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(b"G357img \x18\0\0\0");
        d.extend(block(0x79, 32, 4, 2, 0));
        d.extend_from_slice(&[0x10, 0x01, 0x00, 0x11]);
        d.resize(24 + 32, 0);
        d.extend(block(0x24, 0, 2, 1, 0));
        d.extend_from_slice(&[1, 2, 3, 4, 5, 6]);
        let f = Fsh::parse(&d).unwrap();
        let img = &f.images[0];
        assert_eq!(img.tag, "img");
        assert!(img.levels_fill_block);
        let (w, h, px) = f.decode_rgba(0, 0).unwrap();
        assert_eq!((w, h), (4, 2));
        let a = [1, 2, 3, 255];
        let b = [4, 5, 6, 255];
        let want: Vec<u8> = [a, b, b, a, a, a, b, b].concat();
        assert_eq!(px, want);
    }

    #[test]
    fn indexed_without_palette_fails() {
        let mut d = b"SHPI\0\0\0\0\x01\0\0\0G357img \x18\0\0\0".to_vec();
        d.extend(block(0x7B, 0, 1, 1, 0));
        d.extend_from_slice(&[0; 16]);
        assert!(Fsh::parse(&d).is_err());
    }
}
