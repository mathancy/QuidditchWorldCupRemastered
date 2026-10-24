//! Shared helpers: errors, a little-endian byte reader, CRC-32.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Tried to read past the end of the data.
    Eof { offset: usize, wanted: usize },
    /// The data does not match the expected layout.
    Format(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Eof { offset, wanted } => write!(f, "read of {wanted} bytes at {offset:#x} past end of data"),
            Error::Format(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn bad<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error::Format(msg.into()))
}

/// Bounds-checked slice.
pub(crate) fn slice(d: &[u8], off: usize, len: usize) -> Result<&[u8]> {
    off.checked_add(len).and_then(|end| d.get(off..end)).ok_or(Error::Eof { offset: off, wanted: len })
}

pub(crate) fn u8_at(d: &[u8], off: usize) -> Result<u8> {
    Ok(slice(d, off, 1)?[0])
}

pub(crate) fn u16_at(d: &[u8], off: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(slice(d, off, 2)?.try_into().unwrap()))
}

pub(crate) fn i16_at(d: &[u8], off: usize) -> Result<i16> {
    Ok(i16::from_le_bytes(slice(d, off, 2)?.try_into().unwrap()))
}

pub(crate) fn u32_at(d: &[u8], off: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(slice(d, off, 4)?.try_into().unwrap()))
}

pub(crate) fn f32_at(d: &[u8], off: usize) -> Result<f32> {
    Ok(f32::from_le_bytes(slice(d, off, 4)?.try_into().unwrap()))
}

pub(crate) fn f32s<const N: usize>(d: &[u8], off: usize) -> Result<[f32; N]> {
    let mut out = [0f32; N];
    for (i, v) in out.iter_mut().enumerate() {
        *v = f32_at(d, off + 4 * i)?;
    }
    Ok(out)
}

/// Latin-1 decode (every byte maps to the code point of the same value), as the Python tools do.
pub(crate) fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

/// Python `str.splitlines` boundaries.
pub(crate) fn split_lines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}') {
            out.push(&s[start..i]);
            start = i + c.len_utf8();
            if c == '\r' && it.peek().map(|&(_, n)| n) == Some('\n') {
                it.next();
                start += 1;
            }
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// NUL-terminated Latin-1 string at `off`.
pub(crate) fn cstr_at(d: &[u8], off: usize) -> Result<String> {
    let rest = d.get(off..).ok_or(Error::Eof { offset: off, wanted: 1 })?;
    let end =
        rest.iter().position(|&c| c == 0).ok_or_else(|| Error::Format(format!("unterminated string at {off:#x}")))?;
    Ok(latin1(&rest[..end]))
}

/// Lower-case hex of `b` (Python `bytes.hex()`), for byte ranges kept undecoded.
pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|c| format!("{c:02x}")).collect()
}

/// Sequential little-endian reader over a byte slice.
pub(crate) struct Reader<'a> {
    pub d: &'a [u8],
    pub p: usize,
}

impl<'a> Reader<'a> {
    pub fn new(d: &'a [u8]) -> Self {
        Reader { d, p: 0 }
    }
    pub fn u8(&mut self) -> Result<u8> {
        let v = u8_at(self.d, self.p)?;
        self.p += 1;
        Ok(v)
    }
    pub fn u16(&mut self) -> Result<u16> {
        let v = u16_at(self.d, self.p)?;
        self.p += 2;
        Ok(v)
    }
    pub fn u32(&mut self) -> Result<u32> {
        let v = u32_at(self.d, self.p)?;
        self.p += 4;
        Ok(v)
    }
    pub fn f32(&mut self) -> Result<f32> {
        let v = f32_at(self.d, self.p)?;
        self.p += 4;
        Ok(v)
    }
    pub fn cstr(&mut self) -> Result<String> {
        let s = cstr_at(self.d, self.p)?;
        self.p += s.chars().count() + 1; // one char per Latin-1 byte
        Ok(s)
    }
    /// `u8 present` followed by a C string when present (`FUN_0040b050`).
    pub fn opt_str(&mut self) -> Result<Option<String>> {
        Ok(if self.u8()? != 0 { Some(self.cstr()?) } else { None })
    }
    pub fn at_end(&self) -> bool {
        self.p == self.d.len()
    }
}

/// CRC-32 (IEEE, as `zlib.crc32`), used to summarise large byte ranges in dumps.
pub fn crc32(data: &[u8]) -> u32 {
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
    !data.iter().fold(!0u32, |c, &b| t[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_zlib() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn reader_strings() {
        let d = b"\x01ab\0\x00\xe9\0";
        let mut r = Reader::new(d);
        assert_eq!(r.opt_str().unwrap().as_deref(), Some("ab"));
        assert_eq!(r.opt_str().unwrap(), None);
        assert_eq!(r.cstr().unwrap(), "\u{e9}");
        assert!(r.at_end());
        assert!(r.u8().is_err());
    }
}
