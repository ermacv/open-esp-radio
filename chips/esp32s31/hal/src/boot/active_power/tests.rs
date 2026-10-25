//! The PLLs' active-state power over host memory from the PMU's base.

use super::{ActivePower, PllPower};
use oer_esp32s31_pac::instances;
use std::{vec, vec::Vec};

fn changed(before: &[u32], after: &[u32]) -> Vec<u32> {
    before
        .iter()
        .zip(after)
        .map(|(before, after)| before ^ after)
        .filter(|bits| *bits != 0)
        .collect()
}

fn memory(fill: u32) -> Vec<u32> {
    vec![fill; instances::pmu::SIZE / 4]
}

#[test]
fn the_pll_power_reads_back_as_written() {
    let mut words = memory(0);
    let mut owner = ActivePower::host(&mut words).expect("memory holds the PMU");
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
        let before = memory(fill);
        let mut after = before.clone();
        let mut owner = ActivePower::host(&mut after).expect("memory holds the PMU");
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
