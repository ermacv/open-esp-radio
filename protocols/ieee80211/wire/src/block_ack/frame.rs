//! Stateless Block Ack Action body parsing and wire identifiers.
//!
//! Received bytes reach this module; nothing here panics on them.
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]

use crate::frame::QosControl;
use crate::sequence::SequenceNumber;

pub const BLOCK_ACK_CATEGORY: u8 = 3;
pub const ADDBA_REQUEST_ACTION: u8 = 0;
pub const ADDBA_RESPONSE_ACTION: u8 = 1;
pub const DELBA_ACTION: u8 = 2;
pub const ADDBA_ACTION_BODY_LEN: usize = 9;

/// A field that cannot be represented by an immediate ADDBA request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddbaRequestError {
    InvalidTid(u8),
    InvalidWindow(u16),
}

/// Encode an immediate-policy ADDBA Request action body.
///
/// The session owner supplies the dialog token, timeout and starting sequence;
/// the wire encoder owns the BA Parameter Set and field order.
pub fn encode_addba_request(
    dialog_token: u8,
    tid: u8,
    window: u16,
    timeout_tu: u16,
    amsdu: bool,
    starting_sequence: SequenceNumber,
) -> Result<[u8; ADDBA_ACTION_BODY_LEN], AddbaRequestError> {
    if tid > 15 {
        return Err(AddbaRequestError::InvalidTid(tid));
    }
    if window == 0 || window > 0x03ff {
        return Err(AddbaRequestError::InvalidWindow(window));
    }
    let parameters = AddbaParameters::from_bytes([0; 2])
        .with_amsdu(amsdu)
        .with_immediate(true)
        .with_tid(tid)
        .map_err(|_| AddbaRequestError::InvalidTid(tid))?
        .with_window(window)
        .map_err(|_| AddbaRequestError::InvalidWindow(window))?;
    let mut body = [0; ADDBA_ACTION_BODY_LEN];
    body[0] = BLOCK_ACK_CATEGORY;
    body[1] = ADDBA_REQUEST_ACTION;
    body[2] = dialog_token;
    crate::encoding::write_array::<3, _, _>(&mut body, &parameters.to_bytes());
    crate::encoding::write_array::<5, _, _>(&mut body, &timeout_tu.to_le_bytes());
    crate::encoding::write_array::<7, _, _>(
        &mut body,
        &starting_sequence.sequence_control().to_le_bytes(),
    );
    Ok(body)
}

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
    let [BLOCK_ACK_CATEGORY, action, fields @ ..] = body else {
        return None;
    };
    match *action {
        ADDBA_REQUEST_ACTION => {
            let mut reader = oer_wire::Reader::new(fields);
            let &[dialog_token, p0, p1, t0, t1, s0, s1] = reader.array().ok()?;
            let parameters = AddbaParameters::from_bytes([p0, p1]);
            Some(BlockAckAction::AddbaRequest {
                dialog_token,
                tid: parameters.tid(),
                immediate: parameters.immediate(),
                amsdu: parameters.amsdu(),
                window: parameters.window(),
                timeout_tu: u16::from_le_bytes([t0, t1]),
                starting_sequence: SequenceNumber::from_sequence_control(u16::from_le_bytes([
                    s0, s1,
                ])),
            })
        }
        ADDBA_RESPONSE_ACTION => {
            let mut reader = oer_wire::Reader::new(fields);
            let &[dialog_token, s0, s1, p0, p1, t0, t1] = reader.array().ok()?;
            let parameters = AddbaParameters::from_bytes([p0, p1]);
            Some(BlockAckAction::AddbaResponse {
                dialog_token,
                status: u16::from_le_bytes([s0, s1]),
                tid: parameters.tid(),
                immediate: parameters.immediate(),
                amsdu: parameters.amsdu(),
                window: parameters.window(),
                timeout_tu: u16::from_le_bytes([t0, t1]),
            })
        }
        DELBA_ACTION => {
            let mut reader = oer_wire::Reader::new(fields);
            let &[p0, p1, r0, r1] = reader.array().ok()?;
            let parameters = DelbaParameters::from_bytes([p0, p1]);
            Some(BlockAckAction::Delba {
                tid: parameters.tid(),
                initiator: parameters.initiator(),
                reason: u16::from_le_bytes([r0, r1]),
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
    crate::encoding::write_array::<0, _, _>(&mut frame, &0x0084_u16.to_le_bytes());
    crate::encoding::write_array::<4, _, _>(&mut frame, &receiver);
    crate::encoding::write_array::<10, _, _>(&mut frame, &transmitter);
    let tid = QosControl::from_bytes([tid, 0]).tid();
    let control = crate::trigger::BarControlFields::from_bytes([4, 0]).with_tid(tid);
    crate::encoding::write_array::<16, _, _>(&mut frame, &control.to_bytes());
    crate::encoding::write_array::<18, _, _>(
        &mut frame,
        &starting_sequence.sequence_control().to_le_bytes(),
    );
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
    let length = body.len();
    let Ok(body) = <&mut [u8; ADDBA_ACTION_BODY_LEN]>::try_from(body) else {
        return Err(RxAddbaResponseError::InvalidBodyLength(length));
    };
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
    let parameters = AddbaParameters::from_bytes([0; 2])
        .with_amsdu(false)
        .with_immediate(true)
        .with_tid(tid)
        .map_err(|_| RxAddbaResponseError::InvalidTid(tid))?
        .with_window(window)
        .map_err(|_| RxAddbaResponseError::InvalidWindow(window))?;
    crate::encoding::write_array::<5, _, _>(body, &parameters.to_bytes());
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
    let length = body.len();
    let Ok(body) = <&mut [u8; ADDBA_ACTION_BODY_LEN]>::try_from(body) else {
        return Err(RxAddbaResponseError::InvalidBodyLength(length));
    };
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
    crate::encoding::write_array::<3, _, _>(body, &ADDBA_STATUS_REQUEST_DECLINED.to_le_bytes());
    let parameters = AddbaParameters::from_bytes([0; 2])
        .with_amsdu(false)
        .with_immediate(true)
        .with_tid(tid)
        .map_err(|_| RxAddbaResponseError::InvalidTid(tid))?
        .with_window(requested_window)
        .map_err(|_| RxAddbaResponseError::InvalidWindow(requested_window))?;
    crate::encoding::write_array::<5, _, _>(body, &parameters.to_bytes());
    Ok(())
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.6.5 (Block Ack Action frame details).
    pub struct AddbaParameters[2] (little) {
        pub amsdu, with_amsdu: bool = 0..1;
        pub immediate, with_immediate: bool = 1..2;
        pub tid, with_tid: u8 = 2..6;
        pub window, with_window: u16 = 6..16;
    }
}
oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.6.5 (DELBA frame format).
    pub struct DelbaParameters[2] (little) {
        pub initiator, with_initiator: bool = 11..12;
        pub tid, with_tid: u8 = 12..16;
    }
}

#[cfg(test)]
mod tests;
