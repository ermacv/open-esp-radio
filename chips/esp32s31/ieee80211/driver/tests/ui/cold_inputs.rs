#![no_std]
use oer_esp32s31_hal::{
    ieee80211::mac::WifiMacColdHal,
    types::{MacDelaySlot, MacRtcCalibration},
};

pub fn cold_inputs(hardware: &mut WifiMacColdHal<'_>, random: u32, calibration: u32) {
    #[cfg(ui_reject)]
    hardware.initialize_txrx_callbacks(random as u8); // ui-error: E0308
    #[cfg(not(ui_reject))]
    hardware.initialize_txrx_callbacks(MacDelaySlot::from_random(random));

    #[cfg(ui_reject)]
    hardware.initialize_hal_tail(calibration); // ui-error: E0308
    #[cfg(not(ui_reject))]
    hardware.initialize_hal_tail(MacRtcCalibration::from_osi_value(calibration));
}
