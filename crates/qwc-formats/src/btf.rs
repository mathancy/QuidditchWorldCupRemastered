//! Animation trigger tracks (`.btf`; port of `tools/btf_parse.py`).
//!
//! Same 0x1C-byte header as `.fas` (magic `0xD9CE785F`, `flags & 0x47f == 0x400`, `bulk_len` 0). Stream:
//! `u32 n_anims`; per anim `opt_str name, u32 n_triggers`; per trigger `u32 has_node[, cstr node], opt_str name,
//! f32 time_s`, then `{u8 1, f32 time_s}*` ended by `u8 0` (further fire times). The trigger list reader is
//! `0x40a370` (the node name, then the name, then `0x40a1c0`'s times); every file is consumed to the last byte, and
//! none names a node.

use crate::fas::{Header, MAGIC};
use crate::util::{bad, Reader, Result};

#[derive(Debug, Clone, PartialEq)]
pub struct Trigger {
    /// Nonzero when a node name follows.
    pub u0: u32,
    /// The node it fires at (`+8`, bound to the model's node of that name).
    pub node: Option<String>,
    pub name: Option<String>,
    pub times: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct Btf {
    /// `(anim name, triggers)` in file order.
    pub anims: Vec<(Option<String>, Vec<Trigger>)>,
}

impl Btf {
    pub fn parse(d: &[u8]) -> Result<Btf> {
        let h = Header::parse(d)?;
        if h.magic != MAGIC || h.flags & 0x47F != 0x400 {
            return bad("not a .btf file");
        }
        let mut s = Reader::new(&d[0x1C + h.bulk_len as usize..]);
        let mut anims = Vec::new();
        for _ in 0..s.u32()? {
            let name = s.opt_str()?;
            let mut trig = Vec::new();
            for _ in 0..s.u32()? {
                let u0 = s.u32()?;
                let node = if u0 != 0 { Some(s.cstr()?) } else { None };
                let name = s.opt_str()?;
                let mut times = vec![s.f32()?];
                while s.u8()? != 0 {
                    times.push(s.f32()?);
                }
                trig.push(Trigger { u0, node, name, times });
            }
            anims.push((name, trig));
        }
        if !s.at_end() {
            return bad(format!("stream not fully consumed: {:#x} of {:#x}", s.p, s.d.len()));
        }
        Ok(Btf { anims })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_trigger_two_times() {
        let mut s = 1u32.to_le_bytes().to_vec();
        s.extend_from_slice(b"\x01cam\0");
        s.extend_from_slice(&1u32.to_le_bytes());
        s.extend_from_slice(&0u32.to_le_bytes());
        s.extend_from_slice(b"\x01CS_CAM_CUT\0");
        s.extend_from_slice(&0.5f32.to_le_bytes());
        s.push(1);
        s.extend_from_slice(&2.0f32.to_le_bytes());
        s.push(0);
        let mut d = Vec::new();
        for v in [MAGIC, 0x400, 5, 12, 0, s.len() as u32, 0] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend(s);
        let b = Btf::parse(&d).unwrap();
        assert_eq!(b.anims[0].0.as_deref(), Some("cam"));
        assert_eq!(b.anims[0].1[0].times, vec![0.5, 2.0]);
        d.push(0);
        assert!(Btf::parse(&d).is_err());
    }
}
