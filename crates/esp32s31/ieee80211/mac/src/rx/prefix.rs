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

use oer_esp32s31_hal::layouts::{WifiRxControlPrefix, WifiRxControlTail};
use oer_esp32s31_ieee80211_dma::descriptor::descriptor_address_valid;
use oer_ieee80211_softmac::{MacRxEvidence, MacRxMetadata};

use super::{
    CSI_APPEND_ENABLE, DYNAMIC_TAIL_SIZE, FCS_SIZE, FIXED_PREFIX_SIZE, INGRESS_STRICT_DUMP,
    INGRESS_STRICT_RXEND, INGRESS_VALID_FLAGS, NormalizedRxFrame, RxError, RxFirstSegmentLayout,
    RxIngressConfig, RxPhyInfo, RxSegment,
};

/// The shortest MPDU `sig_len` may describe: its two-byte frame control.
const MIN_FRAME_LENGTH: usize = 2;

/// Tail `rx_state` of a frame whose MIC check failed. Observed; no source:
/// neither the public structure nor a recorded stand run names the value.
const RX_STATE_MIC_FAILURE: u8 = 0xf5;
/// Tail `rx_state` of a frame the MAC quarantined. Observed; no source:
/// neither the public structure nor a recorded stand run names the value.
const RX_STATE_QUARANTINED: u8 = 0xc6;

/// The decoder over a buffer's fixed RX-control prefix; `None` when the
/// buffer is shorter than the prefix, which the hardware always writes whole.
fn control_prefix(buffer: &[u8]) -> Option<WifiRxControlPrefix<'_>> {
    buffer
        .first_chunk::<FIXED_PREFIX_SIZE>()
        .map(WifiRxControlPrefix::new)
}

/// The receive timestamp of the S31 RX-control prefix: the Wi-Fi MAC local
/// time (`WIFI_MAC_LOCAL_TIME`) the hardware recorded for this frame, a
/// wrapping 32-bit value in the counter's unit.
///
/// SOURCE\[ESP_WIFI_SYS_S31_RXCTRL]: the packed public `esp_wifi_rxctrl_t`
/// places the 32-bit `timestamp` at bytes 12..16;
/// SOURCE\[BLOB_LIBPP_MAC_LOCAL_TIME]: vendor `libpp[lmac.o]::lmacAdjustTimestamp`
/// reads and rewrites the same word relative to the local-time counter.
pub fn decode_rx_local_timestamp(buffer: &[u8]) -> Option<u32> {
    control_prefix(buffer).map(|prefix| prefix.timestamp().value())
}

/// Decode the finite PHY-rate view without interpreting the format-specific
/// HE-SIG bitfields.
pub fn decode_rx_phy_info(buffer: &[u8]) -> Option<RxPhyInfo> {
    control_prefix(buffer).map(phy_info)
}

fn phy_info(prefix: WifiRxControlPrefix<'_>) -> RxPhyInfo {
    RxPhyInfo {
        rate: prefix.rate().rate(),
        bb_format: prefix.bb_format().bb_format(),
        he_siga1: prefix.he_siga1().value(),
        he_siga2: prefix.he_siga2().value(),
    }
}

/// The frame's RSSI in dBm: the prefix's signed RSSI byte.
fn rssi_dbm(prefix: WifiRxControlPrefix<'_>) -> i8 {
    i8::from_le_bytes([prefix.rssi().value()])
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
    control_prefix(buffer).map(normalized_metadata)
}

fn normalized_metadata(prefix: WifiRxControlPrefix<'_>) -> MacRxMetadata<RxPhyInfo> {
    let rate = phy_info(prefix);
    MacRxMetadata {
        channel: MacRxEvidence::Unavailable,
        rate: MacRxEvidence::HardwareObserved(rate),
        rssi_dbm: MacRxEvidence::HardwareObserved(rssi_dbm(prefix)),
        crypto: MacRxEvidence::Unavailable,
        s_mpdu: MacRxEvidence::HardwareObserved(prefix.single_mpdu().single_mpdu()),
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
    if !segment.descriptor_word0.rx_done() {
        return Err(RxError::Chain);
    }
    let first = first_segment(segment, config)?;
    // The hardware may have consumed part of a verified cipher trailer, so
    // the captured MPDU is the shorter of `sig_len` and the received bytes.
    let mpdu = first
        .frame
        .split_at_checked(first.layout.expected_frame_length)
        .map_or(first.frame, |(mpdu, _)| mpdu);
    let prefix = WifiRxControlPrefix::new(first.prefix);
    let metadata = normalized_metadata(prefix);
    Ok(NormalizedRxFrame {
        mpdu,
        mpdu_offset: first.layout.frame_offset,
        metadata,
        rssi_dbm: rssi_dbm(prefix),
        logical_length: first.layout.expected_frame_length,
        stamp: Some(prefix.timestamp().value()),
    })
}

/// The bytes the hardware wrote into `segment`'s buffer, when its
/// descriptor is a valid RX node whose length fits its capacity and buffer.
fn received_bytes<'frame>(segment: &RxSegment<'frame>) -> Option<&'frame [u8]> {
    let capacity = usize::from(segment.descriptor_word0.size());
    let length = usize::from(segment.descriptor_word0.length());
    if !descriptor_address_valid(segment.descriptor_address)
        || !segment.descriptor_word0.owner()
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
fn dynamic_tail_offset(prefix: WifiRxControlPrefix<'_>, csi_config: u32) -> Option<usize> {
    let mut offset = FIXED_PREFIX_SIZE;
    let sig_b = prefix.he_sig_b();
    if sig_b.present() {
        let option_length =
            usize::from(sig_b.length_bytes()).checked_add(usize::from(sig_b.length_bits() != 0))?;
        offset = offset.checked_add(option_length.checked_add(3)? & !3)?;
    }
    if csi_config & CSI_APPEND_ENABLE != 0 {
        let channel_estimate = prefix.channel_estimate();
        let csi_length = usize::from(channel_estimate.length());
        if channel_estimate.valid() || csi_length != 0 {
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
    let tail_offset = dynamic_tail_offset(WifiRxControlPrefix::new(prefix), config.csi_config)
        .ok_or(RxError::Bounds)?;
    let (_, tail_and_frame) = received
        .split_at_checked(tail_offset)
        .ok_or(RxError::Bounds)?;
    let (tail, frame) = tail_and_frame
        .split_first_chunk::<DYNAMIC_TAIL_SIZE>()
        .ok_or(RxError::Bounds)?;

    let tail = WifiRxControlTail::new(tail);
    let signal_length = usize::from(tail.signal_length().length());
    let dump_length = usize::from(tail.dump_length().length());
    let rxend_state = WifiRxControlPrefix::new(prefix).rxend_state().value();
    let rx_state = tail.rx_state().value();
    match rx_state {
        RX_STATE_MIC_FAILURE => return Err(RxError::MicFailure),
        RX_STATE_QUARANTINED => return Err(RxError::Quarantined),
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
            internal_state: tail.byte_0005_opaque().bits_0_7_opaque(),
            dump_length_matches,
        },
        prefix,
        frame,
    })
}

#[cfg(test)]
mod tests;
