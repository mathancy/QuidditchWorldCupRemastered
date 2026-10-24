//! The library's pose against the exporters' sampling (`Rig::local_pose`, checked against the game in Blender):
//! one clip playing alone sets every bone it has a channel for to the same local matrix. Runs only when
//! `QWC_DATA_DIR` points at a repo root holding `original/cd_files/`.

use std::path::PathBuf;
use std::sync::Arc;

use qwc_anim::play::{flag, ClipData, ClipInfo, State};
use qwc_anim::{Clip, Rig};
use qwc_assets::Vfs;
use qwc_formats::fas::Fas;

#[test]
fn a_lone_clip_poses_as_the_exporters_sample() {
    let Some(root) = std::env::var_os("QWC_DATA_DIR").map(PathBuf::from) else {
        eprintln!("QWC_DATA_DIR not set; skipping");
        return;
    };
    let vfs = Vfs::open(root.join("original/cd_files")).unwrap();
    let (mut checked, mut worst) = (0, 0f64);
    for f in vfs.with_extension("fas").filter(|f| f.to_lowercase().starts_with("frontend/")).take(40) {
        let b = vfs.read(f).unwrap();
        let fas = Fas::parse(&b).unwrap();
        let Ok((_, skeleton)) = qwc_assets::anim::set_skeleton(&vfs, f, &fas) else { continue };
        let rig = Rig::new(skeleton);
        for i in 0..fas.anims.len().min(3) {
            let clip = Clip::decode(&fas, i, None).unwrap();
            let a = &fas.anims[i];
            let info = ClipInfo { first: a.unk0 as i32, last: a.frames as i32, fps: a.fps };
            let mut s = State::default();
            let l = s.add_layer();
            s.layers[l].mode = 2;
            let id = s.layers[l].channels[0].id;
            let data = Arc::new(ClipData { anim: Some(clip.clone()), triggers: vec![] });
            s.play(id, flag::LOOP, 1.0, info, 0, None, Some(data), None, None);
            let (mut ended, mut fired) = (Vec::new(), Vec::new());
            s.step(0.37, None, &mut ended, &mut fired);
            let mut pose = rig.bind_pose();
            s.evaluate(&mut pose);
            let frame = s.layers[l].channels[0].frame as f64;
            let ours = rig.pose_local(&pose);
            let theirs = rig.local_pose(Some(&clip), frame);
            let bones: Vec<usize> = clip.rotation.iter().chain(&clip.translation).map(|c| c.bone as usize).collect();
            for &bn in bones.iter().filter(|&&bn| bn < rig.bone_count()) {
                for k in 0..16 {
                    worst = worst.max((ours[bn][k] - theirs[bn][k]).abs());
                }
            }
            checked += 1;
        }
    }
    eprintln!("{checked} clips checked; largest difference {worst:e}");
    assert!(checked > 20);
    // single precision against double: within a few ulps of the values
    assert!(worst < 1e-3, "{worst}");
}

/// Every cutscene clip with triggers, played alone at 30 frames a second: each time its lists give within the clip
/// fires once, in time order, on time.
#[test]
fn cutscene_triggers_fire_on_their_times() {
    let Some(root) = std::env::var_os("QWC_DATA_DIR").map(PathBuf::from) else {
        eprintln!("QWC_DATA_DIR not set; skipping");
        return;
    };
    let vfs = Vfs::open(root.join("original/cd_files")).unwrap();
    let (mut clips, mut fired_total, mut outside) = (0, 0, 0);
    for f in vfs.with_extension("btf") {
        let btf = qwc_formats::btf::Btf::parse(&vfs.read(f).unwrap()).unwrap();
        let fas_path = format!("{}.fas", &f[..f.len() - 4]);
        let Ok(b) = vfs.read(&fas_path) else { continue };
        let fas = Fas::parse(&b).unwrap();
        for (name, triggers) in &btf.anims {
            let Some(a) = fas
                .anims
                .iter()
                .find(|a| a.name.as_deref().zip(name.as_deref()).is_some_and(|(x, y)| x.eq_ignore_ascii_case(y)))
            else {
                continue;
            };
            let info = ClipInfo { first: a.unk0 as i32, last: a.frames as i32, fps: a.fps };
            let lists = qwc_anim::trigger::TriggerList::from_btf(triggers, 0.0, |_| None);
            let length = info.seconds();
            let mut want: Vec<(f32, Option<String>)> = lists
                .iter()
                .flat_map(|l| l.times.iter().map(move |&t| (t, l.name.clone())))
                .filter(|&(t, _)| t <= length)
                .collect();
            outside += lists.iter().map(|l| l.times.len()).sum::<usize>() - want.len();
            want.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            let mut s = State::default();
            let l = s.add_layer();
            s.layers[l].mode = 2;
            let id = s.layers[l].channels[0].id;
            let data = Arc::new(ClipData { anim: None, triggers: lists });
            s.play(id, 0, 1.0, info, 0, None, Some(data), None, None);
            let (mut ended, mut fired) = (Vec::new(), Vec::new());
            for _ in 0..(length * 30.0) as usize + 2 {
                s.step(1.0 / 30.0, None, &mut ended, &mut fired);
            }
            assert_eq!(fired.len(), want.len(), "{f} {name:?}");
            for (got, (t, n)) in fired.iter().zip(&want) {
                assert!(got.late.abs() < 1e-4, "{f} {name:?} {n:?} at {t}: late {}", got.late);
            }
            // same-time entries fire in set order, so compare the names as multisets per time
            let mut a: Vec<_> = fired.iter().map(|x| x.name.clone()).collect();
            let mut b: Vec<_> = want.iter().map(|x| x.1.clone()).collect();
            a.sort();
            b.sort();
            assert_eq!(a, b, "{f} {name:?}");
            clips += 1;
            fired_total += fired.len();
        }
    }
    eprintln!("{clips} clips, {fired_total} triggers fired, {outside} times past their clip's end");
    assert!(clips > 100);
}
