//! The platform owners over host memory: each test fills the blocks with one
//! pattern and checks what the owner reads, never where a word lies.

use std::{vec, vec::Vec};

use super::{
    EfuseRegisters, Entropy, EntropyRegisters, EntropyStatus, FactoryIdentity, HpCore, ResetCause,
    ResetCauseRegisters, ResetCauses,
};
use oer_esp32s31_pac::{efuse, lp_aon_reset_cause, lp_periclkrst_rng, trng};
use oer_time_virtual::VirtualClock;

fn memory(size: usize, fill: u32) -> Vec<u32> {
    vec![fill; size / 4]
}

#[test]
fn the_factory_mac_takes_the_low_word_whole_and_splits_the_high_word() {
    let mut words = memory(efuse::SIZE, 0xa1b2_c3d4);
    let mac = FactoryIdentity::read(
        EfuseRegisters::host(&mut words).expect("host memory holds the block"),
    )
    .mac;
    assert_eq!(mac.mac_0, 0xa1b2_c3d4);
    assert_eq!(
        mac.mac_1, 0xc3d4,
        "the high MAC bits are the word's low half"
    );
    assert_eq!(mac.mac_ext, 0xa1b2, "the extension is the word's high half");
    assert_eq!(mac.base_mac(), [0xc3, 0xd4, 0xa1, 0xb2, 0xc3, 0xd4]);
}

#[test]
fn a_generator_held_in_reset_with_bypassed_tests_is_not_healthy() {
    let mut trng_words = memory(trng::SIZE, u32::MAX);
    let mut clock_words = memory(lp_periclkrst_rng::SIZE, u32::MAX);
    let registers = EntropyRegisters::host(&mut trng_words, &mut clock_words)
        .expect("host memory holds the blocks");
    let mut entropy = Entropy::new(registers, VirtualClock::<0>::new());
    let status = entropy.status();
    assert_eq!(
        status,
        EntropyStatus {
            clocked: false,
            sampling: true,
            health_tested: false,
            health_error: true,
        }
    );
    assert!(!status.is_healthy());
    assert_eq!(entropy.random(), u32::MAX);
}

#[test]
fn a_stopped_generator_reports_no_clock_and_no_sampling() {
    let mut trng_words = memory(trng::SIZE, 0);
    let mut clock_words = memory(lp_periclkrst_rng::SIZE, 0);
    let status = Entropy::new(
        EntropyRegisters::host(&mut trng_words, &mut clock_words)
            .expect("host memory holds the blocks"),
        VirtualClock::<0>::new(),
    )
    .status();
    assert!(!status.clocked && !status.sampling && !status.health_tested);
    assert!(!status.health_error);
}

#[test]
fn each_core_reads_its_own_six_bit_cause() {
    let mut words = memory(lp_aon_reset_cause::SIZE, u32::MAX);
    let causes = ResetCauses::read(
        ResetCauseRegisters::host(&mut words).expect("host memory holds the block"),
    );
    assert_eq!(causes.of(HpCore::Core0), ResetCause::Other(0x3f));
    assert_eq!(causes.of(HpCore::Core1), ResetCause::Other(0x3f));
    let mut words = memory(lp_aon_reset_cause::SIZE, 0b11);
    let causes = ResetCauses::read(
        ResetCauseRegisters::host(&mut words).expect("host memory holds the block"),
    );
    assert_eq!(
        causes.of(HpCore::Core0),
        ResetCause::ChipPowerOn,
        "the flag bit is not part of the cause"
    );
}
