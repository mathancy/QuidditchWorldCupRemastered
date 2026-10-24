//! The Seeker game (`docs/GAMEPLAY.md`, "The Snitch and the Seeker game"): the catch test and the Snitch's motion
//! along its stadium spline.

use crate::console::Console;
use crate::passing::TICKS;
use crate::spline::Spline;
use crate::vec3::{add, cross, dot, len, scale, sub, unit, Vec3};

/// The limit on the catch test's first argument (`0x60616c`; what that argument measures is not traced).
pub const GRAB_LIMIT: f64 = 3.5;

/// The `SeekerGame.*` values the catch test and the Snitch motion read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeekerRules {
    pub can_catch: bool,
    pub ai_can_catch: bool,
    pub max_grab_distance: f32,
    pub max_grab_lateral_offset: f32,
    pub snitch_speed_modifier: f32,
    pub spline_base_speed: f32,
    pub spline_base_scale: f32,
}

impl SeekerRules {
    pub fn from_console(c: &Console) -> SeekerRules {
        let f = |k: &str| c.f32(&format!("SeekerGame.{k}"), 0);
        SeekerRules {
            can_catch: c.flag("SeekerGame.CanCatchSnitch"),
            ai_can_catch: c.flag("SeekerGame.AI.CanCatchSnitch"),
            max_grab_distance: f("MaximumGrabSnitchDistance"),
            max_grab_lateral_offset: f("MaximumGrabSnitchLateralOffset"),
            snitch_speed_modifier: f("SnitchSpeedModifier"),
            spline_base_speed: f("Spline.SplineBaseSpeed"),
            spline_base_scale: f("Spline.SplineBaseScale"),
        }
    }

    /// The spline follower's parameter step the Snitch starts with (`0x44d360`): `SplineBaseSpeed + 0.005`.
    pub fn snitch_step(&self) -> f64 {
        self.spline_base_speed as f64 + 0.005
    }
}

/// A seeker at the moment it tries to grab.
#[derive(Debug, Clone, Copy)]
pub struct GrabAttempt {
    /// Controlled by the AI (`+0x8d`).
    pub ai: bool,
    /// The AI's grab-allowed flag (`+0x1d5c`), set when its `GrabSnitchSuccessRate` roll passed.
    pub ai_allowed: bool,
    /// The catch test's first argument (compared with [`GRAB_LIMIT`]).
    pub x: f64,
    pub seeker: [f64; 3],
    pub snitch: [f64; 3],
    /// `+0x1a0` of the object the caller passes (the lateral offset).
    pub lateral_offset: f64,
}

/// `can_grab` (`0x4481d0`): every condition must hold.
pub fn can_grab(r: &SeekerRules, g: &GrabAttempt) -> bool {
    let d = [g.snitch[0] - g.seeker[0], g.snitch[1] - g.seeker[1], g.snitch[2] - g.seeker[2]];
    let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    (!g.ai || (r.ai_can_catch && g.ai_allowed))
        && r.can_catch
        && g.x <= GRAB_LIMIT
        && dist < r.max_grab_distance as f64
        && g.lateral_offset.abs() <= r.max_grab_lateral_offset as f64
}

/// The Snitch's fixed steering constants (`0x6060f8`-`0x606108`, not console variables): how far ahead on the spline
/// it aims (points), how strongly its velocity and its facing turn towards that point, its thrust and its drag.
pub const LOOK_AHEAD: f64 = 2.5;
pub const TURN_VEL: f64 = 0.5;
pub const TURN_FACING: f64 = 0.5;
pub const THRUST: f64 = 1.0;
pub const DRAG: f64 = 0.97;
/// The pace adjustment's decay per tick (`0x5bf2f8`).
pub const PACE_DECAY: f64 = 0.9;
/// The smallest parameter step per tick (the race's call `0x44f060(0.0001, 0)`).
pub const MIN_STEP: f64 = 1e-4;
/// The level stretch the Snitch starts in (`0x4916f0(.., 10, 0.15, 0)`), and the seekers' starts behind it.
pub const START_RUN: f64 = 10.0;
pub const START_LEVEL: f64 = 0.15;
pub const SEEKER_START: [f64; 2] = [-10.0, -8.0];

/// The Snitch in the Seeker game (`docs/GAMEPLAY.md`, "Snitch movement"): object `+0x540` position, `+0x550`
/// velocity (units per second), `+0x560` facing, `+0x6dc` spline parameter; `pace` is the global `0xf91914`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Snitch {
    pub param: f64,
    pub pos: Vec3,
    pub vel: Vec3,
    pub facing: Vec3,
    pub pace: f64,
}

impl Snitch {
    /// The set-up `0x44d360` and the starting velocity `0x44f760`: at the start of the first level stretch, moving
    /// at 24 times the step to the point `SplineBaseSpeed + 0.005` ahead. The starting facing is not traced; the
    /// velocity's direction is used.
    pub fn start(spline: &Spline, r: &SeekerRules) -> Snitch {
        let param = spline.normalise(spline.level_start(START_RUN, START_LEVEL));
        let pos = spline.position(param);
        let vel = scale(sub(spline.position(param + r.snitch_step()), pos), TICKS);
        Snitch { param, pos, vel, facing: unit(vel), pace: 0.0 }
    }

    /// The seekers' spline parameters at the start (`0x44d360`): team 0 ten points behind, team 1 eight.
    pub fn seeker_starts(&self) -> [f64; 2] {
        SEEKER_START.map(|d| self.param + d)
    }

    /// The race's pace adjustment (`0x44f990`, Snitch mode 3, `Game.SeekerGameType` 0): from half the distances to
    /// the two seekers and the ticks since the race began. Early on it runs from a close seeker and waits for distant
    /// ones; after 80 seconds it slows whenever neither seeker is close; it always waits for a far-behind seeker.
    pub fn pace(&mut self, seekers: [Vec3; 2], ticks: i32) {
        let d = seekers.map(|s| 0.5 * len(sub(self.pos, s)));
        let (near, far) = (d[0].min(d[1]), d[0].max(d[1]));
        if ticks <= 1920 {
            if near > 10.0 {
                self.pace = -0.4 * THRUST;
            } else if far < 5.0 && ticks < 1440 {
                self.pace = 0.15 * THRUST;
            }
            if ticks < 720 && near < 6.0 {
                self.pace = 0.5 * THRUST;
            }
        } else if near > 1.5 {
            self.pace = -0.3 * THRUST;
        }
        if far > 20.0 {
            self.pace = -0.8 * THRUST;
        }
    }

    /// One tick: thrust and drag (`0x44f810`), the move and the parameter (`0x44f060`), and the steering towards the
    /// trail ahead (`0x44ed60`). The velocity damping of `0x44f060` multiplies by `1 - a * [0xf91910]`, a global
    /// with no writer, so it is left out.
    pub fn update(&mut self, spline: &Spline, r: &SeekerRules) {
        // 0x44f810: thrust along the facing, drag, then turn the velocity towards the facing
        self.vel = add(self.vel, scale(self.facing, self.pace + THRUST));
        self.pace *= PACE_DECAY;
        self.vel = scale(self.vel, DRAG);
        let speed = len(self.vel);
        self.vel = scale(unit(add(self.facing, self.vel)), speed);
        // 0x44f060: move, stay in the trail's surface at point i, and measure the progress along segment i
        let i = self.param.trunc();
        let (p0, p1) = spline.segment(i);
        let up = spline.frame(i).up;
        self.pos = add(self.pos, scale(self.vel, r.snitch_speed_modifier as f64 / TICKS));
        self.pos = sub(self.pos, scale(up, dot(up, sub(self.pos, p0))));
        let seg = sub(p1, p0);
        let t = unit(seg);
        let d = sub(self.pos, p0);
        let b = unit(cross(unit(cross(d, t)), t));
        let along = sub(d, scale(b, dot(b, d)));
        let next = i + along_fraction(along, seg);
        self.param = if next - self.param < MIN_STEP { self.param + MIN_STEP } else { next };
        // 0x44ed60: turn the velocity (keeping its speed) and the facing towards the trail ahead, within the surface
        let speed = len(self.vel);
        let aim = unit(sub(spline.position(self.param + LOOK_AHEAD), self.pos));
        self.vel = scale(unit(add(self.vel, scale(aim, speed * TURN_VEL))), speed);
        self.vel = sub(self.vel, scale(up, dot(up, self.vel)));
        let f = unit(add(self.facing, scale(aim, TURN_FACING)));
        self.facing = sub(f, scale(up, dot(up, f)));
    }
}

/// The fraction of `seg` that `along` (parallel to it) covers, read on `seg`'s largest component as `0x44f060` does.
fn along_fraction(along: Vec3, seg: Vec3) -> f64 {
    let a = seg.map(f64::abs);
    let k = if a[0] > a[1] && a[0] > a[2] {
        0
    } else if a[2] > a[1] {
        2
    } else {
        1
    };
    along[k] / seg[k]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fmath::{cos, sin};
    use crate::spline::Point;

    fn attempt() -> GrabAttempt {
        GrabAttempt {
            ai: false,
            ai_allowed: false,
            x: 1.0,
            seeker: [0.0; 3],
            snitch: [1.0, 0.0, 1.0],
            lateral_offset: 0.0,
        }
    }

    #[test]
    fn catch_test() {
        let r = SeekerRules::from_console(&Console::new());
        assert_eq!((r.max_grab_distance, r.max_grab_lateral_offset), (1.8, 99.0));
        assert!(can_grab(&r, &attempt()));
        assert!(!can_grab(&r, &GrabAttempt { snitch: [2.0, 0.0, 0.0], ..attempt() }));
        assert!(!can_grab(&r, &GrabAttempt { x: 3.6, ..attempt() }));
        assert!(!can_grab(&r, &GrabAttempt { ai: true, ..attempt() }));
        assert!(can_grab(&r, &GrabAttempt { ai: true, ai_allowed: true, ..attempt() }));
        assert!(!can_grab(&SeekerRules { can_catch: false, ..r }, &attempt()));
    }

    fn rules() -> SeekerRules {
        SeekerRules::from_console(&Console::new())
    }

    /// A circle of 40 points and radius 60, level, anticlockwise seen from above.
    fn circle() -> Spline {
        let pts = (0..40)
            .map(|i| {
                let a = i as f64 / 40.0 * std::f64::consts::TAU;
                // travel direction (-sin a, 0, cos a) is a yaw of -a about +y; stored inverted
                let h = -a / 2.0;
                Point { pos: [60.0 * cos(a), 5.0, 60.0 * sin(a)], rot: [0.0, -sin(h), 0.0, cos(h)] }
            })
            .collect();
        Spline::new(pts, 1.0)
    }

    #[test]
    fn snitch_starts_on_a_level_stretch() {
        let s = circle();
        let (r, n) = (rules(), Snitch::start(&s, &rules()));
        assert_eq!(n.param, 10.0);
        assert_eq!(n.pos, s.position(10.0));
        // 24 times the step to the point 0.405 ahead
        assert!((r.snitch_step() - 0.405).abs() < 1e-6);
        let v = scale(sub(s.position(10.0 + r.snitch_step()), s.position(10.0)), 24.0);
        assert!(len(sub(n.vel, v)) < 1e-12);
        assert_eq!(n.seeker_starts(), [0.0, 2.0]);
    }

    #[test]
    fn snitch_follows_the_trail_at_its_cruising_speed() {
        let (s, r) = (circle(), rules());
        let mut n = Snitch::start(&s, &r);
        let mut last = n.param;
        for _ in 0..24 * 60 {
            n.update(&s, &r);
            assert!(n.param > last);
            last = n.param;
            // in the trail's surface and near its line
            assert!((n.pos[1] - 5.0).abs() < 1e-9);
            let r = (n.pos[0] * n.pos[0] + n.pos[2] * n.pos[2]).sqrt();
            assert!((r - 60.0).abs() < 6.0, "{r}");
        }
        // thrust 1 against drag 0.97: about 0.97 / 0.03 = 32 units per second
        assert!((len(n.vel) - 32.3).abs() < 1.0, "{}", len(n.vel));
        // a lap is about 377 units: a minute covers about 5 laps (200 points)
        assert!((150.0..250.0).contains(&(last - 10.0)), "{last}");
    }

    #[test]
    fn pace_rules() {
        let mut n = Snitch::start(&circle(), &rules());
        let p = n.pos;
        let at = |d: f64| add(p, [2.0 * d, 0.0, 0.0]);
        let mut pace = |a: f64, b: f64, t: i32| {
            n.pace = 0.0;
            let s = [at(a), at(b)];
            n.pace(s, t);
            n.pace
        };
        assert_eq!(pace(11.0, 12.0, 100), -0.4);
        assert_eq!(pace(4.0, 4.5, 100), 0.5); // within 6 in the first 30 seconds
        assert_eq!(pace(5.5, 7.0, 1000), 0.0);
        assert_eq!(pace(4.0, 4.5, 1000), 0.15);
        assert_eq!(pace(4.0, 4.5, 1500), 0.0);
        assert_eq!(pace(2.0, 3.0, 2000), -0.3);
        assert_eq!(pace(1.0, 3.0, 2000), 0.0);
        assert_eq!(pace(1.0, 21.0, 100), -0.8);
    }
}
