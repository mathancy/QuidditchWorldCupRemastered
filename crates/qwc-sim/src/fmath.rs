//! The transcendental functions the rules use, from `libm` (plain Rust, the same bits on every platform) instead of
//! the system's C library, whose last bits differ between Windows and Linux: two machines playing online run the same
//! match only if every tick computes the same numbers (`qwc-net`). The arithmetic and `sqrt` are IEEE-exact already,
//! and the rules' only `powi` is a square. `clippy.toml` refuses the standard library's versions in this crate.

/// A float type the functions take: `f64`, or `f32` where the original kept a single.
pub trait Float: Copy {
    fn sin_(self) -> Self;
    fn cos_(self) -> Self;
    fn acos_(self) -> Self;
    fn exp_(self) -> Self;
}

impl Float for f64 {
    fn sin_(self) -> f64 {
        libm::sin(self)
    }
    fn cos_(self) -> f64 {
        libm::cos(self)
    }
    fn acos_(self) -> f64 {
        libm::acos(self)
    }
    fn exp_(self) -> f64 {
        libm::exp(self)
    }
}

impl Float for f32 {
    fn sin_(self) -> f32 {
        libm::sinf(self)
    }
    fn cos_(self) -> f32 {
        libm::cosf(self)
    }
    fn acos_(self) -> f32 {
        libm::acosf(self)
    }
    fn exp_(self) -> f32 {
        libm::expf(self)
    }
}

pub fn sin<T: Float>(x: T) -> T {
    x.sin_()
}

pub fn cos<T: Float>(x: T) -> T {
    x.cos_()
}

pub fn acos<T: Float>(x: T) -> T {
    x.acos_()
}

pub fn exp<T: Float>(x: T) -> T {
    x.exp_()
}

/// `(sin x, cos x)`.
pub fn sin_cos<T: Float>(x: T) -> (T, T) {
    (x.sin_(), x.cos_())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Within an ulp or so of the system's, at the values the rules meet.
    #[test]
    fn close_to_the_standard_library() {
        #[allow(clippy::disallowed_methods)]
        for x in [0.0f64, 0.1, 0.3, 1.0, std::f64::consts::FRAC_PI_3, 2.5, -1.2, 6.0] {
            assert!((sin(x) - x.sin()).abs() <= 1e-15);
            assert!((cos(x) - x.cos()).abs() <= 1e-15);
            assert!((exp(-x.abs()) - (-x.abs()).exp()).abs() <= 1e-15);
            let c = x.cos();
            assert!((acos(c) - c.acos()).abs() <= 1e-14);
            assert!((sin(x as f32) - (x as f32).sin()).abs() <= 1e-6);
        }
    }

    /// Fixed bits, so a platform whose `libm` build differed would show here (the values from this build).
    #[test]
    fn the_same_bits_everywhere() {
        assert_eq!(sin(1.0f64).to_bits(), 0x3feaed548f090cee);
        assert_eq!(cos(1.0f64).to_bits(), 0x3fe14a280fb5068c);
        assert_eq!(exp(-1.0f64).to_bits(), 0x3fd78b56362cef38);
    }
}
