//! The open promiscuous receive frontier over host memory.

use oer_esp32s31_pac::wifi_mac::{misc_packet_policy, policy};
use oer_mmio::Index;

use crate::ieee80211::registers::HostWords;

#[test]
fn promiscuous_receive_rewrites_only_the_sniffer_policy_and_misc_classes() {
    let mut words = HostWords::filled(u32::MAX);
    let before = words.clone();
    words
        .owners()
        .registers
        .configure_open_mac_promiscuous_receive();

    let changes = before.changes(&words);
    // The misc classes were already all set; only policy three changes.
    let offsets: std::vec::Vec<_> = changes
        .iter()
        .map(|(part, offset, _)| (*part, *offset))
        .collect();
    assert_eq!(
        offsets,
        [("wifi_mac_rx_filter", policy::OFFSET + 3 * policy::STRIDE)]
    );

    let owners = words.owners();
    let filter = &owners.registers.mac.wifi_mac_rx_filter;
    let sniffer = filter.read_policy(Index::constant::<3>());
    assert!(sniffer.auto_ack_disable());
    for (cleared, name) in [
        (sniffer.dump_unicast_check_da(), "dump DA"),
        (sniffer.dump_unicast_check_bssid(), "dump unicast BSSID"),
        (sniffer.dump_broadcast_check_bssid(), "dump broadcast BSSID"),
        (sniffer.abort_group(), "abort group"),
        (
            sniffer.receive_unicast_check_bssid(),
            "receive unicast BSSID",
        ),
        (
            sniffer.receive_broadcast_check_bssid(),
            "receive broadcast BSSID",
        ),
        (sniffer.receive_unicast_check_da(), "receive DA"),
    ] {
        assert!(!cleared, "{name}");
    }
    // Every other field of the word keeps its value.
    assert!(sniffer.pass_beacon() && sniffer.check_bssid() && sniffer.not_group());
    assert_eq!(sniffer.bits_20_31_opaque().get(), 0xfff);
    for other in 0..3 {
        let index = Index::<4>::new(other).expect("an ordinary policy index");
        let policy = filter.read_policy(index);
        assert!(policy.dump_unicast_check_da() && policy.receive_unicast_check_da());
        assert!(policy.abort_group() && policy.auto_ack_disable());
    }
}

#[test]
fn promiscuous_receive_opens_every_misc_packet_class() {
    let mut words = HostWords::filled(0);
    let before = words.clone();
    words
        .owners()
        .registers
        .configure_open_mac_promiscuous_receive();
    let offsets: std::vec::Vec<_> = before
        .changes(&words)
        .iter()
        .map(|(part, offset, _)| (*part, *offset))
        .collect();
    assert_eq!(
        offsets,
        [
            ("wifi_mac_rx_filter", policy::OFFSET + 3 * policy::STRIDE),
            ("wifi_mac_rx_filter", misc_packet_policy::OFFSET),
        ]
    );
    let owners = words.owners();
    let filter = &owners.registers.mac.wifi_mac_rx_filter;
    let misc = filter.read_misc_packet_policy();
    assert_eq!(misc.open_misc_packet_classes(), 0xff);
    assert_eq!(misc.bits_0_7_opaque(), 0);
    let sniffer = filter.read_policy(Index::constant::<3>());
    assert!(sniffer.auto_ack_disable());
    // From an all-zero word only the auto-ACK disable is set.
    assert!(!(sniffer.pass_beacon() || sniffer.check_bssid() || sniffer.not_group()));
    assert_eq!(sniffer.bits_20_31_opaque().get(), 0);
}
