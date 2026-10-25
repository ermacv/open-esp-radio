//! The MAC side of a channel switch over host memory.

use crate::ieee80211::registers::HostWords;

#[test]
fn channel_stop_and_resume_toggle_only_the_stop_request_lanes() {
    let before = HostWords::filled(0);
    let mut memory = before.clone();
    memory
        .owners()
        .registers
        .request_mac_channel_stop_without_power_save();
    {
        let owners = memory.owners();
        let control = owners.registers.mac.wifi_mac_control.read_control();
        assert!(control.no_retention_stop_request() && control.tx_block_stop_request_low());
        assert_eq!(control.power_save_tx_block().get(), 0x7);
        assert_eq!(control.tx_block_stop_request_high().get(), 0xf);
        assert_eq!(control.bits_0_11_opaque().get(), 0);
    }
    memory
        .owners()
        .registers
        .resume_mac_channel_without_power_save();
    assert_eq!(memory, before);
}

#[test]
fn regdma_link_selects_the_no_power_save_link() {
    let mut memory = HostWords::filled(0);
    let mut owners = memory.owners();
    owners.registers.select_wifi_no_power_save_regdma_link();
    assert_eq!(owners.registers.wifi_mac_regdma_link(), 4);
}
