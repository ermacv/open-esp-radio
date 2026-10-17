use PhyMode::{He20, Ht20, Ht40, Legacy};

use Preference::{Automatic, ForceHt20, PreferHe20};

use super::*;

#[test]
fn preference_respects_admitted_modes() {
    for (ht40, he20, automatic, prefer_he20) in [
        (false, false, Ht20, Ht20),
        (true, false, Ht40, Ht40),
        (false, true, He20, He20),
        (true, true, Ht40, He20),
    ] {
        assert_eq!(select_phy(Automatic, true, ht40, he20), automatic);
        assert_eq!(select_phy(PreferHe20, true, ht40, he20), prefer_he20);
        assert_eq!(select_phy(ForceHt20, true, ht40, he20), Ht20);
    }
}

#[test]
fn a_peer_without_ht_is_legacy_whatever_the_preference() {
    for preference in [Automatic, PreferHe20, ForceHt20] {
        assert_eq!(select_phy(preference, false, false, false), Legacy);
        assert_eq!(select_phy(preference, false, true, true), Legacy);
    }
}

mod associated_peer {
    use oer_ieee80211_mac::{
        protection::{ErpProtection, HtProtectionMode},
        scan::ScanRecord,
        station::AssociationResponse,
    };
    use oer_ieee80211_upper_mac::{BasicRates, HePacketPadding};

    use super::super::{PhyMode, StaAssociatedPeer, StaAssociatedPeerError};

    const HE20_MCS9_CAPABILITY: [u8; 24] = [
        255, 22, 35, 0x03, 0x18, 0x9c, 0xca, 0x10, 0x80, 0x00, 0x10, 0x8a, 0x1b, 0x0d, 0xc0, 0x1f,
        0x00, 0x02, 0x82, 0x01, 0xfd, 0xff, 0xfd, 0xff,
    ];
    const HE20_OPERATION: [u8; 9] = [255, 7, 36, 0, 0, 0, 5, 0xfd, 0xff];

    fn he20_access_point() -> ScanRecord {
        let mut access_point = ScanRecord::new(
            oer_ieee80211_mac::address::Bssid::new(
                oer_ieee80211_mac::address::IndividualAddress::new([2, 0, 0, 0, 0, 1]).unwrap(),
            )
            .unwrap(),
            6,
            -40,
        );
        access_point.supported_rates[..4].copy_from_slice(&[0x82, 0x84, 0x8b, 0x96]);
        access_point.supported_rates_len = 4;
        access_point.ht_capability_ie_present = true;
        access_point.ht_capability_ie[4] = 0x17;
        access_point.he_capability_ie[..HE20_MCS9_CAPABILITY.len()]
            .copy_from_slice(&HE20_MCS9_CAPABILITY);
        access_point.he_capability_ie_len = HE20_MCS9_CAPABILITY.len() as u8;
        access_point.he_operation_ie[..HE20_OPERATION.len()].copy_from_slice(&HE20_OPERATION);
        access_point.he_operation_ie_len = HE20_OPERATION.len() as u8;
        access_point
    }

    fn response(status_code: u16) -> AssociationResponse {
        AssociationResponse {
            capability_info: 0,
            status_code,
            association_id: 7,
            ht_capability: true,
            he_capability: true,
            he_operation: true,
            wmm: true,
            wmm_parameters: None,
            association_comeback_tu: None,
        }
    }

    fn eight_microseconds(_: &[u8]) -> HePacketPadding {
        HePacketPadding::Us8
    }

    #[test]
    fn an_he_association_keeps_the_peer_s_he_state() {
        let peer = StaAssociatedPeer::derive(
            &he20_access_point(),
            &response(0),
            PhyMode::He20,
            eight_microseconds,
        )
        .unwrap();
        assert_eq!(peer.phy, PhyMode::He20);
        assert_eq!(peer.ht_ampdu_parameters, 0x17);
        assert_eq!(peer.he_bss_color, 5);
        assert!(peer.he_capabilities.is_some());
        assert!(peer.he_peer_state.is_some());
        assert_eq!(peer.protection.he_packet_padding, HePacketPadding::Us8);
        // An HT association keeps the HE capabilities but no HE state.
        let ht = StaAssociatedPeer::derive(
            &he20_access_point(),
            &response(0),
            PhyMode::Ht20,
            eight_microseconds,
        )
        .unwrap();
        assert!(ht.he_capabilities.is_some() && ht.he_peer_state.is_none());
    }

    #[test]
    fn a_refused_association_has_no_peer() {
        assert_eq!(
            StaAssociatedPeer::derive(
                &he20_access_point(),
                &response(17),
                PhyMode::He20,
                eight_microseconds,
            ),
            Err(StaAssociatedPeerError::Rejected(17))
        );
    }

    #[test]
    fn the_peer_carries_every_bss_protection_fact() {
        let mut access_point = ScanRecord::new(
            oer_ieee80211_mac::address::Bssid::new(
                oer_ieee80211_mac::address::IndividualAddress::new([2, 0, 0, 0, 0, 1]).unwrap(),
            )
            .unwrap(),
            6,
            -40,
        );
        access_point.capability_info = 0x0421;
        access_point.supported_rates = [0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24];
        access_point.supported_rates_len = 8;
        access_point.extended_supported_rates[..4].copy_from_slice(&[0x30, 0x48, 0x60, 0x6c]);
        access_point.extended_supported_rates_len = 4;
        access_point.erp_information = Some(0x07);
        access_point.ht_operation_ie[..2].copy_from_slice(&[61, 22]);
        access_point.ht_operation_ie[4] = 0x03;
        access_point.ht_operation_ie_present = true;
        let peer = StaAssociatedPeer::derive(
            &access_point,
            &response(0),
            PhyMode::Ht20,
            eight_microseconds,
        )
        .unwrap();
        let rate_peer = peer.rate_peer();
        assert_eq!(rate_peer.legacy_rates.rates().count(), 12);
        assert!(
            rate_peer
                .legacy_rates
                .contains(oer_ieee80211_mac::phy::LegacyRate::Ofdm54M)
        );
        let protection = peer.protection;
        assert_eq!(protection.erp, ErpProtection::new(true, true));
        assert_eq!(protection.ht, HtProtectionMode::NonHtMixed);
        assert!(protection.short_preamble);
        assert_eq!(
            protection.basic_rates,
            BasicRates::from_rate_elements(&[0x82, 0x84, 0x8b, 0x96], &[])
        );
        assert_eq!(protection.he_txop_rts_threshold, None);
        // No HE Capabilities element: no padding to read.
        assert_eq!(protection.he_packet_padding, HePacketPadding::None);
    }
}
