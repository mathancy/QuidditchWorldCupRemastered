// The post passes' interface (`docs/SHADERS.md`; the remaster's own, not the original's). A pack's post pass is
// added after this file and gives `fs_main(in: PostIn) -> @location(0) vec4<f32>`; the passes run in order after
// the remastered lighting's tone mapping and colour grade, each over the view's rectangle, each seeing the picture
// the one before it made. The picture is in linear light, tone mapped (mostly 0 to 1), in a float target.

struct Post {
    inv_view_proj: mat4x4<f32>,  // from the view's clip space back into the world
    eye: vec4<f32>,
    rect: vec4<f32>,             // the view's rectangle in the target: x, y, width, height in pixels
    size: vec4<f32>,             // the target's width and height in pixels, and their inverses
    time: vec4<f32>,             // x: the game's clock in seconds
    params: array<vec4<f32>, 2>, // the pass's own numbers (`shaders.ron`'s `params`; 0 past those given)
    engine: vec4<f32>,           // the engine's own
};

@group(0) @binding(0) var picture: texture_2d<f32>;
@group(0) @binding(1) var picture_sampler: sampler;  // linear, clamped to the edge
@group(0) @binding(2) var depth_map: texture_depth_2d;
@group(0) @binding(3) var<uniform> post: Post;

struct PostIn {
    @builtin(position) position: vec4<f32>,  // the pixel's centre in the target
    @location(0) uv: vec2<f32>,              // 0 to 1 across the view, from its top left
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> PostIn {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: PostIn;
    out.position = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(p.x, 1.0 - p.y);
    return out;
}

// Pass number `i` of the pass's own (0 to 7).
fn param(i: u32) -> f32 {
    return post.params[min(i, 7u) / 4u][i % 4u];
}

// The target's texture coordinate of `uv` across the view.
fn target_uv(uv: vec2<f32>) -> vec2<f32> {
    return (post.rect.xy + uv * post.rect.zw) * post.size.zw;
}

// The picture at `uv` across the view, filtered, kept inside the view (a split screen's other half is not read).
fn picture_at(uv: vec2<f32>) -> vec4<f32> {
    let half = 0.5 / post.rect.zw;
    return textureSampleLevel(picture, picture_sampler, target_uv(clamp(uv, half, 1.0 - half)), 0.0);
}

// The depth at `uv` across the view: 0 at the near plane, 1 at the far one and where nothing was drawn.
fn depth_at(uv: vec2<f32>) -> f32 {
    let px = clamp(vec2<i32>(target_uv(uv) * post.size.xy), vec2<i32>(post.rect.xy), vec2<i32>(post.rect.xy + post.rect.zw) - 1);
    return textureLoad(depth_map, px, 0);
}

// The point of the world seen at `uv` across the view.
fn world_at(uv: vec2<f32>) -> vec3<f32> {
    let w = post.inv_view_proj * vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth_at(uv), 1.0);
    return w.xyz / w.w;
}
