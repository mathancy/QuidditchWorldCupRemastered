//! The broom handling `0x442ac0` after its tackle branch (`docs/GAMEPLAY.md`, "Broom handling"; re-read
//! 2026-10-19 from the decompiled function): the stick turns into a velocity, the position moves, the heading
//! turns towards the stick, and the vertical velocity seeks a height. Humans and the AI both drive players through
//! it; the AI fills the stick from its states.
//!
//! The rope (`0x443b1f`-`0x443e8a`, checked under an emulator: `tools/check_handling.py`): with a target (the
//! chase of the Quaffle, `0x436490`) the player is pulled towards it when farther than the rope's length, and held
//! 1.5 units off it when closer than that. A human team's rope is 5 units long, so a human defender keeps on the
//! ball; an AI team's is 100.
//!
//! Not ported: the roll (cosmetic), the hand-off timer `+0xaa0` (set and cleared in the same call when a lunge
//! ends on its victim) and the lunge's heading term.

use crate::tackle::TeamTackle;
use crate::vec3::{add, dot, len, scale, sub, unit, Vec3};

/// The steering assist towards a target (`0x668f2c`), the speed cap (`0x668f28`), and the heading blend constants
/// (`0x668f10`, `0x668f14`, `0x668f18`, `0x668f1c`).
pub const ASSIST: f64 = 0.6;
pub const SPEED_CAP: f64 = 50.0;
const PITCH_FOLLOW: f64 = 0.33;
const PITCH_STICK: f64 = 0.66;
const PITCH_DAMP: f64 = 0.9;
/// The rope (`Rope.ForceScale` `0x6066e4`, `Rope.ForceMax` `0x6066e8`): beyond its length the player is pulled
/// `min(0.2 (d - length), 2)` units a tick towards the target; within `ROPE_NEAR` (`0x6066dc`) he is held there.
pub const ROPE_FORCE: f64 = 0.2;
pub const ROPE_MAX: f64 = 2.0;
pub const ROPE_NEAR: f64 = 1.5;
/// The ropes' lengths (team `+0xa98`): `Rope.Length` (`0x6066d8`) for a human team, 100 for an AI team (`+0x8e`),
/// `Rope.Length` times `Rope.ChallengeLengthModifier` (`0x6066e0`) for both in a challenge.
pub const ROPE_HUMAN: f64 = 5.0;
pub const ROPE_AI: f64 = 100.0;
pub const ROPE_CHALLENGE: f64 = 10.0;

/// `Handling.*` console values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    pub acceleration: f64,
    pub max_speed: f64,
    pub blend_heading_joy: f64,
    pub blend_heading_vel: f64,
}

impl Default for Params {
    fn default() -> Params {
        Params { acceleration: 1.0, max_speed: 20.0, blend_heading_joy: 1.0, blend_heading_vel: 0.0 }
    }
}

/// A flying player: position, velocity (units per second) and forward direction (matrix row 2, `+0x50`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Body {
    pub pos: Vec3,
    pub vel: Vec3,
    pub fwd: Vec3,
}

/// One call's inputs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Call {
    /// The stick (input `+0x00`); longer than 1 it is normalised.
    pub stick: Vec3,
    /// The target's position and velocity (the Quaffle for the general mover with `chase` set).
    pub target: Option<(Vec3, Vec3)>,
    /// The player holds a ball (`+0x190`): no steering assist.
    pub holds_ball: bool,
    /// The team is AI-controlled (`+0x8e`): the assist only pulls towards the target, never away.
    pub ai: bool,
    /// The team's stun (`+0xab4`) when the call was made; the step reads the team's own, which the tackle branch
    /// may have changed since.
    pub stun: i32,
    /// The height sought and the fraction of the error closed per tick (0: keep the vertical velocity).
    pub y_target: f64,
    pub gain: f64,
    /// The rope's length (team `+0xa98`, [`ROPE_HUMAN`], [`ROPE_AI`], [`ROPE_CHALLENGE`]); it pulls only with a
    /// target.
    pub rope: f64,
}

/// The velocity, position, heading and height steps of `0x442ac0` (`0x443a4f` onwards). `tackle` holds the
/// team's lunge (`+0xa64`), accumulator, stun and knock-back; while a lunge runs the accumulator drives the
/// horizontal velocity.
pub fn step(p: &Params, b: &Body, c: &Call, tackle: &mut TeamTackle) -> Body {
    // the knock-back of a tackle on the team (`+0xaa4`, `0x443a4f`): once, on the velocity
    let mut v0 = b.vel;
    if dot(tackle.knock_back, tackle.knock_back) > 0.1 {
        v0 = add(v0, tackle.knock_back);
        tackle.knock_back = [0.0; 3];
    }
    // the rope (`0x443b1f`): the position the step starts from is pulled towards the target, and the heading
    // turned halfway to the pull
    let (mut base, mut fwd0) = (b.pos, b.fwd);
    if let Some((tp, _)) = c.target {
        let r = sub(tp, b.pos);
        let d = len(r);
        if d >= ROPE_NEAR || tackle.stun != 0 {
            if d > c.rope {
                let pull = (ROPE_FORCE * (d - c.rope)).min(ROPE_MAX);
                let to = sub(tp, scale(unit(r), (d - pull).max(c.rope)));
                let h = unit(add(unit(sub(to, base)), fwd0));
                if dot(h, h) > 0.0 {
                    fwd0 = h;
                }
                base = to;
            }
        } else {
            base = sub(tp, scale(unit(r), ROPE_NEAR));
        }
    }
    let orig = b;
    let b = &Body { pos: base, vel: v0, fwd: fwd0 };
    let mut s = c.stick;
    if len(s) > 1.0 {
        s = unit(s);
    }
    let stick_len = len(s);
    let dhat = if stick_len > 0.1 { unit(s) } else { [0.0; 3] };
    let mut a = p.acceleration;
    if let Some((tp, _)) = c.target {
        let d = dot(dhat, unit(sub(tp, orig.pos)));
        if !c.holds_ball && (!c.ai || d > 0.0) {
            a *= d * ASSIST + 1.0;
        }
    }
    if tackle.stun != 0 {
        a = (a * (1.0 - tackle.stun as f64 * 0.2)).max(0.0);
    }
    let k = 1.0 - p.acceleration / p.max_speed;
    let v1 = scale(add(scale(s, a), b.vel), k);
    let lunging = tackle.lunge > 0.0;
    let mut vel = if lunging { tackle.lunge_velocity(s, k) } else { v1 };
    if dot(vel, vel) > SPEED_CAP * SPEED_CAP {
        let l = len(vel);
        vel[0] *= SPEED_CAP / l;
        vel[2] *= SPEED_CAP / l;
    }
    let pos = [b.pos[0] + vel[0] / 24.0, b.pos[1] + v1[1] / 24.0, b.pos[2] + vel[2] / 24.0];
    // heading: towards the stick (and the velocity, with BlendHeadingVel), pitch damped
    let mut h = add(b.fwd, scale(s, a.min(1.0) * p.blend_heading_joy));
    if v1[0] * v1[0] + v1[2] * v1[2] > 1.0 {
        h = add(h, scale(unit(v1), p.blend_heading_vel));
    }
    let mut hy = if stick_len <= PITCH_STICK {
        h[1] * PITCH_DAMP
    } else {
        (PITCH_STICK * unit(sub(pos, b.pos))[1] + h[1]) * PITCH_DAMP
    };
    hy = (hy - b.fwd[1]) * PITCH_FOLLOW + b.fwd[1];
    h[1] = hy;
    let h = unit(h);
    let fwd = if dot(h, h) > 0.1 { h } else { b.fwd };
    // vertical velocity
    let mut vy = vel[1];
    if c.gain > 0.0 {
        vy = ((pos[1] - c.y_target) * c.gain * -24.0).min((vel[0] * vel[0] + vel[2] * vel[2]).sqrt());
    }
    if let (Some((tp, tv)), true) = (c.target, lunging) {
        // match the target's height by the time of contact
        let rel = sub(orig.pos, tp);
        let relv = sub(scale(sub(pos, orig.pos), 24.0), tv);
        let t = if len(relv) <= 0.1 { 1.0 } else { (len(rel) - 1.5).max(0.0) / len(relv) };
        vy = (pos[1] - tp[1]) * (-1.0 / t.max(1.0 / 24.0));
    }
    Body { pos, vel: [vel[0], vy, vel[2]], fwd }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> Body {
        Body { pos: [0.0; 3], vel: [0.0; 3], fwd: [1.0, 0.0, 0.0] }
    }

    fn call(stick: Vec3) -> Call {
        Call { stick, target: None, holds_ball: true, ai: true, stun: 0, y_target: 0.0, gain: 0.0, rope: ROPE_AI }
    }

    #[test]
    fn full_stick_settles_near_19() {
        let p = Params::default();
        let mut b = body();
        let mut t = TeamTackle::default();
        for _ in 0..24 * 10 {
            b = step(&p, &b, &call([0.0, 0.0, 1.0]), &mut t);
        }
        // v = 0.95 (1 + v): v -> 19
        assert!((b.vel[2] - 19.0).abs() < 1e-3, "{:?}", b.vel);
        // the heading turned to the stick
        assert!(b.fwd[2] > 0.99);
        // first tick: v = 0.95, moved 0.95 / 24
        let b1 = step(&p, &body(), &call([0.0, 0.0, 1.0]), &mut t);
        assert!((b1.vel[2] - 0.95).abs() < 1e-12 && (b1.pos[2] - 0.95 / 24.0).abs() < 1e-12);
    }

    #[test]
    fn assist_stun_and_cap() {
        let p = Params::default();
        let mut t = TeamTackle::default();
        // a target straight along the stick: a = 1.6, v = 0.95 * 1.6
        let c = Call { target: Some(([10.0, 0.0, 0.0], [0.0; 3])), holds_ball: false, ..call([1.0, 0.0, 0.0]) };
        assert!((step(&p, &body(), &c, &mut t).vel[0] - 1.52).abs() < 1e-12);
        // AI teams get no negative assist; humans do
        let away = Call { target: Some(([-10.0, 0.0, 0.0], [0.0; 3])), ..c };
        assert!((step(&p, &body(), &away, &mut t).vel[0] - 0.95).abs() < 1e-12);
        let human = Call { ai: false, ..away };
        assert!((step(&p, &body(), &human, &mut t).vel[0] - 0.95 * 0.4).abs() < 1e-12);
        // the team stunned 2: a = 0.6
        let mut stunned = TeamTackle { stun: 2, ..Default::default() };
        assert!((step(&p, &body(), &call([1.0, 0.0, 0.0]), &mut stunned).vel[0] - 0.95 * 0.6).abs() < 1e-12);
        // the cap trims x and z only
        let fast = Body { vel: [60.0, 30.0, 0.0], ..body() };
        let v = step(&p, &fast, &call([0.0; 3]), &mut t).vel;
        let l = (57.0f64 * 57.0 + 28.5 * 28.5).sqrt();
        assert!((v[0] - 57.0 * 50.0 / l).abs() < 1e-9 && (v[1] - 28.5).abs() < 1e-9);
    }

    #[test]
    fn height_gain() {
        let p = Params::default();
        let mut t = TeamTackle::default();
        // 2 units above the target height, gain 0.2: vy = -24 * 0.2 * 2 = -9.6, capped by the horizontal speed
        let b = Body { pos: [0.0, 2.0, 0.0], vel: [10.0, 0.0, 0.0], ..body() };
        let c = Call { gain: 0.2, ..call([0.0; 3]) };
        let n = step(&p, &b, &c, &mut t);
        assert!((n.vel[1] - (-24.0 * 0.2 * 2.0)).abs() < 1e-9);
        // below the target the climb is capped by the horizontal speed (9.5)
        let low = Body { pos: [0.0, -5.0, 0.0], ..b };
        assert!((step(&p, &low, &c, &mut t).vel[1] - 9.5).abs() < 1e-9);
    }

    /// The emulator's numbers (`tools/check_handling.py`): a human chaser at the origin, the Quaffle's carrier
    /// along `x`.
    #[test]
    fn the_rope_keeps_a_human_on_the_ball() {
        let p = Params::default();
        let mut t = TeamTackle::default();
        let chase = |at: f64, rope: f64| Call {
            target: Some(([at, 0.0, 0.0], [0.0; 3])),
            holds_ball: false,
            ai: false,
            rope,
            ..call([0.0; 3])
        };
        // 20 away, no stick: pulled the most, 2 units a tick
        let n = step(&p, &body(), &chase(20.0, ROPE_HUMAN), &mut t);
        assert!(len(sub(n.pos, [2.0, 0.0, 0.0])) < 1e-9 && n.vel == [0.0; 3]);
        // within the rope's length: not at all; within 1.5: held there
        assert_eq!(step(&p, &body(), &chase(4.0, ROPE_HUMAN), &mut t).pos, [0.0; 3]);
        assert!(len(sub(step(&p, &body(), &chase(1.0, ROPE_HUMAN), &mut t).pos, [-0.5, 0.0, 0.0])) < 1e-9);
        // an AI team's rope is 100 long
        assert_eq!(step(&p, &body(), &chase(20.0, ROPE_AI), &mut t).pos, [0.0; 3]);
        // flying along z: pulled along x, and turned halfway to the pull
        let b = Body { vel: [0.0, 0.0, 10.0], fwd: [0.0, 0.0, 1.0], ..body() };
        let n = step(&p, &b, &chase(20.0, ROPE_HUMAN), &mut t);
        assert!(len(sub(n.pos, [2.0, 0.0, 9.5 / 24.0])) < 1e-9);
        assert!(len(sub(n.vel, [0.0, 0.0, 9.5])) < 1e-9);
        assert!(len(sub(n.fwd, [0.5f64.sqrt(), 0.0, 0.5f64.sqrt()])) < 1e-9);
    }

    #[test]
    fn the_knock_back_is_one_push() {
        let p = Params::default();
        let mut t = TeamTackle { knock_back: [0.0, 0.0, 12.0], ..Default::default() };
        let n = step(&p, &body(), &call([0.0; 3]), &mut t);
        assert!(len(sub(n.vel, [0.0, 0.0, 11.4])) < 1e-9);
        assert_eq!(t.knock_back, [0.0; 3]);
    }

    #[test]
    fn lunge_drives_the_velocity() {
        let p = Params::default();
        let mut t = TeamTackle { lunge: 0.3, acc: [20.0, 0.0, 0.0], lunge_disp: [5.0, 0.0, 0.0], ..Default::default() };
        let n = step(&p, &body(), &call([0.0; 3]), &mut t);
        assert!((n.vel[0] - 19.0).abs() < 1e-9);
        assert!((n.pos[0] - 19.0 / 24.0).abs() < 1e-9);
    }
}
