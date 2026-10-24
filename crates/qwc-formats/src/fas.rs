//! Animation sets (`.fas`, magic `0xD9CE785F`) and cutscene streams (`.sfa`).
//! Ports of `tools/fas_parse.py` and `tools/fas_decode.py`; layouts in `docs/RE_NOTES.md`.
//!
//! File = 0x1C-byte header (`magic, flags, ver_major, version, 0, stream_len, bulk_len`) + bulk block + stream.
//! The stream is `CFAnimSet::Reconstruct` (`FUN_0040df50`) read sequentially; `*_off` fields index the bulk block.

use crate::util::{bad, f32_at, f32s, i16_at, slice, u16_at, u32_at, u8_at, Error, Reader, Result};

pub const MAGIC: u32 = 0xD9CE_785F;
/// Keyframed codec used by gameplay and front-end animations.
pub const CEDHAT_3_2: u32 = 0x8699_0700;
/// Per-frame floats streamed from a cutscene's `.sfa`.
pub const STREAMED_3: u32 = 0x37F0_3F00;

/// The shared 0x1C-byte header of `.fas` and `.btf` files.
#[derive(Debug, Clone, Copy)]
pub struct Header {
    pub magic: u32,
    pub flags: u32,
    pub ver_major: u32,
    pub version: u32,
    pub zero: u32,
    pub stream_len: u32,
    pub bulk_len: u32,
}

impl Header {
    pub fn parse(d: &[u8]) -> Result<Header> {
        let f = |i: usize| u32_at(d, 4 * i);
        let h = Header {
            magic: f(0)?,
            flags: f(1)?,
            ver_major: f(2)?,
            version: f(3)?,
            zero: f(4)?,
            stream_len: f(5)?,
            bulk_len: f(6)?,
        };
        if 0x1C + h.bulk_len as usize + h.stream_len as usize != d.len() {
            return bad("section sizes do not add up to file size");
        }
        Ok(h)
    }
}

/// A bit set (`FUN_0040b180`): `nbits` (read by the caller), `nwords`, `mask`, `words[nwords]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitSet {
    pub nbits: u32,
    pub mask: u32,
    pub words: Vec<u32>,
}

impl BitSet {
    pub fn bits(&self) -> Vec<u32> {
        (0..self.nbits).filter(|&i| self.words.get((i >> 5) as usize).is_some_and(|w| w >> (i & 31) & 1 != 0)).collect()
    }
}

/// `u32 count` then `u32 off` when count is nonzero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CountedOff {
    pub count: u32,
    pub off: Option<u32>,
}

fn counted_off(s: &mut Reader) -> Result<CountedOff> {
    let count = s.u32()?;
    Ok(CountedOff { count, off: if count != 0 { Some(s.u32()?) } else { None } })
}

/// One codec slot (`FUN_0040b730`): 0 = rotation, 1 = translation, 2 = unused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodecArray {
    pub codec: u32,
    pub stride: u16,
    pub count: u32,
    pub map_off: Option<u32>,
    pub off: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct Anim {
    pub name: Option<String>,
    pub unk0: u32,
    pub frames: u32,
    pub fps: f32,
    /// Five (six from version 9) optional bone masks; `nbits` equals the target skeleton's bone count.
    pub masks: Vec<Option<BitSet>>,
    pub packed: u8,
    /// The +0x48 table: morph-weight tracks per node ([`Fas::morph_tracks`]).
    pub channels: CountedOff,
    pub root_off: Option<u32>,
    pub codecs: Vec<CodecArray>,
    /// `(scale, 1/scale)` per codec slot.
    pub scales: Vec<(f32, f32)>,
    /// The +0x4c table: visibility tracks per node ([`Fas::vis_tracks`]).
    pub list4c: Option<CountedOff>,
    pub bbox: Option<[f32; 6]>,
    pub list50: Option<CountedOff>,
}

#[derive(Debug, Clone)]
pub struct Fas<'a> {
    pub header: Header,
    pub bulk: &'a [u8],
    pub name: Option<String>,
    pub anims: Vec<Anim>,
    pub tail: Option<[u32; 3]>,
}

impl<'a> Fas<'a> {
    pub fn parse(d: &'a [u8]) -> Result<Fas<'a>> {
        let header = Header::parse(d)?;
        if header.magic != MAGIC {
            return bad("not a .fas file");
        }
        let bulk = slice(d, 0x1C, header.bulk_len as usize)?;
        let mut s = Reader::new(&d[0x1C + bulk.len()..]);
        let v = header.version;
        let name = s.opt_str()?;
        if v > 2 && s.u8()? != 0 {
            return bad("anim-set extra name table (FUN_0040a900) not supported");
        }
        let n = s.u32()?;
        let mut anims = Vec::new();
        for _ in 0..n {
            anims.push(Self::read_anim(&mut s, v)?);
        }
        let tail = if v > 7 { Some([s.u32()?, s.u32()?, s.u32()?]) } else { None };
        if !s.at_end() {
            return bad(format!("stream not fully consumed: {:#x} of {:#x}", s.p, s.d.len()));
        }
        Ok(Fas { header, bulk, name, anims, tail })
    }

    fn read_anim(s: &mut Reader, v: u32) -> Result<Anim> {
        // FUN_0040a4f0 (anim base)
        let name = s.opt_str()?;
        let (unk0, frames, fps) = (s.u32()?, s.u32()?, s.f32()?);
        let mut masks = Vec::new();
        for _ in 0..if v >= 9 { 6 } else { 5 } {
            let nbits = s.u32()?;
            masks.push(if nbits != 0 {
                let (nwords, mask) = (s.u32()?, s.u32()?);
                let words = (0..nwords).map(|_| s.u32()).collect::<Result<Vec<_>>>()?;
                Some(BitSet { nbits, mask, words })
            } else {
                None
            });
        }
        if s.u32()? != 0 {
            return bad("event entries (FUN_0040a1c0) not supported");
        }
        // FUN_0040dbd0 (model anim)
        let packed = if v > 0xB { s.u8()? } else { 0 };
        let channels = counted_off(s)?;
        let root_off = if s.u8()? != 0 { Some(s.u32()?) } else { None };
        let mut codecs = Vec::new();
        if v != 0 {
            for _ in 0..3 {
                let (codec, stride) = (s.u32()?, s.u16()?);
                s.u16()?; // padding
                let count = s.u32()?;
                let (map_off, off) = if count != 0 { (Some(s.u32()?), Some(s.u32()?)) } else { (None, None) };
                codecs.push(CodecArray { codec, stride, count, map_off, off });
            }
        }
        let scales = if v > 1 { (0..3).map(|_| Ok((s.f32()?, s.f32()?))).collect::<Result<Vec<_>>>()? } else { vec![] };
        let list4c = if v >= 4 { Some(counted_off(s)?) } else { None };
        let bbox = if v >= 7 {
            let mut b = [0f32; 6];
            for x in &mut b {
                *x = s.f32()?;
            }
            Some(b)
        } else {
            None
        };
        let list50 = if v > 10 { Some(counted_off(s)?) } else { None };
        Ok(Anim { name, unk0, frames, fps, masks, packed, channels, root_off, codecs, scales, list4c, bbox, list50 })
    }

    /// Decode the channels of codec slot `slot` (0 = rotation angles, 1 = translation).
    /// Streamed3 channels other than constants need the cutscene's `.sfa`.
    pub fn channels(&self, anim: &Anim, slot: usize, sfa: Option<&Sfa>) -> Result<Vec<Channel>> {
        let Some(c) = anim.codecs.get(slot).filter(|c| c.count != 0) else {
            return Ok(vec![]);
        };
        let b = self.bulk;
        let (map_off, off, stride) = (c.map_off.unwrap() as usize, c.off.unwrap() as usize, c.stride as usize);
        let mut out = Vec::with_capacity(c.count as usize);
        for i in 0..c.count as usize {
            let bone = u8_at(b, map_off + i)?;
            let rec = off + i * stride;
            match c.codec {
                STREAMED_3 => {
                    let ch = u16_at(b, rec)?;
                    if ch == 0 {
                        let k = f32s::<3>(b, rec + 8)?.map(f64::from);
                        out.push(Channel { bone, keys: vec![k], times: vec![0] });
                    } else {
                        let sfa =
                            sfa.ok_or_else(|| Error::Format("Streamed3 channel needs the cutscene .sfa file".into()))?;
                        let keys = sfa.frames(ch)?;
                        out.push(Channel { bone, times: (0..keys.len() as u32).collect(), keys });
                    }
                }
                CEDHAT_3_2 => {
                    let inv = anim.scales[slot].1 as f64;
                    let (fl, nk, doff) = (u8_at(b, rec)?, u16_at(b, rec + 2)? as usize, u32_at(b, rec + 4)? as usize);
                    let mut keys = Vec::with_capacity(nk);
                    for k in 0..nk {
                        let p = doff + k * 6;
                        keys.push([
                            i16_at(b, p)? as f64 * inv,
                            i16_at(b, p + 2)? as f64 * inv,
                            i16_at(b, p + 4)? as f64 * inv,
                        ]);
                    }
                    let tbase = doff + nk * 6;
                    let times = if fl & 1 != 0 {
                        (0..nk).map(|k| u16_at(b, tbase + 2 * k).map(u32::from)).collect::<Result<Vec<_>>>()?
                    } else {
                        slice(b, tbase, nk)?.iter().map(|&x| x as u32).collect()
                    };
                    out.push(Channel { bone, keys, times });
                }
                other => return bad(format!("codec {other:#x} not supported")),
            }
        }
        Ok(out)
    }

    /// The morph-weight tracks of `anim` (`0x40d540`, version 10 on): the `+0x48` table has 8-byte
    /// `{u16 node, u16 n, u32 off}` entries, each to `n` 8-byte `{u16 morph, u16 nkeys, u32 off}`; a track's data
    /// is `u8 values[nkeys]`, then its key frames: `u8` when the anim has fewer than 256 frames, otherwise `u16`
    /// from the next even offset. Streamed (packed) anims read theirs from the cutscene stream (`0x40d370`): their
    /// entries are `{u16 morph, u16 stream channel (1-based), u16, u16 frames}`, each frame's weight the channel's
    /// first float in the `.sfa` (`0x4149a0`, `0x414a20`); without the `.sfa` they give none.
    pub fn morph_tracks(&self, anim: &Anim, sfa: Option<&Sfa>) -> Result<Vec<MorphTrack>> {
        let (b, c) = (self.bulk, anim.channels);
        let Some(off) = c.off else { return Ok(vec![]) };
        let wide = anim.frames >= 0x100;
        let mut out = Vec::new();
        for i in 0..c.count as usize {
            let e = off as usize + 8 * i;
            let (node, n, sub) = (u16_at(b, e)?, u16_at(b, e + 2)? as usize, u32_at(b, e + 4)? as usize);
            for k in 0..n {
                let s = sub + 8 * k;
                if anim.packed != 0 {
                    let (morph, ch) = (u16_at(b, s)?, u16_at(b, s + 2)?);
                    let Some(sfa) = sfa.filter(|_| ch != 0) else { continue };
                    let stream = sfa.frames(ch)?.iter().map(|f| f[0] as f32).collect();
                    out.push(MorphTrack { node, morph, values: vec![], times: vec![], wide, stream: Some(stream) });
                    continue;
                }
                let (morph, nk, d) = (u16_at(b, s)?, u16_at(b, s + 2)? as usize, u32_at(b, s + 4)? as usize);
                let values = slice(b, d, nk)?.to_vec();
                let times = if wide {
                    let t = d + nk + (nk & 1);
                    (0..nk).map(|j| u16_at(b, t + 2 * j)).collect::<Result<Vec<_>>>()?
                } else {
                    slice(b, d + nk, nk)?.iter().map(|&x| x as u16).collect()
                };
                out.push(MorphTrack { node, morph, values, times, wide, stream: None });
            }
        }
        Ok(out)
    }

    /// The visibility tracks of `anim` (`0x40d8e0`): the `+0x4c` table has 8-byte `{u16 node, u16 n, u32 off}`
    /// entries, `off` pointing at `u16 times[n]`.
    pub fn vis_tracks(&self, anim: &Anim) -> Result<Vec<VisTrack>> {
        let b = self.bulk;
        let Some((c, off)) = anim.list4c.and_then(|c| Some((c, c.off?))) else { return Ok(vec![]) };
        (0..c.count as usize)
            .map(|i| {
                let e = off as usize + 8 * i;
                let (node, n, t) = (u16_at(b, e)?, u16_at(b, e + 2)? as usize, u32_at(b, e + 4)? as usize);
                let times = (0..n).map(|j| u16_at(b, t + 2 * j)).collect::<Result<Vec<_>>>()?;
                Ok(VisTrack { node, times })
            })
            .collect()
    }
}

/// One morph target's weight over a clip (see [`Fas::morph_tracks`]).
#[derive(Debug, Clone, PartialEq)]
pub struct MorphTrack {
    pub node: u16,
    pub morph: u16,
    pub values: Vec<u8>,
    pub times: Vec<u16>,
    /// Key frames were `u16` (the anim has 256 frames or more); the sampler compares in that width.
    pub wide: bool,
    /// A streamed track's weight per frame (from the cutscene's `.sfa`), in place of the keys.
    pub stream: Option<Vec<f32>>,
}

impl MorphTrack {
    /// The weight at `frame` (`0x40ce60`, `0x40ce00`): `value / 128 - 0.5`, linear between the keys around
    /// `(int)frame + 1` (compared in the keys' width), the last value past them; `None` without keys.
    pub fn sample(&self, frame: f32) -> Option<f32> {
        // a streamed track (`0x4149a0`): linear between the frames around `frame`, the last past them
        if let Some(s) = &self.stream {
            let i = frame.max(0.0).floor() as usize;
            return match (s.get(i), s.get(i + 1)) {
                (Some(&a), Some(&b)) => Some(a + (b - a) * (frame - i as f32)),
                _ => s.last().copied(),
            };
        }
        let (v, t) = (&self.values, &self.times);
        let w = |x: u8| x as f32 * 0.0078125 - 0.5;
        let i = frame as i32 + 1;
        let target = if self.wide { i as u16 } else { i as u8 as u16 };
        for k in 1..v.len() {
            if target <= t[k] {
                let f = (frame - t[k - 1] as f32) / (t[k] as f32 - t[k - 1] as f32);
                return Some(w(v[k - 1]) * (1.0 - f) + w(v[k]) * f);
            }
        }
        v.last().map(|&x| w(x))
    }
}

/// A node's visibility over a clip (see [`Fas::vis_tracks`]).
#[derive(Debug, Clone, PartialEq)]
pub struct VisTrack {
    pub node: u16,
    pub times: Vec<u16>,
}

impl VisTrack {
    /// `0x40d0f0`: visible at first, toggled at every key time not after `(int)frame`.
    pub fn visible(&self, frame: f32) -> bool {
        let f = frame as i32;
        let mut v = true;
        for &k in &self.times {
            if f < k as i32 {
                break;
            }
            v = !v;
        }
        v
    }
}

/// A decoded channel: target bone and its keys (already scaled) at integer frame times.
#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
    pub bone: u8,
    pub keys: Vec<[f64; 3]>,
    pub times: Vec<u32>,
}

impl Channel {
    /// Evaluate at frame time `t` (CEDHAT_3.2 rules, `0x40c440`): the first key time >= `(int)t + 1` ends the
    /// segment; past the last key the curve blends back to key 0 at `length` unless the last key is at `length`.
    pub fn sample(&self, t: f64, length: u32) -> [f64; 3] {
        let (keys, times) = (&self.keys, &self.times);
        if keys.len() == 1 {
            return keys[0];
        }
        let target = t as i64 + 1;
        let (t0, t1, k0, k1) = match (0..keys.len() - 1).find(|&i| times[i + 1] as i64 >= target) {
            Some(i) => (times[i] as f64, times[i + 1] as f64, keys[i], keys[i + 1]),
            None => {
                let last = keys.len() - 1;
                if times[last] == length {
                    return keys[last];
                }
                (times[last] as f64, length as f64, keys[last], keys[0])
            }
        };
        let f = (t - t0) / (t1 - t0);
        [0, 1, 2].map(|j| k0[j] * (1.0 - f) + k1[j] * f)
    }
}

/// `root/CSStream/*.sfa`: `"SFA0" | u32 0x10 | u16 n_channels | u16 n_frames | u16 data_off | u16 frame_size |
/// u16 channel_off[n_channels]`, then `n_frames` frames of `frame_size` bytes of floats.
#[derive(Debug, Clone)]
pub struct Sfa<'a> {
    pub d: &'a [u8],
    pub nframes: u16,
    pub data_off: u16,
    pub frame_size: u16,
    pub channel_offs: Vec<u16>,
}

impl<'a> Sfa<'a> {
    pub fn parse(d: &'a [u8]) -> Result<Sfa<'a>> {
        if slice(d, 0, 4)? != b"SFA0" {
            return bad("not an SFA0 file");
        }
        let (nch, nframes, data_off, frame_size) = (u16_at(d, 8)?, u16_at(d, 10)?, u16_at(d, 12)?, u16_at(d, 14)?);
        let channel_offs = (0..nch as usize).map(|i| u16_at(d, 16 + 2 * i)).collect::<Result<Vec<_>>>()?;
        if data_off as usize + nframes as usize * frame_size as usize != d.len() {
            return bad("SFA size mismatch");
        }
        Ok(Sfa { d, nframes, data_off, frame_size, channel_offs })
    }

    /// Every frame of 1-based channel `channel` (3 floats each).
    pub fn frames(&self, channel: u16) -> Result<Vec<[f64; 3]>> {
        let o = *self
            .channel_offs
            .get((channel as usize).wrapping_sub(1))
            .ok_or_else(|| Error::Format(format!("SFA channel {channel} out of range")))? as usize;
        (0..self.nframes as usize)
            .map(|f| {
                let p = self.data_off as usize + f * self.frame_size as usize + o;
                Ok([f32_at(self.d, p)? as f64, f32_at(self.d, p + 4)? as f64, f32_at(self.d, p + 8)? as f64])
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn le32(d: &mut Vec<u8>, v: &[u32]) {
        for x in v {
            d.extend_from_slice(&x.to_le_bytes());
        }
    }

    /// Version-12 set with one CEDHAT_3.2 animation: one rotation channel with 2 keys.
    fn build() -> Vec<u8> {
        // bulk: bone map [3] at 0, channel record at 4, keys at 12
        let mut bulk = vec![3, 0, 0, 0];
        bulk.extend_from_slice(&[0, 0, 2, 0, 12, 0, 0, 0]);
        for k in [100i16, 0, -100, 200, 0, 0] {
            bulk.extend_from_slice(&k.to_le_bytes());
        }
        bulk.extend_from_slice(&[0, 10, 0, 0]);
        let mut s = vec![1];
        s.extend_from_slice(b"set\0");
        s.push(0);
        le32(&mut s, &[1]);
        s.push(1);
        s.extend_from_slice(b"run\0");
        le32(&mut s, &[0, 20]);
        s.extend_from_slice(&30f32.to_le_bytes());
        le32(&mut s, &[3, 1, 0, 0b101, 0, 0, 0, 0, 0]); // mask0 (3 bits), five absent masks
        le32(&mut s, &[0]); // events
        s.push(0); // packed
        le32(&mut s, &[0]); // channel table
        s.push(0); // no root
        le32(&mut s, &[CEDHAT_3_2, 8, 1, 0, 4]);
        le32(&mut s, &[0, 0, 0, 0, 0, 0]);
        for x in [100f32, 0.01, 1.0, 1.0, 1.0, 1.0] {
            s.extend_from_slice(&x.to_le_bytes());
        }
        le32(&mut s, &[0]); // list4c
        for _ in 0..6 {
            s.extend_from_slice(&0f32.to_le_bytes());
        }
        le32(&mut s, &[0, 7, 8, 9]); // list50, tail
        let mut d = Vec::new();
        le32(&mut d, &[MAGIC, 0x300040, 5, 12, 0, s.len() as u32, bulk.len() as u32]);
        d.extend(bulk);
        d.extend(s);
        d
    }

    #[test]
    fn parse_and_sample() {
        let d = build();
        let f = Fas::parse(&d).unwrap();
        assert_eq!(f.name.as_deref(), Some("set"));
        assert_eq!(f.tail, Some([7, 8, 9]));
        let a = &f.anims[0];
        assert_eq!(a.masks[0].as_ref().unwrap().bits(), vec![0, 2]);
        let ch = f.channels(a, 0, None).unwrap();
        assert_eq!(ch[0].bone, 3);
        assert_eq!(ch[0].times, vec![0, 10]);
        assert!((ch[0].keys[0][0] - 1.0).abs() < 1e-6);
        let mid = ch[0].sample(5.0, a.frames);
        assert!((mid[0] - 1.5).abs() < 1e-6 && (mid[2] + 0.5).abs() < 1e-6);
        // past the last key it blends back to key 0 at the anim length (20)
        assert!((ch[0].sample(15.0, a.frames)[0] - 1.5).abs() < 1e-6);
        assert!(f.channels(a, 1, None).unwrap().is_empty());
    }

    #[test]
    fn truncated_stream_fails() {
        let mut d = build();
        d.pop();
        assert!(Fas::parse(&d).is_err());
    }

    #[test]
    fn sfa_frames() {
        let mut d = b"SFA0".to_vec();
        le32(&mut d, &[0x10]);
        for v in [1u16, 2, 18, 12, 0] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        for x in [1f32, 2., 3., 4., 5., 6.] {
            d.extend_from_slice(&x.to_le_bytes());
        }
        let s = Sfa::parse(&d).unwrap();
        assert_eq!(s.frames(1).unwrap(), vec![[1., 2., 3.], [4., 5., 6.]]);
        assert!(s.frames(2).is_err());
    }
}
