//! The `sound` and `music` commands: play the game's audio through the default output device, or render it to a WAV
//! file with `--out`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use qwc_assets::Vfs;
use qwc_audio::{Bus, Library, Mixer, MusicPlayer, Params, Score, Sound};
use qwc_formats::ea_audio::Pcm;

use crate::{fail, usage, Args};

/// Rate of offline renders.
const RENDER_RATE: u32 = 44100;
/// Length of an offline music render without `--time`.
const RENDER_MUSIC_SECS: f64 = 60.0;

/// Where the mixed audio goes.
enum Sink {
    Device(qwc_audio::Output),
    Wav { mixer: Arc<Mutex<Mixer>>, out: Vec<i16> },
}

impl Sink {
    fn new(args: &Args) -> Sink {
        if args.out.is_some() {
            return Sink::Wav { mixer: Arc::new(Mutex::new(Mixer::new(RENDER_RATE))), out: Vec::new() };
        }
        let out = qwc_audio::Output::open().unwrap_or_else(|e| fail(e));
        eprintln!("playing on {} ({} Hz, {} channels)", out.device, out.mixer().lock().unwrap().rate(), out.channels);
        Sink::Device(out)
    }

    fn mixer(&self) -> &Arc<Mutex<Mixer>> {
        match self {
            Sink::Device(o) => o.mixer(),
            Sink::Wav { mixer, .. } => mixer,
        }
    }

    /// Advance by about 20 ms: wait for the device, or render that much into the file.
    fn step(&mut self) {
        match self {
            Sink::Device(_) => std::thread::sleep(Duration::from_millis(20)),
            Sink::Wav { mixer, out } => {
                let mut buf = vec![0.0f32; RENDER_RATE as usize / 50 * 2];
                mixer.lock().unwrap().render(&mut buf);
                out.extend(buf.iter().map(|s| (s * 32767.0).round() as i16));
            }
        }
    }

    fn elapsed(&self, start: Instant) -> f64 {
        match self {
            Sink::Device(_) => start.elapsed().as_secs_f64(),
            Sink::Wav { out, .. } => out.len() as f64 / 2.0 / RENDER_RATE as f64,
        }
    }

    fn finish(self, args: &Args) {
        if let (Sink::Wav { out, .. }, Some(path)) = (self, &args.out) {
            let pcm = Pcm { channels: 2, rate: RENDER_RATE, samples: out };
            std::fs::write(path, pcm.to_wav()).unwrap_or_else(|e| fail(format!("{}: {e}", path.display())));
            eprintln!("wrote {} ({:.1} s)", path.display(), pcm.frames() as f64 / RENDER_RATE as f64);
        }
    }
}

fn number(s: &str) -> u32 {
    s.parse().unwrap_or_else(|_| usage())
}

/// `sound event CATEGORY.map ID`, `sound bank BANK INDEX`, `sound speech LANGUAGE NAME`: the sounds play one after
/// another.
pub fn sound(args: &Args, vfs: &Vfs) {
    let lib = Library::open(vfs).unwrap_or_else(|e| fail(e));
    let a = &args.models;
    let (sounds, bus): (Vec<Sound>, Bus) = match (a.first().map(String::as_str), a.len()) {
        (Some("event"), 3) => {
            let s = lib.event(vfs, &a[1], number(&a[2]), roll()).unwrap_or_else(|e| fail(e));
            (s.into_iter().collect(), Bus::Effects)
        }
        (Some("bank"), 3) => {
            let s = lib.bank_sound(vfs, &a[1], number(&a[2])).unwrap_or_else(|e| fail(e));
            (s.into_iter().collect(), Bus::Effects)
        }
        (Some("speech"), 3) => (lib.speech(vfs, &a[1], &a[2]).unwrap_or_else(|e| fail(e)), Bus::Speech),
        _ => usage(),
    };
    let Some((first, rest)) = sounds.split_first() else { fail("that event plays no sound") };
    for s in &sounds {
        eprintln!("{} Hz, {} channels, {:.2} s", s.rate, s.channels, s.frames() as f64 / s.rate as f64);
    }
    let mut sink = Sink::new(args);
    {
        let mut m = sink.mixer().lock().unwrap();
        let v = m.play(first.clone(), Params { bus, ..Params::default() });
        for s in rest {
            m.queue(v, s.clone());
        }
    }
    while sink.mixer().lock().unwrap().voice_count() > 0 {
        sink.step();
    }
    sink.finish(args);
}

/// `music SCORE [EVENT]`: play a score from the section an event starts (default 0). Plays until `--time` seconds
/// (forever on the device; one minute into a file).
pub fn music(args: &Args, vfs: &Vfs) {
    let a = &args.models;
    let (name, event) = match a.len() {
        1 => (&a[0], 0),
        2 => (&a[0], number(&a[1]) as usize),
        _ => usage(),
    };
    let score = Score::load(vfs, name).unwrap_or_else(|e| fail(e));
    eprintln!("{}: {} nodes, {} events", score.name, score.graph.nodes.len(), score.graph.events.len());
    let mut player = MusicPlayer::new(score);
    let mut sink = Sink::new(args);
    player.play_event(sink.mixer(), event).unwrap_or_else(|e| fail(e));
    let limit = match (args.time, &sink) {
        (t, _) if t > 0.0 => t,
        (_, Sink::Wav { .. }) => RENDER_MUSIC_SECS,
        _ => f64::INFINITY,
    };
    let start = Instant::now();
    while player.is_playing() && sink.elapsed(start) < limit {
        sink.step();
        player.update(sink.mixer()).unwrap_or_else(|e| fail(e));
    }
    sink.finish(args);
}

/// A pick among an event's weighted samples, from the clock (the game's own random source is not traced).
fn roll() -> f32 {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    (t.subsec_nanos() % 65536) as f32 / 65536.0
}
