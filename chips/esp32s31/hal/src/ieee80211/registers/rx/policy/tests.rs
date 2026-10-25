//! The receive-policy transitions over host memory: what each writes and
//! what it keeps.

use oer_esp32s31_pac::{instances, wifi_mac::WifiMacInterfaceAddress};

use super::{
    ACCESS_POINT, COLD_RECEIVE_QUEUE_INTERFACES, MacRoleReceivePolicy, MacStaPolicyMode, SNIFFER,
    STATION, clear_cold_interface_address,
};
use crate::ieee80211::registers::{HostWords, MacInterface};

const BSSID: [u8; 6] = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];

#[test]
fn the_cold_policy_clears_the_first_three_interfaces_and_keeps_their_high_bits() {
    let mut words = std::vec![u32::MAX; instances::wifi_mac::SIZE.div_ceil(4)];
    let mut block = WifiMacInterfaceAddress::host(&mut words).expect("memory holds the part");
    for interface in COLD_RECEIVE_QUEUE_INTERFACES.into_iter().flatten() {
        clear_cold_interface_address(&mut block, interface);
    }
    for interface in [
        MacInterface::Station,
        MacInterface::AccessPoint,
        MacInterface::Context2,
    ] {
        let high = block.read_address_high(interface.index());
        assert_eq!(high.bytes_4_5(), 0, "{interface:?} address bytes cleared");
        assert!(high.rx_policy_enable(), "{interface:?} enable kept");
        assert_ne!(
            high.bits_17_31_opaque().get(),
            0,
            "{interface:?} opaque bits kept"
        );
    }
    let fourth = block.read_address_high(MacInterface::Context3.index());
    assert_eq!(fourth.bytes_4_5(), u16::MAX, "queue three has no context");
    drop(block);
    // Each of the first three high words lost only its address bytes.
    assert_eq!(words.iter().filter(|word| **word != u32::MAX).count(), 3);
}

#[test]
fn queue_three_has_no_interface_context() {
    assert_eq!(
        COLD_RECEIVE_QUEUE_INTERFACES,
        [
            Some(MacInterface::Station),
            Some(MacInterface::AccessPoint),
            Some(MacInterface::Context2),
            None,
        ]
    );
}

#[test]
fn the_cold_policy_marks_only_the_access_point_bank_soft_ap_and_disables_checks() {
    let before = HostWords::filled(0);
    let mut after = before.clone();
    after.owners().registers.initialize_cold_receive_policy();
    let owners = after.owners();
    let mac = &owners.registers.mac;
    for (position, context) in COLD_RECEIVE_QUEUE_INTERFACES.into_iter().enumerate() {
        let queue = crate::ieee80211::registers::element::<4>(position);
        let filter = mac.wifi_mac_rx_filter.read_policy(queue);
        assert!(filter.receive_broadcast_check_bssid() && filter.receive_unicast_check_da());
        assert!(filter.dump_unicast_check_da() && filter.dump_broadcast_check_bssid());
        assert!(!filter.check_bssid() && !filter.dump_management_not_check_bssid());
        let bssid = mac.wifi_mac_bssid_policy.read_bssid_high(queue);
        assert_eq!(
            bssid.interface_is_soft_ap(),
            context == Some(MacInterface::AccessPoint)
        );
        assert!(!bssid.address_check_enable());
    }
    // Only the four filter words change from zero.
    let changes = before.changes(&after);
    assert_eq!(changes.len(), 4 + 1);
    assert!(
        changes
            .iter()
            .all(|(part, _, _)| ["wifi_mac_rx_filter", "wifi_mac_bssid_policy"].contains(part))
    );
}

#[test]
fn station_policy_six_publishes_the_bssid_and_enables_its_checks() {
    let mut words = HostWords::filled(0);
    let mut owners = words.owners();
    owners
        .registers
        .apply_role_receive_policy(MacRoleReceivePolicy::Station {
            bssid: BSSID,
            mode: MacStaPolicyMode::Mode1,
        });
    let snapshot = owners.registers.sta_receive_policy_snapshot();
    assert_eq!(snapshot.bssid, BSSID);
    assert!(snapshot.bssid_address_check_enabled);
    assert!(!snapshot.interface_is_soft_ap);
    assert!(snapshot.interface_rx_policy_enabled);
    let filter = owners.registers.mac.wifi_mac_rx_filter.read_policy(STATION);
    assert!(filter.receive_unicast_check_bssid() && filter.dump_unicast_check_bssid());
    assert!(!filter.receive_management_not_check_bssid() && !filter.pass_beacon());

    owners
        .registers
        .apply_role_receive_policy(MacRoleReceivePolicy::StationDisabled);
    let snapshot = owners.registers.sta_receive_policy_snapshot();
    assert_eq!(snapshot.bssid, BSSID, "the BSSID bank is retained");
    assert!(!snapshot.bssid_address_check_enabled && !snapshot.interface_rx_policy_enabled);
    let filter = owners.registers.mac.wifi_mac_rx_filter.read_policy(STATION);
    assert!(!filter.receive_unicast_check_bssid() && !filter.dump_unicast_check_bssid());
}

#[test]
fn mode_two_admits_management_without_the_bssid_and_beacons() {
    let mut words = HostWords::filled(0);
    let mut owners = words.owners();
    owners
        .registers
        .apply_sta_policy_six(BSSID, MacStaPolicyMode::Mode2);
    let filter = owners.registers.mac.wifi_mac_rx_filter.read_policy(STATION);
    assert!(filter.receive_management_not_check_bssid() && filter.pass_beacon());
}

#[test]
fn disabling_the_access_point_keeps_the_station_context() {
    let mut words = HostWords::filled(0);
    let mut owners = words.owners();
    owners
        .registers
        .apply_sta_policy_six(BSSID, MacStaPolicyMode::Mode1);
    let station = owners.registers.sta_receive_policy_snapshot();
    let station_filter = owners.registers.mac.wifi_mac_rx_filter.read_policy(STATION);
    owners
        .registers
        .apply_role_receive_policy(MacRoleReceivePolicy::AccessPointDisabled);
    assert_eq!(owners.registers.sta_receive_policy_snapshot(), station);
    assert_eq!(
        owners.registers.mac.wifi_mac_rx_filter.read_policy(STATION),
        station_filter
    );
    let ap = owners.registers.ap_receive_policy_snapshot();
    assert!(ap.interface_is_soft_ap);
    assert!(!ap.bssid_address_check_enabled && !ap.interface_rx_policy_enabled);
    let ap_filter = owners
        .registers
        .mac
        .wifi_mac_rx_filter
        .read_policy(ACCESS_POINT);
    assert!(!ap_filter.pass_beacon() && !ap_filter.dump_management_not_check_bssid());
}

#[test]
fn closing_promiscuous_receive_restores_queue_three_filtering_and_auto_ack() {
    let before = HostWords::filled(0);
    let mut after = before.clone();
    after
        .owners()
        .registers
        .disable_open_mac_promiscuous_receive();
    let owners = after.owners();
    let filter = owners.registers.mac.wifi_mac_rx_filter.read_policy(SNIFFER);
    assert!(!filter.auto_ack_disable());
    assert!(filter.dump_unicast_check_da() && filter.dump_unicast_check_bssid());
    assert!(filter.dump_broadcast_check_bssid() && filter.abort_group());
    assert!(filter.receive_unicast_check_bssid());
    assert!(filter.receive_broadcast_check_bssid() && filter.receive_unicast_check_da());
    drop(owners);
    assert_eq!(before.changes(&after).len(), 1);
}
