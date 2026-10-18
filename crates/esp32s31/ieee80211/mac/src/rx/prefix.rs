//! The S31 RX-control prefix and the first descriptor's geometry: the
//! boundary every completed receive unit crosses first.
//!
//! Received bytes and descriptor words reach this module; nothing here
//! panics on them. Field boundaries come from `first_chunk`,
//! `split_first_chunk` and `split_at_checked` and stay fixed-size arrays.
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]

use oer_esp32s31_ieee80211_dma::descriptor::{
    OWNER, descriptor_address_valid, length as descriptor_length, rx_done, size as descriptor_size,
};
use oer_ieee80211_softmac::{MacRxEvidence, MacRxMetadata};

use super::{
    CSI_APPEND_ENABLE, CSI_LENGTH_FLAGS_OFFSET, CSI_LENGTH_LOW_OFFSET, DYNAMIC_TAIL_SIZE, FCS_SIZE,
    FIXED_PREFIX_SIZE, INGRESS_STRICT_DUMP, INGRESS_STRICT_RXEND, INGRESS_VALID_FLAGS,
    NormalizedRxFrame, OPTION_LENGTH_FLAGS_OFFSET, OPTION_LENGTH_HINT_OFFSET,
    PREFIX_RXEND_STATE_OFFSET, RX_LOCAL_TIMESTAMP_OFFSET, RX_PHY_BB_FORMAT_OFFSET,
    RX_PHY_HE_SIGA1_OFFSET, RX_PHY_HE_SIGA2_OFFSET, RX_PHY_RATE_OFFSET, RX_PHY_RSSI_OFFSET,
    RX_PHY_SINGLE_MPDU_OFFSET, RxError, RxFirstSegmentLayout, RxIngressConfig, RxPhyInfo,
    RxSegment, TAIL_INTERNAL_OFFSET, TAIL_STATE_OFFSET,
};

/// The prefix bytes the PHY view reads: through `cur_bb_format` at `0x25`.
const RX_PHY_INFO_SIZE: usize = RX_PHY_BB_FORMAT_OFFSET + 1;
/// The prefix bytes through the four-byte receive timestamp.
const RX_LOCAL_TIMESTAMP_END: usize = RX_LOCAL_TIMESTAMP_OFFSET + 4;
/// The shortest MPDU `sig_len` may describe: its two-byte frame control.
const MIN_FRAME_LENGTH: usize = 2;

/// The receive timestamp of the S31 RX-control prefix: the Wi-Fi MAC local
/// time (`WIFI_MAC_LOCAL_TIME`) the hardware recorded for this frame, a
/// wrapping 32-bit value in the counter's unit.
///
/// SOURCE: public ESP32-S31 `esp_wifi_rxctrl_t` definition. The packed ABI
/// places the 32-bit `timestamp` at bytes 12..16, after `is_group` in byte
/// 11; vendor `libpp[lmac.o]::lmacAdjustTimestamp` reads and rewrites the same word
/// relative to the local-time counter.
pub fn decode_rx_local_timestamp(buffer: &[u8]) -> Option<u32> {
    buffer
        .first_chunk::<RX_LOCAL_TIMESTAMP_END>()
        .map(local_timestamp)
}

fn local_timestamp(prefix: &[u8; RX_LOCAL_TIMESTAMP_END]) -> u32 {
    let [.., b0, b1, b2, b3] = *prefix;
    u32::from_le_bytes([b0, b1, b2, b3])
}

/// Decode the finite PHY-rate view without interpreting the format-specific
/// HE-SIG bitfields.
pub fn decode_rx_phy_info(buffer: &[u8]) -> Option<RxPhyInfo> {
    buffer.first_chunk::<RX_PHY_INFO_SIZE>().map(phy_info)
}

fn phy_info(prefix: &[u8; RX_PHY_INFO_SIZE]) -> RxPhyInfo {
    RxPhyInfo {
        rate: prefix[RX_PHY_RATE_OFFSET] & 0x1f,
        bb_format: prefix[RX_PHY_BB_FORMAT_OFFSET] >> 4,
        he_siga1: u32::from_le_bytes([
            prefix[RX_PHY_HE_SIGA1_OFFSET],
            prefix[RX_PHY_HE_SIGA1_OFFSET + 1],
            prefix[RX_PHY_HE_SIGA1_OFFSET + 2],
            prefix[RX_PHY_HE_SIGA1_OFFSET + 3],
        ]),
        he_siga2: u16::from_le_bytes([
            prefix[RX_PHY_HE_SIGA2_OFFSET],
            prefix[RX_PHY_HE_SIGA2_OFFSET + 1],
        ]),
    }
}

/// Decode the portable metadata available at the S31 staged-frame boundary.
///
/// RSSI and rate come from instruction-proved raw-DMA fields. Channel does
/// not: S31 hardware showed raw byte `0x1c` reporting 12 for a Probe Request
/// whose DS Parameter Set was 1 while the radio was tuned to 1. The similarly
/// laid out public `esp_wifi_rxctrl_t.channel` is therefore a later vendor callback
/// value, not evidence available at this raw boundary. The adjacent public
/// `cur_single_mpdu` bit is preserved as S-MPDU evidence: the Espressif header
/// defines it as the IEEE
/// VHT/HE single-MPDU A-MPDU form, not as the inverse of arbitrary physical
/// aggregation. Complete vendor `dbg_dump_rx_ppdu` independently proves byte
/// `0x1f`, bit zero. HT obtains A-MPDU containment from the independently
/// proved HT-SIG Aggregation bit. VHT/HE format validation establishes that
/// their MPDUs use an A-MPDU container, while legacy formats establish the
/// opposite; neither path infers a member count. Crypto and A-MSDU remain
/// unavailable until a later owner has direct hardware or protocol evidence;
/// an active BA agreement and the 802.11 Protected bit are not substitutes.
pub fn decode_normalized_rx_metadata(buffer: &[u8]) -> Option<MacRxMetadata<RxPhyInfo>> {
    buffer
        .first_chunk::<RX_PHY_INFO_SIZE>()
        .map(normalized_metadata)
}

fn normalized_metadata(prefix: &[u8; RX_PHY_INFO_SIZE]) -> MacRxMetadata<RxPhyInfo> {
    let rate = phy_info(prefix);
    MacRxMetadata {
        channel: MacRxEvidence::Unavailable,
        rate: MacRxEvidence::HardwareObserved(rate),
        rssi_dbm: MacRxEvidence::HardwareObserved(prefix[RX_PHY_RSSI_OFFSET] as i8),
        crypto: MacRxEvidence::Unavailable,
        s_mpdu: MacRxEvidence::HardwareObserved(prefix[RX_PHY_SINGLE_MPDU_OFFSET] & 1 != 0),
        ampdu: match rate.ht_aggregation() {
            Some(ampdu) => MacRxEvidence::HardwareObserved(ampdu),
            None => match rate.protocol_ampdu_containment() {
                Some(ampdu) => MacRxEvidence::ProtocolValidated(ampdu),
                None => MacRxEvidence::Unavailable,
            },
        },
        amsdu: MacRxEvidence::Unavailable,
    }
}

/// Validate and borrow one contiguous completed unit for passive capture.
///
/// This entry performs no protocol/VIF filtering and mutates no receive state.
/// It is therefore suitable for the promiscuous queue's synchronous monitor
/// sink before the descriptor is recycled. Split DMA chains must first be
/// copied into the existing contiguous staging owner.
#[inline(always)]
pub fn view_normalized_rx_frame<'frame>(
    segment: &RxSegment<'frame>,
    config: RxIngressConfig,
) -> Result<NormalizedRxFrame<'frame>, RxError> {
    if !rx_done(segment.descriptor_word0) {
        return Err(RxError::Chain);
    }
    let first = first_segment(segment, config)?;
    // The hardware may have consumed part of a verified cipher trailer, so
    // the captured MPDU is the shorter of `sig_len` and the received bytes.
    let mpdu = first
        .frame
        .split_at_checked(first.layout.expected_frame_length)
        .map_or(first.frame, |(mpdu, _)| mpdu);
    let metadata = decode_normalized_rx_metadata(first.prefix).ok_or(RxError::Metadata)?;
    Ok(NormalizedRxFrame {
        mpdu,
        mpdu_offset: first.layout.frame_offset,
        metadata,
        rssi_dbm: first.prefix[RX_PHY_RSSI_OFFSET] as i8,
        logical_length: first.layout.expected_frame_length,
        stamp: decode_rx_local_timestamp(first.prefix),
    })
}

/// The bytes the hardware wrote into `segment`'s buffer, when its
/// descriptor is a valid RX node whose length fits its capacity and buffer.
fn received_bytes<'frame>(segment: &RxSegment<'frame>) -> Option<&'frame [u8]> {
    let capacity = descriptor_size(segment.descriptor_word0) as usize;
    let length = descriptor_length(segment.descriptor_word0) as usize;
    if !descriptor_address_valid(segment.descriptor_address)
        || segment.descriptor_word0 & OWNER == 0
        || capacity == 0
        || length == 0
        || length > capacity
    {
        return None;
    }
    segment
        .buffer
        .split_at_checked(length)
        .map(|(received, _)| received)
}

pub(super) fn segment_valid(segment: &RxSegment<'_>) -> bool {
    received_bytes(segment).is_some()
}

/// Where the dynamic tail starts: after the fixed prefix, the optional
/// HE-SIG-B bytes and the optional CSI bytes, each rounded to a word.
fn dynamic_tail_offset(prefix: &[u8; FIXED_PREFIX_SIZE], csi_config: u32) -> Option<usize> {
    let mut offset = FIXED_PREFIX_SIZE;
    let option_flags = prefix[OPTION_LENGTH_FLAGS_OFFSET];
    if option_flags & 0x80 != 0 {
        let hint = prefix[OPTION_LENGTH_HINT_OFFSET];
        let option_length =
            usize::from(option_flags & 0x7f).checked_add(usize::from(hint >> 5 != 0))?;
        offset = offset.checked_add(option_length.checked_add(3)? & !3)?;
    }
    if csi_config & CSI_APPEND_ENABLE != 0 {
        let low = prefix[CSI_LENGTH_LOW_OFFSET];
        let flags = prefix[CSI_LENGTH_FLAGS_OFFSET];
        let csi_length = usize::from(u16::from_le_bytes([low, flags & 0x03]));
        if flags & 0x04 != 0 || csi_length != 0 {
            offset = offset.checked_add(csi_length.checked_add(3)? & 0x7fc)?;
        }
    }
    Some(offset)
}

/// The first descriptor's layout with the byte ranges it was decoded from.
struct FirstSegment<'frame> {
    layout: RxFirstSegmentLayout,
    prefix: &'frame [u8; FIXED_PREFIX_SIZE],
    /// The received bytes after the dynamic tail.
    frame: &'frame [u8],
}

/// Decode the first-descriptor RX layout without requiring the complete MPDU
/// to fit in that descriptor.
///
/// This is also the diagnostic boundary for a split hardware unit: callers
/// can distinguish malformed metadata from a valid first segment whose
/// remaining bytes are carried by following descriptors.
#[inline(always)]
pub fn first_segment_layout(
    segment: &RxSegment<'_>,
    config: RxIngressConfig,
) -> Result<RxFirstSegmentLayout, RxError> {
    first_segment(segment, config).map(|first| first.layout)
}

#[inline(always)]
fn first_segment<'frame>(
    segment: &RxSegment<'frame>,
    config: RxIngressConfig,
) -> Result<FirstSegment<'frame>, RxError> {
    if config.flags & !INGRESS_VALID_FLAGS != 0 {
        return Err(RxError::Invalid);
    }
    let received = received_bytes(segment).ok_or(RxError::Invalid)?;
    let (prefix, _) = received
        .split_first_chunk::<FIXED_PREFIX_SIZE>()
        .ok_or(RxError::Bounds)?;
    let tail_offset = dynamic_tail_offset(prefix, config.csi_config).ok_or(RxError::Bounds)?;
    let (_, tail_and_frame) = received
        .split_at_checked(tail_offset)
        .ok_or(RxError::Bounds)?;
    let (tail, frame) = tail_and_frame
        .split_first_chunk::<DYNAMIC_TAIL_SIZE>()
        .ok_or(RxError::Bounds)?;

    // The tail word: `sig_len` in bits 13:0, the dump length in 29:16.
    let [signal_low, signal_high, dump_low, dump_high, ..] = *tail;
    let signal_length = usize::from(u16::from_le_bytes([signal_low, signal_high]) & 0x3fff);
    let dump_length = usize::from(u16::from_le_bytes([dump_low, dump_high]) & 0x3fff);
    let rxend_state = prefix[PREFIX_RXEND_STATE_OFFSET];
    let rx_state = tail[TAIL_STATE_OFFSET];
    match rx_state {
        0xf5 => return Err(RxError::MicFailure),
        0xc6 => return Err(RxError::Quarantined),
        0 => {}
        _ => return Err(RxError::RxFailure),
    }
    if config.flags & INGRESS_STRICT_RXEND != 0 && rxend_state != 0 {
        return Err(RxError::RxFailure);
    }

    let dump_length_matches = signal_length
        .checked_add(FCS_SIZE)
        .is_some_and(|expected| dump_length == expected);
    if config.flags & INGRESS_STRICT_DUMP != 0 && !dump_length_matches {
        return Err(RxError::Metadata);
    }
    let expected_frame_length = signal_length
        .checked_sub(FCS_SIZE)
        .filter(|length| *length >= MIN_FRAME_LENGTH)
        .ok_or(RxError::Bounds)?;

    let frame_offset = tail_offset
        .checked_add(DYNAMIC_TAIL_SIZE)
        .ok_or(RxError::Bounds)?;
    let available_frame_bytes = frame.len();
    Ok(FirstSegment {
        layout: RxFirstSegmentLayout {
            received_length: received.len(),
            tail_offset,
            frame_offset,
            signal_length,
            dump_length,
            expected_frame_length,
            available_frame_bytes,
            frame_shortfall: expected_frame_length.saturating_sub(available_frame_bytes),
            rxend_state,
            rx_state,
            internal_state: tail[TAIL_INTERNAL_OFFSET],
            dump_length_matches,
        },
        prefix,
        frame,
    })
}

#[cfg(test)]
mod tests;
