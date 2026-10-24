//! Flight splines (`.qsp`, magic "1PSQ"; port of `tools/qsp_parse.py`, loader `FUN_00492660`): the stadium
//! flight loop the seeker camera follows.
//!
//! ```text
//! 0x00 "1PSQ" | u32 file_size | u32 spline_count | u32 ? (0, unused by the loader)
//! 0x10 u32[count] spline offsets; names follow, NUL-terminated, 4-byte padded
//! spline: u32 name_off | u32 n_points | n_points x { f32 x, y, z; u32 flags; f32 qx, qy, qz, qw }
//! ```
//! `flags` is built at load time (0 in every file). The quaternion is stored inverted: `conj(q)` turns local +Z
//! into the direction of travel.

use crate::util::{bad, cstr_at, f32s, slice, u32_at, Result};

pub const POINT_SIZE: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub pos: [f32; 3],
    pub flags: u32,
    pub rot: [f32; 4],
}

#[derive(Debug, Clone)]
pub struct Spline {
    pub name: String,
    pub points: Vec<Point>,
}

#[derive(Debug, Clone)]
pub struct Qsp {
    pub unk_0c: u32,
    pub splines: Vec<Spline>,
}

impl Qsp {
    pub fn parse(d: &[u8]) -> Result<Qsp> {
        if slice(d, 0, 4)? != b"1PSQ" {
            return bad("not a .qsp file");
        }
        let (size, count, unk_0c) = (u32_at(d, 4)?, u32_at(d, 8)? as usize, u32_at(d, 12)?);
        if size as usize != d.len() {
            return bad(format!("header size {size} != file size {}", d.len()));
        }
        let mut splines = Vec::new();
        let mut end = 0x10 + 4 * count;
        for i in 0..count {
            let off = u32_at(d, 0x10 + 4 * i)? as usize;
            let (name_off, n) = (u32_at(d, off)? as usize, u32_at(d, off + 4)? as usize);
            let name = cstr_at(d, name_off)?;
            if !name.is_ascii() {
                return bad(format!("spline name at {name_off:#x} is not ASCII"));
            }
            let mut points = Vec::new();
            for k in 0..n {
                let p = off + 8 + POINT_SIZE * k;
                points.push(Point { pos: f32s::<3>(d, p)?, flags: u32_at(d, p + 12)?, rot: f32s::<4>(d, p + 16)? });
            }
            splines.push(Spline { name, points });
            end = end.max(off + 8 + POINT_SIZE * n);
        }
        if end != d.len() {
            return bad(format!("data not fully consumed: {end:#x} of {:#x}", d.len()));
        }
        Ok(Qsp { unk_0c, splines })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One spline "TST0001" (as `tests/test_qsp_parse.py` builds it).
    fn build(points: &[[f32; 8]]) -> Vec<u8> {
        let mut body = [0x14u32, points.len() as u32].map(u32::to_le_bytes).concat();
        for p in points {
            for (i, v) in p.iter().enumerate() {
                body.extend_from_slice(&if i == 3 { (*v as u32).to_le_bytes() } else { v.to_le_bytes() });
            }
        }
        let head = b"TST0001\0\0\0\0\0";
        let size = 0x14 + head.len() + body.len();
        let mut d = b"1PSQ".to_vec();
        for v in [size, 1, 0, 0x14 + head.len()] {
            d.extend_from_slice(&(v as u32).to_le_bytes());
        }
        d.extend_from_slice(head);
        d.extend(body);
        d
    }

    #[test]
    fn roundtrip() {
        let q = Qsp::parse(&build(&[
            [0.0, 100.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
            [10.0, 100.0, 0.0, 1.0, 0.0, 0.5, 0.0, 0.5],
        ]))
        .unwrap();
        let s = &q.splines[0];
        assert_eq!(s.name, "TST0001");
        assert_eq!(s.points[1], Point { pos: [10.0, 100.0, 0.0], flags: 1, rot: [0.0, 0.5, 0.0, 0.5] });
    }

    #[test]
    fn rejects_bad_files() {
        let mut d = build(&[[0.0; 8]]);
        d.extend_from_slice(&[0; 4]);
        assert!(Qsp::parse(&d).is_err()); // size field
        assert!(Qsp::parse(b"QSP1\0\0\0\0").is_err());
    }
}
