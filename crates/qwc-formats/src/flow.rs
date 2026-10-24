//! The front-end flow (port of `tools/flow_parse.py`): `frontend/ScrnLst/game.flo` (magic "FLOW", loader
//! `FUN_004faaa0`), the screen list `frontend/ScrnLst/Screens.lst` (parser `FUN_004fd990`), and the game's name hash.
//!
//! ```text
//! game.flo, u32 words:
//! 0x00 "FLOW" | version (< 2) | has_names | n_roots | n_groups | n_nodes | n_links
//! 0x1C u32 roots[n_roots]                                   group indices of the top-level groups
//! groups 12 B: hash | entry node | n_children                 children = direct nodes and subgroups
//! nodes  28 B: hash(name) | hash(str1..str3) x3 | id (0) | first_link | n_links
//! links   8 B: target | flags (bit 0: target is a group, entered at its entry node; else a node)
//! names: n_groups x {u32 len, chars}, then n_nodes x 4 x {u32 len, chars}
//! ```
//! Groups are stored in pre-order and nodes in the order of the same depth-first walk, so a group's entry is the
//! first node of its subtree. A node's links are its exits: `General_RequestExit(n)` takes link `n`.
//!
//! `Screens.lst` is `"Name" { LOADSETS { name | @DataStoreKey ... } LAYERS { priority "group" "file.vso"
//! Reload|NoReload ... } } ... EOF`, split on space and `\t..\r` with `"` toggling quoting. A screen belongs to the
//! flow node whose hash is the hash of its name.

use crate::util::{bad, latin1, slice, u32_at, Result};

/// The game's name hash (`FUN_004d2de0`, native `FECK_CalcCRC`): reflected CRC-32 (0xEDB88320) with initial
/// value `seed` (0 everywhere) and no final XOR, over the bytes as written.
pub fn name_hash_seed(name: &[u8], seed: u32) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *e = c;
        }
        t
    });
    name.iter().fold(seed, |c, &b| t[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8))
}

/// [`name_hash_seed`] with seed 0, for a Latin-1 string (one byte per char).
pub fn name_hash(name: &str) -> u32 {
    let bytes: Vec<u8> = name.chars().map(|c| c as u32 as u8).collect();
    name_hash_seed(&bytes, 0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub hash: u32,
    /// Node index the flow enters the group at.
    pub entry: u32,
    /// Direct children (nodes and subgroups).
    pub n_children: u32,
    pub name: Option<String>,
    /// Enclosing group, `None` for a root.
    pub parent: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub hash: u32,
    /// Hashes of the three extra strings (0 when empty, as on the disc).
    pub str_hashes: [u32; 3],
    /// 0 in the file; the loader copies `hash` here.
    pub id: u32,
    pub first_link: u32,
    pub n_links: u32,
    pub name: Option<String>,
    pub strings: Option<[String; 3]>,
    /// Group the node belongs to.
    pub group: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    pub target: u32,
    pub flags: u32,
}

impl Link {
    pub fn to_group(&self) -> bool {
        self.flags & 1 != 0
    }
}

#[derive(Debug, Clone)]
pub struct Flow {
    pub version: u32,
    pub has_names: u32,
    pub roots: Vec<u32>,
    pub groups: Vec<Group>,
    pub nodes: Vec<Node>,
    pub links: Vec<Link>,
}

/// Length-prefixed (u32) Latin-1 string, no terminator.
fn flo_string(d: &[u8], off: &mut usize) -> Result<String> {
    let n = u32_at(d, *off)? as usize;
    let s = latin1(slice(d, *off + 4, n)?);
    *off += 4 + n;
    Ok(s)
}

impl Flow {
    pub fn parse(d: &[u8]) -> Result<Flow> {
        if slice(d, 0, 4)? != b"FLOW" {
            return bad("not a FLOW file");
        }
        let w = |i: usize| u32_at(d, 4 + 4 * i);
        let (version, has_names) = (w(0)?, w(1)?);
        let (n_roots, n_groups, n_nodes, n_links) = (w(2)? as usize, w(3)? as usize, w(4)? as usize, w(5)? as usize);
        if version >= 2 {
            return bad(format!("version {version} (the loader accepts < 2)"));
        }
        let mut off = 0x1C;
        let roots = (0..n_roots).map(|i| u32_at(d, off + 4 * i)).collect::<Result<Vec<_>>>()?;
        off += 4 * n_roots;
        let mut groups = Vec::new();
        for i in 0..n_groups {
            let o = off + 12 * i;
            groups.push(Group {
                hash: u32_at(d, o)?,
                entry: u32_at(d, o + 4)?,
                n_children: u32_at(d, o + 8)?,
                name: None,
                parent: None,
            });
        }
        off += 12 * n_groups;
        let mut nodes = Vec::new();
        for i in 0..n_nodes {
            let o = off + 28 * i;
            let r = |k: usize| u32_at(d, o + 4 * k);
            nodes.push(Node {
                hash: r(0)?,
                str_hashes: [r(1)?, r(2)?, r(3)?],
                id: r(4)?,
                first_link: r(5)?,
                n_links: r(6)?,
                name: None,
                strings: None,
                group: 0,
            });
        }
        off += 28 * n_nodes;
        let mut links = Vec::new();
        for i in 0..n_links {
            links.push(Link { target: u32_at(d, off + 8 * i)?, flags: u32_at(d, off + 8 * i + 4)? });
        }
        off += 8 * n_links;
        if has_names != 0 {
            for g in &mut groups {
                g.name = Some(flo_string(d, &mut off)?);
            }
            for n in &mut nodes {
                n.name = Some(flo_string(d, &mut off)?);
                n.strings = Some([flo_string(d, &mut off)?, flo_string(d, &mut off)?, flo_string(d, &mut off)?]);
            }
        }
        if off != d.len() {
            return bad(format!("data not fully consumed: {off:#x} of {:#x}", d.len()));
        }
        let mut f = Flow { version, has_names, roots, groups, nodes, links };
        f.check()?;
        f.build_tree()?;
        Ok(f)
    }

    fn check(&self) -> Result<()> {
        if let Some(r) = self.roots.iter().find(|&&r| r as usize >= self.groups.len()) {
            return bad(format!("root group {r} out of range"));
        }
        for (i, g) in self.groups.iter().enumerate() {
            if g.entry as usize >= self.nodes.len() {
                return bad(format!("group {i} entry {} out of range", g.entry));
            }
            if let Some(name) = &g.name {
                if name_hash(name) != g.hash {
                    return bad(format!("group {name:?}: hash {:#010x} != crc of its name", g.hash));
                }
            }
        }
        let mut expect = 0u32;
        for (i, n) in self.nodes.iter().enumerate() {
            if n.first_link != expect {
                return bad(format!("node {i}: links start at {}, expected {expect}", n.first_link));
            }
            expect += n.n_links;
            if n.id != 0 {
                return bad(format!("node {i}: id field {:#x} (0 expected in the file)", n.id));
            }
            if let (Some(name), Some(strings)) = (&n.name, &n.strings) {
                if name_hash(name) != n.hash {
                    return bad(format!("node {name:?}: hash {:#010x} != crc of its name", n.hash));
                }
                for (s, &h) in strings.iter().zip(&n.str_hashes) {
                    if (if s.is_empty() { 0 } else { name_hash(s) }) != h {
                        return bad(format!("node {name:?}: string {s:?} does not match hash {h:#010x}"));
                    }
                }
            }
        }
        if expect as usize != self.links.len() {
            return bad(format!("nodes use {expect} links, file has {}", self.links.len()));
        }
        for (i, l) in self.links.iter().enumerate() {
            if l.flags & !1 != 0 {
                return bad(format!("link {i}: unknown flags {:#x}", l.flags));
            }
            let limit = if l.to_group() { self.groups.len() } else { self.nodes.len() };
            if l.target as usize >= limit {
                return bad(format!("link {i}: target {} out of range", l.target));
            }
        }
        Ok(())
    }

    /// Replay the builder's depth-first walk to give every group its parent and every node its group.
    fn build_tree(&mut self) -> Result<()> {
        struct Walk {
            next_group: usize,
            next_node: usize,
        }
        fn walk(f: &mut Flow, w: &mut Walk, g: usize) -> Result<()> {
            for _ in 0..f.groups[g].n_children {
                if w.next_group < f.groups.len() && f.groups[w.next_group].entry as usize == w.next_node {
                    let child = w.next_group;
                    w.next_group += 1;
                    f.groups[child].parent = Some(g);
                    walk(f, w, child)?;
                } else if w.next_node < f.nodes.len() {
                    f.nodes[w.next_node].group = g;
                    w.next_node += 1;
                } else {
                    return bad(format!("group {g}: more children than nodes"));
                }
            }
            Ok(())
        }
        let mut w = Walk { next_group: 0, next_node: 0 };
        for r in self.roots.clone() {
            let r = r as usize;
            if w.next_group != r || self.groups[r].entry as usize != w.next_node {
                return bad(format!("root group {r} is not next in the walk"));
            }
            w.next_group += 1;
            walk(self, &mut w, r)?;
        }
        if w.next_group != self.groups.len() || w.next_node != self.nodes.len() {
            return bad(format!(
                "tree covers {}/{} groups, {}/{} nodes",
                w.next_group,
                self.groups.len(),
                w.next_node,
                self.nodes.len()
            ));
        }
        Ok(())
    }

    /// The exits of node `i`: for each link, the node the flow lands on (a group's entry for group links).
    pub fn exits(&self, i: usize) -> Vec<(Link, usize)> {
        let n = &self.nodes[i];
        self.links[n.first_link as usize..(n.first_link + n.n_links) as usize]
            .iter()
            .map(|l| (*l, if l.to_group() { self.groups[l.target as usize].entry as usize } else { l.target as usize }))
            .collect()
    }

    /// The node a `General_RequestExit(exit)` from node `i` lands on.
    pub fn take_exit(&self, i: usize, exit: usize) -> Option<usize> {
        self.exits(i).get(exit).map(|&(_, lands)| lands)
    }

    /// The exit of node `i` that leads to the node or group with this hash (`FUN_004faa50`, used by
    /// `General_RequestExitToNode`).
    pub fn exit_to(&self, i: usize, hash: u32) -> Option<usize> {
        self.exits(i).iter().position(|(l, _)| {
            if l.to_group() {
                self.groups[l.target as usize].hash == hash
            } else {
                self.nodes[l.target as usize].hash == hash
            }
        })
    }

    /// Node index by hash (`General_RequestJumpToNode` jumps straight to it).
    pub fn node_by_hash(&self, hash: u32) -> Option<usize> {
        self.nodes.iter().position(|n| n.hash == hash)
    }
}

/// One layer of a screen: a `.vso` script from the package registered under `group`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer {
    pub priority: i64,
    pub group: String,
    pub file: String,
    /// 1 for `Reload`, 0 for `NoReload` (or any other word).
    pub reload: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    pub name: String,
    pub hash: u32,
    /// Load-set names (looked up by hash in the exe's table).
    pub loadsets: Vec<(String, u32)>,
    /// `@Key` items: DataStore keys whose value is a load-set slot chosen at run time (key without the `@`, hash).
    pub dynamic_loadsets: Vec<(String, u32)>,
    pub layers: Vec<Layer>,
    /// Words the parser skips (unknown section keywords, layer flags other than Reload/NoReload).
    pub ignored: Vec<String>,
}

/// `FUN_004fd900`: split on space and `\t..\r`; a double quote toggles quoting and is not copied.
pub fn tokens(text: &[u8]) -> Vec<String> {
    let (mut out, mut cur, mut quoted, mut have) = (Vec::new(), Vec::new(), false, false);
    for &c in text {
        if c == b'"' {
            quoted = !quoted;
            have = true;
        } else if (c == b' ' || (b'\t'..=b'\r').contains(&c)) && !quoted {
            if have {
                out.push(latin1(&cur));
                cur.clear();
                have = false;
            }
        } else {
            cur.push(c);
            have = true;
        }
    }
    if have {
        out.push(latin1(&cur));
    }
    out
}

/// C `atol`: leading whitespace, an optional sign, then decimal digits.
pub fn atol(s: &str) -> i64 {
    let s = s.trim_start_matches([' ', '\t', '\n', '\x0b', '\x0c', '\r']);
    let (sign, rest) = match s.as_bytes().first() {
        Some(b'-') => (-1, &s[1..]),
        Some(b'+') => (1, &s[1..]),
        _ => (1, s),
    };
    let digits = rest.bytes().take_while(u8::is_ascii_digit).fold(0i64, |v, b| v * 10 + (b - b'0') as i64);
    sign * digits
}

/// Parse `Screens.lst` as `FUN_004fd990` does.
pub fn parse_screens(text: &[u8]) -> Result<Vec<Screen>> {
    let toks = tokens(text);
    let mut pos = 0;
    let mut next = || -> Result<String> {
        let t = toks.get(pos).cloned().ok_or_else(|| crate::Error::Format("Screens.lst ends before EOF".into()))?;
        pos += 1;
        Ok(t)
    };
    let mut screens: Vec<Screen> = Vec::new();
    let mut tok = next()?;
    while !tok.eq_ignore_ascii_case("EOF") {
        let mut scr = Screen {
            hash: name_hash(&tok),
            name: tok,
            loadsets: Vec::new(),
            dynamic_loadsets: Vec::new(),
            layers: Vec::new(),
            ignored: Vec::new(),
        };
        if next()? != "{" {
            return bad(format!("{}: expected {{", scr.name));
        }
        tok = next()?;
        while tok != "}" {
            let is_sets = tok.eq_ignore_ascii_case("LOADSETS");
            if is_sets || tok.eq_ignore_ascii_case("LAYERS") {
                if next()? != "{" {
                    return bad(format!("{}: expected {{ after {tok}", scr.name));
                }
                tok = next()?;
                while tok != "}" {
                    if is_sets {
                        match tok.strip_prefix('@') {
                            Some(key) => scr.dynamic_loadsets.push((key.to_string(), name_hash(key))),
                            None => scr.loadsets.push((tok.clone(), name_hash(&tok))),
                        }
                    } else {
                        let (group, file, flag) = (next()?, next()?, next()?);
                        let reload = flag.eq_ignore_ascii_case("Reload");
                        if !reload && !flag.eq_ignore_ascii_case("NoReload") {
                            scr.ignored.push(flag);
                        }
                        scr.layers.push(Layer { priority: atol(&tok), group, file, reload: reload as u8 });
                    }
                    tok = next()?;
                }
            } else {
                scr.ignored.push(tok);
            }
            tok = next()?;
        }
        if let Some(other) = screens.iter().find(|s| s.hash == scr.hash) {
            return bad(format!("screens {:?} and {:?} share a hash", other.name, scr.name));
        }
        screens.push(scr);
        tok = next()?;
    }
    if pos != toks.len() {
        return bad(format!("{} tokens after EOF", toks.len() - pos));
    }
    Ok(screens)
}

/// One slot of the exe's load-set table (`FUN_004c1370`, `RE_NOTES.md` "Load sets"): its name and its packages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadSet {
    pub slot: usize,
    pub name: String,
    /// `name_hash(name)`, the word at `0x68d530 + 4 * slot`.
    pub hash: u32,
    /// `(group, package path)`: each package is registered under its group (the path as the exe spells it,
    /// `\` separated, relative to the disc root).
    pub entries: Vec<(String, String)>,
}

/// The function filling the table, its end, the hash table and its slots (as `tools/flow_parse.py`).
const LOADSET_FN: u32 = 0x4c1370;
const LOADSET_END: u32 = 0x4c6346;
const LOADSET_HASHES: u32 = 0x68d530;
const LOADSET_SLOTS: u32 = 0x173;

use crate::exe::{pe_sections, va_offset};

/// Read the load-set table from the code that fills it (`FUN_004c1370`, port of `flow_parse.loadset_table`). Each
/// slot's block holds `mov eax, name` (`B8`), `mov [esi + 4 + 4 * slot], entries` (`C7 46`/`C7 86`) and, last,
/// `mov [0x68d530 + 4 * slot], eax` (`A3`). Fails on an exe without that code (the disc's encrypted one). Slots
/// whose entries lie in zero-initialised memory come back empty.
pub fn loadset_table(exe: &[u8]) -> Result<Vec<LoadSet>> {
    let (base, sections) = pe_sections(exe)?;
    let Some(start) = va_offset(base, &sections, LOADSET_FN) else { return bad("no load-set code") };
    let code = slice(exe, start, (LOADSET_END - LOADSET_FN) as usize)?;
    let cstr = |va: u32| -> Option<String> {
        let o = va_offset(base, &sections, va)?;
        let end = exe[o..].iter().position(|&b| b == 0)?;
        Some(latin1(&exe[o..o + end]))
    };
    let word = |i: usize| u32::from_le_bytes(code[i..i + 4].try_into().unwrap());
    let (mut name_va, mut names, mut defs) =
        (None, std::collections::BTreeMap::new(), std::collections::BTreeMap::new());
    let mut i = 0;
    while i + 5 <= code.len() {
        match code[i] {
            // `push imm32` and `call rel32`: their operands may hold the bytes looked for
            0x68 | 0xe8 => i += 5,
            0xb8 => {
                name_va = Some(word(i + 1));
                i += 5;
            }
            0xc7 if code[i + 1] == 0x46 && i + 7 <= code.len() => {
                let disp = code[i + 2] as u32;
                if disp >= 4 && disp.is_multiple_of(4) {
                    defs.insert((disp - 4) / 4, word(i + 3));
                }
                i += 7;
            }
            0xc7 if code[i + 1] == 0x86 && i + 10 <= code.len() => {
                let disp = word(i + 2);
                if (4..4 + 4 * LOADSET_SLOTS).contains(&disp) && disp.is_multiple_of(4) {
                    defs.insert((disp - 4) / 4, word(i + 6));
                }
                i += 10;
            }
            0xa3 => {
                let at = word(i + 1);
                if (LOADSET_HASHES..LOADSET_HASHES + 4 * LOADSET_SLOTS).contains(&at) && at.is_multiple_of(4) {
                    if let Some(n) = name_va.take() {
                        names.insert((at - LOADSET_HASHES) / 4, n);
                    }
                }
                i += 5;
            }
            _ => i += 1,
        }
    }
    if names.is_empty() || names.keys().ne(defs.keys()) {
        return bad("the load-set code does not read as expected (an encrypted exe?)");
    }
    let mut out = Vec::new();
    for (&slot, &nva) in &names {
        let name = cstr(nva).ok_or_else(|| crate::util::Error::Format(format!("slot {slot}: no name")))?;
        let mut entries = Vec::new();
        let mut va = defs[&slot];
        while let Some(o) = va_offset(base, &sections, va) {
            let (group, path) = (u32_at(exe, o)?, u32_at(exe, o + 4)?);
            if group == 0 {
                break;
            }
            match (cstr(group), cstr(path)) {
                (Some(g), Some(p)) => entries.push((g, p)),
                _ => return bad(format!("slot {slot}: entry strings outside the file")),
            }
            va += 16;
        }
        out.push(LoadSet { slot: slot as usize, hash: name_hash(&name), name, entries });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_str(d: &mut Vec<u8>, s: &str) {
        d.extend_from_slice(&(s.len() as u32).to_le_bytes());
        d.extend_from_slice(s.as_bytes());
    }

    /// Root group "Top" (entry 0) with node "A" and subgroup "Sub" (entry 1) holding node "B".
    /// A exits to B directly and to Sub; B exits back to A.
    fn build() -> Vec<u8> {
        let mut d = b"FLOW".to_vec();
        for v in [1u32, 1, 1, 2, 2, 3, 0] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        for (name, entry, n) in [("Top", 0u32, 2u32), ("Sub", 1, 1)] {
            for v in [name_hash(name), entry, n] {
                d.extend_from_slice(&v.to_le_bytes());
            }
        }
        for (name, first, n) in [("A", 0u32, 2u32), ("B", 2, 1)] {
            for v in [name_hash(name), 0, 0, 0, 0, first, n] {
                d.extend_from_slice(&v.to_le_bytes());
            }
        }
        for (t, f) in [(1u32, 0u32), (1, 1), (0, 0)] {
            d.extend_from_slice(&t.to_le_bytes());
            d.extend_from_slice(&f.to_le_bytes());
        }
        for s in ["Top", "Sub", "A", "", "", "", "B", "", "", ""] {
            push_str(&mut d, s);
        }
        d
    }

    #[test]
    fn hash_matches_the_game() {
        // Reflected CRC-32 with zero initial value and no final XOR; values from game.flo and the native table.
        assert_eq!(name_hash(""), 0);
        assert_eq!(name_hash("MainGroup"), 0x756F_BFB5);
        assert_eq!(name_hash("LayerObj_SetX"), 0xBECB_753E);
        assert_eq!(name_hash_seed(b"123456789", !0), !crate::crc32(b"123456789"));
    }

    #[test]
    fn flow_roundtrip() {
        let f = Flow::parse(&build()).unwrap();
        assert_eq!(f.groups[1].parent, Some(0));
        assert_eq!((f.nodes[0].group, f.nodes[1].group), (0, 1));
        assert_eq!(f.take_exit(0, 0), Some(1));
        assert_eq!(f.take_exit(0, 1), Some(1)); // via the group's entry
        assert_eq!(f.exit_to(0, name_hash("Sub")), Some(1));
        assert_eq!(f.exit_to(1, name_hash("A")), Some(0));
        assert_eq!(f.node_by_hash(name_hash("B")), Some(1));
    }

    #[test]
    fn flow_rejects_bad_files() {
        let good = build();
        let mut d = good.clone();
        d.push(0);
        assert!(Flow::parse(&d).is_err()); // trailing byte
        let mut d = good.clone();
        d[0x28] = 3; // the root group's child count
        assert!(Flow::parse(&d).is_err());
        let mut d = good.clone();
        d[0x38 + 28 + 20] = 9; // node B's first link
        assert!(Flow::parse(&d).is_err());
        let mut d = good;
        let b = d.len() - 13; // "B", followed by three empty strings
        d[b] = b'C';
        assert!(Flow::parse(&d).is_err()); // the node name no longer matches its hash
    }

    #[test]
    fn screens_parse_like_the_game() {
        let text =
            b"\"One\"\r\n{\r\n\tLOADSETS\r\n\t{\r\n\t\tLOADSET_HUD\r\n\t\t@dFontLoadset\r\n\t}\r\n\tLAYERS\r\n\t{\r\n\
            \t\t32768\t\"FE Group\"\t\"a.vso\"\tNoreload\r\n\t\t+10x\t\"G\"\t\"b.vso\"\tReload\r\n\t}\r\n}\r\neof\r\n";
        let s = parse_screens(text).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].hash, name_hash("One"));
        assert_eq!(s[0].loadsets, vec![("LOADSET_HUD".to_string(), name_hash("LOADSET_HUD"))]);
        assert_eq!(s[0].dynamic_loadsets[0].0, "dFontLoadset");
        assert_eq!(
            s[0].layers[0],
            Layer { priority: 32768, group: "FE Group".into(), file: "a.vso".into(), reload: 0 }
        );
        assert_eq!((s[0].layers[1].priority, s[0].layers[1].reload), (10, 1));
        assert!(parse_screens(b"\"One\" { }").is_err()); // no EOF
        assert!(parse_screens(b"A { } A { } EOF").is_err()); // duplicate hash
    }

    #[test]
    fn atol_like_c() {
        assert_eq!(atol("  -12ab"), -12);
        assert_eq!(atol("x1"), 0);
        assert_eq!(atol("+7"), 7);
    }
}
