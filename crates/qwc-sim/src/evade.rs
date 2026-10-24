//! The evade state 0x14 (`0x457090`, enter `0x457070`): how an AI chaser carries the Quaffle. It is the
//! possession state's fallback sub-behaviour (brain `+0x1f8`, vtable `0x5bb15c`), so it runs on every tick a
//! chaser holds the ball and is not passing. It turns the opposing active player's position into a stick: run at
//! the attacked hoops, steer round a defender ahead, loop away from one close by, weave past one in front, and,
//! with the power bar, dodge. Traced 2026-10-19 (`docs/GAMEPLAY.md`, "The carrier: evade").
//!
//! `rand()` order per tick when no move runs (each draw only when its branch is reached): defender within 3:
//! `SpinProb`, then (defender behind and the carrier within half a goal line of the centre) the dodge roll;
//! defender within 15: `WeaveProb`. A loop past its fourth leg draws two (`0.1`, `DodgeProb`); a weave draws two
//! every tick (`0.1`, `DodgeProb`).

use crate::fmath::sin;
use crate::rng::Rand;
use crate::vec3::{add, dot, len, scale, sub, unit, Vec3};

/// Distances (`0x606048`, `0x60604c`), the weave period and amplitude (`0x606050`, `0x606054`), the loop's leg
/// lengths (`0x606058`), the head-for-goal time (`0x60606c`) and the look-ahead (`0x668e1c`).
pub const CLOSE: f64 = 3.0;
pub const NEAR: f64 = 15.0;
pub const WEAVE_TICKS: i32 = 24;
pub const WEAVE_AMPLITUDE: f64 = 1.5;
pub const LOOP_LEGS: [i32; 5] = [6, 12, 18, 24, 48];
pub const SPIN_TICKS: i32 = 12;
pub const LOOK_AHEAD: f64 = 0.5;
/// A loop leg ends when the defender is within 3 (`0x668e18`) and within 45 degrees (`0x5bf394`) of the stick.
pub const LEG_RANGE: f64 = 3.0;
pub const LEG_COS: f64 = 0.707;
/// The dodge rolls' base chances (`0x60601c`, `0x606024`), not console variables.
pub const LOOP_DODGE: f64 = 0.1;
pub const WEAVE_DODGE: f64 = 0.1;

/// `Evading.*` for the carrier's team.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Probs {
    pub spin: f64,
    pub weave: f64,
    pub dodge: f64,
    /// `Evading.DodgeRollProb`, `DodgeLoopProb` (team values, `0x60602c`, `0x606034`): the dodge's kind
    /// ([`dodge_kind`]); `DodgeBoostProb` is only the rest.
    pub roll: f64,
    pub loop_: f64,
}

/// `0x458110`: the kind of dodge (`0x436790`) the evade's dodge makes from one draw `u` (0 to 1): the roll to the
/// right (6) under half the roll's weight, to the left (7) under all of it, the loop (8) under it and the loop's,
/// else the boost (9).
pub fn dodge_kind(u: f64, p: &Probs) -> u8 {
    if u < p.roll * 0.5 {
        6
    } else if u < p.roll {
        7
    } else if u < p.roll + p.loop_ {
        8
    } else {
        9
    }
}

/// What the state reads each tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// The carrier's team's attack direction (-1 or 1).
    pub attack_dir: f64,
    pub pos: Vec3,
    pub vel: Vec3,
    pub fwd: Vec3,
    /// The opposing team's active player.
    pub opponent_pos: Vec3,
    pub opponent_vel: Vec3,
    /// `0x455d50`: outside the central ellipse or in the goal area.
    pub outside_centre: bool,
    /// `97.5 * ScaleGoals`.
    pub goal_line: f64,
    /// `0x436a30`: dodges allowed (the option byte `0x1018f82` and the power bar at `DodgePot`). Without the power
    /// bar this is false.
    pub can_dodge: bool,
}

/// The modes of `+0x2c`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    None,
    Loop,
    Weave,
    Dodge,
    Spin,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Evade {
    pub mode: Mode,
    prev: Mode,
    /// `+0x14`, `+0x18`, `+0x1c`, `+0x20`.
    counter: i32,
    sign: f64,
    behind: bool,
    leg: usize,
}

impl Evade {
    /// `0x457070`.
    pub fn enter(&mut self) {
        self.counter = 0;
        self.mode = Mode::None;
        self.prev = Mode::None;
        self.sign = 1.0;
    }

    /// One tick of `0x457090` up to the movement: the stick, and the kind of dodge asked for (the dodge mode's
    /// `0x458110` by [`dodge_kind`], the loop's and the weave's `0x458250` always the boost, 9).
    pub fn update(&mut self, rng: &mut Rand, probs: &Probs, v: &View) -> (Vec3, Option<u8>) {
        let a = v.attack_dir;
        let mut stick = [a, 0.0, 0.0];
        let d = sub(v.opponent_pos, v.pos);
        let mut resume = false;
        let mut spin = false;
        let mut dodge = None;
        if self.mode == Mode::None {
            self.behind = false;
            self.sign = 1.0;
            let dist = len(d);
            if dist < CLOSE {
                if rng.unit() >= probs.spin {
                    spin = true;
                } else if a * d[0] <= -1.0 {
                    // a defender behind: dodge if the bar allows, else note it (`+0x1c`)
                    let far = len(v.pos) > v.goal_line * 0.5;
                    if far || rng.unit() > probs.dodge || !v.can_dodge {
                        self.behind = true;
                    } else {
                        self.mode = Mode::Dodge;
                    }
                } else {
                    self.mode = Mode::Loop;
                    self.counter = 0;
                    self.leg = 0;
                    if d[2] > 0.0 {
                        self.sign = -1.0;
                    }
                }
            }
            if dist < NEAR && self.mode == Mode::None {
                if rng.unit() >= probs.weave {
                    spin = true;
                } else if self.weave_pays(v) {
                    self.mode = Mode::Weave;
                }
            }
            if self.mode == Mode::None {
                if spin {
                    self.mode = Mode::Spin;
                    self.counter = 0;
                }
                if self.mode == Mode::None && self.prev != Mode::None {
                    self.mode = self.prev;
                    resume = true;
                }
            }
        }
        match self.mode {
            Mode::Loop => stick = self.lop(rng, probs, v, d, &mut dodge),
            Mode::Weave => stick = self.weave(rng, probs, v, &mut dodge),
            Mode::Dodge => {
                // `0x458110`: along the facing; its move's kind (`0x436790`) from one `rand()`
                stick = unit([v.fwd[0], 0.0, v.fwd[2]]);
                dodge = Some(dodge_kind(rng.unit(), probs));
                self.mode = Mode::None;
            }
            Mode::Spin => {
                stick = to_goal(v);
                self.counter += 1;
                if self.counter == SPIN_TICKS {
                    self.mode = Mode::None;
                }
            }
            Mode::None => {
                stick = if a * d[0] > 0.0 { avoid(stick, v) } else { to_goal(v) };
            }
        }
        if resume {
            self.mode = Mode::None;
        }
        self.prev = self.mode;
        (stick, dodge)
    }

    /// The weave test: half a second ahead, flying straight brings the carrier closer to the defender than
    /// veering to one side. Also sets where the weave starts in its period.
    fn weave_pays(&mut self, v: &View) -> bool {
        let a = v.attack_dir;
        let speed = len(v.vel);
        let fx = if self.behind { 0.0 } else { a };
        let opp = add(v.opponent_pos, scale(v.opponent_vel, LOOK_AHEAD));
        let at = |dx: f64, dz: f64| len(sub(add(v.pos, [dx * speed * LOOK_AHEAD, 0.0, dz * speed * LOOK_AHEAD]), opp));
        let straight = at(a, 0.0);
        let left = at(fx, 1.0);
        let right = at(fx, -1.0);
        if !(straight < left || straight < right) {
            return false;
        }
        self.counter = if !v.outside_centre {
            if right <= left {
                0
            } else {
                WEAVE_TICKS / 2
            }
        } else if v.pos[2] <= 0.0 {
            0
        } else {
            WEAVE_TICKS / 2
        };
        true
    }

    /// `0x4578d0`: a loop in legs (ahead along the facing, diagonally ahead, across, back, across the other way), each
    /// ending when the defender is close in front.
    fn lop(&mut self, rng: &mut Rand, p: &Probs, v: &View, d: Vec3, dodge: &mut Option<u8>) -> Vec3 {
        let a = v.attack_dir;
        let mut stick = [a, 0.0, 0.0];
        if self.leg < LOOP_LEGS.len() && self.counter > LOOP_LEGS[self.leg] {
            self.leg += 1;
        }
        if self.leg == 5 {
            self.mode = Mode::None;
        }
        let near_front =
            |s: Vec3| d[0] * s[0] + d[2] * s[2] > LEG_COS && (d[0] * d[0] + d[2] * d[2]).sqrt() < LEG_RANGE;
        if self.leg == 4 {
            stick = unit([v.fwd[0], 0.0, v.fwd[2]]);
            if near_front(stick) {
                self.leg = 3;
            } else {
                let mut ok = self.counter == LOOP_LEGS[3] + SPIN_TICKS && v.can_dodge;
                if rng.unit() > LOOP_DODGE {
                    ok = false;
                }
                if rng.unit() > p.dodge {
                    ok = false;
                }
                if ok {
                    *dodge = Some(9);
                }
            }
        }
        if self.leg == 3 {
            stick = unit([a, 0.0, -a * self.sign]);
            if near_front(stick) {
                self.leg = 2;
                self.sign = -self.sign;
            }
        }
        if self.leg == 2 {
            stick = unit([0.0, 0.0, -a * self.sign]);
            if near_front(stick) {
                self.leg = 1;
            }
        }
        if self.leg == 1 {
            stick = [-a, 0.0, 0.0];
            if near_front(stick) {
                self.leg = 0;
            }
        }
        if self.leg == 0 {
            stick = unit([0.0, 0.0, a * self.sign]);
        }
        self.counter += 1;
        stick
    }

    /// `0x457e40`: half a sine period sideways while flying on.
    fn weave(&mut self, rng: &mut Rand, p: &Probs, v: &View, dodge: &mut Option<u8>) -> Vec3 {
        let x = if self.behind { 0.0 } else { v.attack_dir };
        let phase = self.counter as f64 * std::f64::consts::TAU / WEAVE_TICKS as f64;
        let mut stick = [x, 0.0, WEAVE_AMPLITUDE * sin(phase)];
        if stick[2] * stick[2] + x * x > 0.001 {
            stick = unit(stick);
        }
        let mut ok = (self.counter == 8 || self.counter == WEAVE_TICKS / 2 + 8) && v.can_dodge;
        if rng.unit() > WEAVE_DODGE {
            ok = false;
        }
        if rng.unit() > p.dodge {
            ok = false;
        }
        if ok {
            *dodge = Some(9);
        }
        self.counter += 1;
        if self.counter == WEAVE_TICKS / 2 || self.counter == WEAVE_TICKS {
            self.mode = Mode::None;
        }
        stick
    }
}

/// `0x458030`: straight at the middle of the attacked hoops' line, in `x, z`.
pub fn to_goal(v: &View) -> Vec3 {
    unit([v.attack_dir * v.goal_line - v.pos[0], 0.0, -v.pos[2]])
}

/// `0x4582f0`: with the defender ahead and contact less than a second away, half of the stick turns sideways, away
/// from the way the defender is closing.
pub fn avoid(stick: Vec3, v: &View) -> Vec3 {
    let r = sub(v.opponent_pos, v.pos);
    let rv = sub(v.opponent_vel, v.vel);
    let closing = -dot(unit(r), unit(rv)) * len(rv);
    let dist = len(r);
    let mut t = 10.0;
    if dist > 1.5 {
        t = dist - 1.5;
        if closing > 1.0 {
            t /= closing;
        }
    }
    if t >= 1.0 {
        return stick;
    }
    let mut side = unit([r[2], 0.0, -r[0]]);
    if dot(side, unit(rv)) > 0.0 {
        side = scale(side, -1.0);
    }
    add(scale(side, 0.5), scale(stick, 0.5))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(opp: Vec3) -> View {
        View {
            attack_dir: -1.0,
            pos: [0.0; 3],
            vel: [-10.0, 0.0, 0.0],
            fwd: [-1.0, 0.0, 0.0],
            opponent_pos: opp,
            opponent_vel: [0.0; 3],
            outside_centre: false,
            goal_line: 97.5,
            can_dodge: false,
        }
    }

    const PROBS: Probs = Probs { spin: 1.0, weave: 1.0, dodge: 0.95, roll: 0.33, loop_: 0.33 };

    #[test]
    fn far_from_everyone_heads_for_the_hoops() {
        let mut e = Evade::default();
        e.enter();
        let mut r = Rand::new(1);
        // the defender is behind (team 0 attacks -x): head for the goal, no draw
        let (s, _) = e.update(&mut r, &PROBS, &view([30.0, 0.0, 0.0]));
        assert_eq!(r, Rand::new(1));
        assert!(len(sub(s, [-1.0, 0.0, 0.0])) < 1e-12);
        let mut v = view([30.0, 0.0, 0.0]);
        v.pos = [0.0, 0.0, 97.5];
        let (s, _) = e.update(&mut r, &PROBS, &v);
        assert!(len(sub(s, unit([-97.5, 0.0, -97.5]))) < 1e-12);
    }

    #[test]
    fn a_defender_ahead_starts_a_weave_or_a_spin() {
        let mut e = Evade::default();
        e.enter();
        // ahead at 10: one WeaveProb draw (rand 41 < 1.0 passes), then flying straight is closer: weave
        let mut r = Rand::new(1);
        let (s, _) = e.update(&mut r, &PROBS, &view([-10.0, 0.0, 0.0]));
        assert_eq!(e.mode, Mode::Weave);
        // first weave tick: sin(0) = 0 sideways; two dodge rolls
        assert!(len(sub(s, [-1.0, 0.0, 0.0])) < 1e-12);
        let mut check = Rand::new(1);
        for _ in 0..3 {
            check.rand();
        }
        assert_eq!(r, check);
        // the weave runs 12 ticks, then the evade asks again
        for _ in 0..11 {
            e.update(&mut r, &PROBS, &view([-10.0, 0.0, 0.0]));
        }
        assert_eq!(e.mode, Mode::None);
        // with WeaveProb 0 the draw always fails: spin (head for the goal for 12 ticks)
        let mut e = Evade::default();
        e.enter();
        let p = Probs { weave: 0.0, ..PROBS };
        e.update(&mut Rand::new(1), &p, &view([-10.0, 0.0, 0.0]));
        assert_eq!(e.mode, Mode::Spin);
    }

    #[test]
    fn a_close_defender_in_front_starts_a_loop() {
        let mut e = Evade::default();
        e.enter();
        let mut r = Rand::new(1);
        // within 3, ahead, on the +z side: a loop turning the other way
        let (s, _) = e.update(&mut r, &PROBS, &view([-2.0, 0.0, 0.5]));
        assert_eq!(e.mode, Mode::Loop);
        // leg 0: across, `a * sign` = (-1) * (-1)
        assert!(len(sub(s, [0.0, 0.0, 1.0])) < 1e-12);
    }

    #[test]
    fn avoidance() {
        // a defender 2 units ahead closing at 10: contact in 0.05 s, half the stick turns sideways
        let mut v = view([-2.0, 0.0, 0.0]);
        v.vel = [0.0; 3];
        v.opponent_vel = [10.0, 0.0, 0.0];
        let s = avoid([-1.0, 0.0, 0.0], &v);
        assert!((s[0] + 0.5).abs() < 1e-12 && (s[2].abs() - 0.5).abs() < 1e-12);
        // far away: unchanged
        assert_eq!(avoid([-1.0, 0.0, 0.0], &view([-40.0, 0.0, 0.0])), [-1.0, 0.0, 0.0]);
    }
}
