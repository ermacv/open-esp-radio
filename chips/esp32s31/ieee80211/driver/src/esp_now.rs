//! ESP-NOW RX-rate normalization for the ESP32-S31.
use oer_esp32s31_ieee80211_mac::rx::{RxBasebandFormat, RxPhyInfo};
use oer_ieee80211_port::espressif::EspNowPhyMode;
use oer_ieee80211_port::{MacRxMetadata, RxEvidence};

/// Missing ownership boundary for Long Range receive evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowLongRangeMissing {
    /// The five-bit RX rate has no reviewed mapping to LR descriptor codes.
    RxRateNormalization,
}

/// Status of the chip RX-rate field after applying the configured ESP-NOW PHY
/// context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowRxRateNormalization {
    /// The public RX prefix itself identifies one standard ESP-NOW-capable
    /// baseband format. This does not claim the configured preferred rate was
    /// used: a retry may have arrived in another standard format.
    DecodedStandard { format: RxBasebandFormat },
    /// No decoded standard baseband-format evidence accompanies this sample.
    /// Preserve the observation in the diagnostic, but do not publish it as
    /// a normalized standard rate.
    StandardUnavailable { observed: RxEvidence<RxPhyInfo> },
    /// LR descriptor codes are wider than the public five-bit RX `rate`
    /// field. Preserve the raw observation here, but do not expose it as a
    /// normalized rate where it could alias an ordinary standard code.
    LongRangeUnavailable {
        observed: RxEvidence<RxPhyInfo>,
        missing: EspNowLongRangeMissing,
    },
}

/// ESP-NOW-specific RX metadata with an explicit LR normalization frontier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowRxMetadata {
    pub normalized: MacRxMetadata<RxPhyInfo>,
    pub rate_normalization: EspNowRxRateNormalization,
}

/// Normalize one hardware prefix in the configured peer-PHY context.
///
/// A preferred peer PHY is not proof of the exact received retry rate. The
/// configured mode is used only to quarantine LR's truncated five-bit field;
/// a standard result additionally requires an observed decoded Dot11b, OFDM
/// or HT baseband format. All non-rate evidence is retained unchanged.
pub fn normalize_esp_now_rx_metadata(
    phy_mode: EspNowPhyMode,
    mut metadata: MacRxMetadata<RxPhyInfo>,
) -> EspNowRxMetadata {
    let observed = metadata.rate;
    let rate_normalization = if matches!(phy_mode, EspNowPhyMode::LongRange(_)) {
        metadata.rate = RxEvidence::Unavailable;
        EspNowRxRateNormalization::LongRangeUnavailable {
            observed,
            missing: EspNowLongRangeMissing::RxRateNormalization,
        }
    } else if let Some(format) = decoded_standard_format(observed) {
        EspNowRxRateNormalization::DecodedStandard { format }
    } else {
        metadata.rate = RxEvidence::Unavailable;
        EspNowRxRateNormalization::StandardUnavailable { observed }
    };
    EspNowRxMetadata {
        normalized: metadata,
        rate_normalization,
    }
}

const fn decoded_standard_format(evidence: RxEvidence<RxPhyInfo>) -> Option<RxBasebandFormat> {
    let format = match evidence {
        RxEvidence::HardwareObserved(info) | RxEvidence::ProtocolValidated(info) => {
            info.baseband_format()
        }
        RxEvidence::Unavailable => return None,
    };
    match format {
        RxBasebandFormat::Dot11b | RxBasebandFormat::Ofdm | RxBasebandFormat::Ht => Some(format),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
