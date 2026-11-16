use super::*;

mod associated_peer {
    use oer_protocols_ieee80211_core_upper_mac::protection::{BasicRates, HePacketPadding};

    use oer_protocols_ieee80211_wire::{
        ht::{HT_CAPABILITY_IE_LEN, HT_OPERATION_IE_LEN},
        protection::{ErpProtection, HtProtectionMode},
        scan::ScanRecord,
        station::AssociationResponse,
    };

    use super::super::{PhyMode, StaAssociatedPeer, StaAssociatedPeerError};

    const HE20_MCS9_CAPABILITY: [u8; 24] = [
        255, 22, 35, 0x03, 0x18, 0x9c, 0xca, 0x10, 0x80, 0x00, 0x10, 0x8a, 0x1b, 0x0d, 0xc0, 0x1f,
        0x00, 0x02, 0x82, 0x01, 0xfd, 0xff, 0xfd, 0xff,
    ];
    const HE20_OPERATION: [u8; 9] = [255, 7, 36, 0, 0, 0, 5, 0xfd, 0xff];

    fn he20_access_point() -> ScanRecord {
        let mut access_point = ScanRecord::new(
            oer_protocols_ieee80211_wire::address::Bssid::new(
                oer_protocols_ieee80211_wire::address::IndividualAddress::new([2, 0, 0, 0, 0, 1])
                    .unwrap(),
            )
            .unwrap(),
        );
        access_point
            .set_supported_rates(&[0x82, 0x84, 0x8b, 0x96])
            .unwrap();
        let mut ht_capability = [0; HT_CAPABILITY_IE_LEN];
        ht_capability[4] = 0x17;
        access_point.set_ht_capability_ie(Some(ht_capability));
        access_point
            .set_he_capability_ie(&HE20_MCS9_CAPABILITY)
            .unwrap();
        access_point.set_he_operation_ie(&HE20_OPERATION).unwrap();
        access_point
    }

    fn response(status_code: u16) -> AssociationResponse {
        AssociationResponse {
            capability_info: 0,
            status_code,
            association_id: 7,
            ht_capability: oer_protocols_ieee80211_wire::scan::ElementPresence::Present,
            he: oer_protocols_ieee80211_wire::station::ResponseHeElements::new(
                &HE20_MCS9_CAPABILITY,
                &HE20_OPERATION,
            )
            .unwrap(),
            wmm_element: oer_protocols_ieee80211_wire::scan::ElementPresence::Absent,
            wmm_parameters: None,
            association_comeback_tu: None,
        }
    }

    fn eight_microseconds(_: &[u8]) -> HePacketPadding {
        HePacketPadding::Us8
    }

    #[test]
    fn an_he_association_keeps_the_peer_s_he_state() {
        let padding = std::boxed::Box::new(HePacketPadding::Us8);
        let peer = StaAssociatedPeer::derive(
            &he20_access_point(),
            oer_protocols_ieee80211_wire::protection::ErpProtection::NONE,
            &response(0),
            oer_protocols_ieee80211_core_upper_mac::phy_mode::NegotiatedPhy::He { power_capability: oer_protocols_ieee80211_wire::station::StaPowerCapability::new(0, 20).unwrap(), he_ul_mu_power: oer_protocols_ieee80211_wire::station::HeUlMuPowerCapability::from_rate_power_indices([20; 10]).unwrap() }.mode(),
            move |capability| {
                assert_eq!(capability, HE20_MCS9_CAPABILITY);
                let result = *padding;
                drop(padding);
                result
            },
        )
        .unwrap();
        assert_eq!(
            peer.phy,
            oer_protocols_ieee80211_core_upper_mac::phy_mode::NegotiatedPhy::He { power_capability: oer_protocols_ieee80211_wire::station::StaPowerCapability::new(0, 20).unwrap(), he_ul_mu_power: oer_protocols_ieee80211_wire::station::HeUlMuPowerCapability::from_rate_power_indices([20; 10]).unwrap() }.mode()
        );
        assert_eq!(peer.ht_ampdu_parameters, 0x17);
        assert_eq!(peer.he_bss_color, 5);
        assert!(peer.he_capabilities.is_some());
        assert!(peer.he_peer_state.is_some());
        assert_eq!(peer.protection.he_packet_padding, HePacketPadding::Us8);
        // An HT association keeps the HE capabilities but no HE state.
        let ht = StaAssociatedPeer::derive(
            &he20_access_point(),
            oer_protocols_ieee80211_wire::protection::ErpProtection::NONE,
            &response(0),
            oer_protocols_ieee80211_core_upper_mac::phy_mode::NegotiatedPhy::Ht {
                width: oer_protocols_ieee80211_contracts_port::Width::Mhz20,
            }
            .mode(),
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
                oer_protocols_ieee80211_wire::protection::ErpProtection::NONE,
                &response(17),
                oer_protocols_ieee80211_core_upper_mac::phy_mode::NegotiatedPhy::He { power_capability: oer_protocols_ieee80211_wire::station::StaPowerCapability::new(0, 20).unwrap(), he_ul_mu_power: oer_protocols_ieee80211_wire::station::HeUlMuPowerCapability::from_rate_power_indices([20; 10]).unwrap() }.mode(),
                eight_microseconds,
            ),
            Err(StaAssociatedPeerError::Rejected(17))
        );
    }

    #[test]
    fn the_peer_carries_every_bss_protection_fact() {
        let mut access_point = ScanRecord::new(
            oer_protocols_ieee80211_wire::address::Bssid::new(
                oer_protocols_ieee80211_wire::address::IndividualAddress::new([2, 0, 0, 0, 0, 1])
                    .unwrap(),
            )
            .unwrap(),
        );
        access_point.capability_info = 0x0421;
        access_point
            .set_supported_rates(&[0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24])
            .unwrap();
        access_point
            .set_extended_supported_rates(&[0x30, 0x48, 0x60, 0x6c])
            .unwrap();
        let mut ht_operation = [0; HT_OPERATION_IE_LEN];
        ht_operation[..2].copy_from_slice(&[61, 22]);
        ht_operation[4] = 0x03;
        access_point.set_ht_operation_ie(Some(ht_operation));
        let peer = StaAssociatedPeer::derive(
            &access_point,
            oer_protocols_ieee80211_wire::protection::ErpProtection::from_information(Some(0x07)),
            &response(0),
            oer_protocols_ieee80211_core_upper_mac::phy_mode::NegotiatedPhy::Ht {
                width: oer_protocols_ieee80211_contracts_port::Width::Mhz20,
            }
            .mode(),
            eight_microseconds,
        )
        .unwrap();
        let rate_peer = peer.rate_peer();
        assert_eq!(rate_peer.legacy_rates.rates().count(), 12);
        assert!(
            rate_peer
                .legacy_rates
                .contains(oer_protocols_ieee80211_wire::phy::LegacyRate::Ofdm54M)
        );
        let protection = peer.protection;
        assert_eq!(
            protection.erp,
            ErpProtection::new(
                oer_protocols_ieee80211_wire::protection::ErpInformation::from_bytes([0])
                    .with_use_protection(true)
                    .with_barker(true)
            )
        );
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

mod request {
    use super::super::{StaPhyRequest, StaPhyRequestError};
    use oer_protocols_ieee80211_contracts_port::{
        BandSet, FormatWidths, HardwareServices, Ieee80211Capabilities, LowerMacCapabilities,
        PhyFormat, PhyFormatSet, RateSupport, ReceiveFilter, Support, Width, WidthSet,
    };

    const fn capabilities(formats: FormatWidths) -> LowerMacCapabilities {
        LowerMacCapabilities {
            bands: BandSet::GHZ2_4,
            formats,
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
        }
    }

    /// Non-HT at 20 MHz and HT at 20 and 40 MHz: no HE.
    struct WithoutHe;
    impl Ieee80211Capabilities for WithoutHe {
        type SppScope = oer_protocols_ieee80211_wire::security::spp::Association;
        const CAPS: LowerMacCapabilities = capabilities(FormatWidths {
            non_ht: WidthSet::MHZ20,
            ht: WidthSet::MHZ20.union(WidthSet::MHZ40),
            he: WidthSet::NONE,
        });
    }

    /// Every format at 20 MHz only.
    struct Narrow;
    impl Ieee80211Capabilities for Narrow {
        type SppScope = oer_protocols_ieee80211_wire::security::spp::Association;
        const CAPS: LowerMacCapabilities = capabilities(FormatWidths {
            non_ht: WidthSet::MHZ20,
            ht: WidthSet::MHZ20,
            he: WidthSet::MHZ20,
        });
    }

    const BELOW_HE: PhyFormatSet = PhyFormatSet::NON_HT.union(PhyFormatSet::HT);

    /// The constructor answers a request at run time as it does a constant:
    /// each refusal with its datum, and the requests the chip offers built.
    #[test]
    fn a_request_outside_the_chip_is_refused_with_its_reason() {
        assert_eq!(
            StaPhyRequest::<WithoutHe>::new(PhyFormatSet::NONE, Width::Mhz20),
            Err(StaPhyRequestError::NoFormat)
        );
        assert_eq!(
            StaPhyRequest::<WithoutHe>::new(PhyFormatSet::HE, Width::Mhz20),
            Err(StaPhyRequestError::FormatNotOffered(PhyFormat::He))
        );
        assert_eq!(
            StaPhyRequest::<WithoutHe>::new(BELOW_HE.union(PhyFormatSet::HE), Width::Mhz40),
            Err(StaPhyRequestError::FormatNotOffered(PhyFormat::He))
        );
        let below_he = StaPhyRequest::<WithoutHe>::new(BELOW_HE, Width::Mhz40)
            .expect("non-HT and HT up to 40 MHz are offered");
        assert_eq!(
            (below_he.formats(), below_he.widest()),
            (BELOW_HE, Width::Mhz40)
        );
        assert_eq!(
            StaPhyRequest::<Narrow>::new(BELOW_HE, Width::Mhz40),
            Err(StaPhyRequestError::WidthNotOffered(Width::Mhz40))
        );
        assert_eq!(
            StaPhyRequest::<Narrow>::new(PhyFormatSet::HE, Width::Mhz40),
            Err(StaPhyRequestError::WidthNotOffered(Width::Mhz40))
        );
        assert!(StaPhyRequest::<Narrow>::new(PhyFormatSet::HE, Width::Mhz20).is_ok());
    }
}
