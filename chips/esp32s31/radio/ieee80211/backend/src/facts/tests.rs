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
