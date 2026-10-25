//! Wi-Fi-owned PHY words: the low-rate receive gate and the noise-floor
//! measurement.
//!
//! Only Wi-Fi touches these words, so they belong to the Wi-Fi register set
//! and need no shared radio lease.

use super::{MacPlacement, WifiRadioRegisters, halfword};

/// The signed quarter-dB noise floor of the measurement's twelve-bit field.
const fn decode_noise_floor_quarter_db(raw_low_twelve: u16) -> i32 {
    // Complete ROM `phy_read_hw_noisefloor` subtracts 0x1000 from the
    // generated twelve-bit field unconditionally and shifts by two.
    let signed_sixteenth_db = raw_low_twelve as i32 - 0x1000;
    signed_sixteenth_db >> 2
}

/// The signed byte MAC rate control keeps of a quarter-dB noise floor.
const fn quarter_db_to_dbm(quarter_db: i32) -> i8 {
    // Complete blob `wDev_GetNoiseFloor` consumes the quarter-dB ROM result,
    // adds two, shifts by two and stores a byte.
    ((quarter_db + 2) >> 2) as i8
}

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Enable or disable the complete three-edge PHY low-rate path.
    ///
    /// SOURCE: complete rev0 ROM `phy_enable_low_rate` at `0x2f82_5210`
    /// and `phy_disable_low_rate` at `0x2f82_5230`, both size `0x20`
    /// (ROM_REV0_PHY_LOW_RATE). The two primary-word bits remain separate
    /// RMWs exactly as in ROM.
    pub fn configure_phy_low_rate(&mut self, enabled: bool) {
        let rate = &mut self.mac.wifi_phy_rate_oracle;
        rate.modify_low_rate_primary_control(|w| {
            w.set_low_rate_enable_first(enabled);
        });
        rate.modify_low_rate_primary_control(|w| {
            w.set_low_rate_enable_second(enabled);
        });
        rate.modify_low_rate_secondary_control(|w| {
            w.set_low_rate_enable(enabled);
        });
    }

    /// Read the complete ROM `phy_is_low_rate_enabled` status bit.
    pub fn phy_low_rate_enabled(&self) -> bool {
        self.mac
            .wifi_phy_rate_oracle
            .read_low_rate_primary_control()
            .low_rate_enable_first()
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
        decode_noise_floor_quarter_db(self.noise_floor_code())
    }

    /// The same non-destructive field read as rate control, with its native code retained.
    pub fn noise_floor_snapshot(&self) -> (u16, i32) {
        let raw = self.noise_floor_code();
        (raw, decode_noise_floor_quarter_db(raw))
    }

    fn noise_floor_code(&self) -> u16 {
        halfword(
            self.mac
                .wifi_phy_rate_oracle
                .read_noise_floor_measurement()
                .signed_sixteenth_db_code(),
        )
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
