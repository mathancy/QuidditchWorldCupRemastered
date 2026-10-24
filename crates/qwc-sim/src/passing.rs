//! The AI's passing decisions (`docs/GAMEPLAY.md`, "Passing"): how many passes a team makes, how long a carrier holds
//! the Quaffle, when the wind-up releases, where the pass goes and how long it flies.
//!
//! Each function is one traced piece of the possession state (0x11), the pass state (0x17), the pass aim
//! (`0x43dd30`) or the cone test (`0x441f70`), with the game's draw order kept. The parts the player and team objects
//! supply (whether the carrier is the keeper, receivers, movement) are inputs until those objects are ported.
//! Where a probability roll is combined with a flag (`U < ProbDirectPass` or a keeper carrier), the roll is always drawn
//! first; the evaluation order of those conditions is inferred.

use crate::console::Console;
use crate::rng::{trunc, Rand};

/// Simulation ticks per second; frame counts in the passing code use this.
pub const TICKS: f64 = 24.0;
/// Wind-up length in ticks (`0x668e04`).
pub const WIND_UP_TICKS: i32 = 24;
/// The pass state ends when the ball is still held after this many ticks (`0x668e00`).
pub const PASS_STATE_TICKS: i32 = 30;

/// A team's `Passing.PassAI.*` values (set per team by `team_N.cli`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PassAi {
    pub pass_number_base: f32,
    pub pass_number_range: f32,
    pub pass_out_base: f32,
    pub pass_out_range: f32,
    pub pass_out_base_gp: f32,
    pub pass_out_range_gp: f32,
    pub prob_super_passing_mode: f32,
    pub combo_prob: f32,
    pub combo_min_chain: f32,
    pub combo_range_chain: f32,
    pub pass_in_front_dist: f32,
    pub prob_direct_pass: f32,
    pub prob_throw: f32,
    pub volley_prob: f32,
    pub wind_up_base: i32,
}

impl PassAi {
    pub fn from_console(c: &Console, team: usize) -> PassAi {
        let f = |k: &str| c.f32(&format!("Passing.PassAI.{k}"), team);
        PassAi {
            pass_number_base: f("PassNumberBase"),
            pass_number_range: f("PassNumberRange"),
            pass_out_base: f("PassOutBase"),
            pass_out_range: f("PassOutRange"),
            pass_out_base_gp: f("PassOutBaseGP"),
            pass_out_range_gp: f("PassOutRangeGP"),
            prob_super_passing_mode: f("ProbSuperPassingMode"),
            combo_prob: f("ComboProb"),
            combo_min_chain: f("ComboMinChain"),
            combo_range_chain: f("ComboRangeChain"),
            pass_in_front_dist: f("PassInFrontDist"),
            prob_direct_pass: f("ProbDirectPass"),
            prob_throw: f("ProbThrow"),
            volley_prob: f("VolleyProb"),
            wind_up_base: c.num("Passing.PassAI.WindUpBase", team) as i32,
        }
    }
}

/// The passing globals the possession and pass states share (`0x606044`, `0xf918f4`, `0xf918f0`) and the hold time
/// of the current possession (state `+0x14`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PassPlan {
    pub passes_left: i32,
    pub super_passing: bool,
    /// Ticks the carrier holds the ball before passing.
    pub hold: i32,
    pub combo_left: i32,
}

/// Possession state enter (`0x456170`). `combo_enabled` is bit 3 of the DataStore value with key hash
/// `0x1527016b` (a game option, inferred). `plan.combo_left` is kept when no combo is drawn.
pub fn possession_enter(ai: &PassAi, rng: &mut Rand, combo_enabled: bool, plan: &mut PassPlan) {
    plan.passes_left = trunc(rng.unit() * ai.pass_number_range as f64 + ai.pass_number_base as f64);
    let (base, range) = if rng.unit() < ai.prob_super_passing_mode as f64 {
        plan.super_passing = true;
        (ai.pass_out_base_gp, ai.pass_out_range_gp)
    } else {
        plan.super_passing = false;
        (ai.pass_out_base, ai.pass_out_range)
    };
    plan.hold = trunc(trunc(base as f64 * TICKS) as f64 + rng.unit() * range as f64 * TICKS);
    if rng.unit() < ai.combo_prob as f64 && combo_enabled {
        plan.combo_left = draw_combo(ai, rng);
    }
}

fn draw_combo(ai: &PassAi, rng: &mut Rand) -> i32 {
    trunc(rng.unit() * ai.combo_range_chain as f64 + ai.combo_min_chain as f64)
}

/// The redraw in the possession update (`0x456340`, step 1) once `passes_left` reaches 0: the pass count as on
/// enter, the super-passing roll, the hold time in its integer form `rand() % trunc(range * 24) + trunc(base * 24)`,
/// and a new combo chain when none is running.
///
/// Inferred: the combo redraw uses the same enable bit as enter. A zero hold range would divide by zero in the
/// original; here it gives the base.
pub fn possession_redraw(ai: &PassAi, rng: &mut Rand, combo_enabled: bool, plan: &mut PassPlan) {
    plan.passes_left = trunc(rng.unit() * ai.pass_number_range as f64 + ai.pass_number_base as f64);
    plan.super_passing = rng.unit() < ai.prob_super_passing_mode as f64;
    let (base, range) = match plan.super_passing {
        true => (ai.pass_out_base_gp, ai.pass_out_range_gp),
        false => (ai.pass_out_base, ai.pass_out_range),
    };
    let (base, range) = (trunc(base as f64 * TICKS), trunc(range as f64 * TICKS));
    plan.hold = if range > 0 { rng.rand() % range + base } else { base };
    if plan.combo_left == 0 && rng.unit() < ai.combo_prob as f64 && combo_enabled {
        plan.combo_left = draw_combo(ai, rng);
    }
}

/// The lane a keeper carrying the Quaffle moves towards across the pitch (`0x10439e0`): `(U - 0.5) * 2 * 17`.
pub fn lateral_target(rng: &mut Rand) -> f64 {
    (rng.unit() - 0.5) * 2.0 * 17.0
}

/// Whether a team mate counts as ahead of the carrier for the pass-out test: `attack_dir * (mate.x - carrier.x)`
/// above `PassInFrontDist`.
pub fn mate_ahead(ai: &PassAi, attack_dir: f64, carrier_x: f64, mate_x: f64) -> bool {
    attack_dir * (mate_x - carrier_x) > ai.pass_in_front_dist as f64
}

/// Where a pass goes (pass state, step 2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    /// To the field player at this index among the other mates.
    Mate(usize),
    /// Forward with no receiver, along this unit direction.
    Forward([f64; 3]),
}

/// Inputs to the pass choice that come from the player and pitch objects.
#[derive(Debug, Clone, Copy)]
pub struct PassSituation {
    /// The carrier is the keeper (carrier vtable `+0x08` returns the entity type: 0 chaser, 1 keeper).
    pub keeper: bool,
    /// The team's `+0x18` object is in its special mode (`+0x450`).
    pub special_mode: bool,
    /// `0x455d50`: the carrier is outside the central ellipse or in a goal area.
    pub wide: bool,
    /// Index (among the other mates) of the most central one, smallest `|z|`.
    pub most_central: usize,
    pub attack_dir: f64,
    pub carrier_z: f64,
    /// `0x10439d8` side flag: true selects `+1`.
    pub side_flag: bool,
}

/// Pass state step 2: pass to a mate, or throw forward.
pub fn choose_target(ai: &PassAi, rng: &mut Rand, s: &PassSituation) -> Target {
    if (rng.unit() < ai.prob_direct_pass as f64 || s.keeper) && !s.special_mode {
        if s.wide {
            return Target::Mate(s.most_central);
        }
        return Target::Mate(trunc(rng.rand() as f64 / 16383.5) as usize);
    }
    let sign = if s.carrier_z < -28.0 {
        1.0
    } else if s.carrier_z > 28.0 {
        -1.0
    } else if s.side_flag {
        1.0
    } else {
        -1.0
    };
    let d = [s.attack_dir, 0.0, 0.7 * sign];
    let n = (d[0] * d[0] + d[2] * d[2]).sqrt();
    Target::Forward([d[0] / n, 0.0, d[2] / n])
}

/// The ellipse half of `0x455d50`: `(x / (0.4 L))^2 + (z / (0.4 W))^2 > 1` with `Pitch.Collision` length and width
/// (defaults 225 and 85).
pub fn outside_centre(x: f64, z: f64, length: f64, width: f64) -> bool {
    (x / (0.4 * length)).powi(2) + (z / (0.4 * width)).powi(2) > 1.0
}

/// Whether the wind-up releases this tick (pass state step 3; `timer` before its increment). Ticks above
/// `WindUpBase` and below 24 draw once each against `ProbThrow`; tick 24 always releases.
pub fn releases(ai: &PassAi, rng: &mut Rand, timer: i32, keeper: bool) -> bool {
    match timer {
        WIND_UP_TICKS => true,
        t if t > ai.wind_up_base && t < WIND_UP_TICKS => rng.unit() < ai.prob_throw as f64 || keeper,
        _ => false,
    }
}

/// At release during a combo: whether the pass is a volley (`U < VolleyProb` with the special mode off). The
/// caller decrements `combo_left`.
pub fn volley(ai: &PassAi, rng: &mut Rand, special_mode: bool) -> bool {
    rng.unit() < ai.volley_prob as f64 && !special_mode
}

/// Pass speed for wind-up fraction `f` (`timer / 24`): `6 + 14 * max(f, 0.2)` units per pass-aim step.
pub fn pass_speed(f: f64) -> f64 {
    6.0 + 14.0 * f.max(0.2)
}

/// Flight time in ticks: `max(6, trunc(D / (0.2 + 1.55 f')), trunc(2 R))` with `D` the carrier-to-target distance
/// and `R` the receiver-to-target distance (halved when the receiver is in the cone and `R >= 1`).
pub fn flight_ticks(f: f64, carrier_to_target: f64, receiver_to_target: f64, in_cone: bool) -> i32 {
    let f = f.max(0.2);
    let r = if in_cone && receiver_to_target >= 1.0 { receiver_to_target * 0.5 } else { receiver_to_target };
    6.max(trunc(carrier_to_target / (0.2 + 1.55 * f))).max(trunc(2.0 * r))
}

/// Shorten `offset` (carrier to target, or a lead vector) to at most `max` long (`MaxDistance`, `Lead`).
pub fn clamp_length(offset: [f64; 3], max: f64) -> [f64; 3] {
    let len = (offset[0] * offset[0] + offset[1] * offset[1] + offset[2] * offset[2]).sqrt();
    if len > max && len > 0.0 {
        offset.map(|v| v * max / len)
    } else {
        offset
    }
}

/// The effective pass cone in degrees (`0x441f70`). `opponent` is the vector from the carrier to the opposing
/// carrier; `combo_chain` is team `+0x5c` and only counts during a combo.
pub fn effective_cone(cone: f64, attack_dir: f64, opponent: Option<[f64; 3]>, combo_chain: Option<i32>) -> f64 {
    let mut cone = cone;
    if let Some(v) = opponent {
        let dist = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        let near = if attack_dir * v[0] > 0.0 { 2.0 } else { 1.0 };
        if dist < near {
            cone *= 0.5;
        }
    }
    if let Some(n) = combo_chain {
        cone -= (cone - 10.0) * (n.min(7) + 1) as f64 * 0.125;
        cone = cone.max(10.0);
    }
    cone
}

/// Whether the angle between the throw direction and the receiver (radians) is inside half the cone; a
/// keeper carrier always counts as inside.
pub fn in_cone(angle: f64, cone_degrees: f64, keeper: bool) -> bool {
    keeper || angle <= cone_degrees * 0.00872665
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ai() -> PassAi {
        PassAi::from_console(&Console::new(), 0)
    }

    #[test]
    fn defaults_come_from_the_image() {
        let a = ai();
        assert_eq!((a.pass_number_base, a.pass_number_range), (2.0, 2.0));
        assert_eq!((a.pass_out_base, a.pass_out_range, a.wind_up_base), (1.0, 3.0, 12));
    }

    #[test]
    fn enter_draws_in_order() {
        let a = PassAi { combo_prob: 1.0, ..ai() };
        let mut rng = Rand::new(1);
        let mut plan = PassPlan::default();
        possession_enter(&a, &mut rng, true, &mut plan);
        // draws 41, 18467, 6334, 26500, 19169 (as fractions of 32767)
        let u = |v: f64| v / 32767.0;
        assert_eq!(plan.passes_left, trunc(u(41.0) * 2.0 + 2.0));
        assert!(!plan.super_passing); // 18467/32767 >= 0.25
        assert_eq!(plan.hold, trunc(24.0 + u(6334.0) * 3.0 * 24.0));
        assert_eq!(plan.combo_left, trunc(u(19169.0) * 5.0 + 1.0));
        let mut off = PassPlan::default();
        possession_enter(&a, &mut Rand::new(1), false, &mut off);
        assert_eq!(off.combo_left, 0);
        let mut five = Rand::new(1);
        for _ in 0..5 {
            five.rand();
        }
        assert_eq!(rng, five);
    }

    #[test]
    fn release_timing() {
        let a = PassAi { prob_throw: 0.0, ..ai() };
        let mut rng = Rand::new(7);
        assert!(!releases(&a, &mut rng, 5, false));
        assert!(!releases(&a, &mut rng, 13, false));
        assert!(releases(&a, &mut rng, 13, true));
        assert!(releases(&a, &mut rng, 24, false));
    }

    #[test]
    fn aim_numbers() {
        assert_eq!(pass_speed(0.0), 6.0 + 14.0 * 0.2);
        assert_eq!(flight_ticks(1.0, 35.0, 0.0, false), 20);
        assert_eq!(flight_ticks(0.0, 1.0, 0.0, false), 6);
        assert_eq!(flight_ticks(1.0, 1.0, 10.0, true), 10);
        assert_eq!(effective_cone(45.0, 1.0, Some([1.5, 0.0, 0.0]), None), 22.5);
        assert_eq!(effective_cone(45.0, 1.0, Some([-1.5, 0.0, 0.0]), None), 45.0);
        assert_eq!(effective_cone(30.0, 1.0, None, Some(7)), 10.0);
        assert_eq!(effective_cone(30.0, 1.0, None, Some(1)), 25.0);
        assert_eq!(clamp_length([3.0, 0.0, 4.0], 2.5), [1.5, 0.0, 2.0]);
        assert!(outside_centre(91.0, 0.0, 225.0, 85.0) && !outside_centre(89.0, 0.0, 225.0, 85.0));
    }

    #[test]
    fn forward_throw_leans_to_the_middle() {
        let s = PassSituation {
            keeper: false,
            special_mode: true,
            wide: false,
            most_central: 0,
            attack_dir: 1.0,
            carrier_z: 30.0,
            side_flag: true,
        };
        let Target::Forward(d) = choose_target(&ai(), &mut Rand::new(1), &s) else { panic!() };
        assert!(d[0] > 0.0 && d[2] < 0.0);
        assert!(((d[0] * d[0] + d[2] * d[2]).sqrt() - 1.0).abs() < 1e-12);
    }
}
