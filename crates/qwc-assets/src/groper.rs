//! A Groper material's animated stage (`.gro`, `RE_NOTES.md` "Groper materials"): the frames an `animmap` steps
//! through and the texture-coordinate modifiers (`tcMod`) that move them, evaluated at a time in seconds.
//!
//! **[T]** The keywords and their numbers (the tables at `0x5fc200`-`0x5fc7a0`: tcMod `TRANSFORM`, `TURB`, `SCROLL`,
//! `SCALE`, `STRETCH`, `STRETCH_U`, `STRETCH_V`, `ROTATE`, `ENTITY_TRANSLATE`, `WAVEROT`; waves `SIN`, `SQUARE`,
//! `TRIANGLE`, `SAWTOOTH`, `INVERSE_SAWTOOTH`, `NOISE`, `TURB`; turb axes `U`, `V`, `UV`), and the evaluator
//! `0x4a9770`: a texture matrix from the identity, each of the stage's tcMods (`+0x34` of them, `0x58` bytes each)
//! multiplied in order by its type's function (`0x4a98b0` scroll, `0x4a9a40` scale, `0x4a9ae0` stretch, `0x4a9d40`
//! rotate, `0x4a9cc0` turb, ...) at the engine's clock (`[0x64cbf0]`).
//!
//! **[I]** What each type does is read as Quake 3's shaders do, which the language follows: scroll moves by
//! `speed * t` (wrapped), rotate turns `-degrees * t` about the middle, scale multiplies, stretch scales about the
//! middle by `1 / wave(t)`, turb shifts the axes it names by `wave(t)`; `noise` is a smooth noise of the time;
//! `animmap FPS frames...` shows frame `floor(t * FPS) mod n`.

/// A Groper wave: its function and `base`, `amplitude`, `phase`, `frequency`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wave {
    pub func: Func,
    pub base: f32,
    pub amp: f32,
    pub phase: f32,
    pub freq: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Func {
    Sin,
    Square,
    Triangle,
    Sawtooth,
    InverseSawtooth,
    Noise,
}

impl Wave {
    /// `FUNC base amplitude phase frequency`.
    pub fn parse(w: &[&str]) -> Option<Wave> {
        let func = match w.first()?.to_ascii_lowercase().as_str() {
            "sin" => Func::Sin,
            "square" => Func::Square,
            "triangle" => Func::Triangle,
            "sawtooth" | "saw" => Func::Sawtooth,
            "inversesawtooth" | "inverse_sawtooth" | "saw_inv" => Func::InverseSawtooth,
            "noise" | "turb" => Func::Noise,
            _ => return None,
        };
        let n = |i: usize| w.get(i).and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
        Some(Wave { func, base: n(1), amp: n(2), phase: n(3), freq: n(4) })
    }

    pub fn at(&self, t: f32) -> f32 {
        let x = (self.phase + self.freq * t).rem_euclid(1.0);
        let f = match self.func {
            Func::Sin => (x * std::f32::consts::TAU).sin(),
            Func::Triangle => {
                if x < 0.25 {
                    4.0 * x
                } else if x < 0.75 {
                    2.0 - 4.0 * x
                } else {
                    4.0 * x - 4.0
                }
            }
            Func::Square => {
                if x < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            Func::Sawtooth => x,
            Func::InverseSawtooth => 1.0 - x,
            Func::Noise => {
                // a smooth value noise of the phase and the time
                let s = self.phase * 7.0 + self.freq * t;
                let (i, f) = (s.floor(), s - s.floor());
                let h = |k: f32| ((k * 12.9898).sin() * 43758.547).fract() * 2.0 - 1.0;
                let e = f * f * (3.0 - 2.0 * f);
                h(i) + (h(i + 1.0) - h(i)) * e
            }
        };
        self.base + self.amp * f
    }
}

/// One texture-coordinate modifier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TcMod {
    Scroll { u: f32, v: f32 },
    Scale { u: f32, v: f32 },
    Rotate { degrees: f32 },
    Stretch { wave: Wave, u: bool, v: bool },
    Turb { wave: Wave, u: bool, v: bool },
}

/// A material's animated stage: the frames (texture names) at `fps`, and its modifiers in order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Stage {
    pub frames: Vec<String>,
    pub fps: f32,
    pub tcmods: Vec<TcMod>,
}

/// A 2 x 3 texture matrix: `u' = m[0] . (u, v, 1)`, `v' = m[1] . (u, v, 1)`.
pub type UvMatrix = [[f32; 3]; 2];

const IDENTITY: UvMatrix = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];

/// `a` then `b`.
fn then(a: UvMatrix, b: UvMatrix) -> UvMatrix {
    let row = |r: [f32; 3]| {
        [r[0] * a[0][0] + r[1] * a[1][0], r[0] * a[0][1] + r[1] * a[1][1], r[0] * a[0][2] + r[1] * a[1][2] + r[2]]
    };
    [row(b[0]), row(b[1])]
}

/// Scaling by `(su, sv)` about the middle of the texture.
fn about_middle(su: f32, sv: f32) -> UvMatrix {
    [[su, 0.0, 0.5 - 0.5 * su], [0.0, sv, 0.5 - 0.5 * sv]]
}

impl Stage {
    /// The first stage of `stages` (each a list of directives) that names a texture, when it animates (an
    /// `animmap` of more than one frame, or any `tcMod`); `None` for a still one.
    pub fn of(stages: &[Vec<String>]) -> Option<Stage> {
        for stage in stages {
            let mut s = Stage::default();
            for d in stage {
                let w: Vec<&str> = d.split_whitespace().filter(|w| *w != "(" && *w != ")").collect();
                let Some(k) = w.first().map(|k| k.to_ascii_lowercase()) else { continue };
                let n = |i: usize| w.get(i).and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
                match k.as_str() {
                    "map" | "clampmap" if w.len() > 1 => s.frames = vec![w[1].to_string()],
                    "animmap" if w.len() > 2 => {
                        s.fps = n(1);
                        s.frames = w[2..].iter().map(|f| f.to_string()).collect();
                    }
                    "tcmod" => {
                        let axes = |a: Option<&&str>| match a.map(|a| a.to_ascii_lowercase()).as_deref() {
                            Some("u") => (true, false),
                            Some("v") => (false, true),
                            _ => (true, true),
                        };
                        let m = match w.get(1).map(|m| m.to_ascii_lowercase()).as_deref() {
                            Some("scroll") => Some(TcMod::Scroll { u: n(2), v: n(3) }),
                            Some("scale") => Some(TcMod::Scale { u: n(2), v: n(3) }),
                            Some("rotate") => Some(TcMod::Rotate { degrees: n(2) }),
                            Some("stretch") => {
                                Wave::parse(&w[2..]).map(|wave| TcMod::Stretch { wave, u: true, v: true })
                            }
                            Some("stretch_u") => {
                                Wave::parse(&w[2..]).map(|wave| TcMod::Stretch { wave, u: true, v: false })
                            }
                            Some("stretch_v") => {
                                Wave::parse(&w[2..]).map(|wave| TcMod::Stretch { wave, u: false, v: true })
                            }
                            Some("turb") => {
                                let (u, v) = axes(w.get(2));
                                let at = if w.get(2).is_some_and(|a| a.parse::<f32>().is_err()) { 3 } else { 2 };
                                Wave::parse(&w[at.min(w.len())..]).map(|wave| TcMod::Turb { wave, u, v })
                            }
                            _ => None,
                        };
                        s.tcmods.extend(m);
                    }
                    _ => {}
                }
            }
            if !s.frames.is_empty() {
                return (s.frames.len() > 1 || !s.tcmods.is_empty()).then_some(s);
            }
        }
        None
    }

    /// The frame shown at `t` seconds.
    pub fn frame(&self, t: f32) -> usize {
        if self.frames.len() < 2 || self.fps <= 0.0 {
            return 0;
        }
        ((t * self.fps).floor().max(0.0) as usize) % self.frames.len()
    }

    /// The texture matrix at `t` seconds, the modifiers applied in order.
    pub fn uv(&self, t: f32) -> UvMatrix {
        let mut m = IDENTITY;
        for tc in &self.tcmods {
            let step = match *tc {
                TcMod::Scroll { u, v } => [[1.0, 0.0, (u * t).rem_euclid(1.0)], [0.0, 1.0, (v * t).rem_euclid(1.0)]],
                TcMod::Scale { u, v } => [[u, 0.0, 0.0], [0.0, v, 0.0]],
                TcMod::Rotate { degrees } => {
                    let (s, c) = (-degrees * t).to_radians().sin_cos();
                    // about the middle
                    [[c, -s, 0.5 - 0.5 * c + 0.5 * s], [s, c, 0.5 - 0.5 * s - 0.5 * c]]
                }
                TcMod::Stretch { wave, u, v } => {
                    let k = wave.at(t);
                    let p = if k.abs() > 1e-4 { 1.0 / k } else { 1.0 };
                    about_middle(if u { p } else { 1.0 }, if v { p } else { 1.0 })
                }
                TcMod::Turb { wave, u, v } => {
                    let d = wave.at(t);
                    [[1.0, 0.0, if u { d } else { 0.0 }], [0.0, 1.0, if v { d } else { 0.0 }]]
                }
            };
            m = then(m, step);
        }
        m
    }
}

/// The Groper blend modes by name (`0x5fc0c0`); the engine's mode is the index plus one.
pub const BLEND_NAMES: [&str; 20] = [
    "COPYRGB",
    "SRCALPHA",
    "SRCALPHA_KEEP",
    "COPYALPHA",
    "DESTALPHA",
    "DESTALPHA_KEEP",
    "SRCALPHA_ADD",
    "DESTALPHA_ADD",
    "SRCALPHA_SUB",
    "DESTALPHA_SUB",
    "SRCDEST_5050",
    "SRCALPHA_ADD_KEEP",
    "DESTALPHA_ADD_KEEP",
    "COPYRGB_KEEP",
    "SRCINVALPHA",
    "SRCINVALPHA_KEEP",
    "DESTINVALPHA",
    "DESTINVALPHA_KEEP",
    "SRCALPHA_LIGHTMAP",
    "SRCALPHA_LIGHTMAP_KEEP",
];

/// The engine's blend mode named `name` (any case).
pub fn blend_of(name: &str) -> Option<u8> {
    BLEND_NAMES.iter().position(|b| b.eq_ignore_ascii_case(name)).map(|i| i as u8 + 1)
}

/// The engine mode a stage's `alphamode` gives (`0x4aae20`, the pairs at `0x5fc578`): the name's own but for the
/// lightmaps, one up (`SRCALPHA_LIGHTMAP` is mode 20, `DESTCOLOR, SRCCOLOR`; its `_KEEP` 21). Mode 19 (`ZERO, ONE`)
/// is only `zfill`'s.
pub fn stage_mode(mode: u8) -> u8 {
    if mode >= 19 {
        mode + 1
    } else {
        mode
    }
}

/// How a Groper material draws a model's faces. **[T]** A name with a Groper material draws with its stages'
/// own engine materials (`0x4ac99f`: the director's entry `+0x10`, each stage's from `0x4aa060`), not with the
/// name's flags in the `.fpm` (the base material, `0x4a57a0`). A stage starts (`0x4a9f40`) at mode 14
/// (`COPYRGB_KEEP`, opaque), untested, writing depth; `alphamode` sets the mode (`0x4aae20`), `alphafunc` the test
/// (`0x4aa990`), a bare `depthwrite` turns depth writes off (`0x4aa777` stores 0 in `+0x234`, which `0x507dc0`
/// makes `D3DRS_ZWRITEENABLE`), and `zfill` (`0x4ab250`) is a stage of no texture in mode 19, `ZERO, ONE`
/// (`0x508520`): depth only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Render {
    /// The colour stage's blend mode.
    pub blend: u8,
    /// The least alpha (0-255) the alpha test lets through; `None` untested.
    pub alpha_min: Option<u8>,
    pub depth_write: bool,
    /// A `zfill` stage comes first: the faces' depth before their colour.
    pub zfill: bool,
}

impl Render {
    /// The material of `stages` (each a list of directives): `None` unless it has exactly one stage besides any
    /// `zfill` (**[I]** several colour stages are not drawn as such yet: the name's flags stay).
    ///
    /// **[T]** `alphafunc` (`0x4aa990`, the keywords at `0x5fc398`) keeps its reference halved, as `0..128` alpha
    /// (`0x60` the default 192): `GE128` passes above 128, `GT0` above 0, `GREATER n` above `n`, `GEQUAL n` from `n`,
    /// `TESTENABLE` from 192 (each `n` halved, so even). **[I]** `LESS`, `LEQUAL`, `LT128` and `NOTEQUAL` (unused on
    /// the disc) leave it untested.
    pub fn of(stages: &[Vec<String>]) -> Option<Render> {
        let words = |d: &String| d.split_whitespace().map(str::to_ascii_lowercase).collect::<Vec<_>>();
        let zfill = |s: &Vec<String>| s.iter().any(|d| words(d).first().is_some_and(|k| k == "zfill"));
        let colour: Vec<&Vec<String>> = stages.iter().filter(|s| !zfill(s)).collect();
        let [stage] = colour[..] else { return None };
        let mut r = Render { blend: 14, alpha_min: None, depth_write: true, zfill: stages.iter().any(zfill) };
        for w in stage.iter().map(words) {
            let n = |i: usize| w.get(i).and_then(|s| s.parse::<i32>().ok()).filter(|n| (0..=255).contains(n));
            match (w.first().map(String::as_str), w.get(1).map(String::as_str)) {
                (Some("alphamode"), Some(m)) => r.blend = blend_of(m).map(stage_mode).unwrap_or(r.blend),
                (Some("depthwrite"), _) => r.depth_write = false,
                (Some("alphafunc"), Some(f)) => {
                    // the reference as stored (halved) and doubled back; `above` for a strict test
                    let (above, at) = match f {
                        "ge128" => (true, Some(128)),
                        "gt0" => (true, Some(0)),
                        "greater" => (true, n(2).map(|n| n & !1)),
                        "gequal" => (false, n(2).map(|n| n & !1)),
                        "testenable" => (false, Some(192)),
                        _ => (false, None),
                    };
                    r.alpha_min = at.map(|a: i32| (a + i32::from(above)).min(255) as u8);
                }
                _ => {}
            }
        }
        Some(r)
    }

    /// Whether the colour stage adds to what is drawn (`SRCALPHA_ADD` and `DESTALPHA_ADD`, kept or not).
    pub fn adds(&self) -> bool {
        matches!(self.blend, 7 | 8 | 12 | 13)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_materials_stage_says_how_it_draws() {
        let st = |s: &[&[&str]]| s.iter().map(|d| d.iter().map(|x| x.to_string()).collect()).collect::<Vec<_>>();
        // the ghost's: depth first, then added where alpha is above 0, writing depth
        let ghost =
            st(&[&["zfill", "layer -2"], &["map msc_edg_4_body", "alphafunc gt0", "alphamode srcalpha_add_keep"]]);
        let r = Render::of(&ghost).unwrap();
        assert_eq!(r, Render { blend: 12, alpha_min: Some(1), depth_write: true, zfill: true });
        assert!(r.adds());
        // the defaults: opaque, untested, writing depth; a bare `depthwrite` turns writing off
        let plain = Render::of(&st(&[&["map x"]])).unwrap();
        assert_eq!(plain, Render { blend: 14, alpha_min: None, depth_write: true, zfill: false });
        let ring = Render::of(&st(&[&["map q", "alphamode srcalpha", "depthwrite", "alphafunc ge128"]])).unwrap();
        assert_eq!((ring.blend, ring.depth_write, ring.alpha_min), (2, false, Some(129)));
        // the last alphafunc wins; an odd reference loses its lowest bit
        let leaves = Render::of(&st(&[&["map l", "alphafunc testenable", "alphafunc greater 65"]])).unwrap();
        assert_eq!(leaves.alpha_min, Some(65));
        assert_eq!(Render::of(&st(&[&["map l", "alphafunc testenable"]])).unwrap().alpha_min, Some(192));
        // the lightmaps are one up: `srcalpha_lightmap` is mode 20
        assert_eq!(Render::of(&st(&[&["map lm", "alphamode srcalpha_lightmap"]])).unwrap().blend, 20);
        // several colour stages: the name's flags stay
        assert_eq!(Render::of(&st(&[&["map a"], &["map b"]])), None);
    }

    fn stage(lines: &[&str]) -> Option<Stage> {
        Stage::of(&[lines.iter().map(|l| l.to_string()).collect()])
    }

    #[test]
    fn an_animmap_steps_its_frames_and_a_still_map_is_none() {
        let s = stage(&["animmap 10 a b c", "alphamode srcalpha"]).unwrap();
        assert_eq!((s.frame(0.0), s.frame(0.15), s.frame(0.29), s.frame(0.31)), (0, 1, 2, 0));
        assert_eq!(stage(&["map plain", "alphafunc GE128"]), None);
    }

    #[test]
    fn the_modifiers_move_the_coordinates() {
        let at =
            |m: UvMatrix, u: f32, v: f32| [m[0][0] * u + m[0][1] * v + m[0][2], m[1][0] * u + m[1][1] * v + m[1][2]];
        let s = stage(&["map sky", "tcMod scroll 0 .2"]).unwrap();
        let p = at(s.uv(2.5), 0.3, 0.1);
        assert!((p[0] - 0.3).abs() < 1e-5 && (p[1] - 0.6).abs() < 1e-5);
        // a quarter turn a second about the middle: (1, 0.5) goes to (0.5, 0) after a second of -90
        let s = stage(&["map ring", "tcMod rotate 90"]).unwrap();
        let p = at(s.uv(1.0), 1.0, 0.5);
        assert!((p[0] - 0.5).abs() < 1e-5 && (p[1] - 0.0).abs() < 1e-5, "{p:?}");
        // a stretch to twice keeps the middle and halves the coordinates' spread
        let s = stage(&["map glow", "tcMod stretch sin 2 0 0 1"]).unwrap();
        let p = at(s.uv(0.0), 1.0, 0.5);
        assert!((p[0] - 0.75).abs() < 1e-5 && (p[1] - 0.5).abs() < 1e-5);
        let s = stage(&["map water", "tcMod turb UV sin 0 0.02 0 .2"]).unwrap();
        assert!(matches!(s.tcmods[0], TcMod::Turb { u: true, v: true, .. }));
    }
}
