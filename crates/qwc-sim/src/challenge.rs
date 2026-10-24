//! The challenge game (`0x10349b0`, constructor `0x45e650`, vtable `0x5bcbb8`): the House Challenges of the
//! Hogwarts House Cup. A challenge is a match ([`crate::Match`]) of only the players its description names
//! ([`crate::Match::use_description`]) with a clock and a goal of its own, run here on top of the match's tick.
//!
//! **[T]** (`docs/GAMEPLAY.md`, "The challenge game"):
//! - The phases: the start (`0x45ac60`, once the challenge's loading screen is left: DataStore `0x7da5b162` back to
//!   0) queues the challenge's intro cutscene; the countdown (game `+0xeb0` 1, `0x45b1c0`) runs the clock to 72
//!   ticks ("3, 2, 1"); then the clock restarts at 0 and the challenge runs (`+0xeb0` 2, `0x45b3e0`).
//! - The clock counts ticks; a challenge that is not done by its limit (`0x605f7c[kind]`: 2880 ticks for 1 to 3,
//!   3600 for 4 to 6) is over (game `+0x3ac` 4) unless `MatchChall.NeverEnd`; in its last 480 ticks a warning
//!   ticks every second. It is done (`+0x3ac` 3) when its count (`+0x3be`) reaches 10 passes (1), 3 (2, 5, 6),
//!   1 (3) or 5 (4).
//! - The end (`0x45bd50`): `Challenge_Time` is the clock in seconds (penalties were added to the clock), and DataStore
//!   `0x51e4112a` a score: the counts times 15, 10, 5, 10, 30, 0 and 15 (`0x605f1c`-`0x605f2c`, `[0xf918a8]`,
//!   `0x605f54`).
//! - Challenge 1, passing (`0x45c1a0`, `0x45bf20`): see [`Rings`] and [`Steer`].
//! - Challenges 2, 5 and 6 count goals (the score router's `0x4593e0`, either team's: in 2, 5 and 6 only team 0's
//!   hoops can be reached): in 6 a goal without a combo chain adds 10 seconds (`0x605f28`); an event of type 2 on
//!   `0x1043b34` (`0x459480`) adds 30 seconds (`0x605f2c`) and counts too. Only challenge 6's AI posts it
//!   (`0x455ff8` -> `0x456115`); **[I]** not raised yet. Each goal's handler (`0x45de10`) raises team 1's
//!   `Keeper.Speed` and hardens Tackle 'n' Shoot's defence ([`crate::Match::challenge_goal`]).
//! - Challenge 3 races from the start (`0x45e890`); **[I]** the player's catch is its count, the opponent's ends it.
//! - Challenge 4, the Beaters and Bludgers (`0x45d000`): see [`Targets`].

use crate::fmath::{cos, sin, sin_cos};
use crate::vec3::{add, dot, len, scale, sub, unit, Vec3};
use crate::Match;

/// The countdown's ticks (`0x45b1c0`: `72 - clock`).
pub const COUNTDOWN_TICKS: i32 = 72;
/// The last stretch of the clock in which a warning ticks every second (`0x45b3e0`: `limit <= clock + 480`).
pub const WARNING_TICKS: i32 = 480;
/// Seconds added to the clock by a pass that hit neither ring, the wrong ring, the green one (`0x605f20`,
/// `0x605f24`, `[0xf918a8]` which nothing writes).
pub const MISS_SECONDS: i32 = 10;
pub const WRONG_SECONDS: i32 = 5;
pub const GREEN_SECONDS: i32 = 0;
/// A ring is hit within this distance of the Quaffle (`0x45c1a0`), and passed when the Quaffle moves away from it
/// faster than [`PASSED_SPEED`].
pub const RING_RADIUS: f64 = 1.7;
pub const PASSED_SPEED: f64 = 10.0;
/// A ring is this far from the holder towards the mate it stands for, and this much higher (`0x668de4`).
pub const RING_ALONG: f64 = 0.6;
pub const RING_RISE: f64 = 1.5;
/// The green ring changes side every this many ticks of the ball held (`(count / 96) & 1`).
pub const GREEN_PERIOD: i32 = 96;

/// The clock's limit for challenge `kind` (`0x605f7c`).
pub fn time_limit(kind: u32) -> i32 {
    match kind {
        1..=3 => 2880,
        4..=6 => 3600,
        _ => i32::MAX,
    }
}

/// How many of its count challenge `kind` needs (`0x45b3e0`).
pub fn needed(kind: u32) -> u32 {
    match kind {
        1 => 10,
        2 | 5 | 6 => 3,
        3 => 1,
        4 => 5,
        _ => u32::MAX,
    }
}

/// Where a challenge is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Waiting for the loading screen's Continue (DataStore `0x7da5b162`).
    Waiting,
    /// `+0xeb0` 1.
    Countdown,
    /// `+0xeb0` 2.
    Running,
    /// `+0x3ac` 3 and 4.
    Done,
    TimedOut,
}

/// A ring's colour (`0x459050`'s units, 1 for 128): the green ring, the other one, a missed one (`0x594680`).
pub type Colour = [f64; 4];
pub const GREEN: Colour = [0.5, 1.0, 0.375, 0.5];
pub const YELLOW: Colour = [1.375, 1.0, 0.0, 0.5];
pub const RED: Colour = [1.9, 0.25, 0.225, 0.5];

/// The Beaters' challenge's five targets (`0x45a380`, copied to game `+0x4b1c` by `0x459d20`).
pub const TARGETS: [Vec3; 5] =
    [[16.0, 22.0, -8.0], [106.0, 35.0, 4.0], [40.0, 24.0, 22.0], [-47.0, 20.0, 16.0], [-98.0, 8.0, -44.0]];
/// `0x668ddc`, `0x668de0`: the targets circle together at 2 radians a second, 2 units wider per attempt.
pub const TARGET_SPIN: f64 = 2.0;
pub const TARGET_SPREAD: f64 = 2.0;
/// `0x605f50`: a target is in range while the player's distance to it, times 2 less the cosine of his facing off it,
/// is under 40.
pub const TARGET_RANGE: f64 = 40.0;
/// A flying Bludger within this of a target hits it (`0x45d000`).
pub const TARGET_RADIUS: f64 = 3.0;
/// `0x605f54`: seconds an attempt that hits nothing costs.
pub const TARGET_MISS_SECONDS: i32 = 15;

/// What a tick of the challenge did, for the game's effects, sounds and HUD.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// The countdown's number (`0x1043b28`, 3 to 1, then 0 as it ends).
    Countdown(i32),
    /// A second of the last stretch (`0x1043b34`).
    Warning,
    /// The effects of the rings (`0x1043a70`): the green one hit, a ring failed, both born again.
    RingGreen(usize),
    RingFail(usize),
    RingsBorn,
    /// A target's state changed (`0x1043a70`: `CHALLENGE_TARGET_IN_RANGE`, `OUT_OF_RANGE`, `HIT`).
    TargetHit(usize),
    /// A pass was over (`0x1043b38`): the count before it and its outcome (1 neither ring, 2 the wrong one, 3 the
    /// green one).
    Pass {
        count: u32,
        outcome: u32,
    },
    Done,
    TimedOut,
}

/// The passing challenge's two rings (`0x45c1a0`): one between the holder and each mate, the green one the target.
/// In flight: the Quaffle within [`RING_RADIUS`] of the green ring scores it; past a ring (moving away faster than
/// [`PASSED_SPEED`]) without touching it, or into the wrong ring, fails it. On the catch the pass counts and the
/// clock takes its penalty; 4 ticks on the rings are born again over the new holder.
#[derive(Debug, Clone, PartialEq)]
pub struct Rings {
    pub pos: [Vec3; 2],
    /// Which ring is green (`[0xf918b4]`).
    pub green: usize,
    /// `[0xf918b8]`: 4 green hit, 2 the wrong ring hit, 0x10 and 0x20 the rings passed; after a catch, a tick count.
    flags: u32,
    /// `[0x605f60]`: the last pass is settled (set while the ball is held).
    settled: bool,
    /// `[0xf918ac]`: ticks the ball was held, which pick the green side.
    held: f64,
    /// Game `+0x4ba0` (`param_1[0x12e8]`): bit 0 the green ring passed by, bit 1 the other.
    passed: u32,
    /// The rings' colours, eased to their targets 0.3 of the way a tick (`0x1019024`, `0x1018f68`).
    pub colour: [Colour; 2],
    target: [Colour; 2],
    /// Their sizes, moved towards their targets by at most a step a tick (`0x67d4b4`, `0x67d4bc`, `0x67d4c4`).
    pub size: [f64; 2],
    size_target: [f64; 2],
    size_step: [f64; 2],
    /// The blink in the last second before the green ring changes side (`[0xf918b0]`).
    pub blink: f64,
    pub counts: Counts,
}

/// The outcomes the score counts (`0x67d4dc` green, `0x67d4d8` the wrong ring, `0x67d4a8` neither; `0x67d4b0` a goal
/// without a combo, `0x67d4cc` a failed attempt).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counts {
    pub green: u32,
    pub wrong: u32,
    pub miss: u32,
    pub plain_goal: u32,
    pub failed: u32,
    /// `0x67d4f0`: the Beaters' challenge's attempts that hit nothing.
    pub missed_targets: u32,
}

/// Seconds a goal without a combo chain costs in the combo challenge (`0x605f28`), and a failed attempt
/// (`0x605f2c`).
pub const PLAIN_GOAL_SECONDS: i32 = 10;
pub const FAILED_SECONDS: i32 = 30;

/// `MatchChall.SizeStep*` (all 0.1 by default): the rings' size steps.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SizeSteps {
    pub red_down: f64,
    pub yellow_down: f64,
    pub green_down: f64,
    pub yellow_up: f64,
    pub green_up: f64,
}

impl Default for SizeSteps {
    fn default() -> SizeSteps {
        SizeSteps { red_down: 0.1, yellow_down: 0.1, green_down: 0.1, yellow_up: 0.1, green_up: 0.1 }
    }
}

impl Default for Rings {
    fn default() -> Rings {
        Rings {
            pos: [[0.0; 3]; 2],
            green: 0,
            flags: 0,
            settled: true,
            held: 0.0,
            passed: 0,
            colour: [GREEN, YELLOW],
            target: [GREEN, YELLOW],
            size: [1.0; 2],
            size_target: [1.0; 2],
            size_step: [0.1; 2],
            blink: 1.0,
            counts: Counts::default(),
        }
    }
}

/// The passing challenge's steer (`0x45bf20`): the team's direction `dir`, whose opposite is its Formotions'
/// forward, turns while the holder is outside the ellipse of half-axes 20 (x) and 10 (z), until the forward faces
/// the middle (within `cos 0.8`, measured in 3D from the holder), when the turn decays to 0.9 of itself a tick.
/// One way the turn grows by 0.02 rad a tick to 0.1; the other way the original snaps it to -0.1 (its clamp tests
/// `turn - 0.02 < 0.1`, kept).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Steer {
    /// Game `+0x4b04` (`param_1[0x12c1..]`): starts along `+x` (`0x45a775`: `(1, 0, 0)`), so the Formotions face
    /// `-x`, the way the camera looks.
    pub dir: Vec3,
    /// `param_1[0x12c0]`: radians a tick.
    pub turn: f64,
}

impl Default for Steer {
    fn default() -> Steer {
        Steer { dir: [1.0, 0.0, 0.0], turn: 0.0 }
    }
}

impl Steer {
    pub fn tick(&mut self, holder: Vec3) {
        let (x, z) = (holder[0], holder[2]);
        if ((x / 20.0).powi(2) + (z / 10.0).powi(2)).sqrt() <= 1.0 {
            return;
        }
        let to_middle = unit(scale(holder, -1.0));
        let d = self.dir;
        if dot(scale(d, -1.0), to_middle) >= 0.8 {
            self.turn *= 0.9;
        } else if d[2] * x - z * d[0] <= 0.0 {
            self.turn = (self.turn + 0.02).min(0.1);
        } else {
            self.turn -= 0.02;
            if self.turn < 0.1 {
                self.turn = -0.1;
            }
        }
        // the direction turned by -turn about y (`0x571d01`, `0x570cb9`)
        let (s, c) = sin_cos(-self.turn);
        self.dir = [c * d[0] + s * d[2], d[1], -s * d[0] + c * d[2]];
    }
}

/// The Beaters' challenge's targets (`0x45d000`). They circle together round their places, wider with each attempt.
/// The target nearest the Bludger is its aim (`0x1018fb4`); the Bludger move is allowed (`0x1018f85`) while the
/// player's chaser has a target in range and no Bludger flies. A flying Bludger within [`TARGET_RADIUS`] of a target
/// hits it; **[I]** that ends the move (the original also sends the Bludger on again, `0x43c190`). Each attempt
/// counts when it ends; **[I]** one that hit nothing costs [`TARGET_MISS_SECONDS`] (the original's penalty hangs on
/// a flag of the hit test that this challenge never sets).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Targets {
    pub hit: [bool; 5],
    /// The circling offset (`0x106d63c`), eased 0.1 of the way a tick.
    pub offset: Vec3,
    /// The target in range with the best (lowest) measure (game `+0x4b80`), and the one nearest the Bludger.
    pub in_range: Option<usize>,
    pub nearest: Option<usize>,
    /// `[0xf918bd]`: a Bludger was flying; and whether this attempt hit a target.
    flying: bool,
    scored: bool,
}

impl Targets {
    /// Where target `i` is now.
    pub fn pos(&self, i: usize) -> Vec3 {
        add(TARGETS[i], self.offset)
    }
}

/// A challenge in progress.
#[derive(Debug, Clone, PartialEq)]
pub struct Challenge {
    /// `MatchSubType`: 1 passing, 2 tackle and shoot, 3 Seeker, 4 Beaters and Bludgers, 5 dodging and the special
    /// move tackle, 6 combos.
    pub kind: u32,
    pub phase: Phase,
    /// `[0x1018f9c]`: ticks.
    pub clock: i32,
    /// Game `+0xef8` (`param_1[0x3be]`): the count towards [`needed`].
    pub count: u32,
    /// `MatchChall.NeverEnd`.
    pub never_end: bool,
    pub rings: Rings,
    pub sizes: SizeSteps,
    pub steer: Steer,
    pub targets: Targets,
    /// The match's events already read.
    seen: usize,
}

impl Challenge {
    pub fn new(kind: u32) -> Challenge {
        Challenge {
            kind,
            phase: Phase::Waiting,
            clock: 0,
            count: 0,
            never_end: false,
            rings: Rings::default(),
            sizes: SizeSteps::default(),
            steer: Steer::default(),
            targets: Targets::default(),
            seen: 0,
        }
    }

    /// The loading screen was left (`0x45ac60`): the countdown begins (the Seeker challenge's race with it).
    pub fn start(&mut self, m: &mut Match) {
        if self.phase == Phase::Waiting {
            self.phase = Phase::Countdown;
            self.clock = 0;
            self.seen = m.events.len();
            if self.kind == 3 {
                m.start_race();
            }
        }
    }

    pub fn over(&self) -> bool {
        matches!(self.phase, Phase::Done | Phase::TimedOut)
    }

    /// `Challenge_Time` (`0x45bd50`): the clock in seconds.
    pub fn time(&self) -> f32 {
        self.clock.max(0) as f32 * 41.666668 * 0.001
    }

    /// DataStore `0x51e4112a` at the end (`0x45bd50`): the counts weighted.
    pub fn score(&self) -> f32 {
        let c = &self.rings.counts;
        let w = |s: i32, n: u32| s as u32 * n;
        (w(GREEN_SECONDS, c.green)
            + w(WRONG_SECONDS, c.wrong)
            + w(MISS_SECONDS, c.miss)
            + w(PLAIN_GOAL_SECONDS, c.plain_goal)
            + w(FAILED_SECONDS, c.failed)
            + w(TARGET_MISS_SECONDS, c.missed_targets)) as f32
    }

    /// Whether the match should step this tick: not before the countdown, nor once the challenge is over.
    pub fn plays(&self) -> bool {
        matches!(self.phase, Phase::Countdown | Phase::Running)
    }

    /// One tick, after the match's: the clock, the challenge's own rules, the end.
    pub fn tick(&mut self, m: &mut Match) -> Vec<Event> {
        let mut out = Vec::new();
        match self.phase {
            Phase::Countdown => {
                self.clock += 1;
                let left = COUNTDOWN_TICKS - self.clock;
                out.push(Event::Countdown(left / 24 + 1));
                if self.kind == 1 {
                    self.passing(m, &mut out);
                }
                if self.kind == 4 {
                    self.beaters(m, &mut out);
                }
                if left < 1 {
                    self.phase = Phase::Running;
                    self.clock = 0;
                    out.push(Event::Countdown(0));
                }
            }
            Phase::Running => {
                self.clock += 1;
                let limit = time_limit(self.kind);
                let c = self.clock.max(0);
                if limit <= c + WARNING_TICKS && c % 24 == 0 {
                    out.push(Event::Warning);
                }
                if !self.never_end && limit <= c {
                    self.phase = Phase::TimedOut;
                    self.clock += 1;
                    out.push(Event::TimedOut);
                    return out;
                }
                if self.kind == 1 {
                    self.passing(m, &mut out);
                }
                if self.kind == 4 {
                    self.beaters(m, &mut out);
                }
                // once a second team 1's attack direction is picked again (`0x45b489`)
                if c % 24 == 0 {
                    m.pick_attack();
                }
                self.match_events(m, &mut out);
                if self.phase != Phase::Running {
                    return out;
                }
                if self.count >= needed(self.kind) {
                    self.phase = Phase::Done;
                    out.push(Event::Done);
                }
            }
            _ => {}
        }
        out
    }

    /// The match's events for challenges 2, 3, 5 and 6: goals, failed attempts and catches.
    fn match_events(&mut self, m: &mut Match, out: &mut Vec<Event>) {
        use crate::race::RaceEvent;
        use crate::world::Event as M;
        let start = self.seen.min(m.events.len());
        self.seen = m.events.len();
        let mut level = None;
        for e in &m.events[start..] {
            let outcome = match *e {
                M::Goal { team: 0, combo, .. } if matches!(self.kind, 2 | 5 | 6) => {
                    if self.kind == 6 && !combo {
                        self.clock += PLAIN_GOAL_SECONDS * 24;
                        self.rings.counts.plain_goal += 1;
                        2
                    } else {
                        3
                    }
                }
                M::Race { event: RaceEvent::Caught { team: 0 }, .. } if self.kind == 3 => 3,
                M::Race { event: RaceEvent::Caught { .. }, .. } if self.kind == 3 => {
                    self.phase = Phase::TimedOut;
                    out.push(Event::TimedOut);
                    return;
                }
                _ => continue,
            };
            out.push(Event::Pass { count: self.count, outcome });
            if !self.never_end {
                self.count += 1;
            }
            // a goal's handler (`0x45de10`): the keeper's speed, Tackle 'n' Shoot's defence
            if outcome == 3 && self.kind != 3 {
                level = Some(self.count);
            }
        }
        if let Some(goals) = level {
            m.challenge_goal(goals);
        }
    }

    /// `0x45d000`: the targets, the Bludger's aim and the move's option, and the attempts.
    fn beaters(&mut self, m: &mut Match, out: &mut Vec<Event>) {
        let t = &mut self.targets;
        let angle = self.clock as f64 * TARGET_SPIN / 24.0;
        let r = self.count as f64 * TARGET_SPREAD;
        let want = [cos(angle) * r, 0.0, sin(angle) * r];
        t.offset = add(t.offset, scale(sub(want, t.offset), 0.1));
        let flying = m.teams[0].bludger.is_some_and(|mv| mv.flying);
        let b = m.bludgers[0].pos;
        let player = m.teams[0].active.or_else(|| m.teams[0].chasers.iter().copied().find(|&c| m.players[c].present));
        let (mut nearest, mut best) = (None, (f64::MAX, None));
        for i in 0..5 {
            if t.hit[i] {
                continue;
            }
            let at = t.pos(i);
            let d = len(sub(b, at));
            if flying && d < TARGET_RADIUS {
                t.hit[i] = true;
                t.scored = true;
                out.push(Event::TargetHit(i));
                m.end_bludger_move(0);
                continue;
            }
            if nearest.is_none_or(|(n, _)| d < n) {
                nearest = Some((d, i));
            }
            if let Some(p) = player {
                let (pp, fwd) = (m.players[p].pos, m.players[p].fwd);
                // **[I]** across the ground: the port's chaser cruises at the challenge's goal height (40), above
                // the lower targets, where the original's 3D measure would never let them in range
                let off = [pp[0] - at[0], 0.0, pp[2] - at[2]];
                let measure = len(off) * (dot(fwd, unit(off)) + 2.0);
                if measure < best.0 {
                    best = (measure, Some(i));
                }
            }
        }
        t.nearest = nearest.map(|(_, i)| i);
        t.in_range = best.1.filter(|_| best.0 < TARGET_RANGE);
        m.challenge_aim = t.nearest.map(|i| t.pos(i));
        // the attempt's end: it counts, a miss costs its seconds
        let still = m.teams[0].bludger.is_some_and(|mv| mv.flying);
        if t.flying && !still {
            let outcome = if t.scored {
                3
            } else {
                self.clock += TARGET_MISS_SECONDS * 24;
                self.rings.counts.missed_targets += 1;
                1
            };
            out.push(Event::Pass { count: self.count, outcome });
            if !self.never_end {
                self.count += 1;
            }
            t.scored = false;
        }
        t.flying = still;
        m.bludgers_allowed = t.in_range.is_some() && m.teams[0].bludger.is_none() && t.hit.iter().any(|h| !h);
    }

    /// `0x45c1a0` then `0x45bf20`: the rings, then the team's direction.
    fn passing(&mut self, m: &mut Match, out: &mut Vec<Event>) {
        let r = &mut self.rings;
        // the blink in the last 24 ticks before the green side changes
        r.blink = 1.0;
        let held_ticks = r.held as i32;
        let active = m.teams[0].active;
        let holding = active.is_some() && m.holder == active;
        if holding && held_ticks % GREEN_PERIOD > 72 {
            r.blink = (held_ticks % 12) as f64 * (1.0 / 24.0) + 1.0;
        }
        // colours 0.3 of the way to their targets, sizes a step towards theirs
        for i in 0..2 {
            for k in 0..4 {
                let blink = if k < 3 { r.blink } else { 1.0 };
                let now = r.colour[i][k] * blink;
                r.colour[i][k] = (r.target[i][k] - now) * 0.3 + now;
            }
            let d = r.size_target[i] - r.size[i];
            let step = d.abs().min(r.size_step[i]);
            r.size[i] += d.signum() * step;
        }
        let g = r.green;
        if !holding {
            // the first tick after the release clears the flags, which counted the ticks since the catch
            // (`0x45ca9f`: with `0x605f60` set, `[0xf918b8]` = 0); kept, the count's bit 4 read as the green ring hit
            if r.settled {
                r.flags = 0;
            }
            r.settled = false;
            let (q, v) = (m.quaffle, m.quaffle_vel);
            let to_green = sub(q, r.pos[g]);
            if len(to_green) < RING_RADIUS && r.flags & 4 == 0 {
                out.push(Event::RingGreen(g));
                r.flags |= 4;
                r.size_step[g] = self.sizes.yellow_down;
                r.size_target[g] = 0.0;
            }
            if dot(v, to_green) > 0.0 && len(v) > PASSED_SPEED && r.flags & 0x14 == 0 {
                out.push(Event::RingFail(g));
                r.passed |= 1;
                r.size_step[g] = self.sizes.red_down;
                r.target[g] = RED;
                r.flags |= 0x10;
                r.size_target[g] = 0.0;
            }
            let o = 1 - g;
            let to_other = sub(q, r.pos[o]);
            if len(to_other) < RING_RADIUS && r.flags & 2 == 0 {
                out.push(Event::RingFail(o));
                r.target[o] = RED;
                r.size_target[o] = 0.0;
                r.size_step[o] = self.sizes.green_down;
                r.flags |= 2;
            }
            if dot(v, to_other) > 0.0 && len(v) > PASSED_SPEED && r.flags & 0x22 == 0 {
                out.push(Event::RingFail(o));
                r.passed |= 2;
                r.target[o] = RED;
                r.size_step[o] = self.sizes.red_down;
                r.flags |= 0x20;
                r.size_target[o] = 0.0;
            }
            if r.passed == 3 {
                r.passed = 0;
            }
        } else {
            if !r.settled {
                // the catch: the pass counts, the clock takes its penalty
                let (seconds, outcome) = if r.flags & 4 != 0 {
                    r.counts.green += 1;
                    m.charge(0, 0.1);
                    (GREEN_SECONDS, 3)
                } else if r.flags & 2 != 0 {
                    r.counts.wrong += 1;
                    m.charge(0, 0.05);
                    (WRONG_SECONDS, 2)
                } else {
                    r.counts.miss += 1;
                    (MISS_SECONDS, 1)
                };
                self.clock += seconds * 24;
                out.push(Event::Pass { count: self.count, outcome });
                if !self.never_end {
                    self.count += 1;
                }
                r.flags = 0;
            } else {
                if r.flags == 4 {
                    // four ticks after the catch: the rings are born again over the new holder
                    out.push(Event::RingsBorn);
                    r.size = [0.0; 2];
                    r.size_target = [1.0; 2];
                    r.size_step = if r.green == 0 {
                        [self.sizes.green_up, self.sizes.yellow_up]
                    } else {
                        [self.sizes.yellow_up, self.sizes.green_up]
                    };
                }
                r.flags = r.flags.min(4) + 1;
            }
            r.settled = true;
            r.held += 1.0;
            r.green = ((r.held as i32 / GREEN_PERIOD) & 1) as usize;
            let h = active.unwrap();
            let hp = m.players[h].pos;
            let mut k = 0;
            for &c in &m.teams[0].chasers {
                if c == h || !m.players[c].present || k > 1 {
                    continue;
                }
                let cp = m.players[c].pos;
                let mut p = add(hp, scale(sub(cp, hp), RING_ALONG));
                p[1] += RING_RISE;
                r.target[k] = if r.green == k { GREEN } else { YELLOW };
                r.pos[k] = p;
                k += 1;
            }
            r.passed = 0;
        }
        // the team's direction follows the holder (`0x45bf20`)
        if let Some(h) = active {
            self.steer.tick(m.players[h].pos);
        }
        m.set_formation_heading(0, scale(self.steer.dir, -1.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_and_counts() {
        assert_eq!((time_limit(1), time_limit(4)), (2880, 3600));
        assert_eq!((needed(1), needed(2), needed(3), needed(4)), (10, 3, 1, 5));
        let mut c = Challenge::new(1);
        c.clock = 24 * 61;
        assert!((c.time() - 61.0).abs() < 1e-3);
    }

    /// A pass that touches neither ring is a miss, however long the ball was held before it.
    #[test]
    fn a_pass_through_no_ring_is_a_miss() {
        let mut m = Match::new(&crate::console::Console::new(), crate::rng::Rand::new(3));
        let c = m.teams[0].chasers[0];
        m.holder = Some(c);
        m.teams[0].active = Some(c);
        let mut ch = Challenge::new(1);
        ch.phase = Phase::Running;
        // held long enough for the flags to count past 4
        for _ in 0..10 {
            ch.tick(&mut m);
        }
        m.holder = None;
        (m.quaffle, m.quaffle_vel) = ([500.0, 50.0, 500.0], [0.0; 3]);
        ch.tick(&mut m);
        m.holder = Some(c);
        let events = ch.tick(&mut m);
        assert!(events.contains(&Event::Pass { count: 0, outcome: 1 }), "{events:?}");
        assert_eq!((ch.rings.counts.green, ch.rings.counts.miss), (0, 1));
    }

    #[test]
    fn the_steer_turns_back_towards_the_middle() {
        let mut s = Steer::default();
        // inside the ellipse nothing changes
        s.tick([10.0, 20.0, 5.0]);
        assert_eq!(s, Steer::default());
        // far out along -z with the forward (-dir) along -z, away from the middle: it turns until the forward faces
        // back towards the middle, then the turn dies away
        for _ in 0..300 {
            s.tick([0.0, 2.0, -15.0]);
        }
        assert!(s.dir[2] < -0.5, "{:?}", s.dir);
        assert!(s.turn.abs() < 0.05, "{}", s.turn);
        assert!((len(s.dir) - 1.0).abs() < 1e-9);
    }
}
