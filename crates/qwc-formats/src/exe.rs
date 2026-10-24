//! Tables read from a decrypted `QWC.exe` as the program runs (the repository holds none of the exe's bytes): its
//! sections, and the data the port needs from it (the load-set table is `flow::loadset_table`).

use crate::util::{bad, slice, u32_at, Result};

/// A PE section: (virtual address, raw size, raw offset).
pub(crate) type Section = (u32, u32, u32);

/// The image base and the sections of a PE image.
pub(crate) fn pe_sections(exe: &[u8]) -> Result<(u32, Vec<Section>)> {
    if slice(exe, 0, 2)? != b"MZ" {
        return bad("not an exe");
    }
    let pe = u32_at(exe, 0x3c)? as usize;
    if slice(exe, pe, 4)? != b"PE\0\0" {
        return bad("no PE header");
    }
    let n = u16::from_le_bytes(slice(exe, pe + 6, 2)?.try_into().unwrap()) as usize;
    let opt = u16::from_le_bytes(slice(exe, pe + 20, 2)?.try_into().unwrap()) as usize;
    let base = u32_at(exe, pe + 24 + 28)?;
    let first = pe + 24 + opt;
    let mut s = Vec::new();
    for i in 0..n {
        let h = first + 40 * i;
        s.push((u32_at(exe, h + 12)?, u32_at(exe, h + 16)?, u32_at(exe, h + 20)?));
    }
    Ok((base, s))
}

/// The file offset of a virtual address in initialised data.
pub(crate) fn va_offset(base: u32, sections: &[Section], va: u32) -> Option<usize> {
    let rva = va.checked_sub(base)?;
    sections.iter().find(|&&(v, size, _)| v <= rva && rva < v + size).map(|&(v, _, raw)| (raw + rva - v) as usize)
}

/// The Quidditch cards' numbers (`0x5f1988`, only `GBALink_SendQCardPacket` reads it: `0x4d4cb0`, `0x4d4ab0`): for
/// each of the 15 rows of the DataStore's card bits (`0xacfe6233`), the number (1 to 104) of the card each of its
/// first 10 bits stands for, -1 for none.
pub type CardNumbers = [[i8; 10]; 15];

const CARD_NUMBERS: u32 = 0x5f1988;

/// Read the card numbers. Fails unless every card 1 to 104 is there once and the rest is -1.
pub fn card_numbers(exe: &[u8]) -> Result<CardNumbers> {
    let (base, sections) = pe_sections(exe)?;
    let Some(at) = va_offset(base, &sections, CARD_NUMBERS) else { return bad("no card table") };
    let d = slice(exe, at, 150)?;
    let mut t = [[0i8; 10]; 15];
    let mut seen = [false; 105];
    for (k, &b) in d.iter().enumerate() {
        let v = b as i8;
        t[k / 10][k % 10] = v;
        match v {
            -1 => {}
            1..=104 if !seen[v as usize] => seen[v as usize] = true,
            _ => return bad("the card table does not read as expected"),
        }
    }
    if seen[1..].iter().any(|&s| !s) {
        return bad("the card table lacks a card");
    }
    Ok(t)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A PE image of one section at `va` (image base 0x400000) holding `data` from file offset 0x200.
    pub(crate) fn pe(va: u32, data: &[u8]) -> Vec<u8> {
        let mut d = vec![0u8; 0x200];
        d[..2].copy_from_slice(b"MZ");
        d[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        d[0x40..0x44].copy_from_slice(b"PE\0\0");
        d[0x46..0x48].copy_from_slice(&1u16.to_le_bytes());
        // an optional header of 0xe0 bytes, the image base at its 28
        d[0x54..0x56].copy_from_slice(&0xe0u16.to_le_bytes());
        d[0x58 + 28..0x58 + 32].copy_from_slice(&0x400000u32.to_le_bytes());
        let h = 0x58 + 0xe0;
        for (k, v) in [va - 0x400000, data.len() as u32, 0x200].into_iter().enumerate() {
            d[h + 12 + 4 * k..h + 16 + 4 * k].copy_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(data);
        d
    }

    #[test]
    fn card_numbers_read_and_check() {
        // cards 1-104 in order, ten a row, the rest -1
        let mut table = [0xffu8; 150];
        for c in 1..=104u8 {
            table[(c - 1) as usize] = c;
        }
        let t = card_numbers(&pe(CARD_NUMBERS, &table)).unwrap();
        assert_eq!((t[0][0], t[10][3], t[10][4], t[14][9]), (1, 104, -1, -1));
        // a card twice, or one missing, is refused
        table[1] = 1;
        assert!(card_numbers(&pe(CARD_NUMBERS, &table)).is_err());
        assert!(card_numbers(b"MZ").is_err());
    }
}
