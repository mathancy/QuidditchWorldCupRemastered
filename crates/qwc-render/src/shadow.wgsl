// The sun's shadow maps (`Look::Remastered`): each model's depth from the sun, one cascade at a time. The alpha
// tested and blended materials cut their texture's holes (the crowd, the foliage, the nets).

struct Cascade {
    view_proj: mat4x4<f32>,
};

struct Material {
    flags: vec4<u32>,
    uv_u: vec4<f32>,
    uv_v: vec4<f32>,
};

@group(0) @binding(0) var<uniform> cascade: Cascade;
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
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) alpha: f32,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = cascade.view_proj * (bones[v.bone] * vec4<f32>(v.position, 1.0));
    out.uv = v.uv;
    out.alpha = v.color.a;
    return out;
}

@fragment
fn fs_cut(in: VsOut) {
    if material.flags.x == 1u {
        let st = vec3<f32>(in.uv, 1.0);
        let a = textureSample(tex, samp, vec2<f32>(dot(material.uv_u.xyz, st), dot(material.uv_v.xyz, st))).a;
        if a * in.alpha < 0.5 {
            discard;
        }
    }
}
