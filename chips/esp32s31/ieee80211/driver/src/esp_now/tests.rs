use super::*;

#[test]
fn capability_surface_is_live_for_every_standard_phy_and_closed_for_lr() {
    for mode in [
        EspNowPhyMode::LegacyDsss1M,
        EspNowPhyMode::StandardP2pOfdm(EspNowOfdmRate::Mbps6),
        EspNowPhyMode::StandardP2pOfdm(EspNowOfdmRate::Mbps54),
        EspNowPhyMode::StandardP2pHt20(oer_ieee80211_softmac::EspNowHt20Rate::new(
            EspNowHtMcs::Mcs0,
            EspNowHtGuardInterval::Short400Ns,
        )),
        EspNowPhyMode::StandardP2pHt20(oer_ieee80211_softmac::EspNowHt20Rate::new(
            EspNowHtMcs::Mcs7,
            EspNowHtGuardInterval::Long800Ns,
        )),
    ] {
        assert_eq!(esp32s31_esp_now_phy_support(mode), EspNowPhySupport::Live);
    }
    for rate in [
        oer_ieee80211_softmac::EspressifLongRangeRate::Kbps250,
        oer_ieee80211_softmac::EspressifLongRangeRate::Kbps500,
    ] {
        assert_eq!(
            esp32s31_esp_now_phy_support(EspNowPhyMode::LongRange(rate)),
            EspNowPhySupport::LongRangeFailClosed {
                tx_missing: EspNowLongRangeMissing::TxPlcpQueueVector,
                rx_missing: EspNowLongRangeMissing::RxRateNormalization,
            }
        );
    }
}

#[test]
fn every_ht20_sgi_gap_rate_selects_its_same_mcs_lgi_record() {
    for mcs in [
        EspNowHtMcs::Mcs0,
        EspNowHtMcs::Mcs1,
        EspNowHtMcs::Mcs2,
        EspNowHtMcs::Mcs3,
        EspNowHtMcs::Mcs4,
        EspNowHtMcs::Mcs5,
        EspNowHtMcs::Mcs6,
    ] {
        let OrdinaryRetryRatePolicy::P2pHtSgiFallback(schedule) =
            p2p_ht_sgi_retry_policy(8 - mcs.index())
        else {
            panic!("SGI gap must use the typed fallback policy")
        };
        assert_eq!(schedule.schedule().index(), 8 - mcs.index());
    }
}

#[test]
fn lr_rx_rate_is_quarantined_from_the_truncated_public_field() {
    let lr_raw = RxPhyInfo {
        rate: 0x0a,
        bb_format: 0,
        he_siga1: 0,
        he_siga2: 0,
    };
    let mut metadata = MacRxMetadata::unavailable();
    metadata.rate = MacRxEvidence::HardwareObserved(lr_raw);

    let lr = normalize_esp_now_rx_metadata(
        EspNowPhyMode::LongRange(oer_ieee80211_softmac::EspressifLongRangeRate::Kbps500),
        metadata,
    );
    assert_eq!(lr.normalized.rate, MacRxEvidence::Unavailable);
    assert_eq!(
        lr.rate_normalization,
        EspNowRxRateNormalization::LongRangeUnavailable {
            observed: MacRxEvidence::HardwareObserved(lr_raw),
            missing: EspNowLongRangeMissing::RxRateNormalization,
        }
    );

    let standard = normalize_esp_now_rx_metadata(EspNowPhyMode::LegacyDsss1M, metadata);
    assert_eq!(
        standard.normalized.rate,
        MacRxEvidence::HardwareObserved(lr_raw)
    );
    assert_eq!(
        standard.rate_normalization,
        EspNowRxRateNormalization::DecodedStandard {
            format: RxBasebandFormat::Dot11b,
        }
    );

    let non_standard_raw = RxPhyInfo {
        rate: 7,
        bb_format: RxBasebandFormat::HeSu.raw(),
        he_siga1: 0,
        he_siga2: 0,
    };
    metadata.rate = MacRxEvidence::HardwareObserved(non_standard_raw);
    let unavailable = normalize_esp_now_rx_metadata(
        EspNowPhyMode::StandardP2pOfdm(EspNowOfdmRate::Mbps6),
        metadata,
    );
    assert_eq!(unavailable.normalized.rate, MacRxEvidence::Unavailable);
    assert_eq!(
        unavailable.rate_normalization,
        EspNowRxRateNormalization::StandardUnavailable {
            observed: MacRxEvidence::HardwareObserved(non_standard_raw),
        }
    );
}
