//! FECK, the original front end's script machine: the `.vso` screen scripts run here unchanged, calling into the
//! engine through natives the host provides (`docs/feck_natives.md`). This crate is the VM only; the layer objects,
//! screens and the native set build on it.

pub mod draw;
pub mod frontend;
pub mod keys;
pub mod layer;
pub mod layout;
pub mod memcard;
pub mod memory;
pub mod pad;
pub mod particles;
pub mod place;
pub mod puppet;
pub mod screens;
pub mod spline;
pub mod text;
pub mod vm;
pub mod wipe;

pub use frontend::FrontEnd;
pub use memory::{Fault, Memory};
pub use vm::{Error, Export, Host, Result, Script, Vm};

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn le(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    /// A `.vso` with functions `Main` (hash 0x1111, at 0) and `Second` (0x4444, at `second`), data symbol `gData`
    /// (0x3333) and one import (hash `import`). `relocs` are `(code offset, 0)` fix-ups; `irefs` code offsets.
    pub(crate) fn build(code: &[u8], relocs: &[u32], irefs: &[u32], data: &[u8], second: u32, import: u32) -> Vec<u8> {
        let strings = b"Main\0Native\0gData\0Second\0";
        let syms = [[0, 0x1111, 0, 0x440], [12, 0x3333, 0, 0x804], [18, 0x4444, second, 0x440]];
        let sym_b = le(&syms.concat());
        let imp_b = le(&[5, import, 0]);
        let rel_b: Vec<u8> = relocs.iter().flat_map(|&off| [le(&[off]), vec![4, 0, 0, 0]].concat()).collect();
        let iref_b = le(&irefs.iter().flat_map(|&o| [o, 4]).collect::<Vec<_>>());
        let sym_off = 0x58;
        let imp_off = sym_off + sym_b.len();
        let code_off = imp_off + imp_b.len();
        let data_off = (code_off + code.len() + 3) & !3;
        let str_off = data_off + data.len();
        let rel_off = (str_off + strings.len() + 3) & !3;
        let iref_off = rel_off + rel_b.len();
        let (nr, ni) = (relocs.len() as u32, irefs.len() as u32);
        let mut d = b"JBOF\x01\0\0\0".to_vec();
        d.extend(le(&[
            0xABCD,
            code_off as u32,
            code.len() as u32,
            data_off as u32,
            data.len() as u32,
            str_off as u32,
            0,
            0,
            16,
            rel_off as u32,
            rel_b.len() as u32,
            iref_off as u32,
            iref_b.len() as u32,
            sym_off as u32,
            imp_off as u32,
            str_off as u32,
            strings.len() as u32,
            4 | 3 << 16,
            1 | nr << 16,
            ni,
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

    /// A script that does nothing (`Main` is `ENTER 0, 0; END`), with object hash `hash` (header `0x08`).
    pub(crate) fn empty_script(hash: u32) -> Vec<u8> {
        let mut d = build(&[0x00, 0, 0, 0, 0, 0x01], &[], &[], &[0; 4], 0, 0x2222);
        d[8..12].copy_from_slice(&hash.to_le_bytes());
        d
    }

    /// Native 0x2222 multiplies its argument by 10 and records its calls.
    #[derive(Default)]
    struct Times10 {
        calls: Vec<(Vec<u32>, u32)>,
    }

    impl Host for Times10 {
        fn has_native(&self, hash: u32) -> bool {
            hash == 0x2222
        }
        fn call(&mut self, _: &mut Vm, _: u32, args: &[u32], floats: u32) -> Result<u32> {
            self.calls.push((args.to_vec(), floats));
            Ok(args[0].wrapping_mul(10))
        }
    }

    #[test]
    fn arithmetic_locals_and_a_native() {
        // local0 = (2 + 3) * 4; return Native(local0) + 1
        let mut c = vec![0x00, 8, 0, 4, 0, 0x4A, 0, 0];
        c.extend([0x05, 2, 0, 0, 0, 0x05, 3, 0, 0, 0, 0x4C, 0x05, 4, 0, 0, 0, 0x65, 0x12]);
        c.extend([0x4A, 0, 0, 0x1B, 0x0B, 0x48, 0xFF, 1, 0, 0, 0x35, 0x05, 1, 0, 0, 0, 0x4C, 0x44, 0x01]);
        let iref = c.iter().position(|&b| b == 0xFF).unwrap() as u32;
        let (mut vm, mut host) = (Vm::new(), Times10::default());
        let s = vm.load("t", &build(&c, &[], &[iref], &[0; 4], 0, 0x2222)).unwrap();
        vm.link(&host);
        assert!(vm.unresolved().is_empty());
        assert_eq!(vm.execute(&mut host, s, 0x1111, &[]).unwrap(), 201);
        assert_eq!(host.calls, [(vec![20], 0)]);
        assert!(matches!(vm.execute(&mut host, s, 0x9999, &[]), Err(Error::NoFunction(0x9999))));
    }

    #[test]
    fn branches_narrow_slots_and_conversions() {
        // ENTER 4,0; i = 10; sum in gData: loop { gData += i; i -= 1; if i > 0 goto loop }
        // then CNSTI1 0xff, CV 0x26 (from 1 byte) -> -1; CVIF -> -1.0; CVFI -> -1; RETI
        let mut c = vec![0x00, 4, 0, 0, 0];
        c.extend([0x4A, 0, 0, 0x05, 10, 0, 0, 0, 0x12]); // 5: i = 10
        let lp = c.len() as u32;
        c.extend([0x48, 1, 0, 0, 0, 0x48, 1, 0, 0, 0, 0x1B, 0x4A, 0, 0, 0x1B, 0x4C, 0x12]); // gData += i
        c.extend([0x4A, 0, 0, 0x4A, 0, 0, 0x1B, 0x05, 1, 0, 0, 0, 0x50, 0x12]); // i -= 1
        let br = c.len() as u32;
        c.extend([0x4A, 0, 0, 0x1B, 0x05, 0, 0, 0, 0, 0x6E, 0, lp as u8, 0, 0]); // GTI i, 0 -> loop
        c.extend([0x03, 0xFF, 0x26, 1, 0x23, 4, 0x22, 4, 0x44, 0x01]);
        let relocs = [lp + 1, lp + 6, br + 10];
        // the data relocations point at data+0: section byte 1
        c[lp as usize + 1] = 1;
        c[lp as usize + 6] = 1;
        let (mut vm, mut host) = (Vm::new(), Times10::default());
        let s = vm.load("t", &build(&c, &relocs, &[], &[0; 4], 0, 0x2222)).unwrap();
        vm.link(&host);
        assert_eq!(vm.execute(&mut host, s, 0x1111, &[]).unwrap() as i32, -1);
        assert_eq!(vm.mem.u32(vm.scripts[s].data.0).unwrap(), 55);
    }

    #[test]
    fn script_calls_and_arguments() {
        // Second(a, b) returns a - b (ADDRF 0, ADDRF 4); Main calls it through the import (linked to Second, a
        // script function: its result is pushed) with 7 and 3, and returns the result
        let second = 25u32;
        let mut c = vec![0x00, 0, 0, 8, 0, 0x05, 7, 0, 0, 0, 0x0B, 0x05, 3, 0, 0, 0, 0x0B];
        c.extend([0x48, 0xFF, 1, 0, 0, 0x35, 0x44, 0x01]);
        let iref = 18;
        assert_eq!(c.len(), second as usize);
        c.extend([0x00, 0, 0, 0, 0, 0x49, 0, 0, 0x1B, 0x49, 4, 0, 0x1B, 0x50, 0x44, 0x01]);
        let (mut vm, mut host) = (Vm::new(), Times10::default());
        let s = vm.load("t", &build(&c, &[], &[iref], &[0; 4], second, 0x4444)).unwrap();
        vm.link(&host);
        assert_eq!(vm.execute(&mut host, s, 0x1111, &[]).unwrap(), 4);
        // the host's arguments reach ADDRF
        assert_eq!(vm.execute(&mut host, s, 0x4444, &[10, 4]).unwrap(), 6);
        assert!(host.calls.is_empty());
    }
}
