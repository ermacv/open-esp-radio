//! BIP-CMAC-128: integrity of group-addressed robust management frames.
//!
//! A group-addressed Deauthentication, Disassociation or robust Action frame
//! of an association that protects its management frames ends with a
//! Management MIC element. Its MIC is AES-128-CMAC, truncated to 64 bits,
//! over the frame's additional authentication data and its body with the MIC
//! field zeroed, and its IGTK packet number must exceed every earlier one.
//!
//! SOURCE: IEEE 802.11-2020 12.5.4.4-.6 (BIP replay, transmission and
//! reception), 9.4.2.54 (Management MIC element, six-octet IPN).
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
use oer_ieee80211_core_security_cipher::bip::{self, BipError, BipFrame};
use oer_ieee80211_core_upper_mac::packet_number::{PacketNumber48, PacketNumberFrontier};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// A six-octet BIP IPN. This value cannot be used as a CCMP PN.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct BipPacketNumber(u64);

impl BipPacketNumber {
    pub const fn from_bytes(bytes: [u8; RSN_IPN_LEN]) -> Self {
        Self(oer_ieee80211_wire::bip::ipn_from_bytes(bytes))
    }

    pub const fn to_bytes(self) -> [u8; RSN_IPN_LEN] {
        oer_ieee80211_wire::bip::ipn_to_bytes(self.0)
    }
}

impl PacketNumber48 for BipPacketNumber {
    fn checked_add(self, distance: u64) -> Option<Self> {
        let next = self.0.checked_add(distance)?;
        (next <= oer_ieee80211_wire::bip::IPN_MAX).then_some(Self(next))
    }
}

/// The receive state of one IGTK: its key and the last accepted IPN.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct BipReceiver {
    key_id: u8,
    key: [u8; RSN_IGTK_LEN],
    /// IPNs are public; only the key material needs erasure.
    #[zeroize(skip)]
    replay: PacketNumberFrontier<BipPacketNumber>,
}

impl BipReceiver {
    /// Start with an IGTK and the IPN its KDE delivered.
    pub fn new(igtk: &RsnIgtk) -> Self {
        Self {
            key_id: igtk.key_id(),
            key: *igtk.key(),
            replay: PacketNumberFrontier::new(BipPacketNumber::from_bytes(igtk.packet_number())),
        }
    }

    pub const fn key_id(&self) -> u8 {
        self.key_id
    }

    /// Follow a group rekey: a new IGTK replaces this one, while a repeat of
    /// this IGTK keeps the higher of its replay states, so a retransmitted
    /// key never reopens packet numbers already accepted.
    pub fn rekey(&mut self, igtk: &RsnIgtk) {
        let delivered = BipPacketNumber::from_bytes(igtk.packet_number());
        if igtk.key_id() == self.key_id && *igtk.key() == self.key {
            if let Ok(candidate) = self.replay.prepare(delivered) {
                candidate.commit();
            }
        } else {
            *self = Self::new(igtk);
        }
    }

    /// Verify one received group-addressed robust management frame, from its
    /// Frame Control field to the end of its body, and advance the replay
    /// state when it verifies.
    pub fn verify(&mut self, frame: &[u8]) -> Result<(), BipReceiveError> {
        let frame = BipFrame::parse(frame, self.key_id)?;
        // BipFrame::parse decodes exactly six octets, so this is already bounded.
        let received = BipPacketNumber(frame.packet_number());
        let candidate = self
            .replay
            .prepare(received)
            .map_err(|_| BipReceiveError::Replay)?;
        frame.verify(&self.key)?;
        candidate.commit();
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
    /// The last successfully protected IPN, kept in its bounded domain.
    #[zeroize(skip)]
    packet_numbers: PacketNumberFrontier<BipPacketNumber>,
}

/// Why a group-addressed management frame could not be protected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BipTransmitError {
    /// No larger 48-bit IPN remains; replace the IGTK before sending again.
    Exhausted,
    /// The supplied length or output capacity cannot hold the frame and MMIE.
    InvalidFrame,
}

impl BipTransmitter {
    /// Start with an IGTK after the IPN its KDE announces: a receiver accepts
    /// only IPNs beyond it.
    pub fn new(igtk: &RsnIgtk) -> Self {
        Self {
            key_id: igtk.key_id(),
            key: *igtk.key(),
            packet_numbers: PacketNumberFrontier::new(BipPacketNumber::from_bytes(
                igtk.packet_number(),
            )),
        }
    }

    pub fn packet_number(&self) -> [u8; RSN_IPN_LEN] {
        self.packet_numbers.highest().to_bytes()
    }

    /// Protect once and commit its IPN only on success. Every refusal leaves
    /// both the IPN frontier and the output unchanged.
    pub fn protect(&mut self, frame: &mut [u8], length: usize) -> Result<usize, BipTransmitError> {
        let candidate = self
            .packet_numbers
            .prepare_next(core::num::NonZeroU8::MIN)
            .map_err(|_| BipTransmitError::Exhausted)?;
        let protected = bip::protect(
            self.key_id,
            &self.key,
            candidate.packet_number().0,
            frame,
            length,
        )
        .ok_or(BipTransmitError::InvalidFrame)?;
        candidate.commit();
        Ok(protected)
    }
}

#[cfg(test)]
mod tests;
