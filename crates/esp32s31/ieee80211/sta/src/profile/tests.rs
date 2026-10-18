use super::*;
use oer_ieee80211_mac::sequence::SequenceNumber;
use oer_ieee80211_mac::{
    scan::ScanRecord,
    station::{AssociationRequest, HeUlMuPowerCapability, StaPowerCapability},
};

const LOCAL: [u8; 6] = [0x02, 0, 0, 0x12, 0x34, 0x56];
const BSSID: [u8; 6] = [0x30, 0x05, 0x5c, 0x11, 0x22, 0x33];

/// The WPA2-Personal security elements selected for `record`.
fn wpa2_rsn(record: &ScanRecord) -> oer_ieee80211_mac::station::SelectedRsn {
    oer_ieee80211_mac::station::select_association_rsn(
        record,
        oer_ieee80211_mac::security::StaSecurityPolicy::Wpa2Personal,
    )
    .unwrap()
}

fn access_point_with_rsn(akms: &[[u8; 4]], capabilities: u16) -> ScanRecord {
    let mut record = ScanRecord::EMPTY;
    record.ssid[..4].copy_from_slice(b"test");
    record.ssid_len = 4;
    record.bssid = BSSID;
    record.channel = 6;
    record.privacy = true;
    record.rsn = true;
    record.rsn_ie_count = 1;
    record.supported_rates[..4].copy_from_slice(&[0x82, 0x84, 0x8b, 0x96]);
    record.supported_rates_len = 4;
    let mut offset = 2;
    record.rsn_ie[offset..offset + 2].copy_from_slice(&1_u16.to_le_bytes());
    offset += 2;
    record.rsn_ie[offset..offset + 4].copy_from_slice(&[0, 0x0f, 0xac, 4]);
    offset += 4;
    record.rsn_ie[offset..offset + 2].copy_from_slice(&1_u16.to_le_bytes());
    offset += 2;
    record.rsn_ie[offset..offset + 4].copy_from_slice(&[0, 0x0f, 0xac, 4]);
    offset += 4;
    record.rsn_ie[offset..offset + 2].copy_from_slice(&(akms.len() as u16).to_le_bytes());
    offset += 2;
    for akm in akms {
        record.rsn_ie[offset..offset + 4].copy_from_slice(akm);
        offset += 4;
    }
    record.rsn_ie[offset..offset + 2].copy_from_slice(&capabilities.to_le_bytes());
    offset += 2;
    record.rsn_ie[0] = 48;
    record.rsn_ie[1] = (offset - 2) as u8;
    record.rsn_ie_len = offset as u8;
    record
}

#[test]
fn station_ht_profiles_put_tx_parameters_in_supported_mcs_byte_twelve() {
    for capability in [HT20_CAPABILITY_IE, HE20_HT_CAPABILITY_IE] {
        assert_eq!(capability[5], 0xff);
        assert_eq!(capability[17], 0x01);
        assert_eq!(capability[18], 0);
    }
    assert_eq!(HT40_CAPABILITY_IE[5], 0xff);
    assert_eq!(HT40_CAPABILITY_IE[9], 0);
    assert_eq!(HT40_CAPABILITY_IE[17], 0x01);
    assert_eq!(HT40_CAPABILITY_IE[18], 0);
}

#[test]
fn ht20_association_request_reproduces_the_migration_capabilities() {
    let mut record = access_point_with_rsn(&[[0, 0x0f, 0xac, 2]], 0);
    record.ht_capability_ie_present = true;
    let mut output = [0; 160];
    let length = AssociationRequest {
        source: LOCAL,
        access_point: &record,
        sequence_number: SequenceNumber::new(2).unwrap(),
        listen_interval: 1,
        phy: PhyMode::Ht20,
        security: &wpa2_rsn(&record),
        power_capability: None,
        he_ul_mu_power: None,
    }
    .encode(&mut output, &ASSOCIATION_CAPABILITIES)
    .unwrap();
    let phy_start = length - HT20_CAPABILITY_IE.len() - WMM_INFORMATION_IE.len();
    assert_eq!(
        &output[phy_start..phy_start + HT20_CAPABILITY_IE.len()],
        &HT20_CAPABILITY_IE
    );
    assert_eq!(
        &output[phy_start + HT20_CAPABILITY_IE.len()..length],
        &WMM_INFORMATION_IE
    );
}

#[test]
fn ht40_request_claims_width_short_gi_without_unqualified_mcs32() {
    let mut record = access_point_with_rsn(&[[0, 0x0f, 0xac, 2]], 0);
    record.channel = 6;
    record.ht_capability_ie_present = true;
    record.ht_capability_ie[0..4].copy_from_slice(&[45, 26, 0x02, 0]);
    record.ht_operation_ie_present = true;
    record.ht_operation_ie[0..4].copy_from_slice(&[61, 22, 6, 0x05]);
    let mut output = [0; 160];
    let length = AssociationRequest {
        source: LOCAL,
        access_point: &record,
        sequence_number: SequenceNumber::new(2).unwrap(),
        listen_interval: 1,
        phy: PhyMode::Ht40,
        security: &wpa2_rsn(&record),
        power_capability: None,
        he_ul_mu_power: None,
    }
    .encode(&mut output, &ASSOCIATION_CAPABILITIES)
    .unwrap();
    let phy_start = length - HT40_CAPABILITY_IE.len() - WMM_INFORMATION_IE.len();
    assert_eq!(&output[phy_start..phy_start + 4], &[45, 26, 0x6e, 0]);
    assert_eq!(
        output[phy_start + 9],
        0,
        "the local RX MCS32 bit must remain clear"
    );
    assert_eq!(
        output[phy_start + 17],
        0x01,
        "the ordinary one-stream TX and RX MCS sets remain equal"
    );
}

#[test]
fn association_selection_owns_phy_and_center_channel_policy() {
    let mut record = access_point_with_rsn(&[[0, 0x0f, 0xac, 2]], 0);
    record.channel = 6;
    record.ht_capability_ie_present = true;
    record.ht_capability_ie[0..4].copy_from_slice(&[45, 26, 0x02, 0]);
    record.ht_operation_ie_present = true;
    record.ht_operation_ie[0..4].copy_from_slice(&[61, 22, 6, 0x05]);
    record.he_capability_ie[..HE20_VENDOR_MCS9_CAPABILITY_IE.len()]
        .copy_from_slice(&HE20_VENDOR_MCS9_CAPABILITY_IE);
    record.he_capability_ie_len = HE20_VENDOR_MCS9_CAPABILITY_IE.len() as u8;
    let he_operation = [255, 7, 36, 0, 0, 0, 1, 0xfd, 0xff];
    record.he_operation_ie[..he_operation.len()].copy_from_slice(&he_operation);
    record.he_operation_ie_len = he_operation.len() as u8;

    let automatic = select_association(&record, Preference::Automatic);
    assert_eq!(automatic.phy, PhyMode::Ht40);
    assert_eq!(automatic.primary_channel, 6);
    assert_eq!(automatic.channel_or_frequency, 2_447);
    assert_eq!(automatic.cbw, 2);

    let he20 = select_association(&record, Preference::PreferHe20);
    assert_eq!(he20.phy, PhyMode::He20);
    assert_eq!(he20.channel_or_frequency, 6);
    assert_eq!(he20.cbw, 0);

    let ht20 = select_association(&record, Preference::ForceHt20);
    assert_eq!(ht20.phy, PhyMode::Ht20);
    assert_eq!(ht20.channel_or_frequency, 6);
    assert_eq!(ht20.cbw, 0);

    // An access point without HT Capabilities is joined legacy at 20 MHz.
    record.ht_capability_ie_present = false;
    let legacy = select_association(&record, Preference::Automatic);
    assert_eq!(legacy.phy, PhyMode::Legacy);
    assert_eq!(legacy.channel_or_frequency, 6);
    assert_eq!(legacy.cbw, 0);
}

#[test]
fn he20_request_masks_unowned_power_save_and_feedback_claims() {
    let mut record = access_point_with_rsn(&[[0, 0x0f, 0xac, 2]], 0);
    let ssid = b"FRITZ!Box 7530 FN";
    record.ssid[..ssid.len()].copy_from_slice(ssid);
    record.ssid_len = ssid.len() as u8;
    record.capability_info = 0x0431;
    record.supported_rates[..8].copy_from_slice(&[0x8b, 0x96, 0x82, 0x84, 0x0c, 0x18, 0x30, 0x60]);
    record.supported_rates_len = 8;
    record.extended_supported_rates[..4].copy_from_slice(&[0x6c, 0x12, 0x24, 0x48]);
    record.extended_supported_rates_len = 4;
    record.ht_capability_ie_present = true;
    record.he_capability_ie[..HE20_VENDOR_MCS9_CAPABILITY_IE.len()]
        .copy_from_slice(&HE20_VENDOR_MCS9_CAPABILITY_IE);
    record.he_capability_ie_len = HE20_VENDOR_MCS9_CAPABILITY_IE.len() as u8;
    let he_operation = [255, 7, 36, 0, 0, 0, 1, 0xfd, 0xff];
    record.he_operation_ie[..he_operation.len()].copy_from_slice(&he_operation);
    record.he_operation_ie_len = he_operation.len() as u8;

    let power =
        HeUlMuPowerCapability::from_rate_power_indices([20, 20, 20, 19, 19, 18, 18, 16, 15, 20])
            .unwrap();
    assert_eq!(power.relative_to_rate_16(), [0, 0, 1, 1, 2, 2, 4, 5, 0]);

    let mut output = [0; 192];
    let length = AssociationRequest {
        source: LOCAL,
        access_point: &record,
        sequence_number: SequenceNumber::new(2).unwrap(),
        listen_interval: 3,
        phy: PhyMode::He20,
        security: &wpa2_rsn(&record),
        power_capability: Some(StaPowerCapability::new(-11, 20).unwrap()),
        he_ul_mu_power: Some(power),
    }
    .encode(&mut output, &ASSOCIATION_CAPABILITIES)
    .unwrap();
    assert_eq!(
        &output[24..89],
        &[
            0x31, 0x04, 0x03, 0x00, 0x00, 17, b'F', b'R', b'I', b'T', b'Z', b'!', b'B', b'o', b'x',
            b' ', b'7', b'5', b'3', b'0', b' ', b'F', b'N', 1, 8, 0x8b, 0x96, 0x82, 0x84, 0x0c,
            0x18, 0x30, 0x60, 48, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0,
            0x0f, 0xac, 2, 0, 4, 33, 2, 0xf5, 20, 50, 4, 0x6c, 0x12, 0x24, 0x48,
        ]
    );
    const EXPECTED_UL_MU: [u8; 14] = [255, 12, 60, 0, 0, 1, 1, 2, 2, 4, 5, 0, 0, 0];
    let expected_tail_len = HE20_HT_CAPABILITY_IE.len()
        + HE20_OWNED_MCS9_CAPABILITY_IE.len()
        + EXPECTED_UL_MU.len()
        + WMM_INFORMATION_IE.len()
        + HE20_EXTENDED_CAPABILITY_IE.len();
    let tail = &output[length - expected_tail_len..length];
    let mut offset = 0;
    assert_eq!(
        &tail[offset..offset + HE20_HT_CAPABILITY_IE.len()],
        &HE20_HT_CAPABILITY_IE
    );
    offset += HE20_HT_CAPABILITY_IE.len();
    assert_eq!(
        &tail[offset..offset + HE20_OWNED_MCS9_CAPABILITY_IE.len()],
        &HE20_OWNED_MCS9_CAPABILITY_IE
    );
    assert_eq!(
        HE20_OWNED_MCS9_CAPABILITY_IE[3],
        HE20_VENDOR_MCS9_CAPABILITY_IE[3] & !(1 << 1)
    );
    assert_eq!(
        HE20_OWNED_MCS9_CAPABILITY_IE[15],
        HE20_VENDOR_MCS9_CAPABILITY_IE[15] & !0x1f
    );
    assert_eq!(HE20_OWNED_MCS9_CAPABILITY_IE[13], 0);
    assert_eq!(HE20_OWNED_MCS9_CAPABILITY_IE[14], 0);
    assert_eq!(
        HE20_OWNED_MCS9_CAPABILITY_IE[18],
        HE20_VENDOR_MCS9_CAPABILITY_IE[18] & !(1 << 1)
    );
    for index in 0..HE20_VENDOR_MCS9_CAPABILITY_IE.len() {
        if index != 3 && index != 13 && index != 14 && index != 15 && index != 18 {
            assert_eq!(
                HE20_OWNED_MCS9_CAPABILITY_IE[index],
                HE20_VENDOR_MCS9_CAPABILITY_IE[index],
            );
        }
    }
    offset += HE20_OWNED_MCS9_CAPABILITY_IE.len();
    assert_eq!(
        &tail[offset..offset + EXPECTED_UL_MU.len()],
        &EXPECTED_UL_MU
    );
    offset += EXPECTED_UL_MU.len();
    assert_eq!(
        &tail[offset..offset + WMM_INFORMATION_IE.len()],
        &WMM_INFORMATION_IE
    );
    offset += WMM_INFORMATION_IE.len();
    assert_eq!(&tail[offset..], &HE20_EXTENDED_CAPABILITY_IE);
    assert_eq!(
        tail[offset + 2] & 0x80,
        0x80,
        "the independently retained Event capability must stay set"
    );
    assert_eq!(
        tail[offset + 4] & 0x40,
        0,
        "Multiple BSSID must not be advertised without profile ownership"
    );
}
