//! Safe runtime AGC register transactions.

#![forbid(unsafe_code)]

use crate::RadioPhyRegisters;
use crate::generated::AgcRuntimeEnableState;
pub use crate::generated::ForcedRxGain;

fn vendor_register_argument(input: u32) -> crate::generated::PhyVendorRegisterArgument {
    crate::generated::PhyVendorRegisterArgument::new(input)
        .expect("every u32 fits the complete generated vendor-argument domain")
}

const fn agc_runtime_enable_state(enabled: bool) -> AgcRuntimeEnableState {
    if enabled {
        AgcRuntimeEnableState::Enabled
    } else {
        AgcRuntimeEnableState::Disabled
    }
}

impl RadioPhyRegisters {
    /// Apply complete rev0 ROM `phy_ant_dft_cfg`.
    pub fn configure_antenna_diversity(&mut self, enabled: bool) {
        crate::generated::configure_antenna_diversity_state(
            &self.peripherals.phy_agc_oracle,
            agc_runtime_enable_state(enabled),
        );
    }

    /// Apply complete rev0 ROM `phy_force_rx_gain`.
    pub fn configure_forced_rx_gain(&mut self, enabled: bool, gain: ForcedRxGain) {
        let agc = &self.peripherals.phy_agc_oracle;
        crate::generated::configure_forced_rx_gain_value(agc, gain);
        crate::generated::configure_forced_rx_gain_state(agc, agc_runtime_enable_state(enabled));
    }

    /// Apply complete rev0 ROM `phy_force_rx_gain` directly from its vendor
    /// ABI while keeping both field projections inside generated PAC code.
    pub fn configure_forced_rx_gain_from_vendor_arguments(&mut self, enabled: u32, gain: u32) {
        let agc = &self.peripherals.phy_agc_oracle;
        crate::generated::configure_forced_rx_gain_value_from_vendor_argument(
            agc,
            vendor_register_argument(gain),
        );
        crate::generated::configure_forced_rx_gain_state_from_vendor_argument(
            agc,
            vendor_register_argument(enabled),
        );
    }
}
