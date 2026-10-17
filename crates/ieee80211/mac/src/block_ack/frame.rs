//! Stateless Block Ack Action body parsing and wire identifiers.

use crate::sequence::SequenceNumber;

pub const BLOCK_ACK_CATEGORY: u8 = 3;
pub const ADDBA_REQUEST_ACTION: u8 = 0;
pub const ADDBA_RESPONSE_ACTION: u8 = 1;
pub const DELBA_ACTION: u8 = 2;
pub const ADDBA_ACTION_BODY_LEN: usize = 9;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockAckAction {
    AddbaRequest {
        dialog_token: u8,
        tid: u8,
        immediate: bool,
        amsdu: bool,
        window: u16,
        timeout_tu: u16,
        starting_sequence: SequenceNumber,
    },
    AddbaResponse {
        dialog_token: u8,
        status: u16,
        tid: u8,
        immediate: bool,
        amsdu: bool,
        window: u16,
        timeout_tu: u16,
    },
    Delba {
        tid: u8,
        initiator: bool,
        reason: u16,
    },
}

/// Parse the body of one IEEE 802.11 Block Ack Action frame.
///
/// This is a stateless leaf: it only reads the supplied bytes and does not
/// allocate, wait, access global state or call into the vendor library.
pub fn parse_block_ack_action(body: &[u8]) -> Option<BlockAckAction> {
    if body.len() < 2 || body[0] != BLOCK_ACK_CATEGORY {
        return None;
    }
    match body[1] {
        ADDBA_REQUEST_ACTION if body.len() >= ADDBA_ACTION_BODY_LEN => {
            let parameters = u16::from_le_bytes([body[3], body[4]]);
            let starting_sequence =
                SequenceNumber::from_sequence_control(u16::from_le_bytes([body[7], body[8]]));
            Some(BlockAckAction::AddbaRequest {
                dialog_token: body[2],
                tid: ((parameters >> 2) & 0x0f) as u8,
                immediate: parameters & 0x0002 != 0,
                amsdu: parameters & 0x0001 != 0,
                window: (parameters >> 6) & 0x03ff,
                timeout_tu: u16::from_le_bytes([body[5], body[6]]),
                starting_sequence,
            })
        }
        ADDBA_RESPONSE_ACTION if body.len() >= ADDBA_ACTION_BODY_LEN => {
            let parameters = u16::from_le_bytes([body[5], body[6]]);
            Some(BlockAckAction::AddbaResponse {
                dialog_token: body[2],
                status: u16::from_le_bytes([body[3], body[4]]),
                tid: ((parameters >> 2) & 0x0f) as u8,
                immediate: parameters & 0x0002 != 0,
                amsdu: parameters & 0x0001 != 0,
                window: (parameters >> 6) & 0x03ff,
                timeout_tu: u16::from_le_bytes([body[7], body[8]]),
            })
        }
        DELBA_ACTION if body.len() >= 6 => {
            let parameters = u16::from_le_bytes([body[2], body[3]]);
            Some(BlockAckAction::Delba {
                tid: ((parameters >> 12) & 0x0f) as u8,
                initiator: parameters & 0x0800 != 0,
                reason: u16::from_le_bytes([body[4], body[5]]),
            })
        }
        _ => None,
    }
}

/// Length of a Compressed BlockAckReq frame without its FCS.
pub const BLOCK_ACK_REQUEST_LEN: usize = 20;

/// Encode one Basic-policy Compressed BlockAckReq for `tid` whose window
/// starts at `starting_sequence`.
///
/// Frame Control is control subtype 8 with no flags and Duration is zero;
/// BAR Control carries normal Ack Policy, no Multi-TID, a compressed bitmap
/// and the TID; the Starting Sequence Control has fragment number zero.
///
/// SOURCE(esp32s31): `libpp[pp.o]::ppPrepareBarFrame` writes Frame Control 0x0084
/// once and never a Duration; `libpp[pp.o]::ppFillAMPDUBar(tid, ra, ta, ssn)` copies RA
/// and TA, writes BAR Control `(tid << 12) | 0x0004` and Starting Sequence
/// Control `ssn << 4`, and clears the Retry bit.
pub fn encode_block_ack_request(
    receiver: [u8; 6],
    transmitter: [u8; 6],
    tid: u8,
    starting_sequence: SequenceNumber,
) -> [u8; BLOCK_ACK_REQUEST_LEN] {
    let mut frame = [0_u8; BLOCK_ACK_REQUEST_LEN];
    frame[..2].copy_from_slice(&0x0084_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&receiver);
    frame[10..16].copy_from_slice(&transmitter);
    let control = (u16::from(tid & 0x0f) << 12) | 0x0004;
    frame[16..18].copy_from_slice(&control.to_le_bytes());
    frame[18..20].copy_from_slice(&starting_sequence.sequence_control().to_le_bytes());
    frame
}

/// Why an ADDBA Response body could not be written.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxAddbaResponseError {
    InvalidBodyLength(usize),
    InvalidTid(u8),
    InvalidWindow(u16),
}

/// Write one successful immediate BlockAck response into an owned action body.
///
/// A-MSDU is deliberately not advertised by the initial strict RX path.
pub fn write_successful_addba_response(
    body: &mut [u8],
    dialog_token: u8,
    tid: u8,
    window: u16,
) -> Result<(), RxAddbaResponseError> {
    if body.len() != 9 {
        return Err(RxAddbaResponseError::InvalidBodyLength(body.len()));
    }
    if tid > 15 {
        return Err(RxAddbaResponseError::InvalidTid(tid));
    }
    if window == 0 || window > 0x03ff {
        return Err(RxAddbaResponseError::InvalidWindow(window));
    }
    body.fill(0);
    body[0] = BLOCK_ACK_CATEGORY;
    body[1] = ADDBA_RESPONSE_ACTION;
    body[2] = dialog_token;
    let parameters = 1_u16 << 1 | u16::from(tid) << 2 | window << 6;
    body[5..7].copy_from_slice(&parameters.to_le_bytes());
    Ok(())
}

/// IEEE 802.11 status 37: the recipient cannot accept this agreement.
pub const ADDBA_STATUS_REQUEST_DECLINED: u16 = 37;

/// Write a finite rejection for a syntactically parsed ADDBA request.
///
/// The request fields remain evidence in the response, but status owns the
/// semantic outcome. No software session or hardware bank may accompany this
/// body.
pub fn write_declined_addba_response(
    body: &mut [u8],
    dialog_token: u8,
    tid: u8,
    requested_window: u16,
) -> Result<(), RxAddbaResponseError> {
    if body.len() != 9 {
        return Err(RxAddbaResponseError::InvalidBodyLength(body.len()));
    }
    if tid > 15 {
        return Err(RxAddbaResponseError::InvalidTid(tid));
    }
    if requested_window > 0x03ff {
        return Err(RxAddbaResponseError::InvalidWindow(requested_window));
    }
    body.fill(0);
    body[0] = BLOCK_ACK_CATEGORY;
    body[1] = ADDBA_RESPONSE_ACTION;
    body[2] = dialog_token;
    body[3..5].copy_from_slice(&ADDBA_STATUS_REQUEST_DECLINED.to_le_bytes());
    let parameters = 1_u16 << 1 | u16::from(tid) << 2 | requested_window << 6;
    body[5..7].copy_from_slice(&parameters.to_le_bytes());
    Ok(())
}

#[cfg(test)]
mod tests;
