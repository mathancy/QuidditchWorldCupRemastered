//! Loading an edited character model exported from Blender as a `.glb`, so the viewer can show a remodel
//! next to (or instead of) the original `.fpm`.
//!
//! What it expects, matching what `tools/fpm_to_gltf.py` writes and Blender round-trips:
//! - positions are the bind pose in game space (a skinned mesh ignores its node transform, as glTF says);
//! - one skin whose joints are named like the game skeleton's bones (`sma_common.skl`, ...);
//! - materials named like the original ones, so the original game textures are found by name.
//!
//! Limits, until the renderer grows them:
//! - the renderer skins with one bone per vertex, so each vertex takes its **heaviest** joint (the original
//!   models are rigid-skinned this way; a remodel with blended weights will look stiffer at joints than in Blender);
//! - morph targets are not loaded;
//! - a material's texture is the PNG embedded in the file (the remodel's own, often higher resolution); with
//!   `use_embedded` off, or when none is embedded, it comes from the game data by material name instead, and a
//!   material with no texture at all (for example a new jersey-number quad) is skipped and reported via [`GlbReport`].

use std::collections::BTreeMap;
use std::path::Path;

use qwc_formats::fas::Channel;
use qwc_formats::skl::Skeleton;
use serde_json::Value;

use crate::model::{Material, Mesh, Model};
use crate::texture::{Texture, TextureIndex};
use crate::vfs::{Error, Result, Vfs};

/// A glTF animation converted to the game's clip form: Euler rotation channels and translation channels at integer
/// frame times, which is what `qwc_anim::Clip` plays.
#[derive(Debug, Clone)]
pub struct GlbClip {
    pub name: String,
    pub frames: u32,
    pub fps: f32,
    pub rotation: Vec<Channel>,
    pub translation: Vec<Channel>,
}

/// Row-vector rotation matrix of the glTF quaternion `q = (x, y, z, w)`, as `tools/fas_to_gltf.py` writes them
/// (`quat_from_row`): the transpose of the usual column-vector matrix.
fn quat_to_row(q: [f64; 4]) -> [[f64; 3]; 3] {
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt().max(1e-12);
    let (x, y, z, w) = (q[0] / n, q[1] / n, q[2] / n, q[3] / n);
    let c = [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w)],
        [2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w)],
        [2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y)],
    ];
    std::array::from_fn(|i| std::array::from_fn(|j| c[j][i]))
}

/// The angles `(x, y, z)` with `euler_row(x, y, z) == m`, the game's `Rx * Ry * Rz` row-vector order.
fn row_to_euler(m: [[f64; 3]; 3]) -> [f64; 3] {
    let y = (-m[0][2]).clamp(-1.0, 1.0).asin();
    if y.cos().abs() < 1e-6 {
        [(-m[2][1]).atan2(m[1][1]), y, 0.0]
    } else {
        [m[1][2].atan2(m[2][2]), y, m[0][1].atan2(m[0][0])]
    }
}

/// Make consecutive Euler keys continuous (no 2 pi jumps) so interpolation never spins the long way round.
fn unwrap_angles(keys: &mut [[f64; 3]]) {
    for i in 1..keys.len() {
        let prev = keys[i - 1];
        for (k, p) in keys[i].iter_mut().zip(prev) {
            while *k - p > std::f64::consts::PI {
                *k -= 2.0 * std::f64::consts::PI;
            }
            while *k - p < -std::f64::consts::PI {
                *k += 2.0 * std::f64::consts::PI;
            }
        }
    }
}

/// Every animation in a glTF file, with channels targeting the skin's joints mapped to `skeleton` bones by name.
/// Times are converted to integer frames at the animation's own rate (inferred from its first key interval).
pub fn load_clips(file: &Path, skeleton: &Skeleton) -> Result<Vec<GlbClip>> {
    let data = std::fs::read(file).map_err(|e| Error::Io(file.to_path_buf(), e))?;
    let g = parse_glb(file, &data)?;
    let bone_by_name: BTreeMap<String, usize> =
        skeleton.bones.iter().enumerate().map(|(i, b)| (b.name.to_lowercase(), i)).collect();
    let mut node_bone: BTreeMap<usize, u8> = BTreeMap::new();
    for n in g.json["skins"][0]["joints"].as_array().map_or(&[][..], |v| &v[..]) {
        let n = n.as_u64().unwrap_or(0) as usize;
        let name = g.json["nodes"][n]["name"].as_str().unwrap_or("").to_lowercase();
        if let Some(&b) = bone_by_name.get(&name) {
            node_bone.insert(n, b as u8);
        }
    }
    let mut clips = Vec::new();
    for anim in g.json["animations"].as_array().map_or(&[][..], |v| &v[..]) {
        let mut clip = GlbClip {
            name: anim["name"].as_str().unwrap_or("").to_string(),
            frames: 0,
            fps: 30.0,
            rotation: vec![],
            translation: vec![],
        };
        for ch in anim["channels"].as_array().map_or(&[][..], |v| &v[..]) {
            let Some(&bone) = ch["target"]["node"].as_u64().and_then(|n| node_bone.get(&(n as usize))) else {
                continue;
            };
            let path = ch["target"]["path"].as_str().unwrap_or("");
            let sampler = &anim["samplers"][ch["sampler"].as_u64().unwrap_or(0) as usize];
            let secs = g.accessor(sampler["input"].as_u64().unwrap_or(0) as usize)?.0;
            let (out, ncomp) = g.accessor(sampler["output"].as_u64().unwrap_or(0) as usize)?;
            if secs.len() < 2 {
                continue;
            }
            let fps = (1.0 / (secs[1] - secs[0]).max(1e-6)).round().max(1.0);
            clip.fps = fps as f32;
            let times: Vec<u32> = secs.iter().map(|s| (s * fps).round() as u32).collect();
            clip.frames = clip.frames.max(*times.last().unwrap());
            let mut keys: Vec<[f64; 3]> = match (path, ncomp) {
                ("rotation", 4) => out.chunks(4).map(|q| row_to_euler(quat_to_row([q[0], q[1], q[2], q[3]]))).collect(),
                ("translation", 3) => out.chunks(3).map(|t| [t[0], t[1], t[2]]).collect(),
                _ => continue,
            };
            if path == "rotation" {
                unwrap_angles(&mut keys);
                clip.rotation.push(Channel { bone, keys, times });
            } else {
                clip.translation.push(Channel { bone, keys, times });
            }
        }
        clips.push(clip);
    }
    Ok(clips)
}

/// What the loader left out, for the caller to print.
#[derive(Debug, Default, Clone)]
pub struct GlbReport {
    /// Names of materials with no texture at all; their meshes were skipped.
    pub skipped_materials: Vec<String>,
    /// Materials drawn with a texture embedded in the file: (name, width, height).
    pub embedded_textures: Vec<(String, u32, u32)>,
    /// Joint names with no bone of that name in the skeleton (their vertices fall back to bone 0).
    pub unmatched_joints: Vec<String>,
    /// Vertices whose heaviest joint carries less than 90% of the weight (shown rigidly).
    pub blended_vertices: usize,
    pub vertices: usize,
    /// Morph targets read (each a primitive's target with a `morphNN` name), and the names that are not `morphNN`
    /// (their targets left out).
    pub morph_targets: usize,
    pub unnamed_morphs: Vec<String>,
}

/// A glTF morph target's game weight channel from its name: `morphNN` (as `tools/fpm_to_gltf.py` names them, and a
/// Blender shape key keeps its name), NN the index into the character's weight array.
pub fn morph_channel(name: &str) -> Option<u16> {
    name.strip_prefix("morph")?.parse().ok()
}

fn bad(path: &Path, msg: impl Into<String>) -> Error {
    Error::Format(path.to_path_buf(), qwc_formats::Error::Format(msg.into()))
}

struct Glb<'a> {
    path: &'a Path,
    json: Value,
    bin: Vec<u8>,
}

impl Glb<'_> {
    /// All components of an accessor as f64 (normalized integer types scaled to 0..1 / -1..1), plus the
    /// number of components per element.
    fn accessor(&self, index: usize) -> Result<(Vec<f64>, usize)> {
        let a = &self.json["accessors"][index];
        let count = a["count"].as_u64().ok_or_else(|| bad(self.path, "accessor without count"))? as usize;
        let ncomp = match a["type"].as_str() {
            Some("SCALAR") => 1,
            Some("VEC2") => 2,
            Some("VEC3") => 3,
            Some("VEC4") => 4,
            t => return Err(bad(self.path, format!("accessor type {t:?}"))),
        };
        let ctype = a["componentType"].as_u64().unwrap_or(0);
        let size = match ctype {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            5125 | 5126 => 4,
            c => return Err(bad(self.path, format!("component type {c}"))),
        };
        let normalized = a["normalized"].as_bool().unwrap_or(false);
        let mut out = vec![0.0; count * ncomp];
        let Some(view) = a["bufferView"].as_u64() else {
            return Ok((out, ncomp)); // no bufferView: all zeros by definition
        };
        let v = &self.json["bufferViews"][view as usize];
        let stride = v["byteStride"].as_u64().map_or(size * ncomp, |s| s as usize);
        let base = v["byteOffset"].as_u64().unwrap_or(0) as usize + a["byteOffset"].as_u64().unwrap_or(0) as usize;
        for i in 0..count {
            for c in 0..ncomp {
                let o = base + i * stride + c * size;
                let b =
                    self.bin.get(o..o + size).ok_or_else(|| bad(self.path, "accessor reads past the binary chunk"))?;
                let x = match ctype {
                    5120 => f64::from(b[0] as i8) / if normalized { 127.0 } else { 1.0 },
                    5121 => f64::from(b[0]) / if normalized { 255.0 } else { 1.0 },
                    5122 => f64::from(i16::from_le_bytes([b[0], b[1]])) / if normalized { 32767.0 } else { 1.0 },
                    5123 => f64::from(u16::from_le_bytes([b[0], b[1]])) / if normalized { 65535.0 } else { 1.0 },
                    5125 => f64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                    _ => f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                };
                out[i * ncomp + c] = x;
            }
        }
        Ok((out, ncomp))
    }
}

fn parse_glb<'a>(path: &'a Path, data: &[u8]) -> Result<Glb<'a>> {
    if data.len() < 20 || &data[..4] != b"glTF" {
        return Err(bad(path, "not a GLB file"));
    }
    let mut json = None;
    let mut bin = Vec::new();
    let mut off = 12;
    while off + 8 <= data.len() {
        let len = u32::from_le_bytes(data[off..off + 4].try_into().unwrap()) as usize;
        let kind = &data[off + 4..off + 8];
        let body = data.get(off + 8..off + 8 + len).ok_or_else(|| bad(path, "chunk past end of file"))?;
        match kind {
            b"JSON" => json = Some(serde_json::from_slice(body).map_err(|e| bad(path, format!("JSON: {e}")))?),
            b"BIN\0" => bin = body.to_vec(),
            _ => {}
        }
        off += 8 + len;
    }
    Ok(Glb { path, json: json.ok_or_else(|| bad(path, "no JSON chunk"))?, bin })
}

/// PNG or JPEG bytes to RGBA8 (Blender exports the source format by default, so a model can carry either).
pub fn decode_image(data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    if data.starts_with(&[0xFF, 0xD8]) {
        return decode_jpeg(data);
    }
    decode_png(data)
}

fn decode_jpeg(data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut dec = jpeg_decoder::Decoder::new(std::io::Cursor::new(data));
    let px = dec.decode().ok()?;
    let info = dec.info()?;
    let rgba = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => {
            px.as_chunks::<3>().0.iter().flat_map(|c| [c[0], c[1], c[2], 255]).collect()
        }
        jpeg_decoder::PixelFormat::L8 => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        _ => return None,
    };
    Some((u32::from(info.width), u32::from(info.height), rgba))
}

/// PNG bytes to RGBA8 (8-bit, expanded palettes and greys).
fn decode_png(data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(data));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let px = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px.as_chunks::<3>().0.iter().flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => px.as_chunks::<2>().0.iter().flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some((info.width, info.height, rgba))
}

/// Mip chain by 2x2 box filtering (`levels[l]` is `max(1, w >> l) x max(1, h >> l)`), as [`Texture`] documents.
fn mip_chain(w: u32, h: u32, base: Vec<u8>) -> Vec<Vec<u8>> {
    let mut levels = vec![base];
    let (mut cw, mut ch) = (w as usize, h as usize);
    while cw > 1 || ch > 1 {
        let (nw, nh) = ((cw / 2).max(1), (ch / 2).max(1));
        let src = levels.last().unwrap();
        let mut out = vec![0u8; nw * nh * 4];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..4 {
                    let mut sum = 0u32;
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let sx = (x * 2 + dx).min(cw - 1);
                        let sy = (y * 2 + dy).min(ch - 1);
                        sum += u32::from(src[(sy * cw + sx) * 4 + c]);
                    }
                    out[(y * nw + x) * 4 + c] = ((sum + 2) / 4) as u8;
                }
            }
        }
        levels.push(out);
        (cw, ch) = (nw, nh);
    }
    levels
}

impl Glb<'_> {
    /// The decoded base-colour image of glTF material `m`, when it has an embedded PNG.
    fn embedded_image(&self, m: &Value) -> Option<(u32, u32, Vec<u8>)> {
        let tex = m["pbrMetallicRoughness"]["baseColorTexture"]["index"].as_u64()?;
        let image = self.json["textures"][tex as usize]["source"].as_u64()?;
        let view = &self.json["bufferViews"][self.json["images"][image as usize]["bufferView"].as_u64()? as usize];
        let off = view["byteOffset"].as_u64().unwrap_or(0) as usize;
        decode_image(self.bin.get(off..off + view["byteLength"].as_u64()? as usize)?)
    }
}

/// A node's local matrix (column-major, as glTF stores it): its `matrix`, or its translation, rotation and scale.
fn node_matrix(n: &Value) -> [f64; 16] {
    if let Some(m) = n["matrix"].as_array().filter(|m| m.len() == 16) {
        return std::array::from_fn(|i| m[i].as_f64().unwrap_or(0.0));
    }
    let v = |key: &str, d: &[f64]| -> Vec<f64> {
        n[key].as_array().map_or(d.to_vec(), |a| a.iter().map(|x| x.as_f64().unwrap_or(0.0)).collect())
    };
    let (t, q, s) = (v("translation", &[0.0; 3]), v("rotation", &[0.0, 0.0, 0.0, 1.0]), v("scale", &[1.0; 3]));
    // the row-vector matrix of q, transposed back: column-vector rotation
    let r = quat_to_row([q[0], q[1], q[2], q[3]]);
    let mut m = [0.0; 16];
    for c in 0..3 {
        for row in 0..3 {
            m[c * 4 + row] = r[c][row] * s[c];
        }
    }
    m[12..15].copy_from_slice(&t[..3]);
    m[15] = 1.0;
    m
}

/// `a * b`, both column-major.
fn mat_mul(a: &[f64; 16], b: &[f64; 16]) -> [f64; 16] {
    std::array::from_fn(|i| {
        let (c, r) = (i / 4, i % 4);
        (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum()
    })
}

/// Load `file` as scenery: a static model (no skin) whose materials draw the base-colour images embedded in the
/// file, as `work/environments/export_fe_stadium.py` writes them. Each node's mesh is placed by the node's world
/// matrix; a primitive whose material is `doubleSided` is drawn from both sides; a material with no embedded image
/// is skipped (and reported). The file is in game space (+Y up), like the disc's models exported to glTF.
pub fn load_scenery(file: &Path) -> Result<(Model, GlbReport)> {
    let data = std::fs::read(file).map_err(|e| Error::Io(file.to_path_buf(), e))?;
    let g = parse_glb(file, &data)?;
    let mut report = GlbReport::default();
    let mut materials = Vec::new();
    let mut two_sided = Vec::new();
    for m in g.json["materials"].as_array().map_or(&[][..], |v| &v[..]) {
        let name = m["name"].as_str().unwrap_or("").to_string();
        let image = g.embedded_image(m).map(|(w, h, rgba)| {
            report.embedded_textures.push((name.clone(), w, h));
            std::sync::Arc::new(Texture {
                path: format!("{}#{name}", file.display()),
                width: w,
                height: h,
                levels: mip_chain(w, h, rgba),
            })
        });
        if image.is_none() {
            report.skipped_materials.push(name.clone());
        }
        two_sided.push(m["doubleSided"].as_bool().unwrap_or(false));
        materials.push(Material { name, texture: None, image, surface: None, groper: None });
    }
    // every node with a mesh, with its world matrix
    let mut placed: Vec<(usize, [f64; 16])> = Vec::new();
    let identity = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
    let scene = g.json["scene"].as_u64().unwrap_or(0) as usize;
    let mut stack: Vec<(usize, [f64; 16])> = g.json["scenes"][scene]["nodes"]
        .as_array()
        .map_or(&[][..], |v| &v[..])
        .iter()
        .filter_map(|n| n.as_u64())
        .map(|n| (n as usize, identity))
        .collect();
    while let Some((n, parent)) = stack.pop() {
        let node = &g.json["nodes"][n];
        let world = mat_mul(&parent, &node_matrix(node));
        if let Some(mesh) = node["mesh"].as_u64() {
            placed.push((mesh as usize, world));
        }
        for c in node["children"].as_array().map_or(&[][..], |v| &v[..]).iter().filter_map(|c| c.as_u64()) {
            stack.push((c as usize, world));
        }
    }
    let mut meshes = Vec::new();
    for (mesh, w) in placed {
        for p in g.json["meshes"][mesh]["primitives"].as_array().map_or(&[][..], |v| &v[..]) {
            let material = p["material"].as_u64().map_or(usize::MAX, |i| i as usize);
            if materials.get(material).is_none_or(|m| m.image.is_none()) {
                continue;
            }
            let attr = |name: &str| p["attributes"][name].as_u64().map(|i| i as usize);
            let pos = g.accessor(attr("POSITION").ok_or_else(|| bad(file, "primitive without POSITION"))?)?.0;
            let at = |c: &[f64], k: usize| w[k] * c[0] + w[4 + k] * c[1] + w[8 + k] * c[2];
            let positions: Vec<[f32; 3]> =
                pos.chunks(3).map(|c| std::array::from_fn(|k| (at(c, k) + w[12 + k]) as f32)).collect();
            let n = positions.len();
            let normals = match attr("NORMAL") {
                Some(i) => Some(
                    g.accessor(i)?
                        .0
                        .chunks(3)
                        .map(|c| {
                            let v: [f64; 3] = std::array::from_fn(|k| at(c, k));
                            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
                            v.map(|x| (x / l) as f32)
                        })
                        .collect(),
                ),
                None => None,
            };
            let uvs = match attr("TEXCOORD_0") {
                Some(i) => g.accessor(i)?.0.chunks(2).map(|c| [c[0] as f32, c[1] as f32]).collect(),
                None => vec![[0.0; 2]; n],
            };
            let indices: Vec<u32> = match p["indices"].as_u64() {
                Some(i) => g.accessor(i as usize)?.0.into_iter().map(|x| x as u32).collect(),
                None => (0..n as u32).collect(),
            };
            report.vertices += n;
            meshes.push(Mesh {
                material,
                two_sided: two_sided[material],
                positions,
                normals,
                uvs,
                indices,
                ..Default::default()
            });
        }
    }
    let model = Model {
        path: file.display().to_string(),
        matrix: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
        materials,
        skeleton: None,
        meshes,
    };
    Ok((model, report))
}

/// Load `file` as a [`Model`]. `skeleton_path` is a `.skl` inside the game data (the rig the clips are made for);
/// `tex_from` is an original model path used to resolve material names to textures, like the exporter does.
pub fn load(
    vfs: &Vfs,
    textures: &TextureIndex,
    file: &Path,
    skeleton_path: &str,
    tex_from: &str,
    use_embedded: bool,
) -> Result<(Model, GlbReport)> {
    let data = std::fs::read(file).map_err(|e| Error::Io(file.to_path_buf(), e))?;
    let g = parse_glb(file, &data)?;
    let skeleton = Skeleton::parse(&vfs.read(skeleton_path)?).map_err(|e| Error::Format(skeleton_path.into(), e))?;
    let bone_by_name: BTreeMap<String, usize> =
        skeleton.bones.iter().enumerate().map(|(i, b)| (b.name.to_lowercase(), i)).collect();

    let mut report = GlbReport::default();
    let mut materials = Vec::new();
    let mut usable = Vec::new();
    for m in g.json["materials"].as_array().map_or(&[][..], |v| &v[..]) {
        let name = m["name"].as_str().unwrap_or("").to_string();
        let texture = textures.find(&name, tex_from).map(str::to_string);
        let image = if use_embedded { g.embedded_image(m) } else { None };
        let image = image.map(|(w, h, rgba)| {
            report.embedded_textures.push((name.clone(), w, h));
            std::sync::Arc::new(Texture {
                path: format!("{}#{name}", file.display()),
                width: w,
                height: h,
                levels: mip_chain(w, h, rgba),
            })
        });
        if texture.is_none() && image.is_none() {
            report.skipped_materials.push(name.clone());
        }
        usable.push(texture.is_some() || image.is_some());
        materials.push(Material { name, texture, image, surface: None, groper: None });
    }

    // joint index (position in the skin) -> skeleton bone
    let skin_joints: Vec<usize> = g.json["skins"][0]["joints"]
        .as_array()
        .map(|a| a.iter().filter_map(|n| n.as_u64().map(|n| n as usize)).collect())
        .unwrap_or_default();
    let joint_bone: Vec<usize> = skin_joints
        .iter()
        .map(|&n| {
            let name = g.json["nodes"][n]["name"].as_str().unwrap_or("").to_string();
            bone_by_name.get(&name.to_lowercase()).copied().unwrap_or_else(|| {
                report.unmatched_joints.push(name);
                0
            })
        })
        .collect();

    let mut meshes = Vec::new();
    for m in g.json["meshes"].as_array().map_or(&[][..], |v| &v[..]) {
        // the mesh's morph targets' names: their weight channels
        let names: Vec<String> = m["extras"]["targetNames"]
            .as_array()
            .map(|a| a.iter().map(|n| n.as_str().unwrap_or("").to_string()).collect())
            .unwrap_or_default();
        for n in names.iter().filter(|n| morph_channel(n).is_none()) {
            if !report.unnamed_morphs.contains(n) {
                report.unnamed_morphs.push(n.clone());
            }
        }
        for p in m["primitives"].as_array().map_or(&[][..], |v| &v[..]) {
            let material = p["material"].as_u64().map_or(usize::MAX, |i| i as usize);
            if materials.get(material).is_some() && !usable[material] {
                continue;
            }
            let attr = |name: &str| p["attributes"][name].as_u64().map(|i| i as usize);
            let pos = g.accessor(attr("POSITION").ok_or_else(|| bad(file, "primitive without POSITION"))?)?.0;
            let n = pos.len() / 3;
            let positions: Vec<[f32; 3]> = pos.chunks(3).map(|c| [c[0] as f32, c[1] as f32, c[2] as f32]).collect();
            let normals = match attr("NORMAL") {
                Some(i) => Some(g.accessor(i)?.0.chunks(3).map(|c| [c[0] as f32, c[1] as f32, c[2] as f32]).collect()),
                None => None,
            };
            let uvs = match attr("TEXCOORD_0") {
                Some(i) => g.accessor(i)?.0.chunks(2).map(|c| [c[0] as f32, c[1] as f32]).collect(),
                None => vec![[0.0; 2]; n],
            };
            let bones = match (attr("JOINTS_0"), attr("WEIGHTS_0")) {
                (Some(ji), Some(wi)) if !joint_bone.is_empty() => {
                    let (j, _) = g.accessor(ji)?;
                    let (w, _) = g.accessor(wi)?;
                    let mut out = Vec::with_capacity(n);
                    for v in 0..n {
                        let (mut best, mut best_w, mut total) = (0usize, -1.0, 0.0);
                        for c in 0..4 {
                            let wt = w[v * 4 + c];
                            total += wt;
                            if wt > best_w {
                                best_w = wt;
                                best = j[v * 4 + c] as usize;
                            }
                        }
                        report.vertices += 1;
                        if total > 0.0 && best_w / total < 0.9 {
                            report.blended_vertices += 1;
                        }
                        out.push(joint_bone.get(best).copied().unwrap_or(0) as u16);
                    }
                    Some(out)
                }
                _ => None,
            };
            let indices: Vec<u32> = match p["indices"].as_u64() {
                Some(i) => g.accessor(i as usize)?.0.into_iter().map(|x| x as u32).collect(),
                None => (0..n as u32).collect(),
            };
            // its morph targets: the position deltas of the vertices each moves, by weight channel
            let mut morphs = BTreeMap::new();
            for (k, t) in p["targets"].as_array().map_or(&[][..], |v| &v[..]).iter().enumerate() {
                let (Some(channel), Some(acc)) = (names.get(k).and_then(|n| morph_channel(n)), t["POSITION"].as_u64())
                else {
                    continue;
                };
                let d = g.accessor(acc as usize)?.0;
                let moved: Vec<(u32, [f32; 3])> = d
                    .chunks(3)
                    .enumerate()
                    .filter(|(_, c)| c.iter().any(|v| v.abs() > 1e-7))
                    .map(|(v, c)| (v as u32, [c[0] as f32, c[1] as f32, c[2] as f32]))
                    .collect();
                if !moved.is_empty() {
                    report.morph_targets += 1;
                    morphs.insert(channel, moved);
                }
            }
            meshes.push(Mesh {
                material,
                positions,
                normals,
                colors: None,
                uvs,
                bones,
                indices,
                morphs,
                ..Default::default()
            });
        }
    }
    let path = file.display().to_string();
    let model = Model {
        path,
        matrix: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
        materials,
        skeleton: if skin_joints.is_empty() { None } else { Some((skeleton_path.to_string(), skeleton)) },
        meshes,
    };
    Ok((model, report))
}
