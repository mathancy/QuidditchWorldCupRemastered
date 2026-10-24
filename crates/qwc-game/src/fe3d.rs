//! The front end's 3D objects: its models (`MODL`) and puppets (`PUPT`), drawn in the world the screen's camera looks
//! at, with the stadium (`docs/FECK_VM.md` "Models and puppets"). Where each stands is `qwc_feck::place`; this
//! loads their files through the resource index, poses the puppets' models by their characters and uploads it all.
//! **[I]** Not yet: the regions with their own camera or texture (`FECKREGION`, `SPECREGION`), morphs (faces), the
//! characters' shadows and the engine's lighting.

use std::collections::HashMap;

use qwc_anim::{Mat4, Rig};
use qwc_assets::frontend::Resources;
use qwc_assets::{TextureIndex, Vfs};
use qwc_feck::draw::DrawList;
use qwc_feck::place::Matrix;
use qwc_feck::FrontEnd;
use qwc_formats::fas::Fas;
use qwc_formats::skl::Skeleton;
use qwc_render::{Gpu, GpuModel, Renderer};

use crate::face::{self, Face};

/// A model on the GPU with the skeleton its vertices were bound to (its skinning undoes that bind) and its morphed
/// meshes.
struct Loaded {
    path: String,
    gpu: GpuModel,
    bind: Option<Rig>,
    face: Option<Face>,
    /// The card window's texture it was last given (`General_SetQuidditchCardTexture`); `None` before any.
    card: Option<Option<String>>,
}

/// The material `General_SetQuidditchCardTexture` swaps the texture of (`FE_TeamTent_VCPC.card_window`, `0x4d5050`).
const CARD_WINDOW: &str = "card_window";

/// The 3D objects' files, loaded as the objects first draw: per object, its model; per clip set, the skeleton its
/// pose is in.
#[derive(Default)]
pub(crate) struct Objects {
    textures: Option<TextureIndex>,
    models: HashMap<usize, Option<Loaded>>,
    rigs: HashMap<String, Option<Rig>>,
    /// The card texture asked for and the file it resolved to.
    card: Option<((String, String), Option<String>)>,
}

impl Objects {
    /// Load (once) and place every model and puppet of `list`, a record naming one file per package from the
    /// package `loaded` holds; answers the GPU models to draw.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare<'a>(
        &'a mut self,
        gpu: &Gpu,
        renderer: &mut Renderer,
        vfs: &Vfs,
        resources: &Resources,
        loaded: &dyn Fn(&str) -> bool,
        list: &DrawList,
        fe: &FrontEnd,
    ) -> Vec<&'a GpuModel> {
        let mut drawn = Vec::new();
        // **[I]** only the screen's own region: a viewport or texture region has its own camera (`0x4e7fe0`), not
        // ported
        for m in list.models.iter().filter(|m| m.region.kind == 0) {
            let script = fe.objects[m.object].script;
            if let Some(l) = self.load(gpu, renderer, vfs, resources, loaded, m.object, script, m.resource) {
                place(gpu, renderer, l, &m.world(), None, &[]);
                drawn.push(m.object);
            }
        }
        for p in list.puppets.iter().filter(|p| p.region.kind == 0) {
            let o = &fe.objects[p.object];
            let (anim, model_res) = (o.field(0), o.field(1));
            let Some(root) = p.root() else { continue };
            // the pose (the record's, blended between ticks) in its clip set's skeleton, and its morph weights by
            // node name
            let posed = p.pose.as_ref().and_then(|pose| {
                let path = resources.path_in_script(o.script, anim, 0, loaded)?.to_string();
                let rig = self.rigs.entry(path.clone()).or_insert_with(|| set_rig(vfs, &path)).as_ref()?;
                let morphs: Vec<(String, Vec<f32>)> = (rig.skeleton.bones.iter().zip(&pose.morphs))
                    .filter(|(_, w)| !w.is_empty())
                    .map(|(b, w)| (b.name.clone(), w.clone()))
                    .collect();
                Some((rig.world_pose(&rig.pose_local(pose)), morphs))
            });
            let (world, morphs) = posed.unzip();
            if let Some(l) = self.load(gpu, renderer, vfs, resources, loaded, p.object, o.script, model_res) {
                place(gpu, renderer, l, &root, world.as_deref(), morphs.as_deref().unwrap_or_default());
                drawn.push(p.object);
            }
        }
        // the card window's texture, on every model with the material (it is shared in the original)
        let want = fe.card_texture.as_ref().and_then(|(group, name)| {
            let key = (group.clone(), name.clone());
            if self.card.as_ref().is_none_or(|(k, _)| *k != key) {
                let path = resources.texture(vfs, group, name);
                if path.is_none() {
                    eprintln!("Quidditch card texture {name} of {group}: not found");
                }
                self.card = Some((key, path));
            }
            self.card.as_ref().and_then(|(_, p)| p.clone())
        });
        for l in self.models.values_mut().flatten() {
            if l.card.as_ref() != Some(&want) {
                if let Err(e) = renderer.set_material_texture(gpu, vfs, &mut l.gpu, CARD_WINDOW, want.as_deref()) {
                    eprintln!("Quidditch card texture: {e}");
                }
                l.card = Some(want.clone());
            }
        }
        drawn.iter().filter_map(|i| self.models.get(i)?.as_ref().map(|l| &l.gpu)).collect()
    }

    /// Object `obj`'s model of resource `id` in layer script `script`, from the packages `loaded` holds where the
    /// record names one per team (loaded again when the resource names another file).
    #[allow(clippy::too_many_arguments)]
    fn load(
        &mut self,
        gpu: &Gpu,
        renderer: &mut Renderer,
        vfs: &Vfs,
        resources: &Resources,
        loaded: &dyn Fn(&str) -> bool,
        obj: usize,
        script: u32,
        id: u32,
    ) -> Option<&Loaded> {
        let path = resources.path_in_script(script, id, 0, loaded)?;
        if !path.to_ascii_lowercase().ends_with(".fpm") {
            return None;
        }
        let stale = self.models.get(&obj).is_some_and(|l| l.as_ref().is_some_and(|l| l.path != path));
        if stale || !self.models.contains_key(&obj) {
            let textures = self.textures.get_or_insert_with(|| TextureIndex::build(vfs));
            let loaded = crate::models::load(vfs, textures, path).and_then(|m| {
                let bind = m.skeleton.as_ref().map(|(_, s)| Rig::new(s.clone()));
                let gpu = renderer.upload_model(gpu, vfs, &m)?;
                Ok(Loaded { path: path.to_string(), gpu, bind, face: Face::new(&m), card: None })
            });
            let loaded = loaded.map_err(|e| eprintln!("front-end model {path}: {e}")).ok();
            self.models.insert(obj, loaded);
        }
        self.models.get(&obj)?.as_ref()
    }
}

/// The skeleton of a clip set (`name.skl` beside `name.fas`, as the set's loader found it).
fn set_rig(vfs: &Vfs, path: &str) -> Option<Rig> {
    let bytes = vfs.read(path).ok()?;
    let fas = Fas::parse(&bytes).ok()?;
    let (_, s): (String, Skeleton) = qwc_assets::anim::set_skeleton(vfs, path, &fas).ok()?;
    Some(Rig::new(s))
}

/// A model at `root`: an unskinned one by its transform; a skinned one by its bones, each the pose's world matrix
/// (`world`, the bind pose without one) undoing the model's bind, then `root`, and its faces by the pose's morph
/// weights (`morphs`, by node name).
fn place(
    gpu: &Gpu,
    renderer: &Renderer,
    l: &Loaded,
    root: &Matrix,
    world: Option<&[Mat4]>,
    morphs: &[(String, Vec<f32>)],
) {
    if let Some(face) = &l.face {
        let mut weights = Vec::new();
        for (node, ws) in morphs {
            for (k, &w) in ws.iter().enumerate() {
                if let Some(c) = face.channel(node, k as u16) {
                    face::set(&mut weights, c, w);
                }
            }
        }
        face.apply(&gpu.queue, renderer, &l.gpu, &weights);
    }
    let Some(bind) = &l.bind else {
        renderer.set_transform(&gpu.queue, &l.gpu, Some(*root));
        return;
    };
    let root: Mat4 = root.map(f64::from);
    let n = bind.bone_count();
    let posed: Vec<Mat4> =
        (0..n).map(|b| world.and_then(|w| w.get(b)).copied().unwrap_or(bind.skeleton.world[b])).collect();
    let skin: Vec<Mat4> = bind.skin_matrices(&posed).iter().map(|s| qwc_formats::skl::matmul(s, &root)).collect();
    renderer.set_pose(&gpu.queue, &l.gpu, &skin);
}
