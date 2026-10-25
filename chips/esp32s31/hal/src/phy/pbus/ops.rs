//! PBus register operations over the generated PHY PBus block.
//!
//! Register identities, multifunction fields and packed result windows come
//! from complete rev0 ROM PBus and clock-force bodies cited in the SVD.

#![forbid(unsafe_code)]

use oer_esp32s31_pac::phy_pbus::{PhyPbus, status_clock_force};
use oer_mmio::{Bits, Placement};

/// Publish one of the four reviewed force-TX/RX transition phases.
///
/// SOURCE: ROM_REV0_PHY_CLOCK_FORCE.
pub fn set_force_txrx_mode<P: Placement>(
    pbus: &mut PhyPbus<P>,
    enabled: bool,
    initial_phase: bool,
) {
    use status_clock_force::Bits8_11Opaque as Phase;
    let phase = match (enabled, initial_phase) {
        (true, true) => Phase::EnableInitialPhase,
        (true, false) => Phase::EnableFinalPhase,
        (false, true) => Phase::DisableInitialPhase,
        (false, false) => Phase::DisableFinalPhase,
    };
    pbus.modify_status_clock_force(|w| {
        w.set_bits_8_11_opaque(phase);
    });
}

/// Enable or disable the PBus work-mode selector.
///
/// SOURCE: ROM_REV0_PHY_PBUS.
pub fn set_work_mode<P: Placement>(pbus: &mut PhyPbus<P>, enabled: bool) {
    pbus.modify_mode(|w| {
        w.set_work_mode_enable(enabled);
    });
}

/// Enable or disable the PBus debug-mode selector.
///
/// SOURCE: ROM_REV0_PHY_PBUS.
pub fn set_debug_mode<P: Placement>(pbus: &mut PhyPbus<P>, enabled: bool) {
    pbus.modify_command(|w| {
        w.set_debug_mode_enable(enabled);
    });
}

/// Sample the PBus busy bit exactly once.
pub fn is_busy<P: Placement>(pbus: &PhyPbus<P>) -> bool {
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
pub fn publish_force_test<P: Placement>(
    pbus: &mut PhyPbus<P>,
    selector: u8,
    path: u8,
    test_value: u16,
) {
    let selector = u32::from(selector);
    let test_value = u32::from(test_value);
    let test_field = test_value | (selector >> 4);
    let path_field = u32::from(path) | (test_value >> 9);
    pbus.modify_command(|w| {
        w.set_transaction_start(true)
            .set_selector(Bits::truncate(selector))
            .set_test_value(Bits::truncate(test_field))
            .set_path(Bits::truncate(path_field));
    });
}

/// Clear the force-test transaction bit after one completed observation.
///
/// SOURCE: ROM_REV0_PHY_PBUS.
pub fn clear_transaction<P: Placement>(pbus: &mut PhyPbus<P>) {
    pbus.modify_command(|w| {
        w.set_transaction_start(false);
    });
}

/// Read the packed nine-bit result selected by the recovered ROM tables.
pub fn read_result<P: Placement>(pbus: &PhyPbus<P>, selector: u8, path: u8) -> u16 {
    let path_one = path == 1;
    let bits: Bits<9> = match selector {
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
pub fn set_rx_clock_pair<P: Placement>(pbus: &mut PhyPbus<P>, enabled: bool) {
    pbus.modify_status_clock_force(|w| {
        w.set_bit_14_opaque(enabled).set_bit_15_opaque(enabled);
    });
}

/// Enable or disable both recovered TX clock bits as one pair.
///
/// SOURCE: ROM_REV0_PHY_CLOCK_FORCE.
pub fn set_tx_clock_pair<P: Placement>(pbus: &mut PhyPbus<P>, enabled: bool) {
    use status_clock_force::TxClockEnablePair as Pair;
    pbus.modify_status_clock_force(|w| {
        w.set_tx_clock_enable_pair(if enabled {
            Pair::Enabled
        } else {
            Pair::Disabled
        });
    });
}

/// Publish both RXIQ root status bits through independent fresh RMWs.
pub fn configure_rxiq_root_status<P: Placement>(pbus: &mut PhyPbus<P>) {
    // SOURCE: BLOB_LIBPHY_PHY_RXIQ_CAL_INIT, S31_ESP_PACS_SVD.
    pbus.modify_status_clock_force(|w| {
        w.set_bit_14_opaque(true);
    });
    // SOURCE: BLOB_LIBPHY_PHY_RXIQ_CAL_INIT, S31_ESP_PACS_SVD.
    pbus.modify_status_clock_force(|w| {
        w.set_bit_15_opaque(true);
    });
}

/// Clear the second RXIQ root status bit, the suffix's last edge.
pub fn clear_rxiq_root_status_second<P: Placement>(pbus: &mut PhyPbus<P>) {
    // SOURCE: BLOB_LIBPHY_PHY_RXIQ_CAL_INIT, S31_ESP_PACS_SVD.
    pbus.modify_status_clock_force(|w| {
        w.set_bit_15_opaque(false);
    });
}

/// The PBus edge that opens complete rev0 ROM `phy_fe_reg_init`.
pub fn initialize_front_end<P: Placement>(pbus: &mut PhyPbus<P>) {
    // SOURCE: ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
    pbus.modify_read_result_0(|w| {
        w.set_bit_20_opaque(true);
    });
}
