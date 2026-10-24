//! The AI defender's approach (state 0x21, `0x453ac0`, while its press counter is not past 18): a move kind is
//! drawn, a formotion of that kind flies the defender round the carrier, then the kind's homing steers him in
//! until the carrier gets past, and the approach starts again. Traced 2026-10-24.
//!
//! - **The kinds** (the table `0x655e78`, stride `0x18`: homing, fallback, name, weight, accuracy, 2.0):
//!   `DirectFrontal` (`0x454330`) and `BlockSide` (`0x4549e0`), weights 0.35 and 0.65 and accuracy 1 unless the
//!   team files set them (`AI.Defense.Select`, `AI.Defense.Accuracy`: one table, so the last file loaded sets it
//!   for both teams); the third, `<NULL>`, has no homing and weight 0. The fallbacks (`0x454130`, `0x454720`) run
//!   only when no formotion of the kind exists, which no retail database allows: not ported.
//! - **The start** (phase 0): the defender is *ahead* when he is more than 1 unit along the carrier's attack
//!   direction from him and that direction is within `acos 0.3` of the line to him (`0x668e74`, `0x668e70`); then
//!   phase 1. One `rand()` draws the kind ([`Table::draw`]); [`crate::formation::pick_defence`] draws its
//!   formotion (bit 7: the moves from behind). The formotion runs in a block whose frame follows the carrier
//!   (cap 25, `0x66903c`), scaled (10, 8, 10) and (1, 1, 1), turned half round for team 1, its time stretched by
//!   the team's `Formotions.TweakTime AI_DEF`, the step capped at 30 (`0x669038`); phase 1.
//! - **The run** (`+0x18`, set by the state's enter `0x453900`): each tick the formotion steps (`0x42dee0`); once
//!   it is done the phase is 2 and the homing takes over, and against a human team one `rand() % 3` picks a
//!   player for an event (not affecting play).
//! - **The homing** (phase 2 on, counting up): the steer goes to the general mover with the tackle byte
//!   (`0x436490(1, input)`); see [`direct_frontal`] and [`block_side`]. Both read the carrier as the team's
//!   history holds him `delay` seconds ago ([`History`]); both start the approach again (phase 0, the run on)
//!   once he is past.

use crate::console::{Console, Value};
use crate::formation::Runner;
use crate::vec3::{len, sub, unit, Vec3};

/// The block's scales (`0x42dd40`: `0x106698c` and `0x106697c`).
pub const SCALES: (Vec3, Vec3) = ([10.0, 8.0, 10.0], [1.0, 1.0, 1.0]);
/// The frame's follow cap (`0x66903c`) and the step cap (`0x669038`), units a second.
pub const FOLLOW: f64 = 25.0;
pub const STEP_CAP: f64 = 30.0;
/// The tackle byte is set within this of the carrier, across the ground (`0x5bf0c0`).
pub const REACH: f64 = 2.0;
/// The history's length in ticks (team `+0xae0`, head `+0x780`), and the longest delay read (`0x5bf0e0`).
pub const HISTORY: usize = 24;
pub const MAX_DELAY: f64 = 0.99;

/// The kinds' names, in the table's order.
pub const KINDS: [&str; 3] = ["DirectFrontal", "BlockSide", "<NULL>"];

/// The kinds' weights and accuracies (`+0xc`, `+0x10`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Table {
    pub weight: [f64; 3],
    pub accuracy: [f64; 3],
}

impl Default for Table {
    fn default() -> Table {
        Table { weight: [0.35, 0.65, 0.0], accuracy: [1.0, 1.0, 0.5] }
    }
}

impl Table {
    /// The image's table with the console's `AI.Defense.Select` and `AI.Defense.Accuracy` calls, in order.
    pub fn from_console(c: &Console) -> Table {
        let mut t = Table::default();
        for (path, args) in c.calls() {
            let field = match path {
                "AI.Defense.Select" => &mut t.weight,
                "AI.Defense.Accuracy" => &mut t.accuracy,
                _ => continue,
            };
            let (Some(Value::Text(name)), Some(v)) = (args.first(), args.get(1).and_then(Value::num)) else {
                continue;
            };
            if let Some(k) = KINDS.iter().position(|n| n.eq_ignore_ascii_case(name)) {
                field[k] = v;
            }
        }
        t
    }

    /// The kind for `rand()` value `r`: the first whose running sum of weights passes `r * sum / 32767`; 0 when
    /// none does.
    pub fn draw(&self, r: i32) -> usize {
        let total: f64 = self.weight.iter().sum();
        let u = r as f64 * total * (1.0 / 32767.0);
        let mut sum = 0.0;
        for (k, w) in self.weight.iter().enumerate() {
            sum += w;
            if u < sum {
                return k;
            }
        }
        0
    }
}

/// The opposing active player's position and velocity over the last 24 ticks, pushed at the start of the team's
/// AI update (`0x435270` -> `0x4334e0`) and read `delay` seconds back (`0x433490`, `0x433530`).
#[derive(Debug, Clone, PartialEq)]
pub struct History {
    entries: [(Vec3, Vec3); HISTORY],
    head: usize,
}

impl Default for History {
    fn default() -> History {
        History { entries: [([0.0; 3], [0.0; 3]); HISTORY], head: 0 }
    }
}

impl History {
    pub fn push(&mut self, pos: Vec3, vel: Vec3) {
        self.head = (self.head + 1) % HISTORY;
        self.entries[self.head] = (pos, vel);
    }

    /// The entry `trunc(min(delay, 0.99) * 24)` ticks back.
    pub fn back(&self, delay: f64) -> (Vec3, Vec3) {
        let lag = (delay.min(MAX_DELAY) * 24.0).trunc() as usize;
        self.entries[(self.head + HISTORY - lag) % HISTORY]
    }
}

/// The approach's state (state 0x21's fields; the enter `0x453900` clears them and sets the run).
#[derive(Debug, Clone, Default)]
pub struct Approach {
    /// `+0x10`: 0 to start a move, 1 while its formotion runs, 2 and on (counting) homing.
    pub phase: i32,
    /// `+0x14`: the kind.
    pub kind: usize,
    /// `+0x18`: the formotion is to run (else the homing).
    pub run: bool,
    /// `+0x1c`: ticks within reach (the frontal homing's tackle).
    pub near: i32,
    /// The block (`+0x20`) when a formotion was found (`+0x448`).
    pub runner: Option<Runner>,
    /// The state the approach belongs to: the active player and the defence's choice (**[I]** the state is
    /// entered again when either changes).
    pub key: Option<(usize, u32)>,
}

impl Approach {
    /// The state's enter (`0x453900`).
    pub fn enter(key: (usize, u32)) -> Approach {
        Approach { run: true, key: Some(key), ..Approach::default() }
    }
}

/// The phase-0 test: the defender at `me` is ahead of the carrier at `carrier`, whose team attacks along `dir`.
pub fn ahead(me: Vec3, carrier: Vec3, dir: f64) -> bool {
    let d = sub(me, carrier);
    d[0] * dir > 1.0 && unit(d)[0] * dir > 0.3
}

/// What a homing tick asks of the general mover: the stick, the tackle byte, and whether the approach starts again.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Homing {
    pub stick: Vec3,
    pub tackle: bool,
    pub restart: bool,
}

/// The two homings' view of the play: the defender, the carrier now and `delay` seconds ago, and the teams'
/// attack directions (`own` the defender's).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub me: Vec3,
    pub me_vel: Vec3,
    pub carrier: Vec3,
    /// The history's entry for the kind's delay ([`delay`]).
    pub past: (Vec3, Vec3),
    pub own: f64,
    pub opp: f64,
}

/// The seconds of the carrier's history the kind reads: `DirectFrontal` 0.35 at accuracy 0, none at 1
/// (`0x606094`, `0x606098`); `BlockSide` 0.5 to 0.1 (`0x6060a4`, `0x6060a8`).
pub fn delay(kind: usize, accuracy: f64) -> f64 {
    let a = accuracy;
    match kind {
        0 => 0.35 * (1.0 - a),
        _ => 0.5 * (1.0 - a) + 0.1 * a,
    }
}

/// `DirectFrontal`'s homing (`0x454330`) on homing tick `count` (the phase before it counted on, from 2): a point
/// ahead of the carrier along his attack, 11 units ahead at first and closing to 1 over `40 (1 - a) + 20 a` ticks,
/// at the height and across-place he had `delay` ago; the stick is the way there (a unit at most). The tackle once
/// he has been within reach for more than `trunc(18 (1 - a))` ticks (`near`, counted here).
pub fn direct_frontal(v: &View, count: i32, accuracy: f64, near: &mut i32) -> Homing {
    let a = accuracy;
    let ramp = ((count - 1) as f64 / (40.0 * (1.0 - a) + 20.0 * a)).min(1.0);
    let k = (1.0 - ramp) * 10.0 + 1.0;
    let target = [v.carrier[0] + v.opp * k, v.past.0[1], v.past.0[2]];
    let mut stick = sub(target, v.me);
    if len(stick) > 1.0 {
        stick = unit(stick);
    }
    let (dx, dz) = (v.carrier[0] - v.me[0], v.carrier[2] - v.me[2]);
    let restart = v.own * dx < -1.0;
    let mut tackle = false;
    if (dx * dx + dz * dz).sqrt() < REACH {
        *near += 1;
        tackle = *near > (18.0 * (1.0 - a)).trunc() as i32;
    }
    Homing { stick, tackle, restart }
}

/// `BlockSide`'s homing (`0x4549e0`): a velocity error. The defender aims at the carrier's velocity of `delay`
/// ago plus `5 a + 3 (1 - a)` along his own attack and, across, `2 a + 0.6 (1 - a)` times the gap to the carrier
/// (no more than `4 a + 2 (1 - a)`; the whole across-velocity no more than 10, `0x668e54`, while the carrier is
/// still on the side he started on, `side`, `[0xf91900]`); each part of the stick is then kept within 1, and the
/// stick to a unit. The tackle within reach; the restart once the carrier is 1.7 past.
pub fn block_side(v: &View, accuracy: f64, side: f64) -> Homing {
    let a = accuracy;
    let (dx, dz) = (v.carrier[0] - v.me[0], v.carrier[2] - v.me[2]);
    let m = 4.0 * a + 2.0 * (1.0 - a);
    let across = ((2.0 * a + 0.6 * (1.0 - a)) * dz).clamp(-m, m);
    let mut sx = (v.past.1[0] - v.me_vel[0]) + (5.0 * a + 3.0 * (1.0 - a)) * v.own;
    let mut sz = (v.past.1[2] - v.me_vel[2]) + across;
    let want = sz + v.me_vel[2];
    if want.abs() > 10.0 && dz * side < 0.0 {
        sz = 10.0f64.copysign(want) - v.me_vel[2];
    }
    sx = sx.clamp(-1.0, 1.0);
    sz = sz.clamp(-1.0, 1.0);
    let mut stick = [sx, 0.0, sz];
    if sx * sx + sz * sz > 1.0 {
        stick = unit(stick);
    }
    Homing { stick, tackle: (dx * dx + dz * dz).sqrt() < REACH, restart: v.own * dx < -1.7 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kind_is_drawn_on_the_running_weights() {
        let t = Table::default();
        assert_eq!(t.draw(0), 0);
        assert_eq!(t.draw((0.34 * 32767.0) as i32), 0);
        assert_eq!(t.draw((0.36 * 32767.0) as i32), 1);
        assert_eq!(t.draw(32767), 0, "nothing passes the whole sum");
        let mut c = Console::new();
        c.run("AI.Defense.Select", "DirectFrontal 0.6").unwrap();
        c.run("AI.Defense.Accuracy", "BlockSide 0.25").unwrap();
        let t = Table::from_console(&c);
        assert_eq!((t.weight, t.accuracy[1]), ([0.6, 0.65, 0.0], 0.25));
    }

    #[test]
    fn the_history_reads_back_whole_ticks() {
        let mut h = History::default();
        for k in 0..30 {
            h.push([k as f64, 0.0, 0.0], [0.0; 3]);
        }
        assert_eq!(h.back(0.0).0[0], 29.0);
        assert_eq!(h.back(0.35).0[0], 21.0);
        assert_eq!(h.back(5.0).0[0], 6.0, "at most 23 ticks back");
    }

    #[test]
    fn ahead_is_goal_side_within_the_cone() {
        // a carrier attacking +x: a defender 5 ahead is ahead, one beside him is not
        assert!(ahead([5.0, 0.0, 1.0], [0.0; 3], 1.0));
        assert!(!ahead([0.5, 0.0, 0.0], [0.0; 3], 1.0));
        assert!(!ahead([2.0, 0.0, 10.0], [0.0; 3], 1.0));
        assert!(!ahead([-5.0, 0.0, 0.0], [0.0; 3], 1.0));
    }

    #[test]
    fn the_frontal_homing_aims_ahead_of_the_carrier_and_tackles_late() {
        // the carrier at the origin attacking +x, the defender (attacking -x) 20 ahead of him
        let v = View {
            me: [20.0, 0.0, 0.0],
            me_vel: [0.0; 3],
            carrier: [0.0; 3],
            past: ([0.0; 3], [0.0; 3]),
            own: -1.0,
            opp: 1.0,
        };
        let mut near = 0;
        let h = direct_frontal(&v, 1, 1.0, &mut near);
        assert_eq!(h.stick, [-1.0, 0.0, 0.0]);
        assert!(!h.restart && !h.tackle);
        // in reach: at accuracy 0.75 the tackle comes on the fifth tick
        let v = View { me: [1.0, 0.0, 0.0], ..v };
        let t: Vec<bool> = (0..6).map(|_| direct_frontal(&v, 5, 0.75, &mut near).tackle).collect();
        assert_eq!(t, [false, false, false, false, true, true]);
        // the carrier past him: start again
        let v = View { me: [-2.0, 0.0, 0.0], ..v };
        assert!(direct_frontal(&v, 5, 0.75, &mut near).restart);
    }

    #[test]
    fn the_side_homing_matches_the_carriers_velocity_and_closes_across() {
        let v = View {
            me: [10.0, 0.0, 0.0],
            me_vel: [-3.0, 0.0, 0.0],
            carrier: [0.0, 0.0, 5.0],
            past: ([0.0; 3], [3.0, 0.0, 0.0]),
            own: -1.0,
            opp: 1.0,
        };
        let h = block_side(&v, 1.0, -5.0);
        // along: 3 - (-3) - 5 = 1; across: 2 * 5 = 10 kept to 4, then to 1; the stick a unit
        let r = std::f64::consts::FRAC_1_SQRT_2;
        assert!((h.stick[0] - r).abs() < 1e-12 && (h.stick[2] - r).abs() < 1e-12, "{:?}", h.stick);
        assert!(!h.tackle && !h.restart);
    }
}
