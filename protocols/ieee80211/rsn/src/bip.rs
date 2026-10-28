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

use crate::frames::{RSN_IGTK_LEN, RSN_IPN_LEN, RsnIgtk};
use oer_ieee80211_cipher::bip::{self, BipError, BipFrame};
use zeroize::{Zeroize, ZeroizeOnDrop};

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
    pub fn verify(&mut self, frame: &[u8]) -> Result<(), BipReceiveError> {
        let frame = BipFrame::parse(frame, self.key_id)?;
        let received = frame.packet_number();
        if received <= self.last_packet_number {
            return Err(BipReceiveError::Replay);
        }
        frame.verify(&self.key)?;
        self.last_packet_number = received;
        Ok(())
    }
}

/// Why a received group-addressed management frame is refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BipReceiveError {
    /// The frame failed BIP.
    Frame(BipError),
    /// The IPN does not exceed the last accepted one.
    Replay,
}

impl From<BipError> for BipReceiveError {
    fn from(error: BipError) -> Self {
        Self::Frame(error)
    }
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
        let protected = bip::protect(
            self.key_id,
            &self.key,
            self.next_packet_number,
            frame,
            length,
        )?;
        self.next_packet_number = self.next_packet_number.wrapping_add(1);
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
