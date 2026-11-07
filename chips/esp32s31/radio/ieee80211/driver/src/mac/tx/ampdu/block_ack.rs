//! ESP32-S31 BlockAck completion observations.
//!
//! Frame parsing and one generic agreement state machine live in
//! [`oer_protocols_ieee80211_wire::block_ack`], the station's originator in
//! `oer_protocols_ieee80211_core_upper_mac::block_ack::originator` and the vendor TIDs and Dialog Tokens in
//! `oer_chips_family_ieee80211_policy::block_ack`. This module retains only
//! the S31 window bound and the three-register completion snapshot. It still owns no DMA address or register access.

use oer_protocols_ieee80211_contracts_port::BlockAckReport;
use oer_protocols_ieee80211_wire::sequence::SequenceNumber;
/// The widest TX BlockAck window the S31 serves: the positions one queue's
/// result bank captures
/// ([`TX_BLOCK_ACK_BITMAP_BITS`](oer_chips_esp32s31_hal::radio::ieee80211::registers::block_ack::TX_BLOCK_ACK_BITMAP_BITS)),
/// within those of the compressed BlockAck the port reports
/// ([`BlockAckReport::BITMAP_BITS`]). A window is the agreement's: the
/// MPDUs of every aggregate out under it lie inside it.
pub const TX_BLOCK_ACK_MAX_WINDOW: u16 = {
    let hardware =
        oer_chips_esp32s31_hal::radio::ieee80211::registers::block_ack::TX_BLOCK_ACK_BITMAP_BITS;
    let report = BlockAckReport::BITMAP_BITS as u32;
    let window = if hardware < report { hardware } else { report };
    if window > u16::MAX as u32 {
        panic!("a BlockAck window fits a u16");
    }
    window as u16
};

/// The subframes one aggregate's DMA chain holds: one per bit of the port's
/// subframe masks. An aggregate is no wider than the BlockAck window, which
/// may hold more than one aggregate.
pub const TX_AMPDU_SLOT_CAPACITY: usize =
    oer_protocols_ieee80211_contracts_port::arena::AMPDU_SUBFRAME_MASK_BITS;

const _: () = {
    if TX_AMPDU_SLOT_CAPACITY > TX_BLOCK_ACK_MAX_WINDOW as usize {
        panic!("an aggregate fits the BlockAck window");
    }
};

/// Semantic BlockAck information decoded by the PAC completion owner.
///
/// Bit zero acknowledges `starting_sequence`, bit one the following sequence,
/// and so on, across the result bank's [`TX_BLOCK_ACK_MAX_WINDOW`] positions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxBlockAckBitmap {
    pub starting_sequence: SequenceNumber,
    pub bitmap: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtBlockAckObservation {
    pub control: u8,
    pub block_ack: TxBlockAckBitmap,
}

impl HtBlockAckObservation {
    pub const fn new(control: u8, starting_sequence: SequenceNumber, bitmap: u64) -> Self {
        Self {
            control,
            block_ack: TxBlockAckBitmap::new(starting_sequence, bitmap),
        }
    }
}

impl TxBlockAckBitmap {
    #[inline(always)]
    pub const fn new(starting_sequence: SequenceNumber, bitmap: u64) -> Self {
        Self {
            starting_sequence,
            bitmap,
        }
    }

    /// The portable BlockAck of the lower-MAC port.
    pub const fn report(self) -> BlockAckReport {
        BlockAckReport {
            start_sequence: self.starting_sequence,
            bitmap: self.bitmap,
        }
    }
}
