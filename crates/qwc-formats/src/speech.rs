//! The speech library's data (EA UK's, `0x552ae0`-`0x556400`; port of `tools/speech_parse.py`, whose docstring
//! has the layouts): clip banks (the `.hdr` files' data after the path) and the speech events (`Events.evt`).
//!
//! A bank lists its clips (SCHl streams in `dat.dat`, in order), each with `k` selection keys; an event's variants
//! name the banks their parts pick one clip from, with the key values each part allows.

use crate::util::{bad, Result};

fn align4(x: usize) -> usize {
    (x + 3) & !3
}

fn u16le(d: &[u8], o: usize) -> Result<u16> {
    d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).map_or_else(|| bad("truncated"), Ok)
}

fn u32le(d: &[u8], o: usize) -> Result<u32> {
    d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).map_or_else(|| bad("truncated"), Ok)
}

fn byte(d: &[u8], o: usize) -> Result<u8> {
    d.get(o).copied().map_or_else(|| bad("truncated"), Ok)
}

/// One clip: its offset in the bank's `.dat` file (0x100-byte units) and its keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BankClip {
    pub offset: u16,
    pub keys: Vec<u8>,
}

/// A clip bank (`.hdr`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bank {
    /// The id events use; several commentators share ids.
    pub id: u16,
    /// 1..13 for per-team banks.
    pub team: Option<u16>,
    /// Keys per clip.
    pub keys: u8,
    /// The no-repeat history's length.
    pub history: u8,
    /// End of the stream run in `dat.dat` (0x100-byte units).
    pub size: u16,
    pub clips: Vec<BankClip>,
    /// Per key, bit `v` set when some clip has value `v`.
    pub key_masks: Vec<u32>,
    /// Partitions (a partitioned bank enables `n / parts` clips per session).
    pub parts: Option<u8>,
}

fn pad(b: &[u8], mut o: usize) -> Result<usize> {
    while !o.is_multiple_of(4) {
        let v = byte(b, o)?;
        if v != 0x70 {
            return bad(format!("pad byte 0x{v:02x} at +{o}"));
        }
        o += 1;
    }
    Ok(o)
}

/// Parse a bank; fails unless every byte is accounted for.
pub fn parse_bank(b: &[u8]) -> Result<Bank> {
    let (id, team, flags, n, h, zero7, size, rows) =
        (u16le(b, 0)?, u16le(b, 2)?, byte(b, 4)?, byte(b, 5)?, byte(b, 6)?, byte(b, 7)?, u16le(b, 8)?, u16le(b, 10)?);
    let k = flags & 0x7f;
    if zero7 != 0 || rows != 0 {
        return bad("unexpected header bytes");
    }
    let mut clips = Vec::with_capacity(n as usize);
    let mut o = 12;
    for _ in 0..n {
        let offset = u16::from_be_bytes([byte(b, o)?, byte(b, o + 1)?]);
        let keys = b.get(o + 2..o + 2 + k as usize).map_or_else(|| bad("truncated"), |s| Ok(s.to_vec()))?;
        clips.push(BankClip { offset, keys });
        o += 2 + k as usize;
    }
    o = pad(b, o)?;
    let mut key_masks = Vec::with_capacity(k as usize);
    for j in 0..k as usize {
        key_masks.push(u32le(b, o + 4 * j)?);
    }
    o += 4 * k as usize;
    for (j, &mask) in key_masks.iter().enumerate() {
        let m = clips.iter().fold(0u32, |m, c| m | 1u32.checked_shl(c.keys[j] as u32).unwrap_or(0));
        if m != mask {
            return bad(format!("key {j} mask 0x{mask:x}, clips give 0x{m:x}"));
        }
    }
    if clips.windows(2).any(|w| w[0].offset >= w[1].offset) || clips.first().is_some_and(|c| c.offset != 0) {
        return bad("clip offsets do not start at 0 and rise");
    }
    let mut parts = None;
    if flags & 0x80 != 0 {
        parts = Some(byte(b, o)?);
        let bits = (n as usize).div_ceil(8);
        if b.get(o + 1..o + 1 + bits).is_none_or(|s| s.iter().any(|&v| v != 0)) {
            return bad("partition bitset is not clear");
        }
        o += 1 + bits;
    }
    if h != 0 {
        let ring = b.get(o + 1..o + 1 + h as usize);
        if byte(b, o)? != 0 || ring.is_none_or(|s| s.iter().any(|&v| v != 0xff)) {
            return bad("history is not clear");
        }
        o += 1 + h as usize;
    }
    o = pad(b, o)?;
    if o != b.len() {
        return bad(format!("{} bytes left over", b.len() - o));
    }
    Ok(Bank { id, team: (team != 0xffff).then_some(team), keys: k, history: h, size, clips, key_masks, parts })
}

/// A part's key constraint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartKey {
    pub allowed: u32,
    /// 0 a fixed mask, 1..n the event argument whose bitmask must also allow the value, 0xfe remember the chosen
    /// value, 0xff compare with a remembered one.
    pub source: u8,
    pub x5: u8,
    pub r#ref: u8,
    pub x7: u8,
}

/// A part: one clip from a bank.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub bank: u16,
    pub arg: u8,
    /// 0 any team, 1 team = the argument, 2 team or generic.
    pub bank_mode: u8,
    pub keys: Vec<PartKey>,
}

/// A variant: a line, its parts played in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    pub weight: u32,
    pub chance: u8,
    pub test_mask: u8,
    pub test_value: u8,
    pub mode: u8,
    pub slots: u8,
    pub masks: Vec<u32>,
    pub parts: Vec<Part>,
}

/// A speech event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub id: u16,
    pub t0: u16,
    pub t1: u16,
    pub max_repeats: u8,
    /// The chance it speaks (%).
    pub chance: u8,
    pub flags: u8,
    /// Its context arguments.
    pub args: u8,
    pub tests: Vec<[u8; 3]>,
    pub arg_tests: Vec<[u8; 3]>,
    /// With flags & 8: 2-bit tests per argument and a group per variant.
    pub arg_modes: Option<u32>,
    pub groups: Option<Vec<u16>>,
    /// With flags & 0x10: event ids (inferred).
    pub linked: Option<Vec<u16>>,
    pub variants: Vec<Variant>,
    pub name: String,
}

/// An `Events.evt` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Events {
    /// Game 0/0, FE 0/1.
    pub set: [u8; 2],
    pub unknown: u32,
    pub defaults: [u16; 2],
    pub events: Vec<Event>,
}

pub const EVT_MAGIC: u32 = 0x073c_0f03;

/// Parse an `Events.evt`; fails unless every non-zero byte belongs to exactly one structure.
pub fn parse_evt(d: &[u8]) -> Result<Events> {
    let (magic, size) = (u32le(d, 0)?, u32le(d, 4)? as usize);
    if magic != EVT_MAGIC || size > d.len() {
        return bad("not an Events.evt file");
    }
    let (set_lo, set_hi, unk, n, n_state, d0, d1) = (
        byte(d, 8)?,
        byte(d, 9)?,
        u32le(d, 12)?,
        u16le(d, 16)? as usize,
        byte(d, 18)? as usize,
        u16le(d, 20)?,
        u16le(d, 22)?,
    );
    let mut owner = vec![false; d.len()];
    let mut claim = |a: usize, b: usize| -> Result<()> {
        for (i, o) in owner.iter_mut().enumerate().take(b.min(d.len())).skip(a) {
            if *o {
                return bad(format!("bytes at 0x{i:x} are claimed twice"));
            }
            *o = true;
        }
        Ok(())
    };
    claim(0, 0x18 + align4(2 * n) + n_state)?;
    let pool = size + 32 * n;
    let mut events = Vec::with_capacity(n);
    for i in 0..n {
        let e = u16le(d, 0x18 + 2 * i)? as usize * 4;
        let (id, t0, t1) = (u16le(d, e)?, u16le(d, e + 2)?, u16le(d, e + 4)?);
        let (nv, lh, rep, chance, flags, n_args) = (
            byte(d, e + 6)? as usize,
            byte(d, e + 7)?,
            byte(d, e + 8)?,
            byte(d, e + 9)?,
            byte(d, e + 10)?,
            byte(d, e + 11)?,
        );
        let (lo, hi) = ((lh & 15) as usize, (lh >> 4) as usize);
        let voffs: Vec<usize> = (0..nv).map(|j| u16le(d, e + 12 + 2 * j).map(|v| v as usize)).collect::<Result<_>>()?;
        let mut p = e + 12 + align4(2 * nv);
        let three = |at: usize| -> Result<[u8; 3]> { Ok([byte(d, at)?, byte(d, at + 1)?, byte(d, at + 2)?]) };
        let tests: Vec<[u8; 3]> = (0..lo).map(|j| three(p + 3 * j)).collect::<Result<_>>()?;
        p += ((lo + 1) * 3) & !3;
        let arg_tests: Vec<[u8; 3]> = (0..hi).map(|j| three(p + 1 + 3 * j)).collect::<Result<_>>()?;
        p += ((hi + 1) * 3) & !3;
        let (mut arg_modes, mut groups, mut linked) = (None, None, None);
        if flags & 8 != 0 {
            arg_modes = Some(u32le(d, p)?);
            groups = Some((0..nv).map(|j| u16le(d, p + 4 + 2 * j)).collect::<Result<Vec<_>>>()?);
            p += align4(2 * nv + 4);
        }
        if flags & 0x10 != 0 {
            let cnt = byte(d, p)? as usize;
            linked = Some((0..cnt).map(|j| u16le(d, p + 2 + 2 * j)).collect::<Result<Vec<_>>>()?);
            p += align4(2 + 2 * cnt);
        }
        claim(e, p)?;
        let mut variants = Vec::with_capacity(nv);
        for &vo in &voffs {
            let v = e + 4 * vo;
            let head = d.get(v..v + 8).map_or_else(|| bad("truncated"), |s| Ok(s.to_vec()))?;
            let (weight, vchance, tmask, tval, pm, slots, h) =
                (head[0], head[1], head[2], head[3], head[4], head[5], head[6]);
            let nparts = (pm >> 2) as usize;
            let mut q = v + 8 + align4(nparts);
            let masks: Vec<u32> = (0..h as usize).map(|j| u32le(d, q + 4 * j)).collect::<Result<_>>()?;
            q += 4 * h as usize + 4 * slots as usize;
            claim(v, q)?;
            let mut parts = Vec::with_capacity(nparts);
            for j in 0..nparts {
                let s = v + 4 * byte(d, v + 8 + j)? as usize;
                let (bank, arg, bank_mode, k) =
                    (u16le(d, s)?, byte(d, s + 2)?, byte(d, s + 3)?, byte(d, s + 4)? as usize);
                let keys = (0..k)
                    .map(|j| {
                        let o = s + 8 + 8 * j;
                        Ok(PartKey {
                            allowed: u32le(d, o)?,
                            source: byte(d, o + 4)?,
                            x5: byte(d, o + 5)?,
                            r#ref: byte(d, o + 6)?,
                            x7: byte(d, o + 7)?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                claim(s, s + 8 + 8 * k)?;
                parts.push(Part { bank, arg, bank_mode, keys });
            }
            variants.push(Variant {
                weight: (weight & 0x1f) as u32 * 4u32.pow((weight >> 5) as u32),
                chance: vchance,
                test_mask: tmask,
                test_value: tval,
                mode: pm & 3,
                slots,
                masks,
                parts,
            });
        }
        let rec = size + 32 * i;
        let name_off = u32le(d, rec)? as usize;
        if d.get(rec + 8..rec + 32).is_none_or(|s| s.iter().any(|&v| v != 0)) {
            return bad(format!("name record {i} has non-zero padding"));
        }
        let start = pool + name_off;
        let end = d.get(start..).and_then(|s| s.iter().position(|&c| c == 0)).map(|z| start + z);
        let name: String =
            d[start..end.map_or_else(|| bad("unterminated name"), Ok)?].iter().map(|&c| c as char).collect();
        events.push(Event {
            id,
            t0,
            t1,
            max_repeats: rep,
            chance,
            flags,
            args: n_args,
            tests,
            arg_tests,
            arg_modes,
            groups,
            linked,
            variants,
            name,
        });
    }
    if let Some(i) = (0..size).find(|&i| !owner[i] && d[i] != 0) {
        return bad(format!("unclaimed non-zero bytes at 0x{i:x}"));
    }
    Ok(Events { set: [set_lo, set_hi], unknown: unk, defaults: [d0, d1], events })
}

/// The clip of `bank` whose keys are all allowed by `masks` (one bitmask per key), the first in order.
pub fn pick(bank: &Bank, masks: &[u32]) -> Option<usize> {
    bank.clips.iter().position(|c| {
        c.keys.iter().zip(masks).all(|(&v, &m)| 1u32.checked_shl(v as u32).is_some_and(|bit| m & bit != 0))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bank of three clips with one key (values 0, 1, 2), no history.
    fn bank_bytes() -> Vec<u8> {
        let mut b = vec![7, 0, 0xff, 0xff, 1, 3, 0, 0, 4, 0, 0, 0];
        for (off, key) in [(0u16, 0u8), (1, 1), (3, 2)] {
            b.extend(off.to_be_bytes());
            b.push(key);
        }
        // 12 + 9 = 21 -> pad to 24
        b.extend([0x70; 3]);
        b.extend(0b111u32.to_le_bytes());
        b
    }

    #[test]
    fn a_bank_parses_and_picks() {
        let b = parse_bank(&bank_bytes()).unwrap();
        assert_eq!((b.id, b.team, b.keys, b.clips.len()), (7, None, 1, 3));
        assert_eq!(b.clips[2], BankClip { offset: 3, keys: vec![2] });
        assert_eq!(pick(&b, &[0b100]), Some(2));
        assert_eq!(pick(&b, &[0b1000]), None);
        // a wrong mask or a stray byte is refused
        let mut bad = bank_bytes();
        let n = bad.len();
        bad[n - 4] = 0b11;
        assert!(parse_bank(&bad).is_err());
        let mut extra = bank_bytes();
        extra.extend([0x70; 4]);
        assert!(parse_bank(&extra).is_err());
    }
}
