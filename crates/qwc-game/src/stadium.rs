//! A stadium on the GPU, as the front end and the match both draw it: its sky and stadium models, the six hoops
//! (the hoop renderer's `singlehoop.fpm` at each place, `0x4a1ab0`) and its wavy flags (`qwc_render::flags`). The
//! `stadium` render group (the load sets' packages in it) is where the flags find their script and textures.

use qwc_assets::{model, TextureIndex, Vfs};
use qwc_render::flags::{self, WavyFlags};
use qwc_render::{Gpu, GpuModel, Renderer};

/// The first file called `name` in the packages of `group`.
pub(crate) fn group_file(vfs: &Vfs, group: &[String], name: &str) -> Option<String> {
    group.iter().map(|p| format!("{p}/{name}")).find(|p| vfs.exists(p))
}

/// The models at `paths`, each uploaded as it stands and drawn unlit, as the original draws the stadium (its
/// materials are default ones, which leave the lit bit clear, `0x507670`; the lightmaps do its shading); a file
/// that does not load is reported and left.
pub(crate) fn models(
    gpu: &Gpu,
    renderer: &mut Renderer,
    vfs: &Vfs,
    textures: &TextureIndex,
    paths: &[&str],
) -> Vec<GpuModel> {
    let mut out = Vec::new();
    for p in paths {
        let m = model::load(vfs, textures, p).map_err(|e| e.to_string());
        match m.and_then(|m| renderer.upload_model(gpu, vfs, &m).map(|g| (m, g))) {
            Ok((m, mut g)) => {
                renderer.set_lit(gpu, &mut g, false);
                if p.to_ascii_lowercase().ends_with("/sky.fpm") {
                    renderer.set_sky(gpu, &mut g);
                }
                renderer.keep_original(gpu, &mut g, &is_reflection);
                animate_materials(gpu, renderer, vfs, textures, &m, &mut g);
                out.push(g)
            }
            Err(e) => eprintln!("{p}: {e}"),
        }
    }
    out
}

/// Whether a stadium's material is water or one of its reflections (Japan's `pond_base`, its painted
/// `jpn_st_reflection_end01`, its mirrored `jpn_st_bridge_red_reflection` and `jpn_st_lantern_ref`): the
/// remastered lighting draws them as the original does, since its sun would light them as matt ground.
fn is_reflection(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with("_ref") || ["reflection", "water", "lake", "pond", "moat"].iter().any(|w| n.contains(w))
}

/// The light probes' box over a stadium with no pitch between hoops to go by (a challenge's, the front end's):
/// the middle half of its models across the ground, from just above it up to `arena::PROBE_TOP`.
pub(crate) fn probe_box(models: &[&GpuModel]) -> Option<([f32; 3], [f32; 3])> {
    let (lo, hi) =
        models.iter().filter(|g| g.casts_shadow).filter_map(|g| g.bounds).reduce(|(a, b), (c, d)| {
            (std::array::from_fn(|i| a[i].min(c[i])), std::array::from_fn(|i| b[i].max(d[i])))
        })?;
    let mid = |i: usize, k: f32| (lo[i] + hi[i]) * 0.5 + (hi[i] - lo[i]) * 0.25 * k;
    Some(([mid(0, -1.0), 1.0, mid(2, -1.0)], [mid(0, 1.0), crate::arena::PROBE_TOP, mid(2, 1.0)]))
}

/// Light `renderer` as `look`, the remastered lighting's settings the stadium's look file's (`crate::looks`)
/// changed by `QWC_REMASTER` (`name=value,...`, `qwc_render::remaster::Settings::apply`).
pub(crate) fn set_look(renderer: &mut Renderer, look: qwc_render::Look, stadium: &str) {
    renderer.look = look;
    renderer.settings = crate::looks::load(stadium);
    apply_env(renderer);
}

/// The settings `QWC_REMASTER` changes, over the renderer's.
pub(crate) fn apply_env(renderer: &mut Renderer) {
    if let Some(list) = std::env::var_os("QWC_REMASTER") {
        if let Err(e) = renderer.settings.apply(&list.to_string_lossy()) {
            eprintln!("QWC_REMASTER: {e}");
        }
    }
}

/// Give each of `model`'s materials that is an animated Groper stage (an `animmap`, `tcMod`s) its animation;
/// [`animate`] then plays them.
pub(crate) fn animate_materials(
    gpu: &Gpu,
    renderer: &mut Renderer,
    vfs: &Vfs,
    textures: &TextureIndex,
    model: &model::Model,
    g: &mut GpuModel,
) {
    for (i, m) in model.materials.iter().enumerate() {
        if let Some((stage, frames)) = textures.stage(&m.name, &model.path) {
            let frames: Vec<Option<String>> = frames.iter().map(|f| f.map(str::to_string)).collect();
            renderer.set_animation(gpu, vfs, g, i, stage.clone(), &frames);
        }
    }
}

/// Show the animated materials of `models` as they are `t` seconds in.
pub(crate) fn animate<'a>(gpu: &Gpu, renderer: &Renderer, models: impl IntoIterator<Item = &'a mut GpuModel>, t: f64) {
    // the packs' shaders see the same clock
    renderer.set_time(t as f32);
    for g in models {
        renderer.animate(gpu, g, t as f32);
    }
}

/// A stadium's light set: its `gfx/lights.cli` (`Lights.Ambient.Colour`, and the `Sun`, `Fill` and `Kick` lights'
/// `Colour` and `Dir`), read as the console runs it; `None` without the file.
pub(crate) fn lights(vfs: &Vfs, gfx: &str) -> Option<qwc_render::Lights> {
    let bytes = vfs.read(&format!("{gfx}/lights.cli")).ok()?;
    let mut c = qwc_sim::Console::new();
    c.run_script(&bytes);
    let v3 = |path: &str| -> [f32; 3] {
        let a = c.args(path, 0);
        std::array::from_fn(|i| a.get(i).and_then(|v| v.num()).unwrap_or(0.0) as f32)
    };
    let light = |name: &str| (v3(&format!("Lights.{name}.Colour")), v3(&format!("Lights.{name}.Dir")));
    Some(qwc_render::Lights {
        ambient: v3("Lights.Ambient.Colour"),
        lights: [light("Sun"), light("Fill"), light("Kick")],
    })
}

/// The hoop model at `path`, one copy at each of `places`.
pub(crate) fn hoops(
    gpu: &Gpu,
    renderer: &mut Renderer,
    vfs: &Vfs,
    textures: &TextureIndex,
    path: &str,
    places: &[[f32; 16]],
) -> Vec<GpuModel> {
    let m = match crate::models::load(vfs, textures, path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{path}: {e}");
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for place in places {
        match renderer.upload_model(gpu, vfs, &m) {
            Ok(mut g) => {
                renderer.set_transform(&gpu.queue, &g, Some(*place));
                renderer.set_lit(gpu, &mut g, false);
                animate_materials(gpu, renderer, vfs, textures, &m, &mut g);
                out.push(g);
            }
            Err(e) => eprintln!("{path}: {e}"),
        }
    }
    out
}

/// The stadium's wavy flags (`qwc_render::flags`): each flag in use with a texture found its own one-strip model at
/// its place, and how many `General_RefreshWavyFlags` had been posted when they were loaded.
pub(crate) struct Flags {
    pub set: WavyFlags,
    pub models: Vec<(usize, GpuModel)>,
    pub refreshes: usize,
}

impl Flags {
    /// Run the `stadium` group's `wavyflags.cli` with the DataStore as `store` has it and upload the flags, each
    /// texture found in its group's packages (`groups`). A flag whose texture is not found is not drawn.
    pub fn load(
        gpu: &Gpu,
        renderer: &mut Renderer,
        vfs: &Vfs,
        groups: &dyn Fn(flags::Group) -> Vec<String>,
        store: &flags::Store,
        refreshes: usize,
    ) -> Flags {
        let group = groups(flags::Group::Stadium);
        let mut set = WavyFlags::new();
        let Some(script) = group_file(vfs, &group, "wavyflags.cli") else {
            return Flags { set, models: Vec::new(), refreshes };
        };
        match vfs.read(&script) {
            Ok(b) => {
                for line in set.load(&b, store) {
                    eprintln!("{script}: {line}: ignored");
                }
            }
            Err(e) => eprintln!("{script}: {e}"),
        }
        let mut models = Vec::new();
        for (i, f) in set.active() {
            let Some(t) = &f.texture else { continue };
            let Some(path) = group_file(vfs, &groups(t.group), &format!("{}.fsh", t.name)) else {
                eprintln!("{script}: flag {i}'s texture {:?} in {:?} is not loaded", t.name, t.group);
                continue;
            };
            match renderer.upload_model(gpu, vfs, &set.model(i, Some(path), 0.0)) {
                Ok(mut g) => {
                    // **[I]** unlit, as the stadium (`WavyFlags.LightTransfer` is not traced)
                    renderer.set_lit(gpu, &mut g, false);
                    models.push((i, g))
                }
                Err(e) => eprintln!("{script}: flag {i}: {e}"),
            }
        }
        eprintln!("{script}: {} flags drawn", models.len());
        Flags { set, models, refreshes }
    }

    /// The cloth `time` seconds on.
    pub fn wave(&self, gpu: &Gpu, renderer: &Renderer, time: f64) {
        for (i, g) in &self.models {
            renderer.set_vertices(&gpu.queue, g, 0, &self.set.mesh(*i, time));
        }
    }

    /// The flags to draw: none while the module is switched off.
    pub fn drawn(&self) -> impl Iterator<Item = &GpuModel> {
        self.models.iter().filter(|_| self.set.enabled).map(|(_, g)| g)
    }
}
