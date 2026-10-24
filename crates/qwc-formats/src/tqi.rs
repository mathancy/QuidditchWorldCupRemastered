//! EA TQI movies (`movies/*.tgq`), as described in `tools/tqi_parse.py` (the reference): an EA `SCHl` stream of
//! `pIQT` video chunks (MPEG-1 intra pictures without headers) and `SCDl` audio chunks, 25 frames per second.
//!
//! [`Movie::parse`] walks the chunks; [`Movie::frame`] decodes one picture to YUV 4:2:0 and [`Frame::to_rgba`]
//! converts it; [`Movie::audio`] decodes the sound (`ea_audio::decode_schl`). **[I]** The rounding of the
//! original's integer inverse DCT is not reproduced (samples agree with ffmpeg's `eatqi` to within 3 levels), and
//! the colour conversion is BT.601 limited range.

use std::ops::{Deref, Range};
use std::sync::OnceLock;

use crate::ea_audio::{decode_schl, Pcm};
use crate::util::{bad, Result};

/// Frames per second (the audio's length: 167 frames last 6.67 s).
pub const FPS: u32 = 25;

/// A parsed movie over its bytes (`D` a slice or anything owning one): its pictures and the stream the audio is
/// decoded from.
#[derive(Debug, Clone)]
pub struct Movie<D> {
    data: D,
    /// Where each `pIQT` chunk's payload is, in order.
    pub frames: Vec<Range<usize>>,
    pub width: u16,
    pub height: u16,
}

/// One decoded picture, planar YUV 4:2:0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub y: Vec<u8>,
    pub cb: Vec<u8>,
    pub cr: Vec<u8>,
}

impl<D: Deref<Target = [u8]>> Movie<D> {
    pub fn parse(data: D) -> Result<Movie<D>> {
        let d: &[u8] = &data;
        if d.get(..4) != Some(b"SCHl") {
            return bad("not an EA SCHl movie");
        }
        let mut frames = Vec::new();
        let mut o = 0;
        while o + 8 <= d.len() {
            let n = u32::from_le_bytes(d[o + 4..o + 8].try_into().unwrap()) as usize;
            if n < 8 || o + n > d.len() {
                return bad(format!("chunk of size {n} at {o:#x}"));
            }
            match &d[o..o + 4] {
                b"pIQT" => frames.push(o + 8..o + n),
                b"SCEl" => break,
                _ => {}
            }
            o += n;
        }
        let Some(first) = frames.first().map(|r| &d[r.clone()]).filter(|f| f.len() >= 8) else {
            return bad("no pictures");
        };
        let (width, height) = (u16::from_le_bytes([first[0], first[1]]), u16::from_le_bytes([first[2], first[3]]));
        if width == 0 || height == 0 || width % 16 != 0 || height % 16 != 0 {
            return bad(format!("picture size {width}x{height}"));
        }
        Ok(Movie { data, frames, width, height })
    }

    /// The length in seconds.
    pub fn seconds(&self) -> f64 {
        self.frames.len() as f64 / FPS as f64
    }

    /// Decode picture `k`.
    pub fn frame(&self, k: usize) -> Result<Frame> {
        self.frame_and_bits(k).map(|(f, _)| f)
    }

    /// Decode picture `k`, with the bits its picture data took.
    pub fn frame_and_bits(&self, k: usize) -> Result<(Frame, usize)> {
        let Some(r) = self.frames.get(k) else { return bad(format!("no frame {k}")) };
        decode(&self.data[r.clone()])
    }

    /// Each picture's quantiser (the payload's byte 4).
    pub fn quantisers(&self) -> Vec<u8> {
        self.frames.iter().map(|r| self.data[r.start + 4]).collect()
    }

    /// Each picture's payload.
    pub fn payload(&self, k: usize) -> &[u8] {
        &self.data[self.frames[k].clone()]
    }

    /// The sound, all of it.
    pub fn audio(&self) -> Result<Pcm> {
        decode_schl(&self.data)
    }
}

/// The MPEG-1 default intra matrix, natural order.
const INTRA: [u32; 64] = [
    8, 16, 19, 22, 26, 27, 29, 34, //
    16, 16, 22, 24, 27, 29, 34, 37, //
    19, 22, 26, 27, 29, 34, 34, 38, //
    22, 22, 26, 27, 29, 34, 37, 40, //
    22, 26, 27, 29, 32, 35, 40, 48, //
    26, 27, 29, 32, 35, 40, 48, 58, //
    26, 27, 29, 34, 38, 46, 56, 69, //
    27, 29, 35, 38, 46, 56, 69, 83,
];

/// MPEG-1 DC size codes (ISO 11172-2 B.12 and B.13), by size: (code, length).
const DC_LUMA: [(u16, u8); 9] = [
    (0b100, 3),
    (0b00, 2),
    (0b01, 2),
    (0b101, 3),
    (0b110, 3),
    (0b1110, 4),
    (0b11110, 5),
    (0b111110, 6),
    (0b1111110, 7),
];
const DC_CHROMA: [(u16, u8); 9] = [
    (0b00, 2),
    (0b01, 2),
    (0b10, 2),
    (0b110, 3),
    (0b1110, 4),
    (0b11110, 5),
    (0b111110, 6),
    (0b1111110, 7),
    (0b11111110, 8),
];

/// MPEG-1 table B.14 as (run, level, code), the code as a string of bits without the sign; `0 1` takes its `11`
/// form (an intra block's AC coefficients are never its first).
const AC: [(u8, u8, &str); 111] = [
    (0, 1, "11"),
    (1, 1, "011"),
    (0, 2, "0100"),
    (2, 1, "0101"),
    (0, 3, "00101"),
    (3, 1, "00111"),
    (4, 1, "00110"),
    (1, 2, "000110"),
    (5, 1, "000111"),
    (6, 1, "000101"),
    (7, 1, "000100"),
    (0, 4, "0000110"),
    (2, 2, "0000100"),
    (8, 1, "0000111"),
    (9, 1, "0000101"),
    (0, 5, "00100110"),
    (0, 6, "00100001"),
    (1, 3, "00100101"),
    (3, 2, "00100100"),
    (10, 1, "00100111"),
    (11, 1, "00100011"),
    (12, 1, "00100010"),
    (13, 1, "00100000"),
    (0, 7, "0000001010"),
    (1, 4, "0000001100"),
    (2, 3, "0000001011"),
    (4, 2, "0000001111"),
    (5, 2, "0000001001"),
    (14, 1, "0000001110"),
    (15, 1, "0000001101"),
    (16, 1, "0000001000"),
    (0, 8, "000000011101"),
    (0, 9, "000000011000"),
    (0, 10, "000000010011"),
    (0, 11, "000000010000"),
    (1, 5, "000000011011"),
    (2, 4, "000000010100"),
    (3, 3, "000000011100"),
    (4, 3, "000000010010"),
    (6, 2, "000000011110"),
    (7, 2, "000000010101"),
    (8, 2, "000000010001"),
    (17, 1, "000000011111"),
    (18, 1, "000000011010"),
    (19, 1, "000000011001"),
    (20, 1, "000000010111"),
    (21, 1, "000000010110"),
    (0, 12, "0000000011010"),
    (0, 13, "0000000011001"),
    (0, 14, "0000000011000"),
    (0, 15, "0000000010111"),
    (1, 6, "0000000010110"),
    (1, 7, "0000000010101"),
    (2, 5, "0000000010100"),
    (3, 4, "0000000010011"),
    (5, 3, "0000000010010"),
    (9, 2, "0000000010001"),
    (10, 2, "0000000010000"),
    (22, 1, "0000000011111"),
    (23, 1, "0000000011110"),
    (24, 1, "0000000011101"),
    (25, 1, "0000000011100"),
    (26, 1, "0000000011011"),
    (0, 16, "00000000011111"),
    (0, 17, "00000000011110"),
    (0, 18, "00000000011101"),
    (0, 19, "00000000011100"),
    (0, 20, "00000000011011"),
    (0, 21, "00000000011010"),
    (0, 22, "00000000011001"),
    (0, 23, "00000000011000"),
    (0, 24, "00000000010111"),
    (0, 25, "00000000010110"),
    (0, 26, "00000000010101"),
    (0, 27, "00000000010100"),
    (0, 28, "00000000010011"),
    (0, 29, "00000000010010"),
    (0, 30, "00000000010001"),
    (0, 31, "00000000010000"),
    (0, 32, "000000000011000"),
    (0, 33, "000000000010111"),
    (0, 34, "000000000010110"),
    (0, 35, "000000000010101"),
    (0, 36, "000000000010100"),
    (0, 37, "000000000010011"),
    (0, 38, "000000000010010"),
    (0, 39, "000000000010001"),
    (0, 40, "000000000010000"),
    (1, 8, "000000000011111"),
    (1, 9, "000000000011110"),
    (1, 10, "000000000011101"),
    (1, 11, "000000000011100"),
    (1, 12, "000000000011011"),
    (1, 13, "000000000011010"),
    (1, 14, "000000000011001"),
    (1, 15, "0000000000010011"),
    (1, 16, "0000000000010010"),
    (1, 17, "0000000000010001"),
    (1, 18, "0000000000010000"),
    (6, 3, "0000000000010100"),
    (11, 2, "0000000000011010"),
    (12, 2, "0000000000011001"),
    (13, 2, "0000000000011000"),
    (14, 2, "0000000000010111"),
    (15, 2, "0000000000010110"),
    (16, 2, "0000000000010101"),
    (27, 1, "0000000000011111"),
    (28, 1, "0000000000011110"),
    (29, 1, "0000000000011101"),
    (30, 1, "0000000000011100"),
    (31, 1, "0000000000011011"),
];

/// A decoded AC code: what it means and its length (the sign bit not included).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ac {
    Invalid,
    Eob(u8),
    Escape(u8),
    Pair { run: u8, level: u8, len: u8 },
}

/// The AC table indexed by the next 16 bits.
fn ac_table() -> &'static [Ac] {
    static T: OnceLock<Vec<Ac>> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = vec![Ac::Invalid; 1 << 16];
        let mut put = |code: u32, len: u8, v: Ac| {
            let shift = 16 - len as u32;
            for k in 0..1u32 << shift {
                t[((code << shift) | k) as usize] = v;
            }
        };
        put(0b10, 2, Ac::Eob(2));
        put(0b000001, 6, Ac::Escape(6));
        for &(run, level, s) in &AC {
            let code = u32::from_str_radix(s, 2).unwrap();
            put(code, s.len() as u8, Ac::Pair { run, level, len: s.len() as u8 });
        }
        t
    })
}

/// The zigzag scan: natural index of each position.
fn zigzag() -> &'static [usize; 64] {
    static Z: OnceLock<[usize; 64]> = OnceLock::new();
    Z.get_or_init(|| {
        let mut z = [0; 64];
        let mut k = 0;
        for s in 0..15usize {
            let mut diag: Vec<(usize, usize)> = (0..8).filter(|&y| s >= y && s - y < 8).map(|y| (y, s - y)).collect();
            if s % 2 == 0 {
                diag.reverse();
            }
            for (y, x) in diag {
                z[k] = y * 8 + x;
                k += 1;
            }
        }
        z
    })
}

/// `cos` of the orthonormal 8-point inverse DCT: `C[u][x]`.
fn basis() -> &'static [[f64; 8]; 8] {
    static B: OnceLock<[[f64; 8]; 8]> = OnceLock::new();
    B.get_or_init(|| {
        let mut b = [[0.0; 8]; 8];
        for (u, row) in b.iter_mut().enumerate() {
            let a = if u == 0 { (1.0f64 / 8.0).sqrt() } else { (2.0f64 / 8.0).sqrt() };
            for (x, c) in row.iter_mut().enumerate() {
                *c = a * ((2 * x + 1) as f64 * u as f64 * std::f64::consts::PI / 16.0).cos();
            }
        }
        b
    })
}

/// Bits from 32-bit little-endian words, most significant first.
struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn byte(&self, i: usize) -> u32 {
        // byte `i` of the big-endian stream is byte `i ^ 3` of its little-endian word
        let j = i ^ 3;
        if (i & !3) + 4 <= self.d.len() {
            self.d[j] as u32
        } else {
            0
        }
    }

    fn peek(&self, n: u32) -> u32 {
        let i = self.pos >> 3;
        let w = (self.byte(i) << 24) | (self.byte(i + 1) << 16) | (self.byte(i + 2) << 8) | self.byte(i + 3);
        let w = ((w as u64) << 8 | self.byte(i + 4) as u64) << (self.pos & 7);
        ((w >> (40 - n)) & ((1 << n) - 1)) as u32
    }

    fn get(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let v = self.peek(n);
        self.pos += n as usize;
        v
    }

    fn past_end(&self) -> bool {
        self.pos > self.d.len() / 4 * 32
    }

    fn dc_size(&mut self, chroma: bool) -> Result<u32> {
        let t = if chroma { &DC_CHROMA } else { &DC_LUMA };
        let p = self.peek(8);
        for (size, &(code, len)) in t.iter().enumerate() {
            if p >> (8 - len) == code as u32 {
                self.pos += len as usize;
                return Ok(size as u32);
            }
        }
        bad(format!("bad DC size code at bit {}", self.pos))
    }
}

fn decode(p: &[u8]) -> Result<(Frame, usize)> {
    if p.len() < 8 {
        return bad("picture header cut short");
    }
    let (w, h, q) = (u16::from_le_bytes([p[0], p[1]]) as usize, u16::from_le_bytes([p[2], p[3]]) as usize, p[4]);
    if w == 0 || h == 0 || w % 16 != 0 || h % 16 != 0 {
        return bad(format!("picture size {w}x{h}"));
    }
    let qs = (215 - 2 * q as i32) * 5;
    let m: [i32; 64] = std::array::from_fn(|j| (INTRA[j] as i32 * qs + 8) >> 4);
    let (ac, zz, c) = (ac_table(), zigzag(), basis());
    let mut f = Frame { width: w, height: h, y: vec![0; w * h], cb: vec![0; w * h / 4], cr: vec![0; w * h / 4] };
    let mut bits = Bits { d: &p[8..], pos: 0 };
    let mut dc = [0i32; 3];
    for my in 0..h / 16 {
        for mx in 0..w / 16 {
            for n in 0..6 {
                let comp = if n < 4 { 0 } else { n - 3 };
                let size = bits.dc_size(comp != 0)?;
                let mut diff = bits.get(size) as i32;
                if size > 0 && diff < 1 << (size - 1) {
                    diff -= (1 << size) - 1;
                }
                dc[comp] += diff;
                let mut block = [0f64; 64];
                block[0] = (dc[comp] * 8) as f64;
                let mut i = 0usize;
                loop {
                    let (run, level) = match ac[bits.peek(16) as usize] {
                        Ac::Invalid => return bad(format!("bad AC code at bit {}", bits.pos)),
                        Ac::Eob(len) => {
                            bits.pos += len as usize;
                            break;
                        }
                        Ac::Escape(len) => {
                            bits.pos += len as usize;
                            let run = bits.get(6) as usize;
                            let level = match bits.get(8) {
                                0x80 => bits.get(8) as i32 - 256,
                                0 => bits.get(8) as i32,
                                l if l > 0x80 => l as i32 - 256,
                                l => l as i32,
                            };
                            (run, level)
                        }
                        Ac::Pair { run, level, len } => {
                            bits.pos += len as usize;
                            let neg = bits.get(1) == 1;
                            (run as usize, if neg { -(level as i32) } else { level as i32 })
                        }
                    };
                    i += run + 1;
                    if i > 63 {
                        return bad(format!("coefficient past the block at bit {}", bits.pos));
                    }
                    let j = zz[i];
                    let a = (level.abs() * m[j]) >> 3;
                    block[j] = if level < 0 { -a } else { a } as f64;
                }
                if bits.past_end() {
                    return bad("picture cut short");
                }
                let (plane, stride, x0, y0) = match n {
                    0..=3 => (&mut f.y, w, mx * 16 + (n & 1) * 8, my * 16 + (n >> 1) * 8),
                    4 => (&mut f.cb, w / 2, mx * 8, my * 8),
                    _ => (&mut f.cr, w / 2, mx * 8, my * 8),
                };
                idct_put(&block, c, &mut plane[y0 * stride + x0..], stride);
            }
        }
    }
    Ok((f, bits.pos))
}

/// The orthonormal 2D inverse DCT of `block` (natural order, `[v * 8 + u]`), rounded and clamped into `out`.
fn idct_put(block: &[f64; 64], c: &[[f64; 8]; 8], out: &mut [u8], stride: usize) {
    // columns first: t[v][x] = sum_u block[v][u] c[u][x]
    let mut t = [0f64; 64];
    for v in 0..8 {
        let row = &block[v * 8..v * 8 + 8];
        if row.iter().all(|&x| x == 0.0) {
            continue;
        }
        for x in 0..8 {
            t[v * 8 + x] = (0..8).map(|u| row[u] * c[u][x]).sum();
        }
    }
    for y in 0..8 {
        for x in 0..8 {
            let s: f64 = (0..8).map(|v| c[v][y] * t[v * 8 + x]).sum();
            out[y * stride + x] = s.round().clamp(0.0, 255.0) as u8;
        }
    }
}

impl Frame {
    /// RGBA rows (BT.601, limited range; chroma repeated over each 2x2), alpha 255.
    pub fn to_rgba(&self) -> Vec<u8> {
        let (w, h) = (self.width, self.height);
        let mut out = vec![255u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let c = (y / 2) * (w / 2) + x / 2;
                let yy = (self.y[y * w + x] as f32 - 16.0) * (255.0 / 219.0);
                let (cb, cr) = (self.cb[c] as f32 - 128.0, self.cr[c] as f32 - 128.0);
                let k = 255.0 / 224.0;
                let r = yy + 1.402 * k * cr;
                let g = yy - (0.344_136 * cb + 0.714_136 * cr) * k;
                let b = yy + 1.772 * k * cb;
                let o = (y * w + x) * 4;
                out[o] = r.round().clamp(0.0, 255.0) as u8;
                out[o + 1] = g.round().clamp(0.0, 255.0) as u8;
                out[o + 2] = b.round().clamp(0.0, 255.0) as u8;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ac_codes_are_prefix_free_and_fill_the_table() {
        let t = ac_table();
        // every 16-bit prefix decodes except those starting with the 12 zero bits MPEG-1 forbids
        let invalid = t.iter().filter(|a| **a == Ac::Invalid).count();
        assert_eq!(invalid, 16);
        let mut codes: Vec<String> = AC.iter().map(|x| x.2.to_string()).collect();
        codes.extend(["10".into(), "000001".into()]);
        for a in &codes {
            for b in &codes {
                assert!(a == b || !b.starts_with(a.as_str()), "{a} prefixes {b}");
            }
        }
    }

    #[test]
    fn zigzag_starts_and_ends_as_mpeg() {
        let z = zigzag();
        assert_eq!(&z[..10], &[0, 1, 8, 16, 9, 2, 3, 10, 17, 24]);
        assert_eq!(z[63], 63);
    }

    /// A one-macroblock picture: every block a DC difference and EOB.
    #[test]
    fn a_flat_picture_decodes_to_its_dc() {
        // luma DC size 4 (`110`), +12 (`1100`), EOB; then three luma blocks of size 0 (`100`), EOB;
        // chroma: size 0 (`00`), EOB, twice
        let s = "110".to_string() + "1100" + "10" + "10010" + "10010" + "10010" + "0010" + "0010";
        let mut bytes: Vec<u8> = Vec::new();
        let padded = format!("{s:0<32}");
        let word = u32::from_str_radix(&padded, 2).unwrap();
        bytes.extend_from_slice(&word.to_le_bytes());
        let mut p = vec![16, 0, 16, 0, 100, 0, 0, 0];
        p.extend(bytes);
        let (f, bits) = decode(&p).unwrap();
        assert_eq!(bits, s.len());
        assert!(f.y.iter().all(|&v| v == 12), "{:?}", &f.y[..8]);
        assert!(f.cb.iter().chain(&f.cr).all(|&v| v == 0));
    }
}
