//! Ownership boundary for the open promiscuous receive frontier.

use oer_esp32s31_hal::{
    ieee80211::mac::{WifiMacColdHal, WifiMacHal},
    owner::RadioRuntimeOwner,
};

pub trait MacSnifferHardware {
    fn configure_open_promiscuous_receive(&mut self);
    fn disable_open_promiscuous_receive(&mut self);
}

impl MacSnifferHardware for WifiMacColdHal<'_> {
    fn configure_open_promiscuous_receive(&mut self) {
        WifiMacColdHal::configure_open_promiscuous_receive(self);
    }

    fn disable_open_promiscuous_receive(&mut self) {
        WifiMacColdHal::disable_open_mac_promiscuous_receive(self);
    }
}

impl MacSnifferHardware for WifiMacHal<'_> {
    fn configure_open_promiscuous_receive(&mut self) {
        WifiMacHal::configure_open_promiscuous_receive(self);
    }

    fn disable_open_promiscuous_receive(&mut self) {
        WifiMacHal::disable_open_mac_promiscuous_receive(self);
    }
}

impl MacSnifferHardware for RadioRuntimeOwner {
    fn configure_open_promiscuous_receive(&mut self) {
        self.wifi_mac_hal().configure_open_promiscuous_receive();
    }

    fn disable_open_promiscuous_receive(&mut self) {
        self.wifi_mac_hal().disable_open_mac_promiscuous_receive();
    }
}
