//! Triggers (`Trigger.cpp`, `docs/ANIMATION.md` "Triggers"): the timed events a clip carries (its `.btf` lists).
//! When a clip starts on a channel, each of its lists joins the state's trigger manager (`CITriggerManager`,
//! control `+0x30`) as a set of entries, one per fire time, holding the frame it fires at. The step splits its
//! time at the next entry (`0x416680`) as it does at clip ends, and fires the entries it reaches (`0x4153c0`).
//!
//! Lists with the same name and node share a set (`0x4167b0` gives them one id). When a list joins a set that
//! already has entries and the channel's layer blends (`mode & 0xb`), its times are matched to the entries
//! (`0x4158e0`) and each entry fires once, at a time blended over the layers like the pose (`0x4150f0`); otherwise
//! the times are simply added. Only the cutscenes' clips have `.btf` files.

use crate::play::{flag, Channel, ClipInfo, Layer};

/// A clip's trigger list (`.btf`, read by `0x40a370`): a named event and the times it fires.
#[derive(Debug, Clone, PartialEq)]
pub struct TriggerList {
    pub name: Option<String>,
    /// The node it fires at (`+8`, bound to the model's node of that name by `0x415750`; none on the disc).
    pub node: Option<u32>,
    /// Seconds into the clip.
    pub times: Vec<f32>,
}

/// `[0x669294]`: the `ON_AUDIO_` / `OFF_AUDIO_` names whose times move by the `AudioTriggerOffset` tweak.
const AUDIO_PREFIXES: [&str; 7] = ["SPEECH", "INTRO", "OUTRO", "U", "WINs", "CATCHSNI", "CHLANGE"];

impl TriggerList {
    /// One clip's lists from its `.btf` triggers. `audio_offset` is `AudioTriggerOffset` (`[0xf919a0]`, 0 unless
    /// tweaked), added to every time of the speech lists (`0x4167b0`); `bind` names the model's node.
    pub fn from_btf(
        triggers: &[qwc_formats::btf::Trigger],
        audio_offset: f32,
        bind: impl Fn(&str) -> Option<u32>,
    ) -> Vec<TriggerList> {
        triggers
            .iter()
            .map(|t| {
                let speech = t.name.as_deref().is_some_and(|n| {
                    let rest = n.strip_prefix("ON_AUDIO_").or_else(|| n.strip_prefix("OFF_AUDIO_"));
                    rest.is_some_and(|r| AUDIO_PREFIXES.iter().any(|p| r.starts_with(p)))
                });
                let times = t.times.iter().map(|&s| if speech { audio_offset + s } else { s }).collect();
                TriggerList { name: t.name.clone(), node: t.node.as_deref().and_then(&bind), times }
            })
            .collect()
    }

    /// What makes lists share a set (`0x4167b0`): the node and the name, case not counted.
    pub fn key(&self) -> Key {
        (self.name.as_ref().map(|n| n.to_ascii_lowercase()), self.node)
    }
}

/// A set's identity: the lower-cased name and the node.
pub type Key = (Option<String>, Option<u32>);

/// One list time on one channel (0x18 bytes).
#[derive(Debug, Clone, PartialEq)]
struct Instance {
    /// `+0` bit 0: freeze the channel when it fires (`0x416510`).
    stop: bool,
    /// Seconds into the clip.
    time: f32,
    layer: usize,
    channel: u32,
    clip: ClipInfo,
}

impl Instance {
    /// `0x414a70`: the frame it fires at.
    fn frame(&self) -> f32 {
        self.time * self.clip.fps + self.clip.first as f32
    }
}

/// An entry (12 bytes): one firing.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Bit 0: fired (until the clip loops or a seek goes back over it).
    pub fired: bool,
    /// Bit 1: reached in the slice being run.
    pub due: bool,
    /// The frame it fires at, in its first instance's channel.
    pub frame: f32,
    /// In layer order (`0x414ae0`).
    instances: Vec<Instance>,
}

/// A set (0x1c bytes): a list's entries over every channel playing it.
#[derive(Debug, Clone, PartialEq)]
pub struct Set {
    pub key: Key,
    pub name: Option<String>,
    pub node: Option<u32>,
    pub entries: Vec<Entry>,
}

/// A trigger fired: the event the default handler posts (`0x417ce0`: the name and the owner, to the event router
/// `[0x1043ba8]`), with what else the library passes it.
#[derive(Debug, Clone, PartialEq)]
pub struct Fired {
    pub name: Option<String>,
    pub node: Option<u32>,
    /// The channel of the entry's first instance.
    pub channel: u32,
    /// Seconds since the entry's frame was passed (`-(frame - entry) / (fps × speed)`).
    pub late: f32,
}

/// The state's channels as the triggers see them.
#[derive(Clone, Copy)]
pub(crate) struct View<'a> {
    pub layers: &'a [Layer],
    pub order: &'a [usize],
}

impl View<'_> {
    fn channel(&self, id: u32) -> Option<&Channel> {
        self.layers.get((id >> 18 & 0x7f) as usize)?.channels.get((id >> 25) as usize)
    }

    fn frame(&self, id: u32) -> f32 {
        self.channel(id).map_or(0.0, |c| c.frame)
    }

    /// The front clip's speed. **[I]** an instance outlives its channel's queue only through misuse; 1 then.
    fn speed(&self, id: u32) -> f32 {
        self.channel(id).and_then(|c| c.queue.first()).map_or(1.0, |e| e.speed)
    }

    fn position(&self, layer: usize) -> usize {
        self.order.iter().position(|&l| l == layer).unwrap_or(usize::MAX)
    }
}

/// The trigger manager.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Triggers {
    /// Freed sets leave a hole the next new set fills (`id -1`).
    pub sets: Vec<Option<Set>>,
}

impl Triggers {
    /// `0x416370` -> `0x4158e0`: `list` of the clip starting on `channel` joins the manager.
    pub(crate) fn install(
        &mut self,
        v: View,
        list: &TriggerList,
        layer: usize,
        channel: u32,
        clip: ClipInfo,
        no_blend: bool,
    ) {
        let key = list.key();
        let inst = |time| Instance { stop: false, time, layer, channel, clip };
        let Some(set) = self.sets.iter_mut().flatten().find(|s| s.key == key) else {
            let entries = list
                .times
                .iter()
                .map(|&t| {
                    let i = inst(t);
                    Entry { fired: false, due: false, frame: i.frame(), instances: vec![i] }
                })
                .collect();
            let set = Set { key, name: list.name.clone(), node: list.node, entries };
            match self.sets.iter_mut().find(|s| s.is_none()) {
                Some(hole) => *hole = Some(set),
                None => self.sets.push(Some(set)),
            }
            return;
        };
        let mode = v.layers.get(layer).map_or(0, |l| l.mode);
        let (n_new, n_old) = (list.times.len(), set.entries.len());
        let mut matched: Vec<Option<usize>> = vec![None; n_new];
        if mode & 0xb != 0 && !no_blend {
            if n_new == n_old {
                for (k, &t) in list.times.iter().enumerate() {
                    attach(set, k, inst(t), v);
                }
                sort(set);
                return;
            }
            matched = match_times(set, list, channel, clip, v);
            for k in (0..n_new).rev() {
                if let Some(e) = matched[k] {
                    attach(set, e, inst(list.times[k]), v);
                }
            }
        }
        // `0x414d10`: the unmatched times become entries, each before the first entry not earlier
        for (k, &t) in list.times.iter().enumerate() {
            if matched[k].is_some() {
                continue;
            }
            let i = inst(t);
            let f = i.frame();
            let at = set.entries.iter().position(|e| f <= e.frame).unwrap_or(set.entries.len());
            set.entries.insert(at, Entry { fired: false, due: false, frame: f, instances: vec![i] });
        }
        sort(set);
    }

    /// `0x4161e0`: `channel` leaves the sets of `key`; empty entries go, an empty set is freed, and the others'
    /// times are blended again (`0x4152a0`).
    pub(crate) fn remove(&mut self, v: View, key: &Key, channel: u32) {
        for slot in &mut self.sets {
            let Some(set) = slot.as_mut().filter(|s| &s.key == key) else { continue };
            for e in &mut set.entries {
                e.instances.retain(|i| i.channel != channel);
            }
            set.entries.retain(|e| !e.instances.is_empty());
            if set.entries.is_empty() {
                *slot = None;
            } else {
                recompute_all(set, v);
            }
        }
    }

    /// `0x416460`: a looping clip wraps; entries led by `channel` may fire again.
    pub(crate) fn reset(&mut self, channel: u32) {
        for e in self.entries_mut().filter(|e| e.instances[0].channel == channel) {
            e.fired = false;
            e.due = false;
        }
    }

    /// `0x4164d0` -> `0x4154a0`: `channel` jumps from frame `from` to `to`. Entries it skips forward over count as
    /// fired; those it goes back over may fire again.
    pub(crate) fn seek(&mut self, channel: u32, from: f32, to: f32) {
        for e in self.entries_mut().filter(|e| e.instances[0].channel == channel) {
            let f = e.frame;
            if from < to && from <= f && f <= to {
                e.fired = true;
            } else if to < from && to <= f && f <= from {
                e.fired = false;
                e.due = false;
            }
        }
    }

    fn entries_mut(&mut self) -> impl Iterator<Item = &mut Entry> {
        self.sets.iter_mut().flatten().flat_map(|s| &mut s.entries)
    }

    /// `0x416680`: seconds to the next entry not fired (never below 0), `f32::MAX` with none. An entry's time runs
    /// on its first instance's channel.
    pub(crate) fn next_time(&self, v: View) -> f32 {
        let mut min = f32::MAX;
        for e in self.sets.iter().flatten().flat_map(|s| &s.entries).filter(|e| !e.fired) {
            let t = time_to(e, v);
            if t < min {
                min = t;
            }
        }
        min
    }

    /// `0x411fe0`'s second pass: entries at the earliest time `min`, when it is within `left`, are due.
    pub(crate) fn mark_due(&mut self, v: View, min: f32, left: f32) {
        if min >= f32::MAX {
            return;
        }
        for set in self.sets.iter_mut().flatten() {
            for e in set.entries.iter_mut().filter(|e| !e.fired) {
                let t = time_to(e, v);
                if t == min && t <= left {
                    e.due = true;
                }
            }
        }
    }

    /// `0x4153c0` per set, `0x4152f0` per entry: the due entries fire, in set and time order. A stop instance
    /// freezes its channel (`freeze`).
    pub(crate) fn fire(&mut self, v: View, out: &mut Vec<Fired>, freeze: &mut Vec<u32>) {
        for set in self.sets.iter_mut().flatten() {
            for e in &mut set.entries {
                if e.fired || !e.due {
                    continue;
                }
                for i in &mut e.instances {
                    if i.stop {
                        if v.channel(i.channel).is_some_and(|c| !c.queue.is_empty()) {
                            freeze.push(i.channel);
                        }
                        i.stop = false;
                    }
                }
                let lead = &e.instances[0];
                let fps = v.channel(lead.channel).and_then(|c| c.queue.first()).map_or(lead.clip.fps, |q| q.clip.fps);
                let late = -((e.frame - v.frame(lead.channel)) / (fps * v.speed(lead.channel)));
                e.fired = true;
                out.push(Fired { name: set.name.clone(), node: set.node, channel: lead.channel, late });
                e.due = false;
            }
        }
    }

    /// `0x416510`: the set of that name (case not counted), or else the one of `key`, gets the entry not fired
    /// nearest `frame` ahead of it (after it going `forward`, before it otherwise) marked to freeze `channel` when
    /// it fires. Answers the set's key.
    pub(crate) fn stop_at(
        &mut self,
        name: Option<&str>,
        key: Option<&Key>,
        channel: u32,
        frame: f32,
        forward: bool,
    ) -> Option<Key> {
        let set = self.sets.iter_mut().flatten().find(|s| {
            name.is_some_and(|n| s.name.as_deref().is_some_and(|m| m.eq_ignore_ascii_case(n))) || key == Some(&s.key)
        })?;
        let mut best: Option<(usize, f32)> = None;
        for (k, e) in set.entries.iter().enumerate() {
            let ahead = if forward { frame < e.frame } else { e.frame < frame };
            if !e.fired && ahead && (e.frame - frame).abs() < best.map_or(f32::MAX, |b| b.1) {
                best = Some((k, (e.frame - frame).abs()));
            }
        }
        let (k, _) = best?;
        if let Some(i) = set.entries[k].instances.iter_mut().find(|i| i.channel == channel) {
            i.stop = true;
        }
        Some(set.key.clone())
    }

    /// `0x416640` -> `0x414ae0`: the layers were reordered; every entry's instances follow.
    pub(crate) fn reorder(&mut self, v: View) {
        for e in self.entries_mut() {
            e.instances.sort_by_key(|i| v.position(i.layer));
        }
    }

    /// `0x4163d0` -> `0x415420`: a layer's weight changed; the entries it plays in are blended again.
    pub(crate) fn reweigh(&mut self, v: View, layer: usize) {
        for set in self.sets.iter_mut().flatten() {
            let mut changed = false;
            for k in 0..set.entries.len() {
                if set.entries[k].instances.iter().any(|i| i.layer == layer) {
                    changed |= recompute(set, k, true, v);
                }
            }
            if changed {
                sort(set);
            }
        }
    }
}

/// `0x416680`'s time for one entry: `((frame - channel frame) - first) / speed / fps`, at least 0.
fn time_to(e: &Entry, v: View) -> f32 {
    let i = &e.instances[0];
    let t = (e.frame - v.frame(i.channel) - i.clip.first as f32) / v.speed(i.channel) / i.clip.fps;
    if t <= 0.0 {
        0.0
    } else {
        t
    }
}

/// `0x415000` passes: entries by frame, equals keeping their order.
fn sort(set: &mut Set) {
    set.entries.sort_by(|a, b| a.frame.partial_cmp(&b.frame).unwrap_or(std::cmp::Ordering::Equal));
}

/// `0x414c10`: an instance joins entry `k` in layer order, and the entry's time is blended again.
fn attach(set: &mut Set, k: usize, inst: Instance, v: View) {
    let p = v.position(inst.layer);
    let ins = &mut set.entries[k].instances;
    let at = ins.iter().position(|i| p < v.position(i.layer)).unwrap_or(ins.len());
    ins.insert(at, inst);
    recompute(set, k, true, v);
}

/// `0x4152a0`: every entry blended again, re-sorted if any moved.
fn recompute_all(set: &mut Set, v: View) {
    let mut changed = false;
    for k in 0..set.entries.len() {
        changed |= recompute(set, k, true, v);
    }
    if changed {
        sort(set);
    }
}

/// `0x4150f0`: entry `k`'s frame blended over the layers in order. Each instance's frame is carried into the first
/// instance's channel (by the frames' distance from each channel's playhead and the speeds' ratio); a layer of
/// mode bit 1 blends it in by its weight, bit 3 by its weight for the set's node, anything else sets it, and a
/// layer of mode bit 0 makes the next instance set it. With `rearm`, a fired entry the blend moved more than a
/// tenth of a second ahead of the playhead may fire again. Answers whether the frame moved.
fn recompute(set: &mut Set, k: usize, rearm: bool, v: View) -> bool {
    let node = set.node;
    let e = &mut set.entries[k];
    let before = e.frame;
    let mut sorted: Vec<&Instance> = e.instances.iter().collect();
    sorted.sort_by_key(|i| v.position(i.layer));
    let mut next = sorted.into_iter().peekable();
    let (mut base_frame, mut base_speed, mut first, mut over) = (0.0f32, 0.0f32, true, false);
    for &l in v.order {
        let layer = &v.layers[l];
        if layer.mode & 1 != 0 {
            over = true;
        }
        let Some(&i) = next.peek() else { break };
        if i.layer != l {
            continue;
        }
        next.next();
        let mut t = i.frame();
        let speed = v.speed(i.channel);
        if first {
            base_frame = v.frame(i.channel);
            base_speed = speed;
            first = false;
        } else {
            t = base_speed / speed * (t - v.frame(i.channel)) + base_frame;
        }
        if layer.mode & 2 != 0 && !over {
            e.frame = layer.weight * t + (1.0 - layer.weight) * e.frame;
        } else if layer.mode & 8 != 0 && !over {
            // **[I]** a set without a node reads the layer's weight
            let w = node.and_then(|n| layer.bone_weights.as_ref()?.get(n as usize).copied()).unwrap_or(layer.weight);
            e.frame = w * t + (1.0 - w) * e.frame;
        } else {
            e.frame = t;
            over = false;
        }
    }
    if e.frame == before {
        return false;
    }
    if rearm {
        let tol = e.instances[0].clip.fps * 0.1;
        let moved = if base_speed < 0.0 { e.frame < base_frame - tol } else { base_frame + tol < e.frame };
        if e.fired && moved {
            e.fired = false;
        }
    }
    true
}

/// `0x4158e0`'s matching of a list's times to a set's entries when their counts differ: the times of the side
/// with fewer take, in turn, the nearest free one of the other side, by frame distance once the new times are
/// carried into each entry's channel. As in the original, a nearer one already taken is taken over only when its
/// holder is nearer to it still. Answers each new time's entry.
fn match_times(set: &Set, list: &TriggerList, channel: u32, clip: ClipInfo, v: View) -> Vec<Option<usize>> {
    let (n_new, n_old) = (list.times.len(), set.entries.len());
    let new_is_large = n_old < n_new;
    let (small, large) = if new_is_large { (n_old, n_new) } else { (n_new, n_old) };
    let dist = |new: usize, old: usize| {
        let e = &set.entries[old];
        let lead = e.instances[0].channel;
        let f = list.times[new] * clip.fps + clip.first as f32;
        let carried = (f - v.frame(channel)) * (v.speed(lead) / v.speed(channel)) + v.frame(lead);
        (carried - e.frame).abs()
    };
    let mut best = vec![f32::MAX; small];
    let mut of_small: Vec<Option<usize>> = vec![None; small];
    let mut of_large: Vec<Option<usize>> = vec![None; large];
    let mut held = 0;
    // **[I]** the original loops until every one of the fewer side holds one; the cap stops a cycle it could fall
    // into
    for _ in 0..small * large + 1 {
        if held >= small {
            break;
        }
        for i in 0..small {
            if of_small[i].is_some() {
                continue;
            }
            for j in 0..large {
                let d = if new_is_large { dist(j, i) } else { dist(i, j) };
                if d >= best[i] {
                    continue;
                }
                if let Some(h) = of_large[j] {
                    if best[h] >= d {
                        continue;
                    }
                    of_small[h] = None;
                    best[h] = f32::MAX;
                    held -= 1;
                }
                match of_small[i] {
                    Some(old) => of_large[old] = None,
                    None => held += 1,
                }
                of_small[i] = Some(j);
                of_large[j] = Some(i);
                best[i] = d;
            }
        }
    }
    (0..n_new).map(|k| if new_is_large { of_large[k] } else { of_small[k] }).collect()
}

/// What the step needs to know of a channel being frozen by a stop trigger.
pub(crate) fn freeze(c: &mut Channel) {
    if let Some(e) = c.queue.first_mut() {
        e.flags |= flag::FROZEN;
    }
}
