//! The game's sound in the live front end: what the scripts' `General_SendAudioEvent` (`0x4d33a0`) asks for, and
//! the music of the front end and the match.
//!
//! `General_SendAudioEvent(id)` posts `id` to the router `[0x1043934]`, which the game's audio layer listens to:
//!
//! - **The GUI sounds** (`0x52a680`, the object built at `0x52a2a0`): emitters in the `GUI` category
//!   (`Audio/cats/gui.map`) play an event per id ([`gui`]). `GenGUI` is a pool of three voices that plays one sound
//!   a frame at most (`0x52a650`, `0x528700`); `SnitchGUI`, `PlayerGUI`, `TxtSparkleGUI` and `QCard` have a voice
//!   each (`0x516b80`); `QCard` plays the `OSD` category. Their levels are the mixing desk's faders (`mix.cli`).
//! - **The front end's music driver** (`0x60b9f0`, vtable `0x5a7258`; events `0x525860`, update `0x5257e0`) on
//!   the `FE` score. Id 12 selects the score the first time, 24 every time, 23 deselects it; 15, 16, 19, 20, 21 and
//!   22 set its state (21 keeps the state before, 22 puts it back), and each update posts the state's event when it
//!   changed (state 1 posts event 0, the others their own number). It listens only while the score's package is
//!   loaded: the scores are resources of the load sets' `music` group (`0x5266c0`), whose load (`0x526810`)
//!   attaches the driver (armed, state 1, nothing posted) and whose unload (`0x526840`) takes it off the routers
//!   (`0x496550`) and deselects the score (`0x5265c0`). `audio\music\FE` is in `LOADSET_FRONTEND_COMMON` only
//!   ([`FE_PACKAGE`]), so the boot's first 12 (`PreFlow_TrcStartGame`'s, before the movies) reaches nothing and the
//!   music starts with the first screen holding that set that sends 12 or 24 (`Initial_LoadFile`, before the title).
//! - **The score manager** (`0xf97f38`, `0x5261d0`): one score is current. Selecting another stops the one playing
//!   (`0x5265d0`) and opens the new one; an event posted while none plays is kept and posted when one starts
//!   (`0x526680`, `0x5264a0`).
//! - **The match's driver** (`0x60b9c8`, vtable `0x5a7218`, attached to the `HW`, `Intl` and `QD` scores; the stadium's
//!   load set names which, in its `music` group): it selects the score and posts event 0 when the match starts
//!   (`0x525c20`) and deselects it on `General_DeInitMatch`'s `0x1043b48` (`0x525c90`). In between it posts events
//!   on the game's routers, through a guard (`0x525af0`): never the event last posted, after 7 nothing but 13, 11
//!   and 12 only after 10, and only 11 or 12 after 10.
//!
//! **[I]** which game events those routers carry is not traced, so the match's events come from the sim: a goal posts
//! 5 or 6 (`0x525ca0`: the scores' first value behind the second, or not, for the player's team), the Seeker game
//! 7 and the catch 13. The challenge score's driver (`0x60ba0c`) is not ported. The GUI emitters' own voice replaces
//! what it played.
//!
//! `QWC_AUDIO_LOG=1` prints each id the scripts send.

use std::collections::HashMap;

use qwc_assets::Vfs;
use qwc_audio::{Bus, Library, MusicPlayer, Output, Params, Score, VoiceId};

/// The front-end sounds' category.
const GUI_MAP: &str = "Audio/cats/gui.map";

/// The GUI object's emitters (`0x52a2a0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Emitter {
    Gen,
    Snitch,
    Player,
    TxtSparkle,
    QCard,
}

impl Emitter {
    /// Its fader and category: all play the `GUI` category's slot (`[0xf91458] + 0x134`) but `QCard`, which plays
    /// the `OSD` one (`+0x334`, `Audio/cats/osd.map`).
    fn info(self) -> (&'static str, &'static str) {
        match self {
            Emitter::Gen => ("GenGUI", GUI_MAP),
            Emitter::Snitch => ("SnitchGUI", GUI_MAP),
            Emitter::Player => ("PlayerGUI", GUI_MAP),
            Emitter::TxtSparkle => ("TxtSparkleGUI", GUI_MAP),
            Emitter::QCard => ("QCard", "Audio/cats/osd.map"),
        }
    }
}

/// The mixing desk's settings (`root/audio/cli/mix.cli`, in the system load set).
const MIX_CLI: &str = "root/audio/cli/mix.cli";

/// A score's fader on the mixing desk.
fn score_fader(name: &str) -> &'static str {
    match name.to_ascii_lowercase().as_str() {
        "fe" => "FE",
        "intl" => "INT",
        "hw" => "HW",
        "qd" => "QD",
        _ => "CH",
    }
}

/// What audio event `id` plays in the `GUI` category, and on which emitter (`0x52a680`'s tables `0x52a77c` and
/// `0x52a750`).
fn gui(id: u32) -> Option<(u32, Emitter)> {
    use Emitter::*;
    Some(match id {
        1..=4 => (61, Gen),
        5 => (64, Gen),
        6 => (77, Gen),
        7 => (63, Gen),
        8 => (82, TxtSparkle),
        9 => (26, Snitch),
        10 | 11 => (14, Player),
        13 => (80, Gen),
        25 => (79, Gen),
        26 | 27 => (71, QCard),
        _ => return None,
    })
}

/// The front end's score's package (`LOADSET_FRONTEND_COMMON`'s `music` entry), as a folder of the disc.
pub(crate) const FE_PACKAGE: &str = "audio/music/fe";

/// The front end's music driver (`0x525790`, `0x525860`, `0x5257e0`).
struct FeDriver {
    /// On the routers: its score's package is loaded.
    attached: bool,
    /// Id 12 selects the score once.
    armed: bool,
    state: u32,
    saved: u32,
    /// The event last posted (`None` after a deselect).
    posted: Option<usize>,
}

impl Default for FeDriver {
    fn default() -> FeDriver {
        FeDriver { attached: false, armed: true, state: 1, saved: 1, posted: None }
    }
}

/// What the front end's driver does with an id.
enum FeAction {
    Select,
    Deselect,
    None,
}

impl FeDriver {
    /// Its score's package loaded (`0x526810`): on the routers, armed, in state 1, nothing posted (`0x525790`).
    fn attach(&mut self) {
        (self.attached, self.armed, self.state, self.posted) = (true, true, 1, None);
    }

    /// Its score's package unloaded (`0x526840`): off the routers (`0x496550`).
    fn detach(&mut self) {
        self.attached = false;
    }

    fn event(&mut self, id: u32) -> FeAction {
        if !self.attached {
            return FeAction::None;
        }
        match id {
            12 if self.armed => {
                self.armed = false;
                return FeAction::Select;
            }
            24 => return FeAction::Select,
            23 => {
                self.posted = None;
                return FeAction::Deselect;
            }
            15 if self.state != 3 => self.state = 1,
            19 => self.state = 4,
            16 | 20 => self.state = 2,
            21 if self.state != 3 => {
                self.saved = self.state;
                self.state = 3;
            }
            22 => self.state = self.saved,
            _ => {}
        }
        FeAction::None
    }

    /// The event to post this frame, if the state's changed.
    fn update(&mut self) -> Option<usize> {
        let event = if self.state == 1 { 0 } else { self.state as usize };
        (self.posted != Some(event)).then(|| {
            self.posted = Some(event);
            event
        })
    }
}

/// The match's driver's guard (`0x525af0`): whether `event` may follow `last`.
fn match_allows(last: Option<usize>, locked: bool, event: usize) -> bool {
    if locked && event != 13 {
        return false;
    }
    let Some(last) = last else { return true };
    let outro = matches!(event, 11 | 12);
    !(event == last || (last, event) == (5, 1) || (last, event) == (6, 2) || (last == 10) != outro)
}

/// The sound of the live front end and its matches.
pub(crate) struct Audio {
    pub out: Output,
    lib: Option<Library>,
    vfs: &'static Vfs,
    /// The front end's tick: `GenGUI` plays once in one.
    tick: u64,
    gen_played: Option<u64>,
    gen: [Option<VoiceId>; 3],
    voices: HashMap<Emitter, VoiceId>,
    fe: FeDriver,
    /// The current score (by its directory) and the event waiting for it to play.
    score: Option<MusicPlayer>,
    pending: Option<usize>,
    /// The match's score while a match runs, the match driver's last event and its lock.
    match_score: Option<String>,
    match_posted: Option<usize>,
    match_locked: bool,
    /// The mixing desk's faders (`crate::matchaudio::faders`), the current score's level and whether a movie hushes
    /// it.
    faders: HashMap<String, f32>,
    music_level: f32,
    hushed: bool,
    /// The match's sound effects while one runs.
    game: Option<crate::matchaudio::MatchAudio>,
    /// The cutscenes' speech: the `CS` bank (`Audio/<lang>/hdr/Chal/CS.hdr`) and its clips (`Chal\CS.dat`), loaded
    /// on first use (none when they cannot be).
    cs_speech: Option<Option<(qwc_formats::speech::Bank, Vec<qwc_audio::Sound>)>>,
    /// The match's commentary (`crate::commentary`) while a match runs, and the speech sessions so far (the front
    /// end's is the first).
    commentary: Option<crate::commentary::Commentary>,
    sessions: u32,
    /// The team code of the match cutscene playing's `TEAM1` (the winner, in the cups' winning cutscenes), and its
    /// folder (its sounds).
    cutscene_team: u32,
    cutscene_folder: String,
    /// A crowd cutscene's sound not yet played (`+0x494`: the first `CROWDCUT` trigger plays, once).
    crowd_cut: bool,
    /// The challenge running (its `MatchSubType`), 0 in a match or the menus.
    challenge: u32,
    /// The front end's speech (`crate::fespeech`), loaded on first use (none when it cannot be).
    fe_speech: Option<Option<crate::fespeech::FeSpeech>>,
}

#[cfg(test)]
#[test]
fn cutscene_speech_triggers_make_masks() {
    assert_eq!(cutscene_speech_masks("ON_AUDIO_SPEECH_WOD01FEINTRO"), Some([1 << 3, 1]));
    assert_eq!(cutscene_speech_masks("ON_AUDIO_SPEECH_HRY02FEINTRO"), Some([1 << 2, 2]));
    assert_eq!(cutscene_speech_masks("ON_AUDIO_SPEECH_DUM01WINHOG"), None);
    assert_eq!(cutscene_speech_masks("ON_AUDIO_FOLEY_HANDS"), None);
    // the winners' lines: Hufflepuff's two by Dumbledore, Australia's one by Ludo
    assert_eq!(cutscene_speech_lines("ON_AUDIO_SPEECH_DUM01WINHOG", 3), vec![[0x20, 0x200], [0x20, 0x400]]);
    assert_eq!(cutscene_speech_lines("ON_AUDIO_SPEECH_LDO01WINWORLD", 12), vec![[0x10, 0x8000]]);
    assert_eq!(cutscene_speech_lines("ON_AUDIO_SPEECH_WOD02FEINTRO", 0), vec![[1 << 3, 2]]);
}

/// The animation sound object's event names (`0x52b850`: the table at `0x5cfbd8`, matched exactly).
const ANIM_EVENTS: [(&str, u32); 109] = [
    ("TEMP", 1),
    ("AMBIENCE", 2),
    ("BOOST", 5),
    ("THROW", 6),
    ("CATCH", 7),
    ("IMPACT", 8),
    ("HIT", 9),
    ("DEFEND", 10),
    ("DAMAGE", 11),
    ("TURN", 12),
    ("ATTACK", 13),
    ("FLYING", 14),
    ("CHEER", 15),
    ("CHANT", 16),
    ("SWISH", 17),
    ("LOW", 18),
    ("MED", 19),
    ("HIGH", 20),
    ("RISE", 21),
    ("DIVE", 22),
    ("TACKLE", 24),
    ("INTERCEPT", 25),
    ("FLYBY", 26),
    ("SCORE", 27),
    ("START", 28),
    ("AIRHORNS", 29),
    ("ZONE", 30),
    ("SAVE", 31),
    ("CATCH1", 32),
    ("CATCH2", 33),
    ("CATCH3", 34),
    ("REBOOST", 35),
    ("DRUMS", 45),
    ("SLY", 47),
    ("GRY", 48),
    ("NEUTRAL", 49),
    ("JUMP", 53),
    ("LAND", 54),
    ("CLOAK", 58),
    ("EMOTE", 59),
    ("DODGE", 60),
    ("UP", 61),
    ("DOWN", 62),
    ("BACK", 63),
    ("SELECT", 64),
    ("GOAL", 65),
    ("WHOOSH", 66),
    ("HANDS", 67),
    ("FEET", 68),
    ("CLOTH", 69),
    ("KISSES", 70),
    ("TIME", 71),
    ("SUCK", 72),
    ("WIND", 73),
    ("BANG", 74),
    ("MISSILE", 75),
    ("REVERSE", 76),
    ("FAIL", 77),
    ("LOCKON", 78),
    ("MOVE", 79),
    ("END", 80),
    ("BANK", 81),
    ("TEXT", 82),
    ("CLAP", 83),
    ("SEEKNORM", 84),
    ("FIRST", 85),
    ("SECOND", 86),
    ("THIRD", 87),
    ("FOURTH", 88),
    ("FORTH", 88),
    ("INTRO", 89),
    ("HRY1", 90),
    ("HRY2", 91),
    ("HRY3", 92),
    ("WOOD1", 93),
    ("WOOD2", 94),
    ("WOOD3", 95),
    ("WOOD4", 96),
    ("WOOD5", 97),
    ("WOOD6", 98),
    ("WOOD7", 99),
    ("PASS", 100),
    ("BEATER", 101),
    ("SEEKER", 102),
    ("COMBOS", 103),
    ("SPMOVE", 104),
    ("SLY01", 105),
    ("SLY02", 106),
    ("GRYFF03", 107),
    ("GRYFF04", 108),
    ("HUFF03", 109),
    ("HUFF04", 110),
    ("RAV03", 111),
    ("RAV04", 112),
    ("SLY06", 113),
    ("SLY05", 114),
    ("GRYFF05", 115),
    ("GYRFF06", 116),
    ("HUFF05", 117),
    ("HUFF06", 118),
    ("RAV05", 119),
    ("RAV06", 120),
    ("HOGCOM", 121),
    ("INTCOM7", 122),
    ("INTCOM9", 123),
    ("COJLJ", 124),
    ("COGUD", 125),
    ("COVOG", 126),
    ("HOGCOM04", 127),
];

/// `0x52b310`: a crowd cutscene's sound (`CROWDCUT_<name>`, `cutcrowd.map`) by its crowd's house (`house`: 2
/// Gryffindor, 3 Slytherin, 4 Hufflepuff, 5 Ravenclaw, 1 any other) and its commentator (`voice`: 3 Hogwarts', 4
/// Gudabi, 5 Vogleur, 6 Jeune): the event ids of the name table's house and commentator entries.
fn crowd_cut_event(name: &str, house: u32, voice: u32) -> Option<u32> {
    let by_house = |ids: [u32; 5]| ids.get(house.checked_sub(1)? as usize).copied().filter(|&i| i != 0);
    match name {
        "CROWD01" => by_house([0, 0, 105, 0, 0]),
        "CROWD02" => by_house([0, 0, 106, 0, 0]),
        "CROWD03" => by_house([0, 107, 0, 109, 111]),
        "CROWD04" => by_house([127, 108, 0, 110, 112]),
        "TICKET01" => by_house([0, 115, 114, 117, 119]),
        "TICKET02" => by_house([0, 116, 113, 118, 120]),
        "COCOM01" => Some(122),
        "COCOM03" => Some(123),
        "COCOM02" => match voice {
            4 => Some(125),
            5 => Some(126),
            6 => Some(124),
            _ => None,
        },
        "HOGCOM01" => (voice == 3).then_some(121),
        _ => None,
    }
}

/// Where a sound category's map is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Folder {
    /// `Audio/cats`.
    Global,
    /// The stadium's `Audio/Stadium/<name>`.
    Stadium,
    /// The cutscene's team's `Audio/Teams/<team>`.
    Team,
}

/// A sound category by the name a cutscene's trigger gives it (`0x516f00`'s registrations; `0x52b8c0` hashes the
/// name and looks it up among them): its folder, its map and whether it plays on the streamed emitter
/// (`animStreamed`, **[I]** for the uber moves', the intros', the outros', the catch's and the challenges') or the
/// sound one (`animSound`). `CROWDCUT` (`cutcrowd.map`) picks its events by a case table of its own
/// ([`crowd_cut_event`]); `IMPACTS` (the cutscenes' spelling) names no category (`iteam.map`'s is `IMPACT`), so it
/// is silent as in the original.
fn cutscene_category(name: &str) -> Option<(Folder, &'static str, bool)> {
    use Folder::*;
    Some(match name {
        "CHASER" => (Global, "chaser.map", false),
        "QUAFFLE" => (Global, "quaffle.map", false),
        "FOLEY" => (Global, "foley.map", false),
        "BLUDGER" => (Global, "bludger.map", false),
        "SNITCH" => (Global, "snitch.map", false),
        "SEEKER" => (Global, "seeker.map", false),
        "CATCHSNI" => (Global, "catchsni.map", true),
        "CHLANGE" => (Global, "chlange.map", true),
        "INTROMOV" => (Global, "intromov.map", true),
        "UGRYFF" => (Global, "UGryff.map", true),
        "UHUFF" => (Global, "UHuff.map", true),
        "URAVEN" => (Global, "URaven.map", true),
        "USLYTH" => (Global, "USlyth.map", true),
        "UENG" => (Global, "UEng.map", true),
        "UUSA" => (Global, "UUSA.map", true),
        "UJAPAN" => (Global, "UJapan.map", true),
        "UGERMAN" => (Global, "UGerman.map", true),
        "UFRANCE" => (Global, "UFrance.map", true),
        "UAUST" => (Global, "UAust.map", true),
        "UNORDIC" => (Global, "UNordic.map", true),
        "UBULG" => (Global, "UBulg.map", true),
        "USPAIN" => (Global, "USpain.map", true),
        "INTRO" => (Stadium, "intro.map", true),
        "OUTROS" => (Stadium, "outro.map", true),
        "CROWD" => (Stadium, "crowd.map", false),
        "STADIUM" => (Stadium, "stadium.map", false),
        "TEAM" => (Team, "team.map", false),
        "EMOTE" => (Team, "eteamm.map", false),
        "IMPACT" => (Team, "iteam.map", false),
        "UTEAM" => (Team, "uteam.map", false),
        _ => return None,
    })
}

/// The speech language (**[I]** the front end's language setting is not read: English).
const SPEECH_LANGUAGE: &str = "English";

/// A cutscene's speech trigger as `CSEvt`'s two argument masks (`0x51e050`): `SPEECH_<speaker><line><cutscene>`, the
/// speaker `HRY`, `WOD`, `LDO` or `DUM` as key 0's bit 2 to 5, the line (`1..`) as key 1's bit `line - 1`
/// (`0x51dc00`). Only `FEINTRO`'s lines are raised this way; `WINHOG` and `WINWORLD` pick by the winner (not ported).
pub(crate) fn cutscene_speech_masks(trigger: &str) -> Option<[u32; 2]> {
    let rest = trigger.strip_prefix("ON_AUDIO_SPEECH_")?;
    let (spk, line, cs) = (rest.get(..3)?, rest.get(3..5)?, rest.get(5..)?);
    let key0 = match spk.to_ascii_uppercase().as_str() {
        "HRY" => 2,
        "WOD" => 3,
        "LDO" => 4,
        "DUM" => 5,
        _ => return None,
    };
    let n: u32 = line.parse().ok().filter(|&n| (1..=24).contains(&n))?;
    cs.eq_ignore_ascii_case("FEINTRO").then_some([1 << key0, 1 << (n - 1)])
}

/// The `CSEvt` lines a cutscene's speech trigger says (`0x51e050`), each as its two argument masks: `FEINTRO`'s
/// one ([`cutscene_speech_masks`]); `WINHOG`'s two for the winning house (`winner`, a team code: the line pairs
/// 7-8 Gryffindor, 11-12 Ravenclaw, 9-10 Hufflepuff, 13-14 Slytherin; the second follows the first, `0x51ddb0`)
/// and `WINWORLD`'s one for the winning nation (15 Australia ... 23 Spain).
pub(crate) fn cutscene_speech_lines(trigger: &str, winner: u32) -> Vec<[u32; 2]> {
    if let Some(m) = cutscene_speech_masks(trigger) {
        return vec![m];
    }
    let Some(rest) = trigger.strip_prefix("ON_AUDIO_SPEECH_") else { return Vec::new() };
    let speaker = match rest.get(..3).map(str::to_ascii_uppercase).as_deref() {
        Some("HRY") => 4,
        Some("WOD") => 8,
        Some("LDO") => 0x10,
        Some("DUM") => 0x20,
        _ => return Vec::new(),
    };
    let lines: &[u32] = match rest.get(5..).map(str::to_ascii_uppercase).as_deref() {
        Some("WINHOG") => match winner {
            1 => &[0x80, 0x100],
            2 => &[0x800, 0x1000],
            3 => &[0x200, 0x400],
            4 => &[0x2000, 0x4000],
            _ => &[0],
        },
        Some("WINWORLD") => match winner {
            5 => &[0x40_0000],
            6 => &[0x2_0000],
            7 => &[0x8_0000],
            8 => &[0x4_0000],
            9 => &[0x80_0000],
            10 => &[0x10_0000],
            11 => &[0x20_0000],
            12 => &[0x8000],
            13 => &[0x1_0000],
            _ => &[0],
        },
        _ => &[],
    };
    lines.iter().map(|&l| [speaker, l]).collect()
}

impl Audio {
    /// The player's levels (`qwc_settings::Sound`) on the mixer's music, effects and speech buses.
    pub fn set_volumes(&self, sound: &qwc_settings::Sound) {
        let [music, effects, speech] = sound.gains();
        if let Ok(mut m) = self.out.mixer().lock() {
            m.set_bus_gain(Bus::Music, music);
            m.set_bus_gain(Bus::Effects, effects);
            m.set_bus_gain(Bus::Speech, speech);
        }
    }

    /// The default output device, or `None` (said why) without one.
    pub fn open(vfs: &'static Vfs) -> Option<Audio> {
        let out = Output::open().map_err(|e| eprintln!("no sound: {e}")).ok()?;
        Some(Audio::with(vfs, out))
    }

    /// Mixed at `rate` without a device: the caller renders [`Output::mixer`] (`qwc-game cutscene`'s `QWC_CS_WAV`).
    pub fn offline(vfs: &'static Vfs, rate: u32) -> Audio {
        Audio::with(vfs, Output::offline(rate))
    }

    /// The cutscene playing is `team`'s (a folder of `teams/`), as [`Audio::cutscene_started`] has it without a match.
    pub fn cutscene_team(&mut self, team: &str) {
        self.cutscene_team = crate::commentary::team_code(team);
        self.cutscene_folder = team.to_string();
        self.crowd_cut = true;
    }

    fn with(vfs: &'static Vfs, out: Output) -> Audio {
        let lib = Library::open(vfs).map_err(|e| eprintln!("no sound effects: {e}")).ok();
        let mix = vfs.read(MIX_CLI).map(|b| String::from_utf8_lossy(&b).into_owned());
        let faders = mix.map(|t| crate::matchaudio::faders(&t)).unwrap_or_else(|e| {
            eprintln!("{MIX_CLI}: {e}");
            HashMap::new()
        });
        Audio {
            out,
            lib,
            vfs,
            tick: 0,
            gen_played: None,
            gen: [None; 3],
            voices: HashMap::new(),
            fe: FeDriver::default(),
            score: None,
            pending: None,
            match_score: None,
            match_posted: None,
            match_locked: false,
            faders,
            music_level: 1.0,
            hushed: false,
            game: None,
            cs_speech: None,
            commentary: None,
            sessions: 0,
            cutscene_team: 0,
            cutscene_folder: "gryff".into(),
            crowd_cut: false,
            challenge: 0,
            fe_speech: None,
        }
    }

    /// The front end's speech, loaded on first use.
    fn fe_speech(&mut self) -> Option<&mut crate::fespeech::FeSpeech> {
        let (vfs, lib) = (self.vfs, self.lib.as_ref());
        self.fe_speech.get_or_insert_with(|| crate::fespeech::FeSpeech::load(vfs, lib?, 0)).as_mut()
    }

    /// `General_PlayFrontendSpeech(a, b, c)`: a menu's line (`crate::fespeech`).
    pub fn frontend_speech(&mut self, args: [u32; 3]) {
        if let Some(s) = self.fe_speech() {
            s.frontend(args);
        }
    }

    /// `General_PlayResultsSpeech(a, b, c)`: a results screen's line (`crate::fespeech`).
    pub fn results_speech(&mut self, args: [u32; 3]) {
        if let Some(s) = self.fe_speech() {
            s.results(args);
        }
    }

    /// A challenge starts (`0x51e8c0`): its briefing, for challenge `num` (`CurrentChallengeNum`) of the House Cup
    /// played as house `house` (a team code).
    pub fn challenge_begins(&mut self, num: u32, house: u32) {
        if let Some(s) = self.fe_speech() {
            s.briefing(num, house);
        }
    }

    /// One tick of the menus' speech alone (the live front end's is in [`Audio::tick`]).
    pub fn speech_tick(&mut self) {
        let gain = self.fader("Speech");
        if let (Some(Some(s)), Some(lib)) = (&mut self.fe_speech, &self.lib) {
            s.update(lib, self.vfs, self.out.mixer(), gain);
        }
    }

    /// `General_PlayChallengeSpeech(a, b)`: a challenge's line (`crate::fespeech`).
    pub fn challenge_speech(&mut self, args: [u32; 2]) {
        if let Some(s) = self.fe_speech() {
            s.challenge(args);
        }
    }

    /// A cutscene's audio trigger (`ON_AUDIO_*`, sorted out by `0x4167b0`): its speech lines play on the speech
    /// bus (`CSEvt`, speech event 13: one clip of the `CS` bank by the trigger's keys). **[I]** the other triggers
    /// (foley, the intro's music cues) name sounds through a table that is not traced: silent.
    pub fn cutscene_event(&mut self, trigger: &str) {
        let lines = cutscene_speech_lines(trigger, self.cutscene_team);
        if lines.is_empty() {
            self.cutscene_sound(trigger);
            return;
        }
        let vfs = self.vfs;
        let loaded = self.cs_speech.get_or_insert_with(|| {
            let lib = self.lib.as_ref()?;
            let path = format!("Audio/{SPEECH_LANGUAGE}/hdr/Chal/CS.hdr");
            let bytes = vfs.read(&path).map_err(|e| eprintln!("{path}: {e}")).ok()?;
            let (_, _, data) = qwc_formats::audio::read_hdr(&bytes).map_err(|e| eprintln!("{path}: {e}")).ok()?;
            let bank = qwc_formats::speech::parse_bank(data).map_err(|e| eprintln!("{path}: {e}")).ok()?;
            let clips =
                lib.speech(vfs, SPEECH_LANGUAGE, "Chal\\CS.dat").map_err(|e| eprintln!("CS speech: {e}")).ok()?;
            Some((bank, clips))
        });
        let Some((bank, clips)) = loaded else { return };
        let mut sounds = Vec::new();
        for masks in &lines {
            match qwc_formats::speech::pick(bank, masks).and_then(|i| clips.get(i)).cloned() {
                Some(s) => sounds.push(s),
                None => eprintln!("cutscene speech {trigger}: no clip for keys {masks:?}"),
            }
        }
        let mut it = sounds.into_iter();
        let Some(first) = it.next() else { return };
        if std::env::var_os("QWC_AUDIO_LOG").is_some() {
            eprintln!("speech {trigger}");
        }
        let params = Params { gain: self.fader("Speech"), bus: Bus::Speech, ..Params::default() };
        let mut mixer = self.out.mixer().lock().unwrap();
        let v = mixer.play(first, params);
        for s in it {
            mixer.queue(v, s);
        }
    }

    /// A cutscene's sound trigger `ON_AUDIO_<category>_<event>` (the animation sound object, `0x52b6a0`: split at
    /// the first `_`): the event the name table gives (`0x52b850`) of the category the name gives
    /// ([`cutscene_category`]), at the animation emitters' level (`0x52b790`). **[I]** played once, not placed in
    /// the scene; `OFF_AUDIO_` (a loop's end) is not read; the front end's cutscenes take Hogwarts' summer stadium.
    fn cutscene_sound(&mut self, trigger: &str) {
        let Some((cat, ev)) = trigger.strip_prefix("ON_AUDIO_").and_then(|r| r.split_once('_')) else { return };
        // `chedgar` introduces two challenges and has both their talks: **[I]** the challenge's own (the tackling
        // one's for type 2, the special move's for type 5), both outside a challenge
        if cat == "CHLANGE" && matches!((self.challenge, ev), (5, "TACKLE") | (2, "SPMOVE")) {
            return;
        }
        // a crowd cutscene's: the first of its triggers, by the crowd's house (**[I]** the cutscene's team's) and the
        // match's commentator (**[I]** the pair's colour commentator; the message that sets both is not traced)
        let crowd = (cat == "CROWDCUT").then(|| {
            let house = match self.cutscene_team {
                1 => 2,
                4 => 3,
                3 => 4,
                2 => 5,
                _ => 1,
            };
            let voice = match self.commentary.as_ref().map_or(2, |c| c.pair()) {
                3 => 6,
                4 => 5,
                5 => 4,
                _ => 3,
            };
            (std::mem::take(&mut self.crowd_cut), crowd_cut_event(ev, house, voice))
        });
        let (folder, file, streamed, id) = match crowd {
            Some((true, Some(id))) => (Folder::Global, "cutcrowd.map", true, id),
            Some(_) => return,
            None => {
                let (Some((folder, file, streamed)), Some(&(_, id))) =
                    (cutscene_category(cat), ANIM_EVENTS.iter().find(|(n, _)| *n == ev))
                else {
                    return;
                };
                (folder, file, streamed, id)
            }
        };
        let Some(lib) = &self.lib else { return };
        let path = match folder {
            Folder::Global => format!("Audio/cats/{file}"),
            Folder::Stadium => {
                let stadium = self.game.as_ref().map_or("Audio/Stadium/Hog_Sum", |g| g.folder());
                format!("{stadium}/{file}")
            }
            Folder::Team => format!("Audio/Teams/{}/{file}", self.cutscene_folder),
        };
        let sound = match lib.event(self.vfs, &path, id, roll(self.tick, id)) {
            Ok(Some(s)) => s,
            Ok(None) => return,
            Err(e) => {
                if std::env::var_os("QWC_AUDIO_LOG").is_some() {
                    eprintln!("cutscene sound {trigger}: {e}");
                }
                return;
            }
        };
        if std::env::var_os("QWC_AUDIO_LOG").is_some() {
            eprintln!("cutscene sound {trigger}: {path} {id}");
        }
        let gain = self.fader(if streamed { "animStreamed" } else { "animSound" });
        let params = Params { gain, bus: Bus::Effects, ..Params::default() };
        self.out.mixer().lock().unwrap().play(sound, params);
    }

    /// A fader's value (1 when `mix.cli` does not set it).
    fn fader(&self, name: &str) -> f32 {
        self.faders.get(name).copied().unwrap_or(1.0)
    }

    /// One front-end tick: the audio events the scripts sent since the last, then the drivers' updates. `fe_score`
    /// says whether the front end's score's package ([`FE_PACKAGE`]) is loaded now: its driver listens only then.
    pub fn tick(&mut self, events: &[u32], fe_score: bool) {
        self.tick += 1;
        self.speech_tick();
        // **[I]** a tick's events and load sets come together: a package is loaded before the screen holding it is
        // built, and released after the screen leaving has sent its last
        if fe_score && !self.fe.attached {
            self.fe.attach();
        }
        for &id in events {
            if std::env::var_os("QWC_AUDIO_LOG").is_some() {
                eprintln!("audio event {id}");
            }
            self.gui_event(id);
            match self.fe.event(id) {
                FeAction::Select => self.select("FE"),
                FeAction::Deselect => self.deselect("FE"),
                FeAction::None => {}
            }
        }
        if !fe_score && self.fe.attached {
            self.fe.detach();
            self.deselect("FE");
        }
        if self.current() == Some("FE") {
            if let Some(e) = self.fe.update() {
                self.post(e);
            }
        }
        if let Some(p) = &mut self.score {
            if let Err(e) = p.update(self.out.mixer()) {
                eprintln!("music: {e}");
            }
        }
        // the commentary goes on through the match's cutscenes
        if let (Some(c), Some(lib)) = (&mut self.commentary, &self.lib) {
            c.update(Vec::new(), lib, self.vfs, self.out.mixer());
        }
    }

    fn gui_event(&mut self, id: u32) {
        let (Some((event, emitter)), Some(lib)) = (gui(id), &self.lib) else { return };
        if emitter == Emitter::Gen && self.gen_played.replace(self.tick) == Some(self.tick) {
            return;
        }
        let (fader, map) = emitter.info();
        let sound = match lib.event(self.vfs, map, event, roll(self.tick, id)) {
            Ok(Some(s)) => s,
            Ok(None) => return,
            Err(e) => {
                eprintln!("{map}: {e}");
                return;
            }
        };
        let gain = self.fader(fader) * self.fader("GUI");
        let mut mixer = self.out.mixer().lock().unwrap();
        let params = Params { gain, bus: Bus::Effects, ..Params::default() };
        if emitter == Emitter::Gen {
            // a free voice of the three, else the oldest
            let free = self.gen.iter().position(|v| !v.is_some_and(|v| mixer.is_playing(v)));
            let slot = free.unwrap_or_else(|| {
                if let Some(v) = self.gen[0] {
                    mixer.stop(v);
                }
                self.gen.rotate_left(1);
                2
            });
            self.gen[slot] = Some(mixer.play(sound, params));
        } else {
            if let Some(v) = self.voices.remove(&emitter) {
                mixer.stop(v);
            }
            self.voices.insert(emitter, mixer.play(sound, params));
        }
    }

    fn current(&self) -> Option<&str> {
        self.score.as_ref().map(|p| p.score.name.as_str())
    }

    /// Make score `name` (a `qwc_audio::music::SCORES` directory) current: from its start, then the event waiting.
    fn select(&mut self, name: &str) {
        if self.current().is_some_and(|c| c.eq_ignore_ascii_case(name)) {
            return;
        }
        self.stop_score();
        match Score::load(self.vfs, name) {
            Ok(score) => {
                let mut p = MusicPlayer::new(score);
                self.music_level = self.fader(score_fader(name)) * self.fader("Music");
                p.params.gain = if self.hushed { 0.0 } else { self.music_level };
                let mut started = p.start(self.out.mixer(), 0);
                if let Some(e) = self.pending.take() {
                    started = started.and_then(|_| p.post(self.out.mixer(), e));
                }
                match started {
                    Ok(()) => eprintln!("music: {}", p.score.name),
                    Err(e) => eprintln!("music {name}: {e}"),
                }
                self.score = Some(p);
            }
            Err(e) => eprintln!("music {name}: {e}"),
        }
    }

    fn deselect(&mut self, name: &str) {
        if self.current().is_some_and(|c| c.eq_ignore_ascii_case(name)) {
            self.stop_score();
        }
    }

    fn stop_score(&mut self) {
        if let Some(mut p) = self.score.take() {
            p.stop(self.out.mixer());
        }
    }

    /// Post `event` to the current score, or keep it for the next one to start.
    fn post(&mut self, event: usize) {
        match &mut self.score {
            Some(p) if p.is_playing() => {
                if let Err(e) = p.post(self.out.mixer(), event) {
                    eprintln!("music {}: event {event}: {e}", p.score.name);
                }
            }
            _ => self.pending = Some(event),
        }
    }

    /// The match starts on score `name` (the stadium load set's `music` package's folder: `hw`, `intl`, `qd`).
    /// `folder` is the stadium's sound folder (`crate::matchaudio::stadium_folder`).
    pub fn match_started(&mut self, name: &str, folder: String) {
        self.game = Some(crate::matchaudio::MatchAudio::new(folder));
        self.match_score = Some(name.to_string());
        self.match_posted = None;
        self.match_locked = false;
        self.select(name);
        self.match_event(0);
    }

    /// The match's commentary starts (not in a challenge): its speech session is the next.
    pub fn commentary_started(&mut self, mut setup: crate::commentary::Setup) {
        self.sessions += 1;
        setup.session = self.sessions;
        self.commentary = self.lib.as_ref().and_then(|lib| crate::commentary::Commentary::load(self.vfs, lib, setup));
    }

    /// A match cutscene starts for `team` (its `TEAM1`'s folder): the commentary's intro or result.
    pub fn cutscene_started(&mut self, name: &str, team: &str, m: &qwc_sim::Match) {
        self.cutscene_team = crate::commentary::team_code(team);
        self.cutscene_folder = team.to_string();
        self.crowd_cut = true;
        if let (Some(c), Some(lib)) = (&mut self.commentary, &self.lib) {
            let now = c.cutscene(name, m);
            c.update(now, lib, self.vfs, self.out.mixer());
        }
    }

    /// A match cutscene's audio trigger, for the commentary (the catch's and an intro's speech).
    pub fn commentary_trigger(&mut self, trigger: &str, m: &qwc_sim::Match) {
        if let (Some(c), Some(lib)) = (&mut self.commentary, &self.lib) {
            let now = c.trigger(trigger, m);
            c.update(now, lib, self.vfs, self.out.mixer());
        }
    }

    /// The match driver posts `event` (through its guard).
    pub fn match_event(&mut self, event: usize) {
        if self.match_score.is_none() || !match_allows(self.match_posted, self.match_locked, event) {
            return;
        }
        self.match_posted = Some(event);
        self.match_locked |= event == 7;
        self.post(event);
    }

    /// A challenge starts: its driver (`0x60ba0c`, `0x5255b0`) selects the `chal` score on `General_InitMatch`'s
    /// router (`0x1043b4c`, with a challenge) and posts its event 1 when the countdown's router (`0x1043b28`) reaches
    /// 0 ([`Audio::countdown_over`]); the stadium's sounds play as in a match.
    pub fn challenge_started(&mut self, folder: String, kind: u32) {
        self.challenge = kind;
        self.game = Some(crate::matchaudio::MatchAudio::new(folder));
        self.match_score = Some("chal".into());
        self.match_posted = None;
        self.match_locked = false;
        self.select("chal");
    }

    /// The challenge's countdown reached 0 (`0x525610`): event 1 of the `chal` score.
    pub fn countdown_over(&mut self) {
        if self.current() == Some("chal") {
            self.post(1);
        }
    }

    /// `General_DeInitMatch`: the match driver deselects its score.
    pub fn match_ended(&mut self) {
        self.challenge = 0;
        if let Some(mut g) = self.game.take() {
            g.stop(self.out.mixer());
        }
        if let Some(mut c) = self.commentary.take() {
            c.stop(self.out.mixer());
        }
        if let Some(name) = self.match_score.take() {
            self.deselect(&name);
        }
    }

    /// **[I]** the score is silent while a movie plays (the movie player's own handling of the audio is not traced).
    pub fn hush(&mut self, on: bool) {
        self.hushed = on;
        if let Some(p) = &mut self.score {
            p.params.gain = if on { 0.0 } else { self.music_level };
            if let Some(v) = p.voice() {
                self.out.mixer().lock().unwrap().set_params(v, p.params);
            }
        }
    }

    /// One tick of the running match's sound effects, with the player's `input` when there is one.
    /// The HUD starting again (the kick-off message `0x1043b50`): `HudStart` (`crate::matchaudio`).
    pub fn hud_start(&mut self) {
        let (Some(g), Some(lib)) = (&mut self.game, &self.lib) else { return };
        let ctx = crate::matchaudio::Ctx { lib, vfs: self.vfs, mixer: self.out.mixer(), faders: &self.faders };
        g.hud_start(&ctx);
    }

    /// The team whose player listens on this machine (online play, `crate::online`): its seeker's sounds are the
    /// player's.
    pub fn set_viewer(&mut self, team: usize) {
        if let Some(g) = &mut self.game {
            g.viewer = Some(team);
        }
    }

    pub fn match_tick(&mut self, m: &qwc_sim::Match, input: Option<&qwc_sim::human::Input>) {
        let (Some(g), Some(lib)) = (&mut self.game, &self.lib) else { return };
        let ctx = crate::matchaudio::Ctx { lib, vfs: self.vfs, mixer: self.out.mixer(), faders: &self.faders };
        g.tick(&ctx, m, input);
        if let Some(c) = &mut self.commentary {
            let now = c.match_events(m);
            c.update(now, lib, self.vfs, self.out.mixer());
        }
    }

    /// The mixer, for the movies.
    pub fn mixer(&self) -> &std::sync::Arc<std::sync::Mutex<qwc_audio::Mixer>> {
        self.out.mixer()
    }
}

/// What the match driver hears from the game's routers (`RE_NOTES.md`, "The match music's routers"), as the game
/// posts them; [`MusicRouter::tick`] answers the events the driver posts (`Audio::match_event`, which its guard
/// filters). **[T]** The driver keeps the last score message's two values (team 0's score and team 1's):
/// - a goal (`0x432100` posts `{scorer, score 0, score 1, ..}` on `0x1043b64`, `0x525ca0`): team 0 behind gives 5
///   when it scored itself, 1 when the other team did; level or ahead 6;
/// - the Quaffle's zone (`0x4513a0` posts `{zone, holding team}` on `0x1043b74` when it changes, `0x525d40`), heard
///   only for team 0: entering zone 1 (its attacked goal area) more than 8 s after its last entry posts 4, or 3 past
///   20 s; leaving zone 1 posts 5 or 6 by the standing;
/// - the Seeker game (`0x1043a8c` with 9, `0x525de0`): 7, and the guard's lock; the catch (8): 13;
/// - the match's end (`0x4518a0` posts on `0x1043b58` in the game's states 2 and 3, `0x525bb0`): team 0 behind 11,
///   else 12;
/// - the uber move (`0x43ed10` posts kind 2 on `0x1043b0c`, `0x525b70`): 10.
#[derive(Debug, Default)]
pub(crate) struct MusicRouter {
    seen: usize,
    /// The last zone posted (game `+0x3d0`), team 0's last (`[0x668ab4]`) and the time of its last entry into zone 1
    /// (`+0x18`, ms).
    zone: Option<u8>,
    team0_zone: u8,
    entered_ms: u64,
    ended: bool,
}

impl MusicRouter {
    /// The driver's events for what match `m` did since the last call.
    pub fn tick(&mut self, m: &qwc_sim::Match) -> Vec<usize> {
        use qwc_sim::race::RaceEvent;
        use qwc_sim::world::Event;
        let start = self.seen.min(m.events.len());
        self.seen = m.events.len();
        let behind = m.teams[0].score < m.teams[1].score;
        let mut out = Vec::new();
        for e in &m.events[start..] {
            match *e {
                Event::Goal { team, .. } => out.push(match (behind, team) {
                    (true, 0) => 5,
                    (true, _) => 1,
                    (false, _) => 6,
                }),
                Event::UberMove { .. } => out.push(10),
                Event::SeekerGame { .. } => out.push(7),
                Event::Race { event: RaceEvent::Caught { .. }, .. } => out.push(13),
                _ => {}
            }
        }
        let (zone, team) = crate::matchaudio::quaffle_zone(m);
        if self.zone != Some(zone) {
            self.zone = Some(zone);
            if team == Some(0) {
                if zone == 1 {
                    let now = u64::from(m.tick) * 1000 / 24;
                    let dt = now.saturating_sub(self.entered_ms);
                    if dt > 8000 {
                        out.push(if dt < 20000 { 4 } else { 3 });
                    }
                    self.entered_ms = now;
                }
                if self.team0_zone == 1 && zone != 1 {
                    out.push(if behind { 5 } else { 6 });
                }
                self.team0_zone = zone;
            }
        }
        if m.finished && !self.ended {
            self.ended = true;
            out.push(if behind { 11 } else { 12 });
        }
        out
    }
}

/// The score a match at `stadium` plays without a load-set table: the stadium load sets' `music` packages are
/// `hw` at Hogwarts, `QD` for Queerditch, `Chal` for the challenges and `Intl` everywhere else.
pub(crate) fn match_score(stadium: &str) -> &'static str {
    let s = stadium.to_ascii_lowercase();
    if s.starts_with("hog") {
        "HW"
    } else if s.starts_with("qd") || s.starts_with("qditch") {
        "QD"
    } else if s.starts_with("chal") {
        "chal"
    } else {
        "Intl"
    }
}

/// A pick among an event's weighted samples (the library's own random source is not traced).
pub(crate) fn roll(tick: u64, id: u32) -> f32 {
    let x = (tick ^ ((id as u64) << 32)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    (x >> 40) as f32 / (1u64 << 24) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_front_end_driver_selects_once_and_posts_its_state() {
        let mut d = FeDriver::default();
        d.attach();
        assert!(matches!(d.event(12), FeAction::Select));
        assert!(matches!(d.event(12), FeAction::None));
        assert_eq!(d.update(), Some(0));
        assert_eq!(d.update(), None);
        d.event(21);
        assert_eq!(d.update(), Some(3));
        // 15 does not leave the team tent's state; 22 puts the one before back
        d.event(15);
        assert_eq!(d.update(), None);
        d.event(22);
        assert_eq!(d.update(), Some(0));
        assert!(matches!(d.event(23), FeAction::Deselect));
        assert_eq!(d.update(), Some(0));
    }

    #[test]
    fn the_front_end_driver_hears_nothing_until_its_score_is_loaded() {
        // the boot's card check sends 12 before the movies, without the front end's sets
        let mut d = FeDriver::default();
        assert!(matches!(d.event(12), FeAction::None));
        assert!(matches!(d.event(24), FeAction::None));
        // loaded: armed again, so the first screen holding the set starts the score
        d.attach();
        assert!(matches!(d.event(12), FeAction::Select));
        d.event(16);
        assert_eq!(d.update(), Some(2));
        // gone with the set (a match) and back (its results): armed, in state 1, nothing posted
        d.detach();
        assert!(matches!(d.event(24), FeAction::None));
        d.attach();
        assert!(matches!(d.event(12), FeAction::Select));
        assert_eq!(d.update(), Some(0));
    }

    #[test]
    fn the_routers_post_the_goals_and_team_0_s_runs_into_the_goal_area() {
        use qwc_sim::world::Event;
        let mut m = qwc_sim::Match::new(&qwc_sim::Console::new(), qwc_sim::Rand::new(1));
        let mut r = MusicRouter::default();
        r.tick(&m);
        let goal = |m: &mut qwc_sim::Match, team: usize, scores: [i32; 2]| {
            m.teams[0].score = scores[0];
            m.teams[1].score = scores[1];
            m.events.push(Event::Goal { tick: m.tick, team, hoop: 0, combo: false });
        };
        // behind after its own goal 5, after the other team's 1; level or ahead 6
        goal(&mut m, 0, [10, 20]);
        assert_eq!(r.tick(&m), [5]);
        goal(&mut m, 1, [10, 30]);
        assert_eq!(r.tick(&m), [1]);
        goal(&mut m, 0, [40, 30]);
        assert_eq!(r.tick(&m), [6]);
        // team 0 carries the Quaffle into the goal area it attacks after 10 s, then out of it
        let carrier = m.teams[0].chasers[0];
        m.holder = Some(carrier);
        m.ball_team = Some(0);
        let hoop = m.hoops.iter().find(|h| h.team == 1).unwrap().pos;
        m.tick = 240;
        m.quaffle = [hoop[0] * 0.95, hoop[1], hoop[2]];
        assert!(qwc_sim::shooting::in_goal_area(m.quaffle, 1, m.pitch.scale_goals, 1.0));
        assert_eq!(r.tick(&m), [4]);
        m.quaffle = [0.0, 10.0, 0.0];
        assert_eq!(r.tick(&m), [6]);
        // again within 8 s: nothing on the way in
        m.tick = 300;
        m.quaffle = [hoop[0] * 0.95, hoop[1], hoop[2]];
        assert!(r.tick(&m).is_empty());
    }

    #[test]
    fn the_match_guard() {
        assert!(match_allows(None, false, 0));
        assert!(!match_allows(Some(6), false, 6));
        assert!(!match_allows(Some(5), false, 1));
        assert!(!match_allows(Some(6), false, 2));
        assert!(match_allows(Some(5), false, 6));
        assert!(!match_allows(Some(5), false, 11));
        assert!(match_allows(Some(10), false, 12));
        assert!(!match_allows(Some(10), false, 5));
        assert!(!match_allows(Some(7), true, 5));
        assert!(match_allows(Some(7), true, 13));
    }

    #[test]
    fn every_menu_id_has_a_gui_event() {
        for id in [1, 3, 4, 5, 6, 7, 8, 9, 10, 25, 26] {
            assert!(gui(id).is_some(), "{id}");
        }
        assert!(gui(12).is_none() && gui(24).is_none());
    }
}
