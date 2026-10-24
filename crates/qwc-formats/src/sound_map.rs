//! Sound-effect category maps (`.map`, EA UK audio library; port of `tools/map_parse.py`, loader
//! `TbMapStreamer::RegisterCatagory` `0x558230`): which bank and sound each effect event of a category plays.
//!
//! ```text
//! 0x00 GUID {e9612c01-31d0-11d2-b409-00c0c993f203} | u32 0
//! then two records {u32 size, GUID, body, 1-4 zero bytes to a 4-byte boundary}; size counts from the GUID
//! bank list {..-00a0..}: u32 0 | u32 0 | u32 n | n x 11 B {9 B zero, u8 flags, u8 0} | n x {u32 len, char[len]}
//! sounds    {..-00b0..}: u32 2 | u32 stamp | u32 n_groups | groups (24 B) | per group its events (20 B), per
//!           event its variants (42 B), per variant its samples (16 B) then its links (8 B)
//! group   {u32 n_events, u32 id, u32 flags, f32 1.0, u32, u32}
//! event   {u32 event_id, u32 n_variants, u32 id, u32 value, u32 flags}
//! variant {u32 n_samples, u32 n_links, 34 B not decoded}
//! sample  {u32 sound (1-based in the bank), u32 weight (cumulative, /65535), u32 0, u16 bank (1-based), u16 0}
//! link    {u32 variant (1-based within the event), u32 value}
//! ```
//! Bank names are `Global\<bank>`; `banks.fat` names the bank file `fat_hash(<bank> + ".sdt")`.

use crate::util::{bad, f32_at, slice, u16_at, u32_at, Result};

pub const GUID_FILE: [u8; 16] = *b"\x01\x2c\x61\xe9\xd0\x31\xd2\x11\xb4\x09\x00\xc0\xc9\x93\xf2\x03";
pub const GUID_BANKS: [u8; 16] = *b"\x01\x2c\x61\xe9\xd0\x31\xd2\x11\xb4\x09\x00\xa0\xc9\x93\xf2\x03";
pub const GUID_SOUNDS: [u8; 16] = *b"\x00\x2c\x61\xe9\xd0\x31\xd2\x11\xb4\x09\x00\xb0\xc9\x93\xf2\x03";

#[derive(Debug, Clone)]
pub struct Bank {
    pub name: String,
    /// 0x01 memory bank (`BNKl`), 0x02 streaming bank
    pub flags: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub sound: u32,
    pub weight: u32,
    pub unk_08: u32,
    pub bank: u16,
    pub unk_0e: u16,
}

#[derive(Debug, Clone)]
pub struct Variant {
    pub samples: Vec<Sample>,
    /// (variant, value)
    pub links: Vec<(u32, u32)>,
    /// bytes 8..42, not decoded
    pub fields: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Event {
    pub event_id: u32,
    pub id: u32,
    pub value: u32,
    pub flags: u32,
    pub variants: Vec<Variant>,
}

#[derive(Debug, Clone)]
pub struct Group {
    pub id: u32,
    pub flags: u32,
    pub scale: f32,
    pub unk: [u32; 2],
    pub events: Vec<Event>,
}

#[derive(Debug, Clone)]
pub struct SoundMap {
    pub banks: Vec<Bank>,
    pub stamp: u32,
    pub groups: Vec<Group>,
}

/// Records are followed by 1-4 zero bytes up to the next 4-byte boundary (an aligned end still gets 4).
fn padded_end(d: &[u8], end: usize) -> Result<usize> {
    let pad = 4 - end % 4;
    if slice(d, end, pad)?.iter().any(|&b| b != 0) {
        return bad(format!("non-zero padding at {end:#x}"));
    }
    Ok(end + pad)
}

impl SoundMap {
    pub fn parse(d: &[u8]) -> Result<SoundMap> {
        if slice(d, 0, 16)? != GUID_FILE || u32_at(d, 16)? != 0 {
            return bad("not a .map file");
        }
        let mut records = Vec::new();
        let mut pos = 0x14;
        while pos < d.len() {
            let size = u32_at(d, pos)? as usize;
            records.push((slice(d, pos + 4, 16)?, pos + 20, pos + 4 + size));
            pos += 4 + size;
        }
        if pos != d.len() || records.len() != 2 || records[0].0 != GUID_BANKS || records[1].0 != GUID_SOUNDS {
            return bad("unexpected record layout");
        }
        let banks = Self::banks(d, records[0].1, records[0].2)?;
        let (stamp, groups) = Self::sounds(d, records[1].1, records[1].2)?;
        let m = SoundMap { banks, stamp, groups };
        for e in m.events() {
            for v in &e.variants {
                if v.samples.iter().any(|s| s.bank == 0 || s.bank as usize > m.banks.len()) {
                    return bad(format!("event {}: bank out of range", e.event_id));
                }
                if v.links.iter().any(|&(l, _)| l == 0 || l as usize > e.variants.len()) {
                    return bad(format!("event {}: link out of range", e.event_id));
                }
            }
        }
        Ok(m)
    }

    fn banks(d: &[u8], p: usize, end: usize) -> Result<Vec<Bank>> {
        if u32_at(d, p)? != 0 || u32_at(d, p + 4)? != 0 {
            return bad("bank list header is not zero");
        }
        let n = u32_at(d, p + 8)? as usize;
        let mut q = p + 12 + 11 * n;
        let mut banks = Vec::new();
        for i in 0..n {
            let e = slice(d, p + 12 + 11 * i, 11)?;
            if e[..9].iter().any(|&b| b != 0) || e[10] != 0 {
                return bad("unexpected bank entry bytes");
            }
            let len = u32_at(d, q)? as usize;
            let name = slice(d, q + 4, len)?;
            let Some((&0, name)) = name.split_last() else {
                return bad("bank name is not NUL-terminated");
            };
            banks.push(Bank { name: crate::util::latin1(name), flags: e[9] });
            q += 4 + len;
        }
        if padded_end(d, q)? != end {
            return bad("bank list does not fill its record");
        }
        Ok(banks)
    }

    fn sounds(d: &[u8], p: usize, end: usize) -> Result<(u32, Vec<Group>)> {
        let (version, stamp, n_groups) = (u32_at(d, p)?, u32_at(d, p + 4)?, u32_at(d, p + 8)? as usize);
        if version != 2 {
            return bad(format!("sound table version {version}"));
        }
        let mut q = p + 12;
        let mut heads = Vec::new();
        for i in 0..n_groups {
            let g = q + 24 * i;
            heads.push((
                u32_at(d, g)? as usize,
                u32_at(d, g + 4)?,
                u32_at(d, g + 8)?,
                f32_at(d, g + 12)?,
                [u32_at(d, g + 16)?, u32_at(d, g + 20)?],
            ));
        }
        q += 24 * n_groups;
        let mut groups = Vec::new();
        for (n_events, id, flags, scale, unk) in heads {
            let mut ev_heads = Vec::new();
            for i in 0..n_events {
                let e = q + 20 * i;
                ev_heads.push([0, 4, 8, 12, 16].map(|o| u32_at(d, e + o)).into_iter().collect::<Result<Vec<_>>>()?);
            }
            q += 20 * n_events;
            let mut events = Vec::new();
            for h in ev_heads {
                let n_var = h[1] as usize;
                let raw: Vec<&[u8]> = (0..n_var).map(|i| slice(d, q + 42 * i, 42)).collect::<Result<_>>()?;
                q += 42 * n_var;
                let mut variants = Vec::new();
                for v in raw {
                    let (n_samples, n_links) = (u32_at(v, 0)? as usize, u32_at(v, 4)? as usize);
                    let mut samples = Vec::new();
                    for i in 0..n_samples {
                        let s = q + 16 * i;
                        samples.push(Sample {
                            sound: u32_at(d, s)?,
                            weight: u32_at(d, s + 4)?,
                            unk_08: u32_at(d, s + 8)?,
                            bank: u16_at(d, s + 12)?,
                            unk_0e: u16_at(d, s + 14)?,
                        });
                    }
                    q += 16 * n_samples;
                    let links = (0..n_links)
                        .map(|i| Ok((u32_at(d, q + 8 * i)?, u32_at(d, q + 8 * i + 4)?)))
                        .collect::<Result<_>>()?;
                    q += 8 * n_links;
                    variants.push(Variant { samples, links, fields: v[8..].to_vec() });
                }
                events.push(Event { event_id: h[0], id: h[2], value: h[3], flags: h[4], variants });
            }
            groups.push(Group { id, flags, scale, unk, events });
        }
        if padded_end(d, q)? != end {
            return bad(format!("sound table ends at {q:#x}, record at {end:#x}"));
        }
        Ok((stamp, groups))
    }

    pub fn events(&self) -> impl Iterator<Item = &Event> {
        self.groups.iter().flat_map(|g| &g.events)
    }
}

/// `banks.fat` key of a bank named in a `.map` (`Global\Foley` -> `fat_hash("Foley.sdt")`).
pub fn bank_file_hash(bank_name: &str) -> u32 {
    crate::audio::fat_hash(&format!("{}.sdt", bank_name.rsplit('\\').next().unwrap_or(bank_name)))
}
