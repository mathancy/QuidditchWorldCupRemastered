//! Render a synthetic model offscreen and check the pixels. Skips when the machine has no graphics adapter
//! (install a software Vulkan driver such as Mesa's lavapipe to run it without a GPU).

use std::sync::Arc;

use qwc_assets::model::{Material, Mesh, Model};
use qwc_assets::{Texture, Vfs};
use qwc_formats::fpm::{blend, TextureName};
use qwc_render::overlay::OverlayTarget;
use qwc_render::{Camera, Gpu, Offscreen, Renderer, OFFSCREEN_FORMAT};

const IDENTITY: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// A rectangle `x0..x1` by `-1..1` at depth `z`, clockwise seen from -Z (the camera below) unless not `clockwise`.
fn rect(x: [f32; 2], z: f32, color: [u8; 4], clockwise: bool) -> Mesh {
    let mut indices = vec![0, 1, 2, 0, 2, 3];
    if !clockwise {
        indices = vec![0, 2, 1, 0, 3, 2];
    }
    Mesh {
        material: 0,
        level: 0,
        two_sided: false,
        positions: vec![[x[0], -1.0, z], [x[0], 1.0, z], [x[1], 1.0, z], [x[1], -1.0, z]],
        normals: Some(vec![[0.0, 0.0, -1.0]; 4]),
        colors: Some(vec![color; 4]),
        uvs: vec![[0.0, 0.0]; 4],
        bones: None,
        indices,
        morphs: Default::default(),
    }
}

fn quad(color: [u8; 4], z: f32, clockwise: bool) -> Mesh {
    rect([-1.0, 1.0], z, color, clockwise)
}

fn camera() -> Camera {
    Camera {
        eye: [0.0, 0.0, -4.0].into(),
        target: [0.0, 0.0, 0.0].into(),
        up: glam::Vec3::Y,
        fov_y: 0.8,
        near: 0.1,
        far: 10.0,
    }
}

fn gpu() -> Option<Gpu> {
    match Gpu::headless() {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("skipping: {e}");
            None
        }
    }
}

fn empty_vfs(tag: &str) -> (std::path::PathBuf, Vfs) {
    let dir = std::env::temp_dir().join(format!("qwc-render-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let vfs = Vfs::open(&dir).unwrap();
    (dir, vfs)
}

#[test]
fn draws_front_faces_with_depth() {
    let Some(gpu) = gpu() else { return };
    let (dir, vfs) = empty_vfs("depth");
    let model = Model {
        path: "test".into(),
        matrix: IDENTITY,
        materials: vec![Material { name: "plain".into(), texture: None, image: None, surface: None, groper: None }],
        skeleton: None,
        // A red quad in front of a green one, and a blue quad facing away that culling must drop.
        meshes: vec![
            quad([255, 0, 0, 255], 0.0, true),
            quad([0, 255, 0, 255], 0.5, true),
            quad([0, 0, 255, 255], -0.5, false),
        ],
    };
    let mut renderer = Renderer::new(&gpu.device, &gpu.queue, OFFSCREEN_FORMAT);
    renderer.lights = qwc_render::Lights::simple(1.0, qwc_render::glam::Vec3::NEG_Y);
    let gm = renderer.upload_model(&gpu, &vfs, &model).unwrap();
    let target = Offscreen::new(&gpu.device, (64, 64));
    target.render(&gpu, &renderer, &camera(), &[&gm]);
    let px = target.read_rgba(&gpu).unwrap();
    let at = |x: usize, y: usize| &px[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4];
    assert_eq!(at(32, 32), [255, 0, 0, 255], "centre shows the nearest front-facing quad");
    let corner = at(0, 0);
    assert!(corner[2] > corner[0] && corner[1] < 255, "corner shows the clear colour, got {corner:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn two_sided_draws_show_their_backs() {
    let Some(gpu) = gpu() else { return };
    let (dir, vfs) = empty_vfs("sides");
    // facing away, as the culled blue quad above, but two-sided
    let model = Model {
        path: "test".into(),
        matrix: IDENTITY,
        materials: vec![Material { name: "plain".into(), texture: None, image: None, surface: None, groper: None }],
        skeleton: None,
        meshes: vec![Mesh { two_sided: true, ..quad([0, 0, 255, 255], 0.0, false) }],
    };
    let mut renderer = Renderer::new(&gpu.device, &gpu.queue, OFFSCREEN_FORMAT);
    renderer.lights = qwc_render::Lights::simple(1.0, qwc_render::glam::Vec3::NEG_Y);
    let gm = renderer.upload_model(&gpu, &vfs, &model).unwrap();
    let target = Offscreen::new(&gpu.device, (64, 64));
    target.render(&gpu, &renderer, &camera(), &[&gm]);
    let px = target.read_rgba(&gpu).unwrap();
    assert_eq!(&px[(32 * 64 + 32) * 4..(32 * 64 + 32) * 4 + 4], [0, 0, 255, 255]);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A cloth's mesh takes new vertices each frame: the quad moved off the centre leaves the clear colour there.
#[test]
fn set_vertices_moves_a_mesh() {
    let Some(gpu) = gpu() else { return };
    let (dir, vfs) = empty_vfs("cloth");
    let model = Model {
        path: "test".into(),
        matrix: IDENTITY,
        materials: vec![Material { name: "plain".into(), texture: None, image: None, surface: None, groper: None }],
        skeleton: None,
        meshes: vec![quad([255, 0, 0, 255], 0.0, true)],
    };
    let mut renderer = Renderer::new(&gpu.device, &gpu.queue, OFFSCREEN_FORMAT);
    renderer.lights = qwc_render::Lights::simple(1.0, qwc_render::glam::Vec3::NEG_Y);
    let gm = renderer.upload_model(&gpu, &vfs, &model).unwrap();
    let target = Offscreen::new(&gpu.device, (64, 64));
    let centre = |target: &Offscreen| target.read_rgba(&gpu).unwrap()[(32 * 64 + 32) * 4..][..4].to_vec();
    target.render(&gpu, &renderer, &camera(), &[&gm]);
    assert_eq!(centre(&target), [255, 0, 0, 255]);
    renderer.set_vertices(&gpu.queue, &gm, 0, &rect([0.5, 1.0], 0.0, [255, 0, 0, 255], true));
    target.render(&gpu, &renderer, &camera(), &[&gm]);
    assert_ne!(centre(&target), [255, 0, 0, 255]);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A Groper stage's `tcMod scroll` moves the texture under the mesh: half a texture's width a second later, the
/// left texel's place shows the right one.
#[test]
fn a_scrolled_material_moves_its_texture() {
    use qwc_assets::groper::{Stage, TcMod};
    let Some(gpu) = gpu() else { return };
    let (dir, vfs) = empty_vfs("scroll");
    let texels = vec![255, 0, 0, 255, 0, 255, 0, 255];
    let image = Arc::new(Texture { path: "red green".into(), width: 2, height: 1, levels: vec![texels] });
    let model = Model {
        path: "test".into(),
        matrix: IDENTITY,
        materials: vec![Material {
            name: "scrolled".into(),
            texture: None,
            image: Some(image),
            surface: None,
            groper: None,
        }],
        skeleton: None,
        // every vertex at the left texel's middle
        meshes: vec![Mesh { uvs: vec![[0.25, 0.5]; 4], ..quad([255; 4], 0.0, true) }],
    };
    let mut renderer = Renderer::new(&gpu.device, &gpu.queue, OFFSCREEN_FORMAT);
    renderer.lights = qwc_render::Lights::simple(1.0, qwc_render::glam::Vec3::NEG_Y);
    let mut gm = renderer.upload_model(&gpu, &vfs, &model).unwrap();
    let stage = Stage { frames: vec!["red green".into()], fps: 0.0, tcmods: vec![TcMod::Scroll { u: 0.5, v: 0.0 }] };
    renderer.set_animation(&gpu, &vfs, &mut gm, 0, stage, &[]);
    assert!(Renderer::animates(&gm));
    let target = Offscreen::new(&gpu.device, (64, 64));
    let centre = |target: &Offscreen| target.read_rgba(&gpu).unwrap()[(32 * 64 + 32) * 4..][..4].to_vec();
    renderer.animate(&gpu, &mut gm, 0.0);
    target.render(&gpu, &renderer, &camera(), &[&gm]);
    assert_eq!(centre(&target), [255, 0, 0, 255]);
    renderer.animate(&gpu, &mut gm, 1.0);
    target.render(&gpu, &renderer, &camera(), &[&gm]);
    assert_eq!(centre(&target), [0, 255, 0, 255]);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The original's materials on a target without sRGB (the front end's): textures as stored, a lightmap twice its
/// base times itself and drawn after it whatever the order of the draws, and only the finest level of detail.
#[test]
fn lightmaps_modulate_their_base_twice() {
    let Some(gpu) = gpu() else { return };
    let (dir, vfs) = empty_vfs("lightmap");
    let surface = |flags| Some(TextureName { name: String::new(), hash: 0, flags }.surface());
    let texel = Arc::new(Texture { path: "texel".into(), width: 1, height: 1, levels: vec![vec![128, 64, 32, 255]] });
    let model = Model {
        path: "test".into(),
        matrix: IDENTITY,
        materials: vec![
            // an unblended texture (layer 0), a lightmap (layer 1, its colour from the vertices)
            Material {
                name: "base".into(),
                texture: None,
                image: Some(texel),
                surface: surface(0x0003_0e00),
                groper: None,
            },
            Material { name: "lm".into(), texture: None, image: None, surface: surface(0x0000_1401), groper: None },
        ],
        skeleton: None,
        meshes: vec![
            // the lightmap first: the layers put it after its base
            Mesh { material: 1, ..rect([0.1, 1.0], 0.0, [128, 255, 64, 255], true) },
            rect([-1.0, -0.1], 0.0, [255; 4], true),
            rect([0.1, 1.0], 0.0, [255; 4], true),
            // a coarser level, nearer: not drawn
            Mesh { level: 1, ..quad([0, 0, 255, 255], -0.5, true) },
        ],
    };
    let format = qwc_render::overlay::OVERLAY_FORMAT;
    assert_eq!(blend::SRCALPHA_LIGHTMAP_KEEP, model.materials[1].surface.unwrap().blend);
    let mut renderer = Renderer::new(&gpu.device, &gpu.queue, format);
    renderer.lights = qwc_render::Lights::simple(1.0, qwc_render::glam::Vec3::NEG_Y);
    let gm = renderer.upload_model(&gpu, &vfs, &model).unwrap();
    let target = OverlayTarget::new(&gpu.device, (64, 64));
    let depth = qwc_render::depth_view(&gpu.device, (64, 64));
    renderer.draw(&gpu, &target.view, &depth, (64, 64), &camera(), &[&gm]);
    let px = target.read_rgba(&gpu).unwrap();
    let rgb = |x: usize| <[u8; 3]>::try_from(&px[(32 * 64 + x) * 4..(32 * 64 + x) * 4 + 3]).unwrap();
    let near = |a: [u8; 3], b: [u8; 3]| a.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= 1);
    // the left: the texel as stored
    assert!(near(rgb(16), [128, 64, 32]), "base {:?}", rgb(16));
    // the right: twice base times lightmap, of 255 (2 * 128 * 128, 2 * 64 * 255, 2 * 32 * 64)
    assert!(near(rgb(48), [128, 128, 16]), "lit {:?}", rgb(48));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The remastered lighting with the sun off and no fog draws as the original does: the textures, the vertex
/// colours and the lightmaps (twice the base times the lightmap) in linear light, the same values out.
#[test]
fn the_remaster_without_its_sun_is_the_original() {
    let Some(gpu) = gpu() else { return };
    let (dir, vfs) = empty_vfs("remaster");
    let surface = |flags| Some(TextureName { name: String::new(), hash: 0, flags }.surface());
    let texel = Arc::new(Texture { path: "texel".into(), width: 1, height: 1, levels: vec![vec![128, 64, 32, 255]] });
    let model = Model {
        path: "test".into(),
        matrix: IDENTITY,
        materials: vec![
            Material {
                name: "base".into(),
                texture: None,
                image: Some(texel),
                surface: surface(0x0003_0e00),
                groper: None,
            },
            Material { name: "lm".into(), texture: None, image: None, surface: surface(0x0000_1401), groper: None },
        ],
        skeleton: None,
        meshes: vec![
            Mesh { material: 1, normals: None, ..rect([0.1, 1.0], 0.0, [128, 255, 64, 255], true) },
            Mesh { normals: None, ..rect([-1.0, -0.1], 0.0, [200, 150, 100, 255], true) },
            Mesh { normals: None, ..rect([0.1, 1.0], 0.0, [255; 4], true) },
        ],
    };
    let format = qwc_render::overlay::OVERLAY_FORMAT;
    let mut renderer = Renderer::new(&gpu.device, &gpu.queue, format);
    let mut gm = renderer.upload_model(&gpu, &vfs, &model).unwrap();
    renderer.set_lit(&gpu, &mut gm, false);
    let target = OverlayTarget::new(&gpu.device, (64, 64));
    let depth = qwc_render::depth_view(&gpu.device, (64, 64));
    renderer.settings.apply("sun=0,fog=0,baked=1").unwrap();
    let mut shot = |look| {
        renderer.look = look;
        renderer.draw(&gpu, &target.view, &depth, (64, 64), &camera(), &[&gm]);
        target.read_rgba(&gpu).unwrap()
    };
    let original = shot(qwc_render::Look::Original);
    let remastered = shot(qwc_render::Look::Remastered);
    for x in [16, 48] {
        let (a, b) = (&original[(32 * 64 + x) * 4..][..3], &remastered[(32 * 64 + x) * 4..][..3]);
        assert!(a.iter().zip(b).all(|(p, q)| p.abs_diff(*q) <= 2), "at {x}: original {a:?}, remastered {b:?}");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// In the remastered lighting the sun casts shadows: a strip between it and a wall darkens the wall where the
/// sun's light, slanting, would have reached it, and the wall away from the strip stays lit.
#[test]
fn the_remastered_sun_casts_shadows() {
    use qwc_render::glam::{Vec3, Vec4};
    let Some(gpu) = gpu() else { return };
    let (dir, vfs) = empty_vfs("shadow");
    let model = Model {
        path: "test".into(),
        matrix: IDENTITY,
        materials: vec![Material { name: "plain".into(), texture: None, image: None, surface: None, groper: None }],
        skeleton: None,
        meshes: vec![
            // the wall, a unit behind the strip, both facing the camera
            Mesh { normals: None, ..rect([-3.0, 3.0], 1.0, [255; 4], true) },
            Mesh { normals: None, ..rect([-0.8, -0.2], 0.0, [255; 4], true) },
        ],
    };
    let mut renderer = Renderer::new(&gpu.device, &gpu.queue, OFFSCREEN_FORMAT);
    // the sun behind the camera and to its left: its light travels +x and +z
    let towards = Vec3::new(-1.0, 0.0, -1.0).normalize().to_array();
    let off = ([0.0; 3], [0.0; 3]);
    renderer.lights = qwc_render::Lights { ambient: [0.0; 3], lights: [([1.0; 3], towards), off, off] };
    renderer.look = qwc_render::Look::Remastered;
    renderer.settings.apply("fog=0,sun=1,baked=0.2,baked_sun=1,shadow_strength=1").unwrap();
    let mut gm = renderer.upload_model(&gpu, &vfs, &model).unwrap();
    renderer.set_lit(&gpu, &mut gm, false);
    let target = Offscreen::new(&gpu.device, (128, 128));
    target.render(&gpu, &renderer, &camera(), &[&gm]);
    let px = target.read_rgba(&gpu).unwrap();
    let vp = camera().view_proj(1.0);
    let at = |x: f32| {
        let c = vp * Vec4::new(x, 0.0, 1.0, 1.0);
        let (sx, sy) = (((c.x / c.w) * 0.5 + 0.5) * 128.0, (0.5 - (c.y / c.w) * 0.5) * 128.0);
        px[(sy as usize * 128 + sx as usize) * 4]
    };
    // the strip's shadow falls on x 0.2..0.8 of the wall
    let (shade, lit) = (at(0.5), at(1.5));
    assert!(lit > shade + 60, "lit {lit}, shadowed {shade}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The remastered light probes see their surroundings: inside a sky of even white all round, a probe's light on
/// a lit surface is that of an even white ambient (the bands' projection and its evaluation agree).
#[test]
fn a_probe_in_an_even_sky_lights_as_an_even_ambient() {
    let Some(gpu) = gpu() else { return };
    let (dir, vfs) = empty_vfs("probes");
    // a box of six two-sided faces around the origin, white
    let face = |x: [f32; 2], z: f32| Mesh { two_sided: true, normals: None, ..rect(x, z, [255; 4], true) };
    let mut walls = Vec::new();
    for k in 0..6 {
        let mut m = face([-50.0, 50.0], 50.0);
        for p in &mut m.positions {
            let [x, y, z] = *p;
            *p = match k {
                0 => [x, y * 50.0, z],
                1 => [x, y * 50.0, -z],
                2 => [z, y * 50.0, x],
                3 => [-z, y * 50.0, x],
                4 => [x, z, y * 50.0],
                _ => [x, -z, y * 50.0],
            };
        }
        walls.push(m);
    }
    let sky = Model {
        path: "sky".into(),
        matrix: IDENTITY,
        materials: vec![Material { name: "sky".into(), texture: None, image: None, surface: None, groper: None }],
        skeleton: None,
        meshes: walls,
    };
    let card = Model {
        path: "card".into(),
        matrix: IDENTITY,
        materials: vec![Material { name: "plain".into(), texture: None, image: None, surface: None, groper: None }],
        skeleton: None,
        meshes: vec![quad([255; 4], 0.0, true)],
    };
    let mut renderer = Renderer::new(&gpu.device, &gpu.queue, OFFSCREEN_FORMAT);
    let off = ([0.0; 3], [0.0; 3]);
    renderer.lights = qwc_render::Lights { ambient: [0.0; 3], lights: [([0.0; 3], [0.0, 1.0, 0.0]), off, off] };
    renderer.look = qwc_render::Look::Remastered;
    renderer.settings.apply("fog=0,ao=0,sun=0,fill=0,sky=1,ground=1").unwrap();
    let mut sky_gm = renderer.upload_model(&gpu, &vfs, &sky).unwrap();
    renderer.set_sky(&gpu, &mut sky_gm);
    let card_gm = renderer.upload_model(&gpu, &vfs, &card).unwrap();
    let target = Offscreen::new(&gpu.device, (64, 64));
    let centre = |target: &Offscreen| target.read_rgba(&gpu).unwrap()[(32 * 64 + 32) * 4..][..3].to_vec();
    // an even ambient of white
    renderer.settings.probes = 0.0;
    target.render(&gpu, &renderer, &camera(), &[&card_gm]);
    let even = centre(&target);
    // the probes' light, inside the white box
    renderer.bake_probes(&gpu, &[&sky_gm], [-1.0; 3], [1.0; 3], [2, 2, 2]);
    // (the sky and ground ambient black, so only the probes can light it)
    renderer.settings.apply("probes=1,sky=0,ground=0").unwrap();
    target.render(&gpu, &renderer, &camera(), &[&card_gm]);
    let probed = centre(&target);
    assert!(even[0] > 200, "the even ambient {even:?}");
    assert!(even.iter().zip(&probed).all(|(a, b)| a.abs_diff(*b) <= 6), "even {even:?}, probed {probed:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The packs' shaders in the remastered lighting: a post pass passing the picture through changes nothing, a
/// material shader draws the materials it names, a post pass with a number of its own greys the picture, and a
/// shader that does not compile leaves the stock one drawing.
#[test]
fn pack_shaders_draw_and_broken_ones_fall_back() {
    use qwc_render::custom::{MaterialShader, PostPass, Shaders, PARAMS};
    let Some(gpu) = gpu() else { return };
    let (dir, vfs) = empty_vfs("custom");
    let texel =
        Arc::new(Texture { path: "pics/water.png".into(), width: 1, height: 1, levels: vec![vec![60, 120, 200, 255]] });
    let model = Model {
        path: "test".into(),
        matrix: IDENTITY,
        materials: vec![
            Material { name: "lake".into(), texture: None, image: Some(texel), surface: None, groper: None },
            Material { name: "wall".into(), texture: None, image: None, surface: None, groper: None },
        ],
        skeleton: None,
        meshes: vec![
            rect([-1.0, -0.1], 0.0, [255; 4], true),
            Mesh { material: 1, ..rect([0.1, 1.0], 0.0, [200, 150, 100, 255], true) },
        ],
    };
    let format = qwc_render::overlay::OVERLAY_FORMAT;
    let shot = |shaders: Shaders| {
        let mut renderer = Renderer::new(&gpu.device, &gpu.queue, format);
        renderer.shaders = Arc::new(shaders);
        renderer.look = qwc_render::Look::Remastered;
        renderer.settings.apply("fog=0,ao=0").unwrap();
        let gm = renderer.upload_model(&gpu, &vfs, &model).unwrap();
        let target = OverlayTarget::new(&gpu.device, (64, 64));
        let depth = qwc_render::depth_view(&gpu.device, (64, 64));
        renderer.draw(&gpu, &target.view, &depth, (64, 64), &camera(), &[&gm]);
        let rgba = target.read_rgba(&gpu).unwrap();
        [16, 48].map(|x| <[u8; 3]>::try_from(&rgba[(32 * 64 + x) * 4..][..3]).unwrap())
    };
    let material = |source: &str| MaterialShader {
        name: "test:materials/red.wgsl".into(),
        source: source.into(),
        textures: vec!["*WATER*".into()],
        materials: vec![],
    };
    let pass = |source: &str, grey: f32| {
        let mut params = [0.0; PARAMS];
        params[0] = grey;
        PostPass { name: "test:post/grey.wgsl".into(), source: source.into(), params }
    };
    let red = "@fragment\nfn fs_main(in: VsOut) -> FsOut {\n    return FsOut(vec4<f32>(0.5, 0.0, 0.0, 1.0), vec4<f32>(0.0));\n}\n";
    let grey = "@fragment\nfn fs_main(in: PostIn) -> @location(0) vec4<f32> {\n    let c = picture_at(in.uv);\n    let g = dot(c.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));\n    return vec4<f32>(mix(c.rgb, vec3<f32>(g), param(0u)), c.a);\n}\n";
    let close = |a: [u8; 3], b: [u8; 3]| a.iter().zip(b).all(|(p, q)| p.abs_diff(q) <= 2);

    let stock = shot(Shaders::default());
    assert!(stock[0][2] > stock[0][0] + 20, "the lake is blue: {stock:?}");
    let through = shot(Shaders { post: vec![pass(grey, 0.0)], ..Default::default() });
    assert!(close(through[0], stock[0]) && close(through[1], stock[1]), "{through:?} vs {stock:?}");
    let own = shot(Shaders { materials: vec![material(red)], ..Default::default() });
    assert!(own[0][0] > 150 && own[0][1] < 10 && own[0][2] < 10, "the lake drawn red: {own:?}");
    assert!(close(own[1], stock[1]), "the wall stays: {own:?} vs {stock:?}");
    let greyed = shot(Shaders { post: vec![pass(grey, 1.0)], ..Default::default() });
    assert!(greyed.iter().all(|c| c[0].abs_diff(c[1]) <= 1 && c[1].abs_diff(c[2]) <= 1), "{greyed:?}");
    let broken = shot(Shaders {
        materials: vec![material("@fragment\nfn fs_main(in: VsOut) -> FsOut { return nope; }\n")],
        post: vec![pass("fn not_a_pass() {}", 1.0)],
    });
    assert_eq!(broken, stock);
    // one naga passes but the pipeline refuses (a binding the engine does not give)
    let unbound = "@group(4) @binding(0) var<uniform> extra: vec4<f32>;
@fragment
fn fs_main(in: VsOut) -> FsOut {
    return FsOut(extra, vec4<f32>(0.0));
}
";
    qwc_render::custom::check_material(unbound).unwrap();
    assert_eq!(shot(Shaders { materials: vec![material(unbound)], ..Default::default() }), stock);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A Groper material that adds (the Tackle 'n' Shoot ghost's): its faces' depth first (`zfill`), then their colour
/// added where alpha passes. Of two parts one behind the other, the front one's depth keeps the back one out; without
/// `zfill` both add (the colour stage writes no depth here, so the depth is the `zfill`'s alone).
#[test]
fn a_ghost_adds_only_its_nearest_faces() {
    use qwc_assets::groper::Render;
    let Some(gpu) = gpu() else { return };
    let (dir, vfs) = empty_vfs("ghost");
    let centre = |zfill: bool| {
        let texel =
            Arc::new(Texture { path: "ghost".into(), width: 1, height: 1, levels: vec![vec![100, 100, 100, 188]] });
        // mode 12, `SRCALPHA_ADD_KEEP`
        let groper = Some(Render { blend: 12, alpha_min: Some(1), depth_write: false, zfill });
        let model = Model {
            path: "test".into(),
            matrix: IDENTITY,
            materials: vec![
                Material { name: "backdrop".into(), texture: None, image: None, surface: None, groper: None },
                Material { name: "ghost".into(), texture: None, image: Some(texel), surface: None, groper },
            ],
            skeleton: None,
            meshes: vec![
                quad([0, 0, 0, 255], 1.0, true),
                Mesh { material: 1, ..quad([255; 4], 0.0, true) },
                Mesh { material: 1, ..quad([255; 4], 0.5, true) },
            ],
        };
        let mut renderer = Renderer::new(&gpu.device, &gpu.queue, OFFSCREEN_FORMAT);
        renderer.lights = qwc_render::Lights::simple(1.0, qwc_render::glam::Vec3::NEG_Y);
        let gm = renderer.upload_model(&gpu, &vfs, &model).unwrap();
        let target = Offscreen::new(&gpu.device, (64, 64));
        target.render(&gpu, &renderer, &camera(), &[&gm]);
        target.read_rgba(&gpu).unwrap()[(32 * 64 + 32) * 4..][..4].to_vec()
    };
    let (one, both) = (centre(true), centre(false));
    assert!(one[0] > 20 && one[0] == one[1] && one[1] == one[2], "the ghost adds over the black: {one:?}");
    assert!(both[0] > one[0] + 10, "without zfill the hidden part adds as well: {one:?} {both:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}
