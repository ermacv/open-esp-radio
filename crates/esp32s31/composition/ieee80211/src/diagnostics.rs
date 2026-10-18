//! Stable, value-only diagnostic observations for ESP32-S31 composition.
//!
//! These types deliberately hide MAC descriptors, raw RX prefixes and
//! protocol-owner enums from HIL. Decoding remains inside the driver graph;
//! observers receive borrowed payloads and copied facts only.

#![cfg(feature = "diagnostics")]

use oer_esp32s31_ieee80211_runtime::diagnostics::network::RxObservedEthernetFrame;

use oer_esp32s31_ieee80211_mac::rx::{
    HeSuSignal, HtDuplicateRxClassification, HtSignal, RxPhyInfo,
};

use oer_esp32s31_ieee80211_sta::connected_rx::ConnectedRxEvent;

use oer_ieee80211_softmac::{MacRxEvidence, MacRxMetadata};

/// Evidence provenance retained by a decoded diagnostic fact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReceiveEvidence<T> {
    Hardware(T),
    Protocol(T),
    Unavailable,
}

impl<T> From<MacRxEvidence<T>> for ReceiveEvidence<T> {
    fn from(evidence: MacRxEvidence<T>) -> Self {
        match evidence {
            MacRxEvidence::HardwareObserved(value) => Self::Hardware(value),
            MacRxEvidence::ProtocolValidated(value) => Self::Protocol(value),
            MacRxEvidence::Unavailable => Self::Unavailable,
        }
    }
}

/// Decoded HE-SU fields required by diagnostic rate evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeSuRxObservation {
    pub mcs: u8,
    pub guard_interval_and_ltf: u8,
    pub bandwidth_mhz: u16,
    pub dcm: bool,
    pub ldpc: bool,
}

impl From<HeSuSignal> for HeSuRxObservation {
    fn from(signal: HeSuSignal) -> Self {
        Self {
            mcs: signal.mcs,
            guard_interval_and_ltf: signal.guard_interval_and_ltf.encoding(),
            bandwidth_mhz: signal.bandwidth.mhz(),
            dcm: signal.dcm,
            ldpc: signal.ldpc,
        }
    }
}

/// Decoded HT-SIG fields required by diagnostic rate evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtRxObservation {
    pub mcs: u8,
    pub bandwidth_mhz: u16,
    pub short_guard_interval: bool,
    pub aggregation: bool,
    /// True only for the standard MCS32 selector with HT40 geometry.
    pub duplicate_mcs32: bool,
    /// MCS32 was observed with non-HT40 geometry and was not normalized as
    /// duplicate mode.
    pub duplicate_mcs32_width_mismatch: bool,
}

impl From<HtSignal> for HtRxObservation {
    fn from(signal: HtSignal) -> Self {
        let duplicate = signal.ht_duplicate_mcs32_classification();
        Self {
            mcs: signal.mcs,
            bandwidth_mhz: u16::from(signal.channel_width_mhz),
            short_guard_interval: signal.short_guard_interval,
            aggregation: signal.aggregation,
            duplicate_mcs32: matches!(duplicate, HtDuplicateRxClassification::Ht40(_)),
            duplicate_mcs32_width_mismatch: matches!(
                duplicate,
                HtDuplicateRxClassification::Mismatch { .. }
            ),
        }
    }
}

#[cfg(test)]
mod tests;

/// Decoded public RX-control facts; no raw header storage escapes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecodedRxPhyObservation {
    pub baseband_format: u8,
    /// Five-bit public RX-control summary. For HT, use [`Self::ht`]'s
    /// seven-bit format-specific MCS field; this byte cannot represent MCS32.
    pub rate: u8,
    /// Copied format-specific signal words from the RX descriptor prefix.
    pub signal_words: [u32; 2],
    pub ht: Option<HtRxObservation>,
    pub he_su: Option<HeSuRxObservation>,
}

impl From<RxPhyInfo> for DecodedRxPhyObservation {
    fn from(phy: RxPhyInfo) -> Self {
        Self {
            baseband_format: phy.baseband_format().raw(),
            rate: phy.rate,
            signal_words: [phy.he_siga1, u32::from(phy.he_siga2)],
            ht: phy.ht_signal().map(Into::into),
            he_su: phy.he_su_signal().map(Into::into),
        }
    }
}

/// Stable decoded connected-RX event for diagnostics and HIL evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedRxObservation<'frame> {
    Beacon {
        s_mpdu: ReceiveEvidence<bool>,
    },
    Ethernet {
        frame: RxObservedEthernetFrame<'frame>,
        qos_sequence:
            Option<oer_esp32s31_ieee80211_runtime::diagnostics::network::RxQosSequenceObservation>,
        s_mpdu: ReceiveEvidence<bool>,
        ampdu: ReceiveEvidence<bool>,
        phy: ReceiveEvidence<DecodedRxPhyObservation>,
    },
    Other,
}

impl<'frame> ConnectedRxObservation<'frame> {
    pub(crate) fn decode(event: ConnectedRxEvent<'frame>, include_phy: bool) -> Self {
        match event {
            ConnectedRxEvent::Beacon { metadata, .. } => Self::Beacon {
                s_mpdu: metadata.s_mpdu.into(),
            },
            ConnectedRxEvent::Ethernet {
                frame,
                metadata,
                raw,
                ..
            } => Self::Ethernet {
                frame: frame.into(),
                qos_sequence:
                    oer_esp32s31_ieee80211_runtime::diagnostics::network::decode_public_qos_sequence(
                        raw,
                    ),
                s_mpdu: metadata.s_mpdu.into(),
                ampdu: metadata.ampdu.into(),
                phy: if include_phy {
                    decode_phy(metadata)
                } else {
                    ReceiveEvidence::Unavailable
                },
            },
            _ => Self::Other,
        }
    }
}

fn decode_phy(metadata: MacRxMetadata<RxPhyInfo>) -> ReceiveEvidence<DecodedRxPhyObservation> {
    match metadata.rate {
        MacRxEvidence::HardwareObserved(phy) => ReceiveEvidence::Hardware(phy.into()),
        MacRxEvidence::ProtocolValidated(phy) => ReceiveEvidence::Protocol(phy.into()),
        MacRxEvidence::Unavailable => ReceiveEvidence::Unavailable,
    }
}

/// Observation-only hook. The decoded event is borrowed synchronously and
/// production delivery proceeds independently after the callback returns.
pub trait ConnectedRxObserver: Sync {
    /// Request decoded PHY evidence for this Ethernet frame.
    ///
    /// PHY decoding is intentionally demand-driven: high-rate diagnostic
    /// observers can sample it without charging every frame in the RX hot
    /// path. The frame is already a stable, decoded driver observation; raw
    /// MAC prefixes never cross this boundary.
    fn requests_phy(&self, frame: RxObservedEthernetFrame<'_>) -> bool;

    fn observe(&self, event: ConnectedRxObservation<'_>);
}
