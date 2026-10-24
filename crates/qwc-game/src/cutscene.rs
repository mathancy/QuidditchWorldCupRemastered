//! Cutscenes (`root/cs/<name>/`, played by `CutSceneMgr`, `0x485000`-`0x494000`): the script's characters, each
//! posed by its role's clip, and its camera rigs, cut from shot to shot (`tools/cutscene_to_gltf.py` is the
//! reference for what is assembled from what).
//!
//! **[T]** (`RE_NOTES.md`, "Cutscenes"):
//! - the script (`<name>.txt`, `qwc_formats::cs_script`): the `CUTSCENE` block's settings (fades, the stream file,
//!   placement), `CHARACTER` blocks (type, team, motion type, model) and `ROLE` blocks (a character and its clip);
//! - the folder's clip sets (`*.fas`, with the `.sfa` stream the script names, on the skeleton their suffix
//!   names: `qwc_assets::anim`), and their triggers (`*.btf`): a camera clip's `CS_CAM_CUT` is when its shot
//!   becomes the live one, `CS_CAM_LENS_<mm>` its lens (vertical field of view `2 atan(12 / mm)`, 17 mm by
//!   default), `ON_CS_SLOWMO_<f>` / `OFF_CS_SLOWMO_<f>` a slow-motion stretch;
//! - a camera is its rig's bone 1, looking down its `+z`.
//!
//! **[I]**: a character's model by the exporter's rules (an explicit model name in `root/csmodels`, `TEAMn_ROLEk`
//! the team's player, a controlled or generic flyer the team's first chaser or seeker by body size), the referee,
//! commentators and supporters the match's [`Cast`] (`crate::ccs`), each in the order the script has them; a clip is
//! played on the model's own skeleton with its bones matched by name; a prop without a skeleton hangs at its clip
//! skeleton's node (`SKELETON_NODE_INDEX`, else the last bone); placement against the game's event anchor (the
//! motion types other than `eABSOLUTE`) is the caller's matrix.

use qwc_anim::{Clip, Rig};
use qwc_assets::{TextureIndex, Vfs};
use qwc_formats::btf::Btf;
use qwc_formats::cs_script::{self, Block};
use qwc_formats::fas::{Fas, Sfa};
use qwc_formats::skl::{self, Skeleton};
use qwc_render::glam::Vec3;
use qwc_render::{Camera, Gpu, GpuModel, Renderer};

use crate::face::{self, Face};

/// A 4x4 row-vector matrix.
pub type Mat4 = [f64; 16];

const IDENTITY: Mat4 = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// A character's team (`TEAMCODE_TYPE`, `TEAMn_`) and role files (`teams/<team>/<file>.fpm`).
const ROLE_FILES: [(&str, &[&str]); 4] = [
    ("SEEKER", &["seeker"]),
    ("KEEPER", &["keeper"]),
    ("BEATER", &["beater1", "beater2"]),
    ("CHASER", &["chaser1", "chaser2", "chaser3"]),
];
/// Character types with a model of their own in `root/csmodels` (the match's people are the [`Cast`]'s).
const TYPE_MODELS: [(&str, &str); 3] = [("eQUAFFLE", "quaffle"), ("eBLUDGER", "bludger"), ("eSNITCH", "snitch")];

/// Where a cutscene is placed (`0x48e900` from the queue's event, `RE_NOTES.md`, "Match cutscenes").
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Place {
    /// As it was made (`eABSOLUTE`).
    AsMade,
    /// From the event at `at` towards the hoop at `goal` (`eTOWARDS_GOAL`).
    Towards { at: [f64; 3], goal: [f64; 3] },
    /// At `at`, away from the goal team `team` attacks (`eAWAY_FROM_GOAL`).
    Away { at: [f64; 3], team: usize },
    /// For team `team` (`eTEAM_ABSOLUTE`, the uber moves), with the six hoops' heights in the game's order.
    Team { team: usize, hoops: [f64; 6] },
    /// As [`Place::Team`], the script's `+z` along team `team`'s attack (the special moves' `shvol` and `shrol`).
    Attack { team: usize, hoops: [f64; 6] },
}

/// The match's people a cutscene's `eREFEREE`, `eCROWD_COMMENTATOR` and `eCROWD_SUPPORTER` characters are played by
/// (models of `root/csmodels`; `crate::ccs`). Without one, the referee is `referee` and a commentator `lud_com`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cast {
    pub referee: Option<String>,
    pub commentators: [Option<String>; 2],
    pub supporters: [Option<String>; 2],
    /// The event's own player (`eCONTROLLED_CHARACTER`, `eGOAL_SCORER`): its role file of `TEAM1`'s folder
    /// (`chaser2`, ...); the script's other characters of its role take the others.
    pub own: Option<String>,
    /// The other team's event player (`TEAM2`'s, `eOTHER_TEAM`): a tackle's victim.
    pub other_own: Option<String>,
}

/// One posed character.
struct Actor {
    name: String,
    gpu: GpuModel,
    /// The model's skeleton with the clip remapped onto it, or, for a prop, the clip's own skeleton and the node it
    /// hangs at.
    rig: Rig,
    clip: Clip,
    prop_node: Option<usize>,
    /// Its `CHARACTER_TYPE` and `MOTION_TYPE`.
    ctype: String,
    motion: String,
    /// Its morphed meshes, and the weight channel each of the clip's morph tracks sets (the tracks' nodes are the
    /// clip set's, matched to the model's by name).
    face: Option<Face>,
    channels: Vec<Option<u16>>,
}

/// One camera shot: its rig and clip, when it becomes live (clip seconds) and its lens.
struct Shot {
    rig: Rig,
    clip: Clip,
    cut: f64,
    lens: f64,
}

/// A timed trigger of a character's clip (audio, particles, depth of field, ...), in clip seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct CsEvent {
    pub time: f64,
    pub actor: String,
    pub name: String,
}

/// A cutscene, loaded and uploaded.
pub struct Cutscene {
    pub name: String,
    pub settings: Block,
    actors: Vec<Actor>,
    shots: Vec<Shot>,
    /// The longest role's length (clip seconds).
    pub length: f64,
    /// Slow-motion stretches: (start, end, factor) in clip seconds.
    slowmo: Vec<(f64, f64, f64)>,
    pub events: Vec<CsEvent>,
    /// What could not be found (models, clips), for the log.
    pub notes: Vec<String>,
}

/// A clip's triggers: each name with its times (seconds).
type Triggers = Vec<(String, Vec<f32>)>;

/// A clip set of the folder: the set, its clips and the skeleton it plays on.
struct Set {
    clips: Vec<Clip>,
    skeleton: Skeleton,
    suffix: String,
}

fn number(s: &str) -> Option<f64> {
    s.trim().parse().ok()
}

/// The skeleton's bone index by (lower-case) name.
fn bone_names(sk: &Skeleton) -> Vec<String> {
    sk.bones.iter().map(|b| b.name.to_ascii_lowercase()).collect()
}

/// `clip` (on `from`'s bones) with its channels moved to `to`'s bones of the same names (the rest dropped).
fn remap(clip: &Clip, from: &Skeleton, to: &Skeleton) -> Clip {
    let (a, b) = (bone_names(from), bone_names(to));
    let map = |i: u8| a.get(i as usize).and_then(|n| b.iter().position(|m| m == n)).and_then(|j| u8::try_from(j).ok());
    let mut c = clip.clone();
    for chans in [&mut c.rotation, &mut c.translation, &mut c.scale] {
        chans.retain_mut(|ch| match map(ch.bone) {
            Some(j) => {
                ch.bone = j;
                true
            }
            None => false,
        });
    }
    c
}

impl Cutscene {
    /// Load cutscene `name` (a folder of `root/cs`) with `team` and `other` (folders of `teams/`) for its
    /// `TEAM1`/`eTHIS_TEAM` and `TEAM2`/`eOTHER_TEAM` characters, and `cast` for the match's people.
    pub fn load(
        gpu: &Gpu,
        renderer: &mut Renderer,
        vfs: &Vfs,
        name: &str,
        teams: [&str; 2],
        cast: &Cast,
    ) -> Result<Cutscene, String> {
        let [team, other] = teams;
        let folder = format!("root/cs/{}", name.to_ascii_lowercase());
        let script = format!("{folder}/{}.txt", name.to_ascii_lowercase());
        let blocks = cs_script::parse(&vfs.read(&script).map_err(|e| format!("{script}: {e}"))?);
        let settings = blocks.iter().find(|b| b.kind == "CUTSCENE").cloned().ok_or(format!("{script}: no CUTSCENE"))?;
        let prefix = format!("{folder}/");
        let in_folder = |ext: &str| -> Vec<String> {
            vfs.with_extension(ext)
                .filter(|p| p.to_ascii_lowercase().starts_with(&prefix))
                .map(str::to_string)
                .collect()
        };
        let mut notes = Vec::new();
        // the clip sets, each clip by its lower-case name
        let mut sets: Vec<Set> = Vec::new();
        let mut by_name: Vec<(String, usize, usize)> = Vec::new();
        for path in in_folder("fas") {
            let bytes = vfs.read(&path).map_err(|e| format!("{path}: {e}"))?;
            let fas = Fas::parse(&bytes).map_err(|e| format!("{path}: {e}"))?;
            let stream = if qwc_assets::anim::needs_stream(&fas) {
                qwc_assets::anim::set_stream(vfs, &path).map_err(|e| e.to_string())?
            } else {
                None
            };
            let sfa = stream.as_deref().map(Sfa::parse).transpose().map_err(|e| format!("{path}: {e}"))?;
            let (_, skeleton) = qwc_assets::anim::set_skeleton(vfs, &path, &fas).map_err(|e| e.to_string())?;
            let clips = Clip::decode_all(&fas, sfa.as_ref()).map_err(|e| format!("{path}: {e}"))?;
            let stem = path.rsplit('/').next().unwrap_or(&path).to_ascii_lowercase();
            let suffix = stem.trim_end_matches(".fas").rsplit('_').next().unwrap_or("").to_string();
            for (i, c) in clips.iter().enumerate() {
                by_name.push((c.name.to_ascii_lowercase(), sets.len(), i));
            }
            sets.push(Set { clips, skeleton, suffix });
        }
        // the triggers by clip name
        let mut triggers: Vec<(String, Triggers)> = Vec::new();
        for path in in_folder("btf") {
            let btf = Btf::parse(&vfs.read(&path).map_err(|e| format!("{path}: {e}"))?)
                .map_err(|e| format!("{path}: {e}"))?;
            for (anim, list) in btf.anims {
                let list = list.into_iter().filter_map(|t| Some((t.name?, t.times))).collect();
                triggers.push((anim.unwrap_or_default().to_ascii_lowercase(), list));
            }
        }
        let trig = |anim: &str| triggers.iter().find(|(a, _)| a == anim).map_or(&[][..], |(_, l)| l.as_slice());
        let textures = TextureIndex::build(vfs);
        let chars: Vec<&Block> = blocks.iter().filter(|b| b.kind == "CHARACTER").collect();
        let mut counters: Vec<(String, String, usize)> = Vec::new();
        let (mut actors, mut shots, mut length) = (Vec::new(), Vec::new(), 0.0f64);
        let mut slowmo = Vec::new();
        let mut events = Vec::new();
        for role in blocks.iter().filter(|b| b.kind == "ROLE") {
            let (Some(cname), Some(aname)) = (role.get("CHARACTER"), role.get("ANIM_NAME")) else { continue };
            let aname_l = aname.to_ascii_lowercase();
            let Some(&(_, si, ci)) = by_name.iter().find(|(n, _, _)| *n == aname_l) else {
                notes.push(format!("{name}: clip {aname} not found"));
                continue;
            };
            let set = &sets[si];
            let clip = set.clips[ci].clone();
            length = length.max(clip.duration_secs());
            let ch = chars.iter().find(|c| c.name.eq_ignore_ascii_case(cname));
            let get = |k: &str| ch.and_then(|c| c.get(k)).unwrap_or("");
            // the clip's triggers: cuts and lenses for cameras, slow motion, the rest as events
            let list = trig(&aname_l);
            for (tname, times) in list {
                if let Some(f) = tname.strip_prefix("ON_CS_SLOWMO_").and_then(number) {
                    let off = list.iter().find(|(n, _)| n == &format!("OFF_CS_SLOWMO_{}", &tname[13..]));
                    for &start in times {
                        let end = off.and_then(|(_, t)| t.first()).map_or(start, |&t| t);
                        slowmo.push((start as f64, end as f64, f));
                    }
                } else if !tname.starts_with("CS_CAM_") && !tname.starts_with("OFF_CS_SLOWMO") {
                    for &t in times {
                        events.push(CsEvent { time: t as f64, actor: cname.to_string(), name: tname.clone() });
                    }
                }
            }
            if get("CHARACTER_TYPE") == "eCAMERA" {
                let cut = list.iter().find(|(n, _)| n == "CS_CAM_CUT").and_then(|(_, t)| t.first()).map(|&t| t as f64);
                let lens =
                    list.iter().find_map(|(n, _)| n.strip_prefix("CS_CAM_LENS_").and_then(number)).unwrap_or(17.0);
                shots.push(Shot { rig: Rig::new(set.skeleton.clone()), clip, cut: cut.unwrap_or(-1.0), lens });
                continue;
            }
            let who = Who { team, other, cast };
            let Some(path) = model_for(vfs, cname, ch.copied(), &set.suffix, &who, &mut counters) else {
                notes.push(format!("{name}: {cname} ({}) has no model", get("CHARACTER_TYPE")));
                continue;
            };
            let md = match crate::models::load(vfs, &textures, &path) {
                Ok(m) => m,
                Err(e) => {
                    notes.push(format!("{name}: {cname}: {path}: {e}"));
                    continue;
                }
            };
            let gpu_model = renderer.upload_model(gpu, vfs, &md).map_err(|e| format!("{path}: {e}"))?;
            let (rig, clip, prop_node) = match &md.skeleton {
                Some((_, sk)) => (Rig::new(sk.clone()), remap(&clip, &set.skeleton, sk), None),
                None => {
                    let node = get("SKELETON_NODE_INDEX").parse::<usize>().ok();
                    let last = set.skeleton.bones.len().saturating_sub(1);
                    (Rig::new(set.skeleton.clone()), clip, Some(node.unwrap_or(last).min(last)))
                }
            };
            let (ctype, motion) = (get("CHARACTER_TYPE").to_string(), get("MOTION_TYPE").to_string());
            // `QWC_CS_LOG=morph`: the model's morph channels and the clip's morph tracks (node, target)
            if std::env::var("QWC_CS_LOG").is_ok_and(|v| v == "morph") {
                let channels: Vec<u16> = md.morph_channels();
                let mut tracks: Vec<(u16, u16)> = clip.morphs.iter().map(|t| (t.node, t.morph)).collect();
                tracks.dedup();
                let node = |n: u16| set.skeleton.bones.get(n as usize).map_or("?", |b| b.name.as_str()).to_string();
                let named: Vec<String> = tracks.iter().map(|&(n, m)| format!("{}#{n}:{m}", node(n))).collect();
                eprintln!("  morph {cname}: channels {channels:?}; tracks {named:?}");
                let bones = md.skeleton.as_ref().map(|(_, s)| s);
                for (i, m) in md.meshes.iter().enumerate().filter(|(_, m)| !m.morphs.is_empty()) {
                    let mut on: Vec<String> = m
                        .morphs
                        .values()
                        .flatten()
                        .filter_map(|(v, _)| m.bones.as_ref()?.get(*v as usize))
                        .filter_map(|&b| Some(bones?.bones.get(b as usize)?.name.clone()))
                        .collect();
                    on.sort();
                    on.dedup();
                    let keys: Vec<u16> = m.morphs.keys().copied().collect();
                    eprintln!("    mesh {i} level {}: {keys:?} on {on:?}", m.level);
                }
            }
            let face = Face::new(&md);
            let channels = (clip.morphs.iter())
                .map(|t| face.as_ref()?.channel(&set.skeleton.bones.get(t.node as usize)?.name, t.morph))
                .collect();
            let gpu = gpu_model;
            actors.push(Actor { name: cname.to_string(), gpu, rig, clip, prop_node, ctype, motion, face, channels });
        }
        // the live shot is the last whose cut has passed; the first is live from the start
        shots.sort_by(|a, b| a.cut.total_cmp(&b.cut));
        if let Some(s) = shots.first_mut() {
            s.cut = 0.0;
        }
        slowmo.sort_by(|a, b| a.0.total_cmp(&b.0));
        events.sort_by(|a, b| a.time.total_cmp(&b.time));
        if std::env::var_os("QWC_CS_LOG").is_some() {
            for s in &shots {
                eprintln!(
                    "  shot {} from {:.2} s, {} mm, {} frames at {} fps",
                    s.clip.name, s.cut, s.lens, s.clip.frames, s.clip.fps
                );
            }
            for a in &actors {
                // where its root (or its prop's node) is at the clip's start and end, in the cutscene's space
                let at = |f: f64| {
                    let w = a.rig.world_pose(&a.rig.local_pose(Some(&a.clip), f));
                    let m = w.get(a.prop_node.unwrap_or(1.min(w.len().saturating_sub(1)))).copied().unwrap_or(IDENTITY);
                    [m[12], m[13], m[14]].map(|v| (v * 100.0).round() / 100.0)
                };
                let (first, last) = (at(0.0), at(a.clip.frames as f64));
                eprintln!("  actor {} {} frames at {} fps, {first:?} to {last:?}", a.name, a.clip.frames, a.clip.fps);
                // `QWC_CS_LOG=path`: the Quaffle's path, a point every 10 frames
                if std::env::var("QWC_CS_LOG").is_ok_and(|v| v == "path") && a.name.eq_ignore_ascii_case("QUAFFLE") {
                    let path: Vec<String> =
                        (0..=a.clip.frames).step_by(10).map(|f| format!("{f}: {:?}", at(f as f64))).collect();
                    eprintln!("    {}", path.join(" "));
                }
            }
            for &(a, b, f) in &slowmo {
                eprintln!("  slow motion {a:.2}-{b:.2} s at {f}");
            }
            for e in &events {
                eprintln!("  event {:.2} s {} {}", e.time, e.actor, e.name);
            }
        }
        Ok(Cutscene { name: name.to_string(), settings, actors, shots, length, slowmo, events, notes })
    }

    /// Whether it plays in the stadium's crowd box (`CROWDBOX = 1`, the crowd cutscenes).
    pub fn crowd_box(&self) -> bool {
        self.settings.get("CROWDBOX").and_then(number).is_some_and(|v| v != 0.0)
    }

    /// Playback seconds for clip seconds `t` (each slow-motion stretch lasts `1 / factor` times longer).
    pub fn playback_time(&self, t: f64) -> f64 {
        t + self.slowmo.iter().map(|&(a, b, f)| (t.min(b) - a).max(0.0) * (1.0 / f - 1.0)).sum::<f64>()
    }

    /// Clip seconds at playback seconds `p` (the inverse of [`Cutscene::playback_time`]).
    pub fn clip_time(&self, p: f64) -> f64 {
        // the playback time grows monotonically with the clip time: bisect
        let (mut lo, mut hi) = (0.0, self.length.max(p) + 1.0);
        for _ in 0..60 {
            let mid = (lo + hi) / 2.0;
            if self.playback_time(mid) < p {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        (lo + hi) / 2.0
    }

    /// How long it plays, in seconds.
    pub fn duration(&self) -> f64 {
        self.playback_time(self.length)
    }

    /// The models to draw.
    pub fn drawn(&self) -> Vec<&GpuModel> {
        self.actors.iter().map(|a| &a.gpu).collect()
    }

    /// Pose every character `p` playback seconds in, placed by `anchor` (a row-vector matrix from the cutscene's
    /// space to the world's), and answer the live shot's camera (none without one).
    pub fn frame(&self, gpu: &Gpu, renderer: &Renderer, p: f64, anchor: &Mat4) -> Option<Camera> {
        let t = self.clip_time(p).min(self.length);
        for a in &self.actors {
            let f = (t * a.clip.fps as f64).min(a.clip.frames as f64);
            let world = a.rig.world_pose(&a.rig.local_pose(Some(&a.clip), f));
            match a.prop_node {
                Some(n) => {
                    let m = skl::matmul(&world[n], anchor);
                    renderer.set_transform(&gpu.queue, &a.gpu, Some(m.map(|v| v as f32)));
                }
                None => {
                    let skin: Vec<Mat4> = a.rig.skin_matrices(&world).iter().map(|s| skl::matmul(s, anchor)).collect();
                    renderer.set_pose(&gpu.queue, &a.gpu, &skin);
                    // the face: each track sets its channel (`0x40d540`, the one layer's mode 1 at weight 1)
                    if let Some(face) = &a.face {
                        let mut weights = Vec::new();
                        for (t, c) in a.clip.morphs.iter().zip(&a.channels) {
                            if let (Some(c), Some(v)) = (*c, t.sample(f as f32)) {
                                face::set(&mut weights, c, v);
                            }
                        }
                        face.apply(&gpu.queue, renderer, &a.gpu, &weights);
                    }
                }
            }
        }
        let shot = self.shots.iter().rev().find(|s| s.cut <= t + 1e-9).or(self.shots.first())?;
        let f = (t * shot.clip.fps as f64).min(shot.clip.frames as f64);
        let world = shot.rig.world_pose(&shot.rig.local_pose(Some(&shot.clip), f));
        let m = skl::matmul(world.get(1).or(world.first()).unwrap_or(&IDENTITY), anchor);
        let v = |i: usize| Vec3::new(m[i] as f32, m[i + 1] as f32, m[i + 2] as f32);
        let (eye, up, fwd) = (v(12), v(4).normalize_or(Vec3::Y), v(8).normalize_or(Vec3::Z));
        Some(Camera {
            eye,
            target: eye + fwd,
            up,
            fov_y: (2.0 * (12.0 / shot.lens).atan()) as f32,
            near: 0.3,
            far: 4000.0,
        })
    }

    /// The events between playback seconds `from` (excluded) and `to` (included).
    pub fn events_between(&self, from: f64, to: f64) -> impl Iterator<Item = &CsEvent> {
        let (a, b) = (self.clip_time(from), self.clip_time(to));
        // the first call (from before the start) also takes the events at the very start
        let first = from < 0.0;
        self.events.iter().filter(move |e| (e.time > a || (first && e.time <= 0.0)) && e.time <= b)
    }

    /// A setting's three numbers (none missing as 0).
    fn vec3(&self, key: &str) -> [f64; 3] {
        let mut v = [0.0; 3];
        let s = self.settings.get(key).unwrap_or("");
        for (i, n) in s.split(|c: char| c.is_whitespace() || c == ',').filter_map(number).take(3).enumerate() {
            v[i] = n;
        }
        v
    }

    /// The placement of a cutscene made towards a hoop (`eTOWARDS_GOAL`, `0x48e900`) for the event at `at` and the
    /// hoop at `goal`: with `EVENT_FRAME_OFFSET` `e`, the origin is moved to `e.z` from the hoop on the line to `at`;
    /// then the yaw is `atan2(e.x, e.z) + atan2(d.x, d.z)` and the pitch `atan2(e.y, |e.xz|) - atan2(d.y, |d.xz|)`,
    /// `d` the origin to the hoop, so the script's hoop direction meets the real one. **[I]** how the game composes
    /// it with the characters (`REPOSITION_OFFSET` added in the cutscene's space first, as the tackles' clips, made
    /// 30 up with an offset of -30, suggest).
    pub fn towards(&self, at: [f64; 3], goal: [f64; 3]) -> Mat4 {
        self.placed(at, goal, false)
    }

    /// The matrix from the cutscene's space to the world's for `place`.
    pub fn anchor(&self, place: Place) -> Mat4 {
        match place {
            Place::AsMade => IDENTITY,
            Place::Towards { at, goal } => self.towards(at, goal),
            Place::Away { at, team } => self.away(at, team),
            Place::Team { team, hoops } => self.team_absolute(team, hoops),
            Place::Attack { team, hoops } => {
                // team 0 attacks `-x`: the yaw 270 degrees, team 1's 90
                let yaw = if team == 0 { 270.0 } else { 90.0 };
                self.turned(yaw, hoops)
            }
        }
    }

    /// The placement of a cutscene made for a team (`eTEAM_ABSOLUTE`, the uber moves) **[I]** (its readers are not
    /// traced): the script turned by `REORIENT_ANGLES`' yaw (half a turn more for team 1, who attacks the other way)
    /// and moved by `REPOSITION_OFFSET`, raised by the height of the hoop `HEIGHT_RELATIVE_TO_GOAL_IDX` names when
    /// it names one (the scripts are made with that hoop at the offset's height). Each uber move's Quaffle then
    /// crosses the goal line through a hoop: Australia's last shot the side hoop's 3.75 up, the others the middle's.
    pub fn team_absolute(&self, team: usize, hoops: [f64; 6]) -> Mat4 {
        self.turned(self.vec3("REORIENT_ANGLES")[1] + if team == 1 { 180.0 } else { 0.0 }, hoops)
    }

    /// The script turned by `yaw` degrees about `y`, moved by `REPOSITION_OFFSET` and raised by the height of the
    /// hoop `HEIGHT_RELATIVE_TO_GOAL_IDX` names (`hoops` in the game's order).
    fn turned(&self, yaw: f64, hoops: [f64; 6]) -> Mat4 {
        let yaw = yaw.to_radians();
        let f = [yaw.sin(), 0.0, yaw.cos()];
        // x = up x forward, y = up
        let x = [f[2], 0.0, -f[0]];
        let mut t = self.vec3("REPOSITION_OFFSET");
        let goal = self.settings.get("HEIGHT_RELATIVE_TO_GOAL_IDX").and_then(|v| v.trim().parse::<usize>().ok());
        if let Some(h) = goal.and_then(|i| hoops.get(i)) {
            t[1] += h;
        }
        [x[0], x[1], x[2], 0.0, 0.0, 1.0, 0.0, 0.0, f[0], f[1], f[2], 0.0, t[0], t[1], t[2], 1.0]
    }

    /// The motion type its characters other than the cameras share (`eABSOLUTE` when none says).
    pub fn motion(&self) -> &str {
        self.actors.iter().map(|a| a.motion.as_str()).find(|m| !m.is_empty()).unwrap_or("eABSOLUTE")
    }

    /// The placement of a cutscene made away from a goal (`eAWAY_FROM_GOAL`, `0x48e900` type 5) for the event at
    /// `at`: towards `(100, at.y, 0)` for team 0's, `(-100, at.y, 0)` for team 1's (`0x48e900`). **[I]** the
    /// script's own player (`eGOAL_SCORER`, `eCONTROLLED_CHARACTER`) is brought onto `at`: the celebrations are
    /// made at a goal mouth, away from their origin.
    pub fn away(&self, at: [f64; 3], team: usize) -> Mat4 {
        let x = if team == 0 { 100.0 } else { -100.0 };
        self.placed(at, [x, at[1], 0.0], true)
    }

    /// Where the script's own player is when it starts (its first bone below the root), in the cutscene's space.
    fn own_player(&self) -> Option<[f64; 3]> {
        let a = self.actors.iter().find(|a| matches!(a.ctype.as_str(), "eGOAL_SCORER" | "eCONTROLLED_CHARACTER"))?;
        let w = a.rig.world_pose(&a.rig.local_pose(Some(&a.clip), 0.0));
        let m = w.get(1.min(w.len().saturating_sub(1)))?;
        Some([m[12], m[13], m[14]])
    }

    fn placed(&self, at: [f64; 3], goal: [f64; 3], snap: bool) -> Mat4 {
        let e = self.vec3("EVENT_FRAME_OFFSET");
        let sub = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
        let len = |v: [f64; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        let mut origin = at;
        if e[2] != 0.0 {
            let d = sub(at, goal);
            let l = len(d).max(1e-9);
            origin = std::array::from_fn(|k| goal[k] + d[k] / l * e[2]);
        }
        let d = sub(goal, origin);
        let yaw = e[0].atan2(e[2]) + d[0].atan2(d[2]);
        let pitch = e[1].atan2(e[0].hypot(e[2])) - d[1].atan2(d[0].hypot(d[2]));
        let f = [yaw.sin() * pitch.cos(), -pitch.sin(), yaw.cos() * pitch.cos()];
        // x = up x forward, y = forward x x
        let cross = |a: [f64; 3], b: [f64; 3]| {
            [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
        };
        let x = cross([0.0, 1.0, 0.0], f);
        let lx = len(x).max(1e-9);
        let x = x.map(|v| v / lx);
        let y = cross(f, x);
        let mut r = self.vec3("REPOSITION_OFFSET");
        if let Some(p) = self.own_player().filter(|_| snap) {
            r = std::array::from_fn(|k| r[k] - p[k]);
        }
        // (clip + r) R + origin
        let t: [f64; 3] = std::array::from_fn(|k| r[0] * x[k] + r[1] * y[k] + r[2] * f[k] + origin[k]);
        [x[0], x[1], x[2], 0.0, y[0], y[1], y[2], 0.0, f[0], f[1], f[2], 0.0, t[0], t[1], t[2], 1.0]
    }

    /// How dark the screen is `p` playback seconds in (0 clear, 1 black): the fade in over `FADE_IN_INITIAL_SECS`
    /// (0.25 s when the script has none) and the fade out over the last `FADE_OUT_SECS` (`0x48cc60`, `0x48d650`).
    pub fn fade(&self, p: f64) -> f64 {
        let secs = |k: &str| self.settings.get(k).and_then(number);
        let fade_in = secs("FADE_IN_INITIAL_SECS").unwrap_or(0.25);
        let mut dark: f64 = if fade_in > 0.0 { 1.0 - (p / fade_in).clamp(0.0, 1.0) } else { 0.0 };
        if let Some(out) = secs("FADE_OUT_SECS").filter(|&o| o > 0.0) {
            dark = dark.max(((p - (self.duration() - out)) / out).clamp(0.0, 1.0));
        }
        dark
    }
}

/// The special move's cutscenes made along the attack (`shvol`, `shrol`; [`Place::Attack`]).
pub(crate) fn is_attack(name: &str) -> bool {
    name.eq_ignore_ascii_case("shvol") || name.eq_ignore_ascii_case("shrol")
}

/// `qwc-game cutscene NAME [TEAM OTHER] [--stadium NAME] --time SECS --out FILE.png`: the frame `--time` playback
/// seconds in, through the live shot, over the stadium's models when one is given. `QWC_CS_WAV=FILE.wav` also plays
/// its triggers' sounds through the game's audio from start to end and writes them there ([`sounds`]).
pub fn shot(args: &crate::Args, vfs: &'static Vfs) {
    use crate::fail;
    let name = &args.models[0];
    let team = args.models.get(1).map_or("Gryff", String::as_str);
    let other = args.models.get(2).map_or("Slyth", String::as_str);
    let gpu = Gpu::headless().unwrap_or_else(|e| fail(e));
    let mut renderer = Renderer::new(&gpu.device, &gpu.queue, qwc_render::OFFSCREEN_FORMAT);
    renderer.shaders = crate::packs::shaders();
    renderer.remastered = crate::fe::remastered_textures(args, vfs);
    crate::stadium::set_look(&mut renderer, args.look, args.stadium.as_deref().unwrap_or("england"));
    // the cast of the first match the tables have for these teams there, of any type
    let location = args.stadium.as_deref().map_or("XXX", crate::ccs::location_of_folder);
    let codes = [crate::ccs::team_code(team), crate::ccs::team_code(other)];
    let cast = crate::ccs::Tables::load(vfs)
        .and_then(|t| {
            let r = ["H", "E", "W", "Q", "P"].iter().find_map(|m| t.find(m, codes, location))?;
            Some(t.cast(&t.records[r]))
        })
        .unwrap_or_default();
    let cs = Cutscene::load(&gpu, &mut renderer, vfs, name, [team, other], &cast).unwrap_or_else(|e| fail(e));
    for n in &cs.notes {
        eprintln!("  note: {n}");
    }
    eprintln!(
        "{}: {} characters, {} shots, {:.2} s of clip ({:.2} s played), {} events",
        cs.name,
        cs.actors.len(),
        cs.shots.len(),
        cs.length,
        cs.duration(),
        cs.events.len()
    );
    if let Some(wav) = std::env::var_os("QWC_CS_WAV") {
        sounds(&cs, vfs, team, std::path::Path::new(&wav));
    }
    let mut models: Vec<GpuModel> = Vec::new();
    if let Some(s) = &args.stadium {
        let textures = TextureIndex::build(vfs);
        let gfx = format!("stadium/{s}/gfx");
        // a crowd cutscene plays in the crowd box
        let set = if cs.crowd_box() { "crowdbox" } else { "stadium" };
        let paths = [format!("{gfx}/sky.fpm"), format!("{gfx}/{set}.fpm")];
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        models = crate::stadium::models(&gpu, &mut renderer, vfs, &textures, &refs);
        if let Some(l) = crate::stadium::lights(vfs, &gfx) {
            renderer.lights = l;
        }
        // the hoops where a match has them
        let places: Vec<[f32; 16]> = qwc_sim::shooting::hoops(1.0, 20.0)
            .iter()
            .map(|h| qwc_feck::place::translation(h.pos.map(|v| v as f32)))
            .collect();
        models.extend(crate::stadium::hoops(
            &gpu,
            &mut renderer,
            vfs,
            &textures,
            "stadium/hoopnorm/gfx/singlehoop.fpm",
            &places,
        ));
    }
    // `QWC_CS_AT` and `QWC_CS_GOAL` (`x,y,z`): placed towards that hoop from there
    let point = |k: &str| {
        let v: Vec<f64> = std::env::var(k).ok()?.split(',').filter_map(number).collect();
        (v.len() == 3).then(|| [v[0], v[1], v[2]])
    };
    // `QWC_CS_TEAM` the team it is for (0)
    let team_of = std::env::var("QWC_CS_TEAM").map_or(0, |t| t.parse().unwrap_or(0));
    let anchor = match (point("QWC_CS_AT"), point("QWC_CS_GOAL")) {
        // a team's own (the uber moves) at the match's hoops
        _ if cs.motion() == "eTEAM_ABSOLUTE" || is_attack(name) || name.eq_ignore_ascii_case("hitblud") => {
            let all = qwc_sim::shooting::hoops(1.0, 20.0);
            let hoops = std::array::from_fn(|i| all.get(i).map_or(0.0, |h| h.pos[1]));
            let place = if is_attack(name) {
                Place::Attack { team: team_of, hoops }
            } else {
                Place::Team { team: team_of, hoops }
            };
            cs.anchor(place)
        }
        (Some(at), _) if cs.motion() == "eAWAY_FROM_GOAL" => cs.away(at, team_of),
        (Some(at), Some(goal)) => cs.towards(at, goal),
        _ => IDENTITY,
    };
    let camera = cs.frame(&gpu, &renderer, args.time, &anchor).unwrap_or(Camera {
        eye: Vec3::new(0.0, 20.0, -60.0),
        target: Vec3::ZERO,
        up: Vec3::Y,
        fov_y: 70f32.to_radians(),
        near: 0.3,
        far: 4000.0,
    });
    crate::stadium::animate(&gpu, &renderer, &mut models, args.time);
    let drawn: Vec<&GpuModel> = models.iter().chain(cs.drawn()).collect();
    let target = qwc_render::Offscreen::new(&gpu.device, args.size);
    target.render(&gpu, &renderer, &camera, &drawn);
    let rgba = target.read_rgba(&gpu).unwrap_or_else(|e| fail(e));
    let out = args.out.as_ref().unwrap_or_else(|| fail("cutscene needs --out FILE.png"));
    let file = std::fs::File::create(out).unwrap_or_else(|e| fail(format!("{}: {e}", out.display())));
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), args.size.0, args.size.1);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().unwrap_or_else(|e| fail(e));
    w.write_image_data(&rgba).unwrap_or_else(|e| fail(e));
    eprintln!("camera at {:?} towards {:?}; wrote {}", camera.eye, camera.target, out.display());
}

/// The sounds `cs`'s triggers play, as the live front end plays them for `team`'s cutscene (`crate::audio`), from
/// its start to its end at 24 ticks a second, mixed into `out` (a WAV file); each trigger that played nothing is
/// said. The commentary's lines (a match's) are not among them.
fn sounds(cs: &Cutscene, vfs: &'static Vfs, team: &str, out: &std::path::Path) {
    const RATE: u32 = 44_100;
    let mut audio = crate::audio::Audio::offline(vfs, RATE);
    audio.cutscene_team(team);
    let mixer = audio.out.mixer().clone();
    let (mut samples, mut at) = (Vec::new(), -1.0);
    let step = 1.0 / 24.0;
    let mut t = 0.0;
    // and a second after, for the last sounds' tails
    while t < cs.duration() + 1.0 {
        let names: Vec<String> = cs.events_between(at, t).map(|e| e.name.clone()).collect();
        for n in names.iter().filter(|n| n.starts_with("ON_AUDIO_")) {
            let before = mixer.lock().unwrap().voice_count();
            audio.cutscene_event(n);
            let after = mixer.lock().unwrap().voice_count();
            eprintln!("  {t:6.2} s {n}: {}", if after > before { "plays" } else { "nothing" });
        }
        at = t;
        let frames = (RATE as f64 * step).round() as usize;
        let mut buf = vec![0.0f32; frames * 2];
        mixer.lock().unwrap().render(&mut buf);
        samples.extend_from_slice(&buf);
        t += step;
    }
    let samples = samples.iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect();
    let pcm = qwc_formats::ea_audio::Pcm { channels: 2, rate: RATE, samples };
    match std::fs::write(out, pcm.to_wav()) {
        Ok(()) => eprintln!("wrote {} ({:.1} s)", out.display(), pcm.frames() as f64 / RATE as f64),
        Err(e) => eprintln!("{}: {e}", out.display()),
    }
}

/// A cutscene model of that name (`root/csmodels/<name>/<name>.fpm`).
fn csmodel(vfs: &Vfs, name: &str) -> Option<String> {
    let n = name.to_ascii_lowercase();
    let path = format!("root/csmodels/{n}/{n}.fpm");
    vfs.exists(&path).then_some(path)
}

/// Whom a cutscene's characters are played by: the teams (folders of `teams/`) and the match's people.
struct Who<'a> {
    team: &'a str,
    other: &'a str,
    cast: &'a Cast,
}

/// The `k`-th of a kind of character so far (counting it).
fn nth(counters: &mut Vec<(String, String, usize)>, t: &str, kind: &str) -> usize {
    match counters.iter_mut().find(|(a, b, _)| a == t && b == kind) {
        Some(c) => {
            c.2 += 1;
            c.2 - 1
        }
        None => {
            counters.push((t.to_string(), kind.to_string(), 1));
            0
        }
    }
}

/// The model a character plays with (`tools/cutscene_to_gltf.py`'s `model_for`, with the match's [`Cast`]).
fn model_for(
    vfs: &Vfs,
    cname: &str,
    ch: Option<&Block>,
    suffix: &str,
    who: &Who,
    counters: &mut Vec<(String, String, usize)>,
) -> Option<String> {
    let (team, other) = (who.team, who.other);
    let get = |k: &str| ch.and_then(|c| c.get(k)).unwrap_or("");
    for key in ["GENERAL_PROP_MODEL_NAME", "GENERAL_FLYER_MODEL_NAME", "MODEL_NAME"] {
        if !get(key).is_empty() {
            return csmodel(vfs, get(key));
        }
    }
    let upper = cname.to_ascii_uppercase();
    let ctype = get("CHARACTER_TYPE");
    // the match's people, whatever the character is called (a crowd cutscene's supporters are `TEAM2_CHASERn`)
    let mut cast = |list: &[Option<String>; 2], kind: &str| {
        let k = nth(counters, "", kind);
        list.get(k).cloned().flatten()
    };
    match ctype {
        "eCONTROLLED_CHARACTER" | "eGOAL_SCORER" if who.cast.own.is_some() || who.cast.other_own.is_some() => {
            // this team's event player, or the other's (a tackle's victim)
            let (folder, own) = if get("TEAMCODE_TYPE") == "eOTHER_TEAM" {
                (other, who.cast.other_own.as_deref())
            } else {
                (team, who.cast.own.as_deref())
            };
            if let Some(own) = own {
                let path = format!("teams/{folder}/{own}.fpm");
                return vfs.exists(&path).then_some(path);
            }
        }
        "eREFEREE" => return csmodel(vfs, who.cast.referee.as_deref().unwrap_or("referee")),
        "eCROWD_COMMENTATOR" => {
            return csmodel(vfs, &cast(&who.cast.commentators, ctype).unwrap_or_else(|| "lud_com".into()))
        }
        "eCROWD_SUPPORTER" => return csmodel(vfs, &cast(&who.cast.supporters, ctype)?),
        _ => {}
    }
    let (t, role, idx) = if let Some(rest) = upper.strip_prefix("TEAM") {
        let (num, rest) = rest.split_at(1.min(rest.len()));
        let rest = rest.trim_start_matches('_');
        let role = ROLE_FILES.iter().map(|(r, _)| *r).find(|r| rest.starts_with(r))?;
        let k: usize = rest[role.len()..].parse().unwrap_or(1);
        (if num == "1" { team } else { other }, role, k.saturating_sub(1))
    } else if let Some(role) =
        ctype.strip_prefix('e').and_then(|r| ROLE_FILES.iter().map(|(n, _)| *n).find(|n| *n == r))
    {
        let t = if get("TEAMCODE_TYPE") == "eOTHER_TEAM" { other } else { team };
        (t, role, nth(counters, t, role))
    } else if let Some((_, m)) = TYPE_MODELS.iter().find(|(c, _)| *c == ctype) {
        return csmodel(vfs, m);
    } else {
        (team, if matches!(suffix, "sma" | "sml") { "SEEKER" } else { "CHASER" }, 0)
    };
    // the event's own player is not one of the others
    let own = who.cast.own.as_deref().filter(|_| t == team);
    let files: Vec<&str> =
        ROLE_FILES.iter().find(|(r, _)| *r == role)?.1.iter().copied().filter(|f| Some(*f) != own).collect();
    let files = if files.is_empty() { ROLE_FILES.iter().find(|(r, _)| *r == role)?.1.to_vec() } else { files };
    let path = format!("teams/{t}/{}.fpm", files[idx % files.len()]);
    vfs.exists(&path).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qwc_formats::fas::Channel;

    #[test]
    fn slow_motion_stretches_playback() {
        let cs = Cutscene {
            name: "t".into(),
            settings: Block { kind: "CUTSCENE".into(), name: "t".into(), entries: vec![] },
            actors: vec![],
            shots: vec![],
            length: 10.0,
            slowmo: vec![(2.0, 4.0, 0.5)],
            events: vec![],
            notes: vec![],
        };
        // two clip seconds at half speed play for four
        assert!((cs.playback_time(1.0) - 1.0).abs() < 1e-12);
        assert!((cs.playback_time(4.0) - 6.0).abs() < 1e-12);
        assert!((cs.duration() - 12.0).abs() < 1e-12);
        assert!((cs.clip_time(6.0) - 4.0).abs() < 1e-6);
        assert!((cs.clip_time(4.0) - 3.0).abs() < 1e-6);
    }

    #[test]
    fn clips_move_to_bones_of_the_same_name() {
        let bone = |name: &str, parent: Option<usize>| qwc_formats::skl::Bone {
            index: 0,
            name: name.into(),
            local: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
            flags: 0,
            parent,
            child: None,
            sibling: None,
        };
        let sk = |names: &[&str]| Skeleton {
            bones: names.iter().enumerate().map(|(i, n)| bone(n, i.checked_sub(1))).collect(),
            world: vec![IDENTITY; names.len()],
        };
        let (from, to) = (sk(&["Root", "Spine", "Head"]), sk(&["Root", "Head"]));
        let ch = |b: u8| Channel { bone: b, keys: vec![], times: vec![] };
        let clip = Clip { rotation: vec![ch(0), ch(1), ch(2)], ..Clip::default() };
        let c = remap(&clip, &from, &to);
        assert_eq!(c.rotation.iter().map(|c| c.bone).collect::<Vec<_>>(), vec![0, 1]);
    }
}
