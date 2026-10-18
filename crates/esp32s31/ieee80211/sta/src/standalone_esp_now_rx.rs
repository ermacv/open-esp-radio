//! Standalone ESP-NOW receive dispatch on the normal station RX path.
//!
//! The dispatcher accepts only vendor Action frames identified by the
//! ESP-NOW category/OUI prefix, then delegates all address, peer, version and
//! duplicate checks to the portable receive epoch. It owns no monitor tap,
//! association state, WPA2 keys or network-stack publication.

use oer_esp32s31_ieee80211_dma::rx_ring::RxSegment;
use oer_esp32s31_ieee80211_mac::rx::{
    PUBLIC_HEADER_SIZE, RxError, RxIngressConfig, RxPhyInfo, decode_normalized_rx_metadata,
    extract_management,
};
use oer_ieee80211_mac::extensions::espressif::esp_now::{
    ESP_NOW_ACTION_CATEGORY, ESP_NOW_ORGANIZATION_IDENTIFIER, EspNowVersionError,
    EspNowWireVersion, esp_now_wire_version,
};
use oer_ieee80211_softmac::{
    EspNowPeerId, EspNowReceiveError, EspNowReceivedV1, EspNowReceivedV2, EspNowRxEpoch,
    EspNowRxOutcome, EspNowV2ReceiveError, EspNowV2RxOutcome, MacRxMetadata,
};

use oer_esp32s31_ieee80211::esp_now::normalize_esp_now_rx_metadata;

const ACTION_FRAME_CONTROL: u16 = 0x00d0;

/// Borrowed application event emitted after strict plaintext admission.
#[derive(Clone, Copy, Debug)]
pub struct StandaloneEspNowRxEvent<'frame> {
    pub received: EspNowReceivedV1<'frame>,
    pub metadata: MacRxMetadata<RxPhyInfo>,
}

#[derive(Clone, Copy, Debug)]
pub struct StandaloneEspNowV2RxEvent<'frame> {
    pub received: EspNowReceivedV2<'frame>,
    pub metadata: MacRxMetadata<RxPhyInfo>,
}

/// Finite publication boundary for a standalone application service.
///
/// A sink which retains a datagram must copy it before returning. The shared
/// Embassy ESP-NOW RX mailbox already provides that owned bounded handoff.
pub trait StandaloneEspNowRxSink {
    fn publish(&mut self, event: StandaloneEspNowRxEvent<'_>);

    fn supports_v2(&self) -> bool {
        false
    }

    fn publish_v2(&mut self, _event: StandaloneEspNowV2RxEvent<'_>) {}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StandaloneEspNowRxError {
    PublicHeader,
    Rx(RxError),
    Protocol(EspNowReceiveError),
    V2Protocol(EspNowV2ReceiveError),
    Version(EspNowVersionError),
    V2SinkUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StandaloneEspNowRxDispatch {
    Received { peer: EspNowPeerId },
    Duplicate { peer: EspNowPeerId },
    V2Received { peer: EspNowPeerId },
    V2Duplicate { peer: EspNowPeerId },
    Ignored,
    Rejected(StandaloneEspNowRxError),
}

/// Unique peer snapshot and duplicate history for one standalone RX epoch.
pub struct StandaloneEspNowRxDispatcher<const PEERS: usize> {
    epoch: EspNowRxEpoch<PEERS>,
    ingress: RxIngressConfig,
}

impl<const PEERS: usize> StandaloneEspNowRxDispatcher<PEERS> {
    pub const fn new(epoch: EspNowRxEpoch<PEERS>, ingress: RxIngressConfig) -> Self {
        Self { epoch, ingress }
    }

    pub const fn epoch(&self) -> &EspNowRxEpoch<PEERS> {
        &self.epoch
    }

    /// Clear all replay/duplicate fingerprints before this normal-RX owner is
    /// returned across a stop/restart edge.
    pub fn reset_duplicate_history(&mut self) -> usize {
        self.epoch.reset_duplicate_history()
    }

    pub fn into_epoch(self) -> EspNowRxEpoch<PEERS> {
        self.epoch
    }

    /// Consume one independently owned staged S31 receive segment.
    ///
    /// Non-Action and foreign vendor Action frames are ignored. Once the
    /// category/OUI prefix identifies ESP-NOW, malformed or unauthorized
    /// input is rejected explicitly rather than falling through to another
    /// protocol.
    pub fn dispatch<S: StandaloneEspNowRxSink>(
        &mut self,
        segment: RxSegment<'_>,
        mpdu: &mut [u8],
        sink: &mut S,
    ) -> StandaloneEspNowRxDispatch {
        let raw = segment.buffer;
        let Some(frame_control) = public_frame_control(raw) else {
            return StandaloneEspNowRxDispatch::Rejected(StandaloneEspNowRxError::PublicHeader);
        };
        if frame_control & 0x00fc != ACTION_FRAME_CONTROL {
            return StandaloneEspNowRxDispatch::Ignored;
        }
        let management =
            match extract_management(core::slice::from_ref(&segment), self.ingress, mpdu) {
                Ok(management) => management,
                Err(error) => {
                    return StandaloneEspNowRxDispatch::Rejected(StandaloneEspNowRxError::Rx(
                        error,
                    ));
                }
            };
        if management.length < 24 || !is_esp_now_action_candidate(&mpdu[24..management.length]) {
            return StandaloneEspNowRxDispatch::Ignored;
        }
        let body = &mpdu[24..management.length];
        let version = match esp_now_wire_version(body) {
            Ok(version) => version,
            Err(error) => {
                return StandaloneEspNowRxDispatch::Rejected(StandaloneEspNowRxError::Version(
                    error,
                ));
            }
        };
        match version {
            EspNowWireVersion::V1 => match self.epoch.receive_v1(&mpdu[..management.length]) {
                Ok(EspNowRxOutcome::Received(received)) => {
                    let peer = received.peer();
                    let metadata = decode_normalized_rx_metadata(raw)
                        .unwrap_or_else(MacRxMetadata::unavailable);
                    let phy_mode = self
                        .epoch
                        .peer_phy_mode(peer)
                        .expect("an admitted ESP-NOW peer retains its configured PHY context");
                    let metadata = normalize_esp_now_rx_metadata(phy_mode, metadata).normalized;
                    sink.publish(StandaloneEspNowRxEvent { received, metadata });
                    StandaloneEspNowRxDispatch::Received { peer }
                }
                Ok(EspNowRxOutcome::Duplicate { peer }) => {
                    StandaloneEspNowRxDispatch::Duplicate { peer }
                }
                Err(error) => {
                    StandaloneEspNowRxDispatch::Rejected(StandaloneEspNowRxError::Protocol(error))
                }
            },
            EspNowWireVersion::V2 => {
                if !sink.supports_v2() {
                    return StandaloneEspNowRxDispatch::Rejected(
                        StandaloneEspNowRxError::V2SinkUnavailable,
                    );
                }
                match self.epoch.receive_v2(&mpdu[..management.length]) {
                    Ok(EspNowV2RxOutcome::Received(received)) => {
                        let peer = received.peer();
                        let metadata = decode_normalized_rx_metadata(raw)
                            .unwrap_or_else(MacRxMetadata::unavailable);
                        let phy_mode = self
                            .epoch
                            .peer_phy_mode(peer)
                            .expect("an admitted ESP-NOW peer retains its configured PHY context");
                        let metadata = normalize_esp_now_rx_metadata(phy_mode, metadata).normalized;
                        sink.publish_v2(StandaloneEspNowV2RxEvent { received, metadata });
                        StandaloneEspNowRxDispatch::V2Received { peer }
                    }
                    Ok(EspNowV2RxOutcome::Duplicate { peer }) => {
                        StandaloneEspNowRxDispatch::V2Duplicate { peer }
                    }
                    Err(error) => StandaloneEspNowRxDispatch::Rejected(
                        StandaloneEspNowRxError::V2Protocol(error),
                    ),
                }
            }
        }
    }
}

fn is_esp_now_action_candidate(body: &[u8]) -> bool {
    if body.first() != Some(&ESP_NOW_ACTION_CATEGORY) {
        return false;
    }
    body.len() < 4 || body[1..4] == ESP_NOW_ORGANIZATION_IDENTIFIER
}

fn public_frame_control(raw: &[u8]) -> Option<u16> {
    Some(u16::from_le_bytes([
        *raw.get(PUBLIC_HEADER_SIZE)?,
        *raw.get(PUBLIC_HEADER_SIZE + 1)?,
    ]))
}
