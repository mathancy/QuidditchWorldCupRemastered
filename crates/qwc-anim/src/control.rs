//! A character's animation control (`AnimationObjectControl`, `docs/ANIMATION.md`, "The character's slots"):
//! slots that play clips on pooled records, each record a library layer with one channel, cross-fading between
//! them; and the character's update (`0x468750`), which weights slots 0 and 1 and runs the library.
//!
//! Times: the slots count milliseconds, the library seconds (`0x468750` passes `dt × 0.001`).

use std::collections::VecDeque;
use std::sync::Arc;

use crate::play::{ClipData, ClipInfo, Ended, State};
use crate::pose::Pose;
use crate::trigger::{Fired, TriggerList};
use crate::Clip;

/// The handle no clip has (`[0x668670]`).
pub const NONE: u32 = 0xffff_ffff;
/// `[0x605ec8]`: how long a mode-0 play cross-fades, in milliseconds.
pub const CROSS_FADE_MS: i32 = 500;

/// A clip a set holds, by name (`.fas` clip order).
#[derive(Debug, Clone, PartialEq)]
pub struct SetClip {
    pub name: Option<String>,
    pub clip: ClipInfo,
    /// Its tracks and trigger lists, when the host decoded them.
    pub data: Option<Arc<ClipData>>,
}

/// A `.fas`'s clips as the library binds them: the base part's first frame, last frame and rate
/// (`FUN_0040a4f0`).
pub fn set_clips(fas: &qwc_formats::fas::Fas) -> Vec<SetClip> {
    fas.anims
        .iter()
        .map(|a| SetClip {
            name: a.name.clone(),
            clip: ClipInfo { first: a.unk0 as i32, last: a.frames as i32, fps: a.fps },
            data: None,
        })
        .collect()
}

/// [`set_clips`] with each clip's tracks decoded and, from the set's `.btf`, its trigger lists (bound to the
/// clip of the same name, case not counted: `0x415750`).
pub fn decode_set(
    fas: &qwc_formats::fas::Fas,
    btf: Option<&qwc_formats::btf::Btf>,
    sfa: Option<&qwc_formats::fas::Sfa>,
) -> qwc_formats::Result<Vec<SetClip>> {
    let mut out = set_clips(fas);
    for (i, c) in out.iter_mut().enumerate() {
        let triggers = btf
            .and_then(|b| {
                let name = c.name.as_deref()?;
                b.anims.iter().find(|(n, _)| n.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name)))
            })
            .map_or(vec![], |(_, t)| TriggerList::from_btf(t, 0.0, |_| None));
        c.data = Some(Arc::new(ClipData { anim: Some(Clip::decode(fas, i, sfa)?), triggers }));
    }
    Ok(out)
}

/// A callback waiting for a clip's end (a slot's 12-byte entries at `+0xc`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndCallback {
    /// The port's token for the callback object.
    pub token: u32,
    /// Only for the clip of `handle` (`+0x10` bit 0; the clip bits `0x3ff80` are compared).
    pub match_clip: bool,
    pub handle: u32,
}

/// A pooled record (0x2c bytes): one library layer.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    /// Its layer in the state.
    pub layer: usize,
    /// The slot it was last given to (`+8`, never cleared).
    pub slot: Option<usize>,
    /// The front clip's handle, from the last refresh (`+0x10`, `0x411120`).
    pub handle: u32,
    /// Clips queued on its channel at the last refresh (`+0x1c`).
    pub queued: usize,
    /// `+0x24`
    pub weight: f32,
    /// `+0x28`: 1 fading in, 2 fading out, 0 neither.
    pub fade: u32,
}

/// A slot (0x98 bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct Slot {
    /// `+0x6c`: 1 idle, 2 started, 3 cross-fading, 4 settled.
    pub state: u32,
    /// Its records, oldest first (`+0x70` list).
    pub records: Vec<usize>,
    /// `+0x7c`: the most records it may hold (0 = no limit).
    pub max: usize,
    /// `+0x80`, `+0x84`: the cross-fade's milliseconds left and `1 / length`.
    pub frames: i32,
    pub rate: f32,
    /// `+0x88`: the mode its records' layers get.
    pub mode: u32,
    /// `+0x8c`
    pub weight: f32,
    /// `+0x90`, `+0x94`: fading the slot in (1) or out (2, 3 releasing at the end), and the rate per ms.
    pub fade: u32,
    pub fade_rate: f32,
    pub ends: [Option<EndCallback>; 8],
}

impl Default for Slot {
    /// `0x4791c0` and `0x47a0e0`: idle, mode 1, weight 1.
    fn default() -> Slot {
        Slot {
            state: 1,
            records: Vec::new(),
            max: 0,
            frames: 0,
            rate: 0.0,
            mode: 1,
            weight: 1.0,
            fade: 0,
            fade_rate: 0.0,
            ends: [None; 8],
        }
    }
}

/// What a step told its listeners.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A clip ended and a slot callback waits for it: `(*cb)(slot, handle, 1, flags)` (`0x479e20`).
    End { slot: usize, token: u32, handle: u32, flags: u32 },
    /// A trigger fired (`0x417ce0` posts its name to the event router; the owner is the control's).
    Trigger(Fired),
    /// The slot's own callback object (`+8`): 0 started, 1 cross-fade done, 2 faded in, 3 faded out.
    Slot { slot: usize, code: u32 },
}

/// How [`Control::play`] starts a clip (`0x468580`'s mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 0: cross-fade to it over [`CROSS_FADE_MS`] on a new record (`0x47a2c0`).
    CrossFade,
    /// 1: queue it after the slot's newest clip (an idle slot starts it at once).
    Queue,
    /// 2: cut to it (`0x4795d0`).
    Cut,
}

impl Mode {
    pub fn from_word(w: u32) -> Option<Mode> {
        Some(match w {
            0 => Mode::CrossFade,
            1 => Mode::Queue,
            2 => Mode::Cut,
            _ => return None,
        })
    }
}

/// The control: the library state, the record pool and the slots.
#[derive(Debug, Clone, PartialEq)]
pub struct Control {
    pub state: State,
    /// The anim sets bound, in bind order (`control +8`).
    pub sets: Vec<Vec<SetClip>>,
    pub records: Vec<Record>,
    free: VecDeque<usize>,
    pub slots: Vec<Slot>,
    /// The model's nodes, when one is bound (`+0xc`): the pose is evaluated into them after every step.
    pub pose: Option<Pose>,
}

impl Control {
    /// `0x47a0e0`: `slots` slots and `slots × per_slot` records (characters and flyers: 13 and 2, `0x46bd16`).
    pub fn new(slots: usize, per_slot: usize) -> Control {
        let mut state = State::default();
        let records: Vec<Record> = (0..slots * per_slot)
            .map(|_| Record { layer: state.add_layer(), slot: None, handle: NONE, queued: 0, weight: 0.0, fade: 0 })
            .collect();
        Control {
            state,
            sets: Vec::new(),
            free: (0..records.len()).collect(),
            records,
            slots: vec![Slot::default(); slots],
            pose: None,
        }
    }

    /// `0x40aeb0`: the first clip of that name (case not counted, `_stricmp`) in the sets in bind order. The
    /// handle's layer and channel bits are all set.
    pub fn find(&self, name: &str) -> Option<(u32, ClipInfo)> {
        self.sets.iter().enumerate().find_map(|(s, set)| {
            set.iter()
                .position(|c| c.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name)))
                .map(|k| ((k as u32 & 0x7ff) << 7 | 0xfffc_0000 | s as u32 & 0x7f, set[k].clip))
        })
    }

    fn clip(&self, handle: u32) -> Option<&SetClip> {
        self.sets.get((handle & 0x7f) as usize)?.get((handle >> 7 & 0x7ff) as usize)
    }

    fn channel_id(&self, r: usize) -> u32 {
        self.state.layers[self.records[r].layer].channels[0].id
    }

    /// `0x468580`: play the named clip in a slot; with a callback token, slot callback `sub` waits for this clip's
    /// end. Answers the handle ([`NONE`] when the name is not found). `start` is the frame the channel jumps to
    /// (`0x4784b0`, clamped to its front clip). An unknown mode plays nothing but still registers the callback.
    #[allow(clippy::too_many_arguments)]
    pub fn play(
        &mut self,
        name: &str,
        flags: u32,
        mode: Option<Mode>,
        slot: usize,
        callback: Option<(u32, usize)>,
        speed: f32,
        start: f32,
    ) -> u32 {
        let Some((handle, _)) = self.find(name) else { return NONE };
        if slot >= self.slots.len() {
            return handle;
        }
        match mode {
            None => {}
            Some(Mode::Cut) => self.cut(slot, handle, flags, speed, start),
            Some(Mode::Queue) => {
                let r = if self.slots[slot].state == 1 {
                    self.release_all(slot);
                    let r = self.take(slot);
                    if let Some(r) = r {
                        self.attach(slot, r);
                        self.records[r].weight = 1.0;
                        self.records[r].fade = 0;
                        self.state.layers[self.records[r].layer].mode = self.slots[slot].mode;
                    }
                    self.slots[slot].state = 2;
                    r
                } else {
                    self.slots[slot].records.last().copied()
                };
                if let Some(r) = r {
                    self.play_on(r, handle, flags, speed, start);
                }
            }
            Some(Mode::CrossFade) => self.cross_fade(slot, handle, flags, speed, CROSS_FADE_MS, start),
        }
        if let Some((token, sub)) = callback {
            if let Some(e) = self.slots[slot].ends.get_mut(sub) {
                *e = Some(EndCallback { token, match_clip: true, handle });
            }
        }
        handle
    }

    /// `0x479170`, or failing that the slot's lightest record released and taken (`0x47a030`).
    fn take(&mut self, slot: usize) -> Option<usize> {
        self.free.pop_front().or_else(|| {
            self.release_lightest(slot);
            self.free.pop_front()
        })
    }

    /// `0x479f50`: the record joins the slot (its layer just before the next slot's first, `0x47a2f0`).
    fn attach(&mut self, slot: usize, r: usize) {
        let before = self.slots[slot + 1..].iter().find_map(|s| s.records.first()).map(|&k| self.records[k].layer);
        self.state.move_before(self.records[r].layer, before);
        self.slots[slot].records.push(r);
        self.records[r].slot = Some(slot);
    }

    /// `0x479fb0`: the record's channel stops (no callbacks) and it returns to the pool.
    fn release(&mut self, slot: usize, r: usize) {
        self.state.stop(self.channel_id(r));
        self.slots[slot].records.retain(|&k| k != r);
        self.free.push_back(r);
    }

    /// `0x479500`: every record released; the slot is idle.
    fn release_all(&mut self, slot: usize) {
        for r in self.slots[slot].records.clone() {
            self.release(slot, r);
        }
        self.slots[slot].state = 1;
    }

    /// `0x47a030`: the record of least weight (the first of equals) is released.
    fn release_lightest(&mut self, slot: usize) {
        let mut best: Option<(usize, f32)> = None;
        for &r in &self.slots[slot].records {
            if self.records[r].weight < best.map_or(1e8, |b| b.1) {
                best = Some((r, self.records[r].weight));
            }
        }
        if let Some((r, _)) = best {
            self.release(slot, r);
        }
    }

    /// `0x4795d0`: everything released, the clip on a fresh record at full weight.
    fn cut(&mut self, slot: usize, handle: u32, flags: u32, speed: f32, start: f32) {
        self.release_all(slot);
        if let Some(r) = self.take(slot) {
            self.attach(slot, r);
            self.records[r].weight = 1.0;
            self.records[r].fade = 0;
            self.state.layers[self.records[r].layer].mode = self.slots[slot].mode;
            self.slots[slot].state = 2;
            self.play_on(r, handle, flags, speed, start);
        } else {
            self.slots[slot].state = 2;
        }
    }

    /// `0x479690`: a new record fades in over `ms` (in mode 4 the others fade out; otherwise they stay under it).
    fn cross_fade(&mut self, slot: usize, handle: u32, flags: u32, speed: f32, ms: i32, start: f32) {
        for r in self.slots[slot].records.clone() {
            if self.records[r].weight == 0.0 {
                self.release(slot, r);
            }
        }
        let s = &self.slots[slot];
        let r = if s.max != 0 && s.max <= s.records.len() { None } else { self.free.pop_front() };
        let Some(r) = r.or_else(|| {
            self.release_lightest(slot);
            self.free.pop_front()
        }) else {
            return;
        };
        let out = if self.slots[slot].mode == 4 { 2 } else { 0 };
        for &k in &self.slots[slot].records {
            self.records[k].fade = out;
        }
        self.attach(slot, r);
        self.records[r].weight = 0.0;
        self.records[r].fade = 1;
        let mode = self.slots[slot].mode;
        let l = self.records[r].layer;
        self.state.set_weight(l, 0.0);
        self.state.layers[l].mode = if mode == 1 { 2 } else { mode };
        let s = &mut self.slots[slot];
        s.frames = ms;
        s.rate = 1.0 / (ms as u32 as f32);
        s.state = 3;
        self.play_on(r, handle, flags, speed, start);
    }

    /// `0x479ed0` -> `0x4789a0`: the clip goes on the record's channel (queued behind what plays there), the
    /// channel jumps to `start` within its front clip (`0x4784b0`), and the record's view is refreshed.
    fn play_on(&mut self, r: usize, handle: u32, flags: u32, speed: f32, start: f32) {
        let Some(c) = self.clip(handle) else { return };
        let (clip, data) = (c.clip, c.data.clone());
        let id = self.channel_id(r);
        self.state.play(id, flags, speed, clip, handle, Some(r as u32), data, None, self.pose.as_mut());
        self.state.set_frame(id, start, flags & crate::play::flag::SKIP_TRIGGERS != 0);
        self.refresh(r);
    }

    /// `0x411120`: the record's view of its channel.
    fn refresh(&mut self, r: usize) {
        let c = &self.state.layers[self.records[r].layer].channels[0];
        self.records[r].queued = c.queue.len();
        self.records[r].handle = c.queue.first().map_or(NONE, |e| e.handle);
    }

    /// `0x479530`: the slot's mode; its first record's layer takes it, the others 2 for mode 1.
    pub fn set_mode(&mut self, slot: usize, mode: u32) {
        self.slots[slot].mode = mode;
        for (n, &r) in self.slots[slot].records.iter().enumerate() {
            let m = if n == 0 || mode != 1 { mode } else { 2 };
            self.state.layers[self.records[r].layer].mode = m;
        }
    }

    /// `0x47a270`: fade the slot in over `ms` milliseconds.
    pub fn fade_in(&mut self, slot: usize, ms: i32) {
        let s = &mut self.slots[slot];
        s.fade = 1;
        s.fade_rate = 1.0 / (ms as u32 as f32);
        self.set_mode(slot, 2);
    }

    /// `0x4725a0`'s fade-out branch: the slot fades out over `ms` milliseconds in mode 2 (its records stay).
    pub fn fade_out(&mut self, slot: usize, ms: i32) {
        let s = &mut self.slots[slot];
        s.fade = 2;
        s.fade_rate = 1.0 / (ms as u32 as f32);
        self.set_mode(slot, 2);
    }

    /// `0x479840`: the speed of the clip at the front of the slot's newest record's channel.
    pub fn set_speed(&mut self, slot: usize, speed: f32) {
        let Some(&r) = self.slots[slot].records.last() else { return };
        let l = self.records[r].layer;
        if let Some(e) = self.state.layers[l].channels[0].queue.first_mut() {
            e.speed = speed;
        }
    }

    /// The clip of `name`'s length in seconds (its library clip's, `0x478540`).
    pub fn seconds(&self, name: &str) -> Option<f32> {
        self.find(name).map(|(_, c)| c.seconds())
    }

    /// `0x479240`: one slot's fade and records for `dt` milliseconds.
    fn update_slot(&mut self, slot: usize, dt: f32, events: &mut Vec<Event>) {
        let s = &self.slots[slot];
        match s.fade {
            1 => {
                let mut v = dt * s.fade_rate + s.weight;
                if 1.0 <= v {
                    v = 1.0;
                    self.set_mode(slot, 1);
                    self.slots[slot].fade = 0;
                    events.push(Event::Slot { slot, code: 2 });
                }
                self.slots[slot].weight = v;
            }
            2 | 3 => {
                let mut v = s.weight - dt * s.fade_rate;
                if v <= 0.0 {
                    if s.fade == 3 {
                        self.release_all(slot);
                    }
                    v = 0.0;
                    self.slots[slot].fade = 0;
                    events.push(Event::Slot { slot, code: 3 });
                }
                self.slots[slot].weight = v;
            }
            _ => {}
        }
        self.update_records(slot, dt);
        let s = &mut self.slots[slot];
        if s.state == 2 {
            s.state = 4;
            s.frames = 0;
            events.push(Event::Slot { slot, code: 0 });
        } else if s.state == 3 {
            let d = layer_ftol(dt);
            s.frames = if s.frames < d { 0 } else { s.frames - d };
            let newest = *s.records.last().expect("a cross-fade has records");
            if 1.0 <= self.records[newest].weight {
                for r in self.slots[slot].records.clone() {
                    if r != newest {
                        self.release(slot, r);
                    }
                }
                self.state.layers[self.records[newest].layer].mode = self.slots[slot].mode;
                let s = &mut self.slots[slot];
                s.state = 4;
                s.frames = 0;
                events.push(Event::Slot { slot, code: 1 });
            }
        }
    }

    /// `0x479ca0`: records fade towards 1 or 0 by `dt / length`; a layer's weight is the slot's times its
    /// record's; finished records are released, and a slot left empty is idle.
    fn update_records(&mut self, slot: usize, dt: f32) {
        let d = dt * self.slots[slot].rate;
        for r in self.slots[slot].records.clone() {
            self.refresh(r);
            if self.records[r].queued == 0 {
                self.release(slot, r);
                continue;
            }
            let rec = &mut self.records[r];
            if rec.fade == 1 {
                rec.weight += d;
                if 1.0 < rec.weight {
                    rec.weight = 1.0;
                    rec.fade = 0;
                }
            } else if rec.fade == 2 {
                rec.weight -= d;
                if rec.weight < 0.0 {
                    rec.weight = 0.0;
                    rec.fade = 0;
                }
            }
            let w = self.slots[slot].weight * self.records[r].weight;
            self.state.set_weight(self.records[r].layer, w);
        }
        if self.slots[slot].records.is_empty() {
            self.slots[slot].state = 1;
        }
    }

    /// The slots for `dt` milliseconds (`0x47a1c0`).
    pub fn update_slots(&mut self, dt: f32, events: &mut Vec<Event>) {
        for s in 0..self.slots.len() {
            self.update_slot(s, dt, events);
        }
    }

    /// The library for `dt` seconds (`0x478a10`), each end told to its slot's callbacks (`0x479df0`) and each
    /// trigger to the listeners; then, with a model bound, the pose.
    pub fn step(&mut self, dt: f32, events: &mut Vec<Event>) {
        let (mut ended, mut fired) = (Vec::new(), Vec::new());
        if 0.0 < dt {
            self.state.step(dt, self.pose.as_mut(), &mut ended, &mut fired);
        }
        events.extend(fired.into_iter().map(Event::Trigger));
        if let Some(p) = &mut self.pose {
            self.state.evaluate(p);
        }
        for Ended { notify, handle, flags } in ended {
            let r = notify as usize;
            let Some(slot) = self.records.get(r).and_then(|x| x.slot) else { continue };
            for e in self.slots[slot].ends.iter().flatten() {
                if !e.match_clip || (e.handle ^ handle) & 0x3ff80 == 0 {
                    events.push(Event::End { slot, token: e.token, handle, flags });
                }
            }
            self.refresh(r);
        }
    }
}

/// MSVC's `_ftol` on an f32.
fn layer_ftol(x: f32) -> i32 {
    if x.is_finite() && x > -2147483649.0 && x < 2147483648.0 {
        x.trunc() as i32
    } else {
        i32::MIN
    }
}

/// The C runtime's `rand` (MSVC): the character's idle sway draws its targets from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrtRand(pub u32);

impl CrtRand {
    pub fn rand(&mut self) -> i32 {
        self.0 = self.0.wrapping_mul(214013).wrapping_add(2531011);
        ((self.0 >> 16) & 0x7fff) as i32
    }
}

impl Default for CrtRand {
    fn default() -> Self {
        CrtRand(1)
    }
}

/// A character (`0x468330`): its control and the secondary slot's weight (`+0x10cc`-`+0x10e8`).
#[derive(Debug, Clone, PartialEq)]
pub struct Character {
    pub control: Control,
    /// `+0x10cc`, `+0x10d4`, `+0x10dc`: an idle sway moving at `rate` per second towards random targets in
    /// 0..0.5.
    pub sway: f32,
    pub sway_target: f32,
    pub sway_rate: f32,
    /// `+0x10e0`, `+0x10e4`, `+0x10e8`: the secondary animations' fade (`Puppet_FadeUp/DownSecondaryAnims`).
    pub fade: f32,
    pub fade_target: f32,
    pub fade_rate: f32,
}

impl Default for Character {
    /// `0x4684e0`: 13 slots of 2 records (as flyers, `0x46bd16`); slots 0 and 1 blend (mode 2), slot 0 at weight
    /// 1 and slot 1 at 0; the sway at 0.1 moving at 0.2; no secondary fade.
    fn default() -> Character {
        let mut control = Control::new(13, 2);
        // `0x4684ba`, `0x4684d2`
        control.set_mode(0, 2);
        control.set_mode(1, 2);
        control.slots[1].weight = 0.0;
        Character { control, sway: 0.1, sway_target: 0.1, sway_rate: 0.2, fade: 0.0, fade_target: 0.0, fade_rate: 0.0 }
    }
}

impl Character {
    /// `0x4d66a0` / `0x4d66d0`: the secondary animations fade to 0 or 1 over `seconds`.
    pub fn fade_secondary(&mut self, up: bool, seconds: f32) {
        self.fade_target = if up { 1.0 } else { 0.0 };
        self.fade_rate = if up { 1.0 } else { -1.0 } / seconds;
    }

    /// `0x468750` for `dt` milliseconds: the slots, the secondary fade and sway (slot 1 weighs `sway × fade`, slot
    /// 0 the rest), then the library for `dt / 1000` seconds. **[I]** the rest of the update (`0x46c970`, the
    /// head's look-at `0x473ee0`, `0x470020`) is not ported.
    pub fn update(&mut self, dt: f32, rand: &mut CrtRand, events: &mut Vec<Event>) {
        if 0.0 <= dt {
            self.control.update_slots(dt, events);
        }
        let s = dt * 0.001;
        if self.fade <= self.fade_target {
            if self.fade < self.fade_target {
                self.fade += s * self.fade_rate;
                if self.fade_target < self.fade {
                    self.fade = self.fade_target;
                }
            }
        } else {
            self.fade += s * self.fade_rate;
            if self.fade < self.fade_target {
                self.fade = self.fade_target;
            }
        }
        let new_target = if self.sway_target <= self.sway {
            if self.sway <= self.sway_target {
                true
            } else {
                self.sway -= s * self.sway_rate;
                if self.sway_target <= self.sway {
                    false
                } else {
                    self.sway = self.sway_target;
                    true
                }
            }
        } else {
            self.sway += s * self.sway_rate;
            if self.sway <= self.sway_target {
                false
            } else {
                self.sway = self.sway_target;
                true
            }
        };
        if new_target {
            self.sway_target = rand.rand() as f32 * (1.0 / 32767.0) * 0.5;
        }
        let w = self.sway * self.fade;
        self.control.slots[1].weight = w;
        self.control.slots[0].weight = 1.0 - w;
        // `0x478a10` with AL set (`0x46891d`): the morph weights decay before the step, the clips setting them again
        if let Some(p) = &mut self.control.pose {
            p.decay_morphs(crate::pose::MORPH_REDUCE);
        }
        self.control.step(s, events);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::play::flag;

    fn character() -> Character {
        let mut c = Character::default();
        let clip = |n: &str, last| SetClip {
            name: Some(n.to_string()),
            clip: ClipInfo { first: 0, last, fps: 30.0 },
            data: None,
        };
        c.control.sets.push(vec![clip("Idle", 30), clip("Wave", 15)]);
        c
    }

    #[test]
    fn names_resolve_without_case() {
        let c = character();
        let (h, clip) = c.control.find("WAVE").unwrap();
        assert_eq!((h, clip.last), (1 << 7 | 0xfffc_0000, 15));
        assert!(c.control.find("run").is_none());
    }

    #[test]
    fn a_callback_hears_its_clip_end() {
        let mut c = character();
        let mut rng = CrtRand::default();
        let h = c.control.play("Wave", 0, Some(Mode::Cut), 0, Some((9, 0)), 1.0, 0.0);
        let mut ev = Vec::new();
        // 15 frames at 30 fps: half a second, ending in the 12th 41.67 ms frame
        let mut frames = 0;
        while !ev.iter().any(|e| matches!(e, Event::End { .. })) {
            c.update(1000.0 / 24.0, &mut rng, &mut ev);
            frames += 1;
            assert!(frames < 100);
        }
        assert_eq!(frames, 12);
        let end = ev.iter().find(|e| matches!(e, Event::End { .. })).unwrap();
        assert!(
            matches!(*end, Event::End { slot: 0, token: 9, handle, flags } if handle & 0x3ffff == h & 0x3ffff && flags & flag::REACHED != 0)
        );
        // the slot started (code 0) on the first update, and empties once the clip is gone
        assert!(ev.contains(&Event::Slot { slot: 0, code: 0 }));
        c.update(1000.0 / 24.0, &mut rng, &mut ev);
        assert_eq!(c.control.slots[0].state, 1);
    }

    #[test]
    fn a_cross_fade_takes_its_length_then_releases_the_old_record() {
        let mut c = character();
        let mut rng = CrtRand::default();
        let mut ev = Vec::new();
        c.control.play("Idle", flag::LOOP, Some(Mode::Cut), 0, None, 1.0, 0.0);
        c.update(10.0, &mut rng, &mut ev);
        c.control.play("Wave", flag::LOOP, Some(Mode::CrossFade), 0, None, 1.0, 0.0);
        assert_eq!(c.control.slots[0].records.len(), 2);
        for _ in 0..4 {
            c.update(100.0, &mut rng, &mut ev);
        }
        let new = c.control.slots[0].records[1];
        assert!((c.control.records[new].weight - 0.8).abs() < 1e-5);
        c.update(100.0, &mut rng, &mut ev);
        c.update(100.0, &mut rng, &mut ev);
        assert_eq!(c.control.slots[0].records, [new]);
        assert!(ev.contains(&Event::Slot { slot: 0, code: 1 }));
    }

    #[test]
    fn queueing_plays_after_the_current_clip() {
        let mut c = character();
        let mut rng = CrtRand::default();
        let mut ev = Vec::new();
        c.control.play("Wave", 0, Some(Mode::Queue), 0, None, 1.0, 0.0);
        c.control.play("Idle", 0, Some(Mode::Queue), 0, None, 1.0, 0.0);
        let r = c.control.slots[0].records[0];
        assert_eq!(c.control.state.layers[c.control.records[r].layer].channels[0].queue.len(), 2);
        // 0.6 s: Wave (0.5 s) has given way to Idle
        c.update(600.0, &mut rng, &mut ev);
        assert_eq!(c.control.records[r].handle >> 7 & 0x7ff, 0);
    }

    #[test]
    fn the_secondary_slot_follows_the_fade_and_sway() {
        let mut c = character();
        let mut rng = CrtRand::default();
        let mut ev = Vec::new();
        c.fade_secondary(true, 2.0);
        c.update(1000.0, &mut rng, &mut ev);
        assert_eq!(c.fade, 0.5);
        let w = c.control.slots[1].weight;
        assert_eq!(w, c.sway * 0.5);
        assert_eq!(c.control.slots[0].weight, 1.0 - w);
    }
}
