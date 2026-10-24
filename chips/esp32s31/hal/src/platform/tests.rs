//! The platform owners over host memory and a stepping clock.

use core::cell::Cell;
use std::{vec, vec::Vec};

use oer_esp32s31_knowledge::reset_cause::ResetCause;
use oer_time::{Clock, Instant};

use super::{
    EfuseRegisters, Entropy, EntropyRegisters, FactoryIdentity, HpCore, Interface, READ_GAP,
    ResetCauseRegisters, ResetCauses,
};

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

/// Enough host memory for any of the platform blocks.
const BLOCK_BYTES: usize = 0x100;

#[test]
fn the_identity_derives_each_interface_address_from_the_factory_mac() {
    let mut words = memory(BLOCK_BYTES, 0x0011_2033);
    let identity = FactoryIdentity::read(
        EfuseRegisters::host(&mut words).expect("host memory holds the block"),
    );
    let base = identity.base_mac();
    assert_eq!(base, [0x20, 0x33, 0x00, 0x11, 0x20, 0x33]);
    assert_eq!(identity.mac_extension(), 0x0011);
    assert_eq!(identity.address(Interface::Station), base);
    assert_eq!(
        identity.address(Interface::AccessPoint),
        [0x22, 0x33, 0x00, 0x11, 0x20, 0x33]
    );
}

#[test]
fn reset_causes_decode_each_core_with_the_chip_table() {
    let mut words = memory(BLOCK_BYTES, 0b111);
    let causes = ResetCauses::read(
        ResetCauseRegisters::host(&mut words).expect("host memory holds the block"),
    );
    assert_eq!(causes.of(HpCore::Core0), ResetCause::CoreSoftware);
    assert_eq!(causes.of(HpCore::Core1), ResetCause::CoreSoftware);
    let mut words = memory(BLOCK_BYTES, u32::MAX);
    let causes = ResetCauses::read(
        ResetCauseRegisters::host(&mut words).expect("host memory holds the block"),
    );
    assert_eq!(causes.of(HpCore::Core0), ResetCause::Other(0x3f));
}

#[test]
fn the_first_read_does_not_wait_and_later_reads_keep_the_gap() {
    let mut trng = memory(BLOCK_BYTES, 0x0403_0201);
    let mut clock_words = memory(BLOCK_BYTES, 0);
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
    let mut trng = memory(BLOCK_BYTES, 0x0403_0201);
    let mut clock_words = memory(BLOCK_BYTES, 0);
    let clock = Stepping(Cell::new(0));
    let mut entropy = Entropy::new(
        EntropyRegisters::host(&mut trng, &mut clock_words).expect("host memory holds the blocks"),
        &clock,
    );
    assert_eq!(entropy.random_bytes::<6>(), [1, 2, 3, 4, 1, 2]);
}

#[test]
fn a_running_generator_is_healthy_only_without_a_health_error() {
    let mut trng = memory(BLOCK_BYTES, 0);
    let mut clock_words = memory(BLOCK_BYTES, 0);
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
