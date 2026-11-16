//! The MAC's capabilities on 2.4 GHz.

use super::*;
use crate::ordinary_tx::{TX_CCMP_MIC_SIZE, TX_FCS_SIZE, TX_METADATA_SIZE};
use oer_protocols_ieee80211_contracts_port::{HardwareServices, PhyRate};
use oer_protocols_ieee80211_wire::{
    channel::{Channel, ChannelWidth},
    phy::LegacyRate,
    qos::WmmAccessCategory,
};

#[test]
fn capabilities_are_the_s31_limits_on_2_4_ghz() {
    let caps = esp32s31_lower_mac_capabilities(512);
    assert_eq!(
        caps.services,
        ESP32S31_MAC_SERVICE_CAPABILITIES
            .operations
            .hardware_services()
    );
    assert!(caps.services.contains(HardwareServices::FCS));
    assert!(caps.supports_channel(Channel::ghz2_4(6, ChannelWidth::Mhz40Above).unwrap()));
    assert!(!caps.supports_channel(Channel::ghz5(36, ChannelWidth::Mhz20).unwrap()));
    assert_eq!(caps.vifs, 2);
    assert_eq!(caps.tx_queues, 4);
    assert_eq!(caps.tx_queue(WmmAccessCategory::Voice), 3);
    // Metadata word, MPDU, MIC and FCS fill the slot.
    assert_eq!(
        usize::from(caps.max_mpdu_length) + TX_METADATA_SIZE + TX_CCMP_MIC_SIZE + TX_FCS_SIZE,
        512
    );
    assert!(
        caps.individual_no_ack
            .contains_rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
    );
    assert_eq!(
        caps.access_point_receive_filters,
        ReceiveFilter::BSS_MEMBER.union(ReceiveFilter::PROBE_REQUESTS)
    );
}

#[test]
fn the_longest_mpdu_is_bounded_by_the_transmit_programs_too() {
    // A large buffer does not admit a PSDU the legacy program cannot carry.
    let capabilities = esp32s31_lower_mac_capabilities(8192);
    assert_eq!(
        usize::from(capabilities.max_mpdu_length),
        MAX_PROGRAM_PSDU - TX_CCMP_MIC_SIZE - TX_FCS_SIZE
    );
    // The model's buffers stay the bound below it.
    assert!(esp32s31_lower_mac_capabilities(512).max_mpdu_length < capabilities.max_mpdu_length);
}

/// The HE element's Supported Channel Width Set comes from the capability's
/// HE widths through the contract's conversion: the ESP32-S31's (HE at
/// 20 MHz) gives none; a set with HE at 40 MHz in the 2.4 GHz band gives B0;
/// 40 MHz alone in the 5 GHz band has no encoding and is refused.
#[test]
fn the_he_element_carries_the_widths_of_the_capability() {
    use oer_protocols_ieee80211_contracts_port::{BandSet, FormatWidths, WidthSet};
    use oer_protocols_ieee80211_wire::he::{
        HeChannelWidthSet, HeLocalCapabilities, HeMacCapabilities, HeMcsNssSupport,
        HePhyCapabilities, he_capability_ie,
    };
    let local = HeLocalCapabilities {
        mac: HeMacCapabilities::from_bytes([0; 6]),
        phy: HePhyCapabilities::from_bytes([0; 11]),
        receive: HeMcsNssSupport::Mcs0To9,
        transmit: HeMcsNssSupport::Mcs0To9,
    };
    let field = |widths: HeChannelWidthSet| {
        let element = he_capability_ie(local, widths);
        HePhyCapabilities::from_bytes(element[9..20].try_into().unwrap())
            .channel_width_set()
            .get()
    };
    assert_eq!(
        crate::facts::ESP32S31_FORMAT_WIDTHS.he_channel_width_set(crate::facts::ESP32S31_BANDS),
        Some(crate::facts::ESP32S31_HE_CHANNEL_WIDTH_SET)
    );
    assert_eq!(field(crate::facts::ESP32S31_HE_CHANNEL_WIDTH_SET), 0);
    let wide = FormatWidths {
        he: WidthSet::MHZ20.union(WidthSet::MHZ40),
        ..crate::facts::ESP32S31_FORMAT_WIDTHS
    };
    assert_eq!(
        wide.he_channel_width_set(BandSet::GHZ2_4).map(field),
        Some(0b01)
    );
    assert_eq!(wide.he_channel_width_set(BandSet::GHZ5), None);
    assert_eq!(wide.he_channel_width_set(BandSet::NONE), None);
}

/// The ESP32-S31's pairs: non-HT at 20 MHz, HT at 20 and 40 MHz, HE at
/// 20 MHz are offered; HE at 40 MHz is not.
#[test]
fn the_esp32s31_offers_its_five_pairs_and_not_he_at_40_mhz() {
    use oer_protocols_ieee80211_contracts_port::{FormatWidth, Ieee80211Capabilities, PhyFormat};
    use oer_protocols_ieee80211_wire::channel::ChannelWidth::{Mhz20, Mhz40Above, Mhz40Below};
    struct Esp32s31;
    impl Ieee80211Capabilities for Esp32s31 {
        type SppScope = oer_protocols_ieee80211_wire::security::spp::Association;
        const CAPS: LowerMacCapabilities = esp32s31_lower_mac_capabilities(1664);
    }
    for (format, width) in [
        (PhyFormat::NonHt, Mhz20),
        (PhyFormat::Ht, Mhz20),
        (PhyFormat::Ht, Mhz40Above),
        (PhyFormat::Ht, Mhz40Below),
        (PhyFormat::He, Mhz20),
    ] {
        assert!(FormatWidth::<Esp32s31>::new(format, width).is_some());
    }
    for (format, width) in [
        (PhyFormat::He, Mhz40Above),
        (PhyFormat::He, Mhz40Below),
        (PhyFormat::NonHt, Mhz40Above),
    ] {
        assert!(FormatWidth::<Esp32s31>::new(format, width).is_none());
    }
}
