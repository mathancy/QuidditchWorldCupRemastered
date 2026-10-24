//! Shaders from resource packs (`docs/SHADERS.md`; the remaster's own, not the original's): material shaders, a
//! fragment stage standing in for the remastered lighting's on the materials they name, and post passes run after
//! its tone mapping. Each is WGSL added after the engine's interface (`remaster.wgsl`, `post.wgsl`), checked here
//! with `naga` before the GPU sees it; one that fails is reported and the stock shader draws instead.
//!
//! Only the remastered look ([`crate::Look::Remastered`]) draws them: the original look stays the original's.

/// The material shaders' interface: what a pack's material shader is added after.
pub const MATERIAL_INTERFACE: &str = include_str!("remaster.wgsl");
/// The stock material shader (after [`MATERIAL_INTERFACE`]).
pub(crate) const MATERIAL_STOCK: &str = include_str!("remaster_main.wgsl");
/// The post passes' interface.
pub const POST_INTERFACE: &str = include_str!("post.wgsl");
/// The post passes' last step, into the caller's target (after [`POST_INTERFACE`]).
pub(crate) const POST_PRESENT: &str = include_str!("present.wgsl");

/// How many numbers of its own a post pass takes (`post.wgsl`'s `params`).
pub const PARAMS: usize = 8;

/// A new material shader: the stock one, written out to change.
pub const MATERIAL_TEMPLATE: &str = "// A material shader (docs/SHADERS.md): the stock shading, to change.
@fragment
fn fs_main(in: VsOut, @builtin(front_facing) front: bool) -> FsOut {
    let texel = material_texel(in.uv);
    if alpha_cut(texel.a * in.color.a) {
        discard;
    }
    var o = shade(in, front, texel);
    return o;
}
";

/// A new post pass: the picture as it is.
pub const POST_TEMPLATE: &str =
    "// A post pass (docs/SHADERS.md): the picture as it is. param(0u) to param(7u) are shaders.ron's params.
@fragment
fn fs_main(in: PostIn) -> @location(0) vec4<f32> {
    let c = picture_at(in.uv);
    return c;
}
";

/// A material shader: its WGSL and the materials it draws, by their texture's path or their name (`*` any run of
/// characters, `?` any one; case aside).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MaterialShader {
    /// Where it came from, for the log (`author.pack:materials/water.wgsl`).
    pub name: String,
    pub source: String,
    pub textures: Vec<String>,
    pub materials: Vec<String>,
}

/// A post pass: its WGSL and its numbers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PostPass {
    pub name: String,
    pub source: String,
    pub params: [f32; PARAMS],
}

/// The packs' shaders: the material shaders, the first that names a material winning, and the post passes in the
/// order they run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shaders {
    pub materials: Vec<MaterialShader>,
    pub post: Vec<PostPass>,
}

impl Shaders {
    pub fn is_empty(&self) -> bool {
        self.materials.is_empty() && self.post.is_empty()
    }

    /// The material shader drawing a material named `name` with the texture at `texture`: its index in
    /// [`Shaders::materials`] plus one, 0 for the stock shader.
    pub fn material_for(&self, name: &str, texture: Option<&str>) -> u16 {
        let texture = texture.map(|t| t.replace('\\', "/"));
        self.materials
            .iter()
            .position(|m| {
                m.materials.iter().any(|p| glob(p, name))
                    || texture.as_deref().is_some_and(|t| m.textures.iter().any(|p| glob(p, t)))
            })
            .map_or(0, |i| i as u16 + 1)
    }
}

/// Whether `text` matches `pattern` (`*` any run of characters, `?` any one), case aside.
pub fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    // the classic two-pointer match, going back to the last `*` on a mismatch
    let (mut i, mut j, mut star, mut mark) = (0, 0, None, 0);
    while j < t.len() {
        if i < p.len() && (p[i] == '?' || p[i] == t[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '*' {
            star = Some(i);
            mark = j;
            i += 1;
        } else if let Some(s) = star {
            i = s + 1;
            mark += 1;
            j = mark;
        } else {
            return false;
        }
    }
    p[i..].iter().all(|&c| c == '*')
}

/// Check a material shader: with the interface before it, it parses, validates, and has a fragment `fs_main`.
pub fn check_material(source: &str) -> Result<(), String> {
    check(MATERIAL_INTERFACE, source)
}

/// Check a post pass as [`check_material`] does.
pub fn check_post(source: &str) -> Result<(), String> {
    check(POST_INTERFACE, source)
}

/// `interface` and `source` as one module; an error's line is the pack file's (or says it is in the interface).
fn check(interface: &str, source: &str) -> Result<(), String> {
    let whole = format!("{interface}\n{source}");
    let offset = interface.lines().count() + 1;
    let at = |loc: Option<naga::SourceLocation>| match loc {
        Some(l) if l.line_number as usize > offset => {
            format!("line {}:{}: ", l.line_number as usize - offset, l.line_position)
        }
        Some(l) => format!("(the engine's interface, line {}) ", l.line_number),
        None => String::new(),
    };
    let module =
        naga::front::wgsl::parse_str(&whole).map_err(|e| format!("{}{}", at(e.location(&whole)), e.message()))?;
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::default())
        .validate(&module)
        .map_err(|e| {
            let what = e.spans().next().map(|(_, label)| format!(" ({label})")).unwrap_or_default();
            format!("{}{}{what}", at(e.location(&whole)), e.as_inner())
        })?;
    let fragment = module.entry_points.iter().any(|ep| ep.name == "fs_main" && ep.stage == naga::ShaderStage::Fragment);
    if !fragment {
        return Err("no `@fragment fn fs_main`".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob("*water*", "stadium/hog_sum/Water_01.png"));
        assert!(glob("flag?", "FLAG2"));
        assert!(!glob("flag?", "flag"));
        assert!(glob("*", ""));
        assert!(glob("a*b*c", "aXXbYYc"));
        assert!(!glob("a*b*c", "aXXbYY"));
    }

    #[test]
    fn the_stock_shaders_pass_their_own_checks() {
        check_material(MATERIAL_STOCK).unwrap();
        check_post(POST_PRESENT).unwrap();
        check_material(MATERIAL_TEMPLATE).unwrap();
        check_post(POST_TEMPLATE).unwrap();
    }

    #[test]
    fn a_pack_shader_is_checked_with_its_own_lines() {
        let tinted = "@fragment\nfn fs_main(in: VsOut, @builtin(front_facing) front: bool) -> FsOut {\n    var o = shade(in, front, material_texel(in.uv));\n    o.colour = vec4<f32>(o.colour.rgb * vec3<f32>(1.0, 0.9, 0.8), o.colour.a);\n    return o;\n}\n";
        check_material(tinted).unwrap();
        let wrong = "@fragment\nfn fs_main(in: VsOut) -> FsOut {\n    return nope(in);\n}\n";
        let e = check_material(wrong).unwrap_err();
        assert!(e.starts_with("line 3:"), "{e}");
        assert_eq!(check_post("fn helper() {}").unwrap_err(), "no `@fragment fn fs_main`");
        let grey = "@fragment\nfn fs_main(in: PostIn) -> @location(0) vec4<f32> {\n    let c = picture_at(in.uv);\n    let g = dot(c.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));\n    return vec4<f32>(mix(c.rgb, vec3<f32>(g), param(0u)), c.a);\n}\n";
        check_post(grey).unwrap();
    }

    #[test]
    fn the_first_shader_naming_a_material_wins() {
        let m = |name: &str, textures: &[&str], materials: &[&str]| MaterialShader {
            name: name.into(),
            source: String::new(),
            textures: textures.iter().map(|s| s.to_string()).collect(),
            materials: materials.iter().map(|s| s.to_string()).collect(),
        };
        let s = Shaders { materials: vec![m("a", &["*water*"], &[]), m("b", &["*"], &["glass"])], post: vec![] };
        assert_eq!(s.material_for("x", Some("stadium\\lake_water.png")), 1);
        assert_eq!(s.material_for("glass", None), 2);
        assert_eq!(s.material_for("x", Some("grass.png")), 2);
        assert_eq!(s.material_for("x", None), 0);
    }
}
