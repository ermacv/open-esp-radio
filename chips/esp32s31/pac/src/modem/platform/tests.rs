use super::{PlatformPllSourceBaseline, WifiPowerBaseline};
use crate::generated::hp_sys_clkrst_radio::{self as clkrst, HpSysClkrstRadio};
use crate::modem::syscon::ModemSysconPowerBaseline;
use std::{vec, vec::Vec};

const BASELINE: PlatformPllSourceBaseline = PlatformPllSourceBaseline {
    modem_apb_clock_enabled: false,
    modem_source_clock_enabled: true,
    modem_pll_selected: false,
    modem_pll_clock_enabled: false,
    modem_xtal_clock_enabled: true,
};

#[test]
fn packed_power_baseline_preserves_every_decoded_field() {
    for bus_clock in [false, true] {
        let baseline =
            WifiPowerBaseline::new(bus_clock, BASELINE, ModemSysconPowerBaseline::default());
        assert_eq!(baseline.modem_register_bus_clock_enabled(), bus_clock);
        assert_eq!(baseline.pll_source(), BASELINE);
        assert_eq!(baseline.modem_syscon(), ModemSysconPowerBaseline::default());
    }
}

#[test]
fn restoring_a_captured_pll_source_leaves_the_word_as_it_was() {
    for fill in [0, u32::MAX, 0x3c3c_c3c3] {
        let before = vec![fill; clkrst::SIZE / 4];
        let mut after = before.clone();
        let mut registers = HpSysClkrstRadio::host(&mut after).expect("memory holds the block");
        PlatformPllSourceBaseline::capture(&registers).restore(&mut registers);
        drop(registers);
        assert_eq!(before, after);
    }
}

#[test]
fn a_restored_pll_source_keeps_the_modem_reset_and_reads_back() {
    let before: Vec<u32> = vec![u32::MAX; clkrst::SIZE / 4];
    let mut after = before.clone();
    let mut registers = HpSysClkrstRadio::host(&mut after).expect("memory holds the block");
    BASELINE.restore(&mut registers);
    assert_eq!(PlatformPllSourceBaseline::capture(&registers), BASELINE);
    assert!(
        registers.read_modem_conf().modem_rst_en(),
        "the reset is kept"
    );
    drop(registers);
    let changed: Vec<u32> = before
        .iter()
        .zip(&after)
        .map(|(before, after)| before ^ after)
        .filter(|bits| *bits != 0)
        .collect();
    assert_eq!(changed.len(), 1);
    assert_eq!(
        changed[0].count_ones(),
        3,
        "the three fields BASELINE clears"
    );
}
