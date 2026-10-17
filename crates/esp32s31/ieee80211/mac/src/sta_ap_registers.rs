//! Register-only boundary for one same-channel STA plus SoftAP configuration.
//!
//! This module deliberately stops below runtime ownership. It gives later
//! orchestration one finite operation instead of allowing it to interleave
//! the STA=0 and AP=1 receive-context transactions by hand.

use oer_esp32s31_hal::ieee80211::mac::MacStaApReceivePlan;
use oer_esp32s31_hal::ieee80211::mac::WifiMacHal;

/// Minimal hardware authority required by the combined receive-context plan.
pub trait StaApRegisterHardware {
    fn apply_sta_ap_receive_registers(&mut self, plan: MacStaApReceivePlan);
    fn disable_station_receive_registers(&mut self);
    fn disable_access_point_receive_registers(&mut self);
    fn disable_all_role_receive_registers(&mut self);
}

impl StaApRegisterHardware for WifiMacHal<'_> {
    fn apply_sta_ap_receive_registers(&mut self, plan: MacStaApReceivePlan) {
        self.configure_sta_ap_receive_plan(plan);
    }

    fn disable_station_receive_registers(&mut self) {
        self.disable_station_receive_policy();
    }

    fn disable_access_point_receive_registers(&mut self) {
        self.disable_access_point_receive_policy();
    }

    fn disable_all_role_receive_registers(&mut self) {
        self.disable_all_role_receive_policies();
    }
}

/// Leave the station role while retaining the complete SoftAP register bank.
pub fn disable_station_receive_registers<H: StaApRegisterHardware>(hardware: &mut H) {
    hardware.disable_station_receive_registers();
}

/// Leave the SoftAP role while retaining the complete station register bank.
pub fn disable_access_point_receive_registers<H: StaApRegisterHardware>(hardware: &mut H) {
    hardware.disable_access_point_receive_registers();
}

/// Enter the role-neutral suffix of vendor `wifi_set_rx_policy(0)` after the
/// cold transaction has already published both interface addresses.
///
/// The vendor disables interface zero, then interface one, and only then
/// clears the interface-zero unicast-BSSID checks, so the suffix is one
/// hardware operation rather than the two single-role disables. Keeping it
/// one operation also prevents a stopped runtime from accidentally retaining
/// admission for the role which was not active in the preceding epoch.
pub fn disable_all_role_receive_registers<H: StaApRegisterHardware>(hardware: &mut H) {
    hardware.disable_all_role_receive_registers();
}

/// Apply both reviewed MAC receive contexts as one LMAC operation.
///
/// The caller must separately prove that STA and SoftAP share one physical
/// channel and must own the common RX, TX, interrupt and beacon schedulers.
pub fn configure_sta_ap_receive_registers<H: StaApRegisterHardware>(
    hardware: &mut H,
    station_address: [u8; 6],
    station_bssid: [u8; 6],
    access_point_address: [u8; 6],
) {
    hardware.apply_sta_ap_receive_registers(MacStaApReceivePlan::observed_mode_one(
        station_address,
        station_bssid,
        access_point_address,
    ));
}

#[cfg(test)]
mod tests;
