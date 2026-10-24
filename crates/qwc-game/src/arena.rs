//! A match drawn in 3D: the stadium (`crate::stadium`), the fourteen flyers on their brooms, the Quaffle and the
//! Snitch, placed from `qwc_sim::world::Match` each tick and blended between ticks; the match camera
//! (`crate::matchcam`) runs each tick too, and its views are blended the same way; in the Seeker game the Seeker
//! camera does (`crate::seekercam`); with two human teams each has its view, on the two halves of the screen.
//!
//! The flyers' clips are their controller's (`crate::flyer`, `QFlyerAnimControl`): the flying position by speed and
//! bank, the hair, the broom's tail and the cape, and the match's events as event clips (the request types'
//! choices of `0x471790`, below).
//!
//! **[I]** until their code is traced, these are the port's own:
//! - which request each of the sim's events makes (the player actions `0x4228c0`-`0x427400` are not all traced),
//!   and the sides and quadrants they are taken by;
//! - the flyer's matrix: its forward from the sim (`+0x50`), upright (the handling's roll, `+0x174`, is not ported);
//! - the split screen's layout: team 0's view on the top half, team 1's on the bottom;
//! - the Quaffle in a holder's hands: at the holder, a little ahead;
//! - the hands and faces: the pose's morph weights (every layer's, the hair's and the tail's too);
//! - the Bludgers: `root/Balls/bludger.fpm` at the sim's Bludgers, unspun; the beaters bat with `na_bt_hitbg_*`
//!   (**[I]** which of the four the original picks is not traced), a hit player plays `na_kpch_gethit_*` by the
//!   side the Bludger came from.

use std::collections::HashMap;

use qwc_anim::control::SetClip;
use qwc_anim::{Mat4, Rig};
use qwc_assets::{TextureIndex, Vfs};
use qwc_formats::fas::Fas;
use qwc_render::glam::Vec3;
use qwc_render::{flags, Camera, Gpu, GpuModel, Renderer};
use qwc_sim::race::RaceEvent;
use qwc_sim::world::{Event, Role};
use qwc_sim::Match;

use crate::face::{self, Face};
use crate::flyer::{self, Clip, Tunings};
use crate::matchcam::View;
use crate::stadium;

/// A team's flyer models in their line-up order (`0x4342c0`: keeper, chasers, beaters, seeker).
const LINE_UP: [&str; 7] = ["keeper", "chaser1", "chaser2", "chaser3", "beater1", "beater2", "seeker"];

/// One common set's clips as the library binds them (`root/Anims/med_common.fas` or `sma_common.fas`).
struct ClipSet {
    clips: Vec<SetClip>,
}

impl ClipSet {
    fn load(vfs: &Vfs, path: &str) -> Option<ClipSet> {
        let bytes = vfs.read(path).ok()?;
        let fas = Fas::parse(&bytes).ok()?;
        let clips = qwc_anim::control::decode_set(&fas, None, None).map_err(|e| eprintln!("{path}: {e}")).ok()?;
        Some(ClipSet { clips })
    }
}

/// The Snitch: its skinned model (`root/Balls/snitch.fpm`, on `root/Anims/snitchanims.skl`), placed by its pose,
/// and its wings' flutter (`snitchanims.fas`'s `snitch_flutter`, looping; **[I]** the original's choice of clip in
/// the race is not traced).
struct SnitchModel {
    gpu: GpuModel,
    rig: Option<Rig>,
    flutter: Option<qwc_anim::Clip>,
}

impl SnitchModel {
    fn load(gpu: &Gpu, renderer: &mut Renderer, vfs: &Vfs, textures: &TextureIndex) -> Option<SnitchModel> {
        let path = "root/Balls/snitch.fpm";
        let md = crate::models::load(vfs, textures, path).map_err(|e| fail_soft(path, &e)).ok()?;
        let rig = md.skeleton.as_ref().map(|(_, s)| Rig::new(s.clone()));
        let gpu_model = renderer.upload_model(gpu, vfs, &md).map_err(|e| fail_soft(path, &e)).ok()?;
        let bytes = vfs.read("root/Anims/snitchanims.fas").ok();
        let flutter = bytes.as_deref().and_then(|b| Fas::parse(b).ok()).and_then(|fas| {
            let named = |a: &qwc_formats::fas::Anim| {
                a.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case("snitch_flutter"))
            };
            let i = fas.anims.iter().position(named)?;
            qwc_anim::Clip::decode(&fas, i, None).ok()
        });
        Some(SnitchModel { gpu: gpu_model, rig, flutter })
    }

    /// Placed at `pos` facing `fwd`, its wings at `time` seconds into their beat.
    fn place(&self, gpu: &Gpu, renderer: &Renderer, pos: Vec3, fwd: Vec3, time: f64) {
        let root = facing(pos.as_dvec3().to_array(), fwd.as_dvec3().to_array());
        match &self.rig {
            Some(rig) => {
                let t = self.flutter.as_ref().map_or(0.0, |c| (time * c.fps as f64).rem_euclid(c.frames.max(1) as f64));
                let skin: Vec<Mat4> =
                    rig.skin(self.flutter.as_ref(), t).iter().map(|s| qwc_formats::skl::matmul(s, &root)).collect();
                renderer.set_pose(&gpu.queue, &self.gpu, &skin);
            }
            None => renderer.set_transform(&gpu.queue, &self.gpu, Some(root.map(|v| v as f32))),
        }
    }
}

/// A flyer's model and its controller.
struct Flyer {
    gpu: GpuModel,
    rig: Option<Rig>,
    /// Its morphed meshes (the hands' grips, the face), moved by the pose's morph weights.
    face: Option<Face>,
    /// Its animation controller (skinned models).
    anim: Option<flyer::Flyer>,
}

/// Where everything stood at the end of a tick.
#[derive(Clone, Default)]
struct Snapshot {
    pos: Vec<[f64; 3]>,
    fwd: Vec<[f64; 3]>,
    quaffle: [f64; 3],
    snitch: Option<[f64; 3]>,
    bludgers: [[f64; 3]; 2],
}

impl Snapshot {
    fn of(m: &Match) -> Snapshot {
        Snapshot {
            pos: m.players.iter().map(|p| p.pos).collect(),
            fwd: m.players.iter().map(|p| p.fwd).collect(),
            quaffle: m.quaffle,
            snitch: m.race.as_ref().map(|r| r.snitch.pos),
            bludgers: [m.bludgers[0].pos, m.bludgers[1].pos],
        }
    }
}

fn lerp(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] {
    std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t)
}

/// A row-vector matrix facing `fwd` (the model's `+z`), upright, at `pos`.
fn facing(pos: [f64; 3], fwd: [f64; 3]) -> Mat4 {
    let z = Vec3::new(fwd[0] as f32, fwd[1] as f32, fwd[2] as f32).try_normalize().unwrap_or(Vec3::Z);
    let x = Vec3::Y.cross(z).try_normalize().unwrap_or(Vec3::X);
    let y = z.cross(x);
    let m =
        [x.x, x.y, x.z, 0.0, y.x, y.y, y.z, 0.0, z.x, z.y, z.z, 0.0, pos[0] as f32, pos[1] as f32, pos[2] as f32, 1.0];
    m.map(f64::from)
}

/// The light probes' grid over the pitch (the remastered lighting's): how many each way, how far past the hoops
/// it reaches across the ground, and how high.
pub(crate) const PROBES: [u32; 3] = [9, 3, 7];
const PROBE_MARGIN: f32 = 60.0;
pub(crate) const PROBE_TOP: f32 = 60.0;
/// The hoops' spread that makes a pitch: less (a challenge's hoops) and the grid covers the stadium instead.
const PROBE_PITCH: f32 = 50.0;

/// What a match is played with: the stadium (a folder of `stadium/`), its hoops' package (`stadium/hoopnorm`, or
/// the stadium's own: `hoopjpn`, `hoopqrd`, ...), the teams (folders of `teams/`) and the skill level (the teams'
/// `team_<skill>.cli`; **[I]** both teams take the same, DataStore `SkillLevel`).
#[derive(Debug, Clone, PartialEq)]
pub struct MatchSpec {
    pub stadium: String,
    pub hoops: String,
    pub teams: [String; 2],
    pub skill: u32,
    /// The score the match plays (a `qwc_audio::music::SCORES` directory).
    pub music: String,
    /// The stadium's sound folder from its load set (`Audio/Stadium/<name>`), if it gave one.
    pub sounds: Option<String>,
    /// The match description (a folder of `match/`: `quidd` for Quidditch, `cpass` ... `ccombo` for the challenges).
    pub desc: String,
}

impl MatchSpec {
    /// The match as the game sets it up (`crate::sim::new_match`), `rand()` from `seed`.
    pub fn start(&self, vfs: &Vfs, seed: u32) -> Match {
        let clis = self.teams.clone().map(|t| format!("teams/{t}/team_{}.cli", self.skill.min(3)));
        crate::sim::new_match(vfs, [&clis[0], &clis[1]], &self.stadium, &self.desc, seed).0
    }
}

/// The match's world on the GPU.
pub(crate) struct Arena {
    sky: Vec<GpuModel>,
    stadium: Vec<GpuModel>,
    /// The remastered scenery's surroundings (`crate::scenery`), drawn with the stadium; none for the disc's.
    surroundings: Vec<GpuModel>,
    /// How far the camera sees: the original's, or the scenery's (`StadiumConfig::far`).
    far: f32,
    /// The stadium's crowd box (`gfx/crowdbox.fpm`), the set its crowd cutscenes play in.
    crowdbox: Vec<GpuModel>,
    hoops: Vec<GpuModel>,
    flags: stadium::Flags,
    /// One per player, by the match's index (none where its model did not load).
    flyers: Vec<Option<Flyer>>,
    quaffle: Option<GpuModel>,
    snitch: Option<SnitchModel>,
    /// One model per Bludger (team 0's, team 1's).
    bludgers: [Option<GpuModel>; 2],
    previous: Snapshot,
    current: Snapshot,

    seeker_cam: Option<crate::seekercam::SeekerCam>,
    /// The ticks of each team's seeker's last tackle and barge, which shake the Seeker camera (`+0x1d0c`, `+0x1d08`).
    seeker_shakes: [(Option<u32>, Option<u32>); 2],
    /// The match's events already looked at.
    events_seen: usize,
    /// The effects (`crate::fx`), their pass and this frame's quads.
    fx: crate::fx::Fx,
    effects: qwc_render::effects::Effects,
    fx_quads: Vec<qwc_render::effects::EffectQuad>,
    /// The passing challenge's rings to draw this frame (`crate::fx::Fx::ring`): where, how large and their colour.
    pub rings: Vec<(Vec3, f32, [f32; 4])>,
    /// Each player's highlights last frame (`HIGHLIGHT_ACTIVE`, `_POSSESSION`, `_IN_GOAL_AREA`): their changes
    /// bring `STATE_TRANSITION`.
    fx_prev: Vec<[bool; 3]>,
    /// The hoop scored in, from the goal (`0x1043b60`: its `GOAL`, the world's `GOAL_SCORED`) to the kick-off.
    fx_goal: Option<usize>,
    /// The kick-off's window since the effects last ran (the world's `QUAFFLE_KICKED_OFF` for one step).
    fx_kickoff: bool,
    /// The camera's shots over the follow camera (`crate::shots`), and the follow camera's own direction last tick
    /// (the sticks steer by it).
    shots: crate::shots::Shots,
    ahead: Option<Vec3>,
    /// The camera stays where it is (a challenge's outro): its views are not stepped.
    pub hold_camera: bool,
    /// The match camera, the team it follows, and its views at the last two ticks' ends (blended between, as the
    /// models are).
    match_cam: crate::matchcam::MatchCamera,
    cam_team: usize,
    views: Option<(View, View)>,
    /// With two human teams, the second view of the split screen (team 1's; the first is team 0's).
    split: Option<Split>,
    /// The team whose player watches this screen, when the other human plays on another machine (online, `crate::online`):
    /// its view alone, without the split, and the effects' `TEAM0`.
    pub viewer: Option<usize>,
    /// The stadium's light set (`gfx/lights.cli`), which the flyers and the balls are lit by.
    lights: Option<qwc_render::Lights>,
    /// The match time the effects last ran to.
    fx_time: Option<f64>,
    /// The flyers' `AnimCtrl` tunings, and the match time their controllers have run to.
    tunings: Tunings,
    anim_time: Option<f64>,
}

impl Arena {
    /// The stadium, its hoops and its flags (waved to `time` seconds), without the match: what a cutscene plays in;
    /// for a crowd cutscene (`crowd_box`) the sky and the crowd box. **[I]** the original moves the box and its
    /// cutscene out of sight (`CCS.CrowdBoxOffset`, `StadiumCrowdBoxBaseRotnCode`); here the stadium is left out.
    pub fn scenery(&mut self, gpu: &Gpu, renderer: &Renderer, time: f64, crowd_box: bool) -> Vec<&GpuModel> {
        self.animate(gpu, renderer, time);
        if crowd_box && !self.crowdbox.is_empty() {
            return self.sky.iter().chain(&self.crowdbox).collect();
        }
        self.flags.wave(gpu, renderer, time);
        let scenery = self.sky.iter().chain(&self.stadium).chain(&self.surroundings);
        scenery.chain(&self.hoops).chain(self.flags.drawn()).collect()
    }

    /// How far the camera sees in this stadium (0: as the camera says).
    pub fn far(&self) -> f32 {
        self.far
    }

    /// The stadium's animated materials (the crowd's frames, the sky's scroll) at `time` seconds.
    fn animate(&mut self, gpu: &Gpu, renderer: &Renderer, time: f64) {
        let models = self.sky.iter_mut().chain(&mut self.stadium).chain(&mut self.crowdbox).chain(&mut self.hoops);
        stadium::animate(gpu, renderer, models, time);
    }

    /// Load the stadium and the teams of `spec` for match `m`.
    pub fn load(gpu: &Gpu, renderer: &mut Renderer, vfs: &Vfs, spec: &MatchSpec, m: &Match) -> Arena {
        let stadium_name = spec.stadium.as_str();
        let teams = [spec.teams[0].as_str(), spec.teams[1].as_str()];
        let textures = TextureIndex::build(vfs);
        let gfx = format!("stadium/{stadium_name}/gfx");
        let lights = stadium::lights(vfs, &gfx);
        let sky = stadium::models(gpu, renderer, vfs, &textures, &[&format!("{gfx}/sky.fpm")]);
        // the packs' remastered scenery for the stadium in place of its `stadium.fpm` (`crate::scenery`)
        let scenery = crate::scenery::for_stadium(gpu, renderer, vfs, stadium_name);
        let (stadium_models, surroundings, config) = match scenery {
            Some(s) => {
                crate::scenery::grow_sky(gpu, renderer, &sky, s.config.sky_scale);
                (s.arena, s.surroundings, Some(s.config))
            }
            None => {
                (stadium::models(gpu, renderer, vfs, &textures, &[&format!("{gfx}/stadium.fpm")]), Vec::new(), None)
            }
        };
        let crowdbox_path = format!("{gfx}/crowdbox.fpm");
        let crowdbox = if vfs.exists(&crowdbox_path) {
            stadium::models(gpu, renderer, vfs, &textures, &[&crowdbox_path])
        } else {
            Vec::new()
        };
        let places: Vec<[f32; 16]> =
            m.hoops.iter().map(|h| qwc_feck::place::translation(h.pos.map(|v| v as f32))).collect();
        let hoop_model = format!("stadium/{}/gfx/singlehoop.fpm", spec.hoops);
        let hoops = if config.is_none_or(|c| c.hoops) {
            stadium::hoops(gpu, renderer, vfs, &textures, &hoop_model, &places)
        } else {
            Vec::new()
        };
        // **[I]** in a match the `stadium` group is the stadium's own packages, the hoops' and `STADIUM_COMMON`; a
        // team's is its folder and its flags
        let stadium_group: Vec<String> = [
            gfx.clone(),
            format!("stadium/{stadium_name}/game"),
            "stadium/common/gfx".into(),
            "stadium/hoopnorm/gfx".into(),
        ]
        .into();
        let groups = |g| match g {
            flags::Group::Stadium => stadium_group.clone(),
            flags::Group::Team1 => vec![format!("teams/{}/flags", teams[0]), format!("teams/{}", teams[0])],
            flags::Group::Team2 => vec![format!("teams/{}/flags", teams[1]), format!("teams/{}", teams[1])],
        };
        let store = flags::Store { match_type: 1, in_world_cup_flow: false };
        let mut flags = stadium::Flags::load(gpu, renderer, vfs, &groups, &store, 0);
        if config.is_some_and(|c| !c.flags) {
            flags.models.clear();
        }
        let mut sets: Vec<ClipSet> = Vec::new();
        let mut set_of: HashMap<String, usize> = HashMap::new();
        let mut flyers = Vec::new();
        let tunings = Tunings::load(vfs);
        for (i, p) in m.players.iter().enumerate() {
            let t = &m.teams[p.team];
            let order = [t.keeper, t.chasers[0], t.chasers[1], t.chasers[2], t.beaters[0], t.beaters[1], t.seeker];
            let slot = order.iter().position(|&j| j == i).unwrap_or(6);
            let path = format!("teams/{}/{}.fpm", teams[p.team], LINE_UP[slot]);
            let loaded = crate::models::load(vfs, &textures, &path).and_then(|md| {
                let rig = md.skeleton.as_ref().map(|(_, s)| Rig::new(s.clone()));
                // the common set of the model's skeleton: `.../med_common.skl` plays `root/Anims/med_common.fas`
                let skel = md.skeleton.as_ref().map_or(String::new(), |(s, _)| s.to_ascii_lowercase());
                let common = if skel.contains("sma_common") { "sma_common" } else { "med_common" };
                let set_path = format!("root/Anims/{common}.fas");
                let set = match set_of.get(&set_path) {
                    Some(&i) => i,
                    None => {
                        let s = ClipSet::load(vfs, &set_path).ok_or_else(|| format!("{set_path}: no clips"))?;
                        sets.push(s);
                        set_of.insert(set_path, sets.len() - 1);
                        sets.len() - 1
                    }
                };
                let gpu_model = renderer.upload_model(gpu, vfs, &md)?;
                let face = Face::new(&md);
                let role = match p.role {
                    Role::Seeker => flyer::Role::Seeker,
                    Role::Beater => flyer::Role::Beater,
                    _ => flyer::Role::KeeperChaser,
                };
                let anim = rig.as_ref().map(|r| {
                    let mut pose = r.bind_pose();
                    // the `World` bone faces the flyer forward: every clip of either set that turns it gives it a
                    // half turn about `y`, and the seekers' hover, fly, crouch, catch and look clips leave it as it
                    // was. **[I]** it starts and rests (seeds the blends over those clips) there
                    if let Some(w) = r.skeleton.bones.iter().position(|b| b.name.eq_ignore_ascii_case("World")) {
                        pose.set_rest(w, [0.0, std::f32::consts::PI, 0.0]);
                    }
                    flyer::Flyer::new(sets[set].clips.clone(), pose, role, &tunings)
                });
                Ok(Flyer { gpu: gpu_model, rig, face, anim })
            });
            flyers.push(loaded.map_err(|e| fail_soft(&path, &e)).ok());
        }
        let mut ball = |path: &str| {
            let md = crate::models::load(vfs, &textures, path);
            md.and_then(|md| renderer.upload_model(gpu, vfs, &md)).map_err(|e| fail_soft(path, &e)).ok()
        };
        let quaffle = ball("root/Balls/quaffle.fpm");
        let bludgers = [ball("root/Balls/bludger.fpm"), ball("root/Balls/bludger.fpm")];
        let snitch = SnitchModel::load(gpu, renderer, vfs, &textures);
        let now = Snapshot::of(m);
        let mut fx = crate::fx::Fx::load(vfs, stadium_name, &spec.hoops);
        let mut effects = qwc_render::effects::Effects::new(&gpu.device, renderer.color_format);
        fx.preload(gpu, &mut effects, vfs);
        if std::env::var_os("QWC_FX_DEBUG").is_some() {
            let (g, d, t) = fx.counts();
            eprintln!("effects: {g} geometry, {d} particle emitters, {t} trails");
        }
        let arena = Arena {
            sky,
            stadium: stadium_models,
            surroundings,
            far: config.map_or(0.0, |c| c.far),
            crowdbox,
            hoops,
            flags,
            flyers,
            quaffle,
            snitch,
            bludgers,
            previous: now.clone(),
            current: now,
            seeker_cam: None,
            seeker_shakes: [(None, None); 2],
            events_seen: 0,
            tunings,
            anim_time: None,
            match_cam: Default::default(),
            cam_team: 0,
            views: None,
            split: None,
            viewer: None,
            lights,
            fx_time: None,
            fx,
            effects,
            fx_quads: Vec::new(),
            rings: Vec::new(),
            fx_prev: Vec::new(),
            fx_goal: None,
            fx_kickoff: false,
            shots: Default::default(),
            ahead: None,
            hold_camera: false,
        };
        arena.bake_probes(gpu, renderer, m);
        arena
    }

    /// The remastered lighting's light probes (`qwc_render::Renderer::bake_probes`; the remaster's own): a grid
    /// over the pitch, the hoops' box widened, seeing the sky, the stadium, the hoops and the flags in the
    /// stadium's sun.
    pub(crate) fn bake_probes(&self, gpu: &Gpu, renderer: &mut Renderer, m: &Match) {
        if renderer.look != qwc_render::Look::Remastered {
            return;
        }
        if let Some(l) = self.lights {
            renderer.lights = l;
        }
        let pos = m.hoops.iter().map(|h| h.pos.map(|v| v as f32));
        let (lo, hi) = pos.fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), p| {
            (std::array::from_fn(|i| lo[i].min(p[i])), std::array::from_fn(|i| hi[i].max(p[i])))
        });
        let (lo, hi) = if hi[0] - lo[0] > PROBE_PITCH || hi[2] - lo[2] > PROBE_PITCH {
            ([lo[0] - PROBE_MARGIN, 1.0, lo[2] - PROBE_MARGIN], [hi[0] + PROBE_MARGIN, PROBE_TOP, hi[2] + PROBE_MARGIN])
        } else {
            // no pitch between hoops (a challenge's): over the stadium instead
            let models: Vec<&GpuModel> = self.stadium.iter().collect();
            let Some(b) = stadium::probe_box(&models) else { return };
            b
        };
        let scenery = self.sky.iter().chain(&self.stadium).chain(&self.surroundings);
        let models: Vec<&GpuModel> = scenery.chain(&self.hoops).chain(self.flags.drawn()).collect();
        let start = std::time::Instant::now();
        renderer.bake_probes(gpu, &models, lo, hi, PROBES);
        eprintln!(
            "light probes: {} over {lo:?}..{hi:?} in {:.2}s",
            PROBES.iter().product::<u32>(),
            start.elapsed().as_secs_f32()
        );
    }

    /// After each tick of `m` (at match time `time` seconds): keep the positions to blend from, run each flyer's
    /// glue (`0x465a10`) and send the tick's events to the flyers as event clips.
    pub fn ticked(&mut self, m: &Match, time: f64) {
        // with no frames drawn between ticks (a headless run), the controllers keep up here
        let headless = time - *self.anim_time.get_or_insert(time - 1.0 / 24.0) > 2.0 / 24.0;
        if headless {
            self.advance(time - 1.0 / 24.0);
            // the effects too, a tick at a time
            let q = m.quaffle;
            let actors = self.fx_actors(m, 1.0, Vec3::new(q[0] as f32, q[1] as f32, q[2] as f32));
            self.fx.update(1.0 / 24.0, &actors);
        }
        self.previous = std::mem::replace(&mut self.current, Snapshot::of(m));
        for (i, p) in m.players.iter().enumerate() {
            let Some(Some(Flyer { anim: Some(a), .. })) = self.flyers.get_mut(i) else { continue };
            let right = [p.fwd[2], 0.0, -p.fwd[0]];
            let boosting = p.role == Role::Seeker && m.race.as_ref().is_some_and(|r| r.seekers[p.team].boosting);
            a.tick(p.vel, right, p.fwd, m.holder == Some(i), boosting, &self.tunings);
        }
        let start = self.events_seen.min(m.events.len());
        // a catch is also a touch: each player's same clips once a tick
        let mut sent: Vec<(usize, String)> = Vec::new();
        for e in &m.events[start..] {
            match *e {
                Event::Goal { tick, hoop, .. } => {
                    if std::env::var_os("QWC_FX_DEBUG").is_some() {
                        eprintln!("effects: tick {tick} goal in hoop {hoop}");
                    }
                    self.fx_goal = Some(hoop);
                    let h = m.hoops[hoop].pos;
                    self.fx.goal(Vec3::new(h[0] as f32, h[1] as f32, h[2] as f32));
                }
                Event::KickOff { tick, .. } => {
                    if std::env::var_os("QWC_FX_DEBUG").is_some() {
                        eprintln!("effects: tick {tick} kick-off");
                    }
                    self.fx_goal = None;
                }
                // the kick-off clock's window (`0x1043a60` with 1, `0x4b5f40`): the green ring and sphere at the
                // Quaffle (`Kickoff_geofx.got`)
                Event::KickOffWindow { .. } => self.fx_kickoff = true,
                Event::Race { tick, event: RaceEvent::Hit { team } } => self.seeker_shakes[team].0 = Some(tick),
                Event::Race { tick, event: RaceEvent::Attack { team } } => self.seeker_shakes[team].1 = Some(tick),
                _ => {}
            }
            for (player, clips) in requests(m, &self.previous, e) {
                let key = (player, clips.first().map_or(String::new(), |c| c.name.clone()));
                if sent.contains(&key) {
                    continue;
                }
                sent.push(key);
                if let Some(Some(Flyer { anim: Some(a), .. })) = self.flyers.get_mut(player) {
                    // `QWC_FLYER_DEBUG`: each event's clips
                    if std::env::var_os("QWC_FLYER_DEBUG").is_some() {
                        let names: Vec<&str> = clips.iter().map(|c| c.name.as_str()).collect();
                        eprintln!("t {time:.2} player {player}: {names:?}");
                    }
                    a.request(clips, &self.tunings);
                }
            }
        }
        self.events_seen = m.events.len();
        // the shots (`crate::shots`): the kick-off's side view, the goal camera; their cuts are the follow cameras'
        if self.shots.tick(m) {
            self.match_cam.cut();
            if let Some(s) = self.split.as_mut() {
                s.match_cam.cut();
            }
        }
        self.step_camera(m);
    }

    /// The follow camera's direction (the camera's shots aside): the way the sticks are turned.
    pub fn ahead(&self) -> Option<Vec3> {
        self.ahead
    }

    /// Run the flyers' controllers to match time `to`, a 24th of a second at most at a time.
    fn advance(&mut self, to: f64) {
        let from = *self.anim_time.get_or_insert(to);
        let mut t = from;
        while t < to {
            let dt = (to - t).min(1.0 / 24.0);
            for a in self.flyers.iter_mut().flatten().filter_map(|f| f.anim.as_mut()) {
                a.update(dt as f32, &self.tunings, &mut Vec::new());
            }
            t += dt;
        }
        self.anim_time = Some(to.max(from));
    }

    /// Pose and place everything `alpha` of the way from the last tick to this one, at match time `time`; answers the
    /// camera.
    pub fn frame(&mut self, gpu: &Gpu, renderer: &Renderer, m: &Match, alpha: f64, time: f64) -> Camera {
        self.flags.wave(gpu, renderer, time);
        self.animate(gpu, renderer, time);
        let alpha = alpha.clamp(0.0, 1.0);
        self.advance(time);
        // the holder's right hand, where the Quaffle is held (`0x472780`)
        let mut hand = None;
        for i in 0..self.flyers.len().min(m.players.len()) {
            let pos = lerp(self.previous.pos.get(i).copied().unwrap_or(m.players[i].pos), self.current.pos[i], alpha);
            let fwd = lerp(self.previous.fwd.get(i).copied().unwrap_or(m.players[i].fwd), self.current.fwd[i], alpha);
            let Some(f) = &self.flyers[i] else { continue };
            let root = facing(pos, fwd);
            match (&f.rig, &f.anim) {
                (Some(rig), Some(a)) => {
                    let local = match a.control.pose.as_ref() {
                        Some(p) => rig.pose_local(p),
                        None => rig.local_pose(None, 0.0),
                    };
                    let world = rig.world_pose(&local);
                    let skin: Vec<Mat4> =
                        rig.skin_matrices(&world).iter().map(|s| qwc_formats::skl::matmul(s, &root)).collect();
                    if m.holder == Some(i) {
                        let node = rig.skeleton.bones.iter().position(|b| b.name.eq_ignore_ascii_case("RightHand"));
                        if let Some(w) = node.and_then(|n| world.get(n)) {
                            let k = self.tunings.coord_scale as f64;
                            let at = flyer::QUAFFLE_IN_HAND.map(|v| v as f64 * k);
                            let model = qwc_formats::skl::transform(w, at);
                            hand = Some(qwc_formats::skl::transform(&root, model));
                        }
                    }
                    renderer.set_pose(&gpu.queue, &f.gpu, &skin);
                    // the pose's morph weights (the common sets' nodes are the model skeleton's)
                    if let (Some(face), Some(pose)) = (&f.face, a.control.pose.as_ref()) {
                        let mut weights = Vec::new();
                        for (node, ws) in rig.skeleton.bones.iter().zip(&pose.morphs) {
                            for (k, &w) in ws.iter().enumerate() {
                                if let Some(c) = face.channel(&node.name, k as u16) {
                                    face::set(&mut weights, c, w);
                                }
                            }
                        }
                        face.apply(&gpu.queue, renderer, &f.gpu, &weights);
                    }
                }
                (Some(rig), None) => {
                    let skin: Vec<Mat4> =
                        rig.skin(None, 0.0).iter().map(|s| qwc_formats::skl::matmul(s, &root)).collect();
                    renderer.set_pose(&gpu.queue, &f.gpu, &skin);
                }
                (None, _) => renderer.set_transform(&gpu.queue, &f.gpu, Some(root.map(|v| v as f32))),
            }
        }
        let mut quaffle = lerp(self.previous.quaffle, self.current.quaffle, alpha);
        if let Some(h) = m.holder {
            // in the holder's right hand; **[I]** without a posed model, a little ahead of him at chest height
            quaffle = hand.unwrap_or_else(|| {
                let fwd = m.players[h].fwd;
                [quaffle[0] + fwd[0] * 0.6, quaffle[1] + 0.3, quaffle[2] + fwd[2] * 0.6]
            });
        }
        if let Some(q) = &self.quaffle {
            renderer.set_transform(&gpu.queue, q, Some(qwc_feck::place::translation(quaffle.map(|v| v as f32))));
        }
        let snitch_at = self.snitch_position(m, alpha);
        if let (Some(s), Some(p), Some(r)) = (&self.snitch, snitch_at, m.race.as_ref()) {
            let f = r.snitch.facing;
            s.place(gpu, renderer, p, Vec3::new(f[0] as f32, f[1] as f32, f[2] as f32), time);
        }
        for (b, model) in self.bludgers.iter().enumerate() {
            if let Some(g) = model {
                let p = lerp(self.previous.bludgers[b], self.current.bludgers[b], alpha);
                renderer.set_transform(&gpu.queue, g, Some(qwc_feck::place::translation(p.map(|v| v as f32))));
            }
        }
        // `QWC_CAM_CLOSE`: a close look at the holder from his side (checks)
        if let (Some(h), true) = (m.holder, std::env::var_os("QWC_CAM_CLOSE").is_some()) {
            let p = self.position(m, h, alpha);
            let f = m.players[h].fwd;
            let at = Vec3::new(p[0] as f32, p[1] as f32 + 0.6, p[2] as f32);
            let side = Vec3::new(f[2] as f32, 0.0, -f[0] as f32);
            let eye = at + side * 2.0 + Vec3::new(f[0] as f32, 0.4, f[2] as f32) * 2.6;
            return Camera { eye, target: at, up: Vec3::Y, fov_y: 50f32.to_radians(), near: 0.1, far: 2000.0 };
        }
        let ball = Vec3::new(quaffle[0] as f32, quaffle[1] as f32, quaffle[2] as f32);
        let dt = self.fx_time.map_or(0.0, |t| (time - t) as f32);
        self.fx_time = Some(time);
        // the camera's views at the last two ticks, blended as the models are
        if self.views.is_none() {
            self.step_camera(m);
        }
        let camera = self.views.map_or_else(
            || View { eye: Vec3::ZERO, dir: Vec3::Z, up: Vec3::Y, fov: 70.0 },
            |(prev, now)| now.blend(&prev, alpha as f32),
        );
        let camera = camera.camera();
        let actors = self.fx_actors(m, alpha, ball);
        self.fx.update(dt, &actors);
        self.fx_quads = self.fx.quads(&camera, &actors);
        for &(p, size, colour) in &self.rings {
            self.fx.ring(&mut self.fx_quads, &camera, p, size, colour);
        }
        if let Some(s) = self.split.as_mut() {
            let second = s.views.map_or(camera, |(prev, now)| now.blend(&prev, alpha as f32).camera());
            s.fx_quads = self.fx.quads(&second, &actors);
            for &(p, size, colour) in &self.rings {
                self.fx.ring(&mut s.fx_quads, &second, p, size, colour);
            }
        }
        camera
    }

    /// The split screen's second camera after [`Arena::frame`], when two humans play.
    fn split_camera(&self, alpha: f64) -> Option<Camera> {
        let s = self.split.as_ref()?;
        s.views.map(|(prev, now)| now.blend(&prev, alpha as f32).camera())
    }

    /// The world and its effects for the last [`Arena::frame`] (whose camera is `camera`) on a `size` target: the
    /// whole of it, or with two human teams the top half for team 0's view and the bottom half for team 1's
    /// (**[I]** the original's split is not traced). Each half keeps the horizontal field of view a whole screen
    /// would have.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_world(
        &self,
        gpu: &Gpu,
        renderer: &mut Renderer,
        color: &qwc_render::wgpu::TextureView,
        depth: &qwc_render::wgpu::TextureView,
        size: (u32, u32),
        m: &Match,
        camera: &Camera,
        alpha: f64,
    ) {
        if let Some(l) = self.lights {
            renderer.lights = l;
        }
        let models = self.drawn(m);
        // the scenery's reach (the original's 2000 is the camera's own)
        let camera = &Camera { far: camera.far.max(self.far), ..*camera };
        let Some(second) = self.split_camera(alpha) else {
            renderer.draw(gpu, color, depth, size, camera, &models);
            self.effects.draw(gpu, color, depth, size, camera, &self.fx_quads);
            return;
        };
        let half = size.1 / 2;
        // the vertical field of view that keeps a whole screen's horizontal one on a half as tall
        let narrow = |c: &Camera| Camera { fov_y: 2.0 * ((c.fov_y * 0.5).tan() * 0.5).atan(), ..*c };
        let views = [(narrow(camera), [0, 0, size.0, half]), (narrow(&second), [0, half, size.0, size.1 - half])];
        let quads = [&self.fx_quads, self.split.as_ref().map_or(&self.fx_quads, |s| &s.fx_quads)];
        for (k, (cam, rect)) in views.iter().enumerate() {
            renderer.draw_rect(gpu, color, depth, *rect, k == 0, cam, &models);
            self.effects.draw_rect(gpu, color, depth, *rect, cam, quads[k]);
        }
    }

    /// The actors the effects hang on and their triggers this frame, as the effects' trigger setters set them
    /// (`RE_NOTES.md`, "The match's effects"). **[T]**:
    /// - `ACTIVE` on the player a team acts through (the message on `0x1043b8c`, handler `0x4b63e0`), with
    ///   `IN_GOAL_AREA` when he is in the goal area; `POSSESSION` on the holder (`0x1043b70`, `0x4b6590`);
    ///   `PASS_TARGET` on the receiver the pass aim has in its cone (`0x1043b54`, `0x4b5cb0`); `BOOST` on a
    ///   boosting player (the players' sync `0x1043978`, `0x4b6bc0`);
    /// - each frame (`0x4b5220`): an active player without the Quaffle has `HIGHLIGHT_ACTIVE`, with it
    ///   `HIGHLIGHT_POSSESSION`, or `HIGHLIGHT_IN_GOAL_AREA` in the goal area; `STATE_TRANSITION` comes back on
    ///   when his possession changes or, holding it, his being in the goal area does; `STATE_TRANSITION_DEFENSE`
    ///   when a human's player becomes active without the Quaffle on his side; `NORMAL` on a player with none of
    ///   `SEEKING`, `ACTIVE`, `BOOST`, `TELEPORT` and `TRAIL_CUT`;
    /// - the balls' sync (`0x104398c`, `0x4b6a30`): a Bludger `ATTACKING` in its attack, else `NORMAL`; the
    ///   world's `QUAFFLE_NORMALTRAIL`, or `QUAFFLE_SUPERTRAIL` while a powered throw flies; `ALWAYS_ON`
    ///   (`0x4b4f10`);
    /// - the kick-off's `QUAFFLE_KICKED_OFF` (the world's slot 8, `0x1043a60`, `0x4b5f40`);
    /// - the hoops: `TARGET` instead of `GLOW` on the hoop a shot aims at (`0x1043ab4`, `0x4b6c90`); from a goal
    ///   (`0x1043b60`, `0x4b62a0`) `GOAL` on the hoop scored in and the world's `GOAL_SCORED`.
    ///
    /// **[I]**: the effects' `TEAM0` is the first human team (else the first team); an AI team's active player is
    /// its carrier, else its chaser nearest the Quaffle; the Seeker game's `SEEKING` is the race's; a Bludger
    /// attacks while it flies; a powered throw is a flat one; without a shot aimed, the hoop a carrier in the goal
    /// area faces most is `TARGET` (the human's attack state posts one, `0x446738`, not traced); `GOAL` lasts to
    /// the kick-off.
    fn fx_actors(&mut self, m: &Match, alpha: f64, ball: Vec3) -> Vec<crate::fx::ActorState> {
        use crate::fx::{Actor, ActorState};
        let v3 = |a: [f64; 3]| Vec3::new(a[0] as f32, a[1] as f32, a[2] as f32);
        let first = self.viewer.or(m.teams.iter().position(|t| t.human)).unwrap_or(0);
        let fx_team = |t: usize| usize::from(t != first);
        self.fx_prev.resize(m.players.len(), [false; 3]);
        let seeking = m.race.is_some();
        let mut out = Vec::new();
        let mut target_hoop = None;
        for (t, team) in m.teams.iter().enumerate() {
            let carrier = m.holder.filter(|&h| m.players[h].team == t);
            let active = team.active.or(carrier).or_else(|| m.chaser_nearest(t, m.quaffle));
            let roles: [(usize, &'static str); 7] = [
                (team.keeper, "KEEPER"),
                (team.chasers[0], "CHASER1"),
                (team.chasers[1], "CHASER2"),
                (team.chasers[2], "CHASER3"),
                (team.beaters[0], "BEATER1"),
                (team.beaters[1], "BEATER2"),
                (team.seeker, "SEEKER"),
            ];
            for (i, role) in roles {
                let p = &m.players[i];
                if !p.present {
                    continue;
                }
                let mut triggers = Vec::new();
                // the Seeker game turns `ACTIVE` (and `NORMAL`) off on every player (`0x4b6d40`), and with it the
                // highlights and the players' trails
                let is_active = active == Some(i) && !seeking;
                let possession = m.holder == Some(i);
                let in_goal = is_active && m.in_attacking_goal_area(i);
                let boosting = (p.role == Role::Seeker && m.race.as_ref().is_some_and(|r| r.seekers[t].boosting))
                    || (is_active && team.human && team.boost_hold > 0);
                if is_active {
                    triggers.push("ACTIVE");
                }
                if in_goal {
                    triggers.push("IN_GOAL_AREA");
                }
                if possession {
                    triggers.push("POSSESSION");
                }
                if team.pass_target == Some(i) {
                    triggers.push("PASS_TARGET");
                }
                if boosting {
                    triggers.push("BOOST");
                }
                if seeking {
                    triggers.push("SEEKING");
                }
                if !(seeking || is_active || boosting) {
                    triggers.push("NORMAL");
                }
                // the highlights from last frame's and this frame's states (`0x4b5220`)
                let [was_active, was_possession, was_goal] = self.fx_prev[i];
                let now = if is_active {
                    let transition = (possession != was_possession && !was_goal) || (in_goal != was_goal && possession);
                    if transition {
                        triggers.push("STATE_TRANSITION");
                    }
                    let theirs = m.holder.is_some_and(|h| m.players[h].team != t) || m.holder.is_none();
                    if !was_active && !possession && !was_possession && team.human && theirs {
                        triggers.push("STATE_TRANSITION_DEFENSE");
                    }
                    [!possession, possession && !in_goal, possession && in_goal]
                } else {
                    [false; 3]
                };
                self.fx_prev[i] = now;
                for (on, name) in
                    now.into_iter().zip(["HIGHLIGHT_ACTIVE", "HIGHLIGHT_POSSESSION", "HIGHLIGHT_IN_GOAL_AREA"])
                {
                    if on {
                        triggers.push(name);
                    }
                }
                if possession && in_goal {
                    // the hoop he faces most
                    let fwd = v3(p.fwd);
                    target_hoop = (m.hoops.iter().enumerate())
                        .filter(|(_, h)| h.team != t)
                        .map(|(k, h)| (k, (v3(h.pos) - v3(p.pos)).normalize_or_zero().dot(fwd)))
                        .max_by(|a, b| a.1.total_cmp(&b.1))
                        .map(|(k, _)| k);
                }
                let pos = self.position(m, i, alpha);
                if std::env::var_os("QWC_FX_TRIGGERS").is_some() {
                    eprintln!("effects: tick {} player {i} {triggers:?}", m.tick);
                }
                out.push(ActorState {
                    actor: Actor::Player(i),
                    team: fx_team(t),
                    role,
                    pos: v3(pos),
                    vel: v3(p.vel),
                    fwd: v3(p.fwd),
                    triggers,
                    glow: 0.0,
                    held: false,
                });
            }
        }
        // the hoops (`0x4b6c90`, `0x4b62a0`): the targeted one `TARGET` instead of `GLOW`, the one scored in `GOAL`
        let target_hoop = m.targeted_hoop.or(target_hoop);
        for (k, h) in m.hoops.iter().enumerate() {
            let triggers = if self.fx_goal == Some(k) {
                vec!["GOAL"]
            } else if target_hoop == Some(k) {
                vec!["TARGET"]
            } else {
                vec!["GLOW"]
            };
            out.push(ActorState {
                actor: Actor::Hoop(k),
                team: h.team,
                role: "",
                pos: v3(h.pos),
                vel: Vec3::ZERO,
                // the world's frame: `HOOP_GLOW`'s `ROT_BASE` turns its rect into the hoop's plane
                fwd: Vec3::Z,
                triggers,
                glow: 0.0,
                held: false,
            });
        }
        for (b, bl) in m.bludgers.iter().enumerate().filter(|(_, b)| b.present) {
            let attacking = bl.state == qwc_sim::bludger::State::Flying;
            out.push(ActorState {
                actor: Actor::Bludger(b),
                team: b,
                role: "",
                pos: v3(lerp(self.previous.bludgers[b], self.current.bludgers[b], alpha)),
                vel: v3(bl.vel),
                fwd: v3(bl.vel).try_normalize().unwrap_or(Vec3::Z),
                triggers: vec![if attacking { "ATTACKING" } else { "NORMAL" }],
                glow: 0.0,
                held: false,
            });
        }
        out.push(ActorState {
            actor: Actor::Quaffle,
            team: 0,
            role: "",
            pos: ball,
            vel: v3(m.quaffle_vel),
            fwd: v3(m.quaffle_vel).try_normalize().unwrap_or(Vec3::Z),
            triggers: vec![],
            glow: m.quaffle_glow as f32,
            held: m.holder.is_some(),
        });
        // the Snitch where it is drawn, its trail and sparkles on while seeking (the balls' sync `0x4b6a30`)
        if let (Some(r), Some(at)) = (&m.race, self.snitch_position(m, alpha)) {
            let vel = v3(r.snitch.vel);
            out.push(ActorState {
                actor: Actor::Snitch,
                team: 0,
                role: "",
                pos: at,
                vel,
                fwd: v3(r.snitch.facing).try_normalize().unwrap_or(Vec3::Z),
                triggers: vec![],
                glow: 0.0,
                held: false,
            });
        }
        let powered = m.holder.is_none() && m.flight.as_ref().is_some_and(|f| f.flat);
        out.push(ActorState {
            actor: Actor::World,
            team: 0,
            role: "",
            pos: Vec3::ZERO,
            vel: Vec3::ZERO,
            fwd: Vec3::Z,
            triggers: [Some("ALWAYS_ON"), Some(if powered { "QUAFFLE_SUPERTRAIL" } else { "QUAFFLE_NORMALTRAIL" })]
                .into_iter()
                .chain([
                    self.fx_goal.map(|_| "GOAL_SCORED"),
                    std::mem::take(&mut self.fx_kickoff).then_some("QUAFFLE_KICKED_OFF"),
                    m.race.is_some().then_some("SNITCH_TRAIL"),
                ])
                .flatten()
                .collect(),
            glow: 0.0,
            held: false,
        });
        out
    }

    /// A tick of the camera on match `m` as it stands: the match camera (`crate::matchcam`, the original's) for the
    /// first human team (else the holder's, else the first); in the Seeker game, **[I]** behind and above the
    /// player's seeker (the first human team's, else the first team's), looking ahead of it.
    fn step_camera(&mut self, m: &Match) {
        if self.hold_camera {
            if let Some((_, now)) = self.views {
                self.views = Some((now, now));
                if let Some(s) = &mut self.split {
                    s.views = s.views.map(|(_, now)| (now, now));
                }
                return;
            }
        }
        // two human teams split the screen (`NumberOfPlayers` 2; the camera system keeps a view per team), unless the
        // other plays on another machine
        let split = m.teams[0].human && m.teams[1].human && self.viewer.is_none();
        // the human's team; with none (both teams the AI's), the last holder's; `QWC_CAM_TEAM`: that team's (checks)
        let holder_team = m.holder.map(|h| m.players[h].team);
        let forced = std::env::var("QWC_CAM_TEAM").ok().and_then(|t| t.parse::<usize>().ok()).filter(|&t| t < 2);
        let human = self.viewer.or(m.teams.iter().position(|t| t.human));
        self.cam_team = if split { 0 } else { forced.or(human).or(holder_team).unwrap_or(self.cam_team) };
        // in the Seeker game the player's seeker (the first human team's, else the first team's)
        let seeker_team = if split { 0 } else { human.unwrap_or(0) };
        let (cam_team, shakes) = (self.cam_team, self.seeker_shakes);
        let follow =
            step_view(m, cam_team, seeker_team, &mut self.match_cam, &mut self.seeker_cam, self.views, &shakes);
        self.ahead = Some(follow.dir);
        let view = self.shots.view(m, cam_team, follow);
        self.views = Some((self.views.map_or(view, |(_, now)| now), view));
        if split {
            let s = self.split.get_or_insert_with(Split::default);
            let follow = step_view(m, 1, 1, &mut s.match_cam, &mut s.seeker_cam, s.views, &shakes);
            let view = self.shots.view(m, 1, follow);
            s.views = Some((s.views.map_or(view, |(_, now)| now), view));
        } else {
            self.split = None;
        }
    }

    /// Where the Snitch is drawn `alpha` of the way from the last tick to this one ([`snitch_drawn`]), its effects
    /// with it: none outside the race.
    fn snitch_position(&self, m: &Match, alpha: f64) -> Option<Vec3> {
        let now = self.current.snitch?;
        let p = lerp(self.previous.snitch.unwrap_or(now), now, alpha);
        let p = Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32);
        let age = m.race.as_ref().map_or(f32::MAX, |r| (r.ticks as f32 + alpha as f32) / 24.0);
        Some(match self.views {
            Some((prev, now)) => snitch_drawn(p, age, &now.blend(&prev, alpha as f32)),
            None => p,
        })
    }

    /// Player `i`'s position `alpha` of the way from the last tick to this one.
    pub fn position(&self, m: &Match, i: usize, alpha: f64) -> [f64; 3] {
        let at = |s: &Snapshot| s.pos.get(i).copied().unwrap_or(m.players[i].pos);
        lerp(at(&self.previous), at(&self.current), alpha.clamp(0.0, 1.0))
    }

    /// Everything to draw.
    pub fn drawn(&self, m: &Match) -> Vec<&GpuModel> {
        let scenery = self.sky.iter().chain(&self.stadium).chain(&self.surroundings);
        let mut out: Vec<&GpuModel> = scenery.chain(&self.hoops).chain(self.flags.drawn()).collect();
        // the players this game has (a challenge's description leaves the others out)
        let present = |i: usize| m.players.get(i).is_none_or(|p| p.present);
        out.extend(
            self.flyers.iter().enumerate().filter(|&(i, _)| present(i)).filter_map(|(_, f)| f.as_ref()).map(|f| &f.gpu),
        );
        if m.quaffle_present {
            out.extend(&self.quaffle);
        }
        if m.race.is_some() {
            out.extend(self.snitch.as_ref().map(|s| &s.gpu));
        }
        for (b, model) in self.bludgers.iter().enumerate() {
            if m.bludgers[b].present {
                out.extend(model);
            }
        }
        out
    }
}

/// The split screen's second view (the other human team's): its cameras, its views at the last two ticks' ends and
/// its effects' quads.
#[derive(Default)]
struct Split {
    match_cam: crate::matchcam::MatchCamera,
    seeker_cam: Option<crate::seekercam::SeekerCam>,
    views: Option<(View, View)>,
    fx_quads: Vec<qwc_render::effects::EffectQuad>,
}

/// One tick of a view: the match camera for `team`, or in the Seeker game the Seeker camera on `seeker_team`'s
/// seeker (`views` the last ones, for its starting field of view; `shakes` each team's last tackle and barge).
fn step_view(
    m: &Match,
    team: usize,
    seeker_team: usize,
    match_cam: &mut crate::matchcam::MatchCamera,
    seeker_cam: &mut Option<crate::seekercam::SeekerCam>,
    views: Option<(View, View)>,
    shakes: &[(Option<u32>, Option<u32>); 2],
) -> View {
    let Some(r) = &m.race else {
        *seeker_cam = None;
        return match_cam.step(&crate::matchcam::Inputs::of(m, team));
    };
    let t = seeker_team;
    // the Seeker game's start (the race's starting hold): the camera stays where the match left it while play goes
    // on (**[I]** the original's Snitch cutscene is not played; the user's account of its start, 2026-10-21)
    if r.seekers[t].intro > 0 {
        if let Some((_, now)) = views {
            *seeker_cam = None;
            return now;
        }
    }
    let fov = views.map_or(70.0, |(_, v)| v.fov);
    let cam = seeker_cam.get_or_insert_with(|| crate::seekercam::SeekerCam::new(fov));
    let s = &r.seekers[t];
    let since = |at: Option<u32>| at.map(|a| m.tick.saturating_sub(a));
    let view = cam.step(&crate::seekercam::Inputs {
        pos: crate::matchcam::v3(s.pos),
        vel: crate::matchcam::v3(s.vel),
        fwd: crate::matchcam::v3(s.heading),
        reach: crate::matchcam::v3(s.vel).length(),
        boosting: s.boosting,
        human: m.teams[t].human,
        // **[I]** a human's view widens too: the original widens team 0's, and the port's player may be team 1
        team0: t == 0 || m.teams[t].human,
        since_hit: since(shakes[t].0),
        since_barge: since(shakes[t].1),
        tick: m.tick,
    });
    if std::env::var_os("QWC_CAM_DEBUG").is_some() {
        eprintln!(
            "seeker view {t}: seeker {:?} vel {:?} heading {:?} snitch {:?} eye {:?} dir {:?} up {:?} fov {}",
            s.pos, s.vel, s.heading, r.snitch.pos, view.eye, view.dir, view.up, view.fov
        );
    }
    view
}

/// The Seeker game's start, after the user's account of the original (2026-10-21; **[I]**, not traced): the
/// Snitch leaves the HUD's top bar for the world, hanging where the bar's Snitch was in front of the held camera
/// ([`SNITCH_HANG`] seconds), then flies to its place in the race by the end of the race's starting hold.
const SNITCH_HANG: f32 = 2.2;
/// The race's starting hold in seconds (`qwc_sim::race::INTRO_TICKS`).
const RACE_HOLD: f32 = qwc_sim::race::INTRO_TICKS as f32 / 24.0;
/// How far in front of the camera the Snitch hangs, and how high up the screen (the top bar's middle, 0 the
/// screen's middle and 1 its top edge).
const SNITCH_DEPTH: f32 = 2.0;
const SNITCH_HIGH: f32 = 0.74;

/// Where the Snitch is drawn, `age` seconds into the race (past the hold, where the race has it: `at`).
fn snitch_drawn(at: Vec3, age: f32, view: &View) -> Vec3 {
    if age >= RACE_HOLD {
        return at;
    }
    let cam = view.camera();
    let dir = (cam.target - cam.eye).normalize_or(Vec3::Z);
    let up = (cam.up - dir * cam.up.dot(dir)).normalize_or(Vec3::Y);
    let hang = cam.eye + dir * SNITCH_DEPTH + up * (SNITCH_DEPTH * (cam.fov_y * 0.5).tan() * SNITCH_HIGH);
    let f = ((age - SNITCH_HANG) / (RACE_HOLD - SNITCH_HANG)).clamp(0.0, 1.0);
    hang.lerp(at, f * f * (3.0 - 2.0 * f))
}

/// The bearing of `at` from player `i`'s facing, positive to the right (radians).
fn bearing(m: &Match, i: usize, at: [f64; 3]) -> f64 {
    let p = &m.players[i];
    let (dx, dz) = (at[0] - p.pos[0], at[2] - p.pos[2]);
    let ahead = dx * p.fwd[0] + dz * p.fwd[2];
    let right = dx * p.fwd[2] - dz * p.fwd[0];
    right.atan2(ahead)
}

/// The event clips a tick's event asks of the flyers (`0x471790`'s choice for each request type; **[I]** which
/// request each event makes): `(player, clips)`. `before` is where everything stood before the tick.
fn requests(m: &Match, before: &Snapshot, e: &Event) -> Vec<(usize, Vec<Clip>)> {
    let one = |p: usize, name: &str| vec![(p, vec![Clip::new(name)])];
    let side = |p: usize, at: [f64; 3], left: &str, right: &str| {
        let name = if bearing(m, p, at) > 0.0 { right } else { left };
        vec![(p, vec![Clip::new(name)])]
    };
    let pos = |p: usize| m.players[p].pos;
    // `0x4714b0`: the throw by the target's quadrant, soft or hard, from frame 5 at 1.3 times
    let throw = |p: usize, at: [f64; 3], hard: bool| {
        let q = ["f", "r", "b", "l"][flyer::quadrant(bearing(m, p, at))];
        let name = format!("med_na_kpch_throw_{q}{}_qf", if hard { "h" } else { "s" });
        vec![(p, vec![Clip { name, speed: 1.3, start: 5.0 }])]
    };
    // `0x471790` 9: a catch to the side the ball came from, low or high
    let catch = |p: usize, tick: u32| {
        let high = tick % 2 == 1;
        let right = bearing(m, p, before.quaffle) > 0.0;
        let name = match (right, high) {
            (true, false) => "med_na_kpch_catch_rs_qf",
            (true, true) => "med_na_kpch_catch_rh_qf",
            (false, false) => "med_na_kpch_catch_ls_qf",
            (false, true) => "med_na_kpch_catch_lh_qf",
        };
        vec![(p, vec![Clip::new(name)])]
    };
    let seeker = |p: usize| m.players[p].role == Role::Seeker;
    match *e {
        // `0x4713e0` (request type 6): the wind-up, then its cycle
        Event::WindUp { player, .. } | Event::ShotWindUp { player, .. } if !seeker(player) => {
            vec![(player, vec![Clip::new("med_na_kpch_throw_ph_qf"), Clip::new("med_na_kpch_throw_ch_qf")])]
        }
        // request type 10: the volley
        Event::Pass { from, volley: true, .. } if !seeker(from) => one(from, "med_na_ch_volley_hit7fr"),
        Event::Pass { from, receiver, .. } if !seeker(from) => throw(from, pos(receiver), false),
        Event::Shot { player, hoop, .. } if !seeker(player) => throw(player, m.hoops[hoop].pos, true),
        Event::Catch { player, tick } if !seeker(player) => catch(player, tick),
        // `0x471790` 0xd: the keeper's save to the side of the shot
        Event::Touch { player, state: 9, .. } => {
            side(player, before.quaffle, "med_na_kpch_catch_ls_qf", "med_na_kpch_catch_rs_qf")
        }
        Event::Touch { player, tick, .. } if !seeker(player) => catch(player, tick),
        // `0x471790` 0xf: the tackle to the victim's side, or 0x14 at the end of a lunge (`0x427d80`); the victim's
        // 0x10 (`0x427d43`), hit from the tackler's quadrant
        Event::Tackle { tackler, victim, lunge, .. } => {
            let mut out = if lunge {
                one(tackler, "med_na_ch_bsttackle_end")
            } else {
                side(tackler, pos(victim), "med_na_ch_tackle_l_qf", "med_na_ch_tackle_r_qf")
            };
            let q = flyer::quadrant(bearing(m, victim, pos(tackler)));
            out.push((victim, vec![Clip::new(&format!("med_na_kpch_gethit_{}", ["f", "r", "b", "l"][q]))]));
            out
        }
        // 0x12 and 0x13: the lunge (the boost tackle, `0x426cff`) and its miss (`0x426f16`)
        Event::Lunge { player, .. } => one(player, "med_na_ch_bsttackle"),
        Event::LungeMiss { player, .. } => one(player, "med_na_ch_bsttackle_miss"),
        // **[I]** 0x1b, the push's miss, for the miss on a dodging carrier (`0x4275e0`'s clip is not read)
        Event::Whiff { player, .. } => match m.holder {
            Some(h) => side(player, pos(h), "med_na_ch_pushmiss_l_qf", "med_na_ch_pushmiss_r_qf"),
            None => vec![],
        },
        // 0x11: the roll
        Event::Dodge { player, .. } => one(player, "med_na_na_flyroll_c"),
        // 0xb: the beaters' swings, to the side of the Bludger
        Event::BeaterSwing { player, .. } => {
            let b = m.bludgers[m.players[player].team].pos;
            side(player, b, "med_na_bt_hitbg_l1", "med_na_bt_hitbg_r1")
        }
        Event::BeaterBlock { player, .. } => {
            let b = m.bludgers[1 - m.players[player].team].pos;
            side(player, b, "med_na_bt_hitbg_l2", "med_na_bt_hitbg_r2")
        }
        // 0xc: hit, by the quadrant the Bludger came from
        Event::BludgerHit { team, victim, .. } => {
            let q = flyer::quadrant(bearing(m, victim, m.bludgers[team].pos));
            let name = match m.players[victim].role {
                Role::Seeker => (if q == 3 { "sml_na_sk_gethit_l" } else { "sml_na_sk_gethit_r" }).to_string(),
                Role::Beater => format!("med_na_bt_gethit_{}", ["f", "r", "b", "l"][q]),
                _ => format!("med_na_kpch_gethit_{}", ["f", "r", "b", "l"][q]),
            };
            vec![(victim, vec![Clip::new(&name)])]
        }
        // the Seeker game's moves on the seekers' own clips
        Event::Race { event, .. } => {
            let Some(r) = &m.race else { return vec![] };
            let seeker = |t: usize| m.teams[t].seeker;
            // the other seeker on the left or the right (its offset across the trail)
            let left = |t: usize| r.seekers[1 - t].offset < r.seekers[t].offset;
            match event {
                RaceEvent::Attack { team } => {
                    one(seeker(team), if left(team) { "sml_na_sk_bargeLeft_c" } else { "sml_na_sk_bargeright_c" })
                }
                RaceEvent::Hit { team } => {
                    one(seeker(team), if left(team) { "sml_na_sk_gethit_l" } else { "sml_na_sk_gethit_r" })
                }
                // a human's seeker holds out his hand within reach (`0x44bb80`, entry 0x39) and grabs with the catch
                // (**[I]**: `0x448c00`'s choice is not read); the AI's grab is its reach
                RaceEvent::Stance { team, on: true } => one(seeker(team), "sml_na_sk_flycatch"),
                RaceEvent::Grab { team, .. } if m.teams[team].human => one(seeker(team), "sml_na_sk_catchsnitch_na"),
                RaceEvent::Grab { team, .. } => one(seeker(team), "sml_na_sk_flycatch"),
                RaceEvent::Caught { team } => one(seeker(team), "sml_na_sk_catchsnitch_na"),
                _ => vec![],
            }
        }
        _ => vec![],
    }
}

fn fail_soft(path: &str, e: &str) {
    eprintln!("{path}: {e}");
}
