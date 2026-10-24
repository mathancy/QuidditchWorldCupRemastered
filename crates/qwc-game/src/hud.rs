//! The match's HUD (`HUD\HudElement.cpp`, `HUD\HudElementSeekerBar.cpp`, `0x4ed000`-`0x4f9000`), drawn in the
//! front end's overlay (the 512 x 512 screen) under its layers, from the original's textures in `root/HUD`.
//!
//! **[T]**:
//! - the textures and their material layers (`0x4f57a0`): the top bar (`topbar`, layer `0x34`) and its fills
//!   (`topbar_white_alpha`, `topbar_white_gradient`, `0x32`), the Snitch (`QWC_GUI_SnitchLeft`, `SnitchRight`,
//!   `0x36`) and its glow (`0x35`);
//! - a team's score is drawn as three numerals, `%03d` of it held at 999 at most (`0x4f34e0`), each
//!   `QWC_GUI_Numerals<d>_execversion` at `64 * 0.85` square, `44 * 0.85` apart (`0x668b74`, `0x5bf170`,
//!   `0x5bf2a4`);
//! - the teams' power bars sum to 1 at most and start the Seeker game when they do (`qwc_sim::powerbar`).
//!
//! The top bar, the moves' cards and the Seeker race's gauges are the match scene of `qwc_hud` (`match.ron`, made
//! in QWC Studio, `qwc-studio`), given the values below, and a challenge's buttons, clock and countdown its scene
//! (`challenge.ron`, [`challenge_values`]); each is the highest resource pack's (`qwc_packs`, its `hud/`) while one
//! has it (reloaded when it changes, so the editor's saves show in a running match), else the stock one built in.
//! **[I]** the stock layout (the original's positions come through `0x4ef530`'s transform, not traced), measured
//! from the original's screen in the 640 x 480 the HUD's sizes are in, kept that size in the middle of a wider
//! screen: each team's score box at its end of the top, its numerals inside; its bar (the team folder's
//! `QWC_GUI_SnitchBar`, the team's colour between wooden rails) from the box towards the middle as far as its power
//! reaches; a Snitch half riding each bar's tip, the two making the whole Snitch as the bars meet; the box filled
//! with the team's colour (the bar's own).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use qwc_assets::Vfs;
use qwc_feck::draw::Fit;
use qwc_hud::{Scene, Value, Values};
use qwc_render::overlay::{Overlay, OverlayQuad, OverlayVertex};
use qwc_render::Gpu;
use qwc_sim::Match;

/// The HUD's textures' keys in the overlay (a range of their own).
const KEY: u64 = 1 << 60;

/// The Seeker game's start on the HUD (seconds into the race): the bars draw back from `SEEKER_SHRINK_AT` for
/// `SEEKER_SHRINK_FOR` as the gauges grow in, then the tips' glows fade over `SEEKER_GLOW_FADE`.
const SEEKER_SHRINK_AT: f32 = 1.0;
const SEEKER_SHRINK_FOR: f32 = 1.0;
const SEEKER_GLOW_FADE: f32 = 0.5;

/// A passing-challenge ring on the 640 x 480 screen.
pub(crate) struct Ring {
    pub at: [f32; 2],
    pub radius: f32,
    /// In the engine's units (1 for 128).
    pub colour: [f32; 4],
}

/// What the HUD shows besides the match: the teams' folders (their bars), a challenge's rings and the challenge.
/// (The active players and the pass target are the effects' highlights, drawn in the world: `crate::fx`.)
pub(crate) struct View<'a> {
    pub teams: [&'a str; 2],
    pub rings: &'a [Ring],
    pub challenge: Option<(&'a qwc_sim::challenge::Challenge, &'a [u32])>,
}

/// The textures the HUD draws, uploaded on first use, and its scenes.
pub(crate) struct Hud {
    loaded: HashMap<String, Option<u64>>,
    /// The teams' colours by folder.
    colours: HashMap<String, Option<[f32; 3]>>,
    game: SceneFile,
    challenge: SceneFile,
}

impl Default for Hud {
    fn default() -> Hud {
        Hud {
            loaded: HashMap::new(),
            colours: HashMap::new(),
            game: SceneFile::new("match.ron", qwc_hud::MATCH),
            challenge: SceneFile::new("challenge.ron", qwc_hud::CHALLENGE),
        }
    }
}

/// A scene read from its file while there is one ([`scene_path`]; read again when it changes), else the built-in one.
struct SceneFile {
    file: &'static str,
    built_in: &'static str,
    scene: Scene,
    /// The file's time when it was read (none for the built-in scene); none before the first read.
    read: Option<Option<SystemTime>>,
    checked: Option<Instant>,
}

impl SceneFile {
    fn new(file: &'static str, built_in: &'static str) -> SceneFile {
        SceneFile { file, built_in, scene: Scene::default(), read: None, checked: None }
    }

    /// The scene, read again when its file changed (looked at twice a second).
    fn get(&mut self) -> &Scene {
        if self.checked.is_none_or(|t| t.elapsed() >= Duration::from_millis(500)) {
            self.checked = Some(Instant::now());
            self.refresh();
        }
        &self.scene
    }

    fn refresh(&mut self) {
        let path = scene_path(self.file).unwrap_or_default();
        let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if self.read == Some(modified) {
            return;
        }
        let first = self.read.is_none();
        self.read = Some(modified);
        let scene = match modified {
            Some(_) => std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| Scene::from_ron(&t)),
            None => Scene::from_ron(self.built_in),
        };
        match scene {
            Ok(s) => {
                if !first {
                    eprintln!("HUD: read {} again", path.display());
                }
                self.scene = s;
            }
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                if first {
                    self.scene = Scene::from_ron(self.built_in).unwrap_or_default();
                }
            }
        }
    }
}

/// A quad over `x0..x1` by `y0..y1` of the window as a 640 x 480 screen, `u0..u1` of the texture across, where
/// `fit` puts the front end's screen.
fn quad(texture: u64, layer: u32, rect: [f32; 4], u: [f32; 2], colour: [f32; 4], fit: Fit) -> OverlayQuad {
    let [x0, y0] = fit.from_window([rect[0], rect[1]], (640.0, 480.0));
    let [x1, y1] = fit.from_window([rect[2], rect[3]], (640.0, 480.0));
    let v = |x, y, uu, vv| OverlayVertex { pos: [x, y], uv: [uu, vv], colour };
    OverlayQuad {
        texture,
        blend: 2,
        layer,
        corners: [v(x0, y1, u[0], 1.0), v(x1, y1, u[1], 1.0), v(x0, y0, u[0], 0.0), v(x1, y0, u[1], 0.0)],
    }
}

/// The scene file `file` (`match.ron`) the HUD reads: in `$QWC_HUD_SCENES` when that is set, else the highest
/// pack's (`qwc_packs`, its `hud/`); none without, when the built-in stock scene is drawn.
pub fn scene_path(file: &str) -> Option<PathBuf> {
    match std::env::var_os("QWC_HUD_SCENES") {
        Some(dir) => Some(PathBuf::from(dir).join(file)),
        None => qwc_packs::current().hud(file),
    }
}

impl Hud {
    fn texture(&mut self, overlay: &mut Overlay, gpu: &Gpu, vfs: &Vfs, name: &str) -> Option<u64> {
        let next = KEY + self.loaded.len() as u64;
        if let Some(key) = self.loaded.get(name) {
            return *key;
        }
        let path = qwc_hud::texture_path(name);
        let key = match qwc_assets::texture::load(vfs, &path) {
            Ok(t) => {
                overlay.upload(gpu, next, t.width, t.height, t.rgba());
                Some(next)
            }
            Err(e) => {
                eprintln!("{path}: {e}");
                None
            }
        };
        self.loaded.insert(name.to_string(), key);
        key
    }

    /// Team `folder`'s colour, from its bar.
    fn team_colour(&mut self, vfs: &Vfs, folder: &str) -> Option<[f32; 3]> {
        *self.colours.entry(folder.to_string()).or_insert_with(|| {
            let path = qwc_hud::texture_path(&format!("teams/{folder}/QWC_GUI_SnitchBar"));
            let t = qwc_assets::texture::load(vfs, &path).ok()?;
            qwc_hud::team_colour(t.rgba(), t.width)
        })
    }

    /// The match scene's values for match `m`.
    fn values(&mut self, vfs: &Vfs, m: &Match, view: &View) -> Values {
        use qwc_sim::powerbar::{BLUDGER, DODGE, SPECIAL};
        let mut v = Values::defaults(self.game.get());
        // the Seeker game's start (**[I]**, the user's account of the original, 2026-10-21): a second in, the power
        // bars draw back to the boxes over a second, their glows riding the tips, while the seekers' gauges grow
        // in; the glows then fade
        let age = m.race.as_ref().map(|r| r.ticks as f32 / 24.0);
        let ease = |f: f32| {
            let f = f.clamp(0.0, 1.0);
            f * f * (3.0 - 2.0 * f)
        };
        let appear = age.map_or(0.0, |a| ease((a - SEEKER_SHRINK_AT) / SEEKER_SHRINK_FOR));
        let glow = age.map_or(1.0, |a| 1.0 - ease((a - SEEKER_SHRINK_AT - SEEKER_SHRINK_FOR) / SEEKER_GLOW_FADE));
        v.set("seeker.appear", Value::Scalar(appear));
        v.set("seeker.glow", Value::Scalar(glow));
        let cards = [
            (DODGE, "dodge_button_active"),
            (BLUDGER, "bludger_button_active"),
            (SPECIAL, "specialmove_button_active"),
        ];
        for (t, side) in ["left", "right"].into_iter().enumerate() {
            let name = |what: &str| format!("{what}.{side}");
            v.set(&name("power"), Value::Scalar(m.power.main[t] as f32 * (1.0 - appear)));
            v.set(&name("score"), Value::Scalar(m.teams[t].score as f32));
            v.set(&name("team"), Value::Text(view.teams[t].to_string()));
            let [r, g, b] = self.team_colour(vfs, view.teams[t]).unwrap_or([0.5; 3]);
            v.set(&name("colour"), Value::Colour([r, g, b, 1.0]));
            // a human team's moves, lit when unlocked and their sub-bar is full (**[I]** the rule; the original's
            // buttons are `0x4359e0`'s flags)
            v.set(&name("cards"), Value::Flag(m.teams[t].human && m.race.is_none()));
            for (i, (k, lit)) in cards.into_iter().enumerate() {
                let pb = &m.power;
                let ready = pb.open(t, k) && pb.pots[t][k] > 0.0 && pb.sub[t][k] >= pb.pots[t][k];
                v.set(&format!("card.{side}.{i}"), Value::Text(if ready { lit } else { "STO_buttons_empty" }.into()));
            }
            // the Seeker race: each seeker's boost (`HudElementSeekerBar.cpp`'s `seeker_power_indicator`, filled
            // with `seeker_power_bar_green`, red below `MinimumBoostLevel`; **[I]** the colour rule)
            if let Some(r) = &m.race {
                let s = &r.seekers[t];
                let full = s.boosting || s.level >= m.race_rules.minimum_boost_level;
                let bar = if full { "seeker_power_bar_green" } else { "seeker_power_bar_red" };
                v.set(&format!("seeker.{side}.level"), Value::Scalar(s.level.clamp(0.0, 1.0) as f32));
                v.set(&format!("seeker.{side}.bar"), Value::Text(bar.into()));
            }
        }
        v.set("seeker_game", Value::Flag(m.power.seeker_game));
        v.set("race", Value::Flag(m.race.is_some()));
        v
    }

    /// The HUD for match `m` on a screen of `size` pixels, as overlay quads in material layer order (on the front
    /// end's screen where `fit` puts it); `view` has a challenge's rings. The HUD's scene lays out over the whole
    /// window, its anchors at the window's edges.
    #[allow(clippy::too_many_arguments)]
    pub fn quads(
        &mut self,
        overlay: &mut Overlay,
        gpu: &Gpu,
        vfs: &Vfs,
        m: &Match,
        view: &View,
        size: (u32, u32),
        fit: Fit,
    ) -> Vec<OverlayQuad> {
        let mut out = Vec::new();
        // the passing challenge's rings (`stadium/chal_01/fx/pass_ring`, tinted: **[I]** drawn flat towards the
        // camera at the hit radius times their size; the original's are the render module behind `0x10439d4`)
        if let Some(ring) = self.texture(overlay, gpu, vfs, "stadium/chal_01/fx/pass_ring") {
            for r in view.rings {
                let [x, y] = r.at;
                let k = r.radius;
                let c = r.colour.map(|v| (v * 0.5).min(1.0));
                out.push(quad(ring, 0x32, [x - k, y - k, x + k, y + k], [0.0, 1.0], c, fit));
            }
        }
        let (w, h) = (size.0.max(1) as f32, size.1.max(1) as f32);
        let frame = match view.challenge {
            Some((c, outcomes)) => {
                let values = challenge_values(self.challenge.get(), c, outcomes);
                self.challenge.get().evaluate([w, h], &values, false)
            }
            None => {
                let values = self.values(vfs, m, view);
                self.game.get().evaluate([w, h], &values, false)
            }
        };
        for d in &frame.draws {
            let Some(texture) = self.texture(overlay, gpu, vfs, &d.texture) else { continue };
            let [x0, y0] = fit.from_window([d.rect[0], d.rect[1]], (w, h));
            let [x1, y1] = fit.from_window([d.rect[2], d.rect[3]], (w, h));
            let [u0, v0, u1, v1] = d.uv;
            // the scene's colours leave the texture as it is at 1, the overlay's at 0.5
            let [r, g, b, a] = d.colour;
            let colour = [r * 0.5, g * 0.5, b * 0.5, a];
            let v = |x, y, u, v| OverlayVertex { pos: [x, y], uv: [u, v], colour };
            let corners = [v(x0, y1, u0, v1), v(x1, y1, u1, v1), v(x0, y0, u0, v0), v(x1, y0, u1, v0)];
            out.push(OverlayQuad { texture, blend: 2, layer: d.layer, corners });
        }
        out
    }
}

/// The challenge scene's values (**[I]** the layout, `challenge.ron`): a button per pass needed, by each pass's
/// outcome (`Challenge_button_empty`, `_red`, `_yellow`, `_green`), the clock in seconds, and the countdown.
fn challenge_values(scene: &Scene, c: &qwc_sim::challenge::Challenge, outcomes: &[u32]) -> Values {
    use qwc_sim::challenge::{needed, Phase, COUNTDOWN_TICKS};
    let mut v = Values::defaults(scene);
    let n = needed(c.kind).min(12) as usize;
    v.set("passes", Value::Scalar(n as f32));
    for i in 0..n {
        let name = match outcomes.get(i) {
            Some(3) => "Challenge_button_green",
            Some(2) => "Challenge_button_yellow",
            Some(_) => "Challenge_button_red",
            None => "Challenge_button_empty",
        };
        v.set(&format!("button.{i}"), Value::Text(name.into()));
    }
    v.set("clock", Value::Scalar((c.clock.max(0) / 24) as f32));
    v.set("counting", Value::Flag(c.phase == Phase::Countdown));
    v.set("countdown", Value::Scalar(((COUNTDOWN_TICKS - c.clock) / 24 + 1).clamp(1, 3) as f32));
    v
}
