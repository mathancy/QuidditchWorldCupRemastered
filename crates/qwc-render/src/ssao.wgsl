// The remastered lighting's ambient occlusion (`Look::Remastered`): how much of the sky each pixel's surface sees,
// from the depth around it: samples on a spiral within a radius in the world, each counting by how steeply it rises
// above the surface's plane and fading with its distance. `composite.wgsl` blurs it and darkens only the indirect
// light with it.

struct Post {
    inv_view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    sun_dir: vec4<f32>,
    sun_colour: vec4<f32>,
    fog_colour: vec4<f32>,
    fog: vec4<f32>,
    tone: vec4<f32>,
    rect: vec4<f32>,
    // x: the radius in world units; y: the strength; z: the pixels a unit covers at a unit away (the projection's
    // scale); w: the samples
    ao: vec4<f32>,
};

@group(0) @binding(0) var depth: texture_depth_2d;
@group(0) @binding(1) var<uniform> post: Post;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

fn world_at(px: vec2<i32>) -> vec4<f32> {
    let d = textureLoad(depth, px, 0);
    let f = vec2<f32>(px) + 0.5;
    let ndc = vec2<f32>((f.x - post.rect.x) / post.rect.z * 2.0 - 1.0, 1.0 - (f.y - post.rect.y) / post.rect.w * 2.0);
    let w = post.inv_view_proj * vec4<f32>(ndc, d, 1.0);
    return vec4<f32>(w.xyz / w.w, d);
}

fn inside(px: vec2<i32>) -> bool {
    let lo = vec2<i32>(post.rect.xy);
    let hi = lo + vec2<i32>(post.rect.zw);
    return all(px >= lo) && all(px < hi);
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let px = vec2<i32>(frag.xy);
    let p = world_at(px);
    if p.w >= 1.0 {
        return vec4<f32>(1.0);
    }
    // the surface's normal from its neighbours, each side the nearer one (so an edge does not bend it)
    let l = world_at(px - vec2<i32>(1, 0)).xyz;
    let r = world_at(px + vec2<i32>(1, 0)).xyz;
    let u = world_at(px - vec2<i32>(0, 1)).xyz;
    let d = world_at(px + vec2<i32>(0, 1)).xyz;
    let dx = select(p.xyz - l, r - p.xyz, distance(r, p.xyz) < distance(l, p.xyz));
    let dy = select(p.xyz - u, d - p.xyz, distance(d, p.xyz) < distance(u, p.xyz));
    var n = normalize(cross(dy, dx));
    let to_eye = post.eye.xyz - p.xyz;
    if dot(n, to_eye) < 0.0 {
        n = -n;
    }
    let dist = length(to_eye);
    let radius = post.ao.x;
    let radius_px = radius * post.ao.z / max(dist, 1e-3);
    if radius_px < 1.0 {
        return vec4<f32>(1.0);
    }
    let samples = i32(post.ao.w);
    // interleaved gradient noise turns the spiral from pixel to pixel; the blur evens it out
    let noise = fract(52.9829189 * fract(dot(frag.xy, vec2<f32>(0.06711056, 0.00583715))));
    var sum = 0.0;
    for (var i = 0; i < samples; i++) {
        let t = (f32(i) + 0.5) / f32(samples);
        let angle = (f32(i) * 2.39996 + noise * 6.28318);
        let off = vec2<f32>(cos(angle), sin(angle)) * t * min(radius_px, 64.0);
        let q = px + vec2<i32>(off);
        if !inside(q) {
            continue;
        }
        let v = world_at(q).xyz - p.xyz;
        let vv = dot(v, v);
        let falloff = max(1.0 - vv / (radius * radius), 0.0);
        // how far above the surface's plane the sample rises, as the cosine of its angle from the normal
        sum += falloff * max(dot(v, n) * inverseSqrt(max(vv, 1e-6)) - 0.15, 0.0);
    }
    let ao = clamp(1.0 - 1.6 * post.ao.y * sum / f32(samples), 0.0, 1.0);
    return vec4<f32>(ao, ao, ao, 1.0);
}
