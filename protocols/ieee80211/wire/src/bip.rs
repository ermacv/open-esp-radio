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

/// The IPN six little-endian octets carry.
pub const fn ipn_from_bytes(bytes: [u8; IPN_LEN]) -> oer_portable_wire::U64<48> {
    IpnField::from_bytes(bytes).value()
}

/// The six little-endian octets of an IPN.
pub const fn ipn_to_bytes(value: oer_portable_wire::U64<48>) -> [u8; IPN_LEN] {
    IpnField::from_bytes([0; IPN_LEN])
        .with_value(value)
        .to_bytes()
}

oer_portable_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.54 (Management MIC element:
    /// a six-octet IPN).
    pub struct IpnField[6] (little) {
        pub value, with_value: (u64) = 0..48;
    }
}
oer_portable_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.54 (Management MIC element:
    /// Key ID, two octets).
    pub struct KeyIdField[2] (little) {
        pub key_id, with_key_id: {u16} = 0..16;
    }
}

/// The identifier of an IGTK, 4 or 5.
///
/// SOURCE: IEEE Std 802.11-2020, 12.7.2 (EAPOL-Key frames: the IGTK KDE's
/// KeyID) and 9.4.2.54 (Management MIC element).
///
/// A number is no identifier until it is checked:
///
/// ```compile_fail,E0308
/// use oer_protocols_ieee80211_wire::bip::IgtkKeyId;
/// let key_id: IgtkKeyId = 4;
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IgtkKeyId {
    Four,
    Five,
}

impl IgtkKeyId {
    /// The identifier `field` names; `None` for every other value.
    pub const fn from_field(field: KeyIdField) -> Option<Self> {
        match field.key_id() {
            4 => Some(Self::Four),
            5 => Some(Self::Five),
            _ => None,
        }
    }

    /// The Key ID field that names it.
    pub const fn field(self) -> KeyIdField {
        KeyIdField::from_bytes([0; 2]).with_key_id(match self {
            Self::Four => 4,
            Self::Five => 5,
        })
    }
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
}

/// `header`, when it begins a protocol-version-0 management frame.
fn management(
    header: &[u8; MANAGEMENT_HEADER_LEN],
) -> Result<&[u8; MANAGEMENT_HEADER_LEN], BipFormatError> {
    let [f0, f1, ..] = *header;
    FrameControl::new(crate::frame::FrameControlFields::from_bytes([f0, f1]))
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
    key_id: KeyIdField,
    packet_number: oer_portable_wire::U64<48>,
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
            key_id: KeyIdField::from_bytes([key_low, key_high]),
            packet_number: ipn_from_bytes(ipn),
            mic,
        })
    }

    /// The element's Key ID field.
    pub const fn key_id(&self) -> KeyIdField {
        self.key_id
    }

    /// The element's IPN.
    pub const fn packet_number(&self) -> oer_portable_wire::U64<48> {
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
        key_id: IgtkKeyId,
        packet_number: oer_portable_wire::U64<48>,
    ) -> Result<Self, BipFormatError> {
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
        let [k0, k1] = key_id.field().to_bytes();
        let [i0, i1, i2, i3, i4, i5] = ipn_to_bytes(packet_number);
        *element = [
            MANAGEMENT_MIC_ELEMENT_ID,
            const { crate::encoding::octet::<MANAGEMENT_MIC_BODY_LEN>() },
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
