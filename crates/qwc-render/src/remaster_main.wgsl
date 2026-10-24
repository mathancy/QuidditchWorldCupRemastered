// The stock material shader of the remastered lighting, added after `remaster.wgsl`: the material's texture, its
// alpha test, and the stock shading. A pack's material shader stands in for this file (`docs/SHADERS.md`).

@fragment
fn fs_main(in: VsOut, @builtin(front_facing) front: bool) -> FsOut {
    let texel = material_texel(in.uv);
    if alpha_cut(texel.a * in.color.a) {
        discard;
    }
    return shade(in, front, texel);
}
