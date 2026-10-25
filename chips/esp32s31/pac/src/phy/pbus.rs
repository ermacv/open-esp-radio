//! Ownership-bound access to the recovered ESP32-S31 PHY PBus.
//!
//! Register identities, multifunction fields and packed result windows come
//! from complete rev0 ROM PBus and clock-force bodies cited in the SVD.

#![forbid(unsafe_code)]

use crate::RadioPhyRegisters;
use crate::generated::{
    PbusEnableState, PbusForceTestInput, PbusForceTxRxState, PbusTxClockPairState,
};

const fn pbus_enable_state(enabled: bool) -> PbusEnableState {
    if enabled {
        PbusEnableState::Enabled
    } else {
        PbusEnableState::Disabled
    }
}

impl RadioPhyRegisters {
    /// Publish one of the four reviewed force-TX/RX transition phases.
    pub fn set_pbus_force_txrx_mode(&mut self, enabled: bool, initial_phase: bool) {
        let state = match (enabled, initial_phase) {
            (true, true) => PbusForceTxRxState::EnabledInitial,
            (true, false) => PbusForceTxRxState::EnabledFinal,
            (false, true) => PbusForceTxRxState::DisabledInitial,
            (false, false) => PbusForceTxRxState::DisabledFinal,
        };
        self.peripherals.phy_pbus.set_pbus_force_txrx_state(state);
    }

    /// Enable or disable the PBus work-mode selector.
    pub fn set_pbus_work_mode(&mut self, enabled: bool) {
        self.peripherals
            .phy_pbus
            .set_pbus_work_mode(pbus_enable_state(enabled));
    }

    /// Enable or disable the PBus debug-mode selector.
    pub fn set_pbus_debug_mode(&mut self, enabled: bool) {
        self.peripherals
            .phy_pbus
            .set_pbus_debug_mode(pbus_enable_state(enabled));
    }

    /// Sample the PBus busy bit exactly once.
    pub fn pbus_is_busy(&mut self) -> bool {
        self.peripherals.phy_pbus.read_status_clock_force().busy()
    }

    /// Publish one instruction-exact ROM force-test command in a fresh RMW edge.
    ///
    /// Like ROM, selector and path contribute only the bits retained by the
    /// combined argument mask. The value is also deliberately not narrowed:
    /// RX-DCO passes signed halfword images, and the complete encoder retains
    /// their low eleven bits while composing the physical command word.
    pub fn publish_pbus_force_test(&mut self, selector: u8, path: u8, test_value: u16) {
        self.peripherals
            .phy_pbus
            .publish_pbus_force_test(PbusForceTestInput::compose(selector, path, test_value));
    }

    /// Clear the force-test transaction bit after one completed observation.
    pub fn clear_pbus_transaction(&mut self) {
        self.peripherals.phy_pbus.clear_pbus_transaction();
    }

    /// Read the packed nine-bit result selected by the recovered ROM tables.
    pub fn read_pbus_result(&mut self, selector: u8, path: u8) -> Option<u16> {
        let path_one = path == 1;
        Some(crate::field_halfword(match selector {
            0 if path_one => self
                .peripherals
                .phy_pbus
                .read_read_result_3()
                .bits_18_26_opaque(),
            0 => self
                .peripherals
                .phy_pbus
                .read_read_result_3()
                .bits_9_17_opaque(),
            1 if path_one => self
                .peripherals
                .phy_pbus
                .read_read_result_0()
                .bits_9_17_opaque(),
            1 => self
                .peripherals
                .phy_pbus
                .read_read_result_0()
                .result_window_0_rx_dco(),
            2 if path_one => self
                .peripherals
                .phy_pbus
                .read_read_result_1()
                .bits_0_8_opaque(),
            2 => self
                .peripherals
                .phy_pbus
                .read_read_result_2()
                .bits_18_26_opaque(),
            3 if path_one => self
                .peripherals
                .phy_pbus
                .read_read_result_2()
                .bits_9_17_opaque(),
            3 => self
                .peripherals
                .phy_pbus
                .read_read_result_2()
                .bits_0_8_opaque(),
            4 if path_one => self
                .peripherals
                .phy_pbus
                .read_read_result_3()
                .bits_0_8_opaque(),
            4 => self
                .peripherals
                .phy_pbus
                .read_read_result_4()
                .bits_18_26_opaque(),
            5 => self
                .peripherals
                .phy_pbus
                .read_read_result_4()
                .bits_9_17_opaque(),
            _ => self
                .peripherals
                .phy_pbus
                .read_read_result_4()
                .bits_0_8_opaque(),
        }))
    }

    /// Enable or disable both recovered RX clock bits as one pair.
    pub fn set_pbus_rx_clock_pair(&mut self, enabled: bool) {
        self.peripherals
            .phy_pbus
            .set_pbus_rx_clock_pair(pbus_enable_state(enabled));
    }

    /// Enable or disable both recovered TX clock bits as one pair.
    pub fn set_pbus_tx_clock_pair(&mut self, enabled: bool) {
        let state = if enabled {
            PbusTxClockPairState::Enabled
        } else {
            PbusTxClockPairState::Disabled
        };
        self.peripherals.phy_pbus.set_pbus_tx_clock_pair(state);
    }
}
