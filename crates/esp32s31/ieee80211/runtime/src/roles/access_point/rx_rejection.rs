//! First-failure evidence for an AP ownership epoch. No logging or allocation.

#[cfg(any(feature = "diagnostics", test))]
use super::{AccessPointControlObservation, ApRxDispatch, ApRxError};
#[cfg(any(feature = "diagnostics", test))]
use oer_esp32s31_ieee80211_mac::rx::{RxIngressConfig, RxSegment, view_normalized_rx_frame};
#[cfg(any(feature = "diagnostics", test))]
use oer_ieee80211_mac::ccmp::CcmpHeader;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointRxRejectionReason {
    Data(oer_ieee80211_mac::data::DataDecapError),
    PeerQosMismatch,
    PairwiseKeyId(u8),
    Replay(oer_ieee80211_mac::ccmp::CcmpReplayError),
    KeyGenerationMismatch,
    Fragment(oer_ieee80211_mac::fragmentation::OpenDataFragmentError),
    ReorderStorageExhausted,
    ReorderFrameTooLong,
    DeferredOutputCapacity,
    InPlaceOutputUnsupported,
}

/// Timestamp is radio monotonic time at rejection, including during reorder
/// release. Metadata belongs to the rejected MPDU, not the triggering frame.
/// Absent/truncated header fields remain unknown. No key bytes or payload are
/// retained. The first record survives until the AP epoch is destroyed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccessPointRxRejection {
    pub reason: AccessPointRxRejectionReason,
    pub at_micros: u64,
    pub transmitter: Option<[u8; 6]>,
    pub frame_control: Option<u16>,
    pub sequence_control: Option<u16>,
    pub tid: Option<u8>,
    pub key_id: Option<u8>,
    pub packet_number: Option<u64>,
    pub mpdu_length: usize,
}

#[cfg(any(feature = "diagnostics", test))]
impl AccessPointControlObservation {
    pub(super) fn record_dispatch_rejection(
        &mut self,
        dispatch: ApRxDispatch,
        segment: RxSegment<'_>,
        at_micros: u64,
    ) {
        let ApRxDispatch::Rejected(error) = dispatch else {
            return;
        };
        let reason = match error {
            ApRxError::Data(error) => AccessPointRxRejectionReason::Data(error),
            ApRxError::PeerQosMismatch => AccessPointRxRejectionReason::PeerQosMismatch,
            ApRxError::PairwiseKeyId(id) => AccessPointRxRejectionReason::PairwiseKeyId(id),
            ApRxError::Replay(error) => AccessPointRxRejectionReason::Replay(error),
            ApRxError::KeyGenerationMismatch => AccessPointRxRejectionReason::KeyGenerationMismatch,
            ApRxError::Fragment(error) => AccessPointRxRejectionReason::Fragment(error),
            ApRxError::Radio(_) | ApRxError::SecurityModeMismatch => return,
        };
        self.record_rx_rejection(reason, segment, at_micros);
    }

    pub(super) fn record_rx_rejection(
        &mut self,
        reason: AccessPointRxRejectionReason,
        segment: RxSegment<'_>,
        at_micros: u64,
    ) {
        if self.first_rx_protocol_rejection.is_some() {
            return;
        }
        let frame = view_normalized_rx_frame(
            &segment,
            RxIngressConfig {
                ring_entry_limit: 1,
                csi_config: 0,
                flags: 0,
            },
        )
        .ok();
        let mpdu = frame.as_ref().map_or(&[][..], |frame| frame.mpdu);
        self.first_rx_protocol_rejection =
            Some(AccessPointRxRejection::from_mpdu(reason, mpdu, at_micros));
    }
}

#[cfg(any(feature = "diagnostics", test))]
impl AccessPointRxRejection {
    fn from_mpdu(reason: AccessPointRxRejectionReason, mpdu: &[u8], at_micros: u64) -> Self {
        let word = |offset| {
            mpdu.get(offset..offset + 2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
        };
        let frame_control = word(0);
        let mut record = Self {
            reason,
            at_micros,
            transmitter: mpdu.get(10..16).and_then(|b| b.try_into().ok()),
            frame_control,
            sequence_control: word(22),
            tid: None,
            key_id: None,
            packet_number: None,
            mpdu_length: mpdu.len(),
        };
        if let Some(fc) = frame_control
            && fc & 0x000c == 0x0008
        {
            let mut header = if fc & 0x0300 == 0x0300 { 30 } else { 24 };
            if fc & 0x0080 != 0 {
                record.tid = mpdu.get(header).map(|qos| qos & 0x0f);
                header += 2;
                if fc & 0x8000 != 0 {
                    header += 4;
                }
            }
            if fc & 0x4000 != 0
                && let Some(bytes) = mpdu.get(header..header + 8)
                && let Ok(ccmp) = CcmpHeader::parse(bytes.try_into().expect("eight header bytes"))
            {
                record.key_id = Some(ccmp.key_id().value());
                record.packet_number = Some(ccmp.packet_number().value());
            }
        }
        record
    }
}

#[cfg(test)]
mod tests;
