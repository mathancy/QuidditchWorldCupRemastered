//! The front end's speech: the lines the menus' scripts ask for (`General_PlayFrontendSpeech`,
//! `General_PlayChallengeSpeech`, `General_PlayResultsSpeech`) said by the speech engine
//! (`crate::commentary::Engine`) from the front end's events (`Audio/SpchCmn/FE/Events.evt`) and the `Chal` banks
//! (`Audio/<lang>/hdr/Chal/*.hdr`).
//!
//! **[T]** (`RE_NOTES.md`, "Front-end speech"): the natives post their arguments on the routers `0x1043930` and
//! `0x104392c`; the speech object's handlers (`0x51ea20`, `0x51eae0`) turn them into speech events:
//! - `PlayFrontendSpeech(a, b, c)`: `a` below `0x21` posts `FEHWEvt` (9, the House Cup's) with `[bit(a), mask(b,
//!   c)]` ([`bit`], [`mask`]), or `SCdsEvt` (15) with `[bit(a), 0x10000000, 2]` when the mask is `0x10000000`
//!   (`b` 17); `a` from `0x21` posts `FEWCEvt` (10, the World Cup's) with `[bit(a), mask(b, c)]`;
//! - `PlayChallengeSpeech(a, b)`: `ChalEvt` (11) with `[bit(a), 1 << (b - 1), 1 or 0x20 by b]` ([`chal_kind`]);
//! - `PlayResultsSpeech(a, b, c)` (router `0x1043928`, handler `0x51eb30`): `RsltEvt` (12) with `[speaker(a),
//!   result(b, c)]` ([`speaker`], [`result`]);
//! - a challenge's start (`0x45ac60` posts type 1 on `0x1043b34`; handler `0x51e8c0`): its briefing, `ChalEvt` with
//!   `[line, 1, 1]` for the House Cup's challenge `CurrentChallengeNum` 0, 1, 4 or 5, `[line, 0x200, 1]` for 3 and
//!   `[line, 8, 1]` for 2, followed (`0x51ddb0`, said after it) by `[1, house's, every bit]` ([`briefing_line`],
//!   [`house_line`]). The other posts there (the last seconds' warnings, the AI's) say nothing.
//!
//! `General_PlayCutSceneSpeech`'s router (`0x1043924`) has no handler: it says nothing, as in the original.
//!
//! **[I]**: a waiting line is said when nothing else of the front end's is (one channel); the speech object's flag
//! `+0x1ac`, which sends `SCdsEvt` through `0x51ddb0` instead, is taken as clear; the engine's test 1 (the name
//! said, `0x522e17`) is "not one of the last two values said", as for the match's names.

use qwc_assets::Vfs;
use qwc_audio::{Library, Mixer, VoiceId};

use crate::commentary::{self, Engine, Host, Line};

/// The front end's speech events.
pub(crate) const EVENTS: &str = "Audio/SpchCmn/FE/Events.evt";

/// The speech events the handlers post (`Events.evt` ids).
mod id {
    pub const FE_HOGWARTS: u16 = 9;
    pub const FE_WORLD_CUP: u16 = 10;
    pub const CHALLENGE: u16 = 11;
    pub const RESULTS: u16 = 12;
    pub const CARDS: u16 = 15;
}

/// `0x51d620`: a speaker or screen code as a bit: 1 to 30 as `1 << (a - 1)`, `0x21` to `0x29` as `1 << (a - 0x21)`,
/// `0x2a` as `0x200`, 0 as 0 and anything else as every bit.
pub(crate) fn bit(a: u32) -> u32 {
    match a {
        0 => 0,
        1..=30 => 1 << (a - 1),
        0x21..=0x29 => 1 << (a - 0x21),
        0x2a => 0x200,
        _ => u32::MAX,
    }
}

/// `0x51d880`: a line `b` (and for some, its case `c`) as the mask the House Cup's and World Cup's events key on.
pub(crate) fn mask(b: u32, c: u32) -> u32 {
    match b {
        6 => match c {
            0 | 4 => 0x20,
            5 => 2,
            7 => 4,
            8 => 8,
            9 => 0x10,
            3 | 0xb => 0x40,
            1 | 0xa => 0x80,
            2 | 6 => 0x100,
            0xc => 0x200,
            _ => 0,
        },
        7 => match c {
            0 => 0x200,
            3 | 5 => 0x400,
            1 | 7 => 0x800,
            2 | 8 => 0x1000,
            9 => 0x2000,
            4 => 0x4000,
            0xb => 0x8000,
            0xa => 0x10000,
            6 => 0x20000,
            0xc => 0x40000,
            _ => 0,
        },
        13 => match c {
            0 => 0x20_0000,
            1 => 0x8_0000,
            2 => 0x10_0000,
            3 => 0x4_0000,
            _ => 0,
        },
        16 => match c {
            0 => 0x100_0000,
            1 => 0x400_0000,
            2 => 0x800_0000,
            3 => 0x200_0000,
            _ => 0,
        },
        0 => 0,
        1..=5 => 1 << (b - 1),
        8..=12 => 0x2000 << (b - 8),
        14 => 0x40_0000,
        15 => 0x80_0000,
        17 => 0x1000_0000,
        18 => 0x2000_0000,
        19 => 0x4000_0000,
        20 => 1,
        21 => 0x8000_0000,
        _ => 0,
    }
}

/// `0x51dbb0`: a challenge's line `b` as its third key: `0x20` for 2, 3 and 11, 1 for the others of 1 to 11, every
/// bit outside them.
pub(crate) fn chal_kind(b: u32) -> u32 {
    match b {
        2 | 3 | 11 => 0x20,
        1..=11 => 1,
        _ => u32::MAX,
    }
}

/// `0x51d820`: a results line's speaker as its bit (2 `8`, 3 `2`, 4 `1`, 5 `4`, 30 `0x20`, 33 `0x10`; else none).
pub(crate) fn speaker(a: u32) -> u32 {
    match a {
        2 => 8,
        3 => 2,
        4 => 1,
        5 => 4,
        30 => 0x20,
        33 => 0x10,
        _ => 0,
    }
}

/// `0x51dc60`: a results line `b` (with its case `c` for 16 and 17) as its bit; every bit for one out of range.
pub(crate) fn result(b: u32, c: u32) -> u32 {
    match (b, c) {
        (0, _) => 0,
        (1..=15, _) => 1 << (b - 1),
        (16, 0) => 0x8000,
        (16, 1) => 0x8_0000,
        (16, 2) => 0x20_0000,
        (16, 3) => 0x2_0000,
        (17, 0) => 0x1_0000,
        (17, 1) => 0x10_0000,
        (17, 2) => 0x40_0000,
        (17, 3) => 0x4_0000,
        _ => u32::MAX,
    }
}

/// `0x51e4b0`: the briefing's line for the House Cup's challenge `num` (`CurrentChallengeNum`, 0 to 5) played as
/// `house` (a team code: 1 Gryffindor, 2 Ravenclaw, 3 Hufflepuff, 4 Slytherin; the house's name in `0x60bb08`); 0
/// for any other.
pub(crate) fn briefing_line(num: u32, house: u32) -> u32 {
    const LINES: [[u32; 6]; 4] = [
        [0x40, 0x80, 0x8, 0x400, 0x20, 0x100],
        [0x400_0000, 0x1000_0000, 0x4, 0x80_0000, 0x800_0000, 0x200_0000],
        [0x4_0000, 0x2_0000, 0x10, 0x20_0000, 0x8_0000, 0x40_0000],
        [0x4000, 0x1_0000, 0x2, 0x800, 0x8000, 0x2000],
    ];
    house.checked_sub(1).and_then(|h| LINES.get(h as usize)?.get(num as usize)).copied().unwrap_or(0)
}

/// `0x51e400` and `0x51e950`: the line the third challenge's briefing is followed by, by `house` (a team code).
pub(crate) fn house_line(house: u32) -> u32 {
    match house {
        1 => 0x20,
        2 => 0x100,
        3 => 0x80,
        4 => 0x40,
        _ => 0,
    }
}

/// `0x51db10`: a challenge's line `b` (1 to 11) as a bit, every bit outside them.
fn chal_line(b: u32) -> u32 {
    match b {
        1..=11 => 1 << (b - 1),
        _ => u32::MAX,
    }
}

/// The game's side of the engine for the front end's events.
#[derive(Default)]
struct Fe {
    /// The last two values test 1 passed.
    said: [Option<u32>; 2],
    at: usize,
}

impl Host for Fe {
    fn test(&mut self, id: u8, value: u32) -> Option<bool> {
        match id {
            1 => {
                if self.said.contains(&Some(value)) {
                    return Some(false);
                }
                self.at = (self.at + 1) % 2;
                self.said[self.at] = Some(value);
                Some(true)
            }
            _ => None,
        }
    }

    fn filter(&mut self, _: u16) -> Option<u8> {
        Some(0)
    }
}

/// The front end's speech: its engine and the line being said.
pub(crate) struct FeSpeech {
    engine: Engine,
    host: Fe,
    voice: Option<VoiceId>,
    /// Lines to say at once (an immediate event's), the latest last.
    now: Vec<Line>,
    /// Events to post once nothing is being said (`0x51ddb0`'s list), the first first.
    after: std::collections::VecDeque<(u16, Vec<u32>)>,
    log: bool,
}

impl FeSpeech {
    /// The front end's events and the `Chal` banks for speech session `session`, or none (said why).
    pub fn load(vfs: &Vfs, lib: &Library, session: u32) -> Option<FeSpeech> {
        let events = commentary::events(vfs, EVENTS)?;
        let banks = commentary::banks(vfs, lib, &["Chal"], session);
        if banks.is_empty() {
            eprintln!("front-end speech: no banks");
            return None;
        }
        let log = std::env::var_os("QWC_AUDIO_LOG").is_some();
        Some(FeSpeech {
            engine: Engine::new(events, banks, session),
            host: Fe::default(),
            voice: None,
            now: Vec::new(),
            after: Default::default(),
            log,
        })
    }

    fn post(&mut self, id: u16, args: &[u32]) {
        if self.log {
            let a: Vec<String> = args.iter().map(|a| format!("{a:#x}")).collect();
            eprintln!("front-end speech: event {id} [{}]", a.join(", "));
        }
        if let Some(line) = self.engine.post(&mut self.host, id, args) {
            self.now.push(line);
        }
    }

    /// `General_PlayFrontendSpeech(a, b, c)` (`0x51ea20`).
    pub fn frontend(&mut self, [a, b, c]: [u32; 3]) {
        let m = mask(b, c);
        match a {
            0x21.. => self.post(id::FE_WORLD_CUP, &[bit(a), m]),
            _ if m == 0x1000_0000 => self.post(id::CARDS, &[bit(a), m, 2]),
            _ => self.post(id::FE_HOGWARTS, &[bit(a), m]),
        }
    }

    /// `General_PlayChallengeSpeech(a, b)` (`0x51eae0`).
    pub fn challenge(&mut self, [a, b]: [u32; 2]) {
        self.post(id::CHALLENGE, &[bit(a), chal_line(b), chal_kind(b)]);
    }

    /// `General_PlayResultsSpeech(a, b, c)` (`0x51eb30`).
    pub fn results(&mut self, [a, b, c]: [u32; 3]) {
        self.post(id::RESULTS, &[speaker(a), result(b, c)]);
    }

    /// A challenge's briefing (`0x51e8c0`, for its start's post of type 1): challenge `num` (`CurrentChallengeNum`)
    /// of the House Cup played as `house` (a team code).
    pub fn briefing(&mut self, num: u32, house: u32) {
        let line = briefing_line(num, house);
        match num {
            0 | 1 | 4 | 5 => self.post(id::CHALLENGE, &[line, 1, 1]),
            3 => self.post(id::CHALLENGE, &[line, 0x200, 1]),
            2 => {
                self.post(id::CHALLENGE, &[line, 8, 1]);
                let h = house_line(house);
                self.after.push_back((id::CHALLENGE, vec![1, h, chal_kind(h)]));
            }
            _ => {}
        }
    }

    /// A line said at once interrupts the one being said; then, when nothing is being said, the next.
    pub fn update(&mut self, lib: &Library, vfs: &Vfs, mixer: &std::sync::Mutex<Mixer>, gain: f32) {
        let log = self.log.then_some("front-end speech");
        if let Some(line) = std::mem::take(&mut self.now).pop() {
            self.stop(mixer);
            self.voice = commentary::say(&self.engine, &line, lib, vfs, mixer, gain, log);
            return;
        }
        if self.voice.is_some_and(|v| mixer.lock().unwrap().is_playing(v)) {
            return;
        }
        self.voice = None;
        let mut line = self.engine.next(&mut self.host);
        // what follows a line waits until nothing else is waiting to be said
        if line.is_none() {
            if let Some((id, args)) = self.after.pop_front() {
                self.post(id, &args);
                line = self.now.pop().or_else(|| self.engine.next(&mut self.host));
            }
        }
        if let Some(line) = line {
            self.voice = commentary::say(&self.engine, &line, lib, vfs, mixer, gain, log);
        }
    }

    /// Silence.
    pub fn stop(&mut self, mixer: &std::sync::Mutex<Mixer>) {
        if let Some(v) = self.voice.take() {
            mixer.lock().unwrap().stop(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tables as `QWC.exe`'s helpers give them (emulated, `0x51d620`, `0x51d880`, `0x51dbb0`, `0x51db10`).
    #[test]
    fn the_arguments_are_the_originals() {
        let f620: Vec<u32> = (0..0x30).map(bit).collect();
        let mut want = vec![0];
        want.extend((0..30).map(|i| 1u32 << i));
        want.extend([u32::MAX, u32::MAX]);
        want.extend((0..9).map(|i| 1u32 << i));
        want.push(0x200);
        want.extend([u32::MAX; 5]);
        assert_eq!(f620, want);
        let row = |b| (0..13).map(|c| mask(b, c)).collect::<Vec<_>>();
        assert_eq!(row(6), [0x20, 0x80, 0x100, 0x40, 0x20, 2, 0x100, 4, 8, 0x10, 0x80, 0x40, 0x200]);
        assert_eq!(
            row(7),
            [0x200, 0x800, 0x1000, 0x400, 0x4000, 0x400, 0x20000, 0x800, 0x1000, 0x2000, 0x10000, 0x8000, 0x40000]
        );
        assert_eq!(mask(13, 0), 0x20_0000);
        assert_eq!(mask(13, 3), 0x4_0000);
        assert_eq!(mask(13, 4), 0);
        assert_eq!(mask(16, 1), 0x400_0000);
        let flat: Vec<u32> =
            [0, 1, 2, 3, 4, 5, 8, 9, 10, 11, 12, 14, 15, 17, 18, 19, 20, 21, 22].map(|b| mask(b, 7)).to_vec();
        assert_eq!(
            flat,
            [
                0,
                1,
                2,
                4,
                8,
                0x10,
                0x2000,
                0x4000,
                0x8000,
                0x10000,
                0x20000,
                0x40_0000,
                0x80_0000,
                0x1000_0000,
                0x2000_0000,
                0x4000_0000,
                1,
                0x8000_0000,
                0
            ]
        );
        let kinds: Vec<u32> = (0..13).map(chal_kind).collect();
        assert_eq!(kinds, [u32::MAX, 1, 0x20, 0x20, 1, 1, 1, 1, 1, 1, 1, 0x20, u32::MAX]);
        let speakers: Vec<u32> = [0, 1, 2, 3, 4, 5, 6, 30, 33, 34].map(speaker).to_vec();
        assert_eq!(speakers, [0, 0, 8, 2, 1, 4, 0, 0x20, 0x10, 0]);
        assert_eq!((0..17).map(|b| result(b, 0)).collect::<Vec<_>>()[15], 0x4000);
        assert_eq!([0, 1, 2, 3, 4].map(|c| result(16, c)), [0x8000, 0x8_0000, 0x20_0000, 0x2_0000, u32::MAX]);
        assert_eq!([0, 1, 2, 3, 4].map(|c| result(17, c)), [0x1_0000, 0x10_0000, 0x40_0000, 0x4_0000, u32::MAX]);
        assert_eq!(result(18, 0), u32::MAX);
        assert_eq!(chal_line(0), u32::MAX);
        // `0x51e4b0` and `0x51e400` as emulated, by house and challenge
        assert_eq!((0..7).map(|n| briefing_line(n, 1)).collect::<Vec<_>>(), [0x40, 0x80, 8, 0x400, 0x20, 0x100, 0]);
        assert_eq!(briefing_line(2, 3), 0x10);
        assert_eq!(briefing_line(1, 2), 0x1000_0000);
        assert_eq!(briefing_line(5, 4), 0x2000);
        assert_eq!(briefing_line(0, 6), 0);
        assert_eq!([1, 2, 3, 4, 5].map(house_line), [0x20, 0x100, 0x80, 0x40, 0]);
        assert_eq!(chal_line(11), 0x400);
    }
}
