//! Shots (`docs/GAMEPLAY.md`, "Shooting, saves and tackles"): the hoops, the goal area, the AI's wind-up and hoop
//! choice in the team-options routine `0x4568a0`, the shot's speed and flight time (`0x43f140`) and the contact radii
//! of the catch test (`0x45ff90`).
//!
//! No `rand()` decides whether a shot scores: every shot schedules its goal, and only an opposing chaser or keeper
//! touching the ball in flight stops it (see [`crate::world`]). The draws here are the wind-up start, its length and
//! one hoop draw per wind-up tick, in that order.

use crate::rng::{trunc, Rand};
use crate::vec3::{dot, len, sub, unit, Vec3};

/// `Pitch.GoalOffset` (`0x6060dc`) and `Pitch.GoalRadius` (`0x6060e0`) image defaults.
pub const GOAL_OFFSET: f64 = 150.0;
pub const GOAL_RADIUS: f64 = 84.0;
/// The goal circle's scale inside which a carrier always starts a wind-up (`0x668e34`).
pub const INNER_SCALE: f64 = 0.6;
/// Outside it, the chance per tick (`0x668e30`) and the `|z|` limit (`0x668e2c`).
pub const LONG_SHOT_PROB: f64 = 0.1;
pub const LONG_SHOT_Z: f64 = 20.0;
/// Contact radius of the catch test (`0x605f0c`), halved while a pass is in flight (Quaffle states 5, 6, 7).
pub const CONTACT_RADIUS: f64 = 1.5;

/// One hoop of the game's hoop table (game `+0x0c`, six records of `0x1c` bytes: `x, y, z, w`, owner team).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hoop {
    pub pos: Vec3,
    pub team: usize,
}

/// Half the distance between the hoops' planes: `97.5 * Pitch.ScaleGoals` (`0x5bf118`; 146.25 in the challenge
/// mode 4, not modelled).
pub fn goal_line(scale_goals: f64) -> f64 {
    97.5 * scale_goals
}

/// The hoop table as the match game builds it (`0x4508b0`, traced). For each team `t` (attack direction `a`), at its
/// own end `x = -a * 97.5 * ScaleGoals`, in this order: `z = 7.5 a` at height `h + 3.75`, `z = 0` at `h + 7.5`, and
/// `z = -7.5 a` at `h`. `h` is `goal_height`, `Pitch.GoalHeight`: the global `0x6685f4` is the pitch's `+0x12604`
/// (the pitch is the global at `0x655ff0`), which the command's handler `0x41e8a0` sets before it rebuilds the table.
/// The stadiums set it in their `game/match.cli` (20 for the front end's).
pub fn hoops(scale_goals: f64, goal_height: f64) -> [Hoop; 6] {
    let h = goal_height;
    let l = goal_line(scale_goals);
    let team = |t: usize| {
        let a = if t == 0 { -1.0 } else { 1.0 };
        [
            Hoop { pos: [-a * l, h + 3.75, 7.5 * a], team: t },
            Hoop { pos: [-a * l, h + 7.5, 0.0], team: t },
            Hoop { pos: [-a * l, h, -7.5 * a], team: t },
        ]
    };
    let (a, b) = (team(0), team(1));
    [a[0], a[1], a[2], b[0], b[1], b[2]]
}

/// The goal-area test (game vtable `+0x98`, `0x450d00`, traced): `pos` lies strictly inside the circle in `x, z`
/// of radius `GoalRadius * ScaleGoals * scale` round `x = s * GoalOffset * ScaleGoals`, `s = +1` for `team` 0 and
/// `-1` for team 1. Callers pass the defending team, so the circle is round the end being attacked.
pub fn in_goal_area(pos: Vec3, team: usize, scale_goals: f64, scale: f64) -> bool {
    let s = if team == 0 { 1.0 } else { -1.0 };
    let dx = pos[0] - s * scale_goals * GOAL_OFFSET;
    let r = GOAL_RADIUS * scale_goals * scale;
    r * r > dx * dx + pos[2] * pos[2]
}

/// Whether a carrier in the goal area with no wind-up starts one this tick (`0x4568a0`, step 3.2): always inside
/// the 0.6 circle (no draw), else on `U() < 0.1` with `|z| < 20` (the draw comes first).
pub fn starts_wind_up(rng: &mut Rand, inner: bool, carrier_z: f64) -> bool {
    inner || (rng.unit() < LONG_SHOT_PROB && carrier_z.abs() < LONG_SHOT_Z)
}

/// The wind-up length `N` in ticks (step 3.3): with `T = f32(24 s)`, `clamp(trunc(U() * (24 - T) + T + 1), 2, 24)`.
/// `s` is `AI.ShootingSkill` (team `+0x28`).
pub fn wind_up_ticks(rng: &mut Rand, skill: f32) -> i32 {
    let t = (24.0 * skill) as f64;
    trunc(rng.unit() * (24.0 - t) + t + 1.0).clamp(2, 24)
}

/// The hoop aimed at on a wind-up tick (step 5): each opposing hoop scores `1 - dot(unit(K - c), unit(h - c))` (3D),
/// high when it is far, in angle, from the keeper `K`. One `rand() / 32767` picks the best with probability `s^2`,
/// the middle one with `2 s (1 - s)`, else the worst. `hoops` are `(index, position)` of the keeper's team; ties keep
/// the earlier hoop. No draw without a keeper or a hoop (the caller then aims straight down the pitch).
pub fn pick_hoop(
    rng: &mut Rand,
    skill: f32,
    carrier: Vec3,
    keeper: Option<Vec3>,
    hoops: &[(usize, Vec3)],
) -> Option<usize> {
    let k = keeper?;
    if hoops.is_empty() {
        return None;
    }
    let to_k = unit(sub(k, carrier));
    let scores: Vec<f64> = hoops.iter().map(|&(_, h)| 1.0 - dot(to_k, unit(sub(h, carrier)))).collect();
    let (mut best, mut worst) = (None, None);
    let (mut hi, mut lo) = (-999.0, 999.0);
    for (i, &s) in scores.iter().enumerate() {
        if s > hi {
            hi = s;
            best = Some(i);
        }
        if s < lo {
            lo = s;
            worst = Some(i);
        }
    }
    let (best, worst) = (best.unwrap_or(0), worst.unwrap_or(0));
    // with fewer than three distinct hoops there is no middle one; the best stands in (inferred)
    let mid = (0..hoops.len()).find(|&i| i != best && i != worst).unwrap_or(best);
    let s = skill as f64;
    let r = rng.rand() as f64 / 32767.0;
    let pick = if r < s * s {
        best
    } else if r < s * s + 2.0 * s * (1.0 - s) {
        mid
    } else {
        worst
    };
    Some(hoops[pick].0)
}

/// `0x441640`: the hoop among `hoops` whose bearing from `from` in `x, z` is nearest to `d` (the largest cosine;
/// the measure is inferred). In the AI's path it is the hoop just picked.
pub fn nearest_bearing(from: Vec3, d: Vec3, hoops: &[(usize, Vec3)]) -> Option<usize> {
    let d = unit([d[0], 0.0, d[2]]);
    let mut best: Option<(f64, usize)> = None;
    for &(i, h) in hoops {
        let c = dot(d, unit([h[0] - from[0], 0.0, h[2] - from[2]]));
        if best.is_none_or(|(b, _)| c > b) {
            best = Some((c, i));
        }
    }
    best.map(|(_, i)| i)
}

/// Shot speed in units per second for power `f` (`0x43f140`; `0x5a6130` = 20, `0x5a6134` = 30).
pub fn shot_speed(f: f64) -> f64 {
    20.0 + (30.0 - 20.0) * f
}

/// Shot flight in ticks over distance `d` (3D, carrier to hoop centre) at power `f`: `max(5, trunc(d / v * 24))`.
pub fn shot_ticks(d: f64, f: f64) -> i32 {
    trunc(d / shot_speed(f) * 24.0).max(5)
}

/// Contact radius for the Quaffle in state `state` (`0x45ff90`).
pub fn contact_radius(state: u8) -> f64 {
    if matches!(state, 5..=7) {
        CONTACT_RADIUS * 0.5
    } else {
        CONTACT_RADIUS
    }
}

/// Whether the catch handler (`0x431510`) takes the ball from a touch in this Quaffle state: loose (0), a pass in
/// flight (6) or a shot in flight (9).
pub fn catchable(state: u8) -> bool {
    matches!(state, 0 | 6 | 9)
}

/// Ticks for a catching player to take the ball (`0x431510`): `max(1, trunc(d / 10 * 24))`.
pub fn catch_ticks(d: f64) -> i32 {
    trunc(d / 10.0 * 24.0).max(1)
}

/// The 3D distance helper, re-exported for callers measuring contacts.
pub fn distance(a: Vec3, b: Vec3) -> f64 {
    len(sub(a, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hoop_table() {
        let h = hoops(1.0, 0.0);
        // team 0 attacks -x, so its own hoops are at +97.5, the left one (z = -7.5) at mid height
        assert_eq!(h[0], Hoop { pos: [97.5, 3.75, -7.5], team: 0 });
        assert_eq!(h[1], Hoop { pos: [97.5, 7.5, 0.0], team: 0 });
        assert_eq!(h[2], Hoop { pos: [97.5, 0.0, 7.5], team: 0 });
        assert_eq!(h[3], Hoop { pos: [-97.5, 3.75, 7.5], team: 1 });
        assert_eq!(hoops(2.0, 0.0)[5].pos, [-195.0, 0.0, -7.5]);
        // raised by the goal height
        assert_eq!(hoops(1.0, 20.0)[1].pos, [97.5, 27.5, 0.0]);
    }

    #[test]
    fn goal_area() {
        // team 0 attacks -x, so it shoots in team 1's area round x = -150
        assert!(in_goal_area([-70.0, 0.0, 0.0], 1, 1.0, 1.0));
        assert!(!in_goal_area([-66.0, 0.0, 0.0], 1, 1.0, 1.0)); // on the circle: strict
        assert!(!in_goal_area([-70.0, 0.0, 0.0], 0, 1.0, 1.0));
        assert!(!in_goal_area([-99.0, 0.0, 0.0], 1, 1.0, 0.6)); // inner circle reaches x = -99.6
        assert!(in_goal_area([-100.0, 0.0, 0.0], 1, 1.0, 0.6));
        assert!(!in_goal_area([-150.0, 0.0, 84.0], 1, 1.0, 1.0));
    }

    #[test]
    fn wind_up_length() {
        // srand(1): rand() = 41, so U = 41 / 32767
        let u = 41.0 / 32767.0;
        let mut r = Rand::new(1);
        // s = 0.75: T = 18, trunc(U * 6 + 19) = 19
        assert_eq!(wind_up_ticks(&mut r, 0.75), trunc(u * 6.0 + 19.0));
        assert_eq!(trunc(u * 6.0 + 19.0), 19);
        // s = 1: T = 24, trunc(25) clamps to 24; s = 0: trunc(U * 24 + 1) = 1 clamps to 2
        assert_eq!(wind_up_ticks(&mut Rand::new(1), 1.0), 24);
        assert_eq!(wind_up_ticks(&mut Rand::new(1), 0.0), 2);
    }

    #[test]
    fn long_shot_draw() {
        let mut r = Rand::new(1);
        assert!(starts_wind_up(&mut r, true, 50.0));
        assert_eq!(r, Rand::new(1)); // no draw inside the inner circle
                                     // U = 41 / 32767 < 0.1, so only |z| decides
        assert!(starts_wind_up(&mut Rand::new(1), false, 19.0));
        assert!(!starts_wind_up(&mut Rand::new(1), false, 20.0));
        // rand() after one draw from srand(1) is 18467: U = 0.56 >= 0.1
        let mut r = Rand::new(1);
        r.rand();
        assert!(!starts_wind_up(&mut r, false, 0.0));
    }

    #[test]
    fn hoop_choice() {
        // carrier on the axis at the middle hoop's height, keeper in front of hoop 4: hoop 4 scores 0 (worst); hoop 5
        // is 7.5 below and wider in angle than hoop 3 (3.75 below), so 5 is the best and 3 the middle
        let hs: Vec<(usize, Vec3)> = hoops(1.0, 0.0)[3..].iter().enumerate().map(|(i, h)| (i + 3, h.pos)).collect();
        let carrier = [-70.0, 7.5, 0.0];
        let keeper = Some([-92.5, 7.5, 0.0]);
        // r = 41 / 32767 < s^2 for s = 0.5: best
        assert_eq!(pick_hoop(&mut Rand::new(1), 0.5, carrier, keeper, &hs), Some(5));
        // r = 18467 / 32767 = 0.564: s = 0.5 gives best below 0.25, middle below 0.75
        let mut r = Rand::new(1);
        r.rand();
        assert_eq!(pick_hoop(&mut r, 0.5, carrier, keeper, &hs), Some(3));
        // s = 0: always the worst
        assert_eq!(pick_hoop(&mut Rand::new(1), 0.0, carrier, keeper, &hs), Some(4));
        let mut r = Rand::new(1);
        assert_eq!(pick_hoop(&mut r, 0.5, carrier, None, &hs), None);
        assert_eq!(r, Rand::new(1));
        assert_eq!(nearest_bearing(carrier, sub(hs[2].1, carrier), &hs), Some(5));
    }

    #[test]
    fn shot_flight() {
        assert_eq!(shot_speed(0.0), 20.0);
        assert_eq!(shot_speed(1.0), 30.0);
        // 30 units at power 0.5 (25 u/s): 1.2 s = 28.8 ticks
        assert_eq!(shot_ticks(30.0, 0.5), 28);
        assert_eq!(shot_ticks(1.0, 0.5), 5);
        assert_eq!(contact_radius(6), 0.75);
        assert_eq!(contact_radius(9), 1.5);
        assert!(catchable(9) && catchable(6) && catchable(0));
        assert!(!catchable(8) && !catchable(10) && !catchable(5) && !catchable(7));
        assert_eq!(catch_ticks(1.4), 3);
        assert_eq!(catch_ticks(0.1), 1);
    }
}
