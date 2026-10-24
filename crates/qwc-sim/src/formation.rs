//! Off-ball chaser movement: the Formotion system (`docs/GAMEPLAY.md`, "Chasers off the ball: Formotions";
//! per-team block at `0x101a800 + team * 0xd50`).
//!
//! Each team has a formation state (one of [`STATES`]), changed by possession and passes, and three slots, one per
//! chaser. The team's active player is the leader and has no slot (unless it is the keeper). Every tick a slot that
//! has finished its formotion, or whose state changed, picks a new FORMOTION from the state's transfer list
//! ([`pick`], one `rand()`), and runs it: a few waypoints in a unit frame, scaled per state, placed relative to a
//! smoothed point that follows the leader (or, when marking, an opponent), joined by cubic segments, with the
//! chaser's step capped at 30 units per second.
//!
//! Traced: the state changes (`0x440240`, `0x431f10`, the selector's transitions), slot assignment (`0x440400`,
//! `0x42d060`, `0x42d130`), the selector `0x42d2b0`, the picker `0x42cee0`, the trail point (`0x42db10`), the frame
//! (`0x42ca90`, `0x42cb20`, `0x42cdb0`), the start (`0x42df00`), the waypoint mapping (`0x42eac0`), the runner
//! (`0x42ed00`, with the players' facing: [`face`], [`turn_towards`]) and the segment advance (`0x42e380`).
//! Not modelled: the cruising height (the floor is flat here), `Formotions.TweakTime` for the selector's states
//! (retail files only tweak `AI_DEF`, the AI defender's approach: [`Runner::start_turned`], [`crate::approach`]) and
//! the version-1 `timesteps * 12` rule.

use std::rc::Rc;

use qwc_formats::formotion::{Database, Formotion, STATES};

use crate::fmath::{acos, cos, sin};
use crate::keeper::bezier;
use crate::rng::{trunc, Rand};
use crate::vec3::{add, dot, len, scale, sub, unit, Vec3};

pub use qwc_formats::formotion::STATES as STATE_NAMES;

/// No state (`0x12`).
pub const NONE: usize = 0x12;
pub const FREE: usize = 0;
pub const FREE_TO_DEFEND: usize = 1;
pub const FREE_TO_ATTACK: usize = 2;
pub const ATTACK_TO_ATTACK: usize = 3;
pub const ATTACK_TO_DEFEND: usize = 4;
pub const DEFEND_TO_DEFEND: usize = 5;
pub const DEFEND_TO_ATTACK: usize = 6;
pub const ATTACK_PASS: usize = 7;
pub const DEFEND_PASS: usize = 8;
/// The AI defender's approaches (`eFT_AIDefend`, the list `0x453980` reads: database `+0xb64`).
pub const AI_DEFEND: usize = 9;
pub const KEEPER_ATTACK: usize = 10;
pub const KEEPER_DEFEND: usize = 11;

/// The trail point stays within this of the reference player (`0x669040`).
pub const TRAIL: f64 = 4.0;
/// The chaser's step cap in units per second (`0x669044`, slot `+0x9c`).
pub const STEP_CAP: f64 = 30.0;
/// The frame's follow caps: Free (`0x669050`), the other leader states (`0x66904c`), marking (`0x669048`).
pub const FOLLOW_FREE: f64 = 5.0;
pub const FOLLOW: f64 = 20.0;
pub const FOLLOW_MARK: f64 = 30.0;
/// Frame smoothing weight (`0x669054`) and stage count.
pub const SMOOTH: f64 = 0.9;
pub const STAGES: usize = 20;
/// Picker flag bits: excluded (`0x10`), preferred (8), and the bits the last pass refuses (`0x60`).
pub const AVOID: u32 = 0x10;
pub const PREFERRED: u32 = 8;
pub const GOAL_BITS: u32 = 0x60;

/// One database as the game holds it: per state, the formotions its transfer list names, in list order.
#[derive(Debug, Clone, Default)]
pub struct Db {
    pub lists: Vec<Vec<Rc<Formotion>>>,
}

impl Db {
    pub fn new(d: &Database) -> Db {
        let lists = (0..STATES.len())
            .map(|s| {
                d.list(s)
                    .map(|l| l.formotions.iter().filter_map(|n| d.formotion(n)).map(|f| Rc::new(f.clone())).collect())
                    .unwrap_or_default()
            })
            .collect();
        Db { lists }
    }

    fn list(&self, state: usize) -> &[Rc<Formotion>] {
        self.lists.get(state).map_or(&[], Vec::as_slice)
    }
}

/// The picker's arguments: required bits, excluded bits, preferred bits and the pair index (-1 for any).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Filter {
    pub need: u32,
    pub avoid: u32,
    pub want: u32,
    pub lpair: i32,
}

/// The picker `0x42cee0`. The team list is searched before the global one (only the global one when `team` is
/// `None`, as in the passing and combo challenges: [`Team::select`]'s `mode` 1 and 6). Three passes, each keeping records with `(flags & need) == need` and no
/// `avoid` bit: first those with the pair index `lpair` (any when -1), then those with a `want` bit, then those
/// without the bits `0x60`. The first database and pass with candidates wins; a candidate with flag 8 is taken
/// without a draw (the last one), otherwise one `rand()` picks. With none, the first examined list's first entry.
pub fn pick(rng: &mut Rand, team: Option<&Db>, global: &Db, state: usize, filter: Filter) -> Option<Rc<Formotion>> {
    let Filter { need, avoid, want, lpair } = filter;
    let dbs: Vec<&Db> = team.into_iter().chain([global]).collect();
    for pass in 0..3 {
        for db in &dbs {
            let found: Vec<&Rc<Formotion>> = db
                .list(state)
                .iter()
                .filter(|f| f.flags & need == need && f.flags & avoid == 0)
                .filter(|f| match pass {
                    0 => lpair == -1 || f.lpair == lpair,
                    1 => f.flags & want != 0,
                    _ => f.flags & GOAL_BITS == 0,
                })
                .collect();
            if found.is_empty() {
                continue;
            }
            if let Some(f) = found.iter().rev().find(|f| f.flags & PREFERRED != 0) {
                return Some(Rc::clone(f));
            }
            let n = found.len() as i32;
            let i = trunc(rng.rand() as f64 * n as f64 / 32767.0).min(n - 1);
            return Some(Rc::clone(found[i.max(0) as usize]));
        }
    }
    dbs[0].list(state).first().cloned()
}

/// The state a team asks for on a change of possession (`0x440240`): `attacking` for the team that now has the
/// ball. A keeper holding the Quaffle gives KeeperAttack / KeeperDefend. `None` keeps the current request.
pub fn on_possession(current: usize, attacking: bool, keeper_holds: bool) -> usize {
    if keeper_holds {
        return if attacking { KEEPER_ATTACK } else { KEEPER_DEFEND };
    }
    match (attacking, current) {
        (false, FREE | FREE_TO_DEFEND | FREE_TO_ATTACK | NONE) => FREE_TO_DEFEND,
        (false, ATTACK_TO_ATTACK | ATTACK_TO_DEFEND | DEFEND_TO_ATTACK | KEEPER_ATTACK) => ATTACK_TO_DEFEND,
        (false, DEFEND_TO_DEFEND) => DEFEND_TO_DEFEND,
        (false, KEEPER_DEFEND) => DEFEND_PASS,
        (true, FREE | FREE_TO_DEFEND | FREE_TO_ATTACK | NONE) => FREE_TO_ATTACK,
        (true, ATTACK_TO_ATTACK | ATTACK_TO_DEFEND) => ATTACK_TO_ATTACK,
        (true, DEFEND_TO_DEFEND | DEFEND_TO_ATTACK | KEEPER_DEFEND) => DEFEND_TO_ATTACK,
        (true, KEEPER_ATTACK) => ATTACK_PASS,
        _ => NONE,
    }
}

/// The selector's one-shot transitions (`0x42d260` and the end of `0x42d2b0`): the "to defend" states and
/// DefendPass become DefendToDefend, the "to attack" states and AttackPass become AttackToAttack.
pub fn next_state(state: usize) -> usize {
    match state {
        FREE_TO_DEFEND | ATTACK_TO_DEFEND | DEFEND_PASS => DEFEND_TO_DEFEND,
        FREE_TO_ATTACK | DEFEND_TO_ATTACK | ATTACK_PASS => ATTACK_TO_ATTACK,
        _ => NONE,
    }
}

/// The per-state scale vectors of the selector: `A` for every waypoint, `B` extra for forward waypoints.
pub fn scales(state: usize) -> Option<(Vec3, Vec3)> {
    let one = [1.0; 3];
    match state {
        FREE | FREE_TO_DEFEND | ATTACK_TO_DEFEND | DEFEND_PASS => Some(([10.0, 8.0, 10.0], one)),
        FREE_TO_ATTACK | ATTACK_TO_ATTACK | DEFEND_TO_ATTACK | ATTACK_PASS | KEEPER_ATTACK => {
            Some(([6.0, 4.0, 6.0], [18.0, 12.0, 12.0]))
        }
        DEFEND_TO_DEFEND => Some(([8.0; 3], one)),
        KEEPER_DEFEND => Some(([25.0, 10.0, 25.0], one)),
        _ => None,
    }
}

/// A waypoint's offset in the slot's frame (`0x42eac0`): with `w = (-z, y, x)` (the file's `z` is forward, `x` to
/// the side), `A * w`, plus `z * B * w` when `z > 0`; its velocity `A * (-vz, vy, vx)`.
pub fn map_waypoint(pos: [f32; 3], vel: [f32; 3], a: Vec3, b: Vec3) -> (Vec3, Vec3) {
    let (x, y, z) = (pos[0] as f64, pos[1] as f64, pos[2] as f64);
    let w = [-z, y, x];
    let mut p = [a[0] * w[0], a[1] * w[1], a[2] * w[2]];
    if z > 0.0 {
        p = add(p, [b[0] * w[0] * z, b[1] * w[1] * z, b[2] * w[2] * z]);
    }
    let v = [-a[0] * vel[2] as f64, a[1] * vel[1] as f64, a[2] * vel[0] as f64];
    (p, v)
}

/// `v` turned about `y` by `(cos, sin)`.
fn rotate([c, s]: [f64; 2], v: Vec3) -> Vec3 {
    [c * v[0] - s * v[2], v[1], s * v[0] + c * v[2]]
}

/// The reference frame (`0x42ca90`): stages of position and velocity. Stage 0 follows the input at most
/// `cap / 24` per tick from stage 1; each later stage moves 0.9 of the way to the one before it every tick.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub cap: f64,
    pub pos: Vec<Vec3>,
    pub vel: Vec<Vec3>,
}

impl Frame {
    pub fn new(pos: Vec3, vel: Vec3, cap: f64) -> Frame {
        // 20 stages are initialised; the 21st (written by the first update) starts at zero
        let mut p = vec![pos; STAGES];
        let mut v = vec![vel; STAGES];
        p.push([0.0; 3]);
        v.push([0.0; 3]);
        Frame { cap, pos: p, vel: v }
    }

    /// `0x42cb20` with the follow input `(p, v)`.
    pub fn update(&mut self, p: Vec3, v: Vec3) {
        for s in (1..=STAGES).rev() {
            self.pos[s] = add(scale(self.pos[s - 1], SMOOTH), scale(self.pos[s], 1.0 - SMOOTH));
            self.vel[s] = add(scale(self.vel[s - 1], SMOOTH), scale(self.vel[s], 1.0 - SMOOTH));
        }
        let mut d = sub(p, self.pos[1]);
        let max = self.cap / 24.0;
        let l = len(d);
        if l > max {
            d = scale(d, max / l);
        }
        self.pos[0] = add(self.pos[1], d);
        self.vel[0] = [v[0], 0.0, v[2]];
    }

    /// `0x42cdb0` with the delay index 0 (the delay globals have no writer): the head stage.
    pub fn point(&self) -> (Vec3, Vec3) {
        (self.pos[0], self.vel[0])
    }
}

/// One chaser slot (`0x420` bytes from block `+0xb0`).
#[derive(Debug, Clone, Default)]
pub struct Slot {
    pub active: bool,
    /// `+0x90`: 1 left, 2 right, 4 both sides taken.
    pub side: u32,
    pub player: usize,
    /// `+0x98`: the player the frame follows.
    pub reference: usize,
    pub done: bool,
    pub formotion: Option<Rc<Formotion>>,
    waypoint: usize,
    /// `+0x354`, `+0x410`, `+0x414`: tick counter, segment start and end; `n` is `trunc(timesteps * mult) + 1`.
    tick: i32,
    seg_start: i32,
    seg_end: i32,
    n: i32,
    a: Vec3,
    b: Vec3,
    /// Start and end of the current segment in the slot's frame: (position, velocity).
    start: (Vec3, Vec3),
    end: (Vec3, Vec3),
    pub frame: Option<Frame>,
    /// The trail point (block `+0xd10 + 16 k`).
    pub trail: Vec3,
}

/// What the runner tells a player this tick: a one-tick movement command.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Step {
    pub player: usize,
    pub pos: Vec3,
    pub vel: Vec3,
    /// The heading its waypoint gives (flag 1), in the world; else the player faces along `vel` ([`face`]).
    pub heading: Option<Vec3>,
    /// Where the formotion's path is this tick and how fast it moves (for the remaster's momentum, [`glide`]).
    pub path: (Vec3, Vec3),
}

/// What a slot's run gives a player: his step's position and velocity, his waypoint's heading and the path's point
/// and velocity (see [`Step`]).
type Moved = (Vec3, Vec3, Option<Vec3>, (Vec3, Vec3));

/// The remaster's momentum (not the original's: its runner puts the player on the path's point each tick, capped
/// at 30 a second, so a path that doubles back turns him round in a few ticks): a player at `pos` moving at `vel`
/// aims at the path's own velocity plus a pull of [`PULL`] a second towards its point, no faster than `cap`, and
/// changes his velocity by at most `accel` units a second each second. His new position and velocity, a tick on.
pub fn glide(pos: Vec3, vel: Vec3, path: (Vec3, Vec3), cap: f64, accel: f64) -> (Vec3, Vec3) {
    let (point, path_vel) = path;
    let mut want = add(path_vel, scale(sub(point, pos), PULL));
    let l = len(want);
    if l > cap {
        want = scale(want, cap / l);
    }
    let dv = sub(want, vel);
    let max = accel / 24.0;
    let l = len(dv);
    let vel = if l > max { add(vel, scale(dv, max / l)) } else { want };
    (add(pos, scale(vel, 1.0 / 24.0)), vel)
}

/// How hard [`glide`] pulls a player back onto the path: its gap closed at this rate a second.
pub const PULL: f64 = 1.0;
/// The remaster's acceleration for [`glide`], units a second each second (with [`PULL`], the passing challenge's
/// mates turn back within half a second a quarter as often as the original's, at the same spacing).
pub const MOMENTUM: f64 = 15.0;

/// The runner's turn of a player's facing each tick (`0x42c570` with `[0x669024]`): a slerp of its orientation
/// this far towards the target.
pub const TURN: f64 = 0.3;
/// The sine of the steepest the runner faces a player (`[0x669028]`, 30 degrees).
pub const PITCH_LIMIT: f64 = 0.5;

/// The way the runner faces a player stepping at `vel` (`0x42ed00`): along it, or along its current facing `fwd`
/// when it hardly moves, no steeper than [`PITCH_LIMIT`] (the level part kept, or `fwd`'s, or `+z`).
pub fn face(vel: Vec3, fwd: Vec3) -> Vec3 {
    let d = if dot(vel, vel) < 1e-4 { fwd } else { vel };
    let d = unit(d);
    if d[1].abs() <= PITCH_LIMIT {
        return d;
    }
    let mut level = [d[0], 0.0, d[2]];
    if level[0] * level[0] + level[2] * level[2] < 0.001 {
        level = [fwd[0], 0.0, fwd[2]];
    }
    if level[0] * level[0] + level[2] * level[2] < 0.001 {
        level = [0.0, 0.0, 1.0];
    }
    let k = (1.0 - PITCH_LIMIT * PITCH_LIMIT).sqrt();
    let l = unit(level);
    [l[0] * k, PITCH_LIMIT.copysign(d[1]), l[2] * k]
}

/// `a` turned `t` of the way to `b` (unit directions, the shorter way round).
pub fn turn_towards(a: Vec3, b: Vec3, t: f64) -> Vec3 {
    let c = dot(a, b).clamp(-1.0, 1.0);
    let angle = acos(c);
    if angle < 1e-6 {
        return b;
    }
    let s = sin(angle);
    if s < 1e-6 {
        // opposite: through any perpendicular, the level one when there is one
        let p = unit([-a[2], 0.0, a[0]]);
        let (x, y) = (cos(angle * t), sin(angle * t));
        return unit(add(scale(a, x), scale(p, y)));
    }
    let (wa, wb) = (sin((1.0 - t) * angle) / s, sin(t * angle) / s);
    unit(add(scale(a, wa), scale(b, wb)))
}

/// A player as the formation code reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Body {
    pub pos: Vec3,
    pub vel: Vec3,
    /// No movement command pending (the mover record's idle byte `+0x678`).
    pub idle: bool,
}

/// A team's formation block.
#[derive(Debug, Clone)]
pub struct Team {
    pub team_db: Option<Rc<Db>>,
    pub global: Rc<Db>,
    /// `+0xd40`, `+0xd44`.
    pub current: usize,
    pub requested: usize,
    /// `+0xa0`.
    pub leader: Option<usize>,
    pub slots: [Slot; 3],
    /// The frame's turn about `y` as `(cos, sin)` (the matrix passed to `0x42ce60`): none for team 0, a half turn
    /// (mirroring `x` and `z`) for team 1 (`0x4306c0`); the challenge game sets its own ([`Team::set_heading`]).
    pub turn: [f64; 2],
    /// The match description's mode (`[0x1018f98]`): 0 in a match, the challenge's `MatchSubType` in one.
    pub mode: u32,
}

impl Team {
    pub fn new(global: Rc<Db>, team_db: Option<Rc<Db>>, team: usize) -> Team {
        Team {
            team_db,
            global,
            current: NONE,
            requested: FREE,
            leader: None,
            slots: Default::default(),
            turn: if team == 1 { [-1.0, 0.0] } else { [1.0, 0.0] },
            mode: 0,
        }
    }

    /// Orient the frame so that its forward (team 0's `-x`) is `dir` (across the ground).
    pub fn set_heading(&mut self, dir: Vec3) {
        let l = (dir[0] * dir[0] + dir[2] * dir[2]).sqrt();
        if l > 0.0 {
            self.turn = [-dir[0] / l, -dir[2] / l];
        }
    }

    /// `0x42d060`: activate slot `k` for `player`, taking the free side (1 when no side is taken, 4 when both are).
    pub fn activate(&mut self, k: usize, player: usize) {
        if self.slots[k].active {
            return;
        }
        let taken = self.taken_sides();
        let side = match taken {
            0 => 1,
            3 => 4,
            t => !t & 3,
        };
        let s = &mut self.slots[k];
        s.side = side;
        s.active = true;
        s.player = player;
    }

    fn taken_sides(&self) -> u32 {
        self.slots.iter().filter(|s| s.active).fold(0, |a, s| a | (s.side & 3))
    }

    /// `0x42d130`: free slot `k`; slots left with "both" (4) take a free side.
    pub fn release(&mut self, k: usize) {
        if !self.slots[k].active {
            return;
        }
        // `0x42c8a0` clears the slot's player, formotion and timers; the frame and trail stay
        let (trail, frame) = (self.slots[k].trail, self.slots[k].frame.take());
        self.slots[k] = Slot { trail, frame, ..Slot::default() };
        let taken = self.taken_sides();
        for s in self.slots.iter_mut().filter(|s| s.active && s.side & 3 == 0) {
            if taken == 3 {
                continue;
            }
            s.side &= !4;
            s.side |= if taken == 0 { 1 } else { !taken & 3 };
        }
    }

    /// `0x440400`: the active player leads. A keeper leader leaves all three chasers in slots; a chaser leader
    /// leaves its own slot.
    pub fn assign(&mut self, chasers: [usize; 3], active: Option<usize>, keeper_active: bool) {
        let Some(a) = active else { return };
        self.leader = Some(a);
        if keeper_active {
            for (k, &c) in chasers.iter().enumerate() {
                self.activate(k, c);
            }
            return;
        }
        if let Some(k) = chasers.iter().position(|&c| c == a) {
            self.release(k);
        }
        for (k, &c) in chasers.iter().enumerate() {
            if c != a {
                self.activate(k, c);
            }
        }
    }

    /// The partner slot of `side` in this (the opposing) block (`0x42d1e0`): the first active slot whose side bits
    /// lie within `side` (any active slot when `side` has no left/right bit), else the first active slot with
    /// neither.
    fn partner(&self, side: u32) -> Option<&Slot> {
        let first = self.slots.iter().find(|s| s.active && (side & 3 == 0 || !side & s.side & 3 == 0));
        first.or_else(|| self.slots.iter().find(|s| s.active && s.side & 3 == 0))
    }

    /// The selector `0x42d2b0` for this team. `other` is the opposing block; `bodies` the players; `want` the
    /// preference bits (`0x20` leader in the attacked goal area, `0x40` leader in its own with the keeper
    /// active).
    pub fn select(&mut self, rng: &mut Rand, other: &Team, bodies: &[Body], want: u32) {
        let changed = self.requested != self.current;
        self.current = self.requested;
        let state = self.current;
        let Some((mut a, mut b)) = scales(state) else { return };
        // the passing challenge spreads every state's formotions wide (`0x42d5fd`: mode 1)
        if self.mode == 1 {
            (a, b) = ([25.0, 10.0, 25.0], [1.0; 3]);
        }
        // and it and the combo challenge pick from the challenge's database alone (`0x42cefa`: modes 1 and 6)
        let team_db = if matches!(self.mode, 1 | 6) { None } else { self.team_db.clone() };
        let mut lpair = -1;
        for k in 0..3 {
            if !self.slots[k].active {
                continue;
            }
            self.slots[k].a = a;
            self.slots[k].b = b;
            let none = self.slots[k].formotion.is_none();
            if !none && !self.slots[k].done && !changed {
                continue;
            }
            let side = self.slots[k].side;
            let (reference, cap, picked) =
                if matches!(state, ATTACK_TO_DEFEND | DEFEND_TO_DEFEND | DEFEND_PASS | KEEPER_DEFEND) {
                    // marking: follow the opponent on the same side (or, for KeeperDefend, its leader); the
                    // partner's `ppartner` would name the formotion (always NULL in retail)
                    let partner = other.partner(side);
                    let reference = if state == KEEPER_DEFEND { other.leader } else { partner.map(|p| p.player) };
                    let f = pick(
                        rng,
                        team_db.as_deref(),
                        &self.global,
                        state,
                        Filter { need: side, avoid: AVOID, want: 0, lpair: -1 },
                    );
                    (reference, FOLLOW_MARK, f)
                } else {
                    let cap = if state == FREE { FOLLOW_FREE } else { FOLLOW };
                    let f = pick(
                        rng,
                        team_db.as_deref(),
                        &self.global,
                        state,
                        Filter { need: side, avoid: AVOID, want, lpair },
                    );
                    if let Some(f) = &f {
                        lpair = f.lpair;
                    }
                    (self.leader, cap, f)
                };
            let Some(reference) = reference else { continue };
            // (the leader states also put the frame at the cruising height; the floor is flat here)
            let reinit = if next_state(state) != NONE {
                true
            } else {
                changed && matches!(state, FREE | KEEPER_ATTACK | KEEPER_DEFEND)
            };
            // inferred: a slot whose frame was never set up gets one, where the original would read stale memory
            if (reinit && (changed || none)) || self.slots[k].frame.is_none() {
                let r = bodies[reference];
                self.slots[k].frame = Some(Frame::new(r.pos, r.vel, cap));
                self.slots[k].trail = r.pos;
            }
            self.slots[k].reference = reference;
            if let Some(f) = picked {
                self.start(k, f, bodies);
            }
        }
        let next = next_state(state);
        if next != NONE {
            self.current = next;
            self.requested = next;
        }
    }

    /// `0x42df00`: start formotion `f` in slot `k` (time multiplier 1).
    fn start(&mut self, k: usize, f: Rc<Formotion>, bodies: &[Body]) {
        let body = bodies[self.slots[k].player];
        start_slot(&mut self.slots[k], self.turn, f, body, 1.0);
    }

    /// `0x42db10`: drag each active slot's trail point to within 4 units of its reference player and run the slot
    /// (`0x42ed00`). Returns the one-tick commands for players whose mover is idle.
    pub fn run(&mut self, bodies: &[Body]) -> Vec<Step> {
        let mut out = Vec::new();
        for k in 0..3 {
            if !self.slots[k].active || self.slots[k].formotion.is_none() || self.slots[k].frame.is_none() {
                continue;
            }
            let r = bodies[self.slots[k].reference].pos;
            let t = self.slots[k].trail;
            let d = sub(t, r);
            let (trail, trail_vel) = if dot(d, d) <= TRAIL * TRAIL {
                (t, [0.0; 3])
            } else {
                let n = add(r, scale(unit(d), TRAIL));
                (n, scale(sub(n, t), 24.0))
            };
            self.slots[k].trail = trail;
            if let Some(step) = self.run_slot(k, trail, trail_vel, bodies) {
                out.push(step);
            }
        }
        out
    }

    fn run_slot(&mut self, k: usize, trail: Vec3, trail_vel: Vec3, bodies: &[Body]) -> Option<Step> {
        let body = bodies[self.slots[k].player];
        let player = self.slots[k].player;
        run_slot_on(&mut self.slots[k], self.turn, Some((trail, trail_vel)), body, STEP_CAP)
            .map(|(pos, vel, heading, path)| Step { player, pos, vel, heading, path })
    }
}

/// `0x42df00` on slot `s`, time multiplier `mult`: formotion `f` from `body`'s state relative to the frame point.
fn start_slot(s: &mut Slot, turn: [f64; 2], f: Rc<Formotion>, body: Body, mult: f64) {
    let Some((fp, fv)) = s.frame.as_ref().map(Frame::point) else { return };
    let start = (rotate(turn, sub(body.pos, fp)), rotate(turn, sub(body.vel, fv)));
    s.n = trunc(f.timesteps as f64 * mult) + 1;
    s.tick = 1;
    s.seg_start = 0;
    s.waypoint = 0;
    s.done = false;
    s.start = start;
    let Some(w) = f.waypoints.first() else { return };
    s.seg_end = trunc(s.n as f64 * w.time as f64);
    s.end = map_waypoint(w.pos, w.vel, s.a, s.b);
    s.formotion = Some(f);
}

/// `0x42ed00` on slot `s`: the frame follows `input` (a point and its velocity; none leaves it), the cubic gives
/// this tick's point, and the body steps towards it by at most `cap / 24`. Returns the step (position and
/// velocity) when the body's mover is idle; past the last waypoint the slot is done.
fn run_slot_on(s: &mut Slot, turn: [f64; 2], input: Option<(Vec3, Vec3)>, body: Body, cap: f64) -> Option<Moved> {
    let orient = |v: Vec3| rotate(turn, v);
    // the waypoint aimed at gives the facing when its flag 1 is set (`0x42ed00`: `+0x35c`'s `+0x24`), its heading
    // turned into the world as its position is (the file's `z` forward)
    let heading =
        s.formotion.as_ref().and_then(|f| f.waypoints.get(s.waypoint)).filter(|w| w.flags & 1 != 0).map(|w| {
            let h = w.heading.map(f64::from);
            unit(orient([-h[2], h[1], h[0]]))
        });
    let frame = s.frame.as_mut()?;
    if let Some((p, v)) = input {
        frame.update(p, v);
    }
    let (fp, fv) = frame.point();
    // inferred guard: a segment of no ticks (never seen in retail data) is treated as one tick
    let span = (s.seg_end - s.seg_start).max(1) as f64;
    let f = (s.tick - s.seg_start) as f64 / span;
    let p0 = add(orient(s.start.0), fp);
    let p3 = add(orient(s.end.0), fp);
    let (b, bv) = bezier(p0, orient(s.start.1), p3, orient(s.end.1), span / 24.0, f);
    // the path's own velocity, and the frame point's it moves with
    let path = (b, add(bv, fv));
    let mut step = sub(b, body.pos);
    let max = cap / 24.0;
    let l = len(step);
    if l > max {
        step = scale(step, max / l);
    }
    let out = body.idle.then(|| (add(body.pos, step), scale(step, 24.0), heading, path));
    s.tick += 1;
    if s.tick > s.seg_end {
        let f = Rc::clone(s.formotion.as_ref()?);
        s.waypoint += 1;
        match f.waypoints.get(s.waypoint) {
            None => s.done = true,
            Some(w) => {
                // 0x42e380: the next segment starts where the last one aimed, unless the player is more than
                // one step from it (`0x66902c` = 1 step: the chasers' 30 units a second)
                s.seg_start = s.tick - 1;
                s.seg_end = trunc(s.n as f64 * w.time as f64);
                let local = (orient(sub(body.pos, fp)), orient(sub(body.vel, fv)));
                let off = sub(s.end.0, local.0);
                s.start = if dot(off, off) > max * max { local } else { s.end };
                s.end = map_waypoint(w.pos, w.vel, s.a, s.b);
            }
        }
    }
    out
}

/// A lone formotion block (`0x420` bytes, like a chaser slot) for one body: each Bludger's (`0x102bec0 + i *
/// 0x420`, set up by `0x430d90`) and each beater's (team `+0x1460 + i * 0x420`, `0x440620`). Its frame follows a
/// fixed point (`0x42c8f0`: block `+4`, flag `+2`) or else its reference player (`+0x98`); the step cap is `+0x9c`.
/// **[I]** Its turn is the identity: `0x4306c0` sets only the chasers' blocks. The AI defender's block (state 0x21
/// `+0x20`, [`Runner::start_turned`]) has a turn and the team's time multiplier.
#[derive(Debug, Clone, Default)]
pub struct Runner {
    pub slot: Slot,
    pub cap: f64,
    /// The block's matrix `+0x50` about `y` as `(cos, sin)`.
    turn: [f64; 2],
}

impl Runner {
    /// `0x42ca90` (unless `frame` is none: the old frame stays) and `0x42df00`: formotion `f`, its waypoints
    /// scaled by `scales`, from `body`'s state; the body's step is capped at `cap` units a second.
    pub fn start(&mut self, f: Rc<Formotion>, scales: (Vec3, Vec3), frame: Option<Frame>, body: Body, cap: f64) {
        self.start_turned(f, scales, frame, body, cap, [1.0, 0.0], 1.0);
    }

    /// [`Runner::start`] with the block turned by `turn` and the formotion's time stretched by `mult`.
    #[allow(clippy::too_many_arguments)]
    pub fn start_turned(
        &mut self,
        f: Rc<Formotion>,
        scales: (Vec3, Vec3),
        frame: Option<Frame>,
        body: Body,
        cap: f64,
        turn: [f64; 2],
        mult: f64,
    ) {
        if let Some(fr) = frame {
            self.slot.frame = Some(fr);
        }
        self.slot.a = scales.0;
        self.slot.b = scales.1;
        self.slot.active = true;
        self.cap = cap;
        self.turn = turn;
        start_slot(&mut self.slot, turn, f, body, mult);
    }

    /// `0x42ed00`: one tick, the frame following `input`; the body's one-tick step when its mover is idle.
    pub fn run(&mut self, input: Option<(Vec3, Vec3)>, body: Body) -> Option<(Vec3, Vec3)> {
        self.slot.formotion.as_ref()?;
        run_slot_on(&mut self.slot, self.turn, input, body, self.cap).map(|(p, v, _, _)| (p, v))
    }

    /// Whether a formotion was started and has passed its last waypoint, or none was ever started.
    pub fn finished(&self) -> bool {
        !self.slot.active || self.slot.formotion.is_none() || self.slot.done
    }
}

/// The formotion `min(trunc(rand() * n / 32767), n - 1)` of `found`.
fn draw(rng: &mut Rand, found: &[Rc<Formotion>]) -> Option<Rc<Formotion>> {
    let n = found.len() as i32;
    let i = trunc(rng.rand() as f64 * n as f64 / 32767.0).min(n - 1).max(0);
    found.get(i as usize).cloned()
}

/// The AI defender's pick (`0x453980`): from the [`AI_DEFEND`] list of the team's database, else the match's, the
/// formotions with `defend == kind` whose flags' bit 7 is set when the defender is `behind` the carrier (clear
/// otherwise); failing both, the match list's of that kind whatever their bit. One `rand()` draws among them.
pub fn pick_defence(rng: &mut Rand, team: Option<&Db>, global: &Db, kind: i32, behind: bool) -> Option<Rc<Formotion>> {
    let side = |f: &Rc<Formotion>| f.defend == kind && (f.flags & 0x80 != 0) == behind;
    let mut found: Vec<Rc<Formotion>> =
        team.map(|d| d.list(AI_DEFEND).iter().filter(|f| side(f)).cloned().collect()).unwrap_or_default();
    if found.is_empty() {
        found = global.list(AI_DEFEND).iter().filter(|f| side(f)).cloned().collect();
    }
    if found.is_empty() {
        found = global.list(AI_DEFEND).iter().filter(|f| f.defend == kind).cloned().collect();
    }
    if found.is_empty() {
        return None;
    }
    draw(rng, &found)
}

/// A Bludger's pick (`0x430cd0`): from the match database's list for `state`, the formotions with side bit `side`
/// (1 for Bludger 0, 2 for Bludger 1), or with none of those the whole list; one `rand()` draws.
pub fn pick_bludger(rng: &mut Rand, global: &Db, state: usize, side: u32) -> Option<Rc<Formotion>> {
    let list = global.list(state);
    let found: Vec<Rc<Formotion>> = list.iter().filter(|f| f.flags & side != 0).cloned().collect();
    if found.is_empty() {
        return if list.is_empty() { None } else { draw(rng, list) };
    }
    draw(rng, &found)
}

/// A beater's pick (`0x440620`): from the match database's list for `state`, the formotions with `defend` and
/// side bit `side` (1 for beater 0, 2 for beater 1); with none, the list's first. One `rand()` is drawn either way.
pub fn pick_beater(rng: &mut Rand, global: &Db, state: usize, side: u32, defend: i32) -> Option<Rc<Formotion>> {
    let list = global.list(state);
    let found: Vec<Rc<Formotion>> =
        list.iter().filter(|f| f.defend == defend && f.flags & side != 0).cloned().collect();
    if found.is_empty() {
        rng.rand();
        return list.first().cloned();
    }
    draw(rng, &found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qwc_formats::formotion::{TransferList, Waypoint};

    fn fm(name: &str, flags: u32, lpair: i32, wps: &[(f32, [f32; 3])]) -> Formotion {
        Formotion {
            name: name.into(),
            flags,
            timesteps: 24,
            ppartner: None,
            defend: -1,
            lpair,
            waypoints: wps
                .iter()
                .map(|&(time, pos)| Waypoint { time, pos, vel: [0.0; 3], heading: [0.0; 3], flags: 0 })
                .collect(),
        }
    }

    fn db(fs: Vec<Formotion>, state: usize) -> Db {
        let names = fs.iter().map(|f| f.name.clone()).collect();
        let d = Database {
            version: 2,
            formotions: fs,
            lists: vec![TransferList { state: STATES[state].into(), formotions: names }],
        };
        Db::new(&d)
    }

    #[test]
    fn picker_passes_and_draw() {
        let g = db(
            vec![
                fm("A_L1", 1, 1, &[(1.0, [0.0; 3])]),
                fm("A_L2", 1, 2, &[(1.0, [0.0; 3])]),
                fm("A_R1", 2, 1, &[(1.0, [0.0; 3])]),
                fm("A_C", 0, -1, &[(1.0, [0.0; 3])]),
            ],
            ATTACK_TO_ATTACK,
        );
        // pass 1 with lpair 2: only A_L2, no draw
        let mut r = Rand::new(1);
        assert_eq!(
            pick(&mut r, None, &g, 3, Filter { need: 1, avoid: AVOID, want: 0, lpair: 2 }).unwrap().name,
            "A_L2"
        );
        // left with any pair: A_L1 and A_L2, rand() = 41 picks the first
        let mut r = Rand::new(1);
        assert_eq!(
            pick(&mut r, None, &g, 3, Filter { need: 1, avoid: AVOID, want: 0, lpair: -1 }).unwrap().name,
            "A_L1"
        );
        assert_ne!(r, Rand::new(1));
        // "both" (4): nothing has flag 4, so the fallback is the list's first entry, no draw
        let mut r = Rand::new(1);
        assert_eq!(
            pick(&mut r, None, &g, 3, Filter { need: 4, avoid: AVOID, want: 0, lpair: -1 }).unwrap().name,
            "A_L1"
        );
        assert_eq!(r, Rand::new(1));
        // left with lpair 3 (none): pass 3 takes both left ones; rand() 18467 (second draw of srand(1)) picks 1
        let mut r = Rand::new(1);
        r.rand();
        assert_eq!(
            pick(&mut r, None, &g, 3, Filter { need: 1, avoid: AVOID, want: 0, lpair: 3 }).unwrap().name,
            "A_L2"
        );
        // within a pass the team list comes first; the passes come before the lists, so a global match in pass 1
        // beats a team match in pass 3
        let t = db(vec![fm("T_L", 1, -1, &[(1.0, [0.0; 3])])], ATTACK_TO_ATTACK);
        assert_eq!(
            pick(&mut Rand::new(1), Some(&t), &g, 3, Filter { need: 1, avoid: AVOID, want: 0, lpair: -1 })
                .unwrap()
                .name,
            "T_L"
        );
        assert_eq!(
            pick(&mut Rand::new(1), Some(&t), &g, 3, Filter { need: 1, avoid: AVOID, want: 0, lpair: 2 }).unwrap().name,
            "A_L2"
        );
        // flag 8 wins without a draw
        let p = db(vec![fm("P1", 1 | 8, -1, &[(1.0, [0.0; 3])]), fm("P2", 1, -1, &[(1.0, [0.0; 3])])], 3);
        let mut r = Rand::new(1);
        assert_eq!(pick(&mut r, None, &p, 3, Filter { need: 1, avoid: AVOID, want: 0, lpair: -1 }).unwrap().name, "P1");
        assert_eq!(r, Rand::new(1));
    }

    #[test]
    fn states() {
        assert_eq!(on_possession(FREE, true, false), FREE_TO_ATTACK);
        assert_eq!(on_possession(DEFEND_TO_DEFEND, true, false), DEFEND_TO_ATTACK);
        assert_eq!(on_possession(ATTACK_TO_ATTACK, false, false), ATTACK_TO_DEFEND);
        assert_eq!(on_possession(KEEPER_DEFEND, false, false), DEFEND_PASS);
        assert_eq!(on_possession(ATTACK_PASS, true, false), NONE);
        assert_eq!(on_possession(ATTACK_PASS, false, true), KEEPER_DEFEND);
        assert_eq!(next_state(ATTACK_PASS), ATTACK_TO_ATTACK);
        assert_eq!(next_state(DEFEND_PASS), DEFEND_TO_DEFEND);
        assert_eq!(next_state(FREE), NONE);
    }

    #[test]
    fn waypoint_mapping() {
        // a forward waypoint (z = 0.5) at x = 0.4 with the attack scales: -z * 6 - 0.5 * 0.5 * 18 forward
        let (p, v) = map_waypoint([0.4, 0.0, 0.5], [1.0, 0.0, 2.0], [6.0, 4.0, 6.0], [18.0, 12.0, 12.0]);
        assert!((p[0] - (-3.0 - 4.5)).abs() < 1e-6);
        assert!((p[2] - (2.4 + 0.5 * 0.4 * 12.0)).abs() < 1e-6);
        assert_eq!(v, [-12.0, 0.0, 6.0]);
        // backward waypoints get no B term
        let (p, _) = map_waypoint([0.0, 0.0, -0.5], [0.0; 3], [6.0, 4.0, 6.0], [18.0, 12.0, 12.0]);
        assert!((p[0] - 3.0).abs() < 1e-6);
    }

    #[test]
    fn frame_follows_with_a_cap() {
        let mut f = Frame::new([0.0; 3], [0.0; 3], 24.0);
        f.update([10.0, 0.0, 0.0], [5.0, 1.0, 0.0]);
        // stage 1 is still at 0 (0.9 of stage 0 = 0), so stage 0 moves 1 unit (24 / 24)
        assert_eq!(f.point(), ([1.0, 0.0, 0.0], [5.0, 0.0, 0.0]));
        f.update([10.0, 0.0, 0.0], [5.0, 0.0, 0.0]);
        // stage 1 = 0.9 * 1 + 0.1 * 0 = 0.9, then one more unit
        assert!((f.point().0[0] - 1.9).abs() < 1e-12);
    }

    #[test]
    fn slots_and_sides() {
        let g = Rc::new(Db::default());
        let mut t = Team::new(Rc::clone(&g), None, 0);
        t.assign([1, 2, 3], Some(2), false);
        assert_eq!(t.leader, Some(2));
        assert!(t.slots[0].active && !t.slots[1].active && t.slots[2].active);
        assert_eq!((t.slots[0].side, t.slots[2].side), (1, 2));
        // the keeper leads: the middle chaser joins with "both" (4)
        t.assign([1, 2, 3], Some(0), true);
        assert_eq!(t.slots[1].side, 4);
        // the first chaser leads: its slot is freed and the middle one takes the left
        t.assign([1, 2, 3], Some(1), false);
        assert!(!t.slots[0].active);
        assert_eq!(t.slots[1].side, 1);
    }

    #[test]
    fn a_formotion_runs_to_its_waypoint() {
        // one waypoint 0.5 forward after 24 ticks in the attack state, leader standing still at the origin
        let g = Rc::new(db(vec![fm("ATA_L1", 1, -1, &[(1.0, [0.0, 0.0, 0.5])])], ATTACK_TO_ATTACK));
        let mut t = Team::new(g, None, 0);
        t.requested = ATTACK_TO_ATTACK;
        let mut bodies = vec![Body { pos: [0.0; 3], vel: [0.0; 3], idle: true }; 3];
        bodies[1].pos = [0.0, 0.0, 3.0];
        t.assign([1, 2, 0], Some(0), false);
        let other = Team::new(Rc::new(Db::default()), None, 1);
        let mut rng = Rand::new(1);
        t.select(&mut rng, &other, &bodies, 0);
        assert_eq!(t.slots[0].formotion.as_ref().unwrap().name, "ATA_L1");
        // n = 24 + 1 ticks; the segment ends at tick 25, where f = 1
        for _ in 0..25 {
            for s in t.run(&bodies) {
                bodies[s.player].pos = s.pos;
                bodies[s.player].vel = s.vel;
            }
        }
        // the target: -0.5 * 6 - 0.25 * 18 = -7.5 forward (team 0 attacks -x), 0 across, relative to the frame,
        // which stays at the leader's trail point (the origin)
        assert!(len(sub(bodies[1].pos, [-7.5, 0.0, 0.0])) < 1e-6, "{:?}", bodies[1].pos);
        assert!(t.slots[0].done);
        // team 1 mirrors
        let g = Rc::clone(&t.global);
        let mut t1 = Team::new(g, None, 1);
        t1.requested = ATTACK_TO_ATTACK;
        let mut bodies = vec![Body { pos: [0.0; 3], vel: [0.0; 3], idle: true }; 3];
        t1.assign([1, 2, 0], Some(0), false);
        t1.select(&mut Rand::new(1), &other, &bodies, 0);
        for _ in 0..25 {
            for s in t1.run(&bodies) {
                bodies[s.player].pos = s.pos;
            }
        }
        assert!(len(sub(bodies[1].pos, [7.5, 0.0, 0.0])) < 1e-6, "{:?}", bodies[1].pos);
    }

    #[test]
    fn with_momentum_a_path_that_doubles_back_turns_him_round_gradually() {
        // flying +x at 15 when the path turns back to fly -x at 15 from where he is
        let (mut pos, mut vel) = ([0.0; 3], [15.0, 0.0, 0.0]);
        let mut point = [0.0; 3];
        let mut ticks = 0;
        while vel[0] > 0.0 {
            point = add(point, [-15.0 / 24.0, 0.0, 0.0]);
            let before = vel;
            (pos, vel) = glide(pos, vel, (point, [-15.0, 0.0, 0.0]), STEP_CAP, MOMENTUM);
            assert!(len(sub(vel, before)) <= MOMENTUM / 24.0 + 1e-9);
            ticks += 1;
        }
        // 15 down to 0 at 15 a second each second, give or take the pull: about a second, not a tick
        assert!((20..=30).contains(&ticks), "{ticks}");
        // and he settles back onto the path (the pull closes the gap over a few seconds)
        for _ in 0..240 {
            point = add(point, [-15.0 / 24.0, 0.0, 0.0]);
            (pos, vel) = glide(pos, vel, (point, [-15.0, 0.0, 0.0]), STEP_CAP, MOMENTUM);
        }
        assert!(len(sub(vel, [-15.0, 0.0, 0.0])) < 1.0 && len(sub(pos, point)) < 5.0, "{vel:?} {pos:?}");
    }

    #[test]
    fn players_face_along_their_step_no_steeper_than_thirty_degrees() {
        assert_eq!(face([0.0, 0.0, 5.0], [1.0, 0.0, 0.0]), [0.0, 0.0, 1.0]);
        // hardly moving: as they faced
        assert_eq!(face([0.001, 0.0, 0.0], [1.0, 0.0, 0.0]), [1.0, 0.0, 0.0]);
        // climbing steeply: held to 30 degrees, the level part kept
        let f = face([0.0, 10.0, 1.0], [1.0, 0.0, 0.0]);
        assert!((f[1] - 0.5).abs() < 1e-9 && (f[2] - 0.75f64.sqrt()).abs() < 1e-9, "{f:?}");
        // straight up: the level part from the facing
        let f = face([0.0, -10.0, 0.0], [1.0, 0.0, 0.0]);
        assert!((f[1] + 0.5).abs() < 1e-9 && (f[0] - 0.75f64.sqrt()).abs() < 1e-9, "{f:?}");
    }

    #[test]
    fn the_facing_turns_a_part_of_the_way_each_tick() {
        let a = [1.0, 0.0, 0.0];
        let b = [0.0, 0.0, 1.0];
        let t = turn_towards(a, b, TURN);
        // 0.3 of the right angle
        assert!((acos(dot(t, b)) - 0.7 * std::f64::consts::FRAC_PI_2).abs() < 1e-9);
        // turned round from straight behind, through the level
        let t = turn_towards(a, [-1.0, 0.0, 0.0], 0.5);
        assert!(t[1].abs() < 1e-9 && t[0].abs() < 1e-9, "{t:?}");
    }

    #[test]
    fn the_passing_challenge_spreads_wide_and_takes_its_own_database() {
        // the same formotion in the challenge's database and a different one in the team's
        let g = Rc::new(db(vec![fm("ATA_L1", 1, -1, &[(1.0, [0.0, 0.0, 0.5])])], ATTACK_TO_ATTACK));
        let own = Rc::new(db(vec![fm("ATA2_L1", 1, -1, &[(1.0, [0.0, 0.0, 0.1])])], ATTACK_TO_ATTACK));
        let mut t = Team::new(g, Some(own), 0);
        t.mode = 1;
        t.requested = ATTACK_TO_ATTACK;
        let mut bodies = vec![Body { pos: [0.0; 3], vel: [0.0; 3], idle: true }; 3];
        t.assign([1, 2, 0], Some(0), false);
        let other = Team::new(Rc::new(Db::default()), None, 1);
        t.select(&mut Rand::new(1), &other, &bodies, 0);
        assert_eq!(t.slots[0].formotion.as_ref().unwrap().name, "ATA_L1");
        for _ in 0..25 {
            for s in t.run(&bodies) {
                bodies[s.player].pos = s.pos;
            }
        }
        // the scale is (25, 10, 25) and (1, 1, 1) in every state: -0.5 * 25 - 0.25 forward
        assert!(len(sub(bodies[1].pos, [-12.75, 0.0, 0.0])) < 1e-6, "{:?}", bodies[1].pos);
    }
}
