//! A match the front end starts: `Match.vso`'s `General_InitMatch` (`0x4d3cb0`) while the flow is in a match
//! screen (`ExhibitionMatch`, `ChallengePlay`, ...). What it is played with comes from the DataStore the screens
//! before filled: `Team1LoadSet` and `Team2LoadSet` (the teams' load sets: their first package is the team's
//! folder), `StadiumLoadSet` (the stadium's `gfx` package and its hoops'), `SkillLevel`, `NumberOfPlayers` and
//! `ReverseTeams`. The front end keeps running over it (`Match.vso` is the pause menu and the controller messages);
//! the match runs while the game is not paused.
//!
//! **[I]** (the engine's side of the match screen is not traced):
//! - when the Snitch is caught, the scores go to `Team1Score` and `Team2Score` and, 3 seconds on, the match screen
//!   takes its exit 1 (to the results), as the scripts' own `General_RequestExit` would;
//! - `NumberOfPlayers` 1 gives the first team (the second with `ReverseTeams`) to the player, 2 both, else none;
//! - `rand()` is seeded from the front end's tick count.
//!
//! The match's cutscenes (`crate::ccs` picks them, the painter plays them; the match holds meanwhile): the intro
//! before the kick-off (`0x41aa50`, not in a challenge), the catch for the catcher's team and the outro for the
//! winner's (`0x4483e0`; the winner is the side ahead, the catcher's on a draw), after which the results come at once
//! (`eMATCH_FINISH`), a challenge's intro once its instructions are left (`0x45ac60`), its countdown after, and a
//! goal's crowd cutscene when the goal's roll asks for one (`0x4322e0`: the record's for the scoring side, team 0 its
//! `TEAM1`; none in the Seeker game; quick and practice matches celebrate instead). **[I]** the celebration
//! (`Score2`, placed at the goal) and the House Cup's and World Cup's winning cutscenes (`WinHog`, `WinWorld`) are
//! not played. A goal's cutscene waits for the screen to fade to black, and play restarts behind a fade back in
//! (`crate::restart`).
//!
//! For testing, `QWC_MATCH_SPEED=n` plays both teams with the AI, `n` match ticks to the front end's one,
//! `QWC_SEEKER_GAME=1` starts the match in the Seeker game and `QWC_NO_CUTSCENES=1` leaves the cutscenes out.

use std::collections::VecDeque;

use qwc_feck::frontend::Request;
use qwc_formats::flow::name_hash;
use qwc_sim::challenge::{Challenge, Event as ChallengeEvent, Phase};
use qwc_sim::human::Input;
use qwc_sim::world::Event;
use qwc_sim::Match;

use crate::arena::MatchSpec;
use crate::ccs::{self, Kind};
use crate::controls::Controls;
use crate::fe::{load_set, Flow};

/// The ticks between the catch and the exit to the results.
const OUTRO_TICKS: u64 = 72;
/// The last ticks of a challenge's outro fade the picture to black (the remaster's: the results come out of it).
const OUTRO_FADE: u64 = 24;

/// What the DataStore says the match is (England against France at England without a load-set table).
pub(crate) fn spec(flow: &Flow) -> MatchSpec {
    let mem = &flow.vm.mem;
    let get = |k: &str| flow.fe.datastore_u32(mem, name_hash(k)).unwrap_or(0);
    let set = |k: &str| load_set(&flow.sets, get(k));
    let folder = |p: &str| p.split('\\').nth(1).map(str::to_ascii_lowercase);
    let team = |k: &str, default: &str| {
        let s = set(k);
        let p = s.and_then(|s| s.entries.iter().find(|(_, p)| p.to_ascii_lowercase().starts_with("teams\\")));
        p.and_then(|(_, p)| folder(p)).unwrap_or_else(|| default.into())
    };
    let gfx: Vec<String> = set("StadiumLoadSet")
        .map(|s| {
            let gfx = s.entries.iter().filter(|(_, p)| p.to_ascii_lowercase().ends_with("\\gfx"));
            gfx.filter_map(|(_, p)| folder(p)).collect()
        })
        .unwrap_or_default();
    let stadium = gfx.iter().find(|f| !f.starts_with("hoop")).cloned().unwrap_or_else(|| "england".into());
    let hoops = gfx.iter().find(|f| f.starts_with("hoop")).cloned().unwrap_or_else(|| "hoopnorm".into());
    let skill = get("SkillLevel") & 0xff;
    // the stadium load set's `music` package (`audio\music\Intl`, ...)
    let music = set("StadiumLoadSet")
        .and_then(|s| s.entries.iter().find(|(g, _)| g.eq_ignore_ascii_case("music")))
        .and_then(|(_, p)| p.rsplit('\\').next().map(str::to_string))
        .unwrap_or_else(|| crate::audio::match_score(&stadium).into());
    // and its `audio` package (`audio\Stadium\Hog_Sum`, ...)
    let sounds = set("StadiumLoadSet")
        .and_then(|s| s.entries.iter().find(|(g, p)| g.eq_ignore_ascii_case("audio") && p.contains("tadium")))
        .map(|(_, p)| format!("Audio/Stadium/{}", p.rsplit('\\').next().unwrap_or_default()));
    let teams = [team("Team1LoadSet", "england"), team("Team2LoadSet", "france")];
    // the match description: `MatchDescLoadSet`'s `matchdesc` package (`match\cpass`, ...)
    let desc = set("MatchDescLoadSet")
        .and_then(|s| s.entries.iter().find(|(g, _)| g.eq_ignore_ascii_case("matchdesc")))
        .and_then(|(_, p)| folder(p))
        .unwrap_or_else(|| "quidd".into());
    MatchSpec { stadium, hoops, teams, skill, music, sounds, desc }
}

/// A team's uber move cutscene: its `teams/<folder>/teamdescription.txt`'s `uber = "..."`.
fn uber_cutscene(vfs: &qwc_assets::Vfs, folder: &str) -> Option<String> {
    let text = vfs.read(&format!("teams/{folder}/teamdescription.txt")).ok()?;
    let text = String::from_utf8_lossy(&text);
    let line = text.lines().find(|l| l.trim_start().to_ascii_lowercase().starts_with("uber"))?;
    let name = line.split_once('=')?.1.trim().trim_matches('"').trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// `0x4455c0`: the progress element and bits that unlock a player's team's uber move, by its nationality index
/// (`0x48b2e0`'s order): the houses' bits 6-8, the nations' bits 2-4 (none for Edgar's team).
fn uber_word(folder: &str) -> Option<(usize, u16)> {
    const ELEMENT: [usize; 13] = [11, 6, 9, 5, 2, 0, 8, 12, 1, 3, 7, 10, 13];
    let i = crate::commentary::nationality(ccs::team_code(folder))? as usize;
    let house = matches!(i, 4 | 5 | 8 | 9);
    Some((*ELEMENT.get(i)?, if house { 0x1c0 } else { 0x1c }))
}

/// DataStore `0x7da5b162`: set while the challenge's loading screen shows its instructions (`LoadscreenToChallenge`'s
/// `__Initialise`), cleared by its Continue; the challenge game waits for it (`0x45ac60`).
const KEY_CHALLENGE_WAIT: u32 = 0x7da5b162;
/// DataStore `0x51e4112a`, written at a challenge's end with its score (`0x45bd50`).
const KEY_CHALLENGE_SCORE: u32 = 0x51e4112a;

/// A cutscene the match asks for: its folder of `root/cs`, the team it is for (its `TEAM1`, `eTHIS_TEAM`), where
/// it is placed and the event's own player (its role file, `crate::cutscene::Cast::own`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CsRequest {
    pub name: String,
    pub team: usize,
    pub place: crate::cutscene::Place,
    pub own: Option<String>,
    pub other_own: Option<String>,
}

impl CsRequest {
    fn new(name: &str, team: usize) -> CsRequest {
        CsRequest { name: name.to_string(), team, place: crate::cutscene::Place::AsMade, own: None, other_own: None }
    }
}

/// The match in progress.
pub(crate) struct MatchRun {
    pub spec: MatchSpec,
    pub m: Match,
    /// The player's keys (`crate::controls`), with the first gamepad.
    pub controls: Controls,
    /// Player 2's: the second gamepad, for team 1 when both teams are human (`NumberOfPlayers` 2).
    pub controls2: Controls,
    /// The match's time in seconds (its ticks over 24).
    pub time: f64,
    /// When the catch was seen (the front end's tick), and whether the exit was asked for.
    over_at: Option<u64>,
    /// The front end's tick count at the last [`MatchRun::tick`].
    now: u64,
    exited: bool,
    /// `QWC_MATCH_SPEED`: match ticks per front-end tick.
    speed: u32,
    /// The game's routers to the match driver ([`MatchRun::music`]).
    music: crate::audio::MusicRouter,
    /// The input the player's team had this tick.
    pub input: Option<Input>,
    /// The challenge being played (`qwc_sim::challenge`).
    pub challenge: Option<Challenge>,
    /// The challenge's events of the last tick, for the sounds and the HUD, and every pass's outcome so far.
    pub challenge_events: Vec<ChallengeEvent>,
    pub outcomes: Vec<u32>,
    /// The match's screen (the one being built when `General_InitMatch` ran): its exit is taken once it is current.
    screen: Option<usize>,
    /// The crowd cutscene tables and the match's record in them (none in a challenge, or without a record).
    ccs: Option<(ccs::Tables, usize)>,
    /// The match type's letter (`crate::ccs::mode_letter`), and the match's events already looked at for cutscenes.
    mode: &'static str,
    events_seen: usize,
    /// Each team's last shooter (`Event::Shot`), where its celebration plays.
    shooters: [Option<usize>; 2],
    /// Cutscenes to play in order, whether one plays (the match holds while either), and what has been asked for:
    /// the challenge's intro, the catch's cutscenes (and whether there were any).
    cutscenes: VecDeque<CsRequest>,
    cutscene_playing: bool,
    /// The match's screen is the current one (its loading screen is left): the match and its cutscenes wait for it.
    on_screen: bool,
    challenge_intro_asked: bool,
    /// The challenge has started (`0x45ac60`'s post of type 1 on `0x1043b34`): its `CurrentChallengeNum`, for the
    /// briefing's speech (`crate::fespeech`), until taken.
    pub briefing: Option<u32>,
    briefed: bool,
    /// A button held from the challenge's countdown (the loading screen's Continue): no button counts until all are
    /// let go.
    wait_release: bool,
    ending_asked: bool,
    ending_played: bool,
    /// Each team's uber move cutscene (its `teamdescription.txt` `uber`).
    ubers: [Option<String>; 2],
    /// The restart after a goal (`crate::restart`), and its cue this tick (the HUD's sound).
    pub restart: crate::restart::Restart,
    pub cue: Option<crate::restart::Cue>,
}

impl MatchRun {
    pub fn start(flow: &Flow, vfs: &qwc_assets::Vfs, seed: u32) -> MatchRun {
        let spec = spec(flow);
        let (t, s) = (&spec.teams, &spec.stadium);
        eprintln!("match: {} against {} at {s} (skill {}, {})", t[0], t[1], spec.skill, spec.desc);
        let mut m = spec.start(vfs, seed.max(1));
        let mem = &flow.vm.mem;
        let get = |k: &str| flow.fe.datastore_u32(mem, name_hash(k)).unwrap_or(0);
        let reverse = get("ReverseTeams") & 0xff != 0;
        let speed = std::env::var("QWC_MATCH_SPEED").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
        let seeker_game = std::env::var_os("QWC_SEEKER_GAME").is_some();
        match get("NumberOfPlayers") {
            _ if speed > 0 => {}
            2 => (0..2).for_each(|t| m.set_human(t, true)),
            1 => m.set_human(usize::from(reverse), true),
            _ => {}
        }
        if seeker_game {
            m.start_seeker_game();
        }
        // the game's progress (DataStore `0x1527016b`, `0x445460`/`0x445540`): word 0 bit 3 allows combos, the dodge
        // and the special moves, word 1 bit 2 the Bludger move (**[I]** the challenges' own check and the uber
        // move's team flag are not read: the uber move stays locked)
        let row = |i| flow.fe.datastore_u16(mem, qwc_feck::frontend::KEY_UNLOCK_BITS, i).unwrap_or(0);
        let (w0, w1) = (row(0), row(1));
        // `QWC_UNLOCK_ALL=1` unlocks them whatever the save says (for trying the moves)
        let all = std::env::var_os("QWC_UNLOCK_ALL").is_some();
        let moves = w0 & 8 != 0 || all;
        m.combo_enabled = moves;
        m.power.unlocked = [moves, w1 & 4 != 0 || all, moves, false];
        // the uber move (`0x445460` k 3): an AI team's by element 3 bit 3, a player's team by its own progress
        // word (`0x4455c0`: the houses' bits 6-8, the nations' bits 2-4 of their elements)
        m.power.uber = std::array::from_fn(|t| {
            let progress =
                |i: usize| flow.fe.datastore_u16(mem, qwc_feck::frontend::KEY_UNLOCK_BITS, i as u32).unwrap_or(0);
            all || if m.teams[t].human {
                uber_word(&spec.teams[t]).is_some_and(|(i, mask)| progress(i) & mask == mask)
            } else {
                progress(3) & 8 != 0
            }
        });
        let ubers = [uber_cutscene(vfs, &spec.teams[0]), uber_cutscene(vfs, &spec.teams[1])];
        // a challenge: the player's is team 0
        let challenge = m.challenge.map(|k| {
            m.set_human(0, true);
            // **[I]** the combo challenge teaches combos, whatever the save has unlocked
            if k == 6 {
                m.combo_enabled = true;
            }
            Challenge::new(k)
        });
        // the match's record of the crowd cutscene tables (`0x432d30`, not in a challenge), and its intro
        let no_cutscenes = std::env::var_os("QWC_NO_CUTSCENES").is_some();
        let mode = ccs::mode_letter(get("MatchType") & 0xff);
        let ccs = (challenge.is_none() && !no_cutscenes).then(|| ccs::Tables::load(vfs)).flatten().and_then(|t| {
            let location = ccs::location_code(get("StadiumLoadSet"));
            let codes = [ccs::team_code(&spec.teams[0]), ccs::team_code(&spec.teams[1])];
            let r = t.find(mode, codes, location);
            eprintln!("cutscenes: match {mode} {} {} at {location}: record {r:?}", codes[0], codes[1]);
            Some((t, r?))
        });
        let mut cutscenes = VecDeque::new();
        if let Some((t, r)) = &ccs {
            if let Some(name) = t.records[*r].pick(Kind::Intro, &mut m.rng).and_then(|c| t.cutscene(c)) {
                cutscenes.push_back(CsRequest::new(name, 0));
            }
        }
        MatchRun {
            spec,
            m,
            // the front end's bindings, as its Controls menu left them
            controls: Controls::bound(&flow.fe.keys.table),
            controls2: Controls::bound(&flow.fe.keys.table),
            time: 0.0,
            over_at: None,
            now: 0,
            exited: false,
            speed: speed.max(1),
            music: Default::default(),
            input: None,
            challenge,
            challenge_events: Vec::new(),
            outcomes: Vec::new(),
            screen: flow.m.next.or(flow.m.current),
            ccs,
            mode,
            events_seen: 0,
            shooters: [None; 2],
            cutscenes,
            cutscene_playing: false,
            on_screen: false,
            challenge_intro_asked: no_cutscenes,
            briefing: None,
            briefed: false,
            wait_release: false,
            ending_asked: no_cutscenes,
            ending_played: false,
            ubers,
            restart: Default::default(),
            cue: None,
        }
    }

    /// The next cutscene to play, once the one playing has ended (and the match's screen is current; a goal's once
    /// the screen is black, `crate::restart`).
    pub fn next_cutscene(&mut self) -> Option<CsRequest> {
        if self.cutscene_playing || !self.on_screen || self.restart.cutscenes_wait() {
            return None;
        }
        let next = self.cutscenes.pop_front();
        self.cutscene_playing = next.is_some();
        next
    }

    /// The cutscene playing has ended (or was skipped, or could not play).
    pub fn cutscene_ended(&mut self) {
        self.cutscene_playing = false;
    }

    /// Whether a cutscene plays or waits to (but not while the screen fades to black after a goal: the Quaffle goes
    /// on through the hoop), or the restart after a goal holds: the match holds.
    /// The match does not step: it is over (its outro after the catch), or a challenge has not begun. The picture
    /// keeps the last tick's (blending towards it would swing between the last two).
    pub fn halted(&self) -> bool {
        self.m.finished || self.challenge.as_ref().is_some_and(|c| c.phase == Phase::Waiting)
    }

    /// The camera stays where it is: a challenge's outro, as the original's (play goes on, the player let go).
    pub fn camera_held(&self) -> bool {
        self.challenge.as_ref().is_some_and(|c| c.over())
    }

    /// How dark the picture is, 0 to 1: the restart after a goal's fade, or a challenge's outro fading to black
    /// over its last second before the results.
    pub fn darkness(&self) -> f32 {
        let outro = match (self.challenge.as_ref().is_some_and(|c| c.over()), self.over_at) {
            (true, Some(at)) => {
                let into = self.now.saturating_sub(at) as f32 - (OUTRO_TICKS - OUTRO_FADE) as f32;
                (into / OUTRO_FADE as f32).clamp(0.0, 1.0)
            }
            _ => 0.0,
        };
        self.restart.darkness().max(outro)
    }

    pub fn holding(&self) -> bool {
        let waiting = !self.cutscenes.is_empty() && !self.restart.cutscenes_wait();
        self.cutscene_playing || waiting || self.restart.holds()
    }

    /// Who plays the match's people in its cutscenes (`crate::ccs::Tables::cast`).
    pub fn cast(&self) -> crate::cutscene::Cast {
        self.ccs.as_ref().map(|(t, r)| t.cast(&t.records[*r])).unwrap_or_default()
    }

    /// What the commentary needs to know of the match: the teams, the stadium, the record's colour commentator and
    /// referee, the players' teams.
    pub fn commentary(&self) -> crate::commentary::Setup {
        let record = self.ccs.as_ref().map(|(t, r)| (t, &t.records[*r]));
        let human = [self.m.teams[0].human, self.m.teams[1].human];
        let mut s = crate::commentary::setup(&self.spec.teams, &self.spec.stadium, record, human);
        s.cup = matches!(self.mode, "H" | "W");
        s
    }

    /// A cutscene's `TEAM1` and `TEAM2` for one for `team`: that team's folder and the other's.
    pub fn cutscene_teams(&self, team: usize) -> [&str; 2] {
        let t = &self.spec.teams;
        [&t[team.min(1)], &t[1 - team.min(1)]]
    }

    /// A player's role file in its team's folder (the arena's line-up: keeper, chasers, beaters, seeker).
    fn role_file(&self, p: usize) -> Option<String> {
        let tm = &self.m.teams[self.m.players.get(p)?.team];
        let order = [tm.keeper, tm.chasers[0], tm.chasers[1], tm.chasers[2], tm.beaters[0], tm.beaters[1], tm.seeker];
        let names = ["keeper", "chaser1", "chaser2", "chaser3", "beater1", "beater2", "seeker"];
        order.iter().position(|&q| q == p).map(|i| names[i].to_string())
    }

    /// The cutscenes the goals since the last call asked for: a special move's shot and celebration (`0x4371b0`:
    /// `Shoot30` from the shooter towards the hoop, or `shvol` and `shrol` along the attack, then `Score2` at the
    /// shooter away from the goal), an uber move's (its team's cutscene, then `Score2`), and the goals' rolls
    /// (`0x4322e0`: the crowd's from the record, team 0 its `TEAM1`; the celebration, `Score2`, at the shooter; a
    /// quick or practice match celebrates instead of the crowd's). None in the Seeker game. **[I]**: the shooter the
    /// last to shoot for the team.
    fn goal_cutscenes(&mut self) {
        let start = self.events_seen.min(self.m.events.len());
        let mut asked: Vec<CsRequest> = Vec::new();
        let mut special_tick = None;
        for e in &self.m.events[start..] {
            match *e {
                Event::Shot { player, .. } => {
                    let t = self.m.players[player].team;
                    self.shooters[t] = Some(player);
                }
                Event::SpecialMove { tick, team, player, hoop, kind } => {
                    special_tick = Some(tick);
                    let at = self.m.players[player].pos;
                    let goal = self.m.hoops[hoop].pos;
                    let own = self.role_file(player);
                    // `Shoot30` towards the hoop; `shvol` and `shrol` along the attack, at the hoops' heights
                    let hoops: [f64; 6] = std::array::from_fn(|i| self.m.hoops[i].pos[1]);
                    let (name, place) = match kind {
                        1 => ("shvol", crate::cutscene::Place::Attack { team, hoops }),
                        2 => ("shrol", crate::cutscene::Place::Attack { team, hoops }),
                        _ => ("Shoot30", crate::cutscene::Place::Towards { at, goal }),
                    };
                    asked.push(CsRequest { name: name.into(), team, place, own: own.clone(), other_own: None });
                    let place = crate::cutscene::Place::Away { at, team };
                    asked.push(CsRequest { name: "Score2".into(), team, place, own, other_own: None });
                }
                Event::UberMove { tick, team, hoop, .. } => {
                    // the team's uber cutscene, then `Score2` at the hoop's mouth (`0x43ed10`; **[I]** there)
                    special_tick = Some(tick);
                    let hoops: [f64; 6] = std::array::from_fn(|i| self.m.hoops[i].pos[1]);
                    if let Some(name) = self.ubers[team].clone() {
                        let place = crate::cutscene::Place::Team { team, hoops };
                        asked.push(CsRequest { place, ..CsRequest::new(&name, team) });
                    }
                    let goal = self.m.hoops[hoop].pos;
                    let at = [goal[0] - 10.0 * self.m.teams[team].attack_dir(), goal[1], goal[2]];
                    let place = crate::cutscene::Place::Away { at, team };
                    asked.push(CsRequest { place, ..CsRequest::new("Score2", team) });
                }
                // a Bludger hit from the front (`0x424080`: the victim's facing against the Bludger's flight below
                // -0.6) plays `HitBlud` for the victim's team, placed by its script's own frame (no event point)
                Event::BludgerHit { victim, front: true, .. } => {
                    let team = self.m.players[victim].team;
                    let hoops: [f64; 6] = std::array::from_fn(|i| self.m.hoops[i].pos[1]);
                    let place = crate::cutscene::Place::Team { team, hoops };
                    asked.push(CsRequest { place, ..CsRequest::new("HitBlud", team) });
                }
                Event::SpecialTackle { team, tackler, victim, kind, .. } => {
                    // **[I]** placed from the tackler towards the carrier (`eTOWARDS_QUAFFLE_HOLDER`, `0x48e900` type 6)
                    let name = ["tacar", "takick", "tasum"][usize::from(kind).min(2)];
                    let (at, goal) = (self.m.players[tackler].pos, self.m.players[victim].pos);
                    let place = crate::cutscene::Place::Towards { at, goal };
                    let (own, other_own) = (self.role_file(tackler), self.role_file(victim));
                    asked.push(CsRequest { name: name.into(), team, place, own, other_own });
                }
                Event::GoalCutscene { tick, .. } if special_tick == Some(tick) => {}
                Event::GoalCutscene { team, kind, .. } => {
                    let celebrate = kind == 3 || (kind == 1 && matches!(self.mode, "Q" | "P"));
                    if celebrate {
                        let shooter = self.shooters[team];
                        let at = shooter.map_or([0.0, 25.0, 0.0], |p| self.m.players[p].pos);
                        let place = crate::cutscene::Place::Away { at, team };
                        let own = shooter.and_then(|p| self.role_file(p));
                        asked.push(CsRequest { name: "Score2".into(), team, place, own, other_own: None });
                    } else if let Some((t, r)) = &self.ccs {
                        let k = if team == 0 { Kind::GoalTeam0 } else { Kind::GoalTeam1 };
                        if let Some(name) = t.records[*r].pick(k, &mut self.m.rng).and_then(|c| t.cutscene(c)) {
                            asked.push(CsRequest::new(name, 0));
                        }
                    }
                }
                _ => {}
            }
        }
        self.events_seen = self.m.events.len();
        if self.ccs.is_none() || self.m.race.is_some() {
            return;
        }
        self.cutscenes.extend(asked);
    }

    /// The catch's cutscenes (`0x4483e0`): the catch for the catcher's team, then the outro for the winner's; in a
    /// cup's final won on the standings, the cup's winning cutscene after them (the outro left out when the winner
    /// did not catch).
    fn ask_ending(&mut self, flow: &Flow) {
        self.ending_asked = true;
        let won = self.cup_won(flow);
        let Some((t, r)) = &self.ccs else { return };
        let Some(caught) = self.m.race.as_ref().and_then(|r| r.caught) else { return };
        let rec = &t.records[*r];
        let score = [self.m.teams[0].score, self.m.teams[1].score];
        let winner = if score[0] != score[1] { usize::from(score[0] <= score[1]) } else { caught };
        if let Some(name) = rec.pick(Kind::Catch, &mut self.m.rng).and_then(|c| t.cutscene(c)) {
            self.cutscenes.push_back(CsRequest::new(name, caught));
        }
        if won.is_none() || winner == caught {
            if let Some(name) = rec.outro(ccs::team_code(&self.spec.teams[winner])).and_then(|c| t.cutscene(c)) {
                self.cutscenes.push_back(CsRequest::new(name, winner));
            }
        }
        if let Some(name) = won {
            self.cutscenes.push_back(CsRequest::new(name, 0));
        }
        self.ending_played = !self.cutscenes.is_empty();
    }

    /// `0x448540`'s win check (it logs `WINCHECK`): in the House Cup's final (`HCFinalMatch`) or the World Cup's
    /// (`WCRoundNumber` at least 18), the player's standing with this match's points (team 0's: the final round's
    /// player points plus its score) at least the opponent's (plus team 1's score) and the others' best gives
    /// `WinHog` or `WinWorld`.
    fn cup_won(&self, flow: &Flow) -> Option<&'static str> {
        let mem = &flow.vm.mem;
        let get = |k: &str| flow.fe.datastore_u32(mem, name_hash(k)).unwrap_or(0) as i64;
        let (s0, s1) = (i64::from(self.m.teams[0].score), i64::from(self.m.teams[1].score));
        let (prefix, name) = match self.mode {
            "H" if get("HCFinalMatch") & 0xff != 0 => ("HC", "WinHog"),
            "W" if get("WCRoundNumber") >= 18 => ("WC", "WinWorld"),
            _ => return None,
        };
        let player = get(&format!("{prefix}FinalRoundPlayerPoints")) + s0;
        let opponent = get(&format!("{prefix}FinalRoundOpponentPoints")) + s1;
        let other = get(&format!("{prefix}FinalRoundOtherPoints"));
        eprintln!("WINCHECK: {name}: player {player}, opponent {opponent}, others {other}");
        (player >= opponent && player >= other).then_some(name)
    }

    /// The match driver's events for what happened since the last call (`crate::audio::MusicRouter`).
    pub fn music(&mut self) -> Vec<usize> {
        self.music.tick(&self.m)
    }

    /// One 24 Hz tick of the front end (`tick` its count): the match steps unless the game is paused or over (the
    /// keys made into the human teams' input, turned by the camera's `heading`); answers whether it stepped. After
    /// the catch it hands the score to the DataStore and asks for the results.
    pub fn tick(&mut self, flow: &mut Flow, tick: u64, heading: [f64; 2]) -> bool {
        self.challenge_events.clear();
        self.now = tick;
        if !flow.fe.paused {
            let cutscene = self.cutscene_playing || !self.cutscenes.is_empty();
            self.cue = self.restart.tick(&self.m, cutscene);
        }
        // **[I]** the match waits for its screen (the loading screen before it is left with a key)
        self.on_screen = flow.m.current == self.screen;
        if self.challenge.is_some() {
            return self.challenge_tick(flow, tick, heading);
        }
        if !self.on_screen && !self.m.finished {
            return false;
        }
        if self.m.finished && !self.ending_asked {
            self.ask_ending(flow);
        }
        if self.holding() {
            return false;
        }
        if self.m.finished {
            let at = *self.over_at.get_or_insert(tick);
            // after the catch's cutscenes the results come at once (`eMATCH_FINISH`); without any, a moment on
            let wait = if self.ending_played { 0 } else { OUTRO_TICKS };
            if !self.exited && tick >= at + wait {
                self.exited = true;
                self.log_end();
                let mem = &mut flow.vm.mem;
                for (k, t) in [("Team1Score", 0), ("Team2Score", 1)] {
                    flow.fe.set_datastore_u32(mem, name_hash(k), self.m.teams[t].score as u32);
                }
                if let Some(&layer) = flow.m.current_screen().and_then(|s| s.layers.first()) {
                    flow.fe.requests.push(Request::Exit { layer, exit: 1, finish: true });
                }
            }
            return false;
        }
        if flow.fe.paused {
            return false;
        }
        self.controls.tick();
        self.controls2.tick();
        let input = Input::from_pad(&self.controls.pad, heading, self.m.combo_enabled);
        self.input = self.m.teams.iter().any(|t| t.human).then_some(input);
        let seeker = qwc_sim::race::SeekerPad::from_pad(&self.controls.pad);
        // with two human teams and a second pad, team 1 is player 2's (**[I]** steered by the first view's heading)
        let second = self.m.teams.iter().all(|t| t.human) && self.controls2.gamepad.is_some();
        let input2 = Input::from_pad(&self.controls2.pad, heading, self.m.combo_enabled);
        let seeker2 = qwc_sim::race::SeekerPad::from_pad(&self.controls2.pad);
        for t in 0..2 {
            if self.m.teams[t].human {
                let (i, s) = if t == 1 && second { (input2, seeker2) } else { (input, seeker) };
                self.m.set_input(t, i);
                self.m.set_seeker_pad(t, s);
            }
        }
        for _ in 0..self.speed {
            self.m.step();
        }
        self.goal_cutscenes();
        self.time = self.m.tick as f64 / 24.0;
        true
    }

    /// The match (or challenge) over: a line of the library's match log (`crate::record`), with the Gameplay packs on.
    fn log_end(&self) {
        let Some(log) = crate::record::match_log() else { return };
        let teams = [self.spec.teams[0].as_str(), self.spec.teams[1].as_str()];
        let score = [self.m.teams[0].score as i64, self.m.teams[1].score as i64];
        if let Err(e) = crate::record::log_match(&log, &self.spec.stadium, teams, score, &self.spec.desc) {
            eprintln!("match log: {e}");
        }
    }

    /// The keys made into team 0's input; while `live` is false (the challenge's countdown) none (the remaster's:
    /// the Continue that ends the loading screen would pass the ball), and after it no button until all are let go.
    fn take_input(&mut self, heading: [f64; 2], live: bool) {
        self.controls.tick();
        let pad = Input::from_pad(&self.controls.pad, heading, self.m.combo_enabled);
        let sticks = Input { stick: pad.stick, raw: pad.raw, ..Input::default() };
        let buttons = pad != Input { stick2: pad.stick2, ..sticks };
        if !live {
            self.wait_release = true;
        } else if self.wait_release && !buttons {
            self.wait_release = false;
        }
        let input = match (live, self.wait_release) {
            (false, _) => Input::default(),
            (true, true) => sticks,
            (true, false) => pad,
        };
        self.input = Some(input);
        self.m.set_input(0, input);
        let seeker = if live { qwc_sim::race::SeekerPad::from_pad(&self.controls.pad) } else { Default::default() };
        self.m.set_seeker_pad(0, seeker);
    }

    /// A challenge's tick: it starts when its screen is current and the instructions were left (DataStore
    /// `0x7da5b162` clear), then the match steps under its rules; at its end the time and score go to the DataStore
    /// (`0x45bd50`) and, three seconds on, the screen takes exit 1 to the results.
    fn challenge_tick(&mut self, flow: &mut Flow, tick: u64, heading: [f64; 2]) -> bool {
        let mem = &flow.vm.mem;
        let waiting = flow.fe.datastore_u32(mem, KEY_CHALLENGE_WAIT).unwrap_or(0) & 0xff != 0;
        let ch = self.challenge.as_mut().unwrap();
        if ch.phase == Phase::Waiting {
            if flow.m.current != self.screen || waiting {
                return false;
            }
            if !self.briefed {
                self.briefed = true;
                self.briefing = Some(flow.fe.datastore_u32(mem, name_hash("CurrentChallengeNum")).unwrap_or(0));
            }
            // its intro first, then the countdown
            if !self.challenge_intro_asked {
                self.challenge_intro_asked = true;
                if let Some(name) = ccs::challenge_intro(ch.kind) {
                    self.cutscenes.push_back(CsRequest::new(name, 0));
                }
            }
            if self.cutscene_playing || !self.cutscenes.is_empty() {
                return false;
            }
            ch.start(&mut self.m);
        }
        if ch.over() {
            let at = *self.over_at.get_or_insert(tick);
            if !self.exited && tick >= at + OUTRO_TICKS {
                self.exited = true;
                let (time, score) = (ch.time(), ch.score());
                eprintln!("{}: {:?} in {time:.2} s ({} passes)", self.spec.desc, ch.phase, ch.count);
                self.log_end();
                let mem = &mut flow.vm.mem;
                flow.fe.set_datastore_u32(mem, name_hash("Challenge_Time"), time.to_bits());
                flow.fe.set_datastore_u32(mem, KEY_CHALLENGE_SCORE, score.to_bits());
                if std::env::var_os("QWC_FLOW_LOG").is_some() {
                    let get = |k: &str| flow.fe.datastore_u32(mem, name_hash(k));
                    eprintln!(
                        "challenge end: Challenge_Time {:?} CurrentChallengeNum {:?} PassTime {:?}",
                        get("Challenge_Time").map(f32::from_bits),
                        get("CurrentChallengeNum"),
                        get("PassTime").map(f32::from_bits)
                    );
                }
                if let Some(&layer) = flow.m.current_screen().and_then(|s| s.layers.first()) {
                    flow.fe.requests.push(Request::Exit { layer, exit: 1, finish: true });
                }
            }
            // the outro: play goes on, the player let go (and the camera held where it is, `camera_held`)
            if flow.fe.paused {
                return false;
            }
            self.input = None;
            self.m.set_input(0, Input::default());
            self.m.step();
            self.time = self.m.tick as f64 / 24.0;
            return true;
        }
        if flow.fe.paused {
            return false;
        }
        let live = self.challenge.as_ref().is_some_and(|c| c.phase != Phase::Countdown);
        self.take_input(heading, live);
        let ch = self.challenge.as_mut().unwrap();
        if ch.plays() {
            self.m.step();
        }
        self.challenge_events = ch.tick(&mut self.m);
        for e in &self.challenge_events {
            if let ChallengeEvent::Pass { outcome, .. } = e {
                self.outcomes.push(*outcome);
            }
        }
        self.time = self.m.tick as f64 / 24.0;
        true
    }
}
