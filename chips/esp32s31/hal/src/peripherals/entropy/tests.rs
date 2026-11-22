//! The generator over host memory and a stepping clock.

use std::{vec, vec::Vec};

use oer_chips_esp32s31_pac::instances;
use oer_portable_time_virtual::VirtualCounter;

use super::{Entropy, EntropyError, EntropyRegisters, READ_GAP};

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
    let clock = VirtualCounter::new(100, 1);
    let mut entropy = Entropy::new(
        EntropyRegisters::host(&mut trng, &mut clock_words).expect("host memory holds the blocks"),
        &clock,
    );
    assert_eq!(entropy.random(), Ok(HEALTHY));
    let first = entropy.last.expect("a read records its time");
    assert_eq!(clock.now(), 101, "the first read took one sample");
    entropy.random().expect("a healthy generator gives a word");
    let second = entropy.last.expect("a read records its time");
    assert!(u64::from(second.wrapping_sub(first)) >= READ_GAP.as_micros());
}

#[test]
fn bytes_come_four_from_each_word_little_endian() {
    let mut trng = memory(instances::trng::SIZE, HEALTHY);
    let mut clock_words = memory(instances::lp_periclkrst::SIZE, 0);
    let clock = VirtualCounter::new(0, 1);
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
    let clock = VirtualCounter::new(0, 1);
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
fn a_read_keeps_the_gap_across_the_counter_s_wrap() {
    let mut trng = memory(instances::trng::SIZE, HEALTHY);
    let mut clock_words = memory(instances::lp_periclkrst::SIZE, 0);
    let clock = VirtualCounter::new(u32::MAX - 1, 1);
    let mut entropy = Entropy::new(
        EntropyRegisters::host(&mut trng, &mut clock_words).expect("host memory holds the blocks"),
        &clock,
    );
    entropy.random().expect("the first read does not wait");
    let first = entropy.last.expect("a read records its sample");
    entropy.random().expect("the second read keeps the gap");
    let second = entropy.last.expect("a read records its sample");
    assert!(u64::from(second.wrapping_sub(first)) >= READ_GAP.as_micros());
    // A counter that wrapped round to the last read's sample reads "too
    // soon": the read waits one gap more, never less.
    clock.advance(u32::MAX - 3);
    let before = clock.now();
    entropy.random().expect("a later read keeps the gap");
    assert!(u64::from(clock.now().wrapping_sub(before)) >= READ_GAP.as_micros());
}

#[test]
fn a_running_generator_is_healthy_only_without_a_health_error() {
    let mut trng = memory(instances::trng::SIZE, 0);
    let mut clock_words = memory(instances::lp_periclkrst::SIZE, 0);
    let clock = VirtualCounter::new(0, 1);
    let entropy = Entropy::new(
        EntropyRegisters::host(&mut trng, &mut clock_words).expect("host memory holds the blocks"),
        &clock,
    );
    assert!(
        !entropy.status().is_healthy(),
        "a stopped generator is not healthy"
    );
}

#[test]
fn a_read_sooner_than_the_gap_waits_it_out_and_a_later_one_does_not_wait() {
    /// A hundred cycles a microsecond, one cycle each sample: the gap is
    /// 200 samples, far more than a read takes.
    struct Fast(VirtualCounter);
    impl oer_portable_time::settle::SettleCounter for Fast {
        fn cycles(&self, limit: oer_portable_time::settle::SettleLimit) -> u32 {
            limit.micros() * 100
        }
        fn sample(&self) -> u32 {
            self.0.sample()
        }
    }
    let gap = u32::try_from(READ_GAP.as_micros() * 100).expect("a test gap fits");
    let mut trng = memory(instances::trng::SIZE, HEALTHY);
    let mut clock_words = memory(instances::lp_periclkrst::SIZE, 0);
    let counter = Fast(VirtualCounter::new(1_000, 1));
    let mut entropy = Entropy::new(
        EntropyRegisters::host(&mut trng, &mut clock_words).expect("host memory holds the blocks"),
        &counter,
    );
    entropy.random().expect("the first read does not wait");
    let first = entropy.last.expect("a read records its sample");
    // Less than the gap passes before the next read.
    counter.0.advance(gap / 4);
    entropy.random().expect("the second read waits out the gap");
    let second = entropy.last.expect("a read records its sample");
    assert!(
        second.wrapping_sub(first) >= gap,
        "the second read came {} cycles after the first",
        second.wrapping_sub(first)
    );
    // More than the gap passes: the next read does not wait.
    counter.0.advance(gap * 5);
    let before = counter.0.now();
    entropy.random().expect("a later read does not wait");
    assert!(
        counter.0.now().wrapping_sub(before) <= 4,
        "a read after the gap took {} samples",
        counter.0.now().wrapping_sub(before)
    );
}
