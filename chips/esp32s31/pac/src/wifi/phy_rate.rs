//! Wi-Fi-owned PHY words: the low-rate receive gate and the noise-floor
//! measurement.
//!
//! Only Wi-Fi touches these words, so they belong to the Wi-Fi register set
//! and need no shared radio lease.

#![forbid(unsafe_code)]

use crate::WifiRadioRegisters;
use crate::generated::PhyLowRateState;

const fn phy_low_rate_state(enabled: bool) -> PhyLowRateState {
    if enabled {
        PhyLowRateState::Enabled
    } else {
        PhyLowRateState::Disabled
    }
}

const fn decode_noise_floor_quarter_db(raw_low_twelve: u16) -> i32 {
    // Complete ROM `phy_read_hw_noisefloor` subtracts 0x1000 from the
    // generated twelve-bit field unconditionally and shifts by two.
    let signed_sixteenth_db = raw_low_twelve as i32 - 0x1000;
    signed_sixteenth_db >> 2
}

const fn quarter_db_to_dbm(quarter_db: i32) -> i8 {
    // Complete blob `wDev_GetNoiseFloor` consumes the quarter-dB ROM result,
    // adds two, shifts by two and stores a byte.
    ((quarter_db + 2) >> 2) as i8
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

    /// Read the current hardware noise floor as the signed byte used by MAC
    /// rate control.
    ///
    /// SOURCE: complete rev0 ROM `phy_read_hw_noisefloor` at
    /// `0x2f82_7d72`, size `0x1a`, reads `0x2010_708c[11:0]` and performs the
    /// first arithmetic divide by four. Complete
    /// `libpp[wdev.o]::wDev_GetNoiseFloor`, size `0x36`, applies
    /// `(quarter_db + 2) >> 2` and retains the result as a signed byte.
    pub fn read_noise_floor_dbm(&self) -> i8 {
        quarter_db_to_dbm(self.read_noise_floor_quarter_db())
    }

    /// Read the exact signed quarter-dB result returned by complete rev0 ROM
    /// `phy_read_hw_noisefloor`.
    pub fn read_noise_floor_quarter_db(&self) -> i32 {
        let raw = crate::svd::field_read::observe_phy_noise_floor_sixteenth_db_code(
            &self.peripherals.wifi_mac.wifi_phy_rate_oracle,
        );
        decode_noise_floor_quarter_db(raw)
    }
}

impl WifiRadioRegisters {
    /// The same non-destructive field read as rate control, with its native code retained.
    pub fn noise_floor_snapshot(&self) -> (u16, i32) {
        let raw = crate::svd::field_read::observe_phy_noise_floor_sixteenth_db_code(
            &self.peripherals.wifi_mac.wifi_phy_rate_oracle,
        );
        (raw, decode_noise_floor_quarter_db(raw))
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_noise_floor_quarter_db, quarter_db_to_dbm};

    #[test]
    fn noise_floor_decode_reproduces_both_complete_arithmetic_shifts() {
        // -96 dBm is encoded as -1536 sixteenth-dB, or low twelve bits 0xa00.
        assert_eq!(decode_noise_floor_quarter_db(0x0a00), -384);
        assert_eq!(decode_noise_floor_quarter_db(0x0fff), -1);
        assert_eq!(decode_noise_floor_quarter_db(0x0000), -1024);
        assert_eq!(quarter_db_to_dbm(-384), -96);
        assert_eq!(quarter_db_to_dbm(-1), 0);
        assert_eq!(quarter_db_to_dbm(-1024), 0);
    }
}
