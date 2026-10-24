//! Skeletal animation playback: `.fas` clips evaluated into bone poses and skinning matrices.
//!
//! The rules follow `tools/fas_to_gltf.py`, which was checked against the game:
//! - slot 0 channels are rotation angles, applied as `Rx(x) * Ry(y) * Rz(z)` in the row-vector convention (the
//!   game's matrix builder at `0x407cc5`); slot 1 channels are absolute local translations;
//! - a bone without a channel keeps that part of its bind pose, and the bind pose's per-axis scale is kept;
//! - keys are evaluated with the CEDHAT_3.2 rules ([`fas::Channel::sample`]), at a fractional frame time, which
//!   is how the game interpolates; the glTF exports instead sample whole frames and let the viewer slerp.
//!
//! The run-time side (`docs/ANIMATION.md`): [`play`] is the library's channels and their step, [`trigger`] the
//! timed events clips carry, [`pose`] the layers blended into the model's nodes, [`control`] the characters' slots
//! and cross-fades.

pub mod control;
pub mod play;
pub mod pose;
pub mod trigger;

use qwc_formats::fas::{self, Channel, Fas, MorphTrack, Sfa, VisTrack};
use qwc_formats::skl::{self, Skeleton};
use qwc_formats::Result;

/// Row-major 4x4 matrix, row-vector convention (`v' = v * M`), as in the game and `qwc_formats::skl`.
pub type Mat4 = [f64; 16];

pub const IDENTITY: Mat4 = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// Row-vector rotation `Rx(x) * Ry(y) * Rz(z)` as rows of a 3x3 matrix.
pub fn euler_row(x: f64, y: f64, z: f64) -> [[f64; 3]; 3] {
    let (sx, cx) = x.sin_cos();
    let (sy, cy) = y.sin_cos();
    let (sz, cz) = z.sin_cos();
    [
        [cy * cz, cy * sz, -sy],
        [sx * sy * cz - cx * sz, sx * sy * sz + cx * cz, sx * cy],
        [cx * sy * cz + sx * sz, cx * sy * sz - sx * cz, cx * cy],
    ]
}

/// Inverse of an affine row-vector matrix (3x3 part general, last column `0, 0, 0, 1`).
pub fn affine_inverse(m: &Mat4) -> Mat4 {
    let a = [[m[0], m[1], m[2]], [m[4], m[5], m[6]], [m[8], m[9], m[10]]];
    let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
        + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
    let inv_det = if det.abs() > 1e-300 { 1.0 / det } else { 0.0 };
    let c = |r0: usize, c0: usize, r1: usize, c1: usize| a[r0][c0] * a[r1][c1] - a[r0][c1] * a[r1][c0];
    let i = [
        [c(1, 1, 2, 2), -c(0, 1, 2, 2), c(0, 1, 1, 2)],
        [-c(1, 0, 2, 2), c(0, 0, 2, 2), -c(0, 0, 1, 2)],
        [c(1, 0, 2, 1), -c(0, 0, 2, 1), c(0, 0, 1, 1)],
    ]
    .map(|r| r.map(|v| v * inv_det));
    let t = [m[12], m[13], m[14]];
    let it: [f64; 3] = std::array::from_fn(|k| -(t[0] * i[0][k] + t[1] * i[1][k] + t[2] * i[2][k]));
    [
        i[0][0], i[0][1], i[0][2], 0.0, i[1][0], i[1][1], i[1][2], 0.0, i[2][0], i[2][1], i[2][2], 0.0, it[0], it[1],
        it[2], 1.0,
    ]
}

/// One decoded animation of a set.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Clip {
    pub name: String,
    /// Length in frames; the clip loops at this time.
    pub frames: u32,
    pub fps: f32,
    pub rotation: Vec<Channel>,
    pub translation: Vec<Channel>,
    /// Codec slot 2, scale (no clip on the disc has any).
    pub scale: Vec<Channel>,
    /// The six bone masks' words (`clip +0x14`): rotation, translation, scale, every animated bone, morph nodes,
    /// visibility nodes; empty when absent.
    pub masks: [Vec<u32>; 6],
    pub morphs: Vec<MorphTrack>,
    pub visibility: Vec<VisTrack>,
    /// `0x40ca50`: min then max.
    pub bbox: [f32; 6],
}

impl Clip {
    /// Decode animation `index` of `set` (`sfa` is needed by cutscene `Streamed3` sets).
    pub fn decode(set: &Fas, index: usize, sfa: Option<&Sfa>) -> Result<Clip> {
        let anim = &set.anims[index];
        let masks = std::array::from_fn(|k| anim.masks.get(k).cloned().flatten().map_or(vec![], |m| m.words));
        Ok(Clip {
            name: anim.name.clone().unwrap_or_else(|| format!("anim{index}")),
            frames: anim.frames,
            fps: anim.fps,
            rotation: set.channels(anim, 0, sfa)?,
            translation: set.channels(anim, 1, sfa)?,
            scale: set.channels(anim, 2, sfa)?,
            masks,
            morphs: set.morph_tracks(anim, sfa)?,
            visibility: set.vis_tracks(anim)?,
            bbox: anim.bbox.unwrap_or_default(),
        })
    }

    /// Every clip of a set, in file order.
    pub fn decode_all(set: &Fas, sfa: Option<&Sfa>) -> Result<Vec<Clip>> {
        (0..set.anims.len()).map(|i| Clip::decode(set, i, sfa)).collect()
    }

    pub fn duration_secs(&self) -> f64 {
        if self.fps > 0.0 {
            self.frames as f64 / self.fps as f64
        } else {
            0.0
        }
    }

    /// Frame time for `secs` seconds into the clip, wrapped to its length.
    pub fn frame_at(&self, secs: f64) -> f64 {
        let f = secs * self.fps as f64;
        if self.frames == 0 {
            0.0
        } else {
            f.rem_euclid(self.frames as f64)
        }
    }
}

/// Bind pose of one bone split into the parts a clip can replace.
#[derive(Debug, Clone, Copy)]
struct BindParts {
    scale: [f64; 3],
    rotation: [[f64; 3]; 3],
    translation: [f64; 3],
}

fn bind_parts(local: &[f32; 16]) -> BindParts {
    let m = local.map(f64::from);
    let rows = [[m[0], m[1], m[2]], [m[4], m[5], m[6]], [m[8], m[9], m[10]]];
    let scale = rows.map(|r| (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt());
    let rotation = std::array::from_fn(|i| rows[i].map(|v| if scale[i] > 0.0 { v / scale[i] } else { v }));
    BindParts { scale, rotation, translation: [m[12], m[13], m[14]] }
}

/// Evaluates clips on one skeleton.
#[derive(Debug, Clone)]
pub struct Rig {
    pub skeleton: Skeleton,
    bind: Vec<BindParts>,
    /// `inverse(bind_world)` per bone.
    inverse_bind: Vec<Mat4>,
    /// Bones ordered so every parent comes before its children.
    order: Vec<usize>,
}

impl Rig {
    pub fn new(skeleton: Skeleton) -> Rig {
        let bind = skeleton.bones.iter().map(|b| bind_parts(&b.local)).collect();
        let inverse_bind = skeleton.world.iter().map(affine_inverse).collect();
        let mut order = Vec::with_capacity(skeleton.bones.len());
        let mut placed = vec![false; skeleton.bones.len()];
        while order.len() < skeleton.bones.len() {
            let before = order.len();
            for (i, b) in skeleton.bones.iter().enumerate() {
                if !placed[i] && b.parent.is_none_or(|p| placed[p]) {
                    placed[i] = true;
                    order.push(i);
                }
            }
            assert!(order.len() > before, "skeleton hierarchy has a cycle"); // Skeleton::parse rejects cycles
        }
        Rig { skeleton, bind, inverse_bind, order }
    }

    pub fn bone_count(&self) -> usize {
        self.skeleton.bones.len()
    }

    /// Local bone matrices at frame time `t` (`None` gives the bind pose).
    pub fn local_pose(&self, clip: Option<&Clip>, t: f64) -> Vec<Mat4> {
        let mut rot: Vec<[[f64; 3]; 3]> = self.bind.iter().map(|b| b.rotation).collect();
        let mut trans: Vec<[f64; 3]> = self.bind.iter().map(|b| b.translation).collect();
        if let Some(c) = clip {
            for ch in &c.rotation {
                if let Some(r) = rot.get_mut(ch.bone as usize) {
                    let [x, y, z] = ch.sample(t, c.frames);
                    *r = euler_row(x, y, z);
                }
            }
            for ch in &c.translation {
                if let Some(tr) = trans.get_mut(ch.bone as usize) {
                    *tr = ch.sample(t, c.frames);
                }
            }
        }
        (0..self.bone_count())
            .map(|i| {
                let (s, r, t) = (self.bind[i].scale, rot[i], trans[i]);
                [
                    r[0][0] * s[0],
                    r[0][1] * s[0],
                    r[0][2] * s[0],
                    0.0,
                    r[1][0] * s[1],
                    r[1][1] * s[1],
                    r[1][2] * s[1],
                    0.0,
                    r[2][0] * s[2],
                    r[2][1] * s[2],
                    r[2][2] * s[2],
                    0.0,
                    t[0],
                    t[1],
                    t[2],
                    1.0,
                ]
            })
            .collect()
    }

    /// Local bone matrices from a [`pose::Pose`]: a bone the animation rotated is `Rx·Ry·Rz` of its angles with
    /// each row scaled by its scale (the node's rotation setter, `0x407cc5`); one it never rotated keeps its bind
    /// rotation under its scale.
    pub fn pose_local(&self, pose: &pose::Pose) -> Vec<Mat4> {
        (0..self.bone_count())
            .map(|i| {
                let r = match pose.rotation.get(i).copied().flatten() {
                    Some([x, y, z]) => euler_row(x as f64, y as f64, z as f64),
                    None => self.bind[i].rotation,
                };
                let s = pose.scale.get(i).map_or(self.bind[i].scale, |s| s.map(f64::from));
                let t = pose.translation.get(i).map_or(self.bind[i].translation, |t| t.map(f64::from));
                let mut m = IDENTITY;
                for row in 0..3 {
                    for col in 0..3 {
                        m[row * 4 + col] = r[row][col] * s[row];
                    }
                }
                m[12..15].copy_from_slice(&t);
                m
            })
            .collect()
    }

    /// The bind pose as the nodes start (`pose::Pose::new`): bind translations and scales, no rotation set.
    pub fn bind_pose(&self) -> pose::Pose {
        let n = self.bone_count();
        pose::Pose::new(
            self.bind.iter().map(|b| b.translation.map(|v| v as f32)).collect(),
            self.bind.iter().map(|b| b.scale.map(|v| v as f32)).collect(),
            vec![0; n],
        )
    }

    /// World (model-space) bone matrices from local ones.
    pub fn world_pose(&self, local: &[Mat4]) -> Vec<Mat4> {
        let mut world = vec![IDENTITY; local.len()];
        for &i in &self.order {
            world[i] = match self.skeleton.bones[i].parent {
                None => local[i],
                Some(p) => skl::matmul(&local[i], &world[p]),
            };
        }
        world
    }

    /// Skinning matrices `inverse(bind_world) * pose_world`: they move a bind-pose model-space vertex to its posed
    /// position (row-vector convention).
    pub fn skin_matrices(&self, world: &[Mat4]) -> Vec<Mat4> {
        world.iter().zip(&self.inverse_bind).map(|(w, ib)| skl::matmul(ib, w)).collect()
    }

    /// Skinning matrices for `clip` at frame time `t`.
    pub fn skin(&self, clip: Option<&Clip>, t: f64) -> Vec<Mat4> {
        self.skin_matrices(&self.world_pose(&self.local_pose(clip, t)))
    }
}

/// Bones a set's masks address (the width of its widest mask); the target skeleton needs at least this many.
pub fn mask_bones(set: &Fas) -> usize {
    set.anims.iter().flat_map(|a| a.masks.iter().flatten()).map(|m| m.nbits as usize).max().unwrap_or(0)
}

pub use fas::Channel as FasChannel;

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &Mat4, b: &Mat4) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-9)
    }

    #[test]
    fn euler_matches_axis_products() {
        let rx = euler_row(0.3, 0.0, 0.0);
        assert!((rx[1][1] - 0.3f64.cos()).abs() < 1e-12 && (rx[1][2] - 0.3f64.sin()).abs() < 1e-12);
        let r = euler_row(0.1, 0.2, 0.3);
        // rows stay orthonormal
        for row in r {
            let n: f64 = row.iter().map(|v| v * v).sum();
            assert!((n - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn inverse_round_trips() {
        let r = euler_row(0.4, -0.7, 1.1);
        let m: Mat4 = [
            r[0][0] * 2.0,
            r[0][1] * 2.0,
            r[0][2] * 2.0,
            0.0,
            r[1][0],
            r[1][1],
            r[1][2],
            0.0,
            r[2][0],
            r[2][1],
            r[2][2],
            0.0,
            3.0,
            -1.0,
            5.0,
            1.0,
        ];
        assert!(close(&skl::matmul(&m, &affine_inverse(&m)), &IDENTITY));
    }

    #[test]
    fn bind_pose_skins_to_identity_and_channels_override() {
        use qwc_formats::skl::Bone;
        let mk = |parent, t: [f32; 3]| Bone {
            index: 0,
            name: String::new(),
            local: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, t[0], t[1], t[2], 1.0],
            flags: 0,
            parent,
            child: None,
            sibling: None,
        };
        let bones = vec![mk(None, [0.0, 1.0, 0.0]), mk(Some(0), [0.0, 2.0, 0.0])];
        let world = vec![
            bones[0].local.map(f64::from),
            skl::matmul(&bones[1].local.map(f64::from), &bones[0].local.map(f64::from)),
        ];
        let rig = Rig::new(Skeleton { bones, world });
        for m in rig.skin(None, 0.0) {
            assert!(close(&m, &IDENTITY));
        }
        // Rotate the root 90 degrees about Z: the child (local (0, 2, 0) under the root at (0, 1, 0)) moves to (-2, 1, 0).
        let clip = Clip {
            name: "t".into(),
            frames: 10,
            fps: 30.0,
            rotation: vec![Channel { bone: 0, keys: vec![[0.0, 0.0, std::f64::consts::FRAC_PI_2]], times: vec![0] }],
            ..Default::default()
        };
        let world = rig.world_pose(&rig.local_pose(Some(&clip), 0.0));
        let p = skl::transform(&world[1], [0.0; 3]);
        assert!((p[0] + 2.0).abs() < 1e-9 && (p[1] - 1.0).abs() < 1e-9, "{p:?}");
        assert!((clip.frame_at(0.5) - 5.0).abs() < 1e-9 && (clip.frame_at(1.0) - 10.0 % 10.0).abs() < 1e-9);
    }
}
