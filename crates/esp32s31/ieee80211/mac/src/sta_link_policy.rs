//! Hardware boundary for switching from scan to associated STA RX policy.

use oer_esp32s31_hal::{
    ieee80211::mac::WifiMacHal, owner::RadioRuntimeOwner, types::MacStaPolicyMode,
};

pub trait StaLinkRxPolicyHardware {
    fn apply_sta_link_policy(&mut self, bssid: [u8; 6]);
}

/// Hardware owner capable of admitting management frames with a foreign
/// BSSID while retaining normal station filtering and hardware auto-ACK.
pub trait StaEspNowRxPolicyHardware {
    fn apply_sta_esp_now_policy(&mut self, bssid: [u8; 6]);
}

/// Read-only PHY observation used by the STA layer to complete peer policy.
pub trait StaNoiseFloorHardware {
    fn read_noise_floor_dbm(&self) -> i8;
}

impl StaLinkRxPolicyHardware for WifiMacHal<'_> {
    fn apply_sta_link_policy(&mut self, bssid: [u8; 6]) {
        self.configure_station_receive_policy(bssid);
    }
}

impl StaEspNowRxPolicyHardware for WifiMacHal<'_> {
    fn apply_sta_esp_now_policy(&mut self, bssid: [u8; 6]) {
        // Re-run the complete normal transition first. In particular this
        // clears queue three's AUTOACK_DISABLE bit before policy six enables
        // management admission without BSSID matching on queue zero.
        self.configure_station_receive_policy(bssid);
        self.configure_station_policy_six(bssid, MacStaPolicyMode::Mode2);
    }
}

impl StaNoiseFloorHardware for WifiMacHal<'_> {
    fn read_noise_floor_dbm(&self) -> i8 {
        self.read_noise_floor_dbm()
    }
}

impl StaLinkRxPolicyHardware for RadioRuntimeOwner {
    fn apply_sta_link_policy(&mut self, bssid: [u8; 6]) {
        self.wifi_mac_hal().configure_station_receive_policy(bssid);
    }
}

impl StaEspNowRxPolicyHardware for RadioRuntimeOwner {
    fn apply_sta_esp_now_policy(&mut self, bssid: [u8; 6]) {
        self.wifi_mac_hal().apply_sta_esp_now_policy(bssid);
    }
}

impl StaNoiseFloorHardware for RadioRuntimeOwner {
    fn read_noise_floor_dbm(&self) -> i8 {
        RadioRuntimeOwner::read_noise_floor_dbm(self)
    }
}

/// Apply the reviewed BSSID/filter transaction after off-channel scan and
/// before the first Authentication transmission.
pub fn configure_sta_link_receive_policy<H: StaLinkRxPolicyHardware>(
    hardware: &mut H,
    bssid: [u8; 6],
) {
    hardware.apply_sta_link_policy(bssid);
}

/// Apply the reviewed normal-STA/auto-ACK transition followed by the exact
/// policy-six Mode2 transaction needed to pass broadcast-BSSID Action frames.
///
/// The numeric submode name remains intentional: this API claims only the
/// decoded management/BSSID filter fields, not simultaneous AP+STA behavior.
pub fn configure_sta_esp_now_receive_policy<H: StaEspNowRxPolicyHardware>(
    hardware: &mut H,
    bssid: [u8; 6],
) {
    hardware.apply_sta_esp_now_policy(bssid);
}

/// Apply the reviewed normal-STA/auto-ACK policy for an unassociated
/// station-address VIF, followed by the exact policy-six Mode2 leaf already
/// used for connected ESP-NOW vendor Action admission.
///
/// The protocol-defined broadcast BSSID is supplied to the existing typed
/// transaction; this function introduces no new register image or submode.
pub fn configure_standalone_esp_now_receive_policy<H: StaEspNowRxPolicyHardware>(hardware: &mut H) {
    hardware.apply_sta_esp_now_policy(oer_ieee80211_mac::management::BROADCAST_ADDRESS);
}
