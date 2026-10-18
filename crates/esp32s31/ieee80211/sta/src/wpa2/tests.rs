use oer_ieee80211_rsn::{
    EAPOL_KEY_FIXED_LEN, EAPOL_KEY_PACKET_LEN, EAPOL_PACKET_TYPE_KEY, RSN_KEY_DESCRIPTOR_TYPE,
};

use super::*;

const LOCAL: [u8; 6] = [2, 0, 0, 0, 0, 1];
const BSSID: [u8; 6] = [2, 0, 0, 0, 0, 2];

#[test]
fn eapol_copy_accepts_only_the_selected_station_link() {
    let station = Wpa2Station::new(LOCAL, BSSID);
    let mut frame = [0_u8; 160];
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[24..32].copy_from_slice(&LLC_SNAP_EAPOL);
    let eapol = &mut frame[32..32 + EAPOL_KEY_PACKET_LEN];
    eapol[0] = 2;
    eapol[1] = EAPOL_PACKET_TYPE_KEY;
    eapol[2..4].copy_from_slice(&(EAPOL_KEY_FIXED_LEN as u16).to_be_bytes());
    eapol[4] = RSN_KEY_DESCRIPTOR_TYPE;
    eapol[5..7].copy_from_slice(&(2_u16 | (1 << 3) | (1 << 7)).to_be_bytes());

    let mpdu_length = 32 + EAPOL_KEY_PACKET_LEN;
    let copied = copy_station_eapol(&frame, mpdu_length, 24, station).unwrap();
    assert_eq!(copied.interface(), RsnInterface::Station);
    assert_eq!(copied.peer(), &BSSID);
    assert_eq!(copied.as_bytes(), &frame[32..mpdu_length]);

    frame[10] ^= 1;
    assert!(copy_station_eapol(&frame, mpdu_length, 24, station).is_none());
}
