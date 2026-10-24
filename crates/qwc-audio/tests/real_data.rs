//! Resolve and decode the disc's sound effects, speech and music through the library.
//! Runs only when `QWC_DATA_DIR` points at a repo root holding `original/cd_files/` and the bootstrap's
//! `extracted/` (e.g. `QWC_DATA_DIR=$PWD cargo test --release`); otherwise it is a no-op.

use std::path::PathBuf;
use std::sync::Mutex;

use qwc_assets::Vfs;
use qwc_audio::library::LANGUAGES;
use qwc_audio::music::SCORES;
use qwc_audio::{Library, Mixer, MusicPlayer, Score};

fn data() -> Option<(PathBuf, Vfs)> {
    let Some(root) = std::env::var_os("QWC_DATA_DIR").map(PathBuf::from) else {
        eprintln!("QWC_DATA_DIR not set; skipping");
        return None;
    };
    let vfs = Vfs::open(root.join("original/cd_files")).unwrap();
    Some((root, vfs))
}

#[test]
fn every_effect_event_resolves() {
    let Some((_, vfs)) = data() else { return };
    let lib = Library::open(&vfs).unwrap();
    let maps: Vec<String> = vfs.with_extension("map").map(str::to_string).collect();
    assert_eq!(maps.len(), 124);
    let (mut events, mut samples, mut silent) = (0, 0, 0);
    for path in &maps {
        let map = lib.category(&vfs, path).unwrap();
        for e in map.events() {
            events += 1;
            let mut rolls: Vec<f32> = e.variants.iter().flat_map(|v| &v.samples).map(|s| s.weight as f32).collect();
            rolls.push(0.0);
            for w in rolls {
                if lib.event(&vfs, path, e.event_id, (w - 1.0).max(0.0) / 65535.0).unwrap().is_none() {
                    silent += 1;
                }
            }
            for s in e.variants.iter().flat_map(|v| &v.samples) {
                let bank = &map.banks[s.bank as usize - 1].name;
                let sound = lib.bank_sound(&vfs, bank, s.sound).unwrap().expect("a sample names an empty slot");
                assert!(sound.frames() > 0 && sound.rate > 0, "{path}: {bank} sound {}", s.sound);
                samples += 1;
            }
        }
    }
    eprintln!("{events} events, {samples} samples, {silent} picks with no sound");
    assert_eq!(samples, 1832);
}

#[test]
fn speech_files_decode() {
    let Some((root, vfs)) = data() else { return };
    let lib = Library::open(&vfs).unwrap();
    for lang in LANGUAGES {
        let index = qwc_formats::audio::speech_index_at(&root, lang).unwrap();
        let mut phrases = 0;
        for (name, streams) in index.iter().filter(|(n, _)| !n.starts_with("unnamed_")) {
            let sounds = lib.speech(&vfs, lang, name).unwrap();
            assert_eq!(sounds.len(), streams.len(), "{lang} {name}");
            for (s, st) in sounds.iter().zip(streams) {
                assert_eq!((s.rate as u64, s.channels as u64), (st.rate, st.channels), "{lang} {name}");
            }
            phrases += sounds.len();
        }
        eprintln!("{lang}: {} files, {phrases} phrases", index.len());
        assert!(phrases > 1000);
    }
}

#[test]
fn scores_play_their_sections() {
    let Some((_, vfs)) = data() else { return };
    for (dir, _, _) in SCORES {
        let mut score = Score::load(&vfs, dir).unwrap();
        let entries: Vec<usize> = (0..score.graph.events.len()).filter_map(|e| score.event_node(e)).collect();
        assert!(!entries.is_empty(), "{dir}: no event starts a section");
        for &node in &entries {
            let (sample, _) = score.next(node).unwrap_or_else(|| panic!("{dir}: node {node} reaches no segment"));
            let ms = score.graph.tracks.iter().flatten().nth(sample as usize - 1).unwrap().duration_ms;
            let seg = score.segment(sample).unwrap();
            let secs = seg.frames() as f64 / seg.rate as f64;
            assert!((secs * 1000.0 - ms as f64).abs() < 50.0, "{dir} segment {sample}: {secs} s vs {ms} ms");
        }
        eprintln!("{dir}: {} sections", entries.len());
    }

    // the front-end score keeps playing past the end of its first segment
    let mut player = MusicPlayer::new(Score::load(&vfs, "FE").unwrap());
    let mixer = Mutex::new(Mixer::new(44100));
    player.play_event(&mixer, 1).unwrap();
    assert_eq!(mixer.lock().unwrap().queued(player.voice().unwrap()), Some(1));
    let mut buf = vec![0.0; 44100 * 2];
    let mut peak = 0.0f32;
    for _ in 0..90 {
        mixer.lock().unwrap().render(&mut buf);
        player.update(&mixer).unwrap();
        peak = buf.iter().fold(peak, |p, s| p.max(s.abs()));
    }
    assert!(player.is_playing());
    assert!(peak > 0.1);
}

/// The events' cues: the drivers' numbers name sections in every match score, and a change that waits keeps the
/// playing segment.
#[test]
fn score_events_cue_sections() {
    let Some((_, vfs)) = data() else { return };
    use qwc_audio::Cue;
    for dir in ["HW", "Intl", "QD"] {
        let score = Score::load(&vfs, dir).unwrap();
        assert_eq!(score.graph.events.len(), 14, "{dir}");
        assert!(matches!(score.cue(0), Cue::Section { node: 0, now: true }), "{dir}: {:?}", score.cue(0));
        assert_eq!(score.cue(10), Cue::Stop, "{dir}");
    }
    let fe = Score::load(&vfs, "FE").unwrap();
    assert!(matches!(fe.cue(0), Cue::Section { node: 6, now: true }));
    assert_eq!(fe.cue(5), Cue::Stop);
    let mut player = MusicPlayer::new(Score::load(&vfs, "Intl").unwrap());
    let mixer = Mutex::new(Mixer::new(44100));
    player.post(&mixer, 0).unwrap();
    let voice = player.voice().unwrap();
    // event 6 waits for the segment: the same voice, the new section's segment queued
    player.post(&mixer, 6).unwrap();
    assert_eq!(player.voice(), Some(voice));
    assert_eq!(mixer.lock().unwrap().queued(voice), Some(1));
    player.post(&mixer, 10).unwrap();
    assert!(!player.is_playing());
}
