//! The FECK script VM (`CFECK_VM`, interpreter `FUN_004df7d0`, entry `ExecuteScript` `FUN_004df700`; see
//! `docs/RE_NOTES.md`, ".vso", and `docs/FECK_VM.md` for the traced semantics this follows).
//!
//! An lcc-style stack machine over 4-byte slots. The stack grows upwards and `sp` points past the top slot. A
//! function's `ENTER L, A` reserves `L` bytes of locals and `A` bytes for the outgoing argument block; `ARG*` move the
//! top slot into that block; a call passes the block's address, which the callee reads with `ADDRF`. Narrow constants,
//! loads and conversions touch only the low bytes of a slot, exactly as the original does.

use std::fmt;

use qwc_formats::vso::Vso;

use crate::memory::{
    Fault, Memory, SCRIPT_BASE, SCRIPT_STRIDE, SECTION_OFFSET, SECTION_STRIDE, STACK_BASE, STACK_SIZE,
};

/// Where `execute` puts the host's arguments: one 256-byte block per nesting level.
const HOST_ARGS: u32 = STACK_BASE + STACK_SIZE;
const HOST_ARGS_SIZE: u32 = 256;
const MAX_DEPTH: u32 = 16;

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    Fault(Fault),
    /// `.vso` parse error.
    Format(String),
    /// No exported function with this hash (`0x8004000f` in the original).
    NoFunction(u32),
    /// An import that neither a loaded script nor the host provides (`0x80040012`).
    Unresolved(String),
    /// A call target that is no function.
    BadTarget(u32),
    DivideByZero,
    /// More than the configured number of instructions in one `execute`.
    StepLimit,
    TooDeep,
    /// A native reported an error.
    Native(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Fault(x) => write!(f, "{x}"),
            Error::Format(s) => write!(f, "bad .vso: {s}"),
            Error::NoFunction(h) => write!(f, "no function with hash {h:#010x}"),
            Error::Unresolved(n) => write!(f, "unresolved import {n}"),
            Error::BadTarget(a) => write!(f, "call to {a:#010x}, which is not a function"),
            Error::DivideByZero => write!(f, "integer division by zero"),
            Error::StepLimit => write!(f, "step limit reached"),
            Error::TooDeep => write!(f, "calls nested too deeply"),
            Error::Native(s) => write!(f, "native: {s}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<Fault> for Error {
    fn from(f: Fault) -> Error {
        Error::Fault(f)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// The engine side: the natives (`FUN_004df4b0` tables, 211 in the main one).
pub trait Host {
    /// Whether the host provides the native named by this hash.
    fn has_native(&self, hash: u32) -> bool;
    /// Call native `hash` with the argument slots in order; bit `i` of `floats` is set when `args[i]` was passed
    /// with `ARGF`. The result is a raw slot (a float's bits for `CALLF`); it is ignored for `CALLV`.
    /// The native gets the VM itself: its memory, and the scripts to call back into (a nested `execute`).
    fn call(&mut self, vm: &mut Vm, hash: u32, args: &[u32], floats: u32) -> Result<u32>;
}

/// A symbol a script exports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Export {
    pub name: String,
    pub hash: u32,
    pub addr: u32,
    pub function: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Link {
    Unresolved,
    Native(u32),
    Script { addr: u32 },
}

#[derive(Debug, Clone)]
pub struct Script {
    pub name: String,
    pub hash: u32,
    /// Virtual bases and sizes of code, data, lit and bss.
    pub code: (u32, u32),
    pub data: (u32, u32),
    pub lit: (u32, u32),
    pub bss: (u32, u32),
    pub exports: Vec<Export>,
    imports: Vec<(String, u32, Link)>,
}

impl Script {
    fn contains_code(&self, addr: u32) -> bool {
        addr >= self.code.0 && addr < self.code.0 + self.code.1
    }

    pub fn export(&self, hash: u32) -> Option<&Export> {
        self.exports.iter().find(|e| e.hash == hash)
    }

    pub fn import_names(&self) -> impl Iterator<Item = &str> {
        self.imports.iter().map(|(n, _, _)| n.as_str())
    }
}

/// Decode an import operand (`0xff` + 24-bit number): `Some(index)` as the interpreter computes it.
/// Where section `k` (code, data, bss, lit) of the `idx`-th script loaded starts (`crate::memory::SECTION_OFFSET`).
fn section_base(idx: usize, k: u32) -> u32 {
    SCRIPT_BASE + idx as u32 * SCRIPT_STRIDE + k * SECTION_STRIDE + SECTION_OFFSET
}

fn import_index(v: u32, n_imports: usize) -> Option<usize> {
    if v & 0xff != 0xff {
        return None;
    }
    let (b1, b2, b3) = ((v >> 8) & 0xff, (v >> 16) & 0xff, v >> 24);
    let i = (b2 as i64 * 256 + b1 as i64 - 1 + b3 as i64) >> 1;
    // the original accepts i <= n (one past the table); the shipped code never uses it
    (i >= 0 && (i as usize) < n_imports).then_some(i as usize)
}

/// `_ftol`: truncation, `0x80000000` when out of range.
fn ftol(f: f32) -> u32 {
    let t = f.trunc();
    if t.is_nan() || !(-2147483648.0..2147483648.0).contains(&t) {
        0x8000_0000
    } else {
        t as i32 as u32
    }
}

pub struct Vm {
    pub mem: Memory,
    pub scripts: Vec<Script>,
    sp: u32,
    depth: u32,
    steps: u64,
    /// Instructions allowed per `execute` (0: no limit).
    pub step_limit: u64,
}

impl Default for Vm {
    fn default() -> Self {
        Vm::new()
    }
}

/// One function activation's registers (the interpreter's locals).
struct Frame {
    script: usize,
    pc: u32,
    /// `ADDRF` base: the caller's argument block.
    args: u32,
    entry_sp: u32,
    locals: u32,
    locals_size: u32,
    arg_block: u32,
    arg_size: u32,
    arg_ptr: u32,
    n_args: u32,
    floats: u32,
}

impl Vm {
    pub fn new() -> Vm {
        let mut mem = Memory::new();
        mem.map(HOST_ARGS, vec![0; (HOST_ARGS_SIZE * MAX_DEPTH) as usize]);
        Vm { mem, scripts: Vec::new(), sp: STACK_BASE, depth: 0, steps: 0, step_limit: 5_000_000 }
    }

    /// Load a `.vso` (`FUN_004deb30` and the relocations of `FUN_004dec00`): map its sections, relocate, and list its
    /// exports and imports. Imports are bound by [`Vm::link`].
    pub fn load(&mut self, name: &str, bytes: &[u8]) -> Result<usize> {
        let v = Vso::parse(bytes).map_err(|e| Error::Format(format!("{name}: {e}")))?;
        let idx = self.scripts.len();
        let sec = |k: u32| section_base(idx, k);
        let (code, data, lit) = (v.code.to_vec(), v.data.to_vec(), v.lit.to_vec());
        let mut secs = [code, data, vec![0; v.bss_size as usize], lit];
        let at = |s: &str| ["code", "data", "bss", "lit"].iter().position(|n| *n == s);
        for r in &v.relocs {
            let (Some(s), Some(t)) = (at(r.section), at(r.target)) else {
                return Err(Error::Format(format!("{name}: relocation between {} and {}", r.section, r.target)));
            };
            let o = r.offset as usize;
            let value = (sec(t as u32) as i64 + r.addend as i64) as u32;
            secs[s]
                .get_mut(o..o + 4)
                .ok_or_else(|| Error::Format(format!("{name}: relocation at {o:#x} outside {}", r.section)))?
                .copy_from_slice(&value.to_le_bytes());
        }
        let sizes: Vec<u32> = secs.iter().map(|s| s.len() as u32).collect();
        for (k, s) in secs.into_iter().enumerate() {
            self.mem.map(sec(k as u32), s);
        }
        let exports = v
            .symbols
            .iter()
            .filter_map(|s| {
                let k = at(s.section)?;
                Some(Export {
                    name: s.name.clone(),
                    hash: s.hash,
                    addr: sec(k as u32) + s.value,
                    function: s.flags & 0x40 != 0,
                })
            })
            .collect();
        let imports = v.imports.iter().map(|i| (i.name.clone(), i.hash, Link::Unresolved)).collect();
        self.scripts.push(Script {
            name: name.to_string(),
            hash: v.hash,
            code: (sec(0), sizes[0]),
            data: (sec(1), sizes[1]),
            bss: (sec(2), sizes[2]),
            lit: (sec(3), sizes[3]),
            exports,
            imports,
        });
        Ok(idx)
    }

    /// Bind every import as `FUN_004df380` resolves it: a symbol the script itself exports (`0x4deea0` searches
    /// only the calling script), else a native the host has (`0x4df320`). Nothing binds to another script's
    /// exports, so such imports stay unresolved and fail when used (`0x80040012`), as in the original.
    pub fn link(&mut self, host: &dyn Host) {
        for s in 0..self.scripts.len() {
            for i in 0..self.scripts[s].imports.len() {
                let hash = self.scripts[s].imports[i].1;
                let found = self.scripts[s].export(hash).map(|e| Link::Script { addr: e.addr });
                self.scripts[s].imports[i].2 = match found {
                    Some(l) => l,
                    None if host.has_native(hash) => Link::Native(hash),
                    None => Link::Unresolved,
                };
            }
        }
    }

    /// The imports nothing provides, as `(script, import)` names.
    pub fn unresolved(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for s in &self.scripts {
            for (n, _, l) in &s.imports {
                if *l == Link::Unresolved {
                    out.push((s.name.clone(), n.clone()));
                }
            }
        }
        out
    }

    fn script_of(&self, addr: u32) -> Option<usize> {
        self.scripts.iter().position(|s| s.contains_code(addr))
    }

    /// `ExecuteScript` (`FUN_004df700`): run the function `hash` exported by `script` with `args` and return the
    /// slot its `RET` left (garbage for a function that returns nothing).
    pub fn execute(&mut self, host: &mut dyn Host, script: usize, hash: u32, args: &[u32]) -> Result<u32> {
        let f = self.scripts[script].export(hash).filter(|e| e.function).ok_or(Error::NoFunction(hash))?.addr;
        self.call_addr(host, f, args)
    }

    /// Run the script function at `addr` (as `execute` does once it has the address).
    pub fn call_addr(&mut self, host: &mut dyn Host, addr: u32, args: &[u32]) -> Result<u32> {
        if self.depth >= MAX_DEPTH {
            return Err(Error::TooDeep);
        }
        let script = self.script_of(addr).ok_or(Error::BadTarget(addr))?;
        let block = HOST_ARGS + self.depth * HOST_ARGS_SIZE;
        for (i, a) in args.iter().take((HOST_ARGS_SIZE / 4) as usize).enumerate() {
            self.mem.set_u32(block + 4 * i as u32, *a)?;
        }
        if self.depth == 0 {
            self.steps = 0;
        }
        self.depth += 1;
        let r = self.run(host, script, addr, block);
        self.depth -= 1;
        if let (Err(e), true) = (&r, std::env::var_os("QWC_VM_TRACE").is_some()) {
            let s = &self.scripts[script];
            eprintln!("vm: {}: function at code+{:#x}: {e}", s.name, addr - s.code.0);
        }
        let out = r.and_then(|_| Ok(self.mem.u32(self.sp)?));
        if self.depth == 0 {
            self.sp = STACK_BASE;
        }
        out
    }

    fn push(&mut self, v: u32) -> Result<()> {
        self.mem.set_u32(self.sp, v)?;
        self.sp = self.sp.wrapping_add(4);
        Ok(())
    }

    fn top(&self, k: u32) -> Result<u32> {
        Ok(self.mem.u32(self.sp.wrapping_sub(4 * k))?)
    }

    fn set_top(&mut self, k: u32, v: u32) -> Result<()> {
        Ok(self.mem.set_u32(self.sp.wrapping_sub(4 * k), v)?)
    }

    /// `FUN_004e0410`: an `ADDRG` or branch operand; an import operand becomes the address of the script symbol it
    /// names, and stays encoded for a native (the call resolves it).
    fn operand(&self, f: &Frame) -> Result<u32> {
        let v = self.mem.u32(f.pc)?;
        let s = &self.scripts[f.script];
        Ok(match import_index(v, s.imports.len()).map(|i| s.imports[i].2) {
            Some(Link::Script { addr }) => addr,
            _ => v,
        })
    }

    fn run(&mut self, host: &mut dyn Host, script: usize, pc: u32, args: u32) -> Result<()> {
        let sp = self.sp;
        let mut f = Frame {
            script,
            pc,
            args,
            entry_sp: sp,
            locals: 0,
            locals_size: 0,
            arg_block: 0,
            arg_size: 0,
            arg_ptr: 0,
            n_args: 0,
            floats: 0,
        };
        loop {
            self.steps += 1;
            if self.step_limit != 0 && self.steps > self.step_limit {
                return Err(Error::StepLimit);
            }
            let op = self.mem.u8(f.pc)?;
            f.pc += 1;
            match op {
                0x00 => {
                    // ENTER locals, args
                    f.locals_size = self.mem.u16(f.pc)? as u32;
                    f.arg_size = self.mem.u16(f.pc + 2)? as u32;
                    f.pc += 4;
                    if f.locals_size != 0 {
                        f.locals = self.sp;
                        self.sp += f.locals_size & !3;
                    }
                    if f.arg_size != 0 {
                        f.arg_block = self.sp;
                        f.arg_ptr = self.sp;
                        self.sp += f.arg_size & !3;
                    }
                }
                0x01 => {
                    // END
                    self.sp = f.entry_sp;
                    return Ok(());
                }
                0x02 | 0x05 | 0x08 | 0x09 => {
                    let v = self.mem.u32(f.pc)?;
                    f.pc += 4;
                    self.push(v)?;
                }
                0x03 | 0x06 => {
                    // one-byte constants write only the slot's low byte
                    let v = self.mem.u8(f.pc)?;
                    f.pc += 1;
                    self.mem.set_u8(self.sp, v)?;
                    self.sp += 4;
                }
                0x04 | 0x07 => {
                    let v = self.mem.u16(f.pc)?;
                    f.pc += 2;
                    self.mem.set_u16(self.sp, v)?;
                    self.sp += 4;
                }
                0x0A..=0x0D => {
                    // ARGF/I/U/P
                    let v = self.top(1)?;
                    self.mem.set_u32(f.arg_ptr, v)?;
                    f.arg_ptr += 4;
                    if op == 0x0A {
                        f.floats |= 1 << (f.n_args & 31);
                    }
                    f.n_args += 1;
                    self.sp -= 4;
                }
                0x0F | 0x12 | 0x15 | 0x16 => {
                    let (a, v) = (self.top(2)?, self.top(1)?);
                    self.mem.set_u32(a, v)?;
                    self.sp -= 8;
                }
                0x10 | 0x13 => {
                    let (a, v) = (self.top(2)?, self.top(1)?);
                    self.mem.set_u8(a, v as u8)?;
                    self.sp -= 8;
                }
                0x11 | 0x14 => {
                    let (a, v) = (self.top(2)?, self.top(1)?);
                    self.mem.set_u16(a, v as u16)?;
                    self.sp -= 8;
                }
                0x18 | 0x1B | 0x1E | 0x1F => {
                    let v = self.mem.u32(self.top(1)?)?;
                    self.set_top(1, v)?;
                }
                0x19 | 0x1C => {
                    let v = self.mem.u8(self.top(1)?)?;
                    self.mem.set_u8(self.sp - 4, v)?;
                }
                0x1A | 0x1D => {
                    let v = self.mem.u16(self.top(1)?)?;
                    self.mem.set_u16(self.sp - 4, v)?;
                }
                0x20 => {
                    // ASGNB: u8 0, u16 bytes (whole words copied)
                    let n = self.mem.u16(f.pc + 1)? as u32 >> 2;
                    f.pc += 3;
                    let (dst, src) = (self.top(2)?, self.top(1)?);
                    for i in 0..n {
                        let w = self.mem.u32(src + 4 * i)?;
                        self.mem.set_u32(dst + 4 * i, w)?;
                    }
                    self.sp -= 8;
                }
                0x22..=0x31 => {
                    let size = self.mem.u8(f.pc)?;
                    f.pc += 1;
                    self.convert(op, size)?;
                }
                0x32 => {
                    let v = f32::from_bits(self.top(1)?);
                    self.set_top(1, (-v).to_bits())?;
                }
                0x33 => {
                    let v = self.top(1)? as i32;
                    self.set_top(1, v.wrapping_neg() as u32)?;
                }
                0x34..=0x38 | 0x7A => self.call(host, &mut f, op)?,
                0x43..=0x46 => {
                    // RETF/I/U/P: the value goes to the slot at the new sp (not below it)
                    let v = self.top(1)?;
                    self.sp -= (f.locals_size + f.arg_size) & !3;
                    self.mem.set_u32(self.sp, v)?;
                    return Ok(());
                }
                0x47 => {
                    self.sp -= (f.locals_size + f.arg_size) & !3;
                    return Ok(());
                }
                0x48 => {
                    let v = self.operand(&f)?;
                    f.pc += 4;
                    self.push(v)?;
                }
                0x49 => {
                    let o = self.mem.u16(f.pc)? as u32;
                    f.pc += 2;
                    self.push(f.args + (o >> 2) * 4)?;
                }
                0x4A => {
                    let o = self.mem.u16(f.pc)? as u32;
                    f.pc += 2;
                    self.push(f.locals + o)?;
                }
                0x4B..=0x66 => self.arith(op)?,
                0x67..=0x78 => {
                    let (a, b) = (self.top(2)?, self.top(1)?);
                    let (fa, fb) = (f32::from_bits(a), f32::from_bits(b));
                    let (ia, ib) = (a as i32, b as i32);
                    let taken = match op {
                        // EQF compares the bits, like EQI
                        0x67..=0x69 => a == b,
                        0x6A => fa >= fb,
                        0x6B => ia >= ib,
                        0x6C => a >= b,
                        0x6D => fa > fb,
                        0x6E => ia > ib,
                        0x6F => a > b,
                        0x70 => fa <= fb,
                        0x71 => ia <= ib,
                        0x72 => a <= b,
                        0x73 => fa < fb,
                        0x74 => ia < ib,
                        0x75 => a < b,
                        0x76 => fa != fb,
                        _ => a != b,
                    };
                    let target = self.operand(&f)?;
                    self.sp -= 8;
                    f.pc = if taken { target } else { f.pc + 4 };
                }
                0x79 => {
                    f.pc = self.top(1)?;
                    self.sp -= 4;
                }
                0x7C => self.sp -= 4,
                // ARGB, the unused opcodes, BCOM and BXOR: no effect (the interpreter's default case)
                _ => {}
            }
        }
    }

    /// The conversions 0x22-0x31 (operand: the source size in bytes), traced case by case: to a narrow type they
    /// change nothing (only the low bytes count); widening sign- or zero-extends in place.
    fn convert(&mut self, op: u8, size: u8) -> Result<()> {
        let v = self.top(1)?;
        let nv = match (op, size) {
            (0x22, 4) => ftol(f32::from_bits(v)),
            (0x23, 4) => (v as i32 as f32).to_bits(),
            (0x25 | 0x28, 1) => (v & 0xffff_0000) | (v as u8 as i8 as i16 as u16 as u32),
            (0x2C | 0x2F, 1) => (v & 0xffff_0000) | (v & 0xff),
            (0x26 | 0x29, 2) => v as u16 as i16 as i32 as u32,
            (0x26 | 0x29, 1) => v as u8 as i8 as i32 as u32,
            (0x2D | 0x30, 2) => v & 0xffff,
            (0x2D | 0x30, 1) => v & 0xff,
            _ => v,
        };
        self.set_top(1, nv)
    }

    fn arith(&mut self, op: u8) -> Result<()> {
        let (a, b) = (self.top(2)?, self.top(1)?);
        let (fa, fb) = (f32::from_bits(a), f32::from_bits(b));
        let r = match op {
            0x4B => (fa + fb).to_bits(),
            0x4C..=0x4E => a.wrapping_add(b),
            0x4F => (fa - fb).to_bits(),
            0x50..=0x52 => a.wrapping_sub(b),
            0x53 | 0x54 => a << (b & 31),
            // MODI is an unsigned remainder in the interpreter (traced)
            0x55 | 0x56 => a.checked_rem(b).ok_or(Error::DivideByZero)?,
            // RSHI is a logical shift (traced)
            0x57 | 0x58 => a >> (b & 31),
            0x59 | 0x5A => a & b,
            0x5D | 0x5E => a | b,
            0x61 => (fa / fb).to_bits(),
            0x62 => (a as i32).checked_div(b as i32).ok_or(Error::DivideByZero)? as u32,
            0x63 => a.checked_div(b).ok_or(Error::DivideByZero)?,
            0x64 => (fa * fb).to_bits(),
            0x65 | 0x66 => a.wrapping_mul(b),
            // BCOMI/U, BXORI/U: no effect
            _ => return Ok(()),
        };
        self.set_top(2, r)?;
        self.sp -= 4;
        Ok(())
    }

    /// `CALLF/I/U/P/V` and `CALLB`: pop the target; an import operand calls a native (or, when the import names a
    /// script function, that function with its result pushed); an address runs the script function there.
    fn call(&mut self, host: &mut dyn Host, f: &mut Frame, op: u8) -> Result<()> {
        let target = self.top(1)?;
        self.sp -= 4;
        let s = &self.scripts[f.script];
        let link = import_index(target, s.imports.len()).map(|i| (i, s.imports[i].2));
        match link {
            Some((_, Link::Script { addr })) => {
                let callee = self.script_of(addr).ok_or(Error::BadTarget(addr))?;
                self.run(host, callee, addr, f.arg_block)?;
                self.sp += 4;
            }
            Some((_, Link::Native(hash))) => {
                if op != 0x7A {
                    let n = f.n_args as usize;
                    let mut args = Vec::with_capacity(n);
                    for i in 0..n {
                        args.push(self.mem.u32(f.arg_block + 4 * i as u32)?);
                    }
                    let r = host.call(self, hash, &args, f.floats)?;
                    if op != 0x38 {
                        self.push(r)?;
                    }
                }
            }
            Some((i, Link::Unresolved)) => return Err(Error::Unresolved(s.imports[i].0.clone())),
            None => {
                let callee = self.script_of(target).ok_or(Error::BadTarget(target))?;
                self.run(host, callee, target, f.arg_block)?;
            }
        }
        f.arg_ptr = f.arg_block;
        f.n_args = 0;
        f.floats = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_operands_and_ftol() {
        // import number 3 -> index 1
        assert_eq!(import_index(0x0000_03ff, 4), Some(1));
        assert_eq!(import_index(0x0000_03ff, 1), None);
        assert_eq!(import_index(0x1000_0000, 4), None);
        assert_eq!(ftol(-2.7), (-2i32) as u32);
        assert_eq!(ftol(3e9), 0x8000_0000);
        assert_eq!(ftol(f32::NAN), 0x8000_0000);
    }

    /// No address in a loaded script's sections reads as an import, even with 2,000 imports: a function at a code
    /// offset ending in `0xff` once did, and its callers called a native instead.
    #[test]
    fn script_addresses_are_never_imports() {
        for idx in 0..240 {
            for k in 0..4 {
                for off in (0..0x0010_0000).step_by(0x100) {
                    let a = section_base(idx, k) + off + 0xff;
                    assert_eq!(import_index(a, 2000), None, "script {idx} section {k} at {a:#010x}");
                }
            }
        }
    }
}
