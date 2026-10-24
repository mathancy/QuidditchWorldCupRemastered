//! A match as a 24 Hz tick loop: two teams of seven, the Quaffle, the AI's possession, pass and shot decisions, the
//! pass aim, the throw, shots and goals, the contact test that makes catches, saves and interceptions, the AI
//! defender's press with tackles and lunges, the keeper off the ball and the kick-off placements (`docs/GAMEPLAY.md`, "Team object" to "Player movement").
//!
//! What is traced is ported as traced; the gaps are filled with placeholders until their code is read, each kept in
//! one place so it can be replaced:
//! - **Movement of the active players** ([`Match::move_players`]): the original moves them with the broom handling
//!   `0x442ac0` through `0x436490` and `0x4362b0`, from inputs the AI states set. Here the carrier flies at
//!   `Handling.MaxSpeed * Chaser.Speed` to the end it attacks, the active defender to the ball, and a keeper with
//!   the ball to his lane. Off the ball, chasers follow the traced Formotions ([`crate::formation`]) once
//!   [`Match::use_formotions`] has the databases (else lanes ahead of the carrier, or the ball), and the keeper the
//!   traced guarding (`0x43f380`, [`crate::keeper`]).
//! - **The defending team's active player** is kept while the opponents have the ball and picked again at each of
//!   their passes (`0x4422e0`, traced); with the ball in no list (the kick-off) it is the chaser nearest the ball
//!   (**[I]**). It runs the
//!   defender state 0x21 whenever the opponents hold the ball: before the press, the approach ([`crate::approach`],
//!   traced: a `DirectFrontal` or `BlockSide` formotion round the carrier, then its homing).
//! - **Kick-off**: players start at the traced match-start placements (`0x452950`). [`Match::start_toss`] (the
//!   type-3 event, `0x4519d0`) puts the Quaffle over the centre and the teams race for it (state 0x0e, the human's
//!   state 2; traced): a human claims with Action or a push of the stick from 45 ticks and wins from 50 to 59, the
//!   AI claims at 60, and the winner's centre chaser takes the toss at 70 or later. Without it (the tests) team 0's
//!   second chaser starts with the Quaffle.
//! - **A ball fumbled to a Bludger** stays in its team's ball list (traced): only the opponents can pick it up.
//! - **After a goal** the kick-off happens when the ball's type-7 event fires, 23 ticks after the goal; the goal
//!   cutscene that runs in between in the original is skipped (inferred timing).
//! - **The walls** ([`crate::walls`]) are traced: every move ends with the movers' commit against them, and the
//!   pass aim sweeps its target inside them. Where the original reads a holder's facing, its velocity stands in.
//!
//! - **Beaters and Bludgers** ([`crate::bludger`]): the Bludgers' and beaters' formotions, the Bludger move (the
//!   human's on Boost, the AI's by its weights), its hits and the beaters' blocks.
//!
//! - **The carrier's states** under 0x0f: possession (0x11) with the pass (0x17) and the evade, and the weaves
//!   while the opponents' defensive special (0x12, 0x15) or Bludger move (0x13, 0x16) runs; the defensive
//!   special's run (0x1e) with its lunge (0x24-0x26). States 0x18-0x1c are never asked for in the retail game.
//!
//! Not modelled yet: the tackle knock-back's effect, the lunge's intercept after its first command (the pursuit
//! stands in), the defensive-move databases and the rope pull.

use std::rc::Rc;

use qwc_formats::formotion::Database;

use crate::approach::{self, Approach, History};
use crate::bludger::{self, Bludger};
use crate::console::{Console, Value};
use crate::evade::{self, Evade};
use crate::fmath::{acos, cos, exp, sin};
use crate::formation::{self, Body, Db, Frame, Runner};
use crate::handling::{self, Call};
use crate::human::{self, AttackSub, DefendSub, Input, ThrowStep};
use crate::keeper::{self, BallView, Settle};
use crate::passing::{self, PassAi, PassPlan, PassSituation, Target, PASS_STATE_TICKS, TICKS, WIND_UP_TICKS};
use crate::pitch::{self, HeightGraph, Pitch};
use crate::powerbar::{self, PowerBar};
use crate::race::{Race, RaceEvent, RaceRules, CATCH_POINTS};
use crate::rng::Rand;
use crate::shooting::{self, Hoop};
use crate::spline::Spline;
use crate::tackle::{self, LungeEnd, Outcome, TeamTackle};
pub use crate::vec3::Vec3;
use crate::vec3::{add, dot, len, scale, sub, unit};
use crate::walls::Walls;

/// The ticks between a carrier's dodges (`0x606640`).
pub const DODGE_COOLDOWN: i32 = 14;

/// The receiver's speed as he reaches a pass's target (`0x425c10`).
const RECEIVE_SPEED: f64 = 15.0;

/// The defensive special's run before the tackle: 48 ticks (`0x30`, both enters `0x455a7b`, `0x4472f0`; the
/// human's 96 in the race or a challenge, where the port makes none).
const SPECIAL_TACKLE_TICKS: i32 = 48;
/// The roll (`0x42b090`): `[0x669060]` segments of `[0x669064]` seconds each (7 ticks), round `[0x66905c]` and
/// `[0x669058]` seconds' flight ahead.
const ROLL_SEGMENTS: u32 = 4;
const ROLL_TIME: f32 = 0.33;
const ROLL_RADIUS: f64 = 2.0;
const ROLL_AHEAD: f64 = 1.5;
/// `[0x606610]`: the reach in which the Quaffle cuts the AI's run short (`0x4380b0`), and the latch it sets.
const SPECIAL_REACH: f64 = 1.5;
const SPECIAL_LATCH: i32 = 5000;
/// The lunge (`0x437a90`): a 10-tick command to `[0x606608]` = 6 short of the target, at 6 units a second on.
const LUNGE_TICKS: i32 = 10;
const LUNGE_SHORT: f64 = 6.0;

/// Player roles in the line-up order of `0x4342c0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Keeper,
    Chaser,
    Beater,
    Seeker,
}

#[derive(Debug, Clone)]
pub struct Player {
    pub team: usize,
    pub role: Role,
    pub pos: Vec3,
    /// Units per second.
    pub vel: Vec3,
    /// Forward (matrix row 2, `+0x50`).
    pub fwd: Vec3,
    /// The velocity a tick before (the mover's `+0x70`), which the carriers' height kick reads.
    pub prev_vel: Vec3,
    /// A pending movement command (the tackle's flight); while it runs the player's handling is skipped.
    pub cmd: Option<Command>,
    /// The commands after it, each taken up as the one before ends (a dodge's chain, [`Match::dodge_commands`]).
    pub queue: Vec<Command>,
    /// In this game: a challenge has only the players of its description ([`Match::use_description`]); the
    /// others are parked out of play and skipped by every update.
    pub present: bool,
    /// Knocked back by a Bludger until this tick (`0x424080`): **[I]** no team picks him as its active player
    /// meanwhile.
    pub knocked: u32,
}

/// A channel-0 movement command: the mover's Bezier from the state at `start` to `(to, to_vel)` at `end`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Command {
    pub start: u32,
    pub end: u32,
    pub from: Vec3,
    pub from_vel: Vec3,
    pub to: Vec3,
    pub to_vel: Vec3,
}

/// The AI state of a team's carrier: under state 0x0f (`0x455e40`), 0x11 (possession, with the pass 0x17 under
/// it), or 0x12 and 0x13, the carries while the opponents' defensive special or Bludger move runs.
#[derive(Debug, Clone, Copy, PartialEq)]
enum CarrierState {
    /// `sub`: the evade sub-behaviour has been entered.
    Possession {
        timer: i32,
        sub: bool,
    },
    Pass {
        timer: i32,
        dir: Vec3,
    },
    /// State 0x12's sub-state 0x15 (`0x4585a0`): the weave while the opponents' defensive special runs.
    Weave,
    /// State 0x13's sub-state 0x16 (`0x4586c0`): the turns while the opponents' Bludger move runs; `n` its count
    /// (`+0x10`, cleared on entering, `0x4586b0`).
    BludgerWeave {
        n: i32,
    },
}

/// `[0x668e08]`: the weave's phase step a tick (shared by every carrier, `[0x10439dc]`), wrapped past `2 pi`
/// (`[0x5bf01c]`).
const WEAVE_STEP: f32 = 0.1;
/// State 0x16: the turns start `[0x606078]` = 24 counts in, a sine of period `[0x606070]` = 120 counts and
/// amplitude `[0x606074]` = 1.5 across; the count goes up by 2 a tick.
const BLUDGER_WEAVE_DELAY: f32 = 24.0;
const BLUDGER_WEAVE_PERIOD: f32 = 120.0;
const BLUDGER_WEAVE_SIDE: f32 = 1.5;

#[derive(Debug, Clone)]
pub struct Team {
    pub index: usize,
    /// Player indices: keeper, chasers 0-2, beaters 0-1, seeker (team `+0x1dc8`, `+0x1db4[3]`, ...).
    pub keeper: usize,
    pub chasers: [usize; 3],
    pub beaters: [usize; 2],
    pub seeker: usize,
    pub ai: PassAi,
    pub plan: PassPlan,
    /// `+0x40` degrees.
    pub pass_cone: f64,
    /// `+0x48`, `+0x4c`.
    pub other_team_catch_mod: f64,
    pub this_team_catch_mod: f64,
    /// `+0x50` (`Chaser.Speed`) and `+0x2c` (`Keeper.Speed`).
    pub chaser_speed: f64,
    pub keeper_speed: f64,
    /// `+0x28` (`AI.ShootingSkill`, constructor 0.5) and `+0x30` (`Keeper.Mode`, default 2).
    pub shooting_skill: f32,
    pub keeper_mode: i32,
    /// `+0x5c`: combo passes in a row.
    pub combo_chain: i32,
    /// `SeekerPowerBar.PassIncrement` (team `+0x88`) and `ComboBarMultiplier` (`+0x74`).
    pub pass_increment: f64,
    pub combo_bar_multiplier: f64,
    /// `+0x1cac`, `+0x1cb4`: calls left on the pass-target lock and the locked receiver.
    lock: (i32, Option<usize>),
    /// `+0x174`: receiver candidate of the pass being aimed; `+0x17c` the target; `+0x18c` flight ticks.
    receiver: Option<usize>,
    target: Vec3,
    flight: i32,
    in_cone: bool,
    /// The pass target the effects show (`0x43dd30`'s message on `0x1043b54`): the receiver while the aim has him
    /// in the cone; none outside it, at the throw (`0x43ebbf`) and when the Quaffle changes hands (`0x431e4c`,
    /// `0x4421f4`).
    pub pass_target: Option<usize>,
    state: Option<CarrierState>,
    shot: ShotPlan,
    /// The keeper's hoop-switch bookkeeping and this tick's movement command (target, divisor).
    settle: Settle,
    keeper_cmd: Option<(Vec3, i32)>,
    /// The AI's state 0x20 (`0x455730`) on the active player, as Tackle 'n' Shoot's defence asks for it: its ticks
    /// (0 the dash to the hoops still to give, then the keeper's guard), and the guard's command this tick.
    keeper_run: Option<u32>,
    guard_cmd: Option<(Vec3, i32)>,
    /// The challenge game's attack direction for this team when it is not its own (`0x45f860`: team 1 holding the
    /// Quaffle in Tackle 'n' Shoot attacks `[0x1018fa0]`); set by [`Match::sync_attack`].
    pub dir_override: Option<f64>,
    /// The tackle fields, `Tackle.LungeConeAngle` (team `+0x44`, default 90), the active player (`+0xc4`), the AI
    /// defender's press counter (state 0x21 `+0x450`) and the stick the active player was given this tick.
    pub tackle: TeamTackle,
    pub lunge_cone: f64,
    /// The player the team acts through (the carrier, or the human's chosen flyer).
    pub active: Option<usize>,
    press: i32,
    /// The carrier's evade state and `Evading.*`; this tick's handling call for the active player.
    evade: Evade,
    pub evade_probs: evade::Probs,
    call: Option<Call>,
    pub score: i32,
    /// A human plays the team (its AI byte `+0x8e` clear): its update runs the human brain on [`Team::input`].
    pub human: bool,
    /// This tick's input struct (`0x442690`; [`Match::set_input`]) and the human brain's state.
    pub input: Input,
    human_state: Option<human::State>,
    /// `+0x154`: how many ticks Boost has been held; on release the count turns negative once.
    pub boost_hold: i32,
    /// The team's Bludger move (team state 5, [`crate::bludger`]).
    pub bludger: Option<bludger::Move>,
    /// `+0x1450[i]`: the beaters' modes (0 idle, 1 the Bludger ready, 2 summoning, 4 and 5 blocking); their
    /// formotion blocks (`+0x1460 + i * 0x420`); the block counter (`+0x54`) and the block point (`+0xa24`, its
    /// velocity `+0xa34`).
    pub beater_modes: [u8; 2],
    beater_runners: [Runner; 2],
    block_count: i32,
    block_point: (Vec3, Vec3),
    /// A Bludger hit on one of the team's players (his `+0x170`, set by the game's event 14), handled at the start
    /// of the team's next update (`0x442110`): the player and the Bludger's velocity.
    hit: Option<(usize, Vec3)>,
    /// The AI's defence choice and its tick count (`0x4552d0`: 0 chase, 2 the Bludger).
    defence: Option<(u32, i32)>,
    /// `+0xacc`: Bludger hits.
    pub bludger_hits: i32,
    /// Team `+0xad0`: special moves made.
    pub special_moves: u32,
    /// `SeekerPowerBar.UberPenalty` (team `+0x84`): what the team's uber move takes from the other's bar.
    pub uber_penalty: f64,
    /// `TeamOPs.SpecialMoveShootWeight` and `UberMoveWeight` (`0xf918dc`, `0xf918e4`): the AI's chance to make its
    /// special move or uber move when it may.
    pub special_shoot_weight: f64,
    pub uber_weight: f64,
    /// The AI's counter `c1` (ctx `+0x58` block `+4`): ticks with the ball (state 0x0f's, `0x455e40`) since the
    /// AI last weighed a special or uber move and missed.
    pub c1: i32,
    /// The defensive special's run (the human's sub-state 10, the AI's state 0x1e); while it runs the team's state
    /// is 6 (`0x4376c0`).
    pub special_tackle: Option<SpecialRun>,
    /// The AI defender's approach (state 0x21, [`crate::approach`]), the opposing active player's history it reads
    /// (`+0xae0`) and the team's `Formotions.TweakTime AI_DEF` (formation block `+0x30`).
    approach: Approach,
    history: History,
    tweak_ai_def: f64,
}

/// The defensive special's run: the AI's state 0x1e (enter `0x455a70`, update `0x455ab0`) or the human's sub-state
/// 10 (enter `0x4472d0`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpecialRun {
    /// `+0x10`: the ticks left before the tackle.
    pub left: i32,
    /// `+0x18`: the ticks the Quaffle has been in reach (`0x4380b0`); past 5 the count is cut to 0.
    pub near: i32,
    /// Team `+0x254`: the reach latch, 5000 when the Quaffle is within `[0x606610]` of the runner, less 1 a tick.
    pub latch: i32,
    /// Team `+0x250`: the lunge's first command made (`0x437a90`).
    pub lunged: bool,
    /// The AI's (state 0x1e), with its reach and its lunge; the human's sub-state 10 update (`0x447310`) is not
    /// read (**[I]** the count alone).
    pub ai: bool,
}

impl SpecialRun {
    fn new(ai: bool) -> SpecialRun {
        SpecialRun { left: SPECIAL_TACKLE_TICKS, near: 0, latch: 0, lunged: false, ai }
    }
}

/// The shot wind-up of `0x4568a0`: `c0` (ctx `+0x58`, ticks left), the total `N` (`0x668e3c`), and the aim the
/// wind-up keeps updating (team `+0x16c` hoop, `+0x170` power).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct ShotPlan {
    left: i32,
    total: i32,
    hoop: Option<usize>,
    power: f64,
}

impl Team {
    /// `+0x18`: team 0 attacks towards `-x` (the challenge game's `0x45f860`: [`Team::dir_override`] first).
    pub fn attack_dir(&self) -> f64 {
        if let Some(d) = self.dir_override {
            return d;
        }
        if self.index == 0 {
            -1.0
        } else {
            1.0
        }
    }
}

/// What the Quaffle is doing while no one holds it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FlightKind {
    /// A pass (`0x425c10`): states 5 at the start, 6 from `start + 2`, 7 at `end - 2`; attaches to the receiver.
    Pass { receiver: usize },
    /// A shot (`0x4265c0`) at hoop `hoop` by `team`: states 8 at the start, 9 from `start + 1`, 10 at `end - 2`; the
    /// goal event fires at `end - 1`.
    Shot { hoop: usize, team: usize },
    /// A player who touched the ball takes it (`0x421f10`, inferred: the ball moves onto him and attaches at `end`).
    Take { player: usize },
    /// After a goal, the ballistic drop (`0x464bc0`) from `end` of the shot for 24 ticks; the kick-off follows.
    Drop { team: usize, vel: Vec3 },
    /// A tackle (`0x428260`): the ball stays on the victim until `end`, then goes to the tackler (state 12, then 3).
    /// Inferred: the victim no longer counts as the holder meanwhile, so he cannot pass or shoot.
    Tackle { victim: usize, tackler: usize },
    /// Loose after a Bludger hit (`0x421900`, not read; **[I]** it slows over a second and falls to a unit above
    /// the floor) until someone touches it (state 0).
    Loose { vel: Vec3 },
    /// The kick-off's toss (`0x4519d0`): the Quaffle rises from 1.5 over the centre and comes back down to it by
    /// the toss ([`toss_height`]), where the kicking team's player, once one is chosen, takes it at `end`
    /// (`0x421f10`). It cannot be caught meanwhile.
    Toss { player: Option<usize> },
}

/// The kick-off at a match's start: game `+0x230` (`0x432e90`, set up by the type-3 event's `0x4519d0`; its clock
/// `0x432ef0`), raced for by the brains' state 0x0e (`0x453270`) and the human's state 2 (`0x446430`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KickOff {
    /// `+0x1c`: the tick it started; `+0x4` (the clock) is the ticks since.
    pub start: u32,
    /// `+0x20`: the toss, 70 ticks after the start (`0x6060e4`).
    pub toss: u32,
    /// `+0x14`: the team that takes the Quaffle, once decided.
    pub kick: Option<usize>,
    /// `+0x18[t]`: team `t`'s player has his commands.
    pub engaged: [bool; 2],
    /// The human's stick pushed forward last tick (the brain's `+0x10`): a push claims on its first tick.
    pub pushed: [bool; 2],
    /// `+0x1a`: the window's message has been posted ([`Event::KickOffWindow`]).
    pub window: bool,
}

/// The kick-off's clock (`0x668ec0`, `0x668ebc`, `0x668eb8`): a human may claim from 45 ticks, wins with a claim
/// from 50 to 59, and the AI claims at 60; the toss is at 70 (`0x6060e4`), 1.5 over the centre (`0x6060e8`).
pub const KICK_CLAIM: u32 = 45;
pub const KICK_WINDOW: u32 = 50;
pub const KICK_AI: u32 = 60;
pub const KICK_TOSS: u32 = 70;
const TOSS_HEIGHT: f64 = 1.5;
/// The Quaffle's rise at the toss's start (`0x6060ec`, the command's starting velocity), a second.
const TOSS_RISE: f64 = 6.0;

/// The toss's Quaffle above the centre's floor `ticks` after the kick-off's start (`0x4519d0`): the mover's curve
/// (`0x421780`) from 1.5 up, rising at 6 a second, to the floor at the toss (`[0xf91904]` over the floor, a global
/// nothing writes, so 0), at rest; there it waits. So it rises about 2.25 and is on its way down when the window
/// opens. **[I]** the command runs from the start to the toss.
pub fn toss_height(ticks: u32) -> f64 {
    let f = (ticks as f64 / KICK_TOSS as f64).min(1.0);
    let d = KICK_TOSS as f64 / TICKS;
    let (p, _) = keeper::bezier([0.0, TOSS_HEIGHT, 0.0], [0.0, TOSS_RISE, 0.0], [0.0; 3], [0.0; 3], d, f);
    p[1]
}

/// The Quaffle away from a holder: a channel-0 command from `start_tick` to `end_tick`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Flight {
    /// The flight's flag (`+0x38`): a flatter arc (`c = 3` in place of 6), for a human's throw wound up to the end.
    pub flat: bool,
    pub kind: FlightKind,
    pub start_tick: u32,
    pub end_tick: u32,
    pub from: Vec3,
    pub to: Vec3,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// A carrier started winding up (state 0x17).
    WindUp { tick: u32, player: usize },
    /// The Quaffle was thrown towards `receiver`, arriving at `arrival`.
    Pass { tick: u32, from: usize, receiver: usize, arrival: u32, in_cone: bool, volley: bool },
    /// `player` caught the Quaffle.
    Catch { tick: u32, player: usize },
    /// A carrier in the goal area started a shot wind-up of `ticks` ticks.
    ShotWindUp { tick: u32, player: usize, ticks: i32 },
    /// `player` shot at hoop `hoop` (an index into [`Match::hoops`]) with power `power`; the goal is due at `goal`.
    Shot { tick: u32, player: usize, hoop: usize, power: f64, goal: u32 },
    /// `player` touched the ball in flight or loose and takes it; `state` is the Quaffle's state (9 a shot: a save;
    /// 6 a pass: an interception; 0 loose).
    Touch { tick: u32, player: usize, state: u8 },
    /// `team` scored at hoop `hoop`; `combo`: with a combo chain running (the challenge game reads it, `0x4593e0`).
    Goal { tick: u32, team: usize, hoop: usize, combo: bool },
    /// The goal's cutscene roll asked for one (game `+0x228`, `0x432100`): 1 the crowd's, 3 the celebration
    /// (`Score2`).
    GoalCutscene { tick: u32, team: usize, kind: u8 },
    /// `player` of `team` made the special move at `hoop` (`kind`: 0 `Shoot30`, 1 `shvol`, 2 `shrol`); a goal
    /// follows in the same tick.
    SpecialMove { tick: u32, team: usize, player: usize, hoop: usize, kind: u8 },
    /// `player` of `team` made the team's uber move at `hoop`; a goal follows in the same tick.
    UberMove { tick: u32, team: usize, player: usize, hoop: usize },
    /// `tackler` of `team` took the Quaffle from `victim` with the defensive special (`kind`: 0 `tacar`, 1 `takick`,
    /// 2 `tasum`).
    SpecialTackle { tick: u32, team: usize, tackler: usize, victim: usize, kind: u8 },
    /// Play restarts with `team`'s keeper holding the Quaffle.
    KickOff { tick: u32, team: usize },
    /// The match's kick-off went to `team`: its `player` flies to take the toss.
    Toss { tick: u32, team: usize, player: usize },
    /// The kick-off's window opened (its clock at 50, `0x432ef0`'s message `0x1043a60` with 1): a human's claim
    /// now wins it. The effects burst `QUAFFLE_KICKED_OFF` at the Quaffle (`0x4b5f40`).
    KickOffWindow { tick: u32 },
    /// The two power bars are full: the Seeker game starts.
    SeekerGame { tick: u32 },
    /// `tackler` lunged at the carrier (the boost tackle, request 0x12).
    Lunge { tick: u32, player: usize },
    /// `player`'s lunge missed (request 0x13); his team is stunned for 12 ticks.
    LungeMiss { tick: u32, player: usize },
    /// `tackler` tackled `victim`; the ball changes hands at `end`. `lunge`: at the end of a lunge (request 0x14,
    /// else 0xf).
    Tackle { tick: u32, tackler: usize, victim: usize, end: u32, lunge: bool },
    /// `tackler` missed a dodging carrier.
    Whiff { tick: u32, player: usize },
    /// The carrier `player` dodged (`0x436790`): `kind` 6 and 7 the roll right and left, 8 the loop, 9 the boost.
    Dodge { tick: u32, player: usize, kind: u8 },
    /// Something happened in the Seeker game's race; a catch ([`RaceEvent::Caught`]) ends the match.
    Race { tick: u32, event: RaceEvent },
    /// `beater` began summoning his team's Bludger (`0x43c3c0`).
    BludgerSummon { tick: u32, team: usize, beater: usize },
    /// The summoning beater swings, 10 ticks before the launch (`0x4228c0`).
    BeaterSwing { tick: u32, player: usize },
    /// `team`'s Bludger was launched (`0x43c750`).
    BludgerLaunch { tick: u32, team: usize },
    /// `team`'s Bludger hit `victim` (`0x43da40`); `front`: from the front (the `HitBlud` cutscene's case).
    BludgerHit { tick: u32, team: usize, victim: usize, front: bool },
    /// `player` dropped the Quaffle when a Bludger hit him (`0x442110`).
    Fumble { tick: u32, player: usize },
    /// `player`, a beater, batted the opponents' Bludger away (`0x4405c0`).
    BeaterBlock { tick: u32, player: usize },
}

/// A match in progress.
pub struct Match {
    pub tick: u32,
    pub rng: Rand,
    pub players: Vec<Player>,
    pub teams: [Team; 2],
    /// The player holding the Quaffle (a player's `+0x190`).
    pub holder: Option<usize>,
    /// The team whose ball list (`+0xf0`) holds the Quaffle: set when a holder is set, kept while it flies.
    pub ball_team: Option<usize>,
    pub quaffle: Vec3,
    /// Units per second.
    pub quaffle_vel: Vec3,
    pub flight: Option<Flight>,
    /// The hoop a shot is being aimed at this tick (`0x43f070`'s message on `0x1043ab4`; the match's update posts
    /// none first, `0x450d70`): the effects show it targeted.
    pub targeted_hoop: Option<usize>,
    /// The power of the shot being aimed this tick (0 to 1), which the Quaffle's glow shows (**[I]**: the
    /// original's ball glow, `+0x40` of its render record, has its setter untraced).
    pub quaffle_glow: f64,
    /// The hoop table (game `+0x0c`).
    pub hoops: [Hoop; 6],
    /// Whether the hoops are there to see (their render records' byte `+0x4c`, which the camera's hoop list keeps
    /// at `+0x24`, `0x47ef50`): the passing challenge posts its six hidden (`0x459d20`).
    pub hoops_shown: bool,
    /// Each player's contact record "armed" byte (game `+0xb4`, `+3`): a touch counts once until contact ends.
    armed: Vec<bool>,
    /// The Seeker power bars.
    pub power: PowerBar,
    /// The match option byte `0x1018f82` that allows dodges (assumed set).
    pub dodges_allowed: bool,
    /// `CSMgr.GoalCSProbabilityCeleb` and `Crowd` (the goal cutscene roll).
    goal_cs: (f64, f64),
    /// The Formotion blocks of both teams, once [`Match::use_formotions`] gave the databases; and this tick's
    /// one-tick commands for the chasers they move.
    pub formation: Option<[formation::Team; 2]>,
    formation_steps: Vec<Option<formation::Step>>,
    /// This tick's one-tick commands from the AI defenders' approach formotions ([`approach`]).
    approach_steps: Vec<Option<(Vec3, Vec3)>>,
    /// The approaches' kinds (the table `0x655e78`) and `BlockSide`'s starting side (`[0xf91900]`, one for both).
    pub defence_table: approach::Table,
    block_side: f64,
    /// The remaster's momentum for the players flying formotions ([`formation::glide`]): the most their velocity
    /// changes, units a second each second; 0 flies them as the original does.
    pub momentum: f64,
    pub events: Vec<Event>,
    /// Bit 3 of the DataStore value `0x1527016b`, which allows combo chains (off by default).
    pub combo_enabled: bool,
    max_distance: f64,
    lead: f64,
    max_speed: f64,
    pub handling: handling::Params,
    /// The keeper's pass-out lane (`0x10439e0`, a global kept between possessions).
    lane: f64,
    /// The weave's phase (`[0x10439dc]`, a global every carrier of state 0x15 moves on).
    weave_phase: f32,
    collision: (f64, f64),
    /// The pitch's walls ([`crate::walls`]): the defaults, then the stadium script's.
    pub walls: Walls,
    scale_goals: f64,
    /// The stadium's spline for the Seeker game ([`Match::use_spline`]), and the race once the bars filled.
    spline: Option<Spline>,
    /// Each team's seeker's place on the spline before the race (the follower's parameter, team `+0x12b4`).
    seeker_lap: [f64; 2],
    pub race: Option<Race>,
    pub race_rules: RaceRules,
    /// The Snitch was caught: the match is over and [`Match::step`] does nothing.
    pub finished: bool,
    /// The challenge being played (`MatchSubType` 1 to 6), when this is the challenge game (`0x10349b0`).
    pub challenge: Option<u32>,
    /// Tackle 'n' Shoot's flag (`[0x1018fa4]`, set by the challenge's set-up for challenge 2 only), the direction
    /// team 1 attacks while it holds the Quaffle (`[0x1018fa0]`, 1 at the set-up, picked again each second by
    /// [`Match::pick_attack`]) and the ticks of defence before a challenge team's special tackle (`[0x1018fa8]`).
    pub tns: bool,
    pub challenge_dir: f64,
    pub defence_timer: i32,
    /// The heights: the floor, the end-zone clamp and the cruising height ([`crate::pitch`]), and the height graph
    /// the carriers' turns kick along; `HeightGraph.ScaleJoyKick` and `ScaleAIKick`.
    pub pitch: Pitch,
    pub graph: HeightGraph,
    kick_scale: (f64, f64),
    /// The Bludgers, team 0's and team 1's; their console values; the match option `0x1018f85` that allows the
    /// Bludger move (its writer is not traced: assumed set).
    pub bludgers: [Bludger; 2],
    pub bludger_rules: bludger::Rules,
    pub bludgers_allowed: bool,
    /// The Beaters' challenge's aim for team 0's Bludger (`0x1018fb4`: the target nearest it).
    pub challenge_aim: Option<Vec3>,
    /// The Quaffle is in this game (a challenge's description may leave it out, as the Beaters' does).
    pub quaffle_present: bool,
    /// The kick-off at the match's start, from [`Match::start_toss`] until the Quaffle is taken.
    pub kickoff: Option<KickOff>,
}

/// The challenges 2, 5 and 6 put team 0's hoops this many times as far out (`0x5bf1f8`, `0x45a1c7`).
const CHALLENGE_HOOPS_OUT: f64 = 50.0;

/// The challenge team's placements (`+0x64`, `0x45f150`, traced) for team `team` (`s` = -1 for team 0, +1 for team
/// 1): `k` 0 at the start, 1 and 2 after a goal (`(0, 0, -2.5 s)`, `[0x668dcc]`; the challenge's `[0x10439cc]` and
/// `[0x10439d0]` have no writer), 50 towards `-x` in the tackling challenges (`0x45f32d`); a chaser `(-5 s, 0,
/// 6 s (1 - slot))` from there, a beater `(-15 s, 0, 5 s - 10 s slot)`; the keeper at `(-55 s, 0, 3)` and the seeker
/// at `(-60 s, 0, 2)` whatever `k`. `y` is relative to the cruising height.
pub fn challenge_placement(role: Role, slot: usize, k: usize, team: usize) -> Vec3 {
    let s = if team == 0 { -1.0 } else { 1.0 };
    let base = if k == 0 { [-50.0, 0.0, 0.0] } else { [-50.0, 0.0, -2.5 * s] };
    let n = slot as f64;
    match role {
        Role::Keeper => [-55.0 * s, 0.0, 3.0],
        Role::Seeker => [-60.0 * s, 0.0, 2.0],
        Role::Chaser => add(base, [-5.0 * s, 0.0, 6.0 * s * (1.0 - n)]),
        Role::Beater => add(base, [-15.0 * s, 0.0, 5.0 * s - 10.0 * s * n]),
    }
}

/// Tackle 'n' Shoot's defence (`0x455320`, `0x4552d0`, `0x455730`): the goal area's scale for the keeper switch
/// (`0x3f99999a`), the ticks the special tackle's timer gains on each defence and loses on each tackle, the dash's
/// ticks to the hoops (`[0x668e48]`) and how far short of them towards the middle it ends (`0x5bf214`).
const TNS_KEEPER_AREA: f64 = 1.2;
const TNS_TACKLE_WAIT: i32 = 96;
const TNS_TACKLE_SOONER: i32 = 24;
const TNS_KEEPER_TICKS: i32 = 24;
const TNS_KEEPER_IN: f64 = 8.0;

/// Placements of `0x452950` (traced) for team 0, `k` = 0 match start, 1 the kick-off team after a goal, 2 the other
/// team; `y` is relative to the cruising height (the flat floor here). Team 1 mirrors `x` and `z`.
pub fn placement(role: Role, slot: usize, k: usize, team: usize) -> Vec3 {
    let s = slot as f64;
    let base = match role {
        Role::Chaser => [[5.0, 0.0, -6.0], [4.0, 0.0, 0.0], [5.0, 0.0, 6.0]][slot.min(2)],
        Role::Keeper => [91.5, 0.0, 3.0],
        Role::Beater => [15.0, 20.0, -5.0 + 10.0 * s],
        Role::Seeker => [60.0, 0.0, 2.0],
    };
    let mut p = base;
    if k > 0 {
        // after a goal chasers line up at x = 70 (kick-off team) or -28, beaters move 65 towards the own end or 33
        // towards the attacked end; both shift 2.5 across. Keeper and seeker stay.
        match role {
            Role::Chaser => p[0] = if k == 1 { 70.0 } else { -28.0 },
            Role::Beater => p[0] += if k == 1 { 65.0 } else { -33.0 },
            Role::Keeper | Role::Seeker => {}
        }
        if matches!(role, Role::Chaser | Role::Beater) {
            p[2] += 2.5;
        }
    }
    if team == 1 {
        p[0] = -p[0];
        p[2] = -p[2];
    }
    p
}

/// `Formotions.TweakTime STATE MULT` as team slot `team`'s files last called it for `state` (`0x4390c0` writes the
/// loading team's block), else 1.
fn tweak_time(c: &Console, team: usize, state: &str) -> f64 {
    c.team_calls(team)
        .filter(|(p, _)| *p == "Formotions.TweakTime")
        .filter_map(|(_, a)| match (a.first(), a.get(1).and_then(Value::num)) {
            (Some(Value::Text(n)), Some(v)) if n.eq_ignore_ascii_case(state) => Some(v),
            _ => None,
        })
        .last()
        .unwrap_or(1.0)
}

/// A team command's last numeric argument, or `default` when the team's `.cli` files never set it.
fn team_value(c: &Console, path: &str, team: usize, default: f64) -> f64 {
    c.args(path, team).iter().rev().find_map(|v| v.num()).unwrap_or(default)
}

impl Match {
    /// Kick off with the console's values (load the team files into slots 0 and 1 first) and the `rand()` state.
    pub fn new(console: &Console, rng: Rand) -> Match {
        let pitch = Pitch::new(console, Vec::new());
        let graph = HeightGraph::new(console);
        let mut players = Vec::new();
        let mut team = |index: usize| {
            let mut add = |role: Role, slot: usize| {
                // at the cruising height (`0x445090`)
                let mut pos = placement(role, slot, 0, index);
                pos[1] += pitch.cruise(&graph, pos);
                players.push(Player {
                    team: index,
                    role,
                    pos,
                    vel: [0.0; 3],
                    fwd: [if index == 0 { -1.0 } else { 1.0 }, 0.0, 0.0],
                    prev_vel: [0.0; 3],
                    cmd: None,
                    queue: Vec::new(),
                    present: true,
                    knocked: 0,
                });
                players.len() - 1
            };
            let keeper = add(Role::Keeper, 0);
            let chasers = [add(Role::Chaser, 0), add(Role::Chaser, 1), add(Role::Chaser, 2)];
            let beaters = [add(Role::Beater, 0), add(Role::Beater, 1)];
            let seeker = add(Role::Seeker, 0);
            Team {
                index,
                keeper,
                chasers,
                beaters,
                seeker,
                ai: PassAi::from_console(console, index),
                plan: PassPlan::default(),
                // constructor defaults of `0x433a70` where the files are silent
                pass_cone: team_value(console, "Passing.PassHelp.PassCone", index, 60.0),
                other_team_catch_mod: team_value(console, "Passing.PassHelp.OtherTeamCatchMod", index, 3.0),
                this_team_catch_mod: team_value(console, "Passing.PassHelp.ThisTeamCatchMod", index, 1.0),
                chaser_speed: team_value(console, "Chaser.Speed", index, 1.0),
                keeper_speed: team_value(console, "Keeper.Speed", index, 5.0),
                shooting_skill: team_value(console, "AI.ShootingSkill", index, 0.5) as f32,
                keeper_mode: team_value(console, "Keeper.Mode", index, 2.0) as i32,
                combo_chain: 0,
                pass_increment: team_value(console, "SeekerPowerBar.PassIncrement", index, 0.012),
                combo_bar_multiplier: team_value(console, "SeekerPowerBar.ComboBarMultiplier", index, 1.0),
                lock: (0, None),
                receiver: None,
                target: [0.0; 3],
                flight: 0,
                in_cone: false,
                pass_target: None,
                state: None,
                shot: ShotPlan::default(),
                settle: Settle::default(),
                keeper_cmd: None,
                keeper_run: None,
                guard_cmd: None,
                dir_override: None,
                tackle: TeamTackle::default(),
                lunge_cone: team_value(console, "Tackle.LungeConeAngle", index, 90.0),
                active: None,
                press: 0,
                evade: Evade::default(),
                evade_probs: evade::Probs {
                    spin: console.num("Evading.SpinProb", index),
                    weave: console.num("Evading.WeaveProb", index),
                    dodge: console.num("Evading.DodgeProb", index),
                    roll: console.num("Evading.DodgeRollProb", index),
                    loop_: console.num("Evading.DodgeLoopProb", index),
                },
                call: None,
                score: 0,
                human: false,
                input: Input::default(),
                human_state: None,
                boost_hold: 0,
                bludger: None,
                beater_modes: [0; 2],
                beater_runners: Default::default(),
                block_count: 0,
                block_point: ([0.0; 3], [0.0; 3]),
                hit: None,
                defence: None,
                bludger_hits: 0,
                special_moves: 0,
                uber_penalty: team_value(console, "SeekerPowerBar.UberPenalty", index, 0.0),
                special_shoot_weight: console.num("TeamOPs.SpecialMoveShootWeight", index),
                uber_weight: console.num("TeamOPs.UberMoveWeight", index),
                c1: 0,
                special_tackle: None,
                approach: Approach::default(),
                history: History::default(),
                tweak_ai_def: tweak_time(console, index, "AI_DEF"),
            }
        };
        let teams = [team(0), team(1)];
        let holder = teams[0].chasers[1];
        let quaffle = players[holder].pos;
        let armed = vec![true; players.len()];
        // `Pitch.ScaleGoals` and `Pitch.GoalHeight` are commands; the pitch starts at 1 and 20
        let scale_goals = pitch.scale_goals;
        let goal_height = pitch.goal_height;
        Match {
            tick: 0,
            rng,
            players,
            teams,
            holder: Some(holder),
            ball_team: Some(0),
            quaffle,
            quaffle_vel: [0.0; 3],
            flight: None,
            targeted_hoop: None,
            quaffle_glow: 0.0,
            hoops: shooting::hoops(scale_goals, goal_height),
            hoops_shown: true,
            formation_steps: vec![None; armed.len()],
            approach_steps: vec![None; armed.len()],
            defence_table: approach::Table::from_console(console),
            block_side: 0.0,
            momentum: 0.0,
            power: PowerBar {
                pots: [0, 1].map(|t| {
                    ["DodgePot", "BludgerPot", "SpecialMovePot", "UberMovePot"]
                        .map(|k| team_value(console, &format!("SeekerPowerBar.{k}"), t, 0.0))
                }),
                ..PowerBar::default()
            },
            dodges_allowed: true,
            goal_cs: (console.num("CSMgr.GoalCSProbabilityCeleb", 0), console.num("CSMgr.GoalCSProbabilityCrowd", 0)),
            armed,
            formation: None,
            events: Vec::new(),
            combo_enabled: false,
            max_distance: console.num("Passing.PassHelp.MaxDistance", 0),
            lead: console.num("Passing.PassHelp.Lead", 0),
            max_speed: console.num("Handling.MaxSpeed", 0),
            handling: handling::Params {
                acceleration: console.num("Handling.Acceleration", 0),
                max_speed: console.num("Handling.MaxSpeed", 0),
                blend_heading_joy: console.num("Handling.BlendHeadingJoy", 0),
                blend_heading_vel: console.num("Handling.BlendHeadingVel", 0),
            },
            lane: 0.0,
            weave_phase: 0.0,
            collision: (225.0, 85.0),
            walls: Walls::from_console(console, 225.0, 85.0),
            scale_goals,
            spline: None,
            seeker_lap: [0.0; 2],
            race: None,
            race_rules: RaceRules::from_console(console),
            finished: false,
            challenge: None,
            tns: false,
            challenge_dir: 1.0,
            defence_timer: 0,
            pitch,
            graph,
            kick_scale: (console.num("HeightGraph.ScaleJoyKick", 0), console.num("HeightGraph.ScaleAIKick", 0)),
            // **[I]** the Bludgers start over the middle, either side of the idle point
            bludgers: [-1.0, 1.0].map(|s| Bludger {
                pos: add(bludger::IDLE_POINT, [0.0, 0.0, 10.0 * s]),
                present: true,
                ..Bludger::default()
            }),
            bludger_rules: bludger::Rules::from_console(console),
            bludgers_allowed: true,
            challenge_aim: None,
            quaffle_present: true,
            kickoff: None,
        }
    }

    /// The match's start (the game's type-3 event, `0x4519d0`): the Quaffle waits over the centre and the kick-off
    /// is raced for. Without it (as in the tests) team 0's second chaser starts with the ball. Not in a challenge,
    /// which has its own start.
    pub fn start_toss(&mut self) {
        if self.challenge.is_some() || !self.quaffle_present {
            return;
        }
        self.holder = None;
        self.ball_team = None;
        let centre = [0.0, self.pitch.floor([0.0; 3]) + TOSS_HEIGHT, 0.0];
        self.quaffle = centre;
        self.quaffle_vel = [0.0; 3];
        self.flight = Some(Flight {
            flat: false,
            kind: FlightKind::Toss { player: None },
            start_tick: self.tick,
            end_tick: u32::MAX,
            from: centre,
            to: centre,
        });
        self.kickoff = Some(KickOff {
            start: self.tick,
            toss: self.tick + KICK_TOSS,
            kick: None,
            engaged: [false; 2],
            pushed: [false; 2],
            window: false,
        });
        // **[I]** each team's centre chaser (placement slot 1) is its active player
        for t in 0..2 {
            let c = self.teams[t].chasers[1];
            self.set_active(t, c);
        }
    }

    /// The kick-off's clock (`0x432ef0`, from the common update): once, with the clock in the window (50 to 59), the
    /// window's message ([`Event::KickOffWindow`]).
    fn kickoff_clock(&mut self) {
        let Some(ko) = self.kickoff.as_mut() else { return };
        let c = self.tick - ko.start;
        if (KICK_WINDOW..KICK_AI).contains(&c) && !ko.window {
            ko.window = true;
            self.events.push(Event::KickOffWindow { tick: self.tick });
        }
    }

    /// Team `t`'s part of the kick-off (state 0x0e `0x453270`, the human's state 2 `0x446430`): the claim, then
    /// once the kicking team is known the commands of its player (`0x43fce0`) or of the other's (`0x43fe40`). Until
    /// then nothing moves the team's active player.
    fn kickoff_tick(&mut self, t: usize) {
        let Some(mut ko) = self.kickoff else { return };
        let Some(a) = self.teams[t].active else { return };
        if ko.engaged[t] {
            return;
        }
        let c = self.tick - ko.start;
        if self.teams[t].human {
            // Action pressed, or the stick pushed past half along the player's facing on its first tick
            let input = self.teams[t].input;
            let push = len(input.stick) > 0.5 && dot(self.players[a].fwd, input.stick) > 0.0;
            let claim = input.action_pressed || (push && !ko.pushed[t]);
            ko.pushed[t] = push;
            if ko.kick.is_none() && claim && c >= KICK_CLAIM {
                // in the window it is won; too early it goes to the other side, as does a late claim against the AI
                let won = (KICK_WINDOW..KICK_AI).contains(&c) || (c >= KICK_AI && self.teams[1 - t].human);
                ko.kick = Some(if won { t } else { 1 - t });
            }
        } else if ko.kick.is_none() && c >= KICK_AI {
            ko.kick = Some(t);
        }
        match ko.kick {
            Some(k) if k == t => {
                self.kickoff_take(a, &ko);
                ko.engaged[t] = true;
                self.events.push(Event::Toss { tick: self.tick, team: t, player: a });
            }
            Some(_) => {
                self.kickoff_close(t, a, &ko, c);
                ko.engaged[t] = true;
            }
            None => {}
        }
        self.kickoff = Some(ko);
    }

    /// `0x43fce0`: the kicking team's player flies to the centre, arriving along his attack direction at 5 a
    /// second (`0x421f10`, `0x5bf0a0`) at the toss or 15 ticks on, whichever is later, and takes the Quaffle there.
    fn kickoff_take(&mut self, a: usize, ko: &KickOff) {
        let end = ko.toss.max(self.tick + 15);
        let dir = self.teams[self.players[a].team].attack_dir();
        let centre = [0.0, self.pitch.floor([0.0; 3]), 0.0];
        self.command(a, centre, [dir * 5.0, 0.0, 0.0], (end - self.tick) as i32);
        self.players[a].queue.clear();
        if let Some(fl) = self.flight.as_mut() {
            fl.kind = FlightKind::Toss { player: Some(a) };
            fl.end_tick = end;
        }
    }

    /// `0x43fe40`: the other team's player. Called before the window (the other side's false start), he flies to 2
    /// over the kicking player by the toss and then to his side over 12 ticks; later, under the centre (2 below the
    /// floor) by the toss + 7 or 20 ticks on, whichever is later, then 3 towards the end he attacks over 12 ticks.
    fn kickoff_close(&mut self, t: usize, a: usize, ko: &KickOff, c: u32) {
        let dir = self.teams[t].attack_dir();
        let floor = self.pitch.floor([0.0; 3]);
        let (first, first_end, second) = if c < KICK_WINDOW {
            let kp = ko.kick.and_then(|k| self.teams[k].active).map_or([0.0; 3], |k| self.players[k].pos);
            (add(kp, [0.0, 2.0, 0.0]), ko.toss, ([kp[0], floor, kp[2] + 1.0], [-dir * 3.0, 0.0, 0.0]))
        } else {
            let end = (ko.toss + 7).max(self.tick + 20);
            ([0.0, floor - 2.0, 0.0], end, ([dir * 3.0, floor, 1.0], [0.0; 3]))
        };
        self.command(a, first, [dir * 8.0, 0.0, 0.0], (first_end - self.tick) as i32);
        let (to, to_vel) = second;
        let p = &mut self.players[a];
        p.queue = vec![Command { start: first_end, end: first_end + 12, from: first, from_vel: [0.0; 3], to, to_vel }];
    }

    /// The game a match description sets up (`match/<name>/matchdesc.txt`): for a challenge, only its players
    /// (the first of each role and team, in the line-up's order) are present, the others are parked under the
    /// pitch; the challenge's set-up gives the Quaffle to team 0's first chaser in the passing and combo challenges
    /// (`0x45ac60`).
    pub fn use_description(&mut self, desc: &qwc_formats::matchdesc::MatchDesc) {
        if !desc.is_challenge() {
            return;
        }
        self.challenge = desc.sub_type;
        for p in &mut self.players {
            p.present = false;
        }
        for t in 0..2 {
            let tm = &self.teams[t];
            let mut slots: Vec<(Role, Vec<usize>)> = vec![
                (Role::Keeper, vec![tm.keeper]),
                (Role::Chaser, tm.chasers.to_vec()),
                (Role::Beater, tm.beaters.to_vec()),
                (Role::Seeker, vec![tm.seeker]),
            ];
            for d in desc.players.iter().filter(|d| d.team as usize == t) {
                let role = match d.kind.as_str() {
                    "eKeeper" => Role::Keeper,
                    "eBeater" => Role::Beater,
                    "eSeeker" => Role::Seeker,
                    _ => Role::Chaser,
                };
                if let Some((_, free)) = slots.iter_mut().find(|(r, _)| *r == role) {
                    if !free.is_empty() {
                        let p = free.remove(0);
                        self.players[p].present = true;
                    }
                }
            }
        }
        for p in self.players.iter_mut().filter(|p| !p.present) {
            p.pos = [0.0, -1000.0, 0.0];
            p.vel = [0.0; 3];
            p.cmd = None;
        }
        self.holder = None;
        self.ball_team = None;
        self.flight = None;
        // the description's balls: the Quaffle, the Bludgers (`subtypeid` 0 is team 0's)
        self.quaffle_present = desc.balls.iter().any(|b| b == "eQuaffle");
        // the Beaters' challenge flies the long pitch without the end-zone clamp (`0x41a440`, `0x41a2a0`)
        self.pitch.mode4 = desc.sub_type == Some(4);
        let n = desc.balls.iter().filter(|b| *b == "eBludger").count();
        for (i, b) in self.bludgers.iter_mut().enumerate() {
            b.present = i < n;
        }
        // the challenge's hoops (`0x459d20`): the passing challenge puts all six under the pitch, at (0, -50, 0),
        // and hidden; 2, 5 and 6 (`0x45a159`) keep team 1's at a match's end and put team 0's 50 times as far
        // out (`0x45a1c7`), shown but out of reach: only team 0 can score
        if desc.sub_type == Some(1) {
            for h in &mut self.hoops {
                h.pos = [0.0, -50.0, 0.0];
            }
            self.hoops_shown = false;
        }
        if matches!(desc.sub_type, Some(2 | 5 | 6)) {
            for h in self.hoops.iter_mut().filter(|h| h.team == 0) {
                h.pos[0] *= CHALLENGE_HOOPS_OUT;
            }
        }
        // Tackle 'n' Shoot's set-up (`0x459812`, `0x459ae0`): its flag, team 1's direction `+x`, the defence's
        // first level
        self.tns = desc.sub_type == Some(2);
        self.challenge_dir = 1.0;
        self.defence_timer = 0;
        if self.tns {
            self.tns_level(0);
        }
        if matches!(desc.sub_type, Some(1 | 6)) && desc.balls.iter().any(|b| b == "eQuaffle") {
            let c = self.teams[0].chasers[0];
            self.set_holder(c);
        } else if matches!(desc.sub_type, Some(2 | 5)) {
            // the reset (`0x45a8c8`) gives the Quaffle to team 1's first chaser (Edgar)
            self.challenge_kick_off(desc.sub_type.unwrap_or(2), 0);
        }
    }

    /// Whether team `t` has anyone in this game.
    fn fields(&self, t: usize) -> bool {
        self.players.iter().any(|p| p.team == t && p.present)
    }

    /// The stadium's height maps (`heightmap.csv`, [`pitch::Map`]); the players are placed again at the cruising
    /// height over them. Without them the maps are flat (a height of 1, so the floor is the goal height).
    pub fn use_height_maps(&mut self, maps: Vec<pitch::Map>) {
        if maps.is_empty() {
            return;
        }
        self.pitch.map = self.pitch.map.min(maps.len() - 1);
        self.pitch.maps = maps;
        for t in 0..2 {
            self.place_team(t, 0);
        }
    }

    /// Team `t`'s players at placement `k` (`0x445090`): at once, at the cruising height, still.
    fn place_team(&mut self, t: usize, k: usize) {
        let tm = &self.teams[t];
        let mut slots = vec![(tm.keeper, 0)];
        slots.extend(tm.chasers.iter().enumerate().map(|(s, &p)| (p, s)));
        slots.extend(tm.beaters.iter().enumerate().map(|(s, &p)| (p, s)));
        slots.push((tm.seeker, 0));
        // the tackling challenges' own placements (the challenge team's `+0x64`, `0x45f150`), facing along `s`;
        // **[I]** the other challenges keep the match's
        let own = matches!(self.challenge, Some(2 | 5));
        for (p, s) in slots {
            if !self.players[p].present {
                continue;
            }
            let role = self.players[p].role;
            let mut pos = if own { challenge_placement(role, s, k, t) } else { placement(role, s, k, t) };
            pos[1] += self.pitch.cruise(&self.graph, pos);
            let pl = &mut self.players[p];
            pl.pos = pos;
            pl.vel = [0.0; 3];
            pl.prev_vel = [0.0; 3];
            pl.cmd = None;
            if own {
                pl.fwd = [if t == 0 { -1.0 } else { 1.0 }, 0.0, 0.0];
            }
        }
    }

    /// The stadium's spline (`stadium/<name>/gfx/*.qsp`) that the Seeker game races round. Without it the Seeker
    /// game starts (the shot routine stops) but there is no race, and the seekers wait at their places.
    /// **[T]** The team set-up (`0x452580`) puts each seeker's follower on it (`SeekerStadiumLoop`, `0x491470`), team
    /// 0's at half its points and team 1's at 0, so they lap it on opposite sides until the race.
    pub fn use_spline(&mut self, spline: Spline) {
        self.seeker_lap = [(spline.len() / 2) as f64, 0.0];
        self.spline = Some(spline);
    }

    /// Team `t`'s seeker before the race (`0x44b2c0`, the end of the team's update when the Seeker game is off): his
    /// follower moves on a whole point a tick (step `+0x12b8` 1, `0x491b60`), and he is put on its frame, facing
    /// the point 2 ahead (flag 4: the follower's lookahead `+0x14`), upright.
    fn seeker_lap_tick(&mut self, t: usize) {
        let Some(s) = &self.spline else { return };
        if s.is_empty() {
            return;
        }
        let k = s.normalise(self.seeker_lap[t] + 1.0);
        self.seeker_lap[t] = k;
        let (pos, ahead, back) = (s.position(k), s.position(k + 2.0), s.position(k - 1.0));
        let p = &mut self.players[self.teams[t].seeker];
        if !p.present {
            return;
        }
        p.pos = pos;
        // **[I]** the command keeps the mover's own velocity; the port gives him the lap's, for his animation
        p.vel = scale(sub(pos, back), TICKS);
        let f = sub(ahead, pos);
        if len(f) > 1e-9 {
            p.fwd = unit(f);
        }
    }

    /// Moves the off-ball chasers with the Formotions: `global` is the match database (`match/quidd/formotionDB.txt`
    /// in a normal match), `teams` the teams' own (`teams/<country>/formotionDB.txt`). Without this, every chaser
    /// uses the placeholder movement.
    pub fn use_formotions(&mut self, global: &Database, teams: [Option<&Database>; 2]) {
        let g = Rc::new(Db::new(global));
        let block = |t: usize| formation::Team::new(Rc::clone(&g), teams[t].map(|d| Rc::new(Db::new(d))), t);
        self.formation = Some([block(0), block(1)]);
    }

    /// The team holding the Quaffle.
    pub fn possession(&self) -> Option<usize> {
        self.holder.map(|p| self.players[p].team)
    }

    /// A digest of everything that changes as the match plays: the tick, the random state, the players, the teams
    /// (their AI and the humans' input included), the balls, the bars, the formation blocks' slots and the race. Two
    /// machines playing online compare it tick by tick (`qwc-net`): equal states give equal sums, and a difference
    /// anywhere shows on the tick it appears. The fixed data (the console's values, the Formotions, the height maps,
    /// the spline) is left out; a difference there shows in what it moves. FNV-1a over the fields' `Debug` text,
    /// which prints every float in its shortest exact form, the same on every platform.
    pub fn checksum(&self) -> u64 {
        use std::fmt::Write;
        struct Fnv(u64);
        impl Write for Fnv {
            fn write_str(&mut self, s: &str) -> std::fmt::Result {
                for b in s.bytes() {
                    self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
                }
                Ok(())
            }
        }
        let mut h = Fnv(0xcbf2_9ce4_8422_2325);
        let _ = write!(
            h,
            "{} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?}",
            self.tick,
            self.rng,
            self.players,
            self.teams,
            (self.holder, self.ball_team, self.quaffle, self.quaffle_vel, self.flight, self.finished),
            (&self.armed, self.lane, self.weave_phase, self.block_side, self.seeker_lap),
            self.bludgers,
            self.power,
            self.kickoff,
            (self.challenge_dir, self.defence_timer, self.challenge_aim),
            (self.graph.phase, self.graph.vel),
            self.formation.as_ref().map(|f| f.each_ref().map(|t| (t.current, t.requested, t.leader, &t.slots))),
            self.race.as_ref().map(|r| (&r.snitch, &r.seekers, r.ticks, r.ai_ticks, &r.brain, r.caught)),
        );
        h.0
    }

    /// One 24 Hz tick in the order of the world update (`0x419510` and the match update `0x450d70`).
    pub fn step(&mut self) {
        if self.finished {
            return;
        }
        self.targeted_hoop = None;
        self.quaffle_glow = 0.0;
        // the pre-update's contact test (`0x431470`)
        self.contacts();
        // the team whose ball list holds the Quaffle updates first (`0x430970`); the Snitch moves before the teams
        // (`0x44f990`), each team's update ends with its seeker (`0x44da50`), and the bump test follows (`0x449000`)
        let first = self.ball_team.unwrap_or(0);
        self.kickoff_clock();
        if let Some(r) = self.race.as_mut() {
            r.snitch_tick(&self.race_rules);
        }
        for t in [first, 1 - first] {
            self.update_team(t);
            if let Some(r) = self.race.as_mut() {
                r.seeker_tick(t, &mut self.rng, &self.race_rules);
            } else {
                self.seeker_lap_tick(t);
            }
        }
        if let Some(r) = self.race.as_mut() {
            r.collide();
        }
        // the common update (`0x430ae0`): the height graph moves on (`0x420840`)
        self.graph.tick();
        self.update_formations(first);
        // the Bludgers' formotions end the common update (`0x430d90`)
        self.update_bludgers();
        self.move_players();
        self.move_bludgers();
        self.update_race();
        self.update_quaffle();
        self.tick += 1;
    }

    /// The race's events and the seekers' positions; a catch scores 150 and ends the match (`0x4489f0`).
    fn update_race(&mut self) {
        let Some(r) = self.race.as_mut() else { return };
        for event in r.events.drain(..) {
            self.events.push(Event::Race { tick: self.tick, event });
        }
        for t in 0..2 {
            let p = self.teams[t].seeker;
            self.players[p].pos = r.seekers[t].pos;
            self.players[p].vel = r.seekers[t].vel;
            self.players[p].fwd = r.seekers[t].heading;
        }
        if let Some(t) = r.caught {
            self.teams[t].score += CATCH_POINTS;
            self.finished = true;
        }
    }

    /// The Quaffle's state (`+0x1c`) as the pre-update of this tick sees it: the events of the previous ticks have
    /// fired, those of this tick have not (they fire in the movers' step).
    pub fn quaffle_state(&self) -> u8 {
        if self.holder.is_some() {
            return 3;
        }
        let Some(fl) = self.flight else { return 0 };
        let done = self.tick.saturating_sub(1);
        match fl.kind {
            FlightKind::Pass { .. } if done < fl.start_tick + 2 => 5,
            FlightKind::Pass { .. } if done + 2 < fl.end_tick => 6,
            FlightKind::Pass { .. } if done + 1 < fl.end_tick => 7,
            FlightKind::Pass { .. } => 3,
            FlightKind::Shot { .. } if done < fl.start_tick + 1 => 8,
            FlightKind::Shot { .. } if done + 2 < fl.end_tick => 9,
            // inferred: neither the catch's own state nor the drop's is traced; neither can be caught
            FlightKind::Shot { .. } | FlightKind::Drop { .. } => 10,
            FlightKind::Take { .. } | FlightKind::Tackle { .. } | FlightKind::Toss { .. } => 12,
            FlightKind::Loose { .. } => 0,
        }
    }

    /// The contact test (`0x45ff90` for every player, team 0's first) and the catch handler (`0x431510`): a chaser or
    /// keeper not on the team whose ball list holds the Quaffle takes a loose ball, a pass or a shot he touches.
    /// The state is read once; if two players touch in one tick the later one wins (inferred).
    fn contacts(&mut self) {
        if !self.quaffle_present {
            return;
        }
        let state = self.quaffle_state();
        let r = shooting::contact_radius(state);
        for i in 0..self.players.len() {
            if !self.players[i].present {
                continue;
            }
            let d = shooting::distance(self.players[i].pos, self.quaffle);
            if self.holder.is_some() || d >= r {
                self.armed[i] = true;
                continue;
            }
            if !std::mem::replace(&mut self.armed[i], false) {
                continue;
            }
            let p = &self.players[i];
            if !matches!(p.role, Role::Chaser | Role::Keeper)
                || !shooting::catchable(state)
                || self.ball_team == Some(p.team)
            {
                continue;
            }
            let end = self.tick + shooting::catch_ticks(d) as u32;
            self.flight = Some(Flight {
                flat: false,
                kind: FlightKind::Take { player: i },
                start_tick: self.tick,
                end_tick: end,
                from: self.quaffle,
                to: p.pos,
            });
            self.events.push(Event::Touch { tick: self.tick, player: i, state });
        }
    }

    fn update_team(&mut self, t: usize) {
        self.teams[t].call = None;
        if !self.fields(t) {
            return;
        }
        // the opposing active player into the team's history (`0x435270` -> `0x4334e0`)
        if let Some(o) = self.teams[1 - t].active {
            let (p, v) = (self.players[o].pos, self.players[o].vel);
            self.teams[t].history.push(p, v);
        }
        // a Bludger hit on one of the team's players (`0x442110`, early in the base team update)
        self.take_hit(t);
        // a dodge lasts until its team's next update; then the cooldown runs
        let tk = &mut self.teams[t].tackle;
        tk.dodging = false;
        tk.dodge_cool = (tk.dodge_cool - 1).max(0);
        // **[I]** in the Seeker game the player flies the seeker and the AI takes the rest of the team
        if self.kickoff.is_some() && self.ball_team.is_none() {
            // the Quaffle in neither team's list: the brains' kick-off states
            self.kickoff_tick(t);
        } else if self.teams[t].human && self.race.is_none() {
            self.update_human(t);
        } else {
            self.update_carrier(t);
            self.update_defender(t);
        }
        self.update_keeper(t);
        // the beaters, after the keeper (`0x452816` -> `0x441090`)
        self.update_beaters(t);
    }

    /// Hands team `t` to a human (or back to the AI): its update runs the human brain on the input of
    /// [`Match::set_input`], and in the Seeker game its seeker flies on [`Match::set_seeker_pad`].
    pub fn set_human(&mut self, t: usize, human: bool) {
        self.teams[t].human = human;
        self.teams[t].human_state = None;
        self.teams[t].state = None;
    }

    /// Team `t`'s seeker's controls this tick, when the Seeker game is on and the team is a human's.
    pub fn set_seeker_pad(&mut self, t: usize, pad: crate::race::SeekerPad) {
        if let Some(r) = self.race.as_mut() {
            r.seekers[t].pad = pad;
        }
    }

    /// This tick's input for team `t` (`0x442690`, [`human::Input::from_pad`]); the Boost hold count follows it.
    pub fn set_input(&mut self, t: usize, input: Input) {
        let team = &mut self.teams[t];
        team.boost_hold = if input.boost_held {
            team.boost_hold + 1
        } else if team.boost_hold > 0 {
            -team.boost_hold
        } else {
            0
        };
        team.input = input;
    }

    /// Makes `p` team `t`'s active player; a new one resets the target lock, the accumulator and the lunge
    /// (`0x4352fb`).
    fn set_active(&mut self, t: usize, p: usize) {
        if self.teams[t].active != Some(p) {
            let team = &mut self.teams[t];
            team.active = Some(p);
            team.press = 0;
            team.tackle.acc = [0.0; 3];
            team.tackle.lunge = 0.0;
        }
    }

    /// The human brain (`0x4463c0`): with the Quaffle the attack state (3), the carrier active; without it the
    /// defence (4) or the loose ball (2), each flying the active player at the ball on the stick (the general mover
    /// `0x436490(1, input)`, states 9 and 2's fallback) with Action as the tackle, on a rope of 5 to the ball. The
    /// active player without the ball is chosen as the AI's is ([`Match::off_ball_active`]); the defence's
    /// special branches (Special `0x438920`, Boost's Bludger `0x442a20`) and the loose ball's race for who goes are
    /// not ported.
    fn update_human(&mut self, t: usize) {
        let input = self.teams[t].input;
        self.teams[t].state = None;
        // word 22's press: the uber move (`0x442690` -> `0x43ed10`)
        if input.uber_pressed && self.uber_move(t) {
            return;
        }
        // the defence: the Quaffle in the opponents' ball list, held or passed (their team state); the Beaters'
        // challenge, which has no Quaffle, plays as one
        let defending = self.ball_team == Some(1 - t) || self.challenge == Some(4);
        if !defending {
            // **[I]** leaving the defence ends a Bludger move (the sub-state's exit, `0x4474e5`)
            self.end_bludger(t);
        }
        if let Some(c) = self.holder.filter(|&h| self.players[h].team == t) {
            let state = match self.teams[t].human_state {
                Some(s @ human::State::Attack { .. }) => s,
                // state 3's enter: carrying, a throw allowed
                _ => human::State::Attack { sub: AttackSub::Carry, armed: true },
            };
            self.set_active(t, c);
            let next = self.human_attack(t, c, state, &input);
            self.teams[t].human_state = Some(next);
            return;
        }
        let mut sub = match self.teams[t].human_state {
            Some(human::State::Defend { sub }) if defending => sub,
            _ => DefendSub::Chase,
        };
        // the defence's sub-states (`0x446af0`): 12 waits for the launch, 11 ends on a button or with the move
        match sub {
            DefendSub::Summon if self.teams[t].bludger.is_none() => sub = DefendSub::Chase,
            DefendSub::Summon if self.teams[t].bludger.is_some_and(|m| m.flying) => sub = DefendSub::Steer,
            DefendSub::Steer => {
                // Boost, Action, the Special and (**[I]**) Dodge: context `+0x49`, `+0x44`, `+0x4f`, `+0x4d`
                let quit = input.boost_pressed || input.action_pressed || input.special_pressed || input.dodge_pressed;
                if self.teams[t].bludger.is_none() || quit {
                    self.end_bludger(t);
                    sub = DefendSub::Chase;
                }
            }
            _ => {}
        }
        // the Special starts the defensive special when it is available (`0x446b23`: `0x438920`, then sub-state 10)
        if sub == DefendSub::Chase && defending && input.special_pressed && self.special_tackle_available(t) {
            self.teams[t].special_tackle = Some(SpecialRun::new(false));
        }
        // 9: Boost summons the Bludger when the move is available (`0x442a20`; in a challenge with the option, at once)
        let available =
            (self.challenge.is_some() && self.bludgers_allowed && self.bludger_charged(t)) || self.bludger_available(t);
        if sub == DefendSub::Chase && defending && input.boost_pressed && available && self.bludgers[t].present {
            self.start_bludger(t, true);
            sub = DefendSub::Summon;
        }
        self.teams[t].human_state = Some(if defending { human::State::Defend { sub } } else { human::State::Loose });
        let Some(active) = self.off_ball_active(t) else { return };
        self.set_active(t, active);
        if self.teams[t].bludger.is_some() {
            self.run_bludger(t);
            return;
        }
        if self.run_special_tackle(t) {
            return;
        }
        let mut call = self.chase_call(active, input.stick);
        call.ai = false;
        self.teams[t].call = Some(call);
        self.handle(t, active, input.stick, input.action_pressed);
    }

    /// The attack state (`0x446600`) for carrier `c`: its sub-states, carrying (5) and the throw (7).
    fn human_attack(&mut self, t: usize, c: usize, state: human::State, input: &Input) -> human::State {
        let human::State::Attack { sub: mut step, mut armed } = state else { return state };
        let l2 = len(input.stick2);
        // the second stick arms a throw while Action is not held
        if l2 > 0.5 && !input.action_held {
            armed = true;
        }
        let cp = self.players[c].pos;
        let in_area = shooting::in_goal_area(cp, 1 - t, self.scale_goals, 1.0);
        if let AttackSub::Throw(th) = step {
            if th.done {
                step = AttackSub::Carry;
            }
        }
        // sub-state 5 with the special (Shoot while Boost is held, context `+0x48`): the special move when it may
        // (`0x4467b5`), and no throw this tick either way
        if step == AttackSub::Carry && input.special {
            self.special_move(t, c);
            return human::State::Attack { sub: step, armed };
        }
        if step == AttackSub::Carry && armed {
            // `0x4429f0`: passing is allowed (the option `0x1018f80`, assumed on)
            let pass = input.action_pressed;
            if l2 > 0.5 || pass || (input.shoot_held && in_area) {
                armed = false;
                // state 7's enter (`0x446de0`): a shot when Shoot is held in the goal area
                let shot = input.shoot_held && in_area;
                step = AttackSub::Throw(human::Throw { shot, began_as_shot: shot, ..human::Throw::default() });
            }
        }
        if let (AttackSub::Carry, Some(kind), true) = (step, input.dodge(), self.can_dodge(t)) {
            self.start_dodge(t, c, kind);
        }
        match step {
            AttackSub::Carry => {
                // state 5: the carrier wrapper on the input (`0x4362b0`, a human's kick unscaled by the AI's)
                let chaser = self.players[c].role != Role::Keeper;
                let mut call = self.carrier_call_by(c, input.stick, chaser, false);
                call.ai = false;
                self.teams[t].call = Some(call);
                self.handle(t, c, input.stick, false);
            }
            AttackSub::Throw(mut th) => {
                th.combo_buttons(input, self.teams[t].combo_chain);
                let mut i = *input;
                if !th.began_as_shot {
                    i.shoot_pressed = false;
                    i.shoot_held = false;
                } else {
                    i.action_pressed = false;
                    i.action_held = false;
                    i.stick2 = [0.0; 3];
                }
                let theirs: Vec<(usize, Vec3)> =
                    self.hoops.iter().enumerate().filter(|(_, h)| h.team == 1 - t).map(|(i, h)| (i, h.pos)).collect();
                let fwd = self.players[c].fwd;
                // `0x4419e0` **[I]**: the second stick points within 60 degrees of an opposing hoop and the carrier
                // is in the goal area (the range by power is not traced)
                let dir = if len(i.stick2) >= 0.5 { i.stick2 } else { fwd };
                let aims_at_hoop = in_area
                    && shooting::nearest_bearing(cp, unit(dir), &theirs).is_some_and(|h| {
                        let to = sub(self.hoops[h].pos, cp);
                        let flat = unit([to[0], 0.0, to[2]]);
                        let d = unit([dir[0], 0.0, dir[2]]);
                        flat[0] * d[0] + flat[2] * d[2] >= cos(std::f64::consts::FRAC_PI_3)
                    });
                let first = !th.started;
                match th.step(&i, fwd, aims_at_hoop) {
                    ThrowStep::AimShot { dir, f } => {
                        if first {
                            self.wind_up_start(t, c);
                        }
                        // `0x43f070`: the opposing hoop nearest in bearing, the power
                        let shot = &mut self.teams[t].shot;
                        shot.hoop = shooting::nearest_bearing(cp, unit(dir), &theirs);
                        shot.power = f;
                        self.targeted_hoop = shot.hoop;
                        self.quaffle_glow = f;
                    }
                    ThrowStep::AimPass { dir, f } => {
                        if first {
                            self.wind_up_start(t, c);
                        }
                        self.aim(t, c, f, unit([dir[0], 0.0, dir[2]]));
                        th.in_cone = self.teams[t].in_cone;
                    }
                    ThrowStep::Shoot { flat } => {
                        // a combo shot (`+0x184` 6, `0x43f140`) is a special shot: the chain counts it (**[I]** its
                        // `+0x6c = 72` and game `+0x224 = 1` are not read)
                        if th.combo == 6 && self.teams[t].shot.hoop.is_some() {
                            self.teams[t].combo_chain += 1;
                        }
                        self.shoot(t, c);
                        if let Some(fl) = self.flight.as_mut() {
                            fl.flat = flat;
                        }
                    }
                    ThrowStep::Pass { a, volley } => {
                        if self.teams[t].receiver.is_some_and(|r| r != c) {
                            self.throw_after(t, c, volley, th.combo == 1, a);
                            if let Some(fl) = self.flight.as_mut() {
                                fl.flat = volley;
                            }
                        }
                    }
                    ThrowStep::Wait => {}
                }
                step = AttackSub::Throw(th);
                // the carrier flies on meanwhile (`0x436490(0, input)`: no chase, gain 0)
                if self.holder == Some(c) {
                    let pl = &self.players[c];
                    let call = Call {
                        stick: input.stick,
                        target: None,
                        holds_ball: true,
                        ai: false,
                        stun: self.teams[t].tackle.stun,
                        y_target: self.pitch.floor(pl.pos) + self.pitch.offset(&self.graph, pl.pos),
                        gain: 0.0,
                        rope: handling::ROPE_HUMAN,
                    };
                    self.teams[t].call = Some(call);
                    self.handle(t, c, input.stick, false);
                }
            }
        }
        human::State::Attack { sub: step, armed }
    }

    /// The start of a human's wind-up (`0x43dbd0(24)`, from `0x447133`): no receiver yet, the target the carrier;
    /// the wind-up's clip, as the AI's two callers (the pass state `0x458bb8`, the shot routine `0x456a73`) get
    /// theirs (**[I]** the clip's request, type 6, is not traced to its caller).
    fn wind_up_start(&mut self, t: usize, c: usize) {
        let team = &mut self.teams[t];
        team.receiver = None;
        team.target = self.players[c].pos;
        self.events.push(Event::WindUp { tick: self.tick, player: c });
    }

    /// The team's active player and its handling. With the ball, the carrier (0x4362b0, no tackle). Without it,
    /// the AI defender (state 0x21): placeholder for the brain's choice, the chaser nearest the ball is the active
    /// player; against a chaser carrier it presses and tackles as traced, and before the press it approaches the
    /// opposing active player ([`Match::approach_tick`]).
    fn update_defender(&mut self, t: usize) {
        let carrier = self.holder;
        // the team defence state: the Quaffle in the opponents' ball list, held or passed
        if self.ball_team == Some(1 - t) {
            self.defence_tick(t);
        } else {
            // the team defence state's exit: a Bludger move ends with it
            self.end_bludger(t);
            self.teams[t].defence = None;
        }
        let active = match carrier.filter(|&h| self.players[h].team == t) {
            Some(c) => c,
            None => {
                let Some(a) = self.off_ball_active(t) else { return };
                a
            }
        };
        if self.teams[t].active != Some(active) {
            // a new active player: the target lock, the accumulator and the lunge are reset (`0x4352fb`)
            let team = &mut self.teams[t];
            team.active = Some(active);
            team.press = 0;
            team.tackle.acc = [0.0; 3];
            team.tackle.lunge = 0.0;
        }
        if self.holder == Some(active) {
            // the carrier: its state set the call; the handling's timers run here (no tackle)
            let stick = self.teams[t].call.map_or([0.0; 3], |c| c.stick);
            self.handle(t, active, stick, false);
            return;
        }
        if self.teams[t].keeper_run.is_some() {
            self.keeper_run(t, active);
            return;
        }
        if self.teams[t].bludger.is_some() {
            // the Bludger coordinator (state 0x1f, `0x453340`) moves the active player and the Bludger
            self.run_bludger(t);
            return;
        }
        if self.run_special_tackle(t) {
            return;
        }
        let ap = self.players[active].pos;
        // placeholder for the brain's states without a carrier to press: fly at the ball (or the carrier)
        let mut stick = unit(sub(carrier.map_or(self.quaffle, |h| self.players[h].pos), ap));
        let mut press = false;
        // the state 0x21's approach is on the opposing active player while the Quaffle is in their list, held or
        // passed (a keeper's is chased, not approached)
        let opposing = (self.ball_team == Some(1 - t)).then_some(self.teams[1 - t].active).flatten();
        let approached = opposing.filter(|&o| self.players[o].role != Role::Keeper);
        if let Some(c) = carrier.filter(|&h| self.players[h].team != t) {
            let cp = &self.players[c];
            let counter = tackle::press_counter(self.teams[t].press, cp.role == Role::Keeper, true, cp.vel);
            self.teams[t].press = counter;
            if counter > tackle::PRESS_AFTER {
                (stick, press) = tackle::press_input(counter, ap, cp.pos);
                // the press starts the approach again (`+0x10 = 0`); its first tick picks a player for an event
                // against a human team
                self.teams[t].approach.phase = 0;
                if counter == tackle::PRESS_AFTER + 1 && self.teams[1 - t].human {
                    self.rng.rand();
                }
            } else if approached.is_some_and(|o| self.approach_tick(t, active, o)) {
                return;
            }
        } else {
            self.teams[t].press = 0;
            if approached.is_some_and(|o| self.approach_tick(t, active, o)) {
                return;
            }
        }
        self.teams[t].call = Some(self.chase_call(active, stick));
        self.handle(t, active, stick, press);
        self.teams[t].press = tackle::press_after(self.teams[t].press);
    }

    /// One tick of the AI defender `me`'s approach on the opposing player `c` (`0x453ac0` when the press is off,
    /// [`crate::approach`]). Answers whether it moved him: a formotion step (taken in [`Match::move_player`]) or a
    /// homing tick through the general mover; with no formotions loaded or none found, the placeholder chase.
    fn approach_tick(&mut self, t: usize, me: usize, c: usize) -> bool {
        let Some(global) = self.global_db() else { return false };
        let choice = self.teams[t].defence.map_or(0, |d| d.0);
        if self.teams[t].approach.key != Some((me, choice)) {
            self.teams[t].approach = Approach::enter((me, choice));
        }
        let mut ap = std::mem::take(&mut self.teams[t].approach);
        let (mp, mv) = (self.players[me].pos, self.players[me].vel);
        let (cp, cv) = (self.players[c].pos, self.players[c].vel);
        let body = Body { pos: mp, vel: mv, idle: self.players[me].cmd.is_none() };
        if ap.phase == 0 {
            if approach::ahead(mp, cp, self.teams[1 - t].attack_dir()) {
                ap.phase = 1;
            }
            let r = self.rng.rand();
            ap.kind = self.defence_table.draw(r);
            let team_db = self.formation.as_ref().and_then(|f| f[t].team_db.clone());
            let behind = ap.phase == 0;
            let picked = formation::pick_defence(&mut self.rng, team_db.as_deref(), &global, ap.kind as i32, behind);
            ap.runner = picked.map(|f| {
                let mut r = Runner::default();
                let turn = if t == 1 { [-1.0, 0.0] } else { [1.0, 0.0] };
                let frame = Frame::new(cp, cv, approach::FOLLOW);
                let mult = self.teams[t].tweak_ai_def;
                r.start_turned(f, approach::SCALES, Some(frame), body, approach::STEP_CAP, turn, mult);
                r
            });
            if ap.runner.is_some() {
                ap.phase = 1;
            }
        }
        let moved = if ap.kind >= 2 {
            // `<NULL>` has no homing: nothing moves him
            true
        } else if !ap.run {
            self.homing(t, me, c, &mut ap)
        } else if let Some(r) = ap.runner.as_mut() {
            if r.finished() {
                ap.phase = 2;
                ap.run = false;
                let moved = self.homing(t, me, c, &mut ap);
                if self.teams[1 - t].human {
                    // a player for an event (`[0x1043a50]`), not affecting play
                    self.rng.rand();
                }
                moved
            } else {
                if let Some(step) = r.run(Some((cp, cv)), body) {
                    self.approach_steps[me] = Some(step);
                }
                true
            }
        } else {
            // the kind's fallback (`0x454130`, `0x454720`) is for a database without its formotions: not ported
            false
        };
        self.teams[t].approach = ap;
        moved
    }

    /// The kind's homing (`0x454330`, `0x4549e0`): nothing before phase 2; then it counts on and steers the
    /// defender through the general mover (`0x436490(1, input)`).
    fn homing(&mut self, t: usize, me: usize, c: usize, ap: &mut Approach) -> bool {
        if ap.phase < 2 {
            return true;
        }
        let count = ap.phase;
        ap.phase += 1;
        let a = self.defence_table.accuracy[ap.kind];
        let v = approach::View {
            me: self.players[me].pos,
            me_vel: self.players[me].vel,
            carrier: self.players[c].pos,
            past: self.teams[t].history.back(approach::delay(ap.kind, a)),
            own: self.teams[t].attack_dir(),
            opp: self.teams[1 - t].attack_dir(),
        };
        let h = if ap.kind == 0 {
            approach::direct_frontal(&v, count, a, &mut ap.near)
        } else {
            if count == 2 {
                self.block_side = -(v.carrier[2] - v.me[2]);
            }
            approach::block_side(&v, a, self.block_side)
        };
        if h.restart {
            ap.phase = 0;
            ap.run = true;
        }
        self.teams[t].call = Some(self.chase_call(me, h.stick));
        self.handle(t, me, h.stick, h.tackle);
        true
    }

    /// The Formotion part of the common update (`0x430ae0`): slot assignment for both teams (the first-updated team
    /// first), then the selectors, then the trail points and runners.
    fn update_formations(&mut self, first: usize) {
        let Some(mut f) = self.formation.take() else { return };
        let bodies: Vec<Body> =
            self.players.iter().map(|p| Body { pos: p.pos, vel: p.vel, idle: p.cmd.is_none() }).collect();
        let order = [first, 1 - first];
        for &t in &order {
            let active = self.teams[t].active;
            let keeper = active == Some(self.teams[t].keeper);
            f[t].assign(self.teams[t].chasers, active, keeper);
        }
        for &t in &order {
            let mut want = 0;
            if let Some(l) = f[t].leader {
                let pos = self.players[l].pos;
                if shooting::in_goal_area(pos, 1 - t, self.scale_goals, 1.0) {
                    want |= 0x20;
                }
                if shooting::in_goal_area(pos, t, self.scale_goals, 1.0) && self.players[l].role == Role::Keeper {
                    want |= 0x40;
                }
            }
            let [a, b] = &mut f;
            let (me, other) = if t == 0 { (a, &*b) } else { (b, &*a) };
            me.mode = self.challenge.unwrap_or(0);
            me.select(&mut self.rng, other, &bodies, want);
        }
        self.formation_steps.iter_mut().for_each(|s| *s = None);
        for &t in &order {
            for s in f[t].run(&bodies) {
                let k = s.player;
                self.formation_steps[k] = Some(s);
            }
        }
        self.formation = Some(f);
    }

    /// The parts of the broom handling (`0x442ac0`) ported so far: its timers and the tackle branch. The victim is
    /// the Quaffle's holder. Skipped while the player's mover has a command pending.
    fn handle(&mut self, t: usize, p: usize, stick: Vec3, tackle_input: bool) {
        if self.players[p].cmd.is_some() {
            return;
        }
        let chaser = self.players[p].role == Role::Chaser;
        let tm = &mut self.teams[t].tackle;
        tm.start();
        let allowed = tm.recharge_allows(chaser);
        let victim = self.holder.filter(|&h| self.players[h].team != t);
        // the victim within reach at the tick's start, with no stun on the team (`0x442d0f`)
        let pp = self.players[p].pos;
        let in_range =
            self.teams[t].tackle.stun < 1 && victim.is_some_and(|v| len(sub(self.players[v].pos, pp)) < tackle::RANGE);
        if let (true, true, Some(v)) = (tackle_input, allowed, victim) {
            let s = tackle::Situation {
                tackler: self.players[p].pos,
                facing: self.players[p].fwd,
                stick,
                victim: self.players[v].pos,
                victim_vel: self.players[v].vel,
                victim_dodging: self.teams[1 - t].tackle.dodging,
                cone_degrees: self.teams[t].lunge_cone,
            };
            match self.teams[t].tackle.tackle(&s) {
                Some(Outcome::Tackle { point, ticks }) => return self.start_tackle(p, v, point, ticks, false),
                Some(Outcome::Whiff { ticks }) => {
                    // the miss lunge (`0x4275e0`): on along his velocity, half as far as it would carry him, and
                    // slowed to a quarter of it (`0x5befcc`, `0x5bf044`)
                    let pl = &self.players[p];
                    let to = add(pl.pos, scale(pl.vel, 0.5 * ticks as f64 / TICKS));
                    let to_vel = scale(pl.vel, 0.25);
                    self.command(p, to, to_vel, ticks);
                    self.events.push(Event::Whiff { tick: self.tick, player: p });
                }
                Some(Outcome::Lunge) => self.events.push(Event::Lunge { tick: self.tick, player: p }),
                None => {}
            }
        }
        // the lunge's end, on the opponents' active player (`0x443f27`)
        let dist = self.teams[1 - t].active.map_or(f64::INFINITY, |a| len(sub(self.players[a].pos, pp)));
        let vic = victim.map(|v| (self.players[v].pos, self.players[v].vel));
        let dodging = self.teams[1 - t].tackle.dodging;
        match self.teams[t].tackle.lunge_end(dist, in_range, vic, dodging) {
            Some(LungeEnd::Tackle { point, ticks }) => {
                if let Some(v) = victim {
                    self.start_tackle(p, v, point, ticks, true);
                }
            }
            Some(LungeEnd::Miss) => self.events.push(Event::LungeMiss { tick: self.tick, player: p }),
            None => {}
        }
    }

    /// A successful tackle (`0x426d40`, `0x428260`, `0x436a80`; its moves checked under an emulator,
    /// `tools/check_handling.py`): the tackler flies to `point` in `ticks`, arriving at the average speed
    /// (`0x427d80`); the victim flies there too, arriving at rest a tick before the end, and gets his velocity back
    /// on the last (`0x427900`). The ball stays on the victim until the end and goes to the tackler; the victim's
    /// team is stunned and knocked back.
    fn start_tackle(&mut self, tackler: usize, victim: usize, point: Vec3, ticks: i32, lunge: bool) {
        let end = self.tick + ticks as u32;
        let vv = self.players[victim].vel;
        let arrive = scale(sub(point, self.players[tackler].pos), TICKS / ticks.max(1) as f64);
        self.command(tackler, point, arrive, ticks);
        self.players[tackler].queue.clear();
        self.command(victim, point, [0.0; 3], ticks - 1);
        let last = self.tick + (ticks - 1).max(1) as u32;
        self.players[victim].queue =
            vec![Command { start: last, end: last + 1, from: point, from_vel: [0.0; 3], to: point, to_vel: vv }];
        self.holder = None;
        self.flight = Some(Flight {
            flat: false,
            kind: FlightKind::Tackle { victim, tackler },
            start_tick: self.tick,
            end_tick: end,
            from: self.quaffle,
            to: point,
        });
        let vt = self.players[victim].team;
        let (vp, tp) = (self.players[victim].pos, self.players[tackler].pos);
        self.teams[vt].tackle.penalise(vp, tp, vv);
        self.events.push(Event::Tackle { tick: self.tick, tackler, victim, end, lunge });
    }

    /// Issues a movement command to `to` (arriving with `to_vel`) over `ticks` from now.
    fn command(&mut self, p: usize, to: Vec3, to_vel: Vec3, ticks: i32) {
        let pl = &mut self.players[p];
        pl.cmd = Some(Command {
            start: self.tick,
            end: self.tick + ticks.max(1) as u32,
            from: pl.pos,
            from_vel: pl.vel,
            to,
            to_vel,
        });
    }

    fn update_carrier(&mut self, t: usize) {
        let Some(carrier) = self.holder.filter(|&p| self.players[p].team == t) else {
            self.teams[t].state = None;
            self.teams[t].shot.left = 0;
            return;
        };
        if self.teams[t].state.is_none() {
            // state 0x0f's enter (`0x455e00`) clears the shot countdown (`c0`)
            self.teams[t].shot.left = 0;
        }
        // state 0x0f's tick (`0x455e40`): `c1` counts its ticks; then the sub-state it asks for (`0x4462e0` keeps the
        // one running when it asks for it again): 0x13 while the opponents' Bludger move runs at the team (team
        // `+0x143c`), 0x12 while their defensive special does (their team state 6, `0x4376c0`), else 0x11
        self.teams[t].c1 += 1;
        let bludger = self.teams[1 - t].bludger.is_some();
        let special = self.teams[1 - t].special_tackle.is_some();
        let state = match self.teams[t].state {
            Some(s @ CarrierState::BludgerWeave { .. }) if bludger => s,
            Some(s @ CarrierState::Weave) if special && !bludger => s,
            Some(s @ (CarrierState::Possession { .. } | CarrierState::Pass { .. })) if !special && !bludger => s,
            _ if bludger => CarrierState::BludgerWeave { n: 0 },
            _ if special => CarrierState::Weave,
            _ => {
                // state 0x11's enter (`0x456170`)
                let ai = self.teams[t].ai;
                passing::possession_enter(&ai, &mut self.rng, self.combo_enabled, &mut self.teams[t].plan);
                CarrierState::Possession { timer: 0, sub: false }
            }
        };
        let keeper = self.players[carrier].role == Role::Keeper;
        let next = match state {
            CarrierState::Weave => self.weave(t, carrier),
            CarrierState::BludgerWeave { n } => self.bludger_weave(t, carrier, n),
            CarrierState::Possession { timer, .. } if keeper => self.keeper_possession(t, carrier, timer),
            CarrierState::Possession { timer, sub } => self.possession_update(t, carrier, timer, sub),
            CarrierState::Pass { timer, dir } => {
                let next = self.pass_update(t, carrier, timer, dir);
                // the pass state flies on towards the attacked end (0x436490 for the keeper, 0x4362b0 for a chaser)
                if self.holder == Some(carrier) {
                    let stick = [self.teams[t].attack_dir(), 0.0, 0.0];
                    self.teams[t].call = Some(self.carrier_call(carrier, stick, !keeper));
                }
                next
            }
        };
        self.teams[t].state = if self.holder == Some(carrier) { next } else { None };
    }

    /// The handling call of a carrier: `0x4362b0` (`chaser`: seek the cruising height (`0x41a610`) with gain 0.2,
    /// kicking the height graph along with the turn) or `0x436490` without a target (the keeper: gain 0).
    fn carrier_call(&mut self, p: usize, stick: Vec3, chaser: bool) -> Call {
        self.carrier_call_by(p, stick, chaser, true)
    }

    /// [`Match::carrier_call`] for an AI team or a human's; `0x4363a5` scales an AI team's kick by `ScaleAIKick`.
    fn carrier_call_by(&mut self, p: usize, stick: Vec3, chaser: bool, ai: bool) -> Call {
        let pl = &self.players[p];
        if chaser {
            let scale = if ai { self.kick_scale.0 * self.kick_scale.1 } else { self.kick_scale.0 };
            let k = pitch::carrier_kick(pl.vel, pl.prev_vel, scale);
            self.graph.kick(k);
        }
        let pl = &self.players[p];
        Call {
            stick,
            target: None,
            holds_ball: true,
            ai: true,
            stun: self.teams[pl.team].tackle.stun,
            y_target: if chaser { self.pitch.cruise(&self.graph, pl.pos) } else { pl.pos[1] },
            gain: if chaser { 0.2 } else { 0.0 },
            rope: handling::ROPE_HUMAN,
        }
    }

    /// The general mover chasing the Quaffle (`0x436490` with `chase` set): the steering assist pulls towards the
    /// ball; the height sought is the floor plus the wave (`0x41a570` + `0x432a80`), and the gain dives towards the
    /// ball only when it is above the cruising height and close: `(6 g - (d - 1)) / (6 g)`, `g` the ball's height
    /// over the cruising height.
    fn chase_call(&self, p: usize, stick: Vec3) -> Call {
        let pl = &self.players[p];
        let y_target = if pl.role == Role::Keeper {
            pl.pos[1]
        } else {
            self.pitch.floor(pl.pos) + self.pitch.offset(&self.graph, pl.pos)
        };
        let d = len(sub(self.quaffle, pl.pos));
        let mut gain = 1.0;
        if d > 1.0 {
            let g = self.quaffle[1] - self.pitch.cruise(&self.graph, pl.pos);
            gain = if g > 0.0 { ((6.0 * g - (d - 1.0)) / (6.0 * g)).max(0.0) } else { 0.0 };
        }
        // the rope (`0x443b1f`): a human team's keeps its chaser on the ball
        let rope = if self.challenge.is_some() {
            handling::ROPE_CHALLENGE
        } else if self.teams[pl.team].human {
            handling::ROPE_HUMAN
        } else {
            handling::ROPE_AI
        };
        Call {
            stick,
            target: Some((self.quaffle, self.quaffle_vel)),
            holds_ball: self.holder == Some(p),
            ai: true,
            stun: self.teams[pl.team].tackle.stun,
            y_target,
            gain,
            rope,
        }
    }

    /// The team-options routine `0x4568a0`: in the goal area a carrier with no wind-up first weighs the special
    /// move (its sub-bar full and `c1` past 48: one `rand()` against `SpecialMoveShootWeight`, `0x4370a0`; a miss
    /// clears `c1`), then may start a wind-up; outside it, the uber move alike (`UberMoveWeight`, `0x43ed10`); each
    /// wind-up tick draws the hoop; the last shoots. Returns whether a wind-up or a move runs. **[I]** the team
    /// flags `+0x70`, `+0x72` are taken as clear.
    fn shot_options(&mut self, t: usize, carrier: usize) -> bool {
        if self.power.seeker_game || self.challenge.is_some() {
            return false;
        }
        let cp = self.players[carrier].pos;
        let defending = 1 - t;
        let in_area = shooting::in_goal_area(cp, defending, self.scale_goals, 1.0);
        let free = self.teams[t].special_tackle.is_none();
        if in_area && self.teams[t].shot.left == 0 {
            let k = powerbar::SPECIAL;
            if self.power.sub[t][k] >= self.power.pots[t][k] && self.power.pots[t][k] > 0.0 && self.teams[t].c1 > 48 {
                let u = self.rng.rand() as f64 * (1.0 / 32767.0);
                if u < self.teams[t].special_shoot_weight && free && self.special_move(t, carrier) {
                    return true;
                }
                self.teams[t].c1 = 0;
            }
        }
        if !in_area && self.holder == Some(carrier) {
            let k = powerbar::UBER;
            if self.power.sub[t][k] >= self.power.pots[t][k] && self.power.pots[t][k] > 0.0 && self.teams[t].c1 > 48 {
                let u = self.rng.rand() as f64 * (1.0 / 32767.0);
                if u < self.teams[t].uber_weight && free && self.uber_move(t) {
                    return true;
                }
                self.teams[t].c1 = 0;
            }
        }
        if in_area && self.teams[t].shot.left == 0 {
            let inner = shooting::in_goal_area(cp, defending, self.scale_goals, shooting::INNER_SCALE);
            if !shooting::starts_wind_up(&mut self.rng, inner, cp[2]) {
                return false;
            }
            // `0x43dbd0(24)`: no receiver, the target is the carrier
            let team = &mut self.teams[t];
            team.receiver = None;
            team.target = cp;
            let n = shooting::wind_up_ticks(&mut self.rng, team.shooting_skill);
            team.shot = ShotPlan { left: n, total: n, hoop: None, power: 0.0 };
            self.events.push(Event::ShotWindUp { tick: self.tick, player: carrier, ticks: n });
        }
        let shot = &mut self.teams[t].shot;
        if shot.left <= 0 {
            return false;
        }
        shot.left -= 1;
        if shot.left == 0 {
            self.shoot(t, carrier);
            return true;
        }
        // aim: draw the hoop, then lock the opposing hoop nearest in bearing and the power
        let keeper = Some(self.players[self.teams[defending].keeper].pos);
        let theirs: Vec<(usize, Vec3)> =
            self.hoops.iter().enumerate().filter(|(_, h)| h.team == defending).map(|(i, h)| (i, h.pos)).collect();
        let skill = self.teams[t].shooting_skill;
        let d = match shooting::pick_hoop(&mut self.rng, skill, cp, keeper, &theirs) {
            Some(h) => unit(sub(self.hoops[h].pos, cp)),
            None => [self.teams[t].attack_dir(), 0.0, 0.0],
        };
        let shot = &mut self.teams[t].shot;
        shot.power = (shot.total - shot.left) as f64 / shot.total as f64;
        shot.hoop = shooting::nearest_bearing(cp, d, &theirs);
        self.targeted_hoop = shot.hoop;
        self.quaffle_glow = shot.power;
        true
    }

    /// The shot (`0x43f140` and the commands of `0x4265c0`): the Quaffle flies at the locked hoop's centre; the goal
    /// is scheduled for the tick before it arrives.
    fn shoot(&mut self, t: usize, carrier: usize) {
        let cp = self.players[carrier].pos;
        let team = &mut self.teams[t];
        team.lock = (0, None);
        team.receiver = None;
        let Some(hoop) = team.shot.hoop.take() else { return };
        let target = self.hoops[hoop].pos;
        let f = team.shot.power;
        let n = shooting::shot_ticks(len(sub(target, cp)), f) as u32;
        self.flight = Some(Flight {
            flat: false,
            kind: FlightKind::Shot { hoop, team: t },
            start_tick: self.tick,
            end_tick: self.tick + n,
            from: cp,
            to: target,
        });
        self.holder = None;
        self.events.push(Event::Shot { tick: self.tick, player: carrier, hoop, power: f, goal: self.tick + n - 1 });
    }

    /// A goal for `team` at `hoop` (the type-6 event at the shot's `end - 1`, `0x432100`): 10 points, the goal
    /// cutscene roll, a combo chain paid into the bar; the Quaffle drops from where it is from the next tick.
    fn goal(&mut self, team: usize, hoop: usize) {
        self.teams[team].score += 10;
        let combo = self.teams[team].combo_chain != 0;
        self.events.push(Event::Goal { tick: self.tick, team, hoop, combo });
        // the goal cutscene roll (`0x432100`): `u = U * 100` under `GoalCSProbabilityCeleb` the celebration, under
        // `Celeb + Crowd` the crowd's
        let u = self.rng.rand() as f64 * (100.0 / 32767.0);
        let (celeb, crowd) = self.goal_cs;
        let kind = if (0.0..celeb).contains(&u) {
            3
        } else if u >= celeb && u < celeb + crowd {
            1
        } else {
            0
        };
        if kind != 0 {
            self.events.push(Event::GoalCutscene { tick: self.tick, team, kind });
        }
        // a combo chain pays into the scorer's bar (`0x43ec20`)
        let tm = &mut self.teams[team];
        if tm.combo_chain != 0 {
            let amount = powerbar::COMBO[tm.combo_chain.min(7) as usize] * tm.pass_increment * tm.combo_bar_multiplier;
            tm.combo_chain = 0;
            self.add_power(team, amount);
        }
        self.flight = Some(Flight {
            flat: false,
            kind: FlightKind::Drop { team, vel: self.quaffle_vel },
            start_tick: self.tick + 1,
            end_tick: self.tick + 1 + 24,
            from: self.quaffle,
            to: self.quaffle,
        });
    }

    /// Team `t`'s special move by carrier `c` (`0x4370a0`, then `0x4371b0`), when it may: not in a challenge, the
    /// carrier holding the Quaffle in the goal area and the special sub-bar full. The target is the opposing hoop
    /// nearest in bearing along the attack (`0x441640`, game `+0x25c`); the sub-bar empties; one `rand()` picks the
    /// move among those the team knows (`0x4351a0`: 1, or 2 and 3 with the game's progress, DataStore
    /// `0xa20daf5d` past 9 and 19), and `Shoot30` gives way to `shvol` or `shrol` (a second draw) when the hoop is
    /// more across than ahead. The cutscene shows the shot (`Shoot30`, then `Score2`, `RE_NOTES.md`, "Match
    /// cutscenes"), and its end (`eGOAL_SCORED`) scores it. **[I]**: the team knows one move; the goal is scored at
    /// once, the Quaffle at the hoop (the game layer holds the match while the cutscenes play).
    fn special_move(&mut self, t: usize, c: usize) -> bool {
        if self.challenge.is_some() || self.holder != Some(c) || self.power.seeker_game {
            return false;
        }
        let cp = self.players[c].pos;
        let k = powerbar::SPECIAL;
        if !shooting::in_goal_area(cp, 1 - t, self.scale_goals, 1.0)
            || !self.power.unlocked[k]
            || self.power.sub[t][k] < self.power.pots[t][k]
        {
            return false;
        }
        let theirs: Vec<(usize, Vec3)> =
            self.hoops.iter().enumerate().filter(|(_, h)| h.team == 1 - t).map(|(i, h)| (i, h.pos)).collect();
        let Some(hoop) = shooting::nearest_bearing(cp, [self.teams[t].attack_dir(), 0.0, 0.0], &theirs) else {
            return false;
        };
        self.power.sub[t][k] = 0.0;
        let known = 1.0;
        let mut kind = (crate::rng::trunc(self.rng.rand() as f64 * known * (1.0 / 32767.0))).clamp(0, 2);
        let target = self.hoops[hoop].pos;
        if kind == 0 && (target[2] - cp[2]).abs() / (target[0] - cp[0]).abs() > 1.0 {
            kind = (1 - crate::rng::trunc(self.rng.rand() as f64 * (-2.0 / 32767.0))).clamp(0, 2);
        }
        self.events.push(Event::SpecialMove { tick: self.tick, team: t, player: c, hoop, kind: kind as u8 });
        let tm = &mut self.teams[t];
        tm.lock = (0, None);
        tm.receiver = None;
        tm.shot = ShotPlan::default();
        tm.special_moves += 1;
        self.holder = None;
        self.quaffle = target;
        self.quaffle_vel = [0.0; 3];
        self.goal(t, hoop);
        true
    }

    /// Team `t`'s uber move (`0x43ed10`, word 22's press for a player's team), when it may: no defensive special
    /// running (team `+0x143c`), not in the race or a challenge (`0x1018f94`), the team holding the Quaffle
    /// (`0x434ff0`) and the uber sub-bar full. The sub-bar empties (game vtable `+0x44(t, 3, 0)`), the other team's
    /// bar loses this team's `UberPenalty` (`+0x3c(1 - t, +0x84)`, **[I]** at once), team `+0xad0` counts it, and
    /// the team's uber cutscene plays (its `teamdescription.txt` `uber`, then `Score2`; game `+0x224 = 3`), whose
    /// end (`eGOAL_SCORED`) scores. **[I]**: the goal at once at the hoop nearest in bearing along the attack (the
    /// move names none), the team's carrier as the event's player; the AI's (step 4 of `0x4568a0`) not ported (its
    /// counter `c1` is not traced); the active player's state test (vtable `+8`) left out.
    pub fn uber_move(&mut self, t: usize) -> bool {
        let k = powerbar::UBER;
        let carrier = self.holder.filter(|&h| self.players[h].team == t);
        let Some(c) = carrier else { return false };
        if self.challenge.is_some()
            || self.power.seeker_game
            || self.race.is_some()
            || self.teams[t].special_tackle.is_some()
            || !self.power.open(t, k)
            || self.power.pots[t][k] <= 0.0
            || self.power.sub[t][k] < self.power.pots[t][k]
        {
            return false;
        }
        let theirs: Vec<(usize, Vec3)> =
            self.hoops.iter().enumerate().filter(|(_, h)| h.team == 1 - t).map(|(i, h)| (i, h.pos)).collect();
        let cp = self.players[c].pos;
        let Some(hoop) = shooting::nearest_bearing(cp, [self.teams[t].attack_dir(), 0.0, 0.0], &theirs) else {
            return false;
        };
        self.power.sub[t][k] = 0.0;
        let penalty = self.teams[t].uber_penalty;
        self.power.penalise(1 - t, penalty);
        self.events.push(Event::UberMove { tick: self.tick, team: t, player: c, hoop });
        let tm = &mut self.teams[t];
        tm.lock = (0, None);
        tm.receiver = None;
        tm.shot = ShotPlan::default();
        tm.special_moves += 1;
        self.holder = None;
        self.quaffle = self.hoops[hoop].pos;
        self.quaffle_vel = [0.0; 3];
        self.goal(t, hoop);
        true
    }

    /// State 0x20 (`0x455730`) on team `t`'s active player `p`. Its first tick: a command over 24 ticks
    /// (`[0x668e48]`) to the mean of the team's hoops, 8 (`0x5bf214`) towards the middle of the pitch, at rest
    /// (the heading the attack direction, **[I]** left to the mover). After it, each tick, the keeper's guard
    /// (`0x43f380`) on him.
    fn keeper_run(&mut self, t: usize, p: usize) {
        self.teams[t].call = None;
        let ticks = self.teams[t].keeper_run.unwrap_or(0);
        self.teams[t].keeper_run = Some(ticks + 1);
        let own: Vec<Vec3> = self.hoops.iter().filter(|h| h.team == t).map(|h| h.pos).collect();
        if ticks == 0 {
            self.teams[t].guard_cmd = None;
            if own.is_empty() {
                return;
            }
            let n = own.len() as f64;
            let mean = own.iter().fold([0.0; 3], |a, h| add(a, scale(*h, 1.0 / n)));
            let towards = if mean[0] > 0.0 { -1.0 } else { 1.0 };
            let to = [mean[0] + towards * TNS_KEEPER_IN, mean[1], mean[2]];
            self.command(p, to, [0.0; 3], TNS_KEEPER_TICKS);
            return;
        }
        self.teams[t].guard_cmd = Some(self.guard(t, p));
    }

    /// The keeper's guard (`0x43f380`) for player `p` of team `t`: where he goes this tick and the step's divisor.
    fn guard(&mut self, t: usize, p: usize) -> (Vec3, i32) {
        let own: Vec<(usize, Vec3)> =
            self.hoops.iter().enumerate().filter(|(_, h)| h.team == t).map(|(i, h)| (i, h.pos)).collect();
        let team = &self.teams[t];
        if self.ball_team == Some(1 - t) {
            let ball = BallView {
                pos: self.quaffle,
                vel: self.quaffle_vel,
                held: self.holder.is_some(),
                facing: self.holder.map_or([0.0; 3], |h| self.players[h].vel),
            };
            let (hoop, at) = keeper::guard_point(
                &own,
                &ball,
                team.keeper_mode,
                team.attack_dir(),
                self.teams[1 - t].attack_dir(),
                shooting::goal_line(self.scale_goals),
            );
            let d = keeper::distance(at, self.players[p].pos);
            let speed = team.keeper_speed;
            let n = self.teams[t].settle.divisor(hoop, d, speed);
            (at, n)
        } else {
            (keeper::home_point(&own, team.attack_dir()), keeper::HOME_DIVISOR)
        }
    }

    /// The keeper off the ball (`0x43f380`, called by the match team update when he is not the active player).
    fn update_keeper(&mut self, t: usize) {
        let k = self.teams[t].keeper;
        if !self.players[k].present {
            self.teams[t].keeper_cmd = None;
            return;
        }
        if self.holder == Some(k) {
            self.teams[t].keeper_cmd = None;
            return;
        }
        let cmd = self.guard(t, k);
        self.teams[t].keeper_cmd = Some(cmd);
    }

    /// The kick-off after a goal (`0x451730`): the kick-off team takes placement 1, the other placement 2, and the
    /// kick-off team's keeper gets the Quaffle (`0x451980`).
    fn kick_off(&mut self, team: usize) {
        if let Some(kind) = self.challenge {
            self.challenge_kick_off(kind, 1);
            return;
        }
        for t in 0..2 {
            let k = if t == team { 1 } else { 2 };
            self.place_team(t, k);
            let tm = &mut self.teams[t];
            tm.state = None;
            tm.shot = ShotPlan::default();
            tm.lock = (0, None);
            tm.receiver = None;
        }
        let keeper = self.teams[team].keeper;
        self.flight = None;
        for t in 0..2 {
            self.end_bludger(t);
        }
        self.set_holder(keeper);
        self.events.push(Event::KickOff { tick: self.tick, team });
    }

    /// A challenge's start (`k` 0, after the intro: `0x45e240`) or restart after a goal (`k` 1: `0x45de95`): both
    /// teams at placement `k`, the Quaffle to the opponent's first chaser (Edgar) in the tackling challenges (2 and
    /// 5, `0x45a8c8`, `0x45df2a`), else to team 0's first chaser (**[I]** the combo challenge's restart).
    fn challenge_kick_off(&mut self, kind: u32, k: usize) {
        for t in 0..2 {
            self.place_team(t, k);
            let tm = &mut self.teams[t];
            tm.state = None;
            tm.shot = ShotPlan::default();
            tm.lock = (0, None);
            tm.receiver = None;
        }
        self.flight = None;
        let team = if matches!(kind, 2 | 5) { 1 } else { 0 };
        if let Some(c) = self.teams[team].chasers.iter().copied().find(|&c| self.players[c].present) {
            self.set_holder(c);
            self.events.push(Event::KickOff { tick: self.tick, team });
        }
    }

    /// Starts the Seeker race now, both seekers on the stadium's spline (the Seeker challenge races from the start,
    /// `0x45e890`; **[I]** on the stadium's first spline, the challenge naming `SeekerStadiumLoop`).
    pub fn start_race(&mut self) {
        if let Some(spline) = self.spline.take() {
            self.race = Some(self.new_race(spline));
            self.power.seeker_game = true;
        }
    }

    /// The race on `spline`, both seekers' boost full. **[I]** the set-up (`0x44d360`) zeroes both power levels
    /// (`+0x1308`); the user's footage of the original starts them full (2026-10-21).
    fn new_race(&self, spline: Spline) -> Race {
        let ai = [!self.teams[0].human, !self.teams[1].human];
        let mut race = Race::new(spline, &self.race_rules, ai);
        for s in &mut race.seekers {
            s.level = 1.0;
        }
        race
    }

    /// Adds `amount` to team `t`'s power bar (the game's vtable `+0x30`; the challenge game's own in a challenge).
    pub fn charge(&mut self, t: usize, amount: f64) {
        self.add_power(t, amount);
    }

    /// Turns team `t`'s Formotion frame to face `dir` (the challenge game's steer, `0x45bf20` -> `0x42ce60`).
    pub fn set_formation_heading(&mut self, t: usize, dir: Vec3) {
        if let Some(f) = &mut self.formation {
            f[t].set_heading(dir);
        }
    }

    /// Fills the two power bars half each, which starts the Seeker game now (the port's shortcut for trying it).
    pub fn start_seeker_game(&mut self) {
        self.add_power(0, 0.5);
        self.add_power(1, 0.5);
    }

    /// `0x450e40` with the sub-bars: fills team `t`'s bar; the Seeker game starts when both are full.
    fn add_power(&mut self, t: usize, amount: f64) {
        if self.challenge.is_some() {
            // the challenge game's adder (`0x45dc60`): the bar and its copy unclamped, the sub-bars to their pots
            // with no unlock check, and no Seeker game
            let pb = &mut self.power;
            pb.main[t] += amount;
            pb.total[t] += amount;
            for k in 0..4 {
                pb.sub[t][k] = (pb.sub[t][k] + amount).clamp(0.0, pb.pots[t][k]);
            }
            return;
        }
        if self.power.add(t, amount, true) {
            self.events.push(Event::SeekerGame { tick: self.tick });
            // the cutscene of the Snitch round the seekers is skipped: the race's starting hold follows at once
            if let Some(spline) = self.spline.take() {
                self.race = Some(self.new_race(spline));
            }
        }
    }

    /// A player takes the Quaffle (the holder event): it goes into his team's ball list.
    fn set_holder(&mut self, p: usize) {
        let team = self.players[p].team;
        let keeper = self.players[p].role == Role::Keeper;
        for t in &mut self.teams {
            t.pass_target = None;
        }
        if self.ball_team != Some(team) {
            // a change of possession recharges both teams' tackles (`0x431c40`; on a change only, inferred)
            for t in &mut self.teams {
                t.tackle.recharge = tackle::RECHARGE_TICKS;
            }
            // and asks both formations for a new state (`0x431ec0` -> `0x440240`)
            if let Some(f) = &mut self.formation {
                for (t, b) in f.iter_mut().enumerate() {
                    b.requested = formation::on_possession(b.current, t == team, keeper);
                }
            }
        } else if let Some(f) = &mut self.formation {
            // the ball stays with the team (a completed pass): AttackPass and DefendPass (`0x434de0` -> `0x431f10`)
            if f[team].current != formation::FREE_TO_ATTACK {
                f[team].requested = formation::ATTACK_PASS;
            }
            if f[1 - team].current != formation::FREE_TO_DEFEND {
                f[1 - team].requested = formation::DEFEND_PASS;
            }
        }
        self.holder = Some(p);
        self.ball_team = Some(team);
        self.quaffle = self.players[p].pos;
        self.quaffle_vel = self.players[p].vel;
        self.sync_attack();
    }

    /// The challenge game's attack directions (`0x45f860`): in Tackle 'n' Shoot, team 1 attacks `[0x1018fa0]` while
    /// the Quaffle is in its ball list (`0x45f110`), else `+x` as in a match.
    pub fn sync_attack(&mut self) {
        let own = self.tns && self.ball_team == Some(1);
        self.teams[1].dir_override = own.then_some(self.challenge_dir);
    }

    /// The challenge's running update, once a second (`0x45b489`, the clock a multiple of 24; with the option
    /// `[0x1018f88]`, assumed set): team 1's attack direction picked again (`[0x1018fa0]`). `rand() % 8` above 3
    /// is `+x`, else `-x`; then `-x` with the Quaffle at `x >= 0`, and `+x` with it more than half the goal line
    /// from the middle (`0.5 * 97.5 * ScaleGoals`). The pick is made in every challenge; only Tackle 'n' Shoot's
    /// team 1 reads it.
    pub fn pick_attack(&mut self) {
        let r = self.rng.rand() % 8;
        let mut dir = if r > 3 { 1.0 } else { -1.0 };
        let x = self.quaffle[0];
        if x >= 0.0 {
            dir = -1.0;
        }
        if x.abs() > 0.5 * shooting::goal_line(self.scale_goals) {
            dir = 1.0;
        }
        self.challenge_dir = dir;
        self.sync_attack();
    }

    /// A challenge's goal, the count now `goals` (the game's `+0xa0`, `0x45de10`): team 1's `Keeper.Speed` is 7.5
    /// after the first and 10 after the second (its `+0x2c`), and Tackle 'n' Shoot's defence hardens
    /// ([`Match::tns_level`]).
    pub fn challenge_goal(&mut self, goals: u32) {
        match goals {
            1 => self.teams[1].keeper_speed = 7.5,
            2 => self.teams[1].keeper_speed = 10.0,
            _ => {}
        }
        if self.tns {
            self.tns_level(goals);
        }
    }

    /// Tackle 'n' Shoot's defence after `goals` of the human's (`0x455170`, the set-up's `0x459ae0` for none): the
    /// approach's kinds' weights and accuracies (the table `0x655e78`), harder with each goal. It also sets team
    /// 0's `Formotions.TweakTime AI_DEF` (`[0x101b580]`: 1.75, 1.33, 1), which no AI of team 0 reads here.
    pub fn tns_level(&mut self, goals: u32) {
        let (direct, block, accuracy) = match goals {
            0 => (1.0, 0.0, 0.5),
            1 => (0.8, 0.2, 0.6),
            _ => (0.65, 0.35, 0.8),
        };
        let t = &mut self.defence_table;
        t.weight[0] = direct;
        t.weight[1] = block;
        t.accuracy[0] = accuracy;
        t.accuracy[1] = accuracy;
    }

    /// State 0x11 update (`0x456340`) for a chaser: the first tick counts the possession against `passes_left`
    /// (and draws a new plan when it runs out); then, with a mate ahead and the hold time over and no shot winding
    /// up, the pass state; otherwise the evade sub-behaviour carries the ball, runs the shot routine and moves.
    fn possession_update(&mut self, t: usize, carrier: usize, timer: i32, sub: bool) -> Option<CarrierState> {
        let mut timer = timer;
        if !sub {
            let team = &mut self.teams[t];
            team.plan.passes_left -= 1;
            timer = 0;
            if team.plan.passes_left == 0 {
                let ai = team.ai;
                passing::possession_redraw(&ai, &mut self.rng, self.combo_enabled, &mut team.plan);
            }
        }
        let team = &self.teams[t];
        let cx = self.players[carrier].pos[0];
        let ahead = team
            .chasers
            .iter()
            .filter(|&&m| m != carrier)
            .any(|&m| passing::mate_ahead(&team.ai, team.attack_dir(), cx, self.players[m].pos[0]));
        timer += 1;
        let next = if timer > team.plan.hold && ahead && team.shot.left == 0 {
            Some(self.enter_pass(t, carrier))
        } else {
            if !sub {
                self.teams[t].evade.enter();
            }
            Some(CarrierState::Possession { timer, sub: true })
        };
        // the evade runs when it had been entered (also on the tick the pass state is asked for)
        if sub || matches!(next, Some(CarrierState::Possession { .. })) {
            self.run_evade(t, carrier);
        }
        next
    }

    /// State 0x15 (`0x4585a0`): towards the attacked end on a weave, the stick `unit(attack, 0, sin phase)` with
    /// the shared phase a step on (`[0x10439dc] += [0x668e08]`, less `2 pi` once past it); the move (`0x4362b0`),
    /// then the shot routine (`0x4568a0`). It passes no one.
    fn weave(&mut self, t: usize, carrier: usize) -> Option<CarrierState> {
        self.weave_phase += WEAVE_STEP;
        if self.weave_phase > std::f32::consts::TAU {
            self.weave_phase -= std::f32::consts::TAU;
        }
        let stick = unit([self.teams[t].attack_dir(), 0.0, f64::from(sin(self.weave_phase))]);
        self.teams[t].call = Some(self.carrier_call(carrier, stick, true));
        self.shot_options(t, carrier);
        Some(CarrierState::Weave)
    }

    /// State 0x16 (`0x4586c0`): once the opponents' Bludger is launched (their `+0x1438`), towards the attacked
    /// end, turning across after 24 counts by `1.5 sin(2 pi (n - 24) / 120)`, the count up 2 a tick; before the
    /// launch no stick at all. Then the move and the shot routine, as 0x15.
    fn bludger_weave(&mut self, t: usize, carrier: usize, n: i32) -> Option<CarrierState> {
        let mut n = n;
        let mut stick = [0.0; 3];
        if self.teams[1 - t].bludger.is_some_and(|m| m.flying) {
            let k = n as f32;
            let side = if k > BLUDGER_WEAVE_DELAY {
                BLUDGER_WEAVE_SIDE * sin((k - BLUDGER_WEAVE_DELAY) * std::f32::consts::TAU / BLUDGER_WEAVE_PERIOD)
            } else {
                0.0
            };
            stick = unit([self.teams[t].attack_dir(), 0.0, f64::from(side)]);
            n += 2;
        }
        self.teams[t].call = Some(self.carrier_call(carrier, stick, true));
        self.shot_options(t, carrier);
        Some(CarrierState::BludgerWeave { n })
    }

    /// The evade state (`0x457090`): its stick, then the shot routine, then the move (`0x4362b0`).
    fn run_evade(&mut self, t: usize, carrier: usize) {
        let pl = &self.players[carrier];
        let opp = self.teams[1 - t].active.map(|o| &self.players[o]);
        let view = evade::View {
            attack_dir: self.teams[t].attack_dir(),
            pos: pl.pos,
            vel: pl.vel,
            fwd: pl.fwd,
            opponent_pos: opp.map_or([1e6, 0.0, 0.0], |o| o.pos),
            opponent_vel: opp.map_or([0.0; 3], |o| o.vel),
            outside_centre: passing::outside_centre(pl.pos[0], pl.pos[2], self.collision.0, self.collision.1)
                || shooting::in_goal_area(pl.pos, 1 - t, self.scale_goals, 1.0),
            goal_line: shooting::goal_line(self.scale_goals),
            can_dodge: self.can_dodge(t),
        };
        let probs = self.teams[t].evade_probs;
        let (stick, dodge) = self.teams[t].evade.update(&mut self.rng, &probs, &view);
        if let Some(kind) = dodge {
            self.start_dodge(t, carrier, kind);
        }
        if self.teams[t].evade.mode != evade::Mode::Dodge {
            self.shot_options(t, carrier);
        }
        self.teams[t].call = Some(self.carrier_call(carrier, stick, true));
    }

    /// `0x436a30`: dodges are allowed (the option byte `0x1018f82`) and the dodge sub-bar is at its pot.
    fn can_dodge(&self, t: usize) -> bool {
        self.dodges_allowed && self.power.get(t, powerbar::DODGE) >= self.power.pots[t][powerbar::DODGE]
    }

    /// The dodge (`+0x37` on the carrier's input): with no stun, no dodge on and the cooldown out it starts
    /// (`0x426f60`, 13 ticks, not ported: the move's commands) and sets the team's dodging byte `+0xab8`, so a
    /// tackle by the other team before this team's next update whiffs; then 14 ticks (`0x606640`) before the next.
    fn start_dodge(&mut self, t: usize, carrier: usize, kind: u8) {
        let tk = &mut self.teams[t].tackle;
        if tk.stun > 0 || tk.dodging || tk.dodge_cool > 0 {
            return;
        }
        tk.dodging = true;
        tk.dodge_cool = DODGE_COOLDOWN;
        // `0x436790`: the dodge sub-bar empties (game `+0x44(t, 0, 0)`), the carrier's commands are cleared
        // (`0x42c0a0`) and the move's go on his mover
        self.power.sub[t][powerbar::DODGE] = 0.0;
        let mut chain = self.dodge_commands(carrier, kind);
        let p = &mut self.players[carrier];
        p.cmd = (!chain.is_empty()).then(|| chain.remove(0));
        p.queue = chain;
        self.events.push(Event::Dodge { tick: self.tick, player: carrier, kind });
    }

    /// A dodge's commands (`0x436790` by kind; checked under an emulator, `tools/check_dodges.py`), from the
    /// carrier's position `P`, velocity `v` and facing `f`, the world's up `u`:
    /// - **6, 7** the roll to the right or the left (`0x42b090`, side `s` = 1, -1): [`ROLL_SEGMENTS`] commands of
    ///   `trunc(0.33 * 24)` = 7 ticks. With `a = f . v` and `k = 1..4`, `θ = s k 2 pi / 4`, `r = f x u` across:
    ///   the position `P + a f_h (1.5 k / 4) + (h - P.y) / (4 * 0.33) (1.5 k / 4) u + 2 (1 - cos θ) u
    ///   + (2 sin θ + s k / 2) r`, where `f_h` is `f` along the ground and `h` the cruising height (the floor and
    ///   the height graph, `0x41a570`, `0x432a80`) where the roll's flight would take him, `P + 4 * 0.33 a f_h`;
    ///   the velocity `a f_h + (h - P.y) / 1.32 u + s (6 / 1.32) 2 (sin θ u + cos θ r)`.
    /// - **8** the loop (`0x4296c0`): over the top and back, `P + 5 f + 4 u` at `20 u`, `P + 8 u` at `-20 f`,
    ///   `P - 5 f + 4 u` at `-20 u` (7 ticks each), then `P` at `15 f` (15 ticks).
    /// - **9** the boost (`0x42ad60`): 10 ahead along `f` at the cruising height there, at `25 f`, in 15 ticks.
    ///
    /// The moves also set the mover's facing and two animation parameters (`0x154`), not ported.
    fn dodge_commands(&self, p: usize, kind: u8) -> Vec<Command> {
        let pl = &self.players[p];
        let cruise = |q: Vec3| self.pitch.floor(q) + self.pitch.offset(&self.graph, q);
        Match::dodge_legs(pl.pos, pl.vel, pl.fwd, self.tick, kind, cruise)
    }

    /// [`Match::dodge_commands`] from the carrier's position, velocity and facing at tick `now`, `cruise` the
    /// cruising height at a point.
    fn dodge_legs(pos: Vec3, vel: Vec3, f: Vec3, now: u32, kind: u8, cruise: impl Fn(Vec3) -> f64) -> Vec<Command> {
        let up = [0.0, 1.0, 0.0];
        let at = |ticks: u32, to: Vec3, to_vel: Vec3| (ticks, to, to_vel);
        let legs: Vec<(u32, Vec3, Vec3)> = match kind {
            6 | 7 => {
                let side = if kind == 6 { 1.0 } else { -1.0 };
                let n = ROLL_SEGMENTS as f64;
                let time = f64::from(ROLL_TIME);
                let a = dot(f, vel);
                let ahead = [a * f[0], 0.0, a * f[2]];
                let lands = add(pos, scale(ahead, n * time));
                let h = cruise(lands);
                let climb = (h - pos[1]) / (n * time);
                let across = unit([-f[2], 0.0, f[0]]);
                let spin = side * (6.0 / (n * time)) * ROLL_RADIUS;
                let ticks = (ROLL_TIME * 24.0) as u32;
                (1..=ROLL_SEGMENTS)
                    .map(|k| {
                        let k = k as f64;
                        let th = side * k * std::f64::consts::TAU / n;
                        let go = ROLL_AHEAD * k / n;
                        let rise = ROLL_RADIUS * (1.0 - cos(th));
                        let wide = ROLL_RADIUS * sin(th) + side * k * ROLL_RADIUS / n;
                        let to =
                            add(add(add(pos, scale(ahead, go)), [0.0, climb * go + rise, 0.0]), scale(across, wide));
                        let v = add(
                            add(ahead, [0.0, climb, 0.0]),
                            scale(add(scale(up, sin(th)), scale(across, cos(th))), spin),
                        );
                        at(ticks, to, v)
                    })
                    .collect()
            }
            8 => {
                let fu = |a: f64, b: f64| add(pos, add(scale(f, a), scale(up, b)));
                vec![
                    at(7, fu(5.0, 4.0), scale(up, 20.0)),
                    at(7, fu(0.0, 8.0), scale(f, -20.0)),
                    at(7, fu(-5.0, 4.0), scale(up, -20.0)),
                    at(15, pos, scale(f, 15.0)),
                ]
            }
            9 => {
                let mut to = add(pos, scale(f, 10.0));
                to[1] = cruise(to);
                vec![at(15, to, scale(f, 25.0))]
            }
            _ => Vec::new(),
        };
        let mut start = now;
        legs.into_iter()
            .map(|(ticks, to, to_vel)| {
                let c = Command { start, end: start + ticks, from: pos, from_vel: vel, to, to_vel };
                start += ticks;
                c
            })
            .collect()
    }

    /// State 0x11 for the keeper (`0x456340`, code 1): on the 6th tick a lane `(U - 0.5) * 34` across the pitch
    /// is drawn (a global, so the last one stays until then); he flies across towards it and, once level with it,
    /// forward; after 24 ticks level with it, or after 48, he passes.
    fn keeper_possession(&mut self, t: usize, carrier: usize, timer: i32) -> Option<CarrierState> {
        let timer = timer + 1;
        if timer == 6 {
            self.lane = passing::lateral_target(&mut self.rng);
        }
        let z = self.players[carrier].pos[2];
        let (stick, ready) = if self.lane < 0.0 && z > self.lane {
            ([0.0, 0.0, -1.0], false)
        } else if self.lane > 0.0 && z < self.lane {
            ([0.0, 0.0, 1.0], false)
        } else {
            ([self.teams[t].attack_dir(), 0.0, 0.0], true)
        };
        if (timer > WIND_UP_TICKS && ready) || timer > 2 * WIND_UP_TICKS {
            return Some(self.enter_pass(t, carrier));
        }
        self.teams[t].call = Some(self.carrier_call(carrier, stick, false));
        Some(CarrierState::Possession { timer, sub: false })
    }

    /// State 0x17's choice of direction (`0x4587f0`, step 2).
    fn enter_pass(&mut self, t: usize, carrier: usize) -> CarrierState {
        let team = &self.teams[t];
        let cp = self.players[carrier].pos;
        let mates: Vec<usize> = team.chasers.iter().copied().filter(|&m| m != carrier).collect();
        let most_central = (0..mates.len())
            .min_by(|&a, &b| self.players[mates[a]].pos[2].abs().total_cmp(&self.players[mates[b]].pos[2].abs()))
            .unwrap_or(0);
        let s = PassSituation {
            keeper: self.players[carrier].role == Role::Keeper,
            special_mode: false,
            wide: passing::outside_centre(cp[0], cp[2], self.collision.0, self.collision.1),
            most_central,
            attack_dir: team.attack_dir(),
            carrier_z: cp[2],
            side_flag: cp[2] < 0.0,
        };
        let dir = match passing::choose_target(&team.ai, &mut self.rng, &s) {
            Target::Mate(i) => unit(sub(self.players[mates[i.min(mates.len() - 1)]].pos, cp)),
            Target::Forward(d) => d,
        };
        self.events.push(Event::WindUp { tick: self.tick, player: carrier });
        CarrierState::Pass { timer: 0, dir }
    }

    /// State 0x17 update (`0x4587f0`, step 3).
    fn pass_update(&mut self, t: usize, carrier: usize, timer: i32, dir: Vec3) -> Option<CarrierState> {
        if timer == 0 {
            return Some(CarrierState::Pass { timer: 1, dir });
        }
        let keeper = self.players[carrier].role == Role::Keeper;
        let ai = self.teams[t].ai;
        // the carrier's `+0x184`: a combo pass (step 1)
        let combo = self.teams[t].plan.combo_left > 0;
        if timer <= WIND_UP_TICKS && passing::releases(&ai, &mut self.rng, timer, keeper) {
            self.aim(t, carrier, timer as f64 / TICKS, dir);
            let mut volley = false;
            if self.teams[t].plan.combo_left > 0 {
                volley = passing::volley(&ai, &mut self.rng, false);
                self.teams[t].plan.combo_left -= 1;
            }
            self.throw(t, carrier, volley, combo);
            return None;
        }
        if timer < WIND_UP_TICKS {
            self.aim(t, carrier, timer as f64 / TICKS, dir);
        }
        if timer > PASS_STATE_TICKS {
            return None;
        }
        Some(CarrierState::Pass { timer: timer + 1, dir })
    }

    /// The nearest chaser of team `t` to `point`, other than `except` (team vtable `+0x40`, `0x4413a0`: squared 3D
    /// distance, ties keep the lower index).
    /// Whether `p` is in the goal area of the team defending against him (`shooting::in_goal_area`).
    pub fn in_attacking_goal_area(&self, p: usize) -> bool {
        let t = self.players[p].team;
        shooting::in_goal_area(self.players[p].pos, 1 - t, self.scale_goals, 1.0)
    }

    /// Team `t`'s chaser nearest `point`.
    pub fn chaser_nearest(&self, t: usize, point: Vec3) -> Option<usize> {
        self.nearest_chaser(t, point, usize::MAX)
    }

    fn nearest_chaser(&self, t: usize, point: Vec3, except: usize) -> Option<usize> {
        let mut best: Option<(f64, usize)> = None;
        for &c in &self.teams[t].chasers {
            let d = len(sub(self.players[c].pos, point)).powi(2);
            let up = self.players[c].knocked <= self.tick;
            if c != except && self.players[c].present && up && best.is_none_or(|(b, _)| d < b) {
                best = Some((d, c));
            }
        }
        best.map(|(_, c)| c)
    }

    /// The catch race (team vtable `+0x44`, `0x441210`): own chasers score `d^2 * ThisTeamCatchMod`, opponents
    /// `d^2 * their ThisTeamCatchMod * our OtherTeamCatchMod` (pulled towards 0.5 in a combo); the lowest wins.
    fn catch_race(&self, t: usize, point: Vec3, except: usize, combo: bool) -> Option<usize> {
        const COMBO_K: [f64; 8] = [2.0, 3.0, 5.0, 7.0, 10.0, 15.0, 20.0, 25.0];
        let (us, them) = (&self.teams[t], &self.teams[1 - t]);
        let mut m = them.this_team_catch_mod * us.other_team_catch_mod;
        if combo {
            m = 0.5 + (m - 0.5) / COMBO_K[us.combo_chain.clamp(0, 7) as usize];
        }
        let mut best: Option<(f64, usize)> = None;
        let mut consider = |c: usize, k: f64| {
            let d = len(sub(self.players[c].pos, point)).powi(2) * k;
            if c != except && self.players[c].present && best.is_none_or(|(b, _)| d < b) {
                best = Some((d, c));
            }
        };
        for &c in &us.chasers {
            consider(c, us.this_team_catch_mod);
        }
        for &c in &them.chasers {
            consider(c, m);
        }
        best.map(|(_, c)| c)
    }

    /// The pass aim (`0x43dd30`) for wind-up fraction `f` along `dir`: picks the receiver, the target and the
    /// flight time.
    fn aim(&mut self, t: usize, carrier: usize, f: f64, dir: Vec3) {
        let cp = self.players[carrier].pos;
        let keeper = self.players[carrier].role == Role::Keeper;
        let fp = f.max(0.2);
        // the first target, swept inside the walls for the thrower's type code (`0x43ddf8`)
        let filter = if keeper { 1 } else { 0 };
        let mut v = scale(dir, passing::pass_speed(f));
        let mut target = self.walls.sweep(cp, v, filter);
        // **[T]** in the attacked goal area (outside the passing challenge) a throw the walls cut short by more
        // than 2 turns round: across the pitch when it was heading for the near end, else back (`0x43de59`)
        let moved = sub(add(cp, v), target);
        if self.in_attacking_goal_area(carrier) && self.challenge != Some(1) && dot(moved, moved) > 4.0 {
            if cp[0] * v[0] > 0.0 {
                v[0] = -v[0];
            } else {
                v = scale(v, -1.0);
            }
            target = self.walls.sweep(cp, v, filter);
        }
        let candidate = self.nearest_chaser(t, target, carrier);
        let opponent = self.holder_of(1 - t).map(|o| sub(self.players[o].pos, cp));
        let team = &self.teams[t];
        let combo = team.plan.combo_left > 0;
        let cone =
            passing::effective_cone(team.pass_cone, team.attack_dir(), opponent, combo.then_some(team.combo_chain));
        let inside = candidate.is_some_and(|c| {
            let to = unit(sub(self.players[c].pos, cp));
            let angle = acos((dir[0] * to[0] + dir[1] * to[1] + dir[2] * to[2]).clamp(-1.0, 1.0));
            passing::in_cone(angle, cone, keeper)
        });
        let (lock_left, locked) = self.teams[t].lock;
        let (receiver, in_cone) = if inside {
            self.teams[t].lock = (3, candidate);
            (candidate, true)
        } else if lock_left > 0 && locked.is_some() {
            self.teams[t].lock = (lock_left - 1, locked);
            (locked, true)
        } else {
            (None, false)
        };
        // the height (`0x43dd30` step 5): inside the cone the target's own, outside it a random one; kept between
        // the clamped floor and the clamped floor plus the wave's height, then the end-zone clamp
        let (receiver, mut height) = if in_cone {
            let r = receiver.unwrap();
            let rp = self.players[r].pos;
            target = add(cp, passing::clamp_length(sub(rp, cp), self.max_distance));
            let lead = if keeper { sub(target, cp) } else { self.players[r].vel };
            // the lead, swept inside the walls from the receiver's place (`0x43e3a7`)
            target = self.walls.sweep(target, passing::clamp_length(lead, self.lead), filter);
            (r, target[1])
        } else {
            let height = cp[1] + 3.0 * fp * self.rng.unit() - 6.0 * fp * self.rng.unit();
            (self.catch_race(t, target, carrier, combo).unwrap_or(carrier), height)
        };
        let floor = self.pitch.floor(target);
        let lo = self.pitch.clamp([target[0], floor, target[2]]);
        let hi = self.pitch.clamp([target[0], floor + self.pitch.max_height, target[2]]);
        height = height.max(lo).min(hi);
        target[1] = self.pitch.clamp([target[0], height, target[2]]);
        let d = len(sub(target, cp));
        let r = len(sub(target, self.players[receiver].pos));
        let team = &mut self.teams[t];
        team.receiver = Some(receiver);
        team.target = target;
        team.in_cone = in_cone;
        team.pass_target = in_cone.then_some(receiver);
        team.flight = passing::flight_ticks(f, d, r, in_cone);
    }

    /// Team `t`'s active player while it holds no ball, for the AI and a human alike. **[T]** the team's update
    /// (`0x435270`) sets it only to the team's carrier; the opponents' passes (`0x4422e0`, at the throw), the team's
    /// own (the receiver, `0x43e7f0`) and a Bludger hit (`0x442110`) change it, so it stays put while the
    /// opponents have the ball. **[I]** with a loose ball, or none yet, the chaser nearest the ball.
    fn off_ball_active(&self, t: usize) -> Option<usize> {
        match (self.teams[t].bludger.is_some(), self.teams[t].active) {
            // the active player stays during a Bludger move
            (true, Some(a)) => Some(a),
            _ if self.pass_receiver().is_some_and(|r| self.players[r].team == t) => self.pass_receiver(),
            (_, Some(a)) if self.ball_team == Some(1 - t) && self.can_be_active(a) => Some(a),
            _ => {
                let target = self.holder.map_or(self.quaffle, |h| self.players[h].pos);
                self.nearest_chaser(t, target, usize::MAX)
            }
        }
    }

    /// Whether `p` can stay his team's active player: in this game and not knocked back by a Bludger.
    fn can_be_active(&self, p: usize) -> bool {
        let pl = &self.players[p];
        pl.present && pl.knocked <= self.tick
    }

    /// `0x4422e0`: team `t`'s chaser nearest `target` (3D) becomes its active player, unless its active player is
    /// within 10 of `target` (horizontally).
    fn defence_pick(&mut self, t: usize, target: Vec3) {
        let current = self.teams[t].active;
        if let Some(a) = current {
            let p = self.players[a].pos;
            if ((p[0] - target[0]).powi(2) + (p[2] - target[2]).powi(2)).sqrt() < 10.0 {
                return;
            }
        }
        let best = self.teams[t].chasers.iter().copied().filter(|&c| self.players[c].present).min_by(|&a, &b| {
            let d = |p: usize| len(sub(target, self.players[p].pos));
            d(a).total_cmp(&d(b))
        });
        if let Some(b) = best {
            self.set_active(t, b);
        }
    }

    /// The receiver of the pass in flight.
    fn pass_receiver(&self) -> Option<usize> {
        match self.flight {
            Some(Flight { kind: FlightKind::Pass { receiver }, .. }) => Some(receiver),
            _ => None,
        }
    }

    fn holder_of(&self, t: usize) -> Option<usize> {
        self.holder.filter(|&p| self.players[p].team == t)
    }

    /// The throw (`0x43e7f0` with `a = 0`): the Quaffle flies to the target and attaches to the receiver on arrival.
    fn throw(&mut self, t: usize, carrier: usize, volley: bool, combo: bool) {
        self.throw_after(t, carrier, volley, combo, 0.0);
    }

    /// The throw with its delay `a`: the arrival is `trunc(10 a)` ticks later (a human's throw passes the ticks it
    /// wound on after release over 6, `0x447267`).
    fn throw_after(&mut self, t: usize, carrier: usize, volley: bool, combo: bool, a: f64) {
        let team = &mut self.teams[t];
        let receiver = team.receiver.take().unwrap_or(carrier);
        let arrival = self.tick + (10.0 * a) as u32 + team.flight.max(1) as u32;
        team.lock = (0, None);
        team.pass_target = None;
        let same = self.players[receiver].team == t;
        if !same {
            team.combo_chain = 0;
        } else if combo {
            // `0x43ebe0`
            team.combo_chain += 1;
        }
        if same && !combo {
            // a plain pass to a mate fills the bar (`0x43ea65`)
            let amount = team.pass_increment;
            self.add_power(t, amount);
        }
        if self.challenge.is_none() {
            // the pass builder's defence pick (`0x4422e0`): the team other than the receiver's takes its chaser
            // nearest the target as its active player, unless the one it has is within 10 of it across the pitch
            let other = 1 - self.players[receiver].team;
            let target = self.teams[t].target;
            self.defence_pick(other, target);
        }
        if receiver != carrier {
            // the pass builder (`0x425c10`) flies the receiver, of either team, to the target by the arrival: a
            // movement command ending at speed 15 along `0.5 d + s x`, with `d` the pass's direction and `s` his
            // team's attack direction, turned round for a pass going back (`s d.x <= -0.5`), the height dropped.
            // **[I]** the catch clip's hand offset (`0x46a870`) that the original takes off the end is left out.
            let target = self.teams[t].target;
            let d = unit(sub(target, self.players[carrier].pos));
            let mut s = self.teams[self.players[receiver].team].attack_dir();
            if s * d[0] <= -0.5 {
                s = -s;
            }
            let v = [0.5 * d[0] + s, 0.5 * d[1], 0.5 * d[2]];
            let k = if len(v) > 0.0 { RECEIVE_SPEED / len(v) } else { 0.0 };
            self.command(receiver, target, [v[0] * k, 0.0, v[2] * k], (arrival - self.tick) as i32);
        }
        let team = &mut self.teams[t];
        self.events.push(Event::Pass {
            tick: self.tick,
            from: carrier,
            receiver,
            arrival,
            in_cone: team.in_cone,
            volley,
        });
        self.flight = Some(Flight {
            flat: false,
            kind: FlightKind::Pass { receiver },
            start_tick: self.tick,
            end_tick: arrival,
            from: self.players[carrier].pos,
            to: team.target,
        });
        self.holder = None;
    }

    /// The Quaffle's mover (`0x464530` and the channel-1 events): a pass or shot flies straight in `x` and `z` with
    /// a lift of `6 D f (1 - f)` in `y`; a pass attaches to its receiver, a shot scores at `end - 1` and then drops.
    fn update_quaffle(&mut self) {
        if let Some(h) = self.holder {
            self.quaffle = self.players[h].pos;
            self.quaffle_vel = self.players[h].vel;
            return;
        }
        let Some(fl) = self.flight else { return };
        let span = (fl.end_tick - fl.start_tick).max(1) as f64;
        let f = ((self.tick + 1 - fl.start_tick) as f64 / span).min(1.0);
        let last = self.tick + 1 >= fl.end_tick;
        let p = match fl.kind {
            FlightKind::Pass { .. } | FlightKind::Shot { .. } => {
                let mut p = add(fl.from, scale(sub(fl.to, fl.from), f));
                let c = if fl.flat { 3.0 } else { 6.0 };
                p[1] += c * (span / TICKS) * f * (1.0 - f);
                p
            }
            FlightKind::Take { player } => {
                let to = self.players[player].pos;
                add(fl.from, scale(sub(to, fl.from), f))
            }
            FlightKind::Tackle { victim, .. } => self.players[victim].pos,
            FlightKind::Toss { .. } => {
                let start = self.kickoff.map_or(fl.start_tick, |k| k.start);
                [fl.from[0], fl.from[1] - TOSS_HEIGHT + toss_height(self.tick + 1 - start), fl.from[2]]
            }
            FlightKind::Drop { vel, .. } => {
                let t = (self.tick + 1 - fl.start_tick) as f64 / TICKS;
                let mut p = add(fl.from, scale(vel, t));
                p[1] -= 6.0 * t * t;
                p
            }
            FlightKind::Loose { vel } => {
                let t = (self.tick + 1 - fl.start_tick) as f64 / TICKS;
                let mut p = add(fl.from, scale(vel, 1.0 - exp(-t)));
                p[1] -= 6.0 * t * t;
                p[1] = p[1].max(self.pitch.floor(p) + 1.0);
                p
            }
        };
        self.quaffle_vel = scale(sub(p, self.quaffle), TICKS);
        self.quaffle = p;
        match fl.kind {
            FlightKind::Pass { receiver } if last => self.attach(receiver),
            FlightKind::Take { player } | FlightKind::Tackle { tackler: player, .. } if last => self.attach(player),
            FlightKind::Toss { player: Some(player) } if last => {
                self.kickoff = None;
                self.attach(player);
            }
            FlightKind::Shot { hoop, team } if last => self.goal(team, hoop),
            // the type-7 event at `end + 23` of the shot, then the kick-off goes to the other team
            FlightKind::Drop { team, .. } if last => self.kick_off(1 - team),
            _ => {}
        }
    }

    fn attach(&mut self, p: usize) {
        self.flight = None;
        self.set_holder(p);
        self.events.push(Event::Catch { tick: self.tick, player: p });
    }

    /// Movement: the keeper's command through the Bezier mover (traced); everyone else is a placeholder until the
    /// broom handling and the Formotions are ported (see the module notes). Each move ends with the movers' commit
    /// against the walls ([`Walls::commit`], `0x465120`).
    fn move_players(&mut self) {
        let attacking = self.ball_team;
        let carrier_pos = self.holder.map(|h| self.players[h].pos).unwrap_or(self.quaffle);
        for i in 0..self.players.len() {
            if !self.players[i].present {
                continue;
            }
            self.players[i].prev_vel = self.players[i].vel;
            let old = self.players[i].pos;
            let Some(filter) = self.move_player(i, attacking, carrier_pos) else { continue };
            let p = &mut self.players[i];
            let (pos, vel) = self.walls.commit(filter, p.team, old, p.pos, p.vel);
            p.pos = pos;
            p.vel = vel;
        }
    }

    /// One player's move; returns the walls' filter of his commit (`0x465120`), `None` when it skips them.
    /// **[T]** The filter is the mover's kind (vtable `+0x44`: chaser 0, keeper 1, beater 2); a keeper on a
    /// movement command that is not the broom handling's (`+0x33` clear) takes 2, and a beater on a command skips
    /// the walls outside a challenge. Seekers have their own commit (`0x41fd30`), without walls.
    fn move_player(&mut self, i: usize, attacking: Option<usize>, carrier_pos: Vec3) -> Option<i32> {
        let dt = 1.0 / TICKS;
        let role = self.players[i].role;
        let challenge = self.challenge.is_some();
        if role == Role::Seeker && self.spline.as_ref().is_some_and(|s| !s.is_empty()) && self.race.is_none() {
            // on his lap's one-tick command ([`Match::seeker_lap_tick`])
            return None;
        }
        // a mover on a command (the Bezier's, a Formotion's or the handling's) or not
        let walls = |commanded: bool, handling: bool| match role {
            Role::Chaser => Some(0),
            Role::Keeper if handling => Some(1),
            Role::Keeper => Some(2),
            Role::Beater if commanded && !challenge => None,
            Role::Beater => Some(2),
            Role::Seeker => None,
        };
        if let Some(c) = self.players[i].cmd {
            // the mover follows its command (`0x4636c0`)
            let span = (c.end - c.start).max(1) as f64;
            let f = ((self.tick + 1 - c.start) as f64 / span).min(1.0);
            let (pos, vel) = keeper::bezier(c.from, c.from_vel, c.to, c.to_vel, span / TICKS, f);
            let p = &mut self.players[i];
            p.pos = pos;
            p.vel = vel;
            // the beaters' formotions turn them along their path as the chasers' do (the runner, `0x42ed00`)
            if p.role == Role::Beater {
                p.fwd = formation::turn_towards(unit(p.fwd), formation::face(vel, p.fwd), formation::TURN);
            }
            if self.tick + 1 >= c.end {
                // the chain's next, from where this one left the mover
                p.cmd = (!p.queue.is_empty()).then(|| {
                    let n = p.queue.remove(0);
                    Command { from: p.pos, from_vel: p.vel, ..n }
                });
            }
            return walls(true, false);
        }
        if let Some((pos, vel)) = self.approach_steps[i].take() {
            // the approach's formotion (`0x42ed00` on the defender's block), turned as the runner turns. The
            // remaster's momentum is not applied: eased in, the defenders do not close (2 tackles in five minutes
            // where the original's runner makes 11)
            let p = &mut self.players[i];
            p.pos = pos;
            p.vel = vel;
            p.fwd = formation::turn_towards(unit(p.fwd), formation::face(vel, p.fwd), formation::TURN);
            return walls(true, false);
        }
        let momentum = self.momentum;
        if let Some(s) = self.formation_steps[i].take() {
            // a Formotion's one-tick command (`0x42ed00`), the player turned 0.3 of the way to face along his step
            // (or his waypoint's heading); with the remaster's momentum, glided towards it instead
            let p = &mut self.players[i];
            (p.pos, p.vel) = if momentum > 0.0 {
                formation::glide(p.pos, p.vel, s.path, formation::STEP_CAP, momentum)
            } else {
                (s.pos, s.vel)
            };
            let (vel, heading) = (p.vel, s.heading);
            let target = heading.unwrap_or_else(|| formation::face(vel, p.fwd));
            p.fwd = formation::turn_towards(unit(p.fwd), target, formation::TURN);
            return walls(true, false);
        }
        let t = self.players[i].team;
        if let (Some(call), true) = (self.teams[t].call, self.teams[t].active == Some(i)) {
            // the active player: the broom handling's velocity, position and heading (`0x442ac0`), committed as a
            // one-tick command with `+0x33` set (`0x444e2b`)
            let pl = &self.players[i];
            let body = handling::Body { pos: pl.pos, vel: pl.vel, fwd: pl.fwd };
            let n = handling::step(&self.handling, &body, &call, &mut self.teams[t].tackle);
            let p = &mut self.players[i];
            p.pos = n.pos;
            p.vel = n.vel;
            p.fwd = n.fwd;
            return walls(true, true);
        }
        if self.kickoff.is_some() && self.ball_team.is_none() && self.teams[t].active == Some(i) {
            // the kick-off's active players wait for their commands: a mover without one stays put (`0x4636c0`)
            self.players[i].vel = [0.0; 3];
            return walls(false, false);
        }
        let p = &self.players[i];
        let team = &self.teams[p.team];
        if let (Some((target, n)), true) = (team.guard_cmd, team.active == Some(i) && team.keeper_run.is_some()) {
            // Tackle 'n' Shoot's keeper (state 0x20): the guard's step
            let (pos, vel) = keeper::bezier_step(p.pos, p.vel, target, [0.0; 3], n);
            let p = &mut self.players[i];
            p.pos = pos;
            p.vel = vel;
            return walls(true, false);
        }
        if p.role == Role::Keeper {
            if let Some((target, n)) = team.keeper_cmd {
                let (pos, vel) = keeper::bezier_step(p.pos, p.vel, target, [0.0; 3], n);
                let p = &mut self.players[i];
                p.pos = pos;
                p.vel = vel;
                return walls(true, false);
            }
        }
        let dir = team.attack_dir();
        let (goal, speed) = match p.role {
            Role::Keeper => (p.pos, 0.0),
            Role::Chaser if attacking == Some(p.team) => {
                let lane = (team.chasers.iter().position(|&c| c == i).unwrap_or(0) as f64 - 1.0) * 17.0;
                let goal = [carrier_pos[0] + dir * 12.0, 0.0, lane];
                ([goal[0], self.pitch.cruise(&self.graph, goal), lane], self.max_speed * team.chaser_speed)
            }
            Role::Chaser => (carrier_pos, self.max_speed * team.chaser_speed),
            Role::Beater | Role::Seeker => (p.pos, 0.0),
        };
        let to = sub(goal, p.pos);
        let step = (speed * dt).min(len(to));
        let vel = scale(unit(to), step / dt);
        let p = &mut self.players[i];
        p.vel = vel;
        p.pos = add(p.pos, scale(vel, dt));
        // **[I]** the placeholder movement stands in for the original's commands
        walls(true, false)
    }
}

/// Beaters and Bludgers ([`crate::bludger`]).
impl Match {
    /// The match database's formotions (the chasers' global one), which the Bludgers and the beaters fly.
    fn global_db(&self) -> Option<Rc<Db>> {
        self.formation.as_ref().map(|f| Rc::clone(&f[0].global))
    }

    /// Team `t`'s Bludger sub-bar is full (team vtable `+0x54`, `0x435060`; always in a challenge with the option).
    fn bludger_charged(&self, t: usize) -> bool {
        let k = powerbar::BLUDGER;
        (self.challenge.is_some() && self.bludgers_allowed) || self.power.sub[t][k] >= self.power.pots[t][k]
    }

    /// Team `t` may start its Bludger move (`0x442a20`): the option, the tackle recharge run out, the sub-bar full
    /// and, in a match, an opposing carrier who is not a keeper.
    pub fn bludger_available(&self, t: usize) -> bool {
        if !self.bludgers_allowed || self.teams[t].tackle.recharge >= 0 || !self.bludger_charged(t) {
            return false;
        }
        if self.teams[t].bludger.is_some() || !self.bludgers[t].present {
            return false;
        }
        self.challenge.is_some()
            || self.holder.is_some_and(|h| self.players[h].team != t && self.players[h].role != Role::Keeper)
    }

    /// The move's start (team vtable `+0x70`, `0x43c3c0`): the beater nearer the Bludger (`0x45ed90`) summons it;
    /// the sub-bar is emptied (game vtable `+0x44(t, 1, 0)`).
    fn start_bludger(&mut self, t: usize, human: bool) {
        let b = self.bludgers[t].pos;
        let d = |p: usize| len(sub(self.players[p].pos, b));
        let mut summoner: Option<usize> = None;
        for &p in &self.teams[t].beaters {
            if self.players[p].present && summoner.is_none_or(|s| d(p) < d(s)) {
                summoner = Some(p);
            }
        }
        let Some(summoner) = summoner else { return };
        let bl = &mut self.bludgers[t];
        let mv = bludger::Move::new(summoner, bl.pos, bl.vel, human);
        bl.state = bludger::State::Summoned;
        bl.cmd = None;
        bl.idle_after = false;
        if let Some(i) = self.teams[t].beaters.iter().position(|&p| p == summoner) {
            self.teams[t].beater_modes[i] = 2;
        }
        self.teams[t].bludger = Some(mv);
        self.power.sub[t][powerbar::BLUDGER] = 0.0;
        self.events.push(Event::BludgerSummon { tick: self.tick, team: t, beater: summoner });
    }

    /// Ends team `t`'s Bludger move (`0x43d950`, called by the challenge game on a target hit).
    pub fn end_bludger_move(&mut self, t: usize) {
        self.end_bludger(t);
    }

    /// The move's end (`0x43d950`): the Bludger goes back to its idle formotion and the sub-bar is emptied.
    fn end_bludger(&mut self, t: usize) {
        if self.teams[t].bludger.take().is_none() {
            return;
        }
        self.bludgers[t].state = bludger::State::Idle;
        self.power.sub[t][powerbar::BLUDGER] = 0.0;
        for m in &mut self.teams[t].beater_modes {
            if *m == 2 {
                *m = 0;
            }
        }
    }

    /// What team `t`'s Bludger flies at: the opposing active player (else the Quaffle); in the Beaters' challenge
    /// the human team's flies at the target nearest it (`0x43cc20`'s challenge branch).
    fn bludger_aim(&self, t: usize) -> bludger::Aim {
        let opp = 1 - t;
        if let (Some(p), 0) = (self.challenge_aim, t) {
            return bludger::Aim {
                pos: p,
                vel: [0.0; 3],
                dir: self.teams[opp].attack_dir(),
                ball_y: p[1] + bludger::BELOW_BALL,
            };
        }
        let active = self.teams[opp].active.filter(|&a| self.players[a].present);
        let (pos, vel) =
            active.map_or((self.quaffle, self.quaffle_vel), |a| (self.players[a].pos, self.players[a].vel));
        // **[I]** the port holds the Quaffle at the carrier's origin, where the original's is in his hand, 0.9 above
        // the point the Bludger flies at
        let ball_y =
            if self.holder.is_some() && self.holder == active { pos[1] + bludger::BELOW_BALL } else { self.quaffle[1] };
        bludger::Aim { pos, vel, dir: self.teams[opp].attack_dir(), ball_y }
    }

    /// A tick of team `t`'s move: the active player closes on the carrier (`0x4389a0`); the summon (`0x43c610`)
    /// until the launch (`0x43c750`); then the flight on the human's stick or the AI's steer (`0x43cc20`) and the
    /// hit test (`0x43da40`).
    fn run_bludger(&mut self, t: usize) {
        let Some(mut mv) = self.teams[t].bludger else { return };
        let opp = 1 - t;
        let carrier = self.holder.filter(|&h| self.players[h].team == opp);
        if let (Some(a), Some(c)) = (self.teams[t].active, carrier) {
            if self.players[a].cmd.is_none() {
                let to = bludger::pursue(self.players[a].pos, self.players[c].pos);
                self.command(a, to, [0.0; 3], 1);
            }
        }
        let rules = self.bludger_rules;
        let challenge = self.challenge.is_some();
        if !mv.flying {
            mv.tick += 1;
            let s = &self.players[mv.summoner];
            let (sp, sv, sf) = (s.pos, s.vel, s.fwd);
            if mv.tick >= rules.summon_period {
                let c = carrier.map(|c| (self.players[c].pos, self.players[c].vel));
                let (p, v) = bludger::launch(&mut mv, self.bludgers[t].pos, (sv, sf), c, challenge);
                let b = &mut self.bludgers[t];
                (b.pos, b.vel, b.state) = (p, v, bludger::State::Flying);
                self.events.push(Event::BludgerLaunch { tick: self.tick, team: t });
            } else {
                if mv.tick == rules.summon_period - bludger::SWING_BEFORE {
                    self.events.push(Event::BeaterSwing { tick: self.tick, player: mv.summoner });
                }
                let (p, v) = bludger::summon_point(&mv, (sp, sv, sf), rules.summon_period);
                let b = &mut self.bludgers[t];
                (b.pos, b.vel) = (p, v);
            }
            self.teams[t].bludger = Some(mv);
            return;
        }
        if self.bludgers[t].cmd.is_some() {
            // the Bludger's mover is busy: no steer (`0x43cc20` returns at once)
            return;
        }
        let aim = self.bludger_aim(t);
        let (bp, bv) = (self.bludgers[t].pos, self.bludgers[t].vel);
        let (input, homing) =
            if mv.human { (self.teams[t].input.stick, true) } else { bludger::ai_input(&mut mv, bp, bv, &aim) };
        let (p, v) = bludger::steer(&mut mv, &rules, bp, bv, &aim, input, homing);
        (self.bludgers[t].pos, self.bludgers[t].vel) = (p, v);
        self.teams[t].bludger = Some(mv);
        // the hit: the opposing active player holding the Quaffle (anyone in the Beaters' challenge)
        let victim = self.teams[opp].active.filter(|&a| self.holder == Some(a) || self.challenge == Some(4));
        if let Some(victim) = victim.filter(|&a| bludger::hits(p, self.players[a].pos)) {
            if !challenge {
                self.teams[opp].hit = Some((victim, v));
            }
            let front = bludger::from_front(v, self.players[victim].fwd);
            self.events.push(Event::BludgerHit { tick: self.tick, team: t, victim, front });
            self.end_bludger(t);
            self.teams[t].bludger_hits += 1;
        }
    }

    /// A Bludger hit on team `t`'s player (`0x442110`, `0x424080`): a carrier drops the Quaffle, loose but still
    /// his team's, and his bar
    /// pays the attacker's `BludgerPenalty` (game vtable `+0x40`); he is knocked back for 48 ticks; the Bludger
    /// bounces back for a tick.
    fn take_hit(&mut self, t: usize) {
        let Some((victim, bv)) = self.teams[t].hit.take() else { return };
        if self.holder == Some(victim) {
            let penalty = self.bludger_rules.penalty[1 - t];
            self.power.penalise(t, penalty);
            self.holder = None;
            // **[T]** the ball stays in the team's list (the hit only releases it, `0x421900` at `0x4246c4`; only the
            // possession change `0x431c40` edits the lists), so only the opponents can pick it up
            // **[I]** the release's velocity (`0x421900` is not read): half the victim's and the Bludger's
            let vel = scale(add(self.players[victim].vel, bv), 0.5);
            self.flight = Some(Flight {
                flat: false,
                kind: FlightKind::Loose { vel },
                start_tick: self.tick,
                end_tick: u32::MAX,
                from: self.quaffle,
                to: self.quaffle,
            });
            for tm in &mut self.teams {
                tm.state = None;
                tm.shot = ShotPlan::default();
            }
            self.events.push(Event::Fumble { tick: self.tick, player: victim });
        }
        let pl = &self.players[victim];
        let to = bludger::knock(pl.pos, pl.vel, bv);
        self.command(victim, to, [0.0; 3], bludger::KNOCK_TICKS);
        self.players[victim].knocked = self.tick + bludger::KNOCK_TICKS as u32;
        let b = &mut self.bludgers[1 - t];
        b.cmd = Some(Command {
            start: self.tick,
            end: self.tick + 1,
            from: b.pos,
            from_vel: b.vel,
            to: add(b.pos, scale(b.vel, -1.0 / TICKS)),
            to_vel: scale(b.vel, -1.0),
        });
    }

    /// The AI's team defence (`0x4552d0`, `0x455320`): a choice drawn on entering it, again every 100 ticks or
    /// once its Bludger move has ended; choosing the Bludger starts the move (state 0x1f's enter `0x453300`),
    /// leaving that choice ends it (its exit `0x4474e0`).
    ///
    /// In Tackle 'n' Shoot (`[0x1018fa4]`) the state's enter makes the special tackle wait 96 ticks longer
    /// (`0x4552fb`), and while the opposing active player is in the team's goal area made 1.2 times as wide (the
    /// game's test `+0x98`, `0x45ab30`) the active player turns keeper (state 0x20, [`Match::keeper_run`]); the
    /// choice then waits. In any challenge (`[0x1018f94]`) the choice holds (no draw every 100 ticks) until its
    /// count reaches the timer (`[0x1018fa8]`): then the special tackle, and the timer 24 shorter (`0x4553d6`).
    /// **[I]** ported for Tackle 'n' Shoot only: the combo challenge's keeper rule (`0x455398`) comes first there
    /// and is not ported.
    fn defence_tick(&mut self, t: usize) {
        let was = self.teams[t].defence;
        if was.is_none() && self.tns {
            self.defence_timer = (self.defence_timer + TNS_TACKLE_WAIT).max(0);
        }
        let opposing = self.teams[1 - t].active.map(|a| self.players[a].pos);
        if self.tns && opposing.is_some_and(|p| shooting::in_goal_area(p, t, self.scale_goals, TNS_KEEPER_AREA)) {
            if was.is_none() {
                self.teams[t].defence = Some((self.defence_roll(t), 0));
            }
            // asking for state 0x20 exits the running sub-state: a Bludger move or a special tackle ends
            self.end_bludger(t);
            self.teams[t].special_tackle = None;
            self.teams[t].keeper_run.get_or_insert(0);
            return;
        }
        self.teams[t].keeper_run = None;
        self.teams[t].guard_cmd = None;
        let ended = matches!(was, Some((2, _))) && self.teams[t].bludger.is_none();
        let mut fired = false;
        let next = match was {
            None => (self.defence_roll(t), 0),
            Some((c, n)) if self.tns => {
                if n + 1 >= self.defence_timer {
                    self.defence_timer = (self.defence_timer - TNS_TACKLE_SOONER).max(0);
                    fired = true;
                    (1, 0)
                } else {
                    (c, n + 1)
                }
            }
            Some((c, n)) if n + 1 < bludger::CHOICE_TICKS && !ended => (c, n + 1),
            _ => (self.defence_roll(t), 0),
        };
        self.teams[t].defence = Some(next);
        // choice 1: the defensive special when it is available (`0x455445`: state 0x1e, else the chase)
        let asked = was.map(|w| w.0) != Some(1) || fired;
        if next.0 == 1 && asked && self.teams[t].special_tackle.is_none() && self.special_tackle_available(t) {
            self.teams[t].special_tackle = Some(SpecialRun::new(true));
        }
        if next.0 == 2 && (was.map(|w| w.0) != Some(2) || ended) && self.teams[t].bludger.is_none() {
            self.start_bludger(t, false);
        } else if next.0 != 2 {
            self.end_bludger(t);
        }
    }

    /// `0x455570`: one `rand()` against `TeamOPs.ChaseWeight`, `SpecialMoveWeight` and `BludgerWeight`. The special
    /// move needs (outside a challenge) the Quaffle no deeper in the team's own half than a third of the goal line,
    /// `-(attack * x) <= 0.33 * 97.5 * ScaleGoals` (`0x668e4c`, the option `0x1018f83` assumed on).
    fn defence_roll(&mut self, t: usize) -> u32 {
        let u = self.rng.rand() as f64 / 32767.0;
        let keeper = self.holder.is_some_and(|h| self.players[h].role == Role::Keeper);
        let ok = self.bludger_charged(t) && self.bludgers_allowed && !keeper && self.bludgers[t].present;
        let deep = -(self.teams[t].attack_dir() * self.quaffle[0]) > 0.33 * shooting::goal_line(self.scale_goals);
        let special = self.challenge.is_some() || !deep;
        bludger::defence_choice(self.bludger_rules.weights, u, ok, special, self.power.seeker_game)
    }

    /// Team `t` may make its defensive special (`0x438920`): the option `0x1018f83` (assumed on), the tackle
    /// recharge run out, an opposing carrier (**[I]** for the opposing team's `+0x8c`) who is not dodging, the
    /// special sub-bar full; none already running, none in the Seeker game. In a challenge team 1 may whenever the
    /// recharge has run out (`0x43893a`): ported for Tackle 'n' Shoot's Edgar; **[I]** otherwise none in a
    /// challenge (team 0's bar and option, the combo challenge's keeper rule, are not ported).
    fn special_tackle_available(&self, t: usize) -> bool {
        let k = powerbar::SPECIAL;
        if self.tns {
            return t == 1 && self.teams[t].special_tackle.is_none() && self.teams[t].tackle.recharge < 0;
        }
        self.challenge.is_none()
            && !self.power.seeker_game
            && self.teams[t].special_tackle.is_none()
            && self.teams[t].tackle.recharge < 0
            && self.holder.is_some_and(|h| self.players[h].team != t && self.players[h].role != Role::Keeper)
            && !self.teams[1 - t].tackle.dodging
            && self.power.unlocked[k]
            && self.power.sub[t][k] >= self.power.pots[t][k]
    }

    /// The defensive special's run (the AI's state 0x1e, `0x455ab0`; the human's sub-state 10): while the
    /// opponents hold the Quaffle (a dodge ends it, **[I]**), each tick
    /// 1. the active player moves: the AI's lunge first (`0x437a90`, once its mover is free: a 10-tick command to
    ///    6 short of the carrier, or of the Quaffle, at 6 units a second on), then, while the count runs, the
    ///    pursuit (`0x4389a0`, to 5 units); **[I]** the lunge's later commands (an intercept on the carrier's
    ///    velocity, through `0x418720`) are left to the pursuit;
    /// 2. the count goes down by 1;
    /// 3. the AI's reach (`0x4380b0`): the latch less 1, 5000 while the Quaffle is within 1.5 of the runner; while
    ///    it is set the near count rises, and past 5 the count is cut to 0;
    /// 4. with the count out and a carrier, the tackle (`0x438430`).
    ///
    /// Answers whether it ran this tick.
    fn run_special_tackle(&mut self, t: usize) -> bool {
        let Some(mut run) = self.teams[t].special_tackle else { return false };
        let (Some(a), true) = (self.teams[t].active, self.ball_team == Some(1 - t)) else {
            // the defence is over
            self.teams[t].special_tackle = None;
            return false;
        };
        if self.teams[1 - t].tackle.dodging {
            self.teams[t].special_tackle = None;
            return false;
        }
        // on the carrier, or where the Quaffle flies meanwhile
        let carrier = self.holder.filter(|&h| self.players[h].team != t);
        let target = carrier.map_or(self.quaffle, |c| self.players[c].pos);
        if self.players[a].cmd.is_none() {
            if run.ai && !run.lunged {
                let ap = self.players[a].pos;
                let u = unit([target[0] - ap[0], 0.0, target[2] - ap[2]]);
                let to = sub(target, scale(u, LUNGE_SHORT));
                self.command(a, to, scale(u, LUNGE_SHORT), LUNGE_TICKS);
                run.lunged = true;
                run.latch = 0;
            } else if run.left > 0 {
                let to = bludger::pursue(self.players[a].pos, target);
                self.command(a, to, [0.0; 3], 1);
            }
        }
        if run.left > 0 {
            run.left -= 1;
        }
        if run.ai {
            run.latch = (run.latch - 1).max(0);
            if len(sub(self.quaffle, self.players[a].pos)) < SPECIAL_REACH {
                run.latch = SPECIAL_LATCH;
            }
            if run.latch > 0 {
                run.near += 1;
                if run.near > 5 {
                    run.left = 0;
                }
            }
        }
        // the count run out and a carrier to take it from
        match carrier.filter(|_| run.left == 0) {
            Some(c) => {
                self.teams[t].special_tackle = None;
                self.special_tackle(t, a, c);
            }
            None => self.teams[t].special_tackle = Some(run),
        }
        true
    }

    /// The defensive special (`0x438430` -> `0x4285d0` -> the tackle cutscene `0x428930`): one `rand()` picks the
    /// cutscene among those the team knows (`0x435140`: 1, 2 or 3 by DataStore `0xa20daf5d` past 14 and 24; 0
    /// `tacar`, 1 `takick`, 2 `tasum`); the special sub-bar empties (game `+0x44(t, 2, 0)`). **[I]**: the team knows
    /// one; the tackler takes the Quaffle at once and the carrier's team takes a tackle's penalty (the cutscene's
    /// own commands on the two are not read).
    fn special_tackle(&mut self, t: usize, tackler: usize, victim: usize) {
        let known = 1;
        let kind = crate::rng::trunc(self.rng.rand() as f64 * known as f64 * (1.0 / 32767.0)).clamp(0, known - 1);
        self.power.sub[t][powerbar::SPECIAL] = 0.0;
        let (vp, tp, vf) = (self.players[victim].pos, self.players[tackler].pos, self.players[victim].fwd);
        self.teams[1 - t].tackle.penalise(vp, tp, vf);
        self.events.push(Event::SpecialTackle { tick: self.tick, team: t, tackler, victim, kind: kind as u8 });
        self.attach(tackler);
    }

    /// The active player a beater's formotion follows (`0x440620`): **[I]** with a keeper active, the first chaser
    /// (the original also asks team vtable `+0x60`, not read).
    fn beater_reference(&self, t: usize) -> Option<usize> {
        let tm = &self.teams[t];
        let a = tm.active.filter(|&a| self.players[a].present);
        match a {
            Some(a) if self.players[a].role == Role::Keeper => {
                tm.chasers.iter().copied().find(|&c| self.players[c].present).or(Some(a))
            }
            Some(a) => Some(a),
            None => tm.chasers.iter().copied().find(|&c| self.players[c].present),
        }
    }

    /// A beater's new formotion (`0x440620`): `BeaterIdle`, `BeaterAttack` or `BeaterDefend` by its mode, framed on
    /// the active player (blocking: at the active player's place then, kept by mode 5).
    fn restart_beater(&mut self, t: usize, i: usize, db: &Db) {
        let mode = self.teams[t].beater_modes[i];
        let (state, defend, a) = match mode {
            0 => (12, -1, bludger::BEATER_SCALE),
            1 => (13, -1, bludger::BEATER_SCALE),
            4 => (14, 0, bludger::BEATER_BLOCK_SCALE),
            5 => (14, 1, bludger::BEATER_BLOCK_SCALE),
            _ => return,
        };
        let Some(f) = formation::pick_beater(&mut self.rng, db, state, 1 << i, defend) else { return };
        let frame = match mode {
            4 => {
                let r = self.teams[t].active.or(self.beater_reference(t));
                let (p, v) = r.map_or(([0.0; 3], [0.0; 3]), |r| (self.players[r].pos, self.players[r].vel));
                self.teams[t].block_point = (p, v);
                Some(Frame::new(p, v, bludger::FOLLOW_READY))
            }
            5 => None,
            _ => self
                .beater_reference(t)
                .map(|r| Frame::new(self.players[r].pos, self.players[r].vel, bludger::FOLLOW_READY)),
        };
        let p = &self.players[self.teams[t].beaters[i]];
        let body = Body { pos: p.pos, vel: p.vel, idle: true };
        self.teams[t].beater_runners[i].start(f, (a, [1.0; 3]), frame, body, bludger::BEATER_CAP);
    }

    /// The beaters' update (`0x441090`): their modes, a formotion each (`0x440bc0`), and the block test (`0x4405c0`).
    fn update_beaters(&mut self, t: usize) {
        if self.challenge.is_some_and(|k| k != 4) {
            return;
        }
        let Some(db) = self.global_db() else { return };
        let opp = 1 - t;
        let incoming = self.teams[opp].bludger.is_some();
        let own_flying = self.teams[t].bludger.is_some_and(|m| m.flying);
        for i in 0..2 {
            let p = self.teams[t].beaters[i];
            if !self.players[p].present {
                continue;
            }
            let mode = self.teams[t].beater_modes[i];
            let mut restart = false;
            if !incoming {
                self.teams[t].block_count = 0;
                if !own_flying {
                    let want = if self.bludgers[t].state == bludger::State::Ready { 1 } else { 0 };
                    if mode != want {
                        self.teams[t].beater_modes[i] = want;
                        restart = true;
                    }
                }
            } else {
                // counted for each beater, so twice a tick
                self.teams[t].block_count += 1;
                if mode == 4 || mode == 5 {
                    if self.teams[t].beater_runners[i].finished() {
                        self.teams[t].beater_modes[i] = 5;
                        restart = true;
                    }
                } else {
                    self.teams[t].beater_modes[i] = 4;
                    restart = true;
                }
            }
            if restart {
                self.restart_beater(t, i, &db);
            }
            let mode = self.teams[t].beater_modes[i];
            if mode == 2 || mode == 3 || self.players[p].cmd.is_some() {
                continue;
            }
            if self.teams[t].beater_runners[i].finished() {
                self.restart_beater(t, i, &db);
            }
            let input = if mode == 4 || mode == 5 {
                Some(self.block_point(t))
            } else {
                self.beater_reference(t).map(|r| (self.players[r].pos, self.players[r].vel))
            };
            let pl = &self.players[p];
            let body = Body { pos: pl.pos, vel: pl.vel, idle: true };
            if let Some((pos, vel)) = self.teams[t].beater_runners[i].run(input, body) {
                self.command(p, pos, vel, 1);
            }
        }
        if incoming {
            self.beater_block(t);
        }
    }

    /// The block point (`0x440c50`, **[I]** reduced): it follows the team's active player at 1.25 units a tick
    /// for the AI, 0.75 for a human (`0x668f64`, `0x668f60`).
    fn block_point(&mut self, t: usize) -> (Vec3, Vec3) {
        let (p, _) = self.teams[t].block_point;
        let goal = self.teams[t].active.map_or(p, |a| self.players[a].pos);
        let speed = if self.teams[t].human { 0.75 } else { 1.25 };
        let d = sub(goal, p);
        let n = if len(d) > speed { add(p, scale(unit(d), speed)) } else { goal };
        self.teams[t].block_point = (n, scale(sub(n, p), TICKS));
        self.teams[t].block_point
    }

    /// The block (`0x4405c0`, `0x423880`): once the opponents' Bludger has been coming long enough, a beater within
    /// `Beater.block_dist` of it bats it away and ends their move.
    fn beater_block(&mut self, t: usize) {
        let opp = 1 - t;
        if !self.teams[opp].bludger.is_some_and(|m| m.flying) || self.teams[t].block_count <= bludger::BLOCK_AFTER {
            return;
        }
        let b = self.bludgers[opp].pos;
        let reach = self.bludger_rules.block_dist;
        let Some(beater) = self.teams[t]
            .beaters
            .into_iter()
            .find(|&p| self.players[p].present && len(sub(b, self.players[p].pos)) < reach)
        else {
            return;
        };
        self.end_bludger(opp);
        let bv = self.bludgers[opp].vel;
        let ((end, away), swing) = bludger::bat(b, bv, self.players[beater].pos);
        let bl = &mut self.bludgers[opp];
        bl.cmd = Some(Command {
            start: self.tick,
            end: self.tick + bludger::BAT_TICKS as u32,
            from: b,
            from_vel: bv,
            to: end,
            to_vel: away,
        });
        bl.idle_after = true;
        self.command(beater, swing, [0.0; 3], bludger::BAT_TICKS);
        self.events.push(Event::BeaterBlock { tick: self.tick, player: beater });
    }

    /// The Bludgers' formotions (`0x430d90`, the end of the common update; in a match or the Beaters' challenge):
    /// idle over the middle, or ready round the team's active player while the team defends with its move
    /// available.
    fn update_bludgers(&mut self) {
        if self.challenge.is_some_and(|k| k != 4) {
            return;
        }
        let Some(db) = self.global_db() else { return };
        use bludger::State;
        for b in 0..2 {
            if !self.bludgers[b].present || matches!(self.bludgers[b].state, State::Summoned | State::Flying) {
                continue;
            }
            let cur = self.bludgers[b].state;
            let want = if self.challenge.is_some() {
                if self.bludgers_allowed {
                    State::Ready
                } else {
                    State::Idle
                }
            } else if self.ball_team == Some(1 - b) && self.holder.is_some() {
                if self.bludger_available(b) {
                    State::Ready
                } else {
                    cur
                }
            } else {
                State::Idle
            };
            let changed = want != cur;
            self.bludgers[b].state = want;
            let restart = changed || self.bludgers[b].runner.finished();
            let idle = self.bludgers[b].cmd.is_none();
            if !idle && !restart {
                continue;
            }
            let reference = self.teams[b].active.filter(|&a| self.players[a].present);
            let follow =
                reference.map_or((self.quaffle, self.quaffle_vel), |a| (self.players[a].pos, self.players[a].vel));
            if restart {
                let state = if want == State::Ready { 16 } else { 15 };
                let Some(f) = formation::pick_bludger(&mut self.rng, &db, state, 1 << b) else { continue };
                let frame = if want == State::Ready {
                    Frame::new(follow.0, follow.1, bludger::FOLLOW_READY)
                } else {
                    Frame::new(bludger::IDLE_POINT, [0.0; 3], bludger::FOLLOW_IDLE)
                };
                let bl = &self.bludgers[b];
                let body = Body { pos: bl.pos, vel: bl.vel, idle };
                self.bludgers[b].runner.start(
                    f,
                    (bludger::BLUDGER_SCALE, [1.0; 3]),
                    Some(frame),
                    body,
                    bludger::BLUDGER_CAP,
                );
            }
            let input = if want == State::Idle { Some((bludger::IDLE_POINT, [0.0; 3])) } else { Some(follow) };
            let bl = &self.bludgers[b];
            let body = Body { pos: bl.pos, vel: bl.vel, idle };
            if let Some((pos, vel)) = self.bludgers[b].runner.run(input, body) {
                let bl = &mut self.bludgers[b];
                bl.cmd = Some(Command {
                    start: self.tick,
                    end: self.tick + 1,
                    from: bl.pos,
                    from_vel: bl.vel,
                    to: pos,
                    to_vel: vel,
                });
            }
        }
    }

    /// The Bludgers' movers: a pending command through the Bezier mover; a bat's ends in the idle state.
    fn move_bludgers(&mut self) {
        for b in &mut self.bludgers {
            let Some(c) = b.cmd else { continue };
            let span = (c.end - c.start).max(1) as f64;
            let f = ((self.tick + 1 - c.start) as f64 / span).min(1.0);
            let (pos, vel) = keeper::bezier(c.from, c.from_vel, c.to, c.to_vel, span / TICKS, f);
            b.pos = pos;
            b.vel = vel;
            if self.tick + 1 >= c.end {
                b.cmd = None;
                if std::mem::take(&mut b.idle_after) {
                    b.state = bludger::State::Idle;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(seed: u32, ticks: u32) -> Match {
        let mut m = Match::new(&Console::new(), Rand::new(seed));
        for _ in 0..ticks {
            m.step();
        }
        m
    }

    #[test]
    fn the_seekers_lap_the_spline_on_opposite_sides_before_the_race() {
        let mut m = Match::new(&Console::new(), Rand::new(3));
        // eight points round a square of side 40, 30 up
        let pts = [[0, 0], [20, 0], [40, 0], [40, 20], [40, 40], [20, 40], [0, 40], [0, 20]];
        let points = pts
            .iter()
            .map(|&[x, z]| crate::spline::Point { pos: [x as f64, 30.0, z as f64], rot: [0.0, 0.0, 0.0, 1.0] })
            .collect();
        m.use_spline(Spline::new(points, 1.0));
        m.step();
        let s = [m.teams[0].seeker, m.teams[1].seeker].map(|i| m.players[i].clone());
        // a point a tick from 4 and 0: points 5 and 1, facing two points on
        assert_eq!(s[0].pos, [20.0, 30.0, 40.0]);
        assert_eq!(s[1].pos, [20.0, 30.0, 0.0]);
        let h = std::f64::consts::FRAC_1_SQRT_2;
        assert!((s[1].fwd[0] - h).abs() < 1e-9 && (s[1].fwd[2] - h).abs() < 1e-9, "{:?}", s[1].fwd);
        assert_eq!(s[1].vel, [480.0, 0.0, 0.0]);
        for _ in 0..8 {
            m.step();
        }
        // a lap of eight ticks
        assert_eq!(m.players[m.teams[1].seeker].pos, [20.0, 30.0, 0.0]);
    }

    #[test]
    fn a_human_carrier_flies_on_the_stick_and_passes_on_release() {
        let mut m = Match::new(&Console::new(), Rand::new(3));
        m.set_human(0, true);
        let c = m.holder.unwrap();
        let start = m.players[c].pos;
        // the stick along +z for a second: the carrier goes that way (team 0 attacks -x, so this is across)
        let across = Input { stick: [0.0, 0.0, 1.0], ..Input::default() };
        for _ in 0..24 {
            m.set_input(0, across);
            m.step();
        }
        assert_eq!(m.holder, Some(c), "a human never passes by himself");
        assert!(m.players[c].pos[2] - start[2] > 5.0, "{:?} -> {:?}", start, m.players[c].pos);
        assert!(!m.events.iter().any(|e| matches!(e, Event::Pass { .. } | Event::WindUp { .. })));
        // Action held for 10 ticks, then let go: one pass, to a mate
        let mut passes = 0;
        for k in 0..40 {
            let held = k < 10;
            m.set_input(0, Input { action_pressed: k == 0, action_held: held, ..Input::default() });
            m.step();
            passes = m.events.iter().filter(|e| matches!(e, Event::Pass { .. })).count();
            if passes > 0 {
                break;
            }
        }
        assert_eq!(passes, 1);
        let Some(Event::Pass { from, receiver, .. }) = m.events.iter().rev().find(|e| matches!(e, Event::Pass { .. }))
        else {
            unreachable!()
        };
        assert_eq!((*from, m.players[*receiver].team), (c, 0));
        // one wind-up for it, before the pass (its clip)
        let wind_ups: Vec<u32> = m
            .events
            .iter()
            .filter_map(|e| match *e {
                Event::WindUp { tick, player } if player == c => Some(tick),
                _ => None,
            })
            .collect();
        let pass_tick = m.events.iter().find_map(|e| match *e {
            Event::Pass { tick, .. } => Some(tick),
            _ => None,
        });
        assert_eq!(wind_ups.len(), 1);
        assert!(pass_tick.is_some_and(|p| wind_ups[0] < p), "{wind_ups:?} before {pass_tick:?}");
    }

    /// The dodges' commands against the original's, run under an emulator (`tools/check_dodges.py`, the cruising
    /// height stubbed at 5): positions to 4 decimals, the ticks exact.
    #[test]
    fn the_dodges_lay_the_originals_commands() {
        let legs = |kind, pos, vel, fwd| Match::dodge_legs(pos, vel, fwd, 1000, kind, |_| 5.0);
        let close = |a: Vec3, b: [f64; 3]| a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 2e-3);
        let check = |kind, pos, vel, fwd, want: &[(u32, u32, [f64; 3], [f64; 3])]| {
            let got = legs(kind, pos, vel, fwd);
            assert_eq!(got.len(), want.len(), "kind {kind}");
            for (c, w) in got.iter().zip(want) {
                assert_eq!((c.start, c.end), (w.0, w.1), "kind {kind}");
                assert!(close(c.to, w.2) && close(c.to_vel, w.3), "kind {kind}: {c:?} against {w:?}");
            }
        };
        let (p, v, f) = ([10.0, 20.0, 5.0], [12.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        check(
            6,
            p,
            v,
            f,
            &[
                (1000, 1007, [14.5, 17.7386, 7.5], [12.0, -2.2727, 0.0]),
                (1007, 1014, [19.0, 15.4773, 6.0], [12.0, -11.3636, -9.0909]),
                (1014, 1021, [23.5, 9.2159, 4.5], [12.0, -20.4545, 0.0]),
                (1021, 1028, [28.0, 2.9545, 7.0], [12.0, -11.3636, 9.0909]),
            ],
        );
        check(
            7,
            [-30.0, 6.0, 12.0],
            [0.0, 3.0, -18.0],
            [0.0, 0.0, -1.0],
            &[
                (1000, 1007, [-32.5, 7.7159, 5.25], [0.0, 8.3333, -18.0]),
                (1007, 1014, [-31.0, 9.4318, -1.5], [9.0909, -0.7576, -18.0]),
                (1014, 1021, [-29.5, 7.1477, -8.25], [0.0, -9.8485, -18.0]),
                (1021, 1028, [-32.0, 4.8636, -15.0], [-9.0909, -0.7576, -18.0]),
            ],
        );
        check(
            8,
            p,
            v,
            f,
            &[
                (1000, 1007, [15.0, 24.0, 5.0], [0.0, 20.0, 0.0]),
                (1007, 1014, [10.0, 28.0, 5.0], [-20.0, 0.0, 0.0]),
                (1014, 1021, [5.0, 24.0, 5.0], [0.0, -20.0, 0.0]),
                (1021, 1036, [10.0, 20.0, 5.0], [15.0, 0.0, 0.0]),
            ],
        );
        check(
            9,
            [-30.0, 6.0, 12.0],
            [0.0, 3.0, -18.0],
            [0.0, 0.0, -1.0],
            &[(1000, 1015, [-30.0, 5.0, 2.0], [0.0, 0.0, -25.0])],
        );
    }

    /// A dodge moves the carrier off his line on its commands, with the Quaffle, and empties the dodge sub-bar.
    #[test]
    fn a_dodging_carrier_follows_his_move() {
        let mut m = Match::new(&Console::new(), Rand::new(3));
        let c = m.holder.expect("the kick-off's holder");
        let t = m.players[c].team;
        m.power.sub[t][powerbar::DODGE] = 0.5;
        m.players[c].fwd = [1.0, 0.0, 0.0];
        m.players[c].vel = [10.0, 0.0, 0.0];
        m.start_dodge(t, c, 8);
        assert_eq!(m.power.sub[t][powerbar::DODGE], 0.0);
        let start = m.players[c].pos;
        let mut top = start[1];
        for _ in 0..36 {
            m.move_players();
            m.tick += 1;
            top = top.max(m.players[c].pos[1]);
        }
        // over the loop's top (8 up) and home again
        assert!(top - start[1] > 7.5, "{top} from {}", start[1]);
        assert!(len(sub(m.players[c].pos, start)) < 0.5, "{:?} from {start:?}", m.players[c].pos);
        assert!(m.players[c].cmd.is_none() && m.players[c].queue.is_empty());
    }

    #[test]
    fn a_charged_human_carrier_dodges_with_a_cooldown() {
        let mut m = Match::new(&Console::new(), Rand::new(3));
        m.set_human(0, true);
        let dodges = |m: &Match| m.events.iter().filter(|e| matches!(e, Event::Dodge { .. })).count();
        let press = Input { dodge_pressed: true, ..Input::default() };
        // an empty dodge sub-bar (a pot above 0) stops it
        m.power.pots[0][powerbar::DODGE] = 0.1;
        m.set_input(0, press);
        m.step();
        assert_eq!(dodges(&m), 0);
        m.power.sub[0][powerbar::DODGE] = 0.1;
        m.set_input(0, press);
        m.step();
        assert_eq!(dodges(&m), 1);
        assert!(m.teams[0].tackle.dodging);
        // the dodge emptied the sub-bar (`0x436790`); refilled, a press at once is held off by the cooldown for
        // 14 ticks
        assert_eq!(m.power.sub[0][powerbar::DODGE], 0.0);
        for _ in 0..DODGE_COOLDOWN - 1 {
            m.power.sub[0][powerbar::DODGE] = 0.1;
            m.set_input(0, press);
            m.step();
        }
        assert_eq!(dodges(&m), 1);
        assert!(!m.teams[0].tackle.dodging);
        m.power.sub[0][powerbar::DODGE] = 0.1;
        m.set_input(0, press);
        m.step();
        assert_eq!(dodges(&m), 2);
    }

    #[test]
    fn a_full_special_bar_in_the_goal_area_makes_the_special_move() {
        let mut m = Match::new(&Console::new(), Rand::new(3));
        m.set_human(0, true);
        let specials = |m: &Match| m.events.iter().filter(|e| matches!(e, Event::SpecialMove { .. })).count();
        let press = Input { shoot_pressed: true, boost_held: true, special: true, ..Input::default() };
        m.power.pots[0][powerbar::SPECIAL] = 0.2;
        m.power.unlocked[powerbar::SPECIAL] = true;
        let c = m.holder.expect("a kick-off holder");
        let place = |m: &mut Match| m.players[c].pos = [m.teams[0].attack_dir() * 80.0, 25.0, 0.0];
        // an empty sub-bar: nothing
        place(&mut m);
        m.set_input(0, press);
        m.step();
        assert_eq!(specials(&m), 0);
        // full, but outside the goal area: nothing
        m.power.sub[0][powerbar::SPECIAL] = 0.2;
        m.players[c].pos = [0.0, 25.0, 0.0];
        m.set_input(0, press);
        m.step();
        assert_eq!(specials(&m), 0);
        // full in the goal area: the move, a goal at once, the sub-bar emptied
        place(&mut m);
        m.set_input(0, press);
        m.step();
        assert_eq!(specials(&m), 1);
        assert_eq!(m.teams[0].score, 10);
        assert_eq!(m.power.sub[0][powerbar::SPECIAL], 0.0);
        assert!(m.events.iter().any(|e| matches!(e, Event::Goal { team: 0, .. })));
        assert_eq!(m.holder, None);
    }

    #[test]
    fn the_ai_weighs_its_special_move_once_the_counter_is_past_48() {
        let mut m = Match::new(&Console::new(), Rand::new(3));
        let specials = |m: &Match| m.events.iter().filter(|e| matches!(e, Event::SpecialMove { .. })).count();
        m.power.pots[0][powerbar::SPECIAL] = 0.2;
        m.power.unlocked[powerbar::SPECIAL] = true;
        m.power.sub[0][powerbar::SPECIAL] = 0.2;
        m.teams[0].special_shoot_weight = 1.0;
        // a chaser carries (the keeper's possession weighs no moves); the counter counts the ticks with the ball
        let c = m.teams[0].chasers[0];
        m.attach(c);
        let at = |m: &mut Match| m.players[c].pos = [m.teams[0].attack_dir() * 80.0, 25.0, 0.0];
        at(&mut m);
        m.step();
        assert_eq!((m.teams[0].c1, specials(&m)), (1, 0));
        // in the goal area with no wind-up running: with the counter at 48 nothing; past it the move (the weight 1
        // never misses); the tick counts itself first
        at(&mut m);
        m.teams[0].shot = ShotPlan::default();
        m.teams[0].c1 = 47;
        m.step();
        assert_eq!(specials(&m), 0);
        at(&mut m);
        m.teams[0].shot = ShotPlan::default();
        m.teams[0].c1 = 48;
        m.step();
        assert_eq!(specials(&m), 1);
        assert_eq!(m.teams[0].score, 10);
    }

    #[test]
    fn a_full_uber_bar_makes_the_uber_move_anywhere_with_the_ball() {
        let mut m = Match::new(&Console::new(), Rand::new(3));
        m.set_human(0, true);
        let ubers = |m: &Match| m.events.iter().filter(|e| matches!(e, Event::UberMove { .. })).count();
        let press = Input { uber_pressed: true, ..Input::default() };
        m.power.pots[0][powerbar::UBER] = 0.4;
        m.power.main[1] = 0.5;
        m.teams[0].uber_penalty = 0.2;
        let c = m.holder.expect("a kick-off holder");
        m.players[c].pos = [0.0, 25.0, 0.0];
        // full but locked: nothing
        m.power.sub[0][powerbar::UBER] = 0.4;
        m.set_input(0, press);
        m.step();
        assert_eq!(ubers(&m), 0);
        // unlocked (the bar filled again: a locked sub-bar empties): the move at mid-pitch, a goal, the sub-bar
        // emptied and the other team's bar penalised
        m.power.uber[0] = true;
        m.power.sub[0][powerbar::UBER] = 0.4;
        m.set_input(0, press);
        m.step();
        assert_eq!(ubers(&m), 1);
        assert_eq!(m.teams[0].score, 10);
        assert_eq!(m.power.sub[0][powerbar::UBER], 0.0);
        assert!((m.power.main[1] - 0.3).abs() < 1e-9);
        assert_eq!(m.holder, None);
    }

    #[test]
    fn the_defensive_special_takes_the_ball_after_its_run() {
        let mut m = Match::new(&Console::new(), Rand::new(3));
        // team 1 human, defending team 0's kick-off holder once the recharge has run out
        m.set_human(1, true);
        m.power.pots[1][powerbar::SPECIAL] = 0.2;
        m.power.unlocked[powerbar::SPECIAL] = true;
        m.power.sub[1][powerbar::SPECIAL] = 0.2;
        let press = Input { special_pressed: true, ..Input::default() };
        let tackles = |m: &Match| m.events.iter().filter(|e| matches!(e, Event::SpecialTackle { .. })).count();
        let mut started = None;
        for i in 0..600 {
            let carrier = m.holder.filter(|&h| m.players[h].team == 0 && m.players[h].role != Role::Keeper);
            m.set_input(1, if carrier.is_some() && started.is_none() { press } else { Input::default() });
            m.step();
            if started.is_none() && m.teams[1].special_tackle.is_some() {
                started = Some(i);
            }
            if tackles(&m) > 0 {
                break;
            }
        }
        let started = started.expect("the special started");
        assert_eq!(tackles(&m), 1);
        assert!(m.holder.is_some_and(|h| m.players[h].team == 1));
        assert_eq!(m.power.sub[1][powerbar::SPECIAL], 0.0);
        let at = m.events.iter().find_map(|e| match e {
            Event::SpecialTackle { tick, .. } => Some(*tick),
            _ => None,
        });
        assert!(at.is_some_and(|t| t as i64 - started as i64 >= SPECIAL_TACKLE_TICKS as i64 - 2));
    }

    /// The AI's run (state 0x1e): the lunge first, then the count of 48, cut short 6 ticks after the Quaffle comes
    /// within 1.5 of the runner.
    #[test]
    fn the_ais_special_lunges_and_tackles_when_its_count_runs_out_or_in_reach() {
        let tackles = |m: &Match| m.events.iter().filter(|e| matches!(e, Event::SpecialTackle { .. })).count();
        let setup = || {
            let mut m = Match::new(&Console::new(), Rand::new(3));
            let carrier = m.holder.expect("the kick-off's holder");
            let t = 1 - m.players[carrier].team;
            let a = m.teams[t].chasers[0];
            m.teams[t].active = Some(a);
            m.players[a].cmd = None;
            m.players[a].pos = add(m.players[carrier].pos, [20.0, 0.0, 0.0]);
            m.teams[t].special_tackle = Some(SpecialRun::new(true));
            (m, t, a, carrier)
        };
        // out of reach: the lunge's command (10 ticks, to 6 short of the carrier), then the whole count
        let (mut m, t, a, carrier) = setup();
        assert!(m.run_special_tackle(t));
        let cmd = m.players[a].cmd.expect("the lunge");
        assert_eq!(cmd.end - cmd.start, LUNGE_TICKS as u32);
        assert!((len(sub(cmd.to, m.players[carrier].pos)) - LUNGE_SHORT).abs() < 1e-9);
        for _ in 1..SPECIAL_TACKLE_TICKS {
            m.players[a].pos = add(m.players[carrier].pos, [20.0, 0.0, 0.0]);
            m.run_special_tackle(t);
        }
        assert_eq!(tackles(&m), 1, "on the 48th tick");
        // in reach of the Quaffle: the tackle on the sixth tick there
        let (mut m, t, a, carrier) = setup();
        m.run_special_tackle(t);
        for k in 1..=6 {
            m.players[a].pos = add(m.players[carrier].pos, [1.0, 0.0, 0.0]);
            m.quaffle = m.players[carrier].pos;
            m.run_special_tackle(t);
            assert_eq!(tackles(&m), usize::from(k == 6), "tick {k} in reach");
        }
    }

    /// The carrier's state 0x0f picks 0x12 (the weave 0x15) while the opponents' defensive special runs, and 0x13
    /// (0x16) while their Bludger move does: no stick before the launch, then straight on, then the turns.
    #[test]
    fn the_carrier_weaves_from_the_opponents_specials() {
        let mut m = Match::new(&Console::new(), Rand::new(3));
        let carrier = m.holder.expect("the kick-off's holder");
        let t = m.players[carrier].team;
        let dir = m.teams[t].attack_dir();
        m.teams[1 - t].special_tackle = Some(SpecialRun { left: 1000, ..SpecialRun::new(false) });
        m.update_carrier(t);
        assert_eq!(m.teams[t].state, Some(CarrierState::Weave));
        let want = unit([dir, 0.0, f64::from(sin(0.1f32))]);
        assert_eq!(m.teams[t].call.map(|c| c.stick), Some(want));
        m.update_carrier(t);
        assert!((m.weave_phase - 0.2).abs() < 1e-6);
        // the Bludger move: summoned, then launched
        m.teams[1 - t].special_tackle = None;
        m.start_bludger(1 - t, false);
        m.update_carrier(t);
        assert_eq!(m.teams[t].state, Some(CarrierState::BludgerWeave { n: 0 }));
        assert_eq!(m.teams[t].call.map(|c| c.stick), Some([0.0; 3]));
        m.teams[1 - t].bludger.as_mut().unwrap().flying = true;
        for _ in 0..13 {
            m.update_carrier(t);
        }
        // n 0 .. 24 straight on; at 26 the first turn
        assert_eq!(m.teams[t].call.map(|c| c.stick), Some([dir, 0.0, 0.0]));
        m.update_carrier(t);
        let side = 1.5 * sin(2.0f32 * std::f32::consts::TAU / 120.0);
        assert_eq!(m.teams[t].call.map(|c| c.stick), Some(unit([dir, 0.0, f64::from(side)])));
        // over: possession again
        m.teams[1 - t].bludger = None;
        m.update_carrier(t);
        assert!(matches!(m.teams[t].state, Some(CarrierState::Possession { .. })));
    }

    #[test]
    fn a_human_defender_tackles_on_action() {
        let mut m = Match::new(&Console::new(), Rand::new(5));
        m.set_human(1, true);
        // team 1's nearest chaser sits on the carrier; Action pressed every tick
        let c = m.holder.unwrap();
        for _ in 0..48 {
            let near = m.nearest_chaser(1, m.players[c].pos, usize::MAX).unwrap();
            m.players[near].pos = add(m.players[c].pos, [0.5, 0.0, 0.0]);
            m.set_input(1, Input { action_pressed: true, ..Input::default() });
            m.step();
            if m.events.iter().any(|e| matches!(e, Event::Tackle { .. } | Event::Lunge { .. } | Event::Whiff { .. })) {
                return;
            }
        }
        panic!("no tackle: {:?}", m.events);
    }

    #[test]
    fn a_human_defender_is_kept_on_the_ball() {
        let mut m = Match::new(&Console::new(), Rand::new(5));
        m.set_human(1, true);
        let c = m.holder.unwrap();
        // team 1's chasers 30 away from the carrier, who stays put
        for &p in &m.teams[1].chasers.clone() {
            m.players[p].pos = add(m.players[c].pos, [0.0, 0.0, 30.0 + p as f64]);
        }
        let mut active = None;
        for _ in 0..24 {
            m.players[c].vel = [0.0; 3];
            m.set_input(1, Input::default());
            m.step();
            if m.holder != Some(c) {
                break;
            }
            // the same player throughout: no other chaser takes over while the opponents hold the ball
            let a = m.teams[1].active.unwrap();
            assert_eq!(*active.get_or_insert(a), a);
        }
        // the rope of 5 drew him in, 2 units a tick at first, with the stick at rest
        let a = active.unwrap();
        let d = len(sub(m.players[a].pos, m.players[m.holder.unwrap_or(c)].pos));
        assert!(d < 8.0, "{d}");
    }

    #[test]
    fn a_fumbled_quaffle_stays_its_teams_and_only_the_other_can_take_it() {
        let mut m = Match::new(&Console::new(), Rand::new(3));
        let c = m.holder.unwrap();
        let mate = m.teams[0].chasers[0];
        m.teams[0].hit = Some((c, [10.0, 0.0, 0.0]));
        m.take_hit(0);
        assert!(m.events.iter().any(|e| matches!(e, Event::Fumble { .. })));
        assert_eq!((m.holder, m.ball_team), (None, Some(0)));
        // a team-mate on the ball cannot take it
        m.players[mate].pos = m.quaffle;
        m.armed[mate] = true;
        m.contacts();
        assert!(!m.events.iter().any(|e| matches!(e, Event::Touch { player, .. } if *player == mate)));
        // an opponent can
        let o = m.teams[1].chasers[0];
        m.players[o].pos = m.quaffle;
        m.armed[o] = true;
        m.contacts();
        assert!(m.events.iter().any(|e| matches!(e, Event::Touch { player, .. } if *player == o)));
    }

    /// Runs a kick-off to `ticks`, team 0 pressing Action on `press` when it is a human's.
    fn kick_off(human: bool, press: u32, ticks: u32) -> Match {
        let mut m = Match::new(&Console::new(), Rand::new(3));
        m.set_human(0, human);
        m.start_toss();
        assert_eq!((m.holder, m.ball_team), (None, None));
        while m.tick < ticks {
            m.set_input(0, Input { action_pressed: m.tick == press, ..Input::default() });
            m.step();
        }
        m
    }

    fn toss(m: &Match) -> Option<(u32, usize)> {
        m.events.iter().find_map(|e| match *e {
            Event::Toss { tick, team, .. } => Some((tick, team)),
            _ => None,
        })
    }

    #[test]
    fn the_ai_takes_the_kick_off_at_60_ticks() {
        let m = kick_off(false, u32::MAX, 80);
        // team 0 updates first with the ball in no list, and claims when the clock reaches 60
        assert_eq!(toss(&m), Some((60, 0)));
        // its centre chaser takes the toss 15 ticks on, after the toss at 70 (the catch's tick is the one before)
        let c = m.teams[0].chasers[1];
        assert!(m.events.contains(&Event::Catch { tick: 74, player: c }), "{:?}", m.events);
        assert_eq!((m.holder.is_some(), m.ball_team, m.kickoff), (true, Some(0), None));
        // the other side's player was sent under the centre, then 3 towards the end he attacks
        let o = m.teams[1].chasers[1];
        let last = m.players[o].queue.last().or(m.players[o].cmd.as_ref()).map(|c| c.to);
        assert!(last.is_some_and(|to| (to[0] - 3.0).abs() < 1e-9 && (to[2] - 1.0).abs() < 1e-9), "{last:?}");
    }

    #[test]
    fn a_human_wins_the_kick_off_in_the_window_and_loses_it_early() {
        // pressed at 52: in the window, his
        let m = kick_off(true, 52, 80);
        assert_eq!(toss(&m), Some((52, 0)));
        assert_eq!(m.ball_team, Some(0));
        // the take waits for the toss
        assert!(m.events.contains(&Event::Catch { tick: 69, player: m.teams[0].chasers[1] }));
        // pressed at 46: a false start, and the AI's
        let m = kick_off(true, 46, 80);
        assert_eq!(toss(&m), Some((46, 1)));
        assert_eq!(m.ball_team, Some(1));
        // before 45 a press does nothing; the AI claims at 60
        let m = kick_off(true, 30, 80);
        assert_eq!(toss(&m), Some((60, 1)));
    }

    #[test]
    fn the_toss_rises_and_falls_and_the_window_opens_once() {
        // from 1.5 up, rising, back to the floor at the toss, on its way down when the window opens
        assert!((toss_height(0) - 1.5).abs() < 1e-9);
        let peak = (0..=KICK_TOSS).map(toss_height).fold(f64::MIN, f64::max);
        assert!((3.5..4.0).contains(&peak), "{peak}");
        assert!(toss_height(KICK_WINDOW) < toss_height(KICK_WINDOW - 1));
        assert!(toss_height(KICK_TOSS).abs() < 1e-9 && toss_height(KICK_TOSS + 10).abs() < 1e-9);
        // the match's Quaffle follows it, and the window's message comes once, at 50
        let m = kick_off(false, u32::MAX, 80);
        let windows: Vec<_> = m.events.iter().filter(|e| matches!(e, Event::KickOffWindow { .. })).collect();
        assert_eq!(windows, [&Event::KickOffWindow { tick: KICK_WINDOW }]);
        let mut m = Match::new(&Console::new(), Rand::new(7));
        m.start_toss();
        let floor = m.pitch.floor([0.0; 3]);
        for _ in 0..20 {
            m.step();
        }
        assert!((m.quaffle[1] - floor - toss_height(20)).abs() < 1e-9, "{:?}", m.quaffle);
    }

    #[test]
    fn a_minute_of_passing_is_deterministic() {
        let (a, b) = (run(7, 24 * 60), run(7, 24 * 60));
        assert_eq!(a.events, b.events);
        assert_eq!(a.rng, b.rng);
        let passes: Vec<_> = a.events.iter().filter(|e| matches!(e, Event::Pass { .. })).collect();
        assert!(passes.len() > 5, "only {} passes", passes.len());
        assert_ne!(run(8, 24 * 60).events, a.events);
    }

    #[test]
    fn the_pass_target_is_a_mate_in_the_cone_until_the_throw() {
        let mut m = Match::new(&Console::new(), Rand::new(7));
        let mut seen = 0;
        for _ in 0..24 * 60 {
            let before = m.events.len();
            m.step();
            for (t, team) in m.teams.iter().enumerate() {
                if let Some(r) = team.pass_target {
                    seen += 1;
                    assert_eq!(m.players[r].team, t);
                    assert_ne!(m.holder, Some(r));
                }
            }
            // a throw this tick leaves its team without a target
            for e in &m.events[before..] {
                if let Event::Pass { from, .. } = e {
                    assert_eq!(m.teams[m.players[*from].team].pass_target, None);
                }
            }
        }
        assert!(seen > 0, "no pass was aimed with its receiver in the cone");
    }

    #[test]
    fn the_ball_is_always_held_or_flying() {
        let mut m = Match::new(&Console::new(), Rand::new(3));
        for _ in 0..24 * 30 {
            m.step();
            assert!(m.holder.is_some() != m.flight.is_some());
            if let Some(f @ Flight { kind: FlightKind::Pass { .. }, .. }) = m.flight {
                assert!(f.end_tick - f.start_tick >= 6);
            }
        }
    }

    #[test]
    fn flight_lifts_the_ball_in_the_middle() {
        let mut m = Match::new(&Console::new(), Rand::new(1));
        let (from, receiver) = (m.players[m.teams[0].chasers[1]].pos, m.teams[0].chasers[2]);
        m.holder = None;
        m.flight = Some(Flight {
            flat: false,
            kind: FlightKind::Pass { receiver },
            start_tick: 0,
            end_tick: 24,
            from,
            to: [0.0; 3],
        });
        for _ in 0..11 {
            m.update_quaffle();
            m.tick += 1;
        }
        m.update_quaffle();
        // halfway: half the way along, lifted by 6 * 1 s * 0.25
        assert!((m.quaffle[1] - (from[1] / 2.0 + 1.5)).abs() < 1e-9);
        assert!((m.quaffle[0] - from[0] / 2.0).abs() < 1e-9);
    }

    /// Keeps player `p` where he is for 100 ticks (a movement command to his own position).
    fn hold(m: &mut Match, p: usize) {
        let at = m.players[p].pos;
        m.players[p].cmd =
            Some(Command { start: m.tick, end: m.tick + 100, from: at, from_vel: [0.0; 3], to: at, to_vel: [0.0; 3] });
    }

    /// A match with team 0's middle chaser shooting at hoop 4 from `from` at tick 0, team 1 moved out of the way
    /// except for the players the caller puts back.
    fn shot_from(from: Vec3) -> (Match, u32) {
        let mut m = Match::new(&Console::new(), Rand::new(1));
        for p in 0..m.players.len() {
            if m.players[p].team == 1 {
                m.players[p].pos = [0.0, 0.0, 300.0];
            }
        }
        let c = m.teams[0].chasers[1];
        m.players[c].pos = from;
        m.teams[0].shot = ShotPlan { left: 1, total: 12, hoop: Some(4), power: 11.0 / 12.0 };
        m.shoot(0, c);
        let Some(Flight { end_tick, .. }) = m.flight else { panic!("no shot") };
        (m, end_tick)
    }

    #[test]
    fn an_unopposed_shot_scores_and_the_other_team_kicks_off() {
        let (mut m, end) = shot_from([-75.0, 20.0, 0.0]);
        // 22.5 units in x, 7.5 up: 23.7 units at 29.17 u/s = 19.5 ticks
        assert_eq!(end, 19);
        while m.tick < end + 24 {
            m.step();
        }
        let goals: Vec<_> = m.events.iter().filter(|e| matches!(e, Event::Goal { .. })).collect();
        assert_eq!(goals, [&Event::Goal { tick: end - 1, team: 0, hoop: 4, combo: false }]);
        assert_eq!(m.teams[0].score, 10);
        // the drop lasts 24 ticks from `end`; the kick-off is the type-7 event at `end + 23`
        assert!(m.events.contains(&Event::KickOff { tick: end + 23, team: 1 }));
        assert_eq!(m.holder, Some(m.teams[1].keeper));
        // placed across the pitch as the table says, at the cruising height
        let (a, b) = (m.players[m.teams[1].chasers[0]].pos, m.players[m.teams[0].chasers[0]].pos);
        assert_eq!([a[0], a[2], b[0], b[2]], [-70.0, 3.5, -28.0, -3.5]);
        assert!([a[1], b[1]].iter().all(|y| (20.0..=25.0).contains(y)), "{a:?} {b:?}");
    }

    #[test]
    fn a_defender_in_the_path_saves() {
        let (mut m, end) = shot_from([-75.0, 20.0, 0.0]);
        // a chaser waits where the ball will be four ticks before arrival (placeholder movement off)
        let k = m.teams[1].chasers[0];
        m.max_speed = 0.0;
        m.handling.acceleration = 0.0;
        let Some(fl) = m.flight else { unreachable!() };
        let f = (end - 4 - fl.start_tick) as f64 / (end - fl.start_tick) as f64;
        let mut at = add(fl.from, scale(sub(fl.to, fl.from), f));
        at[1] += 6.0 * (end as f64 / 24.0) * f * (1.0 - f);
        m.players[k].pos = at;
        hold(&mut m, k);
        while m.tick < end + 30 {
            m.step();
        }
        let touch = m.events.iter().find_map(|e| match *e {
            Event::Touch { tick, player, state } => Some((tick, player, state)),
            _ => None,
        });
        let (tick, player, state) = touch.expect("no save");
        assert_eq!((player, state), (k, 9));
        assert!(tick < end - 1);
        assert_eq!(m.teams[0].score, 0);
        assert!(!m.events.iter().any(|e| matches!(e, Event::Goal { .. })));
        assert_eq!(m.ball_team, Some(1));
    }

    #[test]
    fn a_shot_cannot_be_touched_on_its_last_ticks() {
        let (mut m, end) = shot_from([-75.0, 20.0, 0.0]);
        // a chaser sits on the hoop itself: the ball reaches him only in state 10
        let k = m.teams[1].chasers[0];
        m.max_speed = 0.0;
        m.handling.acceleration = 0.0;
        m.players[k].pos = m.hoops[4].pos;
        while m.tick < end {
            m.step();
        }
        assert_eq!(m.teams[0].score, 10);
    }

    #[test]
    fn a_pass_is_intercepted_only_by_the_other_team() {
        let mut m = Match::new(&Console::new(), Rand::new(1));
        let (from, receiver) = (m.players[m.teams[0].chasers[1]].pos, m.teams[0].chasers[2]);
        let to = add(from, [-24.0, 0.0, 0.0]);
        m.holder = None;
        m.flight =
            Some(Flight { flat: false, kind: FlightKind::Pass { receiver }, start_tick: 0, end_tick: 24, from, to });
        // a team-mate on the line touches nothing; an opponent half-way along takes it (state 6, radius 0.75)
        let mate = m.teams[0].chasers[0];
        m.players[mate].pos = add(from, [-6.0, 0.6, 0.0]);
        let opp = m.teams[1].chasers[0];
        m.players[opp].pos = add(from, [-12.0, 1.5, 0.0]);
        for p in [mate, opp] {
            m.players[p].vel = [0.0; 3];
            hold(&mut m, p);
        }
        m.max_speed = 0.0;
        m.handling.acceleration = 0.0;
        for _ in 0..24 {
            m.step();
        }
        let touches: Vec<_> = m.events.iter().filter(|e| matches!(e, Event::Touch { .. })).collect();
        assert_eq!(touches.len(), 1);
        assert!(matches!(touches[0], &Event::Touch { player, state: 6, .. } if player == opp));
        assert_eq!(m.ball_team, Some(1));
    }

    #[test]
    fn matches_shoot_and_score() {
        let m = run(1, 24 * 60 * 5);
        let count = |f: fn(&Event) -> bool| m.events.iter().filter(|e| f(e)).count();
        let shots = count(|e| matches!(e, Event::Shot { .. }));
        let goals = count(|e| matches!(e, Event::Goal { .. }));
        assert!(shots > 0 && goals > 0, "{shots} shots, {goals} goals");
        assert_eq!(goals as i32 * 10, m.teams[0].score + m.teams[1].score);
        // every shot follows a wind-up by the same player, and none has a flight shorter than 5 ticks
        for (i, e) in m.events.iter().enumerate() {
            if let Event::Shot { tick, player, goal, .. } = *e {
                assert!(goal + 1 >= tick + 5);
                assert!(m.events[..i]
                    .iter()
                    .rev()
                    .any(|w| matches!(*w, Event::ShotWindUp { player: p, .. } if p == player)));
            }
        }
    }

    #[test]
    fn placements_mirror() {
        assert_eq!(placement(Role::Chaser, 0, 0, 0), [5.0, 0.0, -6.0]);
        assert_eq!(placement(Role::Chaser, 2, 1, 0), [70.0, 0.0, 8.5]);
        assert_eq!(placement(Role::Beater, 1, 2, 0), [-18.0, 20.0, 7.5]);
        assert_eq!(placement(Role::Keeper, 0, 2, 1), [-91.5, 0.0, -3.0]);
        assert_eq!(placement(Role::Chaser, 1, 1, 1), [-70.0, 0.0, -2.5]);
    }

    /// A match where nobody but the keepers moves on his own, team 0's middle chaser holding the ball at the
    /// centre and team 1's first chaser `gap` units in front of him.
    fn standoff(gap: f64) -> (Match, usize, usize) {
        let mut m = Match::new(&Console::new(), Rand::new(1));
        for t in &mut m.teams {
            t.chaser_speed = 0.0;
        }
        // nobody accelerates: the handling keeps resting players at rest
        m.handling.acceleration = 0.0;
        let (c, d) = (m.teams[0].chasers[1], m.teams[1].chasers[0]);
        m.players[c].pos = [0.0; 3];
        m.players[d].pos = [-gap, 0.0, 0.0];
        for p in 0..m.players.len() {
            if p != c && p != d && m.players[p].role == Role::Chaser {
                m.players[p].pos[2] = 60.0 * if m.players[p].team == 0 { 1.0 } else { -1.0 };
            }
        }
        (m, c, d)
    }

    #[test]
    fn a_pressed_slow_carrier_is_tackled() {
        let (mut m, c, d) = standoff(2.0);
        while m.tick < 40 {
            m.step();
        }
        // the press counter passes 18 on the 19th tick (tick 18); within 2.5 the tackle is pressed and succeeds
        let tackle = m.events.iter().find(|e| matches!(e, Event::Tackle { .. }));
        assert_eq!(tackle, Some(&Event::Tackle { tick: 18, tackler: d, victim: c, end: 28, lunge: false }));
        assert!(m.events.contains(&Event::Catch { tick: 27, player: d }));
        assert_eq!(m.ball_team, Some(1));
        // the tackled team is stunned for 25 ticks from the tackle; its stun counts down in its active player's
        // handling, which waits while the victim's moves run (to tick 28)
        assert_eq!(m.teams[0].tackle.stun, 25 - (m.tick as i32 - 28));
        assert!(m.events.iter().all(|e| !matches!(e, Event::Tackle { tackler, .. } if *tackler == c)));
    }

    #[test]
    fn a_far_defender_lunges_on_the_24th_tick() {
        let (mut m, _, d) = standoff(8.0);
        while m.tick < 40 {
            m.step();
        }
        let lunges: Vec<_> = m.events.iter().filter(|e| matches!(e, Event::Lunge { .. })).collect();
        // the counter is 24 on tick 22 (19 skips to 20)
        assert_eq!(lunges.first(), Some(&&Event::Lunge { tick: 22, player: d }));
    }
    /// Tackle 'n' Shoot's description (`match/ctns`): a chaser each.
    fn tackle_n_shoot() -> Match {
        use qwc_formats::matchdesc::{MatchDesc, Player};
        let chaser = |team| Player { kind: "eChaser".into(), team, subtype_index: None };
        let desc = MatchDesc {
            match_type: "Challenge".into(),
            sub_type: Some(2),
            players: vec![chaser(0), chaser(1)],
            balls: vec!["eQuaffle".into()],
        };
        let mut m = Match::new(&Console::new(), Rand::new(3));
        m.use_description(&desc);
        m
    }

    #[test]
    fn tackle_n_shoot_starts_with_edgar_on_the_ball_before_the_only_hoops_in_reach() {
        let m = tackle_n_shoot();
        let (me, edgar) = (m.teams[0].chasers[0], m.teams[1].chasers[0]);
        assert!(m.tns);
        assert_eq!(m.holder, Some(edgar));
        // team 0's hoops 50 times as far out, team 1's at a match's end
        assert!(m.hoops.iter().filter(|h| h.team == 0).all(|h| h.pos[0] == 4875.0));
        assert!(m.hoops.iter().filter(|h| h.team == 1).all(|h| h.pos[0] == -97.5));
        // the challenge's placements, facing each other's way
        let at = |p: usize| [m.players[p].pos[0], m.players[p].pos[2]];
        assert_eq!((at(me), at(edgar)), ([-45.0, -6.0], [-55.0, 6.0]));
        assert_eq!(challenge_placement(Role::Chaser, 0, 1, 0), [-45.0, 0.0, -3.5]);
        assert_eq!(challenge_placement(Role::Chaser, 0, 1, 1), [-55.0, 0.0, 3.5]);
        // Edgar holding the Quaffle attacks `[0x1018fa0]`, 1 at the set-up
        assert_eq!(m.teams[1].attack_dir(), 1.0);
        assert_eq!((m.defence_table.weight[0], m.defence_table.accuracy[0]), (1.0, 0.5));
    }

    #[test]
    fn tackle_n_shoots_attack_direction_keeps_edgar_in_front_of_the_hoops() {
        let mut m = tackle_n_shoot();
        m.quaffle = [5.0, 20.0, 0.0];
        m.pick_attack();
        assert_eq!((m.challenge_dir, m.teams[1].attack_dir()), (-1.0, -1.0));
        m.quaffle = [-60.0, 20.0, 0.0];
        m.pick_attack();
        assert_eq!(m.teams[1].attack_dir(), 1.0);
        // without the ball team 1 attacks +x whatever the pick
        m.quaffle = [5.0, 20.0, 0.0];
        m.pick_attack();
        let me = m.teams[0].chasers[0];
        m.set_holder(me);
        assert_eq!((m.challenge_dir, m.teams[1].attack_dir()), (-1.0, 1.0));
    }

    #[test]
    fn edgar_dashes_to_his_hoops_in_a_second_when_the_carrier_nears_them() {
        let mut m = tackle_n_shoot();
        let (me, edgar) = (m.teams[0].chasers[0], m.teams[1].chasers[0]);
        m.teams[0].human = true;
        m.set_holder(me);
        m.players[me].pos = [-80.0, 22.0, 0.0];
        m.step();
        assert_eq!(m.teams[1].keeper_run, Some(1));
        let cmd = m.players[edgar].cmd.expect("the dash");
        assert_eq!(cmd.end - cmd.start, 24);
        // the mean of his hoops, 8 towards the middle
        assert_eq!([cmd.to[0], cmd.to[2]], [-89.5, 0.0]);
        for _ in 0..24 {
            m.players[me].pos = [-80.0, 22.0, 0.0];
            m.step();
        }
        let d = len(sub(m.players[edgar].pos, cmd.to));
        assert!(d < 3.0, "{d} from his post");
        assert!(m.teams[1].guard_cmd.is_some());
        // out of the area (100.8 round x = -150) he leaves the post
        m.players[me].pos = [-30.0, 22.0, 0.0];
        m.step();
        assert_eq!(m.teams[1].keeper_run, None);
    }

    #[test]
    fn edgars_special_tackle_waits_on_its_timer() {
        let mut m = tackle_n_shoot();
        let (me, edgar) = (m.teams[0].chasers[0], m.teams[1].chasers[0]);
        m.set_holder(me);
        m.teams[1].active = Some(edgar);
        m.teams[1].tackle.recharge = -1;
        // the defence's enter: 96 ticks more on the timer; the choice is held until the count reaches it
        m.defence_tick(1);
        assert_eq!(m.defence_timer, 96);
        m.teams[1].special_tackle = None;
        m.teams[1].defence = Some((0, 0));
        for _ in 0..95 {
            m.defence_tick(1);
        }
        assert_eq!(m.teams[1].defence, Some((0, 95)));
        m.defence_tick(1);
        assert_eq!((m.teams[1].defence, m.defence_timer), (Some((1, 0)), 72));
        assert!(m.teams[1].special_tackle.is_some());
    }
}
