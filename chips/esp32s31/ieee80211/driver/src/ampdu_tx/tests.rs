use super::*;
use oer_esp32s31_ieee80211_mac::tx::{HtChannelWidth, HtGuardInterval, HtMcs};

#[test]
fn role_adapter_changes_only_interface_and_key_authority() {
    let inputs = HtAmpduPublicationInputs {
        rate: HtRate {
            mcs: HtMcs::Mcs7,
            channel_width: HtChannelWidth::Mhz20,
            guard_interval: HtGuardInterval::Long800Ns,
        },
        aggregate_length: 4_096,
        subframes: 4,
        protection_spacing: HtProtectionSpacing::Density0To4,
        data_power_primary: 1,
        data_power_alternate: 2,
        control: TxControlFrame {
            protection: oer_esp32s31_ieee80211_mac::tx::protection::TxProtection::RtsCts {
                rate: oer_esp32s31_ieee80211_mac::tx::LegacyRate::Ofdm24M,
            },
            power_primary: 3,
            power_alternate: 4,
        },
        aifsn: 3,
        contention_window: 15,
        scheduler_priority: 1,
        packet_priority: 1,
    };
    let station = ht_ampdu_publication_config(
        AmpduTxRoleAdapter {
            interface: MacInterface::Station,
            hardware_key_selector: 2,
        },
        inputs,
    )
    .unwrap();
    let access_point = ht_ampdu_publication_config(
        AmpduTxRoleAdapter {
            interface: MacInterface::AccessPoint,
            hardware_key_selector: 8,
        },
        inputs,
    )
    .unwrap();
    assert_eq!(station.interface, MacInterface::Station);
    assert_eq!(access_point.interface, MacInterface::AccessPoint);
    assert_eq!(station.hardware_key_selector, 2);
    assert_eq!(access_point.hardware_key_selector, 8);
    assert_eq!(station.aggregate_length, access_point.aggregate_length);
    assert_eq!(station.rate, access_point.rate);
    assert_eq!(station.control, inputs.control);
    assert_eq!(access_point.control, inputs.control);
}
