//! The Seeker game's race (`docs/GAMEPLAY.md`, "Seeker race"): both seekers follow the Snitch's trail, moving only
//! across it; the AI picks a mode (chase, grab, attack, keep near the opponent), steers to a lateral offset and sets
//! its speed; staying central recharges the boost; a held grab next to the Snitch catches it.
//!
//! Traced from `0x44da50` (per team), `0x44c720` (the trail), `0x44c430` (steering helpers), `0x44baa0` (distances),
//! `0x449520` (the AI), `0x44b4d0` (boost and power), `0x4489f0` (the catch) and `0x449000` (collisions).
//! The human seeker ([`Race::human`]): the steering `0x44bd60`, Boost pressed for the barge (`0x44ae60`), the grab
//! on the record's `+0x11` (word 14) and the catch test when it lands (`0x4489f0`).
//! Not ported: the grab stance's animation (`0x44bb80`, within `GrabSnitchStanceDistance`), power-ups (boost impulses
//! and multipliers), the human catch-up to the Snitch's history, the block and hit animations, and the
//! opponent-being-hit speed bonus (it reads animation states).
//! Approximated (inferred): the human's grab lands [`GRAB_REACH_TICKS`] after it starts (the animation's trigger,
//! player `+0x15c` = 2, is not read).
//! Approximated (inferred): the bone-sphere collision is a broom-length capsule test, and the tackle's animation is a
//! fixed window of [`TACKLE_TICKS`].

use crate::console::Console;
use crate::fmath::{cos, sin};
use crate::passing::TICKS;
use crate::rng::{trunc, Rand};
use crate::seeker::{can_grab, GrabAttempt, SeekerRules, Snitch};
use crate::spline::Spline;
use crate::vec3::{add, cross, dot, len, scale, sub, unit, Vec3};

/// The ticks of the starting hold (`+0x1ca4`, 76), during which the seekers spread out at a fixed speed.
pub const INTRO_TICKS: i32 = 76;
/// The AI's base speed (units per second) when both seekers are AI.
pub const AI_SPEED: f64 = 25.0;
/// Points scored for the catch (`0x4489f0`: game `+0x260[t] += 0x96`).
pub const CATCH_POINTS: i32 = 150;
/// The ticks an AI holds the grab before it is resolved (`0x449520` mode 3).
pub const GRAB_TICKS: i32 = 96;
/// The parameter step forced each tick during the hold, and otherwise (`0x44da50`).
pub const INTRO_STEP: f64 = 0.02;
pub const MIN_STEP: f64 = 1e-5;
/// Boost recharge per tick at the trail centre in `Game.SeekerGameType` 0 (`0x60612c`).
pub const RECHARGE: f64 = 0.004;
/// The least level an AI needs to keep boosting (`0x5bf240`), also the bonus pool's drip.
pub const AI_BOOST_FLOOR: f64 = 0.0125;
/// How hard a bump pushes the seekers apart (`0x668ecc`), and the root distance within which the spheres are tested
/// (`0x5bf040`, squared).
pub const BUMP_PUSH: f64 = 0.155;
pub const BUMP_RANGE_SQ: f64 = 4.0;
/// The broom capsule standing in for the bone spheres (inferred from the sphere table `0x606268`: spheres of
/// 0.15-0.25 along the broom from 1.4 behind to 0.9 ahead of the root).
pub const BROOM_BACK: f64 = 1.4;
pub const BROOM_FRONT: f64 = 0.9;
pub const BROOM_RADIUS: f64 = 0.2;
/// The tackle animation's window in which a bump lands the hit (inferred; the animation lengths are not read).
pub const TACKLE_TICKS: i32 = 12;
/// The ticks from a human's grab to its catch test, and the edge of the trail past which pushing outwards is cut
/// to a fifth (`0x6065f0`, 0.9 of the half width).
pub const GRAB_REACH_TICKS: i32 = 12;
pub const PLAYER_EDGE: f64 = 0.9;

/// The AI's modes (`+0x1ca8`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Pick = 0,
    Chase = 1,
    GoForGrab = 2,
    Grab = 3,
    Recover = 4,
    Attack = 5,
    KeepNear = 6,
    Lead = 7,
    Idle = 8,
}

/// The `SeekerGame.*` values the race reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RaceRules {
    pub seeker: SeekerRules,
    pub trail_width: f64,
    pub frame_look_ahead: f64,
    pub heading_helper: f64,
    pub velocity_helper: f64,
    pub ai_frame_look_ahead: f64,
    pub ai_heading_helper: f64,
    pub ai_velocity_helper: f64,
    pub sweet_spot: f64,
    pub start_boost_cost: f64,
    pub minimum_boost_level: f64,
    pub hit_by_opponent_penalty: f64,
    pub initial_speed: f64,
    pub max_lead: f64,
    pub aggression: f64,
    pub defensiveness: f64,
    pub defense_refresh: f64,
    pub opportunistic_attack: f64,
    pub attack_distance: f64,
    pub attack_refresh: f64,
    pub chase_refresh: f64,
    pub grab_range: f64,
    pub chase_range: f64,
    pub chase_chance: f64,
    pub grab_chance: f64,
    pub grab_success_rate: f64,
    pub near_player_refresh: f64,
    pub missed_recovery: f64,
    pub missed_speed_penalty: f64,
    pub keep_near_lead: f64,
    pub keep_near_trail: f64,
    pub hit_penalty_time: f64,
    pub ignore_player_move_penalty: f64,
    pub aim_point_speed_boost: f64,
    pub aim_point_brake: f64,
    pub opponent_boost_leading_penalty: f64,
    pub opponent_boost_trailing_penalty: f64,
    pub player_overtake_speed_adjust: f64,
    pub player_drop_back_speed_adjust: f64,
    pub chase_speed_bonus: f64,
    pub lateral_movement_rate: f64,
    pub speed_change_rate: f64,
    pub hit_speed_penalty: f64,
    pub keep_behind_when_opponent_boost: bool,
    pub always_chase_after: f64,
    pub time_until_boost: f64,
    pub ai_minimum_boost: f64,
    pub leave_me_alone: bool,
    pub keep_distance: f64,
    /// The human's steering (`0x44bd60`): `MaxYaw` (degrees from the trail's direction), `YawSpeed`, `LateralVel`,
    /// `InputMod`, `VelDamping`, `PlayerTurnRate`, `PlayerHeadingBias`.
    pub max_yaw: f64,
    pub yaw_speed: f64,
    pub lateral_vel: f64,
    pub input_mod: f64,
    pub vel_damping: f64,
    pub player_turn_rate: f64,
    pub player_heading_bias: f64,
    /// `GrabSnitchStanceDistance` (`0x44bb80`): a human's seeker holds out his hand within this many trail points of
    /// the Snitch and this many units of it.
    pub stance: f64,
}

impl RaceRules {
    pub fn from_console(c: &Console) -> RaceRules {
        let f = |k: &str| c.f32(&format!("SeekerGame.{k}"), 0) as f64;
        let b = |k: &str| c.flag(&format!("SeekerGame.{k}"));
        RaceRules {
            seeker: SeekerRules::from_console(c),
            trail_width: f("TrailWidth"),
            frame_look_ahead: f("FrameLookAhead"),
            heading_helper: f("HeadingHelper"),
            velocity_helper: f("VelocityHelper"),
            ai_frame_look_ahead: f("AI.FrameLookAhead"),
            ai_heading_helper: f("AIHeadingHelper"),
            ai_velocity_helper: f("AIVelocityHelper"),
            sweet_spot: f("BoostRechargeSweetSpotSize"),
            stance: f("GrabSnitchStanceDistance"),
            start_boost_cost: f("StartBoostCost"),
            minimum_boost_level: f("MinimumBoostLevel"),
            hit_by_opponent_penalty: f("HitByOpponentSpeedPenlty"),
            initial_speed: f("AI.InitialSpeed"),
            max_lead: f("AI.MaxLead"),
            aggression: f("AI.Aggression"),
            defensiveness: f("AI.Defensiveness"),
            defense_refresh: f("AI.DefenseRefresh"),
            opportunistic_attack: f("AI.OpportunisticAttack"),
            attack_distance: f("AI.AttackDistance"),
            attack_refresh: f("AI.AttackRefresh"),
            chase_refresh: f("AI.ChaseSnitchRefresh"),
            grab_range: f("AI.GrabSnitchRange"),
            chase_range: f("AI.ChaseSnitchRange"),
            chase_chance: f("AI.ChaseSnitchChance"),
            grab_chance: f("AI.GrabSnitchChance"),
            grab_success_rate: f("AI.GrabSnitchSuccessRate"),
            near_player_refresh: f("AI.NearPlayerRefresh"),
            missed_recovery: f("AI.MissedSnitchRecovery"),
            missed_speed_penalty: f("AI.MissedSnitchSpeedPenalty"),
            keep_near_lead: f("AI.KeepNearLead"),
            keep_near_trail: f("AI.KeepNearTrail"),
            hit_penalty_time: f("AI.HitPenaltyTime"),
            ignore_player_move_penalty: f("AI.IgnorePlayerMovePenalty"),
            aim_point_speed_boost: f("AI.AimPointSpeedBoost"),
            aim_point_brake: f("AI.AimPointBrake"),
            opponent_boost_leading_penalty: f("AI.OpponentBoostLeadingPenalty"),
            opponent_boost_trailing_penalty: f("AI.OpponentBoostTrailingPenalty"),
            player_overtake_speed_adjust: f("AI.PlayerOvertakeSpeedAdjust"),
            player_drop_back_speed_adjust: f("AI.PlayerDropBackSpeedAdjust"),
            chase_speed_bonus: f("AI.ChaseSnitchSpeedBonus"),
            lateral_movement_rate: f("AI.LateralMovementRate"),
            speed_change_rate: f("AI.SpeedChangeRate"),
            hit_speed_penalty: f("AI.HitSpeedPenalty"),
            keep_behind_when_opponent_boost: b("AI.KeepBehindWhenOpponentBoost"),
            always_chase_after: f("AI.AlwaysChaseSnitchAfterTime"),
            time_until_boost: f("AI.TimeUntilBoost"),
            ai_minimum_boost: f("AI.MinimumBoost"),
            leave_me_alone: b("AI.LeaveMeAlone"),
            keep_distance: f("AI.KeepDistance"),
            max_yaw: f("MaxYaw"),
            yaw_speed: f("YawSpeed"),
            lateral_vel: f("LateralVel"),
            input_mod: f("InputMod"),
            vel_damping: f("VelDamping"),
            player_turn_rate: f("PlayerTurnRate"),
            player_heading_bias: f("PlayerHeadingBias"),
        }
    }

    /// `0x448170(x, t)`: `x` times half the trail's width at parameter `t`, which narrows near the Snitch: with
    /// `d = clamp(snitch - t, 1.5, 3)`, `(0.4 x + 0.2 d x) * TrailWidth`.
    pub fn half_width(&self, x: f64, snitch: f64, t: f64) -> f64 {
        let d = (snitch - t).clamp(1.5, 3.0);
        (x * 0.4 + d * x * 0.2) * self.trail_width
    }
}

/// `0x44baa0`'s distances, in spline points.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Distances {
    /// `+0x1d28`: from the seeker (clamped to the Snitch) to the Snitch.
    pub snitch: f64,
    /// `+0x1d2c`: the opponent's.
    pub opponent: f64,
    /// `+0x1d30`: the gap between the two.
    pub lead: f64,
    /// `+0x1d3c`: the gap's change since the last tick.
    pub lead_change: f64,
    /// `+0x1d34`, `+0x1d38`: both parameters clamped to the Snitch's.
    pub own: f64,
    pub opp: f64,
}

/// One seeker in the race (fields of its team object).
#[derive(Debug, Clone, PartialEq)]
pub struct Seeker {
    pub ai: bool,
    /// `+0x12b4`, the parameter on the trail.
    pub param: f64,
    /// `+0x1cb8`, `+0x1cc8`, `+0x1cd8`: position, velocity (units per second) and heading.
    pub pos: Vec3,
    pub vel: Vec3,
    pub heading: Vec3,
    /// The player's `+0x1a0`: across the trail, -1 to 1 inside it (positive along the trail's side axis).
    pub offset: f64,
    /// The follower's up axis as last evaluated (`+0x12dc`; it is evaluated at the race's first tick only).
    pub up: Vec3,
    /// `+0x1ca4`: ticks of the starting hold left.
    pub intro: i32,
    pub mode: Mode,
    pub dist: Distances,
    /// `+0x1d24`: the opponent is nearer the Snitch (set by the distances, cleared every tick).
    pub opp_nearer: bool,
    /// The boost level (`+0x1308[t]`, the power bar's copy) and its smoothed copy `+0x1d10`.
    pub level: f64,
    pub shown: f64,
    /// `+0x1d14` boosting this tick; `+0x1d18` the AI wants to.
    pub boosting: bool,
    pub wants_boost: bool,
    /// `+0x1d64`: it had more boost than the opponent when the race began.
    pub more_boost: bool,
    /// `+0x1d54` the smoothed speed multiplier and `+0x1d58` the one aimed for this tick.
    pub speed: f64,
    pub target: f64,
    /// `+0x1d5d`, `+0x1d60`: waiting to answer the opponent's boost.
    pub boost_wait: bool,
    pub boost_wait_t: f64,
    /// `+0x1d5c`: the grab will be allowed (`GrabSnitchSuccessRate` roll).
    pub grab_allowed: bool,
    /// `+0x1d44`: a bump last tick; `+0x1d45`: just hit by a tackle; `+0x1d48` the hit's countdown (seconds).
    pub bumped: bool,
    pub hit: bool,
    pub hit_t: f64,
    /// `+0x1d4c` the block roll's countdown; `+0x1d50` the player-move countdown (seconds).
    pub defense_t: f64,
    pub ignore_t: f64,
    /// The stick (`+0x1d1c` forward, `+0x1d20` across); zero for an AI.
    pub stick: [f64; 2],
    /// The tackle in progress (ticks left) and whether it already landed (`+0x1ca0`).
    pub tackle: i32,
    pub tackle_landed: bool,
    /// A human's controls this tick ([`SeekerPad`]), its turn (`0xf91920`) and its grab in progress (ticks left).
    pub pad: SeekerPad,
    pub turn: f64,
    pub grab: i32,
    /// A human's seeker is in the reaching stance.
    pub stance: bool,
}

/// What a human seeker's pad record holds for the race: the move stick (`+0x00` vertical, up negative; `+0x04`
/// horizontal, right positive), Boost held and pressed (`+0x12`, `+0x1a`) and the grab held (`+0x11`, word 14).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SeekerPad {
    pub stick: [f64; 2],
    pub boost_held: bool,
    pub boost_pressed: bool,
    pub grab_held: bool,
}

impl SeekerPad {
    /// From the pad record ([`crate::human::Pad`]).
    pub fn from_pad(pad: &crate::human::Pad) -> SeekerPad {
        use crate::human::action;
        SeekerPad {
            stick: [pad.axes[0], pad.axes[1]],
            boost_held: pad.held(action::BOOST),
            boost_pressed: pad.pressed(action::BOOST),
            // **[I]** Action too, which the race leaves unused (the original's grab is word 14, Dodge)
            grab_held: pad.held(action::DODGE) || pad.held(action::ACTION),
        }
    }
}

impl Seeker {
    fn new(spline: &Spline, param: f64, ai: bool) -> Seeker {
        let f = spline.frame(param);
        Seeker {
            ai,
            param,
            pos: spline.position(param),
            vel: [0.0; 3],
            heading: f.fwd,
            offset: 0.0,
            up: f.up,
            intro: INTRO_TICKS,
            mode: Mode::Pick,
            dist: Distances::default(),
            opp_nearer: false,
            level: 0.0,
            shown: 0.0,
            boosting: false,
            wants_boost: false,
            more_boost: false,
            speed: 1.0,
            target: 1.0,
            boost_wait: false,
            boost_wait_t: 0.0,
            grab_allowed: false,
            bumped: false,
            hit: false,
            hit_t: 0.0,
            defense_t: 0.0,
            ignore_t: 0.0,
            stick: [0.0; 2],
            tackle: 0,
            tackle_landed: false,
            pad: SeekerPad::default(),
            turn: 0.0,
            grab: 0,
            stance: false,
        }
    }
}

/// What a tick of the race produced.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RaceEvent {
    /// The AI started a grab (`0x44a7a0`); `allowed` is its success roll.
    Grab {
        team: usize,
        allowed: bool,
    },
    /// A grab was resolved (`0x4489f0`).
    Missed {
        team: usize,
    },
    Caught {
        team: usize,
    },
    /// The AI attacked (`0x44ae60`), and a tackle landed (`0x448d10`).
    Attack {
        team: usize,
    },
    Hit {
        team: usize,
    },
    Bump,
    /// A human's seeker came within reach of the Snitch (`0x44bb80`: the reaching stance, clip `0x39`) or left it.
    Stance {
        team: usize,
        on: bool,
    },
}

/// The Seeker AI's globals (`0xf91928`-`0xf9193c`, `0x10439f0`, `0x10439f4`). The original keeps one copy for the
/// whole game: in play one seeker is human, so only one AI uses it. With two AI seekers the shared copy starves one of
/// its decisions (the countdown runs out on the other's turn) and doubles the always-chase clock, so the port keeps
/// one per seeker (a deliberate departure).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Brain {
    /// `0xf91930`: the decision countdown (ticks).
    pub timer: i32,
    /// `0xf91928` (`AI.KeepDistance`) and `0xf91934` (the opponent boosted: keep behind).
    pub keep_distance: f64,
    pub keep_flag: bool,
    /// `0xf9193c`: seconds since `AlwaysChaseSnitchAfterTime`.
    pub always: f64,
    /// `0xf9192c`, `0x10439f0`, `0x10439f4`.
    pub last_stick: f64,
    pub stick_hold: i32,
    pub grab_ticks: i32,
}

/// The race.
#[derive(Debug, Clone)]
pub struct Race {
    pub spline: Spline,
    pub snitch: Snitch,
    pub seekers: [Seeker; 2],
    /// Ticks since the race began (the Snitch's pace clock, `0x668ec4`).
    pub ticks: i32,
    /// `0xf91938`: the AI's tick count (once a tick).
    pub ai_ticks: f64,
    /// The AI's globals, one copy per seeker (see [`Brain`]).
    pub brain: [Brain; 2],
    pub caught: Option<usize>,
    pub events: Vec<RaceEvent>,
}

impl Race {
    /// The set-up `0x44d360`: the Snitch at its start, the team 0 seeker 10 points behind and team 1's 8, both on the
    /// trail line with no speed and an empty boost.
    pub fn new(spline: Spline, r: &RaceRules, ai: [bool; 2]) -> Race {
        let snitch = Snitch::start(&spline, &r.seeker);
        let starts = snitch.seeker_starts();
        let seekers = [Seeker::new(&spline, starts[0], ai[0]), Seeker::new(&spline, starts[1], ai[1])];
        Race {
            spline,
            snitch,
            seekers,
            ticks: 0,
            ai_ticks: 0.0,
            brain: [Brain { keep_distance: r.keep_distance, ..Brain::default() }; 2],
            caught: None,
            events: Vec::new(),
        }
    }

    /// The Snitch's part of the tick (`0x44f990`, before the teams).
    pub fn snitch_tick(&mut self, r: &RaceRules) {
        if self.caught.is_some() {
            return;
        }
        self.snitch.pace([self.seekers[0].pos, self.seekers[1].pos], self.ticks);
        self.snitch.update(&self.spline, &r.seeker);
        self.ticks += 1;
    }

    /// One team's part (`0x44da50` with race type 3).
    pub fn seeker_tick(&mut self, t: usize, rng: &mut Rand, r: &RaceRules) {
        if self.caught.is_some() {
            return;
        }
        self.seekers[t].opp_nearer = false;
        let s = &mut self.seekers[t];
        if s.tackle > 0 {
            s.tackle -= 1;
            // the hit lands when the attacker bumped the opponent during the tackle (`0x44da50`, `0x448d10`)
            if s.bumped && !s.tackle_landed {
                s.tackle_landed = true;
                let o = &mut self.seekers[1 - t];
                if o.ai {
                    o.hit = true;
                } else {
                    o.vel = scale(o.vel, r.hit_by_opponent_penalty);
                }
                let _hit_animation = rng.rand() % 3;
                self.events.push(RaceEvent::Hit { team: 1 - t });
            }
        }
        let s = &mut self.seekers[t];
        if s.intro > 0 {
            s.intro -= 1;
            self.trail(t, INTRO_STEP, false, r);
        }
        let s = &self.seekers[t];
        if s.intro == 0 {
            // a bump or an action animation (the tackle) lets the seeker fall back
            if s.bumped || s.tackle > 0 {
                self.trail(t, 0.0, true, r);
            } else {
                self.trail(t, MIN_STEP, false, r);
            }
        }
        self.distances(t);
        // a human seeker is the AI's during the starting hold (`0x44ded4` -> `0x44ae40`)
        if self.seekers[t].ai || self.seekers[t].intro > 0 {
            self.ai(t, rng, r);
            if self.seekers[t].intro == 0 {
                self.boost_power(t, r);
            }
        } else {
            self.human(t, r);
            self.boost_power(t, r);
        }
        // 0x44ea40: never past the Snitch
        let s = &mut self.seekers[t];
        s.param = s.param.min(self.snitch.param);
    }

    /// The bump test (`0x449000`, after both teams), approximated by broom capsules: on contact both seekers skip
    /// the forced step next tick and are pushed apart across the trail.
    pub fn collide(&mut self) {
        let [a, b] = &self.seekers;
        let d = sub(b.pos, a.pos);
        let near = dot(d, d) <= BUMP_RANGE_SQ && capsules_touch(a, b);
        self.seekers[0].bumped = near;
        self.seekers[1].bumped = near;
        if near && self.caught.is_none() {
            let side = self.spline.frame(self.seekers[0].param + 1.0).side;
            let push = scale(side, BUMP_PUSH * if dot(side, d) >= 0.0 { 1.0 } else { -1.0 });
            self.seekers[0].pos = sub(self.seekers[0].pos, push);
            self.seekers[1].pos = add(self.seekers[1].pos, push);
            self.events.push(RaceEvent::Bump);
        }
    }

    /// `0x44c720`: move, stay in the trail's surface, measure the progress along the trail (by at least `min_step`
    /// unless `free`, dragging the seeker along), keep within the trail's width, and steer along it.
    fn trail(&mut self, t: usize, min_step: f64, free: bool, r: &RaceRules) {
        let sp = &self.spline;
        let snitch = self.snitch.param;
        let s = &mut self.seekers[t];
        let old = s.param;
        // the segment either side of the parameter (widened past duplicate points) and the frame at its start
        let mut h = 0.5;
        let (mut a, mut b) = (sp.position(old - h), sp.position(old + h));
        while sub(b, a) == [0.0; 3] && h < sp.len() as f64 {
            h += 0.1;
            a = sp.position(old - h);
            b = sp.position(old + h);
        }
        let frame = sp.frame(old - 0.5);
        let up = frame.up;
        s.pos = add(s.pos, scale(s.vel, 1.0 / TICKS));
        s.pos = sub(s.pos, scale(up, dot(up, sub(s.pos, a))));
        let seg = sub(b, a);
        let tu = unit(seg);
        let d = sub(s.pos, a);
        let bn = unit(cross(unit(cross(d, tu)), tu));
        let lat = dot(bn, d);
        let along = sub(d, scale(bn, lat));
        let mut param = along_fraction(along, seg) - 0.5 + old;
        if !free && param - old < min_step {
            param = old + min_step;
            s.pos = add(s.pos, sub(sp.position(param), sp.position(old)));
        }
        s.param = param;
        if param > snitch {
            s.pos = sub(s.pos, sub(sp.position(param), sp.position(snitch)));
        }
        // keep within the trail: pushed back to its edge, steered harder unless the stick pushes outwards
        let width = 2.0 * r.half_width(1.0, snitch, param);
        let excess = lat.abs() - width.abs();
        let (look, hh, vh) = if s.ai {
            (r.ai_frame_look_ahead, r.ai_heading_helper, r.ai_velocity_helper)
        } else {
            (r.frame_look_ahead, r.heading_helper, r.velocity_helper)
        };
        if excess > 0.0 {
            s.pos = if lat <= 0.0 { add(s.pos, scale(bn, excess)) } else { sub(s.pos, scale(bn, excess)) };
            let out = s.stick[1];
            if (s.offset >= 0.0 || out >= 0.0) && (s.offset <= 0.0 || out <= 0.0) {
                steer(sp, s, true, look, 2.0 * r.heading_helper, 2.0 * r.velocity_helper, up);
            }
            s.vel = scale(s.vel, 0.95);
        }
        steer(sp, s, false, look, hh, vh, up);
        // the lateral offset: the distance from the line over the width, positive along the side axis
        let d = sub(s.pos, a);
        let bn = unit(cross(cross(d, tu), tu));
        let off = dot(d, bn).abs() / width.abs();
        s.offset = if dot(frame.side, bn) > 0.0 { -off } else { off };
    }

    /// `0x44baa0`.
    fn distances(&mut self, t: usize) {
        let sn = self.snitch.param;
        let own = self.seekers[t].param.min(sn);
        let opp = self.seekers[1 - t].param.min(sn);
        let s = &mut self.seekers[t];
        let (snitch, opponent) = (sn - own, sn - opp);
        let lead = (opponent - snitch).abs();
        s.dist = Distances { snitch, opponent, lead, lead_change: lead - s.dist.lead, own, opp };
        if opponent < snitch {
            s.opp_nearer = true;
        }
    }

    /// The Seeker AI `0x449520`: the mode machine, the speed multiplier and the velocity.
    fn ai(&mut self, t: usize, rng: &mut Rand, r: &RaceRules) {
        let o = 1 - t;
        let (opp_off, opp_ai, opp_speed, opp_boosting, opp_stick) = {
            let p = &self.seekers[o];
            (p.offset, p.ai, len(p.vel), p.boosting, p.stick[0])
        };
        let leave = r.leave_me_alone;
        let mut aim: Option<f64> = None;
        let mut missed = false;
        let mut lat_target = 0.0;
        self.seekers[t].target = 1.0;
        let mut base = if !self.seekers[t].ai || !opp_ai { opp_speed } else { AI_SPEED };
        if self.seekers[t].intro == 0 {
            if r.always_chase_after >= 0.0 && r.always_chase_after <= self.ai_ticks {
                self.brain[t].always += 1.0 / TICKS;
            }
            if !self.seekers[t].ai || !opp_ai || t == 0 {
                self.ai_ticks += 1.0;
            }
            let free_choice = r.always_chase_after < 0.0 || self.ai_ticks < r.always_chase_after;
            let s = &self.seekers[t];
            let (dist, more_boost, opp_nearer) = (s.dist, s.more_boost, s.opp_nearer);
            let mut mode = s.mode;
            match s.mode {
                Mode::Pick => {
                    mode = if more_boost { Mode::Lead } else { Mode::KeepNear };
                    self.brain[t].timer = 0;
                }
                Mode::Chase if leave => {
                    mode = Mode::KeepNear;
                    self.brain[t].timer = 0;
                }
                Mode::Chase => {
                    aim = Some(dist.own + dist.snitch);
                    self.brain[t].timer -= 1;
                    if self.brain[t].timer < 1 {
                        let grab = |rng: &mut Rand| dist.snitch <= r.grab_range && rng.unit() <= r.grab_chance;
                        if !free_choice {
                            if grab(rng) {
                                mode = Mode::GoForGrab;
                                self.brain[t].timer = 0;
                            } else {
                                self.brain[t].timer = trunc(r.chase_refresh * TICKS);
                            }
                        } else if dist.lead <= r.max_lead || more_boost {
                            if grab(rng) {
                                mode = Mode::GoForGrab;
                                self.brain[t].timer = 0;
                            } else if dist.lead <= r.attack_distance
                                && rng.unit() <= r.opportunistic_attack
                                && !more_boost
                            {
                                mode = Mode::Attack;
                                self.brain[t].timer = 0;
                            } else {
                                self.brain[t].timer = trunc(r.chase_refresh * TICKS);
                            }
                        } else {
                            mode = Mode::KeepNear;
                            self.brain[t].timer = 0;
                        }
                    }
                }
                Mode::GoForGrab => {
                    aim = Some(dist.own + dist.snitch);
                    if leave {
                        mode = Mode::KeepNear;
                    } else {
                        let allowed = rng.unit() <= r.grab_success_rate;
                        self.seekers[t].grab_allowed = allowed;
                        self.events.push(RaceEvent::Grab { team: t, allowed });
                        mode = Mode::Grab;
                        self.brain[t].grab_ticks = 0;
                    }
                    self.brain[t].timer = 0;
                }
                Mode::Grab => {
                    let back = if self.seekers[t].grab_allowed { 0.0 } else { 2.2 };
                    aim = Some(dist.own + dist.snitch - back);
                    self.brain[t].grab_ticks += 1;
                    if self.brain[t].grab_ticks > GRAB_TICKS && !self.resolve_grab(t, r) {
                        // the resolver sets the recovery mode; the AI then overrides it with the chase (traced)
                        mode = Mode::Chase;
                        self.brain[t].timer = 0;
                    }
                }
                Mode::Recover => {
                    self.brain[t].timer -= 1;
                    if self.brain[t].timer < 1 {
                        mode = Mode::Chase;
                        self.brain[t].timer = 0;
                    } else {
                        missed = true;
                    }
                }
                Mode::Attack if leave => {
                    mode = Mode::KeepNear;
                    self.brain[t].timer = 0;
                }
                Mode::Attack if !free_choice => {
                    mode = Mode::Chase;
                    self.brain[t].timer = 0;
                }
                Mode::Attack => {
                    aim = Some(if opp_nearer { dist.own + dist.lead } else { dist.own - dist.lead });
                    self.brain[t].timer -= 1;
                    if self.brain[t].timer < 1 {
                        let mut stop = false;
                        if dist.lead <= r.attack_distance {
                            self.attack(t);
                            stop = rng.unit() > r.aggression;
                        }
                        if stop {
                            mode = Mode::KeepNear;
                            self.brain[t].timer = 0;
                        } else {
                            self.brain[t].timer = trunc(r.attack_refresh * TICKS);
                        }
                    } else if dist.lead <= r.attack_distance {
                        self.attack(t);
                        self.brain[t].timer = 0;
                    }
                }
                Mode::KeepNear => {
                    aim = Some(dist.lead + dist.own);
                    self.brain[t].timer -= 1;
                    if self.brain[t].timer > 0 {
                        let off = self.brain[t].keep_distance + dist.opp - dist.own;
                        aim = Some(off + dist.own);
                        if (-0.1..=0.1).contains(&off) || !opp_nearer {
                            self.brain[t].keep_flag = false;
                        }
                    } else if free_choice || leave {
                        if rng.unit() > r.aggression
                            && (dist.lead > r.attack_distance || rng.unit() > r.opportunistic_attack)
                        {
                            if leave {
                                self.brain[t].keep_distance = -r.keep_near_trail;
                            } else if dist.snitch > r.chase_range || rng.unit() > r.chase_chance {
                                self.brain[t].keep_distance = if (!opp_boosting || !r.keep_behind_when_opponent_boost)
                                    && !self.brain[t].keep_flag
                                {
                                    (r.keep_near_lead + r.keep_near_trail) * rng.unit() - r.keep_near_trail
                                } else {
                                    -r.keep_near_trail * rng.unit()
                                };
                            } else {
                                mode = Mode::Chase;
                            }
                            self.brain[t].timer = trunc(r.near_player_refresh * TICKS);
                        } else {
                            mode = Mode::Attack;
                            self.brain[t].timer = 0;
                        }
                    } else {
                        mode = Mode::Chase;
                        self.brain[t].timer = 0;
                    }
                }
                Mode::Lead => {
                    self.brain[t].timer -= 1;
                    if self.brain[t].timer < 1 {
                        if (free_choice || leave) && (dist.snitch > r.chase_range || rng.unit() > r.chase_chance) {
                            self.brain[t].keep_distance = 5.0;
                            self.brain[t].timer = trunc(r.near_player_refresh * TICKS);
                        } else {
                            mode = Mode::Chase;
                            self.brain[t].timer = 0;
                        }
                    } else {
                        self.brain[t].keep_distance = 5.0;
                        aim = Some(dist.opp + 5.0);
                    }
                }
                Mode::Idle => {}
            }
            if self.caught.is_some() {
                return;
            }
            // the resolver may have switched to the recovery mode; the chase override above wins (traced)
            self.seekers[t].mode = mode;
        } else {
            let more = self.seekers[o].level < self.seekers[t].level;
            let s = &mut self.seekers[t];
            s.wants_boost = false;
            s.speed = r.initial_speed;
            s.target = r.initial_speed;
            base = AI_SPEED;
            s.more_boost = more;
            lat_target = if t != 0 { 0.75 } else { -0.75 };
            s.mode = Mode::Pick;
            self.brain[t].timer = 0;
        }

        let s = &mut self.seekers[t];
        // the block roll every DefenseRefresh seconds (the block itself is an animation)
        s.defense_t = (s.defense_t - 1.0 / TICKS).max(0.0);
        if s.defense_t <= 0.0 {
            s.defense_t = r.defense_refresh;
            let _block = rng.unit() <= r.defensiveness;
        }
        let was_wanting = s.wants_boost;
        s.wants_boost = false;
        if let Some(a) = aim {
            let d = a - s.dist.own;
            if d > 0.1 {
                s.target *= r.aim_point_speed_boost;
            } else if d < 0.1 {
                s.target *= r.aim_point_brake;
            }
        }
        if opp_boosting {
            self.brain[t].keep_flag = true;
            if !s.more_boost {
                if !s.opp_nearer {
                    s.target *= r.opponent_boost_leading_penalty;
                } else if !s.boost_wait {
                    s.boost_wait = true;
                    s.boost_wait_t = r.time_until_boost;
                }
            } else if !s.boost_wait {
                s.boost_wait = true;
                s.boost_wait_t = 0.3;
            }
        }
        let level_ok = |s: &Seeker, floor: f64| s.level >= r.ai_minimum_boost || (was_wanting && s.level > floor);
        if s.boost_wait {
            s.boost_wait_t -= 1.0 / TICKS;
            if s.boost_wait_t <= 0.0 {
                s.boost_wait_t = 0.0;
                if level_ok(s, 0.05) {
                    s.wants_boost = true;
                    s.target *= r.opponent_boost_trailing_penalty;
                } else {
                    s.wants_boost = false;
                }
            }
            if !s.opp_nearer {
                s.boost_wait = false;
            }
        }
        // the human opponent's forward stick: the countdown runs while it is held back
        if self.brain[t].last_stick < 0.0 && opp_stick < 0.0 {
            s.ignore_t = (s.ignore_t - 1.0 / TICKS).max(0.0);
        } else {
            s.ignore_t = r.ignore_player_move_penalty;
        }
        self.brain[t].last_stick = opp_stick;
        let m = s.mode;
        if matches!(m, Mode::Attack | Mode::Chase | Mode::GoForGrab | Mode::Grab | Mode::Recover) {
            if self.brain[t].always > 0.0 && m != Mode::Attack {
                s.target *= 1.1;
            }
        } else if opp_stick > 0.0 {
            s.target *= r.player_drop_back_speed_adjust;
        } else if opp_stick < 0.0 && s.ignore_t > 0.0 {
            s.target *= r.player_overtake_speed_adjust;
        }
        if m == Mode::Chase {
            s.target *= r.chase_speed_bonus;
        }
        if s.hit_t > 0.0 {
            s.target *= r.hit_speed_penalty;
            s.hit_t -= 1.0 / TICKS;
        }
        if s.hit {
            s.hit = false;
            s.hit_t = r.hit_penalty_time;
        }
        if missed {
            s.target *= r.missed_speed_penalty;
        }
        if m == Mode::Lead {
            s.target *= 1.04;
        }
        if s.target - 1.0 > 0.12 {
            if level_ok(s, 0.0) {
                s.wants_boost = !s.opp_nearer || s.dist.lead < 1.0;
            } else {
                s.wants_boost = false;
                s.target = if opp_boosting { 0.9 } else { 1.0 };
            }
        }
        // AlwaysChaseSnitchAfterTime's later phases (off by default); the human catch-up flag is not modelled
        if self.brain[t].always <= 60.0 {
            if self.brain[t].always <= 10.0 {
                if s.target != s.speed {
                    s.speed += r.speed_change_rate * (s.target - s.speed);
                }
            } else if level_ok(s, 0.05) {
                s.target = 1.3;
                s.wants_boost = true;
            } else {
                s.target = 1.1;
                s.wants_boost = false;
            }
        } else if level_ok(s, 0.05) {
            s.target = 1.15;
            s.wants_boost = true;
        } else {
            s.target = 1.07;
            s.wants_boost = false;
        }
        if opp_stick <= 0.0 {
            self.brain[t].stick_hold = 0;
        } else {
            self.brain[t].stick_hold += 1;
            if self.brain[t].stick_hold > 50 {
                s.speed = 1.0;
                base *= 1.3;
            }
        }
        // the lateral offset wanted: beside the opponent in the wider gap when close, else the centre
        if s.intro == 0 {
            lat_target = 0.0;
            if m != Mode::GoForGrab && m != Mode::Grab && s.dist.lead <= 1.5 {
                let a = (1.0 - opp_off).clamp(0.0, 2.0);
                let b = (1.0 + opp_off).clamp(0.0, 2.0);
                lat_target = if a < b { opp_off - b * 0.5 } else { a * 0.5 + opp_off };
            }
        }
        let side = unit(cross(s.up, s.heading));
        let corr = (lat_target - s.offset) * r.lateral_movement_rate;
        let v = add(add(s.vel, s.heading), scale(side, corr));
        s.vel = scale(unit(v), base * s.speed);
    }

    /// A human seeker's tick (`0x44da50`'s player branch): Boost pressed barges (`0x44ae60`), the steering
    /// (`0x44bd60`), and the grab held starts a grab whose catch test follows when it lands.
    fn human(&mut self, t: usize, r: &RaceRules) {
        let pad = self.seekers[t].pad;
        if pad.boost_pressed {
            self.attack(t);
        }
        self.steer_human(t, r);
        // `0x44bb80`: the reaching stance within `GrabSnitchStanceDistance` of the Snitch, along the trail and apart
        let near = {
            let s = &self.seekers[t];
            (self.snitch.param - s.param).abs() <= r.stance && len(sub(self.snitch.pos, s.pos)) <= r.stance
        };
        if near != self.seekers[t].stance {
            self.seekers[t].stance = near;
            self.events.push(RaceEvent::Stance { team: t, on: near });
        }
        let s = &mut self.seekers[t];
        if s.grab > 0 {
            s.grab -= 1;
            if s.grab == 0 {
                self.resolve_grab(t, r);
            }
        } else if pad.grab_held {
            s.grab = GRAB_REACH_TICKS;
            self.events.push(RaceEvent::Grab { team: t, allowed: true });
        }
    }

    /// `0x44bd60`: the stick's across turns the heading (no further than `MaxYaw` from the trail's direction) and
    /// pushes across the trail (a fifth of it past [`PLAYER_EDGE`] outwards); forward and back set the thrust
    /// (1.2 times when boosting); the velocity is damped, and bent towards the heading by the turn, which builds up
    /// while the stick is across and dies away when it is not.
    fn steer_human(&mut self, t: usize, r: &RaceRules) {
        let f = self.spline.frame(self.seekers[t].param);
        let s = &mut self.seekers[t];
        let thrust = if s.boosting { 1.2 } else { 1.0 };
        let [v, h] = s.pad.stick.map(|x| x.clamp(-1.0, 1.0));
        s.stick = [v, h];
        let side = unit(cross(s.up, s.heading));
        let turned = unit(add(s.heading, scale(side, h * r.yaw_speed * 0.1)));
        let yaw = r.max_yaw.to_radians();
        s.heading = if dot(turned, f.fwd) > cos(yaw) {
            turned
        } else {
            let k = if dot(turned, f.side) > 0.0 { 1.0 } else { -1.0 };
            unit(add(scale(f.fwd, cos(yaw)), scale(f.side, k * sin(yaw))))
        };
        let outwards = (h < 0.0 && s.offset < -PLAYER_EDGE) || (h > 0.0 && s.offset > PLAYER_EDGE);
        let lateral = if outwards { 0.2 } else { 1.0 };
        s.vel = add(s.vel, scale(side, h * r.lateral_vel * lateral));
        let a = v * 0.333 - 0.6666;
        s.vel = sub(s.vel, scale(s.heading, a * r.input_mod * thrust));
        s.vel = scale(s.vel, r.vel_damping);
        let speed = len(s.vel);
        let rate = r.player_turn_rate;
        s.turn = if h != 0.0 {
            s.turn + rate * h
        } else if s.turn > 0.0 {
            (s.turn - rate).max(0.0)
        } else {
            (s.turn + rate).min(0.0)
        };
        s.turn = s.turn.clamp(-r.player_heading_bias, r.player_heading_bias);
        s.vel = scale(unit(add(s.vel, scale(s.heading, s.turn.abs()))), speed);
    }

    /// `0x44a870` / `0x44ae60` without the animation choice: a tackle starts.
    fn attack(&mut self, t: usize) {
        let s = &mut self.seekers[t];
        if s.tackle == 0 {
            s.tackle = TACKLE_TICKS;
            s.tackle_landed = false;
            self.events.push(RaceEvent::Attack { team: t });
        }
    }

    /// `0x4489f0`: the catch test with the parameter gap; a catch ends the race, a miss sends an AI to recover.
    fn resolve_grab(&mut self, t: usize, r: &RaceRules) -> bool {
        let s = &self.seekers[t];
        let g = GrabAttempt {
            ai: s.ai,
            ai_allowed: s.grab_allowed,
            x: self.snitch.param - s.param,
            seeker: s.pos,
            snitch: self.snitch.pos,
            lateral_offset: s.offset,
        };
        if can_grab(&r.seeker, &g) {
            self.caught = Some(t);
            self.events.push(RaceEvent::Caught { team: t });
            true
        } else {
            if s.ai {
                self.seekers[t].mode = Mode::Recover;
                self.brain[t].timer = trunc(r.missed_recovery * TICKS);
            }
            self.events.push(RaceEvent::Missed { team: t });
            false
        }
    }

    /// `0x44b4d0`: boosting drains `StartBoostCost` a tick; otherwise the level recharges by
    /// `0.004 * s^2`, `s` being 1 within the sweet spot of the trail's centre and falling to 0 at its edge, through
    /// the smoothed copy (a third of the way a tick).
    fn boost_power(&mut self, t: usize, r: &RaceRules) {
        let w = r.half_width(1.0, self.snitch.param, self.seekers[t].param);
        let s = &mut self.seekers[t];
        let o = s.offset.abs().min(1.0);
        let half = w * 0.5;
        let (x, sweet) = (o * half, r.sweet_spot * half);
        let k = if x >= sweet {
            let a = ((x - sweet) / (half - sweet) * 90.0 + 90.0).to_radians();
            1.0 - cos(a).abs()
        } else {
            1.0
        };
        let mut level = s.level;
        // a human boosts holding the button with `MinimumBoostLevel`, or already boosting with any left
        let wants = if s.ai {
            s.wants_boost && level > AI_BOOST_FLOOR
        } else {
            s.pad.boost_held && (level >= r.minimum_boost_level || (s.boosting && level > 0.0))
        };
        s.boosting = false;
        if wants {
            level -= r.start_boost_cost;
            s.boosting = true;
        }
        if !s.boosting {
            level += RECHARGE * k * k;
            if s.shown < level {
                s.shown = ((level - s.shown) / 3.0 + s.shown).min(1.0);
                level = s.shown;
            }
        }
        s.level = level.max(0.0);
    }
}

/// The fraction of `seg` that `along` (parallel to it) covers, read on `seg`'s largest component.
fn along_fraction(along: Vec3, seg: Vec3) -> f64 {
    let a = seg.map(f64::abs);
    let k = if a[0] > a[1] && a[0] > a[2] {
        0
    } else if a[2] > a[1] {
        2
    } else {
        1
    };
    along[k] / seg[k]
}

/// `0x44c430`: turn the velocity (keeping its speed) and the heading towards the trail `look` points ahead: towards
/// that point (`at_point`) or along the trail's direction there; both lose their component along `up`.
fn steer(sp: &Spline, s: &mut Seeker, at_point: bool, look: f64, heading_w: f64, vel_w: f64, up: Vec3) {
    let speed = len(s.vel);
    let t = s.param + look;
    let dir = if at_point { unit(sub(sp.position(t), s.pos)) } else { sp.frame(t).fwd };
    s.vel = scale(unit(add(s.vel, scale(dir, speed * vel_w))), speed);
    s.vel = sub(s.vel, scale(up, dot(up, s.vel)));
    let h = unit(add(s.heading, scale(dir, heading_w)));
    s.heading = unit(sub(h, scale(up, dot(up, h))));
}

/// Whether the two seekers' broom capsules touch (sampled).
fn capsules_touch(a: &Seeker, b: &Seeker) -> bool {
    const N: usize = 12;
    let pts = |s: &Seeker| -> Vec<Vec3> {
        (0..=N)
            .map(|i| add(s.pos, scale(s.heading, -BROOM_BACK + (BROOM_BACK + BROOM_FRONT) * i as f64 / N as f64)))
            .collect()
    };
    let (pa, pb) = (pts(a), pts(b));
    pa.iter().any(|p| pb.iter().any(|q| len(sub(*p, *q)) < 2.0 * BROOM_RADIUS))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spline::Point;

    /// A level circle of 300 points and radius 150 (about 3.1 units between points, like the retail splines).
    pub(super) fn circle() -> Spline {
        let pts = (0..300)
            .map(|i| {
                let a = i as f64 / 300.0 * std::f64::consts::TAU;
                let h = -a / 2.0;
                Point { pos: [150.0 * cos(a), 0.0, 150.0 * sin(a)], rot: [0.0, -sin(h), 0.0, cos(h)] }
            })
            .collect();
        Spline::new(pts, 1.0)
    }

    fn rules() -> RaceRules {
        RaceRules::from_console(&Console::new())
    }

    #[test]
    fn rules_read_their_defaults() {
        let r = rules();
        assert_eq!((r.trail_width, r.grab_chance, r.keep_near_lead), (1.25, 0.3f32 as f64, 4.0));
        assert!(r.keep_behind_when_opponent_boost && !r.leave_me_alone);
        // the trail narrows near the Snitch
        assert!((r.half_width(1.0, 10.0, 0.0) - 1.25).abs() < 1e-12);
        assert!((r.half_width(1.0, 10.0, 9.5) - 0.875).abs() < 1e-12);
    }

    #[test]
    fn the_hold_spreads_the_seekers_across_the_trail() {
        let (r, mut rng) = (rules(), Rand::new(1));
        let mut race = Race::new(circle(), &r, [true, true]);
        assert_eq!(race.seekers.iter().map(|s| s.param).collect::<Vec<_>>(), [0.0, 2.0]);
        for _ in 0..INTRO_TICKS {
            race.snitch_tick(&r);
            for t in [0, 1] {
                race.seeker_tick(t, &mut rng, &r);
            }
            race.collide();
        }
        let [a, b] = &race.seekers;
        // team 0 drifts to -0.75, team 1 to +0.75; both moved forward at about 25 * 0.98 units per second
        assert!(a.offset < -0.3 && b.offset > 0.3, "{} {}", a.offset, b.offset);
        assert!((len(a.vel) - 24.5).abs() < 0.5, "{}", len(a.vel));
        assert!(a.param > 5.0 && b.param > 7.0, "{} {}", a.param, b.param);
        // the last tick of the hold already runs the AI's first pick
        assert!(race.seekers.iter().all(|s| s.mode == Mode::KeepNear && s.intro == 0));
    }

    #[test]
    fn an_ai_race_ends_with_a_catch() {
        // the team scripts set AlwaysChaseSnitchAfterTime (2520 to 3240 ticks); without it two AIs never commit
        let mut r = rules();
        r.always_chase_after = 2520.0;
        let mut rng = Rand::new(7);
        let mut race = Race::new(circle(), &r, [true, true]);
        let mut modes = std::collections::BTreeSet::new();
        for _ in 0..24 * 60 * 10 {
            race.snitch_tick(&r);
            for t in [0, 1] {
                race.seeker_tick(t, &mut rng, &r);
                modes.insert(race.seekers[t].mode as u8);
            }
            race.collide();
            if race.caught.is_some() {
                break;
            }
            for s in &race.seekers {
                assert!(s.pos.iter().all(|v| v.is_finite()));
                assert!(s.offset.abs() <= 1.0 + 1e-6, "{}", s.offset);
                assert!(s.param <= race.snitch.param);
            }
        }
        eprintln!("{} ticks, modes {modes:?}, events {:?}", race.ticks, &race.events[..race.events.len().min(20)]);
        assert!(race.caught.is_some(), "no catch in ten minutes");
        assert!(race.ticks > 2520 && race.ticks < 2520 + 24 * 60, "{}", race.ticks);
        assert!(modes.contains(&(Mode::Chase as u8)) && modes.contains(&(Mode::KeepNear as u8)));
        assert!(race.seekers.iter().any(|s| s.level > 0.0));
    }

    #[test]
    fn boost_recharges_at_the_centre_only() {
        let r = rules();
        let mut race = Race::new(circle(), &r, [true, true]);
        race.seekers[0].intro = 0;
        race.boost_power(0, &r);
        // a third of 0.004 through the smoothed copy
        assert!((race.seekers[0].level - 0.004 / 3.0).abs() < 1e-12);
        race.seekers[1].offset = 1.0;
        race.boost_power(1, &r);
        assert!(race.seekers[1].level.abs() < 1e-12);
        // boosting drains StartBoostCost
        let s = &mut race.seekers[0];
        s.level = 0.5;
        s.wants_boost = true;
        race.boost_power(0, &r);
        assert!(race.seekers[0].boosting && (race.seekers[0].level - (0.5 - 0.008f32 as f64)).abs() < 1e-9);
    }

    /// A race with team 0 human on `pad` for `ticks` after the hold; the human's speed at the end.
    fn human_race(pad: SeekerPad, ticks: usize) -> (Race, f64) {
        let (r, mut rng) = (rules(), Rand::new(3));
        let mut race = Race::new(circle(), &r, [false, true]);
        for i in 0..INTRO_TICKS as usize + ticks {
            race.seekers[0].pad = if i < INTRO_TICKS as usize { SeekerPad::default() } else { pad };
            race.snitch_tick(&r);
            for t in [0, 1] {
                race.seeker_tick(t, &mut rng, &r);
            }
            race.collide();
            if race.caught.is_some() {
                break;
            }
        }
        let speed = len(race.seekers[0].vel);
        (race, speed)
    }

    #[test]
    fn a_human_seeker_flies_on_the_stick() {
        // the stick at rest keeps about the AI's 25 units a second, forward (up, negative) more, back less
        let (rest, v0) = human_race(SeekerPad::default(), 96);
        let (_, fwd) = human_race(SeekerPad { stick: [-1.0, 0.0], ..SeekerPad::default() }, 96);
        let (_, back) = human_race(SeekerPad { stick: [1.0, 0.0], ..SeekerPad::default() }, 96);
        assert!((v0 - 25.4).abs() < 1.5, "{v0}");
        assert!(fwd > v0 + 5.0 && back < v0 - 5.0, "{back} {v0} {fwd}");
        let s = &rest.seekers[0];
        assert!(s.param > 10.0 && s.offset.abs() <= 1.0 + 1e-6, "{} {}", s.param, s.offset);
        // across the stick moves it across the trail
        let (right, _) = human_race(SeekerPad { stick: [0.0, 1.0], ..SeekerPad::default() }, 24);
        let (left, _) = human_race(SeekerPad { stick: [0.0, -1.0], ..SeekerPad::default() }, 24);
        assert!((right.seekers[0].offset - left.seekers[0].offset).abs() > 0.3);
    }

    #[test]
    fn a_human_grab_next_to_the_snitch_catches_it() {
        let r = rules();
        let mut race = Race::new(circle(), &r, [false, true]);
        let s = &mut race.seekers[0];
        s.intro = 0;
        s.pad.grab_held = true;
        // beside the Snitch on its trail
        race.snitch.param = 20.0;
        race.snitch.pos = race.spline.position(20.0);
        let s = &mut race.seekers[0];
        s.param = 19.5;
        s.pos = race.spline.position(19.5);
        s.offset = 0.0;
        for _ in 0..=GRAB_REACH_TICKS {
            race.human(0, &r);
            race.seekers[0].pos = race.spline.position(19.5);
        }
        assert_eq!(race.caught, Some(0), "{:?}", race.events);
    }
}
