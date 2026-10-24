//! Stadium splines at run time (`GameUpdate\Splines\CSpline.cpp`; `docs/GAMEPLAY.md`, "Spline evaluation"): the
//! parameter is a fractional point index, positions are interpolated linearly between points and scaled, and
//! rotations are slerped. The Snitch and both seekers move along one of these during the Seeker game.
//!
//! Not ported: branches (bits 4-15 of a point's flags; no retail file uses them) and the global height offset
//! `[0xfd307c]` added to every position (zero-filled, no writer found).

use qwc_formats::qsp;

use crate::fmath::{acos, sin};
use crate::vec3::{sub, Vec3};

/// Above this quaternion dot product the slerp falls back to a linear blend (`0x5bf24c`, 0.8 as stored).
const SLERP_LINEAR: f64 = 0.799_999_9;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub pos: Vec3,
    /// `x, y, z, w`, stored inverted (see [`Frame`]).
    pub rot: [f64; 4],
}

/// The axes of the rotation matrix `0x42c6e0` builds from a point's quaternion (its rows): with the stored
/// (inverted) quaternion these are the trail's side, up and forward directions in world space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub side: Vec3,
    pub up: Vec3,
    pub fwd: Vec3,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Spline {
    pub points: Vec<Point>,
    /// `CSpline +0x20` (`SeekerGame.Spline.SplineBaseScale`, written by the loader `0x492660`) times `Pitch.Scale`.
    pub scale: f64,
    /// `CSpline +0x2c` bit 0; the loader sets it on every spline it loads.
    pub looping: bool,
}

impl Spline {
    pub fn new(points: Vec<Point>, scale: f64) -> Spline {
        Spline { points, scale, looping: true }
    }

    pub fn from_qsp(s: &qsp::Spline, scale: f64) -> Spline {
        let points = s.points.iter().map(|p| Point { pos: p.pos.map(f64::from), rot: p.rot.map(f64::from) }).collect();
        Spline::new(points, scale)
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// `0x4924f0` without branches: an index past the end reads point 0.
    fn at(&self, i: i64) -> &Point {
        let n = self.points.len() as i64;
        &self.points[if (0..n).contains(&i) { i as usize } else { 0 }]
    }

    /// `0x491880`: the point `trunc(t)`. Below 0 it counts back from the end; beyond the last point it wraps on a
    /// looping spline (clamps otherwise); exactly one past the end it reads point 0.
    fn point(&self, i: i64) -> &Point {
        let n = self.points.len() as i64;
        if i < 0 {
            self.at(i + n)
        } else if i > n {
            self.at(if self.looping { i % n } else { n - 1 })
        } else {
            self.at(i)
        }
    }

    /// `0x4925a0`: wrap a parameter into the spline (looping) or clamp it to `[0, n - 1]`.
    pub fn normalise(&self, t: f64) -> f64 {
        let n = self.points.len() as f64;
        if !self.looping {
            return if t < 0.0 {
                0.0
            } else if t >= n {
                n - 1.0
            } else {
                t
            };
        }
        let mut t = t;
        if t >= n {
            while t >= n {
                t -= n;
            }
        } else if t < -n {
            while t < -n {
                t += n;
            }
        }
        t
    }

    /// `0x491900`, the position: linear between the points either side of `t`, then scaled.
    pub fn position(&self, t: f64) -> Vec3 {
        let i = t.trunc();
        let f = t - i;
        let a = self.point(i as i64).pos;
        let p = if f > 0.0 {
            let b = self.point(i as i64 + 1).pos;
            [0, 1, 2].map(|k| (b[k] - a[k]) * f + a[k])
        } else {
            a
        };
        p.map(|v| v * self.scale)
    }

    /// `0x491900`, the rotation: the points' quaternions slerped (`0x42c570`).
    pub fn rotation(&self, t: f64) -> [f64; 4] {
        let i = t.trunc();
        let f = t - i;
        let a = self.point(i as i64).rot;
        if f > 0.0 {
            slerp(a, self.point(i as i64 + 1).rot, f)
        } else {
            a
        }
    }

    pub fn frame(&self, t: f64) -> Frame {
        frame(self.rotation(t))
    }

    /// `0x4916f0` (with its last argument 0): the start of the first level stretch, where the up axis is within
    /// `tol` of vertical in x and z, at least `run` points long, plus `run`; failing that the longest such stretch
    /// (a stretch still open at the last point does not count), or just `run`.
    pub fn level_start(&self, run: f64, tol: f64) -> f64 {
        let (mut start, mut best_start, mut best_len, mut len) = (0usize, None, 0usize, 0usize);
        let mut open = false;
        for i in 0..self.points.len() {
            let up = frame(self.points[i].rot).up;
            let level = up[0].abs() <= tol && up[2].abs() <= tol;
            if open {
                if level {
                    len += 1;
                } else {
                    open = false;
                    if run <= len as f64 {
                        return start as f64 + run;
                    }
                    if best_len < len {
                        best_start = Some(start);
                        best_len = len;
                    }
                }
            } else if level {
                open = true;
                len = 1;
                start = i;
            }
        }
        best_start.map_or(run, |s| s as f64 + run)
    }

    /// The segment the Snitch measures its progress on (`0x44f060`): from point `i` to the next point that is not
    /// at the same place (stepping the parameter by 0.1 past duplicates).
    pub fn segment(&self, i: f64) -> (Vec3, Vec3) {
        let a = self.position(i);
        let mut k = 1.0;
        let mut b = self.position(i + k);
        while sub(b, a) == [0.0; 3] && k < self.points.len() as f64 {
            k += 0.1;
            b = self.position(i + k);
        }
        (a, b)
    }
}

/// `0x42c6e0`: the rows of the rotation matrix of `q`.
pub fn frame(q: [f64; 4]) -> Frame {
    let [x, y, z, w] = q;
    let (x2, y2, z2) = (x + x, y + y, z + z);
    let (xx, yy, zz) = (x * x2, y * y2, z * z2);
    let (xy, xz, yz) = (x * y2, x * z2, y * z2);
    let (wx, wy, wz) = (w * x2, w * y2, w * z2);
    Frame {
        side: [1.0 - (zz + yy), xy - wz, xz + wy],
        up: [xy + wz, 1.0 - (zz + xx), yz - wx],
        fwd: [xz - wy, yz + wx, 1.0 - (yy + xx)],
    }
}

/// `0x42c570`: spherical blend along the shorter arc; linear (not renormalised) when the quaternions are close.
pub fn slerp(a: [f64; 4], b: [f64; 4], f: f64) -> [f64; 4] {
    let mut d = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let neg = d < 0.0;
    if neg {
        d = -d;
    }
    let (s0, mut s1) = if d <= SLERP_LINEAR {
        let th = acos(d);
        let s = sin(th);
        (sin((1.0 - f) * th) / s, sin(th * f) / s)
    } else {
        (1.0 - f, f)
    };
    if neg {
        s1 = -s1;
    }
    [0, 1, 2, 3].map(|k| a[k] * s0 + b[k] * s1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fmath::cos;

    /// A square loop of side 100 in the x-z plane (`scale` 1), level, heading round it.
    pub(crate) fn square() -> Spline {
        let q = |deg: f64| {
            let h = deg.to_radians() / 2.0;
            // stored inverted: conj of a yaw about +y
            [0.0, -sin(h), 0.0, cos(h)]
        };
        let pts = [
            ([0.0, 0.0, 0.0], 0.0),
            ([0.0, 0.0, 100.0], 90.0),
            ([100.0, 0.0, 100.0], 180.0),
            ([100.0, 0.0, 0.0], 270.0),
        ];
        Spline::new(pts.iter().map(|&(p, d)| Point { pos: p, rot: q(d) }).collect(), 1.0)
    }

    #[test]
    fn positions_wrap_and_scale() {
        let mut s = square();
        assert_eq!(s.position(0.5), [0.0, 0.0, 50.0]);
        // the last segment closes the loop
        assert_eq!(s.position(3.5), [50.0, 0.0, 0.0]);
        assert_eq!(s.position(4.25), [0.0, 0.0, 25.0]);
        assert_eq!(s.position(-0.5), [0.0; 3]); // trunc(-0.5) = 0 and the fraction is negative: point 0
        assert_eq!(s.position(-1.0), [100.0, 0.0, 0.0]);
        assert_eq!(s.normalise(9.5), 1.5);
        s.scale = 0.5;
        assert_eq!(s.position(1.0), [0.0, 0.0, 50.0]);
        s.looping = false;
        assert_eq!(s.normalise(9.5), 3.0);
        assert_eq!(s.position(6.0), [50.0, 0.0, 0.0]);
    }

    #[test]
    fn frames_point_along_the_trail() {
        let s = square();
        let f = s.frame(0.0);
        assert!((f.fwd[2] - 1.0).abs() < 1e-12 && (f.up[1] - 1.0).abs() < 1e-12, "{f:?}");
        let f = s.frame(1.0);
        assert!((f.fwd[0] - 1.0).abs() < 1e-12, "{f:?}");
        // halfway it has turned 45 degrees (a linear blend: these quaternions are close)
        let f = s.frame(0.5);
        assert!((f.fwd[0] - f.fwd[2]).abs() < 1e-12 && f.fwd[0] > 0.6, "{f:?}");
        // close quaternions blend linearly
        let q = slerp([0.0, 0.0, 0.0, 1.0], [0.0, 0.1, 0.0, 0.995], 0.5);
        assert!((q[1] - 0.05).abs() < 1e-12 && (q[3] - 0.9975).abs() < 1e-12);
    }

    #[test]
    fn level_start_finds_a_long_level_stretch() {
        let mut s = square();
        // 30 points: level from 5 to 19 (15 points), banked elsewhere
        let banked = [sin(0.3f64), 0.0, 0.0, cos(0.3f64)];
        s.points = (0..30)
            .map(|i| Point {
                pos: [i as f64, 0.0, 0.0],
                rot: if (5..20).contains(&i) { [0.0, 0.0, 0.0, 1.0] } else { banked },
            })
            .collect();
        assert_eq!(s.level_start(10.0, 0.15), 15.0);
        // too short: the longest stretch, or just the run when none ends
        assert_eq!(s.level_start(20.0, 0.15), 25.0);
        for p in &mut s.points[20..] {
            p.rot = [0.0, 0.0, 0.0, 1.0];
        }
        assert_eq!(s.level_start(20.0, 0.15), 20.0);
    }
}
