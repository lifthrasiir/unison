//! Exact integer helpers shared by the geometry code.
//!
//! Everything here is used from more than one module. `gcd` in particular had
//! drifted into three separate copies (two of them in `on_demand.rs`, at two
//! different widths) before this file existed; a helper that three modules
//! want is a module, not a private function each of them re-derives.
//!
//! # Binary GCD
//!
//! The implementations are Stein's algorithm — shifts and subtractions, no
//! division. That is not a micro-optimization at `u128`: a 128-bit remainder
//! is a call into `__umodti3`, so a Euclidean loop there costs a software
//! division per iteration, while the binary form stays in registers at every
//! width. The two widths are generated from one body so they cannot drift
//! apart again.

/// Body of Stein's algorithm, instantiated per width.
macro_rules! binary_gcd {
    ($name:ident, $t:ty) => {
        /// Greatest common divisor. `gcd(x, 0) == gcd(0, x) == x`.
        pub fn $name(mut a: $t, mut b: $t) -> $t {
            if a == 0 {
                return b;
            }
            if b == 0 {
                return a;
            }
            // Factor out the common power of two, then keep both odd: an odd
            // difference of two odds is even, so every round strips at least
            // one bit and the loop is O(bits).
            let shift = (a | b).trailing_zeros();
            a >>= a.trailing_zeros();
            loop {
                b >>= b.trailing_zeros();
                if a > b {
                    core::mem::swap(&mut a, &mut b);
                }
                b -= a;
                if b == 0 {
                    return a << shift;
                }
            }
        }
    };
}

binary_gcd!(gcd_u64, u64);
binary_gcd!(gcd_u128, u128);

/// `gcd` of a signed value's magnitude and an unsigned one, as `i128`. The
/// rational arithmetic in `detail.rs` cancels signed numerators against
/// positive denominators and wants the result back as a signed divisor.
pub fn gcd_i128(a: i128, b: i128) -> i128 {
    gcd_u128(a.unsigned_abs(), b.unsigned_abs()) as i128
}

/// Least common multiple, or `None` on overflow.
pub fn lcm_u64(a: u64, b: u64) -> Option<u64> {
    if a == 0 || b == 0 {
        return Some(0);
    }
    (a / gcd_u64(a, b)).checked_mul(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gcd_reference(mut a: u128, mut b: u128) -> u128 {
        while b != 0 {
            let t = a % b;
            a = b;
            b = t;
        }
        a
    }

    #[test]
    fn binary_gcd_agrees_with_the_euclidean_one() {
        // A spread of shapes the shift/subtract form has to get right:
        // zeros, powers of two, coprimes, one dividing the other, equals.
        let cases: &[(u128, u128)] = &[
            (0, 0),
            (0, 7),
            (7, 0),
            (1, 1),
            (12, 18),
            (18, 12),
            (255, 51),
            (51, 255),
            (1 << 40, 1 << 17),
            (0xffff_ffff, 0xffff_fffe),
            (u64::MAX as u128, u64::MAX as u128),
            (u128::MAX, u128::MAX),
            (u128::MAX, 1),
            (1 << 100, 3 << 60),
        ];
        for &(a, b) in cases {
            assert_eq!(gcd_u128(a, b), gcd_reference(a, b), "gcd({a}, {b})");
        }
        // And a deterministic sweep over small pairs, both widths.
        for a in 0u64..64 {
            for b in 0u64..64 {
                let want = gcd_reference(a as u128, b as u128);
                assert_eq!(gcd_u64(a, b) as u128, want, "gcd_u64({a}, {b})");
                assert_eq!(gcd_u128(a as u128, b as u128), want, "gcd_u128({a}, {b})");
            }
        }
    }

    #[test]
    fn lcm_reports_overflow_instead_of_wrapping() {
        assert_eq!(lcm_u64(4, 6), Some(12));
        assert_eq!(lcm_u64(255, 51), Some(255));
        assert_eq!(lcm_u64(0, 5), Some(0));
        assert_eq!(lcm_u64(u64::MAX, u64::MAX - 1), None);
    }
}
