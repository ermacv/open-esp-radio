#![no_std]
use oer_esp32s31_hal::{
    domain::{PhyClockModule, RadioDomain},
    power::{PlatformClockBackend, PlatformClockProvider},
};

pub fn enable<B1: PlatformClockBackend, B2: PlatformClockBackend, T>(
    radio: &mut RadioDomain<B1, T>,
    own: &impl PlatformClockProvider<Backend = B1>,
    foreign: &impl PlatformClockProvider<Backend = B2>,
) {
    let _ = radio.enable_phy_modem_clocks(PhyClockModule::Phy, own);
    #[cfg(ui_reject)]
    let _ = radio.enable_phy_modem_clocks(PhyClockModule::Phy, foreign); // ui-error: E0271
    let _ = foreign;
}
