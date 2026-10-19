#![allow(
    clippy::arithmetic_side_effects,
    reason = "test fixtures build units of a few known lengths"
)]

use std::vec::Vec;

use oer_esp32s31_hal::layouts::WifiRxDmaDescriptorControl;
use oer_ieee80211_mac::{
    channel::{Channel, ChannelWidth},
    fuzz::{FULL, QUICK, XorShift, inputs},
};

use super::*;
use crate::{portable::rx_meta, rx::PUBLIC_HEADER_SIZE};

// Old register image used as the independent packing reference.
const CSI_APPEND_ENABLE: u32 = 0x0080_0000;

const DESCRIPTOR_ADDRESS: u32 = 0x2f00_1000;

/// One completed S31 unit: the 0x38-byte prefix (RSSI at 0, rate at 1,
/// HE-SIG-A1 at 4, timestamp at 0x0c, format at 0x25), the dynamic tail
/// (`sig_len` and dump length at 0x38) and an MPDU from 0x40.
fn unit(format: u8, siga1: u32, mpdu_length: usize) -> Vec<u8> {
    let mut bytes = std::vec![0_u8; PUBLIC_HEADER_SIZE + mpdu_length];
    bytes[0] = (-47_i8) as u8;
    bytes[1] = 0x0b;
    bytes[4..8].copy_from_slice(&siga1.to_le_bytes());
    bytes[0x0c..0x10].copy_from_slice(&0x8765_4321_u32.to_le_bytes());
    bytes[0x25] = format << 4;
    let signal_length = (mpdu_length + 4) as u32;
    let tail_word = signal_length | ((signal_length + 4) << 16);
    bytes[0x38..0x3c].copy_from_slice(&tail_word.to_le_bytes());
    for (index, byte) in bytes[PUBLIC_HEADER_SIZE..].iter_mut().enumerate() {
        *byte = index as u8;
    }
    bytes
}

/// The descriptor word of a completed single-descriptor unit of `length`.
/// Lengths beyond the length field are clamped to its largest value.
fn completed_word(length: usize) -> WifiRxDmaDescriptorControl {
    let largest = WifiRxDmaDescriptorControl::from_bits(u32::MAX).length();
    let length = u16::try_from(length).unwrap_or(u16::MAX).min(largest);
    WifiRxDmaDescriptorControl::ZERO
        .with_size(length)
        .and_then(|word0| word0.with_length(length))
        .expect("a clamped length fits")
        .with_rx_done(true)
        .with_owner(true)
}

fn config(flags: u32, csi_config: u32) -> RxIngressConfig {
    RxIngressConfig {
        ring_entry_limit: 1,
        csi_config,
        flags,
    }
}

#[test]
fn a_seed_unit_decodes_at_the_normalized_boundary() {
    let bytes = unit(1, 0, 24);
    let segment = RxSegment {
        descriptor_address: DESCRIPTOR_ADDRESS,
        descriptor_word0: completed_word(bytes.len()),
        buffer: &bytes,
        next_descriptor_address: 0,
    };
    let frame = view_normalized_rx_frame(&segment, config(0, 0)).unwrap();
    assert_eq!(frame.mpdu, &bytes[PUBLIC_HEADER_SIZE..]);
    assert_eq!(frame.mpdu_offset, PUBLIC_HEADER_SIZE);
    assert_eq!(frame.rssi_dbm, -47);
    assert_eq!(frame.stamp, Some(0x8765_4321));
    assert_eq!(frame.logical_length, 24);
}

/// Run every prefix decoder over `buffer` with a descriptor word and an
/// ingress configuration drawn from `rng`, checking what a success claims;
/// whether the normalized view admitted the unit.
fn exercise(buffer: &[u8], rng: &mut XorShift) -> bool {
    let descriptor_word0 = match rng.below(8) {
        0 => WifiRxDmaDescriptorControl::from_bits(rng.next() as u32),
        1 => completed_word(rng.below(buffer.len().saturating_add(64))),
        2 => completed_word(buffer.len()).with_rx_done(false),
        _ => completed_word(buffer.len()),
    };
    let descriptor_address = if rng.below(16) == 0 {
        rng.next() as u32
    } else {
        DESCRIPTOR_ADDRESS
    };
    let csi_config = if rng.below(2) == 0 {
        CSI_APPEND_ENABLE
    } else {
        0
    };
    let config = config(rng.below(5) as u32, csi_config);
    let segment = RxSegment {
        descriptor_address,
        descriptor_word0,
        buffer,
        next_descriptor_address: 0,
    };

    if let Ok(layout) = first_segment_layout(&segment, config) {
        assert!(layout.received_length <= buffer.len());
        assert!(layout.tail_offset >= FIXED_PREFIX_SIZE);
        assert_eq!(
            layout.frame_offset + layout.available_frame_bytes,
            layout.received_length
        );
        assert_eq!(
            layout.expected_frame_length + FCS_SIZE,
            layout.signal_length
        );
    }
    let viewed = view_normalized_rx_frame(&segment, config);
    if let Ok(frame) = viewed {
        let end = frame.mpdu_offset + frame.mpdu.len();
        assert_eq!(&buffer[frame.mpdu_offset..end], frame.mpdu);
        assert!(frame.mpdu.len() <= frame.logical_length);
        assert!(frame.stamp.is_some());
        let _ = rx_meta(
            frame.metadata,
            Channel::ghz2_4(1, ChannelWidth::Mhz20).unwrap(),
        );
    }
    if let Some(phy) = decode_rx_phy_info(buffer) {
        let _ = phy.phy_rate();
        let _ = phy
            .he_su_signal()
            .map(|signal| signal.spatial_stream_count());
        let _ = phy.ht_signal().map(|signal| signal.ht_duplicate_mcs32());
        let _ = (phy.he_mu_signal(), phy.he_trigger_based_signal());
    }
    let _ = decode_normalized_rx_metadata(buffer);
    let _ = decode_rx_local_timestamp(buffer);
    viewed.is_ok()
}

fn run(count: usize) {
    let mut options = unit(5, 0, 32);
    // An HE-SIG-B option of 0x13 bytes and a CSI length the tail follows.
    options[0x2b] = 0x80 | 0x12;
    options[0x2a] = 0xe0;
    options[0x26] = 0x40;
    options[0x27] = 0x04;
    let seeds = [
        unit(0, 0, 24),
        unit(1, 0, 24),
        unit(2, 0x0800_0087, 40),
        unit(4, 0x00e0_591b, 40),
        unit(6, 0x0040_5b4b, 24),
        options,
    ];
    let seeds: Vec<&[u8]> = seeds.iter().map(Vec::as_slice).collect();
    let mut rng = XorShift::new(0x5331_7278 ^ count as u64);
    let mut viewed = 0_usize;
    inputs(&seeds, count, 0x180, |buffer| {
        viewed += usize::from(exercise(buffer, &mut rng));
    });
    // The mutated seeds keep reaching the decode past every refusal.
    assert!(viewed > count / 20, "{viewed} of {count} units viewed");
}

#[test]
fn the_rx_prefix_decode_never_panics() {
    run(QUICK);
}

#[test]
#[ignore = "the full run"]
fn the_rx_prefix_decode_never_panics_full() {
    run(FULL);
}
