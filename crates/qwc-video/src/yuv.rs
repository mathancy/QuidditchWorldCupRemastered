//! 4:2:0 pictures to 32-bit RGB: BT.709, limited range (Y 16-235, Cb and Cr 16-240), as the remastered movies are
//! tagged; each side divided by `2^shrink` with box averages (the chroma at its own resolution then, or repeated
//! over 2x2 pixels at full size).

/// A picture's planes: Y at `size`, Cb and Cr at half each way (rounded up).
pub struct Planes<'a> {
    pub y: &'a [u8],
    pub u: &'a [u8],
    pub v: &'a [u8],
    pub y_stride: usize,
    pub c_stride: usize,
    pub size: (usize, usize),
}

/// Each side divided by `2^shrink`.
pub fn shrunk((w, h): (usize, usize), shrink: u32) -> (usize, usize) {
    (w >> shrink, h >> shrink)
}

/// One BT.709 limited-range pixel to B G R X (8.8 fixed point: 1.164, 1.793, 0.213, 0.533, 2.112).
#[inline]
fn pixel(y: i32, u: i32, v: i32) -> [u8; 4] {
    let c = 298 * (y - 16) + 128;
    let (u, v) = (u - 128, v - 128);
    let clamp = |x: i32| (x >> 8).clamp(0, 255) as u8;
    [clamp(c + 541 * u), clamp(c - 55 * u - 136 * v), clamp(c + 459 * v), 255]
}

/// Sums of `block` x `block` boxes along one row of boxes starting at `row` of `plane`, into `sums`.
fn box_row(plane: &[u8], stride: usize, row: usize, block: usize, sums: &mut [u32]) {
    sums.fill(0);
    for r in row..row + block {
        let line = &plane[r * stride..];
        for (x, s) in sums.iter_mut().enumerate() {
            *s += line[x * block..x * block + block].iter().map(|&p| p as u32).sum::<u32>();
        }
    }
}

/// The picture as B G R X, rows top down with no padding, at [`shrunk`] size: in bands of rows over up to four
/// threads.
pub fn to_bgra(p: &Planes, shrink: u32) -> Vec<u8> {
    let (w, h) = shrunk(p.size, shrink);
    let mut out = vec![0u8; w * h * 4];
    if w == 0 || h == 0 {
        return out;
    }
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(4);
    // even bands, so that each starts on a chroma row at full size
    let band = h.div_ceil(threads).next_multiple_of(2);
    std::thread::scope(|s| {
        for (i, rows) in out.chunks_mut(band * w * 4).enumerate() {
            s.spawn(move || convert_rows(p, shrink, w, i * band, rows));
        }
    });
    out
}

/// Output rows from `first` into `out` (whole rows `w` wide).
fn convert_rows(p: &Planes, shrink: u32, w: usize, first: usize, out: &mut [u8]) {
    let lines = out.chunks_exact_mut(w * 4).enumerate().map(|(i, l)| (first + i, l.as_chunks_mut::<4>().0));
    match shrink {
        // each chroma sample over 2x2 pixels
        0 => {
            for (row, line) in lines {
                let y = &p.y[row * p.y_stride..][..w];
                let (u, v) = (&p.u[row / 2 * p.c_stride..], &p.v[row / 2 * p.c_stride..]);
                for (x, px) in line.iter_mut().enumerate() {
                    *px = pixel(y[x] as i32, u[x / 2] as i32, v[x / 2] as i32);
                }
            }
        }
        // 2x2 Y boxes, the chroma as it is
        1 => {
            for (row, line) in lines {
                let y0 = &p.y[2 * row * p.y_stride..][..2 * w];
                let y1 = &p.y[(2 * row + 1) * p.y_stride..][..2 * w];
                let (u, v) = (&p.u[row * p.c_stride..][..w], &p.v[row * p.c_stride..][..w]);
                for (x, px) in line.iter_mut().enumerate() {
                    let s = y0[2 * x] as i32 + y0[2 * x + 1] as i32 + y1[2 * x] as i32 + y1[2 * x + 1] as i32;
                    *px = pixel((s + 2) >> 2, u[x] as i32, v[x] as i32);
                }
            }
        }
        _ => {
            let (block, c_block) = (1usize << shrink, 1usize << (shrink - 1));
            let (mut ys, mut us, mut vs) = (vec![0u32; w], vec![0u32; w], vec![0u32; w]);
            let (y_shift, c_shift) = (2 * shrink, 2 * (shrink - 1));
            let round = |s: u32, shift: u32| ((s + ((1 << shift) >> 1)) >> shift) as i32;
            for (row, line) in lines {
                box_row(p.y, p.y_stride, row * block, block, &mut ys);
                box_row(p.u, p.c_stride, row * c_block, c_block, &mut us);
                box_row(p.v, p.c_stride, row * c_block, c_block, &mut vs);
                for (x, px) in line.iter_mut().enumerate() {
                    *px = pixel(round(ys[x], y_shift), round(us[x], c_shift), round(vs[x], c_shift));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(size: (usize, usize), y: u8, u: u8, v: u8) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let c = size.0.div_ceil(2) * size.1.div_ceil(2);
        (vec![y; size.0 * size.1], vec![u; c], vec![v; c])
    }

    #[test]
    fn bt709_limited() {
        // black, white, grey, and BT.709's red, green and blue
        assert_eq!(pixel(16, 128, 128), [0, 0, 0, 255]);
        assert_eq!(pixel(235, 128, 128), [255, 255, 255, 255]);
        assert_eq!(pixel(126, 128, 128), [128, 128, 128, 255]);
        let near = |a: [u8; 4], b: [u8; 4]| a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 2);
        assert!(near(pixel(63, 102, 240), [0, 0, 255, 255]), "{:?}", pixel(63, 102, 240));
        assert!(near(pixel(173, 42, 26), [0, 255, 0, 255]), "{:?}", pixel(173, 42, 26));
        assert!(near(pixel(32, 240, 118), [255, 0, 0, 255]), "{:?}", pixel(32, 240, 118));
    }

    /// The fast paths and the bands against the definition: each output pixel the rounded mean of its Y box and of
    /// its chroma box (or the one chroma sample over it at full size).
    #[test]
    fn matches_the_definition() {
        let (w, h) = (36, 22);
        let mut seed = 7u32;
        let mut noise = |n: usize| -> Vec<u8> {
            (0..n)
                .map(|_| {
                    seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                    (seed >> 16) as u8
                })
                .collect()
        };
        let (y, u, v) = (noise(w * h), noise(w / 2 * h / 2), noise(w / 2 * h / 2));
        let p = Planes { y: &y, u: &u, v: &v, y_stride: w, c_stride: w / 2, size: (w, h) };
        for shrink in 0..3u32 {
            let (sw, sh) = shrunk((w, h), shrink);
            let b = 1usize << shrink;
            let mean = |plane: &[u8], stride: usize, x0: usize, y0: usize, n: usize| {
                let s: u32 = (0..n)
                    .flat_map(|j| (0..n).map(move |i| (i, j)))
                    .map(|(i, j)| plane[(y0 + j) * stride + x0 + i] as u32)
                    .sum();
                ((s + (n * n / 2) as u32) / (n * n) as u32) as i32
            };
            let mut want = Vec::new();
            for row in 0..sh {
                for x in 0..sw {
                    let yy = mean(&y, w, x * b, row * b, b);
                    let (cu, cv) = if shrink == 0 {
                        (u[row / 2 * (w / 2) + x / 2] as i32, v[row / 2 * (w / 2) + x / 2] as i32)
                    } else {
                        (mean(&u, w / 2, x * b / 2, row * b / 2, b / 2), mean(&v, w / 2, x * b / 2, row * b / 2, b / 2))
                    };
                    want.extend(pixel(yy, cu, cv));
                }
            }
            assert_eq!(to_bgra(&p, shrink), want, "shrink {shrink}");
        }
    }

    #[test]
    fn shrinks_by_boxes() {
        // 8x4, the left half Y 16 and the right 235: halved twice gives 2x1, one black and one white pixel
        let size = (8, 4);
        let (mut y, u, v) = flat(size, 16, 128, 128);
        for r in 0..4 {
            y[r * 8 + 4..r * 8 + 8].fill(235);
        }
        let p = Planes { y: &y, u: &u, v: &v, y_stride: 8, c_stride: 4, size };
        assert_eq!(to_bgra(&p, 2), [0, 0, 0, 255, 255, 255, 255, 255]);
        // at full size each chroma sample covers 2x2 pixels
        let (y, mut u, v) = flat((4, 2), 126, 128, 128);
        u[1] = 240;
        let p = Planes { y: &y, u: &u, v: &v, y_stride: 4, c_stride: 2, size: (4, 2) };
        let out = to_bgra(&p, 0);
        assert_eq!(out.len(), 4 * 2 * 4);
        assert_eq!(&out[0..4], &[128, 128, 128, 255]);
        assert_eq!(out[8], 255, "blue raised where Cb is");
        assert_eq!(out[4 * 4 + 12], 255, "and on the row below");
    }
}
