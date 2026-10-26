//! Deterministic inputs for the "never panics on received bytes" tests.
//!
//! A xorshift generator draws random byte strings and mutations of real
//! frames: bit flips, byte overwrites, truncations, extensions and spliced
//! repeats. The same seed always yields the same inputs, so a failure
//! reproduces.
//!
//! Other crates' tests reach it through the `test-support` feature, so it is
//! no `cfg(test)` module and its test-input indexing is linted.

use std::vec::Vec;

/// The inputs per entry point in an ordinary `cargo test`.
pub const QUICK: usize = 100_000;
/// The inputs per entry point in the `#[ignore]`d full run.
pub const FULL: usize = 2_000_000;

/// A xorshift64 generator; the same seed always yields the same sequence.
pub struct XorShift(u64);

impl XorShift {
    /// A generator from `seed`; a zero seed is made odd.
    pub const fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    /// The next value.
    #[expect(
        clippy::should_implement_trait,
        reason = "an endless generator, not an iterator"
    )]
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// A value below `bound`, or zero when `bound` is zero.
    pub fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            match self.next().checked_rem(bound as u64) {
                Some(value) => value as usize,
                None => 0,
            }
        }
    }

    fn byte(&mut self) -> u8 {
        self.next() as u8
    }
}

/// Call `each` with `count` inputs: random strings up to `max_len` bytes and
/// mutations of `seeds`.
pub fn inputs(seeds: &[&[u8]], count: usize, max_len: usize, mut each: impl FnMut(&[u8])) {
    let mut rng = XorShift::new(0x9e37_79b9_7f4a_7c15 ^ count as u64);
    let mut buffer = Vec::with_capacity(max_len.saturating_mul(2));
    for _ in 0..count {
        buffer.clear();
        if seeds.is_empty() || rng.below(4) == 0 {
            let len = rng.below(max_len.saturating_add(1));
            buffer.extend((0..len).map(|_| rng.byte()));
        } else {
            if let Some(seed) = seeds.get(rng.below(seeds.len())) {
                buffer.extend_from_slice(seed);
            }
            for _ in 0..=rng.below(4) {
                mutate(&mut rng, &mut buffer);
            }
        }
        each(&buffer);
    }
}

fn mutate(rng: &mut XorShift, buffer: &mut Vec<u8>) {
    match rng.below(6) {
        0 if !buffer.is_empty() => {
            let at = rng.below(buffer.len());
            if let Some(byte) = buffer.get_mut(at) {
                *byte ^= 1 << rng.below(8);
            }
        }
        1 if !buffer.is_empty() => {
            let at = rng.below(buffer.len());
            let Some(byte) = buffer.get_mut(at) else {
                return;
            };
            *byte = match rng.below(4) {
                0 => 0,
                1 => 0xff,
                2 => byte.wrapping_add(1),
                _ => rng.byte(),
            };
        }
        2 => {
            let len = rng.below(buffer.len().saturating_add(1));
            buffer.truncate(len);
        }
        3 => {
            for _ in 0..rng.below(16) {
                buffer.push(rng.byte());
            }
        }
        4 if !buffer.is_empty() => {
            let start = rng.below(buffer.len());
            let Some(suffix) = buffer.get(start..) else {
                return;
            };
            let Some(bound) = suffix.len().checked_add(1) else {
                return;
            };
            let length = rng.below(bound);
            let Some(repeat) = suffix.get(..length) else {
                return;
            };
            let repeat = repeat.to_vec();
            let Some(bound) = buffer.len().checked_add(1) else {
                return;
            };
            let at = rng.below(bound);
            buffer.splice(at..at, repeat);
        }
        _ if !buffer.is_empty() => {
            // A length byte: the shortest, the longest, or one off.
            let at = rng.below(buffer.len());
            let Some(byte) = buffer.get_mut(at) else {
                return;
            };
            *byte = match rng.below(3) {
                0 => 0,
                1 => 0xff,
                _ => byte.wrapping_sub(1),
            };
        }
        _ => buffer.push(rng.byte()),
    }
}
