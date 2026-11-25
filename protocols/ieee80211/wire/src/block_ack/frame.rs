//! Stateless Block Ack Action body parsing and wire identifiers.
//!
//! Received bytes reach this module; nothing here panics on them.

use crate::frame::{ControlSubtype, FrameControlFields};
use crate::management::DialogToken;
use crate::qos::Tid;
use crate::sequence::{SequenceControl, SequenceNumber};
use oer_protocols_ieee802_wire::Eui48;

pub const BLOCK_ACK_CATEGORY: u8 = 3;
pub const ADDBA_REQUEST_ACTION: u8 = 0;
pub const ADDBA_RESPONSE_ACTION: u8 = 1;
pub const DELBA_ACTION: u8 = 2;
pub const ADDBA_ACTION_BODY_LEN: usize = 9;
/// Category, Block Ack Action, DELBA Parameter Set and Reason Code.
///
/// SOURCE: IEEE Std 802.11-2020, 9.6.5.5 (DELBA frame format).
pub const DELBA_ACTION_BODY_LEN: usize = 6;

/// The A-MSDU Supported subfield of a Block Ack Parameter Set: the
/// agreement's frames may carry A-MSDUs, or not (IEEE Std 802.11-2020,
/// 9.4.1.14).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmsduSupport {
    Supported,
    NotSupported,
}

/// The Initiator subfield of a DELBA Parameter Set: the originator or the
/// recipient of the agreement tears it down (IEEE Std 802.11-2020,
/// 9.4.1.16).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DelbaInitiator {
    Originator,
    Recipient,
}

/// A field that cannot be represented by an immediate ADDBA request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddbaRequestError {
    InvalidWindow(u16),
}

impl From<AddbaParametersError> for AddbaRequestError {
    fn from(error: AddbaParametersError) -> Self {
        match error {
            AddbaParametersError::WindowOutOfRange(window) => Self::InvalidWindow(window),
        }
    }
}

/// Encode an immediate-policy ADDBA Request action body.
///
/// The session owner supplies the dialog token, timeout and starting sequence;
/// the wire encoder owns the BA Parameter Set and field order.
pub fn encode_addba_request(
    dialog_token: DialogToken,
    tid: Tid,
    window: u16,
    timeout_tu: u16,
    amsdu: AmsduSupport,
    starting_sequence: SequenceNumber,
) -> Result<[u8; ADDBA_ACTION_BODY_LEN], AddbaRequestError> {
    let parameters = AddbaParameters::immediate(amsdu, tid, window)?;
    // A request names the buffer it asks for: zero is no buffer.
    if window == 0 {
        return Err(AddbaRequestError::InvalidWindow(window));
    }
    let [p0, p1] = parameters.to_bytes();
    let [t0, t1] = BlockAckTimeout::of(timeout_tu).to_bytes();
    let [s0, s1] = starting_sequence.sequence_control().to_bytes();
    // SOURCE: IEEE Std 802.11-2020, 9.6.5.2 (ADDBA Request frame format):
    // Category, Block Ack Action, Dialog Token, Block Ack Parameter Set,
    // Block Ack Timeout Value, Block Ack Starting Sequence Control.
    Ok([
        BLOCK_ACK_CATEGORY,
        ADDBA_REQUEST_ACTION,
        dialog_token.value(),
        p0,
        p1,
        t0,
        t1,
        s0,
        s1,
    ])
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockAckAction {
    AddbaRequest {
        dialog_token: DialogToken,
        parameters: AddbaParameters,
        timeout_tu: u16,
        starting_sequence: SequenceNumber,
    },
    AddbaResponse {
        dialog_token: DialogToken,
        status: u16,
        parameters: AddbaParameters,
        timeout_tu: u16,
    },
    Delba {
        tid: Tid,
        initiator: DelbaInitiator,
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
    let mut reader = oer_portable_wire::Reader::new(fields);
    match *action {
        // SOURCE: IEEE Std 802.11-2020, 9.6.5.2 (ADDBA Request frame
        // format).
        ADDBA_REQUEST_ACTION => Some(BlockAckAction::AddbaRequest {
            dialog_token: DialogToken::new(reader.u8().ok()?),
            parameters: AddbaParameters::from_bytes(reader.read().ok()?),
            timeout_tu: BlockAckTimeout::from_bytes(reader.read().ok()?).time_units(),
            starting_sequence: SequenceNumber::from_sequence_control(SequenceControl::from_bytes(
                reader.read().ok()?,
            )),
        }),
        // SOURCE: IEEE Std 802.11-2020, 9.6.5.3 (ADDBA Response frame
        // format).
        ADDBA_RESPONSE_ACTION => Some(BlockAckAction::AddbaResponse {
            dialog_token: DialogToken::new(reader.u8().ok()?),
            status: crate::management::StatusCodeField::from_bytes(reader.read().ok()?).code(),
            parameters: AddbaParameters::from_bytes(reader.read().ok()?),
            timeout_tu: BlockAckTimeout::from_bytes(reader.read().ok()?).time_units(),
        }),
        // SOURCE: IEEE Std 802.11-2020, 9.6.5.5 (DELBA frame format).
        DELBA_ACTION => {
            let parameters = DelbaParameters::from_bytes(reader.read().ok()?);
            Some(BlockAckAction::Delba {
                tid: parameters.tid(),
                initiator: if parameters.initiator() {
                    DelbaInitiator::Originator
                } else {
                    DelbaInitiator::Recipient
                },
                reason: crate::management::ReasonCodeField::from_bytes(reader.read().ok()?).code(),
            })
        }
        _ => None,
    }
}

/// A BlockAckReq's Frame Control: control subtype 8, every flag clear.
const BLOCK_ACK_REQUEST_FRAME_CONTROL: FrameControlFields =
    FrameControlFields::from_bytes([0x84, 0x00]);

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
    receiver: Eui48,
    transmitter: Eui48,
    tid: Tid,
    starting_sequence: SequenceNumber,
) -> [u8; BLOCK_ACK_REQUEST_LEN] {
    let mut frame = [0_u8; BLOCK_ACK_REQUEST_LEN];
    crate::encoding::write_array::<0, _, _>(
        &mut frame,
        &BLOCK_ACK_REQUEST_FRAME_CONTROL.to_bytes(),
    );
    crate::encoding::write_array::<4, _, _>(&mut frame, &receiver.octets());
    crate::encoding::write_array::<10, _, _>(&mut frame, &transmitter.octets());
    let control = BarControlFields::from_bytes([4, 0]).with_tid(tid);
    crate::encoding::write_array::<16, _, _>(&mut frame, &control.to_bytes());
    crate::encoding::write_array::<18, _, _>(
        &mut frame,
        &starting_sequence.sequence_control().to_bytes(),
    );
    frame
}

/// Why an ADDBA Response body could not be written.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxAddbaResponseError {
    InvalidBodyLength(usize),
    InvalidWindow(u16),
}

impl From<AddbaParametersError> for RxAddbaResponseError {
    fn from(error: AddbaParametersError) -> Self {
        match error {
            AddbaParametersError::WindowOutOfRange(window) => Self::InvalidWindow(window),
        }
    }
}

/// Write one successful immediate BlockAck response into an owned action body.
///
/// A-MSDU is deliberately not advertised by the initial strict RX path.
pub fn write_successful_addba_response(
    body: &mut [u8],
    dialog_token: DialogToken,
    tid: Tid,
    window: u16,
) -> Result<(), RxAddbaResponseError> {
    let length = body.len();
    let Ok(body) = <&mut [u8; ADDBA_ACTION_BODY_LEN]>::try_from(body) else {
        return Err(RxAddbaResponseError::InvalidBodyLength(length));
    };
    let written =
        encode_addba_response(dialog_token, 0, tid, window, 0, AmsduSupport::NotSupported)?;
    // A successful agreement has a buffer: zero is none.
    if window == 0 {
        return Err(RxAddbaResponseError::InvalidWindow(window));
    }
    *body = written;
    Ok(())
}

/// IEEE 802.11 status 37: the recipient cannot accept this agreement.
pub const ADDBA_STATUS_REQUEST_DECLINED: u16 = crate::status::status_code::REQUEST_DECLINED;

/// Write a finite rejection for a syntactically parsed ADDBA request.
///
/// The request fields remain evidence in the response, but status owns the
/// semantic outcome. No software session or hardware bank may accompany this
/// body.
pub fn write_declined_addba_response(
    body: &mut [u8],
    dialog_token: DialogToken,
    tid: Tid,
    requested_window: u16,
) -> Result<(), RxAddbaResponseError> {
    let length = body.len();
    let Ok(body) = <&mut [u8; ADDBA_ACTION_BODY_LEN]>::try_from(body) else {
        return Err(RxAddbaResponseError::InvalidBodyLength(length));
    };
    *body = encode_addba_response(
        dialog_token,
        ADDBA_STATUS_REQUEST_DECLINED,
        tid,
        requested_window,
        0,
        AmsduSupport::NotSupported,
    )?;
    Ok(())
}

/// Encode one immediate-policy ADDBA Response action body.
///
/// SOURCE: IEEE Std 802.11-2020, 9.6.5.3 (ADDBA Response frame format:
/// Category, Block Ack Action, Dialog Token, Status Code, Block Ack
/// Parameter Set and Block Ack Timeout Value).
///
/// # Errors
///
/// A window wider than ten bits.
pub fn encode_addba_response(
    dialog_token: DialogToken,
    status: u16,
    tid: Tid,
    window: u16,
    timeout_tu: u16,
    amsdu: AmsduSupport,
) -> Result<[u8; ADDBA_ACTION_BODY_LEN], RxAddbaResponseError> {
    let [p0, p1] = AddbaParameters::immediate(amsdu, tid, window)?.to_bytes();
    let [s0, s1] = crate::management::StatusCodeField::of(status).to_bytes();
    let [t0, t1] = BlockAckTimeout::of(timeout_tu).to_bytes();
    Ok([
        BLOCK_ACK_CATEGORY,
        ADDBA_RESPONSE_ACTION,
        dialog_token.value(),
        s0,
        s1,
        p0,
        p1,
        t0,
        t1,
    ])
}

/// Encode one DELBA action body tearing down `tid`'s agreement.
///
/// SOURCE: IEEE Std 802.11-2020, 9.6.5.5 (DELBA frame format).
pub fn encode_delba(
    tid: Tid,
    initiator: DelbaInitiator,
    reason: u16,
) -> [u8; DELBA_ACTION_BODY_LEN] {
    let parameters = DelbaParameters::from_bytes([0; 2])
        .with_tid(tid)
        .with_initiator(initiator == DelbaInitiator::Originator);
    let [p0, p1] = parameters.to_bytes();
    let [r0, r1] = crate::management::ReasonCodeField::of(reason).to_bytes();
    [BLOCK_ACK_CATEGORY, DELBA_ACTION, p0, p1, r0, r1]
}

/// One received Compressed BlockAckReq: a single-TID request whose bitmap
/// answer is compressed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompressedBlockAckRequest {
    pub receiver: Eui48,
    pub transmitter: Eui48,
    pub tid: Tid,
    pub starting_sequence: SequenceNumber,
}

/// BAR Type of a Compressed BlockAckReq.
///
/// SOURCE: IEEE Std 802.11-2020, 9.3.1.7.1, Table 9-24 (BlockAckReq frame
/// variant encoding: 2 is Compressed).
pub const COMPRESSED_BAR_TYPE: u8 = 2;

/// Parse a Compressed BlockAckReq of protocol version 0.
///
/// `None` for another frame, a frame with To/From DS, Protected, More
/// Fragments or Order set, another BAR variant, a starting sequence with a
/// nonzero fragment number or a frame shorter than the request.
///
/// SOURCE: IEEE Std 802.11-2020, 9.3.1.7 (BlockAckReq frame format) and
/// 9.3.1.7.1 (BAR Control field).
pub fn parse_compressed_block_ack_request(frame: &[u8]) -> Option<CompressedBlockAckRequest> {
    let (&[f0, f1, _, _], rest) = frame.split_first_chunk::<4>()?;
    let control = FrameControlFields::from_bytes([f0, f1]);
    if control.version() != 0
        || !control.is_control(ControlSubtype::BlockAckRequest)
        || control.ds() != 0
        || control.protected()
        || control.more_fragments()
        || control.order()
    {
        return None;
    }
    let (&receiver, rest) = rest.split_first_chunk::<{ Eui48::LEN }>()?;
    let (&transmitter, rest) = rest.split_first_chunk::<{ Eui48::LEN }>()?;
    let (&[c0, c1, s0, s1], _) = rest.split_first_chunk::<4>()?;
    let parameters = BarControlFields::from_bytes([c0, c1]);
    if parameters.bar_type() != COMPRESSED_BAR_TYPE {
        return None;
    }
    let sequence = SequenceControl::from_bytes([s0, s1]);
    if sequence.fragment() != 0 {
        return None;
    }
    Some(CompressedBlockAckRequest {
        receiver: Eui48::new(receiver),
        transmitter: Eui48::new(transmitter),
        tid: parameters.tid(),
        starting_sequence: SequenceNumber::from_sequence_control(
            crate::sequence::SequenceControl::from_bytes([s0, s1]),
        ),
    })
}

oer_portable_wire::bitfield! {
    /// The Block Ack Policy subfield of a Block Ack Parameter Set.
    /// SOURCE: IEEE Std 802.11-2020, 9.4.1.14 (Block Ack Parameter Set
    /// field).
    pub enum BlockAckPolicy: u8 [1] { Delayed = 0, Immediate = 1 }
}
oer_portable_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.1.14 (Block Ack Parameter Set
    /// field).
    pub struct AddbaParameters[2] (little) {
        pub amsdu, with_amsdu: bool = 0..1;
        pub policy, with_policy: (BlockAckPolicy) = 1..2;
        pub tid, with_tid: (Tid) = 2..6;
        pub window, with_window: u16 = 6..16;
    }
}

/// Why a Block Ack Parameter Set cannot hold a value (IEEE Std
/// 802.11-2020, 9.4.1.14: a ten-bit Buffer Size).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddbaParametersError {
    WindowOutOfRange(u16),
}

impl AddbaParameters {
    /// The parameters of an agreement under `policy` for `tid` and
    /// `window`.
    ///
    /// # Errors
    ///
    /// A window wider than ten.
    pub const fn new(
        policy: BlockAckPolicy,
        amsdu: AmsduSupport,
        tid: Tid,
        window: u16,
    ) -> Result<Self, AddbaParametersError> {
        let parameters = Self::from_bytes([0; 2])
            .with_amsdu(matches!(amsdu, AmsduSupport::Supported))
            .with_policy(policy)
            .with_tid(tid);
        match parameters.with_window(window) {
            Ok(parameters) => Ok(parameters),
            Err(_) => Err(AddbaParametersError::WindowOutOfRange(window)),
        }
    }

    /// An immediate agreement's parameters; [`Self::new`].
    ///
    /// # Errors
    ///
    /// As [`Self::new`].
    pub const fn immediate(
        amsdu: AmsduSupport,
        tid: Tid,
        window: u16,
    ) -> Result<Self, AddbaParametersError> {
        Self::new(BlockAckPolicy::Immediate, amsdu, tid, window)
    }
}

oer_portable_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.1.15 (Block Ack Timeout Value
    /// field), in time units.
    pub struct BlockAckTimeout[2] (little) {
        pub time_units, with_time_units: {u16} = 0..16;
    }
}

impl BlockAckTimeout {
    /// The field holding `time_units`.
    pub const fn of(time_units: u16) -> Self {
        Self::from_bytes([0; 2]).with_time_units(time_units)
    }
}
oer_portable_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.6.5 (DELBA frame format).
    pub struct DelbaParameters[2] (little) {
        pub initiator, with_initiator: bool = 11..12;
        pub tid, with_tid: (Tid) = 12..16;
    }
}

oer_portable_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.3.1.8 (BlockAckReq frame).
    pub struct BarControlFields[2] (little) {
        pub ack_policy, with_ack_policy: bool = 0..1;
        pub bar_type, with_bar_type: u8 = 1..5;
        pub tid, with_tid: (Tid) = 12..16;
    }
}

#[cfg(test)]
mod tests;
