//! Radio-internal clock-gate operations over the generated PHY clock and
//! modem-syscon blocks.
//!
//! Platform clock/reset and power policy stay in the integration layer.

#![forbid(unsafe_code)]

use oer_esp32s31_pac::{
    modem_syscon_radio::ModemSysconRadio,
    phy_clock_oracle::{self, PhyClockOracle},
};
use oer_mmio::{Bits, Placement};

/// Clear the Wi-Fi enable bit, the only baseband-control bit complete
/// `phy_xpd_rf_new` changes.
///
/// SOURCE: BLOB_LIBPHY_PHY_XPD_RF_NEW_20260907; the vendor leaf performs one
/// preserving update of `WIFI_BB_CFG` that clears bit one alone before its
/// one-microsecond settle.
pub fn clear_rf_baseband_control<P: Placement>(syscon: &mut ModemSysconRadio<P>) {
    syscon.modify_wifi_bb_cfg(|w| {
        w.set_wifi_enable(false);
    });
}

/// Enable or disable the recovered PHY calibration clock.
///
/// SOURCE: BLOB_LIBPHY_PHY_BB_INIT; the complete parent sets this bit
/// before executing the baseband calibration sequence.
pub fn set_calibration_clock<P: Placement>(clock: &mut PhyClockOracle<P>, enabled: bool) {
    clock.modify_word_0800_opaque(|w| {
        w.set_bit_2_opaque(enabled);
    });
}

/// Open the three undocumented front-end and baseband radio clock gates.
///
/// SOURCE: ROM_REV0_PHY_OPEN_FE_BB_CLK; these are the first three
/// operations in the complete ROM body. Its fourth PMU operation belongs to
/// the platform and is sequenced by the caller.
pub fn open_frontend_baseband_internal_clocks<P: Placement>(clock: &mut PhyClockOracle<P>) {
    // SOURCE: ROM_REV0_PHY_OPEN_FE_BB_CLK.
    clock.write_word_0400_opaque(|w| {
        w.set_bits_0_2_opaque(Bits::constant::<0x7>())
            .set_bits_5_8_opaque(Bits::constant::<0xf>());
    });
    // SOURCE: ROM_REV0_PHY_OPEN_FE_BB_CLK.
    clock.modify_word_0800_opaque(|w| {
        w.set_bits_0_1_opaque(phy_clock_oracle::word_0800_opaque::Bits0_1Opaque::Open);
    });
    // SOURCE: ROM_REV0_PHY_OPEN_FE_BB_CLK.
    clock.write_word_7c80_opaque(|w| {
        w.set_state(phy_clock_oracle::word_7c80_opaque::State::Open);
    });
}

/// Close the recovered front-end and baseband clock gates.
///
/// SOURCE: BLOB_LIBPHY_PHY_CLOSE_FE_BB_CLK; this is the exact inverse
/// three-operation blob leaf. It leaves PMU power policy to the surrounding
/// lifecycle owner, matching the source.
pub fn close_frontend_baseband_clocks<P: Placement>(clock: &mut PhyClockOracle<P>) {
    // SOURCE: BLOB_LIBPHY_PHY_CLOSE_FE_BB_CLK.
    clock.write_word_0400_opaque(|w| {
        w.set_bits_0_2_opaque(Bits::constant::<0x0>())
            .set_bits_5_8_opaque(Bits::constant::<0x0>());
    });
    // SOURCE: BLOB_LIBPHY_PHY_CLOSE_FE_BB_CLK.
    clock.modify_word_0800_opaque(|w| {
        w.set_bits_0_1_opaque(phy_clock_oracle::word_0800_opaque::Bits0_1Opaque::Closed);
    });
    // SOURCE: BLOB_LIBPHY_PHY_CLOSE_FE_BB_CLK.
    clock.write_word_7c80_opaque(|w| {
        w.set_state(phy_clock_oracle::word_7c80_opaque::State::Closed);
    });
}
