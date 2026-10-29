//! Hardware capacity and public RX BlockAck classification.
//! Software sequence reordering belongs to the upper MAC.

use oer_ieee80211_wire::sequence::SequenceNumber;

use crate::mac::rx::PUBLIC_HEADER_SIZE;

// SOURCE: complete `libnet80211.a[ieee80211_ht.o]::
// ampdu_rx_start.constprop.0`. The vendor agreement owner selects the smaller
// of the peer request and `g_wifi_menuconfig+0x30`, whose normal configured
// value is 64. Its later ordinary activation path passes the constant 64 to
// `ic_add_rx_ba`; complete `libpp.a[hal_ampdu.o]::
// hal_agreement_add_rx_ba` publishes that value in the seven-bit hardware
// window. Keep the protocol/reorder window independent of the number of RX DMA
// descriptors: descriptors are recycled while this sequence window remains
// active.
pub const RX_BLOCK_ACK_MAX_WINDOW: u16 = 64;
use oer_ieee80211_wire::{
    frame::{FrameControlFields, FrameType, QosControl},
    sequence::SequenceControl,
    trigger::BarControlFields,
};

/// Public ordering identity of one protected QoS data MPDU.
///
/// These fields precede the encrypted body. Both station and access-point
/// receive paths use this exact classifier before deciding whether an active
/// agreement owns the frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxBlockAckMpduKey {
    pub peer: [u8; 6],
    pub tid: u8,
    pub sequence: SequenceNumber,
    pub retry: bool,
}

/// Extract the role-neutral infrastructure RX BlockAck key.
///
/// `local_address` is the receiver address (Address 1). A connected STA also
/// supplies `expected_peer`; an AP leaves it absent and validates admission in
/// its peer table after extraction. Group, foreign, unprotected, non-QoS and
/// fragmented frames do not enter a reorder sequence space.
#[inline(always)]
pub fn rx_block_ack_mpdu_key(
    raw: &[u8],
    local_address: [u8; 6],
    expected_peer: Option<[u8; 6]>,
) -> Option<RxBlockAckMpduKey> {
    let frame_offset = PUBLIC_HEADER_SIZE;
    let frame_control =
        FrameControlFields::from_bytes([*raw.get(frame_offset)?, *raw.get(frame_offset + 1)?]);
    if frame_control.kind() != FrameType::Data
        || !frame_control.protected()
        || !frame_control.qos()
        || frame_control.more_fragments()
        || raw.get(frame_offset + 4..frame_offset + 10)? != local_address
    {
        return None;
    }
    let peer: [u8; 6] = raw
        .get(frame_offset + 10..frame_offset + 16)?
        .try_into()
        .ok()?;
    if expected_peer.is_some_and(|expected| expected != peer) {
        return None;
    }
    let sequence_control =
        u16::from_le_bytes([*raw.get(frame_offset + 22)?, *raw.get(frame_offset + 23)?]);
    if SequenceControl::from_bytes(sequence_control.to_le_bytes()).fragment() != 0 {
        return None;
    }
    let qos_offset = frame_offset + 24 + usize::from(frame_control.ds() == 3) * 6;
    Some(RxBlockAckMpduKey {
        peer,
        tid: QosControl::from_bytes([*raw.get(qos_offset)?, 0])
            .tid()
            .get(),
        sequence: SequenceNumber::from_sequence_control(sequence_control),
        retry: frame_control.retry(),
    })
}

/// Receive BlockAck session and window start named by one BlockAckReq.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxBlockAckRequestKey {
    pub peer: [u8; 6],
    pub tid: u8,
    pub starting_sequence: SequenceNumber,
}

const BLOCK_ACK_REQUEST_LENGTH: usize = 20;

/// Extract the receive session a BlockAckReq addresses.
///
/// Only the TID and the Starting Sequence Control are read. The BAR Control
/// policy bits (BAR Ack Policy, Compressed, Multi-TID) are not validated,
/// and a TID above 7 is ignored.
///
/// SOURCE: `libnet80211.a::sta_recv_ctl`/`hostap_recv_ctl` dispatch control
/// subtype 8 to `ieee80211_recv_bar`, and `ieee80211_process_bar_info` reads
/// only `bar_control >> 12` and `ssc >> 4` (vendor-library RX BlockAckReq
/// answer).
pub fn rx_block_ack_request_key(
    raw: &[u8],
    local_address: [u8; 6],
    expected_peer: Option<[u8; 6]>,
) -> Option<RxBlockAckRequestKey> {
    let frame = raw.get(PUBLIC_HEADER_SIZE..PUBLIC_HEADER_SIZE + BLOCK_ACK_REQUEST_LENGTH)?;
    let frame_control = u16::from_le_bytes([frame[0], frame[1]]);
    if FrameControlFields::from_raw(frame_control).kind() != FrameType::Control
        || FrameControlFields::from_raw(frame_control).subtype() != 8
        || frame[4..10] != local_address
    {
        return None;
    }
    let peer: [u8; 6] = frame[10..16].try_into().ok()?;
    if expected_peer.is_some_and(|expected| expected != peer) {
        return None;
    }
    let control = u16::from_le_bytes([frame[16], frame[17]]);
    let tid = BarControlFields::from_bytes(control.to_le_bytes())
        .tid()
        .get();
    if tid > 7 {
        return None;
    }
    Some(RxBlockAckRequestKey {
        peer,
        tid,
        starting_sequence: SequenceNumber::from_sequence_control(u16::from_le_bytes([
            frame[18], frame[19],
        ])),
    })
}

pub const RX_BLOCK_ACK_BANK_COUNT: usize = 8;

#[cfg(test)]
mod tests;
