//! BIP-CMAC-128: integrity of group-addressed robust management frames.
//!
//! A group-addressed Deauthentication, Disassociation or robust Action frame
//! of an association that protects its management frames ends with a
//! Management MIC element. Its MIC is AES-128-CMAC, truncated to 64 bits,
//! over the frame's additional authentication data and its body with the MIC
//! field zeroed, and its IGTK packet number must exceed every earlier one.
//!
//! SOURCE: IEEE 802.11-2016 12.5.4 (BIP), 9.4.2.55 (Management MIC element).
//!
//! Received frames reach [`BipReceiver::verify`]; nothing here panics on them.
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
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::frames::{RSN_IGTK_LEN, RSN_IPN_LEN, RsnIgtk};

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
    /// The IPN does not exceed the last accepted one.
    Replay,
    /// The MIC does not verify.
    InvalidMic,
}

/// The receive state of one IGTK: its key and the last accepted IPN.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct BipReceiver {
    key_id: u8,
    key: [u8; RSN_IGTK_LEN],
    /// The last accepted IPN, as a 48-bit integer.
    last_packet_number: u64,
}

impl BipReceiver {
    /// Start with an IGTK and the IPN its KDE delivered.
    pub fn new(igtk: &RsnIgtk) -> Self {
        Self {
            key_id: igtk.key_id(),
            key: *igtk.key(),
            last_packet_number: packet_number(igtk.packet_number()),
        }
    }

    pub const fn key_id(&self) -> u8 {
        self.key_id
    }

    /// Follow a group rekey: a new IGTK replaces this one, while a repeat of
    /// this IGTK keeps the higher of its replay states, so a retransmitted
    /// key never reopens packet numbers already accepted.
    pub fn rekey(&mut self, igtk: &RsnIgtk) {
        let delivered = packet_number(igtk.packet_number());
        if igtk.key_id() == self.key_id && *igtk.key() == self.key {
            self.last_packet_number = self.last_packet_number.max(delivered);
        } else {
            *self = Self::new(igtk);
        }
    }

    /// Verify one received group-addressed robust management frame, from its
    /// Frame Control field to the end of its body, and advance the replay
    /// state when it verifies.
    pub fn verify(&mut self, frame: &[u8]) -> Result<(), BipError> {
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
        if u16::from_le_bytes([key_low, key_high]) != u16::from(self.key_id) {
            return Err(BipError::UnknownKeyId);
        }
        let Some((&ipn, mic)) = fields.split_first_chunk::<RSN_IPN_LEN>() else {
            return Err(BipError::Truncated);
        };
        let Some(mme_before_mic) = mme.get(..4 + RSN_IPN_LEN) else {
            return Err(BipError::Truncated);
        };
        let received = packet_number(ipn);
        if received <= self.last_packet_number {
            return Err(BipError::Replay);
        }
        let mut mac = cmac(&self.key);
        update_aad(&mut mac, header);
        mac.update(body);
        mac.update(mme_before_mic);
        mac.update(&[0; BIP_MIC_LEN]);
        mac.verify_truncated_left(mic)
            .map_err(|_| BipError::InvalidMic)?;
        self.last_packet_number = received;
        Ok(())
    }
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

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct BipTransmitter {
    key_id: u8,
    key: [u8; RSN_IGTK_LEN],
    /// The IPN of the next protected frame, as a 48-bit integer.
    next_packet_number: u64,
}

impl BipTransmitter {
    /// Start with an IGTK after the IPN its KDE announces: a receiver accepts
    /// only IPNs beyond it.
    pub fn new(igtk: &RsnIgtk) -> Self {
        Self {
            key_id: igtk.key_id(),
            key: *igtk.key(),
            // A 48-bit packet number plus one fits.
            next_packet_number: packet_number(igtk.packet_number()).wrapping_add(1),
        }
    }

    pub fn packet_number(&self) -> [u8; RSN_IPN_LEN] {
        ipn_bytes(self.next_packet_number.wrapping_sub(1))
    }

    pub fn protect(&mut self, frame: &mut [u8], length: usize) -> Option<usize> {
        let protected = length.checked_add(MANAGEMENT_MIC_ELEMENT_LEN)?;
        if length < MANAGEMENT_HEADER_LEN || self.next_packet_number >= (1_u64 << 48) {
            return None;
        }
        let (frame, _) = frame.split_at_mut_checked(protected)?;
        let (header, rest) = frame.split_first_chunk_mut::<MANAGEMENT_HEADER_LEN>()?;
        let (body, mme) = rest.split_last_chunk_mut::<MANAGEMENT_MIC_ELEMENT_LEN>()?;
        let [k0, k1] = u16::from(self.key_id).to_le_bytes();
        let [i0, i1, i2, i3, i4, i5] = ipn_bytes(self.next_packet_number);
        self.next_packet_number = self.next_packet_number.wrapping_add(1);
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
        let mut mac = cmac(&self.key);
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
}

fn ipn_bytes(value: u64) -> [u8; RSN_IPN_LEN] {
    let [b0, b1, b2, b3, b4, b5, _, _] = value.to_le_bytes();
    [b0, b1, b2, b3, b4, b5]
}

fn packet_number(bytes: [u8; RSN_IPN_LEN]) -> u64 {
    let [b0, b1, b2, b3, b4, b5] = bytes;
    u64::from_le_bytes([b0, b1, b2, b3, b4, b5, 0, 0])
}

#[cfg(test)]
mod tests;
