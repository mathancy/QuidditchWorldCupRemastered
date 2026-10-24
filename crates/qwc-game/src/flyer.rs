//! The flyers' animation controller (`QFlyerAnimControl`, `0x46b000`-`0x473fff`; `docs/ANIMATION.md`, "The flyers'
//! controller"), driving the library's control ([`Control`]) as the original does. A flyer has 13 slots of two
//! records:
//!
//! - 0-2, the flying position: the hover cycle, the fly cycle weighted by the speed, and a lean left or right
//!   weighted by the bank (`0x471b60`, `0x470700`);
//! - 3-5, the event clips (catches, throws, tackles, the brake and the boost), taken in turn (`0x472740`): one
//!   fades in over its blend time while the others and the flying position fade out (`0x471da0`), and the flying
//!   position comes back as the last one nears its end (`0x470b20`);
//! - 6, the cape (straight, or braking during a brake);
//! - 7-9 the hair and 10-12 the broom's tail: a cycle, an up or down clip and a left or right clip, weighted by
//!   the swish (`0x46ef70`, `0x46f130`).
//!
//! The per-tick glue (`0x465a10`, [`Flyer::tick`]) feeds it the speed factor, the bank and the locomotion
//! requests (boost, brake, normal flight); the arena turns the match's events into event clips ([`Flyer::event`],
//! the clips by `0x471790`'s choice of each request type).

use qwc_anim::control::{Control, Event, Mode, SetClip};
use qwc_anim::play::flag;
use qwc_anim::pose::Pose;
use qwc_assets::Vfs;

/// The rows of the flyers' clip table (`0x605c80`).
pub mod row {
    pub const HOVER: usize = 0;
    pub const FLY: usize = 1;
    pub const BOOST: usize = 2;
    pub const LEFT: usize = 3;
    pub const RIGHT: usize = 4;
    pub const BRAKE: usize = 5;
    pub const CAPE: usize = 6;
    pub const CAPE_BRAKE: usize = 7;
    pub const HAIR: usize = 8;
    pub const HAIR_LEFT: usize = 9;
    pub const HAIR_RIGHT: usize = 10;
    pub const HAIR_UP: usize = 11;
    pub const HAIR_DOWN: usize = 12;
    pub const TAIL: usize = 13;
    pub const TAIL_LEFT: usize = 14;
    pub const TAIL_RIGHT: usize = 15;
    pub const TAIL_UP: usize = 16;
    pub const TAIL_DOWN: usize = 17;
}

/// A flyer's role in the table's columns (`+0xd1c`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Keepers and chasers (`med_common`'s `kpch` clips).
    KeeperChaser = 0,
    /// Seekers (`sma_common`).
    Seeker = 1,
    Beater = 2,
}

/// `0x605c80`: each row's clip by role (keeper and chasers, seeker, beater), without and with the Quaffle.
const ROWS: [[[&str; 2]; 3]; 18] = [
    [
        ["med_na_na_hover_c", "med_na_na_hover_c_qf"],
        ["sml_na_sk_flymed_c", "sml_na_sk_flymed_c"],
        ["med_na_bt_hover_c", "med_na_bt_hover_c"],
    ],
    [
        ["med_na_na_fly_c", "med_na_na_fly_c_qf"],
        ["sml_na_sk_flymed_c", "sml_na_sk_flycrouch_c"],
        ["med_na_bt_fly_c", "med_na_bt_fly_c"],
    ],
    [
        ["med_na_na_fly_bst_c", "med_na_na_fly_bst_c_qf"],
        ["sml_na_sk_flycrouch_c", "sml_na_sk_flypose_c"],
        ["med_na_bt_fly_bst", "med_na_bt_fly_bst"],
    ],
    [
        ["med_na_na_flyleft_c", "med_na_na_flyleft_c_qf"],
        ["sml_na_na_flyleft_c", "sml_na_na_flyleft_c"],
        ["med_na_bt_flyleft_c", "med_na_bt_flyleft_c"],
    ],
    [
        ["med_na_na_flyright_c", "med_na_na_flyright_c_qf"],
        ["sml_na_na_flyright_c", "sml_na_na_flyright_c"],
        ["med_na_bt_flyright_c", "med_na_bt_flyright_c"],
    ],
    [
        ["med_na_kpch_brake_c", "med_na_na_slowdown_c_qf"],
        ["sml_na_sk_brake_c", "sml_na_sk_brake_c"],
        ["med_na_bt_brake_c", "med_na_bt_brake_c"],
    ],
    [
        ["med_na_na_cape_slow_straight", "med_na_na_cape_slow_straight"],
        ["sml_na_na_cape_slow_straight", "sml_na_na_cape_slow_straight"],
        ["med_na_na_cape_slow_straight", "med_na_na_cape_slow_straight"],
    ],
    [
        ["med_na_na_cape_slow_brake", "med_na_na_cape_slow_brake"],
        ["sml_na_na_cape_slow_brake", "sml_na_na_cape_slow_brake"],
        ["med_na_na_cape_slow_brake", "med_na_na_cape_slow_brake"],
    ],
    [["hair_cycle"; 2]; 3],
    [["hair_left"; 2]; 3],
    [["hair_right"; 2]; 3],
    [["hair_up"; 2]; 3],
    [["hair_down"; 2]; 3],
    [["broomtail_cycle"; 2]; 3],
    [["broomtail_left"; 2]; 3],
    [["broomtail_right"; 2]; 3],
    [["broomtail_up"; 2]; 3],
    [["broomtail_down"; 2]; 3],
];

/// The clips whose table entries mark a brake (`+0x20` bit 0): the cape brakes while they play (`0x472010` ->
/// `0x471d20`).
const BRAKES: [&str; 4] = ["med_na_kpch_brake_c", "med_na_na_slowdown_c_qf", "med_na_bt_brake_c", "sml_na_sk_brake_c"];

/// The table's playback speeds other than 1 (`+0x18`): the throw's wind-up, the shot and the braking cape.
const SPEEDS: [(&str, f32); 4] = [
    ("med_na_kpch_throw_ph_qf", 0.5),
    ("med_na_ch_shoot_xfh_qf", 0.3),
    ("med_na_na_cape_slow_brake", 0.3),
    ("sml_na_na_cape_slow_brake", 0.3),
];

/// A clip's table speed.
fn table_speed(name: &str) -> f32 {
    SPEEDS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map_or(1.0, |&(_, s)| s)
}

/// The `AnimCtrl` tunings the controller reads (`0x475f90` registers them; the game runs `root/CLI/AnimCtrl.cli`
/// over the defaults at start, `0x475ed0`).
#[derive(Debug, Clone, PartialEq)]
pub struct Tunings {
    /// `ANIM_EVENT_BLEND_TIME_MS`: an event clip's blend in and out.
    pub event_blend_ms: i32,
    /// `FLYING_POSITION_EARLY_FADE_TIME_MS`: how long before the last event's end the flying position returns.
    pub early_fade_ms: f32,
    /// `FLYING_POSITION_BLEND_FACTOR_VELOCITY`, `BANKING_BLEND_FACTOR_VELOCITY`: how far the flying position's
    /// weights move each update.
    pub fly_blend: f32,
    pub bank_blend: f32,
    /// `FLYING_POSITION_BLEND_FACTOR_OVERRIDE`: a fixed weight for the fly cycle (negative: the speed's).
    pub fly_override: f32,
    /// `FLYING_POSITION_BLEND_FACTOR_VELOCITY_DAMPER`: how the speed and bank inputs are damped.
    pub damper: f32,
    /// `BANKING_ROLL_FACTOR`, `BOOST_VEL_FACTOR`, `MIN_BRAKING_VEL_FACTOR`, `MIN_BRAKING_DECCEL`: the glue's.
    pub roll: f32,
    pub boost_vel: f32,
    pub min_brake_vel: f32,
    pub min_brake_deccel: f32,
    /// `ANIM_GLOBAL_PLAYBACK_SPEED` (`+0xe24`; `+0xe28` its inverse).
    pub playback: f32,
    /// `CLOAK_ANIM_BLEND_TIME_MS`
    pub cloak_blend_ms: i32,
    /// `BlendHairSwishValueUD`/`LR`, `BlendHairDamping`, and the broom's.
    pub hair_swish: [f32; 2],
    pub hair_damping: f32,
    pub broom_swish: [f32; 2],
    pub broom_damping: f32,
    /// `ANIM_COORD_SCALING_FACTOR`: the hand offsets' units to the world's.
    pub coord_scale: f32,
}

/// `0x46c380` (into `[0x1017bb8]`, read by `0x472780`): the held Quaffle's offset in the right hand's frame (the
/// left hand's mirrors it in x), before [`Tunings::coord_scale`].
pub const QUAFFLE_IN_HAND: [f32; 3] = [5.472, -16.934, 0.288];

impl Default for Tunings {
    /// The exe's defaults (`[0x601c0c]`-`[0x601cd8]`).
    fn default() -> Tunings {
        Tunings {
            event_blend_ms: 500,
            early_fade_ms: 250.0,
            fly_blend: 1.0,
            bank_blend: 1.0,
            fly_override: 1.0,
            damper: 0.9,
            roll: 1.5,
            boost_vel: 1.0,
            min_brake_vel: 0.2,
            min_brake_deccel: -0.5,
            playback: 1.0,
            cloak_blend_ms: 100,
            hair_swish: [22.5, 15.0],
            hair_damping: 0.3,
            broom_swish: [3.0, 5.0],
            broom_damping: 0.6,
            coord_scale: 0.001,
        }
    }
}

impl Tunings {
    /// The defaults with `root/CLI/AnimCtrl.cli` run over them (each `AnimCtrl.<NAME> <number>` line, the last of
    /// a name winning).
    pub fn load(vfs: &Vfs) -> Tunings {
        let mut t = Tunings::default();
        if let Ok(bytes) = vfs.read("root/CLI/AnimCtrl.cli") {
            t.apply(&String::from_utf8_lossy(&bytes));
        }
        t
    }

    /// Run a `.cli` script's `AnimCtrl` numbers.
    pub fn apply(&mut self, script: &str) {
        for line in script.lines() {
            let line = line.split('#').next().unwrap_or("");
            let mut words = line.split_whitespace();
            let (Some(key), Some(value)) = (words.next(), words.next()) else { continue };
            let Some(name) = key.strip_prefix("AnimCtrl.") else { continue };
            let Ok(v) = value.parse::<f32>() else { continue };
            match name {
                "ANIM_EVENT_BLEND_TIME_MS" => self.event_blend_ms = v as i32,
                "FLYING_POSITION_EARLY_FADE_TIME_MS" => self.early_fade_ms = v,
                "FLYING_POSITION_BLEND_FACTOR_VELOCITY" => self.fly_blend = v,
                "BANKING_BLEND_FACTOR_VELOCITY" => self.bank_blend = v,
                "FLYING_POSITION_BLEND_FACTOR_OVERRIDE" => self.fly_override = v,
                "FLYING_POSITION_BLEND_FACTOR_VELOCITY_DAMPER" => self.damper = v,
                "BANKING_ROLL_FACTOR" => self.roll = v,
                "BOOST_VEL_FACTOR" => self.boost_vel = v,
                "MIN_BRAKING_VEL_FACTOR" => self.min_brake_vel = v,
                "MIN_BRAKING_DECCEL" => self.min_brake_deccel = v,
                "ANIM_GLOBAL_PLAYBACK_SPEED" => self.playback = v,
                "CLOAK_ANIM_BLEND_TIME_MS" => self.cloak_blend_ms = v as i32,
                "BlendHairSwishValueUD" => self.hair_swish[0] = v,
                "BlendHairSwishValueLR" => self.hair_swish[1] = v,
                "BlendHairDamping" => self.hair_damping = v,
                "BlendBroomSwishValueUD" => self.broom_swish[0] = v,
                "BlendBroomSwishValueLR" => self.broom_swish[1] = v,
                "BlendBroomDamping" => self.broom_damping = v,
                "ANIM_COORD_SCALING_FACTOR" => self.coord_scale = v,
                _ => {}
            }
        }
    }
}

/// A slot's state as the controller keeps it (`+0x3b8`): on, fading in, off, fading out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    On,
    In,
    Off,
    Out,
}

/// An event slot's clips (`+0x3d4`, up to four played in turn, `0x4723f0`) and where it is in them.
#[derive(Debug, Clone, PartialEq)]
struct EventSlot {
    clips: Vec<Clip>,
    at: usize,
    /// Milliseconds into the current clip and its length at its speed (`+0x3cc`, `0x472190`).
    elapsed: f32,
    length: f32,
    state: State,
}

/// One clip an event plays: its name, speed and start frame (`+0x3ec`, `+0x3f0`).
#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    pub name: String,
    pub speed: f32,
    pub start: f32,
}

impl Clip {
    pub fn new(name: &str) -> Clip {
        Clip { name: name.to_string(), speed: 1.0, start: 0.0 }
    }
}

/// The hair's or the broom's swish (`+0xed0`, `+0xf50`): the up-down and left-right values, raw and damped, and
/// which clip each slot holds.
#[derive(Debug, Clone, Default, PartialEq)]
struct Swish {
    ud: f32,
    ud_damped: f32,
    lr: f32,
    lr_damped: f32,
    ud_row: Option<usize>,
    lr_row: Option<usize>,
}

/// The glue's locomotion state (the flyer object's `+0x10c4`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Loco {
    None,
    Normal,
    Brake,
    Event,
}

/// A flyer's controller.
pub struct Flyer {
    pub control: Control,
    role: Role,
    quaffle: bool,
    /// The flying position's signed weights and states (slots 0-2, `+0x3c4`, `+0x478`, `+0x52c`).
    weight: [f32; 3],
    state: [State; 3],
    /// The lean slot 2 holds (`+0xf8c` bits `0x1000` left, `0x2000` right) and its last signed weight (`+0xe3c`).
    lean: Option<usize>,
    last_bank: f32,
    events: [Option<EventSlot>; 3],
    /// The event slot last taken (`+0xdf8`, 3-5).
    last_event: usize,
    /// The flying position comes back early once (`+0xf8c` bit `0x20`).
    early: bool,
    /// The damped speed factor and bank (`+0xe2c`, `+0xe30`).
    speed: f32,
    bank: f32,
    loco: Loco,
    /// The glue's memory: the last speed, forward and the brake's wait (`+0x13c`, `+0x144`, `+0x140`).
    prev_speed: Option<f32>,
    prev_fwd: Option<[f64; 3]>,
    brake_wait: u32,
    hair: Swish,
    tail: Swish,
    cape_brake: bool,
    /// `ANIM_GLOBAL_PLAYBACK_SPEED` (`+0xe24`).
    playback: f32,
}

impl Flyer {
    /// A flyer on `set` (its common set's clips), posing into `pose`, flying (`0x46bcd0`, `0x471380`).
    pub fn new(set: Vec<SetClip>, pose: Pose, role: Role, t: &Tunings) -> Flyer {
        let mut control = Control::new(13, 2);
        control.sets = vec![set];
        control.pose = Some(pose);
        // `0x46bd21`-`0x46bd8d`: the flying position, the events and the cape blend by weight; the hair and the
        // broom set (`0x46eb00`, `0x46ece0`)
        for s in 0..13 {
            control.set_mode(s, if s < 7 { 2 } else { 1 });
        }
        for (s, w) in [(0, 1.0), (1, 1.0), (2, 1.0), (3, 0.0), (4, 0.0), (5, 0.0), (7, 1.0)] {
            control.slots[s].weight = w;
        }
        let mut f = Flyer {
            control,
            role,
            quaffle: false,
            weight: [1.0, 0.0, 0.0],
            state: [State::On; 3],
            lean: None,
            last_bank: 0.0,
            events: [None, None, None],
            last_event: 3,
            early: false,
            speed: 0.0,
            bank: 0.0,
            loco: Loco::Normal,
            prev_speed: None,
            prev_fwd: None,
            brake_wait: 0,
            hair: Swish { ud: 0.8, ud_damped: 0.8, ..Default::default() },
            tail: Swish::default(),
            cape_brake: false,
            playback: t.playback,
        };
        f.start_flying();
        f.cape(false);
        f.play(7, &Clip::new(f.clip(row::HAIR)), flag::LOOP);
        f.play(10, &Clip::new(f.clip(row::TAIL)), flag::LOOP);
        f
    }

    /// Row `r`'s clip for this flyer (`0x46b4d0`).
    fn clip(&self, r: usize) -> &'static str {
        ROWS[r][self.role as usize][self.quaffle as usize]
    }

    /// Cut slot `slot` to `clip` (`0x472010` -> `0x4795d0`) at the playback speed times its own and the table's.
    fn play(&mut self, slot: usize, clip: &Clip, flags: u32) {
        let speed = clip.speed * self.playback * table_speed(&clip.name);
        self.control.play(&clip.name, flags, Some(Mode::Cut), slot, None, speed, clip.start);
    }

    /// `0x471b60`: the flying position's three clips start again (the hover, the fly and the lean the bank
    /// calls for).
    fn start_flying(&mut self) {
        let lean = if self.last_bank < 0.0 { row::LEFT } else { row::RIGHT };
        for (s, r) in [(0, row::HOVER), (1, row::FLY), (2, lean)] {
            self.play(s, &Clip::new(self.clip(r)), flag::LOOP);
        }
        self.lean = Some(lean);
    }

    /// The cape straight or braking (`0x471c40`, `0x471d20`): slot 6 cross-fades over the cloak's blend time.
    fn cape(&mut self, brake: bool) {
        let (r, flags) = if brake { (row::CAPE_BRAKE, flag::HOLD) } else { (row::CAPE, flag::LOOP) };
        let name = self.clip(r);
        let speed = self.playback * table_speed(name);
        self.control.play(name, flags, Some(Mode::Cut), 6, None, speed, 0.0);
        self.cape_brake = brake;
    }

    /// Whether any event slot is busy (`0x4650d0`, `0x4733a0`: on, fading in or fading out).
    fn event_busy(&self) -> bool {
        self.events.iter().flatten().any(|e| e.state != State::Off)
    }

    /// `0x472740`: the next event slot after the last taken whose state is off (3, 4, 5 in turn).
    fn free_event_slot(&self) -> Option<usize> {
        let mut s = self.last_event;
        for _ in 0..3 {
            s = if s >= 5 { 3 } else { s + 1 };
            if self.events[s - 3].as_ref().is_none_or(|e| e.state == State::Off) {
                return Some(s);
            }
        }
        None
    }

    /// Play `clips` (in turn) as an event (`0x471020` -> `0x471da0`): it fades in over `blend` milliseconds on a
    /// free event slot, the other events and the flying position fade out. Nothing plays when every event slot is
    /// busy, or when no clip is found.
    pub fn event(&mut self, clips: Vec<Clip>, blend: i32, t: &Tunings) -> bool {
        let Some(slot) = self.free_event_slot() else { return false };
        let Some(first) = clips.first().filter(|c| self.control.find(&c.name).is_some()) else { return false };
        let first = first.clone();
        for s in 3..6 {
            if s != slot {
                self.fade_out_event(s, t.event_blend_ms);
            }
        }
        self.last_event = slot;
        self.control.slots[slot].weight = 0.0;
        self.play(slot, &first, flag::HOLD);
        self.control.fade_in(slot, blend.max(1));
        for s in &mut self.state {
            if !matches!(s, State::Out | State::Off) {
                *s = State::Out;
            }
        }
        self.early = true;
        if BRAKES.iter().any(|b| b.eq_ignore_ascii_case(&first.name)) {
            self.cape(true);
        }
        let length = self.length(&first);
        self.events[slot - 3] = Some(EventSlot { clips, at: 0, elapsed: 0.0, length, state: State::In });
        true
    }

    /// A clip's length in milliseconds at its speed (`0x472190`).
    fn length(&self, c: &Clip) -> f32 {
        let secs = self.control.seconds(&c.name).unwrap_or(0.0);
        let frames = self.control.find(&c.name).map_or(0.0, |(_, i)| i.fps * secs);
        let left = if frames > 0.0 { secs * (1.0 - c.start / frames).max(0.0) } else { secs };
        left * 1000.0 / (c.speed * self.playback * table_speed(&c.name)).max(1e-3)
    }

    /// `0x4725a0(8)`: an event slot fades out.
    fn fade_out_event(&mut self, slot: usize, ms: i32) {
        if let Some(e) = &mut self.events[slot - 3] {
            if matches!(e.state, State::On | State::In) {
                e.state = State::Out;
                self.control.fade_out(slot, ms.max(1));
            }
        }
    }

    /// The glue's normal-flight request (`0x464ff0` -> `0x471380`): the flying position back, the cape straight,
    /// the events out.
    fn normal(&mut self, t: &Tunings) {
        if self.state.iter().any(|s| matches!(s, State::Off | State::Out)) {
            self.start_flying();
        }
        self.state = [State::On; 3];
        if self.cape_brake {
            self.cape(false);
        }
        for s in 3..6 {
            self.fade_out_event(s, t.event_blend_ms);
        }
        self.loco = Loco::Normal;
    }

    /// The glue, once a tick (`0x465a10`): the speed factor and the bank from the flyer's velocity and frames, the
    /// Quaffle held, the boost, the brake and the return to normal flight. `right` and `fwd` are the flyer's
    /// matrix rows 0 and 2; `boosting` is the player's boost (the object at `+0x650`).
    pub fn tick(&mut self, vel: [f64; 3], right: [f64; 3], fwd: [f64; 3], quaffle: bool, boosting: bool, t: &Tunings) {
        if quaffle != self.quaffle {
            // **[I]** a new holder's flying position switches to the Quaffle's clips at once (the writer of
            // `+0xf8c` bit 2 is not traced)
            self.quaffle = quaffle;
            if !self.event_busy() {
                self.start_flying();
            }
        }
        let speed = (vel[0] * vel[0] + vel[1] * vel[1] + vel[2] * vel[2]).sqrt() as f32;
        let delta = speed - self.prev_speed.unwrap_or(speed);
        let mut s = speed * 0.05;
        if delta > 0.05 {
            s *= (delta - 0.05) * 2.0 + 1.0;
        }
        // `0x4732e0`, `0x473340`: damped towards the new values
        self.speed = t.damper * self.speed + (1.0 - t.damper) * s.clamp(0.0, 1.0);
        let pf = self.prev_fwd.unwrap_or(fwd);
        let turn = (pf[0] * right[0] + pf[1] * right[1] + pf[2] * right[2]) as f32;
        let bank = -(turn * t.roll * 4.0).clamp(-1.0, 1.0);
        self.bank = t.damper * self.bank + (1.0 - t.damper) * bank;
        if boosting && !self.event_busy() {
            self.event(vec![Clip::new(self.clip(row::BOOST))], 200, t);
        }
        if self.loco == Loco::Event && !self.event_busy() {
            self.loco = Loco::None;
        }
        if self.loco != Loco::Event {
            self.brake_wait = self.brake_wait.saturating_sub(1);
            let fast = t.boost_vel * 25.0 < speed;
            let mut brake = false;
            if !fast {
                let (mut floor, mut decel) = (t.min_brake_vel * 20.0, t.min_brake_deccel);
                if self.loco == Loco::Brake {
                    (floor, decel) = (floor * 0.5, -0.01);
                }
                brake = delta < decel && floor <= speed;
            }
            if fast {
                // the boost cycle as an event (`0x4733a0`), while no other event plays
                if !self.event_busy() {
                    self.event(vec![Clip::new(self.clip(row::BOOST))], 200, t);
                }
            } else if brake {
                if self.loco != Loco::Brake && self.brake_wait == 0 {
                    if self.bank.abs() < 0.2 {
                        // `0x465040`: the brake clip as an event (request type 3)
                        if self.event(vec![Clip::new(self.clip(row::BRAKE))], t.event_blend_ms, t) {
                            self.loco = Loco::Brake;
                        }
                    }
                    self.brake_wait = 8;
                }
            } else if self.loco != Loco::Normal {
                self.normal(t);
            }
        }
        self.prev_speed = Some(speed);
        self.prev_fwd = Some(fwd);
    }

    /// A gameplay event's clips (the request's kind 0, `0x46aa00`: the glue waits while they play).
    pub fn request(&mut self, clips: Vec<Clip>, t: &Tunings) {
        if self.event(clips, t.event_blend_ms, t) {
            self.loco = Loco::Event;
        }
    }

    /// The controller's update and the library's, `dt` seconds (`0x46c9e0`): the events' sequences and early
    /// return, the cape's speed, the hair and the broom, the flying position's weights; then the slots and the
    /// clips (`0x47a1c0`, `0x478a10`). **[I]** the per-update rates are taken per 1/24 s.
    pub fn update(&mut self, dt: f32, t: &Tunings, events: &mut Vec<Event>) {
        let ms = dt * 1000.0;
        let k = (dt * 24.0).max(0.0);
        self.update_events(ms, t);
        if !self.cape_brake {
            self.control.set_speed(6, self.speed * 2.5 + 0.5);
        }
        self.swish(k, t);
        self.flying_weights(k, t);
        self.control.update_slots(ms, events);
        self.control.step(dt, events);
    }

    /// `0x4723f0` (an event clip's end: the next in turn, or the last held) and `0x470b20` (the flying position back
    /// `FLYING_POSITION_EARLY_FADE_TIME_MS` before the last clip's end); events faded out are let go.
    fn update_events(&mut self, ms: f32, t: &Tunings) {
        let mut early = false;
        for i in 0..3 {
            let slot = i + 3;
            let Some(e) = &mut self.events[i] else { continue };
            e.elapsed += ms;
            if e.state == State::In && self.control.slots[slot].fade == 0 {
                e.state = State::On;
            }
            if e.state == State::Out && self.control.slots[slot].fade == 0 && self.control.slots[slot].weight <= 0.0 {
                e.state = State::Off;
            }
            if e.state == State::Off {
                continue;
            }
            if e.elapsed >= e.length && e.at + 1 < e.clips.len() {
                e.at += 1;
                e.elapsed -= e.length;
                let next = e.clips[e.at].clone();
                let length = self.length(&next);
                self.play(slot, &next, flag::HOLD);
                if let Some(e) = &mut self.events[i] {
                    e.length = length;
                }
                continue;
            }
            let last = e.at + 1 >= e.clips.len();
            if last && matches!(e.state, State::On | State::In) && e.length - e.elapsed < t.early_fade_ms / t.playback {
                early = true;
            }
        }
        if early && self.early && self.loco != Loco::Brake {
            self.early = false;
            for s in 3..6 {
                self.fade_out_event(s, t.event_blend_ms);
            }
            self.start_flying();
            self.state = [State::In; 3];
            self.weight = [1.0, self.fly_target(t), self.bank];
            if self.cape_brake {
                self.cape(false);
            }
        }
    }

    /// The fly cycle's target weight: the override, or the speed (`0x470700`).
    fn fly_target(&self, t: &Tunings) -> f32 {
        if t.fly_override < 0.0 {
            self.speed
        } else {
            t.fly_override
        }
    }

    /// `0x470700`: the flying position's weights move towards the hover's 1, the speed and the bank (or 0 while
    /// events play); the lean slot changes sides with the bank; the slots take the weights.
    fn flying_weights(&mut self, k: f32, t: &Tunings) {
        let on = matches!(self.state[0], State::On | State::In);
        let target = if on { [1.0, self.fly_target(t), self.bank] } else { [0.0; 3] };
        let rate = [
            t.fly_blend * t.playback,
            t.fly_blend * t.playback,
            t.bank_blend * t.playback * if on { 1.0 } else { 0.75 },
        ];
        for i in 0..3 {
            let (w, g, r) = (&mut self.weight[i], target[i], rate[i] * k);
            if (*w - g).abs() <= r {
                *w = g;
            } else if *w <= g {
                *w += r;
            } else {
                *w -= r;
            }
            if *w == g {
                self.state[i] = match self.state[i] {
                    State::In => State::On,
                    State::Out => State::Off,
                    s => s,
                };
            }
        }
        if on {
            let side = if self.last_bank < 0.0 { row::LEFT } else { row::RIGHT };
            if self.last_bank != 0.0 && self.lean != Some(side) {
                let name = self.clip(side);
                self.control.play(name, flag::LOOP, Some(Mode::CrossFade), 2, None, 1.0, 0.0);
                self.lean = Some(side);
                self.weight[2] = 0.0;
            }
        }
        self.last_bank = self.weight[2];
        for i in 0..3 {
            self.control.slots[i].weight = self.weight[i].abs();
        }
    }

    /// `0x46ef70` and `0x46f130`: the hair's and the broom's cycles weighted by the speed (0.1 and 0.2 more) and
    /// played faster with it (1 to 5 times), and their swish clips by the swish's signs, weighted by its size.
    /// **[I]** the swish's motion term (`0x46f910`, from the head's and the broom's nodes) is taken from the bank.
    fn swish(&mut self, k: f32, t: &Tunings) {
        let s = self.speed;
        let playback = (5.0 - 1.0) * s + 1.0;
        self.control.set_speed(7, playback);
        self.control.set_speed(10, playback);
        self.control.slots[7].weight = (0.1 + s).min(1.0);
        self.control.slots[10].weight = (0.2 + s).min(1.0);
        let bank = self.bank;
        let damp = |v: f32, d: f32, f: f32| (d + (v - d) * (f * k).min(1.0)).clamp(-1.0, 1.0);
        self.hair.ud = 0.8 * (1.0 - s);
        self.hair.lr = -bank * t.hair_swish[1] / t.hair_swish[0];
        self.hair.ud_damped = damp(self.hair.ud, self.hair.ud_damped, t.hair_damping);
        self.hair.lr_damped = damp(self.hair.lr, self.hair.lr_damped, t.hair_damping);
        self.tail.ud = 0.0;
        self.tail.lr = -bank * t.broom_swish[1] / t.broom_swish[0] * 0.5;
        self.tail.ud_damped = damp(self.tail.ud, self.tail.ud_damped, t.broom_damping);
        self.tail.lr_damped = damp(self.tail.lr, self.tail.lr_damped, t.broom_damping);
        let hair = std::mem::take(&mut self.hair);
        let tail = std::mem::take(&mut self.tail);
        self.hair = self.swish_slots(hair, [8, 9], [row::HAIR_DOWN, row::HAIR_UP, row::HAIR_LEFT, row::HAIR_RIGHT]);
        self.tail = self.swish_slots(tail, [11, 12], [row::TAIL_DOWN, row::TAIL_UP, row::TAIL_LEFT, row::TAIL_RIGHT]);
    }

    /// A swish's two slots: the up-down clip by its sign (`rows[0]` positive, `rows[1]` not), the left-right clip
    /// likewise (`rows[2]`, `rows[3]`), each weighted by its damped value's size.
    fn swish_slots(&mut self, mut w: Swish, slots: [usize; 2], rows: [usize; 4]) -> Swish {
        let ud = if w.ud_damped > 0.0 { rows[0] } else { rows[1] };
        if w.ud_row != Some(ud) {
            self.play(slots[0], &Clip::new(self.clip(ud)), flag::LOOP);
            w.ud_row = Some(ud);
        }
        let lr = if w.lr_damped > 0.0 { rows[2] } else { rows[3] };
        if w.lr_row != Some(lr) {
            self.play(slots[1], &Clip::new(self.clip(lr)), flag::LOOP);
            w.lr_row = Some(lr);
        }
        self.control.slots[slots[0]].weight = w.ud_damped.abs();
        self.control.slots[slots[1]].weight = w.lr_damped.abs();
        w
    }

    /// The flying position's weights (hover, fly, lean), for checks.
    #[cfg(test)]
    pub fn flying(&self) -> [f32; 3] {
        self.weight
    }
}

/// A direction's quadrant around a facing (`0x475bd0`): 0 ahead, 1 right, 2 behind, 3 left. `angle` is the
/// direction's bearing from the facing, positive to the right, in radians.
pub fn quadrant(angle: f64) -> usize {
    let a = angle.rem_euclid(std::f64::consts::TAU);
    let q = std::f64::consts::FRAC_PI_4;
    if a < q || a >= 7.0 * q {
        0
    } else if a < 3.0 * q {
        1
    } else if a < 5.0 * q {
        2
    } else {
        3
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qwc_anim::play::ClipInfo;

    fn set() -> Vec<SetClip> {
        let names: Vec<&str> =
            ROWS.iter().flat_map(|r| r[0].iter().copied()).chain(["med_na_kpch_catch_rs_qf"]).collect();
        names
            .into_iter()
            .map(|n| SetClip { name: Some(n.into()), clip: ClipInfo { first: 0, last: 30, fps: 30.0 }, data: None })
            .collect()
    }

    fn flyer() -> Flyer {
        Flyer::new(set(), Pose::new(vec![], vec![], vec![]), Role::KeeperChaser, &Tunings::default())
    }

    #[test]
    fn the_cli_overrides_the_defaults_and_the_last_line_wins() {
        let mut t = Tunings::default();
        t.apply(
            "AnimCtrl.ANIM_EVENT_BLEND_TIME_MS  200  # blend\nAnimCtrl.FLYING_POSITION_BLEND_FACTOR_VELOCITY 0.12\n\
             AnimCtrl.FLYING_POSITION_BLEND_FACTOR_VELOCITY 0.2\nAnimCtrl.BUTTON_NUM_ANIMS CIRCLE 1\n\
             AnimCtrl.MODEL_BASE_FILE_NAME \"x\"",
        );
        assert_eq!(t.event_blend_ms, 200);
        assert_eq!(t.fly_blend, 0.2);
        assert_eq!(t.roll, 1.5);
    }

    #[test]
    fn speed_raises_the_fly_cycle_over_the_hover() {
        let t = Tunings { fly_override: -1.0, fly_blend: 0.2, ..Tunings::default() };
        let mut f = flyer();
        let mut ev = Vec::new();
        for _ in 0..200 {
            f.tick([20.0, 0.0, 0.0], [0.0, 0.0, -1.0], [1.0, 0.0, 0.0], false, false, &t);
            f.update(1.0 / 24.0, &t, &mut ev);
        }
        let w = f.flying();
        assert_eq!(w[0], 1.0);
        assert!((w[1] - 1.0).abs() < 0.01, "{w:?}");
        assert_eq!(f.control.slots[1].weight, w[1]);
    }

    #[test]
    fn an_event_takes_over_and_the_flying_position_returns_before_it_ends() {
        let t = Tunings { event_blend_ms: 200, early_fade_ms: 200.0, ..Tunings::default() };
        let mut f = flyer();
        let mut ev = Vec::new();
        f.request(vec![Clip::new("med_na_kpch_catch_rs_qf")], &t);
        assert_eq!(f.last_event, 4);
        for _ in 0..12 {
            f.update(1.0 / 24.0, &t, &mut ev);
        }
        // half a second in: the event is up, the flying position down
        assert_eq!(f.control.slots[4].weight, 1.0);
        assert_eq!(f.flying(), [0.0; 3]);
        for _ in 0..12 {
            f.update(1.0 / 24.0, &t, &mut ev);
        }
        // past 0.8 s of the 1 s clip: the flying position is back and the event fading out
        assert_eq!(f.flying()[0], 1.0);
        assert!(f.control.slots[4].weight < 1.0);
    }

    #[test]
    fn quadrants_go_round_from_ahead_to_the_right() {
        assert_eq!(quadrant(0.1), 0);
        assert_eq!(quadrant(1.6), 1);
        assert_eq!(quadrant(3.1), 2);
        assert_eq!(quadrant(-1.6), 3);
    }
}
