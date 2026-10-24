//! The match's effects (`GameRender\Common`: `DynamicGeomSystem`, `ParticleSystem`, `0x4ac000`-`0x4be000`), from
//! their text definitions (`RE_NOTES.md`, "Text definitions"): the dynamic geometry (`.got`, `GEOM`), the particles
//! (`.POT`, `BEHAVIOUR` and `DESCRIPTOR`) and their materials (their own `MATERIAL` blocks, or the `.gro`'s).
//!
//! **[T]**:
//! - each definition is instanced per matching actor (`0x4bd2c0`): its locator and trigger are named
//!   `<actor>_<node>`, `LOCATOR_PARENT` a pattern over the actors (`ACTOR`, `ACTOR.TEAM0`, `ACTOR.TEAM0.KEEPER`,
//!   `ACTOR.TEAM0!KEEPER` all but the keeper; `BLUDGER`, `QUAFFLE`, `HOOP`; `GLOBAL` and `WORLD` the one world);
//! - the triggers' names and ids (the table at `0x5fb998`: the players' `ACTIVE`, `BOOST`, `HIGHLIGHT_ACTIVE`,
//!   `HIGHLIGHT_POSSESSION`, `HIGHLIGHT_IN_GOAL_AREA`, `PASS_TARGET`, `STATE_TRANSITION`...; the hoops' `GLOW`,
//!   `GOAL`, `TARGET`);
//! - a geometry's colour and size go from start to mid over its mid time and on to end over its life
//!   (`0x4bce10`); a burst lives once;
//! - the behaviours' layout (`0x4afe50`): base, clamps and variance for the acceleration, the position and the
//!   velocity, the ring radius, the velocity threshold, the alpha fades, the size and its velocity and
//!   acceleration, the emission rate, the life, the rotation, and the rotate, reverse and radial flags; a particle
//!   moves as `p0 + v t + a t²`, each value its base plus or minus its variance (`0x4ade30`'s bounds);
//! - the blend modes by name (the Groper table at `0x5fc0c0`; the engine's type is the index plus one);
//! - a trail (`.trl`, `TRAIL`: the fields `0x4b9aa0` reads) lays a point each frame its trigger is on and its
//!   locator moves faster than its threshold (`0x4b8100`): two edges at its `rot_pos` plus and minus its start
//!   width across, turned by `rot_base + rot_vel t` (radians, the Euler helper `0x4213f0`), moved by its `pos`
//!   and put in the locator's frame; a point lasts its life.
//!
//! **[I]** (`crate::arena` sets the triggers from the match, their setters not traced):
//! - the alpha fades as fractions of a particle's life; the emission per second while the locator moves faster
//!   than the threshold; a placed particle (`dynamic_placement_flag`) left in the world, the others following
//!   their locator; the colours 128 for one;
//! - the Quaffle's glow is the shot's wind-up (the original's `+0x40` setter is not traced); the Bludgers' attacking
//!   material (`bludger_attacking`) is not in the data, so they keep `bludger_normal`;
//! - a trail's ribbon narrows or widens from its start width at the head to its end width at the tail, fades in
//!   over its `alpha_fadein` and out over its `alpha_fadeout` (fractions of its life), and takes the texture along
//!   its length every `1 / tile` of it;
//! - a geometry's width and height are half its sides, as the engine's quads go from `-s` to `s` (`0x493b90`,
//!   `0x4a0c70`, `0x4a77a0`); the balls' sprites (`0x494120`, `0x4940e0`): a camera-facing square at the ball,
//!   the Quaffle's in `quaffle_held` while held, else `quaffle_normal`, its half side from 0.65 to 2 and its colour
//!   from white to red by the ball's glow (`+0x40`); the Bludgers' in `bludger_normal`, 0.65;
//! - a `SPHERE` or `ELLIPSOID` geometry is a globe of its `tess` segments round, half as many from pole to pole,
//!   its width the radius across and its height up;
//! - the `RECT_2D` shape and the lights (`.lot`) are not drawn yet.

use std::collections::{BTreeMap, HashMap};

use qwc_assets::groper::blend_of;
use qwc_assets::Vfs;
use qwc_formats::text_defs::{self, FxBlock, FxValue};
use qwc_render::effects::{EffectQuad, EffectVertex, Effects};
use qwc_render::glam::{Mat3, Vec3};
use qwc_render::{Camera, Gpu};

/// The engine's `SRCALPHA_ADD` blend type (the Groper index 6 plus one).
const SRCALPHA_ADD: u8 = 7;

/// The engine's `SRCALPHA` blend type.
const SRCALPHA: u8 = 2;

/// A Groper wave (`func base amplitude phase frequency`). **[I]** Quake 3's shaders' meaning, which the Groper
/// directives follow (`sin`, `triangle`, `square`, `sawtooth`, `inversesawtooth`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Wave {
    func: u8,
    base: f32,
    amp: f32,
    phase: f32,
    freq: f32,
}

impl Wave {
    fn parse(w: &[&str]) -> Option<Wave> {
        let func = match w.first()?.to_ascii_lowercase().as_str() {
            "sin" => 0,
            "triangle" => 1,
            "square" => 2,
            "sawtooth" => 3,
            "inversesawtooth" => 4,
            _ => return None,
        };
        let n = |i: usize| w.get(i).map_or(0.0, |s| num(s));
        Some(Wave { func, base: n(1), amp: n(2), phase: n(3), freq: n(4) })
    }

    fn at(&self, t: f32) -> f32 {
        let x = (self.phase + self.freq * t).rem_euclid(1.0);
        let f = match self.func {
            0 => (x * std::f32::consts::TAU).sin(),
            1 => {
                if x < 0.25 {
                    4.0 * x
                } else if x < 0.75 {
                    2.0 - 4.0 * x
                } else {
                    4.0 * x - 4.0
                }
            }
            2 => {
                if x < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            3 => x,
            _ => 1.0 - x,
        };
        self.base + self.amp * f
    }
}

/// A material's stage as drawn: its texture, blend type, depth write, constant colour, alpha wave and its
/// texture coordinates' turn (degrees a second) and stretch.
#[derive(Debug, Clone, PartialEq)]
struct Stage {
    texture: String,
    blend: u8,
    depth_write: bool,
    rgb: [f32; 3],
    alpha: Option<Wave>,
    rotate: f32,
    stretch: Option<Wave>,
    /// `tcMod scroll`: texture widths a second (`0x4a98b0`: by `speed * t`).
    scroll: [f32; 2],
    /// The texture repeats (`map`) rather than stopping at its edges (`clampmap`, and the effects without a
    /// material).
    wrap: bool,
}

impl Stage {
    fn plain(texture: String, blend: u8, depth_write: bool) -> Stage {
        Stage {
            texture,
            blend,
            depth_write,
            rgb: [1.0; 3],
            alpha: None,
            rotate: 0.0,
            stretch: None,
            scroll: [0.0; 2],
            wrap: false,
        }
    }

    /// A `.gro` stage from its directives (`clampmap`/`map`, `alphamode`, `depthwrite`, `rgbGen const`,
    /// `alphaGen wave`, `tcMod rotate`, `tcMod stretch`, `tcMod scroll`).
    fn of_gro(directives: &[String]) -> Stage {
        let mut s = Stage::plain(String::new(), SRCALPHA, false);
        for d in directives {
            let w: Vec<&str> = d.split_whitespace().filter(|w| *w != "(" && *w != ")").collect();
            let Some(k) = w.first().map(|k| k.to_ascii_lowercase()) else { continue };
            match k.as_str() {
                "clampmap" | "map" => {
                    s.texture = w.get(1).map_or(String::new(), |t| t.to_string());
                    s.wrap = k == "map";
                }
                "alphamode" => s.blend = w.get(1).and_then(|b| blend_of(b)).unwrap_or(SRCALPHA),
                "depthwrite" => s.depth_write = true,
                "rgbgen" if w.get(1).is_some_and(|g| g.eq_ignore_ascii_case("const")) => {
                    s.rgb = std::array::from_fn(|i| w.get(2 + i).map_or(1.0, |v| num(v)));
                }
                "alphagen" if w.get(1).is_some_and(|g| g.eq_ignore_ascii_case("wave")) => {
                    s.alpha = Wave::parse(&w[2..])
                }
                "tcmod" => match w.get(1).map(|m| m.to_ascii_lowercase()).as_deref() {
                    Some("rotate") => s.rotate = w.get(2).map_or(0.0, |v| num(v)),
                    Some("stretch") => s.stretch = Wave::parse(&w[2..]),
                    Some("scroll") => s.scroll = [2, 3].map(|i| w.get(i).map_or(0.0, |v| num(v))),
                    _ => {}
                },
                _ => {}
            }
        }
        s
    }

    /// A corner's texture coordinates at time `t`: turned about the middle, stretched about it, then scrolled.
    fn uv(&self, u: f32, v: f32, t: f32) -> [f32; 2] {
        let (mut x, mut y) = (u - 0.5, v - 0.5);
        if self.rotate != 0.0 {
            let (s, c) = (-self.rotate * t).to_radians().sin_cos();
            (x, y) = (x * c - y * s, x * s + y * c);
        }
        if let Some(w) = self.stretch {
            let k = w.at(t);
            if k.abs() > 1e-4 {
                (x, y) = (x / k, y / k);
            }
        }
        let [su, sv] = self.scroll.map(|k| (k * t).rem_euclid(1.0));
        [x + 0.5 + su, y + 0.5 + sv]
    }
}

/// A material as drawn: its stages, each its own quad.
#[derive(Debug, Clone, PartialEq)]
struct Material {
    stages: Vec<Stage>,
}

/// One of the match's actors an effect can hang on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Actor {
    /// A player by the match's index.
    Player(usize),
    Quaffle,
    Bludger(usize),
    Hoop(usize),
    /// The Snitch, in the Seeker game.
    Snitch,
    World,
}

/// Where an actor is this frame and the triggers it has on.
#[derive(Debug, Clone)]
pub struct ActorState {
    pub actor: Actor,
    /// For a player: its effects team (`TEAM0` or `TEAM1`) and role (`KEEPER`, `CHASER`, `BEATER`, `SEEKER`).
    pub team: usize,
    pub role: &'static str,
    pub pos: Vec3,
    pub vel: Vec3,
    /// A ball's glow (0 to 1: its sprite's size and redness) and whether it is held.
    pub glow: f32,
    pub held: bool,
    /// Its frame: forward (z), up (y).
    pub fwd: Vec3,
    pub triggers: Vec<&'static str>,
}

/// A locator or trigger parent: which actors it names.
#[derive(Debug, Clone, PartialEq)]
enum Pattern {
    Players { team: Option<usize>, only: Option<String>, not: Option<String> },
    Quaffle,
    Bludgers,
    Hoops,
    Snitch,
    World,
    Nothing,
}

impl Pattern {
    fn parse(s: &str) -> Pattern {
        let s = s.trim_matches('"').to_ascii_uppercase();
        match s.as_str() {
            "GLOBAL" | "WORLD" => return Pattern::World,
            "QUAFFLE" => return Pattern::Quaffle,
            "BLUDGER" => return Pattern::Bludgers,
            "HOOP" => return Pattern::Hoops,
            "SNITCH" => return Pattern::Snitch,
            _ => {}
        }
        // a team's players after `ACTOR`; a name without it names nothing (the seekers' `seeker_game_trail`s name
        // `TEAM0.SEEKER`, and the original does not draw them either: the user's footage, 2026-10-21)
        let Some(rest) = s.strip_prefix("ACTOR") else { return Pattern::Nothing };
        let mut team = None;
        let (mut only, mut not) = (None, None);
        let rest = rest.strip_prefix('.').unwrap_or(rest);
        let mut tail = rest;
        if let Some(t) = rest.strip_prefix("TEAM0") {
            team = Some(0);
            tail = t;
        } else if let Some(t) = rest.strip_prefix("TEAM1") {
            team = Some(1);
            tail = t;
        }
        if let Some(r) = tail.strip_prefix('!') {
            not = Some(r.to_string());
        } else if let Some(r) = tail.strip_prefix('.') {
            only = Some(r.to_string());
        }
        Pattern::Players { team, only, not }
    }

    fn matches(&self, a: &ActorState) -> bool {
        match (self, a.actor) {
            (Pattern::Players { team, only, not }, Actor::Player(_)) => {
                team.is_none_or(|t| t == a.team)
                    && only.as_ref().is_none_or(|r| a.role.starts_with(r.as_str()))
                    && not.as_ref().is_none_or(|r| !a.role.starts_with(r.as_str()))
            }
            (Pattern::Quaffle, Actor::Quaffle) | (Pattern::World, Actor::World) | (Pattern::Snitch, Actor::Snitch) => {
                true
            }
            (Pattern::Bludgers, Actor::Bludger(_)) | (Pattern::Hoops, Actor::Hoop(_)) => true,
            _ => false,
        }
    }
}

/// A geometry's shape (the `.got` keyword table: `RECT`, `RECT_2D`, `SPHERE`, `ELLIPSOID`).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Shape {
    Rect,
    /// A sphere or an ellipsoid, of so many segments round.
    Globe(u32),
}

/// A `GEOM` block (`.got`).
#[derive(Debug, Clone)]
struct Geom {
    shape: Shape,
    locator: Pattern,
    trigger: (Pattern, String),
    material: Option<Material>,
    burst: bool,
    camera_face: bool,
    rot_ignore: bool,
    colour: [[f32; 4]; 3],
    width: [f32; 3],
    height: [f32; 3],
    pos: Vec3,
    rot: Vec3,
    life: f32,
    mid: f32,
    uv: [f32; 4],
}

/// A `BEHAVIOUR` block (`.POT`, `0x4afe50`'s fields).
#[derive(Debug, Clone, Default)]
struct Behaviour {
    pos: [Vec3; 2],
    vel: [Vec3; 2],
    acc: [Vec3; 2],
    ring: Vec3,
    vel_thresh: f32,
    fade_in: f32,
    fade_out: f32,
    size: [f32; 2],
    size_vel: [f32; 2],
    size_acc: [f32; 2],
    size_clamp: [f32; 2],
    rate: [f32; 2],
    rate_clamp: [f32; 2],
    life: [f32; 2],
    life_clamp: [f32; 2],
    rot: [f32; 2],
    rot_vel: [f32; 2],
    rot_acc: [f32; 2],
    rotate: bool,
    radial: bool,
}

/// A `DESCRIPTOR` block (`.POT`): an emitter of a behaviour's particles.
#[derive(Debug, Clone)]
struct Descriptor {
    behaviour: Behaviour,
    material: Option<Material>,
    locator: Pattern,
    trigger: (Pattern, String),
    colour: [f32; 4],
    placed: bool,
}

/// A phase of the goal effect (`GoalRender.<ring|flare|sphere>.*`, registered by `0x4a1210`): its span in seconds
/// from the goal, its fades and middle as fractions of the span, its sizes at the start, the middle and the end.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GoalPhase {
    pub start: f32,
    pub mid: f32,
    pub end: f32,
    pub fade_in: f32,
    pub fade_out: f32,
    pub size: [f32; 3],
}

impl GoalPhase {
    /// Its alpha and size `t` seconds after the goal (`0x4a0790`); none outside its span.
    pub fn at(&self, t: f32) -> Option<(f32, f32)> {
        let x = (t - self.start) / (self.end - self.start);
        if !(0.0..1.0).contains(&x) {
            return None;
        }
        let alpha = if x < self.fade_in {
            x / self.fade_in
        } else if x < self.fade_out {
            1.0
        } else {
            1.0 - (x - self.fade_out) / (1.0 - self.fade_out)
        };
        let [s0, s1, s2] = self.size;
        let size = if x < self.mid {
            s0 + (s1 - s0) * (x / self.mid)
        } else {
            s1 + (s2 - s1) * ((x - self.mid) / (1.0 - self.mid))
        };
        Some((alpha, size))
    }
}

/// The goal effect (`GameRender\Common\GoalRender.cpp`: `0x4a04b0`, drawn by `0x4a06e0`): from the goal's message
/// (`0x1043b60`) a ring and a flare in the scored hoop's plane (`0x4a0c70`, `0x4a08f0`) and a sphere about its
/// centre (`0x4a0ff0`, 16 segments, turning by `RotSpeed` over its span), in `GOAL_RING`, `GOAL_FLARE` and
/// `GOAL_PLASMA`, each phase's size times `Size`. The tunings are `stadium/Common/fx/goalfx.cli`'s.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GoalRender {
    pub size: f32,
    pub rot_speed: f32,
    /// The ring, the flare and the sphere.
    pub phases: [GoalPhase; 3],
}

impl Default for GoalRender {
    fn default() -> GoalRender {
        let p = |start, mid, end, fade_in, fade_out, size| GoalPhase { start, mid, end, fade_in, fade_out, size };
        GoalRender {
            size: 3.0,
            rot_speed: 0.0,
            phases: [
                p(0.0, 0.3, 0.9, 0.4, 0.8, [0.4, 1.0, 1.6]),
                p(0.0, 0.7, 1.0, 0.01, 0.7, [0.0, 1.5, 3.0]),
                p(0.0, 0.3, 0.6, 0.01, 0.5, [0.4, 0.5, 0.6]),
            ],
        }
    }
}

impl GoalRender {
    /// The tunings with `script`'s `GoalRender.*` lines applied (a console script: `key value` a line).
    pub fn apply(&mut self, script: &str) {
        for line in script.lines() {
            let mut w = line.split_whitespace();
            let (Some(key), Some(v)) = (w.next(), w.next().and_then(|v| v.parse::<f32>().ok())) else { continue };
            let Some(key) = key.strip_prefix("GoalRender.") else { continue };
            match key {
                "Size" => self.size = v,
                "RotSpeed" => self.rot_speed = v,
                _ => {
                    let Some((phase, field)) = key.split_once('.') else { continue };
                    let Some(k) = ["ring", "flare", "sphere"].iter().position(|p| p.eq_ignore_ascii_case(phase)) else {
                        continue;
                    };
                    let p = &mut self.phases[k];
                    match field {
                        "StartTime" => p.start = v,
                        "MidTime" => p.mid = v,
                        "EndTime" => p.end = v,
                        "FadeInTime" => p.fade_in = v,
                        "FadeOutTime" => p.fade_out = v,
                        "SizeStart" => p.size[0] = v,
                        "SizeMid" => p.size[1] = v,
                        "SizeEnd" => p.size[2] = v,
                        _ => {}
                    }
                }
            }
        }
    }

    /// The last moment any phase shows.
    fn end(&self) -> f32 {
        self.phases.iter().map(|p| p.end).fold(0.0, f32::max)
    }
}

/// A `TRAIL` block (`.trl`, `0x4b9aa0`'s fields).
#[derive(Debug, Clone)]
struct Trail {
    locator: Pattern,
    trigger: (Pattern, String),
    material: Option<Material>,
    fade_in: f32,
    fade_out: f32,
    /// `pos` (after the turn), `rot_pos` (turned), `rot_base` and `rot_vel`.
    pos: Vec3,
    rot_pos: Vec3,
    rot: [Vec3; 2],
    rot_ignore: bool,
    life: f32,
    vel_thresh: f32,
    width: [f32; 2],
    tile: f32,
}

/// A trail's point (`0x4b8100`): the middle of its two edges, half the span between them, when it was laid, and
/// the trail's length up to it.
#[derive(Debug, Clone, Copy)]
struct TrailPoint {
    centre: Vec3,
    half: Vec3,
    born: f32,
    along: f32,
}

/// A particle in flight.
#[derive(Debug, Clone)]
struct Particle {
    descriptor: usize,
    /// Where it was born and its frame (world), or its locator's actor to follow.
    origin: Vec3,
    frame: Mat3,
    follow: Option<Actor>,
    p0: Vec3,
    v: Vec3,
    a: Vec3,
    size: [f32; 3],
    rot: [f32; 3],
    life: f32,
    age: f32,
}

/// A burst geometry playing (`FLAG_BURST`).
#[derive(Debug, Clone)]
struct Burst {
    geom: usize,
    actor: Actor,
    age: f32,
}

/// The match's effects.
#[derive(Default)]
pub struct Fx {
    geoms: Vec<Geom>,
    descriptors: Vec<Descriptor>,
    trails: Vec<Trail>,
    /// The balls' sprites' materials: the Quaffle's normal and held, the Bludgers'.
    ball_materials: [Option<Material>; 3],
    /// The passing challenge's rings' material (`Q_ring_00` of the hoops' `gfx/shader.gro`) and model's triangles
    /// (the hoops' `gfx/singlehoop.fpm`).
    ring_material: Option<Material>,
    ring_mesh: Vec<RingTri>,
    /// The goal effect's tunings, and the scored hoop's centre and the effects' time of the goal while it plays.
    goal_render: GoalRender,
    goal: Option<(Vec3, f32)>,
    /// Each trail's points per actor, oldest first (in a fixed order, so that overlapping ribbons draw the same way
    /// round every frame).
    ribbons: BTreeMap<(usize, Actor), Vec<TrailPoint>>,
    particles: Vec<Particle>,
    bursts: Vec<Burst>,
    /// Particles owed per emitter (fractions carried over), and the triggers on last frame.
    owed: HashMap<(usize, Actor), f32>,
    was_on: HashMap<(usize, Actor), bool>,
    /// The folders the textures are looked for in, and the textures' keys.
    folders: Vec<String>,
    keys: HashMap<String, Option<u64>>,
    seed: u32,
    /// Seconds since the match's effects started (the stages' turns and waves).
    time: f32,
}

/// The effects' textures' keys (a range of their own).
const KEY: u64 = 1 << 61;

fn num(s: &str) -> f32 {
    // the engine's reader takes the longest number at the front (`.2.0` is 0.2)
    let s = s.trim_matches('"');
    (0..=s.len()).rev().find_map(|n| s.get(..n)?.parse::<f32>().ok()).unwrap_or(0.0)
}

fn value<'a>(b: &'a FxBlock, key: &str) -> Option<&'a FxValue> {
    b.values.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn word(b: &FxBlock, key: &str) -> Option<String> {
    match value(b, key)? {
        FxValue::One(s) => Some(s.clone()),
        FxValue::Many(v) => v.first().cloned(),
    }
}

fn float(b: &FxBlock, key: &str) -> f32 {
    word(b, key).map_or(0.0, |s| num(&s))
}

fn floats(b: &FxBlock, key: &str) -> Vec<f32> {
    match value(b, key) {
        Some(FxValue::One(s)) => vec![num(s)],
        Some(FxValue::Many(v)) => v.iter().map(|s| num(s)).collect(),
        None => vec![],
    }
}

fn vec3(b: &FxBlock, key: &str) -> Vec3 {
    let f = floats(b, key);
    Vec3::new(f.first().copied().unwrap_or(0.0), f.get(1).copied().unwrap_or(0.0), f.get(2).copied().unwrap_or(0.0))
}

fn rgba(b: &FxBlock, key: &str) -> [f32; 4] {
    let f = floats(b, key);
    std::array::from_fn(|k| f.get(k).copied().unwrap_or(1.0))
}

fn flag(b: &FxBlock, key: &str) -> bool {
    word(b, key).is_some_and(|s| num(&s) != 0.0)
}

impl Fx {
    /// The effects of the match's folders (`stadium/MatchCom/fx`, the hoops' and the stadium's), with the
    /// `.gro` materials of those folders and `stadium/Common/fx`.
    pub fn load(vfs: &Vfs, stadium: &str, hoops: &str) -> Fx {
        let folders: Vec<String> = [
            "stadium/MatchCom/fx".to_string(),
            "stadium/GameCom/fx".to_string(),
            "stadium/Common/fx".to_string(),
            format!("stadium/{hoops}/fx"),
            format!("stadium/{hoops}/gfx"),
            format!("stadium/{stadium}/fx"),
        ]
        .into();
        let files: Vec<String> = vfs
            .paths()
            .filter(|f| {
                let l = f.to_ascii_lowercase();
                folders.iter().any(|d| l.starts_with(&format!("{}/", d.to_ascii_lowercase())))
            })
            .map(str::to_string)
            .collect();
        // the Groper materials by name
        let mut gro: HashMap<String, Material> = HashMap::new();
        for f in files.iter().filter(|f| f.to_ascii_lowercase().ends_with(".gro")) {
            let Ok(bytes) = vfs.read(f) else { continue };
            let Ok(text_defs::Parsed::Gro { materials }) = text_defs::parse(text_defs::Ext::Gro, &bytes) else {
                continue;
            };
            for (name, stages) in materials {
                let stages: Vec<Stage> =
                    stages.iter().map(|s| Stage::of_gro(s)).filter(|s| !s.texture.is_empty()).collect();
                gro.insert(name.to_ascii_lowercase(), Material { stages });
            }
        }
        let mut fx = Fx { folders, seed: 0x1234_5678, ..Default::default() };
        fx.ball_materials = ["quaffle_normal", "quaffle_held", "bludger_normal"].map(|n| gro.get(n).cloned());
        fx.ring_material = gro.get("q_ring_00").cloned();
        if fx.ring_material.is_some() {
            fx.ring_mesh = ring_mesh(vfs, &format!("stadium/{hoops}/gfx/singlehoop.fpm"));
        }
        if let Ok(bytes) = vfs.read("stadium/Common/fx/goalfx.cli") {
            fx.goal_render.apply(&String::from_utf8_lossy(&bytes));
        }
        let mut behaviours: HashMap<String, Behaviour> = HashMap::new();
        let mut pending: Vec<(FxBlock, Option<Material>)> = Vec::new();
        for f in &files {
            let l = f.to_ascii_lowercase();
            let Some(ext) = text_defs::Ext::from_ext(l.rsplit('.').next().unwrap_or("")) else { continue };
            if !(l.ends_with(".got") || l.ends_with(".pot") || l.ends_with(".trl")) {
                continue;
            }
            let Ok(bytes) = vfs.read(f) else { continue };
            let Ok(text_defs::Parsed::Fx { blocks, .. }) = text_defs::parse(ext, &bytes) else { continue };
            // a file's own materials: a texture (a Groper material's name, or a texture's), a blend and a depth write
            let mut mats: HashMap<String, Material> = HashMap::new();
            for b in blocks.iter().filter(|b| b.kind == "MATERIAL") {
                let tex = word(b, "texture").unwrap_or_default();
                let m = match gro.get(&tex.to_ascii_lowercase()) {
                    Some(g) => g.clone(),
                    None => Material {
                        stages: vec![Stage::plain(
                            tex,
                            word(b, "blend_mode").and_then(|s| blend_of(&s)).unwrap_or(SRCALPHA),
                            word(b, "z_write_enable").is_some_and(|s| num(&s) != 0.0),
                        )],
                    },
                };
                mats.insert(b.name.to_ascii_lowercase(), m);
            }
            let mat = |b: &FxBlock| word(b, "material").and_then(|m| mats.get(&m.to_ascii_lowercase()).cloned());
            for b in &blocks {
                match b.kind.as_str() {
                    "GEOM" if word(b, "shape").is_none_or(|s| !s.eq_ignore_ascii_case("RECT_2D")) => {
                        let globe = word(b, "shape")
                            .is_some_and(|s| s.eq_ignore_ascii_case("SPHERE") || s.eq_ignore_ascii_case("ELLIPSOID"));
                        let shape =
                            if globe { Shape::Globe((float(b, "tess") as u32).clamp(4, 32)) } else { Shape::Rect };
                        let tri = |k: &str| {
                            [
                                float(b, &format!("{k}_start")),
                                float(b, &format!("{k}_mid")),
                                float(b, &format!("{k}_end")),
                            ]
                        };
                        fx.geoms.push(Geom {
                            shape,
                            locator: Pattern::parse(&word(b, "locator_parent").unwrap_or_default()),
                            trigger: (
                                Pattern::parse(&word(b, "trigger_parent").unwrap_or_default()),
                                word(b, "trigger_node").unwrap_or_default().to_ascii_uppercase(),
                            ),
                            material: mat(b),
                            burst: flag(b, "flag_burst"),
                            camera_face: flag(b, "flag_camera_face"),
                            rot_ignore: flag(b, "flag_rot_ignore"),
                            colour: [rgba(b, "colour_start"), rgba(b, "colour_mid"), rgba(b, "colour_end")],
                            width: tri("width"),
                            height: tri("height"),
                            pos: vec3(b, "pos_base"),
                            rot: vec3(b, "rot_base"),
                            life: float(b, "time_life"),
                            mid: float(b, "time_mid"),
                            uv: [float(b, "u0"), float(b, "u1"), float(b, "v0"), float(b, "v1")],
                        });
                    }
                    "BEHAVIOUR" => {
                        let two = |k: &str| [float(b, &format!("{k}_base")), float(b, &format!("{k}_variance"))];
                        let v3 = |k: &str| [vec3(b, &format!("{k}_base")), vec3(b, &format!("{k}_variance"))];
                        behaviours.insert(
                            b.name.to_ascii_lowercase(),
                            Behaviour {
                                pos: v3("pos"),
                                vel: v3("vel"),
                                acc: v3("acc"),
                                ring: vec3(b, "ring_radius"),
                                vel_thresh: float(b, "vel_thresh"),
                                fade_in: float(b, "alpha_fadein"),
                                fade_out: float(b, "alpha_fadeout"),
                                size: two("size"),
                                size_vel: two("size_vel"),
                                size_acc: two("size_acc"),
                                size_clamp: [float(b, "size_clamp_min"), float(b, "size_clamp_max")],
                                rate: two("emission_rate"),
                                rate_clamp: [float(b, "emission_rate_clamp_min"), float(b, "emission_rate_clamp_max")],
                                life: two("life"),
                                life_clamp: [float(b, "life_clamp_min"), float(b, "life_clamp_max")],
                                rot: [float(b, "rot_base"), float(b, "rot_variance")],
                                rot_vel: two("rot_vel"),
                                rot_acc: two("rot_acc"),
                                rotate: flag(b, "do_rotate"),
                                radial: flag(b, "do_radial"),
                            },
                        );
                    }
                    // its material by name among the file's; its behaviour may come from another file
                    "DESCRIPTOR" => pending.push((b.clone(), mat(b))),
                    "TRAIL"
                        if std::env::var_os("QWC_FX_DEBUG").is_some() && {
                            eprintln!(
                                "effects: {f}: trail {} on {:?}, trigger {:?}.{:?}, material {:?}",
                                b.name,
                                word(b, "locator_parent"),
                                word(b, "trigger_parent"),
                                word(b, "trigger_node"),
                                word(b, "material")
                            );
                            false
                        } => {}
                    "TRAIL" => fx.trails.push(Trail {
                        locator: Pattern::parse(&word(b, "locator_parent").unwrap_or_default()),
                        trigger: (
                            Pattern::parse(&word(b, "trigger_parent").unwrap_or_default()),
                            word(b, "trigger_node").unwrap_or_default().to_ascii_uppercase(),
                        ),
                        material: mat(b),
                        fade_in: float(b, "alpha_fadein"),
                        fade_out: float(b, "alpha_fadeout"),
                        pos: vec3(b, "pos"),
                        rot_pos: vec3(b, "rot_pos"),
                        rot: [vec3(b, "rot_base"), vec3(b, "rot_vel")],
                        rot_ignore: flag(b, "flag_rot_ignore"),
                        life: float(b, "life"),
                        vel_thresh: float(b, "vel_thresh"),
                        width: [float(b, "width_start"), float(b, "width_end")],
                        tile: float(b, "tile"),
                    }),
                    _ => {}
                }
            }
        }
        for (b, material) in pending {
            let Some(behaviour) = word(&b, "behaviour").and_then(|n| behaviours.get(&n.to_ascii_lowercase()).cloned())
            else {
                continue;
            };
            let c = rgba(&b, "rgba");
            fx.descriptors.push(Descriptor {
                behaviour,
                material,
                locator: Pattern::parse(&word(&b, "locator_parent").unwrap_or_default()),
                trigger: (
                    Pattern::parse(&word(&b, "trigger_parent").unwrap_or_default()),
                    word(&b, "trigger_node").unwrap_or_default().trim_matches('"').to_ascii_uppercase(),
                ),
                colour: c.map(|v| v / 128.0),
                placed: flag(&b, "dynamic_placement_flag"),
            });
        }
        fx
    }

    fn rand(&mut self) -> f32 {
        // xorshift: the effects' own randomness (visual only)
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        (self.seed >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Plus or minus `v` at random.
    fn vary(&mut self, v: f32) -> f32 {
        v * (self.rand() * 2.0 - 1.0)
    }

    fn vary3(&mut self, b: Vec3, v: Vec3) -> Vec3 {
        Vec3::new(b.x + self.vary(v.x), b.y + self.vary(v.y), b.z + self.vary(v.z))
    }

    /// Whether `node` is on for `actor`'s instance of a definition whose trigger parent is `parent`.
    fn on(parent: &Pattern, node: &str, actor: &ActorState, all: &[ActorState]) -> bool {
        match node {
            "ALWAYS_ON" => return true,
            "NONE" | "NEVER" | "" => return false,
            _ => {}
        }
        let owner = if *parent == Pattern::World { all.iter().find(|a| a.actor == Actor::World) } else { Some(actor) };
        owner.is_some_and(|o| o.triggers.contains(&node))
    }

    /// Start the goal effect at the scored hoop's centre `at` (the message on `0x1043b60`).
    pub fn goal(&mut self, at: Vec3) {
        self.goal = Some((at, self.time));
    }

    /// The effects `dt` seconds on: bursts started and aged, particles emitted, moved and let go.
    pub fn update(&mut self, dt: f32, actors: &[ActorState]) {
        let dt = dt.clamp(0.0, 0.25);
        self.time += dt;
        if self.goal.is_some_and(|(_, t0)| self.time - t0 > self.goal_render.end()) {
            self.goal = None;
        }
        // bursts start as their trigger comes on
        for g in 0..self.geoms.len() {
            if !self.geoms[g].burst {
                continue;
            }
            for a in actors.iter().filter(|a| self.geoms[g].locator.matches(a)) {
                let (p, n) = &self.geoms[g].trigger;
                let on = Fx::on(p, n, a, actors);
                let was = self.was_on.insert((usize::MAX - g, a.actor), on).unwrap_or(false);
                if on && !was {
                    self.bursts.push(Burst { geom: g, actor: a.actor, age: 0.0 });
                }
            }
        }
        for b in &mut self.bursts {
            b.age += dt;
        }
        let geoms = &self.geoms;
        self.bursts.retain(|b| b.age < geoms[b.geom].life.max(1e-3));
        // the particles
        for p in &mut self.particles {
            p.age += dt;
        }
        self.particles.retain(|p| p.age < p.life);
        for d in 0..self.descriptors.len() {
            let locator = self.descriptors[d].locator.clone();
            for a in actors.iter().filter(|a| locator.matches(a)) {
                let (pt, n) = self.descriptors[d].trigger.clone();
                if !Fx::on(&pt, &n, a, actors) {
                    self.owed.remove(&(d, a.actor));
                    continue;
                }
                let beh = self.descriptors[d].behaviour.clone();
                if a.vel.length() < beh.vel_thresh {
                    continue;
                }
                let mut rate = beh.rate[0] + self.vary(beh.rate[1]);
                if beh.rate_clamp[1] > 0.0 {
                    rate = rate.min(beh.rate_clamp[1]);
                }
                rate = rate.max(beh.rate_clamp[0]).max(0.0);
                let owed = self.owed.entry((d, a.actor)).or_insert(0.0);
                *owed += rate * dt;
                let n = owed.floor() as usize;
                *owed -= n as f32;
                for _ in 0..n.min(64) {
                    self.emit(d, a, &beh);
                }
            }
        }
        // the trails: a point a frame while on and moving, each kept for its life
        let now = self.time;
        for (k, tr) in self.trails.iter().enumerate() {
            for a in actors.iter().filter(|a| tr.locator.matches(a)) {
                let on = Fx::on(&tr.trigger.0, &tr.trigger.1, a, actors) && a.vel.length() >= tr.vel_thresh;
                if !on {
                    continue;
                }
                let r = tr.rot[0] + tr.rot[1] * now;
                let turn = Mat3::from_rotation_x(r.x) * Mat3::from_rotation_y(r.y) * Mat3::from_rotation_z(r.z);
                let frame = if tr.rot_ignore { Mat3::IDENTITY } else { Fx::frame_of(a) };
                let across = Vec3::X * tr.width[0];
                let left = a.pos + frame * (turn * (tr.rot_pos - across) + tr.pos);
                let right = a.pos + frame * (turn * (tr.rot_pos + across) + tr.pos);
                let points = self.ribbons.entry((k, a.actor)).or_default();
                let centre = (left + right) * 0.5;
                let along = points.last().map_or(0.0, |p| p.along + p.centre.distance(centre));
                points.push(TrailPoint { centre, half: (right - left) * 0.5, born: now, along });
            }
        }
        let trails = &self.trails;
        for ((k, _), points) in &mut self.ribbons {
            let life = trails[*k].life.max(0.01);
            points.retain(|p| now - p.born < life);
        }
        self.ribbons.retain(|_, p| !p.is_empty());
        if self.particles.len() > 4000 {
            let extra = self.particles.len() - 4000;
            self.particles.drain(..extra);
        }
    }

    /// An actor's frame: its forward as z, the world's up as y.
    fn frame_of(a: &ActorState) -> Mat3 {
        let z = Vec3::new(a.fwd.x, 0.0, a.fwd.z).try_normalize().unwrap_or(Vec3::Z);
        let x = Vec3::Y.cross(z).try_normalize().unwrap_or(Vec3::X);
        Mat3::from_cols(x, Vec3::Y, z)
    }

    fn emit(&mut self, d: usize, a: &ActorState, beh: &Behaviour) {
        let mut p0 = self.vary3(beh.pos[0], beh.pos[1]);
        let mut v = self.vary3(beh.vel[0], beh.vel[1]);
        let acc = self.vary3(beh.acc[0], beh.acc[1]);
        if beh.radial {
            // on a ring about the locator's forward, moving out from it
            let t = self.rand() * std::f32::consts::TAU;
            let (s, c) = t.sin_cos();
            p0 += Vec3::new(c * beh.ring.x, s * beh.ring.y, 0.0);
            v = Vec3::new(c * v.x, s * v.y, v.z);
        }
        let mut life = beh.life[0] + self.vary(beh.life[1]);
        if beh.life_clamp[1] > 0.0 {
            life = life.min(beh.life_clamp[1]);
        }
        life = life.max(beh.life_clamp[0]).max(0.05);
        let size = [
            beh.size[0] + self.vary(beh.size[1]),
            beh.size_vel[0] + self.vary(beh.size_vel[1]),
            beh.size_acc[0] + self.vary(beh.size_acc[1]),
        ];
        let rot = [
            beh.rot[0] + self.vary(beh.rot[1]),
            beh.rot_vel[0] + self.vary(beh.rot_vel[1]),
            beh.rot_acc[0] + self.vary(beh.rot_acc[1]),
        ];
        let placed = self.descriptors[d].placed;
        self.particles.push(Particle {
            descriptor: d,
            origin: a.pos,
            frame: Fx::frame_of(a),
            follow: (!placed).then_some(a.actor),
            p0,
            v,
            a: acc,
            size,
            rot,
            life,
            age: 0.0,
        });
    }

    /// The texture key of `name`, uploaded on first use (none when no folder has it).
    fn key(&mut self, gpu: &Gpu, effects: &mut Effects, vfs: &Vfs, name: &str) -> Option<u64> {
        let next = KEY + self.keys.len() as u64;
        let folders = self.folders.clone();
        *self.keys.entry(name.to_ascii_lowercase()).or_insert_with(|| {
            let path = folders.iter().map(|d| format!("{d}/{name}.fsh")).find(|p| vfs.exists(p))?;
            match qwc_assets::texture::load(vfs, &path) {
                Ok(t) => {
                    effects.upload(gpu, next, t.width, t.height, &t.levels);
                    Some(next)
                }
                Err(e) => {
                    eprintln!("{path}: {e}");
                    None
                }
            }
        })
    }

    /// Upload every texture the definitions' materials name.
    pub fn preload(&mut self, gpu: &Gpu, effects: &mut Effects, vfs: &Vfs) {
        let names: Vec<String> = (self.geoms.iter().filter_map(|g| g.material.as_ref()))
            .chain(self.descriptors.iter().filter_map(|d| d.material.as_ref()))
            .chain(self.trails.iter().filter_map(|t| t.material.as_ref()))
            .chain(self.ball_materials.iter().flatten())
            .chain(self.ring_material.iter())
            .flat_map(|m| m.stages.iter().map(|s| s.texture.clone()))
            .chain(["Goal_ring", "Goal_flare", "Goal_plasma"].map(String::from))
            .collect();
        for n in names {
            if self.key(gpu, effects, vfs, &n).is_none() && std::env::var_os("QWC_FX_DEBUG").is_some() {
                eprintln!("effects: no texture {n:?}");
            }
        }
    }

    fn lookup(&self, name: &str) -> Option<u64> {
        self.keys.get(&name.to_ascii_lowercase()).copied().flatten()
    }

    /// The effects' quads this frame, seen by `camera` (their textures [`Fx::preload`]ed).
    pub fn quads(&mut self, camera: &Camera, actors: &[ActorState]) -> Vec<EffectQuad> {
        let mut out = Vec::new();
        let view = (camera.target - camera.eye).try_normalize().unwrap_or(Vec3::Z);
        let right = view.cross(camera.up).try_normalize().unwrap_or(Vec3::X);
        let up = right.cross(view).try_normalize().unwrap_or(Vec3::Y);
        // the geometry: the lasting ones while their trigger is on, the bursts as they play
        let mut shown: Vec<(usize, &ActorState, f32)> = Vec::new();
        for (g, geom) in self.geoms.iter().enumerate() {
            if geom.burst {
                continue;
            }
            for a in actors.iter().filter(|a| geom.locator.matches(a)) {
                if Fx::on(&geom.trigger.0, &geom.trigger.1, a, actors) {
                    shown.push((g, a, 0.0));
                }
            }
        }
        for b in &self.bursts {
            if let Some(a) = actors.iter().find(|a| a.actor == b.actor) {
                shown.push((b.geom, a, b.age));
            }
        }
        let shown: Vec<(usize, ActorState, f32)> = shown.into_iter().map(|(g, a, t)| (g, a.clone(), t)).collect();
        for (g, a, t) in shown {
            let geom = self.geoms[g].clone();
            let Some(m) = &geom.material else { continue };
            // start to mid over the mid time, mid to end over the rest of the life (`0x4bce10`)
            let (i, f) = if !geom.burst {
                (0, 0.0)
            } else if t < geom.mid {
                (0, t / geom.mid.max(1e-3))
            } else {
                (1, (t - geom.mid) / (geom.life - geom.mid).max(1e-3))
            };
            let mix = |v: [f32; 3]| v[i] + (v[i + 1] - v[i]) * f;
            let (w, h) = (mix(geom.width), mix(geom.height));
            let colour: [f32; 4] =
                std::array::from_fn(|k| geom.colour[i][k] + (geom.colour[i + 1][k] - geom.colour[i][k]) * f);
            let frame = if geom.rot_ignore { Mat3::IDENTITY } else { Fx::frame_of(&a) };
            let rot = Mat3::from_rotation_x(geom.rot.x)
                * Mat3::from_rotation_y(geom.rot.y)
                * Mat3::from_rotation_z(geom.rot.z);
            let centre = a.pos + frame * geom.pos;
            if let Shape::Globe(n) = geom.shape {
                // segments round its axis (the frame's y after the turn), half as many from pole to pole
                let axes = frame * rot;
                let half = Vec3::new(w, h, w);
                let rings = (n / 2).max(2);
                let point = |i: u32, j: u32| {
                    let lon = i as f32 / n as f32 * std::f32::consts::TAU;
                    let lat = j as f32 / rings as f32 * std::f32::consts::PI;
                    let p = Vec3::new(lat.sin() * lon.cos(), lat.cos(), lat.sin() * lon.sin()) * half;
                    (centre + axes * p, i as f32 / n as f32, j as f32 / rings as f32)
                };
                for i in 0..n {
                    for j in 0..rings {
                        let corners = [point(i, j + 1), point(i + 1, j + 1), point(i, j), point(i + 1, j)];
                        self.push_stages(&mut out, m, corners, [colour; 4]);
                    }
                }
                continue;
            }
            let (ax, ay) = if geom.camera_face { (right, up) } else { (frame * rot * Vec3::X, frame * rot * Vec3::Y) };
            let (hx, hy) = (ax * w, ay * h);
            let uv = if geom.uv == [0.0; 4] { [0.0, 1.0, 0.0, 1.0] } else { geom.uv };
            let corners = [
                (centre - hx - hy, uv[0], uv[3]),
                (centre + hx - hy, uv[1], uv[3]),
                (centre - hx + hy, uv[0], uv[2]),
                (centre + hx + hy, uv[1], uv[2]),
            ];
            self.push_stages(&mut out, m, corners, [colour; 4]);
        }
        // the balls' sprites (`0x494120`, `0x4940e0`, drawn by `0x493b90`)
        for a in actors {
            let (m, half, colour) = match a.actor {
                Actor::Quaffle => {
                    let g = a.glow.clamp(0.0, 1.0);
                    (&self.ball_materials[usize::from(a.held)], 0.65 + (2.0 - 0.65) * g, [1.0, 1.0 - g, 1.0 - g, 1.0])
                }
                Actor::Bludger(_) => (&self.ball_materials[2], 0.65, [1.0; 4]),
                _ => continue,
            };
            let Some(m) = m.clone() else { continue };
            let (hx, hy) = (right * half, up * half);
            let corners = [
                (a.pos - hx - hy, 0.0, 1.0),
                (a.pos + hx - hy, 1.0, 1.0),
                (a.pos - hx + hy, 0.0, 0.0),
                (a.pos + hx + hy, 1.0, 0.0),
            ];
            self.push_stages(&mut out, &m, corners, [colour; 4]);
        }
        // the goal effect: the ring and the flare in the hoop's plane (the world's y z), the sphere round its centre
        if let Some((at, t0)) = self.goal {
            let g = self.goal_render;
            let t = self.time - t0;
            let names = ["Goal_ring", "Goal_flare", "Goal_plasma"];
            for (k, phase) in g.phases.iter().enumerate() {
                let Some((alpha, size)) = phase.at(t) else { continue };
                let s = size * g.size;
                let m = Material { stages: vec![Stage::plain(names[k].into(), SRCALPHA_ADD, true)] };
                let colour = [1.0, 1.0, 1.0, alpha];
                if k < 2 {
                    let c = |y: f32, z: f32, u: f32, v: f32| (at + Vec3::new(0.0, y * s, z * s), u, v);
                    let corners = [
                        c(-1.0, -1.0, 0.0, 1.0),
                        c(-1.0, 1.0, 1.0, 1.0),
                        c(1.0, -1.0, 0.0, 0.0),
                        c(1.0, 1.0, 1.0, 0.0),
                    ];
                    self.push_stages(&mut out, &m, corners, [colour; 4]);
                    continue;
                }
                // 16 segments round, 8 from pole to pole
                let x = (t - phase.start) / (phase.end - phase.start);
                let turn = -x * g.rot_speed;
                let point = |i: usize, j: usize| {
                    let (lon, lat) =
                        (i as f32 / 16.0 * std::f32::consts::TAU + turn, j as f32 / 8.0 * std::f32::consts::PI);
                    let p = Vec3::new(lat.sin() * lon.cos(), lat.cos(), lat.sin() * lon.sin());
                    (at + p * s, i as f32 / 16.0, j as f32 / 8.0)
                };
                for i in 0..16 {
                    for j in 0..8 {
                        let corners = [point(i, j + 1), point(i + 1, j + 1), point(i, j), point(i + 1, j)];
                        self.push_stages(&mut out, &m, corners, [colour; 4]);
                    }
                }
            }
        }
        // the trails' ribbons, a quad between each point and the next
        for ((k, _), points) in &self.ribbons {
            let tr = &self.trails[*k];
            let Some(m) = &tr.material else { continue };
            let life = tr.life.max(0.01);
            let edge = |p: &TrailPoint| {
                let x = ((self.time - p.born) / life).clamp(0.0, 1.0);
                let w = tr.width[0] + (tr.width[1] - tr.width[0]) * x;
                let s = if tr.width[0] > 0.0 { w / tr.width[0] } else { 0.0 };
                let mut alpha = 1.0f32;
                if tr.fade_in > 0.0 && x < tr.fade_in {
                    alpha = x / tr.fade_in;
                }
                if tr.fade_out > 0.0 && x > 1.0 - tr.fade_out {
                    alpha = alpha.min((1.0 - x) / tr.fade_out);
                }
                // the texture across the ribbon, and along it once every `1 / tile` of its length
                let v = if tr.tile > 0.0 { p.along * tr.tile } else { x };
                (p.centre - p.half * s, p.centre + p.half * s, v, [1.0, 1.0, 1.0, alpha])
            };
            for pair in points.windows(2) {
                let (a0, a1, va, ca) = edge(&pair[0]);
                let (b0, b1, vb, cb) = edge(&pair[1]);
                let corners = [(a0, 0.0, va), (a1, 1.0, va), (b0, 0.0, vb), (b1, 1.0, vb)];
                self.push_stages(&mut out, m, corners, [ca, ca, cb, cb]);
            }
        }
        // the particles: camera-facing squares of their size, turned by their rotation
        let particles = std::mem::take(&mut self.particles);
        for p in &particles {
            let d = &self.descriptors[p.descriptor];
            let Some(m) = d.material.clone() else { continue };
            let colour_base = d.colour;
            let (fade_in, fade_out, rotate) = (d.behaviour.fade_in, d.behaviour.fade_out, d.behaviour.rotate);
            let size_clamp = d.behaviour.size_clamp;
            let t = p.age;
            let (origin, frame) = match p.follow.and_then(|f| actors.iter().find(|a| a.actor == f)) {
                Some(a) => (a.pos, Fx::frame_of(a)),
                None => (p.origin, p.frame),
            };
            let pos = origin + frame * (p.p0 + p.v * t + p.a * t * t);
            let mut size = p.size[0] + p.size[1] * t + p.size[2] * t * t;
            if size_clamp[1] > 0.0 {
                size = size.min(size_clamp[1]);
            }
            size = size.max(size_clamp[0]).max(0.0);
            let x = t / p.life;
            let mut alpha = 1.0f32;
            if fade_in > 0.0 && x < fade_in {
                alpha = x / fade_in;
            }
            if fade_out > 0.0 && x > 1.0 - fade_out {
                alpha = alpha.min((1.0 - x) / fade_out);
            }
            let angle = if rotate { (p.rot[0] + p.rot[1] * t + p.rot[2] * t * t).to_radians() } else { 0.0 };
            let (s, c) = angle.sin_cos();
            let (rx, ry) = (right * c + up * s, up * c - right * s);
            let (hx, hy) = (rx * size * 0.5, ry * size * 0.5);
            let colour = [colour_base[0], colour_base[1], colour_base[2], colour_base[3] * alpha];
            let corners = [
                (pos - hx - hy, 0.0, 1.0),
                (pos + hx - hy, 1.0, 1.0),
                (pos - hx + hy, 0.0, 0.0),
                (pos + hx + hy, 1.0, 0.0),
            ];
            self.push_stages(&mut out, &m, corners, [colour; 4]);
        }
        self.particles = particles;
        out
    }

    /// A quad of `corners` (position and texture coordinates) in their `colours`, once per stage of `m`.
    fn push_stages(
        &self,
        out: &mut Vec<EffectQuad>,
        m: &Material,
        corners: [(Vec3, f32, f32); 4],
        colours: [[f32; 4]; 4],
    ) {
        let t = self.time;
        for st in &m.stages {
            let Some(key) = self.lookup(&st.texture) else { continue };
            let a = st.alpha.map_or(1.0, |w| w.at(t).clamp(0.0, 1.0));
            let v = |((p, u, vv), c): ((Vec3, f32, f32), [f32; 4])| EffectVertex {
                pos: p.to_array(),
                uv: st.uv(u, vv, t),
                colour: [c[0] * st.rgb[0], c[1] * st.rgb[1], c[2] * st.rgb[2], c[3] * a],
            };
            let [c0, c1, c2, c3] = corners;
            let [k0, k1, k2, k3] = colours;
            out.push(EffectQuad {
                texture: key,
                blend: st.blend,
                depth_write: st.depth_write,
                wrap: st.wrap,
                corners: [v((c0, k0)), v((c1, k1)), v((c2, k2)), v((c3, k3))],
            });
        }
    }

    /// One of the passing challenge's rings (`0x45cf34` posts each to the hoop render module `0x10439d4`, which
    /// draws it as the challenge hoops' model, `0x4c0540`): `stadium/hoopchal/gfx/singlehoop.fpm`, a thick ring (its
    /// hole 1.51 across and 0.63 deep along the model's `x`, its faces out to 2.0-2.1), `size` times, at `pos`,
    /// tinted `colour` (1 the texture as it is), in its `Q_ring_00` material (three scrolling layers of `q_ring_00`,
    /// the last two added). **[T]** Its matrix (`0x45c1a0`) has the rows the game camera's direction, its up and
    /// their cross (team 0's `+0xfc` and `+0x11c`, the camera channel `[0x1043a04]`'s direction and up as `0x442460`
    /// keeps them): the ring's hole lies along the view, so one off the middle of the picture shows its inside. Its
    /// back faces are not drawn (the model is one-sided); nothing without the material or the model.
    pub fn ring(&self, out: &mut Vec<EffectQuad>, camera: &Camera, pos: Vec3, size: f32, colour: [f32; 4]) {
        let Some(m) = &self.ring_material else { return };
        if size <= 0.0 || self.ring_mesh.is_empty() {
            return;
        }
        let look = (camera.target - camera.eye).try_normalize().unwrap_or(Vec3::Z);
        let up = (camera.up - look * camera.up.dot(look)).try_normalize().unwrap_or(Vec3::Y);
        let across = look.cross(up);
        let turn = Mat3::from_cols(look, up, across);
        for t in &self.ring_mesh {
            let p = t.p.map(|v| pos + turn * v * size);
            let n = t.n.map(|v| turn * v);
            let centre = (p[0] + p[1] + p[2]) / 3.0;
            if t.cull && (n[0] + n[1] + n[2]).dot(camera.eye - centre) < 0.0 {
                continue;
            }
            let c = |k: usize| (p[k], t.uv[k][0], t.uv[k][1]);
            self.push_stages(out, m, [c(0), c(1), c(2), c(2)], [colour; 4]);
        }
    }

    /// The definitions loaded: geometry, particle emitters and trails.
    pub fn counts(&self) -> (usize, usize, usize) {
        (self.geoms.len(), self.descriptors.len(), self.trails.len())
    }
}

/// One triangle of the rings' model: its corners, their normals and texture coordinates, and whether its back is
/// left undrawn.
#[derive(Debug, Clone, Copy)]
struct RingTri {
    p: [Vec3; 3],
    n: [Vec3; 3],
    uv: [[f32; 2]; 3],
    cull: bool,
}

/// The triangles of the rings' model at `path`, placed by its root matrix (none when it does not load).
fn ring_mesh(vfs: &Vfs, path: &str) -> Vec<RingTri> {
    let textures = qwc_assets::TextureIndex::default();
    let model = match qwc_assets::model::load(vfs, &textures, path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("the challenge's rings: {e}");
            return Vec::new();
        }
    };
    // the root matrix is row-vector: its rows are glam's columns
    let root = qwc_render::glam::Mat4::from_cols_array(&model.matrix);
    let mut out = Vec::new();
    for mesh in model.meshes.iter().filter(|m| m.level == 0) {
        let at = |i: u32| Vec3::from(mesh.positions[i as usize]);
        let normal = |i: u32| mesh.normals.as_ref().map_or(Vec3::ZERO, |n| Vec3::from(n[i as usize]));
        for &k in mesh.indices.as_chunks::<3>().0 {
            let p = k.map(|i| root.transform_point3(at(i)));
            // without normals, the face's by its counter-clockwise winding
            let face = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero();
            let n = k.map(|i| root.transform_vector3(normal(i)).try_normalize().unwrap_or(face));
            let uv = k.map(|i| mesh.uvs.get(i as usize).copied().unwrap_or([0.0; 2]));
            out.push(RingTri { p, n, uv, cull: !mesh.two_sided });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(team: usize, role: &'static str) -> ActorState {
        ActorState {
            actor: Actor::Player(0),
            team,
            role,
            pos: Vec3::ZERO,
            vel: Vec3::ZERO,
            fwd: Vec3::Z,
            triggers: vec![],
            glow: 0.0,
            held: false,
        }
    }

    #[test]
    fn locator_patterns_name_teams_roles_and_exclusions() {
        let all_but_keeper = Pattern::parse("ACTOR.TEAM0!KEEPER");
        assert!(all_but_keeper.matches(&player(0, "CHASER1")));
        assert!(!all_but_keeper.matches(&player(0, "KEEPER")));
        assert!(!all_but_keeper.matches(&player(1, "CHASER1")));
        let keeper = Pattern::parse("ACTOR.TEAM0.KEEPER");
        assert!(keeper.matches(&player(0, "KEEPER")) && !keeper.matches(&player(0, "SEEKER")));
        assert!(Pattern::parse("ACTOR").matches(&player(1, "BEATER2")));
        assert_eq!(Pattern::parse("GLOBAL"), Pattern::World);
    }

    #[test]
    fn the_goal_phases_grow_and_fade_over_their_span() {
        let mut g = GoalRender::default();
        g.apply(
            "GoalRender.Size 2
GoalRender.ring.EndTime 1
GoalRender.flare.SizeEnd 4
Other.Key 1
",
        );
        assert_eq!((g.size, g.phases[0].end, g.phases[1].size[2]), (2.0, 1.0, 4.0));
        let ring = g.phases[0];
        // fading in over the first 0.4 of the span, growing from 0.4 to 1 by 0.3 and on to 1.6
        let (a, s) = ring.at(0.2).unwrap();
        assert!((a - 0.5).abs() < 1e-5 && (s - (0.4 + 0.6 * 0.2 / 0.3)).abs() < 1e-5, "{a} {s}");
        let (a, s) = ring.at(0.9).unwrap();
        assert!((a - 0.5).abs() < 1e-5 && (s - (1.0 + 0.6 * 0.6 / 0.7)).abs() < 1e-5, "{a} {s}");
        assert_eq!(ring.at(1.0), None);
        assert_eq!(ring.at(-0.1), None);
    }

    #[test]
    fn numbers_read_as_the_engine_does() {
        assert_eq!(num(".2.0"), 0.2);
        assert_eq!(num("-.5"), -0.5);
        assert_eq!(num("\"x\""), 0.0);
        assert_eq!(blend_of("SRCALPHA_ADD"), Some(7));
    }
}
