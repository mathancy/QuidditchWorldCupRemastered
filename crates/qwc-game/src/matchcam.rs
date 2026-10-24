//! The match camera (`CameraSystem`'s game camera, `0x47ed80` mode 0: `0x480250`), for one player's view.
//!
//! **[T]** It runs once a tick: `CAMERASYSTEM` is in the 24 Hz subsystem list with the gameplay (`GAMEPLAY.md`,
//! "Simulation tick"), its spring steps 0.04 s and its eases and counters go by the call. The frames between ticks
//! blend the last two views as the render camera does (`0x4bed60`, [`View::blend`]).
//!
//! **[T]** What its event handlers keep (`0x47eb50`): the team in possession (`+0x21c`, the possession message
//! `0x1043b70`, `0x47ee60`: posted on a catch or a tackle, so it stays through passes and loose balls); the
//! Quaffle's position and velocity and whether it is held, attached to a carrier (`+0x244`, `+0x254`, `+0x313`:
//! the balls' sync `0x1043b84`, `0x4825a0`, from `0x464220`); the two teams' active players (`0x1043b8c`,
//! `0x4829f0`) and their positions and velocities (`+0x264`, `+0x274`, `+0x294`: the players' sync `0x1043b88`,
//! `0x481fa0`, from `0x465610`) and whether its own is the keeper (`+0x315`).
//!
//! **[T]** Each step (`0x480250`) sets a followed point, a target, a distance, a height, a pitch, a lag and a zoom:
//! - the keeper holding the Quaffle: 9 behind, 4.5 up, pitched 0.4 down, zoom 1.5, on him looking at the hoops his
//!   team attacks (on the opponents' active player, at its own hoops, when they have it);
//! - its team in possession, or within 6 ticks of losing it: looking at the hoops it attacks, pitched 0.25 down; with
//!   the Quaffle in flight (a pass, a shot) on the Quaffle, 15 behind, zoom 0.75 (1.2 boosting); held, on the active
//!   player with a zoom by his speed (`0x47f580`: `clamp(2 |v|, 1, 10) × 50 / 400`, eased to 0.75 over the 30 ticks
//!   after a catch), 6 behind and 1.75 up, within 35 of the hoops `h = (35 - d) / 35` more: 7.5 h behind, 8 h up,
//!   0.1 h down (12 behind, 5 up, 0.4 down in the 6 ticks after losing it);
//! - otherwise between the two active players: by their distance `d`, `f = clamp((d - 3) / 22)`, `g = clamp((d -
//!   2) / 18)`, `(12 g + 6) × 1.3` behind, `(11.25 f + 1.75) × 1.3` up, pitched `0.25 + 0.45 f` down, looking at the
//!   midpoint of the Quaffle and its own hoops; with the Quaffle in flight on the Quaffle, 15 behind; the lag raised
//!   with the Quaffle's speed (`(1 - lag) × clamp(|v| / 10 - 1, 0, 2) / 2`); within 20 of the target `h = (20 -
//!   d') / 20` more: 3 h behind, 7 h up, 0.2 h down.
//!
//! **[T]** It eases its own settings towards them (the distance by 0.1, the others by 0.05), leads the point by 0.12
//! of its velocity and springs after it (`0x47f5f0`: stiffness 400 times the zoom, damping 50 and 100, mass 25,
//! steps of 0.04, the stretch held to 15). It is towed: its direction is the one from its last eye to the point,
//! turned towards the target by the lag (a slerp, `0x47fae0`), and held towards the last one over the 25 ticks after
//! the possession changes (`+0x300`). The eye is the distance back along it and the height up; it looks along it
//! pitched about its right, rolled by 5 times the turn of its up (held to ±1.5, eased by 0.05). A cut (`+0x30d`,
//! `0x47ed80`) takes everything as it is for 5 ticks (`+0x308`). The field of view is 70 across a 4:3 screen, 52.5
//! down it.
//!
//! **[I]**: the active players are the sim's (an AI team's carrier, else its chaser nearest the Quaffle); the look's
//! rotations are slerped as directions (`0x47f840` turns each into a quaternion first). The kick-off after a goal
//! cuts (`0x451730` posts `0x1043a94 {0, 1}`), as do the take at the match's start and the goal camera's end, which
//! `crate::shots` asks for.
//!
//! **[T]** (2026-10-21) The defensive special's lock-on: while the target records' message (`0x1043b7c`,
//! `0x482c60`) has one for the camera's team, `+0x304` counts up and `+0x2a4` takes its position; past 4 the camera
//! follows it, on defence 9 behind, 11 up, pitched 0.9 down with the Quaffle's velocity. The Bludger camera
//! (`+0x30e`): the team's Bludger move's start (`0x43c3c0` posts `0x10439f8`, `0x482cf0`) turns it on, a tick
//! later on the active player for 30 ticks, its launch (`0x1043adc`, `0x482d50`) on the way from the Bludger to the
//! Quaffle (0.4 of it, at most 10, the distance raised to 1.2 times it), 11 behind, 8 up, pitched 0.55 down, lag 0.1;
//! the move's end (`0x43d950`) turns it off. The other team's move (`+0x30f`) puts the carrier's camera 12 behind,
//! 10 up, pitched 0.85 down. The goal's fixed camera (mode 1, `0x47fbc0`) and the kick-off's held view (mode 4)
//! are `crate::shots`'. Left out: `0x47fde0`'s broadcast camera (mode 3), `+0x311`, the countdown `+0x2f4` and the
//! Beaters' challenge's case.
//!
//! **The remaster's framing** (not the original's, whose passing challenge lets the mates fly out of the picture):
//! with players to keep in view ([`Inputs::keep`]: the passing challenge's two mates), after the original's aim the
//! camera turns about its followed point towards a mate nearing an edge (past 0.6 of the half width; the turn eased
//! and at most about 3.4 degrees a tick, the original's own turn towards its target waiting meanwhile), and backs off
//! and rises when turning cannot help: mates past the bounds on both sides, above or below, out of the picture, or
//! behind it. It comes in again (a second's half-life) once that is over ([`MatchCamera::frame`]); a cut settles all
//! of it at once. Each mate is measured on his own, so it holds as they fly apart. In headless runs (`qwc-game
//! challenge`, which prints how long a mate was out) a mate is out 7-11% of the time, mostly just after a pass,
//! where the original's camera loses one 22-100% of the time.

use qwc_render::glam::{Quat, Vec3};
use qwc_render::Camera;
use qwc_sim::world::Role;
use qwc_sim::Match;

/// One step of the camera, `0x47f5f0`'s.
const STEP: f32 = 0.04;
/// The ticks after a change of possession that still frame the old attack (`0x5fd394` set: 6).
const GRACE: u32 = 6;
/// The ticks the direction is held over after a change of possession (`+0x300`, `0x5a5bb8`).
const HOLD: f32 = 25.0;
/// The lock-on's ticks before the camera follows its target (`0x5a5bbc`).
const LOCK_AFTER: f32 = 4.0;
/// **[I]** How far above the floor the eye keeps.
const EYE_ABOVE_FLOOR: f32 = 1.0;
/// **[I]** The sines of the steepest the camera's direction looks up and down (20 and 45 degrees).
const DIR_UP: f32 = 0.342;
const DIR_DOWN: f32 = 0.707;
/// The Bludger camera's ticks on the active player before it waits for the launch (`0x5a5ca4`).
const BLUDGER_HOLD: f32 = 30.0;
/// The framing's bounds as fractions of the half width and height: a mate past `KEEP` turns the camera, past
/// `KEEP_V` up or down backs it off; all within `RELAX` lets it come in again.
const KEEP: f32 = 0.6;
const KEEP_V: f32 = 0.6;
const RELAX: f32 = 0.5;
/// How much of a mate's way past the bound the camera turns a tick, at most `MAX_TURN` radians.
const TURN_GAIN: f32 = 0.3;
const MAX_TURN: f32 = 0.06;
/// How much of the way to the turn it wants the camera's turn goes a tick.
const TURN_EASE: f32 = 0.15;
/// How fast the original's turn towards its target comes back after the framing turned, a tick.
const PULL_BACK: f32 = 0.02;
/// The height the camera rises by for each unit it backs off.
const RISE: f32 = 0.35;
/// How far the camera backs off a tick (more the further they are out, at most `MAX_BACK_STEP`), comes in again a
/// tick, and backs off at most.
const BACK_STEP: f32 = 0.15;
const MAX_BACK_STEP: f32 = 1.5;
const IN_STEP: f32 = 0.2;
/// Or this share of the back-off, when that is more (a second's half-life).
const IN_SHARE: f32 = 0.03;
const MAX_BACK: f32 = 40.0;
/// The framing's steps a cut takes at once.
const CUT_FRAMING: usize = 60;

pub(crate) fn v3(a: [f64; 3]) -> Vec3 {
    Vec3::new(a[0] as f32, a[1] as f32, a[2] as f32)
}

/// What the camera's event handlers have heard by the end of a tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Inputs {
    /// The camera's team (`+0x218`).
    pub team: usize,
    /// The holder's team, when someone holds the Quaffle (the possession message).
    pub holder_team: Option<usize>,
    /// The Quaffle is held, attached to a carrier (`+0x313`).
    pub held: bool,
    /// The Quaffle's position and velocity (`+0x244`, `+0x254`).
    pub quaffle: (Vec3, Vec3),
    /// The camera team's active player's position and velocity (`+0x264`, `+0x274`), and whether the keeper view is
    /// on (`+0x315`, the camera team's active player being its keeper; **[I]** a keeper of either team holding the
    /// Quaffle, after the user's footage of the restart after a goal, where the scoring side's view is the other
    /// keeper's too).
    pub ours: (Vec3, Vec3),
    pub keeper: bool,
    /// The other team's active player's position (`+0x294`).
    pub theirs: Vec3,
    /// The camera team boosting (`+0x314`).
    pub boosting: bool,
    /// The centres of the hoops the team attacks and of its own, across the pitch's middle (`0x47ef50`).
    pub attack: Vec3,
    pub own: Vec3,
    /// The camera team's defensive special's target while its lock-on shows (the target records' message
    /// `0x1043b7c`, `0x482c60`).
    pub lock_on: Option<Vec3>,
    /// The camera team's Bludger move: launched or still summoning, and its Bludger's position and velocity.
    pub bludger: Option<(bool, Vec3, Vec3)>,
    /// The other team's Bludger move is on (`0x1043adc` from the other team, `+0x30f`).
    pub attacked: bool,
    /// The floor under the Quaffle, which the eye keeps above ([`MatchCamera::step`]).
    pub floor: f32,
    /// Players the remaster keeps in the picture (the passing challenge's two mates; [`MatchCamera::frame`]).
    pub keep: [Option<Vec3>; 2],
}

impl Inputs {
    /// The inputs from match `m` at the end of a tick, for `team`'s camera.
    pub fn of(m: &Match, team: usize) -> Inputs {
        let holder_team = m.holder.map(|h| m.players[h].team);
        // the carrier first: the sim's team update can run before a catch in the same tick
        let active = |t: usize| {
            let carrier = m.holder.filter(|&h| m.players[h].team == t);
            carrier.or(m.teams[t].active).or_else(|| m.chaser_nearest(t, m.quaffle))
        };
        let quaffle = (v3(m.quaffle), v3(m.quaffle_vel));
        let at = |i: Option<usize>| i.map_or(quaffle, |i| (v3(m.players[i].pos), v3(m.players[i].vel)));
        let ours = active(team);
        // the shown hoops' centres, their z cleared; `(±97.5, 15, 0)` without any (team 0 attacks towards -x): so
        // in the passing challenge, whose hoops are hidden under the pitch
        let centre = |attack: bool| {
            let hoops: Vec<Vec3> =
                (m.hoops.iter()).filter(|h| m.hoops_shown && (h.team == team) != attack).map(|h| v3(h.pos)).collect();
            if hoops.is_empty() {
                let x = if (team == 0) == attack { -97.5 } else { 97.5 };
                Vec3::new(x, 15.0, 0.0)
            } else {
                let c = hoops.iter().copied().sum::<Vec3>() / hoops.len() as f32;
                Vec3::new(c.x, c.y, 0.0)
            }
        };
        Inputs {
            team,
            holder_team,
            held: m.holder.is_some(),
            quaffle,
            ours: at(ours),
            keeper: m.holder.is_some_and(|h| m.players[h].role == Role::Keeper),
            theirs: at(active(1 - team)).0,
            boosting: m.teams[team].boost_hold > 0,
            attack: centre(true),
            own: centre(false),
            // **[I]** the lock-on's target is the carrier the special runs at (TargetRender's entity)
            lock_on: m.teams[team]
                .special_tackle
                .and(m.holder.filter(|&h| m.players[h].team != team))
                .map(|h| v3(m.players[h].pos)),
            bludger: m.teams[team]
                .bludger
                .as_ref()
                .map(|mv| (mv.flying, v3(m.bludgers[team].pos), v3(m.bludgers[team].vel))),
            attacked: m.teams[1 - team].bludger.is_some(),
            floor: m.pitch.floor(m.quaffle) as f32,
            keep: keep(m, team, ours),
        }
    }
}

/// The passing challenge's mates for `team`'s camera: its present chasers other than the active one (`ours`).
fn keep(m: &Match, team: usize, ours: Option<usize>) -> [Option<Vec3>; 2] {
    let mut out = [None; 2];
    if m.challenge != Some(1) {
        return out;
    }
    let mates = m.teams[team].chasers.iter().filter(|&&c| m.players[c].present && Some(c) != ours);
    for (slot, &c) in out.iter_mut().zip(mates) {
        *slot = Some(v3(m.players[c].pos));
    }
    out
}

/// A view of the camera: its eye, the way it looks, its up and its horizontal field of view in degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub eye: Vec3,
    pub dir: Vec3,
    pub up: Vec3,
    pub fov: f32,
}

impl View {
    /// `0x4bed60`: the view `t` of the way from `prev` to `self`, everything blended and the direction normalised
    /// after (as `qwc_feck::draw::View::blend`).
    pub fn blend(&self, prev: &View, t: f32) -> View {
        View {
            eye: prev.eye.lerp(self.eye, t),
            dir: prev.dir.lerp(self.dir, t).try_normalize().unwrap_or(self.dir),
            up: prev.up.lerp(self.up, t),
            fov: prev.fov + (self.fov - prev.fov) * t,
        }
    }

    /// The renderer's camera for this view (the vertical field of view 0.75 of the horizontal, a 4:3 screen's).
    pub fn camera(&self) -> Camera {
        let up = self.up.try_normalize().unwrap_or(Vec3::Y);
        Camera {
            eye: self.eye,
            target: self.eye + self.dir,
            up,
            fov_y: (self.fov * 0.75).to_radians(),
            near: 0.25,
            far: 2000.0,
        }
    }
}

/// The camera's state between ticks: its eased settings, its spring and what it has heard.
#[derive(Debug, Clone)]
pub struct MatchCamera {
    // the eased settings (`+0xd0`-`+0xec`)
    dist: f32,
    height: f32,
    pitch: f32,
    roll: f32,
    lag: f32,
    fov: f32,
    /// How much of the spring's point is used over the led point (`+0xec`, eased to 1; 0 after a cut).
    blend: f32,
    /// The roll the last step asked for (`+0x12c`).
    roll_to: f32,
    /// The spring's point and velocity (`+0x144`, `+0x154`).
    spring: (Vec3, Vec3),
    /// The last eye before its height (`+0x2c4`), the direction and the view's up (`+0x2e4`, `+0x2d4`).
    eye_base: Vec3,
    dir: Vec3,
    up: Vec3,
    /// The team in possession (`+0x21c`), the one last step saw (`+0x220`) and the ticks it has lasted (`+0x224`).
    possession: usize,
    last: usize,
    since: u32,
    /// Ticks left of the hold after a change of possession (`+0x300`).
    hold: f32,
    /// The Quaffle held last tick (`+0x313`) and the ticks since it was caught (`+0x2fc`).
    held: bool,
    since_catch: f32,
    /// Ticks left that take everything as it is (`+0x308`), and a cut asked for (`+0x30d`).
    cut: u32,
    cut_next: bool,
    /// The lock-on's ticks (`+0x304`) and its target (`+0x2a4`).
    lock: f32,
    lock_at: Vec3,
    /// The Bludger camera: on (`+0x30e`), its state (`+0x178`: 0 starting, 1 on the active player, 2 on the
    /// Bludger) and timer (`+0x17c`); the move's launch seen last tick; the other team's move (`+0x30f`).
    bludger_on: bool,
    bludger_state: u8,
    bludger_timer: f32,
    bludger_flying: Option<bool>,
    attacked: bool,
    /// The followed point last step (`+0xf0`, which the Bludger camera's start keeps).
    follow: Vec3,
    /// The remaster's framing: how far it has backed the camera off ([`MatchCamera::frame`]).
    back: f32,
    /// It turned last tick: the original's turn towards its target waits meanwhile; and its turn a tick.
    framing: bool,
    turn: f32,
    /// How much of the original's turn towards its target is used (0 while the framing turns, back to 1).
    pull: f32,
}

impl Default for MatchCamera {
    fn default() -> MatchCamera {
        // `0x47e640` and `0x480250`'s starting values (`+0x120`-`+0x134`); `0x47eb50` asks for a cut
        MatchCamera {
            dist: 12.0,
            height: 1.75,
            pitch: -0.4,
            roll: 0.0,
            lag: 0.05,
            fov: 70.0,
            blend: 0.0,
            roll_to: 0.0,
            spring: (Vec3::ZERO, Vec3::ZERO),
            eye_base: Vec3::ZERO,
            dir: Vec3::Z,
            up: Vec3::Y,
            possession: 0,
            last: 0,
            since: 0,
            hold: 0.0,
            held: false,
            since_catch: 0.0,
            cut: 0,
            cut_next: true,
            lock: 0.0,
            lock_at: Vec3::ZERO,
            bludger_on: false,
            bludger_state: 0,
            bludger_timer: 0.0,
            bludger_flying: None,
            attacked: false,
            follow: Vec3::ZERO,
            back: 0.0,
            framing: false,
            turn: 0.0,
            pull: 1.0,
        }
    }
}

/// Unit direction `d` with its sine of pitch held between `-down` and `up`, its level part kept (`+x` if none).
fn limit_pitch(d: Vec3, down: f32, up: f32) -> Vec3 {
    let y = d.y.clamp(-down, up);
    if (y - d.y).abs() < 1e-6 {
        return d;
    }
    let level = Vec3::new(d.x, 0.0, d.z).try_normalize().unwrap_or(Vec3::X);
    level * (1.0 - y * y).sqrt() + Vec3::Y * y
}

/// The eye, look, right and up of a camera `dist` back along `dir` from `point` and `height` up, pitched by `pitch`
/// (the step's own construction, without the roll).
fn aim(point: Vec3, dir: Vec3, dist: f32, height: f32, pitch: f32) -> (Vec3, Vec3, Vec3, Vec3) {
    let right = Vec3::new(dir.z, 0.0, -dir.x).try_normalize().unwrap_or(Vec3::X);
    let up = dir.cross(right).try_normalize().unwrap_or(Vec3::Y);
    let eye = point - dir * dist + up * height;
    let pitched = Quat::from_axis_angle(right, -pitch);
    (eye, pitched * dir, right, pitched * up)
}

/// `a` turned `t` of the way to `b`, both unit directions.
fn slerp(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    (Quat::IDENTITY.slerp(Quat::from_rotation_arc(a, b), t) * a).try_normalize().unwrap_or(b)
}

impl MatchCamera {
    /// Take everything as it is for the next ticks (`+0x30d`).
    pub fn cut(&mut self) {
        self.cut_next = true;
    }

    /// The remaster's framing of the players to `keep` from the aim along `dir` at `point`: the direction turned
    /// about the point towards a player nearing the picture's side, and the camera's back-off moved on. Points are
    /// measured in the view this tick would give, as fractions of the half width and height.
    fn frame(&mut self, keep: &[Vec3], point: Vec3, dir: Vec3, settle: bool) -> Vec3 {
        let (eye, look, right, up) = aim(point, dir, self.dist + self.back, self.height + self.back * RISE, self.pitch);
        let (th, tv) = ((self.fov * 0.5).to_radians().tan(), (self.fov * 0.375).to_radians().tan());
        let (mut lo, mut hi, mut high, mut behind) = (0.0f32, 0.0f32, 0.0f32, false);
        for &p in keep {
            let d = p - eye;
            let z = d.dot(look);
            if z < 1.0 {
                // behind the camera (or at it): as far out as it can be, on its side
                behind = true;
                if d.dot(right) >= 0.0 {
                    hi = hi.max(2.0);
                } else {
                    lo = lo.min(-2.0);
                }
                continue;
            }
            let (u, v) = (d.dot(right) / (z * th), d.dot(up) / (z * tv));
            (lo, hi, high) = (lo.min(u), hi.max(u), high.max(v.abs()));
        }
        // towards the side a player is leaving, as far as the other side allows
        let turn = (hi - KEEP).max(0.0) - (-KEEP - lo).max(0.0);
        let want = ((turn * th).atan() * TURN_GAIN).clamp(-MAX_TURN, MAX_TURN);
        // the turn eased in and out (at once in a cut's settling)
        self.turn = if settle { want } else { self.turn + (want - self.turn) * TURN_EASE };
        let angle = self.turn;
        let dir = (Quat::from_rotation_y(angle) * dir).try_normalize().unwrap_or(dir);
        self.framing = want != 0.0;
        // back off while they leave it both ways (the turn cannot help: it keeps the followed point in the middle),
        // up or down, out of it altogether, or behind; in again when one side is well inside (the turn sees to the
        // other), all in the picture and well within its height
        let both = (hi - KEEP).min(-KEEP - lo);
        let excess = both.max(high - KEEP_V).max((hi.max(-lo) - 1.0) * 0.5).max(0.0);
        if behind || excess > 0.0 {
            self.back = (self.back + (BACK_STEP + excess * 2.0).min(MAX_BACK_STEP)).min(MAX_BACK);
        } else if hi.min(-lo) < RELAX && hi.max(-lo) < 1.0 && high < RELAX {
            self.back = (self.back - IN_STEP.max(self.back * IN_SHARE)).max(0.0);
        }
        if std::env::var_os("QWC_CAM_FRAME_LOG").is_some() {
            eprintln!(
                "    frame lo {lo:.2} hi {hi:.2} high {high:.2} behind {behind} turn {angle:.3} back {:.1}",
                self.back
            );
        }
        dir
    }

    /// One tick of the camera on what it heard, `i`: its view at the tick's end.
    pub fn step(&mut self, i: &Inputs) -> View {
        // the handlers (`0x47ee60`: a new possession restarts the catch count and the hold; `0x4825a0`: the catch
        // count runs while the Quaffle is held, from 0 when it is caught)
        if let Some(t) = i.holder_team.filter(|&t| t != self.possession) {
            self.possession = t;
            self.since_catch = 0.0;
            self.hold = HOLD;
        }
        if i.held {
            self.since_catch = if self.held { self.since_catch + 1.0 } else { 0.0 };
        }
        self.held = i.held;
        // the target records (`0x482c60`): a lock-on for the camera's team counts up and gives its target
        match i.lock_on {
            Some(p) => {
                self.lock += 1.0;
                self.lock_at = p;
            }
            None => self.lock = 0.0,
        }
        // the Bludger move's messages: its start turns the Bludger camera on (`0x10439f8`, `0x482cf0`: state 0 for
        // a tick), its launch puts it on the Bludger (`0x1043adc`, `0x482d50`: state 2), its end turns it off
        let flying = i.bludger.map(|b| b.0);
        match (self.bludger_flying, flying) {
            (None, Some(_)) => (self.bludger_on, self.bludger_state, self.bludger_timer) = (true, 0, 1.0),
            (Some(false), Some(true)) => (self.bludger_state, self.bludger_timer) = (2, 0.0),
            (Some(_), None) => self.bludger_on = false,
            _ => {}
        }
        self.bludger_flying = flying;
        self.attacked = i.attacked;
        // the dispatcher's cut (`0x47ed80`)
        if std::mem::take(&mut self.cut_next) {
            self.cut = 5;
        } else {
            self.cut = self.cut.saturating_sub(1);
        }
        // `0x480250`: the ticks the possession has lasted
        if self.possession == self.last {
            self.since += 1;
        } else {
            self.since = 0;
            self.last = self.possession;
        }
        let ours = self.possession == i.team;
        let (quaffle, quaffle_vel) = i.quaffle;
        let (mut dist, mut height, mut zoom, mut lag) = (12.0f32, 1.75f32, 1.0f32, 0.05f32);
        let mut pitch: f32;
        let mut follow_vel = Vec3::ZERO;
        // the Quaffle in flight: followed itself, 15 behind
        let flight = |dist: &mut f32, zoom: &mut f32, follow_vel: &mut Vec3| {
            *dist = 15.0;
            *zoom = if i.boosting { 1.2 } else { 0.75 };
            *follow_vel = quaffle_vel;
            quaffle
        };
        let (follow, mut target);
        if i.keeper && i.held {
            (follow, target) = if ours { (i.ours.0, i.attack) } else { (i.theirs, i.own) };
            (pitch, dist, height, zoom) = (-0.4, 9.0, 4.5, 1.5);
        } else if ours || self.since <= GRACE {
            follow_vel = i.ours.1;
            target = i.attack;
            pitch = -0.25;
            if !i.held {
                follow = flight(&mut dist, &mut zoom, &mut follow_vel);
            } else {
                follow = i.ours.0;
                zoom = (i.ours.1.length() * 2.0).clamp(1.0, 10.0) * (50.0 / 400.0);
                if self.attacked {
                    // the other team's Bludger move on (`+0x30f`): 12 behind, 10 up, pitched 0.85 down
                    (dist, height, pitch) = (12.0, 10.0, -0.85);
                } else if ours {
                    dist = 6.0;
                    let d = (quaffle - target).length();
                    let h = if d < 35.0 { (35.0 - d) / 35.0 } else { 0.0 };
                    if h > 0.0 {
                        dist += 7.5 * h;
                        height += 8.0 * h;
                        pitch += -0.1 * h;
                    }
                    target.x += 0.05 * h;
                } else {
                    (dist, height, pitch) = (12.0, 5.0, -0.4);
                }
                if zoom > 1.0 && 30.0 - self.since_catch > 0.0 {
                    zoom += (0.75 - zoom) * (30.0 - self.since_catch) / 30.0;
                }
            }
        } else if self.lock > LOCK_AFTER || self.bludger_on {
            // looking at the midpoint of the Quaffle and its own hoops, as the defence does
            target = quaffle.lerp(i.own, 0.5);
            (dist, height, pitch) = (12.0, 5.0, -0.4);
            if self.lock > LOCK_AFTER {
                // the defensive special's lock-on: on its target, 9 behind, 11 up, pitched 0.9 down
                follow = self.lock_at;
                follow_vel = quaffle_vel;
                (dist, height, pitch) = (9.0, 11.0, -0.9);
            } else {
                // the Bludger camera (`+0x178`): a tick starting, then 30 on the active player, or on the Bludger
                // once it is launched
                if self.bludger_timer != 0.0 {
                    self.bludger_timer -= 1.0;
                    if self.bludger_timer <= 0.0 {
                        self.bludger_timer = if self.bludger_state == 1 {
                            0.0
                        } else {
                            self.bludger_state = 1;
                            BLUDGER_HOLD
                        };
                    }
                }
                let (_, bp, bv) = i.bludger.unwrap_or((false, i.ours.0, Vec3::ZERO));
                match self.bludger_state {
                    0 => follow = self.follow,
                    1 => {
                        follow = i.ours.0;
                        (dist, height, pitch, lag) = (11.0, 8.0, -0.55, 0.1);
                    }
                    _ => {
                        // part of the way from the Bludger to the Quaffle, at most 10, and far enough back to see
                        // both
                        (dist, height, pitch, lag) = (11.0, 8.0, -0.55, 0.1);
                        let to = quaffle - bp;
                        let l = to.length();
                        follow = if l > 0.0 { bp + to * ((0.4f32 * l).min(10.0) / l) } else { bp };
                        if dist < l * 1.2 {
                            let s = l * 1.2 / dist;
                            dist *= s;
                            height *= s;
                        }
                        // **[I]** `0x418620(0.5)`: half the Bludger's velocity
                        follow_vel = bv * 0.5;
                    }
                }
            }
        } else {
            let (a, b) = (i.ours.0, i.theirs);
            let d = (b - a).length();
            let f = ((d - 3.0) / 22.0).clamp(0.0, 1.0);
            let g = ((d - 2.0) / 18.0).clamp(0.0, 1.0);
            dist = (g * 12.0 + 6.0) * 1.3;
            pitch = f * -0.45 - 0.25;
            height = (f * 11.25 + 1.75) * 1.3;
            follow_vel = quaffle_vel;
            target = quaffle.lerp(i.own, 0.5);
            follow = if i.held { a.lerp(b, 0.5) } else { flight(&mut dist, &mut zoom, &mut follow_vel) };
            lag += (1.0 - lag) * (quaffle_vel.length() * 0.1 - 1.0).clamp(0.0, 2.0) / 2.0;
            if self.hold != 0.0 {
                zoom *= 1.0 - self.hold / HOLD * 0.5;
            }
            let d = (quaffle - target).length();
            if d < 20.0 {
                let h = (20.0 - d) / 20.0;
                dist += h * 3.0;
                height += h * 7.0;
                pitch += h * -0.2;
                target.x += h * 0.05;
            }
        }
        // on its own attack too, a lock-on takes the followed point (`+0x2a4`)
        let follow = if (ours || self.since <= GRACE) && !(i.keeper && i.held) && self.lock > LOCK_AFTER {
            self.lock_at
        } else {
            follow
        };
        self.follow = follow;
        // the settings eased, or taken as they are in a cut
        if self.cut == 0 {
            self.dist += (dist - self.dist) * 0.1;
            self.height += (height - self.height) * 0.05;
            self.pitch += (pitch - self.pitch) * 0.05;
            self.lag += (lag - self.lag) * 0.05;
            self.fov += (70.0 - self.fov) * 0.05;
            self.roll += (self.roll_to - self.roll) * 0.05;
            self.blend += (1.0 - self.blend) * 0.05;
        } else {
            follow_vel = Vec3::ZERO;
            self.spring = (follow, Vec3::ZERO);
            self.blend = 0.0;
            (self.dist, self.height, self.pitch, self.lag, self.fov, self.roll) = (dist, height, pitch, lag, 70.0, 0.0);
        }
        // the led point and its spring (`0x47f5f0`)
        let goal = follow + follow_vel * 0.12;
        let (mut p, mut pv) = self.spring;
        let off = p - goal;
        let len = off.length();
        let pull = if len > 0.0 { off / len * -(len.min(15.0) * 400.0 * zoom) } else { Vec3::ZERO };
        pv += (pull - pv * 50.0 - (pv - follow_vel) * 100.0) / 25.0 * STEP;
        p += pv * STEP;
        self.spring = (p, pv);
        let point = goal.lerp(p, self.blend);
        // the direction: towed from the last eye, turned towards the target by the lag
        let mut dir = (target - point).try_normalize().unwrap_or(self.dir);
        if self.cut == 0 {
            let tow = (point - self.eye_base).try_normalize().unwrap_or(dir);
            // the original's turn towards its target waits while the framing turns, and comes back slowly
            self.pull = if self.framing { 0.0 } else { (self.pull + PULL_BACK).min(1.0) };
            dir = slerp(tow, dir, self.lag * self.pull);
            if self.hold != 0.0 {
                dir = slerp(self.dir, dir, 1.0 - self.hold / HOLD);
            }
        }
        // **[I]** never steeply up or down: flying back past the camera, the towed direction can stand nearly on
        // end (the original's has no such limit that is found)
        let mut dir = limit_pitch(dir, DIR_DOWN, DIR_UP);
        // the remaster's framing of the players to keep (not in a cut, which takes everything as it is)
        let keep: Vec<Vec3> = i.keep.iter().flatten().copied().collect();
        if keep.is_empty() {
            (self.back, self.framing, self.turn) = (0.0, false, 0.0);
        } else if self.cut == 0 {
            dir = self.frame(&keep, point, dir, false);
        } else {
            // a cut takes everything as it is: the framing too, settled at once
            for _ in 0..CUT_FRAMING {
                dir = self.frame(&keep, point, dir, true);
            }
        }
        self.dir = dir;
        self.eye_base = point - dir * (self.dist + self.back);
        let right = Vec3::new(dir.z, 0.0, -dir.x).try_normalize().unwrap_or(Vec3::X);
        let up = dir.cross(right).try_normalize().unwrap_or(Vec3::Y);
        // the roll: 5 times the turn of the up since the last view
        self.roll_to = (5.0 * up.cross(self.up).dot(dir)).clamp(-1.5, 1.5);
        let mut eye = self.eye_base + up * (self.height + self.back * RISE);
        // **[I]** never under the ground looking up: the eye kept 1 above the floor (the original's camera has no
        // such test; the passing challenge's look to its far end, 15 up, pulls it down near there)
        eye.y = eye.y.max(i.floor + EYE_ABOVE_FLOOR);
        // looking along the direction pitched about its right, rolled about the look
        let pitched = Quat::from_axis_angle(right, -self.pitch);
        let (look, up) = (pitched * dir, pitched * up);
        let rolled = Quat::from_axis_angle(look, -self.roll);
        let (look, up) = (rolled * look, rolled * up);
        self.up = up;
        if self.hold > 0.0 {
            self.hold = (self.hold - 1.0).max(0.0);
        }
        if std::env::var_os("QWC_CAM_DEBUG").is_some() {
            eprintln!(
                "cam team {} possession {} held {} follow {follow:?} target {target:?} point {point:?} eye {eye:?} \
                 look {look:?} dist {:.1} h {:.1} pitch {:.2} roll {:.3} lag {:.2} zoom {zoom:.2}",
                i.team, self.possession, i.held, self.dist, self.height, self.pitch, self.roll, self.lag
            );
        }
        View { eye, dir: look, up, fov: self.fov }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Team 0's carrier level with the hoops, attacking towards -x along the pitch, its hoops at +x.
    fn inputs() -> Inputs {
        let at = Vec3::new(0.0, 15.0, 0.0);
        Inputs {
            team: 0,
            holder_team: Some(0),
            held: true,
            quaffle: (at, Vec3::ZERO),
            ours: (at, Vec3::ZERO),
            keeper: false,
            theirs: Vec3::new(10.0, 15.0, 5.0),
            boosting: false,
            attack: Vec3::new(-97.5, 15.0, 0.0),
            own: Vec3::new(97.5, 15.0, 0.0),
            lock_on: None,
            bludger: None,
            attacked: false,
            floor: -1000.0,
            keep: [None; 2],
        }
    }

    /// The other team holds the Quaffle at `(-20, 15, 0)`; the camera's team defends.
    fn defending() -> Inputs {
        let mut i = inputs();
        i.holder_team = Some(1);
        i.quaffle = (Vec3::new(-20.0, 15.0, 0.0), Vec3::ZERO);
        i.theirs = i.quaffle.0;
        i
    }

    /// How far into `view` point `p` is: the larger of its fractions of the half width and height (10 behind).
    fn into(view: &View, p: Vec3) -> f32 {
        let d = p - view.eye;
        let z = d.dot(view.dir);
        if z <= 0.0 {
            return 10.0;
        }
        let right = view.dir.cross(view.up).normalize();
        let up = right.cross(view.dir).normalize();
        let (th, tv) = ((view.fov * 0.5).to_radians().tan(), (view.fov * 0.375).to_radians().tan());
        (d.dot(right) / (z * th)).abs().max((d.dot(up) / (z * tv)).abs())
    }

    #[test]
    fn the_framing_turns_to_a_mate_off_to_one_side_and_backs_off_for_mates_on_both() {
        // the carrier attacking towards -x; a mate level with him 25 across: out of the original's picture
        let mut i = inputs();
        let side = Vec3::new(0.0, 15.0, 25.0);
        let mut cam = MatchCamera::default();
        let original = settle(&mut cam, &i, 100);
        assert!(into(&original, side) > 1.0, "{}", into(&original, side));
        i.keep = [Some(side), None];
        let mut cam = MatchCamera::default();
        let view = settle(&mut cam, &i, 400);
        assert!(into(&view, side) <= KEEP + 0.05, "{}", into(&view, side));
        assert!(cam.back < 1.0, "once turned, turning is enough for one side: {}", cam.back);
        // and one on each side, both in: backed off
        let other = Vec3::new(0.0, 15.0, -25.0);
        i.keep = [Some(side), Some(other)];
        let view = settle(&mut cam, &i, 200);
        assert!(into(&view, side) <= 1.0 && into(&view, other) <= 1.0, "{} {}", into(&view, side), into(&view, other));
        assert!(cam.back > 5.0, "{}", cam.back);
        // gone: the original's camera again
        i.keep = [None; 2];
        settle(&mut cam, &i, 1);
        assert_eq!(cam.back, 0.0);
    }

    #[test]
    fn the_defensive_special_s_lock_on_takes_the_camera_to_its_target_after_four_ticks() {
        let mut i = defending();
        let mut cam = MatchCamera::default();
        settle(&mut cam, &i, 100);
        i.lock_on = Some(Vec3::new(-20.0, 15.0, 0.0));
        for _ in 0..4 {
            cam.step(&i);
        }
        // before: the midpoint of the two active players
        assert_eq!(cam.follow, i.ours.0.lerp(i.theirs, 0.5));
        cam.step(&i);
        assert_eq!(cam.follow, Vec3::new(-20.0, 15.0, 0.0));
        let v = settle(&mut cam, &i, 200);
        // settled 9 behind and 11 up the eye's base, pitched 0.9 down
        assert!(v.eye.y > 15.0 + 10.0 && (cam.dist - 9.0).abs() < 0.01 && (cam.pitch + 0.9).abs() < 0.01);
        i.lock_on = None;
        cam.step(&i);
        assert_eq!(cam.lock, 0.0);
    }

    #[test]
    fn the_bludger_camera_watches_the_summoner_then_the_bludger() {
        let mut i = defending();
        let mut cam = MatchCamera::default();
        settle(&mut cam, &i, 50);
        i.bludger = Some((false, Vec3::new(30.0, 20.0, 0.0), Vec3::ZERO));
        // the start's state 0 runs out in the same tick (the message comes before the camera's step)
        cam.step(&i);
        assert!(cam.bludger_on);
        assert_eq!((cam.bludger_state, cam.bludger_timer), (1, 30.0));
        assert_eq!(cam.follow, i.ours.0);
        // launched: part of the way from the Bludger to the Quaffle (0.4 of 50, at most 10)
        i.bludger = Some((true, Vec3::new(30.0, 15.0, 0.0), Vec3::ZERO));
        cam.step(&i);
        assert_eq!(cam.bludger_state, 2);
        assert!((cam.follow - Vec3::new(20.0, 15.0, 0.0)).length() < 1e-4, "{:?}", cam.follow);
        i.bludger = None;
        cam.step(&i);
        assert!(!cam.bludger_on);
    }

    fn settle(cam: &mut MatchCamera, i: &Inputs, ticks: usize) -> View {
        (0..ticks).map(|_| cam.step(i)).last().expect("ticks")
    }

    #[test]
    fn with_the_quaffle_it_sits_close_behind_the_carrier_looking_at_the_hoops_he_attacks() {
        let i = inputs();
        let v = settle(&mut MatchCamera::default(), &i, 200);
        // 6 behind (the +x side), 1.75 up, looking down the pitch pitched 0.25 down
        let back = v.eye - i.ours.0;
        assert!((back.x - 6.0).abs() < 0.3 && back.z.abs() < 0.3, "{v:?}");
        assert!((back.y - 1.75).abs() < 0.3, "{v:?}");
        assert!(v.dir.x < -0.9 && (v.dir.y + 0.25f32.sin()).abs() < 0.05, "{v:?}");
        assert!((v.camera().fov_y.to_degrees() - 52.5).abs() < 0.01);
    }

    #[test]
    fn a_pass_is_followed_along_its_flight_without_turning_round() {
        // a pass from the carrier to a team-mate 30 on and 20 across, over a second, caught at the end
        let mut i = inputs();
        let mut cam = MatchCamera::default();
        settle(&mut cam, &i, 200);
        let (from, to) = (i.ours.0, Vec3::new(-30.0, 22.0, 20.0));
        let ticks = 24;
        for k in 0..=ticks + 48 {
            let t = (k as f32 / ticks as f32).min(1.0);
            let ball = from.lerp(to, t);
            let caught = k >= ticks;
            i.held = caught;
            i.holder_team = caught.then_some(0);
            i.quaffle = (ball, if caught { Vec3::ZERO } else { (to - from) * (24.0 / ticks as f32) });
            // the receiver is the active player from the throw (`0x440620`)
            i.ours = (to, Vec3::ZERO);
            let v = cam.step(&i);
            // still looking towards the hoops it attacks, never back at its own
            assert!(v.dir.x < -0.3, "tick {k}: {v:?}");
            if !caught {
                // behind the ball as it flies
                assert!(v.eye.x > ball.x, "tick {k}: {v:?} ball {ball:?}");
            }
        }
    }

    #[test]
    fn on_defence_it_frames_the_two_active_players_and_looks_between_the_quaffle_and_its_hoops() {
        let mut i = inputs();
        i.holder_team = Some(1);
        i.theirs = Vec3::new(20.0, 15.0, 0.0);
        i.quaffle = (i.theirs, Vec3::ZERO);
        let v = settle(&mut MatchCamera::default(), &i, 300);
        // the point between them, the target the Quaffle's way to its own hoops: looking towards +x, pitched down
        assert!(v.dir.x > 0.7 && v.dir.y < -0.4, "{v:?}");
        // d = 20 between them: g = 1, f = 17/22: 23.4 behind and 13.6 up
        let mid = i.ours.0.lerp(i.theirs, 0.5);
        assert!((v.eye.x - (mid.x - 23.4)).abs() < 1.0 && (v.eye.y - (mid.y + 13.6)).abs() < 1.0, "{v:?}");
    }

    #[test]
    fn the_other_teams_keeper_holding_it_puts_the_camera_behind_him_at_once() {
        // after a goal by team 0, team 1's keeper (at -91.5) restarts: a cut, and at once 9 behind and 4.5 up him,
        // looking down the pitch to team 0's own hoops (+x)
        let mut i = inputs();
        let keeper = Vec3::new(-91.5, 20.0, -3.0);
        (i.holder_team, i.held, i.keeper, i.theirs, i.quaffle) = (Some(1), true, true, keeper, (keeper, Vec3::ZERO));
        let mut cam = MatchCamera::default();
        settle(&mut cam, &inputs(), 100);
        cam.cut();
        let v = cam.step(&i);
        assert!(v.dir.x > 0.8, "{v:?}");
        let back = keeper - v.eye;
        assert!((back.x - 9.0).abs() < 1.0 && (v.eye.y - keeper.y - 4.5).abs() < 1.0, "{v:?}");
    }

    #[test]
    fn the_roll_banks_into_a_turn_and_settles() {
        let mut i = inputs();
        let mut cam = MatchCamera::default();
        settle(&mut cam, &i, 200);
        // the carrier sweeps round a circle: the camera keeps turning one way
        let mut most = 0.0f32;
        let mut last = cam.dir;
        for k in 0..48 {
            let a = k as f32 * 0.05;
            let at = Vec3::new(-20.0 * a.sin(), 15.0, 20.0 * a.cos() - 20.0);
            i.ours = (at, Vec3::new(-a.cos(), 0.0, -a.sin()) * 24.0);
            i.quaffle = (at, Vec3::ZERO);
            let v = cam.step(&i);
            // turning left (towards +x from +z, about +y) leans the up left
            let turn = last.cross(cam.dir).y;
            let lean = v.up.dot(Vec3::Y.cross(v.dir).normalize());
            if turn.abs() > 1e-3 && cam.roll.abs() > 1e-3 {
                assert_eq!(turn > 0.0, lean > 0.0, "tick {k}: turn {turn} lean {lean}");
            }
            last = cam.dir;
            most = most.max(cam.roll.abs());
        }
        assert!(most > 1e-3 && most < 0.5, "{most}");
        // standing still again, it levels out
        i.ours.1 = Vec3::ZERO;
        settle(&mut cam, &i, 400);
        assert!(cam.roll.abs() < 1e-3, "{}", cam.roll);
    }

    #[test]
    fn between_ticks_the_view_blends_from_the_last_to_the_next() {
        let a = View { eye: Vec3::ZERO, dir: Vec3::Z, up: Vec3::Y, fov: 70.0 };
        let b = View { eye: Vec3::new(2.0, 0.0, 0.0), dir: Vec3::X, up: Vec3::Y, fov: 60.0 };
        assert_eq!(b.blend(&a, 0.0), a);
        let half = b.blend(&a, 0.5);
        assert_eq!(half.eye, Vec3::new(1.0, 0.0, 0.0));
        assert!((half.dir.length() - 1.0).abs() < 1e-6 && (half.dir.x - half.dir.z).abs() < 1e-6);
        assert_eq!(half.fov, 65.0);
        assert_eq!(b.blend(&a, 1.0).eye, b.eye);
    }

    #[test]
    fn hidden_hoops_leave_the_camera_looking_down_the_pitch() {
        let console = qwc_sim::console::Console::new();
        let mut m = Match::new(&console, qwc_sim::rng::Rand::new(1));
        let i = Inputs::of(&m, 0);
        assert_eq!(i.attack.x.signum(), -1.0);
        // the passing challenge's: under the pitch and hidden
        for h in &mut m.hoops {
            h.pos = [0.0, -50.0, 0.0];
        }
        m.hoops_shown = false;
        let i = Inputs::of(&m, 0);
        assert_eq!((i.attack, i.own), (Vec3::new(-97.5, 15.0, 0.0), Vec3::new(97.5, 15.0, 0.0)));
    }
}
