//! Localised string tables: `language/INDEX.SDX` and `language/<LANG>/COMMON.STS` (port of `tools/sts_parse.py`;
//! loader `CFECK_StringManager`, `FUN_004e0c60`, string lookup `FUN_004e08d0`).
//!
//! ```text
//! INDEX.SDX : u16 1 | u16 1 | u32 0x3f8443a3 | u32 n_strings | u32 n_files
//!             n_strings x { u32 string_id, u32 file_hash, u32 ? (0 or 2), u32 index }
//!             n_files   x { u32 file_hash, u32 name_off (file-relative C string, e.g. "COMMON") }
//! <LANG>.STS: u16 1 | u16 1 | u32 0x3f8443a3 | u32 language_hash | u32 file_hash | u32 n | u32 0
//!             n x { u32 string_id, u32 offset }               offsets ascending and contiguous
//!             string: u16 n_inserts | u16 n_chars | n_inserts x { u16 char_pos, u16 type, u32 id, u32 kind }
//!                     | n_chars x u16 (UTF-16LE) | u16 0 | pad to 4
//! ```
//! Inside the text, U+000B is followed by two code units holding a u32 (`render` writes it as `{0b:xxxxxxxx}`).

use crate::util::{bad, cstr_at, u16_at, u32_at, Result};

pub const MAGIC: u32 = 0x3F84_43A3;

/// The languages the data may have: the folder under `language/` (and its `.ccd`), the name whose hash
/// (`flow::name_hash`) is the table's language and `General_ChangeLanguage`'s argument, and the remaster's name for
/// it in English.
pub const LANGUAGES: [(&str, &str, &str); 19] = [
    ("US", "US", "English (US)"),
    ("ENGLISH", "English", "English (UK)"),
    ("FRENCH", "French", "French"),
    ("GERMAN", "German", "German"),
    ("ITALIAN", "Italian", "Italian"),
    ("SPANISH", "Spanish", "Spanish"),
    ("DUTCH", "Dutch", "Dutch"),
    ("SWEDISH", "Swedish", "Swedish"),
    ("DANISH", "Danish", "Danish"),
    ("NORWAY", "Norway", "Norwegian"),
    ("POLISH", "Polish", "Polish"),
    ("CZECH", "Czech", "Czech"),
    ("PORTUGAL", "Portugal", "Portuguese (Portugal)"),
    ("BRAZIL", "Brazil", "Portuguese (Brazil)"),
    ("JAPANESE", "Japanese", "Japanese"),
    ("KOREAN", "Korean", "Korean"),
    ("THAI", "Thai", "Thai"),
    ("SIMCHINA", "SimChina", "Chinese (Simplified)"),
    ("TRACHINA", "TraChina", "Chinese (Traditional)"),
];

/// The [`LANGUAGES`] entry of the language named `name` (its hash name, any case).
pub fn language(name: &str) -> Option<(&'static str, &'static str, &'static str)> {
    LANGUAGES.iter().copied().find(|(_, n, _)| n.eq_ignore_ascii_case(name))
}

fn check_header(d: &[u8], what: &str) -> Result<()> {
    if (u16_at(d, 0)?, u16_at(d, 2)?, u32_at(d, 4)?) != (1, 1, MAGIC) {
        return bad(format!("not an {what}"));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SdxString {
    pub id: u32,
    pub file_hash: u32,
    pub unk: u32,
    /// Index of the string in the file's `.STS` table.
    pub index: u32,
}

#[derive(Debug, Clone)]
pub struct Sdx {
    pub strings: Vec<SdxString>,
    /// `(file_hash, name)`; a repeated hash keeps its first position and the last name (like a Python dict).
    pub files: Vec<(u32, String)>,
}

impl Sdx {
    pub fn parse(d: &[u8]) -> Result<Sdx> {
        check_header(d, "SDX index")?;
        let (ns, nf) = (u32_at(d, 8)? as usize, u32_at(d, 12)? as usize);
        let mut strings = Vec::new();
        for i in 0..ns {
            let f = |k: usize| u32_at(d, 16 + 16 * i + 4 * k);
            strings.push(SdxString { id: f(0)?, file_hash: f(1)?, unk: f(2)?, index: f(3)? });
        }
        let base = 16 + 16 * ns;
        let mut end = base + 8 * nf;
        let mut files: Vec<(u32, String)> = Vec::new();
        for i in 0..nf {
            let (h, off) = (u32_at(d, base + 8 * i)?, u32_at(d, base + 8 * i + 4)? as usize);
            let name = cstr_at(d, off)?;
            end = end.max(off + name.chars().count() + 1); // one char per Latin-1 byte
            match files.iter_mut().find(|f| f.0 == h) {
                Some(f) => f.1 = name,
                None => files.push((h, name)),
            }
        }
        if end != d.len() {
            return bad(format!("SDX has {} trailing bytes", d.len() as i64 - end as i64));
        }
        Ok(Sdx { strings, files })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Insert {
    pub pos: u16,
    pub ty: u16,
    pub id: u32,
    pub kind: u32,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub id: u32,
    /// UTF-16 code units, without the terminator.
    pub units: Vec<u16>,
    pub inserts: Vec<Insert>,
}

#[derive(Debug, Clone)]
pub struct Sts {
    pub language: u32,
    pub file_hash: u32,
    /// The u32 at 0x14 (0 in every file; not checked).
    pub unk_14: u32,
    pub entries: Vec<Entry>,
}

impl Sts {
    pub fn parse(d: &[u8]) -> Result<Sts> {
        check_header(d, "STS string table")?;
        let (language, file_hash, n, unk_14) = (u32_at(d, 8)?, u32_at(d, 12)?, u32_at(d, 16)? as usize, u32_at(d, 20)?);
        let mut p = 0x18 + 8 * n;
        let mut entries = Vec::new();
        for i in 0..n {
            let (id, off) = (u32_at(d, 0x18 + 8 * i)?, u32_at(d, 0x1C + 8 * i)? as usize);
            if off != p {
                return bad(format!("string {i} at {off:#x}, expected {p:#x}"));
            }
            let (ni, nc) = (u16_at(d, off)?, u16_at(d, off + 2)?);
            let mut q = off + 4;
            let mut inserts = Vec::new();
            for _ in 0..ni {
                let ins = Insert {
                    pos: u16_at(d, q)?,
                    ty: u16_at(d, q + 2)?,
                    id: u32_at(d, q + 4)?,
                    kind: u32_at(d, q + 8)?,
                };
                if ins.pos > nc {
                    return bad(format!("string {i}: insert position {} > {nc}", ins.pos));
                }
                inserts.push(ins);
                q += 12;
            }
            let units = (0..nc as usize).map(|k| u16_at(d, q + 2 * k)).collect::<Result<Vec<_>>>()?;
            q += 2 * nc as usize;
            if d.get(q..q + 2) != Some(b"\0\0") {
                return bad(format!("string {i} not NUL-terminated"));
            }
            p = (q + 2 + 3) & !3;
            entries.push(Entry { id, units, inserts });
        }
        if p != d.len() {
            return bad(format!("table ends at {p:#x}, file is {:#x}", d.len()));
        }
        Ok(Sts { language, file_hash, unk_14, entries })
    }
}

/// Text of a string: `U+000B` plus two units become `{0b:xxxxxxxx}`, lone surrogates become U+FFFD.
pub fn render(units: &[u16]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < units.len() {
        let u = units[i];
        if u == 0x0B && i + 2 < units.len() {
            out += &format!("{{0b:{:08x}}}", units[i + 1] as u32 | (units[i + 2] as u32) << 16);
            i += 3;
            continue;
        }
        out.push(char::from_u32(u as u32).unwrap_or('\u{FFFD}'));
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(a: u32, b: u32, c: u32) -> Vec<u8> {
        let mut d = [1u8, 0, 1, 0].to_vec();
        for v in [MAGIC, a, b, c] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d
    }

    #[test]
    fn sdx_index() {
        let mut d = header(1, 1, 0);
        d.truncate(16);
        for v in [0xAAu32, 0xF0, 2, 0, 0xF0, 32 + 8] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(b"COMMON\0");
        let s = Sdx::parse(&d).unwrap();
        assert_eq!(s.strings, vec![SdxString { id: 0xAA, file_hash: 0xF0, unk: 2, index: 0 }]);
        assert_eq!(s.files, vec![(0xF0, "COMMON".to_string())]);
        d.push(0);
        assert!(Sdx::parse(&d).is_err());
    }

    #[test]
    fn sts_two_strings() {
        let mut d = header(0x1234, 0xF0, 2);
        d.extend_from_slice(&0u32.to_le_bytes());
        let first = 0x18 + 16;
        // "Hi" with one insert: 4 + 12 + 4 + 2 = 22, padded to 24; then "\x0b" + u32 + "!" (4 + 8 + 2 = 14 -> 16)
        for v in [0x11u32, first, 0x22, first + 24] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        for v in [1u16, 2, 2, 1] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&7u32.to_le_bytes());
        d.extend_from_slice(&0xa9cd_9885u32.to_le_bytes());
        for v in [b'H' as u16, b'i' as u16, 0, 0xAAAA, 0, 4, 0x0B, 0x5678, 0x1234, b'!' as u16, 0, 0] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        let s = Sts::parse(&d).unwrap();
        assert_eq!((s.language, s.file_hash), (0x1234, 0xF0));
        assert_eq!(s.entries[0].inserts, vec![Insert { pos: 2, ty: 1, id: 7, kind: 0xa9cd_9885 }]);
        assert_eq!(render(&s.entries[0].units), "Hi");
        assert_eq!(render(&s.entries[1].units), "{0b:12345678}!");
        assert_eq!(render(&[0xD800, 0x0B, 1]), "\u{FFFD}\u{b}\u{1}");
        d.extend_from_slice(&[0; 4]);
        assert!(Sts::parse(&d).is_err());
        d[0x1C] += 4; // first string no longer where the previous one ends
        assert!(Sts::parse(&d[..d.len() - 4]).is_err());
    }
}
