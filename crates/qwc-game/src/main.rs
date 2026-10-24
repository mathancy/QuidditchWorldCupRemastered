//! `qwc-game`: the remaster's executable. For now it is a viewer for the original models and animations and a player
//! for the original audio.
//!
//! ```text
//! qwc-game [--data DIR] view MODEL... [--anim SET.fas] [--clip NAME|INDEX]
//! qwc-game [--data DIR] shot MODEL... --out FILE.png [--anim SET.fas] [--clip NAME|INDEX] [--time SECS]
//!          [--size WxH] [--yaw DEG] [--pitch DEG] [--zoom F] [--look-at X,Y,Z]
//! qwc-game [--data DIR] list EXT
//! qwc-game [--data DIR] sound event CATEGORY.map ID | bank BANK INDEX | speech LANGUAGE NAME  [--out FILE.wav]
//! qwc-game [--data DIR] music SCORE [EVENT] [--time SECS] [--out FILE.wav]
//! qwc-game [--data DIR] sim TEAM0.cli TEAM1.cli [--stadium NAME] [--time SECS] [--out REPLAY.json]
//! qwc-game [--data DIR] commentary TEAM0 TEAM1 [--stadium NAME] [--time SECS]
//! qwc-game [--data DIR] play [TEAM0 TEAM1] [--stadium NAME] [--size WxH] [--time SECS --out FILE.png]
//! qwc-game [--data DIR] fe SCRIPT.vso --out FILE.png [--time SECS] [--size WxH]
//! qwc-game [--data DIR] screen NODE|boot --out FILE.png [--time SECS] [--size WxH]
//! qwc-game [--data DIR] frontend [NODE|boot] [--size WxH] [--keys SECS:KEY,...] [--time SECS] [--out FILE.png]
//!          [--remastered DIR] [--original-movies] [--openh264] [--textures LIST] [--disc-textures]
//!          [--fonts LIST] [--disc-fonts]
//! ```
//! Paths are inside the game data (`root/Balls/snitch.fpm`, `stadium/england/gfx/stadium.fpm`). The data directory
//! defaults to `$QWC_DATA`, then `original/cd_files`. `--mods DIR` (repeatable, later wins) lays a directory with the
//! same layout over the data: its files replace the disc's (archive members included) or add new ones.
//!
//! `sound` plays a sound effect event (`Audio/cats/chaser.map 3`), a bank sound (`Foley 12`) or a speech file
//! (`English AV\\CCWaxl.dat`); `music` plays a score (`FE`, `HW`, `Intl`, `QD`, `chal`) from the section an event
//! starts. With `--out` they render to a WAV file instead of the audio device. `sim` plays the gameplay simulation
//! headless with two teams' `.cli` files (`teams/England/team_3.cli`) and prints the passes.
//! `commentary` plays a match
//! headless and prints what its commentators say.
//!
//! The camera frames the first model. In the viewer: drag to orbit, wheel to zoom, space pauses, left/right change clip, Esc quits.

// OpenH264's assembly and its C both have a `dct.o`: MSVC's link warns (LNK4255) and drops that object's debug
// information, which is harmless
#![allow(linker_messages)]

use std::sync::Arc;
use std::time::Instant;

use qwc_anim::{Clip, Rig};
use qwc_assets::{glb, model, Model, TextureIndex, Vfs};
use qwc_formats::fas::{Fas, Sfa};
use qwc_game::{
    commentary, cutscene, face, fail, fe, live, movie, parse_args, play, sim, sound, textures, usage, Args,
};
use qwc_render::{wgpu, Camera, Gpu, GpuModel, Offscreen, Renderer};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

/// The models of the scene with their CPU-side data, and the animation playing on the first skinned one.
struct Scene {
    models: Vec<Model>,
    rig: Option<(usize, Rig)>,
    clips: Vec<Clip>,
    clip: usize,
    /// The skinned model's morphed meshes, moved by the clip's morph tracks.
    face: Option<face::Face>,
}

impl Scene {
    fn load(vfs: &Vfs, args: &Args) -> Scene {
        let textures = TextureIndex::build(vfs);
        let models: Vec<Model> = args
            .models
            .iter()
            .map(|p| {
                if p.to_lowercase().ends_with(".glb") {
                    let (m, r) = glb::load(
                        vfs,
                        &textures,
                        std::path::Path::new(p),
                        &args.skeleton,
                        &args.tex_from,
                        args.use_embedded,
                    )
                    .unwrap_or_else(|e| fail(format!("{p}: {e}")));
                    eprintln!(
                        "{p}: edited model; heaviest-joint skinning ({}/{} vertices have blended weights)",
                        r.blended_vertices, r.vertices
                    );
                    for (name, w, h) in &r.embedded_textures {
                        eprintln!("  texture from the file: {name} ({w}x{h})");
                    }
                    if !r.skipped_materials.is_empty() {
                        eprintln!("  skipped (no game texture by that name): {}", r.skipped_materials.join(", "));
                    }
                    if !r.unmatched_joints.is_empty() {
                        eprintln!("  joints with no bone of that name: {}", r.unmatched_joints.join(", "));
                    }
                    m
                } else {
                    model::load(vfs, &textures, p).unwrap_or_else(|e| fail(format!("{p}: {e}")))
                }
            })
            .collect();
        for m in &models {
            let missing = m.materials.iter().filter(|x| x.texture.is_none()).count();
            eprintln!(
                "{}: {} meshes, {} materials ({missing} without texture), skeleton {}",
                m.path,
                m.meshes.len(),
                m.materials.len(),
                m.skeleton.as_ref().map_or("none", |(p, _)| p.as_str())
            );
        }
        let skinned = models.iter().position(|m| m.skeleton.is_some());
        let rig = skinned.map(|i| (i, Rig::new(models[i].skeleton.as_ref().unwrap().1.clone())));
        let mut clips = Vec::new();
        let mut clip = 0;
        if let Some(set_path) = args.anim.as_ref().filter(|p| p.to_lowercase().ends_with(".glb")) {
            // glTF clips (for example the retargeted ones from tools/retarget_hands.py), mapped to bones by name
            let Some((mi, _)) = &rig else { fail("--anim needs a skinned model") };
            let skeleton = &models[*mi].skeleton.as_ref().unwrap().1;
            clips = glb::load_clips(std::path::Path::new(set_path), skeleton)
                .unwrap_or_else(|e| fail(format!("{set_path}: {e}")))
                .into_iter()
                .map(|c| Clip {
                    name: c.name,
                    frames: c.frames,
                    fps: c.fps,
                    rotation: c.rotation,
                    translation: c.translation,
                    ..Default::default()
                })
                .collect();
            if clips.is_empty() {
                fail(format!("{set_path}: no animations"));
            }
            if let Some(c) = &args.clip {
                clip = c
                    .parse::<usize>()
                    .ok()
                    .filter(|&i| i < clips.len())
                    .or_else(|| clips.iter().position(|x| x.name.eq_ignore_ascii_case(c)))
                    .unwrap_or_else(|| fail(format!("no clip {c} in {set_path}")));
            }
            eprintln!("{set_path}: {} clips; playing {} ({})", clips.len(), clip, clips[clip].name);
        } else if let Some(set_path) = &args.anim {
            let Some((_, rig)) = &rig else { fail("--anim needs a skinned model") };
            let bytes = vfs.read(set_path).unwrap_or_else(|e| fail(e));
            let set = Fas::parse(&bytes).unwrap_or_else(|e| fail(format!("{set_path}: {e}")));
            let need = qwc_anim::mask_bones(&set);
            if need > rig.bone_count() {
                fail(format!("{set_path} animates {need} bones; the model's skeleton has {}", rig.bone_count()));
            }
            let stream = if qwc_assets::anim::needs_stream(&set) {
                qwc_assets::anim::set_stream(vfs, set_path).unwrap_or_else(|e| fail(e))
            } else {
                None
            };
            let sfa = stream.as_deref().map(|b| Sfa::parse(b).unwrap_or_else(|e| fail(e)));
            clips = Clip::decode_all(&set, sfa.as_ref()).unwrap_or_else(|e| fail(format!("{set_path}: {e}")));
            if let Some(c) = &args.clip {
                clip = c
                    .parse::<usize>()
                    .ok()
                    .filter(|&i| i < clips.len())
                    .or_else(|| clips.iter().position(|x| x.name.eq_ignore_ascii_case(c)))
                    .unwrap_or_else(|| fail(format!("no clip {c} in {set_path}")));
            }
            eprintln!("{set_path}: {} clips; playing {} ({})", clips.len(), clip, clips[clip].name);
        }
        let face = rig.as_ref().and_then(|(i, _)| face::Face::new(&models[*i]));
        Scene { models, rig, clips, clip, face }
    }

    /// The camera frames the first model (later ones, such as a sky box, are often much larger).
    fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        self.models.first().and_then(Model::bounds).unwrap_or(([-1.0; 3], [1.0; 3]))
    }

    fn pose(&self, renderer: &Renderer, gpu: &Gpu, gpu_models: &[GpuModel], secs: f64) {
        let Some((i, rig)) = &self.rig else { return };
        let clip = self.clips.get(self.clip);
        let t = clip.map_or(0.0, |c| c.frame_at(secs));
        renderer.set_pose(&gpu.queue, &gpu_models[*i], &rig.skin(clip, t));
        if let (Some(face), Some(clip)) = (&self.face, clip) {
            face.apply(&gpu.queue, renderer, &gpu_models[*i], &face.weights(clip, &rig.skeleton, t as f32));
        }
    }
}

fn upload(renderer: &mut Renderer, gpu: &Gpu, vfs: &Vfs, scene: &Scene) -> Vec<GpuModel> {
    scene.models.iter().map(|m| renderer.upload_model(gpu, vfs, m).unwrap_or_else(|e| fail(e))).collect()
}

fn shot(args: &Args, vfs: &Vfs) {
    let scene = Scene::load(vfs, args);
    let gpu = Gpu::headless().unwrap_or_else(|e| fail(e));
    eprintln!("adapter: {}", gpu.adapter.get_info().name);
    let mut renderer = Renderer::new(&gpu.device, &gpu.queue, qwc_render::OFFSCREEN_FORMAT);
    let models = upload(&mut renderer, &gpu, vfs, &scene);
    scene.pose(&renderer, &gpu, &models, args.time);
    let (lo, hi) = scene.bounds();
    let mut cam = Camera::framing(lo, hi, args.yaw.to_radians(), args.pitch.to_radians(), 1.0 / args.zoom);
    if let Some(t) = args.look_at {
        let off = cam.eye - cam.target;
        cam.target = t.into();
        cam.eye = cam.target + off;
    }
    let target = Offscreen::new(&gpu.device, args.size);
    target.render(&gpu, &renderer, &cam, &models.iter().collect::<Vec<_>>());
    let rgba = target.read_rgba(&gpu).unwrap_or_else(|e| fail(e));
    let out = args.out.as_ref().unwrap_or_else(|| usage());
    let file = std::fs::File::create(out).unwrap_or_else(|e| fail(format!("{}: {e}", out.display())));
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), args.size.0, args.size.1);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().unwrap_or_else(|e| fail(e));
    w.write_image_data(&rgba).unwrap_or_else(|e| fail(e));
    eprintln!("wrote {}", out.display());
}

struct Viewer {
    vfs: Vfs,
    scene: Scene,
    state: Option<ViewerState>,
    yaw: f32,
    pitch: f32,
    zoom: f32,
    dragging: Option<(f64, f64)>,
    cursor: (f64, f64),
    start: Instant,
    paused_at: Option<f64>,
}

struct ViewerState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    gpu: Gpu,
    renderer: Renderer,
    models: Vec<GpuModel>,
    depth: wgpu::TextureView,
}

impl Viewer {
    fn secs(&self) -> f64 {
        self.paused_at.unwrap_or_else(|| self.start.elapsed().as_secs_f64())
    }

    fn set_clip(&mut self, delta: isize) {
        let n = self.scene.clips.len();
        if n == 0 {
            return;
        }
        self.scene.clip = (self.scene.clip as isize + delta).rem_euclid(n as isize) as usize;
        self.start = Instant::now();
        if self.paused_at.is_some() {
            self.paused_at = Some(0.0);
        }
        let c = &self.scene.clips[self.scene.clip];
        eprintln!("clip {} ({}), {} frames at {} fps", self.scene.clip, c.name, c.frames, c.fps);
        if let Some(st) = &self.state {
            st.window.set_title(&format!("QWC viewer: {}", c.name));
        }
    }

    fn redraw(&mut self) {
        let secs = self.secs();
        let Some(st) = &mut self.state else { return };
        self.scene.pose(&st.renderer, &st.gpu, &st.models, secs);
        let frame = match st.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            _ => {
                st.surface.configure(&st.gpu.device, &st.config);
                st.window.request_redraw();
                return;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        let (lo, hi) = self.scene.bounds();
        let cam = Camera::framing(lo, hi, self.yaw.to_radians(), self.pitch.to_radians(), 1.0 / self.zoom);
        let size = (st.config.width, st.config.height);
        st.renderer.draw(&st.gpu, &view, &st.depth, size, &cam, &st.models.iter().collect::<Vec<_>>());
        st.gpu.queue.present(frame);
        st.window.request_redraw();
    }
}

impl ApplicationHandler for Viewer {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("QWC viewer")
            .with_inner_size(winit::dpi::LogicalSize::new(1280, 720));
        let window = Arc::new(el.create_window(attrs).unwrap_or_else(|e| fail(e)));
        let instance = wgpu::Instance::new(qwc_render::primary_backends(
            wgpu::InstanceDescriptor::new_with_display_handle(Box::new(el.owned_display_handle())),
        ));
        let surface = instance.create_surface(window.clone()).unwrap_or_else(|e| fail(e));
        let gpu = Gpu::new(instance, Some(&surface)).unwrap_or_else(|e| fail(e));
        eprintln!("adapter: {}", gpu.adapter.get_info().name);
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&gpu.adapter, size.width.max(1), size.height.max(1))
            .unwrap_or_else(|| fail("surface not supported by the adapter"));
        let srgb = surface.get_capabilities(&gpu.adapter).formats.into_iter().find(|f| f.is_srgb());
        config.format = srgb.unwrap_or(config.format);
        surface.configure(&gpu.device, &config);
        let mut renderer = Renderer::new(&gpu.device, &gpu.queue, config.format);
        let models = upload(&mut renderer, &gpu, &self.vfs, &self.scene);
        let depth = qwc_render::depth_view(&gpu.device, (config.width, config.height));
        self.state = Some(ViewerState { window, surface, config, gpu, renderer, models, depth });
        self.set_clip(0);
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(size) => {
                if let Some(st) = &mut self.state {
                    st.config.width = size.width.max(1);
                    st.config.height = size.height.max(1);
                    st.surface.configure(&st.gpu.device, &st.config);
                    st.depth = qwc_render::depth_view(&st.gpu.device, (st.config.width, st.config.height));
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                self.dragging = (state == ElementState::Pressed).then_some(self.cursor);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                if let Some((x, y)) = self.dragging {
                    self.yaw -= (position.x - x) as f32 * 0.3;
                    self.pitch = (self.pitch + (position.y - y) as f32 * 0.3).clamp(-89.0, 89.0);
                    self.dragging = Some(self.cursor);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let d = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                };
                self.zoom = (self.zoom * 1.1f32.powf(d)).clamp(0.05, 50.0);
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => match event.logical_key
            {
                Key::Named(NamedKey::Escape) => el.exit(),
                Key::Named(NamedKey::Space) => {
                    self.paused_at = match self.paused_at {
                        Some(t) => {
                            self.start = Instant::now() - std::time::Duration::from_secs_f64(t);
                            None
                        }
                        None => Some(self.secs()),
                    }
                }
                Key::Named(NamedKey::ArrowRight) => self.set_clip(1),
                Key::Named(NamedKey::ArrowLeft) => self.set_clip(-1),
                _ => {}
            },
            _ => {}
        }
    }
}

/// `export MODEL... [--out DIR]`: each model as a `.glb` to edit (`qwc_assets::gltf_out`), at its pack slot's path
/// under `DIR` (default `work/export`): `players/<team>/<name>.glb`, `props/<name>.glb`, else
/// `models/<path>.glb`, so the folder can be copied into a pack as it is.
fn export(args: &Args, vfs: &Vfs) {
    let out = args.out.clone().unwrap_or_else(|| "work/export".into());
    let textures = TextureIndex::build(vfs);
    for path in &args.models {
        let m = qwc_assets::model::load(vfs, &textures, path).unwrap_or_else(|e| fail(format!("{path}: {e}")));
        let (glb, r) = qwc_assets::gltf_out::export(vfs, &m);
        let file = qwc_packs::model_slot(path).split('/').fold(out.clone(), |d, p| d.join(p));
        if let Some(d) = file.parent() {
            std::fs::create_dir_all(d).unwrap_or_else(|e| fail(format!("{}: {e}", d.display())));
        }
        std::fs::write(&file, glb).unwrap_or_else(|e| fail(format!("{}: {e}", file.display())));
        println!(
            "{path}: {} ({} meshes, {} vertices, {} joints, {} morph channels, {} textures)",
            file.display(),
            r.meshes,
            r.vertices,
            r.joints,
            r.morph_channels,
            r.textures
        );
        for name in r.missing_textures {
            println!("  {name}: its texture did not read; written untextured");
        }
    }
}

fn main() {
    let args = parse_args();
    if args.command == "pack" {
        return qwc_game::packs::cli(&args);
    }
    let mut vfs = Vfs::open(&args.data).unwrap_or_else(|e| fail(format!("game data at {}: {e}", args.data.display())));
    for dir in qwc_packs::current().disc_overlays() {
        let n = vfs.add_overlay(&dir).unwrap_or_else(|e| fail(format!("pack files at {}: {e}", dir.display())));
        eprintln!("{}: {n} files over the game data", dir.display());
    }
    for dir in &args.mods {
        let n = vfs.add_overlay(dir).unwrap_or_else(|e| fail(format!("mods at {}: {e}", dir.display())));
        eprintln!("{}: {n} files over the game data", dir.display());
    }
    match args.command.as_str() {
        "list" => {
            for ext in &args.models {
                for p in vfs.with_extension(ext.trim_start_matches('.')) {
                    println!("{p}");
                }
            }
        }
        "shot" => shot(&args, &vfs),
        "pads" => qwc_game::gamepad::diagnose(if args.time > 0.0 { args.time } else { 10.0 }),
        // the front end's natives keep the disc's textures and fonts (`FrontEnd::assets`): kept for the program's life
        "fe" => fe::fe(&args, Box::leak(Box::new(vfs))),
        "screen" => fe::screen(&args, Box::leak(Box::new(vfs))),
        "frontend" => live::frontend(&args, Box::leak(Box::new(vfs))),
        "movie" => movie::movie(&args, &vfs),
        "cutscene" => cutscene::shot(&args, Box::leak(Box::new(vfs))),
        "sound" => sound::sound(&args, &vfs),
        "music" => sound::music(&args, &vfs),
        "sim" => sim::sim(&args, &vfs),
        "challenge" => sim::challenge(&args, Box::leak(Box::new(vfs))),
        "commentary" => commentary::cli(&args, &vfs),
        "play" => play::play(&args, Box::leak(Box::new(vfs))),
        "online" => qwc_game::online::online(&args, Box::leak(Box::new(vfs))),
        "textures" => textures::textures(&args, &vfs),
        "export" => export(&args, &vfs),
        "view" => {
            let scene = Scene::load(&vfs, &args);
            let el = EventLoop::new().unwrap_or_else(|e| fail(e));
            let mut viewer = Viewer {
                vfs,
                scene,
                state: None,
                yaw: args.yaw,
                pitch: args.pitch,
                zoom: args.zoom,
                dragging: None,
                cursor: (0.0, 0.0),
                start: Instant::now(),
                paused_at: None,
            };
            el.run_app(&mut viewer).unwrap_or_else(|e| fail(e));
        }
        _ => usage(),
    }
}
