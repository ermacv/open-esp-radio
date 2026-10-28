//! Management frame protection policy and replay ownership. All inputs and
//! effects are values; key installation, CCMP integrity and TX are executed
//! by the service through the lower-MAC port.

use super::*;
use oer_ieee80211_cipher::bip::MANAGEMENT_MIC_ELEMENT_LEN;
use oer_ieee80211_wire::{
    ap::ApManagementSubtype, ccmp::CcmpPacketNumber, management_protection::SaQuery,
};

/// Absence means no installed PMF key. A present value stores the highest
/// accepted management PN plus one, so PN zero is representable. The niche
/// keeps activation and replay in one word in the bounded peer storage.
pub(super) struct ApManagementReplay(Option<core::num::NonZeroU64>);

impl ApManagementReplay {
    pub const fn inactive() -> Self {
        Self(None)
    }

    pub fn activate(&mut self) {
        if self.0.is_none() {
            self.0 = core::num::NonZeroU64::new(1);
        }
    }

    pub const fn active(&self) -> bool {
        self.0.is_some()
    }

    fn accept(&mut self, packet_number: CcmpPacketNumber) -> bool {
        let Some(highest) = self.0 else {
            return false;
        };
        let Some(next) = packet_number
            .value()
            .checked_add(1)
            .and_then(core::num::NonZeroU64::new)
        else {
            return false;
        };
        if next <= highest {
            return false;
        }
        self.0 = Some(next);
        true
    }
}

/// Protection the AP requires of one outgoing management frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApManagementProtection {
    Plaintext,
    Pairwise,
    GroupIntegrity,
}

/// Integrity evidence for an individually addressed management frame.
/// `PairwiseVerified` may be supplied only after the port verified CCMP
/// under this peer's installed key; the protocol owns the replay check.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApManagementRxProtection {
    Plaintext,
    PairwiseVerified(CcmpPacketNumber),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApManagementRejection {
    Malformed,
    Unprotected,
    UnexpectedProtection,
    Replay,
}

/// An effect of one admitted management frame, executed by the service.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApManagementAction {
    None,
    SaQueryResponse { transaction: [u8; 2] },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApManagementRx {
    Accepted(ApManagementAction),
    Rejected(ApManagementRejection),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApGroupManagementError {
    InvalidFrame,
    NoManagementKey,
    OutputTooSmall,
    PacketNumbers,
    InvalidKey(RsnFrameError),
}

impl<S: SppScope> AccessPointService<'_, S> {
    /// Decide protection without accessing a key slot or transmitting.
    /// Non-robust actions remain plaintext. A closing PMF peer keeps its
    /// pairwise protection until its association and key are removed.
    pub fn management_tx_protection(
        &self,
        peer: [u8; 6],
        subtype: ApManagementSubtype,
        body: &[u8],
    ) -> ApManagementProtection {
        if !subtype.is_robust(body) {
            return ApManagementProtection::Plaintext;
        }
        if peer[0] & 1 != 0 {
            return if self.group_management.is_some() {
                ApManagementProtection::GroupIntegrity
            } else {
                ApManagementProtection::Plaintext
            };
        }
        if self
            .peer_index(peer)
            .and_then(|index| self.storage().management_replay.get(index))
            .is_some_and(ApManagementReplay::active)
        {
            ApManagementProtection::Pairwise
        } else {
            ApManagementProtection::Plaintext
        }
    }

    /// Admit an individually addressed management body after address and
    /// integrity validation. Rejected input changes neither peer activity
    /// nor replay state. An SA Query Request of an authorized PMF peer
    /// produces a response effect carrying the same transaction identifier.
    pub fn receive_management(
        &mut self,
        peer: [u8; 6],
        subtype: ApManagementSubtype,
        body: &[u8],
        protection: ApManagementRxProtection,
    ) -> ApManagementRx {
        if body.len() < 2 {
            return ApManagementRx::Rejected(ApManagementRejection::Malformed);
        }
        let robust = subtype.is_robust(body);
        let protected_peer = self
            .peer_index(peer)
            .and_then(|index| self.storage().management_replay.get(index))
            .is_some_and(ApManagementReplay::active);
        match protection {
            ApManagementRxProtection::Plaintext if robust && protected_peer => {
                return ApManagementRx::Rejected(ApManagementRejection::Unprotected);
            }
            ApManagementRxProtection::PairwiseVerified(packet_number) => {
                if !protected_peer || !robust {
                    return ApManagementRx::Rejected(ApManagementRejection::UnexpectedProtection);
                }
                let replay = self
                    .peer_index(peer)
                    .and_then(|index| self.storage_mut().management_replay.get_mut(index));
                if !replay.is_some_and(|replay| replay.accept(packet_number)) {
                    return ApManagementRx::Rejected(ApManagementRejection::Replay);
                }
            }
            ApManagementRxProtection::Plaintext => {}
        }
        let action = if protected_peer
            && self.is_authorized(peer)
            && subtype == ApManagementSubtype::Action
            && let Some(SaQuery::Request { transaction }) = SaQuery::parse(body)
        {
            ApManagementAction::SaQueryResponse { transaction }
        } else {
            ApManagementAction::None
        };
        ApManagementRx::Accepted(action)
    }

    /// Authenticate an AP-originated group robust management frame with
    /// this BSS's IGTK. Its IPN belongs to the security epoch and is not reset
    /// by an AP stop/start or by a channel move.
    pub fn protect_group_management(
        &mut self,
        frame: &mut [u8],
        length: usize,
    ) -> Result<usize, ApGroupManagementError> {
        let packet = frame
            .get(..length)
            .ok_or(ApGroupManagementError::InvalidFrame)?;
        let subtype =
            ApManagementSubtype::parse(packet).ok_or(ApGroupManagementError::InvalidFrame)?;
        let Some((&[_, flags, _, _, destination, ..], _)) = packet.split_first_chunk::<24>() else {
            return Err(ApGroupManagementError::InvalidFrame);
        };
        let transmitter = packet
            .get(10..16)
            .ok_or(ApGroupManagementError::InvalidFrame)?;
        let bssid = packet
            .get(16..22)
            .ok_or(ApGroupManagementError::InvalidFrame)?;
        let body = packet
            .get(24..)
            .ok_or(ApGroupManagementError::InvalidFrame)?;
        if body.len() < 2
            || flags & 0x40 != 0
            || destination & 1 == 0
            || transmitter != self.address
            || bssid != self.address
            || !subtype.is_robust(body)
        {
            return Err(ApGroupManagementError::InvalidFrame);
        }
        if frame.len().saturating_sub(length) < MANAGEMENT_MIC_ELEMENT_LEN {
            return Err(ApGroupManagementError::OutputTooSmall);
        }
        let AccessPointSecurityMaterial::Wpa3Personal { igtk, .. } = &mut self.security else {
            return Err(ApGroupManagementError::NoManagementKey);
        };
        let sender = self
            .group_management
            .as_mut()
            .ok_or(ApGroupManagementError::NoManagementKey)?;
        let protected = sender
            .protect(frame, length)
            .ok_or(ApGroupManagementError::PacketNumbers)?;
        // New associations receive the current frontier in Message 3,
        // so old group frames cannot replay under the unchanged IGTK.
        let packet_number = sender.packet_number();
        *igtk = RsnIgtk::new(igtk.key_id(), packet_number, igtk.key())
            .map_err(ApGroupManagementError::InvalidKey)?;
        Ok(protected)
    }
}
