//! Safe runtime AGC register transactions.

#![forbid(unsafe_code)]

use crate::RadioPhyRegisters;
use crate::generated::AgcRuntimeEnableState;
pub use crate::generated::ForcedRxGain;

const fn agc_runtime_enable_state(enabled: bool) -> AgcRuntimeEnableState {
    if enabled {
        AgcRuntimeEnableState::Enabled
    } else {
        AgcRuntimeEnableState::Disabled
    }
}

impl RadioPhyRegisters {
    /// Apply complete rev0 ROM `phy_force_rx_gain`.
    pub fn configure_forced_rx_gain(&mut self, enabled: bool, gain: ForcedRxGain) {
        let agc = &self.peripherals.phy_agc_oracle;
        crate::generated::configure_forced_rx_gain_value(agc, gain);
        crate::generated::configure_forced_rx_gain_state(agc, agc_runtime_enable_state(enabled));
    }
}
