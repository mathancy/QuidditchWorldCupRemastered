//! The input exchange, apart from sockets and clocks: each side's frames by number, which frame runs next and when it
//! may, what the other side still needs, and the checksums compared.
//!
//! Frame `n` runs once both sides' input for it is known. A side gives its input for frame `n + delay` as it runs
//! frame `n`, so the input has `delay` frames' time (24 a second) to cross the network before the other side needs
//! it; frames `0..delay` are blank on both sides. Every message repeats all the frames the other side has not
//! acknowledged, so a lost datagram costs only the wait for the next one. After running a frame each side records
//! the match's checksum; the other side's, as they come, are compared with the same frame's, and the first that
//! differs is the desync.

use std::collections::VecDeque;

use crate::wire::Frames;

/// How many of our checksums are kept to compare with the other side's (ten seconds).
const SUMS_KEPT: usize = 240;

/// One side's frames from `base` on.
#[derive(Debug, Default)]
struct Run {
    base: u32,
    frames: VecDeque<Vec<u8>>,
}

impl Run {
    fn end(&self) -> u32 {
        self.base + self.frames.len() as u32
    }

    fn get(&self, n: u32) -> Option<&[u8]> {
        let k = n.checked_sub(self.base)?;
        self.frames.get(k as usize).map(Vec::as_slice)
    }

    /// Forgets the frames below `n`.
    fn trim(&mut self, n: u32) {
        while self.base < n && !self.frames.is_empty() {
            self.frames.pop_front();
            self.base += 1;
        }
    }
}

#[derive(Debug)]
pub struct Lockstep {
    len: usize,
    delay: u32,
    local: Run,
    remote: Run,
    /// The frame to run next.
    next: u32,
    /// Our frames the other side holds (all below this).
    acked: u32,
    /// Our checksums after the frames we ran, the latest last; the other side's not yet compared (it ran ahead).
    sums: VecDeque<(u32, u64)>,
    theirs: VecDeque<(u32, u64)>,
    desync: Option<u32>,
}

impl Lockstep {
    /// Frames of `len` bytes, each side's input `delay` frames late.
    pub fn new(len: usize, delay: u32) -> Lockstep {
        let blank = || Run { base: 0, frames: (0..delay).map(|_| vec![0; len]).collect() };
        Lockstep {
            len,
            delay,
            local: blank(),
            remote: blank(),
            next: 0,
            acked: 0,
            sums: VecDeque::new(),
            theirs: VecDeque::new(),
            desync: None,
        }
    }

    pub fn frame_len(&self) -> usize {
        self.len
    }

    pub fn delay(&self) -> u32 {
        self.delay
    }

    /// The frame to run next.
    pub fn next(&self) -> u32 {
        self.next
    }

    /// Whether our input for frame `next + delay` is still to be given (one is given a frame run).
    pub fn wants_input(&self) -> bool {
        self.local.end() <= self.next + self.delay
    }

    /// Our input for the next frame that has none; answers its number.
    pub fn push_input(&mut self, frame: Vec<u8>) -> u32 {
        assert_eq!(frame.len(), self.len, "a frame of the session's length");
        let n = self.local.end();
        self.local.frames.push_back(frame);
        n
    }

    /// Both sides' input for frame `next`, ours first, once both are here.
    pub fn ready(&self) -> Option<(&[u8], &[u8])> {
        Some((self.local.get(self.next)?, self.remote.get(self.next)?))
    }

    /// Frame `next` was run and left the match with checksum `sum`.
    pub fn advance(&mut self, sum: u64) {
        debug_assert!(self.ready().is_some(), "a frame run before both inputs were here");
        let n = self.next;
        self.next += 1;
        self.sums.push_back((n, sum));
        if self.sums.len() > SUMS_KEPT {
            self.sums.pop_front();
        }
        self.remote.trim(self.next);
        self.local.trim(self.next.min(self.acked));
        self.compare();
    }

    /// Our frames the other side does not have yet, as many as `max` from the first: `(first, bytes)`.
    pub fn outgoing(&self, max: usize) -> (u32, Vec<u8>) {
        let first = self.acked.max(self.local.base);
        let mut data = Vec::new();
        let mut n = first;
        while let Some(f) = self.local.get(n) {
            if (n - first) as usize >= max {
                break;
            }
            data.extend_from_slice(f);
            n += 1;
        }
        (first, data)
    }

    /// How many of the other side's frames we hold (what we acknowledge).
    pub fn ack(&self) -> u32 {
        self.remote.end()
    }

    /// Our frames given and not yet acknowledged.
    pub fn unacked(&self) -> u32 {
        self.local.end().saturating_sub(self.acked)
    }

    /// Our latest checksum, to send.
    pub fn latest_sum(&self) -> Option<(u32, u64)> {
        self.sums.back().copied()
    }

    /// The first frame after which the two sides' matches differed.
    pub fn desync(&self) -> Option<u32> {
        self.desync
    }

    /// What the other side sent: its frames (in order from the first we lack; a gap waits for the resend), what
    /// it holds of ours, and its checksum. A message of another frame length is ignored.
    pub fn receive(&mut self, f: &Frames) {
        if usize::from(f.len) != self.len {
            return;
        }
        for k in 0..f.count() {
            let n = f.first + k as u32;
            if n == self.remote.end() {
                self.remote.frames.push_back(f.frame(k).to_vec());
            } else if n > self.remote.end() {
                break;
            }
        }
        if f.ack > self.acked && f.ack <= self.local.end() {
            self.acked = f.ack;
            self.local.trim(self.next.min(self.acked));
        }
        if let Some(s) = f.sum {
            if !self.theirs.contains(&s) {
                self.theirs.push_back(s);
            }
            if self.theirs.len() > SUMS_KEPT {
                self.theirs.pop_front();
            }
            self.compare();
        }
    }

    /// Their checksums for frames we have run.
    fn compare(&mut self) {
        let ran = self.next;
        let sums = &self.sums;
        let desync = &mut self.desync;
        self.theirs.retain(|&(n, sum)| {
            if n >= ran {
                return true;
            }
            if let Some(&(_, ours)) = sums.iter().find(|&&(m, _)| m == n) {
                if ours != sum && desync.is_none_or(|d| n < d) {
                    *desync = Some(n);
                }
            }
            false
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `l` sends: its unacknowledged frames, at most `max`.
    fn frames_of(l: &Lockstep, max: usize) -> Frames {
        let (first, data) = l.outgoing(max);
        Frames {
            first,
            len: l.len as u8,
            data,
            ack: l.ack(),
            running: l.next,
            sum: l.latest_sum(),
            stamp: 0,
            echo: None,
        }
    }

    #[test]
    fn the_first_frames_are_blank_and_input_waits_its_delay() {
        let mut a = Lockstep::new(2, 3);
        assert_eq!(a.ready(), Some((&[0u8, 0][..], &[0u8, 0][..])));
        assert!(a.wants_input());
        assert_eq!(a.push_input(vec![1, 1]), 3);
        assert!(!a.wants_input(), "one input a frame run");
        a.advance(0);
        a.advance(0);
        a.advance(0);
        assert!(a.wants_input());
        assert_eq!(a.ready(), None, "frame 3 waits for the other side");
    }

    /// Two sides over a network that loses, repeats and reorders: both run every frame with the same two inputs.
    #[test]
    fn both_sides_run_the_same_frames_over_a_bad_network() {
        let (mut a, mut b) = (Lockstep::new(3, 2), Lockstep::new(3, 2));
        let mut rng = 12345u32;
        let mut roll = |n: u32| {
            rng = rng.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (rng >> 16) % n
        };
        // datagrams in flight: (to b, the message, ticks until it lands)
        let mut wire: Vec<(bool, Frames, u32)> = Vec::new();
        let (mut ran_a, mut ran_b) = (Vec::new(), Vec::new());
        let input = |side: u8, n: u32| vec![side, n as u8, (n >> 8) as u8];
        for _tick in 0..3000 {
            for (side, l, ran) in [(0u8, &mut a, &mut ran_a), (1, &mut b, &mut ran_b)] {
                if l.wants_input() {
                    let n = l.local.end();
                    l.push_input(input(side, n));
                }
                // a side runs what it can, a frame or two a tick
                for _ in 0..2 {
                    let Some((mine, theirs)) = l.ready() else { break };
                    let pair =
                        if side == 0 { (mine.to_vec(), theirs.to_vec()) } else { (theirs.to_vec(), mine.to_vec()) };
                    ran.push(pair);
                    l.advance(ran.len() as u64);
                }
            }
            for (to_b, from) in [(true, &a), (false, &b)] {
                if roll(4) != 0 {
                    // a quarter lost; the rest late by 0-5 ticks (so reordered); some twice
                    let m = frames_of(from, 7);
                    let late = roll(6);
                    if roll(10) == 0 {
                        wire.push((to_b, m.clone(), late + 2));
                    }
                    wire.push((to_b, m, late));
                }
            }
            let mut keep = Vec::new();
            for (to_b, m, late) in wire.drain(..) {
                if late == 0 {
                    if to_b {
                        b.receive(&m)
                    } else {
                        a.receive(&m)
                    }
                } else {
                    keep.push((to_b, m, late - 1));
                }
            }
            wire = keep;
        }
        let n = ran_a.len().min(ran_b.len());
        assert!(n > 1000, "only {n} frames ran");
        assert_eq!(ran_a[..n], ran_b[..n]);
        for (k, (x, y)) in ran_a.iter().enumerate().skip(2) {
            assert_eq!((x, y), (&input(0, k as u32), &input(1, k as u32)), "frame {k}");
        }
        assert_eq!((a.desync(), b.desync()), (None, None));
        // the runs kept are short: what is not acknowledged or not run yet
        assert!(a.local.frames.len() < 64 && a.remote.frames.len() < 64);
    }

    #[test]
    fn a_differing_checksum_is_the_desync() {
        let (mut a, mut b) = (Lockstep::new(1, 1), Lockstep::new(1, 1));
        for n in 0..10u64 {
            for l in [&mut a, &mut b] {
                if l.wants_input() {
                    l.push_input(vec![0]);
                }
            }
            let (ma, mb) = (frames_of(&a, 10), frames_of(&b, 10));
            a.receive(&mb);
            b.receive(&ma);
            a.advance(n);
            b.advance(if n >= 6 { 100 + n } else { n });
        }
        let (ma, mb) = (frames_of(&a, 10), frames_of(&b, 10));
        a.receive(&mb);
        b.receive(&ma);
        // the latest sums are compared (frame 9's): both see a desync, at or after where it began
        assert!(a.desync().is_some_and(|d| d >= 6), "{:?}", a.desync());
        assert!(b.desync().is_some_and(|d| d >= 6), "{:?}", b.desync());
    }

    #[test]
    fn stale_acknowledgements_and_other_lengths_are_ignored() {
        let mut a = Lockstep::new(2, 1);
        a.push_input(vec![5, 5]);
        let f = |ack, len| Frames { first: 0, len, data: vec![], ack, running: 0, sum: None, stamp: 0, echo: None };
        a.receive(&f(2, 2));
        assert_eq!(a.unacked(), 0);
        a.receive(&f(1, 2));
        assert_eq!(a.acked, 2, "an older message's ack does not take it back");
        a.receive(&f(9, 2));
        assert_eq!(a.acked, 2, "an ack beyond what was given is nonsense");
        let mut odd = f(0, 3);
        odd.data = vec![1, 2, 3];
        a.receive(&odd);
        assert_eq!(a.ack(), 1, "a frame of another length is not taken");
    }
}
