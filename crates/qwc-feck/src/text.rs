//! The string manager (`CFECK_StringManager`, `docs/FECK_VM.md` "Text"): string ids to text through
//! `language/INDEX.SDX` and the language's `.STS` table, string handles with their cached formatted text, runtime
//! strings, and the insert formatters that splice DataStore values into a string.
//!
//! Traced from `GetStringHandle` `0x4e08d0`, the acquire `0x4e0a20` / `0x4e0960`, the release `0x4e0b20`, the
//! formatter `0x4e11a0`, the insert dispatch `0x4e1370`, the type formatters `0x5ef884` (`0x4e1610`-`0x4e1c30`), the
//! registered formatters (`0x4e15b0`) and the runtime strings `0x4e1480`. **[T]** unless marked **[I]**.

use qwc_formats::sts::{Sdx, Sts};

/// A string handle's flags (`handle +0x0a`, a copy of the SDX entry's third word).
pub mod flag {
    /// The string has inserts and is formatted when acquired.
    pub const INSERTS: u16 = 2;
    /// Formatted again on every acquire, even while held (never set in the retail data).
    pub const REFORMAT: u16 = 4;
    /// A runtime string (`0x4e1480`); its text is used as is.
    pub const RUNTIME: u16 = 8;
}

/// The id of the string "vs" that the match-line formatters put between two teams.
pub const VS: u32 = 0x1e83_fa9c;

/// The registered insert formatters (`0x4e15b0`), by the insert's kind.
pub mod kind {
    /// `0x4e1de0`: the entry's first word is a string id; its text.
    pub const STRING: u32 = 0xa9cd_9885;
    /// `0x4e1f10`: a result line `team0 \t score0 \t "vs" \t team1 \t score1` from words 0, 1, 2, 3.
    pub const RESULT: u32 = 0x2969_1e0a;
    /// `0x4e1fe0`: a table row `team \t n1 \t n2 \t n3 \t n4 \t n5` from words 0-5.
    pub const TABLE_ROW: u32 = 0x8d6f_92d2;
    /// `0x4e20c0`: a result line from words 0, 2, 1, 3 (team0, score0, team1, score1 stored in another order).
    pub const FIXTURE: u32 = 0xe0b0_bfc2;
}

/// The DataStore key of the current language (`crcCurrentLanguage`) and the language hashes that pick the time
/// format (`0x4e1400`).
pub const CURRENT_LANGUAGE: u32 = 0x01b2_3891;
const FRENCH: u32 = 0x191b_6acb;
const SWEDISH: u32 = 0xe1f1_65ca;

/// One DataStore entry's value as the formatters see it (`entry +0x08` type, `+0x0c` count, `+0x0e` stride,
/// `+0x10` the data).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Value {
    pub kind: u32,
    pub count: u16,
    pub stride: u16,
    pub data: Vec<u8>,
}

impl Value {
    fn word(&self, i: usize) -> u32 {
        (0..4).fold(0, |w, k| w | (self.data.get(4 * i + k).copied().unwrap_or(0) as u32) << (8 * k))
    }
}

/// Where the formatters read DataStore entries by key.
pub trait Values {
    fn value(&self, key: u32) -> Option<Value>;
}

impl Values for () {
    fn value(&self, _: u32) -> Option<Value> {
        None
    }
}

/// Why a string could not be acquired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// No SDX entry or runtime string has this id (`0x80020006`).
    NotFound(u32),
    /// The id plus the index runs past the table (the original reads past it; **[I]** the port refuses).
    BadIndex(u32, u32),
    /// A type insert names a DataStore type above 9 (`0x80020018`).
    BadType(u32),
    /// A runtime string with this id already exists (`0x8002001b`).
    Exists(u32),
    /// Inserts nested too deep (**[I]** a guard of the port; the original would recurse until the stack ran out).
    TooDeep,
}

/// A string handle: an index into the `.STS` entries, then the runtime strings.
pub type Handle = usize;

#[derive(Debug, Clone, Default)]
struct Slot {
    /// `handle +0x04`: the text to show (the raw text, or the formatted copy); `None` when not acquired.
    text: Option<Vec<u16>>,
    /// `handle +0x08`
    refs: u16,
}

/// The string manager for one language.
#[derive(Debug, Clone)]
pub struct Strings {
    sts: Sts,
    /// The SDX entries: (id, flags, handle).
    index: Vec<(u32, u16, Handle)>,
    /// Runtime strings (`+0x98` count, `+0x9c` entries): (id, text).
    runtime: Vec<(u32, Vec<u16>)>,
    slots: Vec<Slot>,
}

/// Each formatter writes into the output; an `Err` stops the whole string, `Ok(false)` stops only this insert
/// (the original's positive return codes).
type Step = std::result::Result<bool, Error>;

const MAX_DEPTH: u32 = 8;

impl Strings {
    /// The manager over the SDX index and the language's table (`FUN_004e0c60` loads `"<file>.STS"` for the file
    /// hash the SDX names; the retail data has one file, `COMMON`).
    pub fn new(sdx: &Sdx, sts: Sts) -> Strings {
        let index = sdx
            .strings
            .iter()
            .filter(|s| s.file_hash == sts.file_hash)
            .map(|s| (s.id, s.unk as u16, s.index as usize))
            .collect();
        let slots = vec![Slot::default(); sts.entries.len()];
        Strings { sts, index, runtime: Vec::new(), slots }
    }

    /// The table's language (its header's hash: the language's name hashed, `French`, `US`, `SimChina`).
    pub fn language(&self) -> u32 {
        self.sts.language
    }

    /// Switch to another language's table (`General_ChangeLanguage`, `0x4d3b30`): every held string is acquired
    /// again from it, keeping its holders, so the texts on show change language. The languages' tables share the
    /// SDX index (each has the same entries in the same order); runtime strings stay as they are.
    pub fn switch(&mut self, sts: Sts, values: &dyn Values) {
        self.sts = sts;
        // every held text dropped first, so a string inserted into another is made from the new table too
        let held: Vec<(Handle, u16)> = (0..self.slots.len().min(self.sts.entries.len()))
            .filter(|&h| self.slots[h].text.is_some())
            .map(|h| (h, self.slots[h].refs))
            .collect();
        for &(h, _) in &held {
            self.slots[h] = Slot::default();
        }
        for (h, refs) in held {
            let flags = self.index.iter().find(|e| e.2 == h).map_or(0, |e| e.1);
            // a string that no longer formats keeps its holders with no text, as a failed acquire does
            if self.slots[h].text.is_none() {
                let _ = self.hold(h, flags, values, 0);
            }
            self.slots[h].refs = refs;
        }
    }

    /// Add a runtime string (`0x4e1480`, used for the PC key names `"Localised PC Key N"`): refused when the id is
    /// already a table or runtime string.
    pub fn add(&mut self, id: u32, text: &[u16]) -> Result<(), Error> {
        if self.index.iter().any(|e| e.0 == id) || self.runtime.iter().any(|e| e.0 == id) {
            return Err(Error::Exists(id));
        }
        let text: Vec<u16> = text.iter().copied().take_while(|&u| u != 0).collect();
        self.runtime.push((id, text));
        self.slots.push(Slot::default());
        Ok(())
    }

    /// `GetStringHandle` plus the index (`0x4e08d0`, `0x4e0a20`): the table is searched before the runtime strings;
    /// `index` picks the string that many entries after the id's in the table. The handle's flags come from the
    /// SDX entry that points at it.
    fn locate(&self, id: u32, index: u32) -> Result<(Handle, u16), Error> {
        let n = self.sts.entries.len();
        if let Some(&(_, _, h)) = self.index.iter().find(|e| e.0 == id) {
            let h = h + index as usize;
            return match self.index.iter().find(|e| e.2 == h) {
                Some(&(_, f, _)) if h < n => Ok((h, f)),
                _ => Err(Error::BadIndex(id, index)),
            };
        }
        match self.runtime.iter().position(|e| e.0 == id) {
            // **[I]** an index on a runtime string reads the next allocation in the original
            Some(i) if index == 0 => Ok((n + i, flag::RUNTIME)),
            Some(_) => Err(Error::BadIndex(id, index)),
            None => Err(Error::NotFound(id)),
        }
    }

    fn raw(&self, h: Handle) -> &[u16] {
        match self.sts.entries.get(h) {
            Some(e) => &e.units,
            None => &self.runtime[h - self.sts.entries.len()].1,
        }
    }

    /// Acquire string `id` (`+ index`) for showing (`0x4e0a20`): a held string's text is shared, otherwise the raw
    /// text is used (no inserts, or a runtime string) or the string is formatted from the DataStore. `Ok(None)`
    /// means the handle was found but formatting stopped (a missing DataStore entry): the original keeps the handle
    /// with no text, so nothing is drawn.
    pub fn acquire(&mut self, id: u32, index: u32, values: &dyn Values) -> Result<Option<Handle>, Error> {
        let (h, flags) = self.locate(id, index)?;
        self.hold(h, flags, values, 0)
    }

    fn hold(&mut self, h: Handle, flags: u16, values: &dyn Values, depth: u32) -> Result<Option<Handle>, Error> {
        if self.slots[h].text.is_some() {
            if flags & flag::REFORMAT == 0 {
                self.slots[h].refs += 1;
                return Ok(Some(h));
            }
            self.slots[h].text = None;
        }
        let has_inserts = self.sts.entries.get(h).is_some_and(|e| !e.inserts.is_empty());
        if flags & flag::INSERTS == 0 || flags & flag::RUNTIME != 0 || !has_inserts {
            self.slots[h].text = Some(self.raw(h).to_vec());
        } else {
            match self.format(h, values, depth)? {
                Some(t) => self.slots[h].text = Some(t),
                None => return Ok(None),
            }
        }
        self.slots[h].refs += 1;
        Ok(Some(h))
    }

    /// Release a handle (`0x4e0b20`): when the last holder lets go, the formatted text is dropped.
    pub fn release(&mut self, h: Handle) {
        let s = &mut self.slots[h];
        if s.text.is_some() {
            s.refs = s.refs.saturating_sub(1);
            if s.refs == 0 {
                s.text = None;
            }
        }
    }

    /// The text of an acquired handle (UTF-16 with the layout's control codes still in it).
    pub fn text(&self, h: Handle) -> Option<&[u16]> {
        self.slots.get(h)?.text.as_deref()
    }

    /// How many holders a handle has.
    pub fn refs(&self, h: Handle) -> u16 {
        self.slots.get(h).map_or(0, |s| s.refs)
    }

    /// `0x4e11a0`: the text between inserts is copied, each insert expanded at its position (positions count the
    /// source text); the rest is copied up to the terminator.
    fn format(&mut self, h: Handle, values: &dyn Values, depth: u32) -> Result<Option<Vec<u16>>, Error> {
        if depth > MAX_DEPTH {
            return Err(Error::TooDeep);
        }
        let e = self.sts.entries[h].clone();
        let mut out = Vec::new();
        let mut used = 0usize;
        for ins in &e.inserts {
            let pos = (ins.pos as usize).clamp(used, e.units.len());
            out.extend_from_slice(&e.units[used..pos]);
            used = pos;
            // `0x4e1370`: the insert's id is a DataStore key; a missing entry stops the string (`0x20017`)
            let Some(v) = values.value(ins.id) else {
                return Ok(None);
            };
            if ins.ty & 1 == 0 {
                type_insert(&v, values, &mut out)?;
            } else {
                // a kind nobody registered inserts nothing
                self.kind_insert(ins.kind, &v, values, &mut out, depth)?;
            }
        }
        out.extend(e.units[used..].iter().copied().take_while(|&u| u != 0));
        Ok(Some(out))
    }

    /// `0x4e1d60`: the text of string `id`, acquired and released around the copy; ids 0 and -1 stop the insert.
    fn string_insert(&mut self, id: u32, values: &dyn Values, out: &mut Vec<u16>, depth: u32) -> Step {
        if id == 0 || id == u32::MAX {
            return Ok(false);
        }
        let (h, flags) = self.locate(id, 0)?;
        match self.hold(h, flags, values, depth + 1)? {
            Some(h) => {
                out.extend_from_slice(self.slots[h].text.as_deref().unwrap_or(&[]));
                self.release(h);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// The registered formatters (`0x4e15b0`); a tab separates the columns.
    fn kind_insert(&mut self, k: u32, v: &Value, values: &dyn Values, out: &mut Vec<u16>, depth: u32) -> Step {
        let tab = |out: &mut Vec<u16>| out.push(9);
        let num = |out: &mut Vec<u16>, n: u32| ascii(out, &format!("{}", n as i32));
        match k {
            kind::STRING => self.string_insert(v.word(0), values, out, depth),
            kind::TABLE_ROW => {
                if !self.string_insert(v.word(0), values, out, depth)? {
                    return Ok(false);
                }
                for i in 1..6 {
                    tab(out);
                    num(out, v.word(i));
                }
                Ok(true)
            }
            kind::RESULT | kind::FIXTURE => {
                // (team0, score0, team1, score1) word indices
                let w = if k == kind::RESULT { [0, 1, 2, 3] } else { [0, 2, 1, 3] };
                if !self.string_insert(v.word(w[0]), values, out, depth)? {
                    return Ok(false);
                }
                tab(out);
                if v.word(w[1]) != u32::MAX {
                    num(out, v.word(w[1]));
                }
                tab(out);
                if !self.string_insert(VS, values, out, depth)? {
                    return Ok(false);
                }
                tab(out);
                if !self.string_insert(v.word(w[2]), values, out, depth)? {
                    return Ok(false);
                }
                tab(out);
                if v.word(w[3]) != u32::MAX {
                    num(out, v.word(w[3]));
                }
                Ok(true)
            }
            _ => Ok(true),
        }
    }
}

fn ascii(out: &mut Vec<u16>, s: &str) {
    out.extend(s.bytes().map(u16::from));
}

/// The type formatters (`0x5ef884`, by DataStore type): the first element as `%u` (U8, U16, U32, and the low word
/// of U64), `%d` (S8, S16, S32, the low word of S64), a time for FP32, and the bytes of a user type.
fn type_insert(v: &Value, values: &dyn Values, out: &mut Vec<u16>) -> Step {
    let b = |i: usize| v.data.get(i).copied().unwrap_or(0);
    let w = v.word(0);
    let s = match v.kind {
        0 => format!("{}", b(0)),
        1 => format!("{}", w as u16),
        2 | 3 => format!("{w}"),
        4 => format!("{}", b(0) as i8),
        5 => format!("{}", w as u16 as i16),
        6 | 7 => format!("{}", w as i32),
        8 => time(f32::from_bits(w), values),
        9 => user_bytes(v),
        k => return Err(Error::BadType(k)),
    };
    ascii(out, &s);
    Ok(true)
}

/// FP32 values are times in seconds (`0x4e1b20`): minutes, seconds and tenths, each floored, in the current
/// language's style (`0x4e1400`): `M:SS.t`, French `M'SS"t`, Swedish `M:SS,t`.
pub fn time(t: f32, values: &dyn Values) -> String {
    let m = (t * (1.0f32 / 60.0)).floor();
    let r = t - m * 60.0;
    let s = r.floor();
    let tenths = ((r - s) * 10.0).floor();
    let (m, s, tenths) = (m as i32, s as i32, tenths as i32);
    match values.value(CURRENT_LANGUAGE).map(|v| v.word(0)) {
        Some(FRENCH) => format!("{m}'{s:02}\"{tenths:01}"),
        Some(SWEDISH) => format!("{m}:{s:02},{tenths:01}"),
        _ => format!("{m}:{s:02}.{tenths:01}"),
    }
}

/// A user-type value (`0x4e1c30`): `(` then `count * stride` bytes as `%02x` joined by `,`, then `)`, in a buffer of
/// 0x7c characters; when the budget runs out the list stops without the `)`.
fn user_bytes(v: &Value) -> String {
    let n = (v.count as u32 * v.stride as u32) as u16 as usize;
    let mut s = String::from("(");
    let mut budget = 0x7ci32;
    let mut i = 0;
    loop {
        if i == n {
            if budget > 0 {
                s.push(')');
            }
            break;
        }
        let x = format!("{:02x}", v.data.get(i).copied().unwrap_or(0));
        budget -= x.len() as i32;
        s += &x;
        i += 1;
        if i != n {
            s.push(',');
            budget -= 1;
        }
        if budget <= 0 {
            break;
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use qwc_formats::sts::{Entry, Insert, SdxString};
    use std::collections::HashMap;

    fn units(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn text(s: &Strings, h: Handle) -> String {
        String::from_utf16_lossy(s.text(h).unwrap())
    }

    struct Store(HashMap<u32, Value>);

    impl Values for Store {
        fn value(&self, key: u32) -> Option<Value> {
            self.0.get(&key).cloned()
        }
    }

    fn words(kind: u32, w: &[u32]) -> Value {
        Value { kind, count: w.len() as u16, stride: 4, data: w.iter().flat_map(|x| x.to_le_bytes()).collect() }
    }

    fn ins(pos: u16, ty: u16, id: u32, kind: u32) -> Insert {
        Insert { pos, ty, id, kind }
    }

    /// A table: 0x10 "Team %s!" (inserting the string named by key 0x77), 0x11 "Gryffindor", 0x12 "Slytherin",
    /// `VS` "vs", 0x13 "Score: " + a type insert of key 0x78, 0x14 a result line from key 0x79.
    fn strings() -> Strings {
        let e = |id, s: &str, inserts| Entry { id, units: units(s), inserts };
        let entries = vec![
            e(0x10, "Team !", vec![ins(5, 1, 0x77, kind::STRING)]),
            e(0x11, "Gryffindor", vec![]),
            e(0x12, "Slytherin", vec![]),
            e(VS, "vs", vec![]),
            e(0x13, "Score: ", vec![ins(7, 0, 0x78, 0)]),
            e(0x14, "", vec![ins(0, 1, 0x79, kind::RESULT)]),
        ];
        let sdx = Sdx {
            strings: entries
                .iter()
                .enumerate()
                .map(|(i, e)| SdxString {
                    id: e.id,
                    file_hash: 0xF0,
                    unk: if e.inserts.is_empty() { 0 } else { 2 },
                    index: i as u32,
                })
                .collect(),
            files: vec![(0xF0, "COMMON".into())],
        };
        let sts = Sts { language: 1, file_hash: 0xF0, unk_14: 0, entries };
        Strings::new(&sdx, sts)
    }

    #[test]
    fn switching_language_changes_the_held_strings() {
        let mut s = strings();
        let store = Store(HashMap::from([(0x77, words(kind::STRING, &[0x11]))]));
        let team = s.acquire(0x10, 0, &store).unwrap().unwrap();
        let gryff = s.acquire(0x11, 0, &store).unwrap().unwrap();
        s.acquire(0x11, 0, &store).unwrap();
        assert_eq!(text(&s, team), "Team Gryffindor!");
        // the same entries in another language
        let mut other = s.sts.clone();
        other.language = 2;
        for (e, t) in other.entries.iter_mut().zip(["Equipe !", "Gryffondor", "Serpentard", "contre", "Score : ", ""]) {
            e.units = units(t);
        }
        other.entries[0].inserts[0].pos = 7;
        s.switch(other, &store);
        assert_eq!(s.language(), 2);
        assert_eq!(text(&s, team), "Equipe Gryffondor!");
        assert_eq!((text(&s, gryff), s.refs(gryff)), ("Gryffondor".to_string(), 2), "the holders kept");
        // a string not held is not made, and is the new language's when it is
        assert!(s.text(s.locate(0x12, 0).unwrap().0).is_none());
        let slyth = s.acquire(0x12, 0, &store).unwrap().unwrap();
        assert_eq!(text(&s, slyth), "Serpentard");
    }

    #[test]
    fn plain_and_indexed() {
        let mut s = strings();
        let h = s.acquire(0x11, 0, &()).unwrap().unwrap();
        assert_eq!(text(&s, h), "Gryffindor");
        // the index picks the next entries in the table
        let h = s.acquire(0x11, 1, &()).unwrap().unwrap();
        assert_eq!(text(&s, h), "Slytherin");
        assert_eq!(s.acquire(0x99, 0, &()), Err(Error::NotFound(0x99)));
        assert_eq!(s.acquire(0x14, 1, &()), Err(Error::BadIndex(0x14, 1)));
    }

    #[test]
    fn string_and_type_inserts() {
        let mut s = strings();
        let store = Store(HashMap::from([(0x77, words(2, &[0x12])), (0x78, words(6, &[(-42i32) as u32]))]));
        let h = s.acquire(0x10, 0, &store).unwrap().unwrap();
        assert_eq!(text(&s, h), "Team Slytherin!");
        let h = s.acquire(0x13, 0, &store).unwrap().unwrap();
        assert_eq!(text(&s, h), "Score: -42");
        // a missing DataStore entry leaves the handle without text
        assert_eq!(s.acquire(0x10, 0, &()), Ok(Some(0)), "held strings share their text");
        let mut fresh = strings();
        assert_eq!(fresh.acquire(0x10, 0, &()), Ok(None));
    }

    #[test]
    fn held_text_is_shared_until_released() {
        let mut s = strings();
        let mut store = Store(HashMap::from([(0x77, words(2, &[0x11]))]));
        let a = s.acquire(0x10, 0, &store).unwrap().unwrap();
        store.0.insert(0x77, words(2, &[0x12]));
        let b = s.acquire(0x10, 0, &store).unwrap().unwrap();
        assert_eq!((a, text(&s, b), s.refs(a)), (b, "Team Gryffindor!".to_string(), 2));
        s.release(a);
        s.release(b);
        assert_eq!(s.text(a), None);
        let c = s.acquire(0x10, 0, &store).unwrap().unwrap();
        assert_eq!(text(&s, c), "Team Slytherin!");
    }

    #[test]
    fn result_line_and_null_team() {
        let mut s = strings();
        let store = Store(HashMap::from([(0x79, words(9, &[0x11, 30, 0x12, u32::MAX]))]));
        let h = s.acquire(0x14, 0, &store).unwrap().unwrap();
        assert_eq!(text(&s, h), "Gryffindor\t30\tvs\tSlytherin\t");
        s.release(h);
        // a null first team stops the insert and nothing else is written
        let store = Store(HashMap::from([(0x79, words(9, &[0, 30, 0x12, 10]))]));
        let h = s.acquire(0x14, 0, &store).unwrap().unwrap();
        assert_eq!(text(&s, h), "");
    }

    #[test]
    fn type_formats() {
        let v = |kind, data: &[u8]| Value { kind, count: 1, stride: data.len() as u16, data: data.to_vec() };
        let f = |v: Value| {
            let mut o = Vec::new();
            type_insert(&v, &(), &mut o).unwrap();
            String::from_utf16(&o).unwrap()
        };
        assert_eq!(f(v(0, &[200])), "200");
        assert_eq!(f(v(4, &[200])), "-56");
        assert_eq!(f(v(1, &[0xff, 0xff])), "65535");
        assert_eq!(f(v(5, &[0xff, 0xff])), "-1");
        assert_eq!(f(v(3, &[1, 0, 0, 0, 9, 0, 0, 0])), "1");
        assert_eq!(f(v(8, &75.35f32.to_le_bytes())), "1:15.3");
        assert_eq!(f(Value { kind: 9, count: 2, stride: 2, data: vec![1, 0xab, 0, 0x10] }), "(01,ab,00,10)");
        assert_eq!(f(Value { kind: 9, count: 0, stride: 4, data: vec![] }), "()");
        assert_eq!(type_insert(&v(10, &[0]), &(), &mut Vec::new()), Err(Error::BadType(10)));
        // a long user value runs out of budget before the `)`
        let long = f(Value { kind: 9, count: 60, stride: 1, data: vec![7; 60] });
        assert!(long.starts_with("(07,07") && !long.ends_with(')'));
    }

    #[test]
    fn time_styles() {
        let lang = |h| Store(HashMap::from([(CURRENT_LANGUAGE, words(2, &[h]))]));
        assert_eq!(time(605.0, &lang(FRENCH)), "10'05\"0");
        assert_eq!(time(59.95, &lang(SWEDISH)), "0:59,9");
        assert_eq!(time(0.0, &()), "0:00.0");
    }

    #[test]
    fn runtime_strings() {
        let mut s = strings();
        s.add(0x500, &units("F1\0junk")).unwrap();
        assert_eq!(s.add(0x11, &units("x")), Err(Error::Exists(0x11)));
        assert_eq!(s.add(0x500, &units("x")), Err(Error::Exists(0x500)));
        let h = s.acquire(0x500, 0, &()).unwrap().unwrap();
        assert_eq!(text(&s, h), "F1");
    }
}
