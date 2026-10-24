//! Screen wipes (`docs/FECK_VM.md` "Wipes"). `LayerObj_ActivateWipe` (`0x4d5df0`) starts one on a layer: a `.wpe`
//! resource (`qwc_formats::wpe`), played over a time. Each draw of the layer steps it (`0x4d9be0` -> `0x4ea830`):
//! the draw list gets a record of the two masks the wipe is between, and the renderer fades the layer's sprites
//! with flag `0x4000` by the mask under each corner (`draw::sprite_quad`). When the time is up the wipe ends and its
//! callback runs. Watchers (`LayerObj_AddWipeWatcher`, `0x4ea960`) post an event when the mask under an object
//! passes a value.

use std::rc::Rc;

use crate::frontend::FeEvent;
use crate::layer::{ftol, Object};

/// A mask: 32 × 32 coverage bytes, 0 the old screen, 255 the new.
pub const CELLS: usize = 1024;

/// A `.wpe` as the wipe reads it: `data + 2` (the frame count's low byte) and 1,024 bytes a frame from `data + 4`,
/// whatever its header's width and height (all three on the disc are 32 × 32).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WipeData {
    pub count: u8,
    bytes: Vec<u8>,
}

impl WipeData {
    /// A `.wpe` file's bytes (`None` when shorter than its header).
    pub fn new(file: &[u8]) -> Option<WipeData> {
        Some(WipeData { count: *file.get(2)?, bytes: file.get(4..)?.to_vec() })
    }

    /// Frame `k`'s mask; `None` past the file (the original reads on into whatever follows).
    pub fn mask(&self, k: i32) -> Option<&[u8; CELLS]> {
        let at = usize::try_from(k).ok()?.checked_mul(CELLS)?;
        self.bytes.get(at..at + CELLS)?.try_into().ok()
    }
}

/// A layer's wipe (its `+0x20`-`+0x44`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayerWipe {
    /// `+0x20` (on) and `+0x24`: the wipe's resource while it plays.
    pub data: Option<Rc<WipeData>>,
    /// `+0x28`: the draws so far.
    pub elapsed: u32,
    /// `+0x2c`: how many draws it lasts (`ms × 0.024`, truncated).
    pub frames: u32,
    /// `+0x30`: played backwards.
    pub reverse: bool,
    /// `+0x34`: the script function run when it ends (0 = none).
    pub callback: u32,
    /// The watchers (`+0x3c` the first, `+0x40` the last, `+0x44` the count).
    pub watchers: Vec<Watcher>,
}

/// A wipe watcher (0x24 bytes, `0x4ea960`).
#[derive(Debug, Clone, PartialEq)]
pub struct Watcher {
    /// `+0x08`: the script's id for it (`LayerObj_DeleteWipeWatcher` finds it by this); the event carries it.
    pub id: u32,
    /// `+0x0c`: the value the mask is tested against (0 to 1).
    pub value: f32,
    /// `+0x10`: the test, -1 the mask below the value, 0 equal to it, 1 above it (any other never passes).
    pub test: i32,
    /// `+0x14`: removed once it has passed.
    pub once: bool,
    /// `+0x1c`: the object whose place is watched.
    pub object: usize,
}

/// A wipe in the draw list (0x24 bytes at the list's `+0x5080`, `0x4ea740`): the layer, the two frames the wipe is
/// between and how far, and the mask the renderer uses (`+0x14` is its slot in the renderer's masks, `0xfb54e4`;
/// the port keeps it here).
#[derive(Debug, Clone, PartialEq)]
pub struct WipeRecord {
    /// The layer's hash (`+0x00`, from layer `+0x54`), which sprites are matched by.
    pub layer: u32,
    /// The frames' masks (`+0x08`, `+0x0c`, in the resource `+0x04`) and the fraction between them (`+0x10`).
    pub frames: [Box<[u8; CELLS]>; 2],
    pub frac: f32,
    /// The mask drawn: the first frame's (`0x4e8310` with no record of the layer in the last tick's list), or
    /// blended with the last tick's ([`WipeRecord::blend`]).
    pub mask: Box<[u8; CELLS]>,
}

impl WipeRecord {
    /// The mask between frames at `frac` for one cell (as `0x4e9fb0` computes it).
    fn at(&self, i: usize) -> f32 {
        let (a, b) = (self.frames[0][i] as f32, self.frames[1][i] as f32);
        (b - a) * self.frac + a
    }

    /// `0x4e9fb0` with `0.5` (`0x4e8917`): each cell halfway from this tick's mask to the last tick's, both taken
    /// between their frames, truncated.
    pub fn blend(&mut self, prev: &WipeRecord) {
        for i in 0..CELLS {
            let (new, old) = (self.at(i), prev.at(i));
            self.mask[i] = ftol((old - new) * 0.5 + new) as u8;
        }
    }
}

/// What a step asks of the front end: the callback to run (the wipe ended) and the events posted.
#[derive(Debug, Default, PartialEq)]
pub struct Stepped {
    pub record: Option<WipeRecord>,
    pub ended: Option<u32>,
    pub events: Vec<FeEvent>,
}

impl LayerWipe {
    /// `0x4ea830`, once per draw of the layer `hash`: the frames at `elapsed / frames` of the way (frame `a` and the
    /// next, or with `reverse` the frame as far from the end and the one before), a record for the draw list, the
    /// watchers' tests; then one draw more, and at the end the wipe stops (its callback to run). A second frame
    /// past either end is the first again, at fraction 0.
    pub fn step(&mut self, hash: u32, objects: &[Object]) -> Stepped {
        let mut out = Stepped::default();
        let Some(data) = self.data.clone() else { return out };
        let n = data.count as i32;
        let t = self.elapsed as f32 / self.frames as f32 * n as f32;
        let mut a = ftol(t);
        let mut frac = t - a as f32;
        let mut b = a.wrapping_add(1);
        if self.reverse {
            a = n.wrapping_sub(a).wrapping_sub(1);
            b = a.wrapping_sub(1);
        }
        if b >= n || b < 0 {
            b = a;
            frac = 0.0;
        }
        // **[I]** a frame past the file (a wipe of no frames, under 42 ms) gives no record; the original reads on
        if let (Some(ma), Some(mb)) = (data.mask(a), data.mask(b)) {
            out.record =
                Some(WipeRecord { layer: hash, frames: [Box::new(*ma), Box::new(*mb)], frac, mask: Box::new(*ma) });
            // `0x4eaa70`: each watcher in turn; one that passes posts its event, and goes when it is once only
            self.watchers.retain(|w| match watch(w, ma, mb, frac, objects) {
                Some(e) => {
                    out.events.push(e);
                    !w.once
                }
                None => true,
            });
        }
        self.elapsed += 1;
        if self.elapsed >= self.frames {
            self.data = None;
            // `0x4ea8f0`
            out.ended = Some(self.callback).filter(|&c| c != 0);
        }
        out
    }
}

/// `0x4eaaf0`: the mask under the watched object, between the frames, against the watcher's value. The cell is
/// `(y / 32) × 32 + x / 32` of the object's place on the screen (`+0x3c`, `+0x40`, as 16-bit integers); when the
/// test passes, an event of type 1 (input) with its "pad" the once flag, its flags the watcher's id and its buttons
/// the value's bits (and 2 at `+4`, which no listener reads).
fn watch(w: &Watcher, a: &[u8; CELLS], b: &[u8; CELLS], frac: f32, objects: &[Object]) -> Option<FeEvent> {
    let o = objects.get(w.object)?;
    let at = |off: u16| ftol(f32::from_bits(o.slot(off))) as u16 as usize;
    let cell = (at(0x40) >> 5 << 5) + (at(0x3c) >> 5);
    // **[I]** a cell past the mask (an object below the 16th row) reads 0; the original reads on
    // `[0x5bf070]`: 1 / 255
    let get = |m: &[u8; CELLS]| m.get(cell).copied().unwrap_or(0) as f32 * 0.003921569;
    let va = get(a);
    let v = (get(b) - va) * frac + va;
    let pass = match w.test {
        -1 => v < w.value,
        0 => v == w.value,
        1 => v > w.value,
        _ => false,
    };
    pass.then_some(FeEvent::Input { pad: w.once as u8, flags: w.id, buttons: v.to_bits() })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wipe of `n` flat frames, frame k all `k * 10`.
    fn flat(n: u8) -> Rc<WipeData> {
        let mut f = vec![32, 32, n, 0];
        for k in 0..n {
            f.extend(std::iter::repeat_n(k * 10, CELLS));
        }
        Rc::new(WipeData::new(&f).unwrap())
    }

    #[test]
    fn a_wipe_steps_through_its_frames_and_ends() {
        let mut w = LayerWipe { data: Some(flat(4)), frames: 8, callback: 0x77, ..Default::default() };
        let mut seen = Vec::new();
        for _ in 0..8 {
            let s = w.step(9, &[]);
            let r = s.record.unwrap();
            seen.push((r.frames[0][0], r.frames[1][0], r.frac));
            assert_eq!(s.ended.is_some(), w.data.is_none());
        }
        // t = elapsed / 8 * 4: frames 0, 0.5, 1, ... 3.5; the last frame's next is past the end
        assert_eq!(seen[0], (0, 10, 0.0));
        assert_eq!(seen[1], (0, 10, 0.5));
        assert_eq!(seen[6], (30, 30, 0.0));
        assert_eq!(seen[7], (30, 30, 0.0));
        assert!(w.data.is_none());
        assert_eq!(w.step(9, &[]), Stepped::default());
    }

    #[test]
    fn a_reversed_wipe_runs_from_the_last_frame() {
        let mut w = LayerWipe { data: Some(flat(4)), frames: 8, reverse: true, ..Default::default() };
        let r = w.step(9, &[]).record.unwrap();
        assert_eq!((r.frames[0][0], r.frames[1][0], r.frac), (30, 20, 0.0));
        let r = w.step(9, &[]).record.unwrap();
        // the fraction still counts forward, towards the frame before
        assert_eq!((r.frames[0][0], r.frames[1][0], r.frac), (30, 20, 0.5));
        let last = (0..6).map(|_| w.step(9, &[]).record.unwrap()).last().unwrap();
        assert_eq!((last.frames[0][0], last.frames[1][0], last.frac), (0, 0, 0.0));
    }

    #[test]
    fn blending_takes_the_midpoint_of_both_ticks() {
        let mut w = LayerWipe { data: Some(flat(4)), frames: 8, ..Default::default() };
        let old = w.step(9, &[]).record.unwrap();
        let mut new = w.step(9, &[]).record.unwrap();
        // old: 0 (frame 0 at 0); new: 5 (halfway from 0 to 10); halfway between: 2.5, truncated
        assert_eq!(new.mask[0], 0);
        new.blend(&old);
        assert_eq!(new.mask[5], 2);
    }

    #[test]
    fn watchers_pass_on_their_test() {
        let mut o = Object::new(crate::layer::Kind::Sprite, 0);
        // at (40, 70): cell 2 × 32 + 1
        o.set_slot(0x3c, 40f32.to_bits());
        o.set_slot(0x40, 70f32.to_bits());
        let objects = [o];
        let mut f = vec![32, 32, 1, 0];
        f.extend([0u8; CELLS]);
        f[4 + 65] = 255;
        let data = Rc::new(WipeData::new(&f).unwrap());
        let watcher = |test, once| Watcher { id: 5, value: 0.5, test, once, object: 0 };
        let mut w = LayerWipe {
            data: Some(data),
            frames: 4,
            watchers: vec![watcher(1, true), watcher(-1, false), watcher(0, false)],
            ..Default::default()
        };
        let s = w.step(9, &objects);
        // 255 / 255 (just over 1 in floats) is above 0.5: the first passes and goes; the others stay
        let v = 255.0f32 * 0.003921569;
        assert_eq!(s.events, [FeEvent::Input { pad: 1, flags: 5, buttons: v.to_bits() }]);
        assert_eq!(w.watchers.len(), 2);
    }
}
