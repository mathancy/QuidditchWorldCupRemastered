//! With the game's data (`QWC_DATA_DIR` at a repo root holding `original/cd_files/` and `analysis/QWC.exe`): the
//! session's checks on the real scripts. No-ops without it.

use std::path::PathBuf;

use qwc_assets::Vfs;

fn data() -> Option<(&'static Vfs, Vec<PathBuf>)> {
    let root = PathBuf::from(std::env::var_os("QWC_DATA_DIR")?);
    let dir = root.join("original/cd_files");
    let vfs = Vfs::open(&dir).ok()?;
    let mut exes = qwc_frontend::session::exe_candidates(None, &dir);
    exes.push(root.join("analysis/QWC.exe"));
    Some((Box::leak(Box::new(vfs)), exes))
}

/// The PC card screens' textures (`General_SetQuidditchCardTexture` with the groups and names of
/// `TeamTentViewCardsTier2_LoPC`'s `g_contexts` and `g_cardTextures`) are all found, each in its group's package;
/// the session takes the screens without render-to-texture.
#[test]
fn every_card_the_pc_screens_ask_for_is_found() {
    let Some((vfs, exes)) = data() else {
        eprintln!("QWC_DATA_DIR not set; skipping");
        return;
    };
    let flow = qwc_frontend::start_flow(vfs, &exes, Some("TeamTentViewCardsTier2_LoPC")).expect("the flow starts");
    assert!(!flow.fe.render_to_texture);
    let names = ["chasers", "keeper", "beaters", "seeker", "shield", "teamphoto", "teammove", "quidcup"];
    for k in 0..4 {
        let group = format!("FE_TeamTent_VCPC{k:02}");
        for name in names {
            let path = flow.resources.texture(vfs, &group, name).unwrap_or_else(|| panic!("{group} {name}"));
            assert!(path.to_ascii_lowercase().starts_with(&format!("frontend/teamtent/vcpc{k:02}/")), "{path}");
        }
    }
}

/// The screens' objects' names: how many of the layers' objects a name is found for (an object's id is its name's
/// hash), from the scripts' words and the exe's, numbered, and two words together.
#[test]
fn most_objects_get_a_name() {
    let Some((vfs, exes)) = data() else {
        eprintln!("QWC_DATA_DIR not set; skipping");
        return;
    };
    let scripts: Vec<Vec<u8>> = vfs.with_extension("vso").map(|p| vfs.read(p).unwrap().to_vec()).collect();
    let exe = exes.iter().find_map(|p| std::fs::read(p).ok());
    let names = qwc_frontend::names::object_names(&scripts, exe.as_deref());
    fn walk(l: &[qwc_formats::vso::Layer], names: &std::collections::HashMap<u32, String>, n: &mut (usize, usize)) {
        for o in l {
            if let qwc_formats::vso::Layer::Object { id, children, .. } = o {
                n.0 += 1;
                n.1 += usize::from(names.contains_key(id));
                walk(children, names, n);
            }
        }
    }
    let mut n = (0, 0);
    for b in &scripts {
        if let Ok((l, _)) = qwc_formats::vso::Vso::parse(b).and_then(|v| v.layers()) {
            walk(&l, &names, &mut n);
        }
    }
    let (objects, named) = n;
    eprintln!("{named} of {objects} objects named, {} distinct names", names.len());
    if std::env::var_os("QWC_NAMES").is_some() {
        let mut all: Vec<&String> = names.values().collect();
        all.sort();
        for w in all {
            eprintln!("  {w}");
        }
    }
    // the title's logo, a scripts' symbol; an entry of a numbered list; two words together
    for w in ["HPLogo", "Entry12", "ActionButton"] {
        assert_eq!(names.get(&qwc_formats::flow::name_hash(w)).map(String::as_str), Some(w));
    }
    // 1,288 of the 3,906 (the scripts' symbols alone named 181)
    assert!(named >= 1200, "{named} of {objects}");
}
