//! Ownership boundary for role-neutral to active-AP receive policy.

use oer_esp32s31_hal::{ieee80211::mac::WifiMacHal, owner::RadioRuntimeOwner};

pub trait ApRxPolicyHardware {
    fn apply_ap_link_policy(&mut self, access_point: [u8; 6]);
    fn disable_ap_link_policy(&mut self);
}

impl ApRxPolicyHardware for WifiMacHal<'_> {
    fn apply_ap_link_policy(&mut self, access_point: [u8; 6]) {
        self.configure_access_point_receive_policy(access_point);
    }

    fn disable_ap_link_policy(&mut self) {
        self.disable_access_point_receive_policy();
    }
}

impl ApRxPolicyHardware for RadioRuntimeOwner {
    fn apply_ap_link_policy(&mut self, access_point: [u8; 6]) {
        self.wifi_mac_hal()
            .configure_access_point_receive_policy(access_point);
    }

    fn disable_ap_link_policy(&mut self) {
        self.wifi_mac_hal().disable_access_point_receive_policy();
    }
}

/// Select the exact vendor policy-eight AP receive frontier.
pub fn configure_ap_receive_policy<H: ApRxPolicyHardware>(hardware: &mut H, access_point: [u8; 6]) {
    hardware.apply_ap_link_policy(access_point);
}

/// Close AP RX admission before its descriptor ring or interrupt route stops.
pub fn disable_ap_receive_policy<H: ApRxPolicyHardware>(hardware: &mut H) {
    hardware.disable_ap_link_policy();
}

#[cfg(test)]
mod tests;
