//! The Management MIC element of a BIP-protected group management frame,
//! and the octets its MIC is computed over.
//!
//! SOURCE: IEEE Std 802.11-2020, 9.4.2.54 (Management MIC element: Key ID,
//! IPN, MIC; an 8-octet MIC for BIP-CMAC-128) and 12.5.4.4 (BIP AAD: Frame
//! Control with Retry, Power Management and More Data masked, then the three
//! addresses; the MIC covers AAD, the body and the element with its MIC
//! field zero).
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

use crate::frame::{FrameControl, FrameControlFields};

/// Management MIC element identifier.
pub const MANAGEMENT_MIC_ELEMENT_ID: u8 = 76;
/// Length of a BIP-CMAC-128 MIC.
pub const BIP_MIC_LEN: usize = 8;
/// Length of the IGTK packet number.
pub const IPN_LEN: usize = 6;
/// The Management MIC element body of BIP-CMAC-128: Key ID, IPN and MIC.
const MANAGEMENT_MIC_BODY_LEN: usize = 2 + IPN_LEN + BIP_MIC_LEN;
/// The Management MIC element of BIP-CMAC-128.
pub const MANAGEMENT_MIC_ELEMENT_LEN: usize = 2 + MANAGEMENT_MIC_BODY_LEN;
/// The largest IPN: six octets.
pub const IPN_MAX: u64 = (1 << 48) - 1;

/// The IPN six little-endian octets carry.
pub const fn ipn_from_bytes(bytes: [u8; IPN_LEN]) -> u64 {
    let [b0, b1, b2, b3, b4, b5] = bytes;
    u64::from_le_bytes([b0, b1, b2, b3, b4, b5, 0, 0])
}

/// The six little-endian octets of an IPN; its bits above 48 are dropped.
pub const fn ipn_to_bytes(value: u64) -> [u8; IPN_LEN] {
    let [b0, b1, b2, b3, b4, b5, _, _] = value.to_le_bytes();
    [b0, b1, b2, b3, b4, b5]
}
const MANAGEMENT_HEADER_LEN: usize = 24;
const AAD_LEN: usize = 2 + 3 * 6;
const ZERO_MIC: [u8; BIP_MIC_LEN] = [0; BIP_MIC_LEN];

/// A frame that cannot carry a BIP-CMAC-128 Management MIC element.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BipFormatError {
    /// Shorter than a management header and the element.
    Truncated,
    /// The header is not that of a protocol-version-0 management frame.
    NotManagement,
    /// The frame does not end with a BIP-CMAC-128 Management MIC element.
    MissingManagementMic,
    /// An IPN above six octets.
    PacketNumberOutOfRange,
}

/// `header`, when it begins a protocol-version-0 management frame.
fn management(
    header: &[u8; MANAGEMENT_HEADER_LEN],
) -> Result<&[u8; MANAGEMENT_HEADER_LEN], BipFormatError> {
    let [f0, f1, ..] = *header;
    FrameControl::new(u16::from_le_bytes([f0, f1]))
        .filter(|control| control.is_management_frame())
        .map(|_| header)
        .ok_or(BipFormatError::NotManagement)
}

/// The BIP additional authentication data of a management header.
fn aad(header: &[u8; MANAGEMENT_HEADER_LEN]) -> [u8; AAD_LEN] {
    let [f0, f1, _, _, addresses @ .., _, _] = *header;
    let [m0, m1] = FrameControlFields::from_bytes([f0, f1])
        .with_retry(false)
        .with_power_management(false)
        .with_more_data(false)
        .to_bytes();
    let mut bytes = [0; AAD_LEN];
    for (target, source) in bytes.iter_mut().zip([m0, m1].iter().chain(&addresses)) {
        *target = *source;
    }
    bytes
}

/// A received frame ending in its Management MIC element.
#[derive(Clone, Copy, Debug)]
pub struct BipFrame<'a> {
    aad: [u8; AAD_LEN],
    body: &'a [u8],
    mme_before_mic: &'a [u8],
    key_id: u16,
    packet_number: u64,
    mic: &'a [u8; BIP_MIC_LEN],
}

impl<'a> BipFrame<'a> {
    /// The management header, body and Management MIC element of `frame`.
    pub fn parse(frame: &'a [u8]) -> Result<Self, BipFormatError> {
        let (header, rest) = frame
            .split_first_chunk::<MANAGEMENT_HEADER_LEN>()
            .ok_or(BipFormatError::Truncated)?;
        let header = management(header)?;
        let (body, mme) = rest
            .split_last_chunk::<MANAGEMENT_MIC_ELEMENT_LEN>()
            .ok_or(BipFormatError::Truncated)?;
        let (before_mic, mic) = mme
            .split_last_chunk::<BIP_MIC_LEN>()
            .ok_or(BipFormatError::Truncated)?;
        let (&[id, length, key_low, key_high], ipn) = before_mic
            .split_first_chunk::<4>()
            .ok_or(BipFormatError::Truncated)?;
        if id != MANAGEMENT_MIC_ELEMENT_ID || usize::from(length) != MANAGEMENT_MIC_BODY_LEN {
            return Err(BipFormatError::MissingManagementMic);
        }
        let &ipn = <&[u8; IPN_LEN]>::try_from(ipn).map_err(|_| BipFormatError::Truncated)?;
        Ok(Self {
            aad: aad(header),
            body,
            mme_before_mic: before_mic,
            key_id: u16::from_le_bytes([key_low, key_high]),
            packet_number: ipn_from_bytes(ipn),
            mic,
        })
    }

    /// The element's Key ID field.
    pub const fn key_id(&self) -> u16 {
        self.key_id
    }

    /// The element's IPN.
    pub const fn packet_number(&self) -> u64 {
        self.packet_number
    }

    pub const fn mic(&self) -> &'a [u8; BIP_MIC_LEN] {
        self.mic
    }

    /// The octets the MIC is computed over, in order.
    pub fn mic_input(&self) -> [&[u8]; 4] {
        [&self.aad, self.body, self.mme_before_mic, &ZERO_MIC]
    }
}

/// A frame of `length` octets in `buffer` with its Management MIC element
/// appended, the MIC still zero.
pub struct BipProtection<'a> {
    aad: [u8; AAD_LEN],
    body: &'a [u8],
    /// The element before its MIC.
    element: &'a [u8],
    mic: &'a mut [u8; BIP_MIC_LEN],
    length: usize,
}

impl<'a> BipProtection<'a> {
    /// Append the element naming `key_id` and `packet_number` after the
    /// `length` octets of a management frame in `buffer`; a refusal,
    /// including a header of another frame type, writes nothing.
    pub fn append(
        buffer: &'a mut [u8],
        length: usize,
        key_id: u16,
        packet_number: u64,
    ) -> Result<Self, BipFormatError> {
        if packet_number > IPN_MAX {
            return Err(BipFormatError::PacketNumberOutOfRange);
        }
        let protected = length
            .checked_add(MANAGEMENT_MIC_ELEMENT_LEN)
            .ok_or(BipFormatError::Truncated)?;
        if length < MANAGEMENT_HEADER_LEN {
            return Err(BipFormatError::Truncated);
        }
        let (frame, _) = buffer
            .split_at_mut_checked(protected)
            .ok_or(BipFormatError::Truncated)?;
        let (header, rest) = frame
            .split_first_chunk_mut::<MANAGEMENT_HEADER_LEN>()
            .ok_or(BipFormatError::Truncated)?;
        let aad = aad(management(header)?);
        let (covered, mic) = rest
            .split_last_chunk_mut::<BIP_MIC_LEN>()
            .ok_or(BipFormatError::Truncated)?;
        let (body, element) = covered
            .split_last_chunk_mut::<{ MANAGEMENT_MIC_ELEMENT_LEN - BIP_MIC_LEN }>()
            .ok_or(BipFormatError::Truncated)?;
        let [k0, k1] = key_id.to_le_bytes();
        let [i0, i1, i2, i3, i4, i5] = ipn_to_bytes(packet_number);
        *element = [
            MANAGEMENT_MIC_ELEMENT_ID,
            MANAGEMENT_MIC_BODY_LEN as u8,
            k0,
            k1,
            i0,
            i1,
            i2,
            i3,
            i4,
            i5,
        ];
        *mic = ZERO_MIC;
        Ok(Self {
            aad,
            body,
            element,
            mic,
            length: protected,
        })
    }

    /// The octets the MIC is computed over, in order.
    pub fn mic_input(&self) -> [&[u8]; 4] {
        [&self.aad, self.body, self.element, &ZERO_MIC]
    }

    /// Write the MIC; returns the protected frame's length.
    pub fn finish(self, mic: &[u8; BIP_MIC_LEN]) -> usize {
        *self.mic = *mic;
        self.length
    }
}

#[cfg(test)]
mod tests;
