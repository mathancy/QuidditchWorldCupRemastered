// The remastered lighting's last step (`Look::Remastered`): the HDR picture fogged by the air between the eye and
// each pixel (a fog thinning with height, brighter towards the sun), exposed and tone mapped (Khronos' PBR Neutral's
// shoulder, which leaves the art's colours as they are below its highlights) into the target, in its encoding.

struct Post {
    inv_view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    sun_dir: vec4<f32>,
    sun_colour: vec4<f32>,
    fog_colour: vec4<f32>,  // w: the density at the base height, per unit
    fog: vec4<f32>,         // x: the base height; y: the density's falloff per unit up; z: the most fog; w: start
    tone: vec4<f32>,        // x: exposure; y: 1 = the target holds stored values (encode them); z: the view shown
    rect: vec4<f32>,        // the view's x, y, width, height in pixels
    ao: vec4<f32>,          // as `ssao.wgsl`'s
    sky: vec4<f32>,         // x: the sky's light probe's row; y: how much of the fog's colour is the sky's mean
    // the colour grade: x temperature, y saturation, z contrast, w vignette; the filter; lift, gamma and gain
    grade: vec4<f32>,
    tint: vec4<f32>,
    lift: vec4<f32>,
    gamma: vec4<f32>,
    gain: vec4<f32>,
};

@group(0) @binding(0) var hdr: texture_2d<f32>;
@group(0) @binding(1) var depth: texture_depth_2d;
@group(0) @binding(2) var<uniform> post: Post;
@group(0) @binding(3) var share: texture_2d<f32>;      // the part of each pixel's light that is indirect
@group(0) @binding(4) var occlusion: texture_2d<f32>;  // `ssao.wgsl`'s
@group(0) @binding(5) var probes: texture_2d<f32>;     // the light probes' coefficients (`probes.wgsl`)

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

// PBR Neutral's shoulder, without its toe (which takes up to 0.04 off every channel: on this art, mostly dark, it
// takes the blue out of the shadows)
fn neutral(c_in: vec3<f32>) -> vec3<f32> {
    let start = 0.8 - 0.04;
    let desaturation = 0.15;
    var c = c_in;
    let peak = max(c.r, max(c.g, c.b));
    if peak < start {
        return c;
    }
    let d = 1.0 - start;
    let new_peak = 1.0 - d * d / (peak + d - start);
    c = c * (new_peak / peak);
    let g = 1.0 - 1.0 / (desaturation * (peak - new_peak) + 1.0);
    return mix(c, vec3<f32>(new_peak), g);
}

// The fog along the ray from the eye to `world`: the density `a * exp(-b * (h - base))` integrated over its length,
// heights below the base taken as the base (the mirrored buildings under a stadium's water are far below it).
fn fog_amount(world: vec3<f32>) -> f32 {
    let ray = world - post.eye.xyz;
    let dist = max(length(ray) - post.fog.w, 0.0);
    let b = post.fog.y;
    let h0 = max(post.eye.y - post.fog.x, 0.0);
    let h1 = max(world.y - post.fog.x, 0.0);
    let e0 = exp(-b * h0);
    var mean = e0;
    if abs(b * (h1 - h0)) > 1e-4 {
        mean = (e0 - exp(-b * h1)) / (b * (h1 - h0));
    }
    return min(1.0 - exp(-post.fog_colour.w * dist * mean), post.fog.z);
}

fn world_at(px: vec2<i32>, d: f32) -> vec3<f32> {
    let f = vec2<f32>(px) + 0.5;
    let ndc = vec2<f32>((f.x - post.rect.x) / post.rect.z * 2.0 - 1.0, 1.0 - (f.y - post.rect.y) / post.rect.w * 2.0);
    let w = post.inv_view_proj * vec4<f32>(ndc, d, 1.0);
    return w.xyz / w.w;
}

// The ambient occlusion over the 4 x 4 pixels around `px`, each by how near its surface is to this one's (so it
// does not bleed across edges).
fn blurred_occlusion(px: vec2<i32>, centre: vec3<f32>) -> f32 {
    let lo = vec2<i32>(post.rect.xy);
    let hi = lo + vec2<i32>(post.rect.zw) - 1;
    let scale = 0.02 * distance(centre, post.eye.xyz) + 0.05;
    var sum = 0.0;
    var weight = 0.0;
    for (var y = -2; y < 2; y++) {
        for (var x = -2; x < 2; x++) {
            let q = clamp(px + vec2<i32>(x, y), lo, hi);
            let w = exp(-distance(world_at(q, textureLoad(depth, q, 0)), centre) / scale);
            sum += textureLoad(occlusion, q, 0).r * w;
            weight += w;
        }
    }
    return sum / max(weight, 1e-4);
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let px = vec2<i32>(frag.xy);
    let picture = textureLoad(hdr, px, 0);
    var c = picture.rgb;
    let d = textureLoad(depth, px, 0);
    let ndc = vec2<f32>((frag.x - post.rect.x) / post.rect.z * 2.0 - 1.0, 1.0 - (frag.y - post.rect.y) / post.rect.w * 2.0);
    let w = post.inv_view_proj * vec4<f32>(ndc, d, 1.0);
    let world = w.xyz / w.w;
    var ao = 1.0;
    if d < 1.0 && post.ao.y > 0.0 {
        ao = blurred_occlusion(px, world);
        c = c * mix(1.0, ao, textureLoad(share, px, 0).r);
    }
    if post.tone.z == 1.0 {
        return vec4<f32>(vec3<f32>(ao), 1.0);
    }
    if post.tone.z == 2.0 {
        return vec4<f32>(vec3<f32>(textureLoad(share, px, 0).r), 1.0);
    }
    let dir = normalize(world - post.eye.xyz);
    // where nothing was drawn (seen through water, say) there is no air to fog: the sky is a model
    let f = select(0.0, fog_amount(world), d < 1.0);
    let towards_sun = pow(max(dot(dir, post.sun_dir.xyz), 0.0), 8.0);
    // the sky's mean radiance: its probe's first coefficient over the constant band's integral (0.282095 * 4 pi)
    let sky = textureLoad(probes, vec2<i32>(0, i32(post.sky.x)), 0).rgb / 3.5449;
    let scattered = mix(post.fog_colour.rgb, sky, post.sky.y) + post.sun_colour.rgb * towards_sun * 0.35;
    c = mix(c, scattered, f);
    // the grade in linear light: exposure, white balance, filter, saturation and contrast (about mid grey, 0.18)
    let t = post.grade.x;
    c = c * post.tone.x * vec3<f32>(1.0 + 0.25 * t, 1.0, 1.0 - 0.25 * t) * post.tint.rgb;
    let lum = vec3<f32>(0.2126, 0.7152, 0.0722);
    c = max(mix(vec3<f32>(dot(c, lum)), c, post.grade.y), vec3<f32>(0.0));
    c = 0.18 * pow(c / 0.18, vec3<f32>(post.grade.z));
    c = neutral(c);
    // then on the display's values: lift, gamma and gain, and the vignette
    var v = pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
    v = v * (post.gain.rgb - post.lift.rgb) + post.lift.rgb;
    v = pow(max(v, vec3<f32>(0.0)), 1.0 / max(post.gamma.rgb, vec3<f32>(0.01)));
    let uv = (frag.xy - post.rect.xy) / post.rect.zw - 0.5;
    v = v * (1.0 - post.grade.w * smoothstep(0.3, 0.8, length(uv * vec2<f32>(post.rect.z / post.rect.w, 1.0)) * 0.8));
    v = clamp(v, vec3<f32>(0.0), vec3<f32>(1.0));
    // the alpha as the draws left it (the front end's masks work in it), the clear's where nothing was drawn
    if post.tone.y == 1.0 {
        return vec4<f32>(v, picture.a);
    }
    return vec4<f32>(pow(v, vec3<f32>(2.2)), picture.a);
}
