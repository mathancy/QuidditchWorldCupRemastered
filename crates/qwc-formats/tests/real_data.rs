//! Parse every real file and check the totals recorded in docs/RE_NOTES.md.
//! Runs only when `QWC_DATA_DIR` points at a repo root holding `original/cd_files/` and the bootstrap's
//! `extracted/` (e.g. `QWC_DATA_DIR=$PWD cargo test --release -- --nocapture`); otherwise it is a no-op.
//! The data is read in place. Field-by-field parity with the Python tools is `tests/rust_python_parity.py`.

use std::path::{Path, PathBuf};

use qwc_formats::{
    audio, btf, ccd, fas, ffn, flow, formotion, fpm, fsh, heightmap, mpf, pop, qsp, skl, sound_map, sts, text_defs,
    tqi, vso, wpe,
};

fn walk(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk(&p, ext, out);
        } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)) {
            out.push(p);
        }
    }
}

fn files(root: &Path, sub: &str, ext: &str) -> Vec<(PathBuf, Vec<u8>)> {
    let mut v = Vec::new();
    walk(&root.join(sub), ext, &mut v);
    v.into_iter().map(|p| (p.clone(), std::fs::read(&p).unwrap())).collect()
}

#[test]
fn every_real_file_parses() {
    let Some(root) = std::env::var_os("QWC_DATA_DIR").map(PathBuf::from) else {
        eprintln!("QWC_DATA_DIR not set; skipping");
        return;
    };
    let ccds = files(&root, "original/cd_files", "ccd");
    let members: usize =
        ccds.iter().map(|(p, d)| ccd::Ccd::parse(d).unwrap_or_else(|e| panic!("{p:?}: {e}")).members.len()).sum();
    assert_eq!((ccds.len(), members), (518, 5945));

    let mut images = 0;
    for (p, d) in files(&root, "extracted", "fsh") {
        let f = fsh::Fsh::parse(&d).unwrap_or_else(|e| panic!("{p:?}: {e}"));
        for i in 0..f.images.len() {
            assert!(f.images[i].levels_fill_block, "{p:?}");
            f.decode_rgba(i, 0).unwrap();
        }
        images += 1;
    }
    assert_eq!(images, 3063);

    let (mut models, mut draws, mut slots, mut finest, mut two_sided) = (0, 0, 0, 0, 0);
    let mut modes = std::collections::BTreeMap::new();
    for (p, d) in files(&root, "extracted", "fpm") {
        let m = fpm::Fpm::parse(&d).unwrap_or_else(|e| panic!("{p:?}: {e}"));
        for (level, dr) in m.draws_with_level() {
            let vc = dr.geom.vcount as usize;
            assert!(m.indices(dr).unwrap().iter().all(|&i| (i as usize) < vc), "{p:?}");
            m.vertex_bytes(dr).unwrap();
            if let Some(mo) = m.morphs(dr).unwrap() {
                slots += mo.slots.iter().flatten().count();
            }
            draws += 1;
            finest += usize::from(level == 0);
            two_sided += usize::from(dr.two_sided());
            // the high half holds nothing else
            assert_eq!(dr.tex >> 16 & !0x20, 0, "{p:?}");
        }
        for n in &m.names {
            let s = n.surface();
            *modes.entry(s.blend).or_insert(0) += 1;
            // the blended names are alpha tested, the others not; lightmaps are clamped; all write depth
            assert_eq!(s.alpha_test, s.blends() && s.blend != fpm::blend::SRCALPHA_LIGHTMAP_KEEP, "{p:?} {}", n.name);
            assert!(s.blend != fpm::blend::SRCALPHA_LIGHTMAP_KEEP || s.wrap == [false, false], "{p:?} {}", n.name);
            assert!(s.depth_write && n.flags >> 24 == 0, "{p:?} {}", n.name);
        }
        models += 1;
    }
    assert_eq!((models, draws, slots, finest, two_sided), (329, 6551, 9415, 4474, 4737));
    use fpm::blend::*;
    assert_eq!(
        modes.into_iter().collect::<Vec<_>>(),
        [(SRCALPHA, 5), (SRCALPHA_KEEP, 834), (COPYRGB_KEEP, 1782), (SRCALPHA_LIGHTMAP_KEEP, 105)]
    );

    let skls = files(&root, "extracted", "skl");
    for (p, d) in &skls {
        skl::Skeleton::parse(d).unwrap_or_else(|e| panic!("{p:?}: {e}"));
    }
    assert_eq!(skls.len(), 44);

    let fas_files = files(&root, "extracted", "fas");
    let anims: usize =
        fas_files.iter().map(|(p, d)| fas::Fas::parse(d).unwrap_or_else(|e| panic!("{p:?}: {e}")).anims.len()).sum();
    assert_eq!((fas_files.len(), anims), (208, 1609));

    let btfs = files(&root, "extracted", "btf");
    assert!(btfs.iter().all(|(_, d)| btf::Btf::parse(d).is_ok()));
    assert_eq!(btfs.len(), 166);

    let sfas = files(&root, "original/cd_files/root/csstream", "sfa");
    for (p, d) in &sfas {
        fas::Sfa::parse(d).unwrap_or_else(|e| panic!("{p:?}: {e}"));
    }
    assert_eq!(sfas.len(), 57);

    let sdx = sts::Sdx::parse(&std::fs::read(root.join("extracted/language/INDEX.SDX")).unwrap()).unwrap();
    let stss = files(&root, "extracted/language", "sts");
    for (p, d) in &stss {
        let s = sts::Sts::parse(d).unwrap_or_else(|e| panic!("{p:?}: {e}"));
        assert_eq!(s.entries.len(), 2156, "{p:?}");
        // every SDX entry names the string at its index
        assert!(sdx.strings.iter().all(|e| e.file_hash != s.file_hash || s.entries[e.index as usize].id == e.id));
    }
    assert_eq!((stss.len(), sdx.strings.len()), (19, 2156));

    let ffns = files(&root, "extracted", "ffn");
    let (mut glyphs, mut pairs) = (0, 0);
    for (p, d) in &ffns {
        let f = ffn::Ffn::parse(d).unwrap_or_else(|e| panic!("{p:?}: {e}"));
        f.atlas().unwrap();
        (glyphs, pairs) = (glyphs + f.glyphs.len(), pairs + f.kerning.len());
    }
    assert_eq!((ffns.len(), glyphs, pairs), (43, 20110, 6691));

    let qsps = files(&root, "extracted", "qsp");
    let points: usize = qsps
        .iter()
        .flat_map(|(p, d)| qsp::Qsp::parse(d).unwrap_or_else(|e| panic!("{p:?}: {e}")).splines)
        .map(|s| s.points.len())
        .sum();
    assert_eq!((qsps.len(), points), (15, 13639));
    let (mut scripts, mut functions, mut instructions, mut resources, mut quirks) = (0, 0, 0, 0, 0);
    for (p, d) in files(&root, "extracted", "vso") {
        let v = vso::Vso::parse(&d).unwrap_or_else(|e| panic!("{p:?}: {e}"));
        for f in v.functions().unwrap() {
            instructions += v.disassemble(f.start, f.end).unwrap().len();
            functions += 1;
        }
        quirks += v.layers().unwrap_or_else(|e| panic!("{p:?}: {e}")).1.is_some() as usize;
        resources += v.resources().unwrap().len();
        scripts += 1;
    }
    assert_eq!((scripts, functions, instructions, resources, quirks), (110, 3760, 395980, 2707, 14));

    // front-end flow: every node but one has a screen, and each flow node's hash is its screen's
    let flo = std::fs::read(root.join("extracted/frontend/ScrnLst/game.flo")).unwrap();
    let f = flow::Flow::parse(&flo).unwrap();
    assert_eq!((f.groups.len(), f.nodes.len(), f.links.len()), (11, 129, 250));
    let lst = std::fs::read(root.join("extracted/frontend/ScrnLst/Screens.lst")).unwrap();
    let screens = flow::parse_screens(&lst).unwrap();
    let layers: usize = screens.iter().map(|s| s.layers.len()).sum();
    assert_eq!((screens.len(), layers), (136, 153));
    let without: Vec<_> = f.nodes.iter().filter(|n| !screens.iter().any(|s| s.hash == n.hash)).collect();
    assert_eq!(without.len(), 1);

    // text definitions: (files, blocks / entries / materials) per extension, leftover text, vocabulary problems
    let (mut leftovers, mut problems) = (0, 0);
    for (ext, want) in [
        ("pot", (42, 523)),
        ("lot", (22, 57)),
        ("trl", (16, 109)),
        ("got", (11, 86)),
        ("cli", (132, 6101)),
        ("gro", (59, 338)),
    ] {
        let e = text_defs::Ext::from_ext(ext).unwrap();
        let fs = files(&root, "extracted", ext);
        let mut items = 0;
        for (p, d) in &fs {
            let parsed = text_defs::parse(e, d).unwrap_or_else(|err| panic!("{p:?}: {err}"));
            items += match &parsed {
                text_defs::Parsed::Fx { blocks, leftover } => {
                    leftovers += !leftover.is_empty() as usize;
                    blocks.len()
                }
                text_defs::Parsed::Cli { values, .. } => values.len(),
                text_defs::Parsed::Gro { materials } => materials.len(),
            };
            problems += text_defs::vocab_problems(e, &parsed).len();
        }
        assert_eq!((fs.len(), items), want, "{ext}");
    }
    assert_eq!((leftovers, problems), (2, 1)); // the extra "}" and "S"; the ATRAIL block

    let music: usize = audio::music_index(&root).unwrap().iter().map(|(_, s)| s.len()).sum();
    assert_eq!(music, 72);
    for (lang, want) in [("English", (460, 5518)), ("French", (457, 5599)), ("Spanish", (456, 5652))] {
        let idx = audio::speech_index_at(&root, lang).unwrap();
        assert_eq!((idx.len(), idx.iter().map(|(_, s)| s.len()).sum::<usize>()), want, "{lang}");
    }
    let banks = audio::bank_index_at(&root).unwrap();
    let (mut mem, mut sounds, mut stm, mut streams) = (0, 0, 0, 0);
    for (_, b) in &banks {
        match b {
            audio::Bank::Bnkl(n) => (mem, sounds) = (mem + 1, sounds + *n as usize),
            audio::Bank::Stream(s) => (stm, streams) = (stm + 1, streams + s.len()),
        }
    }
    assert_eq!((mem, sounds, stm, streams), (52, 857, 9, 102));

    // sound maps: every sample names a sound inside its bank
    let bank_sizes: std::collections::HashMap<u32, usize> = banks
        .iter()
        .map(|(h, b)| {
            (
                *h,
                match b {
                    audio::Bank::Bnkl(n) => *n as usize,
                    audio::Bank::Stream(s) => s.len(),
                },
            )
        })
        .collect();
    let maps = files(&root, "extracted/Audio", "map");
    let mut refs = 0;
    for (p, d) in &maps {
        let m = sound_map::SoundMap::parse(d).unwrap_or_else(|e| panic!("{p:?}: {e}"));
        for s in m.events().flat_map(|e| &e.variants).flat_map(|v| &v.samples) {
            let n = bank_sizes[&sound_map::bank_file_hash(&m.banks[s.bank as usize - 1].name)];
            assert!((1..=n).contains(&(s.sound as usize)), "{p:?}");
            refs += 1;
        }
    }
    assert_eq!((maps.len(), refs), (124, 1832));

    let mpfs = files(&root, "extracted/Audio/Music", "mpf");
    let samples: usize = mpfs
        .iter()
        .map(|(p, d)| {
            mpf::MusicFile::parse(d).unwrap_or_else(|e| panic!("{p:?}: {e}")).tracks.iter().map(Vec::len).sum::<usize>()
        })
        .sum();
    assert_eq!((mpfs.len(), samples), (5, 77));

    let csvs = files(&root, "extracted/stadium", "csv");
    for (p, d) in &csvs {
        heightmap::parse(d).unwrap_or_else(|e| panic!("{p:?}: {e}"));
    }
    let locators: usize = files(&root, "extracted/stadium", "pop")
        .iter()
        .map(|(p, d)| pop::parse(d).unwrap_or_else(|e| panic!("{p:?}: {e}")).len())
        .sum();
    let frames: usize = files(&root, "extracted/frontend", "wpe")
        .iter()
        .map(|(p, d)| wpe::Wipe::parse(d).unwrap_or_else(|e| panic!("{p:?}: {e}")).frames.len())
        .sum();
    assert_eq!((csvs.len(), locators, frames), (8, 71, 76));

    let dbs: Vec<_> = files(&root, "extracted", "txt")
        .into_iter()
        .filter(|(p, _)| p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("formotionDB.txt")))
        .map(|(p, d)| formotion::parse(&d).unwrap_or_else(|e| panic!("{p:?}: {e}")))
        .collect();
    let formotions: usize = dbs.iter().map(|d| d.formotions.len()).sum();
    let waypoints: usize = dbs.iter().flat_map(|d| &d.formotions).map(|f| f.waypoints.len()).sum();
    assert_eq!((dbs.len(), formotions, waypoints), (19, 619, 1380));

    // the movies: every picture decodes, and the sound lasts as long as the pictures
    for name in ["eag.tgq", "wb.tgq", "Qintro.tgq"] {
        let path = if name == "eag.tgq" {
            "original/cd_files/movies/English/eag.tgq".into()
        } else {
            format!("original/cd_files/movies/{name}")
        };
        let d = std::fs::read(root.join(&path)).unwrap();
        let m = tqi::Movie::parse(&d[..]).unwrap_or_else(|e| panic!("{path}: {e}"));
        for k in (0..m.frames.len()).step_by(37) {
            m.frame(k).unwrap_or_else(|e| panic!("{path} frame {k}: {e}"));
        }
        let pcm = m.audio().unwrap_or_else(|e| panic!("{path}: {e}"));
        let seconds = pcm.frames() as f64 / pcm.rate as f64;
        assert!((seconds - m.seconds()).abs() < 0.05, "{path}: {seconds} s of sound, {} s of pictures", m.seconds());
    }

    // the load-set table, from the decrypted exe when there is one (the disc's is encrypted)
    let exe = root.join("analysis/QWC.exe");
    if let Ok(d) = std::fs::read(&exe) {
        let t = flow::loadset_table(&d).unwrap();
        let entries: usize = t.iter().map(|s| s.entries.len()).sum();
        assert_eq!((t.len(), entries, t[0].name.as_str()), (365, 717, "LOADSET_SYSTEM"));
        let tofe = t.iter().find(|s| s.name == "LOADSET_FRONTEND_LOADSCRN_TOFE").unwrap();
        assert_eq!(tofe.entries, vec![("FE_Loadsrcn_FE".to_string(), r"Frontend\Loadscrn\fe".to_string())]);
        // the card numbers: rows of 10, 10, 10, 10, 4 and then 6 cards (as `General_TotalQuidditchCards` counts them)
        let cards = qwc_formats::exe::card_numbers(&d).unwrap();
        let per_row: Vec<usize> = cards.iter().map(|r| r.iter().filter(|&&c| c > 0).count()).collect();
        assert_eq!(per_row, [10, 10, 10, 10, 4, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6]);
    }
    if let Ok(d) = std::fs::read(root.join("original/cd_files/QWC.exe")) {
        assert!(flow::loadset_table(&d).is_err(), "the disc's exe reads as plain code");
        eprintln!("the disc's exe's card table: {:?}", qwc_formats::exe::card_numbers(&d).map(|_| "reads"));
    }
}
