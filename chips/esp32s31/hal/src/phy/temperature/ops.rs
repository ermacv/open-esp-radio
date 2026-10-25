//! LP temperature-sensor operations over the generated LP blocks of the
//! radio-PHY partition.
//!
//! Only complete ROM/libphy-evidenced transitions and one temperature-code
//! read are offered.

#![forbid(unsafe_code)]

use oer_esp32s31_pac::{lp_peri_clkrst::LpPericlkrst, tsens::LpTsens};
use oer_mmio::Placement;

/// Enable the LP temperature-sensor register bank.
///
/// SOURCE: BLOB_LIBPHY_PHY_TSENS_READ_INIT.
pub fn enable_register_bank<P: Placement>(tsens: &mut LpTsens<P>) {
    tsens.modify_clk_conf(|w| {
        w.set_clk_en(true);
    });
}

/// Enable or disable the LP temperature-sensor peripheral clock.
///
/// SOURCE: BLOB_LIBPHY_PHY_TSENS_READ_INIT.
pub fn set_clock<P: Placement>(periclkrst: &mut LpPericlkrst<P>, enabled: bool) {
    periclkrst.modify_tsens_ctrl(|w| {
        w.set_lp_tsens_clk_en(if enabled {
            oer_esp32s31_pac::lp_peri_clkrst::tsens_ctrl::LpTsensClkEn::Enabled
        } else {
            oer_esp32s31_pac::lp_peri_clkrst::tsens_ctrl::LpTsensClkEn::Disabled
        });
    });
}

/// Assert or release the first-owner TSENS peripheral reset.
///
/// SOURCE: IDF_S31_TSENS_POWER_LIFECYCLE.
pub fn set_reset<P: Placement>(periclkrst: &mut LpPericlkrst<P>, asserted: bool) {
    periclkrst.modify_tsens_ctrl(|w| {
        w.set_lp_tsens_rst_en(asserted);
    });
}

/// Enable the blob-evidenced PHY readout path.
///
/// SOURCE: BLOB_LIBPHY_PHY_TSENS_READ_INIT.
pub fn enable_phy_readout<P: Placement>(tsens: &mut LpTsens<P>) {
    tsens.modify_clk_conf(|w| {
        w.set_bit_23_opaque(true);
    });
}

/// Enable the blob-evidenced PHY conversion path.
///
/// SOURCE: BLOB_LIBPHY_PHY_TSENS_READ_INIT.
pub fn enable_phy_conversion<P: Placement>(tsens: &mut LpTsens<P>) {
    tsens.modify_clk_conf(|w| {
        w.set_bit_9_opaque(true);
    });
}

/// Power the sensor up or down through the complete-ROM-evidenced control
/// bit (`phy_set_tsens_power_`).
///
/// SOURCE: ROM_REV0_PHY_TSENS, BLOB_LIBPHY_PHY_TSENS_READ_INIT.
pub fn set_power<P: Placement>(tsens: &mut LpTsens<P>, enabled: bool) {
    tsens.modify_ctrl(|w| {
        w.set_power_up(if enabled {
            oer_esp32s31_pac::tsens::ctrl::PowerUp::Enabled
        } else {
            oer_esp32s31_pac::tsens::ctrl::PowerUp::Disabled
        });
    });
}

/// Sample and return the unsigned temperature code exactly once.
pub fn read_code<P: Placement>(tsens: &LpTsens<P>) -> u8 {
    tsens.read_ctrl().out()
}
