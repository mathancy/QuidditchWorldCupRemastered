// A model's material: its texture times its vertex colour, and for a lit material the light set (an ambient and
// three directional lights, clamped to 1 as Direct3D's lighting is).
// Matrices arrive in the game's row-major, row-vector layout; read as WGSL (column-major) they are the transposes,
// so `m * v` here equals the game's `v * M`.

struct Frame {
    view_proj: mat4x4<f32>,
    ambient: vec4<f32>,
    dirs: array<vec4<f32>, 3>,     // towards each light; zero is off
    colours: array<vec4<f32>, 3>,
};

struct Material {
    // x: 1 = has texture; y: the alpha test, 0 none, 1 cut below half, 2 the original's (kept from z up, of 255);
    // w: bits, 1 lit (2 sky and 4 lightmap are the remastered lighting's)
    flags: vec4<u32>,
    // the texture matrix's rows: u' = dot(uv_u.xyz, (u, v, 1)), and v' likewise (a Groper stage's tcMods)
    uv_u: vec4<f32>,
    uv_v: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(1) @binding(0) var<storage, read> bones: array<mat4x4<f32>>;
@group(2) @binding(0) var tex: texture_2d<f32>;
@group(2) @binding(1) var samp: sampler;
@group(2) @binding(2) var<uniform> material: Material;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) bone: u32,
};

struct VsOut {
    // invariant: a lightmap's draw lands at exactly its base's depth (their vertices are the same)
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    let m = bones[v.bone];
    var out: VsOut;
    let world = m * vec4<f32>(v.position, 1.0);
    out.clip = frame.view_proj * world;
    out.normal = (m * vec4<f32>(v.normal, 0.0)).xyz;
    out.uv = v.uv;
    out.color = v.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var base = in.color;
    if material.flags.x == 1u {
        let st = vec3<f32>(in.uv, 1.0);
        base = base * textureSample(tex, samp, vec2<f32>(dot(material.uv_u.xyz, st), dot(material.uv_v.xyz, st)));
    }
    if (material.flags.y == 1u && base.a < 0.5) || (material.flags.y == 2u && round(base.a * 255.0) < f32(material.flags.z)) {
        discard;
    }
    var light = vec3<f32>(1.0);
    let n_len = length(in.normal);
    // a normal made for a mesh without them is half as long (`MADE_NORMAL`): the original draws those unlit
    if (material.flags.w & 1u) != 0u && n_len > 0.75 {
        let n = in.normal / n_len;
        light = frame.ambient.rgb;
        for (var i = 0; i < 3; i++) {
            let d = frame.dirs[i].xyz;
            if dot(d, d) > 0.0 {
                light += max(dot(n, normalize(d)), 0.0) * frame.colours[i].rgb;
            }
        }
        light = clamp(light, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    return vec4<f32>(base.rgb * light, base.a);
}
