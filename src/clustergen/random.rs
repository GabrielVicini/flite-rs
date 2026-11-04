//! The pseudo-random source Clustergen's excitation is built from.
//!
//! This exists because of a real problem rather than a preference. Upstream's
//! MLSA vocoder draws its unvoiced excitation from the C library's `rand()`:
//!
//! ```c
//! static double plus_or_minus_one()
//! {
//!     /* not sure rand() is portable */
//!     if (rand() > RAND_MAX/2.0)
//!         return 1.0;
//!     else
//!         return -1.0;
//! }
//! ```
//!
//! Its own comment is correct. `rand()` is not specified by the C standard
//! beyond its range, and `srand` is never called, so a Clustergen voice's
//! waveform is a function of whichever C library built it: glibc's `rand()` is
//! the TYPE_3 additive-feedback generator of `random()`, while MSVC's is a
//! 32-bit linear congruential generator, and the two agree on nothing. With
//! mixed excitation on — which all three in-tree CG voices ask for — this is
//! consumed on *every output sample*, so it is not a detail at the edges.
//!
//! That makes "bit-identical to upstream Flite" ill-defined for Clustergen in
//! a way it never was for the diphone path, which never reaches `rand()`.
//! The resolution is the same one `tools/reference/README.md` already argues
//! for floating-point contraction: an unspecified property of the C
//! environment is pinned so that the reference holds still, rather than being
//! inherited from whatever compiler happened to build it.
//!
//! What is pinned to is **glibc's `rand()`**, reimplemented below, because
//! that is what upstream is overwhelmingly built and distributed with and what
//! the reference build in CI uses. So on a glibc host the reference is
//! unmodified upstream and the match is against stock Flite; elsewhere
//! `tools/reference/refrand.c` substitutes this same sequence so that the
//! comparison can run at all. See the README's Clustergen section.

/// glibc's `random()`, which is what its `rand()` returns.
///
/// The TYPE_3 generator: a 31-element additive-feedback sequence seeded from a
/// Lehmer generator, with the first 310 outputs discarded. Reproduced from the
/// algorithm, not from glibc's source, and checked in the tests below against
/// the sequence `rand()` is universally known to produce from the default
/// seed.
pub struct GlibcRandom {
    /// The 31-element feedback state, as a ring.
    state: [u32; 31],
    /// Read and write cursors, 3 apart, which is the `sep` of TYPE_3.
    front: usize,
    rear: usize,
}

impl GlibcRandom {
    /// The state `rand()` has before it is first called, i.e. `srandom(1)`.
    ///
    /// Upstream never calls `srand`, and a C program that does not is defined
    /// to behave as if it had called `srand(1)`.
    pub fn new() -> GlibcRandom {
        GlibcRandom::with_seed(1)
    }

    pub fn with_seed(seed: u32) -> GlibcRandom {
        // A zero seed would make the Lehmer step below a fixed point, so glibc
        // substitutes 1.
        let seed = if seed == 0 { 1 } else { seed };
        let mut state = [0u32; 31];
        state[0] = seed;
        for i in 1..31 {
            // r[i] = 16807 * r[i-1] mod 2^31-1, by Schrage's method so that
            // the intermediate product stays inside 32 signed bits. glibc
            // computes it exactly this way, and the comment there notes the
            // result can be negative before the correction.
            let previous = state[i - 1] as i64;
            let hi = previous / 127_773;
            let lo = previous % 127_773;
            let mut word = 16_807 * lo - 2_836 * hi;
            if word < 0 {
                word += 2_147_483_647;
            }
            state[i] = word as u32;
        }

        let mut rng = GlibcRandom {
            state,
            front: 3,
            rear: 0,
        };
        // glibc discards 10 * 31 outputs so that the seeding sequence stops
        // showing through. Without this the first values are wrong, which is
        // what the tests below would catch.
        for _ in 0..310 {
            rng.next_u31();
        }
        rng
    }

    /// One `random()` value: 0 to 2^31 - 1.
    pub fn next_u31(&mut self) -> u32 {
        // Addition is modulo 2^32 by construction, and the result is the top
        // 31 bits, which is where the low-order bit's short period goes.
        self.state[self.front] = self.state[self.front].wrapping_add(self.state[self.rear]);
        let value = self.state[self.front] >> 1;
        self.front = (self.front + 1) % 31;
        self.rear = (self.rear + 1) % 31;
        value
    }

    /// `plus_or_minus_one()`: upstream's one use of this generator.
    ///
    /// `RAND_MAX` is `2^31 - 1` on glibc, so the test is against
    /// `1073741823.5` and the comparison cannot be a tie.
    pub fn plus_or_minus_one(&mut self) -> f64 {
        const HALF_RAND_MAX: f64 = 2_147_483_647.0 / 2.0;
        if self.next_u31() as f64 > HALF_RAND_MAX {
            1.0
        } else {
            -1.0
        }
    }
}

impl Default for GlibcRandom {
    fn default() -> GlibcRandom {
        GlibcRandom::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first ten values of `rand()` in a C program that never calls
    /// `srand`, on glibc. If this ever fails, the generator is wrong and every
    /// Clustergen sample after the first unvoiced frame is wrong with it.
    #[test]
    fn matches_the_known_glibc_sequence() {
        let mut rng = GlibcRandom::new();
        let expected = [
            1_804_289_383,
            846_930_886,
            1_681_692_777,
            1_714_636_915,
            1_957_747_793,
            424_238_335,
            719_885_386,
            1_649_760_492,
            596_516_649,
            1_189_641_421,
        ];
        let got: Vec<u32> = (0..10).map(|_| rng.next_u31()).collect();
        assert_eq!(got, expected);
    }

    /// `srand(1)` is defined to be the same as never seeding at all.
    #[test]
    fn seeding_with_one_is_the_default_state() {
        let mut unseeded = GlibcRandom::new();
        let mut seeded = GlibcRandom::with_seed(1);
        for _ in 0..100 {
            assert_eq!(unseeded.next_u31(), seeded.next_u31());
        }
    }

    /// The sign decision that the vocoder actually consumes.
    #[test]
    fn sign_follows_the_top_half_of_the_range() {
        let mut rng = GlibcRandom::new();
        // 1804289383 > 1073741823.5, 846930886 is not, 1681692777 is.
        assert_eq!(rng.plus_or_minus_one(), 1.0);
        assert_eq!(rng.plus_or_minus_one(), -1.0);
        assert_eq!(rng.plus_or_minus_one(), 1.0);
    }

    /// Values stay in range over a long run, which a wrong shift would break.
    #[test]
    fn values_stay_within_rand_max() {
        let mut rng = GlibcRandom::new();
        for _ in 0..100_000 {
            assert!(rng.next_u31() <= 2_147_483_647);
        }
    }
}
