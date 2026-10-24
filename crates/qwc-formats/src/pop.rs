//! Stadium effect placers (`stadium/<name>/fx/*.pop`; port of `tools/pop_parse.py`, handler vtable `0x5b4914`):
//! named particle locators with their matrices.
//!
//! ```text
//! 0x00 u32 magic 0x0b0b50f1 | u32 0 | u32 count | u32 names_offset | u32 matrices_offset | 12 B zero
//! matrices_offset: count x 4x4 f32 (row-major, translation in row 3)
//! names_offset:    count x u32 name offset (from the file start) | NUL-terminated names
//! ```
//! The game flips Z (`S M S`, `S = diag(1, 1, -1, 1)`) and zeroes m23 when it places a locator (`0x4af740`).

use crate::util::{bad, cstr_at, f32s, slice, u32_at, Result};

pub const MAGIC: u32 = 0x0b0b_50f1;

#[derive(Debug, Clone)]
pub struct Locator {
    pub name: String,
    pub matrix: [[f32; 4]; 4],
}

pub fn parse(d: &[u8]) -> Result<Vec<Locator>> {
    if u32_at(d, 0)? != MAGIC || u32_at(d, 4)? != 0 {
        return bad("not a .pop file");
    }
    if slice(d, 0x14, 12)?.iter().any(|&b| b != 0) {
        return bad("header padding is not zero");
    }
    let (count, names_off, mat_off) = (u32_at(d, 8)? as usize, u32_at(d, 12)? as usize, u32_at(d, 16)? as usize);
    (0..count)
        .map(|i| {
            let m = f32s::<16>(d, mat_off + 64 * i)?;
            let matrix = [0, 1, 2, 3].map(|r| [m[4 * r], m[4 * r + 1], m[4 * r + 2], m[4 * r + 3]]);
            Ok(Locator { name: cstr_at(d, u32_at(d, names_off + 4 * i)? as usize)?, matrix })
        })
        .collect()
}

/// The matrix the game gives the locator: Z flipped on both sides, m23 forced to 0.
pub fn game_matrix(m: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let s = [1.0, 1.0, -1.0, 1.0];
    let mut out = [0, 1, 2, 3].map(|r| [0, 1, 2, 3].map(|c| s[r] * m[r][c] * s[c]));
    out[2][3] = 0.0;
    out
}
