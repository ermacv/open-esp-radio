//! ESP32-S31 system power-detector control used by the open PHY.
//!
//! `LP_AON_CLKRST` belongs to the shared route-owned PHY PAC partition. This
//! module exposes only the two encodings evidenced by complete ROM code.

use oer_esp32s31_pac::lp_aonclkrst::LpAonClkrst;
use oer_mmio::{Bits, Placement};

/// Complete power-detector initialization and enable bodies select circuit
/// mode four.
///
/// SOURCE: ROM_REV0_PHY_POWER_DETECTOR.
const POWER_DETECTOR_INITIALIZATION_MODE: Bits<3> = Bits::constant::<4>();

/// Complete TX-calibration debug mode selects circuit mode two.
///
/// SOURCE: ROM_REV0_PHY_POWER_DETECTOR.
const POWER_DETECTOR_CALIBRATION_MODE: Bits<3> = Bits::constant::<2>();

/// Select the power-detector circuit mode of initialization and enable.
pub fn select_circuit_initialization_mode<P: Placement>(aon: &mut LpAonClkrst<P>) {
    aon.modify_rtc_sar2_pwdet_cct(|w| {
        w.set_rtc_sar2_pwdet_cct(POWER_DETECTOR_INITIALIZATION_MODE);
    });
}

/// Select the power-detector circuit mode of TX-calibration debug mode.
pub fn select_circuit_calibration_mode<P: Placement>(aon: &mut LpAonClkrst<P>) {
    aon.modify_rtc_sar2_pwdet_cct(|w| {
        w.set_rtc_sar2_pwdet_cct(POWER_DETECTOR_CALIBRATION_MODE);
    });
}

/// Platform capability for `LP_AON_CLKRST.RTC_SAR2_PWDET_CCT`.
pub trait PhyPowerDetectorPlatformControl {
    /// Select encoding four, used by PWDET register initialization and enable.
    fn select_power_detector_initialization_mode(&mut self);

    /// Select encoding two, used by TX-calibration debug mode.
    fn select_power_detector_calibration_mode(&mut self);
}

impl<T: crate::owner::SharedPhyAccess> PhyPowerDetectorPlatformControl for T {
    fn select_power_detector_initialization_mode(&mut self) {
        select_circuit_initialization_mode(&mut crate::phy_blocks_mut(self).lp_aon_clkrst);
    }

    fn select_power_detector_calibration_mode(&mut self) {
        select_circuit_calibration_mode(&mut crate::phy_blocks_mut(self).lp_aon_clkrst);
    }
}

/// Apply the final system-register edge of complete ROM `phy_pwdet_reg_init`.
///
/// SOURCE\[rev0 ROM `phy_pwdet_reg_init` at `0x2f82_634a`, size `0x5c`].
pub fn select_initialization_mode(platform: &mut impl PhyPowerDetectorPlatformControl) {
    platform.select_power_detector_initialization_mode();
}

/// Apply the final system-register edge of complete ROM `phy_en_pwdet` through
/// its complete `phy_pwdet_sar2_init` callee.
///
/// SOURCE\[rev0 ROM `phy_pwdet_sar2_init` at `0x2f82_63a6`, size `0x34`].
pub fn select_enabled_mode(platform: &mut impl PhyPowerDetectorPlatformControl) {
    platform.select_power_detector_initialization_mode();
}

/// Apply the system-register edge of complete ROM `phy_txcal_debuge_mode_`.
///
/// SOURCE\[rev0 ROM `phy_txcal_debuge_mode_` at `0x2f82_44fe`, size `0x56`].
pub fn select_calibration_mode(platform: &mut impl PhyPowerDetectorPlatformControl) {
    platform.select_power_detector_calibration_mode();
}

#[cfg(test)]
mod tests;
