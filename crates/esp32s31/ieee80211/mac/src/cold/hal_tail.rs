//! Ownership boundary for the direct pre-COEX tail of `hal_init`.

use oer_esp32s31_hal::{
    ieee80211::mac::WifiMacColdHal,
    types::{MacInterruptMask, MacRtcCalibration},
};

/// Availability of the platform slow-clock calibration used by the MAC RTC.
///
/// `Unavailable` is intentionally distinct from a measured zero.  The current
/// ESP32-S31 platform cannot obtain this value, and must not silently promote
/// its placeholder to calibrated hardware knowledge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacSlowClockCalibration {
    Unavailable,
    Calibrated(MacRtcCalibration),
}

impl MacSlowClockCalibration {
    /// Preserve the exact truncation performed by `hal_timer_update_by_rtc`.
    pub const fn from_osi_value(value: u32) -> Self {
        Self::Calibrated(MacRtcCalibration::from_osi_value(value))
    }

    /// Convert the explicit platform state to the value written by the
    /// currently recovered vendor-compatible transaction.
    ///
    /// The vendor OS adapter returns zero when no calibration source exists.
    /// Keeping that mapping here preserves the observed transaction while the
    /// enum prevents callers from treating the zero as reviewed calibration.
    pub const fn register_value(self) -> MacRtcCalibration {
        match self {
            Self::Unavailable => MacRtcCalibration::ZERO,
            Self::Calibrated(value) => value,
        }
    }
}

pub trait MacColdHalTailHardware {
    fn initialize_hal_tail(
        &mut self,
        event_mask: MacInterruptMask,
        slow_clock_calibration: MacSlowClockCalibration,
    );
}

impl MacColdHalTailHardware for WifiMacColdHal<'_> {
    fn initialize_hal_tail(
        &mut self,
        event_mask: MacInterruptMask,
        slow_clock_calibration: MacSlowClockCalibration,
    ) {
        WifiMacColdHal::initialize_hal_tail(
            self,
            event_mask,
            slow_clock_calibration.register_value(),
        );
    }
}
