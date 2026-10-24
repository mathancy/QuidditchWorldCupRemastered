//! A game model written as a binary glTF (`.glb`), to edit and bring back in a resource pack (QWC Studio's export,
//! `qwc-game export`). Not the original's: the remaster's own tool, a Rust writer in the spirit of
//! `tools/fpm_to_gltf.py`, and read back by [`crate::glb`].
//!
//! - Only the finest level of detail is written: the one a pack's replacement stands in for.
//! - Positions are the model's own (bind pose for a skinned model), in game space (+Y up), as the loader takes them;
//!   UVs as the game has them.
//! - A skinned model carries its skeleton: one node per bone (its local matrix, its children), every bone a joint
//!   with its inverse bind matrix, and each vertex on its one bone at weight 1 (the original's skinning).
//! - Morph targets: one per weight channel the model uses, named `morphNN` (the channel), on every primitive (zero
//!   where a primitive has none), so Blender keeps them as shape keys of those names and [`crate::glb::load`] maps
//!   them back.
//! - Materials keep their names (the loader finds the original's textures by them) and embed their textures as PNGs.
//!
//! The skeleton's matrices are row-vector, row-major: as an array that is glTF's column-major column-vector order, so
//! they are written as they are.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::model::Model;
use crate::Vfs;

/// What the export wrote.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ExportReport {
    pub meshes: usize,
    pub vertices: usize,
    pub joints: usize,
    pub morph_channels: usize,
    pub textures: usize,
    /// Materials whose texture could not be read (written untextured).
    pub missing_textures: Vec<String>,
}

/// The binary chunk and the JSON's accessors and views as they are added.
struct Writer {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Writer {
    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let mut v = json!({ "buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len() });
        if let Some(t) = target {
            v["target"] = json!(t);
        }
        self.bin.extend_from_slice(bytes);
        self.views.push(v);
        self.views.len() - 1
    }

    /// An accessor of `count` elements of `kind` over `bytes`; `normalized` for unit integers, `bounds` as min/max.
    #[allow(clippy::too_many_arguments)]
    fn accessor(
        &mut self,
        bytes: &[u8],
        component: u32,
        count: usize,
        kind: &str,
        target: Option<u32>,
        normalized: bool,
        bounds: Option<([f32; 3], [f32; 3])>,
    ) -> usize {
        let view = self.view(bytes, target);
        let mut a = json!({ "bufferView": view, "componentType": component, "count": count, "type": kind });
        if normalized {
            a["normalized"] = json!(true);
        }
        if let Some((lo, hi)) = bounds {
            a["min"] = json!(lo);
            a["max"] = json!(hi);
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }

    fn vec3(&mut self, v: &[[f32; 3]], target: Option<u32>) -> usize {
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in v {
            for i in 0..3 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
        if v.is_empty() {
            (lo, hi) = ([0.0; 3], [0.0; 3]);
        }
        let bytes: Vec<u8> = v.iter().flatten().flat_map(|f| f.to_le_bytes()).collect();
        self.accessor(&bytes, 5126, v.len(), "VEC3", target, false, Some((lo, hi)))
    }
}

/// PNG bytes of an RGBA8 image.
fn png_bytes(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, width, height);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        let mut w = e.write_header().ok()?;
        w.write_image_data(rgba).ok()?;
    }
    Some(out)
}

/// `model` as a `.glb`, its textures read from `vfs`.
pub fn export(vfs: &Vfs, model: &Model) -> (Vec<u8>, ExportReport) {
    let mut w = Writer { bin: Vec::new(), views: Vec::new(), accessors: Vec::new() };
    let mut report = ExportReport::default();
    let finest: Vec<&crate::model::Mesh> = model.meshes.iter().filter(|m| m.level == 0).collect();

    // materials, each with its texture as a PNG
    let (mut materials, mut images, mut textures) = (Vec::new(), Vec::new(), Vec::new());
    for (i, m) in model.materials.iter().enumerate() {
        let picture = match (&m.image, &m.texture) {
            (Some(t), _) => Some((t.width, t.height, t.levels[0].clone())),
            (None, Some(path)) => {
                crate::texture::load(vfs, path).ok().map(|t| (t.width, t.height, t.levels[0].clone()))
            }
            (None, None) => None,
        };
        let two_sided = finest.iter().any(|x| x.material == i && x.two_sided);
        let mut mat = json!({
            "name": m.name,
            "pbrMetallicRoughness": { "metallicFactor": 0.0, "roughnessFactor": 1.0 },
            "doubleSided": two_sided,
        });
        match picture.and_then(|(wd, ht, rgba)| Some((png_bytes(wd, ht, &rgba)?, rgba))) {
            Some((png, rgba)) => {
                let view = w.view(&png, None);
                images.push(json!({ "name": m.name, "mimeType": "image/png", "bufferView": view }));
                textures.push(json!({ "source": images.len() - 1, "sampler": 0 }));
                mat["pbrMetallicRoughness"]["baseColorTexture"] = json!({ "index": textures.len() - 1 });
                // cut-outs keep their holes
                if rgba.chunks(4).any(|p| p[3] < 250) {
                    mat["alphaMode"] = json!("MASK");
                    mat["alphaCutoff"] = json!(0.5);
                }
                report.textures += 1;
            }
            None => {
                if m.texture.is_some() || m.image.is_some() {
                    report.missing_textures.push(m.name.clone());
                }
            }
        }
        materials.push(mat);
    }

    // every morph channel the finest level uses, the same targets on every primitive
    let channels: Vec<u16> = {
        let mut c: Vec<u16> = finest.iter().flat_map(|m| m.morphs.keys().copied()).collect();
        c.sort_unstable();
        c.dedup();
        c
    };
    report.morph_channels = channels.len();
    let skinned = model.skeleton.is_some();
    let mut prims = Vec::new();
    for m in &finest {
        let n = m.positions.len();
        let mut attrs = serde_json::Map::new();
        attrs.insert("POSITION".into(), json!(w.vec3(&m.positions, Some(34962))));
        if let Some(normals) = m.normals.as_ref().filter(|v| v.len() == n) {
            attrs.insert("NORMAL".into(), json!(w.vec3(normals, Some(34962))));
        }
        if m.uvs.len() == n {
            let bytes: Vec<u8> = m.uvs.iter().flatten().flat_map(|f| f.to_le_bytes()).collect();
            attrs.insert("TEXCOORD_0".into(), json!(w.accessor(&bytes, 5126, n, "VEC2", Some(34962), false, None)));
        }
        if let Some(colors) = m.colors.as_ref().filter(|v| v.len() == n) {
            let bytes: Vec<u8> = colors.iter().flatten().copied().collect();
            attrs.insert("COLOR_0".into(), json!(w.accessor(&bytes, 5121, n, "VEC4", Some(34962), true, None)));
        }
        if let (true, Some(bones)) = (skinned, m.bones.as_ref().filter(|v| v.len() == n)) {
            let joints: Vec<u8> = bones.iter().flat_map(|&b| [b, 0, 0, 0]).flat_map(u16::to_le_bytes).collect();
            let weights: Vec<u8> = (0..n).flat_map(|_| [1f32, 0.0, 0.0, 0.0]).flat_map(f32::to_le_bytes).collect();
            attrs.insert("JOINTS_0".into(), json!(w.accessor(&joints, 5123, n, "VEC4", Some(34962), false, None)));
            attrs.insert("WEIGHTS_0".into(), json!(w.accessor(&weights, 5126, n, "VEC4", Some(34962), false, None)));
        }
        let idx: Vec<u8> = m.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
        let indices = w.accessor(&idx, 5125, m.indices.len(), "SCALAR", Some(34963), false, None);
        let mut prim = json!({ "attributes": attrs, "indices": indices, "mode": 4 });
        if m.material < model.materials.len() {
            prim["material"] = json!(m.material);
        }
        if !channels.is_empty() {
            let targets: Vec<Value> = channels
                .iter()
                .map(|c| {
                    let mut d = vec![[0f32; 3]; n];
                    for &(v, delta) in m.morphs.get(c).map_or(&[][..], |x| &x[..]) {
                        if let Some(slot) = d.get_mut(v as usize) {
                            *slot = delta;
                        }
                    }
                    json!({ "POSITION": w.vec3(&d, Some(34962)) })
                })
                .collect();
            prim["targets"] = json!(targets);
        }
        report.vertices += n;
        prims.push(prim);
    }
    report.meshes = prims.len();
    let mut mesh = json!({ "name": model.path, "primitives": prims });
    if !channels.is_empty() {
        let names: Vec<String> = channels.iter().map(|c| format!("morph{c:02}")).collect();
        mesh["extras"] = json!({ "targetNames": names });
        mesh["weights"] = json!(vec![0.0; channels.len()]);
    }

    // the skeleton's nodes, then the mesh's
    let mut nodes = Vec::new();
    let mut roots = Vec::new();
    let mut skins = Vec::new();
    if let Some((_, sk)) = &model.skeleton {
        let mut children: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (i, b) in sk.bones.iter().enumerate() {
            match b.parent {
                Some(p) => children.entry(p).or_default().push(i),
                None => roots.push(i),
            }
        }
        for (i, b) in sk.bones.iter().enumerate() {
            let mut node = json!({ "name": b.name, "matrix": b.local.iter().map(|&x| x as f64).collect::<Vec<_>>() });
            if let Some(c) = children.get(&i) {
                node["children"] = json!(c);
            }
            nodes.push(node);
        }
        let inverse: Vec<u8> = sk.world.iter().flat_map(invert).flat_map(|x| (x as f32).to_le_bytes()).collect();
        let ibm = w.accessor(&inverse, 5126, sk.bones.len(), "MAT4", None, false, None);
        skins.push(json!({
            "name": "skeleton",
            "joints": (0..sk.bones.len()).collect::<Vec<_>>(),
            "inverseBindMatrices": ibm,
            "skeleton": roots.first().copied().unwrap_or(0),
        }));
        report.joints = sk.bones.len();
    }
    let mut mesh_node = json!({ "name": model.path, "mesh": 0 });
    if skinned {
        mesh_node["skin"] = json!(0);
    } else if model.matrix != IDENTITY {
        mesh_node["matrix"] = json!(model.matrix.iter().map(|&x| x as f64).collect::<Vec<_>>());
    }
    nodes.push(mesh_node);
    let mut scene_nodes = roots.clone();
    scene_nodes.push(nodes.len() - 1);

    let mut doc = json!({
        "asset": { "version": "2.0", "generator": "QWC Remastered (qwc_assets::gltf_out)" },
        "scene": 0,
        "scenes": [{ "nodes": scene_nodes }],
        "nodes": nodes,
        "meshes": [mesh],
        "materials": materials,
        "accessors": w.accessors,
        "bufferViews": w.views,
        "buffers": [{ "byteLength": w.bin.len() }],
    });
    if !images.is_empty() {
        doc["images"] = json!(images);
        doc["textures"] = json!(textures);
        doc["samplers"] = json!([{ "magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497 }]);
    }
    if !skins.is_empty() {
        doc["skins"] = json!(skins);
    }
    (glb_bytes(&doc, w.bin), report)
}

const IDENTITY: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// The `.glb` container: the header, the JSON chunk (padded with spaces), the binary chunk (padded with zeros).
fn glb_bytes(doc: &Value, mut bin: Vec<u8>) -> Vec<u8> {
    let mut json = serde_json::to_vec(doc).expect("the document serialises");
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}

/// The inverse of a 4x4 matrix (any convention: the inverse of the transpose is the transpose of the inverse), by
/// cofactors; the identity for a singular one.
fn invert(m: &[f64; 16]) -> [f64; 16] {
    let a = |r: usize, c: usize| m[r * 4 + c];
    let mut inv = [0f64; 16];
    for r in 0..4 {
        for c in 0..4 {
            // the cofactor of (c, r): the transposed adjugate
            let rows: Vec<usize> = (0..4).filter(|&x| x != c).collect();
            let cols: Vec<usize> = (0..4).filter(|&x| x != r).collect();
            let s = |i: usize, j: usize| a(rows[i], cols[j]);
            let det3 = s(0, 0) * (s(1, 1) * s(2, 2) - s(1, 2) * s(2, 1))
                - s(0, 1) * (s(1, 0) * s(2, 2) - s(1, 2) * s(2, 0))
                + s(0, 2) * (s(1, 0) * s(2, 1) - s(1, 1) * s(2, 0));
            inv[r * 4 + c] = if (r + c) % 2 == 0 { det3 } else { -det3 };
        }
    }
    let det: f64 = (0..4).map(|c| a(0, c) * inv[c * 4]).sum();
    if det.abs() < 1e-12 {
        let mut id = [0f64; 16];
        id[0] = 1.0;
        id[5] = 1.0;
        id[10] = 1.0;
        id[15] = 1.0;
        return id;
    }
    inv.map(|x| x / det)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse() {
        // a translation then a scale, row-major
        let m = [2.0, 0.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 0.0, 2.0, 0.0, 1.0, 2.0, 3.0, 1.0];
        let i = invert(&m);
        let p = qwc_formats::skl::matmul(&m, &i);
        for (k, v) in p.iter().enumerate() {
            let want = if k % 5 == 0 { 1.0 } else { 0.0 };
            assert!((v - want).abs() < 1e-9, "{p:?}");
        }
    }

    #[test]
    fn container() {
        let b = glb_bytes(&json!({"asset": {"version": "2.0"}}), vec![1, 2, 3]);
        assert_eq!(&b[0..4], b"glTF");
        assert_eq!(u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize, b.len());
        assert_eq!(b.len() % 4, 0);
    }
}
