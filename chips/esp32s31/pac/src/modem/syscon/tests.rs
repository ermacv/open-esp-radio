//! The captured clock gates of the modem system controller over host memory.

use super::{calibration_clocks, restore_bluetooth_peripheral_clocks, restore_calibration_clocks};
use crate::generated::modem_syscon_radio::{self as syscon, ModemSysconRadio};
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
fn restoring_the_captured_calibration_clocks_leaves_the_word_as_it_was() {
    for fill in [0, u32::MAX, 0xa5a5_5a5a] {
        let before = vec![fill; syscon::SIZE / 4];
        let mut after = before.clone();
        let mut registers = ModemSysconRadio::host(&mut after).expect("memory holds the block");
        let clocks = calibration_clocks(&registers);
        restore_calibration_clocks(&mut registers, clocks);
        drop(registers);
        assert!(changed(&before, &after).is_empty());
    }
}

#[test]
fn each_restored_calibration_clock_lands_in_its_own_gate_and_one_word() {
    for gate in 0..18 {
        let before = vec![0; syscon::SIZE / 4];
        let mut after = before.clone();
        let mut registers = ModemSysconRadio::host(&mut after).expect("memory holds the block");
        let mut clocks = [false; 18];
        clocks[gate] = true;
        restore_calibration_clocks(&mut registers, clocks);
        assert_eq!(calibration_clocks(&registers), clocks);
        drop(registers);
        let difference = changed(&before, &after);
        assert_eq!(difference.len(), 1);
        assert_eq!(difference[0].count_ones(), 1);
    }
}

#[test]
fn the_bluetooth_peripheral_clocks_change_five_gates_of_one_word() {
    let before = vec![0x0f0f_f0f0; syscon::SIZE / 4];
    let mut after = before.clone();
    let mut registers = ModemSysconRadio::host(&mut after).expect("memory holds the block");
    let current = registers.read_clk_conf();
    restore_bluetooth_peripheral_clocks(
        &mut registers,
        [
            !current.clk_modem_sec_en(),
            !current.clk_modem_sec_ecb_en(),
            !current.clk_modem_sec_ccm_en(),
            !current.clk_modem_sec_bah_en(),
            !current.clk_ble_timer_en(),
        ],
    );
    drop(registers);
    let difference = changed(&before, &after);
    assert_eq!(difference.len(), 1);
    assert_eq!(difference[0].count_ones(), 5);
}
