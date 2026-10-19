use crate::{support::*, *};

#[test]
fn sta_pairwise_ccmp_install_owns_one_bounded_hardware_slot() {
    let mut mmio = MockMmio::default();
    let peer = [0xdc, 0x15, 0xc8, 0x54, 0xbc, 0x1e];
    let temporal_key = core::array::from_fn(|index| index as u8);

    let mut slot = install_sta_pairwise_ccmp(&mut mmio, peer, &temporal_key).unwrap();
    assert_eq!(slot.hardware_index(), 4);
    assert_eq!(slot.peer(), &peer);
    assert_eq!(
        mmio.operations(),
        &[Operation::InstallCcmp(
            MacInterface::Station,
            4,
            MacCcmpKeyIdentity::Pairwise { peer },
        )]
    );
    assert_eq!(slot.next_tx_ccmp_header(), Ok([3, 0, 0, 0x20, 0, 0, 0, 0]));
    assert_eq!(slot.next_tx_ccmp_header(), Ok([6, 0, 0, 0x20, 0, 0, 0, 0]));

    slot.clear(&mut mmio);
    assert!(!mmio.ccmp_valid[4]);
    assert_eq!(mmio.operations().last(), Some(&Operation::ClearCcmp(4)));

    mmio.ccmp_valid[4] = true;
    assert_eq!(
        install_sta_pairwise_ccmp(&mut mmio, peer, &temporal_key).err(),
        Some(CryptoKeyError::Occupied)
    );
}

#[test]
fn sta_group_ccmp_install_uses_the_owned_semantic_slot() {
    let mut mmio = MockMmio::default();
    let temporal_key = core::array::from_fn(|index| 0xf0 | index as u8);

    let slot = install_sta_group_ccmp(
        &mut mmio,
        oer_ieee80211_mac::ccmp::GroupKeyId::new(1).unwrap(),
        &temporal_key,
    )
    .unwrap();
    assert_eq!(slot.hardware_index(), 1);
    assert_eq!(slot.key_id().get(), 1);
    assert_eq!(
        mmio.operations(),
        &[Operation::InstallCcmp(
            MacInterface::Station,
            1,
            MacCcmpKeyIdentity::Group { key_id: 1 },
        )]
    );

    slot.clear(&mut mmio);
    assert!(!mmio.ccmp_valid[1]);
    assert!(oer_ieee80211_mac::ccmp::GroupKeyId::new(4).is_none());
}

#[test]
fn station_key_teardown_consumes_and_clears_both_hardware_slots() {
    let mut mmio = MockMmio::default();
    let pairwise = install_sta_pairwise_ccmp(&mut mmio, [1, 2, 3, 4, 5, 6], &[0x55; 16]).unwrap();
    let group = install_sta_group_ccmp(
        &mut mmio,
        oer_ieee80211_mac::ccmp::GroupKeyId::new(2).unwrap(),
        &[0xaa; 16],
    )
    .unwrap();
    assert!(mmio.ccmp_valid[4]);
    assert!(mmio.ccmp_valid[1]);

    let report = clear_sta_ccmp_slots(&mut mmio, pairwise, group);
    assert_eq!(report.pairwise_hardware_index, 4);
    assert_eq!(report.group_hardware_index, 1);
    assert!(!mmio.ccmp_valid[4]);
    assert!(!mmio.ccmp_valid[1]);
    assert!(
        mmio.operations()
            .ends_with(&[Operation::ClearCcmp(1), Operation::ClearCcmp(4)])
    );
}
