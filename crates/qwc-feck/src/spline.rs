//! The spline actions (`docs/FECK_VM.md`, "Spline actions"): `LayerObj_*_AddAction_Spline` move an object along
//! keys in the script's own memory over a time, like the other timed actions. Positions are Catmull-Rom curves,
//! rotations quaternion blends (`0x42c570`) with the exe's own arc tangent (`0x42c460`).
//!
//! **[I]** The original keeps some intermediates at x87 precision; the port rounds where the decompiled code
//! stores a float, so a result can differ from the original's in its last bit.

use std::f32::consts::{FRAC_PI_2, PI};

use crate::layer::{ftol, Object};
use crate::memory::{Fault, Memory};

/// Which object a spline drives (the action vtables `0x5abea4`, `0x5abe40`, `0x5abe7c`, `0x5abe54`, `0x5abddc`,
/// `0x5abe18`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spline {
    /// A model: keys `{position, quaternion}` (7 floats), the rotation into `+0xc4`.
    Model,
    /// A Quidditch card: as a model, the rotation tilted towards none and normalised into `+0xc8`.
    Card,
    /// A camera: keys of 9 floats, looping, positions scaled by 0.01, the rotation as two rows of its matrix.
    Camera,
    /// A sprite: 2D keys, the curve's direction into the rotation; it may loop.
    Sprite,
    /// A puppet: as a model, into the puppet's own position and rotation (`+0xcc`-`+0xe8`).
    Puppet,
    /// A puppet relative to where it stands: camera-like keys, positions scaled by 0.2.
    PuppetRelative,
}

/// The second puppet spline adds this to its height (`[0x668bfc]`, a global that starts at -3). **[I]** the
/// port takes it as constant; what changes it is not traced.
pub const RELATIVE_HEIGHT: f32 = -3.0;

const ATAN_A: f32 = f32::from_bits(0x3edc_f805); // 0.43158
const ATAN_B: f32 = f32::from_bits(0x3f43_b24e); // 0.76444
const ATAN_C: f32 = f32::from_bits(0x3d6e_e04d); // 0.05832
/// Quaternions closer than this blend linearly (`0x5bf24c`, 0.8).
const SLERP_LINEAR: f32 = f32::from_bits(0x3f4c_ccbc);

/// `0x42c460`: a rational arc tangent, `(x + 0.43158x³) / (1 + 0.76444x² + 0.05832x⁴)`, through `±π/2 - atan(1/x)`
/// past ±1.
pub fn atan(x: f32) -> f64 {
    let (x, k, off) = if x > 1.0 || x < -1.0 {
        let r = 1.0 / x;
        (r, -1.0, if r < 0.0 { -FRAC_PI_2 } else { FRAC_PI_2 } as f64)
    } else {
        (x, 1.0, 0.0)
    };
    let x = x as f64;
    let (x2, x3) = (x * x, x * x * x);
    off + k * (ATAN_A as f64 * x3 + x) / (ATAN_B as f64 * x2 + x3 * x * ATAN_C as f64 + 1.0)
}

/// `0x42c510`: `atan(sqrt(1 - x²) / |x|)`, from π for negative `x`; 0 past ±1.
pub fn acos(x: f32) -> f64 {
    let ax = x.abs();
    if ax > 1.0 {
        return 0.0;
    }
    let r = atan(((1.0 - x as f64 * x as f64).sqrt() / ax as f64) as f32);
    if x < 0.0 {
        PI as f64 - r
    } else {
        r
    }
}

/// `0x42c570`: from quaternion `a` (`[x, y, z, w]`) to `b` at `t`, the shorter way, linearly when they are close.
pub fn slerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let mut dot = b[3] * a[3] + b[2] * a[2] + b[1] * a[1] + b[0] * a[0];
    let neg = dot < 0.0;
    if neg {
        dot = -dot;
    }
    let (wa, mut wb) = if dot <= SLERP_LINEAR {
        let th = acos(dot) as f32;
        let inv = 1.0 / (th as f64).sin() as f32;
        let wa = ((1.0 - t) as f64 * th as f64).sin() as f32 * inv;
        (wa, ((th * t) as f64).sin() as f32 * inv)
    } else {
        (1.0 - t, t)
    };
    if neg {
        wb = -wb;
    }
    [0, 1, 2, 3].map(|k| a[k] * wa + b[k] * wb)
}

/// `0x42c6e0`: a quaternion's rotation matrix, row by row (16 floats, the last row `0, 0, 0, 1`).
pub fn matrix(q: [f32; 4]) -> [f32; 16] {
    let [x, y, z, w] = q;
    let (x2, y2, z2) = (x + x, y + y, z + z);
    let (zz, wz) = (z2 * z, z2 * w);
    [
        1.0 - (zz + y2 * y),
        x2 * y - wz,
        x2 * z + y2 * w,
        0.0,
        x2 * y + wz,
        1.0 - (zz + x2 * x),
        y2 * z - x2 * w,
        0.0,
        x2 * z - y2 * w,
        y2 * z + x2 * w,
        1.0 - (y2 * y + x2 * x),
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ]
}

/// Not the original's: the unit quaternion whose [`matrix`] is `m`'s rotation (its upper 3x3, a pure rotation).
pub fn quaternion(m: &[f32; 16]) -> [f32; 4] {
    let (m00, m01, m02, m10, m11, m12, m20, m21, m22) = (m[0], m[1], m[2], m[4], m[5], m[6], m[8], m[9], m[10]);
    let q = if 0.0 < m00 + m11 + m22 {
        let s = (1.0 + m00 + m11 + m22).sqrt() * 2.0;
        [(m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s, s / 4.0]
    } else if m11 < m00 && m22 < m00 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        [s / 4.0, (m01 + m10) / s, (m02 + m20) / s, (m21 - m12) / s]
    } else if m22 < m11 {
        let s = (1.0 - m00 + m11 - m22).sqrt() * 2.0;
        [(m01 + m10) / s, s / 4.0, (m12 + m21) / s, (m02 - m20) / s]
    } else {
        let s = (1.0 - m00 - m11 + m22).sqrt() * 2.0;
        [(m02 + m20) / s, (m12 + m21) / s, s / 4.0, (m10 - m01) / s]
    };
    let n = q.iter().map(|v| v * v).sum::<f32>().sqrt();
    q.map(|v| v / n)
}

/// The Catmull-Rom weights of points `p0`-`p3` at `f`, halved at the end: `p1` weighs `3f³ + 2 - 5f²`, `p2`
/// `f(4f + 1 - 3f²)`, `p0` `-f(1-f)²` and `p3` `-(1-f)f²`.
fn catmull(p: [f32; 4], f: f32) -> f32 {
    let (f2, g) = (f * f, 1.0 - f);
    let w1 = f2 * f * 3.0 + (2.0 - f2 * 5.0);
    let w2 = (f * 4.0 + 1.0) - f2 * 3.0;
    ((w1 * p[1] + (w2 * p[2] - g * g * p[0]) * f) - g * f2 * p[3]) * 0.5
}

/// Clamped to 0..1 (NaN passes), as every update starts.
fn clamp01(t: f32) -> f32 {
    t.clamp(0.0, 1.0)
}

/// `t * n (+ add)` with `n` an unsigned count (a negative one is 2³² more), split into its whole part (`_ftol`)
/// and the rest.
fn split(t: f32, n: i32, add: f32) -> (i32, f32) {
    let n = if n < 0 { n as f64 + 4294967296.0 } else { n as f64 };
    let s = t as f64 * n + add as f64;
    let i = ftol(s as f32);
    (i, (s - i as f64) as f32)
}

/// Key `k` of `n` floats at `keys` (the script's memory; the original reads what lies there, past the ends too).
fn key<const N: usize>(mem: &Memory, keys: u32, k: i32) -> Result<[f32; N], Fault> {
    let base = keys.wrapping_add((k as u32).wrapping_mul(4 * N as u32));
    let mut out = [0.0; N];
    for (n, v) in out.iter_mut().enumerate() {
        *v = f32::from_bits(mem.u32(base.wrapping_add(4 * n as u32))?);
    }
    Ok(out)
}

fn quat(k: &[f32]) -> [f32; 4] {
    [k[3], k[4], k[5], k[6]]
}

fn set_f(o: &mut Object, off: u16, v: &[f32]) {
    for (n, x) in v.iter().enumerate() {
        o.set_slot(off + 4 * n as u16, x.to_bits());
    }
}

/// The action's start (its `+0`, run as it is added).
pub fn begin(o: &mut Object, s: Spline) {
    match s {
        // the rotation now comes from the quaternion (`0x4d7ba0`, `0x4d82d0`)
        Spline::Model => o.set_slot(0xd4, 1),
        Spline::Card => o.set_slot(0xd8, 1),
        // `0x4d52e0`: no interest object
        Spline::Camera => o.set_slot(0xbc, 0),
        // `0x4d8030` also keeps the puppet's heading and position in the action, which nothing reads
        Spline::Puppet | Spline::PuppetRelative => o.set_slot(0x130, 1),
        Spline::Sprite => {}
    }
}

/// The finish method (`+0x10`): no end value is set, the last update stands. The relative puppet spline lets the
/// puppet go (`0x4d53d0`).
pub fn finish(o: &mut Object, s: Spline) {
    if s == Spline::PuppetRelative {
        o.set_slot(0x130, 0);
    }
}

/// The update at `t` (the fraction of the time gone, `+0xc`). `elapsed` is the action's frame count, which a
/// looping sprite spline sets back to 0 before its last frame, so it never ends.
#[allow(clippy::too_many_arguments)]
pub fn update(
    o: &mut Object,
    s: Spline,
    mem: &Memory,
    keys: u32,
    count: u32,
    looping: bool,
    elapsed: &mut u32,
    duration: u32,
    t: f32,
) -> Result<(), Fault> {
    let n = count as i32;
    match s {
        // `0x4d7bb0`, `0x4d82e0`, `0x4d8070`
        Spline::Model | Spline::Card | Spline::Puppet => {
            let t = clamp01(t);
            let (i, f) = split(t, if s == Spline::Puppet { n.wrapping_sub(3) } else { n.wrapping_sub(2) }, 1.0);
            let last = if s != Spline::Puppet && count <= (i + 2) as u32 { i + 1 } else { i + 2 };
            let k: Vec<[f32; 7]> =
                [i - 1, i, i + 1, last].iter().map(|&k| key(mem, keys, k)).collect::<Result<_, _>>()?;
            let pos = [0, 1, 2].map(|a| catmull([k[0][a], k[1][a], k[2][a], k[3][a]], f));
            let q = slerp(quat(&k[1]), quat(&k[2]), f);
            match s {
                Spline::Model => {
                    set_f(o, 0xc4, &q);
                    o.pos = [pos[0], pos[1], pos[2], 0.0];
                }
                Spline::Card => {
                    let q = [q[0] * 0.6, q[1] * 0.6, q[2] * 0.6, q[3]];
                    let inv = 1.0 / (q[3] * q[3] + q[2] * q[2] + q[1] * q[1] + q[0] * q[0]).sqrt();
                    set_f(o, 0xc8, &q.map(|x| x * inv));
                    o.pos = [pos[0], pos[1], pos[2], 0.0];
                }
                _ => {
                    set_f(o, 0xcc, &q);
                    set_f(o, 0xdc, &pos);
                    o.set_slot(0xe8, 0);
                }
            }
        }
        // `0x4d7970`: the last key runs on to the first
        Spline::Camera => {
            let (i, f) = split(clamp01(t), n, 0.0);
            let j = if i + 1 == n { 0 } else { i + 1 };
            let (a, b): ([f32; 9], [f32; 9]) = (key(mem, keys, i)?, key(mem, keys, j)?);
            let m = matrix(slerp(quat(&a), quat(&b), f));
            let p = [0, 1, 2].map(|k| ((b[k] - a[k]) * f + a[k]) * 0.01);
            o.pos = [p[0], p[1], p[2], a[6]];
            set_f(o, 0xc4, &m[8..12]);
            set_f(o, 0xd4, &m[4..8]);
            o.set_slot(0xbc, 0);
        }
        // `0x4d7e10`: the last key holds
        Spline::PuppetRelative => {
            let (i, f) = split(clamp01(t), n.wrapping_sub(1), 0.0);
            let j = if n <= i + 1 { i } else { i + 1 };
            let (a, b): ([f32; 9], [f32; 9]) = (key(mem, keys, i)?, key(mem, keys, j)?);
            set_f(o, 0xcc, &slerp(quat(&a), quat(&b), f));
            o.set_slot(0x131, 1);
            o.set_slot(0xe8, b[6].to_bits());
            let x = ((b[0] - a[0]) * f + a[0]) * 0.2;
            let y = ((b[1] - a[1]) * f + a[1]) * 0.2;
            let z = (RELATIVE_HEIGHT + (b[2] - a[2]) * f + a[2]) * 0.2;
            set_f(o, 0xdc, &[x, y, z]);
        }
        // `0x4d85b0`: a looping one takes the fraction of `t` and starts again before it would end
        Spline::Sprite => {
            let t = if looping {
                if *elapsed + 1 >= duration {
                    *elapsed = 0;
                }
                t - t.floor()
            } else {
                clamp01(t)
            };
            let n = n.wrapping_sub(3);
            let s = t as f64 * if n < 0 { n as f64 + 4294967296.0 } else { n as f64 } + 1.0;
            let i = ftol(s as f32);
            let f = s - i as f64;
            let p: Vec<[f32; 2]> = (i - 1..=i + 2).map(|k| key(mem, keys, k)).collect::<Result<_, _>>()?;
            let (f2, g) = ((f * f) as f32 as f64, 1.0 - f);
            let w2 = ((f * 4.0 + 1.0) - f2 * 3.0) as f32 as f64;
            let w1 = (f * f * f * 3.0 + (2.0 - f2 * 5.0)) as f32 as f64;
            let g2 = (g * g) as f32 as f64;
            let at = |a: usize| {
                let c = |k: usize| p[k][a] as f64;
                (((w1 * c(1) + (w2 * c(2) - g2 * c(0)) * f) - g as f32 as f64 * f2 * c(3)) * 0.5) as f32
            };
            let dir = |a: usize| {
                let c = |k: usize| p[k][a] as f64;
                let d = c(2) - c(0);
                (((c(3) - c(1)) - d) * f + d) as f32
            };
            let (dx, dy) = (dir(0), dir(1));
            let inv = 1.0 / (dx * dx + dy * dy).sqrt();
            o.pos[0] = at(0);
            o.pos[1] = at(1);
            o.rot[0] = inv * dx;
            o.rot[1] = inv * dy;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_exe_arc_tangent_is_close_to_the_real_one() {
        for x in [-5.0f32, -1.0, -0.3, 0.0, 0.25, 0.5, 1.0, 2.0, 40.0] {
            assert!((atan(x) - (x as f64).atan()).abs() < 2e-5, "{x}");
        }
        assert!((acos(0.5) - 0.5f64.acos()).abs() < 2e-5);
        assert!((acos(-0.5) - (-0.5f64).acos()).abs() < 2e-5);
        assert_eq!(acos(1.5), 0.0);
    }

    #[test]
    fn a_rotation_matrix_gives_back_its_quaternion() {
        for q in [[0.0, 0.0, 0.0, 1.0], [0.6, 0.0, 0.0, 0.8], [0.0, -0.6, 0.0, 0.8], [0.0, 0.0, 1.0, 0.0], [0.5; 4]] {
            let back = quaternion(&matrix(q));
            let same = back.iter().zip(&q).all(|(a, b)| (a - b).abs() < 1e-5);
            let opposite = back.iter().zip(&q).all(|(a, b)| (a + b).abs() < 1e-5);
            assert!(same || opposite, "{q:?} {back:?}");
        }
    }

    #[test]
    fn slerp_takes_the_short_way_and_blends_close_ones_linearly() {
        let id = [0.0, 0.0, 0.0, 1.0];
        let s = std::f32::consts::FRAC_1_SQRT_2;
        // 90 degrees about z: halfway is 45 degrees
        let h = slerp(id, [0.0, 0.0, s, s], 0.5);
        assert!((h[2] - (PI / 8.0).sin()).abs() < 1e-4 && (h[3] - (PI / 8.0).cos()).abs() < 1e-4, "{h:?}");
        // the same rotation written negated goes the short way too, to the same halfway
        let n = slerp(id, [0.0, 0.0, -s, -s], 0.5);
        assert!((n[2] - h[2]).abs() < 1e-6 && (n[3] - h[3]).abs() < 1e-6, "{n:?}");
        // close ones: a plain blend
        assert_eq!(slerp(id, [0.0, 0.0, 0.0, 1.0], 0.25), id);
        assert_eq!(matrix(id), [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn catmull_rom_passes_through_its_keys() {
        let p = [0.0, 1.0, 3.0, 4.0];
        assert_eq!(catmull(p, 0.0), 1.0);
        assert_eq!(catmull(p, 1.0), 3.0);
        // evenly spaced points: the middle is the middle
        assert_eq!(catmull([0.0, 1.0, 2.0, 3.0], 0.5), 1.5);
    }
}
