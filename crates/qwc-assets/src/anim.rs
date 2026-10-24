//! Finding the skeleton and stream file an animation set (`.fas`) plays on, as `tools/fas_to_gltf.py` does:
//! a `.skl` with the same stem next to the set, else the skeleton named by the cutscene file suffix; it must have at
//! least as many bones as the set's masks. Cutscene sets with `Streamed3` channels read their frames from the
//! `.sfa` that the cutscene script names (`STREAMING_ANIMS_FILE`).

use qwc_formats::fas::{Fas, STREAMED_3};
use qwc_formats::skl::Skeleton;

use crate::vfs::{Bytes, Error, Result, Vfs};

/// Cutscene `.fas` suffix -> skeleton; `sni` depends on the bone count.
pub const SUFFIX_SKELETONS: [(&str, &str); 19] = [
    ("sma", "root/Anims/sma_common.skl"),
    ("sml", "root/Anims/sma_common.skl"),
    ("med", "root/Anims/med_common.skl"),
    ("cam", "root/Anims/cam_cutscenes.skl"),
    ("qfl", "root/Anims/qfl_cutscenes.skl"),
    ("blg", "root/Anims/blg_cutscenes.skl"),
    ("bl1", "root/Anims/blg_cutscenes.skl"),
    ("fli", "root/csmodels/flitwick/uni_fli_common.skl"),
    ("box", "root/csmodels/ballbox/ballbox_cutscenes.skl"),
    ("bat", "root/csmodels/beatbat/beatbat_cutscenes.skl"),
    ("cup", "root/csmodels/hogcup/hogwartscup_cutscenes.skl"),
    ("tic", "root/csmodels/ticket/ticket_cutscenes.skl"),
    ("wal", "root/csmodels/wall/wall_cutscenes.skl"),
    ("prp", "root/csmodels/worldcup/worldcup_cutscenes.skl"),
    ("firework", "root/csmodels/firework/firework_cutscenes.skl"),
    ("worldcup", "root/csmodels/worldcup/worldcup_cutscenes.skl"),
    ("micropho", "root/csmodels/micropho/micropho_cutscenes.skl"),
    ("sni12", "root/Anims/sni_cutscenes.skl"),
    ("sni11", "root/csmodels/snitch/sni_cutscenes.skl"),
];

fn mask_bones(set: &Fas) -> usize {
    set.anims.iter().flat_map(|a| a.masks.iter().flatten()).map(|m| m.nbits as usize).max().unwrap_or(0)
}

fn strip_ext(path: &str) -> &str {
    match path.rfind('.') {
        Some(i) if !path[i..].contains('/') => &path[..i],
        _ => path,
    }
}

/// The skeleton a set plays on: `(path, skeleton)`.
pub fn set_skeleton(vfs: &Vfs, set_path: &str, set: &Fas) -> Result<(String, Skeleton)> {
    let nb = mask_bones(set);
    let mut cands = vec![format!("{}.skl", strip_ext(set_path))];
    let stem = strip_ext(set_path).rsplit('/').next().unwrap_or("").to_ascii_lowercase();
    let mut suffix = stem.rsplit('_').next().unwrap_or("").to_string();
    if suffix == "sni" {
        suffix += &nb.to_string();
    }
    if let Some((_, p)) = SUFFIX_SKELETONS.iter().find(|(s, _)| *s == suffix) {
        cands.push(p.to_string());
    }
    for c in cands {
        if !vfs.exists(&c) {
            continue;
        }
        let sk = Skeleton::parse(&vfs.read(&c)?).map_err(|e| Error::Format(c.clone().into(), e))?;
        if sk.bones.len() >= nb {
            return Ok((c, sk));
        }
    }
    Err(Error::NotFound(format!("{set_path}: skeleton with {nb} bones")))
}

/// True when any clip of the set streams its frames from a `.sfa`.
pub fn needs_stream(set: &Fas) -> bool {
    set.anims.iter().flat_map(|a| &a.codecs).any(|c| c.count != 0 && c.codec == STREAMED_3)
}

/// The `.sfa` stream file of a cutscene set: `<folder>/<folder>.txt` names it with `STREAMING_ANIMS_FILE`.
pub fn set_stream(vfs: &Vfs, set_path: &str) -> Result<Option<Bytes>> {
    let Some((folder, _)) = set_path.replace('\\', "/").rsplit_once('/').map(|(a, b)| (a.to_string(), b.to_string()))
    else {
        return Ok(None);
    };
    let name = folder.rsplit('/').next().unwrap_or(&folder);
    let script = format!("{folder}/{name}.txt");
    if !vfs.exists(&script) {
        return Ok(None);
    }
    let text: String = vfs.read(&script)?.iter().map(|&b| b as char).collect();
    let key = "STREAMING_ANIMS_FILE";
    // First `STREAMING_ANIMS_FILE = "..."` in the script (the regex of the Python tool).
    let file = text.match_indices(key).find_map(|(i, _)| {
        let rest = text[i + key.len()..].trim_start().strip_prefix('=')?.trim_start().strip_prefix('"')?;
        let end = rest.find('"').filter(|&e| e > 0)?;
        Some(rest[..end].to_string())
    });
    let Some(file) = file else { return Ok(None) };
    let file = file.as_str();
    if vfs.exists(file) {
        Ok(Some(vfs.read(file)?))
    } else {
        Ok(None)
    }
}
