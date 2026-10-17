//! BIP-CMAC-128: integrity of group-addressed robust management frames.
//!
//! A group-addressed Deauthentication, Disassociation or robust Action frame
//! of an association that protects its management frames ends with a
//! Management MIC element. Its MIC is AES-128-CMAC, truncated to 64 bits,
//! over the frame's additional authentication data and its body with the MIC
//! field zeroed, and its IGTK packet number must exceed every earlier one.
//!
//! SOURCE: IEEE 802.11-2016 12.5.4 (BIP), 9.4.2.55 (Management MIC element).

use aes::Aes128;
use cmac::{Cmac, Mac};
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
        if frame.len() < MANAGEMENT_HEADER_LEN + MANAGEMENT_MIC_ELEMENT_LEN {
            return Err(BipError::Truncated);
        }
        let mme_start = frame.len() - MANAGEMENT_MIC_ELEMENT_LEN;
        let mme = &frame[mme_start..];
        if mme_start < MANAGEMENT_HEADER_LEN
            || mme[0] != MANAGEMENT_MIC_ELEMENT_ID
            || usize::from(mme[1]) != MANAGEMENT_MIC_BODY_LEN
        {
            return Err(BipError::MissingManagementMic);
        }
        if u16::from_le_bytes([mme[2], mme[3]]) != u16::from(self.key_id) {
            return Err(BipError::UnknownKeyId);
        }
        let mut ipn = [0; RSN_IPN_LEN];
        ipn.copy_from_slice(&mme[4..4 + RSN_IPN_LEN]);
        let received = packet_number(ipn);
        if received <= self.last_packet_number {
            return Err(BipError::Replay);
        }
        let mut mac =
            Cmac::<Aes128>::new_from_slice(&self.key).expect("a 16-byte IGTK is an AES-128 key");
        mac.update(&[frame[0], frame[1] & !AAD_MASKED_FLAGS]);
        mac.update(&frame[4..MANAGEMENT_HEADER_LEN - 2]);
        mac.update(&frame[MANAGEMENT_HEADER_LEN..frame.len() - BIP_MIC_LEN]);
        mac.update(&[0; BIP_MIC_LEN]);
        mac.verify_truncated_left(&mme[4 + RSN_IPN_LEN..])
            .map_err(|_| BipError::InvalidMic)?;
        self.last_packet_number = received;
        Ok(())
    }
}

/// The transmit state of one IGTK: its key and the next IPN.
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
            next_packet_number: packet_number(igtk.packet_number()) + 1,
        }
    }

    /// The last allocated IPN, or the initial delivered IPN before any
    /// transmission. A new peer must receive this frontier with its IGTK.
    pub fn packet_number(&self) -> [u8; RSN_IPN_LEN] {
        let bytes = (self.next_packet_number - 1).to_le_bytes();
        let mut ipn = [0; RSN_IPN_LEN];
        ipn.copy_from_slice(&bytes[..RSN_IPN_LEN]);
        ipn
    }

    /// Protect one group-addressed robust management frame in place: `frame`
    /// holds the header and body, and `MANAGEMENT_MIC_ELEMENT_LEN` spare
    /// octets after them receive the Management MIC element. Returns the
    /// protected frame's length, or `None` when the spare octets are missing
    /// or the 48-bit IPN space is exhausted. Failure consumes no IPN.
    pub fn protect(&mut self, frame: &mut [u8], length: usize) -> Option<usize> {
        let protected = length.checked_add(MANAGEMENT_MIC_ELEMENT_LEN)?;
        if length < MANAGEMENT_HEADER_LEN
            || frame.len() < protected
            || self.next_packet_number >= (1_u64 << 48)
        {
            return None;
        }
        let ipn = self.next_packet_number.to_le_bytes();
        self.next_packet_number += 1;
        let mme = &mut frame[length..protected];
        mme[0] = MANAGEMENT_MIC_ELEMENT_ID;
        mme[1] = MANAGEMENT_MIC_BODY_LEN as u8;
        mme[2..4].copy_from_slice(&u16::from(self.key_id).to_le_bytes());
        mme[4..4 + RSN_IPN_LEN].copy_from_slice(&ipn[..RSN_IPN_LEN]);
        mme[4 + RSN_IPN_LEN..].fill(0);
        let mut mac =
            Cmac::<Aes128>::new_from_slice(&self.key).expect("a 16-byte IGTK is an AES-128 key");
        mac.update(&[frame[0], frame[1] & !AAD_MASKED_FLAGS]);
        mac.update(&frame[4..MANAGEMENT_HEADER_LEN - 2]);
        mac.update(&frame[MANAGEMENT_HEADER_LEN..protected]);
        let tag = mac.finalize().into_bytes();
        frame[protected - BIP_MIC_LEN..protected].copy_from_slice(&tag[..BIP_MIC_LEN]);
        Some(protected)
    }
}

/// A little-endian six-byte IPN as an integer.
fn packet_number(bytes: [u8; RSN_IPN_LEN]) -> u64 {
    let mut value = [0; 8];
    value[..RSN_IPN_LEN].copy_from_slice(&bytes);
    u64::from_le_bytes(value)
}

#[cfg(test)]
mod tests;
