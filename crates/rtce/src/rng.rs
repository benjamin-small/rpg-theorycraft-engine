//! A tiny in-crate seeded PCG32 (permuted congruential generator, XSH-RR
//! output function) — **not public API** (see `lib.rs`: this module is
//! `mod`, not `pub mod`). Exists purely so `plan::Plan::evaluate_phase_sampled`
//! and `sim::exec`'s Monte Carlo mode can sample deterministically without
//! pulling in a `rand` dependency (the zero-dependency rule holds — see
//! `docs/superpowers/specs/2026-07-22-p6-sequencing-design.md`'s
//! "Executor" MC bullet). Standard O'Neill PCG32 constants/algorithm
//! (<https://www.pcg-random.org/>): a 64-bit LCG state advanced by a fixed
//! multiplier, with a per-instance ODD "increment" (stream constant)
//! derived from the seed, and XSH-RR (xorshift-high, random-rotate) as the
//! output permutation — this is the same construction as the reference
//! `pcg32_random_r`/`pcg32_srandom_r` pair, just without the "insetseq"
//! second seed input (irrelevant here: `new` only ever takes one `u64`).

/// A seeded PCG32 generator. `state` is the 64-bit LCG state; `inc` is the
/// per-instance odd stream constant fixed at construction (see [`Pcg32::new`]) —
/// together they make two different seeds two different (state, stream)
/// pairs, so their output sequences diverge from the very first draw (see
/// this module's `different_seeds_diverge` test).
#[derive(Debug, Clone)]
pub(crate) struct Pcg32 {
    state: u64,
    inc: u64,
}

/// The PCG32 LCG multiplier — the standard constant from the reference
/// implementation (Knuth's MMIX constant), not tunable.
const MULTIPLIER: u64 = 6364136223846793005;

impl Pcg32 {
    /// Seed a new generator from a single `u64`. Two calls with the SAME
    /// `seed` produce an IDENTICAL output sequence — determinism is the
    /// entire point (Monte Carlo reproducibility depends on it; see
    /// `same_seed_is_deterministic` below and `sim::exec`'s
    /// same-seed-twice test).
    pub(crate) fn new(seed: u64) -> Self {
        // Reference `pcg32_srandom_r` recipe: start at state 0 with the
        // stream constant derived from `seed`, step once, fold `seed` into
        // the state, step again.
        let mut rng = Pcg32 {
            state: 0,
            inc: (seed << 1) | 1, // the stream constant must be odd
        };
        rng.next_u32();
        rng.state = rng.state.wrapping_add(seed);
        rng.next_u32();
        rng
    }

    /// Advance the LCG state and return the next 32-bit output (XSH-RR:
    /// xorshift the high bits down, then rotate by the top 5 bits of the
    /// OLD state).
    fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(MULTIPLIER).wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32; // top 5 bits of a u64 => 0..=31
        xorshifted.rotate_right(rot)
    }

    /// The next uniform sample in `[0, 1)` — `next_u32`'s 32 bits of
    /// entropy divided by 2^32: every one of the 2^32 possible `u32`
    /// outputs maps to a distinct value, and the top output
    /// (`u32::MAX`) still lands strictly below `1.0`
    /// (`4294967295.0 / 4294967296.0 < 1.0`).
    pub(crate) fn next_f64(&mut self) -> f64 {
        (self.next_u32() as f64) / 4294967296.0 // 2^32
    }
}

/// SplitMix64's mixing step, used ONLY to derive each Monte Carlo
/// iteration's own [`Pcg32`] seed from a run's master seed and the
/// iteration index (`sim::exec::run`'s `Mode::MonteCarlo` path) —
/// deliberately a DIFFERENT algorithm from `Pcg32` itself, so "iteration
/// N's derived seed" and "iteration N's own output stream" never share
/// structure. Standard SplitMix64 constants (Sebastiano Vigna's `splitmix64`).
pub(crate) fn mix_seed(seed: u64, index: u64) -> u64 {
    let mut z = seed.wrapping_add(index.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same seed, twice, must produce byte-identical draws — the whole
    /// reason this crate rolled its own PCG32 instead of depending on
    /// `rand` (whose generator choice/version could silently change).
    #[test]
    fn same_seed_is_deterministic() {
        let mut a = Pcg32::new(42);
        let mut b = Pcg32::new(42);
        let av: Vec<f64> = (0..5).map(|_| a.next_f64()).collect();
        let bv: Vec<f64> = (0..5).map(|_| b.next_f64()).collect();
        assert_eq!(av, bv, "same seed must reproduce the same sequence");
    }

    /// Every draw lands in the documented `[0, 1)` range — checked over a
    /// large sample since a range bug (e.g. an off-by-one letting `1.0`
    /// through) would only show up on a rare high output.
    #[test]
    fn output_is_in_range() {
        let mut rng = Pcg32::new(7);
        for _ in 0..100_000 {
            let v = rng.next_f64();
            assert!((0.0..1.0).contains(&v), "got {v} — must be in [0, 1)");
        }
    }

    /// Different seeds must diverge (this PCG32 initializes each seed to
    /// its OWN stream constant, not just a different starting state on a
    /// shared stream — the two together are what makes divergence robust).
    #[test]
    fn different_seeds_diverge() {
        let mut a = Pcg32::new(1);
        let mut b = Pcg32::new(2);
        let av: Vec<f64> = (0..5).map(|_| a.next_f64()).collect();
        let bv: Vec<f64> = (0..5).map(|_| b.next_f64()).collect();
        assert_ne!(av, bv, "different seeds must not coincide on 5 draws");
    }

    /// `mix_seed` (used to derive per-MC-iteration seeds) is itself
    /// deterministic and index-sensitive — the two properties MC's
    /// same-seed-reproducibility and inter-iteration-independence rest on.
    #[test]
    fn mix_seed_is_deterministic_and_index_sensitive() {
        assert_eq!(mix_seed(99, 3), mix_seed(99, 3));
        assert_ne!(mix_seed(99, 3), mix_seed(99, 4));
        assert_ne!(mix_seed(99, 3), mix_seed(100, 3));
    }
}
