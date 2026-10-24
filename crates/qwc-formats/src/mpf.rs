//! Interactive music (`Audio/Music/*/*.mpf`, EA PathFinder 4.0; port of `tools/mpf_parse.py`): the music graph and
//! the table of streams in the paired `.mus` (paired by `0x5262b0`).
//!
//! ```text
//! 0x00 "xDFP" | u8 4 | u8 0 | ... | 0x0d u8 n_tracks | u8 n_sections | u8 n_events | u8 n_routers | u8 n_vars
//! 0x12 u16 n_nodes | ... | 0x20 u16 node_offset[n_nodes] (4-byte units)
//! node  u16 sample (1-based across all tracks; 0 or 0xffff none) | 14 B (BE u32 at +4, bits 20-23 = n_branches)
//!       | n_branches x {u8, u8, u16 target node (0xffff none)}
//! the last node is followed by u16 event_offset[n_events] (4-byte units)
//! event 16 B (BE u32 at +0x0c, bits 8-13 = n_actions) + n_actions x 16 B
//! the last event is followed by n_routers x 4 B and u32 track_table / 4
//! track table: u32 track_start[n_tracks + 1] / 4 (the last is the file end), samples x 8 B
//!   {u32 offset in the .mus / 0x80, u32 duration ms}
//! ```

use crate::util::{bad, slice, u16_at, u32_at, Result};

#[derive(Debug, Clone)]
pub struct Branch {
    pub a: u8,
    pub b: u8,
    pub target: u16,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub offset: usize,
    pub sample: u16,
    /// bytes 2..16, not decoded
    pub fields: Vec<u8>,
    pub branches: Vec<Branch>,
}

#[derive(Debug, Clone)]
pub struct Event {
    pub offset: usize,
    pub head: Vec<u8>,
    pub actions: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, Copy)]
pub struct Sample {
    /// byte offset of the SCHl stream in the `.mus`
    pub offset: u32,
    pub duration_ms: u32,
}

#[derive(Debug, Clone)]
pub struct MusicFile {
    pub n_sections: u8,
    pub n_vars: u8,
    pub nodes: Vec<Node>,
    pub events: Vec<Event>,
    pub routers: Vec<Vec<u8>>,
    pub tracks: Vec<Vec<Sample>>,
}

fn be32(d: &[u8], o: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(slice(d, o, 4)?.try_into().unwrap()))
}

impl MusicFile {
    pub fn parse(d: &[u8]) -> Result<MusicFile> {
        if slice(d, 0, 0x20)?[..5] != *b"xDFP\x04" {
            return bad("not a PathFinder 4 .mpf file");
        }
        let (n_tracks, n_sections, n_events, n_routers, n_vars) =
            (d[0x0d] as usize, d[0x0e], d[0x0f] as usize, d[0x10] as usize, d[0x11]);
        let n_nodes = u16_at(d, 0x12)? as usize;
        if n_nodes == 0 || n_events == 0 {
            return bad("no nodes or events");
        }
        let mut nodes = Vec::new();
        for i in 0..n_nodes {
            let o = u16_at(d, 0x20 + 2 * i)? as usize * 4;
            let n_br = ((be32(d, o + 4)? >> 20) & 0xf) as usize;
            let branches = (0..n_br)
                .map(|k| {
                    let b = slice(d, o + 0x10 + 4 * k, 4)?;
                    Ok(Branch { a: b[0], b: b[1], target: u16_at(b, 2)? })
                })
                .collect::<Result<_>>()?;
            nodes.push(Node { offset: o, sample: u16_at(d, o)?, fields: slice(d, o + 2, 14)?.to_vec(), branches });
        }
        let last = nodes.last().unwrap();
        let p = last.offset + 0x10 + 4 * last.branches.len();
        let mut events = Vec::new();
        for i in 0..n_events {
            let o = u16_at(d, p + 2 * i)? as usize * 4;
            let n_act = ((be32(d, o + 0x0c)? >> 8) & 0x3f) as usize;
            let actions =
                (0..n_act).map(|k| Ok(slice(d, o + 0x10 * (k + 1), 0x10)?.to_vec())).collect::<Result<_>>()?;
            events.push(Event { offset: o, head: slice(d, o, 0x10)?.to_vec(), actions });
        }
        let last = events.last().unwrap();
        let p = last.offset + 0x10 * (1 + last.actions.len());
        let routers = (0..n_routers).map(|i| Ok(slice(d, p + 4 * i, 4)?.to_vec())).collect::<Result<_>>()?;
        let table = u32_at(d, p + 4 * n_routers)? as usize * 4;
        let starts: Vec<usize> =
            (0..=n_tracks).map(|i| Ok(u32_at(d, table + 4 * i)? as usize * 4)).collect::<Result<_>>()?;
        if starts[n_tracks] != d.len() {
            return bad("track table does not end at the file end");
        }
        let mut tracks = Vec::new();
        let mut a = table + 4 * (n_tracks + 1);
        for &b in &starts[1..] {
            if a > b || !(b - a).is_multiple_of(8) {
                return bad("bad track bounds");
            }
            tracks.push(
                (a..b)
                    .step_by(8)
                    .map(|q| Ok(Sample { offset: u32_at(d, q)? * 0x80, duration_ms: u32_at(d, q + 4)? }))
                    .collect::<Result<_>>()?,
            );
            a = b;
        }
        let n_samples: usize = tracks.iter().map(Vec::len).sum();
        for n in &nodes {
            if n.sample != 0xffff && n.sample as usize > n_samples {
                return bad(format!("node at {:#x}: sample out of range", n.offset));
            }
            if n.branches.iter().any(|b| b.target != 0xffff && b.target as usize >= n_nodes) {
                return bad(format!("node at {:#x}: branch out of range", n.offset));
            }
        }
        Ok(MusicFile { n_sections, n_vars, nodes, events, routers, tracks })
    }
}
