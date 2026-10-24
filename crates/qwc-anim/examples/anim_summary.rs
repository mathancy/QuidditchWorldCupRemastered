//! One JSON line per animation with posed bone sums, for `tests/anim_parity.py`:
//! `cargo run --release -p qwc-anim --example anim_summary -- original/cd_files`

use qwc_anim::{Clip, Rig};
use qwc_assets::{anim, Vfs};
use qwc_formats::fas::{Fas, Sfa};
use serde_json::json;

fn main() {
    let root = std::env::args().nth(1).expect("usage: anim_summary DATA_DIR");
    let vfs = Vfs::open(&root).unwrap();
    for path in vfs.with_extension("fas") {
        let bytes = vfs.read(path).unwrap();
        let set = Fas::parse(&bytes).unwrap();
        let (skl, sk) = match anim::set_skeleton(&vfs, path, &set) {
            Ok(v) => v,
            Err(e) => {
                println!("{}", json!({"path": path, "error": e.to_string()}));
                continue;
            }
        };
        let stream = if anim::needs_stream(&set) { anim::set_stream(&vfs, path).unwrap() } else { None };
        let sfa = stream.as_deref().map(|b| Sfa::parse(b).unwrap());
        let rig = Rig::new(sk);
        for i in 0..set.anims.len() {
            let clip = Clip::decode(&set, i, sfa.as_ref()).unwrap();
            let frames = [0, clip.frames / 2, clip.frames];
            let sums: Vec<[f64; 2]> = frames
                .iter()
                .map(|&f| {
                    let world = rig.world_pose(&rig.local_pose(Some(&clip), f as f64));
                    let t: f64 = world.iter().map(|m| m[12] + m[13] + m[14]).sum();
                    let r: f64 = world
                        .iter()
                        .map(|m| {
                            (0..3).flat_map(|r| (0..3).map(move |c| (r, c))).map(|(r, c)| m[r * 4 + c]).sum::<f64>()
                        })
                        .sum();
                    [t, r]
                })
                .collect();
            println!("{}", json!({"path": path, "skeleton": skl, "anim": i, "frames": frames, "sums": sums}));
        }
    }
}
