//! Software CCMP of individually addressed robust management frames.
//!
//! With management frame protection, a Deauthentication, Disassociation or
//! robust Action frame to the station arrives CCMP-protected under the
//! pairwise temporal key. The vendor opens these frames in software with its
//! own copy of the key (`ieee80211_ccmp_decrypt` through wpa_supplicant's
//! `ccmp_decrypt`), so the station keeps the temporal key for as long as its
//! association protects management frames.
//!
//! The nonce of a management frame sets the Management flag with priority
//! zero, its additional authentication data keeps the Frame Control subtype,
//! and its packet numbers form one replay counter apart from every data TID.
//!
//! SOURCE(esp32s31): IEEE 802.11-2016 12.5.3 (CCMP); complete pinned
//! `libnet80211.a[ieee80211_sta.o]::sta_recv_mgmt` and
//! `libnet80211.a[ieee80211_crypto_ccmp.o]::ieee80211_ccmp_decrypt`.
//!
//! At esp32-wifi-lib revision `af55a0ca258ce9d791d1661d7c2bbb65f08c0c21`,
//! the latter function requires Extended IV at offsets `0x58..0x60`, then
//! rejects a nonzero reserved CCMP octet (`ccmp[2]`) or any reserved Key ID
//! bits (`ccmp[3] & 0x1f`) at `0x98..0xc6`. Either violation returns `-1`
//! before replay checking or decryption. [`oer_ieee80211_wire::ccmp::CcmpHeader::parse`] enforces the
//! same header requirements for protected management reception.
//!
//! Received frames reach this module; nothing here panics on them.
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]

use oer_ieee80211_cipher::management_ccmp::{ManagementCcmpError, ManagementCcmpFrame};
use oer_ieee80211_wire::ccmp::Ccmp128Key;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// The receive state of protected management frames under one pairwise key.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct ManagementCcmpReceiver {
    temporal_key: Ccmp128Key,
    last_packet_number: u64,
}

impl ManagementCcmpReceiver {
    /// Copy from the association's zeroizing key owner directly into this
    /// zeroizing software receiver, without a by-value array argument.
    pub fn new(temporal_key: &[u8; 16]) -> Self {
        Self {
            temporal_key: Ccmp128Key::new(temporal_key),
            last_packet_number: oer_ieee80211_wire::ccmp::CcmpPacketNumber::ZERO.value(),
        }
    }

    /// Decrypt one received protected management frame in place and return
    /// its plaintext body. A frame that does not verify leaves the replay
    /// state unchanged. MIC failure erases tentative plaintext through the
    /// shared `oer_ieee80211_cipher::ccmp_mpdu::CcmpAssociation` transform; this wrapper only owns replay.
    pub fn open<'a>(&mut self, frame: &'a mut [u8]) -> Result<&'a [u8], ManagementCcmpError> {
        let frame = ManagementCcmpFrame::parse(frame)?;
        let packet_number = frame.packet_number();
        if packet_number <= self.last_packet_number {
            return Err(ManagementCcmpError::Replay);
        }

        let body = frame.open(self.temporal_key.as_bytes())?;
        self.last_packet_number = packet_number;
        Ok(body)
    }
}

#[cfg(test)]
mod tests;
