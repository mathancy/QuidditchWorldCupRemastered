//! The match's sound effects: the crowd, the stadium's horns and fireworks, the Quaffle and the player's broom,
//! played from the match's events in place of the game's routers (`crate::audio` has the music).
//!
//! **[T]** the emitters, their categories and mixing-desk faders (the audio layer's objects, `0x519000`-`0x52c000`;
//! the categories' slots `[0xf91458] + 0x14 + 0x20 n`, `0x516f00`; the faders in `root/audio/cli/mix.cli`), and
//! what the crowd's handlers play:
//! - a goal (`0x51c290`, on the score router `0x1043b64`): `CrowdHi` 20 and `CrowdCheer` 15 (for the first
//!   value 0), the horn `cGoalHorn` 65 (the stadium's category) 200-500 ms later, the fireworks `cGoalExplosion` 27,
//!   and `cGoalCheer` 27 200 ms later, each delayed sound fading after 4 s (`0x51b570`, `0x51b610`);
//! - where the Quaffle is (`0x51c050`, posted by `0x451420` when it changes): one goal area (zone 0) or the other
//!   (1), else the half by side (2, 3). For the first side, zone 1 fades `CrowdMid` 19 and `CrowdHi` 20 in over a
//!   second and plays `CrowdCheer` 15 and `CrowdHorns` 29; zone 3 fades `CrowdMid` in and `CrowdHi` out (3 s);
//!   zones 0 and 2 fade both out. For the other side only `CrowdMid` moves (in for 1 and 3);
//! - a change of possession (`0x51c540`, on `0x1043b70`, which `0x431c40` posts when a holder of another team is
//!   set): `CrowdLow` 18 again, `CrowdHi` stopped, and unless the kick-off gave the ball `CrowdCheer` 15 for team
//!   0 or `CrowdTackle` 31 for team 1; a save (`0x51c600`): `cSave` 31, `CrowdHi` stopped;
//! - the player's chaser (`0x527800`-`0x528610`): the boost charge `ACBoostCharge` 13 while Boost is held, then
//!   `ACReBoost` 22 with the charge fading over 0.5 s; `ACDodge` 60; a contact `Impact` 6 (the Quaffle's category);
//!   the game's start `QuaffleStart` 28;
//! - the Bludgers (`0x529bb0`; `BludgerHit`, `BludgerFlyBy` in `bludger.map`, `BludgerSummon` in `foley.map`):
//!   the summon (router `0x1043ae0`) plays `BludgerSummon` 81, a player hit (`0x1043aec`) `BludgerSummon` 74, a
//!   beater's bat (`0x1043af0`, game event 13) `BludgerHit` 9, with `BludgerFlyBy` 26 when the event's flag is
//!   set (**[I]** set for the launch); `BludgerFly` rides the animation sound events (`0x104398c`, not ported);
//! - `General_DeInitMatch` (`0x1043b48`, `0x51bb80`) ends them.
//!
//! **[T]** (2026-10-21) the routers' posters: the goal's message carries the scoring team first (`0x432100`), the
//! zone's is `0x4513a0`'s ([`quaffle_zone`]), the possession's `0x431c40`'s.
//!
//! **[I]**: `CrowdLow` 18 from the kick-off; the beds and the Snitch's `SnitchFly` 12 (from the Seeker game to the
//! catch) loop; a fader missing from `mix.cli` is 1; an emitter's level is its fader times its group's; zone 4 (no
//! team has the Quaffle) fades the beds out as zones 0 and 2 do.

use std::collections::HashMap;

use qwc_assets::Vfs;
use qwc_audio::{Bus, Library, Mixer, Params, VoiceId};
use qwc_sim::human::Input;
use qwc_sim::race::RaceEvent;
use qwc_sim::shooting::in_goal_area;
use qwc_sim::world::Event;
use qwc_sim::Match;

/// **[T]** The Quaffle's zone as `0x4513a0` posts it on `0x1043b74`, with the team whose ball list holds it: none 4;
/// in that team's own goal area 0, in the one it attacks 1, else 2 in its own half (`x >= 0` for team 0, which
/// attacks `-x`; `x <= 0` for team 1) and 3 in the half it attacks.
pub(crate) fn quaffle_zone(m: &Match) -> (u8, Option<usize>) {
    let Some(team) = m.ball_team else { return (4, None) };
    let q = m.quaffle;
    let s = m.pitch.scale_goals;
    let zone = if in_goal_area(q, team, s, 1.0) {
        0
    } else if in_goal_area(q, 1 - team, s, 1.0) {
        1
    } else if (if team != 0 { 1.0 } else { -1.0 }) * q[0] <= 0.0 {
        2
    } else {
        3
    };
    (zone, Some(team))
}

/// A sound category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Category {
    /// The stadium's own (`Audio/Stadium/<name>/crowd.map`, `stadium.map`).
    Crowd,
    Stadium,
    /// `Audio/cats/<name>.map`.
    Cats(&'static str),
}

/// An emitter: its name (its fader), its group's fader and its category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Emitter {
    CrowdLow,
    CrowdMid,
    CrowdHi,
    CrowdCheer,
    CrowdHorns,
    CrowdTackle,
    GoalCheer,
    GoalHorn,
    GoalExplosion,
    Save,
    BoostCharge,
    ReBoost,
    Dodge,
    Impact,
    QuaffleStart,
    SnitchFly,
    BludgerHit,
    BludgerFlyBy,
    BludgerSummon,
    SkLow,
    SkHi,
    SkCheer,
    SkHorns,
    SkAhhh,
    SkBoostStart,
    SkBoostLoop,
    SkFly,
    SkReboost,
    HudStart,
}

impl Emitter {
    fn info(self) -> (&'static str, &'static str, Category) {
        use Category::*;
        use Emitter::*;
        match self {
            CrowdLow => ("CrowdLow", "Crowd", Crowd),
            CrowdMid => ("CrowdMid", "Crowd", Crowd),
            CrowdHi => ("CrowdHi", "Crowd", Crowd),
            CrowdCheer => ("CrowdCheer", "Crowd", Crowd),
            CrowdHorns => ("CrowdHorns", "Crowd", Stadium),
            CrowdTackle => ("CrowdTackle", "Crowd", Crowd),
            GoalCheer => ("cGoalCheer", "Crowd", Crowd),
            GoalHorn => ("cGoalHorn", "SFX", Stadium),
            GoalExplosion => ("cGoalExplosion", "SFX", Stadium),
            Save => ("cSave", "Crowd", Crowd),
            BoostCharge => ("ACBoostCharge", "Chaser", Cats("chaser")),
            ReBoost => ("ACReBoost", "Chaser", Cats("chaser")),
            Dodge => ("ACDodge", "Chaser", Cats("chaser")),
            Impact => ("Impact1", "Chaser", Cats("quaffle")),
            QuaffleStart => ("QuaffleStart", "Chaser", Cats("quaffle")),
            SnitchFly => ("SnitchFly", "SFX", Cats("snitch")),
            BludgerHit => ("BludgerHit", "SFX", Cats("bludger")),
            BludgerFlyBy => ("BludgerFlyBy", "SFX", Cats("bludger")),
            BludgerSummon => ("BludgerSummon", "SFX", Cats("foley")),
            // the Seeker game's crowd (`0x51cad0`: group `Seeker`, the crowd's category 7)
            SkLow => ("SKCrowdLow", "Seeker", Crowd),
            SkHi => ("SKCrowdHi", "Seeker", Crowd),
            SkCheer => ("SKCrowdCheer", "Seeker", Crowd),
            SkHorns => ("SKCrowdHorns", "Seeker", Crowd),
            SkAhhh => ("SKCrowdAhhh", "Seeker", Crowd),
            // the player's seeker (`0x528cd0`: group `Seeker`, category 9, `Audio/cats/seeker.map`)
            SkBoostStart => ("AS1BoostSt", "Seeker", Cats("seeker")),
            SkBoostLoop => ("AS1BoostLp", "Seeker", Cats("seeker")),
            SkFly => ("ASFly", "Seeker", Cats("seeker")),
            SkReboost => ("ASReboost", "Seeker", Cats("seeker")),
            // the OSD's (`0x526870`: `osd.map`), its own fader
            HudStart => ("HudStart", "OSD", Cats("osd")),
        }
    }

    fn looping(self) -> bool {
        use Emitter::*;
        matches!(self, CrowdLow | CrowdMid | CrowdHi | SnitchFly | SkLow | SkHi | SkBoostLoop | SkFly)
    }
}

/// The mixing desk's faders: `Audio.Mixingdesk.FaderSetValue NAME VALUE` lines.
pub(crate) fn faders(text: &str) -> HashMap<String, f32> {
    text.lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            (w.next()? == "Audio.Mixingdesk.FaderSetValue").then_some(())?;
            Some((w.next()?.to_string(), w.next()?.parse().ok()?))
        })
        .collect()
}

/// A voice with its fade, in ticks.
struct Voice {
    id: VoiceId,
    level: f32,
    gain: f32,
    target: f32,
    step: f32,
}

/// The match's sound effects.
pub(crate) struct MatchAudio {
    /// The stadium's sound folder (`Audio/Stadium/<name>`).
    folder: String,
    voices: HashMap<Emitter, Voice>,
    /// Sounds waiting: (tick due, emitter, event), and the voices to fade at a tick.
    delayed: Vec<(u64, Emitter, u32)>,
    fades: Vec<(u64, Emitter, f32)>,
    tick: u64,
    zone: Option<(u8, bool)>,
    /// The team that last had the Quaffle (game `+0x3d4`, -1 at first).
    possession: Option<usize>,
    /// The Seeker game's leader (`0x10439ec`, `0x10439ed`) and the seekers at the trail's edge.
    leader: Option<usize>,
    at_edge: [bool; 2],
    started: bool,
    events_seen: usize,
    boost: bool,
    /// The player's seeker boosting, and in the trail's sweet spot (the handler's `+0x538`, `+0x540`).
    seeker_boost: bool,
    seeker_sweet: bool,
    /// The team whose player listens on this machine, when the other human plays on another (online,
    /// `crate::online`); else the first human team is the player's.
    pub viewer: Option<usize>,
}

/// Ticks in `ms` milliseconds.
fn ticks(ms: f32) -> u64 {
    (ms * 24.0 / 1000.0).round() as u64
}

impl MatchAudio {
    /// The sounds of a match at the stadium whose sounds are in `folder`.
    /// The stadium's sound folder.
    pub fn folder(&self) -> &str {
        &self.folder
    }

    pub fn new(folder: String) -> MatchAudio {
        MatchAudio {
            folder,
            voices: HashMap::new(),
            delayed: Vec::new(),
            fades: Vec::new(),
            tick: 0,
            zone: None,
            possession: None,
            leader: None,
            at_edge: [false; 2],
            started: false,
            events_seen: 0,
            boost: false,
            seeker_boost: false,
            seeker_sweet: false,
            viewer: None,
        }
    }

    fn path(&self, c: Category) -> String {
        match c {
            Category::Crowd => format!("{}/crowd.map", self.folder),
            Category::Stadium => format!("{}/stadium.map", self.folder),
            Category::Cats(n) => format!("Audio/cats/{n}.map"),
        }
    }

    /// Start `event` on emitter `e` at `gain` of its level (the voice it played before stops), fading to 1 over
    /// `fade_ms` when that is not 0.
    fn play(&mut self, ctx: &Ctx, e: Emitter, event: u32, gain: f32, fade_ms: f32) {
        let (name, group, cat) = e.info();
        let path = self.path(cat);
        let sound = match ctx.lib.event(ctx.vfs, &path, event, crate::audio::roll(self.tick, event)) {
            Ok(Some(s)) => s,
            Ok(None) | Err(qwc_audio::Error::Missing(_)) => return,
            Err(err) => {
                eprintln!("{path}: {err}");
                return;
            }
        };
        if std::env::var_os("QWC_AUDIO_LOG").is_some() {
            eprintln!("sound {name} {event} ({path})");
        }
        let level = ctx.fader(name) * ctx.fader(group);
        let mut mixer = ctx.mixer.lock().unwrap();
        if let Some(v) = self.voices.remove(&e) {
            mixer.stop(v.id);
        }
        let (gain, target) = if fade_ms > 0.0 { (gain, 1.0) } else { (1.0, 1.0) };
        let params = Params { gain: gain * level, looping: e.looping(), bus: Bus::Effects, ..Params::default() };
        let id = mixer.play(sound, params);
        let step = if fade_ms > 0.0 { (target - gain) / ticks(fade_ms).max(1) as f32 } else { 0.0 };
        self.voices.insert(e, Voice { id, level, gain, target, step });
    }

    /// Fade emitter `e` to `target` over `ms` (out to 0 stops it).
    fn fade(&mut self, e: Emitter, target: f32, ms: f32) {
        if let Some(v) = self.voices.get_mut(&e) {
            v.target = target;
            v.step = (target - v.gain) / ticks(ms).max(1) as f32;
        }
    }

    /// Fade `e` in from silence when it is not playing, else to full.
    fn fade_in(&mut self, ctx: &Ctx, e: Emitter, event: u32, ms: f32) {
        if self.voices.contains_key(&e) {
            self.fade(e, 1.0, ms);
        } else {
            self.play(ctx, e, event, 0.0, ms);
        }
    }

    fn silence(&mut self, mixer: &std::sync::Mutex<Mixer>, e: Emitter) {
        if let Some(v) = self.voices.remove(&e) {
            mixer.lock().unwrap().stop(v.id);
        }
    }

    /// `0x51b570`: `event` on `e` in `delay` ms, fading out 4 s after.
    fn later(&mut self, e: Emitter, event: u32, delay: f32) {
        let due = self.tick + ticks(delay);
        self.delayed.push((due, e, event));
        self.fades.push((due + ticks(4000.0), e, 1000.0));
    }

    /// The HUD starting again (the kick-off message `0x1043b50`, `0x4f7cc0` posting `0x1043b20` kind 1, the OSD's
    /// `0x527070`): `HudStart` 28.
    pub fn hud_start(&mut self, ctx: &Ctx) {
        self.play(ctx, Emitter::HudStart, 28, 1.0, 0.0);
    }

    /// One tick of the match `m` (its events since the last), with the player's `input` when there is one.
    pub fn tick(&mut self, ctx: &Ctx, m: &Match, input: Option<&Input>) {
        self.tick += 1;
        if !self.started {
            // the match's start: the kick-off object's message (`0x432e90`) plays `QuaffleStart`, and the intro's end
            // the HUD's start (`0x419be0`, `0x1043b50`)
            self.started = true;
            self.play(ctx, Emitter::CrowdLow, 18, 1.0, 0.0);
            self.play(ctx, Emitter::QuaffleStart, 28, 1.0, 0.0);
            self.play(ctx, Emitter::HudStart, 28, 1.0, 0.0);
        }
        let start = self.events_seen.min(m.events.len());
        self.events_seen = m.events.len();
        let kick_off = m.events[start..].iter().any(|e| matches!(e, Event::KickOff { .. }));
        // the possession message (`0x431c40` on `0x1043b70`, `0x51c540`) when a holder of another team than the last
        // (game `+0x3d4`) is set: `CrowdLow` 18 again and `CrowdHi` stopped; unless the kick-off gave it (the setter's
        // mode 2), `CrowdCheer` 15 for team 0's possession and `CrowdTackle` 31 for team 1's
        if let Some(t) = m.holder.map(|h| m.players[h].team).filter(|&t| self.possession != Some(t)) {
            self.possession = Some(t);
            self.play(ctx, Emitter::CrowdLow, 18, 1.0, 0.0);
            self.silence(ctx.mixer, Emitter::CrowdHi);
            if !kick_off {
                if t == 0 {
                    self.play(ctx, Emitter::CrowdCheer, 15, 1.0, 0.0);
                } else {
                    self.play(ctx, Emitter::CrowdTackle, 31, 1.0, 0.0);
                }
            }
        }
        for e in &m.events[start..] {
            match *e {
                Event::Goal { team, .. } => self.goal(ctx, team != 0),
                Event::Touch { state: 9, .. } => {
                    self.silence(ctx.mixer, Emitter::CrowdHi);
                    self.play(ctx, Emitter::Save, 31, 1.0, 0.0);
                    self.play(ctx, Emitter::Impact, 6, 1.0, 0.0);
                }
                Event::Catch { .. } | Event::Touch { .. } => self.play(ctx, Emitter::Impact, 6, 1.0, 0.0),
                Event::SeekerGame { .. } => {
                    self.play(ctx, Emitter::SnitchFly, 12, 1.0, 0.0);
                    // the Seeker crowd's start (`0x51d110`, the race router's 9): its beds in to half over a second,
                    // a cheer and the horns
                    self.play(ctx, Emitter::SkLow, 18, 0.0, 1000.0);
                    self.fade(Emitter::SkLow, 0.5, 1000.0);
                    self.play(ctx, Emitter::SkHi, 20, 0.0, 1000.0);
                    self.fade(Emitter::SkHi, 0.5, 1000.0);
                    self.play(ctx, Emitter::SkCheer, 15, 1.0, 0.0);
                    self.play(ctx, Emitter::SkHorns, 29, 1.0, 0.0);
                }
                // the race router's catch (8), the AI's grab (24), a bump (17)
                Event::Race { event: RaceEvent::Caught { .. }, .. } => {
                    self.silence(ctx.mixer, Emitter::SnitchFly);
                    self.play(ctx, Emitter::SkCheer, 15, 1.0, 0.0);
                }
                Event::Race { event: RaceEvent::Grab { .. }, .. } => self.play(ctx, Emitter::SkCheer, 15, 1.0, 0.0),
                Event::Race { event: RaceEvent::Bump, .. } => self.play(ctx, Emitter::SkAhhh, 31, 1.0, 0.0),
                Event::Dodge { .. } => self.play(ctx, Emitter::Dodge, 60, 1.0, 0.0),
                Event::BludgerSummon { .. } => self.play(ctx, Emitter::BludgerSummon, 81, 1.0, 0.0),
                Event::BludgerLaunch { .. } => {
                    self.play(ctx, Emitter::BludgerFlyBy, 26, 1.0, 0.0);
                    self.play(ctx, Emitter::BludgerHit, 9, 1.0, 0.0);
                }
                Event::BeaterBlock { .. } => self.play(ctx, Emitter::BludgerHit, 9, 1.0, 0.0),
                Event::BludgerHit { .. } => self.play(ctx, Emitter::BludgerSummon, 74, 1.0, 0.0),
                _ => {}
            }
        }
        self.zone(ctx, m);
        if let Some(r) = &m.race {
            // the race router's 13 (`0x44e920`): the lead changes hands by more than a unit of the trail
            let d = r.seekers[0].param - r.seekers[1].param;
            if d.abs() > 1.0 {
                let ahead = usize::from(d < 0.0);
                if self.leader.is_some_and(|l| l != ahead) {
                    self.play(ctx, Emitter::SkCheer, 15, 1.0, 0.0);
                }
                self.leader = Some(ahead);
            }
            // 15 (`0x44c720`): a seeker pushed back at the trail's edge (**[I]** once as it reaches it)
            for (t, s) in r.seekers.iter().enumerate() {
                let edge = s.offset.abs() >= 0.999;
                if edge && !self.at_edge[t] {
                    self.play(ctx, Emitter::SkHorns, 29, 1.0, 0.0);
                }
                self.at_edge[t] = edge;
            }
        }
        self.seeker(ctx, m);
        if let Some(i) = input.filter(|_| m.race.is_none()) {
            if i.boost_held && !self.boost {
                self.play(ctx, Emitter::BoostCharge, 13, 1.0, 0.0);
            } else if !i.boost_held && self.boost {
                self.fade(Emitter::BoostCharge, 0.0, 500.0);
                self.play(ctx, Emitter::ReBoost, 22, 1.0, 0.0);
            }
            self.boost = i.boost_held;
        }
        let now = self.tick;
        let due: Vec<_> = self.delayed.iter().filter(|d| d.0 <= now).map(|d| (d.1, d.2)).collect();
        self.delayed.retain(|d| d.0 > now);
        for (e, event) in due {
            self.play(ctx, e, event, 1.0, 0.0);
        }
        let fading: Vec<_> = self.fades.iter().filter(|f| f.0 <= now).map(|f| (f.1, f.2)).collect();
        self.fades.retain(|f| f.0 > now);
        for (e, ms) in fading {
            self.fade(e, 0.0, ms);
        }
        self.apply_fades(ctx.mixer);
    }

    /// The player's seeker in the race (`0x529580`, on the race router `0x1043a8c`; traced 2026-10-21): the boost
    /// starting (4, and `0x44b4d0`'s 25) plays `AS1BoostSt` 5, fades `AS1BoostLp` 14 in over half a second and plays
    /// `ASFly` 2; its end (5) fades those two out over half a second. Coming into the trail's sweet spot (18) or
    /// leaving it (19), not boosting, plays `ASReboost` 30 or 77. **[I]** the handler's seeker test (`+0x24` of the
    /// message's object clear) is read as the first human team's seeker.
    fn seeker(&mut self, ctx: &Ctx, m: &Match) {
        let (Some(r), Some(t)) = (&m.race, self.viewer.or(m.teams.iter().position(|t| t.human))) else {
            if self.seeker_boost {
                self.fade(Emitter::SkBoostLoop, 0.0, 500.0);
                self.fade(Emitter::SkFly, 0.0, 500.0);
                self.seeker_boost = false;
            }
            return;
        };
        let s = &r.seekers[t];
        if s.boosting && !self.seeker_boost {
            self.play(ctx, Emitter::SkBoostStart, 5, 1.0, 0.0);
            self.play(ctx, Emitter::SkBoostLoop, 14, 0.0, 500.0);
            self.play(ctx, Emitter::SkFly, 2, 1.0, 0.0);
        } else if !s.boosting && self.seeker_boost {
            self.fade(Emitter::SkBoostLoop, 0.0, 500.0);
            self.fade(Emitter::SkFly, 0.0, 500.0);
        }
        self.seeker_boost = s.boosting;
        let sweet = s.offset.abs().min(1.0) < m.race_rules.sweet_spot;
        if sweet != self.seeker_sweet {
            self.seeker_sweet = sweet;
            if !s.boosting {
                self.play(ctx, Emitter::SkReboost, if sweet { 30 } else { 77 }, 1.0, 0.0);
            }
        }
    }

    /// `0x51c290`: the crowd on a goal (`second`: the event's first value is not 0).
    fn goal(&mut self, ctx: &Ctx, second: bool) {
        if !second {
            self.play(ctx, Emitter::CrowdHi, 20, 1.0, 0.0);
        }
        self.play(ctx, Emitter::CrowdCheer, 15, 1.0, 0.0);
        let horn = 200.0 + (crate::audio::roll(self.tick, 65) * 300.0).floor();
        self.later(Emitter::GoalHorn, 65, horn);
        self.play(ctx, Emitter::GoalExplosion, 27, 1.0, 0.0);
        self.later(Emitter::GoalCheer, 27, 200.0);
    }

    /// `0x51c050`: the crowd follows where the Quaffle is ([`quaffle_zone`]).
    fn zone(&mut self, ctx: &Ctx, m: &Match) {
        let (zone, team) = quaffle_zone(m);
        let now = (zone, team.is_some_and(|t| t != 0));
        if self.zone == Some(now) {
            return;
        }
        self.zone = Some(now);
        match now {
            (1, false) => {
                self.fade_in(ctx, Emitter::CrowdMid, 19, 1000.0);
                self.fade_in(ctx, Emitter::CrowdHi, 20, 1000.0);
                self.play(ctx, Emitter::CrowdCheer, 15, 1.0, 0.0);
                self.play(ctx, Emitter::CrowdHorns, 29, 1.0, 0.0);
            }
            (3, false) => {
                self.fade_in(ctx, Emitter::CrowdMid, 19, 1000.0);
                self.fade(Emitter::CrowdHi, 0.0, 3000.0);
            }
            (_, false) => {
                self.fade(Emitter::CrowdMid, 0.0, 3000.0);
                self.fade(Emitter::CrowdHi, 0.0, 3000.0);
            }
            (1 | 3, true) => self.fade_in(ctx, Emitter::CrowdMid, 19, 1000.0),
            (_, true) => self.fade(Emitter::CrowdMid, 0.0, 3000.0),
        }
    }

    fn apply_fades(&mut self, mixer: &std::sync::Mutex<Mixer>) {
        let mut mixer = mixer.lock().unwrap();
        self.voices.retain(|_, v| {
            if !mixer.is_playing(v.id) {
                return false;
            }
            if v.step == 0.0 {
                return true;
            }
            v.gain += v.step;
            if (v.step > 0.0 && v.gain >= v.target) || (v.step < 0.0 && v.gain <= v.target) {
                v.gain = v.target;
                v.step = 0.0;
            }
            if v.gain <= 0.0 {
                mixer.stop(v.id);
                return false;
            }
            if let Some(mut p) = mixer.params(v.id) {
                p.gain = v.gain * v.level;
                mixer.set_params(v.id, p);
            }
            true
        });
    }

    /// The match is over (`0x51bb80`).
    pub fn stop(&mut self, mixer: &std::sync::Mutex<Mixer>) {
        let mut mixer = mixer.lock().unwrap();
        for (_, v) in self.voices.drain() {
            mixer.stop(v.id);
        }
        self.delayed.clear();
        self.fades.clear();
    }
}

/// What a tick plays with.
pub(crate) struct Ctx<'a> {
    pub lib: &'a Library,
    pub vfs: &'a Vfs,
    pub mixer: &'a std::sync::Mutex<Mixer>,
    pub faders: &'a HashMap<String, f32>,
}

impl Ctx<'_> {
    fn fader(&self, name: &str) -> f32 {
        self.faders.get(name).copied().unwrap_or(1.0)
    }
}

/// The stadium's sound folder: the stadium load set's `audio` package, else the one named like the stadium (the
/// Hogwarts grounds' for theirs, the first challenge's for the challenges).
pub(crate) fn stadium_folder(vfs: &Vfs, stadium: &str) -> String {
    let s = stadium.to_ascii_lowercase();
    let name = if s.starts_with("hog") || s.starts_with("sum") {
        "Hog_Sum"
    } else if s.starts_with("chal") {
        "chal_01"
    } else {
        stadium
    };
    let folder = format!("Audio/Stadium/{name}");
    if vfs.exists(&format!("{folder}/crowd.map")) || vfs.exists(&format!("{folder}/stadium.map")) {
        folder
    } else {
        "Audio/Stadium/england".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mixing_desk() {
        let f = faders("Audio.Mixingdesk.FaderSetValue Crowd 0.9500\nAudio.Mixingdesk.FaderSetValue HW 0.4750\nx y\n");
        assert_eq!(f.get("Crowd"), Some(&0.95));
        assert_eq!(f.get("HW"), Some(&0.475));
        assert_eq!(f.len(), 2);
    }

    #[test]
    fn delays_in_ticks() {
        assert_eq!(ticks(1000.0), 24);
        assert_eq!(ticks(200.0), 5);
    }
}
