#![no_std]
use oer_protocols_ieee80211_contracts_port::{
    BandSet, FormatWidth, FormatWidths, HardwareServices, Ieee80211Capabilities,
    LowerMacCapabilities, PhyFormat, PhyFormatSet, RateSupport, ReceiveFilter, Support, WidthSet,
};
use oer_protocols_ieee80211_wire::channel::ChannelWidth;

/// The ESP32-S31's pairs: non-HT at 20 MHz, HT at 20 and 40 MHz, HE at
/// 20 MHz.
pub struct S31;

impl Ieee80211Capabilities for S31 {
    type SppScope = oer_protocols_ieee80211_wire::security::spp::Association;
    const CAPS: LowerMacCapabilities = LowerMacCapabilities {
        bands: BandSet::GHZ2_4,
        formats: FormatWidths {
            non_ht: WidthSet::MHZ20,
            ht: WidthSet::MHZ20.union(WidthSet::MHZ40),
            he: WidthSet::MHZ20,
        },
        rates: RateSupport {
            dsss_cck: Support::Supported,
            ofdm: Support::Supported,
            ht_max_mcs: oer_protocols_ieee80211_wire::phy::HtMcs::new(7),
            he_max_mcs: oer_protocols_ieee80211_wire::phy::HeMcs::new(9),
            he_dcm: Support::Supported,
            he_ldpc: Support::Supported,
            spatial_streams: 1,
        },
        services: HardwareServices::FCS,
        vifs: 2,
        tx_queues: 4,
        max_mpdu_length: 2304,
        tx_power_ceiling_min_dbm: Some(0),
        individual_no_ack: PhyFormatSet::NON_HT,
        station_receive_filters: ReceiveFilter::BSS_MEMBER,
        access_point_receive_filters: ReceiveFilter::BSS_MEMBER,
        rx_block_ack_max_tid: 7,
        rx_block_ack_max_window: 64,
        rx_amsdu_in_ampdu: true,
        edca_max_aifsn: 15,
        edca_max_cw_exponent: 10,
        edca_max_txop_limit_units_32_us: u16::MAX,
    };
}

/// HT at 40 MHz: the S31 offers the pair, and the constant builds.
pub const HT_AT_40_MHZ: FormatWidth<S31> =
    match FormatWidth::new(PhyFormat::Ht, ChannelWidth::Mhz40Above) {
        Some(pair) => pair,
        None => panic!("the ESP32-S31 offers no HT at 40 MHz"),
    };

/// HE at 40 MHz: the S31 does not offer the pair, and the constant does not
/// build.
#[cfg(ui_reject)]
pub const HE_AT_40_MHZ: FormatWidth<S31> =
    match FormatWidth::new(PhyFormat::He, ChannelWidth::Mhz40Above) {
        Some(pair) => pair,
        None => panic!("the ESP32-S31 offers no HE at 40 MHz"), // ui-error: E0080: evaluation panicked: the ESP32-S31 offers no HE at 40 MHz
    };
