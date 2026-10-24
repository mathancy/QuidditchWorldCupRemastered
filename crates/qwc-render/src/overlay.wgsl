// The front end's 2D pass: quads in the front end's 512 x 512 screen, a texture times the vertex colour.
// The colour's 0x80 is one (the PS2's convention): the texture stage doubles it (MODULATE2X, inferred).

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) colour: vec4<f32>,
};

// the target's left, top, right and bottom in the screen's units (0, 0, 512, 512 stretches it over the target)
@group(1) @binding(0) var<uniform> span: vec4<f32>;

@vertex
fn vs_main(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) colour: vec4<f32>) -> VOut {
    var o: VOut;
    let k = (pos - span.xy) / (span.zw - span.xy);
    o.pos = vec4<f32>(k.x * 2.0 - 1.0, 1.0 - k.y * 2.0, 0.0, 1.0);
    o.uv = uv;
    o.colour = colour;
    return o;
}

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

@fragment
fn fs_main(i: VOut) -> @location(0) vec4<f32> {
    let t = textureSample(tex, samp, i.uv);
    return vec4<f32>(min(t.rgb * i.colour.rgb * 2.0, vec3<f32>(1.0)), t.a * i.colour.a);
}
