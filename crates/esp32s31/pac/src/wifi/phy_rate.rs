//! Wi-Fi-owned PHY words: the low-rate receive gate and the noise-floor
//! measurement.
//!
//! Only Wi-Fi touches these words, so they belong to the Wi-Fi register set
//! and need no shared radio lease.

#![forbid(unsafe_code)]

use crate::WifiRadioRegisters;
use crate::generated::{PhyLowRateState, PhyRx11bLowRateArgument};

const fn phy_low_rate_state(enabled: bool) -> PhyLowRateState {
    if enabled {
        PhyLowRateState::Enabled
    } else {
        PhyLowRateState::Disabled
    }
}

impl WifiRadioRegisters {
    /// Enable or disable the complete three-edge PHY low-rate path.
    ///
    /// SOURCE: complete rev0 ROM `phy_enable_low_rate` at `0x2f82_5210`
    /// and `phy_disable_low_rate` at `0x2f82_5230`, both size `0x20`.
    /// The two primary-word bits remain separate RMWs exactly as in ROM.
    pub fn configure_phy_low_rate(&mut self, enabled: bool) {
        let rate = &self.peripherals.wifi_mac.wifi_phy_rate_oracle;
        let state = phy_low_rate_state(enabled);
        crate::generated::configure_phy_low_rate_first_state(rate, state);
        crate::generated::configure_phy_low_rate_second_state(rate, state);
        crate::generated::configure_phy_low_rate_secondary_state(rate, state);
    }

    /// Read the complete ROM `phy_is_low_rate_enabled` status bit.
    pub fn phy_low_rate_enabled(&self) -> bool {
        crate::svd::field_read::observe_phy_low_rate_enabled(
            &self.peripherals.wifi_mac.wifi_phy_rate_oracle,
        )
    }

    /// Apply complete rev0 ROM `phy_rx11blr_cfg` without widening the caller
    /// low-bit contract into a boolean ABI.
    pub fn configure_rx_11b_low_rate(&mut self, input: u32) {
        let input = PhyRx11bLowRateArgument::new(input)
            .expect("every u32 is a complete phy_rx11blr_cfg argument");
        let rate = &self.peripherals.wifi_mac.wifi_phy_rate_oracle;
        crate::generated::configure_phy_rx11b_first_low_rate_state(rate, input);
        crate::generated::configure_phy_rx11b_second_low_rate_state(rate, input);
        crate::generated::configure_phy_rx11b_secondary_low_rate_state(rate, input);
    }

    /// Read the current hardware noise floor as the signed byte used by MAC
    /// rate control.
    ///
    /// SOURCE: complete rev0 ROM `phy_read_hw_noisefloor` at
    /// `0x2f82_7d72`, size `0x1a`, reads `0x2010_708c[11:0]` and performs the
    /// first arithmetic divide by four. Complete
    /// `libpp[wdev.o]::wDev_GetNoiseFloor`, size `0x36`, applies
    /// `(quarter_db + 2) >> 2` and retains the result as a signed byte.
    pub fn read_noise_floor_dbm(&self) -> i8 {
        crate::phy::baseband::quarter_db_to_dbm(self.read_noise_floor_quarter_db())
    }

    /// Read the exact signed quarter-dB result returned by complete rev0 ROM
    /// `phy_read_hw_noisefloor`.
    pub fn read_noise_floor_quarter_db(&self) -> i32 {
        let raw = crate::svd::field_read::observe_phy_noise_floor_sixteenth_db_code(
            &self.peripherals.wifi_mac.wifi_phy_rate_oracle,
        );
        crate::phy::baseband::decode_noise_floor_quarter_db(raw)
    }
}
