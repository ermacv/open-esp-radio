//! The generator over host memory and a stepping clock.

use core::cell::Cell;
use std::{vec, vec::Vec};

use oer_esp32s31_pac::instances;
use oer_time::{Clock, Instant};

use super::{Entropy, EntropyRegisters, READ_GAP};

/// A clock that advances one microsecond at every read and counts them.
struct Stepping(Cell<u64>);

impl Clock for Stepping {
    fn now(&self) -> Instant {
        let now = self.0.get();
        self.0.set(now + 1);
        Instant::from_micros(now)
    }
}

/// Host memory of `bytes` bytes, each word `fill`.
fn memory(bytes: usize, fill: u32) -> Vec<u32> {
    vec![fill; bytes.div_ceil(4)]
}

#[test]
fn the_first_read_does_not_wait_and_later_reads_keep_the_gap() {
    let mut trng = memory(instances::trng::SIZE, 0x0403_0201);
    let mut clock_words = memory(instances::lp_periclkrst::SIZE, 0);
    let clock = Stepping(Cell::new(100));
    let mut entropy = Entropy::new(
        EntropyRegisters::host(&mut trng, &mut clock_words).expect("host memory holds the blocks"),
        &clock,
    );
    assert_eq!(entropy.random(), 0x0403_0201);
    let first = entropy.last.expect("a read records its time");
    assert_eq!(clock.0.get(), 101, "the first read took one clock reading");
    entropy.random();
    let second = entropy.last.expect("a read records its time");
    assert!(second.saturating_duration_since(first) >= READ_GAP);
}

#[test]
fn bytes_come_four_from_each_word_little_endian() {
    let mut trng = memory(instances::trng::SIZE, 0x0403_0201);
    let mut clock_words = memory(instances::lp_periclkrst::SIZE, 0);
    let clock = Stepping(Cell::new(0));
    let mut entropy = Entropy::new(
        EntropyRegisters::host(&mut trng, &mut clock_words).expect("host memory holds the blocks"),
        &clock,
    );
    assert_eq!(entropy.random_bytes::<6>(), [1, 2, 3, 4, 1, 2]);
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
