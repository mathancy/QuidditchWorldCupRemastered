//! The pitch's heights (`docs/GAMEPLAY.md` "Cruising height", `RE_NOTES.md` `heightmap.csv`): the floor the flyers
//! keep above, from the stadium's height map (`0x41a570`); the end-zone clamp that brings them down to the goal
//! height near the hoops (`0x41a2a0`); and the cruising height the carriers seek, the floor plus a wave of up to
//! `HeightGraphMaxHeight` whose phase the carriers' turns kick along (the height graph, `0x4206c0`, `0x420840`,
//! `0x42fba0`; `0x41a610`, `0x432a80`). **[T]** unless marked.

use crate::console::Console;
use crate::fmath::{self, cos};
use crate::vec3::Vec3;

/// A height map's rows (along the pitch) and columns (across it).
pub const ROWS: usize = 20;
pub const COLS: usize = 10;

/// One of a stadium's three height maps (`heightmap.csv`, `qwc_formats::heightmap`).
pub type Map = [[f64; COLS]; ROWS];

/// A command's last numeric argument, or `default` when no script gave it (the pitch's constructor values).
fn command(c: &Console, path: &str, default: f64) -> f64 {
    c.args(path, 0).iter().rev().find_map(|v| v.num()).unwrap_or(default)
}

/// The pitch fields the heights read (the world object `0x655ff0`).
#[derive(Debug, Clone, PartialEq)]
pub struct Pitch {
    /// `Pitch.GoalHeight` (`+0x12604`, 20 until a script sets it): the hoops' and the floor's base.
    pub goal_height: f64,
    /// `Pitch.HeightMapHeight` (`+0x1260c`, 15): how far a map's 1 is from its 0.
    pub height_map_height: f64,
    /// `Pitch.ScaleGoals` (`[0x6071bc]`, 1).
    pub scale_goals: f64,
    /// `HeightGraph.HeightGraphMaxHeight` (5): the wave's height above the floor.
    pub max_height: f64,
    /// Challenge mode 4 (`[0x1018f94]` with `[0x1018f98] == 4`): the long pitch (146.25 by 157.5), no clamp.
    pub mode4: bool,
    /// The maps (the table at `0x606858`) and the one in use (`+0x12608`, `Pitch.DefaultHeightMap`).
    pub maps: Vec<Map>,
    pub map: usize,
}

impl Pitch {
    /// The console's pitch values (load the stadium's `game/match.cli` first) with `maps` (a stadium's
    /// `heightmap.csv`; flat maps of 1 without one).
    pub fn new(c: &Console, maps: Vec<Map>) -> Pitch {
        let maps = if maps.is_empty() { vec![[[1.0; COLS]; ROWS]; 3] } else { maps };
        let map = (command(c, "Pitch.DefaultHeightMap", 0.0).max(0.0) as usize).min(maps.len() - 1);
        Pitch {
            goal_height: command(c, "Pitch.GoalHeight", 20.0),
            height_map_height: command(c, "Pitch.HeightMapHeight", 15.0),
            scale_goals: command(c, "Pitch.ScaleGoals", 1.0),
            max_height: c.num("HeightGraph.HeightGraphMaxHeight", 0),
            mode4: false,
            maps,
            map,
        }
    }

    /// Half the pitch's length and its width (`0x41a440`).
    fn size(&self) -> (f64, f64) {
        if self.mode4 {
            (146.25, 157.5)
        } else {
            (97.5 * self.scale_goals, 105.0 * self.scale_goals)
        }
    }

    /// The map cell under `p` and the fractions into it (`0x41a440`): the row along the length, the column across,
    /// the row clamped to 0-18 and the column to 0-8 (the fractions are not, so past the ends the map extrapolates).
    fn cell(&self, p: Vec3) -> (usize, usize, f64, f64) {
        let (l, w) = self.size();
        let fr = (p[0] + l) / (2.0 * l) * ROWS as f64;
        let fc = (p[2] + w * 0.5) / w * COLS as f64;
        let (r, c) = (fr as i32, fc as i32);
        let r = r.clamp(0, ROWS as i32 - 2) as usize;
        let c = if c <= 0 { 0 } else { c.min(COLS as i32 - 2) as usize };
        (r, c, fr - r as f64, fc - c as f64)
    }

    /// The floor under `p` (`0x41a570`): `GoalHeight + HeightMapHeight * (h - 1)`, `h` the map between its four
    /// corners.
    pub fn floor(&self, p: Vec3) -> f64 {
        let m = &self.maps[self.map];
        let (r, c, a, b) = self.cell(p);
        let r0 = m[r][c] + (m[r + 1][c] - m[r][c]) * a;
        let r1 = m[r][c + 1] + (m[r + 1][c + 1] - m[r][c + 1]) * a;
        let h = r0 + (r1 - r0) * b;
        self.goal_height - self.height_map_height + h * self.height_map_height
    }

    /// The end-zone clamp of `p`'s height (`0x41a2a0`), with `d` the distance in from 15 short of the end:
    /// unchanged 70 in or more; the goal height at or past it; between, at least the floor and at most a ceiling
    /// that eases from the floor plus the wave's height down to the goal height.
    pub fn clamp(&self, p: Vec3) -> f64 {
        if self.mode4 {
            return p[1];
        }
        let (l, _) = self.size();
        let d = (l - 15.0) - p[0].abs();
        if d > 70.0 {
            return p[1];
        }
        if d <= 0.0 {
            return self.goal_height;
        }
        let c = cos((1.0 - d / 70.0) * std::f64::consts::FRAC_PI_2);
        let ceiling = (self.floor(p) - self.goal_height + self.max_height) * c;
        if ceiling < p[1] - self.goal_height {
            return ceiling + self.goal_height;
        }
        self.floor(p).max(p[1])
    }

    /// `p` with its height at the floor, clamped (`0x41a610`'s first term).
    fn clamped_floor(&self, p: Vec3) -> f64 {
        self.clamp([p[0], self.floor(p), p[2]])
    }

    /// The wave above the floor at `p` (`0x432a80`): the clamped span from the floor to the floor plus the wave's
    /// height, times `(1 + T) / 2`.
    pub fn offset(&self, graph: &HeightGraph, p: Vec3) -> f64 {
        let f = self.floor(p);
        let top = self.clamp([p[0], f + self.max_height, p[2]]);
        (top - self.clamped_floor(p)) * 0.5 * (1.0 + graph.wave())
    }

    /// The cruising height at `p` (`0x41a610`): the clamped floor plus the wave.
    pub fn cruise(&self, graph: &HeightGraph, p: Vec3) -> f64 {
        self.clamped_floor(p) + self.offset(graph, p)
    }
}

/// The height graph (the game's `+0x3d8`): the wave's phase and its speed, the kick records, and the wave's table.
#[derive(Debug, Clone, PartialEq)]
pub struct HeightGraph {
    pub phase: f64,
    pub vel: f64,
    /// `HeightGraph.ChartMaxVel` (0.08) and `ChartDampVel` (0.1).
    pub max_vel: f64,
    pub damp: f64,
    /// The last `HeightGraph.NumKickRecords` kicks (5, `0x42fcd7`) and where the next goes.
    kicks: Vec<f64>,
    next: usize,
    /// `T[i] = 2 ((1 + sin(f1 s) sin(f2 s)) / 2)^2 - 1` at `s = i / 256` (`0x4206c0`; `HeightGraph.Sin`, `2π` and
    /// `8.79645`).
    table: Vec<f64>,
}

impl HeightGraph {
    pub fn new(c: &Console) -> HeightGraph {
        let sin = c.args("HeightGraph.Sin", 0);
        let f = |k: usize, d: f64| sin.get(k).and_then(|v| v.num()).unwrap_or(d);
        // the exe's constants (`0x60683c`, `0x606840`), a little short of 2π and 2.8π
        #[allow(clippy::approx_constant)]
        let (f1, f2) = (f(0, 6.28318), f(1, 8.796452));
        let table = (0..256)
            .map(|i| {
                let s = i as f64 / 256.0;
                let h = (1.0 + fmath::sin(f1 * s) * fmath::sin(f2 * s)) * 0.5;
                2.0 * h * h - 1.0
            })
            .collect();
        HeightGraph {
            phase: 0.0,
            vel: 0.0,
            max_vel: c.num("HeightGraph.ChartMaxVel", 0),
            damp: c.num("HeightGraph.ChartDampVel", 0),
            kicks: vec![0.0; 5],
            next: 0,
            table,
        }
    }

    /// `T` at the phase, between its neighbours (`0x432b04`).
    pub fn wave(&self) -> f64 {
        let f = self.phase * 256.0;
        let i = f.trunc() as i64;
        let a = f - i as f64;
        let i = if i == 256 { 0 } else { i.clamp(0, 255) as usize };
        let j = (i + 1) % 256;
        (1.0 - a) * self.table[i] + a * self.table[j]
    }

    /// One tick (`0x420840`): the speed clamped and damped, the phase moved on and wrapped.
    pub fn tick(&mut self) {
        self.vel = self.vel.clamp(-self.max_vel, self.max_vel) * self.damp;
        self.phase += self.vel;
        self.phase -= self.phase.floor();
    }

    /// A kick (`0x42fba0`): recorded, and added to the speed when above the records' average.
    pub fn kick(&mut self, k: f64) {
        let n = self.kicks.len();
        self.kicks[self.next] = k;
        self.next = (self.next + 1) % n;
        let average = self.kicks.iter().sum::<f64>() / n as f64;
        if k > average {
            self.vel += k;
        }
    }
}

/// A carrier's kick (`0x4362b0`): the previous tick's horizontal speed, signed by the turn from it to this tick's
/// velocity, when both the change and the speed reach 0.1 (squared); times `scale` (`HeightGraph.ScaleJoyKick`,
/// and for an AI team `ScaleAIKick` too).
pub fn carrier_kick(vel: Vec3, prev: Vec3, scale: f64) -> f64 {
    let (dx, dz) = (vel[0] - prev[0], vel[2] - prev[2]);
    let mut k = 0.0;
    if dx * dx + dz * dz >= 0.1 && vel[0] * vel[0] + vel[2] * vel[2] >= 0.1 {
        k = (prev[0] * prev[0] + prev[2] * prev[2]).sqrt();
    }
    if dz * vel[0] - vel[2] * dx <= 0.0 {
        k = -k;
    }
    k * scale
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat() -> Pitch {
        let mut c = Console::new();
        c.run_script(b"Pitch.GoalHeight 20\nPitch.HeightMapHeight 11\n");
        Pitch::new(&c, Vec::new())
    }

    #[test]
    fn floor_from_the_map() {
        let mut p = flat();
        assert_eq!(p.floor([0.0, 0.0, 0.0]), 20.0);
        // a map of 2 in one corner cell: the floor rises by the map height there
        let mut m = [[1.0; COLS]; ROWS];
        m[10][5] = 2.0;
        p.maps = vec![m; 3];
        // row 10 starts at x = 0, column 5 at z = 0
        assert!((p.floor([0.0, 0.0, 0.0]) - 31.0).abs() < 1e-9);
        // halfway to the next row and column: a quarter of the way up
        let (dx, dz) = (97.5 / 20.0, 105.0 / 20.0);
        assert!((p.floor([dx, 0.0, dz]) - (20.0 + 11.0 * 0.25)).abs() < 1e-9);
    }

    #[test]
    fn the_end_zones_bring_flyers_to_goal_height() {
        let p = flat();
        // far from the ends: unchanged; past 82.5: the goal height
        assert_eq!(p.clamp([0.0, 40.0, 0.0]), 40.0);
        assert_eq!(p.clamp([90.0, 40.0, 0.0]), 20.0);
        // between: under the floor raised to it; above the eased ceiling lowered to it
        assert_eq!(p.clamp([50.0, 0.0, 0.0]), 20.0);
        let d: f64 = 82.5 - 50.0;
        let ceiling = 20.0 + 5.0 * cos((1.0 - d / 70.0) * std::f64::consts::FRAC_PI_2);
        assert!((p.clamp([50.0, 40.0, 0.0]) - ceiling).abs() < 1e-9);
    }

    #[test]
    fn the_wave_rides_on_the_floor() {
        let c = Console::new();
        let p = flat();
        let mut g = HeightGraph::new(&c);
        // T(0) = 2 (1/2)^2 - 1 = -1/2: a quarter of the wave's height up
        assert!((p.cruise(&g, [0.0, 0.0, 0.0]) - 21.25).abs() < 1e-9);
        // a kick above the average speeds the phase; the damping takes it back down
        g.kick(1.0);
        assert_eq!(g.vel, 1.0);
        g.tick();
        assert!((g.vel - 0.008).abs() < 1e-12 && (g.phase - 0.008).abs() < 1e-12);
        g.kick(0.0);
        assert_eq!(g.vel, 0.008);
        // near the hoops the wave has no room
        assert!((p.cruise(&g, [90.0, 0.0, 0.0]) - 20.0).abs() < 1e-9);
    }

    #[test]
    fn kicks_follow_the_turn() {
        // flying +x at 10, now turning towards +z: a positive kick of the old speed, scaled
        assert!((carrier_kick([10.0, 0.0, 2.0], [10.0, 0.0, 0.0], 0.25) - 2.5).abs() < 1e-9);
        assert!((carrier_kick([10.0, 0.0, -2.0], [10.0, 0.0, 0.0], 0.025) + 0.25).abs() < 1e-9);
        assert_eq!(carrier_kick([10.0, 0.0, 0.1], [10.0, 0.0, 0.0], 0.25), 0.0);
    }
}
