#![no_std]
use oer_chips_esp32s31_hal::power::PlatformClockBackend;
use oer_chips_esp32s31_radio_phy::bluetooth_client::BluetoothRfReady;

pub fn consume<B: PlatformClockBackend>(ready: BluetoothRfReady<'_, B>) {
    let _ = ready.into_radio();
    #[cfg(ui_reject)]
    let _ = ready.into_radio(); // ui-error: E0382
}
