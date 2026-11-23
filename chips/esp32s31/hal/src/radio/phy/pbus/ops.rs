//! PBus register operations over the generated PHY PBus block.
//!
//! Register identities, multifunction fields and packed result windows come
//! from complete rev0 ROM PBus and clock-force bodies cited in the SVD.

#![forbid(unsafe_code)]

use oer_chips_esp32s31_pac::fe::{PhyPbus, status_clock_force};
use oer_portable_mmio::Placement;

/// Publish one of the four reviewed force-TX/RX transition phases.
///
/// SOURCE: ROM_REV0_PHY_CLOCK_FORCE.
pub(crate) fn set_force_txrx_mode<P: Placement>(
    pbus: &mut PhyPbus<P>,
    phase: crate::types::PhyForceTxRxPhase,
) {
    pbus.modify_status_clock_force(|w| {
        w.set_bits_8_11_opaque(phase);
    });
}

/// Enable or disable the PBus work-mode selector.
///
/// SOURCE: ROM_REV0_PHY_PBUS.
#[cfg(target_arch = "riscv32")]
pub(crate) fn set_work_mode<P: Placement>(
    pbus: &mut PhyPbus<P>,
    mode: oer_chips_esp32s31_pac::fe::mode::WorkModeEnable,
) {
    pbus.modify_mode(|w| {
        w.set_work_mode_enable(mode);
    });
}

/// Enable or disable the PBus debug-mode selector.
///
/// SOURCE: ROM_REV0_PHY_PBUS.
#[cfg(target_arch = "riscv32")]
pub(crate) fn set_debug_mode<P: Placement>(
    pbus: &mut PhyPbus<P>,
    mode: oer_chips_esp32s31_pac::fe::pbus_command::DebugModeEnable,
) {
    pbus.modify_pbus_command(|w| {
        w.set_debug_mode_enable(mode);
    });
}

/// Sample the PBus busy bit exactly once.
#[cfg(target_arch = "riscv32")]
pub(crate) fn is_busy<P: Placement>(pbus: &PhyPbus<P>) -> bool {
    pbus.read_status_clock_force().busy()
}

/// Publish one instruction-exact ROM force-test command in one fresh RMW.
///
/// SOURCE: ROM_REV0_PHY_PBUS, ROM_REV0_PHY_RX_DCO. ROM composes the command
/// as `selector << 2 | test_value << 6 | path << 15` under the argument mask
/// of bits 16:2 and sets the transaction bit. No argument is narrowed first,
/// so the wider arguments overlap the next field up: the selector's high
/// nibble lands on the test value's low bits, and the test value's bits
/// above its nine-bit field land on the path. RX-DCO passes signed halfword
/// images, whose high bits take that path.
pub(crate) fn publish_force_test<P: Placement>(
    pbus: &mut PhyPbus<P>,
    selector: u8,
    path: u8,
    test_value: u16,
) {
    // The ROM's own composition: each field takes its bits of this word.
    let command = u32::from(selector) << 2 | u32::from(test_value) << 6 | u32::from(path) << 15;
    pbus.modify_pbus_command(|w| {
        w.set_transaction_start(true)
            .set_selector_of_command(command)
            .set_test_value_of_command(command)
            .set_path_of_command(command);
    });
}

/// Clear the force-test transaction bit after one completed observation.
///
/// SOURCE: ROM_REV0_PHY_PBUS.
#[cfg(target_arch = "riscv32")]
pub(crate) fn clear_transaction<P: Placement>(pbus: &mut PhyPbus<P>) {
    pbus.modify_pbus_command(|w| {
        w.set_transaction_start(false);
    });
}

/// Read the packed nine-bit result selected by the recovered ROM tables.
#[cfg(target_arch = "riscv32")]
pub(crate) fn read_result<P: Placement>(pbus: &PhyPbus<P>, selector: u8, path: u8) -> u16 {
    let path_one = path == 1;
    let bits: oer_portable_mmio::Bits<9> = match selector {
        0 if path_one => pbus.read_read_result_3().bits_18_26_opaque(),
        0 => pbus.read_read_result_3().bits_9_17_opaque(),
        1 if path_one => pbus.read_read_result_0().bits_9_17_opaque(),
        1 => pbus.read_read_result_0().result_window_0_rx_dco(),
        2 if path_one => pbus.read_read_result_1().bits_0_8_opaque(),
        2 => pbus.read_read_result_2().bits_18_26_opaque(),
        3 if path_one => pbus.read_read_result_2().bits_9_17_opaque(),
        3 => pbus.read_read_result_2().bits_0_8_opaque(),
        4 if path_one => pbus.read_read_result_3().bits_0_8_opaque(),
        4 => pbus.read_read_result_4().bits_18_26_opaque(),
        5 => pbus.read_read_result_4().bits_9_17_opaque(),
        _ => pbus.read_read_result_4().bits_0_8_opaque(),
    };
    let [low, high, _, _] = bits.get().to_le_bytes();
    u16::from_le_bytes([low, high])
}

/// Enable or disable both recovered RX clock bits as one pair.
///
/// SOURCE: ROM_REV0_PHY_CLOCK_FORCE.
pub(crate) fn set_rx_clock_pair<P: Placement>(pbus: &mut PhyPbus<P>, pair: super::PhyClockPair) {
    use status_clock_force::{Bit14Opaque, Bit15Opaque};
    let (first, second) = match pair {
        super::PhyClockPair::Enabled => (Bit14Opaque::Enabled, Bit15Opaque::Enabled),
        super::PhyClockPair::Disabled => (Bit14Opaque::Disabled, Bit15Opaque::Disabled),
    };
    pbus.modify_status_clock_force(|w| {
        w.set_bit_14_opaque(first).set_bit_15_opaque(second);
    });
}

/// Enable or disable both recovered TX clock bits as one pair.
///
/// SOURCE: ROM_REV0_PHY_CLOCK_FORCE.
pub(crate) fn set_tx_clock_pair<P: Placement>(pbus: &mut PhyPbus<P>, pair: super::PhyClockPair) {
    use status_clock_force::TxClockEnablePair as Pair;
    pbus.modify_status_clock_force(|w| {
        w.set_tx_clock_enable_pair(match pair {
            super::PhyClockPair::Enabled => Pair::Enabled,
            super::PhyClockPair::Disabled => Pair::Disabled,
        });
    });
}

/// Publish both RXIQ root status bits through independent fresh RMWs.
#[cfg(target_arch = "riscv32")]
pub(crate) fn configure_rxiq_root_status<P: Placement>(pbus: &mut PhyPbus<P>) {
    // SOURCE: BLOB_LIBPHY_PHY_RXIQ_CAL_INIT, S31_ESP_PACS_SVD.
    pbus.modify_status_clock_force(|w| {
        w.set_bit_14_opaque(oer_chips_esp32s31_pac::fe::status_clock_force::Bit14Opaque::Enabled);
    });
    // SOURCE: BLOB_LIBPHY_PHY_RXIQ_CAL_INIT, S31_ESP_PACS_SVD.
    pbus.modify_status_clock_force(|w| {
        w.set_bit_15_opaque(oer_chips_esp32s31_pac::fe::status_clock_force::Bit15Opaque::Enabled);
    });
}

/// Clear the second RXIQ root status bit, the suffix's last edge.
#[cfg(target_arch = "riscv32")]
pub(crate) fn clear_rxiq_root_status_second<P: Placement>(pbus: &mut PhyPbus<P>) {
    // SOURCE: BLOB_LIBPHY_PHY_RXIQ_CAL_INIT, S31_ESP_PACS_SVD.
    pbus.modify_status_clock_force(|w| {
        w.set_bit_15_opaque(oer_chips_esp32s31_pac::fe::status_clock_force::Bit15Opaque::Disabled);
    });
}

/// The PBus edge that opens complete rev0 ROM `phy_fe_reg_init`.
#[cfg(target_arch = "riscv32")]
pub(crate) fn initialize_front_end<P: Placement>(pbus: &mut PhyPbus<P>) {
    // SOURCE: ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
    pbus.modify_read_result_0(|w| {
        w.set_bit_20_opaque(true);
    });
}
