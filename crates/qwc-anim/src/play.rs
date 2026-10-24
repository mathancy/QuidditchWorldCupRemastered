//! The animation library's playback (`docs/ANIMATION.md`, "Channels"): channels play queues of clips, and the
//! state's step (`0x4124a0`) runs them for a time, split at every clip end and trigger so both land exactly and
//! their callbacks run in order. The pose the channels make is [`crate::pose`].
//!
//! The arithmetic is single precision, as the original's stores are.

use std::sync::Arc;

use crate::pose::{Buffers, Pose};
use crate::trigger::{self, Fired, Key, TriggerList, Triggers, View};
use crate::Clip;

/// Queue entry flags (`entry +0`).
pub mod flag {
    /// Wrap to the start at the end while it is the only entry.
    pub const LOOP: u32 = 1;
    /// Start from the last frame; the speed is negated.
    pub const REVERSE: u32 = 2;
    /// Stay on the last frame at the end (frozen) instead of leaving the queue.
    pub const HOLD: u32 = 4;
    /// Its triggers are added to a set's entries, never blended into them (`0x416370`'s last argument).
    pub const NO_TRIGGER_BLEND: u32 = 0x20;
    /// A jump to a start frame counts the triggers it skips as fired (`0x4784b0`).
    pub const SKIP_TRIGGERS: u32 = 0x40;
    /// Ends within the slice being run (set by the scheduler, `0x411fe0`).
    pub const ENDS_NOW: u32 = 0x0800_0000;
    /// Freezes at a named trigger (`0x410fc0`), and on being resumed at the next (`0x411330`).
    pub const STOPS: u32 = 0x1000_0000;
    /// No longer advances.
    pub const FROZEN: u32 = 0x2000_0000;
    /// Reached its end: loop, leave the queue or hold.
    pub const REACHED: u32 = 0x4000_0000;
    /// The end callback is due.
    pub const CALLBACK: u32 = 0x8000_0000;
}

/// What the library keeps of a clip (`+8`, `+0xc`, `+0x10`, from the `.fas` base part `FUN_0040a4f0`): its first
/// and last frames and its frames per second.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipInfo {
    pub first: i32,
    pub last: i32,
    pub fps: f32,
}

impl ClipInfo {
    /// `0x478540`: the length in seconds.
    pub fn seconds(&self) -> f32 {
        (self.last - self.first) as f32 / self.fps
    }
}

/// What a clip carries besides its timing: its tracks for the pose and its trigger lists (`.btf`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClipData {
    pub anim: Option<Clip>,
    pub triggers: Vec<TriggerList>,
}

/// A queued clip (0x1c bytes in the original).
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub flags: u32,
    /// Frames advance by `speed × seconds × fps`; negative plays backwards.
    pub speed: f32,
    pub clip: ClipInfo,
    /// The clip's handle (anim set in bits 0-6, clip in bits 7-17).
    pub handle: u32,
    /// Who hears the end (the callback and its argument, `entry +0x10`, `+0x14`): the port's own token.
    pub notify: Option<u32>,
    /// The clip's tracks and triggers. **[I]** word 6 is a rig-to-rig mapping (`CIModelToModelMapping`), which
    /// the port does not use: clips play on the skeleton they were made for.
    pub data: Option<Arc<ClipData>>,
}

impl Entry {
    fn anim(&self) -> Option<&Clip> {
        self.data.as_ref()?.anim.as_ref()
    }

    fn lists(&self) -> &[TriggerList] {
        self.data.as_ref().map_or(&[], |d| &d.triggers)
    }
}

/// A channel (`CIAnimChannel`): the clip playing and those queued after it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Channel {
    /// Its handle (layer in bits 18-24, channel in 25-31; `+8`).
    pub id: u32,
    /// `+0x10`
    pub frame: f32,
    /// The time this slice runs (`+0x18`, seconds).
    pub slice: f32,
    /// `+0xc` bit 0: the step skips it.
    pub paused: bool,
    pub queue: Vec<Entry>,
    /// The trigger set it freezes at (`+0x44`).
    pub stop: Option<Key>,
}

/// An end the step reached: `(notify token, handle, code 1, flags)` as the original calls the entry's callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ended {
    pub notify: u32,
    /// The channel's layer and channel bits with the entry's set and clip bits.
    pub handle: u32,
    pub flags: u32,
}

impl Channel {
    pub fn new(id: u32) -> Channel {
        Channel { id, ..Default::default() }
    }

    /// `0x410fc0`'s queueing: the clip joins the queue; a channel that was idle starts it at once. Answers
    /// whether it started. (The state's [`State::play`] also brings its triggers.)
    pub fn play(&mut self, flags: u32, speed: f32, clip: ClipInfo, handle: u32, notify: Option<u32>) -> bool {
        let flags = flags & 0x0fff_ffff;
        let speed = if flags & flag::REVERSE != 0 { -1.0 } else { 1.0 } * speed;
        self.queue.push(Entry { flags, speed, clip, handle, notify, data: None });
        let started = self.queue.len() == 1;
        if started {
            self.start_front();
        }
        started
    }

    /// The front entry starts: at its first frame (its last when reversed).
    fn start_front(&mut self) {
        let e = &mut self.queue[0];
        e.flags &= !flag::ENDS_NOW;
        self.stop = None;
        self.frame = if e.flags & flag::REVERSE != 0 { e.clip.last } else { e.clip.first } as f32;
    }

    /// Whether the step runs it: something queued and not paused.
    pub fn active(&self) -> bool {
        !self.queue.is_empty() && !self.paused
    }

    /// The scheduler's time to the front entry's end (`0x411fe0`), never below 0; `None` when frozen.
    pub fn time_to_end(&self) -> Option<f32> {
        let e = self.queue.first()?;
        if e.flags & flag::FROZEN != 0 || self.paused {
            return None;
        }
        let end = if e.speed < 0.0 { e.clip.first } else { e.clip.last } as f32;
        let t = (end - self.frame) / (e.clip.fps * e.speed);
        Some(if 0.0 <= t { t } else { 0.0 })
    }

    /// `0x410d00` as the step runs it: advance the front entry by the slice. Inside the clip the frame is clamped
    /// to it; an entry marked to end this slice lands on its end and is marked reached with its callback due (and
    /// frozen when it holds). Answers whether it ended.
    fn advance(&mut self) -> bool {
        let e = &mut self.queue[0];
        if e.flags & flag::FROZEN != 0 {
            return false;
        }
        let (first, last) = (e.clip.first as f32, e.clip.last as f32);
        let frame = e.speed * self.slice * e.clip.fps + self.frame;
        self.frame = frame;
        if e.flags & flag::ENDS_NOW == 0 {
            self.frame = frame.min(last).max(first);
            self.slice = 0.0;
            return false;
        }
        self.slice = 0.0;
        self.frame = if e.speed < 0.0 { first } else { last };
        e.flags |= if e.flags & flag::HOLD == 0 { 0xc000_0000 } else { 0xa000_0000 };
        true
    }
}

/// An animation layer (`CIAnimLayer`): channels, and how it blends (`+0x20` mode: 1 sets, 2 blends by the weight,
/// 4 adds by it, 8 blends by a weight per bone; `+0x24` weight; `+0x28` the weights per bone).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layer {
    pub mode: u32,
    pub weight: f32,
    pub bone_weights: Option<Vec<f32>>,
    pub channels: Vec<Channel>,
}

/// An animation state (`CIAnimState`): its layers by creation (`+0x44`, what handles index) and the order they
/// run and blend in (the `+0x14` list), with the control's trigger manager and the pose's working buffers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub layers: Vec<Layer>,
    pub order: Vec<usize>,
    pub triggers: Triggers,
    /// The node whose morph weights fade after its clip ended, and how far (`+0x4c`, `+0x50`, `0x4118d0`).
    pub morph_fade: Option<(u16, f32)>,
    pub(crate) buffers: Buffers,
}

impl State {
    /// `0x411c10` (CreateLayer): a layer with one channel, appended to the order; its channel's handle has the
    /// layer in bits 18-24, channel 0 and all low bits set (`0x4114b0`).
    pub fn add_layer(&mut self) -> usize {
        let l = self.layers.len();
        let id = (l as u32 & 0x7f) << 18 | 0x3ffff;
        self.layers.push(Layer { channels: vec![Channel::new(id)], ..Default::default() });
        self.order.push(l);
        l
    }

    fn view(&self) -> View<'_> {
        View { layers: &self.layers, order: &self.order }
    }

    /// `0x411e00`: move layer `l` to just before `before` in the order (to the end without one); the trigger
    /// instances follow the new order (`0x416640`).
    pub fn move_before(&mut self, l: usize, before: Option<usize>) {
        if before == Some(l) {
            return;
        }
        self.order.retain(|&k| k != l);
        let at = before.and_then(|b| self.order.iter().position(|&k| k == b)).unwrap_or(self.order.len());
        self.order.insert(at, l);
        let v = View { layers: &self.layers, order: &self.order };
        self.triggers.reorder(v);
    }

    /// `0x4163d0`'s callers: a layer's weight, its blended trigger times following (`0x415420`).
    pub fn set_weight(&mut self, l: usize, w: f32) {
        self.layers[l].weight = w;
        let v = View { layers: &self.layers, order: &self.order };
        self.triggers.reweigh(v, l);
    }

    /// `0x411f30` -> `0x410fc0`: queue a clip on channel `id`. A channel that was idle starts it at once and its
    /// trigger lists join the manager; with `stop_at`, it freezes at the first trigger of that name ahead.
    /// A clip with morph tracks ends the fade of the last one's face at once (`0x4118d0` to 0).
    #[allow(clippy::too_many_arguments)]
    pub fn play(
        &mut self,
        id: u32,
        flags: u32,
        speed: f32,
        clip: ClipInfo,
        handle: u32,
        notify: Option<u32>,
        data: Option<Arc<ClipData>>,
        stop_at: Option<&str>,
        pose: Option<&mut Pose>,
    ) {
        let morphs = data.as_ref().and_then(|d| d.anim.as_ref()).is_some_and(|a| !a.morphs.is_empty());
        let Some(c) = self.channel_mut(id) else { return };
        let started = c.play(flags, speed, clip, handle, notify);
        c.queue.last_mut().unwrap().data = data;
        if started {
            self.start_triggers(id);
            if let Some(name) = stop_at {
                let c = self.channel_mut(id).unwrap();
                let e = &mut c.queue[0];
                e.flags |= flag::STOPS;
                let reversed = e.speed < 0.0;
                let from = if reversed { e.clip.last as f32 + 1.0 } else { -1.0 };
                let key = self.triggers.stop_at(Some(name), None, id, from, !reversed);
                self.channel_mut(id).unwrap().stop = key;
            }
        }
        if morphs && self.morph_fade.is_some_and(|f| f.1 != 0.0) {
            self.fade_morphs(0.0, Some(0.0), pose);
            self.morph_fade = None;
        }
    }

    /// `0x416370`: the front clip's trigger lists join the manager.
    fn start_triggers(&mut self, id: u32) {
        let l = (id >> 18 & 0x7f) as usize;
        let Some(e) = self.channel_mut(id).and_then(|c| c.queue.first()).cloned() else { return };
        for list in e.lists() {
            let v = View { layers: &self.layers, order: &self.order };
            self.triggers.install(v, list, l, id, e.clip, e.flags & flag::NO_TRIGGER_BLEND != 0);
        }
    }

    /// `0x4161e0` for each list of the front clip: its triggers leave the manager.
    fn end_triggers(&mut self, id: u32) {
        let Some(e) = self.channel_mut(id).and_then(|c| c.queue.first()).cloned() else { return };
        for list in e.lists() {
            let v = View { layers: &self.layers, order: &self.order };
            self.triggers.remove(v, &list.key(), id);
        }
    }

    /// `0x4784b0`: channel `id` jumps to `frame`, clamped to its front clip; with `skip`, the triggers it jumps
    /// over count as fired (or may fire again, going back).
    pub fn set_frame(&mut self, id: u32, frame: f32, skip: bool) {
        let Some(c) = self.channel_mut(id) else { return };
        let Some(e) = c.queue.first() else { return };
        let (first, last) = (e.clip.first as f32, e.clip.last as f32);
        let from = c.frame;
        let v = if frame < first { first } else { frame };
        c.frame = if v <= last { v } else { last };
        if skip {
            self.triggers.seek(id, from, frame);
        }
    }

    /// `0x411190`: channel `id` stops, its triggers leaving the manager, and nothing it queued calls back.
    pub fn stop(&mut self, id: u32) {
        self.end_triggers(id);
        if let Some(c) = self.channel_mut(id) {
            c.queue.clear();
        }
    }

    /// `0x411330`: a frozen channel runs on; one that stops at a trigger stops at its next.
    pub fn resume(&mut self, id: u32) -> bool {
        let Some(c) = self.channel_mut(id) else { return false };
        let Some(e) = c.queue.first_mut() else { return false };
        if e.flags & flag::FROZEN == 0 {
            return false;
        }
        e.flags &= !flag::FROZEN;
        if e.flags & flag::STOPS != 0 {
            let (key, frame, forward) = (c.stop.clone(), c.frame, 0.0 <= e.speed);
            let key = self.triggers.stop_at(None, key.as_ref(), id, frame, forward);
            self.channel_mut(id).unwrap().stop = key;
        }
        true
    }

    /// `0x411fe0`: whether anything is queued, and the earliest end or trigger within `dt`. Entries ending at that
    /// time, when it is before `dt`, are marked to end in the slice, and triggers at it (within `dt`) are due.
    fn schedule(&mut self, dt: f32) -> (bool, f32) {
        let mut min = self.triggers.next_time(self.view());
        let (mut any, mut early) = (false, false);
        for c in self.order.iter().flat_map(|&l| &self.layers[l].channels) {
            any |= !c.queue.is_empty();
            if let Some(t) = c.time_to_end() {
                if t < min {
                    min = t;
                    early = t < dt;
                }
            }
        }
        if !any {
            return (false, min);
        }
        let v = View { layers: &self.layers, order: &self.order };
        self.triggers.mark_due(v, min, dt);
        if early {
            for c in self.layers.iter_mut().flat_map(|l| &mut l.channels) {
                if c.time_to_end().is_some_and(|t| t == min && t < dt) {
                    c.queue[0].flags |= flag::ENDS_NOW;
                }
            }
        }
        (true, min)
    }

    /// `0x4124a0`: run every channel for `dt` seconds, appending the ends reached (in layer order) to `ended` and
    /// the triggers fired to `fired`. A clip with morph tracks that leaves fades its face out (into `pose`).
    pub fn step(&mut self, dt: f32, pose: Option<&mut Pose>, ended: &mut Vec<Ended>, fired: &mut Vec<Fired>) {
        if dt == 0.0 {
            return;
        }
        let mut left = dt;
        loop {
            let (any, min) = self.schedule(left);
            if !any {
                break;
            }
            let m = if 0.0 <= min { min } else { 0.0 };
            let slice = if m < left { m } else { left };
            left -= slice;
            if left <= 0.0 {
                left = 0.0;
            }
            for c in self.layers.iter_mut().flat_map(|l| &mut l.channels) {
                c.slice = slice;
                if c.active() {
                    c.advance();
                }
            }
            let mut freeze = Vec::new();
            let v = View { layers: &self.layers, order: &self.order };
            self.triggers.fire(v, fired, &mut freeze);
            for id in freeze {
                if let Some(c) = self.channel_mut(id) {
                    trigger::freeze(c);
                }
            }
            for l in self.order.clone() {
                for k in 0..self.layers[l].channels.len() {
                    if !self.end_entry(l, k, ended) {
                        // the rest of this layer's channels wait for the next slice
                        break;
                    }
                }
            }
            if left == 0.0 {
                break;
            }
        }
        if self.morph_fade.is_some_and(|f| f.1 != 0.0) {
            self.fade_morphs(dt, None, pose);
        }
    }

    /// The step's end processing for one channel: its callback, then a reached entry loops (its triggers may fire
    /// again), or leaves (its triggers with it) for the next, whose triggers join. Answers false when it left the
    /// channel empty.
    fn end_entry(&mut self, l: usize, k: usize, ended: &mut Vec<Ended>) -> bool {
        let c = &mut self.layers[l].channels[k];
        if !c.active() {
            return true;
        }
        let id = c.id;
        let e = &mut c.queue[0];
        if let Some(n) = e.notify.filter(|_| e.flags & flag::CALLBACK != 0) {
            e.flags &= !flag::CALLBACK;
            ended.push(Ended { notify: n, handle: (id ^ e.handle) & 0x3ffff ^ id, flags: e.flags });
        }
        let flags = e.flags;
        if flags & flag::REACHED != 0 {
            if flags & flag::LOOP == 0 || c.queue.len() != 1 {
                if let Some(node) = c.queue[0].anim().and_then(|a| a.morphs.first()).map(|m| m.node) {
                    self.morph_fade = Some((node, 1.0));
                }
                self.end_triggers(id);
                let c = &mut self.layers[l].channels[k];
                c.queue.remove(0);
                if c.queue.is_empty() {
                    return false;
                }
                c.start_front();
                self.start_triggers(id);
            } else {
                c.start_front();
                self.triggers.reset(id);
            }
        }
        self.layers[l].channels[k].queue[0].flags &= !flag::REACHED;
        true
    }

    /// `0x4118d0`: the fading face's weights scale with the fade, which falls by 4/3 a second (`to` none) or is
    /// set to `to`; at 0.005 or less it is 0.
    fn fade_morphs(&mut self, dt: f32, to: Option<f32>, pose: Option<&mut Pose>) {
        let Some((node, old)) = self.morph_fade else { return };
        let mut new = match to {
            None => old - dt * 1.333_333_4,
            Some(t) => {
                if old == 0.0 || t == old {
                    return;
                }
                t
            }
        };
        if new <= 0.005 {
            new = 0.0;
        }
        self.morph_fade = Some((node, new));
        if let Some(w) = pose.and_then(|p| p.morphs.get_mut(node as usize)) {
            let r = 1.0 / old;
            for x in w.iter_mut().filter(|x| **x != 0.0) {
                *x = r * *x * new;
            }
        }
    }

    /// The channel with this handle's layer and channel bits.
    pub fn channel_mut(&mut self, id: u32) -> Option<&mut Channel> {
        let (l, c) = ((id >> 18 & 0x7f) as usize, (id >> 25) as usize);
        self.layers.get_mut(l)?.channels.get_mut(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIP: ClipInfo = ClipInfo { first: 0, last: 30, fps: 30.0 };

    fn one() -> State {
        let mut s = State::default();
        s.add_layer();
        s.layers[0].channels[0].id = 0;
        s
    }

    fn step(s: &mut State, dt: f32, ended: &mut Vec<Ended>) -> Vec<Fired> {
        let mut fired = Vec::new();
        s.step(dt, None, ended, &mut fired);
        fired
    }

    #[test]
    fn a_clip_plays_to_its_end_and_leaves() {
        let mut s = one();
        s.layers[0].channels[0].play(0, 1.0, CLIP, 0x80, Some(7));
        let mut ended = Vec::new();
        step(&mut s, 0.5, &mut ended);
        assert_eq!(s.layers[0].channels[0].frame, 15.0);
        assert!(ended.is_empty());
        // the end lands exactly, in the middle of the step
        step(&mut s, 0.75, &mut ended);
        // the callback sees the entry's flags with "ends now" still set (only a restart clears it)
        assert_eq!(ended, [Ended { notify: 7, handle: 0x80, flags: flag::REACHED | flag::ENDS_NOW }]);
        assert!(s.layers[0].channels[0].queue.is_empty());
    }

    #[test]
    fn loops_wrap_and_queued_clips_follow() {
        let mut s = one();
        let c = &mut s.layers[0].channels[0];
        c.play(flag::LOOP, 1.0, CLIP, 0x80, Some(1));
        let mut ended = Vec::new();
        step(&mut s, 1.5, &mut ended);
        // one end at 1 s, then half way again
        assert_eq!(ended.len(), 1);
        assert_eq!(s.layers[0].channels[0].frame, 15.0);
        // a second clip queued behind a loop: the loop now leaves at its end, and the rest of the step runs the
        // new clip (0.1 s at speed 2 and 10 fps: 2 frames)
        let c = &mut s.layers[0].channels[0];
        c.play(0, 2.0, ClipInfo { first: 10, last: 20, fps: 10.0 }, 0x100, Some(2));
        ended.clear();
        step(&mut s, 0.6, &mut ended);
        assert_eq!(ended.len(), 1);
        let c = &s.layers[0].channels[0];
        assert_eq!((c.queue.len(), c.queue[0].handle), (1, 0x100));
        assert!((c.frame - 12.0).abs() < 1e-4, "{}", c.frame);
        // an end exactly at the step's end is not reached until the next step
        let mut s = one();
        s.layers[0].channels[0].play(0, 1.0, CLIP, 0, Some(1));
        ended.clear();
        step(&mut s, 1.0, &mut ended);
        assert!(ended.is_empty());
        assert_eq!(s.layers[0].channels[0].frame, 30.0);
        step(&mut s, 0.01, &mut ended);
        assert_eq!(ended.len(), 1);
    }

    #[test]
    fn holding_freezes_on_the_last_frame_and_reverse_starts_at_it() {
        let mut s = one();
        s.layers[0].channels[0].play(flag::HOLD | flag::REVERSE, 1.0, CLIP, 0, Some(3));
        assert_eq!(s.layers[0].channels[0].frame, 30.0);
        let mut ended = Vec::new();
        step(&mut s, 2.0, &mut ended);
        let c = &s.layers[0].channels[0];
        assert_eq!(ended.len(), 1);
        assert_eq!((c.frame, c.queue[0].flags & flag::FROZEN != 0), (0.0, true));
        step(&mut s, 1.0, &mut ended);
        assert_eq!(ended.len(), 1, "a frozen entry ends once");
    }

    fn with_triggers(lists: Vec<TriggerList>) -> Option<Arc<ClipData>> {
        Some(Arc::new(ClipData { anim: None, triggers: lists }))
    }

    fn list(name: &str, times: &[f32]) -> TriggerList {
        TriggerList { name: Some(name.into()), node: None, times: times.to_vec() }
    }

    #[test]
    fn triggers_fire_on_time_and_again_after_a_loop() {
        let mut s = one();
        let data = with_triggers(vec![list("CS_CAM_CUT", &[0.25, 0.75]), list("ON_AUDIO_X", &[0.5])]);
        s.play(0, flag::LOOP, 1.0, CLIP, 0x80, None, data, None, None);
        let mut ended = Vec::new();
        // one 0.6 s step reaches 0.25 and 0.5: the step splits there, so each fires exactly on its frame
        let fired = step(&mut s, 0.6, &mut ended);
        let names: Vec<_> = fired.iter().map(|f| f.name.as_deref().unwrap()).collect();
        assert_eq!(names, ["CS_CAM_CUT", "ON_AUDIO_X"]);
        assert!(fired.iter().all(|f| f.late.abs() < 1e-6), "{fired:?}");
        // the rest of the first pass, the loop, and the first trigger again
        let fired = step(&mut s, 0.7, &mut ended);
        let names: Vec<_> = fired.iter().map(|f| f.name.as_deref().unwrap()).collect();
        assert_eq!(names, ["CS_CAM_CUT", "CS_CAM_CUT"]);
        // a trigger exactly at the step's end fires in that step (an end there waits for the next)
        let mut s = one();
        s.play(0, 0, 1.0, CLIP, 0x80, None, with_triggers(vec![list("A", &[0.5])]), None, None);
        assert_eq!(step(&mut s, 0.5, &mut ended).len(), 1);
    }

    #[test]
    fn a_clip_leaving_takes_its_triggers() {
        let mut s = one();
        s.play(0, 0, 1.0, CLIP, 0x80, None, with_triggers(vec![list("A", &[0.9])]), None, None);
        s.play(0, 0, 1.0, CLIP, 0x100, None, with_triggers(vec![list("B", &[0.1])]), None, None);
        assert_eq!(s.triggers.sets.iter().flatten().count(), 1);
        let mut ended = Vec::new();
        let fired = step(&mut s, 1.2, &mut ended);
        let names: Vec<_> = fired.iter().map(|f| f.name.as_deref().unwrap()).collect();
        assert_eq!(names, ["A", "B"]);
        // A's set was freed when its clip left; B's hole-filling set is the only one
        assert_eq!(s.triggers.sets.iter().flatten().map(|t| t.name.clone().unwrap()).collect::<Vec<_>>(), ["B"]);
        s.stop(0);
        assert!(s.triggers.sets.iter().all(|t| t.is_none()));
    }

    #[test]
    fn stopping_at_a_trigger_freezes_until_resumed() {
        let mut s = one();
        let data = with_triggers(vec![list("STOP", &[0.25, 0.5])]);
        s.play(0, 0, 1.0, CLIP, 0x80, None, data, Some("stop"), None);
        let mut ended = Vec::new();
        step(&mut s, 0.4, &mut ended);
        assert_eq!(s.layers[0].channels[0].frame, 7.5);
        assert!(s.resume(0));
        step(&mut s, 1.0, &mut ended);
        assert_eq!(s.layers[0].channels[0].frame, 15.0);
    }

    #[test]
    fn a_start_frame_can_skip_triggers() {
        let mut s = one();
        s.play(0, 0, 1.0, CLIP, 0x80, None, with_triggers(vec![list("A", &[0.1, 0.6])]), None, None);
        s.set_frame(0, 9.0, true);
        let mut ended = Vec::new();
        let fired = step(&mut s, 1.0, &mut ended);
        assert_eq!(fired.len(), 1);
        // without skipping, a trigger already behind the playhead fires at once, late
        let mut s = one();
        s.play(0, 0, 1.0, CLIP, 0x80, None, with_triggers(vec![list("A", &[0.1])]), None, None);
        s.set_frame(0, 9.0, false);
        let fired = step(&mut s, 0.05, &mut ended);
        assert_eq!(fired.len(), 1);
        assert!((fired[0].late - 0.2).abs() < 1e-5, "{}", fired[0].late);
    }

    #[test]
    fn blending_layers_share_one_firing() {
        // two layers blending the same list: one entry per time, fired once, at the weighted frame
        let mut s = State::default();
        let (a, b) = (s.add_layer(), s.add_layer());
        for l in [a, b] {
            s.layers[l].mode = 2;
        }
        s.layers[a].weight = 1.0;
        let (ia, ib) = (s.layers[a].channels[0].id, s.layers[b].channels[0].id);
        s.play(ia, flag::LOOP, 1.0, CLIP, 0x80, None, with_triggers(vec![list("STEP", &[0.4])]), None, None);
        s.play(ib, flag::LOOP, 1.0, CLIP, 0x80, None, with_triggers(vec![list("step", &[0.2])]), None, None);
        let set = s.triggers.sets.iter().flatten().next().unwrap();
        assert_eq!(set.entries.len(), 1);
        // layer b at weight 0 leaves the time on a's frame 12; at 0.5 it moves half way to b's frame 6
        assert_eq!(set.entries[0].frame, 12.0);
        s.set_weight(b, 0.5);
        assert_eq!(s.triggers.sets.iter().flatten().next().unwrap().entries[0].frame, 9.0);
        let mut ended = Vec::new();
        assert_eq!(step(&mut s, 0.35, &mut ended).len(), 1);
        // with a third time it cannot pair, the unmatched one is its own entry
        let mut s2 = State::default();
        let (a, b) = (s2.add_layer(), s2.add_layer());
        s2.layers[a].mode = 2;
        s2.layers[b].mode = 2;
        let (ia, ib) = (s2.layers[a].channels[0].id, s2.layers[b].channels[0].id);
        s2.play(ia, 0, 1.0, CLIP, 0x80, None, with_triggers(vec![list("S", &[0.4])]), None, None);
        s2.play(ib, 0, 1.0, CLIP, 0x80, None, with_triggers(vec![list("S", &[0.1, 0.45])]), None, None);
        let set = s2.triggers.sets.iter().flatten().next().unwrap();
        assert_eq!(set.entries.iter().map(|e| e.frame).collect::<Vec<_>>(), [3.0, 12.0]);
        // and a layer that does not blend just adds its times
        let mut s3 = State::default();
        let (a, b) = (s3.add_layer(), s3.add_layer());
        let (ia, ib) = (s3.layers[a].channels[0].id, s3.layers[b].channels[0].id);
        s3.play(ia, 0, 1.0, CLIP, 0x80, None, with_triggers(vec![list("S", &[0.4])]), None, None);
        s3.play(ib, 0, 1.0, CLIP, 0x80, None, with_triggers(vec![list("S", &[0.1])]), None, None);
        assert_eq!(s3.triggers.sets.iter().flatten().next().unwrap().entries.len(), 2);
    }
}
