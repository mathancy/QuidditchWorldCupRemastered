//! A software mixer: any number of voices, each a decoded sound resampled to the output rate, mixed to interleaved
//! stereo `f32`. It runs wherever it is called from (the playback callback, a test, an offline render), so it does no
//! decoding or allocation per sample; sounds arrive already decoded.
//!
//! A voice can carry a queue of sounds that follow the current one with no gap, which is how interactive music
//! changes segments: the game thread decodes the next segment and queues it while the current one plays.

use std::collections::VecDeque;
use std::sync::Arc;

use qwc_formats::ea_audio::Pcm;

/// A decoded sound, shared between the library cache and the voices playing it.
pub type Sound = Arc<Pcm>;

/// Mix groups with their own volume, matching the game's separate effects, speech and music settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bus {
    Effects,
    Speech,
    Music,
}

const BUSES: usize = 3;

/// How a voice plays.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    /// Linear gain, 1.0 = as decoded.
    pub gain: f32,
    /// -1.0 = left only, 0.0 = centre (both channels at full level), 1.0 = right only.
    pub pan: f32,
    /// Playback speed, 1.0 = the sound's own rate.
    pub pitch: f32,
    pub looping: bool,
    pub bus: Bus,
}

impl Default for Params {
    fn default() -> Params {
        Params { gain: 1.0, pan: 0.0, pitch: 1.0, looping: false, bus: Bus::Effects }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VoiceId(u64);

#[derive(Debug)]
struct Voice {
    id: VoiceId,
    sound: Sound,
    /// Position in frames of `sound`, fractional between two frames.
    pos: f64,
    queue: VecDeque<Sound>,
    params: Params,
}

impl Voice {
    /// The sound's frame `i` as (left, right), with mono copied to both sides.
    fn frame(pcm: &Pcm, i: usize) -> (f32, f32) {
        const SCALE: f32 = 1.0 / 32768.0;
        let ch = pcm.channels;
        let s = &pcm.samples[i * ch..];
        let l = s[0] as f32 * SCALE;
        (l, if ch > 1 { s[1] as f32 * SCALE } else { l })
    }

    /// Mix into `out` (interleaved stereo). Returns false once the voice has finished.
    fn mix(&mut self, out: &mut [f32], out_rate: u32, gain: f32) -> bool {
        let p = self.params.pan.clamp(-1.0, 1.0);
        let (gl, gr) = (gain * (1.0 - p).min(1.0), gain * (1.0 + p).min(1.0));
        let speed = self.params.pitch.max(0.0) as f64 / out_rate as f64;
        for o in out.as_chunks_mut::<2>().0 {
            let mut n = self.sound.frames();
            while self.pos >= n as f64 {
                if self.params.looping && n > 0 {
                    self.pos %= n as f64;
                } else if let Some(next) = self.queue.pop_front() {
                    self.pos -= n as f64;
                    self.sound = next;
                    n = self.sound.frames();
                } else {
                    return false;
                }
            }
            // linear interpolation; the last frame is held unless the sound loops
            let i = self.pos as usize;
            let t = (self.pos - i as f64) as f32;
            let (l0, r0) = Voice::frame(&self.sound, i);
            let (l1, r1) = match (i + 1 < n, self.params.looping) {
                (true, _) => Voice::frame(&self.sound, i + 1),
                (false, true) => Voice::frame(&self.sound, 0),
                (false, false) => (l0, r0),
            };
            o[0] += (l0 + (l1 - l0) * t) * gl;
            o[1] += (r0 + (r1 - r0) * t) * gr;
            self.pos += self.sound.rate as f64 * speed;
        }
        true
    }
}

/// The mixer state: voices, bus volumes and the output rate.
#[derive(Debug)]
pub struct Mixer {
    rate: u32,
    voices: Vec<Voice>,
    next_id: u64,
    master: f32,
    buses: [f32; BUSES],
}

impl Mixer {
    /// A mixer producing stereo at `rate` frames per second.
    pub fn new(rate: u32) -> Mixer {
        Mixer { rate: rate.max(1), voices: Vec::new(), next_id: 1, master: 1.0, buses: [1.0; BUSES] }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Start playing `sound`. Empty sounds start and finish at once.
    pub fn play(&mut self, sound: Sound, params: Params) -> VoiceId {
        let id = VoiceId(self.next_id);
        self.next_id += 1;
        self.voices.push(Voice { id, sound, pos: 0.0, queue: VecDeque::new(), params });
        id
    }

    /// Append `sound` to play straight after what voice `id` is playing. False when the voice has finished.
    pub fn queue(&mut self, id: VoiceId, sound: Sound) -> bool {
        self.voice_mut(id).map(|v| v.queue.push_back(sound)).is_some()
    }

    /// Drop what is queued behind voice `id`'s sound. False when the voice has finished.
    pub fn clear_queue(&mut self, id: VoiceId) -> bool {
        self.voice_mut(id).map(|v| v.queue.clear()).is_some()
    }

    /// Sounds waiting in voice `id`'s queue, or `None` when the voice has finished.
    pub fn queued(&self, id: VoiceId) -> Option<usize> {
        self.voices.iter().find(|v| v.id == id).map(|v| v.queue.len())
    }

    pub fn is_playing(&self, id: VoiceId) -> bool {
        self.voices.iter().any(|v| v.id == id)
    }

    pub fn params(&self, id: VoiceId) -> Option<Params> {
        self.voices.iter().find(|v| v.id == id).map(|v| v.params)
    }

    pub fn set_params(&mut self, id: VoiceId, params: Params) -> bool {
        self.voice_mut(id).map(|v| v.params = params).is_some()
    }

    pub fn stop(&mut self, id: VoiceId) {
        self.voices.retain(|v| v.id != id);
    }

    pub fn stop_bus(&mut self, bus: Bus) {
        self.voices.retain(|v| v.params.bus != bus);
    }

    pub fn stop_all(&mut self) {
        self.voices.clear();
    }

    pub fn voice_count(&self) -> usize {
        self.voices.len()
    }

    pub fn set_master(&mut self, gain: f32) {
        self.master = gain;
    }

    pub fn set_bus_gain(&mut self, bus: Bus, gain: f32) {
        self.buses[bus as usize] = gain;
    }

    pub fn bus_gain(&self, bus: Bus) -> f32 {
        self.buses[bus as usize]
    }

    fn voice_mut(&mut self, id: VoiceId) -> Option<&mut Voice> {
        self.voices.iter_mut().find(|v| v.id == id)
    }

    /// Fill `out` (interleaved stereo, so an even length) with the next `out.len() / 2` frames, clamped to
    /// [-1, 1]. Finished voices are dropped.
    pub fn render(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        let (rate, master, buses) = (self.rate, self.master, self.buses);
        self.voices.retain_mut(|v| {
            let gain = v.params.gain * buses[v.params.bus as usize] * master;
            v.mix(out, rate, gain)
        });
        for s in out.iter_mut() {
            *s = s.clamp(-1.0, 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(channels: usize, rate: u32, samples: Vec<i16>) -> Sound {
        Arc::new(Pcm { channels, rate, samples })
    }

    fn render(m: &mut Mixer, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames * 2];
        m.render(&mut out);
        out
    }

    #[test]
    fn plays_once_at_the_same_rate() {
        let mut m = Mixer::new(100);
        let id = m.play(pcm(1, 100, vec![16384, -16384, 8192]), Params::default());
        let out = render(&mut m, 5);
        assert_eq!(out, [0.5, 0.5, -0.5, -0.5, 0.25, 0.25, 0.0, 0.0, 0.0, 0.0]);
        assert!(!m.is_playing(id));
    }

    #[test]
    fn pans_resamples_and_loops() {
        let mut m = Mixer::new(200);
        let p = Params { pan: 1.0, looping: true, ..Params::default() };
        m.play(pcm(1, 100, vec![0, 16384]), p);
        // half speed: 0, halfway, 16384, halfway back to frame 0 (looping), 0, ...
        let left: Vec<f32> = render(&mut m, 5).chunks(2).map(|f| f[0]).collect();
        assert_eq!(left, [0.0; 5]);
        let mut m = Mixer::new(200);
        m.play(pcm(1, 100, vec![0, 16384]), p);
        let right: Vec<f32> = render(&mut m, 5).chunks(2).map(|f| f[1]).collect();
        assert_eq!(right, [0.0, 0.25, 0.5, 0.25, 0.0]);
        assert_eq!(m.voice_count(), 1);
    }

    #[test]
    fn queue_follows_without_a_gap_and_buses_scale() {
        let mut m = Mixer::new(100);
        let id = m.play(pcm(2, 100, vec![16384, 0]), Params { bus: Bus::Music, ..Params::default() });
        assert!(m.queue(id, pcm(1, 100, vec![8192])));
        assert_eq!(m.queued(id), Some(1));
        m.set_bus_gain(Bus::Music, 0.5);
        assert_eq!(render(&mut m, 3), [0.25, 0.0, 0.125, 0.125, 0.0, 0.0]);
        assert_eq!(m.queued(id), None);
    }

    #[test]
    fn clamps() {
        let mut m = Mixer::new(100);
        for _ in 0..3 {
            m.play(pcm(1, 100, vec![30000]), Params::default());
        }
        assert_eq!(render(&mut m, 1), [1.0, 1.0]);
    }
}
