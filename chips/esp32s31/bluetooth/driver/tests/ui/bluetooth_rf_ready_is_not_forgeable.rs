#![no_std]
use oer_esp32s31_hal::{domain::RadioDomain, power::PlatformClockBackend};
use oer_esp32s31_phy::{bluetooth_client::{BluetoothPhyMembership, bluetooth_rf_ready}, concurrent::ConcurrentPhy};

pub fn check<B: PlatformClockBackend>(radio: &mut RadioDomain<B, ConcurrentPhy>, member: &BluetoothPhyMembership) {
    let _ = bluetooth_rf_ready(radio, member);
    #[cfg(ui_reject)]
    let _ = oer_esp32s31_phy::bluetooth_client::BluetoothRfReady { radio, _membership: member }; // ui-error: E0451
}
