//! Front-end scripts (`.vso`, magic "JBOF"; port of `tools/vso_parse.py`): a relocatable object file holding
//! the bytecode of a FECK screen, its data (`_ResourceData`, `_LayerData`), literals and the natives it imports.
//! Loader `FUN_004deb30`, relocations `FUN_004dec00`, interpreter `FUN_004df7d0`; layouts in the Python tool.
//!
//! ```text
//! header 0x58: "JBOF" | u16 version 1 | u16 0 | u32 hash | code, data, lit (off, size) | u32 bss ptr 0 | bss_size
//!              | reloc, iref (off, size) | sym_off | imp_off | str (off, size) | u16 n_sym+n_imp, n_sym, n_imp,
//!              n_reloc, n_iref, flags 0
//! file order: header, symbols (16 B), imports (12 B), code, data, lit, strings, relocations (8 B), import refs (8 B)
//! ```

use crate::util::{bad, cstr_at, slice, u16_at, u32_at, u8_at, Result};

pub const MAGIC: &[u8; 4] = b"JBOF";
pub const HEADER_SIZE: usize = 0x58;

/// Section names by section number (relocation fix-up and target fields).
fn section_name(n: u32) -> Option<&'static str> {
    ["code", "data", "bss", "lit"].get(n as usize).copied()
}

/// Section of a symbol by `flags & 0xFF00`.
fn sym_section(fl: u32) -> Option<&'static str> {
    Some(match fl & 0xFF00 {
        0x400 => "code",
        0x800 => "data",
        0x1000 => "bss",
        0x2000 => "lit",
        _ => return None,
    })
}

const TYPES: [&str; 8] = ["F4", "I1", "I2", "I4", "U1", "U2", "U4", "P4"];
const CNST: [&str; 8] = ["CNSTF4", "CNSTI1", "CNSTI2", "CNSTI4", "CNSTU1", "CNSTU2", "CNSTU4", "CNSTP4"];
const ARG: [&str; 5] = ["ARGF", "ARGI", "ARGU", "ARGP", "ARGB"];
const ASGN: [&str; 8] = ["ASGNF4", "ASGNI1", "ASGNI2", "ASGNI4", "ASGNU1", "ASGNU2", "ASGNU4", "ASGNP4"];
const INDIR: [&str; 8] = ["INDIRF4", "INDIRI1", "INDIRI2", "INDIRI4", "INDIRU1", "INDIRU2", "INDIRU4", "INDIRP4"];
/// 0x22-0x31: the operand is the source size in bytes; only the first two are known by effect.
const CV: [&str; 16] = [
    "CVFI", "CVIF", "CV24", "CV25", "CV26", "CV27", "CV28", "CV29", "CV2A", "CV2B", "CV2C", "CV2D", "CV2E", "CV2F",
    "CV30", "CV31",
];
const CALL: [&str; 5] = ["CALLF", "CALLI", "CALLU", "CALLP", "CALLV"];
const RET: [&str; 5] = ["RETF", "RETI", "RETU", "RETP", "RETV"];
const ARITH: [&str; 28] = [
    "ADDF", "ADDI", "ADDP", "ADDU", "SUBF", "SUBI", "SUBP", "SUBU", "LSHI", "LSHU", "MODI", "MODU", "RSHI", "RSHU",
    "BANDI", "BANDU", "BCOMI", "BCOMU", "BORI", "BORU", "BXORI", "BXORU", "DIVF", "DIVI", "DIVU", "MULF", "MULI",
    "MULU",
];
/// Compare and branch to a (relocated) code offset when the comparison holds.
const CMP: [&str; 18] = [
    "EQF", "EQI", "EQU", "GEF", "GEI", "GEU", "GTF", "GTI", "GTU", "LEF", "LEI", "LEU", "LTF", "LTI", "LTU", "NEF",
    "NEI", "NEU",
];

/// `(mnemonic, operand bytes)` of an opcode (lcc's operator set and type suffixes), `None` if unknown.
pub fn opcode(op: u8) -> Option<(&'static str, usize)> {
    let i = op as usize;
    Some(match op {
        0x00 => ("ENTER", 4),
        0x01 => ("END", 0),
        0x02..=0x09 => (CNST[i - 2], (TYPES[i - 2].as_bytes()[1] - b'0') as usize),
        0x0A..=0x0E => (ARG[i - 0x0A], 0),
        0x0F..=0x16 => (ASGN[i - 0x0F], 0),
        0x18..=0x1F => (INDIR[i - 0x18], 0),
        0x20 => ("ASGNB", 3), // u8 0, u16 byte count
        0x22..=0x31 => (CV[i - 0x22], 1),
        0x32 => ("NEGF", 0),
        0x33 => ("NEGI", 0),
        0x34..=0x38 => (CALL[i - 0x34], 0),
        0x43..=0x47 => (RET[i - 0x43], 0),
        0x48 => ("ADDRG", 4),
        0x49 => ("ADDRF", 2),
        0x4A => ("ADDRL", 2),
        0x4B..=0x66 => (ARITH[i - 0x4B], 0),
        0x67..=0x78 => (CMP[i - 0x67], 4),
        0x79 => ("JUMPV", 0),
        0x7A => ("CALLB", 0),
        0x7C => ("POP", 0),
        _ => return None,
    })
}

/// Layer object tags and the number of u32 fields before their id/flags pair.
const OBJECTS: [(&[u8; 4], usize); 12] = [
    (b"NULL", 0),
    (b"BMNU", 1),
    (b"SPRT", 10),
    (b"TEXT", 4),
    (b"CMRA", 14),
    (b"MODL", 2),
    (b"PUPT", 5),
    (b"ASPT", 11),
    (b"PTCL", 0),
    (b"QDHC", 3),
    (b"SLDR", 4),
    (b"PMNU", 4),
];
/// Layer property tags and their word counts after the tag (`None`: depends on the data).
const PROPERTIES: [(&[u8; 4], Option<usize>); 14] = [
    (b"POS ", Some(3)),
    (b"POSX", Some(1)),
    (b"POSY", Some(1)),
    (b"POSZ", Some(1)),
    (b"HEAD", Some(1)),
    (b"OVNT", Some(2)),
    (b"SIZE", Some(2)),
    (b"TEXt", Some(1)),
    (b"BMNE", Some(2)),
    (b"SLDE", Some(2)),
    (b"COL ", None),
    (b"UDTA", None),
    (b"BMNP", None),
    (b"ASPD", None),
];

/// A tag as the file stores it: four ASCII characters as a little-endian u32 ("NULL" is the bytes LLUN).
fn tag(t: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*t)
}

fn tag_name(t: &[u8; 4]) -> &str {
    std::str::from_utf8(t).unwrap()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub hash: u32,
    pub section: &'static str,
    /// Offset in its section.
    pub value: u32,
    pub flags: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    pub name: String,
    pub hash: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reloc {
    /// Section the fix-up is in.
    pub section: &'static str,
    pub offset: u32,
    pub target: &'static str,
    pub addend: i32,
}

/// An ADDRG operand holding an import number (`FUN_004df380` resolves it when the instruction runs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportRef {
    pub offset: u32,
    pub import: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    /// The code symbol's name, or `sub_<offset>` for a static function.
    pub name: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Instruction<'a> {
    pub pc: usize,
    pub op: &'static str,
    pub arg: &'a [u8],
}

/// A `_LayerData` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Layer {
    Object {
        kind: &'static str,
        fields: Vec<u32>,
        id: u32,
        flags: u32,
        children: Vec<Layer>,
    },
    /// The property name has its trailing space removed (`POS`, `COL`).
    Property {
        name: &'static str,
        words: Vec<u32>,
    },
}

/// A `_ResourceData` entry; `kind` 0 texture, 1 font, 2 string table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    pub id: u32,
    pub group: String,
    pub name: String,
    pub kind: u32,
    /// Word 6: a table the record takes its group, name and type from as it registers (`0x4ea420`): the title's
    /// logo per language, the fonts per font set.
    pub choices: Option<Choices>,
}

/// A resource's choices: the hash of a function of the same script, whose result (run by `0x4df700`,
/// `CFECK_VM::ExecuteScript`) indexes them, and each `{group, name, type, word}` (16 bytes) as `(group, name,
/// type)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choices {
    pub function: u32,
    pub entries: Vec<(String, String, u32)>,
}

#[derive(Debug, Clone)]
pub struct Vso<'a> {
    pub version: u16,
    pub hash: u32,
    pub flags: u16,
    pub bss_size: u32,
    pub code: &'a [u8],
    pub data: &'a [u8],
    pub lit: &'a [u8],
    pub symbols: Vec<Symbol>,
    pub imports: Vec<Import>,
    pub relocs: Vec<Reloc>,
    pub irefs: Vec<ImportRef>,
}

impl<'a> Vso<'a> {
    pub fn parse(d: &'a [u8]) -> Result<Vso<'a>> {
        if d.get(..4) != Some(MAGIC) {
            return bad("not a JBOF/.vso file");
        }
        slice(d, 0, HEADER_SIZE)?;
        let w = |i: usize| u32_at(d, 8 + 4 * i).map(|v| v as usize);
        let h = |i: usize| u16_at(d, 0x4C + 2 * i).map(|v| v as usize);
        let (version, zero, hash) = (u16_at(d, 4)?, u16_at(d, 6)?, u32_at(d, 8)?);
        let (code_off, code_size, data_off, data_size, lit_off, lit_size) = (w(1)?, w(2)?, w(3)?, w(4)?, w(5)?, w(6)?);
        let (bss_ptr, bss_size, rel_off, rel_size, iref_off, iref_size) = (w(7)?, w(8)?, w(9)?, w(10)?, w(11)?, w(12)?);
        let (sym_off, imp_off, str_off, str_size) = (w(13)?, w(14)?, w(15)?, w(16)?);
        let (n_all, n_sym, n_imp, n_rel, n_iref) = (h(0)?, h(1)?, h(2)?, h(3)?, h(4)?);
        let flags = u16_at(d, 0x56)?;
        if version != 1 || zero != 0 || bss_ptr != 0 || flags != 0 {
            return bad("unexpected header values");
        }
        if n_all != n_sym + n_imp || rel_size != 8 * n_rel || iref_size != 8 * n_iref {
            return bad("header counts disagree with section sizes");
        }
        // the sections tile the file in this order; all but the string table start 4-byte aligned
        let layout = [
            (0, HEADER_SIZE),
            (sym_off, 16 * n_sym),
            (imp_off, 12 * n_imp),
            (code_off, code_size),
            (data_off, data_size),
            (lit_off, lit_size),
            (str_off, str_size),
            (rel_off, rel_size),
            (iref_off, iref_size),
        ];
        let mut pos = 0;
        for (off, size) in layout {
            let gap = d.get(pos.min(d.len())..off.min(d.len())).unwrap_or(&[]);
            if (off != pos && off != (pos + 3) & !3) || gap.iter().any(|&b| b != 0) {
                return bad(format!("section at {off:#x} does not follow the previous one (expected {pos:#x})"));
            }
            pos = off + size;
        }
        if pos != d.len() {
            return bad(format!("file has {} bytes after the last section", d.len() as i64 - pos as i64));
        }
        let (code, data, lit) =
            (slice(d, code_off, code_size)?, slice(d, data_off, data_size)?, slice(d, lit_off, lit_size)?);
        let strings = slice(d, str_off, str_size)?;

        let mut symbols = Vec::with_capacity(n_sym);
        for i in 0..n_sym {
            let f = |k: usize| u32_at(d, sym_off + 16 * i + 4 * k);
            let (name_off, hash, value, fl) = (f(0)?, f(1)?, f(2)?, f(3)?);
            let section = match sym_section(fl) {
                Some(s) if (name_off as usize) < str_size => s,
                _ => return bad(format!("symbol {i}: bad flags {fl:#x} or name")),
            };
            let size = match section {
                "code" => code_size,
                "data" => data_size,
                "lit" => lit_size,
                _ => bss_size,
            };
            if value as usize >= size && !(value == 0 && size == 0) {
                return bad(format!("symbol {i} points outside its section"));
            }
            symbols.push(Symbol { name: cstr_at(strings, name_off as usize)?, hash, section, value, flags: fl });
        }
        let mut imports = Vec::with_capacity(n_imp);
        for i in 0..n_imp {
            let f = |k: usize| u32_at(d, imp_off + 12 * i + 4 * k);
            let (name_off, hash, resolved) = (f(0)?, f(1)?, f(2)?);
            if name_off >> 24 != 0 || resolved != 0 || name_off as usize >= str_size {
                return bad(format!("import {i}: unexpected runtime fields"));
            }
            imports.push(Import { name: cstr_at(strings, name_off as usize)?, hash });
        }

        let section_data = |s: u8| match s {
            0 => Some(code),
            1 => Some(data),
            3 => Some(lit),
            _ => None,
        };
        let mut relocs = Vec::with_capacity(n_rel);
        for i in 0..n_rel {
            let r = rel_off + 8 * i;
            let (off, size, sec, pad) = (u32_at(d, r)?, u8_at(d, r + 4)?, u8_at(d, r + 5)?, u16_at(d, r + 6)?);
            let Some(bytes) = section_data(sec).filter(|b| size == 4 && pad == 0 && off as usize + 4 <= b.len()) else {
                return bad(format!("reloc {i} malformed"));
            };
            let word = u32_at(bytes, off as usize)?;
            let Some(target) = section_name(word & 0xFF) else {
                return bad(format!("reloc {i}: bad target section {}", word & 0xFF));
            };
            let addend = (word as i32) >> 8; // 24-bit signed
            relocs.push(Reloc { section: section_name(sec as u32).unwrap(), offset: off, target, addend });
        }
        let mut irefs = Vec::with_capacity(n_iref);
        for i in 0..n_iref {
            let (off, size) = (u32_at(d, iref_off + 8 * i)?, u32_at(d, iref_off + 8 * i + 4)?);
            let word = if size == 4 && off as usize + 4 <= code_size { u32_at(code, off as usize)? } else { 0 };
            let n = (word >> 8) as usize;
            if word & 0xFF != 0xFF || n == 0 || (n - 1) >> 1 >= n_imp {
                return bad(format!("import ref {i} does not hold an import number"));
            }
            irefs.push(ImportRef { offset: off, import: (n - 1) >> 1 });
        }
        let v =
            Vso { version, hash, flags, bss_size: bss_size as u32, code, data, lit, symbols, imports, relocs, irefs };
        v.check_code()?;
        Ok(v)
    }

    /// The functions in address order. Exported functions are named by their symbol; a few static functions
    /// have no symbol and are named `sub_<offset>`.
    pub fn functions(&self) -> Result<Vec<Function>> {
        let mut named = std::collections::HashMap::new();
        for s in self.symbols.iter().filter(|s| s.section == "code") {
            named.insert(s.value as usize, &s.name);
        }
        let (mut out, mut start, mut pc) = (Vec::new(), 0, 0);
        while pc < self.code.len() {
            let op = self.code[pc];
            let Some((_, n)) = opcode(op) else {
                return bad(format!("unknown opcode {op:#04x} at {pc:#x}"));
            };
            pc += 1 + n;
            if op == 0x01 {
                // END closes the function that began at start
                let name = named.get(&start).map_or_else(|| format!("sub_{start:x}"), |n| n.to_string());
                out.push(Function { name, start, end: pc });
                start = pc;
            }
        }
        if start != self.code.len() {
            return bad("code ends inside a function");
        }
        if named.keys().any(|k| !out.iter().any(|f| f.start == *k)) {
            return bad("a code symbol does not point at a function start");
        }
        Ok(out)
    }

    /// The instructions of `code[start..end]`.
    pub fn disassemble(&self, start: usize, end: usize) -> Result<Vec<Instruction<'a>>> {
        let (code, mut pc, mut out) = (self.code, start, Vec::new());
        while pc < end {
            let op = u8_at(code, pc)?;
            let Some((name, n)) = opcode(op) else {
                return bad(format!("unknown opcode {op:#04x} at {pc:#x}"));
            };
            let arg = &code[(pc + 1).min(code.len())..(pc + 1 + n).min(code.len())];
            out.push(Instruction { pc, op: name, arg });
            pc += 1 + n;
        }
        if pc != end {
            return bad(format!("function at {start:#x} runs past its end"));
        }
        Ok(out)
    }

    fn check_code(&self) -> Result<()> {
        let fs = self.functions()?;
        let mut operand4 = std::collections::HashSet::new();
        let mut addrg = Vec::new();
        for f in &fs {
            let ins = self.disassemble(f.start, f.end)?;
            let inner = ins.get(1..ins.len().saturating_sub(1)).unwrap_or(&[]);
            if ins[0].op != "ENTER"
                || ins[ins.len() - 1].op != "END"
                || inner.iter().any(|i| matches!(i.op, "ENTER" | "END"))
            {
                return bad(format!("function at {:#x} is not ENTER ... END", f.start));
            }
            operand4.extend(ins.iter().filter(|i| i.arg.len() == 4 && i.op != "ENTER").map(|i| i.pc + 1));
            addrg.extend(ins.iter().filter(|i| i.op == "ADDRG").map(|i| i.pc + 1));
        }
        // every code fix-up and import reference sits on a 4-byte operand, and together they cover all ADDRG operands
        let fixups: Vec<usize> = self
            .relocs
            .iter()
            .filter(|r| r.section == "code")
            .map(|r| r.offset as usize)
            .chain(self.irefs.iter().map(|r| r.offset as usize))
            .collect();
        let unique: std::collections::HashSet<usize> = fixups.iter().copied().collect();
        if !unique.is_subset(&operand4) || unique.len() != fixups.len() {
            return bad("a code fix-up is not on an instruction operand");
        }
        if !addrg.iter().all(|a| unique.contains(a)) {
            return bad("an ADDRG operand has no fix-up");
        }
        Ok(())
    }

    /// Human-readable disassembly with symbol and import names (byte-identical to the Python tool's).
    pub fn listing(&self) -> Result<String> {
        use std::collections::HashMap;
        let imp: HashMap<usize, &str> =
            self.irefs.iter().map(|r| (r.offset as usize, self.imports[r.import].name.as_str())).collect();
        let rel: HashMap<usize, &Reloc> =
            self.relocs.iter().filter(|r| r.section == "code").map(|r| (r.offset as usize, r)).collect();
        let names: HashMap<(&str, i64), &str> =
            self.symbols.iter().map(|s| ((s.section, s.value as i64), s.name.as_str())).collect();
        let mut out = Vec::new();
        for f in self.functions()? {
            out.push(format!("\n{}:  ; {:#x}", f.name, f.start));
            for i in self.disassemble(f.start, f.end)? {
                let a = i.arg;
                let text = if let Some(n) = imp.get(&(i.pc + 1)) {
                    n.to_string()
                } else if let Some(r) = rel.get(&(i.pc + 1)) {
                    names.get(&(r.target, r.addend as i64)).map_or_else(
                        || {
                            format!(
                                "{}+0x{}{:x}",
                                r.target,
                                if r.addend < 0 { "-" } else { "" },
                                r.addend.unsigned_abs()
                            )
                        },
                        |n| n.to_string(),
                    )
                } else if i.op == "ENTER" {
                    format!("locals={} args={}", u16_at(a, 0)?, u16_at(a, 2)?)
                } else if i.op == "ASGNB" {
                    u16_at(a, 1)?.to_string()
                } else if i.op == "CNSTF4" {
                    py_repr(f32::from_le_bytes(slice(a, 0, 4)?.try_into().unwrap()) as f64)
                } else if !a.is_empty() {
                    let v = a.iter().rev().fold(0u64, |v, &b| v << 8 | b as u64);
                    let bits = 8 * a.len() as u32;
                    if i.op.starts_with("CNSTI") {
                        ((v << (64 - bits)) as i64 >> (64 - bits)).to_string()
                    } else {
                        v.to_string()
                    }
                } else {
                    String::new()
                };
                out.push(format!("  {:05x}  {:<7} {text}", i.pc, i.op));
            }
        }
        Ok(out.join("\n"))
    }

    fn symbol(&self, name: &str) -> Option<&Symbol> {
        self.symbols.iter().find(|s| s.name == name)
    }

    /// A data word, 0 past the end (as the Python tool reads).
    fn word(&self, off: usize) -> u32 {
        u32_at(self.data, off).unwrap_or(0)
    }

    fn property_words(&self, name: &str, n: Option<usize>, off: usize) -> Result<usize> {
        if let Some(n) = n {
            return Ok(n);
        }
        let first = self.word(off);
        Ok(match name {
            "COL " => {
                if first == 4 {
                    5 // 4 + all four channels
                } else {
                    2 // channel 0-3 + value
                }
            }
            "UDTA" => 2 + first as usize, // count, key, count values
            "BMNP" => {
                if first <= 1 {
                    2
                } else {
                    1
                }
            }
            _ => {
                // ASPD: frame data up to and including the "ASPd" end tag
                let mut n = 0;
                while self.word(off + 4 * n) != tag(b"ASPd") {
                    n += 1;
                    if off + 4 * n >= self.data.len() {
                        return bad("ASPD without its end tag");
                    }
                }
                n + 1
            }
        })
    }

    fn words(&self, off: usize, n: usize) -> Result<Vec<u32>> {
        (0..n).map(|i| u32_at(self.data, off + 4 * i)).collect()
    }

    /// Returns (records, offset after the list, terminator word).
    fn children(&self, mut off: usize) -> Result<(Vec<Layer>, usize, u32)> {
        let mut out = Vec::new();
        if self.word(off) == 1 {
            return Ok((out, off + 4, 1));
        }
        loop {
            let t = self.word(off);
            if let Some((name, n)) = PROPERTIES.iter().find(|p| tag(p.0) == t) {
                let n = self.property_words(tag_name(name), *n, off + 4)?;
                out.push(Layer::Property { name: tag_name(name).trim(), words: self.words(off + 4, n)? });
                off += 4 + 4 * n;
            } else if OBJECTS.iter().any(|o| tag(o.0) == t) {
                let (node, next) = self.object(off)?;
                out.push(node);
                off = next;
            } else {
                return bad(format!("unknown layer tag {t:#010x} at data+{off:#x}"));
            }
            let end = self.word(off);
            if end <= 1 {
                return Ok((out, off + 4, end));
            }
        }
    }

    fn object(&self, off: usize) -> Result<(Layer, usize)> {
        let t = self.word(off);
        let Some((name, n)) = OBJECTS.iter().find(|o| tag(o.0) == t) else {
            return bad(format!("layer record {t:#010x} at data+{off:#x} is not an object"));
        };
        let fields = self.words(off + 4, n + 2)?;
        let (children, off, _) = self.children(off + 4 + 4 * (n + 2))?;
        let node = Layer::Object {
            kind: tag_name(name),
            fields: fields[..*n].to_vec(),
            id: fields[*n],
            flags: fields[n + 1],
            children,
        };
        Ok((node, off))
    }

    /// The `_LayerData` tree as the loader builds it, and a note on any shipped quirk (`None` if clean).
    pub fn layers(&self) -> Result<(Vec<Layer>, Option<&'static str>)> {
        let Some(sym) = self.symbol("_LayerData") else {
            return Ok((vec![], None));
        };
        let (mut off, mut roots) = (sym.value as usize, Vec::new());
        while self.word(off) != 0 {
            let (node, next) = self.object(off)?;
            off = next;
            roots.push(node);
            if self.word(off - 4) == 0 {
                return Ok((roots, Some("last object closed by the final 0; the loader reads on past it")));
            }
            if self.word(off) == 1 {
                return Ok((roots, Some("stray 1 after the last object; the loader stops with an error here")));
            }
        }
        Ok((roots, None))
    }

    /// The `_ResourceData` entries (28 bytes each, until an all-zero one).
    pub fn resources(&self) -> Result<Vec<Resource>> {
        let Some(sym) = self.symbol("_ResourceData") else {
            return Ok(vec![]);
        };
        let lit_ptr: std::collections::HashMap<usize, i32> = self
            .relocs
            .iter()
            .filter(|r| r.section == "data" && r.target == "lit")
            .map(|r| (r.offset as usize, r.addend))
            .collect();
        let data_ptr: std::collections::HashMap<usize, i32> = self
            .relocs
            .iter()
            .filter(|r| r.section == "data" && r.target == "data")
            .map(|r| (r.offset as usize, r.addend))
            .collect();
        let s = |at: usize| match lit_ptr.get(&at) {
            // a negative start counts from the end, as Python's bytes.index does
            Some(&a) => {
                cstr_at(self.lit, if a < 0 { (self.lit.len() as i64 + a as i64).max(0) as usize } else { a as usize })
            }
            None => Ok(String::new()),
        };
        let (mut out, mut off) = (Vec::new(), sym.value as usize);
        let len = self.data.len();
        while self.data[off.min(len)..(off + 28).min(len)].iter().any(|&b| b != 0) {
            let (id, kind) = (u32_at(self.data, off)?, u32_at(self.data, off + 12)?);
            // the entries run while they name a group (the original indexes them without a bound)
            let choices = match data_ptr.get(&(off + 24)) {
                Some(&t) if t >= 0 => {
                    let t = t as usize;
                    let (function, mut entries, mut e) = (u32_at(self.data, t)?, Vec::new(), t + 4);
                    while e + 16 <= len && lit_ptr.contains_key(&e) {
                        entries.push((s(e)?, s(e + 4)?, u32_at(self.data, e + 8)?));
                        e += 16;
                    }
                    Some(Choices { function, entries })
                }
                _ => None,
            };
            out.push(Resource { id, group: s(off + 4)?, name: s(off + 8)?, kind, choices });
            off += 28;
        }
        Ok(out)
    }
}

/// Python's `repr(float)`: shortest round-trip digits, scientific below 1e-4 and from 1e16.
pub fn py_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf" } else { "-inf" }.into();
    }
    let s = format!("{:e}", x.abs());
    let (m, e) = s.split_once('e').unwrap();
    let (digits, e) = (m.replace('.', ""), e.parse::<i32>().unwrap());
    let sign = if x.is_sign_negative() { "-" } else { "" };
    let body = if !(-4..16).contains(&e) {
        let (a, b) = digits.split_at(1);
        let dot = if b.is_empty() { "" } else { "." };
        format!("{a}{dot}{b}e{}{:02}", if e < 0 { '-' } else { '+' }, e.abs())
    } else if e < 0 {
        format!("0.{}{digits}", "0".repeat((-e - 1) as usize))
    } else {
        let p = e as usize + 1;
        if digits.len() > p {
            format!("{}.{}", &digits[..p], &digits[p..])
        } else {
            format!("{digits}{}.0", "0".repeat(p - digits.len()))
        }
    };
    format!("{sign}{body}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn le(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    /// One exported function "Main" at 0 (plus an optional second one), one import "Native", one data symbol.
    fn build(code: &[u8], relocs: &[(u32, u8)], irefs: &[u32], data: &[u8], extra_fn: Option<u32>) -> Vec<u8> {
        let strings = b"Main\0Native\0gData\0Second\0";
        let mut syms = vec![[0, 0x1111, 0, 0x440], [12, 0x3333, 0, 0x804]];
        if let Some(v) = extra_fn {
            syms.push([18, 0x4444, v, 0x440]);
        }
        let sym_b = le(&syms.concat());
        let imp_b = le(&[5, 0x2222, 0]);
        let rel_b: Vec<u8> = relocs.iter().flat_map(|&(off, sec)| [le(&[off]), vec![4, sec, 0, 0]].concat()).collect();
        let iref_b = le(&irefs.iter().flat_map(|&o| [o, 4]).collect::<Vec<_>>());
        let sym_off = HEADER_SIZE;
        let imp_off = sym_off + sym_b.len();
        let code_off = imp_off + imp_b.len();
        let data_off = (code_off + code.len() + 3) & !3;
        let str_off = data_off + data.len();
        let rel_off = (str_off + strings.len() + 3) & !3;
        let iref_off = rel_off + rel_b.len();
        let (ns, nr, ni) = (syms.len() as u32, relocs.len() as u32, irefs.len() as u32);
        let mut d = b"JBOF\x01\0\0\0".to_vec();
        d.extend(le(&[
            0xABCD,
            code_off as u32,
            code.len() as u32,
            data_off as u32,
            data.len() as u32,
            str_off as u32, // the lit section is empty
            0,
            0,
            0,
            rel_off as u32,
            rel_b.len() as u32,
            iref_off as u32,
            iref_b.len() as u32,
            sym_off as u32,
            imp_off as u32,
            str_off as u32,
            strings.len() as u32,
            (ns + 1) | ns << 16, // n_sym + n_imp, n_sym
            1 | nr << 16,        // n_imp, n_reloc
            ni,                  // n_iref, flags
        ]));
        d.extend(sym_b);
        d.extend(imp_b);
        d.extend_from_slice(code);
        d.resize(data_off, 0);
        d.extend_from_slice(data);
        d.extend_from_slice(strings);
        d.resize(rel_off, 0);
        d.extend(rel_b);
        d.extend(iref_b);
        d
    }

    /// ENTER 0,0 | ADDRG <import 0> | CALLV | ADDRG <code+0> | POP | CNSTI4 0 | CNSTI4 1 | EQI <code+0> | END
    fn code() -> Vec<u8> {
        let mut c = vec![0x00, 0, 0, 0, 0, 0x48];
        c.extend(le(&[0xFF | 1 << 8]));
        c.extend([0x38, 0x48, 0, 0, 0, 0, 0x7C, 0x05, 0, 0, 0, 0, 0x05, 1, 0, 0, 0, 0x68, 0, 0, 0, 0, 0x01]);
        c
    }

    const RELOCS: [(u32, u8); 2] = [(12, 0), (28, 0)];

    #[test]
    fn parse_and_disassemble() {
        let d = build(&code(), &RELOCS, &[6], &[0; 4], None);
        let v = Vso::parse(&d).unwrap();
        assert_eq!(v.functions().unwrap()[0].name, "Main");
        assert_eq!(v.imports[0].name, "Native");
        assert_eq!(v.irefs, [ImportRef { offset: 6, import: 0 }]);
        let ops: Vec<_> = v.disassemble(0, code().len()).unwrap().iter().map(|i| i.op).collect();
        assert_eq!(ops, ["ENTER", "ADDRG", "CALLV", "ADDRG", "POP", "CNSTI4", "CNSTI4", "EQI", "END"]);
        let listing = v.listing().unwrap();
        assert!(listing.contains("ADDRG   Native"));
        assert!(listing.contains("ADDRG   Main"));
        assert!(listing.starts_with("\nMain:  ; 0x0\n  00000  ENTER   locals=0 args=0\n"));
    }

    #[test]
    fn static_function_gets_a_name() {
        let mut c = code();
        c.extend([0x00, 4, 0, 0, 0, 0x01]);
        let d = build(&c, &RELOCS, &[6], &[0; 4], None);
        let names: Vec<_> = Vso::parse(&d).unwrap().functions().unwrap().into_iter().map(|f| f.name).collect();
        assert_eq!(names, ["Main".to_string(), format!("sub_{:x}", code().len())]);
    }

    #[test]
    fn rejects_addrg_without_fixup() {
        assert!(Vso::parse(&build(&code(), &[(28, 0)], &[6], &[0; 4], None)).is_err());
    }

    #[test]
    fn rejects_symbol_inside_function() {
        assert!(Vso::parse(&build(&code(), &RELOCS, &[6], &[0; 4], Some(5))).is_err());
    }

    #[test]
    fn rejects_bad_import_reference() {
        let mut c = code();
        c[7] = 9; // import number 9 -> import index 4 of 1
        assert!(Vso::parse(&build(&c, &RELOCS, &[6], &[0; 4], None)).is_err());
    }

    fn with_layers(words: &[u32]) -> (Vec<Layer>, Option<&'static str>) {
        let d = build(&code(), &RELOCS, &[6], &[0; 4], None);
        let data = le(words);
        let mut v = Vso::parse(&d).unwrap();
        v.data = &data;
        v.symbols.push(Symbol { name: "_LayerData".into(), hash: 0, section: "data", value: 0, flags: 0x810 });
        v.layers().unwrap()
    }

    fn prop(name: &'static str, words: &[u32]) -> Layer {
        Layer::Property { name, words: words.to_vec() }
    }

    #[test]
    fn layer_tree() {
        let words = [
            tag(b"NULL"),
            0x11,
            0xA0,
            tag(b"OVNT"),
            1,
            0x1234,
            tag(b"SPRT"),
            7,
            0x3C,
            2,
            1,
            0,
            0,
            0,
            0,
            0,
            0,
            0x22,
            4,
            tag(b"POS "),
            0x10000,
            0x20000,
            0,
            1,
            tag(b"COL "),
            4,
            1,
            2,
            3,
            4,
            1,
            0,
        ];
        let (roots, note) = with_layers(&words);
        assert_eq!(note, None);
        let sprite = Layer::Object {
            kind: "SPRT",
            fields: vec![7, 0x3C, 2, 1, 0, 0, 0, 0, 0, 0],
            id: 0x22,
            flags: 4,
            children: vec![prop("POS", &[0x10000, 0x20000, 0])],
        };
        let children = vec![prop("OVNT", &[1, 0x1234]), sprite, prop("COL", &[4, 1, 2, 3, 4])];
        assert_eq!(roots, [Layer::Object { kind: "NULL", fields: vec![], id: 0x11, flags: 0xA0, children }]);
    }

    #[test]
    fn layer_stray_end_is_reported() {
        let (roots, note) = with_layers(&[tag(b"NULL"), 1, 0, 1, 1, 0]);
        assert_eq!(roots.len(), 1);
        assert!(note.unwrap().contains("stray"));
    }

    #[test]
    fn float_repr_matches_python() {
        for (x, s) in [
            (1.0, "1.0"),
            (-0.0, "-0.0"),
            (0.1, "0.1"),
            (1e-5, "1e-05"),
            (1.5e-5, "1.5e-05"),
            (0.0001, "0.0001"),
            (1e16, "1e+16"),
            (1234567890123456.0, "1234567890123456.0"),
            (f32::MAX as f64, "3.4028234663852886e+38"),
            (0.1f32 as f64, "0.10000000149011612"),
        ] {
            assert_eq!(py_repr(x), s);
        }
    }
}
