//! Interactive music: a PathFinder `.mpf` graph over the segments of its `.mus` stream file.
//!
//! The graphs are built from sections of three nodes (the `FE` and `QD` scores are nothing else): an
//! entry node with no sample, a node that plays one segment, and a decision node with no sample whose branches lead
//! back to the entry (second byte `0x7f`) or on to the next section (second byte `0`). Events start a section: an
//! event's action ending in `0xff` names its entry node.
//!
//! Inferred, not traced: [`Score::next`] follows the `0x7f` branch, so a section loops until the game starts another
//! with an event. How the game's music variables pick the other branch is in the unread music engine
//! (`0x53a000`-`0x556000`).
//!
//! **[T]** the game posts events by number (the score manager's `0x526680` -> `0x550620`), and the drivers' numbers
//! match the scores' event lists: the match scores have 14 events and the match driver posts 0-13, the front end's
//! has 6 and its driver posts 0, 2, 3 and 4. **[I]** what an event's actions do ([`Score::cue`]): an action ending
//! in `0xff` names the section's entry node; an action whose last byte is 1 makes the change at once, else it waits
//! for the playing segment to end; an event with only that action stops the music.

use std::collections::HashMap;
use std::sync::Mutex;

use qwc_assets::{Bytes, Vfs};
use qwc_formats::mpf::MusicFile;

use crate::library::stream_at;
use crate::mixer::{Bus, Mixer, Params, VoiceId};
use crate::{Error, Result, Sound};

/// The game's scores: (directory under `Audio/Music`, `.mpf`, `.mus`). The pairs are a table in the exe (read by
/// `0x5262b0`); the names do not follow one rule.
pub const SCORES: [(&str, &str, &str); 5] = [
    ("FE", "fe.mpf", "qwcfe.mus"),
    ("HW", "QWC_HW.mpf", "qwchw.mus"),
    ("Intl", "QWCINT.mpf", "QWCINT.mus"),
    ("QD", "QWC_QD.mpf", "qwcqd.mus"),
    ("chal", "CHAL.mpf", "chal.mus"),
];

/// No node (a branch target or a sample number of `0xffff`).
const NONE: u16 = 0xFFFF;

/// One score: the graph, its stream file and the segments decoded so far.
pub struct Score {
    pub name: String,
    pub graph: MusicFile,
    mus: Bytes,
    mus_path: String,
    segments: HashMap<u16, Sound>,
}

impl Score {
    /// Load the score `name` (a [`SCORES`] directory: `FE`, `HW`, `Intl`, `QD` or `chal`, any case).
    pub fn load(vfs: &Vfs, name: &str) -> Result<Score> {
        let &(dir, mpf, mus) = SCORES
            .iter()
            .find(|s| s.0.eq_ignore_ascii_case(name))
            .ok_or_else(|| Error::Missing(format!("score {name}")))?;
        let mpf_path = format!("Audio/Music/{dir}/{mpf}");
        let graph = MusicFile::parse(&vfs.read(&mpf_path)?).map_err(|e| Error::Format(mpf_path, e))?;
        let mus_path = format!("Audio/Music/{mus}");
        let mus = vfs.read(&mus_path)?;
        Ok(Score { name: dir.to_string(), graph, mus, mus_path, segments: HashMap::new() })
    }

    /// Segment `sample` (1-based across all tracks, as nodes count them), decoded.
    pub fn segment(&mut self, sample: u16) -> Result<Sound> {
        if let Some(s) = self.segments.get(&sample) {
            return Ok(s.clone());
        }
        let missing = || Error::Missing(format!("segment {sample} of score {}", self.name));
        let s = self.graph.tracks.iter().flatten().nth((sample as usize).checked_sub(1).ok_or_else(missing)?);
        let offset = s.ok_or_else(missing)?.offset as usize;
        let what = format!("{} at {offset:#x}", self.mus_path);
        let sound = stream_at(&self.mus, offset, &what)?;
        self.segments.insert(sample, sound.clone());
        Ok(sound)
    }

    /// The entry node event `event` starts, if it starts one.
    pub fn event_node(&self, event: usize) -> Option<usize> {
        let e = self.graph.events.get(event)?;
        let a = e.actions.iter().rev().find(|a| a.len() >= 16 && a[14] == 0xFF)?;
        let node = u16::from_le_bytes([a[12], a[13]]);
        (node != NONE && (node as usize) < self.graph.nodes.len()).then_some(node as usize)
    }

    /// What event `event` does (**[I]**, see the module notes).
    pub fn cue(&self, event: usize) -> Cue {
        let Some(e) = self.graph.events.get(event) else { return Cue::Nothing };
        if e.actions.is_empty() {
            return Cue::Nothing;
        }
        let now = e.actions.iter().any(|a| a.len() >= 16 && a[14] != 0xFF && a[15] == 1);
        match self.event_node(event) {
            Some(node) => Cue::Section { node, now },
            None => Cue::Stop,
        }
    }

    /// The node after `node`: its `0x7f` branch, else its first.
    fn follow(&self, node: usize) -> Option<usize> {
        let n = self.graph.nodes.get(node)?;
        let b = n.branches.iter().find(|b| b.b == 0x7F).or(n.branches.first())?;
        (b.target != NONE && (b.target as usize) < self.graph.nodes.len()).then_some(b.target as usize)
    }

    /// From `node`, the next node that plays a segment and the node after it. `None` when the path ends first.
    pub fn next(&self, mut node: usize) -> Option<(u16, Option<usize>)> {
        // a path that never reaches a segment would loop forever; it cannot be longer than the graph
        for _ in 0..=self.graph.nodes.len() {
            let sample = self.graph.nodes.get(node)?.sample;
            if sample != 0 && sample != NONE {
                return Some((sample, self.follow(node)));
            }
            node = self.follow(node)?;
        }
        None
    }
}

/// What an event does to the music ([`Score::cue`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cue {
    /// Go to the section at entry node `node`: at once, or when the playing segment ends.
    Section {
        node: usize,
        now: bool,
    },
    Stop,
    /// No actions, or no such event.
    Nothing,
}

/// Plays a score on one mixer voice, keeping the next segment queued so segments follow with no gap.
pub struct MusicPlayer {
    pub score: Score,
    pub params: Params,
    voice: Option<VoiceId>,
    /// Where the path continues after the last queued segment.
    node: Option<usize>,
}

impl MusicPlayer {
    pub fn new(score: Score) -> MusicPlayer {
        MusicPlayer { score, params: Params { bus: Bus::Music, ..Params::default() }, voice: None, node: None }
    }

    /// The next segment on the path, advancing it.
    fn advance(&mut self) -> Result<Option<Sound>> {
        let Some((sample, after)) = self.node.and_then(|n| self.score.next(n)) else {
            self.node = None;
            return Ok(None);
        };
        self.node = after;
        self.score.segment(sample).map(Some)
    }

    /// Stop what is playing and start at `node` (an entry node, see [`Score::event_node`]).
    pub fn start(&mut self, mixer: &Mutex<Mixer>, node: usize) -> Result<()> {
        self.stop(mixer);
        self.node = Some(node);
        if let Some(first) = self.advance()? {
            self.voice = Some(mixer.lock().unwrap().play(first, self.params));
        }
        self.update(mixer)
    }

    /// Start event `event`'s section.
    pub fn play_event(&mut self, mixer: &Mutex<Mixer>, event: usize) -> Result<()> {
        let node = self
            .score
            .event_node(event)
            .ok_or_else(|| Error::Missing(format!("event {event} of {}", self.score.name)))?;
        self.start(mixer, node)
    }

    /// Post event `event` as the game does ([`Score::cue`]): a change that waits takes the place of the segment
    /// queued behind the playing one.
    pub fn post(&mut self, mixer: &Mutex<Mixer>, event: usize) -> Result<()> {
        match self.score.cue(event) {
            Cue::Section { node, now } if now || !self.is_playing() => self.start(mixer, node),
            Cue::Section { node, .. } => {
                self.node = Some(node);
                if let Some(v) = self.voice {
                    mixer.lock().unwrap().clear_queue(v);
                }
                self.update(mixer)
            }
            Cue::Stop => {
                self.stop(mixer);
                Ok(())
            }
            Cue::Nothing => Ok(()),
        }
    }

    /// Keep one segment queued behind the playing one. Call it every frame; decoding happens here, outside the lock.
    pub fn update(&mut self, mixer: &Mutex<Mixer>) -> Result<()> {
        let Some(voice) = self.voice else { return Ok(()) };
        match mixer.lock().unwrap().queued(voice) {
            None => {
                self.voice = None;
                return Ok(());
            }
            Some(0) => {}
            Some(_) => return Ok(()),
        }
        if let Some(next) = self.advance()? {
            mixer.lock().unwrap().queue(voice, next);
        }
        Ok(())
    }

    /// The mixer voice playing the score.
    pub fn voice(&self) -> Option<VoiceId> {
        self.voice
    }

    pub fn is_playing(&self) -> bool {
        self.voice.is_some()
    }

    pub fn stop(&mut self, mixer: &Mutex<Mixer>) {
        if let Some(v) = self.voice.take() {
            mixer.lock().unwrap().stop(v);
        }
        self.node = None;
    }
}
