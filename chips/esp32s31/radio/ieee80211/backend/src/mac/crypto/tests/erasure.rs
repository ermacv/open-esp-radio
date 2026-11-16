//! The erasure of every key the MAC can hold, over the key table's double.

use crate::test_hardware::{BSSID, Hardware, PEER};

#[test]
fn emergency_erasure_reaches_the_last_ap_peer_as_well_as_station_and_group_keys() {
    use crate::mac::crypto::*;
    let mut hardware = Hardware::default();
    let station = install_sta_pairwise_ccmp(&mut hardware, BSSID, &[1; 16]).expect("STA PTK");
    let group = install_sta_group_ccmp(
        &mut hardware,
        oer_protocols_ieee80211_wire::ccmp::GroupKeyId::new(0).expect("KeyID"),
        &[2; 16],
    )
    .expect("GTK");
    let ap = install_ap_pairwise_ccmp(
        &mut hardware,
        PEER,
        u16::from(AP_PAIRWISE_SLOT_COUNT),
        &[3; 16],
    )
    .expect("last AP AID");
    let ap_group = install_ap_group_ccmp(
        &mut hardware,
        ApGroupKeyId::new(1).expect("AP KeyID"),
        &[4; 16],
    )
    .expect("AP GTK");
    assert_eq!(hardware.installed_keys.len(), 4);
    erase_lower_mac_keys(&mut hardware);
    assert!(hardware.installed_keys.is_empty());
    for index in [
        station.hardware_index(),
        group.hardware_index(),
        ap.hardware_index(),
        ap_group.hardware_index(),
    ] {
        assert!(hardware.cleared_keys.contains(&index));
    }
}
