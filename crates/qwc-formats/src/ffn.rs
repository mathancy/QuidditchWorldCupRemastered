//! Bitmap fonts (`.ffn`, magic "FntF"; port of `tools/ffn_parse.py`, loader `FONT::Load` at `FUN_004ff6b0`).
//!
//! ```text
//! 0x00 "FntF" | u32 file_size | u16 ? (0x198) | u16 n_glyphs | u32 flags (0x40000 set: 16-byte glyphs, else 12)
//! 0x12 u8 a | u8 b              line height a + b
//! 0x18 u32 kerning_off (0 = none) | u32 texture_off | ...   0x30 u32 descriptor_off (32 bytes, = texture_off - 0x20)
//! 0x80 glyphs, ascending code:
//!      16-byte: u16 code | u8 w | u8 h | u16 u | u16 v | u8 0 | i8 x_offset | u8 y_offset | u8 n_kern | u16 kern_first | u16 advance
//!      12-byte: u16 code | u8 w | u8 h | u16 u | u16 v | u8 advance | i8 x_offset | u8 y_offset | u8 n_kern
//! kerning: u32 count | count x { u16 prev, i8 adjust, u8 next }   grouped by next, in glyph order
//! texture_off: an FSH block chain (0x79 4-bit image + 0x2A palette, or 0x2D = 16-bit A1R5G5B5 palette, [+ 0x69])
//! ```
//! Bytes between the tables and the descriptor are padding.

use crate::fsh::{bits_per_pixel, decode_palette, decode_pixels, is_palette, read_blocks, BlockHeader};
use crate::util::{bad, slice, u16_at, u32_at, u8_at, Result};

pub const GLYPH_TABLE: usize = 0x80;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyph {
    pub code: u16,
    pub w: u8,
    pub h: u8,
    pub u: u16,
    pub v: u16,
    pub x_offset: i8,
    pub y_offset: u8,
    pub advance: u16,
    pub n_kern: u8,
    /// Start of this glyph's run in the kerning table (implicit, consecutive, for 12-byte glyphs).
    pub kern_first: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kern {
    pub prev: u16,
    pub adjust: i8,
    pub next: u8,
}

#[derive(Debug, Clone)]
pub struct Ffn<'a> {
    pub data: &'a [u8],
    /// The u16 at 0x08 (0x198 in every file).
    pub unk_08: u16,
    pub flags: u32,
    pub line_a: u8,
    pub line_b: u8,
    pub kern_off: u32,
    pub tex_off: u32,
    pub desc_off: u32,
    pub glyphs: Vec<Glyph>,
    pub kerning: Vec<Kern>,
    pub padding: &'a [u8],
    pub descriptor: &'a [u8],
    /// The texture's FSH block chain; `blocks[0]` is the image.
    pub blocks: Vec<BlockHeader>,
    pub tex_w: u16,
    pub tex_h: u16,
}

impl<'a> Ffn<'a> {
    pub fn parse(d: &'a [u8]) -> Result<Ffn<'a>> {
        if slice(d, 0, 4)? != b"FntF" {
            return bad("not FntF");
        }
        let (size, unk_08, n, flags) = (u32_at(d, 4)?, u16_at(d, 8)?, u16_at(d, 10)? as usize, u32_at(d, 12)?);
        if size as usize != d.len() {
            return bad(format!("size field {size} != {}", d.len()));
        }
        let (line_a, line_b) = (u8_at(d, 0x12)?, u8_at(d, 0x13)?);
        let (kern_off, tex_off, desc_off) = (u32_at(d, 0x18)?, u32_at(d, 0x1C)?, u32_at(d, 0x30)?);
        let gs = if flags & 0x40000 != 0 { 16 } else { 12 };
        let mut glyphs = Vec::with_capacity(n);
        for i in 0..n {
            let o = GLYPH_TABLE + i * gs;
            let code = u16_at(d, o)?;
            let b = |k: usize| u8_at(d, o + k);
            let (w, h, u, v, x_offset, y_offset, n_kern) =
                (b(2)?, b(3)?, u16_at(d, o + 4)?, u16_at(d, o + 6)?, b(9)? as i8, b(10)?, b(11)?);
            let (advance, kern_first) = if gs == 16 {
                if b(8)? != 0 {
                    return bad(format!("glyph {code:#x}: byte 8 is {}, expected 0", b(8)?));
                }
                (u16_at(d, o + 14)?, u16_at(d, o + 12)?)
            } else {
                (b(8)? as u16, u16::MAX) // kern_first filled in below
            };
            glyphs.push(Glyph { code, w, h, u, v, x_offset, y_offset, advance, n_kern, kern_first });
        }
        let mut end = GLYPH_TABLE + n * gs;
        if glyphs.windows(2).any(|p| p[0].code >= p[1].code) {
            return bad("glyph codes not ascending");
        }
        let mut kerning = Vec::new();
        if kern_off != 0 {
            let ko = kern_off as usize;
            if ko != end {
                return bad(format!("kerning at {ko:#x}, glyphs end at {end:#x}"));
            }
            let cnt = u32_at(d, ko)? as usize;
            for i in 0..cnt {
                let e = ko + 4 + 4 * i;
                kerning.push(Kern { prev: u16_at(d, e)?, adjust: u8_at(d, e + 2)? as i8, next: u8_at(d, e + 3)? });
            }
            end = ko + 4 + 4 * cnt;
        }
        let mut first = 0usize;
        for g in &mut glyphs {
            if gs == 12 {
                g.kern_first = first as u16;
            } else if g.n_kern != 0 && g.kern_first as usize != first {
                return bad(format!("glyph {:#x}: kerning run at {}, expected {first}", g.code, g.kern_first));
            }
            if kerning.iter().skip(first).take(g.n_kern as usize).any(|k| k.next as u16 != g.code) {
                return bad(format!("glyph {:#x}: kerning run names another glyph", g.code));
            }
            first += g.n_kern as usize;
        }
        if first != kerning.len() {
            return bad(format!("glyph kerning counts sum to {first}, table has {}", kerning.len()));
        }
        if !(end <= desc_off as usize && desc_off as i64 == tex_off as i64 - 0x20) {
            return bad(format!("tables end {end:#x}, descriptor {desc_off:#x}, texture {tex_off:#x}"));
        }
        let padding = slice(d, end, desc_off as usize - end)?;
        let descriptor = slice(d, desc_off as usize, 0x20)?;
        let blocks = read_blocks(d, tex_off as usize)?;
        let img = blocks[0];
        if bits_per_pixel(img.kind).is_none() {
            return bad(format!("texture type {:#x}", img.kind));
        }
        let (tw, th) = (img.w as u32, img.h as u32);
        let bad_rects = glyphs.iter().filter(|g| g.u as u32 + g.w as u32 > tw || g.v as u32 + g.h as u32 > th).count();
        if bad_rects > 0 {
            return bad(format!("{bad_rects} glyph rects outside the {tw}x{th} texture"));
        }
        Ok(Ffn {
            data: d,
            unk_08,
            flags,
            line_a,
            line_b,
            kern_off,
            tex_off,
            desc_off,
            glyphs,
            kerning,
            padding,
            descriptor,
            blocks,
            tex_w: img.w,
            tex_h: img.h,
        })
    }

    /// Pen adjustment between code `prev` and the following code `next`.
    pub fn kern(&self, prev: u16, next: u16) -> i8 {
        let Some(g) = self.glyphs.iter().find(|g| g.code == next) else { return 0 };
        let mut run = self.kerning.iter().skip(g.kern_first as usize).take(g.n_kern as usize);
        run.find(|k| k.prev == prev).map_or(0, |k| k.adjust)
    }

    /// Decode the texture to RGBA8 (palette types as in `fsh`, plus `0x2D` = 16-bit A1R5G5B5).
    pub fn atlas(&self) -> Result<Vec<u8>> {
        let img = self.blocks[0];
        let mut pal = None;
        for b in &self.blocks[1..] {
            if is_palette(b.kind) {
                pal = Some(decode_palette(self.data, b)?);
            } else if b.kind == 0x2D {
                let c5 = |c: u16, s: u16| (((c >> s) & 31) as u32 * 255 / 31) as u8;
                let colours = (0..b.w as usize).map(|i| u16_at(self.data, b.offset + 16 + 2 * i));
                pal = Some(
                    colours
                        .map(|c| c.map(|c| [c5(c, 10), c5(c, 5), c5(c, 0), if c & 0x8000 != 0 { 255 } else { 0 }]))
                        .collect::<Result<Vec<_>>>()?,
                );
            }
        }
        if img.kind != 0x7D && pal.is_none() {
            return bad("indexed texture without palette");
        }
        let (w, h) = (img.w as usize, img.h as usize);
        decode_pixels(self.data, img.offset + 16, img.kind, w, h, pal.as_deref().unwrap_or(&[]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 12-byte-glyph font with glyphs 'A' and 'V' (V kerned after A by -2) and a 2x2 4-bit texture + 0x2D palette.
    fn font() -> Vec<u8> {
        let mut d = b"FntF".to_vec();
        d.resize(GLYPH_TABLE, 0);
        d[8..10].copy_from_slice(&0x198u16.to_le_bytes());
        d[10..12].copy_from_slice(&2u16.to_le_bytes());
        d[0x12] = 7;
        d[0x13] = 2;
        for (code, u, adv, nk) in [(b'A', 0u16, 5u8, 0u8), (b'V', 1, 6, 1)] {
            d.extend_from_slice(&(code as u16).to_le_bytes());
            d.extend_from_slice(&[1, 2]);
            d.extend_from_slice(&u.to_le_bytes());
            d.extend_from_slice(&0u16.to_le_bytes());
            d.extend_from_slice(&[adv, 0xFF, 1, nk]);
        }
        let kern_off = d.len() as u32;
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&(b'A' as u16).to_le_bytes());
        d.extend_from_slice(&[0xFE, b'V']);
        let desc = d.len() as u32 + 4;
        d.resize(desc as usize + 0x20, 0);
        let tex = d.len() as u32;
        for (o, v) in [(0x18, kern_off), (0x1C, tex), (0x30, desc)] {
            d[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&(0x79u32 | 32 << 8).to_le_bytes());
        for v in [2u16, 2, 0, 0, 0, 0] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&[0x10, 0x01]);
        d.resize(tex as usize + 32, 0);
        d.extend_from_slice(&0x2Du32.to_le_bytes());
        for v in [2u16, 1, 0, 0, 0, 0, 0x7C00, 0x801F] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        let n = d.len() as u32;
        d[4..8].copy_from_slice(&n.to_le_bytes());
        d
    }

    #[test]
    fn small_font() {
        let d = font();
        let f = Ffn::parse(&d).unwrap();
        assert_eq!(f.line_a + f.line_b, 9);
        assert_eq!(f.glyphs[1].kern_first, 0);
        assert_eq!(f.glyphs[0].x_offset, -1);
        assert_eq!(f.padding.len(), 4);
        assert_eq!((f.kern(b'A' as u16, b'V' as u16), f.kern(b'V' as u16, b'A' as u16)), (-2, 0));
        let red = [255, 0, 0, 0];
        let blue = [0, 0, 255, 255];
        assert_eq!(f.atlas().unwrap(), [red, blue, blue, red].concat());
    }

    #[test]
    fn rejects_bad_fonts() {
        let mut d = font();
        d.push(0);
        assert!(Ffn::parse(&d).is_err()); // size field
        let mut d = font();
        d[GLYPH_TABLE + 12 + 11] = 0; // V no longer claims the kerning pair
        assert!(Ffn::parse(&d).is_err());
        let mut d = font();
        d[GLYPH_TABLE + 4] = 2; // A's rect leaves the 2x2 texture
        assert!(Ffn::parse(&d).is_err());
    }
}
