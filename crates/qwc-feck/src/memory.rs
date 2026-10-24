//! The VM's 32-bit address space. In the original the scripts address host memory directly; here every loaded
//! section, the script stack and a host heap are regions at fixed virtual bases, and anything else faults.

use std::fmt;

/// Where the regions live. Each loaded script gets a 16 MB window: code, data, bss and lit 4 MB apart, each from
/// [`SECTION_OFFSET`] into its 4 MB.
pub const STACK_BASE: u32 = 0x0800_0000;
pub const STACK_SIZE: u32 = 0x0004_0000;
pub const HEAP_BASE: u32 = 0x0900_0000;
pub const SCRIPT_BASE: u32 = 0x1000_0000;
pub const SCRIPT_STRIDE: u32 = 0x0100_0000;
pub const SECTION_STRIDE: u32 = 0x0040_0000;
/// Where a section starts in its 4 MB. Not 0: the calls and operands take an address ending in `0xff` for an import
/// number (`(b2 * 256 + b1 - 1 + b3) / 2`, `docs/FECK_VM.md` "Calls"), and the original's heap addresses always
/// decode past the import table. A code section at its window's start has bits 16-23 at 0, so a function at an
/// offset ending in `0xff` read as an import (the audio options' `SetSpriteFrame` called a GBA native instead,
/// found 2026-10-23). From 1 MB in, those bits are at least `0x10`: no import number below 2048.
pub const SECTION_OFFSET: u32 = 0x0010_0000;

/// A memory access outside every region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fault {
    pub addr: u32,
    pub len: u32,
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "access to unmapped memory at {:#010x} ({} bytes)", self.addr, self.len)
    }
}

#[derive(Debug, Clone)]
struct Region {
    base: u32,
    bytes: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct Memory {
    /// Sorted by base; regions never overlap.
    regions: Vec<Region>,
    heap_top: u32,
}

impl Memory {
    pub fn new() -> Memory {
        let mut m = Memory { regions: Vec::new(), heap_top: HEAP_BASE };
        m.map(STACK_BASE, vec![0; STACK_SIZE as usize]);
        m
    }

    /// Map `bytes` at `base` (replacing a region with the same base).
    pub fn map(&mut self, base: u32, bytes: Vec<u8>) {
        match self.regions.binary_search_by_key(&base, |r| r.base) {
            Ok(i) => self.regions[i].bytes = bytes,
            Err(i) => self.regions.insert(i, Region { base, bytes }),
        }
    }

    /// Allocate `len` zeroed bytes on the host heap (for strings and structures the natives hand to scripts),
    /// 16-byte aligned. Returns the address.
    pub fn alloc(&mut self, len: usize) -> u32 {
        let addr = self.heap_top;
        self.map(addr, vec![0; len.max(1)]);
        self.heap_top = addr + ((len.max(1) as u32 + 15) & !15);
        addr
    }

    /// Allocate and fill a NUL-terminated string.
    pub fn alloc_cstr(&mut self, s: &[u8]) -> u32 {
        let a = self.alloc(s.len() + 1);
        self.write(a, s).unwrap();
        a
    }

    fn find(&self, addr: u32, len: u32) -> Result<(usize, usize), Fault> {
        let i = match self.regions.binary_search_by_key(&addr, |r| r.base) {
            Ok(i) => i,
            Err(0) => return Err(Fault { addr, len }),
            Err(i) => i - 1,
        };
        let r = &self.regions[i];
        let off = (addr - r.base) as usize;
        if off + len as usize <= r.bytes.len() {
            Ok((i, off))
        } else {
            Err(Fault { addr, len })
        }
    }

    pub fn read(&self, addr: u32, len: u32) -> Result<&[u8], Fault> {
        let (i, off) = self.find(addr, len)?;
        Ok(&self.regions[i].bytes[off..off + len as usize])
    }

    pub fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), Fault> {
        let (i, off) = self.find(addr, data.len() as u32)?;
        self.regions[i].bytes[off..off + data.len()].copy_from_slice(data);
        Ok(())
    }

    pub fn u8(&self, addr: u32) -> Result<u8, Fault> {
        Ok(self.read(addr, 1)?[0])
    }

    pub fn u16(&self, addr: u32) -> Result<u16, Fault> {
        let b = self.read(addr, 2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&self, addr: u32) -> Result<u32, Fault> {
        let b = self.read(addr, 4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn f32(&self, addr: u32) -> Result<f32, Fault> {
        self.u32(addr).map(f32::from_bits)
    }

    pub fn set_u8(&mut self, addr: u32, v: u8) -> Result<(), Fault> {
        self.write(addr, &[v])
    }

    pub fn set_u16(&mut self, addr: u32, v: u16) -> Result<(), Fault> {
        self.write(addr, &v.to_le_bytes())
    }

    pub fn set_u32(&mut self, addr: u32, v: u32) -> Result<(), Fault> {
        self.write(addr, &v.to_le_bytes())
    }

    pub fn set_f32(&mut self, addr: u32, v: f32) -> Result<(), Fault> {
        self.set_u32(addr, v.to_bits())
    }

    /// A NUL-terminated string (at most 4096 bytes).
    pub fn cstr(&self, addr: u32) -> Result<Vec<u8>, Fault> {
        let mut out = Vec::new();
        for i in 0..4096 {
            match self.u8(addr + i)? {
                0 => break,
                b => out.push(b),
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_and_faults() {
        let mut m = Memory::new();
        m.map(0x1000, vec![1, 2, 3, 4, 5]);
        assert_eq!(m.u32(0x1000).unwrap(), 0x0403_0201);
        assert_eq!(m.u32(0x1002), Err(Fault { addr: 0x1002, len: 4 }));
        assert!(m.u8(0xfff).is_err());
        m.set_u16(0x1003, 0xbeef).unwrap();
        assert_eq!(m.read(0x1000, 5).unwrap(), [1, 2, 3, 0xef, 0xbe]);
        let s = m.alloc_cstr(b"hi");
        assert_eq!(m.cstr(s).unwrap(), b"hi");
        assert_eq!(m.alloc(1) % 16, 0);
        assert_eq!(m.u32(STACK_BASE + STACK_SIZE - 4).unwrap(), 0);
    }
}
