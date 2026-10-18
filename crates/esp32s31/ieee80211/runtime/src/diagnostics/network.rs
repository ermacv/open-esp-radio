//! Role-neutral network publication observations.

#[cfg(feature = "diagnostics")]
use oer_esp32s31_ieee80211_mac::rx::PUBLIC_HEADER_SIZE;
#[cfg(feature = "diagnostics")]
use oer_ieee80211_mac::data::EthernetFrameParts;
use oer_ieee80211_mac::sequence::SequenceNumber;
#[cfg(feature = "diagnostics")]
use oer_network_interface::RxEnqueueError;

/// Stable, decoded Ethernet view exposed to diagnostic consumers.
#[cfg(feature = "diagnostics")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxObservedEthernetFrame<'frame> {
    pub destination: [u8; 6],
    pub source: [u8; 6],
    pub ether_type: u16,
    pub payload: &'frame [u8],
}

#[cfg(feature = "diagnostics")]
impl<'frame> From<EthernetFrameParts<'frame>> for RxObservedEthernetFrame<'frame> {
    fn from(frame: EthernetFrameParts<'frame>) -> Self {
        Self {
            destination: frame.destination,
            source: frame.source,
            ether_type: frame.ether_type,
            payload: frame.payload,
        }
    }
}

/// Decoded QoS ordering identity, when the delivered MPDU has one.
#[cfg(feature = "diagnostics")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxQosSequenceObservation {
    pub tid: u8,
    pub sequence: SequenceNumber,
}

/// Diagnostic observation of one exact network admission decision.
#[cfg(feature = "diagnostics")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxNetworkDeliveryEvent<'frame> {
    pub frame: RxObservedEthernetFrame<'frame>,
    pub qos_sequence: Option<RxQosSequenceObservation>,
}

#[cfg(feature = "diagnostics")]
impl<'frame> RxNetworkDeliveryEvent<'frame> {
    pub(crate) fn decoded(frame: EthernetFrameParts<'frame>, raw: Option<&[u8]>) -> Self {
        Self {
            frame: frame.into(),
            qos_sequence: raw.and_then(decode_public_qos_sequence),
        }
    }
}

#[cfg(feature = "diagnostics")]
/// Decode the existing QoS identity without retaining RX storage.
pub fn decode_public_qos_sequence(raw: &[u8]) -> Option<RxQosSequenceObservation> {
    const DATA_TYPE: u16 = 0x0008;
    const DATA_TYPE_MASK: u16 = 0x000c;
    const QOS_SUBTYPE: u16 = 0x0080;
    const TO_FROM_DS: u16 = 0x0300;

    let frame_offset = PUBLIC_HEADER_SIZE;
    let frame_control = u16::from_le_bytes([*raw.get(frame_offset)?, *raw.get(frame_offset + 1)?]);
    if frame_control & (DATA_TYPE_MASK | QOS_SUBTYPE) != DATA_TYPE | QOS_SUBTYPE {
        return None;
    }
    let sequence_control =
        u16::from_le_bytes([*raw.get(frame_offset + 22)?, *raw.get(frame_offset + 23)?]);
    let qos_offset = frame_offset + 24 + usize::from(frame_control & TO_FROM_DS == TO_FROM_DS) * 6;
    Some(RxQosSequenceObservation {
        tid: *raw.get(qos_offset)? & 0x0f,
        sequence: SequenceNumber::from_sequence_control(sequence_control),
    })
}

#[cfg(feature = "diagnostics")]
pub trait RxNetworkDeliveryObserver: Sync {
    fn admitted(&self, event: RxNetworkDeliveryEvent<'_>);

    fn dropped(&self, event: RxNetworkDeliveryEvent<'_>, error: RxEnqueueError);
}
