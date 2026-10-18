//! Deterministic inputs for the "never panics on received bytes" tests.
//!
//! A xorshift generator draws random byte strings and mutations of real
//! frames: bit flips, byte overwrites, truncations, extensions and spliced
//! repeats. The same seed always yields the same inputs, so a failure
//! reproduces.

use std::vec::Vec;

/// The inputs per entry point in an ordinary `cargo test`.
pub(crate) const QUICK: usize = 100_000;
/// The inputs per entry point in the `#[ignore]`d full run.
#[allow(dead_code, reason = "the ignored full runs use it")]
pub(crate) const FULL: usize = 2_000_000;

pub(crate) struct XorShift(u64);

impl XorShift {
    pub(crate) const fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    pub(crate) fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub(crate) fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next() % bound as u64) as usize
        }
    }

    fn byte(&mut self) -> u8 {
        self.next() as u8
    }
}

/// Call `each` with `count` inputs: random strings up to `max_len` bytes and
/// mutations of `seeds`.
pub(crate) fn inputs(seeds: &[&[u8]], count: usize, max_len: usize, mut each: impl FnMut(&[u8])) {
    let mut rng = XorShift::new(0x9e37_79b9_7f4a_7c15 ^ count as u64);
    let mut buffer = Vec::with_capacity(max_len.saturating_mul(2));
    for _ in 0..count {
        buffer.clear();
        if seeds.is_empty() || rng.below(4) == 0 {
            let len = rng.below(max_len.saturating_add(1));
            buffer.extend((0..len).map(|_| rng.byte()));
        } else {
            buffer.extend_from_slice(seeds[rng.below(seeds.len())]);
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
            buffer[at] ^= 1 << rng.below(8);
        }
        1 if !buffer.is_empty() => {
            let at = rng.below(buffer.len());
            buffer[at] = match rng.below(4) {
                0 => 0,
                1 => 0xff,
                2 => buffer[at].wrapping_add(1),
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
            let end = start + rng.below(buffer.len() - start + 1);
            let repeat = buffer[start..end].to_vec();
            let at = rng.below(buffer.len() + 1);
            buffer.splice(at..at, repeat);
        }
        _ if !buffer.is_empty() => {
            // A length byte: the shortest, the longest, or one off.
            let at = rng.below(buffer.len());
            buffer[at] = match rng.below(3) {
                0 => 0,
                1 => 0xff,
                _ => buffer[at].wrapping_sub(1),
            };
        }
        _ => buffer.push(rng.byte()),
    }
}
