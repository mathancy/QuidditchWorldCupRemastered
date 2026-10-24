//! `.ccd` "FKNL" archives (port of `tools/ccd_extract.py`).
//!
//! ```text
//! 0x00 "FKNL" | u32 version (2) | u32 header_size | u32 unpacked_size | u32 packed_size | u32 count
//! 0x38 count x { u32 name_off, u32 data_off, u32 size, u32 size2 }
//! header_size..: RefPack blob holding every member back to back
//! ```

use crate::refpack;
use crate::util::{bad, cstr_at, slice, u32_at, Result};

#[derive(Debug, Clone)]
pub struct Member {
    pub name: String,
    pub offset: usize,
    pub size: usize,
    pub size2: u32,
}

#[derive(Debug, Clone)]
pub struct Ccd {
    pub version: u32,
    /// The decompressed blob; members are ranges of it.
    pub blob: Vec<u8>,
    pub members: Vec<Member>,
}

/// The archive's header and member table, read without decompressing the blob.
#[derive(Debug, Clone)]
pub struct Directory {
    pub version: u32,
    pub header_size: usize,
    pub unpacked_size: usize,
    pub packed_size: usize,
    pub members: Vec<Member>,
}

impl Directory {
    pub fn parse(data: &[u8]) -> Result<Directory> {
        if slice(data, 0, 4)? != b"FKNL" {
            return bad("not an FKNL archive");
        }
        let version = u32_at(data, 4)?;
        let header_size = u32_at(data, 8)? as usize;
        let unpacked_size = u32_at(data, 12)? as usize;
        let packed_size = u32_at(data, 16)? as usize;
        let count = u32_at(data, 20)? as usize;
        if header_size + packed_size != data.len() {
            return bad(format!("size check failed: {header_size}+{packed_size} != {}", data.len()));
        }
        let mut members = Vec::with_capacity(count);
        for i in 0..count {
            let e = 0x38 + i * 16;
            let name = cstr_at(data, u32_at(data, e)? as usize)?;
            let offset = u32_at(data, e + 4)? as usize;
            let size = u32_at(data, e + 8)? as usize;
            if offset + size > unpacked_size {
                return bad(format!("member {name} overruns blob"));
            }
            members.push(Member { name, offset, size, size2: u32_at(data, e + 12)? });
        }
        Ok(Directory { version, header_size, unpacked_size, packed_size, members })
    }

    /// Decompress the archive's blob (`data` is the whole `.ccd` file this directory was read from).
    pub fn unpack(&self, data: &[u8]) -> Result<Vec<u8>> {
        let blob = refpack::decompress(slice(data, self.header_size, self.packed_size)?)?;
        if blob.len() != self.unpacked_size {
            return bad(format!("unpacked size {} != header {}", blob.len(), self.unpacked_size));
        }
        Ok(blob)
    }
}

impl Ccd {
    pub fn parse(data: &[u8]) -> Result<Ccd> {
        let dir = Directory::parse(data)?;
        let blob = dir.unpack(data)?;
        Ok(Ccd { version: dir.version, blob, members: dir.members })
    }

    pub fn member_data(&self, m: &Member) -> &[u8] {
        &self.blob[m.offset..m.offset + m.size]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_members() {
        // 0xE0: 4 literals "hell"; 0xFD: final 1 literal "o"
        let packed = [0x10, 0xFB, 0, 0, 5, 0xE0, b'h', b'e', b'l', b'l', 0xFD, b'o'];
        let names = b"a.txt\0b.txt\0";
        let hdr = 0x38 + 2 * 16 + names.len();
        let mut d = Vec::new();
        d.extend_from_slice(b"FKNL");
        for v in [2u32, hdr as u32, 5, packed.len() as u32, 2] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.resize(0x38, 0);
        let name0 = (0x38 + 32) as u32;
        for v in [name0, 0, 3, 3, name0 + 6, 3, 2, 2] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(names);
        d.extend_from_slice(&packed);
        let c = Ccd::parse(&d).unwrap();
        assert_eq!(c.version, 2);
        assert_eq!(c.member_data(&c.members[0]), b"hel");
        assert_eq!(c.members[1].name, "b.txt");
        assert_eq!(c.member_data(&c.members[1]), b"lo");
        d.push(0);
        assert!(Ccd::parse(&d).is_err());
    }
}
