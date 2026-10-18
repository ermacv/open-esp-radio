use super::*;

#[test]
fn lowering_contains_the_chip_encoding_in_the_chip_crate() {
    assert_eq!(
        lower_wifi_channel(WifiChannel::mhz20(6).unwrap()),
        PhyChannel {
            channel_or_frequency: 6,
            cbw: 0,
        }
    );
    assert_eq!(
        lower_wifi_channel(WifiChannel::new_2_4_ghz(6, WifiChannelWidth::Mhz40Above).unwrap()),
        PhyChannel {
            channel_or_frequency: 2_447,
            cbw: 2,
        }
    );
}

#[test]
fn the_tx_limit_is_the_legal_channels_ceiling_in_quarter_dbm() {
    use oer_ieee80211_mac::regulatory::{CountryCode, RegulatoryDomain};
    let channel = |country: &str| {
        RegulatoryDomain::for_country(CountryCode::parse(country).unwrap())
            .unwrap()
            .legal(WifiChannel::mhz20(6).unwrap())
            .unwrap()
    };
    assert_eq!(tx_ceiling_quarter_dbm(&channel("DE")), 80);
    // The lab cap lowers it; nothing else does.
    assert_eq!(tx_ceiling_quarter_dbm(&channel("DE").capped(10)), 40);
    // 30 dBm is 120 quarter-dBm; a ceiling past i8 saturates.
    assert_eq!(tx_ceiling_quarter_dbm(&channel("US")), 120);
    assert_eq!(tx_ceiling_quarter_dbm(&channel("US").capped(-40)), -128);
}
