//! Decode one SCHl stream or one BNKl bank sound to WAV (used by `tests/audio_parity.py`):
//! `cargo run --release -p qwc-formats --example decode_audio -- IN OUT.wav [SUBSONG]`
//! For a bank, SUBSONG counts the non-empty sounds from 1, as vgmstream's `-s` does; `all` writes every sound,
//! replacing `?s` in the output name with its number (vgmstream's `-S 0` naming).

use qwc_formats::ea_audio;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 2 {
        eprintln!("usage: decode_audio IN OUT.wav [SUBSONG]");
        std::process::exit(2);
    }
    let d = std::fs::read(&a[0]).expect("read input");
    if !d.starts_with(b"BNKl") {
        let pcm = ea_audio::decode_schl(&d).expect("decode stream");
        std::fs::write(&a[1], pcm.to_wav()).expect("write output");
        return;
    }
    let sounds: Vec<_> = ea_audio::decode_bnk(&d).expect("decode bank").into_iter().flatten().collect();
    match a.get(2).map(String::as_str) {
        Some("all") => {
            for (i, pcm) in sounds.iter().enumerate() {
                std::fs::write(a[1].replace("?s", &(i + 1).to_string()), pcm.to_wav()).expect("write output");
            }
        }
        sub => {
            let n: usize = sub.map_or(1, |s| s.parse().expect("subsong"));
            let pcm = sounds.get(n - 1).expect("no such subsong");
            std::fs::write(&a[1], pcm.to_wav()).expect("write output");
        }
    }
}
