//! Enhanced acknowledgement of an IEEE 802.15.4-2015 frame, ported from
//! OpenThread `TxFrame::GenerateEnhAck` and the header preparation it uses
//! (`src/core/mac/mac_frame.cpp` at OpenThread
//! `43cc05a9bcf780bd758bad53d897e2d88cf8cb75`, the submodule of ESP-IDF
//! `4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d`; sha256
//! `33f704b1ee9f1026362f06bcd97a19f88e8a42ddfafe3ee9c2e42cdb91db51a1`).
//!
//! ESP-IDF's OpenThread port builds the enhanced ACK in its driver callback
//! (`ot_radio_enh_ack_generator`); this module is the frame part of that
//! callback. The acknowledgement echoes the received sequence number,
//! addresses the received frame's source from no source, carries the PAN ID
//! the received frame names first by source, and mirrors the received
//! security level and key identifier. Its frame counter is the caller's: see
//! [`EnhancedAck::set_frame_counter`](crate::EnhancedAck::set_frame_counter). The MIC is a zero placeholder that the
//! transmit security engine fills.
//!
//! Frames are MAC bytes without the FCS. Every read is bounded by the
//! received bytes; a truncated header is an error where OpenThread would
//! read past its frame.

use crate::mac::frame::{FrameView, MAX_MAC_FRAME_LEN};
use oer_wire::{PrefixLength, Reader, Writer};

const FCF_FRAME_TYPE_ACK: u16 = 0x0002;
const FCF_SECURITY_ENABLED: u16 = 1 << 3;
const FCF_FRAME_PENDING: u16 = 1 << 4;
const FCF_ACK_REQUEST: u16 = 1 << 5;
const FCF_PANID_COMPRESSION: u16 = 1 << 6;
const FCF_SEQUENCE_SUPPRESSION: u16 = 1 << 8;
const FCF_IE_PRESENT: u16 = 1 << 9;
const FCF_DST_ADDR_SHIFT: u16 = 10;
const FCF_VERSION_MASK: u16 = 0x3 << 12;
const FCF_VERSION_2015: u16 = 0x2 << 12;
const FCF_SRC_ADDR_SHIFT: u16 = 14;

const ADDR_NONE: u16 = 0;
const ADDR_SHORT: u16 = 2;
const ADDR_EXT: u16 = 3;

const FCF_SIZE: usize = 2;
const DSN_SIZE: usize = 1;
const PANID_SIZE: usize = 2;
const SHORT_ADDR_SIZE: usize = 2;
const EXT_ADDR_SIZE: usize = 8;
const SECURITY_CONTROL_SIZE: usize = 1;
const FRAME_COUNTER_SIZE: usize = 4;
const MIC32_SIZE: usize = 4;

const SEC_LEVEL_MASK: u8 = 0x07;
const SEC_ENC_MIC32: u8 = 5;
const KEY_ID_MODE_MASK: u8 = 0x18;
const BROADCAST_SHORT: [u8; 2] = [0xff, 0xff];

/// Key identifier mode of an auxiliary security header.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum KeyIdMode {
    /// Mode 0: the key is implicit.
    Implicit,
    /// Mode 1: a one-byte key index.
    Index,
    /// Mode 2: a four-byte key source and a key index.
    Source4,
    /// Mode 3: an eight-byte key source and a key index.
    Source8,
}

impl KeyIdMode {
    const fn from_security_control(control: u8) -> Self {
        match (control & KEY_ID_MODE_MASK) >> 3 {
            0 => Self::Implicit,
            1 => Self::Index,
            2 => Self::Source4,
            _ => Self::Source8,
        }
    }

    const fn key_source_size(self) -> usize {
        match self {
            Self::Implicit | Self::Index => 0,
            Self::Source4 => 4,
            Self::Source8 => 8,
        }
    }
}

/// Why no enhanced ACK was generated for a received frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnhancedAckError {
    /// The frame is not an IEEE 802.15.4-2015 frame.
    NotVersion2015,
    /// The frame requests no acknowledgement.
    NoAckRequest,
    /// The frame suppresses its sequence number, which OpenThread asserts
    /// never happens.
    SequenceSuppressed,
    /// The header is truncated or uses a reserved addressing mode.
    Malformed,
    /// The destination is absent or the broadcast address.
    NotUnicast,
    /// The frame has no source to acknowledge.
    NoSource,
    /// The frame is secured with a level other than ENC-MIC-32.
    UnsupportedSecurityLevel,
    /// The acknowledgement with these header IEs exceeds one MAC frame.
    TooLong,
}

/// The auxiliary security header of a secured enhanced ACK.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EnhancedAckSecurity {
    /// Key identifier mode, mirrored from the received frame.
    pub key_id_mode: KeyIdMode,
    /// Key index, mirrored from the received frame; absent in mode 0.
    pub key_index: Option<u8>,
    frame_counter_offset: usize,
}

/// A generated enhanced ACK: MAC bytes without the FCS.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EnhancedAck {
    bytes: [u8; MAX_MAC_FRAME_LEN],
    len: PrefixLength<MAX_MAC_FRAME_LEN>,
    security: Option<EnhancedAckSecurity>,
}

impl EnhancedAck {
    /// The MAC bytes, including the MIC placeholder of a secured ACK.
    pub fn bytes(&self) -> &[u8] {
        self.len.borrow(&self.bytes)
    }

    /// The security header, when the received frame was secured.
    pub const fn security(&self) -> Option<EnhancedAckSecurity> {
        self.security
    }

    /// Write the frame counter of a secured ACK (`Frame::SetFrameCounter`).
    /// An unsecured ACK has no frame counter and is left unchanged.
    pub fn set_frame_counter(&mut self, frame_counter: u32) {
        if let Some(security) = self.security {
            let offset = security.frame_counter_offset;
            // Only the generator creates this offset, after writing the whole
            // auxiliary header. Its four counter octets are admitted by that write.
            for (destination, source) in self
                .bytes
                .iter_mut()
                .skip(offset)
                .zip(frame_counter.to_le_bytes())
            {
                *destination = source;
            }
        }
    }
}

/// The received frame's header, read with OpenThread's 2015 rules.
struct Received<'frame> {
    bytes: &'frame [u8],
    fcf: u16,
}

impl<'frame> Received<'frame> {
    const fn dst_mode(&self) -> u16 {
        (self.fcf >> FCF_DST_ADDR_SHIFT) & 0x3
    }

    const fn src_mode(&self) -> u16 {
        (self.fcf >> FCF_SRC_ADDR_SHIFT) & 0x3
    }

    const fn compressed(&self) -> bool {
        self.fcf & FCF_PANID_COMPRESSION != 0
    }

    /// `Frame::IsDstPanIdPresent` of a 2015 frame.
    const fn dst_pan_present(&self) -> bool {
        !matches!(
            (self.dst_mode(), self.src_mode(), self.compressed()),
            (ADDR_NONE, ADDR_NONE, false)
                | (ADDR_SHORT | ADDR_EXT, ADDR_NONE, true)
                | (ADDR_NONE, ADDR_SHORT | ADDR_EXT, _)
                | (ADDR_EXT, ADDR_EXT, true)
        )
    }

    /// `Frame::IsSrcPanIdPresent` of a 2015 frame: never with two extended
    /// addresses, otherwise with a source and no compression.
    const fn src_pan_present(&self) -> bool {
        if self.dst_mode() == ADDR_EXT && self.src_mode() == ADDR_EXT {
            return false;
        }
        self.src_mode() != ADDR_NONE && !self.compressed()
    }

    fn field(&self, offset: usize, len: usize) -> Result<&'frame [u8], EnhancedAckError> {
        let end = offset.checked_add(len).ok_or(EnhancedAckError::Malformed)?;
        self.bytes
            .get(offset..end)
            .ok_or(EnhancedAckError::Malformed)
    }

    const fn address_size(mode: u16) -> usize {
        match mode {
            ADDR_SHORT => SHORT_ADDR_SIZE,
            ADDR_EXT => EXT_ADDR_SIZE,
            _ => 0,
        }
    }

    const fn dst_pan_offset() -> usize {
        FCF_SIZE + DSN_SIZE
    }

    fn dst_addr_offset(&self) -> usize {
        if self.dst_pan_present() {
            const { FCF_SIZE + DSN_SIZE + PANID_SIZE }
        } else {
            Self::dst_pan_offset()
        }
    }

    fn src_pan_offset(&self) -> Result<usize, EnhancedAckError> {
        self.dst_addr_offset()
            .checked_add(Self::address_size(self.dst_mode()))
            .ok_or(EnhancedAckError::Malformed)
    }

    fn src_addr_offset(&self) -> Result<usize, EnhancedAckError> {
        self.src_pan_offset()?
            .checked_add(if self.src_pan_present() {
                PANID_SIZE
            } else {
                0
            })
            .ok_or(EnhancedAckError::Malformed)
    }

    fn security_offset(&self) -> Result<usize, EnhancedAckError> {
        self.src_addr_offset()?
            .checked_add(Self::address_size(self.src_mode()))
            .ok_or(EnhancedAckError::Malformed)
    }

    fn byte(&self, offset: usize) -> Result<u8, EnhancedAckError> {
        Reader::new(self.field(offset, 1)?)
            .u8()
            .map_err(|_| EnhancedAckError::Malformed)
    }
}

/// Generate the enhanced ACK of `received` (`TxFrame::GenerateEnhAck`).
///
/// `frame_pending` is the ACK's frame-pending bit; `header_ies` are header
/// IEs placed after the security header, as the port places its CSL and
/// probing IEs.
///
/// # Errors
///
/// The received frame cannot be acknowledged this way, or the ACK does not
/// fit one MAC frame.
pub fn generate_enhanced_ack(
    received: FrameView<'_>,
    frame_pending: bool,
    header_ies: &[u8],
) -> Result<EnhancedAck, EnhancedAckError> {
    let bytes = received.bytes();
    let fcf = match bytes {
        [low, high, ..] => u16::from_le_bytes([*low, *high]),
        _ => return Err(EnhancedAckError::Malformed),
    };
    let rx = Received { bytes, fcf };

    if fcf & FCF_VERSION_MASK != FCF_VERSION_2015 {
        return Err(EnhancedAckError::NotVersion2015);
    }
    if fcf & FCF_ACK_REQUEST == 0 {
        return Err(EnhancedAckError::NoAckRequest);
    }
    if fcf & FCF_SEQUENCE_SUPPRESSION != 0 {
        return Err(EnhancedAckError::SequenceSuppressed);
    }
    let sequence = rx.byte(FCF_SIZE)?;

    // A valid unicast destination; the ACK itself has no source.
    let destination = match rx.dst_mode() {
        ADDR_SHORT | ADDR_EXT => {
            rx.field(rx.dst_addr_offset(), Received::address_size(rx.dst_mode()))?
        }
        _ => return Err(EnhancedAckError::NotUnicast),
    };
    if destination == BROADCAST_SHORT {
        return Err(EnhancedAckError::NotUnicast);
    }

    // The received source becomes the ACK's destination.
    let ack_dst_mode = match rx.src_mode() {
        ADDR_NONE => return Err(EnhancedAckError::NoSource),
        ADDR_SHORT | ADDR_EXT => rx.src_mode(),
        _ => return Err(EnhancedAckError::Malformed),
    };
    let ack_destination = rx.field(rx.src_addr_offset()?, Received::address_size(ack_dst_mode))?;

    let security = if fcf & FCF_SECURITY_ENABLED != 0 {
        let offset = rx.security_offset()?;
        let control = rx.byte(offset)?;
        if control & SEC_LEVEL_MASK != SEC_ENC_MIC32 {
            return Err(EnhancedAckError::UnsupportedSecurityLevel);
        }
        let key_id_mode = KeyIdMode::from_security_control(control);
        let key_index = match key_id_mode {
            KeyIdMode::Implicit => None,
            _ => {
                let offset = offset
                    .checked_add(SECURITY_CONTROL_SIZE)
                    .and_then(|end| end.checked_add(FRAME_COUNTER_SIZE))
                    .and_then(|end| end.checked_add(key_id_mode.key_source_size()))
                    .ok_or(EnhancedAckError::Malformed)?;
                Some(rx.byte(offset)?)
            }
        };
        Some((
            control & (SEC_LEVEL_MASK | KEY_ID_MODE_MASK),
            key_id_mode,
            key_index,
        ))
    } else {
        None
    };

    let pan = if rx.src_pan_present() {
        Some(rx.field(rx.src_pan_offset()?, PANID_SIZE)?)
    } else if rx.dst_pan_present() {
        Some(rx.field(Received::dst_pan_offset(), PANID_SIZE)?)
    } else {
        None
    };

    // `BuildInfo::PrepareHeadersIn` of an ACK with a destination and no
    // source: rows 3 and 4 of the 2015 PAN ID table.
    let mut ack_fcf = FCF_FRAME_TYPE_ACK | FCF_VERSION_2015 | (ack_dst_mode << FCF_DST_ADDR_SHIFT);
    if security.is_some() {
        ack_fcf |= FCF_SECURITY_ENABLED;
    }
    if pan.is_none() {
        ack_fcf |= FCF_PANID_COMPRESSION;
    }
    if frame_pending {
        ack_fcf |= FCF_FRAME_PENDING;
    }
    if !header_ies.is_empty() {
        ack_fcf |= FCF_IE_PRESENT;
    }

    // The cursor admits complete fields before each write. A failed generation
    // owns only local bytes, so no partial ACK escapes to a caller.
    let mut bytes = [0; MAX_MAC_FRAME_LEN];
    let mut writer = Writer::new(&mut bytes);
    writer
        .u16_le(ack_fcf)
        .map_err(|_| EnhancedAckError::TooLong)?;
    writer.u8(sequence).map_err(|_| EnhancedAckError::TooLong)?;
    if let Some(pan) = pan {
        writer.write(pan).map_err(|_| EnhancedAckError::TooLong)?;
    }
    writer
        .write(ack_destination)
        .map_err(|_| EnhancedAckError::TooLong)?;
    let security = if let Some((control, key_id_mode, key_index)) = security {
        writer.u8(control).map_err(|_| EnhancedAckError::TooLong)?;
        let frame_counter_offset = writer.written();
        writer
            .write(&[0; FRAME_COUNTER_SIZE])
            .map_err(|_| EnhancedAckError::TooLong)?;
        match key_id_mode {
            KeyIdMode::Source4 => writer
                .write(&[0; 4])
                .map_err(|_| EnhancedAckError::TooLong)?,
            KeyIdMode::Source8 => writer
                .write(&[0; 8])
                .map_err(|_| EnhancedAckError::TooLong)?,
            KeyIdMode::Implicit | KeyIdMode::Index => {}
        }
        if let Some(index) = key_index {
            writer.u8(index).map_err(|_| EnhancedAckError::TooLong)?;
        }
        Some(EnhancedAckSecurity {
            key_id_mode,
            key_index,
            frame_counter_offset,
        })
    } else {
        None
    };
    writer
        .write(header_ies)
        .map_err(|_| EnhancedAckError::TooLong)?;
    if security.is_some() {
        writer
            .write(&[0; MIC32_SIZE])
            .map_err(|_| EnhancedAckError::TooLong)?;
    }
    let len = PrefixLength::new(writer.written()).ok_or(EnhancedAckError::TooLong)?;
    Ok(EnhancedAck {
        bytes,
        len,
        security,
    })
}

#[cfg(test)]
mod tests;
