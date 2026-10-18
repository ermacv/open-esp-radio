use super::*;

#[test]
fn selected_channel_preserves_negotiated_ht40_geometry() {
    let mut access_point = ScanRecord {
        channel: 6,
        ht_capability_ie_present: true,
        ht_operation_ie_present: true,
        ..ScanRecord::EMPTY
    };
    access_point.ht_capability_ie[0..4].copy_from_slice(&[45, 26, 0x02, 0]);
    access_point.ht_operation_ie[0..4].copy_from_slice(&[61, 22, 6, 0x05]);
    let station = StaAttemptStation {
        station_address: [0; 6],
        access_point,
        association_preference: Preference::Automatic,
        security: oer_ieee80211_mac::security::StaSecurityPolicy::Open,
    };
    assert_eq!(
        station.selected_channel(),
        WifiChannel::new_2_4_ghz(6, WifiChannelWidth::Mhz40Above)
    );
}
