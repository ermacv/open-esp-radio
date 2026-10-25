//! The receive-interface address publication over host memory.

use oer_esp32s31_pac::wifi_mac::{address_high, address_low};

use crate::ieee80211::registers::{HostWords, MacInterface};

const ADDRESS: [u8; 6] = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];

#[test]
fn an_interface_address_is_stored_whole_and_then_enabled() {
    let mut words = HostWords::filled(u32::MAX);
    let before = words.clone();
    words
        .owners()
        .registers
        .program_receive_interface_address(MacInterface::AccessPoint, ADDRESS);

    // The access point's pair is the second: the low four bytes whole, the
    // high two bytes from zero with the receive-policy enable set after.
    assert_eq!(
        before.changes(&words),
        [
            (
                "interface_address",
                address_low::OFFSET + address_low::STRIDE,
                0x3322_1102
            ),
            (
                "interface_address",
                address_high::OFFSET + address_high::STRIDE,
                0x0001_5544
            ),
        ]
    );
    let owners = words.owners();
    let high = owners
        .registers
        .interface
        .read_address_high(MacInterface::AccessPoint.index());
    assert_eq!(high.bytes_4_5(), 0x5544);
    assert!(high.rx_policy_enable());
    assert_eq!(high.bits_17_31_opaque().get(), 0);
}

#[test]
fn each_interface_selects_its_own_address_pair() {
    for (interface, pair) in [
        (MacInterface::Station, 0),
        (MacInterface::AccessPoint, 1),
        (MacInterface::Context2, 2),
        (MacInterface::Context3, 3),
    ] {
        let mut words = HostWords::filled(0);
        let before = words.clone();
        words
            .owners()
            .registers
            .program_receive_interface_address(interface, ADDRESS);
        let changes = before.changes(&words);
        let offsets: std::vec::Vec<_> = changes
            .iter()
            .map(|(part, offset, _)| (*part, *offset))
            .collect();
        assert_eq!(
            offsets,
            [
                (
                    "interface_address",
                    address_low::OFFSET + pair * address_low::STRIDE
                ),
                (
                    "interface_address",
                    address_high::OFFSET + pair * address_high::STRIDE
                ),
            ],
            "{interface:?}"
        );
    }
}

#[test]
fn station_and_access_point_addresses_are_published_in_order() {
    let mut words = HostWords::filled(0);
    let before = words.clone();
    words
        .owners()
        .registers
        .program_sta_ap_receive_addresses(ADDRESS, [0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b]);
    assert_eq!(
        before.changes(&words),
        [
            ("interface_address", address_low::OFFSET, 0x3322_1102),
            ("interface_address", address_high::OFFSET, 0x0001_5544),
            (
                "interface_address",
                address_low::OFFSET + address_low::STRIDE,
                0x0908_0706
            ),
            (
                "interface_address",
                address_high::OFFSET + address_high::STRIDE,
                0x0001_0b0a
            ),
        ]
    );
}
