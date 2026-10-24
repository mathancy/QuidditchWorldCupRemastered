//! A match drawn into a texture for a tool (the HUD editor's Looks mode): England and France, both the AI's,
//! playing in a stadium, drawn by the game's own world ([`crate::arena`]) from the match's camera or a free one,
//! in either look. Not the original's: the remaster's own.

use qwc_assets::Vfs;
use qwc_render::glam::Vec3;
use qwc_render::{wgpu, Camera, Gpu, Look, Renderer};
use qwc_sim::Match;

use crate::arena::{Arena, MatchSpec};

/// A camera circling a point: its heading round it and its height above it (degrees), its distance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Orbit {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub target: [f32; 3],
}

impl Default for Orbit {
    fn default() -> Orbit {
        Orbit { yaw: 30.0, pitch: 25.0, distance: 160.0, target: [0.0, 15.0, 0.0] }
    }
}

impl Orbit {
    pub fn camera(&self) -> Camera {
        let (yaw, pitch) = (self.yaw.to_radians(), self.pitch.to_radians());
        let dir = Vec3::new(yaw.sin() * pitch.cos(), pitch.sin(), -yaw.cos() * pitch.cos());
        let target = Vec3::from(self.target);
        Camera {
            eye: target + dir * self.distance,
            target,
            up: Vec3::Y,
            fov_y: 50f32.to_radians(),
            near: 0.5,
            far: 4000.0,
        }
    }
}

pub struct MatchPreview {
    pub spec: MatchSpec,
    m: Match,
    /// The renderer: its `look` and `settings` are the preview's to change.
    pub renderer: Renderer,
    arena: Arena,
    /// Match time, and the part of a tick since the last.
    time: f64,
    behind: f64,
    /// Whether the match plays on.
    pub playing: bool,
}

/// The game's tick, in seconds.
const TICK: f64 = 1.0 / 24.0;

impl MatchPreview {
    /// The match at the stadium in folder `stadium` (England against France), drawn into `format` targets in the
    /// remastered look with the stadium's look file (`crate::looks`), a few seconds played.
    pub fn new(gpu: &Gpu, vfs: &Vfs, stadium: &str, format: wgpu::TextureFormat) -> Result<MatchPreview, String> {
        if !vfs.exists(&format!("stadium/{stadium}/gfx/stadium.fpm")) {
            return Err(format!("stadium/{stadium}: no stadium"));
        }
        let team = |name: &str| {
            let want = format!("teams/{}/", name.to_ascii_lowercase());
            vfs.with_extension("fpm")
                .find(|p| p.to_ascii_lowercase().starts_with(&want))
                .and_then(|p| p.split('/').nth(1).map(str::to_string))
                .ok_or_else(|| format!("no team folder teams/{name}"))
        };
        let spec = MatchSpec {
            stadium: stadium.to_string(),
            hoops: "hoopnorm".into(),
            teams: [team("England")?, team("France")?],
            skill: 1,
            music: crate::audio::match_score(stadium).into(),
            sounds: None,
            desc: "quidd".into(),
        };
        let mut m = spec.start(vfs, 1);
        let mut renderer = Renderer::new(&gpu.device, &gpu.queue, format);
        renderer.shaders = crate::packs::shaders();
        renderer.look = Look::Remastered;
        renderer.settings = crate::looks::load(stadium);
        let mut arena = Arena::load(gpu, &mut renderer, vfs, &spec, &m);
        // into the play: past the kick-off
        for _ in 0..(6.0 / TICK) as u32 {
            m.step();
            arena.ticked(&m, m.tick as f64 * TICK);
        }
        let time = m.tick as f64 * TICK;
        Ok(MatchPreview { spec, m, renderer, arena, time, behind: 0.0, playing: true })
    }

    /// The stadiums on the disc (the folders of `stadium/` with a `gfx/stadium.fpm`).
    pub fn stadiums(vfs: &Vfs) -> Vec<String> {
        let mut out: Vec<String> = vfs
            .with_extension("fpm")
            .filter_map(|p| {
                let parts: Vec<&str> = p.split('/').collect();
                (parts.len() == 4
                    && parts[0].eq_ignore_ascii_case("stadium")
                    && parts[3].eq_ignore_ascii_case("stadium.fpm"))
                .then(|| parts[1].to_string())
            })
            .collect();
        out.sort_by_key(|s| s.to_ascii_lowercase());
        out.dedup();
        out
    }

    /// The match `seconds` on, while it plays.
    pub fn advance(&mut self, seconds: f64) {
        if !self.playing {
            return;
        }
        self.behind += seconds.min(0.25);
        while self.behind >= TICK {
            self.m.step();
            self.time = self.m.tick as f64 * TICK;
            self.arena.ticked(&self.m, self.time);
            self.behind -= TICK;
        }
    }

    /// Make the light probes again (after the settings that light them change: the sun, the baked light).
    pub fn relight(&mut self, gpu: &Gpu) {
        self.arena.bake_probes(gpu, &mut self.renderer, &self.m);
    }

    /// Draw the match into `view`/`depth` (`size` pixels) from the match's camera, or from `orbit`; answers the
    /// camera drawn from.
    pub fn draw(
        &mut self,
        gpu: &Gpu,
        view: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        size: (u32, u32),
        orbit: Option<&Orbit>,
    ) -> Camera {
        let alpha = self.behind / TICK;
        let camera = self.arena.frame(gpu, &self.renderer, &self.m, alpha, self.time + alpha * TICK);
        let camera = orbit.map_or(camera, Orbit::camera);
        self.arena.draw_world(gpu, &mut self.renderer, view, depth, size, &self.m, &camera, alpha);
        camera
    }
}
