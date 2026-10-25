//! The CCMP key-table transactions over host memory.

use oer_esp32s31_pac::wifi_mac::{
    entry_key_word_0, entry_key_word_1, entry_key_word_2, entry_key_word_3, entry_peer_control,
    entry_peer_low, entry_tail_word_0, entry_tail_word_1, entry_tail_word_2, entry_tail_word_3,
    interface_control, key_valid_bitmap, policy_control, word_080c_opaque,
};
use oer_mmio::Index;

use super::{MacCcmpKeyIdentity, MacKeyInstallOutcome, MacSppMode};
use crate::ieee80211::registers::{HostWords, MacInterface, MacKeyEntryIndex};

const PEER: [u8; 6] = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];
const KEY: [u8; 16] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
];

fn entry(index: u32) -> MacKeyEntryIndex {
    MacKeyEntryIndex::new(index).expect("a key-table entry")
}

/// The (part, offset) of each changed word.
fn changed(before: &HostWords, after: &HostWords) -> std::vec::Vec<(&'static str, usize)> {
    before
        .changes(after)
        .into_iter()
        .map(|(part, offset, _)| (part, offset))
        .collect()
}

/// The ten word offsets of key entry `index`, in table order.
fn entry_words(index: usize) -> [usize; 10] {
    [
        entry_peer_low::OFFSET,
        entry_peer_control::OFFSET,
        entry_key_word_0::OFFSET,
        entry_key_word_1::OFFSET,
        entry_key_word_2::OFFSET,
        entry_key_word_3::OFFSET,
        entry_tail_word_0::OFFSET,
        entry_tail_word_1::OFFSET,
        entry_tail_word_2::OFFSET,
        entry_tail_word_3::OFFSET,
    ]
    .map(|offset| offset + index * entry_peer_low::STRIDE)
}

#[test]
fn the_cold_bypass_stores_each_interface_control_then_clears_two_words() {
    let mut words = HostWords::filled(u32::MAX);
    let before = words.clone();
    words.owners().registers.initialize_mac_crypto_bypass();
    assert_eq!(
        changed(&before, &words),
        [
            ("wifi_mac_crypto_control", interface_control::OFFSET),
            ("wifi_mac_crypto_control", interface_control::OFFSET + 4),
            ("wifi_mac_crypto_control", interface_control::OFFSET + 8),
            ("wifi_mac_crypto_control", word_080c_opaque::OFFSET),
            ("wifi_mac_crypto_control", policy_control::OFFSET),
        ]
    );
    let owners = words.owners();
    let control = &owners.registers.mac.wifi_mac_crypto_control;
    for (position, bits_16_17) in [(0, 3), (1, 3), (2, 0)] {
        let word = control.read_interface_control(Index::new(position).expect("an interface"));
        assert_eq!(word.bits_0_1_opaque().get(), 0);
        assert!(!(word.bit_8_opaque() || word.spp_amsdu() || word.bit_28_opaque()));
        assert_eq!(word.bits_16_17_opaque().get(), bits_16_17);
        assert_eq!(word.bits_30_31_opaque().get(), 0);
    }
    assert_eq!(control.read_word_080c_opaque().bits_0_31_opaque(), 0);
    assert_eq!(control.read_policy_control().bits_6_21_opaque(), 0);
}

#[test]
fn an_association_enables_ccmp_with_its_spp_decision() {
    let mut words = HostWords::filled(0);
    let before = words.clone();
    words
        .owners()
        .registers
        .configure_mac_ccmp_association(MacInterface::AccessPoint, MacSppMode::Authenticated);
    assert_eq!(
        changed(&before, &words),
        [("wifi_mac_crypto_control", interface_control::OFFSET + 4)]
    );
    let owners = words.owners();
    let word = owners
        .registers
        .mac
        .wifi_mac_crypto_control
        .read_interface_control(Index::constant::<1>());
    assert_eq!(word.bits_0_1_opaque().get(), 3);
    assert!(word.bit_8_opaque() && word.spp_amsdu());
    assert_eq!(word.bits_16_17_opaque().get(), 3);
    assert!(!word.bit_28_opaque());
    assert_eq!(word.bits_30_31_opaque().get(), 0);
}

#[test]
fn a_pairwise_station_key_is_written_validated_and_enabled() {
    let mut words = HostWords::filled(0);
    let before = words.clone();
    let outcome = words.owners().registers.install_sta_ccmp_key_entry(
        entry(2),
        MacCcmpKeyIdentity::Pairwise { peer: PEER },
        &KEY,
    );
    assert_eq!(outcome, MacKeyInstallOutcome::Installed);

    let [peer_low, peer_control, k0, k1, k2, k3, ..] = entry_words(2);
    let changes = before.changes(&words);
    assert_eq!(
        changed(&before, &words),
        [
            ("wifi_mac_crypto_control", interface_control::OFFSET),
            ("wifi_mac_crypto_control", key_valid_bitmap::OFFSET),
            ("wifi_mac_key_table", peer_low),
            ("wifi_mac_key_table", peer_control),
            ("wifi_mac_key_table", k0),
            ("wifi_mac_key_table", k1),
            ("wifi_mac_key_table", k2),
            ("wifi_mac_key_table", k3),
        ]
    );
    let values: std::vec::Vec<u32> = changes.iter().map(|change| change.2).collect();
    assert_eq!(
        values.get(2..),
        Some(
            [
                0x3322_1102,
                values.get(3).copied().unwrap_or_default(),
                0x0302_0100,
                0x0706_0504,
                0x0b0a_0908,
                0x0f0e_0d0c,
            ]
            .as_slice()
        )
    );
    let owners = words.owners();
    let table = &owners.registers.mac.wifi_mac_key_table;
    let control = table.read_entry_peer_control(Index::constant::<2>());
    assert_eq!(control.peer_high(), 0x5544);
    assert_eq!(control.algorithm().get(), 3);
    assert_eq!(control.direction().get(), 3);
    assert_eq!(control.interface(), MacInterface::Station.field());
    assert!(control.logical_index_not_three());
    assert_eq!(control.logical_key_index().get(), 0);
    assert!(!(control.bit_20_opaque() || control.bit_26_opaque()));
    assert_eq!(control.bits_16_17_opaque().get(), 0);
    assert!(owners.registers.mac_key_entry_is_valid(entry(2)));
    let interface = owners
        .registers
        .mac
        .wifi_mac_crypto_control
        .read_interface_control(Index::constant::<0>());
    assert_eq!(interface.bits_0_1_opaque().get(), 3);
    assert!(interface.bit_8_opaque() && !interface.spp_amsdu());
}

#[test]
fn a_group_access_point_key_uses_the_broadcast_peer_and_its_key_id() {
    let mut words = HostWords::filled(0);
    let outcome = words.owners().registers.install_ap_ccmp_key_entry(
        entry(1),
        MacCcmpKeyIdentity::Group { key_id: 3 },
        &KEY,
    );
    assert_eq!(outcome, MacKeyInstallOutcome::Installed);
    let owners = words.owners();
    let table = &owners.registers.mac.wifi_mac_key_table;
    assert_eq!(
        table.read_entry_peer_low(Index::constant::<1>()).peer_low(),
        u32::MAX
    );
    let control = table.read_entry_peer_control(Index::constant::<1>());
    assert_eq!(control.peer_high(), u16::MAX);
    assert_eq!(control.direction().get(), 6);
    assert_eq!(control.interface(), MacInterface::AccessPoint.field());
    // Logical key index three carries no flag.
    assert!(!control.logical_index_not_three());
    assert_eq!(control.logical_key_index().get(), 3);
}

#[test]
fn an_occupied_entry_or_an_invalid_group_key_changes_nothing() {
    let mut words = HostWords::filled(0);
    let before = words.clone();
    let outcome = words.owners().registers.install_sta_ccmp_key_entry(
        entry(0),
        MacCcmpKeyIdentity::Group { key_id: 4 },
        &KEY,
    );
    assert_eq!(outcome, MacKeyInstallOutcome::Rejected);
    assert_eq!(before.changes(&words), []);

    let mut words = HostWords::filled(u32::MAX);
    let before = words.clone();
    let outcome = words.owners().registers.install_sta_ccmp_key_entry(
        entry(24),
        MacCcmpKeyIdentity::Pairwise { peer: PEER },
        &KEY,
    );
    assert_eq!(outcome, MacKeyInstallOutcome::Occupied);
    assert_eq!(before.changes(&words), []);
}

#[test]
fn clearing_an_entry_invalidates_it_and_zeroes_its_ten_words() {
    let mut words = HostWords::filled(u32::MAX);
    let before = words.clone();
    words.owners().registers.clear_mac_key_entry(entry(7));
    let mut expected = std::vec![("wifi_mac_crypto_control", key_valid_bitmap::OFFSET)];
    expected.extend(entry_words(7).map(|offset| ("wifi_mac_key_table", offset)));
    assert_eq!(changed(&before, &words), expected);
    let owners = words.owners();
    assert!(!owners.registers.mac_key_entry_is_valid(entry(7)));
    assert!(owners.registers.mac_key_entry_is_valid(entry(6)));
    assert!(owners.registers.mac_key_entry_is_valid(entry(24)));
    let table = &owners.registers.mac.wifi_mac_key_table;
    assert_eq!(
        table.read_entry_peer_low(Index::constant::<7>()).peer_low(),
        0
    );
    assert_eq!(
        table.read_entry_key_word_3(Index::constant::<7>()).value(),
        0
    );
    assert_eq!(
        table.read_entry_tail_word_3(Index::constant::<7>()).value(),
        0
    );
}
