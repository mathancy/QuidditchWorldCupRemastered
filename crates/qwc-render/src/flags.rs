//! The wavy flags: the stadium's pennants and banners, cloth strips waved by the wind (`GameRender\Common\
//! FlagRender.cpp`, the render module whose vtable is at `0x5b6030`; `docs/RE_NOTES.md` "Wavy flags" has the trace).
//!
//! [`WavyFlags`] holds what a stadium's `wavyflags.cli` sets through the `WavyFlags.*` commands' own handlers
//! (`0x49d920`-`0x49e200`, the records at `0x64d320`). Their calls build on each other (`WindDir` turns every flag, a
//! `Size` of two numbers sizes them all), so a script runs here in order rather than through the console's
//! last-call values. A flag's texture is chosen when its `Texture` line runs, from two DataStore entries
//! ([`Store`], [`texture`]). [`WavyFlags::strip`] is a flag's cloth at a moment and [`Flag::matrix`] where it
//! stands (the draw, `0x49bb60`).
//!
//! **[I]** not drawn: the levels of detail (the finest, [`COLUMNS`], always), the frustum test, and the lighting with
//! its light transfer (the port has no lighting yet; the original draws each side as its own strip, the back one
//! with its normals turned, which unlit is one strip drawn from both sides).

use qwc_assets::model::{Material, Mesh, Model};
use qwc_formats::fpm;
use qwc_formats::text_defs::{self, Ext, Parsed};

/// The most flags: `WavyFlags.NumFlags` clamps to it (`0x49d990`).
pub const MAX_FLAGS: usize = 64;

/// The flags' material (`0x49d7c6`-`0x49d8bb`): registered in layer 10 (`0x502ab0`), so after the stadium's, blended
/// `SRCALPHA_KEEP` (`0x502f10`'s fields set in place), alpha tested and writing depth (the material's `+0xc` bit 0,
/// `0x503390`; `+0x10` bit 2), clamped (`+0x10` bits 0 and 1, the default of `0x507670`).
pub const SURFACE: fpm::Surface =
    fpm::Surface { layer: 10, blend: fpm::blend::SRCALPHA_KEEP, wrap: [false; 2], alpha_test: true, depth_write: true };

/// A strip's columns at the finest level (`[0x5fccdc]`); level `n` has `COLUMNS >> n` (`0x492b00`).
pub const COLUMNS: usize = 24;

/// DataStore `MatchType` and `InWorldCupFlow` (their names' `name_hash`), read by the texture lookup (`0x49d5a0`).
pub const KEY_MATCH_TYPE: u32 = 0x0131_d63e;
pub const KEY_IN_WORLD_CUP_FLOW: u32 = 0x8817_337d;

/// The exe's 2π (`0x5bf1e0`) and degrees to radians (`0x5bf328`), a little short of the true ones.
#[allow(clippy::approx_constant)]
const TWO_PI: f32 = 6.28318;
const DEGREE: f32 = 0.017453;

/// A wind: two waves summed, wave `k` being `sin((s - t * timescale[k]) * 2π * frequency[k]) * amplitude[k]` at `s`
/// along the flag, `t` seconds on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wind {
    pub timescale: [f32; 2],
    pub frequency: [f32; 2],
    pub amplitude: [f32; 2],
}

impl Wind {
    /// The arguments of `WindParams` and `WindParamsBanner`, in their order.
    fn from_args(a: [f32; 6]) -> Wind {
        Wind { timescale: [a[0], a[1]], frequency: [a[2], a[3]], amplitude: [a[4], a[5]] }
    }

    /// The two waves' phases at `time` (`-t * timescale`) and their factors on `s` (`2π * frequency`), as the draw
    /// keeps them (`0x49bb9a`-`0x49bc69`).
    fn at(&self, time: f64) -> ([f32; 2], [f32; 2]) {
        (self.timescale.map(|ts| -(time * f64::from(ts)) as f32), self.frequency.map(|f| f * TWO_PI))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    /// Type 0 (`pennant`): flown from its pole at `x = 0`, streaming along `+x`, its wave growing to the tip.
    #[default]
    Pennant,
    /// Type 1 (`banner`): hung from its top edge, centred on its place, its bottom edge swinging.
    Banner,
}

/// The render group a texture is looked up in (`0x4c7760` before the lookup `0x504420`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Stadium,
    Team1,
    Team2,
}

/// A flag's texture: its name in a render group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Texture {
    pub group: Group,
    pub name: String,
}

/// The DataStore entries a texture's name follows (their first bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Store {
    pub match_type: u32,
    pub in_world_cup_flow: bool,
}

/// One flag's record (`0x38` bytes from `0x64d320`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Flag {
    /// `+0x00`.
    pub kind: Kind,
    /// `+0x04`.
    pub pos: [f32; 3],
    /// `+0x18`: turned about Y by this many degrees.
    pub rot: f32,
    /// `+0x24` along the cloth, `+0x28` across.
    pub length: f32,
    pub height: f32,
    /// `+0x2c`: how much of the light behind the cloth reaches its front (**[I]** not drawn).
    pub light_transfer: f32,
    /// `+0x30` (the texture) and `+0x34` (its material, `0x502ab0` in layer 10).
    pub texture: Option<Texture>,
}

impl Flag {
    /// Where the cloth stands (`0x49bd38`-`0x49bda0`): `RY(-rot) · T(pos)`, for row vectors, the translation in the
    /// last row.
    pub fn matrix(&self) -> [f32; 16] {
        let (s, c) = (-(self.rot * DEGREE)).sin_cos();
        let [x, y, z] = self.pos;
        [c, 0.0, -s, 0.0, 0.0, 1.0, 0.0, 0.0, s, 0.0, c, 0.0, x, y, z, 1.0]
    }
}

/// A flag's cloth at a moment, in its own space: the vertices in triangle-strip order, the top edge's then the
/// bottom edge's of each column.
#[derive(Debug, Clone, PartialEq)]
pub struct Strip {
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
}

/// The flag module's settings (the render module at `0x64d310`).
#[derive(Debug, Clone, PartialEq)]
pub struct WavyFlags {
    /// `WavyFlags.Enable` (`+0xe18`, on from the start, `0x49b250`).
    pub enabled: bool,
    /// `WavyFlags.NumFlags` (`+0xe14`): the records used.
    pub count: usize,
    /// All [`MAX_FLAGS`] records.
    pub flags: Vec<Flag>,
    /// `WavyFlags.WindDir` (`[0xf917c8]`), degrees.
    pub wind_dir: f32,
    /// `WavyFlags.WindParams` (`0x5fcce8`-`0x5fccfc`) and `WavyFlags.WindParamsBanner` (`0xfd5xxx`, zero-filled).
    pub pennant: Wind,
    pub banner: Wind,
}

impl Default for WavyFlags {
    fn default() -> WavyFlags {
        WavyFlags::new()
    }
}

/// `name` without `prefix` when it starts with it (without regard to case) and is longer (`0x58df49`).
fn strip_prefix<'a>(name: &'a str, prefix: &str) -> Option<&'a str> {
    let n = prefix.len();
    (name.len() > n && name.as_bytes()[..n].eq_ignore_ascii_case(prefix.as_bytes())).then(|| &name[n..])
}

/// The texture a flag's `WavyFlags.Texture` names (`0x49d5a0`). Outside a match (`MatchType` 0) every name is the
/// stadium's, a `team1_`, `team2_` or `generic_` prefix dropped, and `banner` is `Gry_Banner` unless the World Cup's
/// flow is on. In a match the prefix picks the group: the first or second team's, else the stadium's.
pub fn texture(name: &str, store: &Store) -> Texture {
    let prefixes = [("team1_", Group::Team1), ("team2_", Group::Team2), ("generic_", Group::Stadium)];
    let found = prefixes.iter().find_map(|&(p, g)| strip_prefix(name, p).map(|rest| (rest, g)));
    let (rest, group) = found.unwrap_or((name, Group::Stadium));
    if store.match_type != 0 {
        return Texture { group, name: rest.to_string() };
    }
    let rest = if !store.in_world_cup_flow && rest.eq_ignore_ascii_case("banner") { "Gry_Banner" } else { rest };
    Texture { group: Group::Stadium, name: rest.to_string() }
}

/// An integer argument: decimal or `0x` hex (`0x4cfd90`), as the handlers keep it (32 bits, compared unsigned).
fn int(s: &str) -> Option<u32> {
    let v = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => i64::from_str_radix(h, 16).ok()?,
        None => s.parse::<i64>().ok()?,
    };
    Some(v as u32)
}

fn float(s: &str) -> Option<f32> {
    s.parse().ok()
}

/// A flag number given as a float (`Size`, `LightTransfer`): truncated (`_ftol`), compared unsigned.
fn float_index(s: &str) -> Option<u32> {
    Some(float(s)? as i32 as u32)
}

impl WavyFlags {
    /// As the module starts: on, no flags, the pennants' wind at its image default (`10 15 1/6 1/17 1 3`), the
    /// banners' still.
    pub fn new() -> WavyFlags {
        WavyFlags {
            enabled: true,
            count: 0,
            flags: vec![Flag::default(); MAX_FLAGS],
            wind_dir: 0.0,
            pennant: Wind { timescale: [10.0, 15.0], frequency: [1.0 / 6.0, 1.0 / 17.0], amplitude: [1.0, 3.0] },
            banner: Wind { timescale: [0.0; 2], frequency: [0.0; 2], amplitude: [0.0; 2] },
        }
    }

    /// Load the stadium's `wavyflags.cli` (`0x49b640`, and `General_RefreshWavyFlags`, `0x49b6d0`): the records
    /// cleared and the count with them (the first load keeps the count, which the script sets first), then the
    /// script run. The winds and the switch stay as they were. Answers the lines that did nothing.
    pub fn load(&mut self, script: &[u8], store: &Store) -> Vec<String> {
        self.count = 0;
        self.flags.iter_mut().for_each(|f| *f = Flag::default());
        let Ok(Parsed::Cli { values, .. }) = text_defs::parse(Ext::Cli, script) else { return Vec::new() };
        let mut ignored = Vec::new();
        for (key, args) in values {
            let args = args.unwrap_or_default();
            if !self.command(&key, &args, store) {
                ignored.push(format!("{key} {args}"));
            }
        }
        ignored
    }

    /// The flags in use, by number.
    pub fn active(&self) -> impl Iterator<Item = (usize, &Flag)> {
        self.flags.iter().take(self.count).enumerate()
    }

    /// Run one `WavyFlags.*` line as its handler does; answers whether it did anything (a line with the wrong number
    /// of arguments, or for a flag past the count, only prints in the original).
    pub fn command(&mut self, path: &str, args: &str, store: &Store) -> bool {
        let Some((group, name)) = path.split_once('.') else { return false };
        if !group.eq_ignore_ascii_case("WavyFlags") {
            return false;
        }
        let a: Vec<&str> = args.split_whitespace().collect();
        self.run(&name.to_ascii_lowercase(), &a, store).is_some()
    }

    fn flag(&mut self, i: u32) -> Option<&mut Flag> {
        let n = self.count;
        self.flags.get_mut(i as usize).filter(|_| (i as usize) < n)
    }

    fn run(&mut self, name: &str, a: &[&str], store: &Store) -> Option<()> {
        let floats = |from: usize| -> Option<Vec<f32>> { a[from..].iter().map(|s| float(s)).collect() };
        match (name, a.len()) {
            // `0x49d920`: its byte
            ("enable", 1) => {
                self.enabled = match a[0].to_ascii_lowercase().as_str() {
                    "true" | "1" => true,
                    "false" | "0" => false,
                    _ => return None,
                }
            }
            // `0x49d990`
            ("numflags", 1) => self.count = (int(a[0])? as usize).min(MAX_FLAGS),
            // `0x49da00`
            ("pos", 4) => {
                let p = floats(1)?;
                self.flag(int(a[0])?)?.pos = [p[0], p[1], p[2]];
            }
            // `0x49db50`: the angles `(0, y, 0)`
            ("rot", 2) => self.flag(int(a[0])?)?.rot = float(a[1])?,
            // `0x49dd30`: two numbers size every flag (and the template strip, `0x49b7a0`, which nothing draws);
            // three size one
            ("size", 2) => {
                let s = floats(0)?;
                let n = self.count;
                for f in &mut self.flags[..n] {
                    (f.length, f.height) = (s[0], s[1]);
                }
            }
            ("size", 3) => {
                let s = floats(1)?;
                let f = self.flag(float_index(a[0])?)?;
                (f.length, f.height) = (s[0], s[1]);
            }
            // `0x49dc90` -> `0x49d5a0`
            ("texture", 2) => self.flag(int(a[0])?)?.texture = Some(texture(a[1], store)),
            // `0x49e180` -> `0x49d8d0`: another name leaves the type
            ("flagtype", 2) => {
                let f = self.flag(int(a[0])?)?;
                if a[1].eq_ignore_ascii_case("pennant") {
                    f.kind = Kind::Pennant;
                } else if a[1].eq_ignore_ascii_case("banner") {
                    f.kind = Kind::Banner;
                }
            }
            // `0x49e0d0`: the wind turns every flag to it
            ("winddir", 1) => {
                let d = float(a[0])?;
                self.wind_dir = d;
                let n = self.count;
                self.flags[..n].iter_mut().for_each(|f| f.rot = d);
            }
            // `0x49df30`, `0x49e000`
            ("windparams", 6) => self.pennant = Wind::from_args(floats(0)?.try_into().ok()?),
            ("windparamsbanner", 6) => self.banner = Wind::from_args(floats(0)?.try_into().ok()?),
            // `0x49e200`: one number sets every flag's, two one flag's
            ("lighttransfer", 1) => {
                let t = float(a[0])?;
                let n = self.count;
                self.flags[..n].iter_mut().for_each(|f| f.light_transfer = t);
            }
            ("lighttransfer", 2) => self.flag(float_index(a[0])?)?.light_transfer = float(a[1])?,
            _ => return None,
        }
        Some(())
    }

    /// Flag `i`'s cloth `time` seconds on, at the finest level (`0x49bb60`): [`COLUMNS`] columns `L / (COLUMNS - 1)`
    /// apart, `u` across them in 1/4096ths (cut down, `_ftol`), `v` 0 on the top edge and 1 on the bottom. `s`, a
    /// column's place along the wave, is its `x` plus the flag's `x + z`, so flags apart wave apart.
    /// - A pennant (`0x49c013`) runs from `x = 0` to `L`, its edges at `y = ±H/2`, both pushed out by the
    ///   pennants' wind times `x / L`.
    /// - A banner (`0x49c851`) runs from `x = -L/2` to `L/2`, its top edge still at `y = 0` and its bottom at
    ///   `y = -H` pushed out by the banners' wind. Its waves run with the wind: facing away from it (the wind's
    ///   direction and the banner's turn more than 90° apart) both run back, and side on (their cosine under 0.1)
    ///   the first does.
    pub fn strip(&self, i: usize, time: f64) -> Strip {
        let f = &self.flags[i];
        let cols = COLUMNS;
        let du = 1.0 / (cols as f32 - 1.0);
        let dx = f.length * du;
        let u_step = du * 2048.0;
        let phase = f.pos[2] + f.pos[0];
        let (wind, offset, top, bottom) = match f.kind {
            Kind::Pennant => (&self.pennant, 0.0, f.height * 0.5, f.height * -0.5),
            Kind::Banner => (&self.banner, f.length * 0.5, 0.0, -f.height),
        };
        let (mut p, k) = wind.at(time);
        if f.kind == Kind::Banner {
            let (sw, cw) = (self.wind_dir * DEGREE).sin_cos();
            let (sr, cr) = (f.rot * DEGREE).sin_cos();
            let d = sw * sr + cw * cr;
            if d < 0.0 {
                p = p.map(|v| -v);
            } else if d.abs() < 0.1 {
                p[0] = -p[0];
            }
        }
        let inv_length = 1.0 / f.length;
        let (mut positions, mut uvs) = (Vec::with_capacity(2 * cols), Vec::with_capacity(2 * cols));
        for j in 0..cols {
            let x = j as f32 * dx - offset;
            let s = x + phase;
            let w = [0, 1].map(|n| (f64::from((s + p[n]) * k[n])).sin() as f32);
            let push = w[1] * wind.amplitude[1] + w[0] * wind.amplitude[0];
            let u = ((2 * j) as f64 * f64::from(u_step)).trunc() as f32 / 4096.0;
            let (z_top, z_bottom) = match f.kind {
                Kind::Pennant => {
                    let z = push * x * inv_length;
                    (z, z)
                }
                Kind::Banner => (0.0, push),
            };
            positions.extend([[x, top, z_top], [x, bottom, z_bottom]]);
            uvs.extend([[u, 0.0], [u, 1.0]]);
        }
        Strip { positions, uvs }
    }

    /// Flag `i`'s cloth `time` seconds on as a mesh: one strip drawn from both sides, without normals (unlit).
    pub fn mesh(&self, i: usize, time: f64) -> Mesh {
        let s = self.strip(i, time);
        let indices = strip_triangles(s.positions.len());
        Mesh { two_sided: true, positions: s.positions, uvs: s.uvs, indices, ..Mesh::default() }
    }

    /// Flag `i` as a model standing at its place, its texture the `.fsh` at `texture` (white without one).
    pub fn model(&self, i: usize, texture: Option<String>, time: f64) -> Model {
        let f = &self.flags[i];
        let name = f.texture.as_ref().map_or_else(String::new, |t| t.name.clone());
        Model {
            path: format!("wavyflags/{i}"),
            matrix: f.matrix(),
            materials: vec![Material { name, texture, image: None, surface: Some(SURFACE), groper: None }],
            skeleton: None,
            meshes: vec![self.mesh(i, time)],
        }
    }
}

/// A triangle strip of `n` vertices as a list: triangle `i` is `(i, i+1, i+2)`, every other one turned so all face
/// one way.
pub fn strip_triangles(n: usize) -> Vec<u32> {
    (0..n.saturating_sub(2) as u32)
        .flat_map(|i| if i % 2 == 0 { [i, i + 1, i + 2] } else { [i + 1, i, i + 2] })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &[u8] = b"WavyFlags.NumFlags 3\nWavyFlags.WindDir 30\n# a comment\n\
        WavyFlags.Size 8 2\nWavyFlags.FlagType 2 banner\nWavyFlags.Size 2 4.0 6\nWavyFlags.Rot 2 120\n\
        WavyFlags.Pos 1 1.5\t2 -3\nWavyFlags.Texture 0 team1_pennant\nWavyFlags.Texture 2 banner\n\
        WavyFlags.Pos 7 1 1 1\nWavyFlags.LightTransfer 0.5\nWavyFlags.LightTransfer 1.9 0.25\n\
        WavyFlags.WindParamsBanner 2 4 0.5 0.25 1 2\n";

    #[test]
    fn script_lines_build_on_each_other() {
        let mut w = WavyFlags::new();
        let ignored = w.load(SCRIPT, &Store::default());
        assert_eq!(ignored, ["WavyFlags.Pos 7 1 1 1"]);
        assert_eq!(w.count, 3);
        // the wind turned all three, the second size line only the third, its own turn after
        assert_eq!(w.flags.iter().take(3).map(|f| f.rot).collect::<Vec<_>>(), [30.0, 30.0, 120.0]);
        assert_eq!([w.flags[0].length, w.flags[0].height, w.flags[2].length, w.flags[2].height], [8.0, 2.0, 4.0, 6.0]);
        assert_eq!(w.flags[1].pos, [1.5, 2.0, -3.0]);
        assert_eq!([w.flags[0].light_transfer, w.flags[1].light_transfer], [0.5, 0.25]);
        assert_eq!(
            w.flags.iter().take(3).map(|f| f.kind).collect::<Vec<_>>(),
            [Kind::Pennant, Kind::Pennant, Kind::Banner]
        );
        assert_eq!(w.flags[0].texture, Some(Texture { group: Group::Stadium, name: "pennant".into() }));
        assert_eq!(w.flags[2].texture.as_ref().map(|t| t.name.as_str()), Some("Gry_Banner"));
        assert_eq!(w.banner, Wind { timescale: [2.0, 4.0], frequency: [0.5, 0.25], amplitude: [1.0, 2.0] });
        // loading again starts the records over
        w.load(b"WavyFlags.NumFlags 1000\n", &Store::default());
        assert_eq!((w.count, w.flags[2].kind), (MAX_FLAGS, Kind::Pennant));
    }

    #[test]
    fn textures_follow_the_data_store() {
        let fe = Store::default();
        let cup = Store { in_world_cup_flow: true, ..fe };
        let game = Store { match_type: 2, ..fe };
        let t = |n: &str, s: &Store| texture(n, s);
        assert_eq!(t("BANNER", &fe).name, "Gry_Banner");
        assert_eq!(t("banner", &cup).name, "banner");
        assert_eq!(t("generic_banner", &fe).name, "Gry_Banner");
        assert_eq!(t("team2_flag", &fe), Texture { group: Group::Stadium, name: "flag".into() });
        assert_eq!(t("team2_flag", &game), Texture { group: Group::Team2, name: "flag".into() });
        assert_eq!(t("banner", &game).name, "banner");
        // a prefix alone is a name
        assert_eq!(t("team1_", &game), Texture { group: Group::Stadium, name: "team1_".into() });
    }

    #[test]
    fn pennants_stream_from_the_pole_and_banners_hang() {
        let mut w = WavyFlags::new();
        w.load(b"WavyFlags.NumFlags 2\nWavyFlags.Size 6 3\nWavyFlags.FlagType 1 banner\n", &Store::default());
        w.banner = w.pennant;
        let p = w.strip(0, 1.25);
        assert_eq!((p.positions.len(), p.uvs.len()), (2 * COLUMNS, 2 * COLUMNS));
        // fixed at the pole, the edges 3 apart, the far end at the length
        assert_eq!(&p.positions[..2], &[[0.0, 1.5, 0.0], [0.0, -1.5, 0.0]]);
        assert!((p.positions[2 * COLUMNS - 1][0] - 6.0).abs() < 1e-5);
        assert!(p.positions.iter().any(|v| v[2].abs() > 0.1));
        assert_eq!(&p.uvs[..2], &[[0.0, 0.0], [0.0, 1.0]]);
        assert!(p.uvs[2 * COLUMNS - 1][0] > 0.999 && p.uvs[2 * COLUMNS - 1][0] <= 1.0);
        let b = w.strip(1, 1.25);
        // centred, the top edge still, the bottom 3 down and swinging
        assert!((b.positions[0][0] + 3.0).abs() < 1e-5);
        assert!(b.positions.iter().step_by(2).all(|v| v[1] == 0.0 && v[2] == 0.0));
        assert!(b.positions.iter().skip(1).step_by(2).all(|v| v[1] == -3.0));
        assert!(b.positions.iter().skip(1).step_by(2).any(|v| v[2].abs() > 0.1));
        // the waves move on
        assert_ne!(w.strip(0, 2.0), p);
    }

    #[test]
    fn placed_turned_about_y() {
        let f = Flag { pos: [1.0, 2.0, 3.0], rot: 90.0, ..Flag::default() };
        let m = f.matrix();
        // the cloth's +x runs along the world's +z
        assert!((m[0]).abs() < 1e-4 && (m[2] - 1.0).abs() < 1e-4);
        assert_eq!(&m[12..], &[1.0, 2.0, 3.0, 1.0]);
        assert_eq!(strip_triangles(4), [0, 1, 2, 2, 1, 3]);
    }
}
