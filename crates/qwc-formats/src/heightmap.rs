//! Pitch height maps (`stadium/<name>/gfx/heightmap.csv`; port of `tools/heightmap_parse.py`, parser `0x4d0e30`).
//!
//! The game splits the whole file on `" ,\n\r"` and reads floats with `atof` into 3 maps x 20 rows x 10 columns;
//! line breaks carry no meaning. Rows run along the pitch length, columns across it. The height under a point is
//! `base + Pitch.HeightMapHeight * (h - 1)` with `h` the bilinear lookup (`0x41a570`).

use crate::util::{bad, Result};

pub const MAPS: usize = 3;
pub const ROWS: usize = 20;
pub const COLS: usize = 10;

/// The three 20x10 maps; fails unless the file holds exactly 600 numbers.
pub fn parse(d: &[u8]) -> Result<Vec<Vec<Vec<f64>>>> {
    let text = std::str::from_utf8(d).map_err(|e| crate::Error::Format(format!("not text: {e}")))?;
    let tokens: Vec<&str> = text.split([' ', ',', '\n', '\r']).filter(|t| !t.is_empty()).collect();
    if tokens.len() != MAPS * ROWS * COLS {
        return bad(format!("{} numbers, expected {}", tokens.len(), MAPS * ROWS * COLS));
    }
    let values: Vec<f64> = tokens
        .iter()
        .map(|t| t.parse().map_err(|_| crate::Error::Format(format!("not a number: {t:?}"))))
        .collect::<Result<_>>()?;
    Ok(values.chunks(ROWS * COLS).map(|m| m.chunks(COLS).map(<[f64]>::to_vec).collect()).collect())
}
