//! The permanent transfer from esp-hal's radio singletons to the C5 HAL.

use esp_hal::peripherals::{BT, I2C_ANA_MST, IEEE802154, MODEM_LPCON, MODEM_SYSCON, WIFI};
use oer_esp32c5_hal::root::{RadioHandoff, RadioHardware};

/// Every whole-block platform owner consumed by the radio handoff.
pub struct RadioSingletons {
    pub i2c_ana_mst: I2C_ANA_MST<'static>,
    pub modem_syscon: MODEM_SYSCON<'static>,
    pub modem_lpcon: MODEM_LPCON<'static>,
    pub wifi: WIFI<'static>,
    pub bt: BT<'static>,
    pub ieee802154: IEEE802154<'static>,
}

/// Transfer the radio blocks once; no MMIO or second singleton acquisition.
pub fn take_radio(singletons: RadioSingletons) -> RadioHardware {
    let _consumed = singletons;
    #[allow(
        unsafe_code,
        reason = "the consumed esp-hal owners prove the permanent radio handoff"
    )]
    // SAFETY: esp-hal issues each singleton once. All radio singletons are
    // consumed above and never returned, so no platform radio owner remains.
    let handoff = unsafe { RadioHandoff::assume_exclusive() };
    RadioHardware::from_handoff(handoff)
}
