//! Engine-ready models: an `.fpm` turned into triangle lists with materials, a skeleton and morph targets.
//!
//! This is the mesh-building half of `tools/fpm_to_gltf.py`, kept to the same rules:
//! - strips become triangle lists (`strip_to_tris`); the draw's material is the low 16 bits of its `tex` word;
//! - skinned vertices (`fpm::SKINNED_FVF`) are stored bone-local with one bone per vertex (`palette[byte 32]`);
//!   they are moved into model space with the skeleton's bind pose, so the mesh is ready to draw unanimated and
//!   skins with `pose_world * inverse(bind_world)` per bone;
//! - the skeleton is picked from the model's folder, then by name, then the two character rigs, by the fewest
//!   folded joint triangles ([`fold_score`]) with ties broken by the shorter mean seam edge ([`seam_score`]);
//! - morph deltas are bone-local too and are rotated into model space the same way;
//! - every level of detail is kept, each mesh marked with its own ([`Mesh::level`]) and whether it is drawn from
//!   both sides ([`Mesh::two_sided`]), and each material carries how the original draws it ([`Material::surface`],
//!   from its name's flags).

use std::collections::BTreeMap;

use qwc_formats::fpm::{self, Draw, Fpm, SKINNED_FVF};
use qwc_formats::skl::{self, Skeleton};

use crate::texture::TextureIndex;
use crate::vfs::{normalize, Error, Result, Vfs};

/// The character rigs every player model can use (`tools/fpm_to_gltf.py` `CHARACTER_SKELETONS`).
pub const CHARACTER_SKELETONS: [&str; 2] =
    ["frontend/Common/Misc/sma_common.skl", "frontend/Common/Misc/med_common.skl"];

#[derive(Debug, Clone)]
pub struct Material {
    pub name: String,
    /// Path of the `.fsh` holding the texture, when one was found.
    pub texture: Option<String>,
    /// A decoded image that replaces `texture` (edited models carry their own, see [`crate::glb`]).
    pub image: Option<std::sync::Arc<crate::texture::Texture>>,
    /// How the original draws it (its name's flags in the `.fpm`); `None` for an edited model's material.
    pub surface: Option<fpm::Surface>,
    /// How its Groper material draws it instead, when the name has one ([`crate::groper::Render`]).
    pub groper: Option<crate::groper::Render>,
}

#[derive(Debug, Clone, Default)]
pub struct Mesh {
    pub material: usize,
    /// The level of detail its draw is in (0 the finest; `qwc_formats::fpm` on the groups' children).
    pub level: usize,
    /// Drawn from both sides (`qwc_formats::fpm::Draw::two_sided`); culled otherwise.
    pub two_sided: bool,
    /// Model-space positions (bind pose for skinned draws).
    pub positions: Vec<[f32; 3]>,
    pub normals: Option<Vec<[f32; 3]>>,
    /// Vertex colours as RGBA.
    pub colors: Option<Vec<[u8; 4]>>,
    pub uvs: Vec<[f32; 2]>,
    /// Skeleton bone per vertex (skinned draws only).
    pub bones: Option<Vec<u16>>,
    /// Triangle list, counter-clockwise front faces.
    pub indices: Vec<u32>,
    /// Morph weight channel -> model-space position deltas by vertex.
    pub morphs: BTreeMap<u16, Vec<(u32, [f32; 3])>>,
}

impl Mesh {
    /// Its positions with each morph target added by its channel's weight (`weight`, 0 for a channel it has not;
    /// `0x492bd0` skips weights of 0 or less), as `0x50ce10` adds them before skinning.
    pub fn morphed(&self, weight: impl Fn(u16) -> f32) -> Vec<[f32; 3]> {
        let mut p = self.positions.clone();
        for (&c, deltas) in &self.morphs {
            let w = weight(c);
            if w <= 0.0 {
                continue;
            }
            for &(v, d) in deltas {
                if let Some(p) = p.get_mut(v as usize) {
                    for k in 0..3 {
                        p[k] += w * d[k];
                    }
                }
            }
        }
        p
    }
}

#[derive(Debug, Clone)]
pub struct Model {
    pub path: String,
    /// The root node matrix (row-vector convention); the exporter applies it to unskinned models only.
    pub matrix: [f32; 16],
    pub materials: Vec<Material>,
    /// Path and bind pose of the skeleton, for skinned models.
    pub skeleton: Option<(String, Skeleton)>,
    pub meshes: Vec<Mesh>,
}

impl Model {
    /// Axis-aligned bounds of every mesh (as placed by [`Model::matrix`] when unskinned).
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        let mut it = self.meshes.iter().flat_map(|m| m.positions.iter()).map(|&p| self.place(p));
        let first = it.next()?;
        Some(it.fold((first, first), |(lo, hi), p| {
            (std::array::from_fn(|k| lo[k].min(p[k])), std::array::from_fn(|k| hi[k].max(p[k])))
        }))
    }

    /// Model-space point to the space the model is drawn in.
    pub fn place(&self, p: [f32; 3]) -> [f32; 3] {
        if self.skeleton.is_some() {
            return p;
        }
        let m = self.matrix.map(f64::from);
        skl::transform(&m, p.map(f64::from)).map(|v| v as f32)
    }

    /// The finest level's meshes with morph targets, each with its index among the finest level's meshes (the
    /// renderer's upload order).
    pub fn morph_meshes(&self) -> Vec<(usize, &Mesh)> {
        self.meshes.iter().filter(|m| m.level == 0).enumerate().filter(|(_, m)| !m.morphs.is_empty()).collect()
    }

    /// Every morph weight channel the model uses, sorted.
    pub fn morph_channels(&self) -> Vec<u16> {
        let mut v: Vec<u16> = self.meshes.iter().flat_map(|m| m.morphs.keys().copied()).collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}

fn fmt_err(path: &str, e: qwc_formats::Error) -> Error {
    Error::Format(path.into(), e)
}

fn f32x3(d: &[u8], off: usize) -> [f64; 3] {
    std::array::from_fn(|k| f32::from_le_bytes(d[off + 4 * k..off + 4 * k + 4].try_into().unwrap()) as f64)
}

/// Bone-local positions, normals and skeleton bones of a skinned draw.
struct SkinnedDraw {
    bone: Vec<usize>,
    pos: Vec<[f64; 3]>,
    nrm: Vec<[f64; 3]>,
    tris: Vec<[u16; 3]>,
}

fn skinned_draws(m: &Fpm, path: &str) -> Result<Vec<SkinnedDraw>> {
    let mut out = Vec::new();
    for dr in m.draws().filter(|d| d.geom.fvf == SKINNED_FVF) {
        let pal = m.palette(dr).map_err(|e| fmt_err(path, e))?.unwrap_or_default();
        let vb = m.vertex_bytes(dr).map_err(|e| fmt_err(path, e))?;
        let mut sd = SkinnedDraw { bone: vec![], pos: vec![], nrm: vec![], tris: vec![] };
        for v in vb.as_chunks::<36>().0 {
            let b = *pal.get(v[32] as usize).ok_or_else(|| Error::NotFound(format!("{path}: palette entry")))?;
            sd.bone.push(b as usize);
            sd.pos.push(f32x3(v, 0));
            sd.nrm.push(f32x3(v, 12));
        }
        sd.tris = fpm::strip_to_tris(&m.indices(dr).map_err(|e| fmt_err(path, e))?);
        out.push(sd);
    }
    Ok(out)
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// Joint triangles (vertices on two or more bones) whose face normal opposes their vertex normals in the
/// skeleton's bind pose. The wrong skeleton folds joints; the right one gives about zero.
fn fold_score(draws: &[SkinnedDraw], sk: &Skeleton) -> usize {
    let mut bad = 0;
    for d in draws {
        let pos: Vec<_> = d.pos.iter().zip(&d.bone).map(|(&p, &b)| skl::transform(&sk.world[b], p)).collect();
        let nrm: Vec<_> = d.nrm.iter().zip(&d.bone).map(|(&n, &b)| skl::transform_dir(&sk.world[b], n)).collect();
        for t in &d.tris {
            let [a, b, c] = t.map(usize::from);
            if d.bone[a] == d.bone[b] && d.bone[b] == d.bone[c] {
                continue;
            }
            let n = cross(sub(pos[b], pos[a]), sub(pos[c], pos[a]));
            let vn = [0, 1, 2].map(|k| nrm[a][k] + nrm[b][k] + nrm[c][k]);
            if dot(n, n) > 1e-18 && dot(n, vn) < 0.0 {
                bad += 1;
            }
        }
    }
    bad
}

/// Mean length of edges joining vertices on different bones, in the skeleton's bind pose.
fn seam_score(draws: &[SkinnedDraw], sk: &Skeleton) -> f64 {
    let (mut tot, mut n) = (0.0, 0usize);
    for d in draws {
        let pos: Vec<_> = d.pos.iter().zip(&d.bone).map(|(&p, &b)| skl::transform(&sk.world[b], p)).collect();
        for t in &d.tris {
            let [a, b, c] = t.map(usize::from);
            for (u, v) in [(a, b), (b, c), (a, c)] {
                if d.bone[u] != d.bone[v] {
                    tot += dot(sub(pos[u], pos[v]), sub(pos[u], pos[v])).sqrt();
                    n += 1;
                }
            }
        }
    }
    if n == 0 {
        0.0
    } else {
        tot / n as f64
    }
}

fn parent_dir(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

/// Candidate skeletons for a model, as `tools/fpm_to_gltf.py` `SkeletonResolver.candidates`.
pub fn skeleton_candidates(vfs: &Vfs, model_path: &str, max_bone: usize) -> Result<Vec<(String, Skeleton)>> {
    let norm = normalize(model_path);
    let folder = parent_dir(&norm).to_string();
    let stem = norm.rsplit('/').next().unwrap_or(&norm).rsplit_once('.').map_or(norm.as_str(), |(s, _)| s);
    let all: Vec<&str> = vfs.with_extension("skl").collect();
    let local: Vec<&str> = all.iter().copied().filter(|p| parent_dir(&normalize(p)) == folder).collect();
    let named: Vec<&str> =
        all.iter().copied().filter(|p| normalize(p).rsplit('/').next().unwrap_or("").starts_with(stem)).collect();
    let load = |p: &str| -> Result<Skeleton> {
        let b = vfs.read(p)?;
        Skeleton::parse(&b).map_err(|e| fmt_err(p, e))
    };
    for group in [local, named, CHARACTER_SKELETONS.to_vec()] {
        let mut ok = Vec::new();
        for p in group {
            let sk = load(p)?;
            if sk.bones.len() > max_bone {
                ok.push((p.to_string(), sk));
            }
        }
        if ok.is_empty() {
            continue;
        }
        if ok.iter().any(|(_, s)| s.bones.len() == 39) {
            // A local character rig: let both body sizes compete.
            for p in CHARACTER_SKELETONS {
                if !ok.iter().any(|(q, _)| normalize(q) == normalize(p)) {
                    ok.push((p.to_string(), load(p)?));
                }
            }
        }
        return Ok(ok);
    }
    Ok(Vec::new())
}

/// Pick the skeleton for a model's skinned draws (`None` when it has none).
fn choose_skeleton(vfs: &Vfs, m: &Fpm, path: &str) -> Result<Option<(String, Skeleton)>> {
    let draws = skinned_draws(m, path)?;
    let Some(max_bone) = draws.iter().flat_map(|d| d.bone.iter().copied()).max() else {
        return Ok(None);
    };
    let mut cands = skeleton_candidates(vfs, path, max_bone)?;
    if cands.is_empty() {
        return Err(Error::NotFound(format!("{path}: no skeleton with more than {max_bone} bones")));
    }
    if cands.len() == 1 {
        return Ok(cands.pop());
    }
    let scores: Vec<(usize, f64)> = cands.iter().map(|(_, s)| (fold_score(&draws, s), seam_score(&draws, s))).collect();
    let mut best = 0;
    for i in 1..scores.len() {
        if scores[i].0 < scores[best].0 || (scores[i].0 == scores[best].0 && scores[i].1 < scores[best].1) {
            best = i;
        }
    }
    Ok(Some(cands.swap_remove(best)))
}

fn build_mesh(m: &Fpm, dr: &Draw, sk: Option<&Skeleton>, path: &str) -> Result<Option<Mesh>> {
    let g = &dr.geom;
    let Some((stride, n_off, c_off, uv_off)) = fpm::layout(g.fvf) else {
        return Err(fmt_err(path, qwc_formats::Error::Format(format!("unknown vertex format {:#x}", g.fvf))));
    };
    let tris = fpm::strip_to_tris(&m.indices(dr).map_err(|e| fmt_err(path, e))?);
    if tris.is_empty() {
        return Ok(None);
    }
    let vb = m.vertex_bytes(dr).map_err(|e| fmt_err(path, e))?;
    let pal = if g.fvf == SKINNED_FVF { m.palette(dr).map_err(|e| fmt_err(path, e))? } else { None };
    let skin = pal.as_ref().zip(sk);
    let mut mesh = Mesh { material: (dr.tex & 0xFFFF) as usize, ..Default::default() };
    let mut normals = Vec::new();
    let mut colors = Vec::new();
    let mut bones = Vec::new();
    let vertex_bone = |v: &[u8]| -> Option<usize> { skin.map(|(pal, _)| pal[v[32] as usize] as usize) };
    for v in vb.chunks_exact(stride) {
        let mut p = f32x3(v, 0);
        let mut n = n_off.map(|o| f32x3(v, o));
        if let (Some(b), Some((_, sk))) = (vertex_bone(v), skin) {
            p = skl::transform(&sk.world[b], p);
            n = n.map(|n| skl::transform_dir(&sk.world[b], n));
            bones.push(b as u16);
        }
        mesh.positions.push(p.map(|x| x as f32));
        if let Some(n) = n {
            normals.push(n.map(|x| x as f32));
        }
        if let Some(o) = c_off {
            colors.push([v[o + 2], v[o + 1], v[o], v[o + 3]]); // stored B, G, R, A
        }
        mesh.uvs.push(std::array::from_fn(|k| {
            f32::from_le_bytes(v[uv_off + 4 * k..uv_off + 4 * k + 4].try_into().unwrap())
        }));
    }
    mesh.normals = (!normals.is_empty()).then_some(normals);
    mesh.colors = (!colors.is_empty()).then_some(colors);
    mesh.bones = skin.is_some().then_some(bones);
    mesh.indices = tris.iter().flat_map(|t| t.map(u32::from)).collect();
    if let Some(morphs) = m.morphs(dr).map_err(|e| fmt_err(path, e))? {
        for (k, slot) in morphs.slots.iter().enumerate() {
            let Some(deltas) = slot else { continue };
            let list = deltas
                .iter()
                .map(|(&v, &dv)| {
                    let dv = match skin {
                        Some((_, sk)) => {
                            let b = vertex_bone(&vb[v * stride..]).unwrap();
                            skl::transform_dir(&sk.world[b], dv)
                        }
                        None => dv,
                    };
                    (v as u32, dv.map(|x| x as f32))
                })
                .collect();
            mesh.morphs.insert(morphs.weight_base + k as u16, list);
        }
    }
    Ok(Some(mesh))
}

/// Load the model at `path`, resolving its skeleton and textures.
pub fn load(vfs: &Vfs, textures: &TextureIndex, path: &str) -> Result<Model> {
    let bytes = vfs.read(path)?;
    let m = Fpm::parse(&bytes).map_err(|e| fmt_err(path, e))?;
    let skeleton = choose_skeleton(vfs, &m, path)?;
    let materials = m
        .names
        .iter()
        .map(|n| Material {
            name: n.name.clone(),
            texture: textures.find(&n.name, path).map(str::to_string),
            image: None,
            surface: Some(n.surface()),
            groper: textures.render(&n.name, path),
        })
        .collect();
    let mut meshes = Vec::new();
    for (level, dr) in m.draws_with_level() {
        if let Some(mesh) = build_mesh(&m, dr, skeleton.as_ref().map(|(_, s)| s), path)? {
            meshes.push(Mesh { level, two_sided: dr.two_sided(), ..mesh });
        }
    }
    Ok(Model { path: path.to_string(), matrix: m.matrix, materials, skeleton, meshes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn morphed_adds_each_positive_weight_times_its_deltas() {
        let mut m = Mesh { positions: vec![[0.0; 3], [1.0, 1.0, 1.0]], ..Default::default() };
        m.morphs.insert(4, vec![(1, [2.0, 0.0, 0.0])]);
        m.morphs.insert(5, vec![(1, [0.0, 4.0, 0.0]), (0, [0.0, 0.0, 8.0])]);
        m.morphs.insert(6, vec![(0, [100.0; 3])]);
        let w = |c: u16| match c {
            4 => 0.5,
            5 => 0.25,
            6 => -1.0,
            _ => 0.0,
        };
        assert_eq!(m.morphed(w), [[0.0, 0.0, 2.0], [2.0, 2.0, 1.0]]);
    }
}
