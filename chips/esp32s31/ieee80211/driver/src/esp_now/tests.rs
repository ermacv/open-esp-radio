use super::*;
use oer_ieee80211_port::espressif::EspNowOfdmRate;

#[test]
fn lr_rx_rate_is_quarantined_from_the_truncated_public_field() {
    let lr_raw = RxPhyInfo {
        rate: 0x0a,
        bb_format: 0,
        he_siga1: 0,
        he_siga2: 0,
    };
    let mut metadata = MacRxMetadata::unavailable();
    metadata.rate = RxEvidence::HardwareObserved(lr_raw);

    let lr = normalize_esp_now_rx_metadata(
        EspNowPhyMode::LongRange(oer_ieee80211_port::espressif::EspressifLongRangeRate::Kbps500),
        metadata,
    );
    assert_eq!(lr.normalized.rate, RxEvidence::Unavailable);
    assert_eq!(
        lr.rate_normalization,
        EspNowRxRateNormalization::LongRangeUnavailable {
            observed: RxEvidence::HardwareObserved(lr_raw),
            missing: EspNowLongRangeMissing::RxRateNormalization,
        }
    );

    let standard = normalize_esp_now_rx_metadata(EspNowPhyMode::LegacyDsss1M, metadata);
    assert_eq!(
        standard.normalized.rate,
        RxEvidence::HardwareObserved(lr_raw)
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
    metadata.rate = RxEvidence::HardwareObserved(non_standard_raw);
    let unavailable = normalize_esp_now_rx_metadata(
        EspNowPhyMode::StandardP2pOfdm(EspNowOfdmRate::Mbps6),
        metadata,
    );
    assert_eq!(unavailable.normalized.rate, RxEvidence::Unavailable);
    assert_eq!(
        unavailable.rate_normalization,
        EspNowRxRateNormalization::StandardUnavailable {
            observed: RxEvidence::HardwareObserved(non_standard_raw),
        }
    );
}
