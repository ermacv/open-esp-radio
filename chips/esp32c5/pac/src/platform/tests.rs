//! The platform owners over host memory: each test fills the blocks with one
//! pattern and checks what the owner reads or leaves, never where a word
//! lies.

use std::{vec, vec::Vec};

use super::{EntropyRegisters, ResetCauseRegisters};
use crate::generated::{lp_clkrst_reset_cause, lpperi_rng};

fn memory(size: usize, fill: u32) -> Vec<u32> {
    vec![fill; size / 4]
}

#[test]
fn enabling_a_stopped_generator_configures_it_as_esp_idf_does() {
    let mut words = memory(lpperi_rng::SIZE, 0);
    EntropyRegisters::host(&mut words)
        .expect("host memory holds the block")
        .enable();
    let block = lpperi_rng::LpperiRng::host(&mut words).expect("host memory holds the block");
    assert!(block.read_clk_en().rng_ck_en(), "the generator is clocked");
    let config = block.read_rng_cfg();
    assert!(config.rng_sample_enable(), "the oscillator is sampled");
    assert_eq!(config.rng_timer_pscale(), 255);
    assert!(config.rng_timer_en(), "the generator timer is mixed in");
    assert_eq!(
        config.rtc_timer_en().get(),
        0b11,
        "the RTC timer is mixed in before and after the CRC"
    );
}

#[test]
fn enabling_only_sets_and_keeps_every_other_bit() {
    let mut words = memory(lpperi_rng::SIZE, u32::MAX);
    EntropyRegisters::host(&mut words)
        .expect("host memory holds the block")
        .enable();
    assert!(
        words.iter().all(|&word| word == u32::MAX),
        "no other clock gate or bit was cleared"
    );
}

#[test]
fn the_generator_reads_its_synchronized_output() {
    let mut words = memory(lpperi_rng::SIZE, 0xa5c3_0f96);
    let registers = EntropyRegisters::host(&mut words).expect("host memory holds the block");
    assert_eq!(registers.random_word(), 0xa5c3_0f96);
}

#[test]
fn the_cause_is_the_five_low_bits_without_the_flag() {
    let mut words = memory(lp_clkrst_reset_cause::SIZE, u32::MAX);
    let registers = ResetCauseRegisters::host(&mut words).expect("host memory holds the block");
    assert_eq!(registers.cause_code(), 0x1f);
    let mut words = memory(lp_clkrst_reset_cause::SIZE, 0b10_0011);
    let registers = ResetCauseRegisters::host(&mut words).expect("host memory holds the block");
    assert_eq!(
        registers.cause_code(),
        0x03,
        "the flag bit is not part of the cause"
    );
}
