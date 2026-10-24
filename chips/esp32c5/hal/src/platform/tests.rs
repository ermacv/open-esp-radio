//! The platform owners over host memory and a stepping clock.

use core::cell::Cell;
use std::{vec, vec::Vec};

use oer_time::{Clock, Instant};

use super::{Entropy, EntropyRegisters, READ_GAP, ResetCause, ResetCauseRegisters, ResetCauses};

/// A clock that advances one microsecond at every read.
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

/// Enough host memory for any of the platform blocks.
const BLOCK_BYTES: usize = 0x100;

#[test]
fn the_reset_cause_decodes_with_the_chip_table() {
    let mut words = memory(BLOCK_BYTES, 0b10_0011);
    let causes = ResetCauses::read(
        ResetCauseRegisters::host(&mut words).expect("host memory holds the block"),
    );
    assert_eq!(causes.core(), ResetCause::CoreSoftware);
    let mut words = memory(BLOCK_BYTES, 0x15);
    let causes = ResetCauses::read(
        ResetCauseRegisters::host(&mut words).expect("host memory holds the block"),
    );
    assert_eq!(causes.core(), ResetCause::CoreUsbUart);
    let mut words = memory(BLOCK_BYTES, u32::MAX);
    let causes = ResetCauses::read(
        ResetCauseRegisters::host(&mut words).expect("host memory holds the block"),
    );
    assert_eq!(causes.core(), ResetCause::Other(0x1f));
}

#[test]
fn owning_the_generator_enables_it() {
    let mut words = memory(BLOCK_BYTES, 0);
    let clock = Stepping(Cell::new(0));
    {
        let _entropy = Entropy::new(
            EntropyRegisters::host(&mut words).expect("host memory holds the block"),
            &clock,
        );
    }
    assert!(
        words.iter().any(|&word| word != 0),
        "making the owner configured the stopped generator"
    );
}

#[test]
fn the_first_read_does_not_wait_and_later_reads_keep_the_gap() {
    let mut words = memory(BLOCK_BYTES, 0x0403_0201);
    let clock = Stepping(Cell::new(100));
    let mut entropy = Entropy::new(
        EntropyRegisters::host(&mut words).expect("host memory holds the block"),
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
    let mut words = memory(BLOCK_BYTES, 0x0403_0201);
    let clock = Stepping(Cell::new(0));
    let mut entropy = Entropy::new(
        EntropyRegisters::host(&mut words).expect("host memory holds the block"),
        &clock,
    );
    assert_eq!(entropy.random_bytes::<6>(), [1, 2, 3, 4, 1, 2]);
}
