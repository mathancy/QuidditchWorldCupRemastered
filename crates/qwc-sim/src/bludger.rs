//! Beaters and Bludgers (`docs/GAMEPLAY.md`, "Beaters and Bludgers"; traced 2026-10-20, **[T]** unless marked).
//!
//! Each team owns one Bludger (game vtable `+0x64(team)`) and two beaters. Off the move a Bludger flies a formotion
//! of the match database (`0x430d90`): `BludgerIdle` round a point over the middle of the pitch, or, once its team's
//! Bludger move is ready, `BludgerDefend` round the team's active player. The beaters fly `BeaterIdle`,
//! `BeaterAttack` (the Bludger ready) or `BeaterDefend` (the opponents' Bludger coming) round the active player
//! (`0x441090`, `0x440620`).
//!
//! The Bludger move (team state 5, `0x43c3c0`): when the opponents hold the Quaffle, the team's tackle recharge has
//! run out and its Bludger sub-bar is full (`0x442a20`, `0x435060`), the beater nearer the Bludger summons it for
//! `Bludger.SummonPeriod` ticks (`0x43c610`, `0x4228c0`), then bats it at the opposing active player (`0x43c750`).
//! It flies with a homing pull and the steer's input (`0x43cc20`), guided along a curve at first; within a unit of
//! the carrier it hits him (`0x43da40`): he is knocked back for two seconds and drops the Quaffle (`0x442110`,
//! `0x424080`). Meanwhile the team's active player closes to five units of the carrier (`0x4389a0`). The human
//! steers with the stick (player sub-states 12 and 11 of the defence); the AI picks the move by its weights
//! (`0x455570`) and steers with a lead (state 0x27, `0x453420`). An opposing beater within `Beater.block_dist` of
//! the Bludger, once it has been coming for 48 ticks, bats it away and ends the move (`0x4405c0`, `0x423880`).

use crate::console::Console;
use crate::fmath::exp;
use crate::formation::Runner;
use crate::keeper::bezier;
use crate::vec3::{add, len, scale, sub, unit, Vec3};
use crate::world::Command;

/// `0x606604`, `0x668fbc`: the drag starts at `1 + 2` times `Bludger.drag` and eases to it over 24 ticks.
pub const DRAG_RAMP_TICKS: i32 = 24;
pub const DRAG_RAMP: f64 = 2.0;
/// `0x668fc4`: the homing pull is `(|d| * 0.2)^2` towards the aim, at most 5 (`0x43cc20`).
pub const HOMING_K: f64 = 0.2;
pub const HOMING_MAX: f64 = 5.0;
/// `0x668fc0`: without the homing pull the input counts twice.
pub const CONTROL_SCALE: f64 = 2.0;
/// `0x606664`: within this in `x` of the aim the side offset starts to fade (by 0.1 a tick).
pub const CLOSE_X: f64 = 4.0;
/// `0x668fb8`: the Bludger's step cap, 50 units a second.
pub const MAX_STEP: f64 = 50.0 / 24.0;
/// `0x60666c`, `0x606678`: ticks of the guided curve after the launch, in a match and in a challenge.
pub const GUIDED_TICKS: i32 = 48;
pub const GUIDED_TICKS_CHALLENGE: i32 = 96;
/// `0x668fb4`: the guided curve's first part (to above the carrier) lasts 30 ticks.
pub const CURVE_SPLIT: i32 = 30;
/// `0x606670`, `0x606674` (match) and `0x60667c`, `0x606680` (challenge): the curve's rise above the carrier and
/// its end ahead of him.
pub const CURVE_RISE: f64 = 5.0;
pub const CURVE_AHEAD: f64 = 5.0;
/// `0x668fb0`: the curve's weight is `1 - exp(-2 |1 - progress|)`.
pub const CURVE_BLEND: f64 = 2.0;
/// `0x668fcc`, `0x668fc8`, `0x668fd0`: the launch at twice the carrier's ground velocity, a step of `4 / 24 * 2`
/// towards him.
pub const LAUNCH_SPEED: f64 = 2.0;
pub const LAUNCH_STEP: f64 = 2.0;
pub const LAUNCH_TICKS: f64 = 4.0;
/// With no carrier the launch is along the summoner's facing, 8 faster than him.
pub const LAUNCH_FREE: f64 = 8.0;
/// `0x668fa8`: the hit radius.
pub const HIT_RADIUS: f64 = 1.0;
/// `0x66912c`: the summoner swings this many ticks before the launch.
pub const SWING_BEFORE: i32 = 10;
/// `0x6066e4`, `0x6066e8`, team `+0xa98` (`0x6066d8`): the active player closes on the carrier by a fifth of the
/// gap beyond 5 units a tick, at most 2.
pub const PURSUIT_RATE: f64 = 0.2;
pub const PURSUIT_MAX: f64 = 2.0;
pub const PURSUIT_DIST: f64 = 5.0;
/// `0x668e94`: the Bludger flies at the Quaffle's height less 0.9 (team vtable `+0x6c`, `0x452fe0`).
pub const BELOW_BALL: f64 = 0.9;
/// `0x668f80`: the block counter (two a tick while the opponents' Bludger is out) must pass this.
pub const BLOCK_AFTER: i32 = 96;
/// `0x668f7c`: a beater's bat (`0x423880`) takes 10 ticks; the Bludger leaves at 5 units a second (`0x66910c`).
pub const BAT_TICKS: i32 = 10;
pub const BAT_SPEED: f64 = 5.0;
/// `0x6690f8`, `0x6690f4`: a hit player flies 48 ticks along half his and the Bludger's velocities.
pub const KNOCK_TICKS: i32 = 48;
pub const KNOCK_SCALE: f64 = 0.5;
/// `0x669020`, `0x668f78`: the Bludgers' and the beaters' formotion step caps (units a second).
pub const BLUDGER_CAP: f64 = 100.0;
pub const BEATER_CAP: f64 = 40.0;
/// The formotion frames: a Bludger's follows its team's active player at 25 units a second when ready, else the
/// point `(0, 10, 0)` at 10; the beaters' follow at 25 (`0x42ca90` caps).
pub const FOLLOW_READY: f64 = 25.0;
pub const FOLLOW_IDLE: f64 = 10.0;
pub const IDLE_POINT: Vec3 = [0.0, 10.0, 0.0];
/// The formotions' scales (`0x42c930`): the Bludgers' `(40, 30, 40)`, the beaters' `(20, 10, 20)` (`(10, 5, 10)`
/// when blocking); the forward scale is 1.
pub const BLUDGER_SCALE: Vec3 = [40.0, 30.0, 40.0];
pub const BEATER_SCALE: Vec3 = [20.0, 10.0, 20.0];
pub const BEATER_BLOCK_SCALE: Vec3 = [10.0, 5.0, 10.0];
/// The AI's steer (`0x453420`): homing stops when the Bludger is 5 behind the carrier along his attack and starts
/// again 5 ahead (`0x668e84`, `0x668e80`); off it, it swings away within 5 across (`0x668e7c`) and back beyond 8
/// (`0x668e78`).
pub const AI_BEHIND: f64 = 5.0;
pub const AI_AHEAD: f64 = 5.0;
pub const AI_NEAR: f64 = 5.0;
pub const AI_FAR: f64 = 8.0;
/// `0x606080`: the AI's defence choice is drawn again every 100 ticks.
pub const CHOICE_TICKS: i32 = 100;
/// **[I]** The summon's end: the summoner's bat, a unit ahead and half a unit up.
pub const BAT_REACH: f64 = 1.0;
pub const BAT_RISE: f64 = 0.5;

/// A Bludger's state (its entity's `+0x1c`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum State {
    /// 13: the idle formotion over the middle.
    #[default]
    Idle,
    /// 14: its team's move is ready: the formotion round the active player.
    Ready,
    /// 15: being summoned.
    Summoned,
    /// 16: in flight.
    Flying,
}

impl State {
    pub fn code(self) -> u32 {
        match self {
            State::Idle => 13,
            State::Ready => 14,
            State::Summoned => 15,
            State::Flying => 16,
        }
    }
}

/// A Bludger.
#[derive(Debug, Clone, Default)]
pub struct Bludger {
    pub pos: Vec3,
    /// Units a second.
    pub vel: Vec3,
    pub state: State,
    /// Its formotion block (`0x102bec0 + team * 0x420`).
    pub runner: Runner,
    /// A pending movement command (the bounce off a hit, a beater's bat); while it runs no formotion moves it.
    pub cmd: Option<Command>,
    /// State 13 once `cmd` ends (the bat's `0x421900(.., 0xd)`).
    pub idle_after: bool,
    /// In this game (a challenge has only its description's balls).
    pub present: bool,
}

/// A team's Bludger move (team state 5; the fields from `+0x13c4`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Move {
    /// `+0x1430`: the summoning beater.
    pub summoner: usize,
    /// `+0x13cc`: the summon's ticks.
    pub tick: i32,
    /// `+0x1438` set: launched.
    pub flying: bool,
    /// The Bludger's position and velocity when the summon began.
    pub from: (Vec3, Vec3),
    /// `+0x13c4`: the side offset's weight, 1 at the start (`0x606844`).
    pub ramp: f64,
    /// `+0x13c8`: ticks since the launch, for the drag ramp.
    pub since: i32,
    /// `+0x13d0`: ticks of the guided curve left, of `guided_total`.
    pub guided: i32,
    pub guided_total: i32,
    /// `+0x13e0`: the Bludger's state at the guided curve's start.
    pub launch: (Vec3, Vec3),
    /// A human steers it (sub-states 12 and 11), else the AI (states 0x1f, 0x27).
    pub human: bool,
    /// The AI's homing flag (state 0x27 `+0x10`, `0x668e88` = 1 at its start).
    pub ai_homing: bool,
}

impl Move {
    pub fn new(summoner: usize, pos: Vec3, vel: Vec3, human: bool) -> Move {
        Move {
            summoner,
            tick: 0,
            flying: false,
            from: (pos, vel),
            ramp: 1.0,
            since: 0,
            guided: 0,
            guided_total: GUIDED_TICKS,
            launch: (pos, vel),
            human,
            ai_homing: true,
        }
    }
}

/// The console values the Bludgers read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rules {
    /// `Bludger.acc_control` (`0x655ec4`), `Bludger.drag` (`0x655ec8`), `Bludger.max_dist` (`0x655ecc`: the side
    /// offset is three quarters of it), `Bludger.SummonPeriod` (`0x606668`), `Beater.block_dist` (`0x655ed4`).
    pub acc_control: f64,
    pub drag: f64,
    pub max_dist: f64,
    pub summon_period: i32,
    pub block_dist: f64,
    /// `TeamOPs.ChaseWeight`, `SpecialMoveWeight`, `BludgerWeight` (`0x60607c`, `0xf918f8`, `0xf918fc`; globals, so
    /// the last team file's win).
    pub weights: [f64; 3],
    /// `SeekerPowerBar.BludgerPenalty` (team `+0x7c`): what a hit costs the victim's bar, the attacker's value.
    pub penalty: [f64; 2],
}

impl Rules {
    pub fn from_console(c: &Console) -> Rules {
        let or = |v: f64, d: f64| if v == 0.0 { d } else { v };
        Rules {
            acc_control: or(c.num("Bludger.acc_control", 0), 2.5),
            drag: or(c.num("Bludger.drag", 0), 0.06),
            max_dist: or(c.num("Bludger.max_dist", 0), 10.0),
            summon_period: or(c.num("Bludger.SummonPeriod", 0), 24.0) as i32,
            block_dist: or(c.num("Beater.block_dist", 0), 1.5),
            weights: [
                c.num("TeamOPs.ChaseWeight", 0),
                c.num("TeamOPs.SpecialMoveWeight", 0),
                c.num("TeamOPs.BludgerWeight", 0),
            ],
            penalty: [0, 1].map(|t| c.num("SeekerPowerBar.BludgerPenalty", t)),
        }
    }
}

/// What the Bludger flies at: the opposing active player's position and velocity, his team's attack direction
/// (`-1` team 0) and the Quaffle's height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aim {
    pub pos: Vec3,
    pub vel: Vec3,
    pub dir: f64,
    pub ball_y: f64,
}

/// `0x4228c0`, reduced (**[I]**: its curve block is not read): the summoned Bludger's state at `mv.tick` of
/// `period`, on a cubic from its state at the summon's start to the summoner's bat (`beater`: position, velocity,
/// facing).
pub fn summon_point(mv: &Move, beater: (Vec3, Vec3, Vec3), period: i32) -> (Vec3, Vec3) {
    let bat = add(add(beater.0, scale(beater.2, BAT_REACH)), [0.0, BAT_RISE, 0.0]);
    let span = period.max(1) as f64;
    let f = (mv.tick as f64 / span).clamp(0.0, 1.0);
    bezier(mv.from.0, mv.from.1, bat, beater.1, span / 24.0, f)
}

/// The launch (`0x43c750`): with no carrier, along the summoner's facing 8 faster than him, not guided; else at
/// twice the carrier's ground velocity from a step towards him, guided for 48 ticks (96 in a challenge). The
/// summoner is `(velocity, facing)`; returns the Bludger's position and velocity.
pub fn launch(
    mv: &mut Move,
    pos: Vec3,
    summoner: (Vec3, Vec3),
    carrier: Option<(Vec3, Vec3)>,
    challenge: bool,
) -> (Vec3, Vec3) {
    mv.flying = true;
    mv.since = 0;
    mv.tick = 0;
    match carrier {
        None => {
            let vel = add(summoner.0, scale(summoner.1, LAUNCH_FREE));
            mv.guided = 0;
            (add(pos, scale(vel, LAUNCH_TICKS / 24.0)), vel)
        }
        Some((cp, cv)) => {
            let vel = [cv[0] * LAUNCH_SPEED, 0.0, cv[2] * LAUNCH_SPEED];
            let p = add(pos, scale(unit(sub(cp, pos)), LAUNCH_TICKS / 24.0 * LAUNCH_STEP));
            mv.guided_total = if challenge { GUIDED_TICKS_CHALLENGE } else { GUIDED_TICKS };
            mv.guided = mv.guided_total;
            (p, vel)
        }
    }
}

/// One tick of the flying Bludger (`0x43cc20`): the input (`acc_control` times it, plus the homing pull when
/// `homing`, else twice it) and the velocity, damped by the drag, move it; its height eases to below the Quaffle;
/// its step is capped; while guided it is pulled onto a curve over the carrier. Returns the new position and
/// velocity.
pub fn steer(mv: &mut Move, rules: &Rules, pos: Vec3, vel: Vec3, aim: &Aim, input: Vec3, homing: bool) -> (Vec3, Vec3) {
    let target = aim.pos;
    // the side offset: three quarters of `max_dist` on the Bludger's side of the carrier, fading once close in x
    let mut off = rules.max_dist * 0.75;
    if off > 0.0 && pos[2] < target[2] {
        off = -off;
    }
    let w = mv.ramp.min(1.0);
    if (pos[0] - target[0]).abs() < CLOSE_X || mv.ramp < 1.0 {
        mv.ramp = (mv.ramp - 0.1).max(0.0);
    }
    let d = [target[0] - pos[0], 0.0, target[2] + w * off - pos[2]];
    let l = len(d);
    let mut pull = scale(unit(d), l * HOMING_K * HOMING_K * l);
    if len(pull) > HOMING_MAX {
        pull = scale(unit(pull), HOMING_MAX);
    }
    let a = if homing {
        add(scale(input, rules.acc_control), pull)
    } else {
        scale(input, rules.acc_control * CONTROL_SCALE)
    };
    let mut drag = rules.drag;
    if mv.since < DRAG_RAMP_TICKS {
        let left = DRAG_RAMP_TICKS - mv.since;
        mv.since += 1;
        drag *= left as f64 / DRAG_RAMP_TICKS as f64 * DRAG_RAMP + 1.0;
    }
    let mut next = add(pos, scale(add(vel, a), (1.0 - drag) / 24.0));
    // the height: towards the Quaffle's less 0.9, by its share of the distance (at most a unit a tick)
    let to = [target[0] - next[0], aim.ball_y - BELOW_BALL - next[1], target[2] - next[2]];
    next[1] += to[1] / len(to).max(1.0);
    let step = sub(next, pos);
    if len(step) > MAX_STEP {
        next = add(pos, scale(unit(step), MAX_STEP));
    }
    if mv.guided > 0 {
        if mv.guided == mv.guided_total {
            mv.launch = (pos, vel);
        }
        mv.guided -= 1;
        let total = mv.guided_total;
        let elapsed = total - mv.guided;
        let progress = elapsed as f64 / total as f64;
        // a cubic from the launch to above the carrier (30 ticks), then on to ahead of him along his attack; both
        // ends move with him
        let up = add(target, [0.0, CURVE_RISE, 0.0]);
        let (c, _) = if elapsed < CURVE_SPLIT {
            let n = CURVE_SPLIT as f64;
            bezier(mv.launch.0, mv.launch.1, up, aim.vel, n / 24.0, elapsed as f64 / n)
        } else {
            let n = (total - CURVE_SPLIT).max(1) as f64;
            let end = add(target, [aim.dir * CURVE_AHEAD, 0.0, 0.0]);
            bezier(up, aim.vel, end, aim.vel, n / 24.0, (elapsed - CURVE_SPLIT) as f64 / n)
        };
        let k = exp(-CURVE_BLEND * (1.0 - progress).abs());
        next = add(scale(c, 1.0 - k), scale(next, k));
        // **[I]** the step cap again, so a new target (a pass) cannot make the curve jump the Bludger
        let step = sub(next, pos);
        if len(step) > MAX_STEP {
            next = add(pos, scale(unit(step), MAX_STEP));
        }
    }
    (next, scale(sub(next, pos), 24.0))
}

/// The AI's input for the steer (state 0x27, `0x453420`) and its homing flag. Homing, it leads the carrier by the
/// Bludger's time to reach him and stops once 5 behind him along his attack; not homing, it pushes along his attack
/// and swings across (away within 5, back beyond 8) until 5 ahead, then homes again.
pub fn ai_input(mv: &mut Move, pos: Vec3, vel: Vec3, aim: &Aim) -> (Vec3, bool) {
    let unit_xz = |v: Vec3| unit([v[0], 0.0, v[2]]);
    if mv.ai_homing {
        let speed = len(vel) / 24.0;
        let t = if speed > 1e-9 { len(sub(aim.pos, pos)) / speed } else { 0.0 };
        let lead = [aim.pos[0] + aim.vel[0] / 24.0 * t, aim.pos[1], aim.pos[2] + aim.vel[2] / 24.0 * t];
        let v = [lead[0] - pos[0] - vel[0] / 24.0, 0.0, lead[2] - pos[2] - vel[2] / 24.0];
        if (pos[0] - aim.pos[0]) * aim.dir < -AI_BEHIND {
            mv.ai_homing = false;
        }
        return (unit_xz(v), mv.ai_homing);
    }
    let dz = aim.pos[2] - pos[2];
    let z = if dz.abs() < AI_NEAR {
        if dz > 0.0 {
            -1.0
        } else {
            1.0
        }
    } else if dz.abs() > AI_FAR {
        if dz > 0.0 {
            1.0
        } else {
            -1.0
        }
    } else {
        0.0
    };
    if (pos[0] - aim.pos[0]) * aim.dir > AI_AHEAD {
        mv.ai_homing = true;
    }
    (unit_xz([aim.dir, 0.0, z]), mv.ai_homing)
}

/// The active player's place while the team's Bludger move runs (`0x4389a0`): towards the carrier, closing a fifth
/// of the gap beyond 5 units a tick (at most 2), never nearer than 5.
pub fn pursue(active: Vec3, carrier: Vec3) -> Vec3 {
    let d = len(sub(active, carrier));
    let close = ((d - PURSUIT_DIST) * PURSUIT_RATE).min(PURSUIT_MAX);
    let k = (d - close).max(PURSUIT_DIST);
    add(carrier, scale(unit(sub(active, carrier)), k))
}

/// Whether the Bludger at `pos` hits a player at `victim` (`0x43da40`).
pub fn hits(pos: Vec3, victim: Vec3) -> bool {
    len(sub(pos, victim)) < HIT_RADIUS
}

/// A hit player's knock-back (`0x424080`): where he flies in [`KNOCK_TICKS`], along half the sum of his and the
/// Bludger's velocities, arriving still.
pub fn knock(victim: Vec3, victim_vel: Vec3, bludger_vel: Vec3) -> Vec3 {
    let t = KNOCK_TICKS as f64 / 24.0 * KNOCK_SCALE;
    add(victim, scale(add(victim_vel, bludger_vel), t))
}

/// A hit's side (`0x424080`): a Bludger from the front (its direction against the victim's facing below -0.6)
/// starts the `HitBlud` cutscene, any other the hit animation.
pub fn from_front(bludger_vel: Vec3, victim_fwd: Vec3) -> bool {
    let b = unit([bludger_vel[0], 0.0, bludger_vel[2]]);
    let f = unit([victim_fwd[0], 0.0, victim_fwd[2]]);
    b[0] * f[0] + b[2] * f[2] < -0.6
}

/// A beater's bat (`0x423880`, **[I]** one command each where the original chains three): the Bludger runs on
/// along its velocity for half the bat's ticks, then leaves away from the beater at [`BAT_SPEED`]; the beater
/// meets it there. Returns the Bludger's end (position, velocity) and the beater's end position.
pub fn bat(bludger: Vec3, bludger_vel: Vec3, beater: Vec3) -> ((Vec3, Vec3), Vec3) {
    let half = (BAT_TICKS / 2) as f64 / 24.0;
    let mid = add(bludger, scale(bludger_vel, half));
    let mut dir = sub(bludger, beater);
    if dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2] <= 0.05 {
        dir = [0.0, 1.0, 0.0];
    }
    let dir = unit(dir);
    let away = scale(dir, BAT_SPEED);
    let end = add(mid, scale(away, 1.0 / 24.0));
    // the beater ends half a unit short of the Bludger, a little below it
    let swing = add(add(mid, scale(dir, -0.5)), [0.0, -0.3, 0.0]);
    ((end, away), swing)
}

/// The AI's defence choice (`0x455570`) from `u` (the `rand()` draw scaled to `0..1`): 0 chase, 1 the defensive
/// special, 2 the Bludger. The special needs `special_ok` (the Quaffle not deep in the team's own half and the
/// option `0x1018f83`); the Bludger needs the move charged, the Bludger option `0x1018f85` and a carrier who is not
/// a keeper. No choice in the Seeker game.
pub fn defence_choice(weights: [f64; 3], u: f64, bludger_ok: bool, special_ok: bool, seeker_game: bool) -> u32 {
    let [chase, special, bludger] = weights;
    let x = u * (chase + special + bludger);
    if seeker_game || x < chase {
        0
    } else if x < chase + special {
        u32::from(special_ok)
    } else if bludger_ok {
        2
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Rules {
        Rules {
            acc_control: 2.5,
            drag: 0.06,
            max_dist: 10.0,
            summon_period: 24,
            block_dist: 1.5,
            weights: [1.0, 0.5, 0.5],
            penalty: [0.04; 2],
        }
    }

    #[test]
    fn the_pursuit_closes_to_five() {
        // 25 away: closes 2 a tick (a fifth of 20 is 4, capped)
        let p = pursue([25.0, 0.0, 0.0], [0.0; 3]);
        assert!((p[0] - 23.0).abs() < 1e-12);
        // 6 away: a fifth of 1
        let p = pursue([6.0, 0.0, 0.0], [0.0; 3]);
        assert!((p[0] - 5.8).abs() < 1e-12);
        // nearer than 5: pushed out to 5
        let p = pursue([0.0, 0.0, 3.0], [0.0; 3]);
        assert!((p[2] - 5.0).abs() < 1e-12);
    }

    #[test]
    fn a_launched_bludger_homes_in_and_hits_a_still_carrier() {
        let r = rules();
        let carrier = [0.0, 10.0, 0.0];
        let aim = Aim { pos: carrier, vel: [0.0; 3], dir: -1.0, ball_y: 10.9 };
        let mut mv = Move::new(0, [30.0, 10.0, 12.0], [0.0; 3], false);
        let (mut p, mut v) =
            launch(&mut mv, [30.0, 10.0, 12.0], ([0.0; 3], [-1.0, 0.0, 0.0]), Some((carrier, [0.0; 3])), false);
        assert!(mv.flying && mv.guided == GUIDED_TICKS);
        let mut hit = None;
        for tick in 0..240 {
            let (input, homing) = ai_input(&mut mv, p, v, &aim);
            (p, v) = steer(&mut mv, &r, p, v, &aim, input, homing);
            assert!(len(v) <= 50.0 + 1e-9, "{v:?}");
            if hits(p, carrier) {
                hit = Some(tick);
                break;
            }
        }
        assert!(hit.is_some(), "never hit; ended at {p:?}");
    }

    #[test]
    fn the_drag_eases_over_a_second() {
        let r = rules();
        let aim = Aim { pos: [0.0; 3], vel: [0.0; 3], dir: 1.0, ball_y: 0.9 };
        let mut mv = Move::new(0, [0.0; 3], [0.0; 3], true);
        mv.flying = true;
        // first tick: drag 0.06 * 3 on 24 units a second straight down the aim's line, no input
        let (_, v) = steer(&mut mv, &r, [0.0, 0.0, 0.0], [24.0, 0.0, 0.0], &aim, [0.0; 3], false);
        assert!((v[0] - 24.0 * (1.0 - 0.18)).abs() < 1e-9, "{v:?}");
        assert_eq!(mv.since, 1);
    }

    #[test]
    fn a_bludger_from_the_front_is_the_cutscene() {
        assert!(from_front([-10.0, 0.0, 0.0], [1.0, 0.0, 0.0]));
        assert!(!from_front([10.0, 0.0, 0.0], [1.0, 0.0, 0.0]));
    }

    #[test]
    fn the_ai_picks_by_its_weights() {
        let w = [1.0, 0.5, 0.5];
        assert_eq!(defence_choice(w, 0.2, true, true, false), 0);
        // the special's band: 1 when it may, else the chase
        assert_eq!(defence_choice(w, 0.6, true, true, false), 1);
        assert_eq!(defence_choice(w, 0.6, true, false, false), 0);
        assert_eq!(defence_choice(w, 0.9, true, true, false), 2);
        assert_eq!(defence_choice(w, 0.9, false, true, false), 0);
        assert_eq!(defence_choice(w, 0.9, true, true, true), 0);
    }
}
