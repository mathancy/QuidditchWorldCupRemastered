//! `qwc-game movie FILE.tgq --out OUT [--time SECS]`: a movie's picture at `--time` (`.png`), every picture as
//! raw planar YUV 4:2:0 (`.yuv`, to compare with `ffmpeg -fps_mode passthrough -pix_fmt yuv420p`), or its sound
//! (`.wav`).

use qwc_assets::Vfs;
use qwc_formats::tqi::{Movie, FPS};

use crate::{fail, Args};

pub fn movie(args: &Args, vfs: &Vfs) {
    let name = args.models.first().unwrap_or_else(|| fail("movie needs a file"));
    let out = args.out.clone().unwrap_or_else(|| fail("movie needs --out FILE.png|.yuv|.wav"));
    let bytes = vfs.read(name).unwrap_or_else(|e| fail(format!("{name}: {e}")));
    let m = Movie::parse(&bytes[..]).unwrap_or_else(|e| fail(format!("{name}: {e}")));
    eprintln!("{name}: {}x{}, {} frames ({:.2} s)", m.width, m.height, m.frames.len(), m.seconds());
    let ext = out.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "yuv" => {
            let mut raw = Vec::new();
            let t = std::time::Instant::now();
            for k in 0..m.frames.len() {
                let f = m.frame(k).unwrap_or_else(|e| fail(format!("frame {k}: {e}")));
                raw.extend_from_slice(&f.y);
                raw.extend_from_slice(&f.cb);
                raw.extend_from_slice(&f.cr);
            }
            eprintln!("decoded in {:.2} s", t.elapsed().as_secs_f64());
            std::fs::write(&out, raw).unwrap_or_else(|e| fail(e));
        }
        "wav" => {
            let pcm = m.audio().unwrap_or_else(|e| fail(e));
            eprintln!("{} Hz, {} channels, {:.2} s", pcm.rate, pcm.channels, pcm.frames() as f64 / pcm.rate as f64);
            std::fs::write(&out, pcm.to_wav()).unwrap_or_else(|e| fail(e));
        }
        _ => {
            let k = ((args.time * FPS as f64) as usize).min(m.frames.len() - 1);
            let f = m.frame(k).unwrap_or_else(|e| fail(format!("frame {k}: {e}")));
            crate::fe::write_png(&out, (f.width as u32, f.height as u32), f.to_rgba());
        }
    }
    eprintln!("wrote {}", out.display());
}
