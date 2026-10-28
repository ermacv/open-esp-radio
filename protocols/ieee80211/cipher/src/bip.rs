//! Stateless BIP-CMAC-128 management-frame integrity transforms.

#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]

use aes::Aes128;
use cmac::{Cmac, Mac, digest::KeyInit};

const RSN_IGTK_LEN: usize = 16;
const RSN_IPN_LEN: usize = 6;
/// Management MIC element identifier.
pub const MANAGEMENT_MIC_ELEMENT_ID: u8 = 76;
/// Management MIC element body of BIP-CMAC-128: key id, IPN and MIC.
const MANAGEMENT_MIC_BODY_LEN: usize = 2 + RSN_IPN_LEN + BIP_MIC_LEN;
/// Management MIC element of BIP-CMAC-128, closing a protected group frame.
pub const MANAGEMENT_MIC_ELEMENT_LEN: usize = 2 + MANAGEMENT_MIC_BODY_LEN;
const BIP_MIC_LEN: usize = 8;
/// Frame Control flags BIP leaves out of its additional authentication data:
/// Retry, Power Management and More Data.
const AAD_MASKED_FLAGS: u8 = 0x08 | 0x10 | 0x20;
const MANAGEMENT_HEADER_LEN: usize = 24;

/// Why a group-addressed management frame failed BIP.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BipError {
    /// The frame is shorter than a management header and its MIC element.
    Truncated,
    /// The frame does not end with a BIP-CMAC-128 Management MIC element.
    MissingManagementMic,
    /// The MIC element names another IGTK.
    UnknownKeyId,
    /// The MIC does not verify.
    InvalidMic,
}

/// A parsed BIP frame; replay admission remains with the caller.
pub struct BipFrame<'a> {
    header: &'a [u8; MANAGEMENT_HEADER_LEN],
    body: &'a [u8],
    mme_before_mic: &'a [u8],
    mic: &'a [u8],
    packet_number: u64,
}

impl<'a> BipFrame<'a> {
    pub fn parse(frame: &'a [u8], key_id: u8) -> Result<Self, BipError> {
        let Some((header, rest)) = frame.split_first_chunk::<MANAGEMENT_HEADER_LEN>() else {
            return Err(BipError::Truncated);
        };
        let Some((body, mme)) = rest.split_last_chunk::<MANAGEMENT_MIC_ELEMENT_LEN>() else {
            return Err(BipError::Truncated);
        };
        let Some((&[id, length, key_low, key_high], fields)) = mme.split_first_chunk::<4>() else {
            return Err(BipError::Truncated);
        };
        if id != MANAGEMENT_MIC_ELEMENT_ID || usize::from(length) != MANAGEMENT_MIC_BODY_LEN {
            return Err(BipError::MissingManagementMic);
        }
        if u16::from_le_bytes([key_low, key_high]) != u16::from(key_id) {
            return Err(BipError::UnknownKeyId);
        }
        let Some((&ipn, mic)) = fields.split_first_chunk::<RSN_IPN_LEN>() else {
            return Err(BipError::Truncated);
        };
        let Some(mme_before_mic) = mme.get(..4 + RSN_IPN_LEN) else {
            return Err(BipError::Truncated);
        };
        let received = packet_number(ipn);
        Ok(Self {
            header,
            body,
            mme_before_mic,
            mic,
            packet_number: received,
        })
    }

    pub const fn packet_number(&self) -> u64 {
        self.packet_number
    }

    pub fn verify(self, key: &[u8; RSN_IGTK_LEN]) -> Result<(), BipError> {
        let Self {
            header,
            body,
            mme_before_mic,
            mic,
            ..
        } = self;
        let mut mac = cmac(key);
        update_aad(&mut mac, header);
        mac.update(body);
        mac.update(mme_before_mic);
        mac.update(&[0; BIP_MIC_LEN]);
        mac.verify_truncated_left(mic)
            .map_err(|_| BipError::InvalidMic)?;
        Ok(())
    }
}

/// Protect a frame with the caller's IPN, without allocating or advancing it.
pub fn protect(
    key_id: u8,
    key: &[u8; RSN_IGTK_LEN],
    packet_number: u64,
    frame: &mut [u8],
    length: usize,
) -> Option<usize> {
    let protected = length.checked_add(MANAGEMENT_MIC_ELEMENT_LEN)?;
    if length < MANAGEMENT_HEADER_LEN || packet_number >= (1_u64 << 48) {
        return None;
    }
    let (frame, _) = frame.split_at_mut_checked(protected)?;
    let (header, rest) = frame.split_first_chunk_mut::<MANAGEMENT_HEADER_LEN>()?;
    let (body, mme) = rest.split_last_chunk_mut::<MANAGEMENT_MIC_ELEMENT_LEN>()?;
    let [k0, k1] = u16::from(key_id).to_le_bytes();
    let [i0, i1, i2, i3, i4, i5] = ipn_bytes(packet_number);
    *mme = [
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
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    ];
    let mut mac = cmac(key);
    update_aad(&mut mac, header);
    mac.update(body);
    mac.update(mme.as_slice());
    let tag = mac.finalize().into_bytes();
    let (_, mic) = mme.split_last_chunk_mut::<BIP_MIC_LEN>()?;
    for (out, byte) in mic.iter_mut().zip(tag) {
        *out = byte;
    }
    Some(protected)
}

/// AES-128-CMAC over a 16-byte IGTK.
fn cmac(key: &[u8; RSN_IGTK_LEN]) -> Cmac<Aes128> {
    <Cmac<Aes128> as KeyInit>::new(&(*key).into())
}

/// The BIP additional authentication data of a management header: frame
/// control with Retry, Power Management and More Data masked, and the
/// three addresses; duration and sequence control are left out.
fn update_aad(mac: &mut Cmac<Aes128>, header: &[u8; MANAGEMENT_HEADER_LEN]) {
    let [frame_control, flags, _, _, addresses @ .., _, _] = header;
    mac.update(&[*frame_control, flags & !AAD_MASKED_FLAGS]);
    mac.update(addresses);
}

fn ipn_bytes(value: u64) -> [u8; RSN_IPN_LEN] {
    let [b0, b1, b2, b3, b4, b5, _, _] = value.to_le_bytes();
    [b0, b1, b2, b3, b4, b5]
}

fn packet_number(bytes: [u8; RSN_IPN_LEN]) -> u64 {
    let [b0, b1, b2, b3, b4, b5] = bytes;
    u64::from_le_bytes([b0, b1, b2, b3, b4, b5, 0, 0])
}
