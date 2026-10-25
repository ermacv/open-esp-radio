//! Cold MAC preparation over host memory.

use crate::ieee80211::registers::HostWords;

#[test]
fn hold_closes_the_gates_and_prepare_opens_them() {
    let mut memory = HostWords::filled(0);
    let mut owners = memory.owners();
    owners.registers.hold_mac_runtime();
    assert!(owners.registers.mac_core_gates_closed());
    // The cold control image keeps the no-power-save stop requested.
    let control = owners.registers.mac.wifi_mac_control.read_control();
    assert!(control.no_retention_stop_request());
    owners.registers.prepare_mac_runtime();
    assert!(!owners.registers.mac_core_gates_closed());
}

#[test]
fn the_cold_control_image_clears_every_writable_lane_before_the_stop() {
    let mut memory = HostWords::filled(u32::MAX);
    let mut owners = memory.owners();
    owners.registers.hold_mac_runtime();
    let control = owners.registers.mac.wifi_mac_control.read_control();
    assert_eq!(control.bits_0_11_opaque().get(), 0);
    assert_eq!(control.bits_24_31_opaque(), 0);
    assert!(control.no_retention_stop_request());
}
