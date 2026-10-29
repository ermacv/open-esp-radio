//! ESP32-S31 BlockAck completion observations.
//!
//! Frame parsing and one generic agreement state machine live in
//! [`oer_ieee80211_wire::block_ack`], the station's originator in
//! `oer_ieee80211_upper_mac::block_ack::originator` and the vendor TIDs and Dialog Tokens in
//! `oer_espressif_ieee80211_policy::block_ack`. This module retains only
//! the S31 window bound and the three-register completion snapshot. It still owns no DMA address or register access.

use oer_ieee80211_port::BlockAckReport;
use oer_ieee80211_wire::sequence::SequenceNumber;
/// Strict S31 TX window recovered from the fixed vendor queue geometry.
pub const TX_BLOCK_ACK_MAX_WINDOW: u16 = 32;
pub const TX_AMPDU_SLOT_CAPACITY: usize = TX_BLOCK_ACK_MAX_WINDOW as usize;

/// Semantic BlockAck information decoded by the PAC completion owner.
///
/// Bit zero acknowledges `starting_sequence`, bit one the following sequence,
/// and so on. The S31 completion block exposes 64 bits even though strict mode
/// deliberately negotiates a window of at most 32 frames.
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
