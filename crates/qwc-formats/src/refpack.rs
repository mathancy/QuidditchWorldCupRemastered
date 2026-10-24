//! EA RefPack (QFS) decompression, as used inside `.ccd` archives (port of `ccd_extract.refpack_decompress`).

use crate::util::{bad, slice, Error, Result};

fn byte(src: &[u8], i: usize) -> Result<u8> {
    src.get(i).copied().ok_or(Error::Eof { offset: i, wanted: 1 })
}

/// Decompress a RefPack stream. The header is `flags, 0xFB`, an optional compressed size, then the
/// big-endian unpacked size (4 bytes when `flags & 0x84`, else 3).
pub fn decompress(src: &[u8]) -> Result<Vec<u8>> {
    if src.len() < 2 || src[1] != 0xFB {
        return bad(format!("not RefPack (header {:02x?})", &src[..src.len().min(2)]));
    }
    let flags = src[0];
    let mut pos = 2;
    let size_len = if flags & 0x84 != 0 { 4 } else { 3 };
    if flags & 0x01 != 0 && flags & 0x04 == 0 {
        pos += size_len; // classic layout: the compressed size comes first
    }
    let out_size = slice(src, pos, size_len)?.iter().fold(0usize, |a, &b| (a << 8) | b as usize);
    pos += size_len;

    let mut out: Vec<u8> = Vec::with_capacity(out_size);
    while pos < src.len() {
        let b0 = src[pos];
        let (lit, length, offset);
        if b0 < 0x80 {
            let b1 = byte(src, pos + 1)? as usize;
            pos += 2;
            lit = (b0 & 3) as usize;
            length = (((b0 & 0x1C) >> 2) + 3) as usize;
            offset = (((b0 as usize) & 0x60) << 3) + b1 + 1;
        } else if b0 < 0xC0 {
            let (b1, b2) = (byte(src, pos + 1)? as usize, byte(src, pos + 2)? as usize);
            pos += 3;
            lit = b1 >> 6;
            length = (b0 as usize & 0x3F) + 4;
            offset = ((b1 & 0x3F) << 8) + b2 + 1;
        } else if b0 < 0xE0 {
            let (b1, b2, b3) =
                (byte(src, pos + 1)? as usize, byte(src, pos + 2)? as usize, byte(src, pos + 3)? as usize);
            pos += 4;
            lit = (b0 & 3) as usize;
            length = ((b0 as usize & 0x0C) << 6) + b3 + 5;
            offset = ((b0 as usize & 0x10) << 12) + (b1 << 8) + b2 + 1;
        } else if b0 < 0xFC {
            pos += 1;
            let lit = ((b0 as usize & 0x1F) << 2) + 4;
            out.extend_from_slice(slice(src, pos, lit)?);
            pos += lit;
            continue;
        } else {
            // 0xFC-0xFF: final literals, end of stream
            let lit = (b0 & 3) as usize;
            out.extend_from_slice(slice(src, pos + 1, lit)?);
            break;
        }
        out.extend_from_slice(slice(src, pos, lit)?);
        pos += lit;
        let start = out
            .len()
            .checked_sub(offset)
            .ok_or_else(|| Error::Format(format!("back-reference {offset} before start of output")))?;
        for i in 0..length {
            // byte by byte: the source may overlap the bytes being written
            let b = out[start + i];
            out.push(b);
        }
    }
    if out.len() != out_size {
        return bad(format!("RefPack size mismatch: got {}, header says {out_size}", out.len()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_and_overlapping_copy() {
        // header 10 FB, 3-byte size 7; 0xE0 = 4 literals "abcd"; short copy (0x00 0x00): 0 lit, len 3, offset 1;
        // 0xFC = end with 0 literals.
        let src = [0x10, 0xFB, 0, 0, 7, 0xE0, b'a', b'b', b'c', b'd', 0x00, 0x00, 0xFC];
        assert_eq!(decompress(&src).unwrap(), b"abcdddd");
    }

    #[test]
    fn four_byte_size_and_trailing_literals() {
        let src = [0x15, 0xFB, 0, 0, 0, 2, 0xFE, b'x', b'y'];
        assert_eq!(decompress(&src).unwrap(), b"xy");
    }

    #[test]
    fn size_mismatch_is_an_error() {
        assert!(decompress(&[0x10, 0xFB, 0, 0, 9, 0xFD, b'x']).is_err());
        assert!(decompress(&[0x10, 0xFA]).is_err());
    }
}
