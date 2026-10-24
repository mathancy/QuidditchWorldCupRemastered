//! `cargo run -p qwc-video --release --example movie_speed -- FILE.mp4 [WxH]`: decodes a remastered movie as fast as
//! it goes with each decoder, for showing within `WxH` (default 1024x768), and prints the pictures a second (the
//! movies need 24).

// OpenH264's assembly and its C both have a `dct.o`: MSVC's link warns (LNK4255) and drops that object's debug
// information, which is harmless
#![allow(linker_messages)]

use std::time::Instant;

use qwc_video::{open, Decoder};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = std::path::PathBuf::from(args.first().expect("usage: movie_speed FILE.mp4 [WxH]"));
    let within = args
        .get(1)
        .and_then(|s| s.split_once('x'))
        .map(|(w, h)| (w.parse().unwrap(), h.parse().unwrap()))
        .unwrap_or((1024, 768));
    let decoders: &[Decoder] = if cfg!(windows) { &[Decoder::Any, Decoder::OpenH264] } else { &[Decoder::OpenH264] };
    for &d in decoders {
        let t0 = Instant::now();
        let mut m = match open(&path, within, d) {
            Ok(m) => m,
            Err(e) => {
                println!("{e}");
                continue;
            }
        };
        let opened = t0.elapsed().as_secs_f64();
        let t0 = Instant::now();
        let n = std::iter::from_fn(|| m.next_picture()).count();
        let secs = t0.elapsed().as_secs_f64();
        let (w, h) = m.decoded;
        println!(
            "{}: opened in {opened:.2} s ({:.1} s of sound); {w}x{h}: {n} pictures in {secs:.2} s, {:.0} a second",
            m.decoder,
            m.seconds(),
            n as f64 / secs
        );
    }
}
