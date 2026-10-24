//! Front-end screen wipes (`frontend/Common/Wipes/*.WPE`; port of `tools/wpe_parse.py`), played by
//! `LayerObj_ActivateWipe` (`0x4d5df0`).
//!
//! ```text
//! u8 width | u8 height | u16 frames | frames x height x width u8 coverage (0 = old screen, 255 = new)
//! ```

use crate::util::{bad, slice, u16_at, Result};

#[derive(Debug, Clone)]
pub struct Wipe {
    pub width: u8,
    pub height: u8,
    pub frames: Vec<Vec<u8>>,
}

impl Wipe {
    pub fn parse(d: &[u8]) -> Result<Wipe> {
        let (w, h, n) = (slice(d, 0, 2)?[0], d[1], u16_at(d, 2)? as usize);
        let size = w as usize * h as usize;
        if d.len() != 4 + n * size {
            return bad(format!("size {} does not match {w}x{h}x{n}", d.len()));
        }
        Ok(Wipe { width: w, height: h, frames: d[4..].chunks(size.max(1)).map(<[u8]>::to_vec).collect() })
    }
}
