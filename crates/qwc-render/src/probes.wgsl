// The remastered lighting's light probes (`Look::Remastered`): each probe's six cube faces, drawn into an atlas
// (face across, probe down), projected onto the first two bands of spherical harmonics. One fragment is one
// coefficient (x, of 4) of one probe (y); the lit models' shader (`remaster.wgsl`) turns them into the light
// arriving from every side.

struct Bake {
    // each face's inverse view-projection, its eye at the origin
    inv_faces: array<mat4x4<f32>, 6>,
    // x: the face's size in texels
    size: vec4<u32>,
};

@group(0) @binding(0) var faces: texture_2d<f32>;
@group(0) @binding(1) var<uniform> bake: Bake;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let c = u32(frag.x);
    let probe = u32(frag.y);
    let size = bake.size.x;
    let texel = 2.0 / f32(size);
    var sum = vec3<f32>(0.0);
    for (var f = 0u; f < 6u; f++) {
        for (var j = 0u; j < size; j++) {
            for (var i = 0u; i < size; i++) {
                let ndc = vec2<f32>((f32(i) + 0.5) * texel - 1.0, 1.0 - (f32(j) + 0.5) * texel);
                let w = bake.inv_faces[f] * vec4<f32>(ndc, 1.0, 1.0);
                let d = normalize(w.xyz / w.w);
                // the texel's solid angle on a face a unit away
                let solid = texel * texel / pow(1.0 + dot(ndc, ndc), 1.5);
                var y = 0.282095;
                if c == 1u { y = 0.488603 * d.y; }
                if c == 2u { y = 0.488603 * d.z; }
                if c == 3u { y = 0.488603 * d.x; }
                let light = textureLoad(faces, vec2<u32>(f * size + i, probe * size + j), 0).rgb;
                sum += light * y * solid;
            }
        }
    }
    return vec4<f32>(sum, 1.0);
}
