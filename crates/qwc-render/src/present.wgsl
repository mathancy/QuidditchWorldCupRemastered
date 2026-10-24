// The post passes' last step: their picture into the caller's target, in its encoding (`engine.x` 1: the target
// holds stored values, so linear light is encoded for it). Added after `post.wgsl`.

@fragment
fn fs_main(in: PostIn) -> @location(0) vec4<f32> {
    let c = textureLoad(picture, vec2<i32>(in.position.xy), 0);
    if post.engine.x > 0.5 {
        return vec4<f32>(pow(max(c.rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2)), c.a);
    }
    return c;
}
