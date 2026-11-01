//! Block Ack negotiation and receive reorder owners.

use oer_ieee80211_contracts_port::BlockAckReport;
use oer_ieee80211_wire::block_ack::{BlockAckAction, parse_block_ack_action};
use oer_ieee80211_wire::sequence::SequenceNumber;

/// Whether the transmitter may consider `sequence` delivered by `report`.
///
/// Recipients use both standard-compliant starting-sequence conventions:
/// some keep the oldest possible sequence and describe it with the
/// bitmap, others advance the start to the first sequence not yet
/// received. In the latter form an MPDU just before the window is
/// already delivered although it has no bitmap bit. Only a bounded
/// predecessor, at most one bitmap width behind the start, counts; a
/// sequence beyond either side of the window stays unacknowledged, so a
/// stale report cannot release newer traffic.
pub const fn acknowledges(report: BlockAckReport, sequence: SequenceNumber) -> bool {
    let distance = crate::sequence::forward_distance(report.start_sequence, sequence);
    if distance < BlockAckReport::BITMAP_BITS {
        report.bitmap & (1_u64 << distance) != 0
    } else {
        crate::sequence::forward_distance(sequence, report.start_sequence)
            <= BlockAckReport::BITMAP_BITS
    }
}

pub mod banks;
pub mod originator;
pub mod recipient;
pub mod reorder;
pub mod session;

pub use banks::{
    AgreementStop, Parked, RX_BLOCK_ACK_WINDOW, ReorderCause, ReplyPlace, RxBlockAckCapacity,
    RxBlockAckEvent, RxBlockAckPort, RxBlockAckReplyBody, RxBlockAckStorage, RxBlockAckTypes,
    RxBlockAckView, RxDisposition,
};
pub use originator::{
    TX_BLOCK_ACK_MAX_TIDS, TxBlockAckOriginator, TxBlockAckOriginatorConfig,
    TxBlockAckOriginatorError, TxBlockAckOriginatorPolicy, TxBlockAckOriginatorResponse,
    TxBlockAckResponseDisposition, TxBlockAckRetry, next_nonzero_dialog_token,
};
pub use recipient::{
    AddbaResponse, AddbaStatus, RxAddbaRequest, RxBlockAckError, RxBlockAckNegotiation,
    RxBlockAckSession,
};
pub use reorder::{
    MAX_RX_REORDER_WINDOW, RxReorderBuffer, RxReorderError, RxReorderMpdu, RxReorderRelease,
};
pub use session::{
    AddbaRequest, MAX_TX_BLOCK_ACK_WINDOW, OperationalTxBlockAck, TxBlockAckAlarm,
    TxBlockAckConfig, TxBlockAckDialogToken, TxBlockAckError, TxBlockAckResponse,
    TxBlockAckSession,
};

pub mod batch;

#[cfg(test)]
mod tests;
