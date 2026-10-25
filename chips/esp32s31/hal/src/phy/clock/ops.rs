//! Radio-internal clock-gate operations over the generated PHY clock and
//! modem-syscon blocks.
//!
//! Platform clock/reset and power policy stay in the integration layer.

#![forbid(unsafe_code)]

use crate::phy::registers::{ClockRegisters, PhyPlacement};
use oer_esp32s31_pac::{fe, modem_syscon::ModemSysconRadio, wifi_bb};
use oer_mmio::{Bits, Placement};

/// Clear the Wi-Fi enable bit, the only baseband-control bit complete
/// `phy_xpd_rf_new` changes.
///
/// SOURCE: BLOB_LIBPHY_PHY_XPD_RF_NEW_20260907; the vendor leaf performs one
/// preserving update of `WIFI_BB_CFG` that clears bit one alone before its
/// one-microsecond settle.
pub fn clear_rf_baseband_control<P: Placement>(syscon: &mut ModemSysconRadio<P>) {
    syscon.modify_wifi_bb_cfg(|w| {
        w.set_wifi_enable(if false {
            oer_esp32s31_pac::modem_syscon::wifi_bb_cfg::WifiEnable::Enabled
        } else {
            oer_esp32s31_pac::modem_syscon::wifi_bb_cfg::WifiEnable::Disabled
        });
    });
}

/// Enable or disable the recovered PHY calibration clock.
///
/// SOURCE: BLOB_LIBPHY_PHY_BB_INIT; the complete parent sets this bit
/// before executing the baseband calibration sequence.
pub fn set_calibration_clock<P: PhyPlacement>(clock: &mut ClockRegisters<P>, enabled: bool) {
    clock.fe.modify_word_0800_opaque(|w| {
        w.set_bit_2_opaque(enabled);
    });
}

/// Open the three undocumented front-end and baseband radio clock gates.
///
/// SOURCE: ROM_REV0_PHY_OPEN_FE_BB_CLK; these are the first three
/// operations in the complete ROM body. Its fourth PMU operation belongs to
/// the platform and is sequenced by the caller.
pub fn open_frontend_baseband_internal_clocks<P: PhyPlacement>(clock: &mut ClockRegisters<P>) {
    // SOURCE: ROM_REV0_PHY_OPEN_FE_BB_CLK.
    clock.fe.write_word_0400_opaque(|w| {
        w.set_bits_0_2_opaque(Bits::constant::<0x7>())
            .set_bits_5_8_opaque(Bits::constant::<0xf>());
    });
    // SOURCE: ROM_REV0_PHY_OPEN_FE_BB_CLK.
    clock.fe.modify_word_0800_opaque(|w| {
        w.set_bits_0_1_opaque(fe::word_0800_opaque::Bits0_1Opaque::Open);
    });
    // SOURCE: ROM_REV0_PHY_OPEN_FE_BB_CLK.
    clock.wifi_bb.write_word_0c80_opaque(|w| {
        w.set_state(wifi_bb::word_0c80_opaque::State::Open);
    });
}

/// Close the recovered front-end and baseband clock gates.
///
/// SOURCE: BLOB_LIBPHY_PHY_CLOSE_FE_BB_CLK; this is the exact inverse
/// three-operation blob leaf. It leaves PMU power policy to the surrounding
/// lifecycle owner, matching the source.
pub fn close_frontend_baseband_clocks<P: PhyPlacement>(clock: &mut ClockRegisters<P>) {
    // SOURCE: BLOB_LIBPHY_PHY_CLOSE_FE_BB_CLK.
    clock.fe.write_word_0400_opaque(|w| {
        w.set_bits_0_2_opaque(Bits::constant::<0x0>())
            .set_bits_5_8_opaque(Bits::constant::<0x0>());
    });
    // SOURCE: BLOB_LIBPHY_PHY_CLOSE_FE_BB_CLK.
    clock.fe.modify_word_0800_opaque(|w| {
        w.set_bits_0_1_opaque(fe::word_0800_opaque::Bits0_1Opaque::Closed);
    });
    // SOURCE: BLOB_LIBPHY_PHY_CLOSE_FE_BB_CLK.
    clock.wifi_bb.write_word_0c80_opaque(|w| {
        w.set_state(wifi_bb::word_0c80_opaque::State::Closed);
    });
}
