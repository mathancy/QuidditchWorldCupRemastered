//! The game's random numbers: the MSVC C runtime `rand()` (`0x57f696`) and its `[0, 1]` scaling (`0x418790`).
//! Reproducing the generator and the order of draws is what makes a match replay like the original.

/// `rand()`: `s = s * 214013 + 2531011`, result `(s >> 16) & 0x7fff`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rand {
    pub state: u32,
}

impl Rand {
    /// The state after `srand(seed)`. The game's seed has not been traced.
    pub fn new(seed: u32) -> Rand {
        Rand { state: seed }
    }

    /// The next value, 0 to 32767.
    pub fn rand(&mut self) -> i32 {
        self.state = self.state.wrapping_mul(214013).wrapping_add(2531011);
        ((self.state >> 16) & 0x7fff) as i32
    }

    /// `rand() / 32767`, in `[0, 1]` (the game multiplies by the reciprocal; the arithmetic is x87, so `f64` here).
    pub fn unit(&mut self) -> f64 {
        self.rand() as f64 * (1.0 / 32767.0)
    }
}

/// The runtime's float-to-int conversion (`_ftol`, `0x581d24`): truncation toward zero. Out of range (or NaN) the
/// x87 `fistp` gives the "integer indefinite" value `0x80000000`.
pub fn trunc(v: f64) -> i32 {
    if v.is_nan() || v <= i32::MIN as f64 - 1.0 || v >= i32::MAX as f64 + 1.0 {
        i32::MIN
    } else {
        v as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_msvc() {
        // srand(1) then rand() x 5 with the Microsoft C runtime
        let mut r = Rand::new(1);
        let v: Vec<i32> = (0..5).map(|_| r.rand()).collect();
        assert_eq!(v, [41, 18467, 6334, 26500, 19169]);
        assert_eq!(trunc(-2.7), -2);
        assert_eq!(trunc(2.7), 2);
        assert_eq!(trunc(3e9), i32::MIN);
        assert_eq!(trunc(f64::NAN), i32::MIN);
    }
}
