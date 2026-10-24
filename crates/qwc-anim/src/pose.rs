//! The pose (`docs/ANIMATION.md`, "The pose"): every frame the state's playing clips are evaluated in layer order
//! into working buffers and the result written to the model's nodes ([`Pose`]).
//!
//! - `0x412970` unions the clips' bone masks per layer. Bones a blending layer (mode 2, 4 or 8) touches that no
//!   layer under it does are seeded from the nodes, so the blend has something to blend with.
//! - `0x413010` -> `0x40d540`: the morph-weight tracks, blended straight into the nodes by each layer's mode.
//! - `0x413130` -> `0x40d8e0` -> `0x40b550`: the rotation, translation and scale channels. The first clip of the
//!   pose sets what it touches; later ones meet bones already set and combine by their layer's mode (1 sets, 2
//!   blends by the weight, 4 adds by it, 8 blends by the bone's weight). Then every bone a clip touches is written
//!   to its node, translations and scales times the model's scale; and when any clip has visibility tracks, every
//!   node's visibility.

use crate::play::State;
use crate::Clip;

/// The model's nodes as the animation leaves them.
#[derive(Debug, Clone, PartialEq)]
pub struct Pose {
    /// The angles last set (radians, `Rx·Ry·Rz`); `None` until a clip rotates the bone, which keeps its bind
    /// rotation until then.
    pub rotation: Vec<Option<[f32; 3]>>,
    pub translation: Vec<[f32; 3]>,
    pub scale: Vec<[f32; 3]>,
    pub visible: Vec<bool>,
    /// Morph weights per node, as many as a clip has set. **[I]** the model's own count would bound them.
    pub morphs: Vec<Vec<f32>>,
    /// `0x412970`'s bounds of the clips playing (the model's `+0x14` hears them): min then max.
    pub bounds: Option<[f32; 6]>,
    /// The control's model scale (`+0x98`). **[I]** where it is set is not traced; 1 for now.
    pub model_scale: [f32; 3],
    /// The rest angles a blending layer's rotations are seeded from (`0x413130`: the state's `+0x28` object's
    /// `+0x18`, else 0). **[I]** their writer is not traced; a bone without one seeds at 0.
    pub rest: Vec<Option<[f32; 3]>>,
}

impl Pose {
    /// The nodes at their bind translations and scales, visible, with no morph weights.
    pub fn new(translation: Vec<[f32; 3]>, scale: Vec<[f32; 3]>, morphs: Vec<usize>) -> Pose {
        let n = translation.len();
        Pose {
            rotation: vec![None; n],
            translation,
            scale,
            visible: vec![true; n],
            morphs: morphs.into_iter().map(|k| vec![0.0; k]).collect(),
            bounds: None,
            model_scale: [1.0; 3],
            rest: vec![None; n],
        }
    }

    /// Gives bone `i` the rest angles `r`, and starts it there.
    pub fn set_rest(&mut self, i: usize, r: [f32; 3]) {
        if i < self.bone_count() {
            self.rest[i] = Some(r);
            self.rotation[i] = Some(r);
        }
    }

    pub fn bone_count(&self) -> usize {
        self.translation.len()
    }

    /// `0x478b60`: every morph weight moves `by` towards 0 (characters, each update, by `MorphTargetReduceAmount`).
    pub fn decay_morphs(&mut self, by: f32) {
        for w in self.morphs.iter_mut().flatten().filter(|w| **w != 0.0) {
            *w = if w.abs() <= by { 0.0 } else { *w + if 0.0 < *w { -by } else { by } };
        }
    }

    /// Not the original's (which draws each tick's pose until the next): the pose `t` of the way from `prev` to
    /// `self`, to draw between ticks. Angles, translations, scales and morph weights go in a straight line (the
    /// clips' keys are linear in the same angles, so within a key's span this is the clip sampled between the
    /// ticks); an angle by the shorter way round. At `t` 1 it is `self`. A bone or node the two do not both have,
    /// and the visibility and bounds, are `self`'s.
    pub fn blend(&self, prev: &Pose, t: f32) -> Pose {
        if 1.0 <= t {
            return self.clone();
        }
        let t = t.max(0.0);
        let lerp = |a: f32, b: f32| (b - a) * t + a;
        let l3 = |a: &[f32; 3], b: &mut [f32; 3]| *b = std::array::from_fn(|k| lerp(a[k], b[k]));
        let mut out = self.clone();
        for (r, p) in out.rotation.iter_mut().zip(&prev.rotation) {
            if let (Some(r), Some(p)) = (r.as_mut(), p) {
                for k in 0..3 {
                    let d =
                        (r[k] - p[k] + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
                    r[k] = p[k] + d * t;
                }
            }
        }
        out.translation.iter_mut().zip(&prev.translation).for_each(|(v, p)| l3(p, v));
        out.scale.iter_mut().zip(&prev.scale).for_each(|(v, p)| l3(p, v));
        for (ws, p) in out.morphs.iter_mut().zip(&prev.morphs).filter(|(w, p)| w.len() == p.len()) {
            ws.iter_mut().zip(p).for_each(|(w, p)| *w = lerp(*p, *w));
        }
        out
    }
}

/// `[0x601be0]` `MorphTargetReduceAmount` (an `AnimCtrl` tweak, `0x475f90`): 1/24 a call.
pub const MORPH_REDUCE: f32 = 1.0 / 24.0;

/// The evaluation's working buffers (`[0x6787c4]`: rotations, translations, scales, and the bones set, `[0x67c7b8]`).
/// **[I]** the original shares one set between every state; a bone written by no clip of this pose then keeps a
/// value from whichever state ran last, where the port keeps this state's own.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Buffers {
    rotation: Vec<[f32; 3]>,
    translation: Vec<[f32; 3]>,
    scale: Vec<[f32; 3]>,
    written: Vec<bool>,
}

/// `0x412970`'s results (the state's `+0x1c`-`+0x3c` masks).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Masks {
    /// Every bone a playing clip has in each mask: rotation, translation, scale, morph nodes, visibility nodes.
    pub touched: [Vec<u32>; 5],
    /// Rotation, translation or scale (`+0x28`).
    pub any: Vec<u32>,
    /// Rotation, translation and scale bones to seed (`+0x34`-`+0x3c`).
    pub seeds: [Vec<u32>; 3],
    pub bounds: Option<[f32; 6]>,
}

fn or_into(dst: &mut Vec<u32>, src: &[u32]) {
    if dst.len() < src.len() {
        dst.resize(src.len(), 0);
    }
    for (d, s) in dst.iter_mut().zip(src) {
        *d |= s;
    }
}

fn bit(m: &[u32], i: usize) -> bool {
    m.get(i >> 5).is_some_and(|w| w >> (i & 31) & 1 != 0)
}

fn bits(m: &[u32]) -> impl Iterator<Item = usize> + '_ {
    (0..m.len() * 32).filter(move |&i| bit(m, i))
}

impl State {
    /// The front clip of every playing channel, in layer order, with its layer.
    fn playing(&self) -> impl Iterator<Item = (usize, f32, &Clip)> + '_ {
        self.order.iter().flat_map(move |&l| {
            self.layers[l].channels.iter().filter_map(move |c| {
                let anim = c.queue.first()?.data.as_ref()?.anim.as_ref()?;
                Some((l, c.frame, anim))
            })
        })
    }

    /// `0x412970`: the masks of the clips playing, by layer.
    pub fn masks(&self) -> Masks {
        let mut m = Masks::default();
        for &l in &self.order {
            let layer = &self.layers[l];
            let mut t: [Vec<u32>; 5] = Default::default();
            let mut any = false;
            for c in &layer.channels {
                // **[I]** a clip the port has no tracks for has no masks
                let Some(anim) = c.queue.first().and_then(|e| e.data.as_ref()?.anim.as_ref()) else { continue };
                for (k, src) in [0, 1, 2, 4, 5].into_iter().enumerate() {
                    or_into(&mut t[k], &anim.masks[src]);
                }
                any = true;
                // without a rig-to-rig mapping the bounds are the clip's own (with one, 15 wider)
                let b = anim.bbox;
                m.bounds = Some(match m.bounds {
                    None => b,
                    Some(o) => {
                        [o[0].min(b[0]), o[1].min(b[1]), o[2].min(b[2]), o[3].max(b[3]), o[4].max(b[4]), o[5].max(b[5])]
                    }
                });
            }
            if !any {
                continue;
            }
            if layer.mode & 0xe != 0 {
                for ((seed, layer_bits), below) in m.seeds.iter_mut().zip(&t).zip(&m.touched) {
                    let new: Vec<u32> =
                        layer_bits.iter().enumerate().map(|(i, &w)| w & !below.get(i).copied().unwrap_or(0)).collect();
                    or_into(seed, &new);
                }
            }
            for (touched, layer_bits) in m.touched.iter_mut().zip(&t) {
                or_into(touched, layer_bits);
            }
        }
        for k in 0..3 {
            let t = m.touched[k].clone();
            or_into(&mut m.any, &t);
        }
        m
    }

    /// `0x478a10`'s pose: the morph tracks (`0x413010`), then the bones (`0x413130`), into `pose`.
    pub fn evaluate(&mut self, pose: &mut Pose) {
        let masks = self.masks();
        pose.bounds = masks.bounds;
        let playing: Vec<(usize, f32, Clip)> = self.playing().map(|(l, f, c)| (l, f, c.clone())).collect();
        for (l, frame, clip) in &playing {
            let layer = &self.layers[*l];
            morphs(clip, *frame, layer.mode, layer.weight, pose);
        }
        let n = pose.bone_count();
        let ms = pose.model_scale;
        let b = &mut self.buffers;
        b.rotation.resize(n, [0.0; 3]);
        b.translation.resize(n, [0.0; 3]);
        b.scale.resize(n, [0.0; 3]);
        b.written.resize(n, false);
        if masks.seeds.iter().any(|s| s.iter().any(|&w| w != 0)) {
            // the rotations seed from the rest angles when there are any, else 0
            for i in bits(&masks.seeds[0]).filter(|&i| i < n) {
                b.rotation[i] = pose.rest.get(i).copied().flatten().unwrap_or([0.0; 3]);
            }
            for i in bits(&masks.seeds[1]).filter(|&i| i < n) {
                b.translation[i] = std::array::from_fn(|k| pose.translation[i][k] / ms[k]);
            }
            for i in bits(&masks.seeds[2]).filter(|&i| i < n) {
                b.scale[i] = std::array::from_fn(|k| pose.scale[i][k] / ms[k]);
            }
        }
        let mut vis = vec![true; n];
        let mut any_vis = false;
        let mut first = true;
        for (l, frame, clip) in &playing {
            let layer = &self.layers[*l];
            let mode = if first {
                b.written.iter_mut().for_each(|w| *w = false);
                first = false;
                1
            } else {
                layer.mode
            };
            let weights = Weights { weight: layer.weight, bones: layer.bone_weights.as_deref() };
            bones(clip, *frame, mode, weights, b);
            if !clip.visibility.is_empty() {
                any_vis = true;
                for t in &clip.visibility {
                    if let Some(v) = vis.get_mut(t.node as usize) {
                        *v = t.visible(*frame);
                    }
                }
            }
        }
        for i in 0..n {
            if !bit(&masks.any, i) {
                continue;
            }
            let (rot, scale) = (bit(&masks.touched[0], i), bit(&masks.touched[2], i));
            if scale {
                b.scale[i] = std::array::from_fn(|k| ms[k] * b.scale[i][k]);
                pose.scale[i] = b.scale[i];
            }
            if rot {
                pose.rotation[i] = Some(b.rotation[i]);
            }
            if bit(&masks.touched[1], i) {
                b.translation[i] = std::array::from_fn(|k| ms[k] * b.translation[i][k]);
                pose.translation[i] = b.translation[i];
            }
        }
        if any_vis {
            pose.visible = vis;
        }
    }
}

#[derive(Clone, Copy)]
struct Weights<'a> {
    weight: f32,
    bones: Option<&'a [f32]>,
}

/// `0x40b550` for one value: the first write of the pose sets it, later ones combine by mode.
fn combine(out: &mut [f32; 3], written: &mut bool, v: [f32; 3], mode: u32, w: Weights, bone: usize) {
    if !*written {
        *out = v;
        *written = true;
        return;
    }
    let lerp = |out: &mut [f32; 3], w: f32| {
        for k in 0..3 {
            out[k] = v[k] * w + (1.0 - w) * out[k];
        }
    };
    match mode {
        1 => *out = v,
        2 => lerp(out, w.weight),
        4 => {
            for k in 0..3 {
                out[k] += v[k] * w.weight;
            }
        }
        // **[I]** the original passes the layer's weight where this mode reads one per bone
        8 => lerp(out, w.bones.and_then(|b| b.get(bone).copied()).unwrap_or(w.weight)),
        _ => {}
    }
}

/// `0x40d8e0`'s three codec slots.
fn bones(clip: &Clip, frame: f32, mode: u32, w: Weights, b: &mut Buffers) {
    let slots = [(&clip.rotation, 0), (&clip.translation, 1), (&clip.scale, 2)];
    for (channels, slot) in slots {
        for ch in channels.iter() {
            let i = ch.bone as usize;
            if i >= b.written.len() || ch.keys.is_empty() {
                continue;
            }
            let v = ch.sample(frame as f64, clip.frames).map(|x| x as f32);
            let out = match slot {
                0 => &mut b.rotation[i],
                1 => &mut b.translation[i],
                _ => &mut b.scale[i],
            };
            combine(out, &mut b.written[i], v, mode, w, i);
        }
    }
}

/// `0x40d540` (version 10 on): each morph track into its node's weights by the layer's mode (1 sets `value ×
/// weight`, 2 blends by the weight, 4 adds by it).
fn morphs(clip: &Clip, frame: f32, mode: u32, weight: f32, pose: &mut Pose) {
    let n = pose.bone_count();
    for t in &clip.morphs {
        let node = t.node as usize;
        if node >= n {
            continue;
        }
        let Some(v) = t.sample(frame) else { continue };
        if pose.morphs.len() < n {
            pose.morphs.resize(n, Vec::new());
        }
        let ws = &mut pose.morphs[node];
        let k = t.morph as usize;
        if ws.len() <= k {
            ws.resize(k + 1, 0.0);
        }
        match mode {
            1 => ws[k] = v * weight,
            2 => ws[k] = v * weight + (1.0 - weight) * ws[k],
            4 => ws[k] += v * weight,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use qwc_formats::fas::{Channel, MorphTrack, VisTrack};

    use super::*;
    use crate::play::{flag, ClipData, ClipInfo};

    const INFO: ClipInfo = ClipInfo { first: 0, last: 10, fps: 10.0 };

    fn clip(rot: f64, trans: Option<f64>) -> Arc<ClipData> {
        let ch = |bone, v: f64| Channel { bone, keys: vec![[v, 0.0, 0.0]], times: vec![0] };
        let mut c = Clip { frames: 10, fps: 10.0, rotation: vec![ch(0, rot), ch(1, rot)], ..Default::default() };
        c.masks[0] = vec![0b11];
        if let Some(t) = trans {
            c.translation = vec![ch(1, t)];
            c.masks[1] = vec![0b10];
        }
        Arc::new(ClipData { anim: Some(c), triggers: vec![] })
    }

    fn state(modes: &[(u32, f32)]) -> State {
        let mut s = State::default();
        for &(mode, weight) in modes {
            let l = s.add_layer();
            s.layers[l].mode = mode;
            s.layers[l].weight = weight;
        }
        s
    }

    fn play(s: &mut State, l: usize, data: Arc<ClipData>) {
        let id = s.layers[l].channels[0].id;
        s.play(id, flag::LOOP, 1.0, INFO, 0, None, Some(data), None, None);
    }

    fn pose() -> Pose {
        Pose::new(vec![[0.0, 5.0, 0.0]; 2], vec![[1.0; 3]; 2], vec![])
    }

    #[test]
    fn the_first_clip_sets_and_later_layers_blend() {
        let mut s = state(&[(2, 0.25), (2, 0.5)]);
        play(&mut s, 0, clip(1.0, None));
        play(&mut s, 1, clip(3.0, Some(2.0)));
        let mut p = pose();
        s.evaluate(&mut p);
        // the first clip sets whatever its layer's mode and weight; the second blends half way
        assert_eq!(p.rotation[0], Some([2.0, 0.0, 0.0]));
        // bone 1's translation: no layer under the second gives one, so it is seeded from the node (5 in y) and
        // blended half way to (2, 0, 0)
        assert_eq!(p.translation[1], [1.0, 2.5, 0.0]);
        // a layer that adds
        let mut s = state(&[(1, 1.0), (4, 0.5)]);
        play(&mut s, 0, clip(1.0, None));
        play(&mut s, 1, clip(3.0, None));
        let mut p = pose();
        s.evaluate(&mut p);
        assert_eq!(p.rotation[1], Some([2.5, 0.0, 0.0]));
        // nothing playing leaves the nodes alone
        let mut p = pose();
        State::default().evaluate(&mut p);
        assert_eq!(p, pose());
    }

    #[test]
    fn morphs_and_visibility_reach_the_nodes() {
        let mut c = Clip { frames: 10, fps: 10.0, ..Default::default() };
        c.morphs = vec![MorphTrack {
            node: 1,
            morph: 2,
            values: vec![192, 64],
            times: vec![0, 10],
            wide: false,
            stream: None,
        }];
        c.visibility = vec![VisTrack { node: 0, times: vec![5] }];
        let mut s = state(&[(2, 1.0)]);
        play(&mut s, 0, Arc::new(ClipData { anim: Some(c), triggers: vec![] }));
        let mut p = pose();
        s.evaluate(&mut p);
        // 192 / 128 - 0.5 = 1 at frame 0
        assert_eq!(p.morphs[1], [0.0, 0.0, 1.0]);
        assert_eq!(p.visible, [true, true]);
        let (mut ended, mut fired) = (Vec::new(), Vec::new());
        s.step(0.5, Some(&mut p), &mut ended, &mut fired);
        s.evaluate(&mut p);
        // half way to 64 / 128 - 0.5 = 0; node 0 hidden from frame 5
        assert_eq!(p.morphs[1][2], 0.5);
        assert_eq!(p.visible, [false, true]);
    }

    #[test]
    fn a_morph_clip_ending_fades_the_face() {
        let mut c = Clip { frames: 10, fps: 10.0, ..Default::default() };
        c.morphs = vec![MorphTrack { node: 1, morph: 0, values: vec![192], times: vec![0], wide: false, stream: None }];
        let mut s = state(&[(2, 1.0)]);
        let id = s.layers[0].channels[0].id;
        s.play(id, 0, 1.0, INFO, 0, None, Some(Arc::new(ClipData { anim: Some(c), triggers: vec![] })), None, None);
        let mut p = pose();
        s.evaluate(&mut p);
        let (mut ended, mut fired) = (Vec::new(), Vec::new());
        // the clip ends in the step after 1 s, which then fades by its whole time at 4/3 a second: 3/8 s, half
        s.step(1.0, Some(&mut p), &mut ended, &mut fired);
        assert_eq!(p.morphs[1][0], 1.0);
        s.step(0.375, Some(&mut p), &mut ended, &mut fired);
        assert!((p.morphs[1][0] - 0.5).abs() < 1e-6, "{:?}", p.morphs[1]);
        s.step(1.0, Some(&mut p), &mut ended, &mut fired);
        assert_eq!(p.morphs[1][0], 0.0);
        // a character's own decay: 1/24 towards 0 a call, to 0 within it
        p.morphs[1] = vec![0.5, -0.5, 0.01];
        p.decay_morphs(MORPH_REDUCE);
        assert_eq!(p.morphs[1], [0.5 - MORPH_REDUCE, -0.5 + MORPH_REDUCE, 0.0]);
    }

    #[test]
    fn a_pose_blends_between_ticks() {
        let mut a = Pose::new(vec![[0.0; 3]; 2], vec![[1.0; 3]; 2], vec![1, 0]);
        let mut b = a.clone();
        a.rotation[0] = Some([0.0, 3.0, -0.5]);
        b.rotation[0] = Some([1.0, -3.0, 0.5]);
        b.rotation[1] = Some([2.0; 3]);
        b.translation[1] = [4.0, 0.0, -2.0];
        b.scale[0] = [3.0; 3];
        b.morphs[0] = vec![0.5];
        b.visible[1] = false;
        let m = b.blend(&a, 0.5);
        // y from 3 to -3 the short way, through pi
        let r = m.rotation[0].unwrap();
        assert!((r[0] - 0.5).abs() < 1e-6 && (r[1] - std::f32::consts::PI).abs() < 1e-5 && r[2].abs() < 1e-6);
        // a bone only the newer pose turns takes its angles
        assert_eq!(m.rotation[1], Some([2.0; 3]));
        assert_eq!((m.translation[1], m.scale[0], m.morphs[0][0]), ([2.0, 0.0, -1.0], [2.0; 3], 0.25));
        assert!(!m.visible[1]);
        assert_eq!(b.blend(&a, 1.0), b);
    }
}
