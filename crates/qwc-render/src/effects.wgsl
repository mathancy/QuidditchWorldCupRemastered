// The effects' pass: world-space quads (the particle, trail and geometry effects), a texture times the vertex
// colour (1 the texture as it is), depth-tested against the scene.

struct Frame {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) colour: vec4<f32>,
};

@vertex
fn vs_main(@location(0) pos: vec3<f32>, @location(1) uv: vec2<f32>, @location(2) colour: vec4<f32>) -> VOut {
    var o: VOut;
    o.pos = frame.view_proj * vec4<f32>(pos, 1.0);
    o.uv = uv;
    o.colour = colour;
    return o;
}

@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

@fragment
fn fs_main(i: VOut) -> @location(0) vec4<f32> {
    let t = textureSample(tex, samp, i.uv);
    let c = vec4<f32>(min(t.rgb * i.colour.rgb, vec3<f32>(1.0)), t.a * i.colour.a);
    // the materials' alpha test (`alphafunc GT0`)
    if (c.a <= 0.0) {
        discard;
    }
    return c;
}
