//! The remastered movies: `remastered/movies/NAME.mp4` plays in place of the disc's `NAME.tgq` (`eag.mp4` for every
//! language's `Movies/<LANGUAGE>/eag.tgq`). They are not the original's: H.264 pictures (2880x1620, 24 a second,
//! BT.709 limited range; Constrained Baseline, so no reordered pictures) and AAC sound. The original stretches its
//! 512x512 TQI picture over the screen (`0x4d1e80`); these keep their shape (`qwc-game`'s `Painter::movie`).
//!
//! Two decoders:
//! - Windows' Media Foundation ([`mf`], Windows only, tried first there): the source reader decodes (on the GPU
//!   where it can), converts to 32-bit RGB and scales to the size the picture is shown at;
//! - OpenH264 ([`openh264`], everywhere) with Symphonia's MP4 reader and AAC decoder: pictures converted here
//!   ([`yuv`]), halved (box averaged) while they stay at least the size shown, the GPU scaling the rest.
//!
//! The pictures are decoded on a thread of their own, a few ahead of the clock; the sound is decoded whole when the
//! movie opens, for the mixer.

// OpenH264's assembly and its C both have a `dct.o`: MSVC's link warns (LNK4255) and drops that object's debug
// information, which is harmless
#![cfg_attr(test, allow(linker_messages))]

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError};

use qwc_formats::ea_audio::Pcm;

#[cfg(windows)]
mod mf;
mod openh264;
pub mod yuv;

/// The remastered file standing in for the movie the script names (`Movies/English/eag.tgq`), if `dir` has one: the
/// file whose stem is the movie's, any case.
pub fn find(dir: &Path, name: &str) -> Option<PathBuf> {
    let file = name.rsplit(['/', '\\']).next()?;
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| {
        p.extension().is_some_and(|x| x.eq_ignore_ascii_case("mp4"))
            && p.file_stem().is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(stem))
    })
}

/// A decoded picture, B G R X, rows top down with no padding: shown from `at` seconds.
pub struct Picture {
    pub at: f64,
    pub bgra: Vec<u8>,
}

/// Which decoder opens a movie.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decoder {
    /// The system's (Media Foundation on Windows), else OpenH264.
    Any,
    OpenH264,
}

/// A remastered movie being decoded.
pub struct HdMovie {
    /// The pictures' size as stored, and as decoded.
    pub size: (u32, u32),
    pub decoded: (u32, u32),
    /// The sound, whole (48 kHz stereo for the three).
    pub sound: Option<Pcm>,
    /// Which decoder it is (for the log).
    pub decoder: &'static str,
    pictures: Receiver<Result<Picture, String>>,
    /// The next picture, not due yet.
    queued: Option<Picture>,
    /// The decoder has sent its last picture.
    done: bool,
    /// When the last picture shown went up.
    last_at: f64,
}

/// Pictures decoded ahead of the clock (each up to 2880 x 1620 x 4 bytes).
const AHEAD: usize = 4;

/// `size` scaled to fit `within` keeping its shape, never enlarged, even sides.
pub fn fitted(size: (u32, u32), within: (u32, u32)) -> (u32, u32) {
    let k = (within.0 as f64 / size.0 as f64).min(within.1 as f64 / size.1 as f64).min(1.0);
    let even = |v: u32| ((v as f64 * k / 2.0).round() as u32 * 2).max(2);
    (even(size.0), even(size.1))
}

/// Open `path`, its pictures decoded for showing within `within` (pixels).
pub fn open(path: &Path, within: (u32, u32), decoder: Decoder) -> Result<HdMovie, String> {
    // what the system's decoder said, when it was tried
    #[cfg(windows)]
    let failed = match decoder {
        Decoder::Any => match mf::open(path, within) {
            Ok(m) => return Ok(m),
            Err(e) => format!("Media Foundation: {e}; "),
        },
        Decoder::OpenH264 => String::new(),
    };
    #[cfg(not(windows))]
    let failed = {
        let _ = decoder;
        String::new()
    };
    openh264::open(path, within).map_err(|e| format!("{}: {failed}OpenH264: {e}", path.display()))
}

impl HdMovie {
    /// Start `decode` on the pictures' thread: it sends each picture in turn and stops when a send fails (the
    /// movie was dropped).
    fn start(
        size: (u32, u32),
        decoded: (u32, u32),
        sound: Option<Pcm>,
        decoder: &'static str,
        decode: impl FnOnce(SyncSender<Result<Picture, String>>) + Send + 'static,
    ) -> Result<HdMovie, String> {
        let (tx, rx) = std::sync::mpsc::sync_channel(AHEAD);
        std::thread::Builder::new().name("movie".into()).spawn(move || decode(tx)).map_err(|e| e.to_string())?;
        Ok(HdMovie { size, decoded, sound, decoder, pictures: rx, queued: None, done: false, last_at: 0.0 })
    }

    /// The picture due at `t` seconds into the movie when it is a new one (pictures already passed are dropped).
    pub fn picture(&mut self, t: f64) -> Option<Picture> {
        let mut due = None;
        loop {
            if self.queued.is_none() && !self.done {
                match self.pictures.try_recv() {
                    Ok(Ok(p)) => self.queued = Some(p),
                    Ok(Err(e)) => {
                        eprintln!("movie: {e}");
                        self.done = true;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => self.done = true,
                }
            }
            match self.queued.take() {
                Some(p) if p.at <= t => {
                    self.last_at = p.at;
                    due = Some(p);
                }
                p => {
                    self.queued = p;
                    break;
                }
            }
        }
        due
    }

    /// The next picture whenever it is due, waiting for the decoder: `None` after the last (for tools and tests,
    /// which take every picture).
    pub fn next_picture(&mut self) -> Option<Picture> {
        if let Some(p) = self.queued.take() {
            return Some(p);
        }
        if self.done {
            return None;
        }
        match self.pictures.recv() {
            Ok(Ok(p)) => Some(p),
            Ok(Err(e)) => {
                eprintln!("movie: {e}");
                self.done = true;
                None
            }
            Err(_) => {
                self.done = true;
                None
            }
        }
    }

    /// Whether the movie has ended at `t` seconds: the last picture has been up for a frame (24 a second).
    pub fn ended(&self, t: f64) -> bool {
        self.done && self.queued.is_none() && t >= self.last_at + 1.0 / 24.0
    }

    /// How long the movie runs, by its sound.
    pub fn seconds(&self) -> f64 {
        self.sound.as_ref().map_or(0.0, |s| s.frames() as f64 / s.rate as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits() {
        assert_eq!(fitted((2880, 1620), (1024, 768)), (1024, 576));
        assert_eq!(fitted((2880, 1620), (800, 800)), (800, 450));
        assert_eq!(fitted((2880, 1620), (3840, 2160)), (2880, 1620));
    }

    #[test]
    fn finds_by_stem() {
        let dir = std::env::temp_dir().join(format!("qwc-video-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["eag.mp4", "Qintro.mp4", "wb.txt"] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        assert_eq!(find(&dir, r"Movies\English\eag.tgq"), Some(dir.join("eag.mp4")));
        assert_eq!(find(&dir, "Movies/French/eag.tgq"), Some(dir.join("eag.mp4")));
        assert_eq!(find(&dir, "movies/qintro.tgq"), Some(dir.join("Qintro.mp4")));
        assert_eq!(find(&dir, "movies/wb.tgq"), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `remastered/movies/wb.mp4`, when it is checked out (not an LFS pointer).
    fn wb() -> Option<PathBuf> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../remastered/movies");
        let path = find(&dir, "movies/wb.tgq").filter(|p| std::fs::metadata(p).is_ok_and(|m| m.len() > 1024));
        if path.is_none() {
            eprintln!("no checked-out wb.mp4 in {}: skipped", dir.display());
        }
        path
    }

    /// Every picture in turn, checking they come in time order and that the clock shows them.
    fn all_pictures(m: &mut HdMovie) -> Vec<Picture> {
        let mut all: Vec<Picture> = std::iter::from_fn(|| m.next_picture()).collect();
        assert!(all.windows(2).all(|w| w[0].at < w[1].at));
        assert!(m.ended(all.last().unwrap().at + 0.5));
        // by the clock: none due before the first, then the first
        let mut again = open(&wb().unwrap(), (1024, 768), Decoder::OpenH264).unwrap();
        let first = again.next_picture().unwrap().at;
        again.queued = Some(Picture { at: first, bgra: vec![] });
        assert!(again.picture(first - 0.01).is_none());
        assert!(again.picture(first).is_some());
        all.shrink_to_fit();
        all
    }

    fn check_sound(m: &HdMovie) {
        let s = m.sound.as_ref().unwrap();
        assert_eq!((s.channels, s.rate), (2, 48000));
        assert!((m.seconds() - 6.68).abs() < 0.05, "{}", m.seconds());
    }

    #[test]
    fn openh264_decodes_wb() {
        let Some(path) = wb() else { return };
        let mut m = open(&path, (1024, 768), Decoder::OpenH264).unwrap();
        assert_eq!(m.decoder, "OpenH264");
        assert_eq!(m.size, (2880, 1620));
        // halved once: 1440 is the last halving at least 1024 wide
        assert_eq!(m.decoded, (1440, 810));
        check_sound(&m);
        let pictures = all_pictures(&mut m);
        assert_eq!(pictures.len(), 161);
        assert!(pictures.iter().all(|p| p.bgra.len() == 1440 * 810 * 4));
    }

    /// The two decoders agree on a picture at full size.
    #[cfg(windows)]
    #[test]
    fn decoders_agree() {
        let Some(path) = wb() else { return };
        let mut a = open(&path, (1024, 768), Decoder::Any).unwrap();
        assert_eq!((a.decoder, a.decoded), ("Media Foundation", (1024, 576)));
        check_sound(&a);
        assert_eq!(all_pictures(&mut a).len(), 161);
        let full = (4000, 4000);
        let mut a = open(&path, full, Decoder::Any).unwrap();
        let mut b = open(&path, full, Decoder::OpenH264).unwrap();
        assert_eq!(a.decoded, b.decoded);
        let (a, b) = (all_pictures(&mut a), all_pictures(&mut b));
        let (pa, pb) = (&a[80], &b[80]);
        assert!((pa.at - pb.at).abs() < 1e-3, "{} {}", pa.at, pb.at);
        let diff: u64 = pa
            .bgra
            .iter()
            .zip(&pb.bgra)
            .enumerate()
            .filter(|(i, _)| i % 4 != 3)
            .map(|(_, (x, y))| x.abs_diff(*y) as u64)
            .sum();
        let mean = diff as f64 / (pa.bgra.len() / 4 * 3) as f64;
        assert!(mean < 1.5, "mean difference {mean}");
    }
}
