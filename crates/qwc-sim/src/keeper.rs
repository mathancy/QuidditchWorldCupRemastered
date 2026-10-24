//! The keeper off the ball (`0x43f380`, `docs/GAMEPLAY.md`, "Keeper positioning" and "Player movement") and the
//! player mover's Bezier step (`0x4636c0` through `0x421780`) that carries out its movement commands.
//!
//! The keeper is a goal-keeper only by position: he guards a point 5 units in front of the hoop he picks and, when
//! the ball is in flight towards that hoop, matches its predicted height. A save is his body touching the ball
//! (see [`crate::shooting::CONTACT_RADIUS`]). No `rand()`.

use crate::rng::trunc;
use crate::vec3::{add, dot, flat, len, scale, sub, unit, Vec3};

/// Distance of the guard point in front of the hoop (`0x5bf0a0`).
pub const GUARD: f64 = 5.0;
/// Extra divisor ticks while switching hoops (`0x668f84`) and the divisor once settled.
pub const SWITCH_TICKS: i32 = 5;
pub const SETTLED_DIVISOR: i32 = 3;
/// Divisor of the keeper's drift back to his hoops while his own team has the ball.
pub const HOME_DIVISOR: i32 = 24;

/// What the keeper sees of the Quaffle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BallView {
    pub pos: Vec3,
    /// Units per second.
    pub vel: Vec3,
    /// Whether a player holds it (the Quaffle's `+0x70`).
    pub held: bool,
    /// The holder's facing (`+0x50`), used for the direction while held.
    pub facing: Vec3,
}

/// The guard point while the opponents have the ball (traced). `own` are the keeper's hoops as
/// `(table index, position)`; `mode` is `Keeper.Mode` (team `+0x30`); `attack_dir` the keeper's team's and
/// `opponent_attack_dir` the other's; `goal_line` is `97.5 * ScaleGoals`. Returns the hoop index and the point.
pub fn guard_point(
    own: &[(usize, Vec3)],
    ball: &BallView,
    mode: i32,
    attack_dir: f64,
    opponent_attack_dir: f64,
    goal_line: f64,
) -> (usize, Vec3) {
    // 1. the ball's direction, flattened
    let mut u = flat(if ball.held { ball.facing } else { ball.vel });
    if dot(u, u) < 0.0001 {
        u = [opponent_attack_dir, 0.0, 0.0];
    }
    let u = unit(u);
    // 2. the hoop with the lowest score; quirk: index 0 means "none", so table hoop 0 is always replaced by the next
    let mut best = 0;
    let mut best_score = 0.0;
    for &(i, h) in own {
        let v = flat(sub(h, ball.pos));
        let d2 = dot(v, v);
        let score = if mode == 1 || (mode == 2 && ball.held) || d2 < 1.0 { d2 } else { -dot(unit(v), u) };
        if best == 0 || score < best_score {
            best = i;
            best_score = score;
        }
    }
    let hoop = own.iter().find(|&&(i, _)| i == best).map_or([0.0; 3], |&(_, h)| h);
    // 3. 5 units in front of the hoop, towards the ball
    let to_hoop = sub(hoop, ball.pos);
    let dir = if dot(to_hoop, to_hoop) < 1.0 { [attack_dir, 0.0, 0.0] } else { unit(to_hoop) };
    let mut p = sub(hoop, scale(dir, GUARD));
    // 4. a ball in flight: its height where it crosses the plane 5 units in front of the hoop, if near the hoop's
    if !ball.held && ball.vel[0].abs() > 0.1 {
        let plane = hoop[0] - if hoop[0] > 0.0 { GUARD } else { -GUARD };
        let t = (plane - ball.pos[0]) / ball.vel[0];
        if t > 0.0 && t < 5.0 {
            let y = ball.pos[1] + ball.vel[1] * t - 6.0 * t * t;
            if (y - hoop[1]).abs() < 3.0 {
                p[1] = y;
            }
        }
    }
    // 5. never behind the hoops' plane
    let limit = goal_line.abs() - 1.0;
    p[0] = p[0].clamp(-limit, limit);
    (best, p)
}

/// Where the keeper drifts while his own team has the ball: the mean of his hoops plus 5 towards the attacked end.
pub fn home_point(own: &[(usize, Vec3)], attack_dir: f64) -> Vec3 {
    let n = own.len().max(1) as f64;
    let mean = own.iter().fold([0.0; 3], |a, &(_, h)| add(a, h));
    add(scale(mean, 1.0 / n), [GUARD * attack_dir, 0.0, 0.0])
}

/// The hoop-switch bookkeeping (team `+0x34` settled hoop, `+0x38` chosen hoop, `+0x3c` countdown).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settle {
    pub settled: i32,
    pub chosen: i32,
    pub count: i32,
}

impl Default for Settle {
    fn default() -> Settle {
        Settle { settled: -1, chosen: -1, count: 0 }
    }
}

impl Settle {
    /// The movement divisor for this tick (traced at `0x43fa1a`-`0x43fb1e`): `n = max(1, trunc(20 d / Keeper.Speed))`
    /// with `d` the distance to the guard point. A new hoop unsettles the keeper; he settles once `n - 1 <= 0`, i.e.
    /// within `Keeper.Speed / 20` units of the point. Settled the divisor is 3, else `n + 5`.
    pub fn divisor(&mut self, hoop: usize, distance: f64, speed: f64) -> i32 {
        let n = trunc(distance / speed * 20.0).max(1);
        self.count = n;
        self.chosen = hoop as i32;
        if self.chosen != self.settled {
            self.settled = -1;
            self.count -= 1;
            if self.count <= 0 {
                self.settled = self.chosen;
            }
        }
        if self.settled == self.chosen {
            SETTLED_DIVISOR
        } else {
            n + SWITCH_TICKS
        }
    }
}

/// One tick of a one-tick movement command with progress divisor `n` (cmd `+0xe4`): the mover evaluates the cubic
/// Bezier from the current state `(p0, v0)` to the target `(p3, v3)` over `D = n / 24` seconds at `f = 1 / n`
/// (`P1 = P0 + V0 D / 3`, `P2 = P3 - V3 D / 3`) and returns the new position and velocity `B'(f) / D`. So each tick
/// covers about `1/n` of the remaining way, with no speed cap.
pub fn bezier_step(p0: Vec3, v0: Vec3, p3: Vec3, v3: Vec3, n: i32) -> (Vec3, Vec3) {
    let n = n.max(1) as f64;
    bezier(p0, v0, p3, v3, n / 24.0, 1.0 / n)
}

/// The mover's cubic (`0x421780`) from `(p0, v0)` to `(p3, v3)` over `d` seconds, at progress `f`: position and
/// velocity.
pub fn bezier(p0: Vec3, v0: Vec3, p3: Vec3, v3: Vec3, d: f64, f: f64) -> (Vec3, Vec3) {
    let p1 = add(p0, scale(v0, d / 3.0));
    let p2 = sub(p3, scale(v3, d / 3.0));
    let g = 1.0 - f;
    let pos = add(
        add(scale(p0, g * g * g), scale(p1, 3.0 * g * g * f)),
        add(scale(p2, 3.0 * g * f * f), scale(p3, f * f * f)),
    );
    let deriv =
        add(add(scale(sub(p1, p0), 3.0 * g * g), scale(sub(p2, p1), 6.0 * g * f)), scale(sub(p3, p2), 3.0 * f * f));
    (pos, scale(deriv, 1.0 / d))
}

/// Distance from the keeper to his target, for [`Settle::divisor`].
pub fn distance(a: Vec3, b: Vec3) -> f64 {
    len(sub(a, b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shooting::hoops;

    fn own(team: usize) -> Vec<(usize, Vec3)> {
        hoops(1.0, 0.0).iter().enumerate().filter(|(_, h)| h.team == team).map(|(i, h)| (i, h.pos)).collect()
    }

    #[test]
    fn guards_the_hoop_the_ball_heads_for() {
        // team 1 keeper (hoops at x = -97.5), a carrier at (-60, 0, -20) flying towards -x
        let ball = BallView { pos: [-60.0, 0.0, -20.0], vel: [-10.0, 0.0, 0.0], held: false, facing: [0.0; 3] };
        let (h, p) = guard_point(&own(1), &ball, 0, 1.0, -1.0, 97.5);
        // hoop 5 (z = -7.5) is most nearly in the ball's direction
        assert_eq!(h, 5);
        let hoop = hoops(1.0, 0.0)[5].pos;
        assert!((len(sub(hoop, p)) - 5.0).abs() < 1e-9);
        // mode 1: the nearest hoop in x, z, which is also hoop 5
        assert_eq!(guard_point(&own(1), &ball, 1, 1.0, -1.0, 97.5).0, 5);
    }

    #[test]
    fn hoop_zero_quirk() {
        // team 0: the ball right in front of hoop 0 still yields hoop 1 or 2, never 0
        let ball = BallView { pos: [80.0, 3.75, -7.5], vel: [0.0; 3], held: true, facing: [1.0, 0.0, 0.0] };
        let (h, _) = guard_point(&own(0), &ball, 1, -1.0, 1.0, 97.5);
        assert_eq!(h, 1);
    }

    #[test]
    fn matches_a_shot_height() {
        // a shot at hoop 4 (y 7.5) crossing x = -92.5 in 0.5 s at y = 2 + 12 * 0.5 - 6 * 0.25 = 6.5
        let ball = BallView { pos: [-72.5, 2.0, 0.0], vel: [-40.0, 12.0, 0.0], held: false, facing: [0.0; 3] };
        let (h, p) = guard_point(&own(1), &ball, 0, 1.0, -1.0, 97.5);
        assert_eq!(h, 4);
        assert!((p[1] - 6.5).abs() < 1e-9);
        assert!(p[0] > -96.5 && p[0] < -92.0);
    }

    #[test]
    fn settles() {
        let mut s = Settle::default();
        // far from the point with Keeper.Speed 5: n = trunc(20 * 10 / 5) = 40, switching: 45
        assert_eq!(s.divisor(4, 10.0, 5.0), 45);
        assert_eq!(s.settled, -1);
        // within 0.25 units: n = 1, settles at once
        assert_eq!(s.divisor(4, 0.2, 5.0), 3);
        assert_eq!(s.divisor(4, 10.0, 5.0), 3);
        // a new hoop unsettles
        assert_eq!(s.divisor(3, 10.0, 5.0), 45);
    }

    #[test]
    fn bezier() {
        // one tick with divisor 1 lands on the target with its velocity
        let (p, v) = bezier_step([0.0; 3], [5.0, 0.0, 0.0], [1.0, 2.0, 3.0], [0.0, 1.0, 0.0], 1);
        assert!(len(sub(p, [1.0, 2.0, 3.0])) < 1e-12);
        assert!(len(sub(v, [0.0, 1.0, 0.0])) < 1e-12);
        // from rest towards a target at rest, divisor 3: B(1/3) = 3 (2/3)(1/3)^2 + (1/3)^3 = 7/27 of the way
        let (p, _) = bezier_step([0.0; 3], [0.0; 3], [27.0, 0.0, 0.0], [0.0; 3], 3);
        assert!((p[0] - 7.0).abs() < 1e-9);
    }
}
