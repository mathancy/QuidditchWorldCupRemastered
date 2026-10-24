//! The pitch's walls (`docs/GAMEPLAY.md`, "Pitch walls"; traced 2026-10-21): the world's list of collision volumes
//! (`0x655ff0 + 0x125fc`), the movers' commit that keeps the players inside them (`0x465120`) and the sweep that
//! pulls a pass's target back inside (`0x41a870`).
//!
//! **[T]** The game's init (`0x42fc60`) builds the default list (`0x418d00`) and then runs the stadium's
//! `game/match.cli`, whose `Pitch.Collision.*` commands may clear and replace it (the challenge stadiums do). The
//! default is an ellipse of `Pitch.CollisionArea` (225 x 85, which the test halves into half-axes of 112.5 and 42.5)
//! for everyone, lines at `x = +-92.5` for the chasers, and each keeper's slab in front of his hoops
//! (`86.5 < |x| < 93.5`).
//!
//! The tests are flat (`x` and `z`). Every number here was checked by running the exe's own functions on the same
//! inputs (the unit tests' values).

use crate::console::Console;
use crate::vec3::Vec3;

/// The commit's soft distance (`[0x668d5c]`, 10, set nowhere): within it of a wall a move outward is slowed by the
/// square of how far in it is.
pub const SOFT: f64 = 10.0;

/// How far inside every wall the sweep leaves a point (`[0x5bf1a0]`).
pub const SWEEP_MARGIN: f64 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    /// An ellipse round `centre` (`x`, `z`) whose axes are `size` (the test uses half of each, `0x419e70`).
    /// `outside`: `Pitch.Collision.Ellipse2`, an obstacle the players keep out of (the byte `+0x24`).
    Ellipse { centre: [f64; 2], size: [f64; 2], outside: bool },
    /// A line through `point`; `normal` (unit) points to the side the players keep to (`0x41a140`).
    Line { point: [f64; 2], normal: [f64; 2] },
}

/// One collision volume: its shape, its type (`+0x8`: 0 everyone, 1 the chasers, 2 team 0's keeper, 3 team 1's)
/// and the script's index (the command's slot, 0 to 9; the defaults have none).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wall {
    pub shape: Shape,
    pub kind: u32,
    pub index: Option<u32>,
}

/// A wall's test of a point: `depth` is how far past the wall it is (negative on the allowed side), `normal` points
/// back to the allowed side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Contact {
    pub depth: f64,
    pub normal: [f64; 2],
}

fn unit2(v: [f64; 2]) -> [f64; 2] {
    let l = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if l > 0.0 {
        [v[0] / l, v[1] / l]
    } else {
        v
    }
}

impl Wall {
    /// The volume's test at `(x, z)` (vtable `+0x04`).
    pub fn test(&self, x: f64, z: f64) -> Contact {
        match self.shape {
            Shape::Line { point, normal } => {
                let d = normal[0] * (x - point[0]) + normal[1] * (z - point[1]);
                Contact { depth: -d, normal }
            }
            Shape::Ellipse { centre, size, outside } => {
                let (a, b) = (size[0] * 0.5, size[1] * 0.5);
                let (dx, dz) = (x - centre[0], z - centre[1]);
                let (u, w) = (dx / a, dz / b);
                let r2 = u * u + w * w;
                // **[T]** a point inside is measured to the ellipse's `+x` tip, not along its own direction
                let dir = if r2 < 1.0 { [1.0, 0.0] } else { unit2([u, w]) };
                let on = [a * dir[0], b * dir[1]];
                let mut depth = ((on[0] - dx).powi(2) + (on[1] - dz).powi(2)).sqrt();
                if r2.sqrt() < 1.0 {
                    depth = -depth;
                }
                let g = unit2([dir[0] * b, dir[1] * a]);
                let mut normal = [-g[0], -g[1]];
                if outside {
                    depth = -depth;
                    normal = [-normal[0], -normal[1]];
                }
                Contact { depth, normal }
            }
        }
    }

    /// Whether the commit's query (`0x41a670`, `0x419e20`) takes this wall for a mover of `filter` (its kind: 0
    /// chaser, 1 keeper on the broom, 2 keeper on a command or beater) on `team`.
    fn for_mover(&self, filter: i32, team: usize) -> bool {
        match self.kind {
            0 => true,
            1 => filter == 0 || filter == -1,
            2 => filter == -1 || (filter == 1 && team == 0),
            3 => filter == -1 || (filter == 1 && team == 1),
            _ => filter == -1,
        }
    }

    /// Whether the sweep (`0x41a870`) tests this wall for `filter` (the thrower's type code; -1 every wall).
    fn for_sweep(&self, filter: i32) -> bool {
        filter == -1 || self.kind == 0 || (self.kind == 1 && filter == 0)
    }
}

/// The world's walls in list order (the head first: the commit applies them in turn).
#[derive(Debug, Clone, PartialEq)]
pub struct Walls {
    pub list: Vec<Wall>,
}

impl Walls {
    /// The default list of `0x418d00` for `Pitch.CollisionArea` `(length, width)`.
    pub fn pitch(length: f64, width: f64) -> Walls {
        let line = |x: f64, nx: f64, kind| Wall {
            shape: Shape::Line { point: [x, 0.0], normal: [nx, 0.0] },
            kind,
            index: None,
        };
        Walls {
            list: vec![
                line(86.5, 1.0, 2),
                line(-86.5, -1.0, 3),
                line(93.5, -1.0, 2),
                line(-93.5, 1.0, 3),
                line(92.5, -1.0, 1),
                line(-92.5, 1.0, 1),
                Wall {
                    shape: Shape::Ellipse { centre: [0.0, 0.0], size: [length, width], outside: false },
                    kind: 0,
                    index: None,
                },
            ],
        }
    }

    /// The defaults, then the stadium script's `Pitch.Collision.Clear`, `Line`, `Ellipse` and `Ellipse2` calls in
    /// order (`0x41ebf0`, `0x41ec00`, `0x41ed70`): a call puts its wall at the head of the list, replacing the wall
    /// of the same index; one with fewer than five arguments, an index outside 0 to 9 or a line's normal shorter
    /// than 0.1 is refused.
    pub fn from_console(console: &Console, length: f64, width: f64) -> Walls {
        let mut walls = Walls::pitch(length, width);
        for (path, args) in console.calls() {
            let n: Vec<f64> = args.iter().map(|v| v.num().unwrap_or(0.0)).collect();
            let kind = n.get(5).copied().unwrap_or(0.0) as u32;
            let wall = match path {
                "Pitch.Collision.Clear" => {
                    walls.list.clear();
                    continue;
                }
                "Pitch.Collision.Line" if n.len() >= 5 => {
                    let normal = [n[1], n[2]];
                    if normal[0] * normal[0] + normal[1] * normal[1] < 0.01 {
                        continue;
                    }
                    Shape::Line { point: [n[3], n[4]], normal: unit2(normal) }
                }
                "Pitch.Collision.Ellipse" | "Pitch.Collision.Ellipse2" if n.len() >= 5 => {
                    Shape::Ellipse { centre: [n[1], n[2]], size: [n[3], n[4]], outside: path.ends_with('2') }
                }
                _ => continue,
            };
            let index = n[0] as i64;
            if !(0..10).contains(&index) {
                continue;
            }
            let index = Some(index as u32);
            walls.list.retain(|w| w.index != index);
            walls.list.insert(0, Wall { shape: wall, kind, index });
        }
        walls
    }

    /// The movers' commit (`0x465120`): the step from `old` to `new` with velocity `vel` (units per second), for
    /// a mover of kind `filter` on `team`. Each wall the mover's kind takes, tested at `new`, within [`SOFT`] of it
    /// (`f = ((SOFT + depth) / SOFT)^2`, at most 1), with the step going out: the velocity's outward part is
    /// taken off by `f` (and its height halved), the step's outward part too; if the step still ends past the wall it
    /// is pushed back onto it. Returns the new position (its height `new`'s) and velocity.
    pub fn commit(&self, filter: i32, team: usize, old: Vec3, new: Vec3, vel: Vec3) -> (Vec3, Vec3) {
        let mut d = [new[0] - old[0], new[2] - old[2]];
        let mut v = vel;
        for wall in self.list.iter().filter(|w| w.for_mover(filter, team)) {
            let c = wall.test(new[0], new[2]);
            let mut f = (SOFT + c.depth) / SOFT;
            if f > 0.0 {
                f *= f;
            }
            if f > 1.0 {
                f = 1.0;
            } else if f <= 0.0 {
                continue;
            }
            let n = c.normal;
            let out = n[0] * d[0] + n[1] * d[1];
            if out >= 0.0 {
                continue;
            }
            let vn = n[0] * v[0] + n[1] * v[2];
            if vn < 0.0 {
                v = [v[0] - n[0] * vn * f, v[1] * 0.5, v[2] - n[1] * vn * f];
            }
            d = [d[0] - n[0] * out * f, d[1] - n[1] * out * f];
            let c = wall.test(old[0] + d[0], old[2] + d[1]);
            if c.depth > 0.0 {
                d = [d[0] + c.normal[0] * c.depth, d[1] + c.normal[1] * c.depth];
            }
        }
        ([old[0] + d[0], new[1], old[2] + d[1]], v)
    }

    /// The sweep (`0x41a870`): `start + delta`, walked back towards `start` a unit at a time (at most `|delta|`
    /// steps) while any wall `filter` takes has it within [`SWEEP_MARGIN`] of it or past it. A step shorter than 0.1
    /// returns `start`.
    pub fn sweep(&self, start: Vec3, delta: Vec3, filter: i32) -> Vec3 {
        let l2 = delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2];
        if l2 <= 0.01 {
            return start;
        }
        let l = l2.sqrt();
        let step = [-delta[0] / l, -delta[1] / l, -delta[2] / l];
        let mut p = [start[0] + delta[0], start[1] + delta[1], start[2] + delta[2]];
        let mut t = 0.0;
        while t <= l {
            if self.list.iter().filter(|w| w.for_sweep(filter)).any(|w| w.test(p[0], p[2]).depth > -SWEEP_MARGIN) {
                p = [p[0] + step[0], p[1] + step[1], p[2] + step[2]];
            }
            t += 1.0;
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) {
        for k in 0..3 {
            assert!((a[k] - b[k]).abs() < 2e-3, "{a:?} != {b:?}");
        }
    }

    // The expected values below come from running the exe's own functions (`0x419e70`, `0x41a140`, `0x41a870`,
    // `0x465120`) under an emulator on the same inputs.

    #[test]
    fn the_ellipse_measures_inside_points_to_its_tip_and_outside_ones_along_their_direction() {
        let w = Walls::pitch(225.0, 85.0).list[6];
        let c = w.test(100.0, 0.0);
        assert!((c.depth + 12.5).abs() < 1e-9 && c.normal == [-1.0, 0.0]);
        assert!((w.test(0.0, 40.0).depth + 119.3995).abs() < 1e-3);
        let c = w.test(80.0, 35.0);
        assert!((c.depth - 7.0673).abs() < 1e-3);
        assert!((c.normal[0] + 0.3101).abs() < 1e-3 && (c.normal[1] + 0.9507).abs() < 1e-3);
        let c = w.test(-120.0, 5.0);
        assert!((c.depth - 8.1852).abs() < 1e-3 && (c.normal[0] - 0.9599).abs() < 1e-3);
        let obstacle = Wall {
            shape: Shape::Ellipse { centre: [35.0, -90.0], size: [94.0, 94.0], outside: true },
            kind: 0,
            index: None,
        };
        let c = obstacle.test(35.0, -90.0);
        assert!((c.depth - 47.0).abs() < 1e-9 && c.normal == [1.0, 0.0]);
        let c = obstacle.test(35.0, -40.0);
        assert!((c.depth + 3.0).abs() < 1e-9 && c.normal == [0.0, 1.0]);
    }

    #[test]
    fn the_sweep_leaves_the_target_inside() {
        let w = Walls::pitch(225.0, 85.0);
        close(w.sweep([0.0, 10.0, 0.0], [50.0, 0.0, 0.0], 0), [50.0, 10.0, 0.0]);
        close(w.sweep([80.0, 10.0, 0.0], [30.0, 0.0, 0.0], 0), [90.0, 10.0, 0.0]);
        close(w.sweep([80.0, 10.0, 0.0], [30.0, 0.0, 0.0], 1), [110.0, 10.0, 0.0]);
        close(w.sweep([0.0, 10.0, 30.0], [0.0, 0.0, 30.0], 0), [0.0, 10.0, 42.0]);
        close(w.sweep([-80.0, 10.0, 20.0], [-30.0, 0.0, 20.0], 0), [-88.3668, 10.0, 25.5778]);
    }

    #[test]
    fn the_commit_slows_and_stops_a_mover_at_the_walls() {
        let w = Walls::pitch(225.0, 85.0);
        let cases: [(i32, usize, Vec3, Vec3, Vec3, Vec3, Vec3); 9] = [
            (0, 0, [100.0, 10.0, 0.0], [100.5, 10.0, 0.0], [12.0, 2.0, 0.0], [92.5, 10.0, 0.0], [0.0, 1.0, 0.0]),
            (0, 0, [85.0, 10.0, 0.0], [85.5, 10.0, 0.0], [12.0, 2.0, 0.0], [85.455, 10.0, 0.0], [10.92, 1.0, 0.0]),
            (0, 0, [91.0, 10.0, 5.0], [93.0, 10.0, 6.0], [48.0, 4.0, 24.0], [91.0, 10.0, 6.0], [0.0, 2.0, 24.0]),
            (
                0,
                0,
                [0.0, 10.0, 41.0],
                [0.5, 10.0, 43.0],
                [12.0, 0.0, 48.0],
                [0.4967, 10.0, 40.9992],
                [11.9203, 0.0, -0.0198],
            ),
            (0, 0, [0.0, 10.0, 30.0], [0.5, 10.0, 31.0], [12.0, 0.0, 24.0], [0.5, 10.0, 31.0], [12.0, 0.0, 24.0]),
            (1, 0, [90.0, 10.0, 3.0], [94.0, 10.0, 3.0], [96.0, 0.0, 0.0], [90.0, 10.0, 3.0], [0.0, 0.0, 0.0]),
            (1, 1, [90.0, 10.0, 3.0], [94.0, 10.0, 3.0], [96.0, 0.0, 0.0], [-86.5, 10.0, 3.0], [0.0, 0.0, 0.0]),
            (
                2,
                0,
                [110.0, 10.0, 10.0],
                [112.0, 10.0, 12.0],
                [48.0, 6.0, 48.0],
                [109.2655, 10.0, 9.9927],
                [-5.7445, 3.0, 7.6518],
            ),
            (0, 0, [108.0, 10.0, 2.0], [109.0, 10.0, 2.0], [24.0, 3.0, 0.0], [92.5, 10.0, 2.0], [0.0, 1.5, 0.0]),
        ];
        for (filter, team, old, new, vel, pos, v) in cases {
            let (p, nv) = w.commit(filter, team, old, new, vel);
            close(p, pos);
            close(nv, v);
        }
    }

    #[test]
    fn a_script_clears_and_replaces_the_walls() {
        let mut c = Console::new();
        c.run_script(
            b"Pitch.Collision.Clear\nPitch.Collision.Line 0 0.05 -1.00 0.00 35.0\nPitch.Collision.Ellipse2 1 35 -90 94 94\n\
              Pitch.Collision.Line 0 1 0 -85 0\nPitch.Collision.Line 11 1 0 0 0\n",
        );
        let w = Walls::from_console(&c, 225.0, 85.0);
        assert_eq!(w.list.len(), 2);
        assert_eq!(w.list[0].index, Some(0));
        assert_eq!(w.list[0].shape, Shape::Line { point: [-85.0, 0.0], normal: [1.0, 0.0] });
        assert!(matches!(w.list[1].shape, Shape::Ellipse { outside: true, .. }));
        assert_eq!(Walls::from_console(&Console::new(), 225.0, 85.0), Walls::pitch(225.0, 85.0));
    }
}
