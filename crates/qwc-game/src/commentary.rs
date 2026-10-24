//! The match commentary: EA UK's speech engine (`QWC.exe` `0x552ae0`-`0x556400`) and the game's side of it
//! (`0x51d000`-`0x526000`), traced in docs/RE_NOTES.md ("Commentary").
//!
//! - [`Engine`]: speech events wait in 16 slots (`0x66d5b8`). When nothing is being said the best one is taken (the
//!   highest priority, the newest among equals; after an event that names follow-ups, one of those first), the
//!   game's filter may refuse it, the slots posted before it that may not wait are dropped, and it plays: the first
//!   of its variants, in a weighted shuffle, that passes its chance, its tests and argument masks and whose every
//!   part finds a clip in its bank (each of the clip's keys allowed by the part and, for most, by an argument's bit
//!   mask; a clip said lately only when nothing else fits).
//! - [`Commentary`]: the match's events become speech events with the arguments the game's handlers compute (sides,
//!   teams, players and the stadium as codes or bit masks), and each line plays on the speech bus clip after clip.
//!
//! The engine's clock is never set (`0x555b40` reads the unset `0x66d6dc`), so it always reads 0 and no waiting
//! event expires: an event leaves its slot when it plays, when a later one plays first (unless its flag 4 keeps
//! it) or when the slots are full and a more important one needs its place.

use qwc_assets::Vfs;
use qwc_audio::{Bus, Library, Mixer, Params, VoiceId};
use qwc_formats::speech::{Bank, BankClip, Event, Part, Variant};
use qwc_sim::race::RaceEvent;
use qwc_sim::world::{Event as MatchEvent, Role};
use qwc_sim::Match;

/// The speech language (**[I]** as for the cutscenes' speech).
const LANG: &str = "English";
/// The match's speech events.
pub(crate) const EVENTS: &str = "Audio/SpchCmn/Game/Events.evt";

/// The speech events the game posts (`Events.evt` ids).
mod id {
    pub const INTRO: u16 = 0;
    pub const PASS: u16 = 3;
    pub const GOAL: u16 = 4;
    pub const SHOT: u16 = 5;
    pub const SAVE: u16 = 7;
    pub const START_MATCH: u16 = 8;
    pub const SEEKER_SHOVE: u16 = 9;
    pub const SNITCH_SPOTTED: u16 = 13;
    pub const TACKLE: u16 = 14;
    pub const GOAL_FOLLOW: u16 = 15;
    pub const INTRO_CC: u16 = 16;
    pub const REFEREE: u16 = 20;
    pub const SNITCH_CAUGHT: u16 = 21;
    pub const SEEKER_LEAD: u16 = 22;
    pub const SEEKER_CLOSE: u16 = 23;
    pub const SEEKER_BOOST: u16 = 24;
    pub const SPECIAL_MOVE: u16 = 35;
    pub const INTERCEPT_PASS: u16 = 30;
    pub const MATCH_RESULT: u16 = 31;
    pub const TEAM_REVIEW: u16 = 39;
    pub const NEG_LINK: u16 = 40;
    pub const WAX_LYRICAL: u16 = 43;
    pub const CATCH: u16 = 44;
}

/// The engine's own random numbers (`0x555b50`, seeded by `0x555cd0` with the speech session) and the history of
/// tagged draws that keeps them from repeating (`0x66da80`: 32 entries of value and tag).
#[derive(Debug, Clone)]
pub(crate) struct Rng {
    s: [u32; 6],
    ring: [(u16, u16); 32],
    at: usize,
}

impl Rng {
    pub fn new(seed: u32) -> Rng {
        let s0 = seed.wrapping_mul(0x10001).wrapping_sub(0x0dd2_f1aa);
        let s1 = s0.wrapping_sub(0x69fb_e76d);
        let s2 = s1.wrapping_add(0x3df3_b646);
        let s3 = s2.wrapping_add(0x40dd_e76d);
        let s4 = s3.wrapping_sub(0x68cd_851f);
        let s5 = s4.wrapping_add(0xd1a9_fbe7);
        Rng { s: [s0, s1, s2, s3, s4, s5], ring: [(0xffff, 0xffff); 32], at: 0 }
    }

    /// `0x555b50`: a chain of additions with carries over the six words, the last counting up.
    fn next(&mut self) -> u32 {
        let [s0, s1, s2, s3, s4, s5] = self.s;
        let a = s4.wrapping_add(s5);
        let c = u32::from(a < s5 || a < s4);
        let n4 = a;
        let a = a.wrapping_add(c.wrapping_add(s3));
        let c = u32::from(a < s3);
        let n3 = a;
        let a = a.wrapping_add(c.wrapping_add(s2));
        let c = u32::from(a < s2);
        let n2 = a;
        let a = a.wrapping_add(c.wrapping_add(s1));
        let c = u32::from(a < s1);
        let n1 = a;
        let mut n = [s0.wrapping_add(a.wrapping_add(c)), n1, n2, n3, n4, s5.wrapping_add(1)];
        if n[5] == 0 {
            for k in (0..5).rev() {
                n[k] = n[k].wrapping_add(1);
                if n[k] != 0 {
                    break;
                }
            }
        }
        self.s = n;
        n[0]
    }

    /// `0x555d50(n, tag)`: a number below `n`. With a tag, up to 32 draws look for one not among the tag's last
    /// `min(n / 2, 10)` (or keep the least recent) and that one is remembered, though the last draw is what comes
    /// back (as the original returns it).
    pub fn draw(&mut self, n: u32, tag: Option<u16>) -> u16 {
        let first = self.below(n);
        let Some(tag) = tag else { return first };
        let depth = (n / 2).min(10) as usize;
        let (mut best, mut keep, mut last) = (-1i32, first, first);
        for _ in 0..32 {
            let v = self.below(n);
            last = v;
            match self.recent(depth, v, tag) {
                None => {
                    keep = v;
                    break;
                }
                Some(p) if p as i32 > best => (best, keep) = (p as i32, v),
                Some(_) => {}
            }
        }
        self.at = (self.at + 1) % 32;
        self.ring[self.at] = (keep, tag);
        last
    }

    fn below(&mut self, n: u32) -> u16 {
        ((self.next() >> 16).wrapping_mul(n) >> 16) as u16
    }

    /// `0x555c40`: where `v` is among the last `depth` draws with `tag` (0 the latest), if it is.
    fn recent(&self, depth: usize, v: u16, tag: u16) -> Option<usize> {
        let (mut i, mut count) = (self.at, 0);
        for _ in 0..=32 {
            if count >= depth {
                return None;
            }
            if self.ring[i].1 == tag {
                count += 1;
                if self.ring[i].0 == v {
                    return Some(count - 1);
                }
            }
            i = (i + 31) % 32;
        }
        None
    }
}

/// The speech events of `path` (`Events.evt`), or none (said why).
pub(crate) fn events(vfs: &Vfs, path: &str) -> Option<Vec<Event>> {
    let evt = vfs.read(path).map_err(|e| eprintln!("{path}: {e}")).ok()?;
    Some(qwc_formats::speech::parse_evt(&evt).map_err(|e| eprintln!("{path}: {e}")).ok()?.events)
}

/// The clip banks of `Audio/<lang>/hdr/<folder>` for each of `folders`, for speech session `session`. Where two
/// `.hdr` files give the same file (`LB`'s `MCName06` and `MCNames06`, ...), the one the file's trailer repeats is
/// kept (**[I]** the original loads both and its binary search finds either).
pub(crate) fn banks(vfs: &Vfs, lib: &Library, folders: &[&str], session: u32) -> Vec<Loaded> {
    let prefix = format!("audio\\{}\\dat\\", LANG.to_ascii_lowercase());
    let mut banks: Vec<(Loaded, Vec<u8>)> = Vec::new();
    for folder in folders {
        let dir = format!("Audio/{LANG}/hdr/{folder}");
        let paths: Vec<String> = vfs.list_dir(&dir).map(str::to_string).collect();
        for path in paths.iter().filter(|p| p.to_ascii_lowercase().ends_with(".hdr")) {
            let loaded = vfs.read(path).ok().and_then(|b| {
                let (vpath, _, data) = qwc_formats::audio::read_hdr(&b).ok()?;
                let bank = qwc_formats::speech::parse_bank(data).ok()?;
                let lower = vpath.to_ascii_lowercase();
                let dat = lower.strip_prefix(&prefix).map(|_| vpath[prefix.len()..].to_string())?;
                let name = path.rsplit(['/', '\\']).next().unwrap_or(path).to_string();
                Some((Loaded::new(bank, dat, name, session), data.to_vec()))
            });
            match loaded {
                Some(l) => banks.push(l),
                None => eprintln!("{path}: not a speech bank"),
            }
        }
    }
    let same = |a: &Loaded, b: &Loaded| a.dat.eq_ignore_ascii_case(&b.dat);
    let shared: Vec<bool> = banks.iter().map(|(a, _)| banks.iter().filter(|(b, _)| same(a, b)).count() > 1).collect();
    let mut kept = Vec::new();
    for ((l, data), shared) in banks.into_iter().zip(shared) {
        if shared {
            let trailer = lib.speech_sentences(vfs, LANG, &l.dat).unwrap_or_default();
            if !trailer.is_empty() && !data.starts_with(&trailer) {
                continue;
            }
        }
        kept.push(l);
    }
    kept
}

/// Say `line` of `engine`: its clips decoded and played back to back on the speech bus at `gain`; the voice, when
/// a clip decoded. `log` names who says it in the log.
pub(crate) fn say(
    engine: &Engine,
    line: &Line,
    lib: &Library,
    vfs: &Vfs,
    mixer: &std::sync::Mutex<Mixer>,
    gain: f32,
    log: Option<&str>,
) -> Option<VoiceId> {
    let mut sounds = Vec::new();
    for &(b, c) in &line.clips {
        let l = &engine.banks[b];
        let clips = &l.bank.clips;
        let end = clips.get(c + 1).map_or(l.bank.size, |n| n.offset);
        match lib.speech_clip(vfs, LANG, &l.dat, clips[c].offset, end) {
            Ok(s) => sounds.push(s),
            Err(e) => eprintln!("speech: {e}"),
        }
    }
    if let Some(who) = log {
        let name = engine.events.iter().find(|e| e.id == line.event).map_or("?", |e| e.name.as_str());
        let parts: Vec<String> = line.clips.iter().map(|&(b, c)| format!("{} {c}", engine.banks[b].name)).collect();
        eprintln!("{who}: {name}: {}", parts.join(", "));
    }
    let mut it = sounds.into_iter();
    let first = it.next()?;
    let mut mx = mixer.lock().unwrap();
    let v = mx.play(first, Params { gain, bus: Bus::Speech, ..Params::default() });
    for s in it {
        mx.queue(v, s);
    }
    Some(v)
}

/// A clip bank as loaded (`0x553fb0`): the bank, its clips' file and its state.
pub(crate) struct Loaded {
    pub bank: Bank,
    /// The bank's `.dat` file as its `.hdr` names it after `Audio\<lang>\dat\` (`LB\MCn_Goal.dat`).
    pub dat: String,
    /// The bank's file name, for the log.
    pub name: String,
    /// The no-repeat ring (`0x553e30`, `0xff` for none) and where the next goes.
    ring: Vec<u8>,
    cursor: usize,
    /// The clips a partitioned bank allows this session (`0x553f30`); each is cleared once said.
    enabled: Option<Vec<bool>>,
}

impl Loaded {
    pub fn new(bank: Bank, dat: String, name: String, session: u32) -> Loaded {
        let n = bank.clips.len();
        // partition `session % parts` of `parts` equal runs
        let enabled = bank.parts.filter(|&p| p > 0).map(|p| {
            let (p, k) = (p as usize, session as usize % p as usize);
            let run = n * k / p..n * (k + 1) / p;
            (0..n).map(|c| run.contains(&c)).collect()
        });
        Loaded { ring: vec![0xff; bank.history as usize], cursor: 0, enabled, bank, dat, name }
    }

    /// How lately clip `c` was said (`0x553e70`): 1 the last one, 0 not lately.
    fn rank(&self, c: usize) -> usize {
        let h = self.ring.len();
        (0..h).find(|&d| self.ring[(self.cursor + h - 1 - d) % h] as usize == c).map_or(0, |d| d + 1)
    }

    /// Clip `c` was said: into the ring (`0x553e30`) and out of the partition.
    fn said(&mut self, c: usize) {
        let h = self.ring.len();
        if h > 0 {
            self.ring[self.cursor] = c as u8;
            self.cursor = (self.cursor + 1) % h;
        }
        if let Some(e) = self.enabled.as_mut().and_then(|e| e.get_mut(c)) {
            *e = false;
        }
    }

    fn allowed(&self, c: usize) -> bool {
        self.enabled.as_ref().is_none_or(|e| e.get(c).copied().unwrap_or(false))
    }
}

/// An event waiting to be said: its arguments (`args[0]` the event id, `args[s]` the `s`-th) and its post order.
#[derive(Debug, Clone)]
struct Slot {
    event: usize,
    args: Vec<u32>,
    counter: u16,
}

/// A line to say: the event and its clips (bank, clip), in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Line {
    pub event: u16,
    pub clips: Vec<(usize, usize)>,
}

/// The game's callbacks the engine calls.
pub(crate) trait Host {
    /// The game's test `id` on an argument's value (`0x522d90`), `None` when it cannot tell.
    fn test(&mut self, id: u8, value: u32) -> Option<bool>;
    /// The game's filter (`0x522a90`): whether event `id` may be said now, with the channel's mode for it.
    fn filter(&mut self, id: u16) -> Option<u8>;
}

/// The speech engine with one channel (the match's).
pub(crate) struct Engine {
    pub events: Vec<Event>,
    pub banks: Vec<Loaded>,
    slots: [Option<Slot>; 16],
    counter: u16,
    /// The channel's mode (`0x66d4d0`): 2 allows the variants for one speaker (mode 1), else those for two
    /// (mode 2); mode 0 variants play either way.
    mode: u8,
    /// The event said last and how many times in a row (`0x66d4f8`, `0x66d4fc`).
    last: Option<(u16, u32)>,
    rng: Rng,
}

impl Engine {
    pub fn new(events: Vec<Event>, banks: Vec<Loaded>, session: u32) -> Engine {
        Engine { events, banks, slots: Default::default(), counter: 0, mode: 0, last: None, rng: Rng::new(session) }
    }

    fn event(&self, id: u16) -> Option<usize> {
        self.events.iter().position(|e| e.id == id)
    }

    /// Post event `id` (`0x553250` -> `0x553110`): it speaks at all by its chance, then waits in a free slot (or
    /// one of no higher priority). An event flagged `0x20` is said at once (`0x552fd0`): its line comes back.
    pub fn post(&mut self, host: &mut impl Host, id: u16, args: &[u32]) -> Option<Line> {
        let ei = self.event(id)?;
        let e = &self.events[ei];
        let (chance, priority, flags, n) = (e.chance, e.t1, e.flags, e.args as usize);
        if u32::from(self.rng.draw(100, None)) > u32::from(chance) {
            return None;
        }
        let s = self.free_slot(priority)?;
        self.counter = self.counter.wrapping_add(1);
        let mut a = vec![u32::from(id)];
        a.extend((0..n).map(|i| args.get(i).copied().unwrap_or(0)));
        self.slots[s] = Some(Slot { event: ei, args: a, counter: self.counter });
        if flags & 0x20 == 0 {
            return None;
        }
        let slot = self.slots[s].take()?;
        self.play_slot(&slot, host)
    }

    /// `0x553050`: a free slot, else the first whose event matters no more than `priority`.
    fn free_slot(&self, priority: u16) -> Option<usize> {
        self.slots
            .iter()
            .position(Option::is_none)
            .or_else(|| self.slots.iter().position(|s| s.as_ref().is_some_and(|s| self.events[s.event].t1 <= priority)))
    }

    /// When nothing is being said (`0x553530`): the next line, if an event plays.
    pub fn next(&mut self, host: &mut impl Host) -> Option<Line> {
        loop {
            let s = self.pick()?;
            let slot = self.slots[s].take()?;
            let Some(mode) = host.filter(self.events[slot.event].id) else { continue };
            self.mode = mode;
            // the slots posted before it go, unless their flag 4 keeps them (`0x552f30`)
            for o in &mut self.slots {
                if o.as_ref().is_some_and(|o| o.counter < slot.counter && self.events[o.event].flags & 4 == 0) {
                    *o = None;
                }
            }
            return self.play_slot(&slot, host);
        }
    }

    /// `0x5534c0`: after an event with follow-ups, the best of those waiting; else the best of all.
    fn pick(&self) -> Option<usize> {
        let linked = self.last.and_then(|(id, _)| self.event(id)).and_then(|e| self.events[e].linked.as_deref());
        linked.and_then(|l| self.best(Some(l))).or_else(|| self.best(None))
    }

    /// `0x553330`: the waiting slot of the highest priority (`t1`), the newest among equals.
    fn best(&self, only: Option<&[u16]>) -> Option<usize> {
        let mut best: Option<(usize, u16, u16)> = None;
        for (i, s) in self.slots.iter().enumerate() {
            let Some(s) = s else { continue };
            let e = &self.events[s.event];
            if only.is_some_and(|l| !l.contains(&e.id)) {
                continue;
            }
            if best.is_none_or(|(_, p, c)| e.t1 > p || (e.t1 == p && s.counter > c)) {
                best = Some((i, e.t1, s.counter));
            }
        }
        best.map(|b| b.0)
    }

    /// `0x555a40`: say a slot's event; when no variant fits, once more.
    fn play_slot(&mut self, slot: &Slot, host: &mut impl Host) -> Option<Line> {
        self.play(slot, host).or_else(|| self.play(slot, host))
    }

    /// `0x5557f0`: the first variant that fits, its clips chosen and remembered.
    fn play(&mut self, slot: &Slot, host: &mut impl Host) -> Option<Line> {
        let ei = slot.event;
        let (eid, max) = (self.events[ei].id, self.events[ei].max_repeats);
        if self.last.is_some_and(|(l, n)| l == eid && max > 0 && n >= u32::from(max)) {
            return None;
        }
        let args = &slot.args;
        let (value, unknown) = self.tests(ei, args, host);
        let order = self.shuffle(ei);
        let e = &self.events[ei];
        let group = (e.flags & 8 != 0).then(|| group_of(e, args));
        for vi in order {
            let e = &self.events[ei];
            let Some(v) = e.variants.get(vi) else { break };
            if self.rng.draw(100, None) >= u16::from(v.chance) {
                continue;
            }
            if !arg_tests(e, v, args) {
                continue;
            }
            if group.is_some_and(|g| e.groups.as_ref().and_then(|gs| gs.get(vi)).copied() != Some(g)) {
                continue;
            }
            let Some(parts) = self.build(v, args, value, unknown) else { continue };
            let mut clips = Vec::with_capacity(parts.len());
            for (b, cands) in parts {
                let k = self.rng.draw(cands.len() as u32, Some(b as u16)) as usize;
                clips.push((b, cands[k.min(cands.len() - 1)]));
            }
            for &(b, c) in &clips {
                self.banks[b].said(c);
            }
            self.last = match self.last {
                Some((l, n)) if l == eid => Some((l, n + 1)),
                _ => Some((eid, 1)),
            };
            return Some(Line { event: eid, clips });
        }
        None
    }

    /// `0x556170`: the event's tests as bits from 7 down, those on an argument first; and the bits no one could
    /// tell.
    fn tests(&self, ei: usize, args: &[u32], host: &mut impl Host) -> (u8, u8) {
        let (mut value, mut unknown) = (0u8, 0u8);
        let tests = &self.events[ei].tests;
        for on_arg in [true, false] {
            for (j, &[test, arg, _]) in tests.iter().enumerate().take(8) {
                if (arg != 0) != on_arg {
                    continue;
                }
                let bit = 0x80u8 >> j;
                let v = if arg != 0 { args.get(arg as usize).copied().unwrap_or(0) } else { 0 };
                match host.test(test, v) {
                    Some(true) => value |= bit,
                    Some(false) => {}
                    None => unknown |= bit,
                }
            }
        }
        (value, unknown)
    }

    /// `0x5548e0`: the variants in a weighted shuffle, those of no weight last.
    fn shuffle(&mut self, ei: usize) -> Vec<usize> {
        let mut w: Vec<u32> = self.events[ei].variants.iter().map(|v| v.weight).collect();
        let mut total: u32 = w.iter().sum();
        let mut order = Vec::with_capacity(w.len());
        while total > 0 {
            let mut r = i64::from(self.rng.draw(total, None));
            let Some(i) = (0..w.len()).find(|&i| {
                r -= i64::from(w[i]);
                r < 0
            }) else {
                break;
            };
            order.push(i);
            total -= w[i];
            w[i] = 0;
        }
        order.extend(self.events[ei].variants.iter().enumerate().filter(|(_, v)| v.weight == 0).map(|(i, _)| i));
        order
    }

    /// `0x554ca0`: each part's bank and the clips it may say, if the variant fits the channel's mode and the tests.
    fn build(&self, v: &Variant, args: &[u32], value: u8, unknown: u8) -> Option<Vec<(usize, Vec<usize>)>> {
        if v.parts.len() > 12 {
            return None;
        }
        let mode_ok = match v.mode {
            0 => true,
            1 => self.mode == 2,
            2 => self.mode != 2,
            _ => false,
        };
        // `0x5562e0`: the tested bits someone could tell must be the variant's
        let m = v.test_mask & !unknown;
        if !mode_ok || m & value != m & v.test_value {
            return None;
        }
        v.parts
            .iter()
            .map(|p| {
                let b = self.bank_for(p, args)?;
                let c = self.candidates(b, p, args);
                (!c.is_empty()).then_some((b, c))
            })
            .collect()
    }

    /// `0x5543d0`: a part's bank by id; by id and the team in argument `arg` for bank mode 1. (Mode 2's team rows,
    /// `0x553d40`, need the bank's row count, 0 in every file: the bank by id.)
    fn bank_for(&self, p: &Part, args: &[u32]) -> Option<usize> {
        let team = args.get(p.arg as usize).copied().unwrap_or(0);
        let id = |b: &&Loaded| b.bank.id == p.bank;
        match p.bank_mode {
            1 => self.banks.iter().position(|b| id(&b) && b.bank.team.map(u32::from) == Some(team)),
            0 | 2 => self
                .banks
                .iter()
                .position(|b| id(&b) && b.bank.team.is_none())
                .or_else(|| self.banks.iter().position(|b| id(&b))),
            _ => None,
        }
    }

    /// `0x554660`: the clips of bank `b` the part allows that were not said lately (at most 200); failing those,
    /// the least recent of the others.
    fn candidates(&self, b: usize, p: &Part, args: &[u32]) -> Vec<usize> {
        let l = &self.banks[b];
        let (mut out, mut fallback) = (Vec::new(), None::<(usize, usize)>);
        for (c, clip) in l.bank.clips.iter().enumerate() {
            if !keys_ok(p, clip, args) || !l.allowed(c) {
                continue;
            }
            match l.rank(c) {
                0 if out.len() < 200 => out.push(c),
                0 => {}
                r if fallback.is_none_or(|(best, _)| r > best) => fallback = Some((r, c)),
                _ => {}
            }
        }
        if out.is_empty() {
            out.extend(fallback.map(|f| f.1));
        }
        out
    }
}

/// `0x5542d0`: each of the part's keys allows the clip's value and, for a key on an argument, the argument has the
/// value's bit. (The remembered values, sources `0xfe` and `0xff`, are not used by the match's events.)
fn keys_ok(p: &Part, clip: &BankClip, args: &[u32]) -> bool {
    p.keys.iter().enumerate().all(|(j, k)| {
        let Some(&v) = clip.keys.get(j) else { return false };
        let bit = 1u32.checked_shl(u32::from(v)).unwrap_or(0);
        k.allowed & bit != 0
            && match k.source {
                0 | 0xfe | 0xff => true,
                s => args.get(s as usize).is_some_and(|a| a & bit != 0),
            }
    })
}

/// `0x555750`: the variant's masks against the event's argument tests' arguments.
fn arg_tests(e: &Event, v: &Variant, args: &[u32]) -> bool {
    e.arg_tests.iter().enumerate().all(|(i, t)| {
        let m = v.masks.get(i).copied().unwrap_or(0);
        m == 0 || args.get(t[0] as usize).is_some_and(|a| a & m != 0)
    })
}

/// `0x555ac0`: the group the arguments make, a bit per argument by its two-bit test (1 set when not 0, 2 when not
/// -1).
fn group_of(e: &Event, args: &[u32]) -> u16 {
    let modes = e.arg_modes.unwrap_or(0);
    (0..16).fold(0, |g, i| {
        let a = args.get(i + 1).copied().unwrap_or(0);
        let on = match (modes >> (2 * i)) & 3 {
            1 => a != 0,
            2 => a != u32::MAX,
            _ => false,
        };
        g | (u16::from(on) << i)
    })
}

/// What the match says about itself to the commentary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Setup {
    /// The teams' codes (`0x51fb40`: 1 Gryffindor ... 13 Bulgaria; 0 none).
    pub teams: [u32; 2],
    /// The stadium's code (`0x51fa00`: 1 Hogwarts ... 11 Queerditch).
    pub stadium: u32,
    /// The second commentator's model in the match's crowd cutscene record (`Jeune`, `Vogleur`, `Gudabi`).
    pub colour: Option<String>,
    /// The referee's nationality's index (`0x48b2e0`: 0 AUS, 1 FRA, 2 ENG, 3 GER, ... 7 NDK, ...).
    pub referee: Option<u32>,
    /// The teams the players play (the team records' `+0` byte, **[I]**).
    pub human: [bool; 2],
    /// The speech session (`0xf913fc`): one per match this run.
    pub session: u32,
    /// A cup's match (`[0xf94d2c]` 3 or 4: the House and World Cups), where special moves are commentated.
    pub cup: bool,
}

impl Setup {
    /// `[0xf94d20]`: an international match (Ludo Bagman speaks), not at Hogwarts or Queerditch (`0x5227c0`).
    pub fn international(&self) -> bool {
        !matches!(self.stadium, 1 | 11)
    }

    /// `[0xf94d24]` (`0x5227c0`): 2 Lee Jordan and Seamus Finnigan, else Ludo Bagman with 3 Jeune, 4 Vogleur or 5
    /// Gudabi (the record's second commentator; Gudabi failing the others).
    pub fn pair(&self) -> u32 {
        let c = self.colour.as_deref().unwrap_or("");
        if !self.international() {
            2
        } else if c.eq_ignore_ascii_case("Jeune") {
            3
        } else if c.eq_ignore_ascii_case("Vogleur") {
            4
        } else {
            5
        }
    }

    /// The pair's speech folders: the match commentator's, the colour commentator's.
    pub fn folders(&self) -> [&'static str; 2] {
        match self.pair() {
            2 => ["LJ", "SF"],
            3 => ["LB", "JL"],
            4 => ["LB", "AV"],
            _ => ["LB", "MG"],
        }
    }

    /// `0x523870`: the colour commentator as a mask.
    fn colour_mask(&self) -> u32 {
        match self.pair() {
            2 => 8,
            3 => 1,
            4 => 2,
            _ => 4,
        }
    }
}

/// The setup for a match of `teams` (folders) at `stadium` (a folder) with its crowd cutscene record, if it has one.
pub(crate) fn setup(
    teams: &[String; 2],
    stadium: &str,
    record: Option<(&crate::ccs::Tables, &crate::ccs::Record)>,
    human: [bool; 2],
) -> Setup {
    let colour = record.and_then(|(t, r)| t.cast(r).commentators[1].clone());
    Setup {
        teams: [team_code(&teams[0]), team_code(&teams[1])],
        stadium: stadium_code(stadium),
        colour,
        referee: record.and_then(|(_, r)| nationality(&r.fields[4])),
        human,
        session: 0,
        cup: false,
    }
}

/// `0x5237c0` for an uber move: its team's bit (as a bank key: Gryffindor 30, Ravenclaw 24, Hufflepuff 18, Slytherin
/// 26, Spain 6, England 23, Germany 14, France 15, USA 19, Japan 11, Nordic 3, Australia 21, Bulgaria 27), else
/// bit 31 (any other special move's).
fn uber_bit(code: u32) -> u32 {
    let bit = match code {
        1 => 30,
        2 => 24,
        3 => 18,
        4 => 26,
        5 => 6,
        6 => 23,
        7 => 14,
        8 => 15,
        9 => 19,
        10 => 11,
        11 => 3,
        12 => 21,
        13 => 27,
        _ => 31,
    };
    1 << bit
}

/// A team folder's code (`0x51fb40`'s, **[I]** by the folders they stand for), 0 for none.
pub(crate) fn team_code(folder: &str) -> u32 {
    match folder.to_ascii_lowercase().as_str() {
        "gryff" => 1,
        "raven" => 2,
        "huffle" => 3,
        "slyth" => 4,
        "spain" => 5,
        "england" => 6,
        "germany" => 7,
        "france" => 8,
        "usa" => 9,
        "japan" => 10,
        "nordic" => 11,
        "aus" => 12,
        "bulgaria" => 13,
        _ => 0,
    }
}

/// A stadium folder's code (`0x51fa00`'s, **[I]** by folder), 0 for none.
pub(crate) fn stadium_code(folder: &str) -> u32 {
    match folder.to_ascii_lowercase().as_str() {
        "hog_sum" | "hog_win" => 1,
        "spain" => 2,
        "england" => 3,
        "germany" => 4,
        "france" => 5,
        "usa" => 6,
        "japan" => 7,
        "nordic" => 8,
        "aus" => 9,
        "bulgaria" => 10,
        "qditch" => 11,
        _ => 0,
    }
}

/// A nationality code's index (`0x48b2e0`), the referee's in a crowd cutscene record.
pub(crate) fn nationality(code: &str) -> Option<u32> {
    const CODES: [&str; 14] =
        ["AUS", "FRA", "ENG", "GER", "GRY", "HUF", "JPN", "NDK", "RAV", "SLY", "USA", "SPA", "BUL", "EDG"];
    CODES.iter().position(|c| c.eq_ignore_ascii_case(code)).map(|i| i as u32)
}

/// `0x523580`: a team code as a bit (Ravenclaw's and Hufflepuff's swapped).
fn team_bit(code: u32) -> u32 {
    match code {
        1 => 1,
        3 => 2,
        2 => 4,
        4..=13 => 1 << (code - 1),
        _ => 0,
    }
}

/// `0x5236a0`: a player's side as a mask (`+0x20` 0 -> 2, 1 -> 1; **[I]** team 0's players have side 0).
fn side(team: usize) -> u32 {
    if team == 0 {
        2
    } else {
        1
    }
}

/// `0x5236c0`: a player as a mask: chasers 1, 2, 4, beaters 8, `0x10`, the seeker `0x20`, the keeper `0x40`.
fn player_mask(m: &Match, p: usize) -> u32 {
    let Some(pl) = m.players.get(p) else { return 0 };
    let t = &m.teams[pl.team];
    match pl.role {
        Role::Chaser => t.chasers.iter().position(|&q| q == p).map_or(0, |i| 1 << i),
        Role::Beater => t.beaters.iter().position(|&q| q == p).map_or(0, |i| 8 << i),
        Role::Seeker => 0x20,
        Role::Keeper => 0x40,
    }
}

/// The game's side: its state and the handlers (`0x523ce0`, `0x524880`, ...).
struct Game {
    setup: Setup,
    /// The names spoken of lately (`0xf97998`: five of team code and player mask, the newest at the cursor) and
    /// the last two said by a name test (`+0x30`).
    names: [(u32, u32); 5],
    names_at: usize,
    said: [(u32, u32); 2],
    said_at: usize,
    /// The filter's last event let through (`0x5cff68`).
    last: Option<u16>,
    /// The Seeker game's filter (`0xf91409`) and the goals-only one (`0xf91408`).
    seeker_game: bool,
    goal_only: bool,
    /// `Start_match` was said (`[0xf913f4] + 0x50`).
    started: bool,
    /// The handlers' random numbers (`0x5d0170`, from `0x4d41524b`).
    lcg: u32,
    /// The team records (`0xf94d30`, `0xf94d50`): each team's score as last seen and its chasers' goals.
    scores: [i32; 2],
    goals: [[u32; 3]; 2],
    /// `[0xf91410]` (`0x51fed0`): a quarter-step of the Seeker race's state (`0x1043ad0`); **[I]** 0 throughout.
    bucket: u32,
    /// Each team's last shooter, the team that caught the Snitch.
    shooters: [Option<usize>; 2],
    catcher: Option<usize>,
    /// The team that had the Quaffle last (the game's `+0x3d4`): a possession by the other is a steal.
    possessing: Option<usize>,
    /// The Seeker race as the lines follow it: who was well ahead (`0x10439ec`, `0x10439ed`), who took the lead last
    /// (the team records' `+1`) and who boosts (`+2`).
    ahead: [bool; 2],
    leading: [bool; 2],
    boosting: [bool; 2],
}

impl Host for Game {
    fn test(&mut self, id: u8, value: u32) -> Option<bool> {
        match id {
            // `0x522e17`: the name `value` stands for (the newest of the names spoken of with that mask) is not one
            // of the last two said; it is said now
            1 => {
                let at = self.names_at;
                let team = (0..5).map(|k| self.names[(at + k) % 5]).find(|n| n.1 == value).map_or(0, |n| n.0);
                if self.said.contains(&(team, value)) {
                    return Some(false);
                }
                self.said_at = (self.said_at + 1) % 2;
                self.said[self.said_at] = (team, value);
                Some(true)
            }
            // the argument is 1, or the race state is 1 or 2
            2 => Some(value == 1 || matches!(self.bucket, 1 | 2)),
            _ => Some(false),
        }
    }

    fn filter(&mut self, id: u16) -> Option<u8> {
        let mut mode = 0;
        if self.last == Some(id::MATCH_RESULT) || (self.goal_only && id != id::GOAL) {
            return None;
        }
        let refused = if self.seeker_game {
            // only the Seeker game's events, and the result right after the catch
            match id {
                9..=13 | 21..=24 => false,
                id::MATCH_RESULT => self.last != Some(id::SNITCH_CAUGHT),
                _ => true,
            }
        } else {
            match id {
                id::INTRO => {
                    if self.setup.international() {
                        mode = 2;
                    }
                    false
                }
                id::TACKLE | id::INTERCEPT_PASS => {
                    matches!(self.last, Some(l) if l == id || l == id::GOAL || l == id::GOAL_FOLLOW)
                }
                id::INTRO_CC => self.last != Some(id::INTRO),
                // **[I]** its other condition, a timer (`0x1004970`) past 6, is not read
                id::REFEREE => {
                    if self.setup.international() {
                        mode = 2;
                    }
                    self.last != Some(id::INTRO_CC)
                }
                id::NEG_LINK => self.last != Some(id::TEAM_REVIEW),
                id::WAX_LYRICAL => self.setup.session.is_multiple_of(2),
                _ => false,
            }
        };
        if refused {
            return None;
        }
        self.last = Some(id);
        Some(mode)
    }
}

impl Game {
    fn new(setup: Setup) -> Game {
        Game {
            setup,
            names: [(0, 0); 5],
            names_at: 0,
            said: [(0, 0); 2],
            said_at: 0,
            last: None,
            seeker_game: false,
            goal_only: false,
            started: false,
            lcg: 0x4d41_524b,
            scores: [0; 2],
            goals: [[0; 3]; 2],
            bucket: 0,
            shooters: [None; 2],
            catcher: None,
            possessing: None,
            ahead: [false; 2],
            leading: [false; 2],
            boosting: [false; 2],
        }
    }

    /// One step of the handlers' generator; whether it falls on a multiple of `n` ("one time in `n`").
    fn one_in(&mut self, n: u32) -> bool {
        self.lcg = self.lcg.wrapping_mul(0x10dcd).wrapping_add(0x29);
        self.lcg.is_multiple_of(n.max(1))
    }

    /// A name spoken of: into the ring of five (`0x522d60`), the newest before the others.
    fn spoken(&mut self, team: u32, mask: u32) {
        self.names_at = (self.names_at + 4) % 5;
        self.names[self.names_at] = (team, mask);
    }

    fn code(&self, team: usize) -> u32 {
        self.setup.teams[team.min(1)]
    }

    /// `0x523730`: the style key a line takes, by the race state and whether the match is international.
    fn mode(&self, code: u32, flag: bool) -> u32 {
        let (flag, b) = (flag && code != 5, self.bucket);
        match (self.setup.international(), flag) {
            (false, true) if b == 3 => 8,
            (false, true) if b > 3 => 0,
            (false, _) => 4,
            (true, true) => [8, 2, 4, 0x10].get(b as usize).copied().unwrap_or(0),
            (true, false) => [2, 2, 4, 4].get(b as usize).copied().unwrap_or(0),
        }
    }

    /// The intro (`0x51ef90`): `Intro` (the stadium, the teams, the colour commentator at Hogwarts), `Intro_CC`
    /// and the `Referee` (an international's named referee: French 1, German 2, Nordic 4).
    fn intro(&mut self, e: &mut Engine) -> Vec<Line> {
        let st = &self.setup;
        let stadium = if (1..=10).contains(&st.stadium) { 1 << (st.stadium - 1) } else { 0 };
        let cc = st.colour_mask();
        let intl = st.international();
        let teams = [team_bit(st.teams[0]), team_bit(st.teams[1])];
        let referee = match st.referee {
            _ if !intl => 0,
            Some(1) => 1,
            Some(3) => 2,
            Some(7) => 4,
            _ => 0,
        };
        let mut out = Vec::new();
        out.extend(e.post(self, id::INTRO, &[stadium, teams[0], teams[1], if intl { 0 } else { cc }]));
        out.extend(e.post(self, id::INTRO_CC, &[cc, stadium]));
        if stadium != 0 {
            out.extend(e.post(self, id::REFEREE, &[referee]));
        }
        out
    }

    /// The match's events since the last call.
    fn events(&mut self, e: &mut Engine, m: &Match, events: &[MatchEvent]) -> Vec<Line> {
        let mut out = Vec::new();
        for ev in events {
            match *ev {
                MatchEvent::Pass { from, receiver, .. } => out.extend(self.pass(e, m, from, receiver)),
                MatchEvent::Shot { player, .. } => {
                    if let Some(t) = m.players.get(player).map(|p| p.team) {
                        self.shooters[t] = Some(player);
                    }
                    out.extend(self.shot(e, m, player));
                }
                MatchEvent::Goal { team, .. } => out.extend(self.goal(e, m, team)),
                // a possession (`0x431c40`): a steal when the other team had the ball; a tackle's by the tackle
                MatchEvent::Catch { player, .. } | MatchEvent::Touch { player, .. } => {
                    if let MatchEvent::Touch { state: 9, .. } = *ev {
                        if m.players.get(player).is_some_and(|p| p.role == Role::Keeper) {
                            out.extend(self.save(e, m, player));
                        }
                    }
                    let steal = self.took(m, player);
                    out.extend(self.possession(e, m, player, steal, false));
                }
                MatchEvent::Tackle { tackler, .. } | MatchEvent::SpecialTackle { tackler, .. } => {
                    self.took(m, tackler);
                    out.extend(self.possession(e, m, tackler, true, true))
                }
                MatchEvent::KickOff { team, .. } => self.possessing = Some(team),
                MatchEvent::SpecialMove { team, player, .. } => out.extend(self.special(e, m, team, player, false)),
                MatchEvent::UberMove { team, player, .. } => out.extend(self.special(e, m, team, player, true)),
                MatchEvent::SeekerGame { .. } => {
                    // **[I]** the Seeker game's router (`0x1043ad4`, `0x5234e0`) and the Snitch's kind 0 come with it
                    self.seeker_game = true;
                    out.extend(e.post(self, id::SNITCH_SPOTTED, &[]));
                }
                MatchEvent::Race { event: RaceEvent::Caught { team }, .. } => self.catcher = Some(team),
                // a grab that missed (`0x4489f0`, kind 7); a barge that landed (`0x448d10`, kind 6, **[I]** for the
                // barging seeker)
                MatchEvent::Race { event: RaceEvent::Missed { team }, .. } => {
                    out.extend(self.seeker(e, id::SEEKER_CLOSE, team))
                }
                MatchEvent::Race { event: RaceEvent::Hit { team }, .. } => {
                    out.extend(self.seeker(e, id::SEEKER_SHOVE, 1 - team))
                }
                _ => {}
            }
        }
        out
    }

    /// A special move in a cup (`0x523c30`, router `0x1043b58`): the move (an uber move's by its team, `0x5237c0`;
    /// any other bit 31), the team code, the player, 4. **[I]** said as the move starts (the router's sender
    /// `0x4513a0` is not read).
    fn special(&mut self, e: &mut Engine, m: &Match, team: usize, player: usize, uber: bool) -> Option<Line> {
        if !self.setup.cup {
            return None;
        }
        let (code, mask) = (self.code(team), player_mask(m, player));
        let mask = if mask == 0 { 0x80 } else { mask };
        let mv = if uber { uber_bit(code) } else { 1 << 31 };
        self.spoken(code, mask);
        e.post(self, id::SPECIAL_MOVE, &[mv, code, mask, 4])
    }

    /// A Seeker race line about team `t`'s seeker (`0x524aa0`): the team, the seeker, 4, the side.
    fn seeker(&mut self, e: &mut Engine, event: u16, t: usize) -> Option<Line> {
        let code = self.code(t);
        self.spoken(code, 0x20);
        e.post(self, event, &[code, 0x20, 4, side(t)])
    }

    /// The race's tick (`0x44e920`, kinds 13, 4 and 5): a seeker more than a point ahead on the trail after the
    /// other was takes the lead (`Seeker_lead`); the leader starting to boost, `Seeker_boost`.
    fn race(&mut self, e: &mut Engine, m: &Match) -> Vec<Line> {
        let mut out = Vec::new();
        let Some(r) = &m.race else { return out };
        let d = r.seekers[0].param - r.seekers[1].param;
        if d.abs() > 1.0 {
            let t = usize::from(d < 0.0);
            self.ahead[t] = true;
            if std::mem::take(&mut self.ahead[1 - t]) {
                self.leading = [t == 0, t == 1];
                out.extend(self.seeker(e, id::SEEKER_LEAD, t));
            }
        }
        for t in 0..2 {
            let b = r.seekers[t].boosting;
            if b && !self.boosting[t] && self.leading[t] {
                out.extend(self.seeker(e, id::SEEKER_BOOST, t));
            }
            self.boosting[t] = b;
        }
        out
    }

    /// `player` takes the Quaffle: whether his team did not have it.
    fn took(&mut self, m: &Match, player: usize) -> bool {
        let Some(t) = m.players.get(player).map(|p| p.team) else { return false };
        self.possessing.replace(t) != Some(t)
    }

    /// A pass by a chaser (`0x5242d0`), one time in 3: its kind by the throw against the passer's facing (across
    /// by more than 0.7: 8 to one side, 4 to the other), else 1 past 22 units, else 2.
    fn pass(&mut self, e: &mut Engine, m: &Match, from: usize, to: usize) -> Option<Line> {
        let (p, q) = (m.players.get(from)?, m.players.get(to)?);
        if p.role != Role::Chaser {
            return None;
        }
        let (dx, dz) = (q.pos[0] - p.pos[0], q.pos[2] - p.pos[2]);
        let (fx, fz) = (p.fwd[0], p.fwd[2]);
        let (dl, fl) = ((dx * dx + dz * dz).sqrt(), (fx * fx + fz * fz).sqrt());
        let across = if dl > 0.0 && fl > 0.0 { (dz * fx - fz * dx) / (dl * fl) } else { 0.0 };
        let kind = if across.abs() > 0.7 {
            if across > 0.0 {
                8
            } else {
                4
            }
        } else if dx * dx + dz * dz > 484.0 {
            1
        } else {
            2
        };
        let code = self.code(p.team);
        let mode = self.mode(code, false);
        let n = match self.bucket {
            0 => 3,
            1 => 2,
            _ => 1,
        };
        if !self.one_in(n) {
            return None;
        }
        e.post(self, id::PASS, &[side(p.team), code, mode, kind, 2, 0x2000])
    }

    /// A shot (`0x524880`), one time in 2.
    fn shot(&mut self, e: &mut Engine, m: &Match, player: usize) -> Option<Line> {
        let p = m.players.get(player)?;
        let (code, mask) = (self.code(p.team), player_mask(m, player));
        let mode = self.mode(code, false);
        if !self.one_in(if self.bucket <= 1 { 2 } else { 1 }) {
            return None;
        }
        self.spoken(code, mask);
        e.post(self, id::SHOT, &[0, side(p.team), code, mask, mode])
    }

    /// A save by the keeper (`0x524960`): kind 1 one time in 3, else 2 (**[I]** the message's flag for kind 4 is
    /// not read).
    fn save(&mut self, e: &mut Engine, m: &Match, keeper: usize) -> Option<Line> {
        let t = m.players.get(keeper)?.team;
        let code = self.code(t);
        let kind = if self.one_in(3) { 1 } else { 2 };
        let mode = self.mode(code, false);
        self.spoken(code, 0x40);
        e.post(self, id::SAVE, &[kind, side(t), code, 0x40, mode, 0])
    }

    /// A chaser takes the Quaffle (`0x524680`): the first time `Start_match`; a catch one time in 2 (`Catch`); a
    /// steal one time in 2, by a tackle (`Tackle`) or else (`Intercept_pass`).
    fn possession(&mut self, e: &mut Engine, m: &Match, player: usize, steal: bool, tackle: bool) -> Option<Line> {
        let p = m.players.get(player)?;
        if p.role != Role::Chaser {
            return None;
        }
        let (t, code, mask) = (p.team, self.code(p.team), player_mask(m, player));
        if !self.started {
            self.started = true;
            return e.post(self, id::START_MATCH, &[]);
        }
        if !steal {
            let mode = self.mode(code, true);
            if !self.one_in(if self.bucket <= 1 { 2 } else { 1 }) {
                return None;
            }
            self.spoken(code, mask);
            return e.post(self, id::CATCH, &[code, mask, mode]);
        }
        let mode = self.mode(code, false);
        let n = match self.bucket {
            0 | 2 => 2,
            1 => 3,
            _ => 1,
        };
        if !self.one_in(n) {
            return None;
        }
        if tackle {
            self.spoken(code, mask);
            e.post(self, id::TACKLE, &[side(t), mode, 2, code, mask])
        } else {
            e.post(self, id::INTERCEPT_PASS, &[side(t), mode, 0x20, 4])
        }
    }

    /// A goal (`0x523ce0`): `Goal` (the side, the team, the scorer, the style), then one time in 2 `GoalFollow`
    /// (also the other team, the state of the game, the scorer's tally, the lead as a milestone).
    fn goal(&mut self, e: &mut Engine, m: &Match, team: usize) -> Vec<Line> {
        let mut out = Vec::new();
        let (s0, s1) = (m.teams[0].score, m.teams[1].score);
        let first = self.scores[0] + self.scores[1] == 0;
        let (prev, diff) = ((self.scores[0] - self.scores[1]).abs(), (s0 - s1).abs());
        self.scores = [s0, s1];
        let scorer = self.shooters[team];
        let index = scorer.and_then(|p| m.teams[team].chasers.iter().position(|&q| q == p));
        if let Some(i) = index {
            self.goals[team][i] += 1;
        }
        let (code, other) = (self.code(team), self.code(1 - team));
        let leader = (s0 != s1).then_some(usize::from(s1 > s0));
        let trailing = leader.map(|l| 1 - l);
        let state = if first {
            1
        } else if trailing == Some(team) && prev > diff && diff == 10 {
            2
        } else if diff == 0 {
            4
        } else if leader == Some(team) && diff > prev && diff == 40 {
            0x10
        } else if leader == Some(team) && diff > prev && diff == 60 {
            8
        } else {
            0
        };
        let mode = self.mode(code, false);
        let chaser = index.map_or(0, |i| 1 << i);
        self.spoken(code, chaser);
        out.extend(e.post(self, id::GOAL, &[side(team), code, chaser, mode]));
        let n = [2, 3, 4, 5].get(self.bucket as usize).copied().unwrap_or(5);
        if self.one_in(n) {
            let milestone = match diff {
                d if leader == Some(team) && d % 10 == 0 && (10..=100).contains(&d) => 1u32 << (d / 10 - 1),
                _ => 0,
            };
            let tally = match index.map_or(0, |i| self.goals[team][i]) {
                0 => 0,
                g @ 1..=5 => 1 << g,
                _ => 1,
            };
            self.spoken(code, chaser);
            let args = [side(team), code, other, team_bit(other), state, tally, mode, milestone, chaser];
            out.extend(e.post(self, id::GOAL_FOLLOW, &args));
        }
        out
    }

    /// The catch cutscene's speech (`0x51f380`, `CATCHSNI`): `Snitch_caught` when the catching team wins.
    fn caught(&mut self, e: &mut Engine, m: &Match) -> Option<Line> {
        let t = self.catcher?;
        if m.teams[t].score <= m.teams[1 - t].score {
            return None;
        }
        e.post(self, id::SNITCH_CAUGHT, &[side(t), self.code(t), 0x20, 4])
    }

    /// The outro's start (`0x51f090`): `Match_result` for the winner, by whether a player's team won (`cup`: the
    /// cup's winning cutscene).
    fn result(&mut self, e: &mut Engine, m: &Match, cup: bool) -> Option<Line> {
        let (s0, s1) = (m.teams[0].score, m.teams[1].score);
        if s0 == s1 {
            return None;
        }
        let w = usize::from(s1 > s0);
        let (cw, cl) = (self.code(w), self.code(1 - w));
        let r5 = if self.one_in(5) { 1 } else { 2 };
        let args = if self.setup.human[w] {
            let a1 = match (cup, self.setup.international()) {
                (false, _) => 1,
                (true, false) => 0x80,
                (true, true) => 0x100,
            };
            [a1, 1, cw, 4, 0, team_bit(cw)]
        } else if (s0 - s1).abs() < 200 {
            [2, 2, cl, 4, r5, team_bit(cl)]
        } else {
            let a1 = if self.one_in(2) { 4 } else { 8 };
            [a1, 2, cl, 4, r5, team_bit(cl)]
        };
        e.post(self, id::MATCH_RESULT, &args)
    }
}

/// The match's commentary: the engine, the game's side, and the line being said.
pub(crate) struct Commentary {
    engine: Engine,
    game: Game,
    voice: Option<VoiceId>,
    /// The match's events already handled.
    seen: usize,
    log: bool,
}

impl Commentary {
    /// The commentary for a match: the events and both commentators' banks (`0x5227c0`, `0x5228e0`), or none (said
    /// why). Where two `.hdr` files give the same file (`LB`'s `MCName06` and `MCNames06`, ...), the one the file's
    /// trailer repeats is kept (**[I]** the original loads both and its binary search finds either).
    pub fn load(vfs: &Vfs, lib: &Library, setup: Setup) -> Option<Commentary> {
        let events = events(vfs, EVENTS)?;
        let banks = banks(vfs, lib, &setup.folders(), setup.session);
        let [mc, cc] = setup.folders();
        eprintln!("commentary: {mc} and {cc}, {} banks", banks.len());
        let engine = Engine::new(events, banks, setup.session);
        let log = std::env::var_os("QWC_AUDIO_LOG").is_some();
        Some(Commentary { engine, game: Game::new(setup), voice: None, seen: 0, log })
    }

    /// The commentator pair (`[0xf94d24]`, [`Setup::pair`]).
    pub fn pair(&self) -> u32 {
        self.game.setup.pair()
    }

    /// The match's new events.
    pub fn match_events(&mut self, m: &Match) -> Vec<Line> {
        let start = self.seen.min(m.events.len());
        self.seen = m.events.len();
        let mut out = self.game.events(&mut self.engine, m, &m.events[start..]);
        out.extend(self.game.race(&mut self.engine, m));
        out
    }

    /// A match cutscene starts (`0x51f1f0`): the intro's speak the intro, the outros the result.
    pub fn cutscene(&mut self, name: &str, m: &Match) -> Vec<Line> {
        let n = name.to_ascii_uppercase();
        let g = &mut self.game;
        match n.as_str() {
            "INTINTV1" | "INTINTV2" | "INTINTV4" | "SCHINTV1" | "SCHINTV2" => g.intro(&mut self.engine),
            "OUTROSCH" | "OUTROSC2" | "OUTROSC3" | "OUTROINT" => {
                g.result(&mut self.engine, m, false).into_iter().collect()
            }
            "WINHOG" | "WINWORLD" => g.result(&mut self.engine, m, true).into_iter().collect(),
            _ => Vec::new(),
        }
    }

    /// A cutscene's speech trigger (`0x51f380`): `SPEECH_<speaker><line><cutscene>` in `INTROV3` speaks the intro,
    /// in `CATCHSNI` the catch.
    pub fn trigger(&mut self, trigger: &str, m: &Match) -> Vec<Line> {
        let Some(cs) = trigger.strip_prefix("ON_AUDIO_SPEECH_").and_then(|r| r.get(5..)) else { return Vec::new() };
        let g = &mut self.game;
        match cs.to_ascii_uppercase().as_str() {
            "INTROV3" => g.intro(&mut self.engine),
            "CATCHSNI" => g.caught(&mut self.engine, m).into_iter().collect(),
            _ => Vec::new(),
        }
    }

    /// Lines said at once (an immediate event's) interrupt the one being said; then, when nothing is being said,
    /// the next line.
    pub fn update(&mut self, now: Vec<Line>, lib: &Library, vfs: &Vfs, mixer: &std::sync::Mutex<Mixer>) {
        if let Some(line) = now.into_iter().last() {
            if let Some(v) = self.voice.take() {
                mixer.lock().unwrap().stop(v);
            }
            self.say(&line, lib, vfs, mixer);
            return;
        }
        if self.voice.is_some_and(|v| mixer.lock().unwrap().is_playing(v)) {
            return;
        }
        self.voice = None;
        if let Some(line) = self.engine.next(&mut self.game) {
            self.say(&line, lib, vfs, mixer);
        }
    }

    /// Say a line: its clips decoded and played back to back on the speech bus.
    fn say(&mut self, line: &Line, lib: &Library, vfs: &Vfs, mixer: &std::sync::Mutex<Mixer>) {
        let log = self.log.then_some("commentary");
        if let Some(v) = say(&self.engine, line, lib, vfs, mixer, 1.0, log) {
            self.voice = Some(v);
        }
    }

    /// The match ends: silence.
    pub fn stop(&mut self, mixer: &std::sync::Mutex<Mixer>) {
        if let Some(v) = self.voice.take() {
            mixer.lock().unwrap().stop(v);
        }
    }

    /// A line's text for the log: the event's name and each clip's bank and index.
    fn describe(&self, line: &Line) -> String {
        let name = self.engine.events.iter().find(|e| e.id == line.event).map_or("?", |e| e.name.as_str());
        let parts: Vec<String> =
            line.clips.iter().map(|&(b, c)| format!("{} {c}", self.engine.banks[b].name)).collect();
        format!("{name}: {}", parts.join(", "))
    }

    /// How long a line takes to say (its clips decoded), in seconds.
    fn seconds(&self, line: &Line, lib: &Library, vfs: &Vfs) -> f64 {
        line.clips
            .iter()
            .filter_map(|&(b, c)| {
                let l = &self.engine.banks[b];
                let end = l.bank.clips.get(c + 1).map_or(l.bank.size, |n| n.offset);
                lib.speech_clip(vfs, LANG, &l.dat, l.bank.clips[c].offset, end).ok()
            })
            .map(|s| s.frames() as f64 / f64::from(s.rate.max(1)))
            .sum()
    }
}

/// `commentary TEAM0 TEAM1 [--stadium NAME] [--time SECS]`: a match between the AI's teams (folders of `teams/`)
/// with no sound, its commentary printed line by line as it would be said, each line taking its clips' time: the
/// intro first, then the match (`--time` seconds, default 120, or to the catch), then the catch and the result.
pub fn cli(args: &crate::Args, vfs: &Vfs) {
    let [t0, t1] = &args.models[..] else { crate::usage() };
    let stadium = args.stadium.clone().unwrap_or_else(|| "england".into());
    let teams = [t0.to_ascii_lowercase(), t1.to_ascii_lowercase()];
    let spec = crate::arena::MatchSpec {
        stadium: stadium.clone(),
        hoops: "hoopnorm".into(),
        teams: teams.clone(),
        skill: 1,
        music: crate::audio::match_score(&stadium).into(),
        sounds: None,
        desc: "quidd".into(),
    };
    let mut m = spec.start(vfs, 1);
    let lib = Library::open(vfs).unwrap_or_else(|e| crate::fail(e));
    let tables = crate::ccs::Tables::load(vfs);
    let codes = [crate::ccs::team_code(&teams[0]), crate::ccs::team_code(&teams[1])];
    let record = tables
        .as_ref()
        .and_then(|t| Some((t, &t.records[t.find("E", codes, crate::ccs::location_of_folder(&stadium))?])));
    let mut s = setup(&teams, &stadium, record, [false; 2]);
    s.session = 1;
    let Some(mut c) = Commentary::load(vfs, &lib, s) else { crate::fail("no commentary") };
    let mut free_at = 0.0;
    let mut say = |c: &mut Commentary, at: f64, now: Vec<Line>| {
        let line = match now.into_iter().last() {
            Some(l) => Some(l),
            None if at >= free_at => c.engine.next(&mut c.game),
            None => None,
        };
        if let Some(line) = line {
            let secs = c.seconds(&line, &lib, vfs);
            println!("{at:7.2}  {} ({secs:.1} s)", c.describe(&line));
            free_at = at + secs;
        }
    };
    // the intro cutscene, said through before the kick-off
    let intro = if stadium.to_ascii_lowercase().starts_with("hog") { "SCHINTV2" } else { "INTINTV1" };
    let now = c.cutscene(intro, &m);
    say(&mut c, 0.0, now);
    let mut t = 0.0;
    while t < 60.0 {
        say(&mut c, t, Vec::new());
        t += 1.0 / 24.0;
    }
    let start = t;
    let secs = if args.time > 0.0 { args.time } else { 120.0 };
    while t - start < secs && !m.finished {
        m.step();
        t += 1.0 / 24.0;
        let now = c.match_events(&m);
        say(&mut c, t, now);
    }
    println!("{t:7.2}  ({} {} - {} {})", teams[0], m.teams[0].score, m.teams[1].score, teams[1]);
    // the catch's speech and the outro's result, when the match was played out
    if m.finished {
        let now = c.trigger("ON_AUDIO_SPEECH_LDO01CATCHSNI", &m);
        say(&mut c, t, now);
        let now = c.cutscene("OutroSch", &m);
        say(&mut c, t, now);
    }
    let end = t + 30.0;
    while t < end {
        say(&mut c, t, Vec::new());
        t += 1.0 / 24.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qwc_formats::speech::PartKey;

    struct Yes;
    impl Host for Yes {
        fn test(&mut self, _: u8, _: u32) -> Option<bool> {
            Some(true)
        }
        fn filter(&mut self, _: u16) -> Option<u8> {
            Some(0)
        }
    }

    fn bank(id: u16, team: Option<u16>, keys: &[u8]) -> Loaded {
        let clips: Vec<BankClip> =
            keys.iter().enumerate().map(|(i, &k)| BankClip { offset: i as u16, keys: vec![k] }).collect();
        let mask = keys.iter().fold(0, |m, &k| m | 1 << k);
        let b =
            Bank { id, team, keys: 1, history: 2, size: keys.len() as u16, clips, key_masks: vec![mask], parts: None };
        Loaded::new(b, "X\\x.dat".into(), format!("bank{id}"), 0)
    }

    fn part(bank: u16, source: u8) -> Part {
        let keys = vec![PartKey { allowed: 0xff, source, x5: 0xff, r#ref: 0xff, x7: 0 }];
        Part { bank, arg: 1, bank_mode: 0, keys }
    }

    fn event(id: u16, priority: u16, flags: u8, variants: Vec<Variant>) -> Event {
        Event {
            id,
            t0: 0,
            t1: priority,
            max_repeats: 0,
            chance: 100,
            flags,
            args: 2,
            tests: vec![],
            arg_tests: vec![],
            arg_modes: None,
            groups: None,
            linked: None,
            variants,
            name: format!("e{id}"),
        }
    }

    fn variant(parts: Vec<Part>) -> Variant {
        Variant { weight: 100, chance: 100, test_mask: 0, test_value: 0, mode: 0, slots: 0, masks: vec![], parts }
    }

    #[test]
    fn the_generator_draws_below_its_bound_and_avoids_recent_tagged_values() {
        let mut r = Rng::new(3);
        assert!((0..1000).all(|_| r.draw(7, None) < 7));
        // a tag's draws of 2 values: the second differs from the first while one is remembered
        let mut r = Rng::new(5);
        let mut kept = Vec::new();
        for _ in 0..50 {
            kept.push(r.draw(4, Some(9)));
        }
        assert!(kept.iter().all(|&v| v < 4));
        assert!(kept.windows(2).filter(|w| w[0] == w[1]).count() < 10);
    }

    #[test]
    fn a_line_picks_clips_by_the_arguments_bits_and_not_the_last_said() {
        // clips with key values 0, 1, 1, 2; the argument allows values 1 and 2
        let events = vec![event(4, 10, 0, vec![variant(vec![part(7, 1)])])];
        let mut e = Engine::new(events, vec![bank(7, None, &[0, 1, 1, 2])], 1);
        let mut said = Vec::new();
        for _ in 0..3 {
            assert_eq!(e.post(&mut Yes, 4, &[0b110, 0]), None);
            let line = e.next(&mut Yes).expect("a line");
            assert_eq!(line.event, 4);
            said.push(line.clips[0].1);
        }
        assert!(said.iter().all(|&c| (1..=3).contains(&c)), "{said:?}");
        // the history of two keeps the three apart
        assert!(said[0] != said[1] && said[1] != said[2] && said[0] != said[2], "{said:?}");
        assert_eq!(e.next(&mut Yes), None);
    }

    #[test]
    fn the_queue_says_the_most_important_and_drops_what_waited_before_it() {
        let v = || vec![variant(vec![part(7, 0)])];
        // event 1 keeps its place (flag 4), event 2 does not; event 3 matters most
        let events = vec![event(1, 5, 4, v()), event(2, 5, 0, v()), event(3, 50, 0, v())];
        let mut e = Engine::new(events, vec![bank(7, None, &[0, 0, 0])], 1);
        e.post(&mut Yes, 1, &[]);
        e.post(&mut Yes, 2, &[]);
        e.post(&mut Yes, 3, &[]);
        assert_eq!(e.next(&mut Yes).map(|l| l.event), Some(3));
        assert_eq!(e.next(&mut Yes).map(|l| l.event), Some(1));
        assert_eq!(e.next(&mut Yes), None);
    }

    #[test]
    fn a_per_team_part_takes_the_arguments_team() {
        let mut p = part(9, 0);
        p.bank_mode = 1;
        let events = vec![event(5, 10, 0, vec![variant(vec![p])])];
        let banks = vec![bank(9, Some(3), &[0]), bank(9, Some(6), &[0])];
        let mut e = Engine::new(events, banks, 1);
        e.post(&mut Yes, 5, &[6]);
        assert_eq!(e.next(&mut Yes).map(|l| l.clips[0].0), Some(1));
        e.post(&mut Yes, 5, &[4]);
        assert_eq!(e.next(&mut Yes), None);
    }

    #[test]
    fn codes_and_masks() {
        assert_eq!([team_bit(1), team_bit(2), team_bit(3), team_bit(4), team_bit(13)], [1, 4, 2, 8, 0x1000]);
        assert_eq!(
            (team_code("Gryff"), team_code("aus"), stadium_code("hog_win"), stadium_code("qditch")),
            (1, 12, 1, 11)
        );
        assert_eq!((nationality("FRA"), nationality("NDK"), nationality("XXX")), (Some(1), Some(7), None));
        let mut s = Setup { stadium: 3, colour: Some("Vogleur".into()), ..Setup::default() };
        assert_eq!((s.pair(), s.folders(), s.colour_mask()), (4, ["LB", "AV"], 2));
        s.stadium = 1;
        assert_eq!((s.pair(), s.folders(), s.colour_mask()), (2, ["LJ", "SF"], 8));
    }
}
