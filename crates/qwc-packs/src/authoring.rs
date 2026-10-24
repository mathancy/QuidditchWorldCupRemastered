//! Changing a pack's parts while it is made (QWC Studio): the texture list's lines, the files the author adds. Every
//! other line of the list (comments, other textures, `include`s) is kept as it is.

use std::path::{Path, PathBuf};

use qwc_assets::vfs::normalize;

/// The texture list of the pack in `dir`.
pub fn texture_list(dir: &Path) -> PathBuf {
    dir.join("textures").join("textures.txt")
}

/// Where a texture's PNG goes in the pack: `textures/` and its game path, `.png` for `.fsh`.
pub fn texture_png(dir: &Path, texture: &str) -> PathBuf {
    let rel = texture.replace('\\', "/");
    let rel = rel.strip_suffix(".fsh").or_else(|| rel.strip_suffix(".FSH")).unwrap_or(&rel);
    rel.split('/').fold(dir.join("textures"), |d, p| d.join(p)).with_extension("png")
}

/// The texture list's text: `(png relative to the list, the textures it replaces)` per line that has one.
fn entries(text: &str) -> Vec<(usize, String, Vec<String>)> {
    text.lines()
        .enumerate()
        .filter_map(|(n, line)| {
            let body = line.split('#').next().unwrap_or("").trim();
            let (png, textures) = body.split_once('=')?;
            Some((n, png.trim().to_string(), textures.split_whitespace().map(normalize).collect()))
        })
        .collect()
}

/// Add `png` (a file in the pack) for `texture` to the pack's list, made when missing. An existing line for the
/// texture is replaced.
pub fn add_texture(dir: &Path, texture: &str, png: &Path) -> Result<(), String> {
    let list = texture_list(dir);
    let base = list.parent().expect("a folder");
    let rel = png.strip_prefix(base).map_err(|_| format!("{} is not under {}", png.display(), base.display()))?;
    let rel = rel.to_string_lossy().replace('\\', "/");
    remove_texture(dir, texture)?;
    let mut text = std::fs::read_to_string(&list).unwrap_or_else(|_| {
        "# The pack's textures: each PNG (relative to this file) is drawn in place of the game's textures after it.\n"
            .to_string()
    });
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text += &format!("{rel} = {texture}\n");
    std::fs::create_dir_all(base).map_err(|e| format!("{}: {e}", base.display()))?;
    std::fs::write(&list, text).map_err(|e| format!("{}: {e}", list.display()))
}

/// Take `texture` out of the pack's list; answers the PNG its line named when the line named no other texture (the
/// file is left for the caller to delete).
pub fn remove_texture(dir: &Path, texture: &str) -> Result<Option<PathBuf>, String> {
    let list = texture_list(dir);
    let Ok(text) = std::fs::read_to_string(&list) else { return Ok(None) };
    let key = normalize(texture);
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut freed = None;
    for (n, png, textures) in entries(&text) {
        if !textures.contains(&key) {
            continue;
        }
        let rest: Vec<&str> = text.lines().nth(n).unwrap_or("").split_once('=').map_or(vec![], |(_, t)| {
            t.split('#').next().unwrap_or("").split_whitespace().filter(|t| normalize(t) != key).collect()
        });
        if rest.is_empty() {
            lines[n] = String::new();
            freed = Some(list.parent().expect("a folder").join(&png));
        } else {
            lines[n] = format!("{png} = {}", rest.join(" "));
        }
    }
    let mut out: Vec<String> = Vec::new();
    for (n, l) in lines.into_iter().enumerate() {
        // the emptied lines go; blank lines the author left stay
        if !(l.is_empty() && text.lines().nth(n).is_some_and(|o| !o.trim().is_empty())) {
            out.push(l);
        }
    }
    std::fs::write(&list, out.join("\n") + "\n").map_err(|e| format!("{}: {e}", list.display()))?;
    Ok(freed)
}

/// Copy `file` into the pack at `rel` (folders made), answering where it went.
pub fn add_file(dir: &Path, rel: &str, file: &Path) -> Result<PathBuf, String> {
    let to = rel.split('/').fold(dir.to_path_buf(), |d, p| d.join(p));
    if let Some(d) = to.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    std::fs::copy(file, &to).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_and_removes_lines_keeping_the_rest() {
        let dir = std::env::temp_dir().join(format!("qwc-packs-authoring-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let list = texture_list(&dir);
        std::fs::create_dir_all(list.parent().unwrap()).unwrap();
        std::fs::write(&list, "# mine\nshared.png = a.fsh b.fsh  # two\n\nkeep.png = c.fsh\n").unwrap();
        let png = texture_png(&dir, "Frontend/Common/x.fsh");
        assert_eq!(png, dir.join("textures").join("Frontend").join("Common").join("x.png"));
        add_texture(&dir, "Frontend/Common/x.fsh", &png).unwrap();
        // a texture on a shared line comes off it; the PNG stays theirs
        assert_eq!(remove_texture(&dir, "B.FSH").unwrap(), None);
        let text = std::fs::read_to_string(&list).unwrap();
        assert_eq!(
            text,
            "# mine\nshared.png = a.fsh\n\nkeep.png = c.fsh\nFrontend/Common/x.png = Frontend/Common/x.fsh\n"
        );
        // the list reads as the game reads it
        let r = qwc_assets::remaster::Remastered::read(&list).unwrap();
        assert_eq!(r.png("frontend/common/x.fsh"), Some(png.as_path()));
        // the last texture of a line frees its PNG
        assert_eq!(remove_texture(&dir, "frontend/common/x.fsh").unwrap(), Some(png.clone()));
        assert_eq!(std::fs::read_to_string(&list).unwrap(), "# mine\nshared.png = a.fsh\n\nkeep.png = c.fsh\n");
        // adding again replaces rather than doubling
        add_texture(&dir, "c.fsh", &dir.join("textures/new.png")).unwrap();
        assert!(qwc_assets::remaster::Remastered::read(&list).is_ok());
        assert!(!std::fs::read_to_string(&list).unwrap().contains("keep.png"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
