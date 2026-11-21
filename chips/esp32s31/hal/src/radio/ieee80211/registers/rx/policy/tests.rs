//! The receive-policy transitions over host memory: what each writes and
//! what it keeps.

use crate::radio::ieee80211::registers::interface_index;
use oer_chips_esp32s31_pac::{instances, wifi_mac::WifiMacInterfaceAddress};

use super::{
    ACCESS_POINT, COLD_RECEIVE_QUEUE_INTERFACES, MacRoleReceivePolicy, MacStaPolicyMode, SNIFFER,
    STATION, clear_cold_interface_address,
};
use crate::radio::ieee80211::registers::{
    HostWords, MacInterface, MacPlacement, WifiRadioRegisters, byte,
};

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
        let high = block.read_address_high(interface_index(interface));
        assert_eq!(high.bytes_4_5(), 0, "{interface:?} address bytes cleared");
        assert!(
            (high.rx_policy_enable()
                == oer_chips_esp32s31_pac::wifi_mac::address_high::RxPolicyEnable::Enabled),
            "{interface:?} enable kept"
        );
        assert_ne!(
            high.bits_17_31_opaque().get(),
            0,
            "{interface:?} opaque bits kept"
        );
    }
    let fourth = block.read_address_high(interface_index(MacInterface::Context3));
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
        let queue = crate::radio::ieee80211::registers::element::<4>(position);
        let filter = mac.wifi_mac_rx_filter.read_policy(queue);
        assert!(filter.receive_broadcast_check_bssid() && filter.receive_unicast_check_da());
        assert!(filter.dump_unicast_check_da() && filter.dump_broadcast_check_bssid());
        assert!(!filter.check_bssid() && !filter.dump_management_not_check_bssid());
        let bssid = mac.wifi_mac_bssid_policy.read_bssid_high(queue);
        assert_eq!(
            bssid.interface_is_soft_ap(),
            context == Some(MacInterface::AccessPoint)
        );
        assert_eq!(
            bssid.address_check_enable(),
            oer_chips_esp32s31_pac::wifi_mac::bssid_high::AddressCheckEnable::Disabled
        );
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
    let snapshot = sta_receive_policy_snapshot(&owners.registers);
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
    let snapshot = sta_receive_policy_snapshot(&owners.registers);
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
fn a_scan_keeps_mode_two_and_drops_the_unicast_bssid_check() {
    let mut words = HostWords::filled(0);
    let mut owners = words.owners();
    owners.registers.apply_sta_scan_policy(BSSID);
    let filter = owners.registers.mac.wifi_mac_rx_filter.read_policy(STATION);
    assert!(filter.receive_management_not_check_bssid() && filter.pass_beacon());
    assert!(!filter.receive_unicast_check_bssid() && !filter.dump_unicast_check_bssid());
}

#[test]
fn a_link_policy_after_a_scan_checks_the_unicast_bssid_again() {
    let mut words = HostWords::filled(0);
    let mut owners = words.owners();
    owners.registers.apply_sta_scan_policy(BSSID);
    owners
        .registers
        .apply_sta_policy_six(BSSID, MacStaPolicyMode::Mode1);
    let filter = owners.registers.mac.wifi_mac_rx_filter.read_policy(STATION);
    assert!(filter.receive_unicast_check_bssid() && filter.dump_unicast_check_bssid());
    assert!(!filter.pass_beacon());
}

#[test]
fn mode_two_for_a_connected_station_keeps_the_unicast_bssid_check() {
    let mut words = HostWords::filled(0);
    let mut owners = words.owners();
    owners
        .registers
        .apply_sta_policy_six(BSSID, MacStaPolicyMode::Mode2);
    let filter = owners.registers.mac.wifi_mac_rx_filter.read_policy(STATION);
    assert!(filter.receive_unicast_check_bssid() && filter.dump_unicast_check_bssid());
}

#[test]
fn disabling_the_access_point_keeps_the_station_context() {
    let mut words = HostWords::filled(0);
    let mut owners = words.owners();
    owners
        .registers
        .apply_sta_policy_six(BSSID, MacStaPolicyMode::Mode1);
    let station = sta_receive_policy_snapshot(&owners.registers);
    let station_filter = owners.registers.mac.wifi_mac_rx_filter.read_policy(STATION);
    owners
        .registers
        .apply_role_receive_policy(MacRoleReceivePolicy::AccessPointDisabled);
    assert_eq!(sta_receive_policy_snapshot(&owners.registers), station);
    assert_eq!(
        owners.registers.mac.wifi_mac_rx_filter.read_policy(STATION),
        station_filter
    );
    let ap = ap_receive_policy_snapshot(&owners.registers);
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

// Reads of what the writers left, from the register image: the HAL keeps
// no read of what it wrote.

/// Read-only associated-STA receive-policy evidence.
///
/// Only reviewed semantic fields cross the HAL boundary. Complete policy
/// register images stay private because several bits are still
/// undocumented and no production decision consumes them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MacStaReceivePolicySnapshot {
    bssid: [u8; 6],
    association_id: u16,
    minimum_mpdu_start_spacing: u8,
    bssid_address_check_enabled: bool,
    interface_is_soft_ap: bool,
    interface_rx_policy_enabled: bool,
    beacon_filter_enabled: bool,
    broadcast_wakeup_enabled: bool,
    unicast_wakeup_enabled: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MacApReceivePolicySnapshot {
    bssid: [u8; 6],
    bssid_address_check_enabled: bool,
    interface_is_soft_ap: bool,
    interface_rx_policy_enabled: bool,
}

fn ap_receive_policy_snapshot<M: MacPlacement>(
    registers: &WifiRadioRegisters<M>,
) -> MacApReceivePolicySnapshot {
    let bssids = &registers.mac.wifi_mac_bssid_policy;
    let bssid_high = bssids.read_bssid_high(ACCESS_POINT);
    MacApReceivePolicySnapshot {
        bssid: bssid(bssids, ACCESS_POINT),
        bssid_address_check_enabled: (bssid_high.address_check_enable()
            == oer_chips_esp32s31_pac::wifi_mac::bssid_high::AddressCheckEnable::Enabled),
        interface_is_soft_ap: bssid_high.interface_is_soft_ap(),
        interface_rx_policy_enabled: registers
            .interface
            .read_address_high(interface_index(MacInterface::AccessPoint))
            .rx_policy_enable()
            == oer_chips_esp32s31_pac::wifi_mac::address_high::RxPolicyEnable::Enabled,
    }
}

/// Snapshot every register frontier that can suppress associated-STA
/// beacons while still admitting directed management traffic.
fn sta_receive_policy_snapshot<M: MacPlacement>(
    registers: &WifiRadioRegisters<M>,
) -> MacStaReceivePolicySnapshot {
    let bssids = &registers.mac.wifi_mac_bssid_policy;
    let bssid_high = bssids.read_bssid_high(STATION);
    let interface = registers
        .interface
        .read_address_high(interface_index(MacInterface::Station));

    let bssid = bssid(bssids, STATION);
    let beacon_filter = registers
        .mac
        .wifi_mac_sta_beacon_filter
        .read_sta_beacon_filter_control();
    MacStaReceivePolicySnapshot {
        bssid,
        association_id: crate::radio::ieee80211::registers::halfword(bssid_high.association_id()),
        minimum_mpdu_start_spacing: byte(bssid_high.minimum_mpdu_start_spacing()),
        bssid_address_check_enabled: (bssid_high.address_check_enable()
            == oer_chips_esp32s31_pac::wifi_mac::bssid_high::AddressCheckEnable::Enabled),
        interface_is_soft_ap: bssid_high.interface_is_soft_ap(),
        interface_rx_policy_enabled: (interface.rx_policy_enable()
            == oer_chips_esp32s31_pac::wifi_mac::address_high::RxPolicyEnable::Enabled),
        beacon_filter_enabled: beacon_filter.beacon_filter_enable(),
        broadcast_wakeup_enabled: beacon_filter.broadcast_wakeup_enable(),
        unicast_wakeup_enabled: beacon_filter.unicast_wakeup_enable(),
    }
}

/// The six BSSID bytes of one bank.
fn bssid<P: oer_portable_mmio::Placement>(
    bssids: &oer_chips_esp32s31_pac::wifi_mac::WifiMacBssidPolicy<P>,
    bank: oer_portable_mmio::Index<4>,
) -> [u8; 6] {
    let [b0, b1, b2, b3] = bssids.read_bssid_low(bank).bytes_0_3().to_le_bytes();
    let [b4, b5] = bssids.read_bssid_high(bank).bssid_high().to_le_bytes();
    [b0, b1, b2, b3, b4, b5]
}
