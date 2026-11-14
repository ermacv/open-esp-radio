use super::*;

/// The capabilities record of a backend of these tests.
const CAPS: crate::LowerMacCapabilities = crate::LowerMacCapabilities {
    bands: crate::BandSet::GHZ2_4,
    widths: crate::WidthSet::MHZ20,
    rates: crate::RateSupport {
        dsss_cck: true,
        ofdm: true,
        ht_max_mcs: oer_protocols_ieee80211_wire::phy::HtMcs::new(7),
        he_max_mcs: oer_protocols_ieee80211_wire::phy::HeMcs::new(9),
        he_max_bandwidth_mhz: 20,
        he_dcm: false,
        he_ldpc: false,
        spatial_streams: 1,
    },
    services: crate::HardwareServices::FCS,
    vifs: 2,
    tx_queues: 4,
    max_mpdu_length: 2304,
    tx_power_ceiling_min_dbm: Some(0),
    individual_no_ack: crate::PhyFormatSet::NON_HT,
    station_receive_filters: crate::ReceiveFilter::BSS_MEMBER,
    access_point_receive_filters: crate::ReceiveFilter::BSS_MEMBER,
    rx_block_ack_max_tid: 7,
    rx_block_ack_max_window: 64,
    rx_amsdu_in_ampdu: true,
    edca_max_aifsn: 15,
    edca_max_cw_exponent: 10,
    edca_max_txop_limit_units_32_us: u16::MAX,
};

/// A backend that has sourced neither the beacon's Timestamp nor its TSF
/// control.
struct Unsourced;
impl Ieee80211Capabilities for Unsourced {
    type SppScope = oer_protocols_ieee80211_wire::security::spp::Association;
    const CAPS: crate::LowerMacCapabilities = CAPS;
}
impl AccessPointCapabilities for Unsourced {
    const ACCESS_POINT: AccessPointServices = AccessPointServices {
        beacon_timestamp: None,
        tsf_control: false,
    };
}

/// A backend whose transmitter writes the Timestamp and whose TSF control
/// is sourced.
struct Sourced;
impl Ieee80211Capabilities for Sourced {
    type SppScope = oer_protocols_ieee80211_wire::security::spp::Association;
    const CAPS: crate::LowerMacCapabilities = CAPS;
}
impl AccessPointCapabilities for Sourced {
    const ACCESS_POINT: AccessPointServices = AccessPointServices {
        beacon_timestamp: Some(BeaconTimestamp::Transmitter),
        tsf_control: true,
    };
}

#[test]
fn an_unsourced_timestamp_or_tsf_control_has_no_checked_value() {
    assert!(BeaconStamp::<Unsourced>::new(BeaconTimestamp::Transmitter).is_none());
    for control in [TsfControl::Restart, TsfControl::Stop] {
        assert!(SupportedTsfControl::<Unsourced>::new(control).is_none());
    }
}

#[test]
fn only_the_declared_timestamp_rule_has_a_stamp() {
    assert_eq!(
        BeaconStamp::<Sourced>::new(BeaconTimestamp::Transmitter).map(|stamp| stamp.source()),
        Some(BeaconTimestamp::Transmitter)
    );
    for control in [TsfControl::Restart, TsfControl::Stop] {
        assert_eq!(
            SupportedTsfControl::<Sourced>::new(control).map(|control| control.get()),
            Some(control)
        );
    }
}

#[test]
fn a_beacon_contends_as_voice() {
    assert_eq!(BEACON_ACCESS_CATEGORY, WmmAccessCategory::Voice);
}
