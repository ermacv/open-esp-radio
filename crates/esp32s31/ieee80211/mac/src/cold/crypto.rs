//! Ownership boundary for the cold hardware-crypto bypass state.

use oer_esp32s31_hal::ieee80211::mac::WifiMacColdHal;

pub trait MacColdCryptoHardware {
    fn initialize_crypto_bypass(&mut self);
}

impl MacColdCryptoHardware for WifiMacColdHal<'_> {
    fn initialize_crypto_bypass(&mut self) {
        WifiMacColdHal::initialize_crypto_bypass(self);
    }
}
