//! The bootstrap's PLL power fields over host memory.

use super::{BootstrapPllPower, PllPower};
use crate::generated::pmu_radio;
use std::{vec, vec::Vec};

fn changed(before: &[u32], after: &[u32]) -> Vec<u32> {
    before
        .iter()
        .zip(after)
        .map(|(before, after)| before ^ after)
        .filter(|bits| *bits != 0)
        .collect()
}

#[test]
fn the_pll_power_reads_back_as_written() {
    let mut memory = vec![0; pmu_radio::SIZE / 4];
    let mut owner = BootstrapPllPower::host(&mut memory);
    let power = PllPower {
        cpll: true,
        cpll_i2c: false,
        mpll: true,
        mpll_i2c: true,
    };
    owner.set_pll_power(power);
    assert_eq!(owner.pll_power(), power);
}

#[test]
fn setting_the_pll_power_changes_one_word_and_four_bits_at_most() {
    for fill in [0, u32::MAX, 0x5a5a_a5a5] {
        let before = vec![fill; pmu_radio::SIZE / 4];
        let mut after = before.clone();
        let mut owner = BootstrapPllPower::host(&mut after);
        let current = owner.pll_power();
        owner.set_pll_power(PllPower {
            cpll: !current.cpll,
            cpll_i2c: !current.cpll_i2c,
            mpll: !current.mpll,
            mpll_i2c: !current.mpll_i2c,
        });
        drop(owner);
        let difference = changed(&before, &after);
        assert_eq!(difference.len(), 1, "one word changed");
        assert_eq!(
            difference[0].count_ones(),
            4,
            "the four PLL power bits alone"
        );
    }
}

#[test]
fn the_radio_front_end_clock_image_survives_the_bootstrap() {
    // The radio's preserving update of the same word keeps the PLL power
    // the bootstrap set.
    let mut memory = vec![0; pmu_radio::SIZE / 4];
    let mut owner = BootstrapPllPower::host(&mut memory);
    let power = PllPower {
        cpll: true,
        cpll_i2c: true,
        mpll: true,
        mpll_i2c: true,
    };
    owner.set_pll_power(power);
    drop(owner);
    let mut radio = pmu_radio::PmuRadio::host(&mut memory).expect("memory holds the view");
    radio.enable_frontend_baseband_power();
    drop(radio);
    let owner = BootstrapPllPower::host(&mut memory);
    assert_eq!(owner.pll_power(), power);
}
