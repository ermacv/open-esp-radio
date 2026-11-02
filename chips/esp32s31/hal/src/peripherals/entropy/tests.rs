//! The generator over host memory and a stepping clock.

use core::cell::Cell;
use std::{vec, vec::Vec};

use oer_chips_esp32s31_pac::instances;
use oer_portable_time::{Clock, Instant};

use super::{Entropy, EntropyError, EntropyRegisters, READ_GAP};

/// A clock that advances one microsecond at every read and counts them.
struct Stepping(Cell<u64>);

impl Clock for Stepping {
    fn now(&self) -> Instant {
        let now = self.0.get();
        self.0.set(now + 1);
        Instant::from_micros(now)
    }
}

/// A generator word whose bit 0, the health-error flag in the generator's
/// status word, is clear.
const HEALTHY: u32 = 0x0403_0200;

/// A generator word whose bit 0, the health-error flag in the generator's
/// status word, is set.
const FAILED: u32 = 0x0403_0201;

/// Host memory of `bytes` bytes, each word `fill`.
fn memory(bytes: usize, fill: u32) -> Vec<u32> {
    vec![fill; bytes.div_ceil(4)]
}

#[test]
fn the_first_read_does_not_wait_and_later_reads_keep_the_gap() {
    let mut trng = memory(instances::trng::SIZE, HEALTHY);
    let mut clock_words = memory(instances::lp_periclkrst::SIZE, 0);
    let clock = Stepping(Cell::new(100));
    let mut entropy = Entropy::new(
        EntropyRegisters::host(&mut trng, &mut clock_words).expect("host memory holds the blocks"),
        &clock,
    );
    assert_eq!(entropy.random(), Ok(HEALTHY));
    let first = entropy.last.expect("a read records its time");
    assert_eq!(clock.0.get(), 101, "the first read took one clock reading");
    entropy.random().expect("a healthy generator gives a word");
    let second = entropy.last.expect("a read records its time");
    assert!(second.saturating_duration_since(first) >= READ_GAP);
}

#[test]
fn bytes_come_four_from_each_word_little_endian() {
    let mut trng = memory(instances::trng::SIZE, HEALTHY);
    let mut clock_words = memory(instances::lp_periclkrst::SIZE, 0);
    let clock = Stepping(Cell::new(0));
    let mut entropy = Entropy::new(
        EntropyRegisters::host(&mut trng, &mut clock_words).expect("host memory holds the blocks"),
        &clock,
    );
    assert_eq!(entropy.random_bytes::<6>(), Ok([0, 2, 3, 4, 0, 2]));
}

#[test]
fn a_health_error_refuses_the_word_and_clears_the_bytes() {
    let mut trng = memory(instances::trng::SIZE, FAILED);
    let mut clock_words = memory(instances::lp_periclkrst::SIZE, 0);
    let clock = Stepping(Cell::new(0));
    let mut entropy = Entropy::new(
        EntropyRegisters::host(&mut trng, &mut clock_words).expect("host memory holds the blocks"),
        &clock,
    );
    assert_eq!(entropy.random(), Err(EntropyError::HealthTest));
    let mut bytes = [0xff; 6];
    assert_eq!(entropy.fill(&mut bytes), Err(EntropyError::HealthTest));
    assert_eq!(bytes, [0; 6], "no byte of a failing generator is left");
}

#[test]
fn a_clock_that_never_shows_the_gap_is_a_typed_error() {
    let mut trng = memory(instances::trng::SIZE, HEALTHY);
    let mut clock_words = memory(instances::lp_periclkrst::SIZE, 0);
    let clock = Stepping(Cell::new(1_000));
    let mut entropy = Entropy::new(
        EntropyRegisters::host(&mut trng, &mut clock_words).expect("host memory holds the blocks"),
        &clock,
    );
    entropy.random().expect("the first read does not wait");
    // The clock restarts below the last read: its time since that read
    // stays zero while the settle's own limit passes.
    clock.0.set(0);
    assert_eq!(entropy.random(), Err(EntropyError::ReadGap));
}

#[test]
fn a_running_generator_is_healthy_only_without_a_health_error() {
    let mut trng = memory(instances::trng::SIZE, 0);
    let mut clock_words = memory(instances::lp_periclkrst::SIZE, 0);
    let clock = Stepping(Cell::new(0));
    let entropy = Entropy::new(
        EntropyRegisters::host(&mut trng, &mut clock_words).expect("host memory holds the blocks"),
        &clock,
    );
    assert!(
        !entropy.status().is_healthy(),
        "a stopped generator is not healthy"
    );
}
