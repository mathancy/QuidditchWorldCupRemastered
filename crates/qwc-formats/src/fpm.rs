//! "FPM1" models (`.fpm`; port of `tools/fpm_parse.py` plus the vertex rules of `tools/fpm_to_gltf.py`).
//!
//! Loader: FPM resource handler (vtable `0x5b6404`); fixups `0x49aae0` -> `0x49a9b0` -> `0x49a950` -> `0x49a880`
//! -> geometry setup `0x50b960`. All offsets are file-relative unless noted.
//! ```text
//! header : "FPM1" | u32 file_size | u32 root_off | u32 geom_region_off
//! node   : f32[16] matrix | f32[4] min | f32[4] max | u32 nA | u32 A_off | u32 nB | u32 B_off
//! B (12) : u32 name_off | u32 hash | u32 flags                      texture names
//! A (48) : f32[4] min | f32[4] max | u32 nC | u32 C_off | 0 | 0
//! C (48) : f32[4] min | f32[4] max | u32 nD | u32 ? | u32 D_off | 0
//! D (48) : f32[4] min | f32[4] max | u32 geom_off | u32 opt1_off (morphs) | u32 opt2_off (bone palette) | u32 tex
//! geom   : u32 vtx_off | u32 idx_off (relative to geom_region_off) | u32 fvf | u16 h0 | u16 prim_count | u16 h1
//!          | u16 idx_count | u8 stride | u8 prim_type (5 = strip) | u16 vtx_count | 24 more bytes
//! vertex block: 16-byte prefix, then vtx_count * stride bytes; indices are u16
//! ```
//! Morph targets (`opt1`, decoder `FUN_0050ce10`): see [`Fpm::morphs`].
//!
//! A group's children (C) are its levels of detail: the renderer (`0x4a4d40`) draws one child of each group, the
//! level `0x492b00` picks from the distance (clamped to the scene's least level and to the group's last child).
//! Child 0 is the finest. Each level is a whole model (`gry_beater1.fpm`: 2364 vertices at level 0, 1033 at 3).

use std::collections::BTreeMap;

use crate::util::{bad, cstr_at, f32s, slice, u16_at, u32_at, u8_at, Result};

/// Vertex format of skinned draws: pos, normal, uv, u8 palette index, 3 filler bytes (stride 36).
pub const SKINNED_FVF: u32 = 0x1000_0112;

/// `(stride, normal_off, color_off, uv_off)` for each vertex format found on the disc.
pub fn layout(fvf: u32) -> Option<(usize, Option<usize>, Option<usize>, usize)> {
    match fvf {
        0x102 => Some((20, None, None, 12)),
        0x142 => Some((24, None, Some(12), 16)),
        0x112 => Some((32, Some(12), None, 24)),
        SKINNED_FVF => Some((36, Some(12), None, 24)),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct Geom {
    pub off: usize,
    pub vtx_off: u32,
    pub idx_off: u32,
    pub fvf: u32,
    pub h0: u16,
    pub prim: u16,
    pub h1: u16,
    pub idx_count: u16,
    pub stride: u8,
    pub ptype: u8,
    pub vcount: u16,
    pub tail: [u8; 24],
}

#[derive(Debug, Clone)]
pub struct Draw {
    pub entry: usize,
    pub bbox: [f32; 8],
    pub geom: Geom,
    pub opt1: u32,
    pub opt2: u32,
    /// Low 16 bits: the texture name; high 16: flags ([`Draw::two_sided`]).
    pub tex: u32,
}

impl Draw {
    /// Drawn without culling: the model draw passes bit `0x20` of `+0x2e` on (`0x4a4d40`) and the draw's states
    /// set `D3DCULL_NONE` for it, `D3DCULL_CCW` otherwise (`0x50f1f8`, `0x50f798`). 4737 of the 6551 draws.
    pub fn two_sided(&self) -> bool {
        self.tex >> 16 & 0x20 != 0
    }
}

#[derive(Debug, Clone)]
pub struct Child {
    pub entry: usize,
    pub unk: u32,
    pub draws: Vec<Draw>,
}

#[derive(Debug, Clone)]
pub struct Group {
    pub entry: usize,
    pub children: Vec<Child>,
}

#[derive(Debug, Clone)]
pub struct TextureName {
    pub name: String,
    /// Read as an `f32` by the renderer (`0x4a57e3`: its logarithm goes into the material's key); not a hash.
    pub hash: u32,
    /// How the texture is drawn: see [`TextureName::surface`].
    pub flags: u32,
}

/// The Groper blend modes a model's names use (byte 9 of the flags, `0x502f10`; numbered as the keyword table at
/// `0x5fc4d8`). Each picks a blend record built at `0x508340` (`0xf9d188 + mode * 0x3c`).
pub mod blend {
    /// `ONE, ZERO`, unblended.
    pub const COPYRGB: u8 = 1;
    /// `SRCALPHA, INVSRCALPHA`.
    pub const SRCALPHA: u8 = 2;
    /// `SRCALPHA, INVSRCALPHA`, the destination's alpha kept (colour writes `0x5a5578`).
    pub const SRCALPHA_KEEP: u8 = 3;
    /// `ONE, ZERO`, unblended, the destination's alpha kept.
    pub const COPYRGB_KEEP: u8 = 14;
    /// `DESTCOLOR, SRCCOLOR`: the texture times twice what is drawn (a lightmap over its base).
    pub const SRCALPHA_LIGHTMAP_KEEP: u8 = 20;
}

/// A texture name's flags word, as `ModelRender.cpp` sets up its material (`0x4a57a0`-`0x4a59e3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Surface {
    /// Byte 8: the draw's layer, added to the instance's (`0x4a57b4`); the engine draws lower layers first. On the
    /// disc: 0 the unblended names, 1-2 the lightmaps, 1-3 the blended names.
    pub layer: u8,
    /// Byte 9: the blend mode ([`blend`]). Every mode but [`blend::COPYRGB`] and [`blend::COPYRGB_KEEP`] blends
    /// (`0x502f10` sets the material's `0x40`, which `0x507dc0` makes `ALPHABLENDENABLE`).
    pub blend: u8,
    /// Byte 10 bits 0 and 1: the texture wraps in U and in V; clamped otherwise (`0x5032a0`; the address modes of
    /// `0x5eab9c`).
    pub wrap: [bool; 2],
    /// Byte 10 bit 2: alpha tested, with the default material's test (`0x507670`: alpha at least 192).
    pub alpha_test: bool,
    /// Byte 10 bit 3 clears it (`0x4a59af`): the draw writes depth.
    pub depth_write: bool,
}

/// The default material's alpha test reference (`0x507670`), compared `D3DCMP_GREATEREQUAL` (`0xf9d008`).
pub const ALPHA_REF: u8 = 0xc0;

impl Surface {
    /// Whether the draw blends with what is under it.
    pub fn blends(&self) -> bool {
        !matches!(self.blend, blend::COPYRGB | blend::COPYRGB_KEEP)
    }
}

impl TextureName {
    pub fn surface(&self) -> Surface {
        let [layer, blend, options, _] = self.flags.to_le_bytes();
        Surface {
            layer,
            blend,
            wrap: [options & 1 != 0, options & 2 != 0],
            alpha_test: options & 4 != 0,
            depth_write: options & 8 == 0,
        }
    }
}

/// Morph targets of one draw: slot `k` is driven by the character's weight array at `weight_base + k`.
#[derive(Debug, Clone, PartialEq)]
pub struct Morphs {
    pub weight_base: u16,
    /// `None` for an empty slot; otherwise vertex index -> summed position delta (bone-local).
    pub slots: Vec<Option<BTreeMap<usize, [f64; 3]>>>,
}

#[derive(Debug, Clone)]
pub struct Fpm<'a> {
    pub d: &'a [u8],
    pub root_off: u32,
    pub geom_base: u32,
    pub matrix: [f32; 16],
    pub names: Vec<TextureName>,
    pub groups: Vec<Group>,
}

impl<'a> Fpm<'a> {
    pub fn parse(d: &'a [u8]) -> Result<Fpm<'a>> {
        if slice(d, 0, 4)? != b"FPM1" {
            return bad("not FPM1");
        }
        let size = u32_at(d, 4)? as usize;
        if size != d.len() {
            return bad(format!("size field {size} != {}", d.len()));
        }
        let root_off = u32_at(d, 8)?;
        let geom_base = u32_at(d, 12)?;
        let r = root_off as usize;
        let matrix = f32s::<16>(d, r)?;
        let (n_a, off_a, n_b, off_b) = (
            u32_at(d, r + 0x60)? as usize,
            u32_at(d, r + 0x64)? as usize,
            u32_at(d, r + 0x68)? as usize,
            u32_at(d, r + 0x6C)? as usize,
        );
        let mut names = Vec::with_capacity(n_b);
        for i in 0..n_b {
            let e = off_b + i * 12;
            names.push(TextureName {
                name: cstr_at(d, u32_at(d, e)? as usize)?,
                hash: u32_at(d, e + 4)?,
                flags: u32_at(d, e + 8)?,
            });
        }
        let mut groups = Vec::with_capacity(n_a);
        for a in 0..n_a {
            let ea = off_a + a * 0x30;
            let (n_c, off_c) = (u32_at(d, ea + 0x20)? as usize, u32_at(d, ea + 0x24)? as usize);
            let mut children = Vec::with_capacity(n_c);
            for c in 0..n_c {
                let ec = off_c + c * 0x30;
                let (n_d, unk, off_d) =
                    (u32_at(d, ec + 0x20)? as usize, u32_at(d, ec + 0x24)?, u32_at(d, ec + 0x28)? as usize);
                let mut draws = Vec::with_capacity(n_d);
                for k in 0..n_d {
                    let ed = off_d + k * 0x30;
                    draws.push(Draw {
                        entry: ed,
                        bbox: f32s::<8>(d, ed)?,
                        geom: Self::geom(d, u32_at(d, ed + 0x20)? as usize)?,
                        opt1: u32_at(d, ed + 0x24)?,
                        opt2: u32_at(d, ed + 0x28)?,
                        tex: u32_at(d, ed + 0x2C)?,
                    });
                }
                children.push(Child { entry: ec, unk, draws });
            }
            groups.push(Group { entry: ea, children });
        }
        Ok(Fpm { d, root_off, geom_base, matrix, names, groups })
    }

    fn geom(d: &[u8], off: usize) -> Result<Geom> {
        Ok(Geom {
            off,
            vtx_off: u32_at(d, off)?,
            idx_off: u32_at(d, off + 4)?,
            fvf: u32_at(d, off + 8)?,
            h0: u16_at(d, off + 0x0C)?,
            prim: u16_at(d, off + 0x0E)?,
            h1: u16_at(d, off + 0x10)?,
            idx_count: u16_at(d, off + 0x12)?,
            stride: u8_at(d, off + 0x14)?,
            ptype: u8_at(d, off + 0x15)?,
            vcount: u16_at(d, off + 0x16)?,
            tail: slice(d, off + 0x18, 24)?.try_into().unwrap(),
        })
    }

    /// All draws in file order (groups, then children, then draws).
    pub fn draws(&self) -> impl Iterator<Item = &Draw> {
        self.groups.iter().flat_map(|g| g.children.iter().flat_map(|c| c.draws.iter()))
    }

    /// All draws in file order with the level of detail each is in (its child's index in its group).
    pub fn draws_with_level(&self) -> impl Iterator<Item = (usize, &Draw)> {
        self.groups
            .iter()
            .flat_map(|g| g.children.iter().enumerate().flat_map(|(l, c)| c.draws.iter().map(move |d| (l, d))))
    }

    /// The draw's vertex bytes (after the 16-byte prefix), `vcount * stride` long.
    pub fn vertex_bytes(&self, dr: &Draw) -> Result<&'a [u8]> {
        let g = &dr.geom;
        slice(self.d, (self.geom_base + g.vtx_off) as usize + 16, g.vcount as usize * g.stride as usize)
    }

    /// The draw's u16 strip indices.
    pub fn indices(&self, dr: &Draw) -> Result<Vec<u16>> {
        let raw = slice(self.d, (self.geom_base + dr.geom.idx_off) as usize, dr.geom.idx_count as usize * 2)?;
        Ok(raw.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect())
    }

    /// Bone palette (`opt2`: `u8 count, u8 bones[count]`), present on skinned draws.
    pub fn palette(&self, dr: &Draw) -> Result<Option<Vec<u8>>> {
        if dr.opt2 == 0 {
            return Ok(None);
        }
        let n = u8_at(self.d, dr.opt2 as usize)? as usize;
        Ok(Some(slice(self.d, dr.opt2 as usize + 1, n)?.to_vec()))
    }

    /// Decode a draw's `opt1` morph targets.
    ///
    /// `opt1 -> u16 weight_base | u16 slot_count | u32 slots_off`, each slot `{u32 data_off (0 = empty), u32 size}`.
    /// Slot data is runs of `u32 h | (h >> 26) x f32[3]`: `h & 0xfffff` is a byte offset into the draw's vertex
    /// copy with a 0x30 bias, `(h >> 20) & 0x3f` the byte step between deltas (= stride). Deltas add up.
    pub fn morphs(&self, dr: &Draw) -> Result<Option<Morphs>> {
        if dr.opt1 == 0 {
            return Ok(None);
        }
        let d = self.d;
        let g = &dr.geom;
        let stride = g.stride as usize;
        let o = dr.opt1 as usize;
        let (weight_base, count, arr) = (u16_at(d, o)?, u16_at(d, o + 2)? as usize, u32_at(d, o + 4)? as usize);
        let mut slots = Vec::with_capacity(count);
        for k in 0..count {
            let (off, size) = (u32_at(d, arr + 8 * k)? as usize, u32_at(d, arr + 8 * k + 4)? as usize);
            if off == 0 {
                slots.push(None);
                continue;
            }
            let mut deltas = BTreeMap::new();
            let (mut p, end) = (off, off + size);
            while p < end {
                let h = u32_at(d, p)?;
                p += 4;
                let (n, step) = ((h >> 26) as usize, ((h >> 20) & 0x3F) as usize);
                let dst = (h & 0xFFFFF) as i64 - 0x30;
                if stride == 0 || dst < 0 || !(dst as usize).is_multiple_of(stride) || (n > 1 && step != stride) {
                    return bad(format!("morph run 0x{h:08x} not aligned to stride {stride}"));
                }
                let v = dst as usize / stride + g.h1 as usize;
                for j in 0..n {
                    if v + j >= g.vcount as usize {
                        return bad(format!("morph vertex {} >= {}", v + j, g.vcount));
                    }
                    let dv = f32s::<3>(d, p + 12 * j)?;
                    let e = deltas.entry(v + j).or_insert([0f64; 3]);
                    for c in 0..3 {
                        e[c] += dv[c] as f64;
                    }
                }
                p += 12 * n;
            }
            if p != end {
                return bad(format!("morph slot {k} overruns by {} bytes", p - end));
            }
            slots.push(Some(deltas));
        }
        Ok(Some(Morphs { weight_base, slots }))
    }
}

/// Convert a D3D triangle strip to triangles: even-index triangles `(a, b, c)` face front (CCW), odd ones are
/// reversed to (a, c, b); degenerate triangles are dropped (as `fpm_to_gltf.strip_to_tris`).
pub fn strip_to_tris(idx: &[u16]) -> Vec<[u16; 3]> {
    let mut out = Vec::new();
    for i in 0..idx.len().saturating_sub(2) {
        let (a, b, c) = (idx[i], idx[i + 1], idx[i + 2]);
        if a == b || b == c || a == c {
            continue;
        }
        out.push(if i % 2 == 0 { [a, b, c] } else { [a, c, b] });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips() {
        assert_eq!(strip_to_tris(&[0, 1, 2, 3, 3, 4]), vec![[0, 1, 2], [1, 3, 2]]);
    }

    fn le(v: &[u32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    /// Minimal model: one group / child / draw, 2 skinned vertices, one morph slot.
    fn model() -> Vec<u8> {
        let mut d = vec![0u8; 0x200];
        d[..4].copy_from_slice(b"FPM1");
        d[8..16].copy_from_slice(&le(&[0x20, 0x180]));
        // root node at 0x20: identity matrix, then counts at +0x60
        d[0x80..0x90].copy_from_slice(&le(&[1, 0xA0, 1, 0x190]));
        d[0xA0 + 0x20..0xA0 + 0x28].copy_from_slice(&le(&[1, 0xD0])); // A
        d[0xD0 + 0x20..0xD0 + 0x2C].copy_from_slice(&le(&[1, 7, 0x100])); // C
        d[0x100 + 0x20..0x100 + 0x30].copy_from_slice(&le(&[0x130, 0x150, 0x170, 0x20])); // D
                                                                                          // geom at 0x130: vtx_off 0, idx_off 0x40 (relative to 0x180), fvf, h0, prim, h1, idx_count, stride, ptype, vcount
        d[0x130..0x13C].copy_from_slice(&le(&[0, 0x40, SKINNED_FVF]));
        d[0x13C..0x144].copy_from_slice(&[0, 0, 1, 0, 0, 0, 3, 0]);
        d[0x144..0x148].copy_from_slice(&[36, 5, 2, 0]);
        // opt1 at 0x150: base 4, 1 slot at 0x158 -> data 0x1C8 (8 bytes header + 12)
        d[0x150..0x160].copy_from_slice(&le(&[4 | 1 << 16, 0x158, 0x1C8, 16]));
        d[0x170..0x173].copy_from_slice(&[2, 9, 11]); // palette
        d[0x190..0x19C].copy_from_slice(&le(&[0x1A0, 0xABCD, 0x20]));
        d[0x1A0..0x1A4].copy_from_slice(b"tex\0");
        let h: u32 = (1 << 26) | (0x30 + 36);
        d[0x1C8..0x1D8].copy_from_slice(&le(&[h, 1.5f32.to_bits(), 0, (-1f32).to_bits()]));
        d.truncate(0x1D8);
        let n = d.len() as u32;
        d[4..8].copy_from_slice(&n.to_le_bytes());
        d
    }

    #[test]
    fn parse_minimal_model() {
        let d = model();
        let m = Fpm::parse(&d).unwrap();
        assert_eq!(m.names[0].name, "tex");
        let dr = m.draws().next().unwrap();
        assert_eq!(dr.geom.vcount, 2);
        assert_eq!(dr.geom.stride, 36);
        assert_eq!(m.palette(dr).unwrap(), Some(vec![9, 11]));
        let mo = m.morphs(dr).unwrap().unwrap();
        assert_eq!(mo.weight_base, 4);
        let slot = mo.slots[0].as_ref().unwrap();
        assert_eq!(slot.get(&1), Some(&[1.5, 0.0, -1.0]));
        assert_eq!(m.vertex_bytes(dr).unwrap().len(), 72);
    }

    #[test]
    fn surfaces() {
        let s = |flags| TextureName { name: String::new(), hash: 0, flags }.surface();
        // an unblended name that wraps both ways; a lightmap, clamped; a blended, alpha-tested one
        let base = s(0x0003_0e00);
        assert_eq!(
            base,
            Surface { layer: 0, blend: blend::COPYRGB_KEEP, wrap: [true, true], alpha_test: false, depth_write: true }
        );
        assert!(!base.blends());
        let lm = s(0x0000_1401);
        assert_eq!(
            (lm.layer, lm.blend, lm.wrap, lm.alpha_test),
            (1, blend::SRCALPHA_LIGHTMAP_KEEP, [false, false], false)
        );
        assert!(lm.blends());
        let tree = s(0x0004_0303);
        assert_eq!(
            (tree.layer, tree.blend, tree.wrap, tree.alpha_test),
            (3, blend::SRCALPHA_KEEP, [false, false], true)
        );
        assert!(!s(0x0008_0e00).depth_write);
    }

    #[test]
    fn morph_vertex_out_of_range() {
        let mut d = model();
        let h: u32 = (1 << 26) | (0x30 + 72);
        d[0x1C8..0x1CC].copy_from_slice(&h.to_le_bytes());
        let m = Fpm::parse(&d).unwrap();
        assert!(m.morphs(m.draws().next().unwrap()).is_err());
    }
}
