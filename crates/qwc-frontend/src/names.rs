//! Names for the screens' objects, for people (the HUD editor's tree). An object's id is its name's hash
//! (`qwc_formats::flow::name_hash`, a CRC-32 with no final XOR: the title's logo is `HPLogo`), and the names
//! themselves are not on the disc, so they are found by trying words against the ids:
//! - the words the scripts hold: their symbols, their resources' names and groups, and the strings in their data;
//!   the exe's strings when one is given; each with its `_` parts and the runs of them;
//! - those words with a number after them (`Entry0`-`Entry19`, `Star_2`);
//! - two words of the scripts run together, or joined by `_` (`ActionButton`, `Team1_Score`). The CRC is linear, so
//!   the first word a name would need is worked out from each id and the second word, and looked up.
//!
//! **[I]** A two-word name is a guess that hashes right, not a name read from the data: a name is only taken when
//! each of its words looks like one (`Team`, `team`, `GBA`, `12`), which leaves out most of the chance matches.

use std::collections::{HashMap, HashSet};

use qwc_formats::flow::{name_hash, name_hash_seed};
use qwc_formats::vso::{Layer, Vso};

/// The ids of every object in `scripts`' layers, with names for those a word gives.
pub fn object_names(scripts: &[Vec<u8>], exe: Option<&[u8]>) -> HashMap<u32, String> {
    let parsed: Vec<Vso> = scripts.iter().filter_map(|b| Vso::parse(b).ok()).collect();
    let mut targets = HashSet::new();
    for v in &parsed {
        if let Ok((layers, _)) = v.layers() {
            ids(&layers, &mut targets);
        }
    }
    let mut words: Vec<String> = Vec::new();
    for v in &parsed {
        words.extend(v.symbols.iter().map(|s| s.name.clone()));
        for r in v.resources().unwrap_or_default() {
            words.push(r.name);
            words.push(r.group);
        }
        strings(v.lit, 2, &mut words);
        strings(v.data, 2, &mut words);
    }
    let mut names = HashMap::new();
    let take = |w: &str, names: &mut HashMap<u32, String>| {
        let h = name_hash(w);
        if targets.contains(&h) && !names.contains_key(&h) && wordy(w) {
            names.insert(h, w.to_string());
        }
    };
    let mut parts: HashSet<String> = HashSet::new();
    let exe_words = exe.map(|b| {
        let mut w = Vec::new();
        strings(b, 3, &mut w);
        w
    });
    for (w, own) in words.iter().map(|w| (w, true)).chain(exe_words.iter().flatten().map(|w| (w, false))) {
        for token in w.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).filter(|t| !t.is_empty()) {
            let ps: Vec<&str> = token.split('_').collect();
            for i in 0..ps.len() {
                for j in i + 1..=ps.len() {
                    let run = ps[i..j].join("_");
                    take(&run, &mut names);
                    if own && !run.is_empty() && run.len() <= 40 {
                        parts.insert(run);
                    }
                }
            }
        }
    }
    // numbered
    for p in &parts {
        for n in 0..40 {
            for w in [format!("{p}{n}"), format!("{p}{n:02}"), format!("{p}_{n}"), format!("{p}_{n:02}")] {
                take(&w, &mut names);
            }
        }
    }
    // two words: the scripts' words and their CamelCase pieces, the second a number too
    let mut vocab: HashSet<String> = HashSet::new();
    for w in &words {
        for t in w.split(|c: char| !c.is_ascii_alphanumeric()).filter(|t| (2..=24).contains(&t.len())) {
            vocab.insert(t.to_string());
            vocab.extend(pieces(t).into_iter().filter(|p| p.len() >= 2).map(str::to_string));
        }
    }
    let firsts: HashMap<u32, &String> = vocab.iter().map(|w| (name_hash(w), w)).collect();
    let mut seconds: Vec<String> = vocab.iter().cloned().collect();
    seconds.extend((0..20).flat_map(|n| [format!("{n}"), format!("{n:02}")]));
    seconds.sort();
    let mut open: Vec<u32> = targets.iter().copied().filter(|t| !names.contains_key(t)).collect();
    open.sort();
    for joiner in ["", "_"] {
        for s in &seconds {
            let tail = format!("{joiner}{s}");
            let crc = name_hash(&tail);
            for &t in &open {
                let Some(first) = firsts.get(&unshift(t ^ crc, tail.len())) else { continue };
                let name = format!("{first}{tail}");
                if wordy(&name) {
                    names.entry(t).or_insert(name);
                }
            }
        }
    }
    names
}

fn ids(layers: &[Layer], out: &mut HashSet<u32>) {
    for l in layers {
        if let Layer::Object { id, children, .. } = l {
            out.insert(*id);
            ids(children, out);
        }
    }
}

/// The C strings in `bytes`: printable runs of at least `min` with a NUL (or an end) on both sides.
fn strings(bytes: &[u8], min: usize, out: &mut Vec<String>) {
    let mut start = Some(0);
    for (i, &c) in bytes.iter().enumerate() {
        if c == 0 {
            if let Some(s) = start.filter(|&s| i - s >= min) {
                out.push(String::from_utf8_lossy(&bytes[s..i]).into_owned());
            }
            start = Some(i + 1);
        } else if !(c.is_ascii_graphic() || c == b' ') {
            start = None;
        }
    }
    if let Some(s) = start.filter(|&s| bytes.len() - s >= min) {
        out.push(String::from_utf8_lossy(&bytes[s..]).into_owned());
    }
}

/// A word's CamelCase pieces: a capital starts one (a run of capitals is one, its last starting the next when a
/// small letter follows, `GBALink`), and so does a run of digits.
fn pieces(w: &str) -> Vec<&str> {
    let b = w.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    for i in 1..b.len() {
        let (p, c) = (b[i - 1], b[i]);
        let next_small = b.get(i + 1).is_some_and(|n| n.is_ascii_lowercase());
        let cut = (c.is_ascii_uppercase() && (p.is_ascii_lowercase() || p.is_ascii_digit()))
            || (c.is_ascii_uppercase() && p.is_ascii_uppercase() && next_small)
            || (c.is_ascii_digit() != p.is_ascii_digit());
        if cut {
            out.push(&w[start..i]);
            start = i;
        }
    }
    out.push(&w[start..]);
    out
}

/// Whether every piece of a name (between its `_`s, [`pieces`]) looks like a word: `Team`, `team`, `GBA`, `12`,
/// with a vowel in a small-lettered piece of two or more, and no lone small letter beside other pieces (`aW`).
fn wordy(name: &str) -> bool {
    !name.is_empty()
        && name.split('_').all(|part| {
            let ps = pieces(part);
            !part.is_empty()
                && ps.iter().all(|p| {
                    let b = p.as_bytes();
                    let digits = b.iter().all(u8::is_ascii_digit);
                    let caps = b.iter().all(u8::is_ascii_uppercase);
                    let small = b[1..].iter().all(u8::is_ascii_lowercase) && b[0].is_ascii_alphabetic();
                    let vowel = p.chars().any(|c| "aeiouyAEIOUY".contains(c));
                    digits || caps || (small && if b.len() < 2 { ps.len() == 1 } else { vowel })
                })
        })
}

/// The CRC before `n` bytes of zeros made it `c` ([`name_hash_seed`] run backwards: each step's table entry is the
/// one whose top byte the result has).
fn unshift(mut c: u32, n: usize) -> u32 {
    static TOP: std::sync::OnceLock<[(u32, u8); 256]> = std::sync::OnceLock::new();
    let top = TOP.get_or_init(|| {
        let mut top = [(0, 0); 256];
        for i in 0..=255u8 {
            // the table entry for `i`: the CRC of the byte `i` from 0
            let t = name_hash_seed(&[i], 0);
            top[(t >> 24) as usize] = (t, i);
        }
        top
    });
    for _ in 0..n {
        let (t, i) = top[(c >> 24) as usize];
        c = ((c ^ t) << 8) | i as u32;
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crc_runs_backwards_over_its_last_bytes() {
        let (a, b) = ("Action", "Button");
        // the CRC of `ab` is the CRC of `b` from `a`'s, which is `b`'s from 0 plus `a`'s through its zeros
        let ab = name_hash(&format!("{a}{b}"));
        assert_eq!(unshift(ab ^ name_hash(b), b.len()), name_hash(a));
    }

    #[test]
    fn names_are_taken_when_their_words_look_like_words() {
        assert_eq!(pieces("GBALinkError2"), ["GBA", "Link", "Error", "2"]);
        for w in ["ActionButton", "Team1_Score", "SELGRYCAM", "player1_arrows", "A"] {
            assert!(wordy(w), "{w}");
        }
        for w in ["AGjA", "JESB_aW", "Djj", "OIgry_Fx"] {
            assert!(!wordy(w), "{w}");
        }
    }

    #[test]
    fn c_strings_need_their_nuls() {
        let mut out = Vec::new();
        strings(b"\x01AGj\x05\0HPLogo\0x\0Title", 2, &mut out);
        assert_eq!(out, ["HPLogo", "Title"]);
    }
}
