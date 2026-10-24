//! Audio containers (index side of `tools/audio_parse.py`; the codecs are in [`crate::ea_audio`]).
//!
//! ```text
//! SCHl stream  blocks {char[4] tag, u32 size incl. the 8-byte head}: SCHl SCCl SCDl... SCEl. The SCHl body is
//!              "PT\0\0" then {u8 tag, u8 n, n bytes big-endian}; 0xfc-0xfe have no length, 0xff ends it.
//!              0x82 channels (default 1), 0x84 rate (default 22050), 0x85 samples, 0xa0 codec (4 = MicroTalk)
//! .fat         u32 0x18 | u32 1 | u32 8 | u32 12 | u32 n_nodes | u32 n_files
//!              node {u32 hash(dir), u16 first file, u8 n_files, u8 n_subdirs}; file {u32 hash(name), u32 off, u32 size}
//! .hdr         u32 data offset | NUL-terminated virtual path | sentence data
//! dat.dat      per file: SCHl streams on 0x100 boundaries, zero padding, "HrIn" sentence data "HrSz" u32be length
//! banks.dat    per file: a memory bank ("BNKl", version 5) or u32 n, n x u32 stream offsets, padding to 0x80, streams
//! ```

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use crate::util::{bad, latin1, slice, u16_at, u32_at, u8_at, Error, Result};

const BLOCK_TAGS: [&[u8]; 4] = [b"SCHl", b"SCCl", b"SCDl", b"SCEl"];

/// `FUN_00518cd0`: lower-case ELF hash, '/' treated as '\\'. `text` is Latin-1.
pub fn fat_hash(text: &str) -> u32 {
    let mut h: u32 = 0;
    for c in text.chars() {
        let ch = match c as u32 as u8 {
            0x2F => 0x5C,
            c @ 0x41..=0x5A => c + 32,
            c => c,
        };
        h = (h << 4).wrapping_add(ch as u32);
        let g = h & 0xF000_0000;
        if g != 0 {
            h ^= g >> 24;
        }
        h &= !g;
    }
    h
}

/// Python slice `d[a:b]` (clamped, empty when reversed).
fn pyslice(d: &[u8], a: usize, b: usize) -> &[u8] {
    let b = b.min(d.len());
    d.get(a.min(b)..b).unwrap_or(&[])
}

/// The SCHl header body at `off` ("PT\0\0"...): tag -> value.
pub fn parse_pt(d: &[u8], off: usize) -> Result<BTreeMap<u8, u64>> {
    if pyslice(d, off, off + 4) != b"PT\0\0" {
        return bad(format!("SCHl at {:#x} has no PT header", off.wrapping_sub(8)));
    }
    let mut off = off + 4;
    let mut tags = BTreeMap::new();
    loop {
        let tag = u8_at(d, off)?;
        off += 1;
        match tag {
            0xFF => return Ok(tags),
            0xFC..=0xFE => continue,
            _ => {}
        }
        let n = u8_at(d, off)? as usize;
        if n > 8 {
            return bad(format!("PT tag {tag:#x} at {off:#x} has {n} bytes"));
        }
        tags.insert(tag, slice(d, off + 1, n)?.iter().fold(0u64, |v, &b| v << 8 | b as u64));
        off += 1 + n;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stream {
    pub offset: usize,
    pub size: usize,
    pub blocks: u32,
    pub channels: u64,
    pub rate: u64,
    pub samples: Option<u64>,
    /// "ea-xa", "microtalk" or "?".
    pub codec: &'static str,
}

/// Follows one SCHl...SCEl stream at `off` within `d[..end]`. Returns the stream and the offset after SCEl.
pub fn walk_stream(d: &[u8], off: usize, end: Option<usize>) -> Result<(Stream, usize)> {
    let end = end.unwrap_or(d.len());
    if pyslice(d, off, off + 4) != b"SCHl" {
        return bad(format!("no SCHl at {off:#x}"));
    }
    let start = off;
    let tags = parse_pt(d, off + 8)?;
    let (mut off, mut blocks) = (off, 0);
    loop {
        if off + 8 > end {
            return bad(format!("stream at {start:#x} runs past its container"));
        }
        let tag = slice(d, off, 4)?;
        let size = u32_at(d, off + 4)? as usize;
        if !BLOCK_TAGS.contains(&tag) || size < 8 || off + size > end {
            return bad(format!("bad block {:?} at {off:#x}", latin1(tag)));
        }
        off += size;
        blocks += 1;
        if tag == b"SCEl" {
            break;
        }
    }
    let codec = match tags.get(&0xA0) {
        None => "ea-xa",
        Some(4) => "microtalk",
        Some(_) => "?",
    };
    let s = Stream {
        offset: start,
        size: off - start,
        blocks,
        channels: tags.get(&0x82).copied().unwrap_or(1),
        rate: tags.get(&0x84).copied().unwrap_or(22050),
        samples: tags.get(&0x85).copied(),
        codec,
    };
    Ok((s, off))
}

/// Every stream in `d[start..end]`, each starting on an `align` boundary; the gaps must be zero.
pub fn stream_run(d: &[u8], start: usize, end: usize, align: usize) -> Result<Vec<Stream>> {
    let (mut out, mut off) = (Vec::new(), start);
    while off < end {
        if pyslice(d, off, off + 4) != b"SCHl" {
            let nxt = (off - start) / align * align + align + start;
            if pyslice(d, off, nxt.min(end)).iter().any(|&b| b != 0) {
                return bad(format!("non-zero bytes between streams at {off:#x}"));
            }
            off = nxt;
            continue;
        }
        let (s, after) = walk_stream(d, off, Some(end))?;
        out.push(s);
        off = after;
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FatNode {
    pub hash: u32,
    pub first: u16,
    pub count: u8,
    pub subdirs: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FatFile {
    pub hash: u32,
    pub offset: u32,
    pub size: u32,
}

/// A `.fat` file table (depth-first directory nodes, each owning a contiguous range of files).
#[derive(Debug, Clone)]
pub struct Fat {
    pub nodes: Vec<FatNode>,
    pub files: Vec<FatFile>,
}

impl Fat {
    pub fn parse(d: &[u8]) -> Result<Fat> {
        let h: Vec<u32> = (0..6).map(|i| u32_at(d, 4 * i)).collect::<Result<_>>()?;
        let (n_nodes, n_files) = (h[4] as usize, h[5] as usize);
        if h[..4] != [0x18, 1, 8, 12] || d.len() != 0x18 + 8 * n_nodes + 12 * n_files {
            return bad("not a .fat table");
        }
        let nodes = (0..n_nodes)
            .map(|i| {
                let o = 0x18 + 8 * i;
                Ok(FatNode { hash: u32_at(d, o)?, first: u16_at(d, o + 4)?, count: d[o + 6], subdirs: d[o + 7] })
            })
            .collect::<Result<Vec<_>>>()?;
        let base = 0x18 + 8 * n_nodes;
        let files = (0..n_files)
            .map(|i| {
                let o = base + 12 * i;
                Ok(FatFile { hash: u32_at(d, o)?, offset: u32_at(d, o + 4)?, size: u32_at(d, o + 8)? })
            })
            .collect::<Result<Vec<_>>>()?;
        // every node's file range lies inside the table, and the ranges tile it in order
        let mut pos = 0;
        for n in &nodes {
            if n.count != 0 {
                if n.first as usize != pos {
                    return bad("node file ranges are not contiguous");
                }
                pos = n.first as usize + n.count as usize;
            }
        }
        if pos != n_files {
            return bad("node ranges do not cover the file table");
        }
        Ok(Fat { nodes, files })
    }

    pub fn directory_of(&self, index: usize) -> Option<&FatNode> {
        self.nodes.iter().find(|n| (n.first as usize..n.first as usize + n.count as usize).contains(&index))
    }

    /// `FUN_00518b90`: `path` relative to the table's root.
    pub fn lookup(&self, path: &str) -> Option<&FatFile> {
        let path = path.replace('/', "\\");
        let (dir, name) = path.rsplit_once('\\').unwrap_or(("", &path));
        let node = if dir.is_empty() {
            self.nodes.first()?
        } else {
            let h = fat_hash(dir);
            self.nodes.iter().find(|n| n.hash == h)?
        };
        let h = fat_hash(name);
        let first = (node.first as usize).min(self.files.len());
        let end = (node.first as usize + node.count as usize).min(self.files.len());
        self.files[first..end].iter().find(|f| f.hash == h)
    }
}

/// `(virtual path, data offset, sentence bytes)` of a speech `.hdr`.
pub fn read_hdr(d: &[u8]) -> Result<(String, usize, &[u8])> {
    let off = u32_at(d, 0)? as usize;
    let len = d[4..].iter().position(|&c| c == 0).ok_or_else(|| Error::Format("no NUL in .hdr path".into()))?;
    if off < 4 + len + 1 || off > d.len() {
        return bad("bad .hdr data offset");
    }
    Ok((latin1(&d[4..4 + len]), off, &d[off..]))
}

/// `(virtual path, streams)` for `<lang>/dat.dat`, named through the `.hdr` files (`(file name, bytes)`).
pub fn speech_index(
    dat: &[u8],
    fat: &Fat,
    hdrs: &[(String, Vec<u8>)],
    lang: &str,
) -> Result<Vec<(String, Vec<Stream>)>> {
    let mut names: HashMap<u32, String> = HashMap::new();
    let mut sentences: HashMap<u32, Vec<&[u8]>> = HashMap::new();
    let prefix = format!("Audio\\{lang}\\dat\\");
    for (f, bytes) in hdrs {
        let (path, _, sentence) = read_hdr(bytes)?;
        if !path.to_lowercase().starts_with(&prefix.to_lowercase()) {
            return bad(format!("{f}: unexpected path {path}"));
        }
        let rel: String = path.chars().skip(prefix.chars().count()).collect();
        let Some(hit) = fat.lookup(&rel) else {
            return bad(format!("{f}: {rel} is not in dat.fat"));
        };
        sentences.entry(hit.offset).or_default().push(sentence);
        names.insert(hit.offset, rel);
    }
    let mut files = fat.files.clone();
    files.sort_by_key(|f| f.offset);
    let (mut out, mut pos) = (Vec::new(), 0usize);
    for f in &files {
        let o = f.offset as usize;
        if o != (pos + 0xFF) & !0xFF {
            return bad("dat.dat files do not follow each other");
        }
        let end = o + f.size as usize;
        let no_trailer = || Error::Format(format!("dat.dat file at {o:#x} has no HrIn/HrSz trailer"));
        let n = u32::from_be_bytes(slice(dat, end.checked_sub(4).ok_or_else(no_trailer)?, 4)?.try_into().unwrap());
        let trailer = end.checked_sub(12 + n as usize).ok_or_else(no_trailer)?;
        if pyslice(dat, end - 8, end - 4) != b"HrSz" || pyslice(dat, trailer, trailer + 4) != b"HrIn" {
            return Err(no_trailer());
        }
        // a few .hdr files exist twice (LB "MCName0x" next to "MCNames0x"); the trailer matches one of them
        let body = pyslice(dat, trailer + 4, end - 8);
        if let Some(hs) = sentences.get(&f.offset) {
            if hs.iter().all(|h| body != &h[..h.len().min(n as usize)]) {
                return bad(format!("dat.dat trailer at {trailer:#x} differs from its .hdr"));
            }
        }
        let streams = stream_run(dat, o, trailer, 0x100)?;
        let name = names.get(&f.offset).cloned().unwrap_or_else(|| format!("unnamed_{:08x}.dat", f.hash));
        out.push((name, streams));
        pos = end;
    }
    if (pos + 0x7FF) & !0x7FF != dat.len() && pos != dat.len() {
        return bad("dat.dat has data after the last file");
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bank {
    /// A memory bank ("BNKl"): its sound count.
    Bnkl(u16),
    /// A streaming bank's streams (offsets are into banks.dat).
    Stream(Vec<Stream>),
}

/// `(hash, bank)` per file of banks.dat, in table order.
pub fn bank_index(data: &[u8], fat: &Fat) -> Result<Vec<(u32, Bank)>> {
    let mut out = Vec::new();
    for f in &fat.files {
        let (o, size) = (f.offset as usize, f.size as usize);
        if pyslice(data, o, o + 4) == b"BNKl" {
            let (version, count) = (u16_at(data, o + 4)?, u16_at(data, o + 6)?);
            if version != 5 || u32_at(data, o + 8)? as usize > size {
                return bad(format!("unexpected BNKl header at {o:#x}"));
            }
            out.push((f.hash, Bank::Bnkl(count)));
            continue;
        }
        let n = u32_at(data, o)? as usize;
        let offs: Vec<usize> =
            slice(data, o + 4, 4 * n)?.chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap()) as usize).collect();
        if 4 + 4 * n > 0x80
            || pyslice(data, o + 4 + 4 * n, o + 0x80).iter().any(|&b| b != 0)
            || offs.first() != Some(&0x80)
        {
            return bad(format!("bad streaming bank header at {o:#x}"));
        }
        let mut streams = Vec::new();
        for (k, &so) in offs.iter().enumerate() {
            let (s, after) = walk_stream(data, o + so, Some(o + size))?;
            let nxt = offs.get(k + 1).map_or(o + size, |&x| o + x);
            if pyslice(data, after, nxt).iter().any(|&b| b != 0) {
                return bad(format!("non-zero bytes after a stream in bank at {o:#x}"));
            }
            streams.push(s);
        }
        out.push((f.hash, Bank::Stream(streams)));
    }
    Ok(out)
}

fn read(p: &Path) -> Result<Vec<u8>> {
    std::fs::read(p).map_err(|e| Error::Format(format!("{}: {e}", p.display())))
}

/// [`speech_index`] for `Audio/<lang>/dat.dat` under a repo root (`original/cd_files` + `extracted`).
pub fn speech_index_at(root: &Path, lang: &str) -> Result<Vec<(String, Vec<Stream>)>> {
    let dat = read(&root.join("original/cd_files/Audio").join(lang).join("dat.dat"))?;
    let fat = Fat::parse(&read(&root.join("extracted/Audio").join(lang).join("fat/dat.fat"))?)?;
    let mut paths = Vec::new();
    walk(&root.join("extracted/Audio").join(lang).join("hdr"), &mut paths);
    paths.sort();
    let hdrs = paths
        .iter()
        .map(|p| Ok((p.file_name().unwrap().to_string_lossy().into_owned(), read(p)?)))
        .collect::<Result<Vec<_>>>()?;
    speech_index(&dat, &fat, &hdrs, lang)
}

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// [`bank_index`] for `Audio/banks.dat` under a repo root.
pub fn bank_index_at(root: &Path) -> Result<Vec<(u32, Bank)>> {
    let data = read(&root.join("original/cd_files/Audio/banks.dat"))?;
    bank_index(&data, &Fat::parse(&read(&root.join("extracted/Audio/cats/banks.fat"))?)?)
}

/// `(file name, streams)` for every `.mus` under a repo root, sorted by name.
pub fn music_index(root: &Path) -> Result<Vec<(String, Vec<Stream>)>> {
    let dir = root.join("original/cd_files/Audio/Music");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .map_err(|e| Error::Format(format!("{}: {e}", dir.display())))?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|f| f.to_lowercase().ends_with(".mus"))
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|f| {
            let d = read(&dir.join(&f))?;
            Ok((f, stream_run(&d, 0, d.len(), 0x80)?))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(tag: &[u8], body: &[u8]) -> Vec<u8> {
        let mut v = tag.to_vec();
        v.extend_from_slice(&(8 + body.len() as u32).to_le_bytes());
        v.extend_from_slice(body);
        v
    }

    fn schl(samples: u32, codec: Option<u8>, channels: Option<u8>) -> Vec<u8> {
        let mut pt = b"PT\0\0\x06\x01\x65\xfd\x80\x01\x02\x85\x03".to_vec();
        pt.extend_from_slice(&samples.to_be_bytes()[1..]);
        if let Some(c) = codec {
            pt.extend_from_slice(&[0xA0, 1, c]);
        }
        if let Some(c) = channels {
            pt.extend_from_slice(&[0x82, 1, c]);
        }
        pt.push(0xFF);
        pt.resize(pt.len().next_multiple_of(4), 0);
        [block(b"SCHl", &pt), block(b"SCCl", &1u32.to_le_bytes()), block(b"SCDl", &[0x11; 20]), block(b"SCEl", b"")]
            .concat()
    }

    #[test]
    fn hash_matches_known_names() {
        // node and file hashes taken from the shipped dat.fat
        assert_eq!(fat_hash("AV"), 0x686);
        assert_eq!(fat_hash("AV\\CCsNames"), 0x0B0C_C153);
        assert_eq!(fat_hash("av/ccsnames"), 0x0B0C_C153);
    }

    #[test]
    fn stream_header_and_run() {
        let (s1, s2) = (schl(1000, Some(4), None), schl(500, None, Some(2)));
        let mut data = s1.clone();
        data.resize(0x100, 0);
        data.extend(&s2);
        let (info, end) = walk_stream(&data, 0, None).unwrap();
        assert_eq!((info.samples, info.codec, info.channels, info.rate), (Some(1000), "microtalk", 1, 22050));
        assert_eq!(end, s1.len());
        let run = stream_run(&data, 0, data.len(), 0x100).unwrap();
        assert_eq!(run.iter().map(|s| s.offset).collect::<Vec<_>>(), [0, 0x100]);
        assert_eq!((run[1].codec, run[1].channels), ("ea-xa", 2));
        assert!(walk_stream(&data, 0, Some(s1.len() - 1)).is_err());
    }

    #[test]
    fn stream_run_rejects_junk_between_streams() {
        let mut d = schl(10, None, None);
        let n = d.len();
        d.push(1);
        d.resize(n + 0x201, 0);
        assert!(stream_run(&d, 0, d.len(), 0x100).is_err());
    }

    fn fat_bytes(nodes: &[(u32, u16, u8, u8)], files: &[(u32, u32, u32)]) -> Vec<u8> {
        let mut raw: Vec<u8> =
            [0x18u32, 1, 8, 12, nodes.len() as u32, files.len() as u32].iter().flat_map(|v| v.to_le_bytes()).collect();
        for &(h, first, count, sub) in nodes {
            raw.extend_from_slice(&h.to_le_bytes());
            raw.extend_from_slice(&first.to_le_bytes());
            raw.extend_from_slice(&[count, sub]);
        }
        for &(h, o, s) in files {
            raw.extend([h, o, s].iter().flat_map(|v| v.to_le_bytes()));
        }
        raw
    }

    #[test]
    fn fat_lookup() {
        let nodes = [(0x1234, 0, 1, 1), (fat_hash("Sub"), 1, 2, 0)];
        let files =
            [(fat_hash("root.dat"), 0, 0x100), (fat_hash("a.dat"), 0x100, 0x80), (fat_hash("b.dat"), 0x200, 0x40)];
        let fat = Fat::parse(&fat_bytes(&nodes, &files)).unwrap();
        assert_eq!(fat.lookup("Sub\\b.dat").unwrap().offset, 0x200);
        assert_eq!(fat.lookup("sub/A.DAT").unwrap().size, 0x80);
        assert_eq!(fat.lookup("root.dat").unwrap().offset, 0);
        assert!(fat.lookup("Sub\\root.dat").is_none());
        assert_eq!(fat.directory_of(2).unwrap().hash, fat_hash("Sub"));
        assert!(Fat::parse(&fat_bytes(&[(0, 1, 2, 0)], &files)).is_err());
    }

    #[test]
    fn speech_file_with_trailer() {
        let sentence = b"\x01\x02\x03";
        let mut dat = schl(10, Some(4), None);
        dat.resize(0x100, 0);
        dat.extend(b"HrIn");
        dat.extend(sentence);
        dat.extend(b"HrSz");
        dat.extend(&(sentence.len() as u32).to_be_bytes());
        let size = dat.len() as u32;
        let fat = Fat::parse(&fat_bytes(&[(fat_hash("SF"), 0, 1, 0)], &[(fat_hash("x.dat"), 0, size)])).unwrap();
        let mut hdr = 0x40u32.to_le_bytes().to_vec();
        hdr.extend(b"Audio\\English\\dat\\SF\\x.dat\0");
        hdr.resize(0x40, 0);
        hdr.extend(sentence);
        let hdrs = [("x.hdr".to_string(), hdr.clone())];
        let idx = speech_index(&dat, &fat, &hdrs, "English").unwrap();
        assert_eq!(idx[0].0, "SF\\x.dat");
        assert_eq!(idx[0].1[0].codec, "microtalk");
        hdr[0x40] = 9;
        assert!(speech_index(&dat, &fat, &[("x.hdr".into(), hdr)], "English").is_err());
        assert!(speech_index(&dat, &fat, &hdrs, "French").is_err());
    }

    #[test]
    fn banks() {
        let s = schl(10, None, None);
        let mut data = b"BNKl\x05\x00\x03\x00\x10\x00\x00\x00".to_vec();
        data.resize(0x10, 0);
        let o = data.len() as u32;
        data.extend(1u32.to_le_bytes());
        data.extend(0x80u32.to_le_bytes());
        data.resize(o as usize + 0x80, 0);
        data.extend(&s);
        let fat = Fat::parse(&fat_bytes(&[(0, 0, 2, 0)], &[(7, 0, 0x10), (8, o, data.len() as u32 - o)])).unwrap();
        let b = bank_index(&data, &fat).unwrap();
        assert_eq!(b[0], (7, Bank::Bnkl(3)));
        let Bank::Stream(ss) = &b[1].1 else { panic!() };
        assert_eq!(ss[0].offset, o as usize + 0x80);
        data[o as usize + 0x10] = 1;
        assert!(bank_index(&data, &fat).is_err());
    }
}
