use super::*;
use oer_esp32s31_ieee80211::tx::WifiTxPowerPair;

const HE20_MCS9_CAPABILITY: [u8; 24] = [
    255, 22, 35, 0x03, 0x18, 0x9c, 0xca, 0x10, 0x80, 0x00, 0x10, 0x8a, 0x1b, 0x0d, 0xc0, 0x1f,
    0x00, 0x02, 0x82, 0x01, 0xfd, 0xff, 0xfd, 0xff,
];

struct Power;

impl WifiTxPowerProfile for Power {
    fn power_pair(&self, rate_code: u8) -> WifiTxPowerPair {
        let primary = [20, 20, 20, 19, 19, 18, 18, 16, 15, 20]
            [usize::from(rate_code.saturating_sub(16).min(9))];
        WifiTxPowerPair {
            primary,
            alternate: primary,
        }
    }
}

#[test]
fn he_profile_owns_calibrated_power_derivation() {
    let mut access_point = ScanRecord {
        channel: 6,
        ..ScanRecord::EMPTY
    };
    // An HE access point on 2.4 GHz carries HT Capabilities too.
    access_point.ht_capability_ie_present = true;
    access_point.he_capability_ie[..HE20_MCS9_CAPABILITY.len()]
        .copy_from_slice(&HE20_MCS9_CAPABILITY);
    access_point.he_capability_ie_len = HE20_MCS9_CAPABILITY.len() as u8;
    let operation = [255, 7, 36, 0, 0, 0, 1, 0xfd, 0xff];
    access_point.he_operation_ie[..operation.len()].copy_from_slice(&operation);
    access_point.he_operation_ie_len = operation.len() as u8;

    let profile =
        esp32s31_sta_association_profile(&access_point, Preference::PreferHe20, &Power).unwrap();
    assert_eq!(profile.phy, PhyMode::He20);
    assert_eq!(
        profile.rate_16_through_25,
        Some([20, 20, 20, 19, 19, 18, 18, 16, 15, 20])
    );
    assert_eq!(profile.power_capability.unwrap().minimum_dbm(), -11);
    assert_eq!(profile.power_capability.unwrap().maximum_dbm(), 20);
    assert_eq!(
        profile.he_ul_mu_power.unwrap().relative_to_rate_16(),
        [0, 0, 1, 1, 2, 2, 4, 5, 0]
    );
}

#[test]
fn a_legacy_access_point_fails_the_association_profile() {
    let access_point = ScanRecord {
        channel: 6,
        ..ScanRecord::EMPTY
    };
    assert_eq!(
        esp32s31_sta_association_profile(&access_point, Preference::Automatic, &Power),
        Err(StaAssociationProfileError::LegacyUnsupported)
    );
}
