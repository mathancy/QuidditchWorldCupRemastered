//! `.qwcpack` files: a pack folder zipped ([`build`]) and unzipped into the packs' folder ([`install`]).

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

use crate::{Manifest, Pack, Severity, MANIFEST};

/// Files already compressed, stored as they are.
const STORED: &[&str] = &["png", "jpg", "jpeg", "mp4", "ktx2", "ogg", "zip", "qwcpack", "glb", "woff2"];

/// Zip the pack in `dir` into `out` (a file, or a folder for `<id>-<version>.qwcpack`), after [`Pack::check`]: an
/// error there stops it. Answers the file written and the warnings.
pub fn build(dir: &Path, out: &Path) -> Result<(PathBuf, Vec<String>), String> {
    let pack = Pack::open(dir)?;
    let found = pack.check();
    let errors: Vec<&String> = found.iter().filter(|(s, _)| *s == Severity::Error).map(|(_, t)| t).collect();
    if !errors.is_empty() {
        return Err(errors.iter().map(|e| e.as_str()).collect::<Vec<_>>().join("\n"));
    }
    let warnings = found.into_iter().map(|(_, t)| t).collect();
    let m = &pack.manifest;
    let file = if out.is_dir() { out.join(format!("{}-{}.qwcpack", m.id, m.version)) } else { out.to_path_buf() };
    let fail = |e: &dyn std::fmt::Display| format!("{}: {e}", file.display());
    // written beside, then moved over, so a failed build leaves no half file
    let part = file.with_extension("qwcpack.part");
    let mut zip = zip::ZipWriter::new(File::create(&part).map_err(|e| fail(&e))?);
    let mut files = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || p == part || p == file {
                continue;
            }
            if p.is_dir() {
                todo.push(p);
            } else {
                files.push(p);
            }
        }
    }
    files.sort();
    let mut buf = Vec::new();
    for p in &files {
        let rel = p.strip_prefix(dir).expect("under the pack").to_string_lossy().replace('\\', "/");
        let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let method =
            if STORED.contains(&ext.as_str()) { CompressionMethod::Stored } else { CompressionMethod::Deflated };
        let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        let opts = SimpleFileOptions::default().compression_method(method).large_file(size >= u32::MAX as u64);
        zip.start_file(rel.as_str(), opts).map_err(|e| fail(&e))?;
        buf.clear();
        File::open(p).and_then(|mut f| f.read_to_end(&mut buf)).map_err(|e| format!("{}: {e}", p.display()))?;
        zip.write_all(&buf).map_err(|e| fail(&e))?;
    }
    zip.finish().map_err(|e| fail(&e))?;
    std::fs::rename(&part, &file).map_err(|e| fail(&e))?;
    Ok((file, warnings))
}

/// Unzip the `.qwcpack` at `file` into `packs_dir/<id>`, replacing an older copy of the same pack. Every member must
/// stay inside the pack's folder, and the manifest must be at its top. Answers the pack installed.
pub fn install(file: &Path, packs_dir: &Path) -> Result<Pack, String> {
    let fail = |e: &dyn std::fmt::Display| format!("{}: {e}", file.display());
    let mut zip = zip::ZipArchive::new(File::open(file).map_err(|e| fail(&e))?).map_err(|e| fail(&e))?;
    let manifest = {
        let mut m = zip.by_name(MANIFEST).map_err(|_| fail(&format!("no {MANIFEST} at the top")))?;
        let mut text = String::new();
        m.read_to_string(&mut text).map_err(|e| fail(&e))?;
        Manifest::parse(&text).map_err(|e| fail(&e))?
    };
    std::fs::create_dir_all(packs_dir).map_err(|e| format!("{}: {e}", packs_dir.display()))?;
    let dest = packs_dir.join(&manifest.id);
    // unpacked beside, then swapped in, so a bad file leaves the installed copy as it was
    let fresh = packs_dir.join(format!(".{}.installing", manifest.id));
    let _ = std::fs::remove_dir_all(&fresh);
    let unpack = |zip: &mut zip::ZipArchive<File>| -> Result<(), String> {
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).map_err(|e| fail(&e))?;
            let Some(rel) = entry.enclosed_name() else {
                return Err(fail(&format!("{} points outside the pack", entry.name().unwrap_or_default())));
            };
            let to = fresh.join(rel);
            if entry.is_dir() {
                std::fs::create_dir_all(&to).map_err(|e| format!("{}: {e}", to.display()))?;
                continue;
            }
            if let Some(d) = to.parent() {
                std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
            }
            let mut out = File::create(&to).map_err(|e| format!("{}: {e}", to.display()))?;
            std::io::copy(&mut entry, &mut out).map_err(|e| format!("{}: {e}", to.display()))?;
        }
        Ok(())
    };
    if let Err(e) = unpack(&mut zip) {
        let _ = std::fs::remove_dir_all(&fresh);
        return Err(e);
    }
    if dest.exists() {
        std::fs::remove_dir_all(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    }
    std::fs::rename(&fresh, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    Pack::open(&dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Kind;

    #[test]
    fn builds_and_installs() {
        let root = std::env::temp_dir().join(format!("qwc-packs-archive-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let src = root.join("src");
        std::fs::create_dir_all(src.join("movies")).unwrap();
        std::fs::create_dir_all(src.join(".git")).unwrap();
        let m = Manifest {
            id: "me.test".into(),
            name: "Test".into(),
            version: "1.2.3".into(),
            authors: vec![],
            description: String::new(),
            kind: Kind::Visual,
            engine: String::new(),
            source: None,
            requires: vec![],
            conflicts: vec![],
        };
        std::fs::write(src.join(MANIFEST), m.to_text()).unwrap();
        std::fs::write(src.join("movies/wb.mp4"), [7u8; 1000]).unwrap();
        std::fs::write(src.join(".git/HEAD"), "x").unwrap();
        let out = root.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let (file, warnings) = build(&src, &out).unwrap();
        assert_eq!(file, out.join("me.test-1.2.3.qwcpack"));
        assert!(warnings.is_empty(), "{warnings:?}");

        let packs = root.join("packs");
        let p = install(&file, &packs).unwrap();
        assert_eq!((p.dir.clone(), p.manifest.clone()), (packs.join("me.test"), m.clone()));
        assert_eq!(std::fs::read(packs.join("me.test/movies/wb.mp4")).unwrap(), [7u8; 1000]);
        assert!(!packs.join("me.test/.git").exists(), "hidden files stay out");
        // again over the installed copy: replaced, nothing left beside it
        std::fs::write(packs.join("me.test/stale.txt"), "old").unwrap();
        install(&file, &packs).unwrap();
        assert!(!packs.join("me.test/stale.txt").exists());
        assert_eq!(std::fs::read_dir(&packs).unwrap().count(), 1);

        // a member climbing out of the pack is refused, and nothing is installed
        let evil = root.join("evil.qwcpack");
        let mut z = zip::ZipWriter::new(File::create(&evil).unwrap());
        z.start_file(MANIFEST, SimpleFileOptions::default()).unwrap();
        z.write_all(Manifest { id: "me.evil".into(), ..m }.to_text().as_bytes()).unwrap();
        z.start_file("../outside.txt", SimpleFileOptions::default()).unwrap();
        z.write_all(b"x").unwrap();
        z.finish().unwrap();
        assert!(install(&evil, &packs).unwrap_err().contains("outside"));
        assert!(!packs.join("me.evil").exists() && !root.join("outside.txt").exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
