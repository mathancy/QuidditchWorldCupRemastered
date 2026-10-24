//! Tackles (`docs/GAMEPLAY.md`, "Tackles"): the tackle branch of the broom handling `0x442ac0`, the lunge, the
//! penalty on the tackled team (`0x436a80`) and the AI defender's press (state 0x21, `0x453ac0`). No `rand()`.
//!
//! A tackle needs the tackle input, a victim (the Quaffle's holder), no stun on the tackler's team, the recharge
//! run out and no lunge in progress. Within 2.5 units it succeeds (unless the victim's team is dodging): the tackler
//! flies to where the victim will be in 10 ticks and takes the ball there. Farther away the tackler lunges (the
//! boost tackle), and the lunge ends on its victim: within 1.5 units, or when its time runs out, it tackles if the
//! victim was within 2.5 at the tick's start, and otherwise misses and stuns the tackler's own team for 12 ticks
//! (`0x443f27`-`0x4441fb`; checked under an emulator, `tools/check_handling.py`).

use crate::fmath::cos;
use crate::rng::trunc;
use crate::vec3::{add, dot, flat, len, scale, sub, unit, Vec3};

/// Tackle range (`0x60661c`) and the ticks a successful tackle takes (`0x606634`).
pub const RANGE: f64 = 2.5;
pub const TACKLE_TICKS: i32 = 10;
/// Lunge timing: `t = max(0.3, (d - 1.5) / (10 + w))` seconds (`0x606620`, `0x6066dc`, `0x606624`), then 0.1 s more
/// (`0x606628`).
pub const LUNGE_MIN: f64 = 0.3;
pub const LUNGE_REACH: f64 = 1.5;
pub const LUNGE_SPEED: f64 = 10.0;
pub const LUNGE_EXTRA: f64 = 0.1;
/// A lunge's tackle leads its victim by at least this many ticks (`0x606630`), and a miss stuns the tackler's team
/// for `MISS_STUN` ticks (`0x60663c`).
pub const LUNGE_LEAD: f64 = 8.0;
pub const MISS_STUN: i32 = 12;
/// Outside the lunge cone the lunge shrinks to `1 - 0.7 k`, `k = clamp(3 (cos - c), 0, 1)` (`0x668f34`, `0x668f3c`).
pub const CONE_SLOPE: f64 = 3.0;
pub const CONE_FLOOR: f64 = 0.3;
/// The tackled team's stun (`0x668fdc`) and the knock-back speed (`0x668fe0`).
pub const STUN_TICKS: i32 = 25;
pub const KNOCK_BACK: f64 = 12.0;
/// The recharge both teams get on a change of possession (`0x431c40`): `trunc(0.5 * 24)`.
pub const RECHARGE_TICKS: i32 = 12;
/// The defender presses after this many ticks of a slow carrier (`0x668e6c`), tackling every 24th tick or within
/// `PRESS_RANGE` (`0x5bf3bc` = 6.25 squared).
pub const PRESS_AFTER: i32 = 18;
pub const PRESS_RANGE: f64 = 2.5;
/// A carrier slower than this (squared, `0x5bf358`) lets the press counter run.
pub const SLOW_SPEED: f64 = 5.0;

/// A team's tackle fields.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TeamTackle {
    /// `+0x58`: counts down every handled tick; tackles need it negative.
    pub recharge: i32,
    /// `+0xab4`: ticks of stun after being tackled; no tackle or lunge while positive.
    pub stun: i32,
    /// `+0xa64`: seconds of lunge left.
    pub lunge: f64,
    /// `+0xab8`: the team's carrier is dodging (set by a dodge, cleared by the team's next update).
    pub dodging: bool,
    /// Ticks before the carrier may dodge again (`0x606640`, 14).
    pub dodge_cool: i32,
    /// `+0xa44`: the velocity accumulator (the lunge velocity while lunging).
    pub acc: Vec3,
    /// `+0xa54`: the lunge displacement; while lunging the accumulator keeps at least its length along it.
    pub lunge_disp: Vec3,
    /// `+0xaa4`: knock-back vector set on the tackled team (its reader is not traced).
    pub knock_back: Vec3,
}

/// What the tackle branch did this tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome {
    /// The tackle succeeds: the tackler flies to `point` and takes the ball after `ticks` (`0x426d40`).
    Tackle { point: Vec3, ticks: i32 },
    /// The victim's team is dodging: a miss lunge of `ticks` (`0x4275e0`).
    Whiff { ticks: i32 },
    /// A lunge started; the team's accumulator holds its velocity.
    Lunge,
}

/// How a lunge ended ([`TeamTackle::lunge_end`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LungeEnd {
    /// On its victim: the tackle, to `point` in `ticks` (`0x426d40` with the lunge's flag, `0x444096`).
    Tackle { point: Vec3, ticks: i32 },
    /// Off it: the team is stunned (`0x4441b2`) and the miss plays (`0x426df0`).
    Miss,
}

/// The tackler and the victim as the handling sees them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Situation {
    pub tackler: Vec3,
    /// The tackler's facing (`+0x50`), used when the stick is short.
    pub facing: Vec3,
    /// The stick (input `+0x00`).
    pub stick: Vec3,
    pub victim: Vec3,
    pub victim_vel: Vec3,
    /// The victim's team's `+0xab8`.
    pub victim_dodging: bool,
    /// `Tackle.LungeConeAngle` (team `+0x44`, degrees, default 90).
    pub cone_degrees: f64,
}

impl TeamTackle {
    /// The timers at the start of the handling: the stun counts down to 0 (every call).
    pub fn start(&mut self) {
        if self.stun != 0 {
            self.stun -= 1;
        }
    }

    /// The recharge counts down when the active player has no movement command pending; the tackle input survives
    /// only when it is negative and the player is a chaser.
    pub fn recharge_allows(&mut self, chaser: bool) -> bool {
        self.recharge -= 1;
        self.recharge < 0 && chaser
    }

    /// The tackle branch (`0x442cfb`-`0x4433c8`), for a tackle input that survived [`TeamTackle::recharge_allows`].
    pub fn tackle(&mut self, s: &Situation) -> Option<Outcome> {
        if self.stun > 0 || self.lunge > 0.0 {
            return None;
        }
        let r = sub(s.victim, s.tackler);
        let d = len(r);
        if d < RANGE {
            if s.victim_dodging {
                return Some(Outcome::Whiff { ticks: TACKLE_TICKS });
            }
            let point = add(s.victim, scale(s.victim_vel, TACKLE_TICKS as f64 / 24.0));
            return Some(Outcome::Tackle { point, ticks: TACKLE_TICKS });
        }
        // the lunge: aim where the victim will be after `t`, sooner when it flies at the tackler (`w` its speed
        // towards him; checked under an emulator)
        let w = dot(unit(sub(s.tackler, s.victim)), s.victim_vel);
        let mut t = (d - LUNGE_REACH) / LUNGE_SPEED;
        if w > 0.0 {
            t = (d - LUNGE_REACH) / (w + LUNGE_SPEED);
        }
        let t = t.max(LUNGE_MIN);
        let mut disp = sub(add(s.victim, scale(s.victim_vel, t)), s.tackler);
        let mut dir = if dot(s.stick, s.stick) < 0.25 { s.facing } else { s.stick };
        dir[1] = 0.0;
        let dir = unit(dir);
        let c = dot(unit(disp), dir);
        let cos = cos(s.cone_degrees * std::f64::consts::PI / 360.0);
        if c <= cos {
            // outside the cone: along the stick, shortened
            let k = (CONE_SLOPE * (cos - c)).clamp(0.0, 1.0);
            disp = scale(dir, len(disp) * ((1.0 - k) + k * CONE_FLOOR));
        }
        self.lunge_disp = disp;
        self.acc = scale(disp, 1.0 / t);
        self.lunge = t + LUNGE_EXTRA;
        Some(Outcome::Lunge)
    }

    /// Later in the handling (`0x443f27`): the lunge time runs down by a tick, never below 0, and the lunge ends
    /// when it runs out or the tackler is within 1.5 of the opponents' active player (`dist`). It then tackles the
    /// victim (`pos`, `vel`) if it was within [`RANGE`] at the tick's start (`in_range`) and its team is not
    /// dodging, leading it by the lunge's time left, at least 8 ticks; otherwise it misses and the team is
    /// stunned for 12 ticks.
    pub fn lunge_end(
        &mut self,
        dist: f64,
        in_range: bool,
        victim: Option<(Vec3, Vec3)>,
        victim_dodging: bool,
    ) -> Option<LungeEnd> {
        if self.lunge <= 0.0 {
            return None;
        }
        self.lunge = (self.lunge - 1.0 / 24.0).max(0.0);
        if self.lunge > 0.0 && dist > LUNGE_REACH {
            return None;
        }
        let out = match victim {
            Some((pos, vel)) if in_range && !victim_dodging => {
                let t = (LUNGE_LEAD / 24.0).max(self.lunge);
                LungeEnd::Tackle { point: add(pos, scale(vel, t)), ticks: trunc(t * 24.0 + 1e-9) }
            }
            _ => {
                self.stun = MISS_STUN;
                LungeEnd::Miss
            }
        };
        self.lunge = 0.0;
        self.acc = [0.0; 3];
        Some(out)
    }

    /// The horizontal velocity while lunging (`0x44454c`-`0x4446a2`): `A = (A + s) k` with `s` the stick times the
    /// acceleration, then `A`'s component along the lunge displacement is raised to at least its length.
    pub fn lunge_velocity(&mut self, s: Vec3, k: f64) -> Vec3 {
        let a = scale(add(self.acc, s), k);
        let e = unit(self.lunge_disp);
        let along = dot(e, a);
        let across = sub(a, scale(e, along));
        let along = along.max(len(self.lunge_disp));
        self.acc = add(across, scale(e, along));
        self.acc
    }

    /// The penalty on the tackled team (`0x436a80`, without the power-bar charge): a knock-back of 12 units per
    /// second away from the tackler (in `x, z`; sideways of the victim's facing when they overlap) and 25 ticks of
    /// stun.
    pub fn penalise(&mut self, victim: Vec3, tackler: Vec3, victim_facing: Vec3) {
        let mut v = flat(sub(victim, tackler));
        if dot(v, v) < 0.1 {
            v = [victim_facing[2], 0.0, -victim_facing[0]];
        }
        self.knock_back = scale(unit(v), KNOCK_BACK);
        self.stun = STUN_TICKS;
    }
}

/// The AI defender's press counter (state 0x21 `+0x450`, `0x453b10`-`0x453b95`) for one tick: it runs while the
/// carrier (a chaser with the ball) is slower than 5 units per second, and keeps running once past 18; it resets
/// when the carrier is the keeper or holds no ball. Returns the new counter.
pub fn press_counter(counter: i32, carrier_is_keeper: bool, carrier_has_ball: bool, carrier_vel: Vec3) -> i32 {
    if carrier_is_keeper || !carrier_has_ball {
        return 0;
    }
    if dot(carrier_vel, carrier_vel) < SLOW_SPEED * SLOW_SPEED || counter > PRESS_AFTER {
        counter + 1
    } else {
        counter
    }
}

/// The press input once the counter is past 18 (`0x453ba1`-`0x453c69`): the stick points at the carrier (3D, unit
/// length) and the tackle button is pressed on every 24th tick of the counter or within 2.5 units.
pub fn press_input(counter: i32, defender: Vec3, carrier: Vec3) -> (Vec3, bool) {
    let d = sub(carrier, defender);
    let tackle = counter % 24 == 0 || dot(d, d) < PRESS_RANGE * PRESS_RANGE;
    (unit(d), tackle)
}

/// The counter value after the press tick: at 19 it skips to 20 (where, against a human team, one `rand() % 3`
/// picks a chaser for a commentary event; AI teams draw nothing).
pub fn press_after(counter: i32) -> i32 {
    if counter == PRESS_AFTER + 1 {
        counter + 1
    } else {
        counter
    }
}

/// Ticks of a tackle command as an integer (`_ftol(10)`).
pub fn tackle_ticks() -> i32 {
    trunc(TACKLE_TICKS as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn situation(victim: Vec3, victim_vel: Vec3, stick: Vec3) -> Situation {
        Situation {
            tackler: [0.0; 3],
            facing: [1.0, 0.0, 0.0],
            stick,
            victim,
            victim_vel,
            victim_dodging: false,
            cone_degrees: 90.0,
        }
    }

    #[test]
    fn recharge_and_stun() {
        let mut t = TeamTackle { recharge: RECHARGE_TICKS, ..Default::default() };
        // 12 -> 0 over 12 calls still blocks; the 13th call makes it negative
        for _ in 0..12 {
            assert!(!t.recharge_allows(true));
        }
        assert!(t.recharge_allows(true));
        assert!(!t.recharge_allows(false));
        t.stun = 2;
        t.start();
        assert_eq!(t.stun, 1);
        let s = situation([1.0, 0.0, 0.0], [0.0; 3], [1.0, 0.0, 0.0]);
        assert_eq!(t.tackle(&s), None);
        t.start();
        assert!(t.tackle(&s).is_some());
        t.start();
        assert_eq!(t.stun, 0);
    }

    #[test]
    fn in_range_tackle_meets_the_victim() {
        let mut t = TeamTackle::default();
        let s = situation([2.0, 0.0, 0.0], [12.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        // 10 ticks at 12 u/s: 5 units further on
        assert_eq!(t.tackle(&s), Some(Outcome::Tackle { point: [7.0, 0.0, 0.0], ticks: 10 }));
        let s = Situation { victim_dodging: true, ..s };
        assert_eq!(t.tackle(&s), Some(Outcome::Whiff { ticks: 10 }));
    }

    #[test]
    fn lunge_leads_the_victim() {
        let mut t = TeamTackle::default();
        // 11.5 away, flying at the tackler at 10: t = 10 / 20 = 0.5 s, aim 5 nearer: displacement 6.5 along x
        let s = situation([11.5, 0.0, 0.0], [-10.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        assert_eq!(t.tackle(&s), Some(Outcome::Lunge));
        assert!((t.lunge - 0.6).abs() < 1e-12);
        assert!(len(sub(t.acc, [13.0, 0.0, 0.0])) < 1e-9);
        // flying away does not lengthen it: t = 10 / 10, 10 further on
        let mut t2 = TeamTackle::default();
        t2.tackle(&situation([11.5, 0.0, 0.0], [10.0, 0.0, 0.0], [1.0, 0.0, 0.0]));
        assert!((t2.lunge - 1.1).abs() < 1e-12);
        assert!(len(sub(t2.acc, [21.5, 0.0, 0.0])) < 1e-9);
        // no tackle while lunging
        assert_eq!(t.tackle(&s), None);
        // a short, slow lunge takes at least 0.3 s
        let mut t = TeamTackle::default();
        t.tackle(&situation([3.0, 0.0, 0.0], [0.0; 3], [1.0, 0.0, 0.0]));
        assert!((t.lunge - 0.4).abs() < 1e-12);
        assert!(len(sub(t.acc, [10.0, 0.0, 0.0])) < 1e-9);
    }

    #[test]
    fn lunge_outside_the_cone_follows_the_stick() {
        let mut t = TeamTackle::default();
        // victim straight ahead in x, stick along z: c = 0 < cos 45, k = clamp(3 * 0.7071) = 1, scale 0.3
        t.tackle(&situation([11.5, 0.0, 0.0], [0.0; 3], [0.0, 0.0, 1.0]));
        assert!(len(sub(t.lunge_disp, [0.0, 0.0, 11.5 * 0.3])) < 1e-9);
        // a short stick falls back to the facing (+x): inside the cone, unchanged
        let mut t = TeamTackle::default();
        t.tackle(&situation([11.5, 0.0, 0.0], [0.0; 3], [0.0, 0.0, 0.4]));
        assert!(len(sub(t.lunge_disp, [11.5, 0.0, 0.0])) < 1e-9);
    }

    #[test]
    fn lunge_velocity_keeps_its_drive() {
        let mut t = TeamTackle { acc: [20.0, 0.0, 0.0], lunge_disp: [5.0, 0.0, 0.0], ..Default::default() };
        // (20 + 0) * 0.95 = 19 along x, above the floor of 5; the stick's z part is kept
        let v = t.lunge_velocity([0.0, 0.0, 1.0], 0.95);
        assert!(len(sub(v, [19.0, 0.0, 0.95])) < 1e-9);
        // a stick against the lunge cannot take the forward part below |D|
        let mut t = TeamTackle { acc: [1.0, 0.0, 0.0], lunge_disp: [5.0, 0.0, 0.0], ..Default::default() };
        assert!(len(sub(t.lunge_velocity([-1.0, 0.0, 0.0], 0.95), [5.0, 0.0, 0.0])) < 1e-9);
    }

    #[test]
    fn a_lunge_ends_on_its_victim() {
        let v = Some(([6.0, 0.0, 0.0], [-10.0, 0.0, 0.0]));
        // far off with time left: it runs on
        let mut t = TeamTackle { lunge: 0.3, ..Default::default() };
        assert_eq!(t.lunge_end(3.0, false, v, false), None);
        assert!((t.lunge - (0.3 - 1.0 / 24.0)).abs() < 1e-12);
        // within 1.5, the victim in reach at the tick's start: the tackle, 8 ticks ahead of the victim
        // (`tools/check_handling.py`: from 1.2 away at 10 towards the tackler, to 3.33 nearer him)
        let Some(LungeEnd::Tackle { point, ticks }) = t.lunge_end(1.2, true, v, false) else { panic!() };
        assert!(len(sub(point, [6.0 - 10.0 / 3.0, 0.0, 0.0])) < 1e-12 && ticks == 8);
        assert_eq!((t.lunge, t.acc, t.stun), (0.0, [0.0; 3], 0));
        // a long lunge left leads by its time
        let mut t = TeamTackle { lunge: 0.55, ..Default::default() };
        let left = 0.55 - 1.0 / 24.0;
        let Some(LungeEnd::Tackle { point, ticks }) = t.lunge_end(1.0, true, v, false) else { panic!() };
        assert!(len(sub(point, [6.0 - 10.0 * left, 0.0, 0.0])) < 1e-12 && ticks == 12);
        // the time runs out out of reach, or on a dodging victim: the miss, and the team is stunned
        let mut t = TeamTackle { lunge: 0.03, ..Default::default() };
        assert_eq!(t.lunge_end(4.0, false, v, false), Some(LungeEnd::Miss));
        assert_eq!(t.stun, MISS_STUN);
        let mut t = TeamTackle { lunge: 0.3, ..Default::default() };
        assert_eq!(t.lunge_end(1.0, true, v, true), Some(LungeEnd::Miss));
        assert_eq!(t.lunge_end(1.0, true, v, false), None);
    }

    #[test]
    fn penalty() {
        let mut t = TeamTackle::default();
        t.penalise([3.0, 1.0, 4.0], [0.0, 5.0, 0.0], [0.0; 3]);
        assert_eq!(t.stun, 25);
        assert!(len(sub(t.knock_back, [7.2, 0.0, 9.6])) < 1e-9);
    }

    #[test]
    fn press() {
        let slow = [3.0, 0.0, 0.0];
        let fast = [6.0, 0.0, 0.0];
        assert_eq!(press_counter(5, false, true, slow), 6);
        assert_eq!(press_counter(5, false, true, fast), 5);
        assert_eq!(press_counter(19, false, true, fast), 20);
        assert_eq!(press_counter(19, true, true, slow), 0);
        assert_eq!(press_counter(19, false, false, slow), 0);
        let (stick, tackle) = press_input(25, [0.0; 3], [0.0, 3.0, 4.0]);
        assert!(len(sub(stick, [0.0, 0.6, 0.8])) < 1e-12);
        assert!(!tackle);
        assert!(press_input(48, [0.0; 3], [0.0, 3.0, 4.0]).1);
        assert!(press_input(25, [0.0; 3], [0.0, 1.5, 1.5]).1);
        assert_eq!(press_after(19), 20);
        assert_eq!(press_after(20), 20);
        assert_eq!(tackle_ticks(), 10);
    }
}
