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
        crate::generated::set_pbus_force_txrx_state(&self.peripherals.phy_pbus, state);
    }

    /// Enable or disable the PBus work-mode selector.
    pub fn set_pbus_work_mode(&mut self, enabled: bool) {
        crate::generated::set_pbus_work_mode(
            &self.peripherals.phy_pbus,
            pbus_enable_state(enabled),
        );
    }

    /// Enable or disable the PBus debug-mode selector.
    pub fn set_pbus_debug_mode(&mut self, enabled: bool) {
        crate::generated::set_pbus_debug_mode(
            &self.peripherals.phy_pbus,
            pbus_enable_state(enabled),
        );
    }

    /// Sample the PBus busy bit exactly once.
    pub fn pbus_is_busy(&mut self) -> bool {
        crate::svd::field_read::observe_pbus_busy(&self.peripherals.phy_pbus)
    }

    /// Publish one instruction-exact ROM force-test command in a fresh RMW edge.
    ///
    /// Like ROM, selector and path contribute only the bits retained by the
    /// combined argument mask. The value is also deliberately not narrowed:
    /// RX-DCO passes signed halfword images, and the complete encoder retains
    /// their low eleven bits while composing the physical command word.
    pub fn publish_pbus_force_test(&mut self, selector: u8, path: u8, test_value: u16) {
        crate::generated::publish_pbus_force_test(
            &self.peripherals.phy_pbus,
            PbusForceTestInput::compose(selector, path, test_value),
        );
    }

    /// Clear the force-test transaction bit after one completed observation.
    pub fn clear_pbus_transaction(&mut self) {
        crate::generated::clear_pbus_transaction(&self.peripherals.phy_pbus);
    }

    /// Read the packed nine-bit result selected by the recovered ROM tables.
    pub fn read_pbus_result(&mut self, selector: u8, path: u8) -> Option<u16> {
        let path_one = path == 1;
        Some(match selector {
            0 if path_one => crate::svd::field_read::observe_pbus_selector0_path1_result(
                &self.peripherals.phy_pbus,
            ),
            0 => crate::svd::field_read::observe_pbus_selector0_other_path_result(
                &self.peripherals.phy_pbus,
            ),
            1 if path_one => crate::svd::field_read::observe_pbus_selector1_path1_result(
                &self.peripherals.phy_pbus,
            ),
            1 => crate::svd::field_read::observe_pbus_selector1_other_path_result(
                &self.peripherals.phy_pbus,
            ),
            2 if path_one => crate::svd::field_read::observe_pbus_selector2_path1_result(
                &self.peripherals.phy_pbus,
            ),
            2 => crate::svd::field_read::observe_pbus_selector2_other_path_result(
                &self.peripherals.phy_pbus,
            ),
            3 if path_one => crate::svd::field_read::observe_pbus_selector3_path1_result(
                &self.peripherals.phy_pbus,
            ),
            3 => crate::svd::field_read::observe_pbus_selector3_other_path_result(
                &self.peripherals.phy_pbus,
            ),
            4 if path_one => crate::svd::field_read::observe_pbus_selector4_path1_result(
                &self.peripherals.phy_pbus,
            ),
            4 => crate::svd::field_read::observe_pbus_selector4_other_path_result(
                &self.peripherals.phy_pbus,
            ),
            5 => crate::svd::field_read::observe_pbus_selector5_result(&self.peripherals.phy_pbus),
            _ => crate::svd::field_read::observe_pbus_fallback_result(&self.peripherals.phy_pbus),
        })
    }

    /// Enable or disable both recovered RX clock bits as one pair.
    pub fn set_pbus_rx_clock_pair(&mut self, enabled: bool) {
        crate::generated::set_pbus_rx_clock_pair(
            &self.peripherals.phy_pbus,
            pbus_enable_state(enabled),
        );
    }

    /// Enable or disable both recovered TX clock bits as one pair.
    pub fn set_pbus_tx_clock_pair(&mut self, enabled: bool) {
        let state = if enabled {
            PbusTxClockPairState::Enabled
        } else {
            PbusTxClockPairState::Disabled
        };
        crate::generated::set_pbus_tx_clock_pair(&self.peripherals.phy_pbus, state);
    }
}
