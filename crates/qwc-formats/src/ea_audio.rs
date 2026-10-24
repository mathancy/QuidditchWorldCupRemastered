//! Audio decoding: EA-XA ADPCM (v1 and v2) and EA MicroTalk, from SCHl streams and BNKl memory banks.
//!
//! The game's audio uses three codecs (see `tools/audio_parse.py` for the containers):
//! - music (`Audio/Music/*.mus`): SCHl version 1, EA-XA v1, one mono channel per block segment, with a 4-byte
//!   history field before each channel's data that the decoder ignores;
//! - streaming banks and BNKl memory banks (`banks.dat`): version 2, EA-XA v2 (frames may be raw PCM, flag `0xEE`);
//! - speech (`dat.dat`): version 2, MicroTalk 10:1 with optional raw PCM patches per frame.
//!
//! These are ports of vgmstream's decoders (`ea_xa_decoder.c`, `blocked_ea_schl.c`, `ea_schl.c` and `utkdec.c`;
//! vgmstream is ISC-licensed, and `utkdec.c` builds on Andrew D'Addesio's public-domain `utkencode`). They are
//! checked sample for sample against vgmstream-cli by `tests/audio_parity.py`.

use crate::audio::parse_pt;
use crate::util::{bad, slice, u16_at, u32_at, Result};

/// Decoded audio: interleaved 16-bit samples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcm {
    pub channels: usize,
    pub rate: u32,
    /// `frames * channels` samples, interleaved.
    pub samples: Vec<i16>,
}

impl Pcm {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1)
    }

    /// A 16-bit PCM WAV file.
    pub fn to_wav(&self) -> Vec<u8> {
        let data_len = (self.samples.len() * 2) as u32;
        let block_align = (self.channels * 2) as u16;
        let mut w = Vec::with_capacity(44 + data_len as usize);
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data_len).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&(self.channels as u16).to_le_bytes());
        w.extend_from_slice(&self.rate.to_le_bytes());
        w.extend_from_slice(&(self.rate * block_align as u32).to_le_bytes());
        w.extend_from_slice(&block_align.to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&data_len.to_le_bytes());
        for s in &self.samples {
            w.extend_from_slice(&s.to_le_bytes());
        }
        w
    }
}

// ---------------------------------------------------------------------------------------------------------------
// EA-XA

const XA_COEF1: [i32; 4] = [0, 240, 460, 392];
const XA_COEF2: [i32; 4] = [0, 0, -208, -220];

/// One channel's ADPCM history, kept across blocks.
#[derive(Debug, Clone, Copy, Default)]
struct XaState {
    hist1: i32,
    hist2: i32,
}

fn clamp16(v: i32) -> i16 {
    v.clamp(-32768, 32767) as i16
}

fn xa_sample(st: &mut XaState, nibble: u8, coef_index: usize, shift: u32, round: i32) -> i16 {
    let (c1, c2) = (XA_COEF1[coef_index & 3], XA_COEF2[coef_index & 3]);
    let v = ((nibble as i32) << 28) >> shift;
    let s = clamp16((v + c1 * st.hist1 + c2 * st.hist2 + round) >> 8);
    st.hist2 = st.hist1;
    st.hist1 = s as i32;
    s
}

/// EA-XA v1, one mono channel: 15-byte frames of 28 samples (`coef << 4 | shift`, then high nibble first), with
/// rounding. Appends `count` samples decoded from `d[off..]`.
fn xa_v1_mono(d: &[u8], off: usize, count: usize, st: &mut XaState, out: &mut Vec<i16>) -> Result<()> {
    let mut p = off;
    let mut left = count;
    while left > 0 {
        let frame = slice(d, p, 15)?;
        let (coef, shift) = ((frame[0] >> 4) as usize, (frame[0] & 0x0F) as u32 + 8);
        for i in 0..28.min(left) {
            let b = frame[1 + i / 2];
            let nib = if i & 1 == 0 { b >> 4 } else { b & 0x0F };
            out.push(xa_sample(st, nib, coef, shift, 128));
        }
        left -= 28.min(left);
        p += 15;
    }
    Ok(())
}

/// One `SCDl` block of the old stereo EA-XA (at `off`, the block's tag): the sample count, the history `{left
/// current, left previous, right current, right previous}` (i16 each; each block starts from its own), then frames
/// of `coefs` (left high nibble), `shifts` (the same) and 28 bytes, each a left (high) and a right (low) nibble.
/// The arithmetic is EA-XA v1's, with rounding.
fn xa_stereo_block(d: &[u8], off: usize, chans: &mut [Vec<i16>]) -> Result<()> {
    let n = u32_at(d, off + 8)? as usize;
    let h = slice(d, off + 12, 8)?;
    let s16 = |o: usize| i16::from_le_bytes([h[o], h[o + 1]]) as i32;
    let mut st = [XaState { hist1: s16(0), hist2: s16(2) }, XaState { hist1: s16(4), hist2: s16(6) }];
    let mut p = off + 20;
    let mut left = n;
    while left > 0 {
        let frame = slice(d, p, 30)?;
        let take = 28.min(left);
        for (c, ch) in chans.iter_mut().enumerate().take(2) {
            let sel = |b: u8| if c == 0 { b >> 4 } else { b & 0x0F };
            let (coef, shift) = (sel(frame[0]) as usize, sel(frame[1]) as u32 + 8);
            for i in 0..take {
                ch.push(xa_sample(&mut st[c], sel(frame[2 + i]), coef, shift, 128));
            }
        }
        left -= take;
        p += 30;
    }
    Ok(())
}

/// EA-XA v2, one mono channel: like v1 without rounding, and a frame starting with `0xEE` is raw: big-endian
/// history (2 x i16) then 28 big-endian samples.
fn xa_v2_mono(d: &[u8], off: usize, count: usize, st: &mut XaState, out: &mut Vec<i16>) -> Result<()> {
    let mut p = off;
    let mut left = count;
    while left > 0 {
        let n = 28.min(left);
        let info = slice(d, p, 1)?[0];
        if info == 0xEE {
            let frame = slice(d, p, 1 + 4 + 56)?;
            let be = |o: usize| i16::from_be_bytes([frame[o], frame[o + 1]]);
            st.hist1 = be(1) as i32;
            st.hist2 = be(3) as i32;
            for i in 0..n {
                out.push(be(5 + 2 * i));
            }
            p += 61;
        } else {
            let frame = slice(d, p, 15)?;
            let (coef, shift) = ((info >> 4) as usize, (info & 0x0F) as u32 + 8);
            for i in 0..n {
                let b = frame[1 + i / 2];
                let nib = if i & 1 == 0 { b >> 4 } else { b & 0x0F };
                out.push(xa_sample(st, nib, coef, shift, 0));
            }
            p += 15;
        }
        left -= n;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// MicroTalk (UTK)

#[rustfmt::skip]
const UTK_RC_TABLE: [f32; 64] = [
    0.000000, -0.996776, -0.990327, -0.983879, -0.977431, -0.970982, -0.964534, -0.958085,
    -0.951637, -0.930754, -0.904960, -0.879167, -0.853373, -0.827579, -0.801786, -0.775992,
    -0.750198, -0.724405, -0.698611, -0.670635, -0.619048, -0.567460, -0.515873, -0.464286,
    -0.412698, -0.361111, -0.309524, -0.257937, -0.206349, -0.154762, -0.103175, -0.051587,
    0.000000, 0.051587, 0.103175, 0.154762, 0.206349, 0.257937, 0.309524, 0.361111,
    0.412698, 0.464286, 0.515873, 0.567460, 0.619048, 0.670635, 0.698611, 0.724405,
    0.750198, 0.775992, 0.801786, 0.827579, 0.853373, 0.879167, 0.904960, 0.930754,
    0.951637, 0.958085, 0.964534, 0.970982, 0.977431, 0.983879, 0.990327, 0.996776,
];

/// Huffman lookup (8 peeked bits -> command) for the normal and the large-pulse model.
#[rustfmt::skip]
const UTK_CODEBOOKS: [[u8; 256]; 2] = [
    [
        4, 6, 5, 9, 4, 6, 5, 13, 4, 6, 5, 10, 4, 6, 5, 17,
        4, 6, 5, 9, 4, 6, 5, 14, 4, 6, 5, 10, 4, 6, 5, 21,
        4, 6, 5, 9, 4, 6, 5, 13, 4, 6, 5, 10, 4, 6, 5, 18,
        4, 6, 5, 9, 4, 6, 5, 14, 4, 6, 5, 10, 4, 6, 5, 25,
        4, 6, 5, 9, 4, 6, 5, 13, 4, 6, 5, 10, 4, 6, 5, 17,
        4, 6, 5, 9, 4, 6, 5, 14, 4, 6, 5, 10, 4, 6, 5, 22,
        4, 6, 5, 9, 4, 6, 5, 13, 4, 6, 5, 10, 4, 6, 5, 18,
        4, 6, 5, 9, 4, 6, 5, 14, 4, 6, 5, 10, 4, 6, 5, 0,
        4, 6, 5, 9, 4, 6, 5, 13, 4, 6, 5, 10, 4, 6, 5, 17,
        4, 6, 5, 9, 4, 6, 5, 14, 4, 6, 5, 10, 4, 6, 5, 21,
        4, 6, 5, 9, 4, 6, 5, 13, 4, 6, 5, 10, 4, 6, 5, 18,
        4, 6, 5, 9, 4, 6, 5, 14, 4, 6, 5, 10, 4, 6, 5, 26,
        4, 6, 5, 9, 4, 6, 5, 13, 4, 6, 5, 10, 4, 6, 5, 17,
        4, 6, 5, 9, 4, 6, 5, 14, 4, 6, 5, 10, 4, 6, 5, 22,
        4, 6, 5, 9, 4, 6, 5, 13, 4, 6, 5, 10, 4, 6, 5, 18,
        4, 6, 5, 9, 4, 6, 5, 14, 4, 6, 5, 10, 4, 6, 5, 2,
    ],
    [
        4, 11, 7, 15, 4, 12, 8, 19, 4, 11, 7, 16, 4, 12, 8, 23,
        4, 11, 7, 15, 4, 12, 8, 20, 4, 11, 7, 16, 4, 12, 8, 27,
        4, 11, 7, 15, 4, 12, 8, 19, 4, 11, 7, 16, 4, 12, 8, 24,
        4, 11, 7, 15, 4, 12, 8, 20, 4, 11, 7, 16, 4, 12, 8, 1,
        4, 11, 7, 15, 4, 12, 8, 19, 4, 11, 7, 16, 4, 12, 8, 23,
        4, 11, 7, 15, 4, 12, 8, 20, 4, 11, 7, 16, 4, 12, 8, 28,
        4, 11, 7, 15, 4, 12, 8, 19, 4, 11, 7, 16, 4, 12, 8, 24,
        4, 11, 7, 15, 4, 12, 8, 20, 4, 11, 7, 16, 4, 12, 8, 3,
        4, 11, 7, 15, 4, 12, 8, 19, 4, 11, 7, 16, 4, 12, 8, 23,
        4, 11, 7, 15, 4, 12, 8, 20, 4, 11, 7, 16, 4, 12, 8, 27,
        4, 11, 7, 15, 4, 12, 8, 19, 4, 11, 7, 16, 4, 12, 8, 24,
        4, 11, 7, 15, 4, 12, 8, 20, 4, 11, 7, 16, 4, 12, 8, 1,
        4, 11, 7, 15, 4, 12, 8, 19, 4, 11, 7, 16, 4, 12, 8, 23,
        4, 11, 7, 15, 4, 12, 8, 20, 4, 11, 7, 16, 4, 12, 8, 28,
        4, 11, 7, 15, 4, 12, 8, 19, 4, 11, 7, 16, 4, 12, 8, 24,
        4, 11, 7, 15, 4, 12, 8, 20, 4, 11, 7, 16, 4, 12, 8, 3,
    ],
];

/// `(next model, code size, pulse value)` per command.
#[rustfmt::skip]
const UTK_COMMANDS: [(usize, u32, f32); 29] = [
    (1, 8, 0.0), (1, 7, 0.0), (0, 8, 0.0), (0, 7, 0.0),
    (0, 2, 0.0), (0, 2, -1.0), (0, 2, 1.0), (0, 3, -1.0), (0, 3, 1.0),
    (1, 4, -2.0), (1, 4, 2.0), (1, 3, -2.0), (1, 3, 2.0),
    (1, 5, -3.0), (1, 5, 3.0), (1, 4, -3.0), (1, 4, 3.0),
    (1, 6, -4.0), (1, 6, 4.0), (1, 5, -4.0), (1, 5, 4.0),
    (1, 7, -5.0), (1, 7, 5.0), (1, 6, -5.0), (1, 6, 5.0),
    (1, 8, -6.0), (1, 8, 6.0), (1, 7, -6.0), (1, 7, 6.0),
];

/// LSB-first bit reader over a byte slice; reads past the end give zeros (as the original's buffer does).
struct BitReader<'a> {
    d: &'a [u8],
    pos: usize,
    value: u32,
    count: u32,
}

impl BitReader<'_> {
    fn byte(&mut self) -> u8 {
        let b = self.d.get(self.pos).copied().unwrap_or(0);
        self.pos += 1;
        b
    }

    fn s16(&mut self) -> i16 {
        let hi = self.byte();
        i16::from_be_bytes([hi, self.byte()])
    }

    fn init(&mut self) {
        if self.count == 0 {
            self.value = self.byte() as u32;
            self.count = 8;
        }
    }

    fn peek(&self, n: u32) -> u32 {
        self.value & ((1 << n) - 1)
    }

    fn read(&mut self, n: u32) -> u32 {
        let r = self.peek(n);
        self.value >>= n;
        self.count -= n;
        if self.count < 8 {
            self.value |= (self.byte() as u32) << self.count;
            self.count += 8;
        }
        r
    }
}

/// MicroTalk decoder state for one channel.
struct Utk {
    pcm_blocks: bool,
    parsed_header: bool,
    reduced_bandwidth: bool,
    multipulse_threshold: u32,
    fixed_gains: [f32; 64],
    rc_data: [f32; 12],
    synth_history: [f32; 12],
    /// `adapt_cb` (324) followed by `samples` (432): the pitch predictor may read into the current samples.
    subframes: [f32; 324 + 432],
}

const ADAPT: usize = 0;
const SAMPLES: usize = 324;

fn rc_to_lpc(rc: &[f32; 12], lpc: &mut [f32; 12]) {
    let mut tmp1 = [0f32; 12];
    let mut tmp2 = [0f32; 12];
    for i in (0..=10).rev() {
        tmp2[i + 1] = rc[i];
    }
    tmp2[0] = 1.0;
    for i in 0..12 {
        let mut x = -(rc[11] * tmp2[11]);
        for j in (0..=10).rev() {
            x -= rc[j] * tmp2[j];
            tmp2[j + 1] = x * rc[j] + tmp2[j];
        }
        tmp2[0] = x;
        tmp1[i] = x;
        for j in 0..i {
            x -= tmp1[i - 1 - j] * lpc[j];
        }
        lpc[i] = x;
    }
}

impl Utk {
    fn new(pcm_blocks: bool) -> Utk {
        Utk {
            pcm_blocks,
            parsed_header: false,
            reduced_bandwidth: false,
            multipulse_threshold: 0,
            fixed_gains: [0.0; 64],
            rc_data: [0.0; 12],
            synth_history: [0.0; 12],
            subframes: [0.0; 324 + 432],
        }
    }

    fn parse_header(&mut self, br: &mut BitReader) {
        self.reduced_bandwidth = br.read(1) == 1;
        let base_thre = br.read(4);
        let base_gain = br.read(4);
        let base_mult = br.read(6);
        self.multipulse_threshold = 32 - base_thre;
        self.fixed_gains[0] = 8.0 * (1 + base_gain) as f32;
        let multiplier = 1.04f32 + base_mult as f32 * 0.001f32;
        for i in 1..64 {
            self.fixed_gains[i] = self.fixed_gains[i - 1] * multiplier;
        }
    }

    fn excitation(br: &mut BitReader, multipulse: bool, out: &mut [f32; 118], start: usize, stride: usize) {
        let mut i = 0;
        if multipulse {
            let mut model = 0;
            while i < 108 {
                let cmd = UTK_CODEBOOKS[model][br.peek(8) as usize] as usize;
                let (next, size, value) = UTK_COMMANDS[cmd];
                model = next;
                br.read(size);
                if cmd > 3 {
                    out[start + i] = value;
                    i += stride;
                } else if cmd > 1 {
                    let mut count = 7 + br.read(6) as usize;
                    if i + count * stride > 108 {
                        count = (108 - i) / stride;
                    }
                    for _ in 0..count {
                        out[start + i] = 0.0;
                        i += stride;
                    }
                } else {
                    let mut x = 7i32;
                    while br.read(1) == 1 {
                        x += 1;
                    }
                    if br.read(1) == 0 {
                        x = -x;
                    }
                    out[start + i] = x as f32;
                    i += stride;
                }
            }
        } else {
            while i < 108 {
                let (val, bits) = match br.peek(2) {
                    1 => (-2.0, 2),
                    3 => (2.0, 2),
                    _ => (0.0, 1),
                };
                br.read(bits);
                out[start + i] = val;
                i += stride;
            }
        }
    }

    fn synthesis(&mut self, offset: usize, blocks: usize) {
        let mut lpc = [0f32; 12];
        rc_to_lpc(&self.rc_data, &mut lpc);
        let mut p = SAMPLES + offset;
        for _ in 0..blocks {
            for j in 0..12 {
                let mut x = self.subframes[p];
                for (k, l) in lpc.iter().enumerate() {
                    let h = if k < j { self.synth_history[k + 12 - j] } else { self.synth_history[k - j] };
                    x += l * h;
                }
                self.synth_history[11 - j] = x;
                self.subframes[p] = x;
                p += 1;
            }
        }
    }

    fn decode_main(&mut self, br: &mut BitReader) {
        br.init();
        if !self.parsed_header {
            self.parse_header(br);
            self.parsed_header = true;
        }
        let mut multipulse = false;
        let mut rc_delta = [0f32; 12];
        for (i, delta) in rc_delta.iter_mut().enumerate() {
            let idx = if i == 0 {
                let idx = br.read(6);
                if idx < self.multipulse_threshold {
                    multipulse = true;
                }
                idx as usize
            } else if i < 4 {
                br.read(6) as usize
            } else {
                16 + br.read(5) as usize
            };
            *delta = (UTK_RC_TABLE[idx] - self.rc_data[i]) * 0.25;
        }
        let mut exc = [0f32; 118];
        for i in 0..4 {
            let pitch_lag = br.read(8) as i32;
            let pitch_value = br.read(4);
            let gain_index = br.read(6) as usize;
            let pitch_gain = pitch_value as f32 / 15.0;
            let mut fixed_gain = self.fixed_gains[gain_index];
            if !self.reduced_bandwidth {
                Self::excitation(br, multipulse, &mut exc, 5, 1);
            } else {
                let align = br.read(1) as usize;
                let zero = br.read(1) == 1;
                Self::excitation(br, multipulse, &mut exc, 5 + align, 2);
                if zero {
                    for j in 0..54 {
                        exc[5 + (1 - align) + 2 * j] = 0.0;
                    }
                } else {
                    exc[..5].fill(0.0);
                    exc[113..].fill(0.0);
                    let base = 5 + (1 - align);
                    for k in (0..108).step_by(2) {
                        let e = |o: isize| exc[(base as isize + k as isize + o) as usize];
                        let t1 = (e(-5) + e(5)) * 0.018_032_68;
                        let t2 = (e(-3) + e(3)) * 0.114_591_56;
                        let t3 = (e(-1) + e(1)) * 0.597_385_97;
                        exc[base + k] = t1 - t2 + t3;
                    }
                    fixed_gain *= 0.5;
                }
            }
            for j in 0..108 {
                let idx = (108 * i as i32 + 216 - pitch_lag + j as i32).max(0) as usize;
                let t1 = fixed_gain * exc[5 + j];
                let t2 = pitch_gain * self.subframes[ADAPT + idx];
                self.subframes[SAMPLES + 108 * i + j] = t1 + t2;
            }
        }
        self.subframes.copy_within(SAMPLES + 108..SAMPLES + 432, ADAPT);
        for i in 0..4 {
            for (rc, d) in self.rc_data.iter_mut().zip(&rc_delta) {
                *rc += d;
            }
            self.synthesis(12 * i, if i < 3 { 1 } else { 33 });
        }
    }

    /// Decode one 432-sample frame; returns the samples (not rounded or clamped).
    fn decode_frame(&mut self, br: &mut BitReader) -> Result<&[f32]> {
        if !self.pcm_blocks {
            self.decode_main(br);
        } else {
            let pcm = br.byte() == 0xEE;
            self.decode_main(br);
            // Unread the byte the bit reader fetched ahead; the next frame starts byte-aligned.
            br.pos -= 1;
            br.count = 0;
            if pcm {
                let (offset, count) = (br.s16() as i32, br.s16() as i32);
                if !(0..=432).contains(&offset) || count < 0 || count > 432 - offset {
                    return bad(format!("MicroTalk PCM patch {offset}+{count} out of range"));
                }
                for k in 0..count as usize {
                    self.subframes[SAMPLES + offset as usize + k] = br.s16() as f32;
                }
            }
        }
        Ok(&self.subframes[SAMPLES..])
    }
}

fn utk_round(x: f32) -> i16 {
    let r = if x >= 0.0 { x + 0.5 } else { x - 0.5 };
    (r as i32).clamp(-32768, 32767) as i16
}

// ---------------------------------------------------------------------------------------------------------------
// Containers

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Coding {
    /// EA-XA v1, split mono channels, 4-byte history before each channel in a block.
    XaV1,
    /// EA-XA v2 with PCM frames.
    XaV2,
    MicroTalk {
        pcm_blocks: bool,
    },
    /// EA-XA in the old stereo layout (`codec1` 7, the movies): each block holds its sample count, both channels'
    /// history and 30-byte frames of 28 stereo samples.
    XaStereo,
}

#[derive(Debug, Clone, Copy)]
struct Header {
    coding: Coding,
    channels: usize,
    rate: u32,
    samples: Option<usize>,
    version: u64,
}

/// Read a PT header (at the `PT\0\0` magic) the way vgmstream's `parse_variable_header` does for PC data.
fn header(d: &[u8], pt: usize) -> Result<(Header, std::collections::BTreeMap<u8, u64>)> {
    let tags = parse_pt(d, pt)?;
    let platform = u16_at(d, pt + 2)?;
    if platform != 0 {
        return bad(format!("platform {platform} is not PC"));
    }
    let version = tags.get(&0x80).copied().unwrap_or(0);
    let pcm_blocks = version >= 2;
    let coding = match (tags.get(&0x83).copied(), tags.get(&0xA0).copied().unwrap_or(0x0A)) {
        (Some(7), _) => Coding::XaStereo,
        (Some(c), _) => return bad(format!("codec1 {c:#x} not supported")),
        (None, 0x0A) if pcm_blocks => Coding::XaV2,
        (None, 0x0A) => Coding::XaV1,
        (None, 0x04 | 0x16) => Coding::MicroTalk { pcm_blocks },
        (None, c) => return bad(format!("codec {c:#x} not supported")),
    };
    let h = Header {
        coding,
        channels: tags.get(&0x82).copied().unwrap_or(1) as usize,
        rate: tags.get(&0x84).copied().unwrap_or(22050) as u32,
        samples: tags.get(&0x85).map(|&n| n as usize),
        version,
    };
    if h.channels == 0 || h.channels > 6 || (h.coding == Coding::XaStereo && h.channels != 2) {
        return bad(format!("{} channels", h.channels));
    }
    Ok((h, tags))
}

fn interleave(chans: Vec<Vec<i16>>, frames: usize) -> Vec<i16> {
    let n = chans.len();
    let mut out = vec![0i16; frames * n];
    for (c, ch) in chans.iter().enumerate() {
        for (f, &s) in ch.iter().take(frames).enumerate() {
            out[f * n + c] = s;
        }
    }
    out
}

/// Decode a whole SCHl stream (`SCHl` ... `SCEl`).
pub fn decode_schl(d: &[u8]) -> Result<Pcm> {
    if slice(d, 0, 4)? != b"SCHl" {
        return bad("not an SCHl stream");
    }
    let (h, _) = header(d, 8)?;
    let mut chans: Vec<Vec<i16>> = vec![Vec::new(); h.channels];
    let mut xa = vec![XaState::default(); h.channels];
    let mut utk: Vec<Utk> =
        (0..h.channels).map(|_| Utk::new(matches!(h.coding, Coding::MicroTalk { pcm_blocks: true }))).collect();
    let mut off = u32_at(d, 4)? as usize;
    while off + 8 <= d.len() {
        let tag = slice(d, off, 4)?;
        let size = u32_at(d, off + 4)? as usize;
        if tag == b"SCEl" {
            break;
        }
        if size < 8 {
            return bad(format!("block of size {size} at {off:#x}"));
        }
        if tag == b"SCDl" && h.coding == Coding::XaStereo {
            xa_stereo_block(d, off, &mut chans)?;
        } else if tag == b"SCDl" {
            let n = u32_at(d, off + 8)? as usize;
            for c in 0..h.channels {
                let start = if h.version > 0 {
                    off + 0x0C + 4 * h.channels + u32_at(d, off + 0x0C + 4 * c)? as usize
                } else {
                    off + 0x0C + 8 + c * (n / 28 * 15)
                };
                match h.coding {
                    Coding::XaV1 => xa_v1_mono(d, start + 4, n, &mut xa[c], &mut chans[c])?,
                    Coding::XaV2 => xa_v2_mono(d, start, n, &mut xa[c], &mut chans[c])?,
                    Coding::XaStereo => unreachable!(),
                    Coding::MicroTalk { .. } => {
                        // Each block restarts the bit reader one byte after the channel's offset; the decoder
                        // state carries over.
                        let mut br = BitReader { d, pos: start + 1, value: 0, count: 0 };
                        let mut left = n;
                        while left > 0 {
                            let frame = utk[c].decode_frame(&mut br)?;
                            let take = left.min(432);
                            chans[c].extend(frame[..take].iter().map(|&x| utk_round(x)));
                            left -= take;
                        }
                    }
                }
            }
        }
        off += size;
    }
    let frames = chans.iter().map(Vec::len).min().unwrap_or(0);
    let frames = h.samples.map_or(frames, |s| s.min(frames));
    Ok(Pcm { channels: h.channels, rate: h.rate, samples: interleave(chans, frames) })
}

/// Decode every sound of a BNKl (version 4 or 5, little-endian) memory bank. Entries with a zero offset are
/// placeholders and come back as `None`.
pub fn decode_bnk(d: &[u8]) -> Result<Vec<Option<Pcm>>> {
    if slice(d, 0, 4)? != b"BNKl" {
        return bad("not a little-endian BNKl bank");
    }
    let version = slice(d, 4, 1)?[0];
    if !matches!(version, 4 | 5) {
        return bad(format!("BNKl version {version} not supported"));
    }
    let count = u16_at(d, 6)? as usize;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let entry = 0x14 + 4 * i;
        let rel = u32_at(d, entry)? as usize;
        if rel == 0 {
            out.push(None);
            continue;
        }
        let pt = entry + rel;
        let (h, tags) = header(d, pt)?;
        let frames = h.samples.ok_or_else(|| crate::Error::Format(format!("bank sound {i} has no sample count")))?;
        let offs = [0x88u8, 0x89, 0x94, 0x95, 0xA2, 0xA3];
        let mut chans = Vec::with_capacity(h.channels);
        for (c, tag) in offs.iter().take(h.channels).enumerate() {
            let start = *tags
                .get(tag)
                .ok_or_else(|| crate::Error::Format(format!("bank sound {i} has no offset for channel {c}")))?
                as usize;
            let mut st = XaState::default();
            let mut s = Vec::with_capacity(frames);
            match h.coding {
                Coding::XaV2 => xa_v2_mono(d, start, frames, &mut st, &mut s)?,
                Coding::XaV1 => xa_v1_mono(d, start, frames, &mut st, &mut s)?,
                Coding::MicroTalk { .. } => return bad("MicroTalk bank sounds are not supported"),
                Coding::XaStereo => return bad("stereo EA-XA bank sounds are not supported"),
            }
            chans.push(s);
        }
        out.push(Some(Pcm { channels: h.channels, rate: h.rate, samples: interleave(chans, frames) }));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xa_v1_frame() {
        // coef 0, shift 12+8: nibble 1 -> (1 << 28 >> 20) = 256, +128 >> 8 = 1; nibble 0xF -> -256+128 >> 8 = -1
        let mut f = vec![0x0C, 0x1F];
        f.resize(15, 0);
        let (mut st, mut out) = (XaState::default(), Vec::new());
        xa_v1_mono(&f, 0, 3, &mut st, &mut out).unwrap();
        assert_eq!(out, [1, -1, 0]);
    }

    #[test]
    fn xa_v2_pcm_frame_sets_history() {
        let mut f = vec![0xEE, 0x00, 0x05, 0xFF, 0xFE, 0x12, 0x34];
        f.resize(61, 0);
        let (mut st, mut out) = (XaState::default(), Vec::new());
        xa_v2_mono(&f, 0, 2, &mut st, &mut out).unwrap();
        assert_eq!(out, [0x1234, 0]);
        assert_eq!((st.hist1, st.hist2), (5, -2));
    }

    #[test]
    fn bit_reader_is_lsb_first() {
        let d = [0b1010_1100, 0xFF];
        let mut br = BitReader { d: &d, pos: 0, value: 0, count: 0 };
        br.init();
        assert_eq!(br.read(2), 0b00);
        assert_eq!(br.read(3), 0b011);
        assert_eq!(br.read(4), 0b1101);
    }

    #[test]
    fn wav_header() {
        let pcm = Pcm { channels: 2, rate: 22050, samples: vec![1, -1, 2, -2] };
        let w = pcm.to_wav();
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(w.len(), 44 + 8);
        assert_eq!(u32::from_le_bytes(w[28..32].try_into().unwrap()), 22050 * 4);
        assert_eq!(pcm.frames(), 2);
    }
}
