use core::fmt;

use crc::{CRC_32_ISCSI, Crc};
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::{Envelope, Key, Message, WireKind};

// Covers the maximum peripheral snapshot, including full-width maintenance
// timeline edges. The framing tests exercise maximal values, not typical runs.
pub const MAX_POSTCARD_BYTES: usize = 560;
pub const WIRE_MAGIC: [u8; 4] = *b"ORHL";
/// The header layout. A payload's own type is identified by the header's
/// [`Key`], so the framing version changes only with the header itself.
pub const FRAMING_VERSION: u8 = 2;
/// magic 4, framing 1, kind 1, boot 8, sequence 4, session 8, request 4,
/// key 8, payload length 2.
pub const WIRE_HEADER_BYTES: usize = 40;
const _: () = {
    if WIRE_HEADER_BYTES != 4 + 1 + 1 + 8 + 4 + 8 + 4 + 8 + 2 {
        panic!("the header must hold every fixed-width field");
    }
};
#[cfg(test)]
const KEY_RANGE: core::ops::Range<usize> = 30..38;
#[cfg(test)]
const LENGTH_RANGE: core::ops::Range<usize> = 38..40;
const CHECKSUM_BYTES: usize = size_of::<u32>();
const MAX_RAW_FRAME_BYTES: usize = WIRE_HEADER_BYTES + MAX_POSTCARD_BYTES + CHECKSUM_BYTES;
const MAX_COBS_FRAME_BYTES: usize = cobs::max_encoding_length(MAX_RAW_FRAME_BYTES);
pub const MAX_WIRE_FRAME_BYTES: usize = 2 + MAX_COBS_FRAME_BYTES + 1;

const CRC32C: Crc<u32> = Crc::<u32>::new(&CRC_32_ISCSI);

/// The CRC-32C of a network session's evidence digest.
#[cfg(feature = "network")]
pub(crate) fn crc32c(bytes: &[u8]) -> u32 {
    CRC32C.checksum(bytes)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncodeError {
    Serialize,
}

impl fmt::Display for EncodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize => {
                formatter.write_str("HIL message exceeds the wire frame or cannot be serialized")
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeError {
    Cobs,
    TooShort,
    Magic,
    FramingVersion,
    MessageKind,
    PayloadLength,
    Checksum,
    /// An intact frame whose key names this type but whose payload does not
    /// decode as it: a defect of the sender, since equal keys mean equal
    /// schemas.
    Payload,
}

/// The identity a frame's header gives its request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestIdentity {
    pub boot_id: u64,
    pub session_id: u64,
    pub request_id: u32,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cobs => formatter.write_str("invalid COBS frame"),
            Self::TooShort => {
                formatter.write_str("HIL frame does not contain a complete header and checksum")
            }
            Self::Magic => formatter.write_str("invalid HIL wire magic"),
            Self::FramingVersion => formatter.write_str("unsupported HIL framing version"),
            Self::MessageKind => formatter.write_str("HIL frame has the wrong message direction"),
            Self::PayloadLength => {
                formatter.write_str("HIL frame payload length does not match its header")
            }
            Self::Checksum => formatter.write_str("HIL frame checksum mismatch"),
            Self::Payload => {
                formatter.write_str("HIL frame payload does not decode as the type its key names")
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct DecodeCounters {
    pub frames: u32,
    pub cobs_errors: u32,
    pub too_short: u32,
    pub header_errors: u32,
    pub framing_version_errors: u32,
    pub message_kind_errors: u32,
    pub payload_length_errors: u32,
    pub checksum_errors: u32,
    pub deserialize_errors: u32,
    pub overflows: u32,
}

pub struct FrameEncoder {
    raw: [u8; MAX_RAW_FRAME_BYTES],
    wire: [u8; MAX_WIRE_FRAME_BYTES],
}

impl Default for FrameEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameEncoder {
    pub const fn new() -> Self {
        Self {
            raw: [0; MAX_RAW_FRAME_BYTES],
            wire: [0; MAX_WIRE_FRAME_BYTES],
        }
    }

    /// Encodes one independently resynchronizable frame.
    ///
    /// A leading delimiter discards any preceding ROM or text output. The
    /// trailing delimiter terminates this frame for an incremental decoder.
    pub fn encode<M: Message>(&mut self, message: &Envelope<M>) -> Result<&[u8], EncodeError> {
        let payload_length = postcard::to_slice(
            &message.body,
            self.raw
                .get_mut(WIRE_HEADER_BYTES..WIRE_HEADER_BYTES + MAX_POSTCARD_BYTES)
                .ok_or(EncodeError::Serialize)?,
        )
        .map_err(|_| EncodeError::Serialize)?
        .len();
        let header = Header {
            kind: M::WIRE_KIND,
            key: M::KEY,
            boot_id: message.boot_id,
            message_sequence: message.message_sequence,
            session_id: message.session_id,
            request_id: message.request_id,
        };
        self.frame(&header, payload_length)
    }

    /// Frames `message`, serialized earlier by its producer, for boot
    /// `boot_id`.
    pub fn encode_outbound(
        &mut self,
        boot_id: u64,
        message: &Outbound,
    ) -> Result<&[u8], EncodeError> {
        let payload_length = message.payload.len();
        let end = WIRE_HEADER_BYTES
            .checked_add(payload_length)
            .ok_or(EncodeError::Serialize)?;
        let destination = self
            .raw
            .get_mut(WIRE_HEADER_BYTES..end)
            .ok_or(EncodeError::Serialize)?;
        for (byte, source) in destination.iter_mut().zip(&message.payload) {
            *byte = *source;
        }
        let header = Header {
            kind: message.kind,
            key: message.key,
            boot_id,
            message_sequence: message.message_sequence,
            session_id: message.session_id,
            request_id: message.request_id,
        };
        self.frame(&header, payload_length)
    }

    /// Completes the frame whose payload `raw` already holds.
    fn frame(&mut self, header: &Header, payload_length: usize) -> Result<&[u8], EncodeError> {
        let payload_length_u16 =
            u16::try_from(payload_length).map_err(|_| EncodeError::Serialize)?;
        let boot = header.boot_id.to_le_bytes();
        let sequence = header.message_sequence.to_le_bytes();
        let session = header.session_id.to_le_bytes();
        let request = header.request_id.to_le_bytes();
        let length = payload_length_u16.to_le_bytes();
        let fields = [
            WIRE_MAGIC.as_slice(),
            &[FRAMING_VERSION, header.kind as u8],
            &boot,
            &sequence,
            &session,
            &request,
            &header.key.0,
            &length,
        ];
        let destination = self
            .raw
            .first_chunk_mut::<WIRE_HEADER_BYTES>()
            .ok_or(EncodeError::Serialize)?;
        for (byte, source) in destination.iter_mut().zip(fields.into_iter().flatten()) {
            *byte = *source;
        }
        let protected_length = WIRE_HEADER_BYTES
            .checked_add(payload_length)
            .ok_or(EncodeError::Serialize)?;
        let raw_length = protected_length
            .checked_add(CHECKSUM_BYTES)
            .ok_or(EncodeError::Serialize)?;
        let protected = self
            .raw
            .get(..protected_length)
            .ok_or(EncodeError::Serialize)?;
        let checksum = CRC32C.checksum(protected).to_le_bytes();
        let checksum_bytes = self
            .raw
            .get_mut(protected_length..raw_length)
            .and_then(<[u8]>::first_chunk_mut::<CHECKSUM_BYTES>)
            .ok_or(EncodeError::Serialize)?;
        *checksum_bytes = checksum;

        // Two leading delimiters recover even when arbitrary pre-protocol
        // binary output contained a zero and made the decoder enter a false
        // frame. In the normal case both simply keep it armed for a body.
        let [first, second, body @ ..] = &mut self.wire;
        *first = 0;
        *second = 0;
        let encoded = body
            .first_chunk_mut::<MAX_COBS_FRAME_BYTES>()
            .ok_or(EncodeError::Serialize)?;
        let raw = self.raw.get(..raw_length).ok_or(EncodeError::Serialize)?;
        let encoded_length = cobs::try_encode(raw, encoded).map_err(|_| EncodeError::Serialize)?;
        *body.get_mut(encoded_length).ok_or(EncodeError::Serialize)? = 0;
        let wire_length = encoded_length
            .checked_add(3)
            .ok_or(EncodeError::Serialize)?;
        self.wire.get(..wire_length).ok_or(EncodeError::Serialize)
    }
}

struct Header {
    kind: WireKind,
    key: Key,
    boot_id: u64,
    message_sequence: u32,
    session_id: u64,
    request_id: u32,
}

/// One message its producer serialized, waiting for the sender's single
/// [`FrameEncoder`], which adds the boot and completes the frame. A queue of
/// these carries messages of every type.
#[derive(Clone, Eq, PartialEq)]
pub struct Outbound {
    pub kind: WireKind,
    pub key: Key,
    pub message_sequence: u32,
    pub session_id: u64,
    pub request_id: u32,
    pub payload: heapless::Vec<u8, MAX_POSTCARD_BYTES>,
}

impl fmt::Debug for Outbound {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Outbound")
            .field("kind", &self.kind)
            .field("key", &self.key)
            .field("message_sequence", &self.message_sequence)
            .field("session_id", &self.session_id)
            .field("request_id", &self.request_id)
            .field("payload_length", &self.payload.len())
            .finish()
    }
}

impl Outbound {
    pub fn new<M: Message>(
        message_sequence: u32,
        session_id: u64,
        request_id: u32,
        body: &M,
    ) -> Result<Self, EncodeError> {
        let mut payload = heapless::Vec::from_array([0; MAX_POSTCARD_BYTES]);
        let length = postcard::to_slice(body, &mut payload)
            .map_err(|_| EncodeError::Serialize)?
            .len();
        payload.truncate(length);
        Ok(Self {
            kind: M::WIRE_KIND,
            key: M::KEY,
            message_sequence,
            session_id,
            request_id,
            payload,
        })
    }

    /// Whether this is message `M`.
    pub fn is<M: Message>(&self) -> bool {
        self.key == M::KEY
    }
}

impl Drop for Outbound {
    fn drop(&mut self) {
        self.payload.zeroize();
    }
}

impl Drop for FrameEncoder {
    fn drop(&mut self) {
        self.raw.zeroize();
        self.wire.zeroize();
    }
}

pub struct FrameDecoder {
    encoded: [u8; MAX_COBS_FRAME_BYTES],
    length: usize,
    inside_frame: bool,
    discard_until_delimiter: bool,
    counters: DecodeCounters,
}

impl Default for FrameDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameDecoder {
    pub const fn new() -> Self {
        Self {
            encoded: [0; MAX_COBS_FRAME_BYTES],
            length: 0,
            inside_frame: false,
            discard_until_delimiter: false,
            counters: DecodeCounters {
                frames: 0,
                cobs_errors: 0,
                too_short: 0,
                header_errors: 0,
                framing_version_errors: 0,
                message_kind_errors: 0,
                payload_length_errors: 0,
                checksum_errors: 0,
                deserialize_errors: 0,
                overflows: 0,
            },
        }
    }

    pub const fn counters(&self) -> DecodeCounters {
        self.counters
    }

    /// Counts a frame whose payload did not decode as the type its key
    /// names; the receiver decodes payloads, so it reports them.
    pub fn count_payload_error(&mut self) {
        self.counters.deserialize_errors = self.counters.deserialize_errors.saturating_add(1);
    }

    /// Feeds arbitrary serial chunks and calls `receive` for every complete
    /// frame of direction `kind`. Empty delimiters are ignored, so senders may
    /// prefix every frame with a delimiter to recover from unframed boot
    /// output.
    pub fn feed(
        &mut self,
        kind: WireKind,
        bytes: &[u8],
        mut receive: impl FnMut(Result<Frame<'_>, DecodeError>),
    ) {
        for &byte in bytes {
            if byte == 0 {
                if self.discard_until_delimiter {
                    self.discard_until_delimiter = false;
                    self.inside_frame = false;
                    self.length = 0;
                    continue;
                }
                if !self.inside_frame {
                    self.inside_frame = true;
                    self.length = 0;
                    continue;
                }
                if self.length == 0 {
                    // Repeated leading delimiters deliberately keep the
                    // decoder armed for a body.
                    continue;
                }
                let length = self.length;
                self.inside_frame = false;
                self.length = 0;
                match self.decode(kind, length) {
                    Ok(frame) => receive(Ok(frame)),
                    Err(error) => receive(Err(error)),
                }
                self.encoded.zeroize();
                continue;
            }

            if self.discard_until_delimiter || !self.inside_frame {
                continue;
            }
            if let Some((slot, next_length)) = self
                .encoded
                .get_mut(self.length)
                .zip(self.length.checked_add(1))
            {
                *slot = byte;
                self.length = next_length;
            } else {
                self.counters.overflows = self.counters.overflows.saturating_add(1);
                self.encoded.zeroize();
                self.discard_until_delimiter = true;
                self.length = 0;
            }
        }
    }

    fn decode(&mut self, kind: WireKind, length: usize) -> Result<Frame<'_>, DecodeError> {
        let encoded = self
            .encoded
            .get_mut(..length)
            .ok_or(DecodeError::TooShort)?;
        let decoded_length = cobs::decode_in_place(encoded).map_err(|_| {
            self.counters.cobs_errors = self.counters.cobs_errors.saturating_add(1);
            DecodeError::Cobs
        })?;
        if decoded_length < WIRE_HEADER_BYTES + CHECKSUM_BYTES {
            self.counters.too_short = self.counters.too_short.saturating_add(1);
            return Err(DecodeError::TooShort);
        }
        let decoded = encoded.get(..decoded_length).ok_or(DecodeError::Cobs)?;
        let mut remaining = decoded;
        if take::<4>(&mut remaining)? != WIRE_MAGIC {
            self.counters.header_errors = self.counters.header_errors.saturating_add(1);
            return Err(DecodeError::Magic);
        }
        let [version] = take(&mut remaining)?;
        if version != FRAMING_VERSION {
            self.counters.framing_version_errors =
                self.counters.framing_version_errors.saturating_add(1);
            return Err(DecodeError::FramingVersion);
        }
        let [direction] = take(&mut remaining)?;
        if direction != kind as u8 {
            self.counters.message_kind_errors = self.counters.message_kind_errors.saturating_add(1);
            return Err(DecodeError::MessageKind);
        }
        let boot_id = u64::from_le_bytes(take(&mut remaining)?);
        let message_sequence = u32::from_le_bytes(take(&mut remaining)?);
        let session_id = u64::from_le_bytes(take(&mut remaining)?);
        let request_id = u32::from_le_bytes(take(&mut remaining)?);
        let key = Key(take(&mut remaining)?);
        let payload_length = usize::from(u16::from_le_bytes(take(&mut remaining)?));
        let Some((payload, checksum)) = remaining.split_at_checked(payload_length) else {
            self.counters.payload_length_errors =
                self.counters.payload_length_errors.saturating_add(1);
            return Err(DecodeError::PayloadLength);
        };
        let Ok(checksum) = <[u8; CHECKSUM_BYTES]>::try_from(checksum) else {
            self.counters.payload_length_errors =
                self.counters.payload_length_errors.saturating_add(1);
            return Err(DecodeError::PayloadLength);
        };
        let protected_length = WIRE_HEADER_BYTES
            .checked_add(payload_length)
            .ok_or(DecodeError::PayloadLength)?;
        let protected = decoded
            .get(..protected_length)
            .ok_or(DecodeError::PayloadLength)?;
        if CRC32C.checksum(protected) != u32::from_le_bytes(checksum) {
            self.counters.checksum_errors = self.counters.checksum_errors.saturating_add(1);
            return Err(DecodeError::Checksum);
        }
        self.counters.frames = self.counters.frames.saturating_add(1);
        Ok(Frame {
            boot_id,
            message_sequence,
            session_id,
            request_id,
            key,
            payload,
        })
    }
}

/// Consumes an entire fixed-width field, retaining its width in the type.
fn take<const N: usize>(remaining: &mut &[u8]) -> Result<[u8; N], DecodeError> {
    let (field, rest) = remaining
        .split_first_chunk::<N>()
        .ok_or(DecodeError::TooShort)?;
    *remaining = rest;
    Ok(*field)
}

/// One intact frame: its header and its still encoded payload. The key
/// names the payload's type; the receiver decodes the types it serves.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Frame<'a> {
    pub boot_id: u64,
    pub message_sequence: u32,
    pub session_id: u64,
    pub request_id: u32,
    pub key: Key,
    pub payload: &'a [u8],
}

impl fmt::Debug for Frame<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Frame")
            .field("boot_id", &self.boot_id)
            .field("key", &self.key)
            .field("message_sequence", &self.message_sequence)
            .field("session_id", &self.session_id)
            .field("request_id", &self.request_id)
            .field("payload_length", &self.payload.len())
            .finish()
    }
}

impl Frame<'_> {
    /// The request identity the header gives this frame.
    pub const fn identity(&self) -> RequestIdentity {
        RequestIdentity {
            boot_id: self.boot_id,
            session_id: self.session_id,
            request_id: self.request_id,
        }
    }

    /// This frame as message `M`: `None` when its key names another type.
    pub fn decode<M: Message>(&self) -> Option<Result<Envelope<M>, DecodeError>> {
        (self.key == M::KEY).then(|| {
            postcard::from_bytes(self.payload)
                .map(|body| Envelope {
                    boot_id: self.boot_id,
                    message_sequence: self.message_sequence,
                    session_id: self.session_id,
                    request_id: self.request_id,
                    body,
                })
                .map_err(|_| DecodeError::Payload)
        })
    }
}

impl Drop for FrameDecoder {
    fn drop(&mut self) {
        self.encoded.zeroize();
    }
}

#[cfg(test)]
mod tests;
