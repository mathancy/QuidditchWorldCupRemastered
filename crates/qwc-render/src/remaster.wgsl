// The remastered lighting (`Look::Remastered`; the remaster's own, not the original's): linear light into an HDR
// target, the sun with cascaded shadow maps, a sky and ground ambient for the lit models, and the stadium's baked
// light (its vertex colours and lightmaps) kept as its indirect light. `composite.wgsl` adds the fog and tone maps.
// Matrices as `shader.wgsl` takes them (the game's row-vector ones read as their transposes).
//
// This file has no fragment entry point: `remaster_main.wgsl`'s (the stock one) or a pack's material shader is
// added after it (`docs/SHADERS.md`), so what is here is the material shaders' interface: keep it as documented.

struct Frame {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    sun_dir: vec4<f32>,      // towards the sun
    sun_colour: vec4<f32>,
    sky: vec4<f32>,          // the ambient from above, and from below
    ground: vec4<f32>,
    fill_dir: vec4<f32>,
    fill_colour: vec4<f32>,
    // x: the baked light's gain; y: the sun's on baked surfaces; z: 1 = textures hold stored values to decode;
    // w: the sky's gain
    baked: vec4<f32>,
    cascades: array<mat4x4<f32>, 3>,
    splits: vec4<f32>,       // each cascade's far end in view depth; w: the shadows' strength
    texels: vec4<f32>,       // each cascade's texel in world units; w: the lightmaps' strength (0 none)
    // the light probes' grid (`probes.wgsl`): its first probe's place (w: their light's gain, 0 none), the step
    // between probes, and how many there are each way
    probe_lo: vec4<f32>,
    probe_step: vec4<f32>,
    probe_count: vec4<f32>,
    time: vec4<f32>,         // x: the game's clock in seconds
};

struct Material {
    // x: 1 = has texture; y: alpha test (0 none, 1 half, 2 from z of 255); w: bits, 1 lit, 2 sky, 4 lightmap,
    // 8 drawn as the original
    flags: vec4<u32>,
    uv_u: vec4<f32>,
    uv_v: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(1) @binding(0) var<storage, read> bones: array<mat4x4<f32>>;
@group(2) @binding(0) var tex: texture_2d<f32>;
@group(2) @binding(1) var samp: sampler;
@group(2) @binding(2) var<uniform> material: Material;
@group(3) @binding(0) var shadow_map: texture_depth_2d_array;
@group(3) @binding(1) var shadow_samp: sampler_comparison;
@group(3) @binding(2) var probes: texture_2d<f32>;

const LIT: u32 = 1u;
const SKY: u32 = 2u;
const LIGHTMAP: u32 = 4u;
const AS_ORIGINAL: u32 = 8u;
const SHADOW_SIZE: f32 = 2048.0;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) bone: u32,
};

struct VsOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) world: vec3<f32>,
    @location(4) depth: f32,
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
    out.world = world.xyz;
    out.depth = out.clip.w;
    return out;
}

fn decode(c: vec3<f32>) -> vec3<f32> {
    return pow(max(c, vec3<f32>(0.0)), vec3<f32>(2.2));
}

fn encode(c: vec3<f32>) -> vec3<f32> {
    return pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
}

// The sun's light reaching `world` (1 lit, 0 shadowed), from the first cascade holding it, 3 x 3 filtered.
fn sunlight(world: vec3<f32>, n: vec3<f32>, depth: f32) -> f32 {
    var i = 0;
    if depth > frame.splits.x { i = 1; }
    if depth > frame.splits.y { i = 2; }
    if depth > frame.splits.z { return 1.0; }
    let texel = frame.texels[i];
    // pushed off the surface by its normal, a texel and a half, so a surface does not shadow itself
    let p = frame.cascades[i] * vec4<f32>(world + n * texel * 1.5, 1.0);
    let uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) || p.z > 1.0 {
        return 1.0;
    }
    var lit = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let o = vec2<f32>(f32(x), f32(y)) / SHADOW_SIZE;
            lit += textureSampleCompareLevel(shadow_map, shadow_samp, uv + o, i, p.z);
        }
    }
    lit /= 9.0;
    // the last cascade fades out over its last tenth
    let fade = select(1.0, clamp((frame.splits.z - depth) / (frame.splits.z * 0.1), 0.0, 1.0), i == 2);
    return mix(1.0, lit, frame.splits.w * fade);
}

struct FsOut {
    @location(0) colour: vec4<f32>,
    // the part of the light that is indirect, which the ambient occlusion darkens (`composite.wgsl`)
    @location(1) share: vec4<f32>,
};

fn shaded(colour: vec3<f32>, alpha: f32, indirect: vec3<f32>, all: vec3<f32>) -> FsOut {
    let lum = vec3<f32>(0.2126, 0.7152, 0.0722);
    let share = dot(indirect, lum) / max(dot(all, lum), 1e-4);
    return FsOut(vec4<f32>(colour, alpha), vec4<f32>(share, 0.0, 0.0, 1.0));
}

// The light arriving at a surface facing `n` at `world`, from the probes around it (their coefficients mixed
// trilinearly, then evaluated: the irradiance over pi, as an ambient colour).
fn probe_light(world: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    let count = frame.probe_count.xyz;
    let g = clamp((world - frame.probe_lo.xyz) / frame.probe_step.xyz, vec3<f32>(0.0), count - 1.0);
    let base = min(floor(g), max(count - 2.0, vec3<f32>(0.0)));
    let f = g - base;
    var sh: array<vec3<f32>, 4>;
    for (var k = 0; k < 8; k++) {
        let o = vec3<f32>(f32(k & 1), f32((k >> 1) & 1), f32((k >> 2) & 1));
        let at = min(base + o, count - 1.0);
        let w = select(1.0 - f.x, f.x, o.x > 0.0) * select(1.0 - f.y, f.y, o.y > 0.0) * select(1.0 - f.z, f.z, o.z > 0.0);
        let index = i32(at.x + count.x * (at.y + count.y * at.z));
        for (var c = 0; c < 4; c++) {
            sh[c] += textureLoad(probes, vec2<i32>(c, index), 0).rgb * w;
        }
    }
    // irradiance by Ramamoorthi and Hanrahan's bands 0 and 1 (pi and 2 pi / 3), over pi
    let e = sh[0] * 0.282095 + (2.0 / 3.0) * 0.488603 * (sh[1] * n.y + sh[2] * n.z + sh[3] * n.x);
    return max(e, vec3<f32>(0.0)) * frame.probe_lo.w;
}

// The material's texture at `uv` through its texture matrix (white without a texture).
fn material_texel(uv: vec2<f32>) -> vec4<f32> {
    if material.flags.x != 1u {
        return vec4<f32>(1.0);
    }
    let st = vec3<f32>(uv, 1.0);
    return textureSample(tex, samp, vec2<f32>(dot(material.uv_u.xyz, st), dot(material.uv_v.xyz, st)));
}

// Whether the material's alpha test drops a pixel of `alpha` (cut at half, or below its reference).
fn alpha_cut(alpha: f32) -> bool {
    return (material.flags.y == 1u && alpha < 0.5) || (material.flags.y == 2u && round(alpha * 255.0) < f32(material.flags.z));
}

// The stock shading of a pixel whose texture gave `texel`: a lightmap, the sky and the materials drawn as the
// original as they are, the rest lit by the sun and its shadows, and the probes (or the sky and ground) and the
// fill for the lit ones, the baked light for the stadium.
fn shade(in: VsOut, front: bool, texel: vec4<f32>) -> FsOut {
    let alpha = texel.a * in.color.a;
    // the texture's stored values (sampled as they are, or decoded by an sRGB texture)
    var stored = texel.rgb;
    if frame.baked.z == 0.0 {
        stored = encode(texel.rgb);
    }
    let kind = material.flags.w;
    if (kind & LIGHTMAP) != 0u {
        // drawn `DESTCOLOR, SRCCOLOR` (twice it times what is there): twice the stored value, as linear light,
        // mixed towards no change by the lightmaps' strength (`texels.w`)
        let factor = mix(vec3<f32>(1.0), decode(2.0 * stored * in.color.rgb), frame.texels.w);
        return FsOut(vec4<f32>(0.5 * factor, 1.0), vec4<f32>(0.0));
    }
    let albedo = decode(stored);
    if (kind & SKY) != 0u {
        return FsOut(vec4<f32>(albedo * decode(in.color.rgb) * frame.baked.w, alpha), vec4<f32>(0.0));
    }
    if (kind & AS_ORIGINAL) != 0u {
        return FsOut(vec4<f32>(albedo * decode(in.color.rgb), alpha), vec4<f32>(0.0));
    }
    var n = in.normal;
    let len = length(n);
    if len > 0.0 {
        n = n / len;
    }
    if !front {
        n = -n;
    }
    let sun = frame.sun_dir.xyz;
    let facing = max(dot(n, sun), 0.0);
    var shadow = 1.0;
    if facing > 0.0 {
        shadow = sunlight(in.world, n, in.depth);
    }
    let direct = frame.sun_colour.rgb * facing * shadow;
    var indirect: vec3<f32>;
    var sunlit: vec3<f32>;
    if (kind & LIT) != 0u {
        var ambient = mix(frame.ground.rgb, frame.sky.rgb, n.y * 0.5 + 0.5);
        if frame.probe_lo.w > 0.0 {
            ambient = probe_light(in.world, n);
        }
        let fill = max(dot(n, frame.fill_dir.xyz), 0.0) * frame.fill_colour.rgb;
        let tint = decode(in.color.rgb);
        indirect = (ambient + fill) * tint;
        sunlit = direct * tint;
    } else {
        // the stadium: its vertex colours are the original's baked light, kept as its indirect light
        indirect = decode(in.color.rgb) * frame.baked.x;
        sunlit = direct * frame.baked.y;
    }
    let light = indirect + sunlit;
    return shaded(albedo * light, alpha, indirect, light);
}
