//! Morph targets drawn: a skinned model's face, hands and broom tail moved by its character's weights each frame.
//!
//! A character's morph weights are one array (`0x492bd0`: a draw's slot `k` uses `weights[weight_base + k]`), each
//! skeleton node with morph targets owning a run of it ([`Skeleton::morph_bases`]); the clips' morph tracks set them
//! by node and target (`0x40d540`). The draws add their targets to their vertices before skinning (`0x50ce10`); here
//! the finest level's morphed meshes are rebuilt on the CPU and their vertices uploaded again.
//!
//! [`Skeleton::morph_bases`]: qwc_formats::skl::Skeleton::morph_bases

use std::cell::Cell;

use qwc_anim::Clip;
use qwc_assets::model::{Mesh, Model};
use qwc_formats::skl::Skeleton;
use qwc_render::{wgpu, GpuModel, Renderer};

pub struct Face {
    /// The finest level's meshes with morph targets, by their upload index, at their bind positions.
    meshes: Vec<(usize, Mesh)>,
    /// The model skeleton's nodes with morph targets: name, first weight channel, count.
    nodes: Vec<(String, u16, u16)>,
    /// The meshes were last uploaded unmorphed.
    rest: Cell<bool>,
}

impl Face {
    /// A skinned model's morphed meshes; none without any.
    pub fn new(model: &Model) -> Option<Face> {
        let (_, sk) = model.skeleton.as_ref()?;
        let meshes: Vec<(usize, Mesh)> = model.morph_meshes().into_iter().map(|(i, m)| (i, m.clone())).collect();
        if meshes.is_empty() {
            return None;
        }
        let nodes = (sk.bones.iter().zip(sk.morph_bases()))
            .filter(|(b, _)| b.flags != 0)
            .map(|(b, base)| (b.name.clone(), base, b.flags.min(0xff) as u16))
            .collect();
        Some(Face { meshes, nodes, rest: Cell::new(true) })
    }

    /// The weight channel of morph `k` of the node named `node` (of any skeleton: the nodes match by name, the
    /// clips' sets and the models having the two character rigs' node orders).
    pub fn channel(&self, node: &str, k: u16) -> Option<u16> {
        let &(_, base, count) = self.nodes.iter().find(|(n, _, _)| n.eq_ignore_ascii_case(node))?;
        (k < count).then_some(base + k)
    }

    /// The weights clip `clip`'s morph tracks set at `frame` (`0x40d540` for one layer of mode 1 at weight 1), its
    /// nodes named by `skeleton` (its set's).
    pub fn weights(&self, clip: &Clip, skeleton: &Skeleton, frame: f32) -> Vec<f32> {
        let mut weights = Vec::new();
        for t in &clip.morphs {
            let node = skeleton.bones.get(t.node as usize).map(|b| b.name.as_str());
            if let (Some(c), Some(v)) = (node.and_then(|n| self.channel(n, t.morph)), t.sample(frame)) {
                set(&mut weights, c, v);
            }
        }
        weights
    }

    /// Upload the meshes moved by `weights` (by channel); nothing while they stay unmorphed.
    pub fn apply(&self, queue: &wgpu::Queue, renderer: &Renderer, model: &GpuModel, weights: &[f32]) {
        let rest = weights.iter().all(|&w| w <= 0.0);
        if rest && self.rest.get() {
            return;
        }
        self.rest.set(rest);
        for (i, m) in &self.meshes {
            let p = m.morphed(|c| weights.get(c as usize).copied().unwrap_or(0.0));
            renderer.set_positions(queue, model, *i, m, &p);
        }
    }
}

/// Set channel `c` of `weights` to `v`, growing it as needed.
pub fn set(weights: &mut Vec<f32>, c: u16, v: f32) {
    let c = c as usize;
    if weights.len() <= c {
        weights.resize(c + 1, 0.0);
    }
    weights[c] = v;
}
